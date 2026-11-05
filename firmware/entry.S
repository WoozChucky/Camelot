# Camelot's firmware: its entry, at 0x8000_0000 (spec section 7, "Firmware").
#
# Grail loads this image with --bios default, as QEMU loads OpenSBI, and its
# boot ROM sends every hart here in M-mode, hart 0 at reset and the others
# when the coldboot wakes them from the ROM's park loop, with what QEMU's
# reset vector hands OpenSBI:
#
#   a0 = the hart's id
#   a1 = the physical address of the device tree Grail made for the machine
#   a2 = a struct fw_dynamic_info in the ROM (fw.h): the next stage's
#        address (0 for none) and its mode
#
# The firmware is OpenSBI's fw_jump in its layout, the first 2 MiB of RAM
# for itself and the next stage at 0x8020_0000 (FW_JUMP_ADDR), and reads the
# ROM's fw_dynamic_info as fw_dynamic does to know whether there is a next
# stage at all:
#
#   * a program Grail loaded (grail --bios default kernel.elf): next_addr is
#     its entry, which it starts there, in S-mode;
#   * none (QEMU's next_addr 0, a machine started with no program): it
#     boots from disk 0 (disk.S), as Grail's boot ROM does without
#     firmware, and starts the disk's boot block at FW_JUMP_ADDR in S-mode.
#
# Hart 0 is the boot hart, the one the ROM starts (fw_dynamic_info's
# boot_hart is 0). It clears .bss, finds the next stage and hands over to
# the SBI's coldboot (sbi.S), which sets up M-mode, readies the other harts
# and enters the next stage in S-mode with a0 = 0 and a1 = the device tree.
# Every other hart sets up its own M-mode and waits, STOPPED, for an HSM
# hart_start. No stacks: like the SBI calls, nothing here needs one.
#
# Grail includes the committed crates/grail/rom/firmware.elf
# (src/firmware.rs). Rebuild it with our own tools after changing a file of
# firmware/, `cargo xtask boot`, which does the same as
#
#   merlin -E -march=rv64ima_zicsr_zifencei -I include firmware/entry.S -o entry.s
#   merlin-as -march=rv64ima_zicsr_zifencei entry.s -o entry.o
#   merlin -E -march=rv64ima_zicsr_zifencei -I include firmware/sbi.S -o sbi.s
#   merlin-as -march=rv64ima_zicsr_zifencei sbi.s -o sbi.o
#   merlin -E -march=rv64ima_zicsr_zifencei -I include firmware/disk.S -o disk.s
#   merlin-as -march=rv64ima_zicsr_zifencei disk.s -o disk.o
#   merlin-ld -T firmware/firmware.ld entry.o sbi.o disk.o -o crates/grail/rom/firmware.elf
#
# (`cargo test` checks the committed file is up to date). asm.h is the
# repository's (include/asm.h): REG_L, REG_S, SZREG.

#include <asm.h>
#include "fw.h"

    .section .text.entry
    .globl _start
_start:
    la    t0, sbi_trap_error    # a trap before sbi_hart_init: report it
    csrw  mtvec, t0
    csrr  t0, mhartid
    bnez  t0, fw_other_hart

    mv    s0, a1                # s0 = the device tree
    mv    s1, a2                # s1 = the ROM's fw_dynamic_info
    la    t0, __bss_start
    la    t1, __bss_end
1:  bgeu  t0, t1, 2f
    REG_S zero, 0(t0)
    addi  t0, t0, SZREG
    j     1b

    # The next stage: the address the fw_dynamic_info gives, if there is
    # one (with its magic and a version that has the fields), else
    # FW_JUMP_ADDR, where a program for fw_jump loads.
2:  li    s2, FW_JUMP_ADDR
    beqz  s1, 3f
    REG_L t0, FW_DINFO_MAGIC(s1)
    li    t1, FW_DYNAMIC_INFO_MAGIC
    bne   t0, t1, 3f
    REG_L t0, FW_DINFO_VERSION(s1)
    li    t1, FW_DYNAMIC_INFO_VERSION_MIN
    bltu  t0, t1, 3f
    REG_L t0, FW_DINFO_NEXT_MODE(s1)
    li    t1, FW_DYNAMIC_INFO_NEXT_MODE_S
    la    a0, s_mode
    bne   t0, t1, fw_fail       # M-mode and U-mode next stages: not ours
    REG_L s2, FW_DINFO_NEXT_ADDR(s1)
3:  bnez  s2, 4f
    # None: the boot block of disk 0, at FW_JUMP_ADDR, with a2 = the disk's
    # registers.
    call  fw_disk_boot
    li    s2, FW_JUMP_ADDR

    # The SBI's coldboot: the next stage at s2, in S-mode, with a1 = the
    # device tree.
4:  mv    a0, s2
    mv    a1, s0
    j     sbi_coldboot

# Any other hart, which the coldboot woke: .bss and the HSM states are hart
# 0's work, done by now. Its own M-mode, then STOPPED until a hart_start.
fw_other_hart:
    call  sbi_hart_init
    j     sbi_hart_wait

# Prints "firmware: " and the message at a0 on the UART, then stops the
# machine with exit code 0xfd, as Grail's boot ROM does when it cannot boot
# (the disk loader's failures come here too).
    .globl fw_fail
fw_fail:
    mv    s3, a0
    la    a0, s_prefix
    call  fw_puts
    mv    a0, s3
    call  fw_puts
1:  li    a0, EXIT_BOOT_FAILED
    xcal.halt a0
    j     1b

# fw_puts(a0 = NUL-terminated string): on the UART, waiting for THRE before
# each byte. A leaf: t0-t2.
    .globl fw_puts
fw_puts:
    li    t1, UART_BASE
1:  lbu   t0, 0(a0)
    beqz  t0, 3f
2:  lbu   t2, UART_LSR(t1)
    andi  t2, t2, UART_LSR_THRE
    beqz  t2, 2b
    sb    t0, UART_THR(t1)
    addi  a0, a0, 1
    j     1b
3:  ret

    .section .rodata
s_prefix:  .asciz "firmware: "
s_mode:    .asciz "the next stage is not for S-mode (fw_dynamic_info next_mode)\n"
