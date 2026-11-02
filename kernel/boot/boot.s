# Camelot's bootloader: the boot block of a bootable disk (docs/camelot-fs.md,
# "Booting"). It loads the kernel, an ELF executable that `camelot-fs mkfs
# --boot --kernel` stored on the disk right after the filesystem, and starts it
# as Grail's boot ROM would have started it from an ELF file.
#
# Grail's boot ROM reads this block (sectors 0 and 1 of disk 0) into the last
# 64 KiB of RAM, checks its signature and checksum, and jumps to its first byte
# in M-mode with (spec section 7, "Boot from disk"):
#
#   a0 = 0, the hart id
#   a1 = the sysinfo block (0x3000_2000)
#   a2 = the registers of the disk it was read from
#   mtvec = the ROM's trap handler, which reports any trap and stops
#
# The 64 KiB from here on are ours. The block is linked at address 0 but runs
# at whatever address the RAM size puts it: every address used is pc-relative
# (`la` is `auipc` + `addi`) or a fixed device address. mkfs fills in the last
# 16 bytes of the block:
#
#   1008  kernel_sector   first sector of the kernel ELF on this disk
#   1012  kernel_sectors  its length in sectors
#   1016  checksum        makes the 256 words of the block add up to 0
#   1020  "BOOT"          the signature the ROM looks for
#
# The steps:
#
#   1. read the kernel's first 1 KiB, which must hold its ELF header and its
#      program headers, and check it is an Excalibur executable (ELF32,
#      little-endian, RISC-V, ET_EXEC);
#   2. for each PT_LOAD segment: check it lies in RAM below this block and in
#      the kernel's sectors, read its whole sectors straight to p_paddr with the
#      disk's DMA, its last partial sector through a buffer (so nothing past
#      p_filesz is written), and zero the rest up to p_memsz (.bss). Its file
#      offset must be a multiple of 512, as merlin-ld and ld.lld make it (they
#      align segments to 4 KiB);
#   3. write the kernel's entry point to the ROM's entry word (0x1010), so that
#      harts the kernel wakes later with an IPI jump to it from the park loop,
#      as after an ELF boot;
#   4. jump to the entry point with a0 = 0 and a1 = the sysinfo block.
#
# A failure prints "boot: <why>" on the UART and stops the machine with exit
# code 0xfc. No stack: the calls are leaf calls, and the steps keep their state
# in s0-s11.
#
# Built with our own tools by `cargo xtask boot`, which does the same as
#
#   merlin-as kernel/boot/boot.s -o boot.o
#   merlin-ld -T kernel/boot/link.ld boot.o -o kernel/boot/boot.elf
#
# (`cargo test` checks the committed boot.elf is up to date).

    .equ RAM_BASE,         0x80000000
    .equ SYSINFO_BASE,     0x30002000
    .equ UART_BASE,        0x10000000
    .equ BOOT_ENTRY,       0x1010       # the boot ROM's entry word
    .equ EXIT_BOOT_FAILED, 0xfc

    # This block, as mkfs fills it in.
    .equ KERNEL_SECTOR,    1008
    .equ KERNEL_SECTORS,   1012
    # The rest of the boot area: the kernel's headers, then a buffer of one
    # sector right after them.
    .equ HEADERS,          0x400
    .equ HEADERS_SIZE,     0x400
    .equ SECTOR_SIZE,      512

    # The disk's registers (spec section 9.6).
    .equ BLK_SECTOR,       0x08
    .equ BLK_DMA,          0x0c
    .equ BLK_COUNT,        0x10
    .equ BLK_CMD,          0x14
    .equ BLK_STATUS,       0x18
    .equ BLK_ERROR,        0x1c
    .equ CMD_READ,         1
    .equ STATUS_DONE,      2

    # ELF32: the header's fields and a program header's.
    .equ ELF_MAGIC,        0x464c457f   # "\x7fELF"
    .equ EI_CLASS,         4            # with EI_DATA: 1, 1 (32-bit, little-endian)
    .equ ELF_CLASS_DATA,   0x0101
    .equ E_TYPE,           16           # with e_machine: ET_EXEC (2), EM_RISCV (243)
    .equ ELF_EXEC_RISCV,   0x00f30002
    .equ E_ENTRY,          24
    .equ E_PHOFF,          28
    .equ E_PHENTSIZE,      42
    .equ E_PHNUM,          44
    .equ PHDR_SIZE,        32
    .equ P_TYPE,           0
    .equ P_OFFSET,         4
    .equ P_PADDR,          12
    .equ P_FILESZ,         16
    .equ P_MEMSZ,          20
    .equ PT_LOAD,          1

    .text
    .globl _start
_start:
    auipc s0, 0                         # s0 = this block
    mv    s1, a2                        # s1 = the disk's registers
    lw    s2, KERNEL_SECTOR(s0)         # s2 = the kernel's first sector
    lw    s3, KERNEL_SECTORS(s0)
    la    a0, s_none
    beqz  s3, fail
    slli  s3, s3, 9                     # s3 = the kernel's length in bytes

    la    a0, s_loading                 # "boot: kernel at sector 0x..."
    jal   puts
    mv    a0, s2
    jal   put_hex
    la    a0, s_newline
    jal   puts

    # 1. The headers.
    mv    a3, s2
    li    a4, HEADERS_SIZE / SECTOR_SIZE
    addi  s4, s0, HEADERS               # s4 = the kernel's first 1 KiB
    mv    a5, s4
    jal   read
    la    a0, s_elf
    lw    t0, 0(s4)
    li    t1, ELF_MAGIC
    bne   t0, t1, fail
    lhu   t0, EI_CLASS(s4)
    li    t1, ELF_CLASS_DATA
    bne   t0, t1, fail
    lw    t0, E_TYPE(s4)
    li    t1, ELF_EXEC_RISCV
    bne   t0, t1, fail
    lhu   t0, E_PHENTSIZE(s4)
    li    t1, PHDR_SIZE
    bne   t0, t1, fail
    lw    s5, E_PHOFF(s4)
    lhu   s6, E_PHNUM(s4)               # s6 = program headers left
    la    a0, s_phdrs
    li    t1, HEADERS_SIZE
    bgtu  s5, t1, fail
    slli  t0, s6, 5
    add   t0, t0, s5
    bgtu  t0, t1, fail
    add   s5, s5, s4                    # s5 = the next program header

    # 2. The segments.
segment:
    beqz  s6, start_kernel
    lw    t0, P_TYPE(s5)
    li    t1, PT_LOAD
    bne   t0, t1, next_segment
    lw    s7, P_OFFSET(s5)
    lw    s8, P_PADDR(s5)
    lw    s9, P_FILESZ(s5)
    lw    s10, P_MEMSZ(s5)
    la    a0, s_align
    andi  t0, s7, SECTOR_SIZE - 1
    bnez  t0, fail
    la    a0, s_short                   # offset + filesz within the kernel
    add   t0, s7, s9
    bltu  t0, s7, fail
    bgtu  t0, s3, fail
    la    a0, s_place                   # RAM_BASE <= paddr <= paddr + memsz <= s0
    bltu  s10, s9, fail
    li    t1, RAM_BASE
    bltu  s8, t1, fail
    add   t0, s8, s10
    bltu  t0, s8, fail
    bgtu  t0, s0, fail

    # Its whole sectors, straight to p_paddr.
    srli  a3, s7, 9
    add   a3, a3, s2                    # the segment's first sector
    srli  a4, s9, 9
    mv    a5, s8
    beqz  a4, 1f
    jal   read
    # The last part of a sector, through the buffer.
1:  andi  s11, s9, SECTOR_SIZE - 1
    beqz  s11, 3f
    srli  t0, s9, 9
    srli  a3, s7, 9
    add   a3, a3, s2
    add   a3, a3, t0
    li    a4, 1
    addi  a5, s4, HEADERS_SIZE          # the sector buffer, after the headers
    jal   read
    addi  t0, s4, HEADERS_SIZE
    andi  t1, s9, -SECTOR_SIZE
    add   t1, t1, s8
2:  lbu   t2, 0(t0)
    sb    t2, 0(t1)
    addi  t0, t0, 1
    addi  t1, t1, 1
    addi  s11, s11, -1
    bnez  s11, 2b
    # Zeros from p_filesz to p_memsz.
3:  add   t0, s8, s9
    add   t1, s8, s10
4:  bgeu  t0, t1, next_segment
    sb    zero, 0(t0)
    addi  t0, t0, 1
    j     4b
next_segment:
    addi  s5, s5, PHDR_SIZE
    addi  s6, s6, -1
    j     segment

    # 3 and 4.
start_kernel:
    lw    t0, E_ENTRY(s4)
    li    t1, BOOT_ENTRY
    sw    t0, 0(t1)
    li    a0, 0
    li    a1, SYSINFO_BASE
    fence.i                             # the segments are code now
    jr    t0

# read(a3 = first sector, a4 = sector count, a5 = RAM address): a read command,
# waiting for DONE. Returns through ra; on an error prints it and stops.
read:
    sw    a3, BLK_SECTOR(s1)
    sw    a5, BLK_DMA(s1)
    sw    a4, BLK_COUNT(s1)
    li    t0, CMD_READ
    sw    t0, BLK_CMD(s1)
1:  lw    t0, BLK_STATUS(s1)
    andi  t0, t0, STATUS_DONE
    beqz  t0, 1b
    li    t0, STATUS_DONE
    sw    t0, BLK_STATUS(s1)
    lw    t0, BLK_ERROR(s1)
    bnez  t0, 2f
    ret
2:  mv    s11, t0                       # "boot: disk read error 0x..."
    la    a0, s_prefix
    jal   puts
    la    a0, s_read
    jal   puts
    mv    a0, s11
    jal   put_hex
    la    a0, s_newline
    jal   puts
    j     halt

# Prints "boot: " and the message at a0, then stops the machine.
fail:
    mv    s11, a0
    la    a0, s_prefix
    jal   puts
    mv    a0, s11
    jal   puts
halt:
    li    a0, EXIT_BOOT_FAILED
    xcal.halt a0
    j     halt

# puts(a0 = NUL-terminated string). Clobbers a0, t0, t1.
puts:
    li    t1, UART_BASE
1:  lbu   t0, 0(a0)
    beqz  t0, 2f
    sb    t0, 0(t1)
    addi  a0, a0, 1
    j     1b
2:  ret

# put_hex(a0): "0x" and eight hex digits. Clobbers t0-t3.
put_hex:
    li    t1, UART_BASE
    li    t0, '0'
    sb    t0, 0(t1)
    li    t0, 'x'
    sb    t0, 0(t1)
    li    t2, 28                        # shift of the next digit
1:  srl   t0, a0, t2
    andi  t0, t0, 15
    li    t3, 10
    blt   t0, t3, 2f
    addi  t0, t0, 'a' - 10 - '0'
2:  addi  t0, t0, '0'
    sb    t0, 0(t1)
    addi  t2, t2, -4
    bgez  t2, 1b
    ret

s_loading: .asciz "boot: kernel at sector "
s_prefix:  .asciz "boot: "
s_none:    .asciz "no kernel on this disk\n"
s_read:    .asciz "disk read error "
s_elf:     .asciz "the kernel is not an Excalibur ELF executable\n"
s_phdrs:   .asciz "the kernel's program headers are past its first 1 KiB\n"
s_align:   .asciz "a kernel segment is not at a sector boundary\n"
s_short:   .asciz "a kernel segment runs past the end of the kernel\n"
s_place:   .asciz "a kernel segment is not in free RAM\n"
s_newline: .asciz "\n"
