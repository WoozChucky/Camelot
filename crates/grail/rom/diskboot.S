# Grail's boot ROM, disk part: boots a machine started with no program from its
# disk 0, like a PC's BIOS loading the master boot record (spec section 7,
# "Boot from disk"). One source for both widths: Grail puts the build of the
# machine's width in its ROM (src/boot.rs), diskboot.elf in an RV32 machine's
# and diskboot-rv64.elf in an RV64 one's. On RV64 what it does is the same,
# with RV64's registers and devices:
#
#   * sysinfo's SYS_RAM_SIZE (0x10) and the disk's BLK_CAPACITY, BLK_SECTOR
#     and BLK_DMA are 64-bit registers (version 2 of the interfaces, spec
#     sections 9.5 and 9.6), read with ld and written with sd (REG_L, REG_S);
#   * the checksum adds the 256 words with addw: lw sign-extends each one, and
#     addw keeps the sum a sign-extended 32-bit value, so it is 0 exactly when
#     the words add up to 0 modulo 2^32, as the spec defines the checksum;
#   * the trap handler prints mcause, mepc and mtval in 16 hex digits;
#   * the entry at 0x1010 is a doubleword.
#
# The ROM's stub at 0x1000 jumps here (its entry word at 0x1010 points here)
# with a0 = 0. This code:
#
#   1. points mtvec at rom_trap, so that a trap here or in the boot block (a
#      boot block that is not code, say) stops the machine with a message
#      instead of looping on a fault forever;
#   2. reads the boot block, sectors 0 and 1 of disk 0 (1 KiB), into the last
#      64 KiB of RAM (RAM_BASE + SYS_RAM_SIZE - 64 KiB), with the disk's
#      registers, waiting for BLK_STATUS.DONE;
#   3. checks it: its last word must be the signature "BOOT" (0x544f4f42), and
#      its 256 words must add up to 0 (mod 2^32), so an empty disk, one that
#      was never made bootable or a damaged boot block are refused;
#   4. jumps to its first byte in M-mode with a0 = 0 (the hart id), a1 = the
#      sysinfo block (0x3000_2000) and a2 = the registers of the disk it came
#      from (disk 0, 0x1000_1000). mtvec still points at rom_trap.
#
# Any failure prints "boot ROM: <why>" on the UART and stops the machine with
# exit code 0xfd (xcal.halt). Nothing here uses RAM but the boot block's 1 KiB:
# no stack, the calls link through t6.
#
# Harts other than hart 0 never get here on their own: they wait in the park
# loop (0x1020) for an IPI and then jump to the entry word. The boot block's
# program writes its kernel's entry point there before starting it (the word
# at 0x1010 is the ROM's one writable word, with an 8-byte store on RV64), so
# harts woken later go to the kernel. One woken before that comes here, and goes
# back to the park loop.
#
# Grail includes the committed diskboot.elf and diskboot-rv64.elf (src/boot.rs).
# Rebuild them with our own tools after changing this file, `cargo xtask boot`,
# which does the same as
#
#   merlin -E -I include crates/grail/rom/diskboot.S -o diskboot.s
#   merlin-as diskboot.s -o diskboot.o
#   merlin-ld -T crates/grail/rom/link.ld diskboot.o -o crates/grail/rom/diskboot.elf
#
# and, for RV64, the same as
#
#   merlin -E -march=rv64ima_zicsr_zifencei -I include crates/grail/rom/diskboot.S -o diskboot.s
#   merlin-as -march=rv64ima_zicsr_zifencei diskboot.s -o diskboot.o
#   merlin-ld -T crates/grail/rom/link.ld diskboot.o -o crates/grail/rom/diskboot-rv64.elf
#
# (`cargo test` checks the committed files are up to date). asm.h is the
# repository's (include/asm.h), the examples' too: REG_L, REG_S, SZREG.

#include <asm.h>

#if __riscv_xlen == 64
#define ADDW addw       /* a 32-bit sum, kept sign-extended */
#else
#define ADDW add
#endif

    .equ PARK_PC,          0x1020       # the park loop
    .equ RAM_BASE,         0x80000000
    .equ BOOT_AREA,        0x10000      # the boot block's area: the last 64 KiB of RAM
    .equ BOOT_SIZE,        1024         # the boot block: sectors 0 and 1
    .equ BOOT_SECTORS,     2
    .equ BOOT_SIG,         1020         # where the signature is
    .equ BOOT_MAGIC,       0x544f4f42   # "BOOT", the boot block's last word
    .equ EXIT_BOOT_FAILED, 0xfd

    .equ UART_BASE,        0x10000000   # THR at +0; THRE is always set
    .equ SYSINFO_BASE,     0x30002000
#if __riscv_xlen == 64
    .equ SYS_RAM_SIZE,     0x10         # 64-bit
    .equ DISK0,            0x10001000   # disk 0's registers (spec section 9.6)
    .equ BLK_CAPACITY,     0x00         # 64-bit
    .equ BLK_SECTOR,       0x08         # 64-bit
    .equ BLK_DMA,          0x10         # 64-bit
    .equ BLK_COUNT,        0x18
    .equ BLK_CMD,          0x20
    .equ BLK_STATUS,       0x24
    .equ BLK_ERROR,        0x28
#else
    .equ SYS_RAM_SIZE,     0x0c
    .equ DISK0,            0x10001000   # disk 0's registers (spec section 9.6)
    .equ BLK_CAPACITY,     0x00
    .equ BLK_SECTOR,       0x08
    .equ BLK_DMA,          0x0c
    .equ BLK_COUNT,        0x10
    .equ BLK_CMD,          0x14
    .equ BLK_STATUS,       0x18
    .equ BLK_ERROR,        0x1c
#endif
    .equ CMD_READ,         1
    .equ STATUS_DONE,      2

    .text
    .globl disk_boot
disk_boot:
    csrr  t0, mhartid
    beqz  t0, 1f
    li    t0, PARK_PC                   # not hart 0: woken too early, wait again
    jr    t0
1:  la    t0, rom_trap
    csrw  mtvec, t0

    # s0 = sysinfo, s1 = where the boot block goes, s2 = disk 0's registers.
    li    s0, SYSINFO_BASE
    REG_L t0, SYS_RAM_SIZE(s0)
    li    s1, RAM_BASE - BOOT_AREA
    add   s1, s1, t0
    li    s2, DISK0

    REG_L t0, BLK_CAPACITY(s2)
    li    t1, BOOT_SECTORS
    la    a0, s_small
    bltu  t0, t1, fail

    # Read sectors 0 and 1. Grail's disks finish a command at once, but a disk
    # may take its time: wait for DONE, then acknowledge it.
    REG_S zero, BLK_SECTOR(s2)
    REG_S s1, BLK_DMA(s2)
    sw    t1, BLK_COUNT(s2)
    li    t0, CMD_READ
    sw    t0, BLK_CMD(s2)
2:  lw    t0, BLK_STATUS(s2)
    andi  t0, t0, STATUS_DONE
    beqz  t0, 2b
    li    t0, STATUS_DONE
    sw    t0, BLK_STATUS(s2)
    lw    t0, BLK_ERROR(s2)
    la    a0, s_read
    bnez  t0, fail

    lw    t0, BOOT_SIG(s1)
    li    t1, BOOT_MAGIC
    la    a0, s_magic
    bne   t0, t1, fail

    # The checksum: every word of the block, added up, is 0 (mod 2^32: addw on RV64).
    mv    t0, s1
    addi  t2, s1, BOOT_SIZE
    li    t1, 0
3:  lw    t3, 0(t0)
    ADDW  t1, t1, t3
    addi  t0, t0, 4
    bne   t0, t2, 3b
    la    a0, s_sum
    bnez  t1, fail

    li    a0, 0                         # the hart id
    mv    a1, s0                        # sysinfo
    mv    a2, s2                        # the boot disk's registers
    fence.i                             # the block is code now
    jr    s1

# Prints "boot ROM: " and the message at a0, then stops the machine.
fail:
    mv    s3, a0
    la    a0, s_prefix
    jal   t6, puts
    mv    a0, s3
    jal   t6, puts
halt:
    li    a0, EXIT_BOOT_FAILED
    xcal.halt a0
    j     halt

# A trap, here or in the boot block before it set mtvec: shows where and stops.
    .align 2
rom_trap:
    la    a0, s_trap
    jal   t6, puts
    csrr  a0, mcause
    jal   t6, put_hex
    la    a0, s_mepc
    jal   t6, puts
    csrr  a0, mepc
    jal   t6, put_hex
    la    a0, s_mtval
    jal   t6, puts
    csrr  a0, mtval
    jal   t6, put_hex
    la    a0, s_newline
    jal   t6, puts
    j     halt

# puts(a0 = NUL-terminated string), returns through t6. Clobbers a0, t0, t1.
puts:
    li    t1, UART_BASE
1:  lbu   t0, 0(a0)
    beqz  t0, 2f
    sb    t0, 0(t1)
    addi  a0, a0, 1
    j     1b
2:  jr    t6

# put_hex(a0): "0x" and XLEN/4 hex digits (8, or 16 on RV64), returns through
# t6. Clobbers t0-t3.
put_hex:
    li    t1, UART_BASE
    li    t0, '0'
    sb    t0, 0(t1)
    li    t0, 'x'
    sb    t0, 0(t1)
    li    t2, __riscv_xlen - 4          # shift of the next digit
1:  srl   t0, a0, t2
    andi  t0, t0, 15
    li    t3, 10
    blt   t0, t3, 2f
    addi  t0, t0, 'a' - 10 - '0'
2:  addi  t0, t0, '0'
    sb    t0, 0(t1)
    addi  t2, t2, -4
    bgez  t2, 1b
    jr    t6

s_prefix:  .asciz "boot ROM: "
s_small:   .asciz "disk 0 is too small to hold a boot block\n"
s_read:    .asciz "cannot read the boot block from disk 0\n"
s_magic:   .asciz "disk 0 is not bootable (no boot signature)\n"
s_sum:     .asciz "the boot block on disk 0 is corrupt (bad checksum)\n"
s_trap:    .asciz "boot ROM: unexpected trap, mcause "
s_mepc:    .asciz ", mepc "
s_mtval:   .asciz ", mtval "
s_newline: .asciz "\n"
