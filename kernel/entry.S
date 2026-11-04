# Camelot kernel entry. The boot ROM (or the bootloader) jumps here, to the
# start of RAM (0x8000_0000), in M-mode with interrupts off and paging off:
# hart 0 at reset, every other hart when hart 0 wakes it with an IPI (spec
# section 7, "Boot"; smp.c). Set up a trap vector and a stack, clear .bss
# (hart 0 only), let start() configure the hart, drop to S-mode and run
# kmain() there (hart_main() on the other harts), then halt with its return
# value as the exit code.
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
# Every hart then turns paging on with it, jumps to the high address of the
# next instruction, and from there on runs high; kvminit() and kvminithart()
# replace the early table with the final one, which has no identity mapping.
# M-mode keeps running at physical addresses (machine.S): it doesn't
# translate.
#
# The Xcal instructions are written as .word because stock assemblers don't
# know them:
#   xcal.dbg  a0 = 0x0005000b   (rs1 = a0 = x10, funct3 = 000, custom-0)
#   xcal.halt a0 = 0x0005100b   (rs1 = a0 = x10, funct3 = 001, custom-0)

#include <asm.h>
#include "spinlock.h"           /* NCPU */
#include "trap.h"               /* EXIT_UNEXPECTED_TRAP */
#include "vm.h"                 /* EARLY_TABLES, OVERFLOW_STACK_SIZE */

    .section .text.start
    .globl _start
_start:
    la    t0, early_trap
    csrw  mtvec, t0
    # tp = the hart's id, for good: mycpu() is &cpus[tp] (spinlock.h). mret
    # keeps it, and only user programs change it, which user_vec undoes.
    csrr  tp, mhartid
    bnez  tp, other_hart
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

    # start() (start.c) sets up delegation, the trap vectors and the timer,
    # and leaves mepc = supervisor_entry and mstatus.MPP = S.
2:  call  start
    mret

# Any other hart: hart 0 has booted the kernel, put the physical address of
# the top of a stack page for this hart in hart_boot_sp[tp] (smp.c) and sent
# the IPI that brought the hart here from the boot ROM's park loop. .bss is
# hart 0's by now: leave it.
other_hart:
    la    t0, hart_boot_sp
    slli  t1, tp, RISCV_LGPTR
    add   t0, t0, t1
    REG_L sp, 0(t0)
    call  start
    mret

    .text

# S-mode from here on. xcal.halt is M-only, so the exit code goes to M-mode
# as a machine call. The other harts never return from hart_main(): they run
# the scheduler until hart 0 stops the machine.
    .globl supervisor_entry
supervisor_entry:
#if __riscv_xlen == 64
    # Paging off, at the physical address. A trap here is a bug: stop.
    la    t0, early_s_trap
    csrw  stvec, t0
    bnez  tp, 1f
    call  setup_vm              # hart 0: the early page table (vm.c)
1:  la    t0, early_satp
    REG_L t0, 0(t0)
    sfence.vma
    csrw  satp, t0
    sfence.vma
    # Paging is on, and the early table maps this code where it runs, too.
    # Jump to its link address.
    la    t0, va_kernel_pa_offset
    REG_L t0, 0(t0)
    la    t1, 2f
    add   t1, t1, t0
    jr    t1
    # High from here: pc-relative addresses are link addresses now. The
    # stack: hart 0's boot stack in the image, at its link address; another
    # hart's own kernel stack, which hart 0 gave it in hart_sp[tp]. That one
    # is in the vmalloc area, which only the kernel page table maps, and
    # hart 0 has built that table by now (kvminit()): another hart switches
    # to it first, which also drops the identity mapping.
2:  bnez  tp, 3f
    la    sp, __stack_top
    j     4f
3:  la    t0, kernel_satp
    REG_L t0, 0(t0)
    csrw  satp, t0
    sfence.vma
    la    t0, hart_sp
    slli  t1, tp, RISCV_LGPTR
    add   t0, t0, t1
    REG_L sp, 0(t0)
    # The S-mode trap vector (trap.h), at its link address. (On RV32 start()
    # has set it: there the two addresses are one.)
4:  la    t0, kernel_vec
    csrw  stvec, t0
#endif
    bnez  tp, 2f
    call  kmain
    call  machine_halt
1:  j     1b
2:  call  hart_main
    j     1b

# void xcal_dbg(u32 val)
    .globl xcal_dbg
xcal_dbg:
    .word 0x0005000b            # xcal.dbg a0
    ret

# void xcal_halt(u32 code), M-mode only
    .globl xcal_halt
xcal_halt:
    .word 0x0005100b            # xcal.halt a0
1:  j     1b

# A trap in M-mode is a bug: before start() installs machine_vec, or one that
# machine_vec doesn't handle. Show mcause, mepc and mtval on the debug console
# and stop with exit code 0xff.
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
# console and stop with exit code 0xff, through M-mode.
    .align 2
early_s_trap:
    csrr  a0, scause
    .word 0x0005000b            # xcal.dbg a0
    csrr  a0, sepc
    .word 0x0005000b            # xcal.dbg a0
    csrr  a0, stval
    .word 0x0005000b            # xcal.dbg a0
    li    a0, EXIT_UNEXPECTED_TRAP
    call  machine_halt

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
