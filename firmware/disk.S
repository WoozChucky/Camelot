# Camelot's firmware: booting from disk 0 when Grail loaded no program
# (spec section 7, "Firmware", and "Boot from disk"). Without firmware the
# boot ROM's disk loader (crates/grail/rom/diskboot.S) does this in M-mode;
# with it, the firmware does it, and the boot block becomes its next stage,
# which runs in S-mode, as a kernel does:
#
#   1. read the boot block, sectors 0 and 1 of disk 0 (1 KiB), to
#      FW_JUMP_ADDR (0x8020_0000), the next stage's place, with the disk's
#      registers, waiting for BLK_STATUS.DONE and acknowledging it;
#   2. check it as the ROM does: the disk has at least 2 sectors, the read
#      worked, the block's last word is the signature "BOOT" (0x544f4f42)
#      and its 256 words add up to 0 (mod 2^32: addw, as the boot ROM);
#   3. return with a2 = the disk's registers, which the coldboot leaves for
#      the boot block (sbi.S), with a0 = 0 and a1 = the device tree.
#
# A failure prints "firmware: <why>" on the UART and stops the machine with
# exit code 0xfd (fw_fail, entry.S), the ROM's messages with the firmware's
# prefix. Version 2 of the disk's registers.

#include <asm.h>
#include "fw.h"

    .equ BOOT_SIZE,        1024         # the boot block: sectors 0 and 1
    .equ BOOT_SECTORS,     2
    .equ BOOT_SIG,         1020         # where the signature is
    .equ BOOT_MAGIC,       0x544f4f42   # "BOOT", the boot block's last word

    .text
# fw_disk_boot(): loads and checks the boot block; returns with a2 = the
# disk's registers, or does not return. A leaf: a0, a2, t0-t4.
    .globl fw_disk_boot
fw_disk_boot:
    li    a2, DISK0
    REG_L t0, BLK_CAPACITY(a2)
    li    t1, BOOT_SECTORS
    la    a0, s_small
    bltu  t0, t1, fw_fail

    # Read sectors 0 and 1. Grail's disks finish a command at once, but a
    # disk may take its time: wait for DONE, then acknowledge it.
    li    t2, FW_JUMP_ADDR
    REG_S zero, BLK_SECTOR(a2)
    REG_S t2, BLK_DMA(a2)
    sw    t1, BLK_COUNT(a2)
    li    t0, BLK_CMD_READ
    sw    t0, BLK_CMD(a2)
1:  lw    t0, BLK_STATUS(a2)
    andi  t0, t0, BLK_STATUS_DONE
    beqz  t0, 1b
    li    t0, BLK_STATUS_DONE
    sw    t0, BLK_STATUS(a2)
    lw    t0, BLK_ERROR(a2)
    la    a0, s_read
    bnez  t0, fw_fail

    lw    t0, BOOT_SIG(t2)
    li    t1, BOOT_MAGIC
    la    a0, s_magic
    bne   t0, t1, fw_fail

    # The checksum: every word of the block, added up, is 0 (mod 2^32).
    mv    t0, t2
    addi  t3, t2, BOOT_SIZE
    li    t1, 0
2:  lw    t4, 0(t0)
    addw  t1, t1, t4
    addi  t0, t0, 4
    bne   t0, t3, 2b
    la    a0, s_sum
    bnez  t1, fw_fail

    fence.i                             # the block is code now
    ret

    .section .rodata
s_small:   .asciz "disk 0 is too small to hold a boot block\n"
s_read:    .asciz "cannot read the boot block from disk 0\n"
s_magic:   .asciz "disk 0 is not bootable (no boot signature)\n"
s_sum:     .asciz "the boot block on disk 0 is corrupt (bad checksum)\n"
