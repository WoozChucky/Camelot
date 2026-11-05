# Camelot kernel entry.
#
# RV32: the boot ROM (or the bootloader) jumps here, to the start of RAM
# (0x8000_0000), in M-mode with interrupts off and paging off: hart 0 at
# reset, every other hart when hart 0's SBI coldboot wakes it from the boot
# ROM's park loop with an IPI (spec section 7, "Boot"). The RV32 kernel
# carries its SBI (machine.S, the firmware's sbi.S) until #159.
#
# RV64: the firmware (spec section 7, "Firmware": Camelot's, which Grail
# loads with --bios default, or OpenSBI) starts the boot hart here, at the
# kernel's physical load address 0x8020_0000, in S-mode with paging off, a0
# = its hart id and a1 = the device tree's physical address, as OpenSBI's
# fw_jump starts Linux (Documentation/arch/riscv/boot.rst). The other harts
# wait in the firmware for a hart_start. The RV64 kernel has no M-mode code.
#
# The file has two halves:
#   * RV32's M-mode: _start, which sends hart 0 to the SBI's coldboot and
#     every other hart to wait for a hart_start (machine.S), and early_trap.
#   * S-mode: _start on RV64, supervisor_entry and secondary_entry, where the
#     SBI enters the kernel, as OpenSBI enters Linux (arch/riscv/kernel/
#     head.S's _start_kernel and secondary_start_sbi): a0 = the hart's id,
#     a1 = the device tree (hart 0) or hart_start's opaque value (the
#     others), paging off. From there on the kernel reaches M-mode only
#     through SBI calls (include/sbi.h).
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
# one, which has no identity mapping.
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
#if __riscv_xlen == 32
    la    t0, early_trap
    csrw  mtvec, t0
    csrr  t0, mhartid
    bnez  t0, 3f
    la    sp, __stack_top

    # Zero .bss (the linker script aligns both ends to a register's size).
    la    t0, __bss_start
    la    t1, __bss_end
1:  bgeu  t0, t1, 2f
    REG_S zero, 0(t0)
    addi  t0, t0, SZREG
    j     1b

    # The SBI's coldboot (machine.S): it sets M-mode up, readies the other
    # harts and enters the kernel in S-mode at supervisor_entry, with a1 =
    # 0, no device tree. sp stays the boot stack, which supervisor_entry
    # runs on.
2:  la    a0, supervisor_entry
    li    a1, 0
    j     sbi_coldboot

# Any other hart, which the coldboot has woken: .bss is hart 0's by now,
# leave it. Its own M-mode, then STOPPED until a hart_start.
3:  call  sbi_hart_init
    j     sbi_hart_wait
#else
    # The boot hart, from the firmware, as Linux's _start_kernel begins:
    # nothing enabled or pending from what ran before.
    csrw  sie, zero
    csrw  sip, zero
    la    sp, __stack_top

    # Zero .bss and the boot stack after it, so that kstack_max_used()
    # (vm.c) can tell how deep it has been used. Nothing is on it yet. a0
    # and a1 stay the firmware's.
    la    t0, __bss_start
    la    t1, __stack_top
1:  bgeu  t0, t1, 2f
    REG_S zero, 0(t0)
    addi  t0, t0, SZREG
    j     1b

    # The device tree's physical address, kept for the kernel, as Linux's
    # setup_vm() keeps dtb_early_pa: the kernel reads sysinfo until it
    # reads a device tree.
2:  la    t0, dtb_early_pa
    REG_S a1, 0(t0)
    j     supervisor_entry
#endif

    .text

# Hart 0 in S-mode: a0 = 0, a1 = the device tree (RV32: none), on the boot
# stack at its physical address. Then kmain(), and the machine stops with
# its return value as the exit code. tp = the hart's id, for good: mycpu()
# is &cpus[tp] (spinlock.h); only user programs change it, which user_vec
# undoes.
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
    # As Linux's secondary_start_sbi: nothing enabled or pending from
    # whatever ran before, a trap vector that needs nothing (Linux's
    # .Lsecondary_park; on RV64 early_s_trap, which uses no stack), then tp
    # and sp, all with paging still off. On RV32 the stack is a page at its
    # physical address, which paging maps where it is, so kernel_vec can use
    # it from the start; sstatus.SIE is 0, and nothing before it can trap.
    # On RV64 it is at its link address in the vmalloc area, usable only
    # once kernel_satp is in satp.
    csrw  sie, zero
    csrw  sip, zero
#if __riscv_xlen == 64
    la    t0, early_s_trap
    csrw  stvec, t0
#endif
    mv    tp, a0
    mv    sp, a1
#if __riscv_xlen == 64
    call  relocate_enable_mmu
    # High. The trap vector goes high first: early_s_trap at its link
    # address, which the early page table and the kernel's both map, as
    # Linux's relocate_enable_mmu points stvec at .Lsecondary_park's
    # virtual address before it switches tables. Then the kernel page table
    # (hart 0 has built it by now, kvminit()), which maps the stack and
    # drops the identity mapping, and only then kernel_vec, which uses the
    # stack (Linux's .Lsetup_trap_vector, after relocate_enable_mmu).
    la    t0, early_s_trap
    csrw  stvec, t0
    la    t0, kernel_satp
    REG_L t0, 0(t0)
    csrw  satp, t0
    sfence.vma
#endif
    la    t0, kernel_vec
    csrw  stvec, t0
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

#if __riscv_xlen == 32
# A trap in M-mode before sbi_hart_init installs machine_vec is a bug (one
# machine_vec doesn't handle goes to the SBI's sbi_trap_error, which does
# the same). Show mcause, mepc and mtval on the debug console and stop with
# exit code 0xff.
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
#endif

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
# The device tree's physical address, from the firmware's a1 (_start).
    .balign 8
    .globl dtb_early_pa
dtb_early_pa:
    .zero 8
#endif
