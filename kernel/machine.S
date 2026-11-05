# Camelot's SBI implementation on Grail: the M-mode layer, the firmware the
# S-mode kernel runs on (spec section 10, "Kernel traps"). It implements
# the part of the RISC-V Supervisor Binary Interface v2.0 the kernel uses,
# in the shape OpenSBI gives it, so that the same S-mode kernel runs on it
# and on a board's OpenSBI (include/sbi.h has the numbers):
#
#   BASE  the version, the implementation, probe_extension, the machine IDs
#   TIME  set_timer: the hart's mtimecmp, forwarded as the supervisor timer
#   IPI   send_ipi: a supervisor software interrupt to other harts
#   HSM   hart_start, hart_stop, hart_get_status, hart_suspend (retentive)
#   SRST  system_reset: shutting down, with an exit code (xcal.halt)
#   DBCN  the debug console, on the UART
#
# Anything else returns SBI_ERR_NOT_SUPPORTED. The kernel image carries this
# code (#174 moves it into a firmware image of its own), but no S-mode code
# touches the CLINT, an M-mode CSR or a scratch block: only these calls.
#
# Booting (_start, entry.S, sends each hart here):
#   * Hart 0's coldboot (sbi_coldboot) reads the number of harts and RAM
#     from sysinfo, marks every other hart STOPPED, sets up its own M-mode
#     (sbi_hart_init), wakes the other harts out of the boot ROM's park loop
#     (spec section 7, "Boot") and enters the kernel in S-mode at
#     supervisor_entry with a0 = 0, its hart id, and a1 = 0, the device tree
#     a board's firmware would pass (Grail has none: the kernel reads
#     sysinfo).
#   * Every other hart sets up its own M-mode and waits in sbi_hart_wait,
#     STOPPED, until a hart_start asks it to start, then enters S-mode at
#     the address asked for, with a0 = its id and a1 = the opaque value.
#
# After that M-mode only runs machine_vec, for three traps (every other
# exception and every S-level interrupt is delegated to S-mode):
#   * the machine timer interrupt (mcause XLEN-1 | 7): the deadline set_timer
#     set has come. Raise the supervisor timer interrupt (mip.STIP) and mask
#     the machine timer (mie.MTIE) until the next set_timer, as OpenSBI
#     does. Excalibur has no Sstc, so S-mode can't program the timer itself.
#   * the machine software interrupt (mcause XLEN-1 | 3): an IPI that
#     send_ipi sent. Acknowledge it (msip = 0) and raise the supervisor
#     software interrupt (mip.SSIP).
#   * ecall from S-mode (mcause 9): an SBI call.
# Anything else is a kernel bug and goes to early_trap (exit code 0xff).
#
# machine_vec runs with no stack. It swaps t6 with mscratch, so t6 points
# at the hart's scratch block (include/machine.h) and a0-a7 stay what the
# caller passed, and saves t0-t5 there: the SBI preserves every register
# but a0 and a1, the result. M-mode does not translate addresses, so
# everything here is at its physical address: the scratch blocks, the
# CLINT, the code itself (the RV64 kernel is linked high, but this code is
# reached at the physical addresses that the pc-relative `la` gives in
# M-mode: mtvec, mscratch).

#include <asm.h>
#include "clint.h"              /* CLINT_BASE: msip of hart h at + 4 * h */
#include "machine.h"            /* MSCRATCH_* */
#include "riscv.h"              /* CSR bits, CAUSE_*, IRQ_* */
#include "sbi.h"                /* SBI_EXT_*, SBI_ERR_*, SBI_HSM_STATE_* */
#include "spinlock.h"           /* NCPU */
#include "sysinfo.h"            /* SYSINFO_BASE, SYS_HARTS, SYS_RAM_* */
#include "trap.h"               /* PANIC_EXIT_CODE */
#include "uart.h"               /* UART_BASE, UART_LSR_* */

    .equ SCRATCH_T0, (MSCRATCH_SAVE + 0) * SZREG
    .equ SCRATCH_T1, (MSCRATCH_SAVE + 1) * SZREG
    .equ SCRATCH_T2, (MSCRATCH_SAVE + 2) * SZREG
    .equ SCRATCH_T3, (MSCRATCH_SAVE + 3) * SZREG
    .equ SCRATCH_T4, (MSCRATCH_SAVE + 4) * SZREG
    .equ SCRATCH_T5, (MSCRATCH_SAVE + 5) * SZREG
    .equ SCRATCH_STATE, MSCRATCH_HSM_STATE * SZREG
    .equ SCRATCH_START_ADDR, MSCRATCH_START_ADDR * SZREG
    .equ SCRATCH_START_ARG, MSCRATCH_START_ARG * SZREG
    .equ SCRATCH_BYTES, MSCRATCH_WORDS * SZREG

    .equ CLINT_MTIMECMP0, CLINT_MTIMECMP(0)
    .equ MSTATUS_MPP, MSTATUS_MPP_MASK
    .equ MIP_S_PENDING, MIP_SSIP | MIP_STIP

# Every exception the kernel handles itself. Left out: ecall from S-mode,
# which is how the kernel calls the SBI, and ecall from M-mode, which can't
# be delegated.
    .equ KERNEL_EXCEPTIONS, (1 << CAUSE_INSTR_MISALIGNED) | (1 << CAUSE_INSTR_ACCESS_FAULT) | (1 << CAUSE_ILLEGAL_INSTR) | (1 << CAUSE_BREAKPOINT) | (1 << CAUSE_LOAD_MISALIGNED) | (1 << CAUSE_LOAD_ACCESS_FAULT) | (1 << CAUSE_STORE_MISALIGNED) | (1 << CAUSE_STORE_ACCESS_FAULT) | (1 << CAUSE_ECALL_U) | (1 << CAUSE_INSTR_PAGE_FAULT) | (1 << CAUSE_LOAD_PAGE_FAULT) | (1 << CAUSE_STORE_PAGE_FAULT)
# The S-level interrupts: software, timer and external. M-level interrupts
# can't be delegated, so the machine timer and the machine software
# interrupt always trap to machine_vec.
    .equ KERNEL_INTERRUPTS, (1 << IRQ_S_SOFT) | (1 << IRQ_S_TIMER) | (1 << IRQ_S_EXT)

    .text

# ---- Booting ----

# Hart 0, from _start, on the boot stack with .bss cleared. Never returns.
    .globl sbi_coldboot
sbi_coldboot:
    # What the machine has, from sysinfo (spec section 9.5): the harts (1
    # to NCPU, the scratch blocks there are) and RAM. Without sysinfo: one
    # hart and no RAM, so nothing can be started (the kernel stops at once
    # anyway: sysinfo_check()).
    li    t0, SYSINFO_BASE
    li    t3, 1                 # harts
    li    t4, 0                 # RAM base
    li    t5, 0                 # RAM size
    lw    t1, SYS_MAGIC(t0)
    li    t2, SYSINFO_MAGIC
    bne   t1, t2, 2f
    LWU   t3, SYS_HARTS(t0)
    REG_L t4, SYS_RAM_BASE(t0)
    REG_L t5, SYS_RAM_SIZE(t0)
    li    t2, NCPU
    bleu  t3, t2, 1f
    mv    t3, t2
1:  bnez  t3, 2f
    li    t3, 1
2:  la    t0, sbi_hart_count
    sw    t3, 0(t0)
    la    t0, sbi_ram_base
    REG_S t4, 0(t0)
    la    t0, sbi_ram_size
    REG_S t5, 0(t0)

    # Every hart but this one is STOPPED until a hart_start (hart 0's state
    # is .bss's 0: STARTED).
    la    t0, machine_scratch
    li    t1, 1
    li    t2, SBI_HSM_STATE_STOPPED
3:  bgeu  t1, t3, 4f
    addi  t0, t0, SCRATCH_BYTES
    sw    t2, SCRATCH_STATE(t0)
    addi  t1, t1, 1
    j     3b
4:  call  sbi_hart_init

    # Wake the other harts out of the boot ROM's park loop: each comes
    # through _start to sbi_hart_wait. They must see the states above, so
    # the stores to normal memory go before the device stores (RVWMO: msip
    # is I/O, which only a fence with the I/O bits orders, #170).
    fence rw, o
    li    t0, CLINT_BASE + 4    # hart 1's msip
    li    t1, 1
    li    t2, 1
5:  bgeu  t1, t3, 6f
    sw    t2, 0(t0)
    addi  t0, t0, 4
    addi  t1, t1, 1
    j     5b

    # The kernel, in S-mode: a0 = 0 (this hart), a1 = 0 (no device tree).
6:  la    t0, supervisor_entry
    li    a1, 0
    j     sbi_enter_supervisor

# The M-mode state of the hart it runs on, the same on every hart: the trap
# vector, the scratch block, which traps S-mode takes itself, the `time`
# CSR for S-mode, and only the IPI enabled (set_timer enables the timer). A
# leaf: t0 and t1 only, no stack.
    .globl sbi_hart_init
sbi_hart_init:
    la    t0, machine_vec
    csrw  mtvec, t0
    csrr  t0, mhartid
    slli  t0, t0, MSCRATCH_SHIFT
    la    t1, machine_scratch
    add   t0, t0, t1
    csrw  mscratch, t0
    li    t0, KERNEL_EXCEPTIONS
    csrw  medeleg, t0
    li    t0, KERNEL_INTERRUPTS
    csrw  mideleg, t0
    # S-mode reads the clock through the time CSR, not the CLINT, which its
    # page tables don't map.
    li    t0, MCOUNTEREN_TM
    csrw  mcounteren, t0
    li    t0, MIE_MSIE
    csrw  mie, t0
    ret

# A STOPPED hart: from _start once sbi_hart_init has run, or from
# hart_stop. Waits with only the IPI enabled, which wakes wfi without a
# trap (mstatus.MIE is 0 in M-mode), until a hart_start leaves a start
# address in this hart's scratch block, then starts there. msip is cleared
# before each look at the block, so an IPI that comes after the look ends
# the next wfi, and one that came before it (any IPI to a stopped hart, a
# send_ipi's too) wakes it for nothing: it looks again and waits again. No
# stack, every register free: nothing runs on this hart.
    .globl sbi_hart_wait
sbi_hart_wait:
    csrr  t6, mscratch          # this hart's scratch block
    csrr  t0, mhartid
    slli  t0, t0, 2
    li    t1, CLINT_BASE
    add   t1, t1, t0            # t1 = &msip[this hart]
    li    t0, MIE_MSIE
    csrw  mie, t0
1:  sw    zero, 0(t1)
    # The store clearing msip before the load of the request: the other
    # way round, a request made in between would be missed, its IPI
    # cleared.
    fence o, r
    REG_L t0, SCRATCH_START_ADDR(t6)
    bnez  t0, 2f
    wfi
    j     1b
    # The request's address seen: its opaque value (stored before it,
    # hart_start) is to be read after it.
2:  fence r, rw
    REG_L a1, SCRATCH_START_ARG(t6)
    REG_S zero, SCRATCH_START_ADDR(t6)
    li    t2, SBI_HSM_STATE_STARTED
    fence rw, w
    sw    t2, SCRATCH_STATE(t6)
    # Nothing pending for S-mode from before it stopped.
    li    t2, MIP_S_PENDING
    csrc  mip, t2
    j     sbi_enter_supervisor

# mret to S-mode at t0, as an SBI starts a kernel: paging off (satp 0),
# sstatus.SIE 0, a0 = the hart's id, a1 as the caller set it.
sbi_enter_supervisor:
    csrw  mepc, t0
    # A whole-register update of mstatus: on RV64 its upper half holds SXL,
    # UXL and SD.
    li    t0, MSTATUS_MPP
    csrc  mstatus, t0
    li    t0, MSTATUS_MPP_S
    csrs  mstatus, t0
    csrci mstatus, MSTATUS_SIE
    csrw  satp, zero
    csrr  a0, mhartid
    mret

# ---- The trap vector ----

    .align 2
    .globl machine_vec
machine_vec:
    csrrw t6, mscratch, t6      # t6 = scratch block, mscratch = caller's t6
    REG_S t0, SCRATCH_T0(t6)
    REG_S t1, SCRATCH_T1(t6)
    REG_S t2, SCRATCH_T2(t6)
    REG_S t3, SCRATCH_T3(t6)
    REG_S t4, SCRATCH_T4(t6)
    REG_S t5, SCRATCH_T5(t6)
    csrr  t0, mcause
    bltz  t0, machine_interrupt # the interrupt bit is the top one
    li    t1, CAUSE_ECALL_S
    beq   t0, t1, sbi_call
    j     early_trap

machine_interrupt:
    slli  t0, t0, 1             # the cause without the interrupt bit
    srli  t0, t0, 1
    li    t1, IRQ_M_TIMER
    beq   t0, t1, machine_timer
    li    t1, IRQ_M_SOFT
    beq   t0, t1, machine_soft
    j     early_trap

# The deadline has come: the supervisor timer interrupt, which stays
# pending until the next set_timer, and the machine timer masked until
# then (its MTIP stays pending as long as mtime >= mtimecmp).
machine_timer:
    li    t0, MIE_MTIE
    csrc  mie, t0
    li    t0, MIP_STIP
    csrs  mip, t0
    j     machine_return

# msip[mhartid] = 0, then the IPI on to S-mode.
machine_soft:
    csrr  t0, mhartid
    slli  t0, t0, 2
    li    t1, CLINT_BASE
    add   t0, t0, t1
    sw    zero, 0(t0)
    # The clear before every access that follows, S-mode's loads of what
    # the senders asked (tlb_serve()'s of tlb_asked[]) above all. msip is
    # I/O, so nothing else orders the store before them (RVWMO): a load
    # performed first could read a request from before a second IPI, whose
    # msip = 1 the clear, landing later, would then erase, and the request
    # would wait for the next tick. OpenSBI has a wmb() (fence ow, ow) after
    # the clear (sbi_ipi_raw_clear()), for the writes after it; the loads
    # need O before R too, so O before RW, as sbi_hart_wait's fence o, r.
    fence o, rw
    csrsi mip, MIP_SSIP
    j     machine_return

# Back to where the trap came from, with t0-t6 as they were.
machine_return:
    REG_L t0, SCRATCH_T0(t6)
    REG_L t1, SCRATCH_T1(t6)
    REG_L t2, SCRATCH_T2(t6)
    REG_L t3, SCRATCH_T3(t6)
    REG_L t4, SCRATCH_T4(t6)
    REG_L t5, SCRATCH_T5(t6)
    csrrw t6, mscratch, t6
    mret

# ---- SBI calls: a7 = extension, a6 = function, a0-a5 = arguments ----
#
# Each function ends at one of the exits below, with a0 = the error and
# a1 = the value.

sbi_call:
    csrr  t0, mepc
    addi  t0, t0, 4             # resume after the ecall
    csrw  mepc, t0
    li    t0, SBI_EXT_BASE
    beq   a7, t0, sbi_base
    li    t0, SBI_EXT_TIME
    beq   a7, t0, sbi_time
    li    t0, SBI_EXT_IPI
    beq   a7, t0, sbi_ipi
    li    t0, SBI_EXT_HSM
    beq   a7, t0, sbi_hsm
    li    t0, SBI_EXT_SRST
    beq   a7, t0, sbi_srst
    li    t0, SBI_EXT_DBCN
    beq   a7, t0, sbi_dbcn
sbi_not_supported:
    li    a0, SBI_ERR_NOT_SUPPORTED
    j     sbi_error
sbi_invalid_param:
    li    a0, SBI_ERR_INVALID_PARAM
    j     sbi_error
sbi_invalid_address:
    li    a0, SBI_ERR_INVALID_ADDRESS
sbi_error:                      # a0 = the error, no value
    li    a1, 0
    j     machine_return
sbi_success:                    # no value
    li    a1, 0
sbi_value:                      # a1 = the value
    li    a0, SBI_SUCCESS
    j     machine_return

# BASE. probe_extension answers 1 for every extension here.
sbi_base:
    li    t0, SBI_BASE_GET_SPEC_VERSION
    bne   a6, t0, 1f
    li    a1, SBI_SPEC_VERSION
    j     sbi_value
1:  li    t0, SBI_BASE_GET_IMPL_ID
    bne   a6, t0, 2f
    li    a1, CAMELOT_SBI_IMPL_ID
    j     sbi_value
2:  li    t0, SBI_BASE_GET_IMPL_VERSION
    bne   a6, t0, 3f
    li    a1, CAMELOT_SBI_IMPL_VERSION
    j     sbi_value
3:  li    t0, SBI_BASE_PROBE_EXT
    bne   a6, t0, 4f
    li    a1, 1
    li    t0, SBI_EXT_BASE
    beq   a0, t0, sbi_value
    li    t0, SBI_EXT_TIME
    beq   a0, t0, sbi_value
    li    t0, SBI_EXT_IPI
    beq   a0, t0, sbi_value
    li    t0, SBI_EXT_HSM
    beq   a0, t0, sbi_value
    li    t0, SBI_EXT_SRST
    beq   a0, t0, sbi_value
    li    t0, SBI_EXT_DBCN
    beq   a0, t0, sbi_value
    li    a1, 0
    j     sbi_value
4:  li    t0, SBI_BASE_GET_MVENDORID
    bne   a6, t0, 5f
    csrr  a1, mvendorid
    j     sbi_value
5:  li    t0, SBI_BASE_GET_MARCHID
    bne   a6, t0, 6f
    csrr  a1, marchid
    j     sbi_value
6:  li    t0, SBI_BASE_GET_MIMPID
    bne   a6, t0, sbi_not_supported
    csrr  a1, mimpid
    j     sbi_value

# TIME set_timer(stime_value): this hart's mtimecmp = the value (on RV32 a0
# is its low half and a1 its high one), no supervisor timer interrupt
# pending any more, and the machine timer enabled, to raise it when the
# value comes.
sbi_time:
    bnez  a6, sbi_not_supported
    csrr  t0, mhartid
    slli  t0, t0, 3
    li    t1, CLINT_MTIMECMP0
    add   t0, t0, t1            # t0 = &mtimecmp[this hart]
#if __riscv_xlen == 64
    sd    a0, 0(t0)
#else
    # Two halves: the high word at its maximum while the low one changes,
    # so that no value in between is in the past.
    li    t1, -1
    sw    t1, 4(t0)
    sw    a0, 0(t0)
    sw    a1, 4(t0)
#endif
    li    t0, MIP_STIP
    csrc  mip, t0
    li    t0, MIE_MTIE
    csrs  mie, t0
    j     sbi_success

# IPI send_ipi(hart_mask, hart_mask_base): msip = 1 for hart
# hart_mask_base + i, for every bit i set in hart_mask; for every hart that
# can take it when hart_mask_base is -1. As OpenSBI's sbi_ipi_send_many(),
# only a hart that can take an interrupt is sent one: STARTED or SUSPENDED
# (sbi_hsm_hart_interruptible_mask()). A mask that names any other hart,
# STOPPED, on its way, or one the machine doesn't have, is
# SBI_ERR_INVALID_PARAM, and nothing is sent. The receiving hart's
# machine_soft turns it into SSIP.
sbi_ipi:
    bnez  a6, sbi_not_supported
    la    t0, sbi_hart_count
    lw    t1, 0(t0)             # t1 = harts
    # t0 = the harts that can take an interrupt, bit i for hart i. The
    # states are read as hart_get_status reads them: a hart that changes
    # its own meanwhile (hart_stop) is one an IPI may or may not reach.
    li    t0, 0
    la    t2, machine_scratch   # t2 = hart t3's scratch block
    li    t3, 0
1:  bgeu  t3, t1, 3f
    lw    t4, SCRATCH_STATE(t2)
    beqz  t4, 2f                # SBI_HSM_STATE_STARTED
    li    t5, SBI_HSM_STATE_SUSPENDED
    bne   t4, t5, 4f
2:  li    t5, 1
    sll   t5, t5, t3
    or    t0, t0, t5
4:  addi  t2, t2, SCRATCH_BYTES
    addi  t3, t3, 1
    j     1b
3:  li    t2, -1
    beq   a1, t2, 5f            # every hart: those
    beqz  a0, sbi_success       # no hart: nothing to do
    bgeu  a1, t1, sbi_invalid_param
    sub   t2, t1, a1            # the harts from the base up, 1 or more
    srl   t3, a0, t2
    bnez  t3, sbi_invalid_param # a bit past the last hart
    sll   t2, a0, a1            # t2 = the harts named, bit i for hart i
    and   t3, t2, t0
    bne   t3, t2, sbi_invalid_param # one that can't take it
    mv    t0, t2
    # The caller's stores (a TLB shootdown's request counts, the threads
    # an idle hart is woken for) must be visible to the harts before the
    # interrupt that tells them to look. msip is an I/O store, which
    # neither the ecall nor an AMO's aq/rl bits order after ordinary
    # memory (RVWMO; #170). Linux's writel() puts a fence w, o before its
    # store, and OpenSBI's sbi_ipi_raw_send() a wmb(), fence ow, ow, before
    # the device's ipi_send(). What both are for is W before O: the stores
    # to memory before the msip store. fence rw, o orders that, and the
    # caller's loads before it as well. wmb()'s other bits are for what
    # OpenSBI can't know of its drivers, whose ipi_send() may write memory
    # (W after) or follow other device writes (O before): here the send is
    # this one msip store, and no device store before it means anything to
    # the target.
5:  fence rw, o
    li    t1, CLINT_BASE
    li    t2, 1
6:  beqz  t0, sbi_success
    andi  t3, t0, 1
    beqz  t3, 7f
    sw    t2, 0(t1)
7:  srli  t0, t0, 1
    addi  t1, t1, 4
    j     6b

# HSM.
sbi_hsm:
    li    t0, SBI_HSM_HART_START
    beq   a6, t0, sbi_hart_start
    li    t0, SBI_HSM_HART_STOP
    beq   a6, t0, sbi_hart_stop
    li    t0, SBI_HSM_HART_GET_STATUS
    beq   a6, t0, sbi_hart_get_status
    li    t0, SBI_HSM_HART_SUSPEND
    beq   a6, t0, sbi_hart_suspend
    j     sbi_not_supported

# hart_start(hartid, start_addr, opaque).
sbi_hart_start:
    la    t0, sbi_hart_count
    lw    t1, 0(t0)
    bgeu  a0, t1, sbi_invalid_param
    # Somewhere in RAM a hart can execute: 4-byte aligned, as instructions
    # are.
    andi  t0, a1, 3
    bnez  t0, sbi_invalid_address
    la    t0, sbi_ram_base
    REG_L t0, 0(t0)
    sub   t0, a1, t0            # its offset into RAM (huge if below)
    la    t1, sbi_ram_size
    REG_L t1, 0(t1)
    bgeu  t0, t1, sbi_invalid_address
    slli  t2, a0, MSCRATCH_SHIFT
    la    t1, machine_scratch
    add   t2, t2, t1            # t2 = the hart's scratch block
    addi  t3, t2, SCRATCH_STATE
    # STOPPED -> START_PENDING, or the error OpenSBI gives: a started hart
    # is "already available", one on its way somewhere else can't be
    # started.
1:  lr.w.aq t0, (t3)
    li    t1, SBI_HSM_STATE_STOPPED
    bne   t0, t1, 2f
    li    t1, SBI_HSM_STATE_START_PENDING
    sc.w.rl t4, t1, (t3)
    bnez  t4, 1b
    # The request: the opaque value, then the address, which the hart
    # waits for (sbi_hart_wait), so it reads a complete request.
    REG_S a2, SCRATCH_START_ARG(t2)
    fence rw, w
    REG_S a1, SCRATCH_START_ADDR(t2)
    # And the IPI that wakes it, after every store before it, the
    # caller's too: what the new hart will read, its stack among them
    # (#170, as in send_ipi).
    fence rw, o
    slli  t0, a0, 2
    li    t1, CLINT_BASE
    add   t0, t0, t1
    li    t1, 1
    sw    t1, 0(t0)
    j     sbi_success
2:  li    t1, SBI_HSM_STATE_STARTED
    li    a0, SBI_ERR_INVALID_PARAM
    bne   t0, t1, sbi_error
    li    a0, SBI_ERR_ALREADY_AVAILABLE
    j     sbi_error

# hart_stop(): the calling hart stops, for a hart_start to start again.
# The S-mode registers are gone, so there is nothing to give back but
# mscratch: the timer off, nothing pending, then sbi_hart_wait.
sbi_hart_stop:
    csrr  t0, mhartid
    slli  t0, t0, 3
    li    t1, CLINT_MTIMECMP0
    add   t0, t0, t1
    li    t1, -1                # mtimecmp at "never"
#if __riscv_xlen == 64
    sd    t1, 0(t0)
#else
    sw    t1, 4(t0)
    sw    t1, 0(t0)
#endif
    csrw  mie, zero
    li    t0, MIP_S_PENDING
    csrc  mip, t0
    li    t0, SBI_HSM_STATE_STOPPED
    fence rw, w
    sw    t0, SCRATCH_STATE(t6)
    csrw  mscratch, t6
    j     sbi_hart_wait

# hart_get_status(hartid): its state in a1.
sbi_hart_get_status:
    la    t0, sbi_hart_count
    lw    t1, 0(t0)
    bgeu  a0, t1, sbi_invalid_param
    slli  t0, a0, MSCRATCH_SHIFT
    la    t1, machine_scratch
    add   t0, t0, t1
    lw    a1, SCRATCH_STATE(t0)
    j     sbi_value

# hart_suspend(suspend_type, resume_addr, opaque). The default retentive
# suspend: SUSPENDED until an interrupt is pending (wfi wakes for an
# enabled one, S-level ones too, whether or not sstatus.SIE lets it trap),
# then the call returns. The default non-retentive one would need the hart
# to resume at resume_addr with its state lost, which Grail has no reason
# to do: not supported. The other types are reserved or a platform's own,
# which this one has none of.
sbi_hart_suspend:
#if __riscv_xlen == 64
    slli  t0, a0, 32            # suspend_type is 32 bits
    srli  t0, t0, 32
#else
    mv    t0, a0
#endif
    beqz  t0, 1f
    li    t1, SBI_HSM_SUSPEND_NON_RET_DEFAULT
    beq   t0, t1, sbi_not_supported
    j     sbi_invalid_param
1:  li    t0, SBI_HSM_STATE_SUSPENDED
    sw    t0, SCRATCH_STATE(t6)
    wfi
    li    t0, SBI_HSM_STATE_STARTED
    sw    t0, SCRATCH_STATE(t6)
    j     sbi_success

# SRST system_reset(reset_type, reset_reason), both 32 bits. A shutdown
# stops the machine with an exit code (xcal.halt): 0 for no reason,
# PANIC_EXIT_CODE (0xfe) for a system failure, and `code` for
# 0xE000_0000 | code, a reason in the range the SBI leaves to each
# implementation. A reboot is not supported: Grail can't reset. Reserved
# types and reasons, and a vendor's, are invalid.
sbi_srst:
    bnez  a6, sbi_not_supported
#if __riscv_xlen == 64
    slli  t0, a0, 32
    srli  t0, t0, 32            # t0 = reset_type
    slli  t1, a1, 32
    srli  t1, t1, 32            # t1 = reset_reason
#else
    mv    t0, a0
    mv    t1, a1
#endif
    li    t2, SBI_SRST_TYPE_WARM_REBOOT
    bgtu  t0, t2, sbi_invalid_param
    li    t3, 0                 # t3 = the exit code
    beqz  t1, 1f
    li    t3, PANIC_EXIT_CODE
    li    t2, SBI_SRST_REASON_SYSFAIL
    beq   t1, t2, 1f
    li    t2, SBI_SRST_REASON_SBI_START
    bltu  t1, t2, sbi_invalid_param
    sub   t3, t1, t2
    li    t2, SBI_SRST_REASON_SBI_END
    bgtu  t1, t2, sbi_invalid_param
1:  bnez  t0, sbi_not_supported # a reboot
    mv    a0, t3
    .word 0x0005100b            # xcal.halt a0
2:  j     2b

# DBCN, the debug console, on the UART at its physical address (the
# kernel's driver uses the same one: kernel/uart.c).
sbi_dbcn:
    li    t0, SBI_DBCN_CONSOLE_WRITE
    beq   a6, t0, sbi_console_write
    li    t0, SBI_DBCN_CONSOLE_READ
    beq   a6, t0, sbi_console_read
    li    t0, SBI_DBCN_CONSOLE_WRITE_BYTE
    beq   a6, t0, sbi_console_write_byte
    j     sbi_not_supported

# console_write_byte(byte): blocking, until the UART takes it.
sbi_console_write_byte:
    li    t0, UART_BASE
1:  lbu   t1, UART_LSR(t0)
    andi  t1, t1, UART_LSR_THRE
    beqz  t1, 1b
    sb    a0, UART_THR(t0)
    j     sbi_success

# console_write(num_bytes, base_addr_lo, base_addr_hi): the bytes, from
# physical memory, while the UART takes them (non-blocking: it stops when
# it doesn't). The value is how many were written.
sbi_console_write:
    jal   t5, sbi_check_buffer
    li    t0, UART_BASE
    mv    t1, a1                # t1 = the next byte
    add   t2, a1, a0            # t2 = the end
1:  beq   t1, t2, 2f
    lbu   t3, UART_LSR(t0)
    andi  t3, t3, UART_LSR_THRE
    beqz  t3, 2f
    lbu   t3, 0(t1)
    sb    t3, UART_THR(t0)
    addi  t1, t1, 1
    j     1b
2:  sub   a1, t1, a1
    j     sbi_value

# console_read(num_bytes, base_addr_lo, base_addr_hi): the bytes the UART
# has, up to num_bytes, into physical memory (non-blocking). The value is
# how many were read.
sbi_console_read:
    jal   t5, sbi_check_buffer
    li    t0, UART_BASE
    mv    t1, a1
    add   t2, a1, a0
1:  beq   t1, t2, 2f
    lbu   t3, UART_LSR(t0)
    andi  t3, t3, UART_LSR_DR
    beqz  t3, 2f
    lbu   t3, UART_RBR(t0)
    sb    t3, 0(t1)
    addi  t1, t1, 1
    j     1b
2:  sub   a1, t1, a1
    j     sbi_value

# Returns to t5 if the a0 bytes at physical address a2:a1 are all in RAM;
# otherwise ends the call with SBI_ERR_INVALID_PARAM. Clobbers t0 and t1.
sbi_check_buffer:
    bnez  a2, sbi_invalid_param # past XLEN bits: no RAM there
    la    t0, sbi_ram_base
    REG_L t0, 0(t0)
    sub   t0, a1, t0            # the buffer's offset into RAM
    la    t1, sbi_ram_size
    REG_L t1, 0(t1)
    bgtu  t0, t1, sbi_invalid_param
    sub   t1, t1, t0            # the bytes from there to the end of RAM
    bgtu  a0, t1, sbi_invalid_param
    jr    t5

# ---- Data ----

    .bss
    .balign 8
# The scratch blocks, one per hart (include/machine.h).
    .globl machine_scratch
machine_scratch:
    .zero NCPU * SCRATCH_BYTES
# What the coldboot read from sysinfo.
    .globl sbi_ram_base
sbi_ram_base:
    .zero SZREG
    .globl sbi_ram_size
sbi_ram_size:
    .zero SZREG
    .globl sbi_hart_count
sbi_hart_count:
    .zero 4
