# Camelot's bootloader: the boot block of a bootable disk (docs/camelot-fs.md,
# "Booting"). It loads the kernel, an ELF executable that `camelot-fs mkfs
# --boot --kernel` stored on the disk right after the filesystem, and starts it
# as Grail's boot ROM would have started it from an ELF file.
#
# The firmware reads this block (sectors 0 and 1 of disk 0), checks its
# signature and checksum (spec section 7, "Firmware"), and starts it as its
# next stage: it runs at 0x8020_0000 in S-mode with paging off, which reaches
# the disk and the UART at their physical addresses, with
#
#   a0 = 0, the hart id
#   a1 = the device tree's physical address
#   a2 = the registers of the disk it was read from
#
# 0x8020_0000 is where the kernel loads, so the block first moves itself
# out of the way, as a PC's boot sector moves itself out of 0x7c00: to the 64
# KiB below the device tree, which Grail puts near the end of RAM, at the last
# 2 MiB boundary that leaves it room (QEMU's place), and goes on there. The
# kernel has the RAM from 0x8020_0000 up to there.
#
# The 64 KiB from here on are ours. The block is linked at address 0 but runs
# at whatever address the RAM size puts it: every address used is relative to
# s0, the block's address, which the first instruction takes from the pc
# (`auipc s0, 0`), or a fixed device address. Since the block is linked at 0,
# a label's address is its offset in the block, below 1 KiB, so %lo(label) is
# that offset itself and `addi a0, s0, %lo(label)` is the label's address: one
# instruction where `la` takes two, and the block has 1008 bytes for code and
# messages. mkfs fills in the last 16 bytes of the block:
#
#   1008  kernel_sector   first sector of the kernel ELF on this disk
#   1012  kernel_sectors  its length in sectors
#   1016  checksum        makes the 256 words of the block add up to 0
#   1020  "BOOT"          the signature the ROM looks for
#
# The steps:
#
#   1. read the kernel's first 1 KiB, which must hold its ELF header and its
#      program headers, and check it is an Excalibur executable (ELF64,
#      little-endian, RISC-V, ET_EXEC);
#   2. for each PT_LOAD segment: check it lies in free RAM (from 0x8020_0000,
#      after the firmware's 2 MiB) below this block and in the kernel's
#      sectors, read its whole sectors straight to p_paddr with the
#      disk's DMA, its last partial sector through a buffer (so nothing past
#      p_filesz is written), and zero the rest up to p_memsz (.bss). A segment
#      with file bytes must start at a multiple of 512 in the file, as merlin-ld
#      and ld.lld place segments that follow 4 KiB pages; a .bss-only one
#      (p_filesz 0) reads nothing, so its offset does not matter;
#   3. find the entry point's load address: e_entry is a link address, in one
#      of the segments, so its load address is that segment's p_paddr plus
#      its distance from p_vaddr (the kernel is linked at
#      0xffff_ffff_8000_0000 and loaded at 0x8020_0000, and starts with
#      paging off). Nothing needs it written: the kernel starts the other
#      harts through the firmware's SBI (HSM hart_start);
#   4. jump to it with a0 = 0 and a1 = the device tree, a1 as the firmware
#      gave it, in S-mode.
#
# A failure prints "boot: <why>" on the UART and stops the machine with exit
# code 0xfc, through the SBI's system_reset (this block runs in S-mode, where
# xcal.halt is illegal). No stack: the calls are leaf calls, and the steps
# keep their state in s0-s11, the disk's registers in a2.
#
# <asm.h> names the register-wide loads and stores (REG_L, REG_S), as
# Linux's assembly does. FW_JUMP_ADDR is loaded with li, which gives
# 0x8020_0000 itself (lui alone would sign-extend it to
# 0xffff_ffff_8020_0000).
#
# Built with our own tools by `cargo xtask boot`, which does the same as
#
#   merlin -E -I include kernel/boot/boot.S -o boot.s
#   merlin-as boot.s -o boot.o
#   merlin-ld -T kernel/boot/link.ld boot.o -o kernel/boot/boot.elf
#
# (`cargo test` checks it is up to date).

#include <asm.h>

    .equ UART_BASE,        0x10000000
    .equ FW_JUMP_ADDR,     0x80200000   # the firmware's next stage, here
    .equ BOOT_AREA,        0x10000      # the 64 KiB this block may use
    .equ BOOT_SIZE,        1024         # the block
    .equ EXIT_BOOT_FAILED, 0xfc
    # The SBI's system_reset (SRST): a shutdown whose reason Camelot's SBI
    # makes the exit code, 0xE000_0000 | code (kernel/include/sbi.h). The
    # reason is a uint32_t, of which the SBI takes a1's low 32 bits: written
    # sign-extended here, -0x2000_0000, li makes it in two instructions.
    .equ SBI_EXT_SRST,     0x53525354
    .equ SBI_SRST_REASON,  -0x20000000

    # This block, as mkfs fills it in.
    .equ KERNEL_SECTOR,    1008
    .equ KERNEL_SECTORS,   1012
    # The rest of the boot area: the kernel's headers, then a buffer of one
    # sector right after them.
    .equ HEADERS,          0x400
    .equ HEADERS_SIZE,     0x400
    .equ SECTOR_SIZE,      512

    # The disk's registers (spec section 9.6), with the 64-bit BLK_SECTOR
    # and BLK_DMA.
    .equ BLK_SECTOR,       0x08         # 64-bit
    .equ BLK_DMA,          0x10         # 64-bit
    .equ BLK_COUNT,        0x18
    .equ BLK_CMD,          0x20
    .equ BLK_STATUS,       0x24
    .equ BLK_ERROR,        0x28
    .equ CMD_READ,         1
    .equ STATUS_DONE,      2

    # The ELF header's fields and a program header's: ELF64's, where the
    # addresses and sizes are 8 bytes (REG_L).
    .equ ELF_MAGIC,        0x464c457f   # "\x7fELF"
    .equ EI_CLASS,         4            # with EI_DATA: class, 1 (little-endian)
    .equ E_TYPE,           16           # with e_machine: ET_EXEC (2), EM_RISCV (243)
    .equ ELF_EXEC_RISCV,   0x00f30002
    .equ E_ENTRY,          24
    .equ ELF_CLASS_DATA,   0x0102       # ELFCLASS64
    .equ E_PHOFF,          32
    .equ E_PHENTSIZE,      54
    .equ E_PHNUM,          56
    .equ PHDR_SIZE,        56
    .equ P_TYPE,           0
    .equ P_OFFSET,         8
    .equ P_VADDR,          16
    .equ P_PADDR,          24
    .equ P_FILESZ,         32
    .equ P_MEMSZ,          40
    .equ PT_LOAD,          1

    .text
    .globl _start
_start:
    auipc s0, 0                         # s0 = this block
    # Out of the kernel's way, to the 64 KiB below the device tree: copy the
    # whole block there a doubleword at a time, mkfs's words included, and go
    # on in the copy, at its `moved` (a label's offset in the block is its
    # %lo, the block being linked at 0).
    li    t1, -BOOT_AREA
    add   t1, t1, a1                    # t1 = the new place
    addi  t3, s0, BOOT_SIZE             # t3 = the end of this block
1:  ld    t4, 0(s0)
    sd    t4, 0(t1)
    addi  s0, s0, 8
    addi  t1, t1, 8
    bltu  s0, t3, 1b
    addi  s0, t1, -BOOT_SIZE            # s0 = the copy
    fence.i                             # the copy is code now
    jalr  zero, %lo(moved)(s0)
moved:
    lwu   s2, KERNEL_SECTOR(s0)         # s2 = the kernel's first sector
    lwu   s3, KERNEL_SECTORS(s0)
    addi  a0, s0, %lo(s_none)
    beqz  s3, fail
    slli  s3, s3, 9                     # s3 = the kernel's length in bytes

    addi  a0, s0, %lo(s_loading)       # "boot: kernel at sector 0x..."
    jal   puts
    mv    a0, s2
    jal   put_hex

    # 1. The headers.
    mv    a3, s2
    li    a4, HEADERS_SIZE / SECTOR_SIZE
    addi  s4, s0, HEADERS               # s4 = the kernel's first 1 KiB
    mv    a5, s4
    jal   read
    addi  a0, s0, %lo(s_elf)
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
    REG_L s5, E_PHOFF(s4)
    lhu   s6, E_PHNUM(s4)               # s6 = program headers left
    addi  a0, s0, %lo(s_phdrs)
    li    t1, HEADERS_SIZE
    bgtu  s5, t1, fail
    li    t0, PHDR_SIZE                 # phoff + phnum * PHDR_SIZE <= 1 KiB
    mul   t0, t0, s6
    add   t0, t0, s5
    bgtu  t0, t1, fail
    add   s5, s5, s4                    # s5 = the next program header

    # 2. The segments.
segment:
    beqz  s6, start_kernel
    lw    t0, P_TYPE(s5)
    li    t1, PT_LOAD
    bne   t0, t1, next_segment
    REG_L s7, P_OFFSET(s5)
    REG_L s8, P_PADDR(s5)
    REG_L s9, P_FILESZ(s5)
    REG_L s10, P_MEMSZ(s5)
    # 3. If the entry point is in this segment, its load address replaces
    # e_entry in the headers: p_paddr + (e_entry - p_vaddr). (mkfs checks
    # that a segment holds it.)
    REG_L t0, P_VADDR(s5)
    REG_L t1, E_ENTRY(s4)
    sub   t1, t1, t0                    # e_entry - p_vaddr, huge if below
    bgeu  t1, s10, 5f
    add   t1, t1, s8
    REG_S t1, E_ENTRY(s4)
5:  addi  a0, s0, %lo(s_place)         # free RAM <= paddr <= paddr + memsz <= s0
    bltu  s10, s9, fail
    li    t1, FW_JUMP_ADDR              # the firmware's RAM is below it
    bltu  s8, t1, fail
    add   t0, s8, s10
    bltu  t0, s8, fail
    bgtu  t0, s0, fail
    beqz  s9, 3f                        # nothing in the file: only zeros
    addi  a0, s0, %lo(s_align)
    andi  t0, s7, SECTOR_SIZE - 1
    bnez  t0, fail
    addi  a0, s0, %lo(s_short)         # offset + filesz within the kernel
    add   t0, s7, s9
    bltu  t0, s7, fail
    bgtu  t0, s3, fail

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

    # 4.
start_kernel:
    REG_L t0, E_ENTRY(s4)
    li    a0, 0                         # a1 is still the device tree
    fence.i                             # the segments are code now
    jr    t0

# read(a3 = first sector, a4 = sector count, a5 = RAM address): a read command,
# waiting for DONE. Returns through ra; on an error prints it and stops.
read:
    REG_S a3, BLK_SECTOR(a2)
    REG_S a5, BLK_DMA(a2)
    sw    a4, BLK_COUNT(a2)
    li    t0, CMD_READ
    sw    t0, BLK_CMD(a2)
1:  lw    t0, BLK_STATUS(a2)
    andi  t0, t0, STATUS_DONE
    beqz  t0, 1b
    sw    t0, BLK_STATUS(a2)            # t0 = STATUS_DONE: clears it
    lw    t0, BLK_ERROR(a2)
    bnez  t0, 2f
    ret
2:  mv    s11, t0                       # "boot: disk read error 0x..."
    addi  a0, s0, %lo(s_prefix)
    jal   puts
    addi  a0, s0, %lo(s_read)
    jal   puts
    mv    a0, s11
    jal   put_hex
    j     halt

# Prints "boot: " and the message at a0, then stops the machine: the SBI's
# system_reset(SHUTDOWN, reason 0xE000_0000 | EXIT_BOOT_FAILED).
fail:
    mv    s11, a0
    addi  a0, s0, %lo(s_prefix)
    jal   puts
    mv    a0, s11
    jal   puts
halt:
    li    a7, SBI_EXT_SRST
    li    a6, 0                         # system_reset
    li    a0, 0                         # SHUTDOWN
    li    a1, SBI_SRST_REASON | EXIT_BOOT_FAILED
    ecall
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

# put_hex(a0): eight hex digits and a newline (the "0x" before them is the
# message's). Clobbers t0-t3.
put_hex:
    li    t1, UART_BASE
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
    li    t0, '\n'
    sb    t0, 0(t1)
    ret

s_loading: .asciz "boot: kernel at sector 0x"
s_prefix:  .asciz "boot: "
s_none:    .asciz "no kernel on this disk\n"
s_read:    .asciz "disk read error 0x"
s_elf:     .asciz "the kernel is not an Excalibur ELF executable\n"
s_phdrs:   .asciz "the kernel's program headers are past its first 1 KiB\n"
s_align:   .asciz "a kernel segment is not at a sector boundary\n"
s_short:   .asciz "a kernel segment runs past the end of the kernel\n"
s_place:   .asciz "a kernel segment is not in free RAM\n"
