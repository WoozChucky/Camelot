# Camelot kernel entry. The boot ROM (or the bootloader) jumps here, to the
# start of RAM (0x8000_0000), in M-mode with interrupts off and paging off:
# hart 0 at reset, every other hart when hart 0's SBI coldboot wakes it
# from the boot ROM's park loop with an IPI (spec section 7, "Boot").
#
# The file has two halves:
#   * M-mode: _start, which sends hart 0 to the SBI's coldboot and every
#     other hart to wait for a hart_start (machine.S), and early_trap.
#   * S-mode: supervisor_entry and secondary_entry, where the SBI enters the
#     kernel, as OpenSBI enters Linux (arch/riscv/kernel/head.S's
#     _start_kernel and secondary_start_sbi): a0 = the hart's id, a1 = the
#     device tree (hart 0) or hart_start's opaque value (the others),
#     paging off. From there on the kernel reaches M-mode only through SBI
#     calls (include/sbi.h).
#
# One source for both widths (#193): <asm.h> names the instructions that
# differ (REG_L, REG_S, SZREG), and #if __riscv_xlen marks the code that
# does.
#
# RV32: the kernel runs where it is linked, at its physical address, and
# paging, once on, maps it there (identity).
#
# RV64: a higher-half kernel, as Linux's riscv64 head.S boots one. The image
# is linked at 0xffff_ffff_8000_0000 (kernel.lds.S) but runs here at its
# physical address, so until paging is on everything reaches its own symbols
# pc-relatively (la, call: -mcmodel=medany), which works because the image
# keeps its layout at both addresses. A link address is out of auipc's reach
# from here (more than 2 GiB away), so the jump to the high half adds
# va_kernel_pa_offset, the distance between the two, as Linux's
# relocate_enable_mmu adds va_pa_offset. In S-mode, still with paging off,
# hart 0 builds the early page table (setup_vm(), vm.c): the image at both
# addresses, RAM in the direct map, sysinfo and the UART in the fixmap.
# Every hart then turns paging on with it and jumps to the high address of
# the code after the call (relocate_enable_mmu), and from there on runs
# high; kvminit() and kvminithart() replace the early table with the final
# one, which has no identity mapping. M-mode keeps running at physical
# addresses (machine.S): it doesn't translate.
#
# The Xcal instructions are written as .word because stock assemblers don't
# know them:
#   xcal.dbg  a0 = 0x0005000b   (rs1 = a0 = x10, funct3 = 000, custom-0)
#   xcal.halt a0 = 0x0005100b   (rs1 = a0 = x10, funct3 = 001, custom-0)

#include <asm.h>
#include "sbi.h"                /* SBI_EXT_SRST, SBI_SRST_* */
#include "spinlock.h"           /* NCPU */
#include "trap.h"               /* EXIT_UNEXPECTED_TRAP */
#include "vm.h"                 /* EARLY_TABLES, OVERFLOW_STACK_SIZE */

    .section .text.start
    .globl _start
_start:
    la    t0, early_trap
    csrw  mtvec, t0
    csrr  t0, mhartid
    bnez  t0, 3f
    la    sp, __stack_top

    # Zero .bss (the linker script aligns both ends to a register's size),
    # and on RV64 the boot stack after it too, so that kstack_max_used()
    # (vm.c) can tell how deep it has been used. Nothing is on it yet.
    la    t0, __bss_start
#if __riscv_xlen == 64
    la    t1, __stack_top
#else
    la    t1, __bss_end
#endif
1:  bgeu  t0, t1, 2f
    REG_S zero, 0(t0)
    addi  t0, t0, SZREG
    j     1b

    # The SBI's coldboot (machine.S): it sets M-mode up, readies the other
    # harts and enters the kernel at supervisor_entry. sp stays the boot
    # stack, which supervisor_entry runs on.
2:  j     sbi_coldboot

# Any other hart, which the coldboot has woken: .bss is hart 0's by now,
# leave it. Its own M-mode, then STOPPED until a hart_start.
3:  call  sbi_hart_init
    j     sbi_hart_wait

    .text

# Hart 0 in S-mode, from the coldboot: a0 = 0, a1 = the device tree (none
# on Grail), on the boot stack at its physical address. Then kmain(), and
# the machine stops with its return value as the exit code. tp = the hart's
# id, for good: mycpu() is &cpus[tp] (spinlock.h); only user programs
# change it, which user_vec undoes.
    .globl supervisor_entry
supervisor_entry:
    mv    tp, a0
#if __riscv_xlen == 64
    # Paging off, at the physical address. A trap here is a bug: stop.
    la    t0, early_s_trap
    csrw  stvec, t0
    call  setup_vm              # the early page table (vm.c)
    call  relocate_enable_mmu
    # High from here: pc-relative addresses are link addresses now. The
    # boot stack, at its link address.
    la    sp, __stack_top
#endif
    # The S-mode trap vector (trap.h), at its link address.
    la    t0, kernel_vec
    csrw  stvec, t0
    call  kmain
    call  machine_power_off
1:  j     1b

# Any other hart, from the SBI's hart_start (smp_boot()): a0 = its id, a1 =
# the top of its kernel stack, paging off, at the physical address, with
# satp 0 and sstatus.SIE 0 as the SBI guarantees. Never returns: the hart
# runs the scheduler until hart 0 stops the machine.
    .globl secondary_entry
secondary_entry:
    # tp and sp first, as Linux's secondary_start_sbi loads them before
    # anything else, so that nothing runs here without them: on RV32 the
    # stack is a page at its physical address, which paging maps where it
    # is, so kernel_vec can use it from the start. On RV64 it is at its link
    # address in the vmalloc area, usable once kernel_satp is in satp: until
    # then the trap vector is early_s_trap, which uses no stack (as Linux's
    # .Lsecondary_park doesn't).
    mv    tp, a0
    mv    sp, a1
    # Nothing enabled or pending from whatever ran before (Linux does the
    # same).
    csrw  sie, zero
    csrw  sip, zero
#if __riscv_xlen == 64
    la    t0, early_s_trap
    csrw  stvec, t0
    call  relocate_enable_mmu
    # High. The stack is in the vmalloc area, which only the kernel page
    # table maps, and hart 0 has built that table by now (kvminit()): switch
    # to it first, which also drops the identity mapping, so the trap
    # vector goes high before it does.
    la    t0, kernel_vec
    csrw  stvec, t0
    la    t0, kernel_satp
    REG_L t0, 0(t0)
    csrw  satp, t0
    sfence.vma
#else
    la    t0, kernel_vec
    csrw  stvec, t0
#endif
    call  hart_main
1:  j     1b

#if __riscv_xlen == 64
# Turns paging on with the early page table, which maps the image at its
# physical address too, so this code keeps running after the satp write,
# and returns to the caller's link address: ra plus the distance between
# the two (Linux's relocate_enable_mmu). A leaf: t0 only, no stack.
relocate_enable_mmu:
    la    t0, va_kernel_pa_offset
    REG_L t0, 0(t0)
    add   ra, ra, t0
    la    t0, early_satp
    REG_L t0, 0(t0)
    sfence.vma
    csrw  satp, t0
    sfence.vma
    ret
#endif

# void xcal_dbg(u32 val)
    .globl xcal_dbg
xcal_dbg:
    .word 0x0005000b            # xcal.dbg a0
    ret

# A trap in M-mode is a bug: before sbi_hart_init installs machine_vec, or
# one that machine_vec doesn't handle. Show mcause, mepc and mtval on the
# debug console and stop with exit code 0xff.
    .align 2
    .globl early_trap
early_trap:
    csrr  a0, mcause
    .word 0x0005000b            # xcal.dbg a0
    csrr  a0, mepc
    .word 0x0005000b            # xcal.dbg a0
    csrr  a0, mtval
    .word 0x0005000b            # xcal.dbg a0
    li    a0, EXIT_UNEXPECTED_TRAP
    .word 0x0005100b            # xcal.halt a0
1:  j     1b

#if __riscv_xlen == 64
# A trap in S-mode before stvec points at kernel_vec, at the high address
# (paging off, or going on): a bug. Show scause, sepc and stval on the debug
# console and stop with exit code 0xff, through the SBI. No stack: a hart
# from secondary_entry has one that neither paging off nor the early page
# table maps, so this is machine_power_off(EXIT_UNEXPECTED_TRAP) without
# its C (include/sbi.h): an SRST shutdown with Camelot's SBI's reason
# 0xE000_0000 | 0xff, then, if an SBI refuses that one, "system failure";
# then wait for good. No warning on the console: the UART may not be
# mapped yet.
    .align 2
early_s_trap:
    csrr  a0, scause
    .word 0x0005000b            # xcal.dbg a0
    csrr  a0, sepc
    .word 0x0005000b            # xcal.dbg a0
    csrr  a0, stval
    .word 0x0005000b            # xcal.dbg a0
    li    a7, SBI_EXT_SRST
    li    a6, SBI_SRST_RESET
    li    a0, SBI_SRST_TYPE_SHUTDOWN
    li    a1, SBI_SRST_REASON_SBI_START | EXIT_UNEXPECTED_TRAP
    ecall
    li    a0, SBI_SRST_TYPE_SHUTDOWN
    li    a1, SBI_SRST_REASON_SYSFAIL
    ecall
1:  wfi
    j     1b

# The early page table and the pool of its lower tables (vm.c, setup_vm()),
# page aligned, and each hart's overflow stack (kernelvec.S), 16-aligned.
    .bss
    .balign PGSIZE
    .globl early_pg_dir
early_pg_dir:
    .zero PGSIZE
    .globl early_tables
early_tables:
    .zero EARLY_TABLES * PGSIZE
    .globl overflow_stacks
overflow_stacks:
    .zero NCPU * OVERFLOW_STACK_SIZE
#endif
