# Runs on every hart of the machine (spec section 7, "Harts" and "Boot"), and checks
# three ways harts share memory and signal each other:
#
#   amoadd  every hart adds 1 to a shared counter ITERATIONS times with amoadd.w;
#   lr/sc   every hart adds 1 to another counter ITERATIONS times with an lr.w/sc.w
#           loop, counting the sc.w that fail because another hart's store to the
#           counter broke its reservation (a retry);
#   ipi     an inter-processor interrupt goes round the harts IPI_ROUNDS times: each
#           hart's machine software interrupt handler passes it on to the next hart
#           by writing that hart's msip in the CLINT.
#
# Hart 0 starts at _start from the boot ROM. The others wait in the boot ROM until
# hart 0 sends each an IPI, then start at _start too, with a0 = their hart id.
# Hart 0 waits for all of them, prints the totals and halts with exit code 0, or 1 if
# a total is wrong. Try:
#
#   grail examples/smp/smp.elf
#   grail --harts 4 examples/smp/smp.elf
#
# Built with our own tools by `cargo xtask examples smp`, which does the same as
#
#   merlin-as examples/smp/smp.S -o smp.o
#   merlin-ld -T examples/smp/link.ld smp.o -o examples/smp/smp.elf
#
# Reference build with LLVM, only for comparison:
#
#   clang --target=riscv32 -march=rv32ima_zicsr -mabi=ilp32 -mno-relax -nostdlib -static \
#         -fuse-ld=lld -Wl,--no-relax,--discard-none -include examples/xcal.inc \
#         -T examples/smp/link.ld examples/smp/smp.S -o smp.elf

    .equ UART_BASE,       0x10000000
    .equ UART_LSR,        5
    .equ LSR_THRE,        0x20

    .equ CLINT_BASE,      0x02000000    # hart i's msip at +4*i
    .equ SYSINFO_BASE,    0x30002000
    .equ SYS_HARTS,       0x20

    .equ MIE_MSIE,        0x8           # mie bit 3: machine software interrupt
    .equ MSTATUS_MIE,     0x8           # mstatus bit 3: M-mode interrupts on

    .equ ITERATIONS,      1000
    .equ IPI_ROUNDS,      3
    .equ STACK_SHIFT,     12            # 4 KiB of stack per hart

    .section .text.start
    .globl _start
_start:
    # Every hart, a0 = its hart id. s0 and s1 stay as set here: the IPI handler
    # reads them.
    mv    s0, a0                    # s0 = this hart's id
    la    sp, __stack_top           # hart i's stack is the i-th 4 KiB down from the top
    slli  t0, s0, STACK_SHIFT
    sub   sp, sp, t0
    li    t0, SYSINFO_BASE
    lw    s1, SYS_HARTS(t0)         # s1 = number of harts
    bnez  s0, work

    # Hart 0: say how many harts there are, then wake the others, one IPI each.
    la    a0, s_title               # "harts: <n>"
    call  puts
    mv    a0, s1
    call  put_dec
    la    a0, s_harts
    call  puts
    li    t0, CLINT_BASE
    li    t1, 1                     # hart 1 first
    li    t2, 1
1:  bgeu  t1, s1, work
    slli  t3, t1, 2
    add   t3, t0, t3
    sw    t2, 0(t3)                 # msip[t1] = 1
    addi  t1, t1, 1
    j     1b

work:
    # amoadd: ITERATIONS atomic increments of `counter`.
    la    t0, counter
    li    t1, ITERATIONS
    li    t2, 1
1:  amoadd.w zero, t2, (t0)
    addi  t1, t1, -1
    bnez  t1, 1b

    # lr/sc: ITERATIONS increments of `lrsc_counter`, each retried until its sc.w
    # succeeds; t3 counts the retries.
    la    t0, lrsc_counter
    li    t1, ITERATIONS
    li    t3, 0
1:  lr.w  t4, (t0)
    addi  t4, t4, 1
    sc.w  t5, t4, (t0)
    beqz  t5, 2f
    addi  t3, t3, 1                 # reservation lost: try again
    j     1b
2:  addi  t1, t1, -1
    bnez  t1, 1b
    la    t0, retries
    amoadd.w zero, t3, (t0)

    # Ready for IPIs: the handler, MSIE in mie, interrupts on. Then check in.
    la    t0, ipi_handler
    csrw  mtvec, t0
    li    t0, MIE_MSIE
    csrw  mie, t0
    csrsi mstatus, MSTATUS_MIE
    la    t0, ready
    li    t1, 1
    amoadd.w zero, t1, (t0)
    beqz  s0, main_wait

    # The other harts are done: they only take IPIs from now on.
park:
    wfi
    j     park

main_wait:
    # Hart 0: wait until every hart has checked in, then send the first IPI to hart 1
    # (to itself on a one-hart machine), and wait until it has gone round.
    li    t0, IPI_ROUNDS
    mul   s2, t0, s1                # s2 = IPIs in all
    la    t0, ready
1:  lw    t1, 0(t0)
    bltu  t1, s1, 1b
    li    t1, 1
    bltu  t1, s1, 2f
    li    t1, 0
2:  li    t0, CLINT_BASE
    slli  t1, t1, 2
    add   t0, t0, t1
    li    t1, 1
    sw    t1, 0(t0)
    # From here the handler may run between any two instructions of this hart too,
    # and it clobbers t0-t2: the wait loop uses s registers.
    la    s5, ipis
3:  lw    s6, 0(s5)
    bltu  s6, s2, 3b
    csrw  mie, zero                 # no more interrupts

    # The results. s3 = harts x ITERATIONS, s4 = 0 while everything is right.
    li    t0, ITERATIONS
    mul   s3, t0, s1
    li    s4, 0

    la    a0, s_amoadd              # "amoadd: <counter> = <n> x 1000"
    call  puts
    la    t0, counter
    lw    a0, 0(t0)
    sub   t1, a0, s3
    or    s4, s4, t1
    call  put_dec
    call  harts_times

    la    a0, s_lrsc                # "lr/sc: <counter> = <n> x 1000, <r> retries"
    call  puts
    la    t0, lrsc_counter
    lw    a0, 0(t0)
    sub   t1, a0, s3
    or    s4, s4, t1
    call  put_dec
    call  harts_times
    la    a0, s_comma
    call  puts
    la    t0, retries
    lw    a0, 0(t0)
    call  put_dec
    la    a0, s_retries
    call  puts

    la    a0, s_ipi                 # "ipi: <ipis> interrupts, <rounds> on each hart"
    call  puts
    la    t0, ipis
    lw    a0, 0(t0)
    call  put_dec
    la    a0, s_interrupts
    call  puts
    li    a0, IPI_ROUNDS
    call  put_dec
    la    a0, s_each
    call  puts
    la    t0, per_hart              # every hart took IPI_ROUNDS of them
    li    t1, 0
    li    t2, IPI_ROUNDS
1:  lw    t3, 0(t0)
    sub   t3, t3, t2
    or    s4, s4, t3
    addi  t0, t0, 4
    addi  t1, t1, 1
    bltu  t1, s1, 1b

    beqz  s4, 2f
    la    a0, s_failed
    call  puts
    li    a0, 1
    xcal.halt a0
2:  xcal.halt zero

# The machine software interrupt (the only one enabled): acknowledge it, count it,
# and pass it on to the next hart until it has gone round IPI_ROUNDS times. Uses
# t0-t2 only, which the code it interrupts (a wait loop) does not.
    .balign 4
ipi_handler:
    li    t0, CLINT_BASE
    slli  t1, s0, 2
    add   t1, t0, t1
    sw    zero, 0(t1)               # msip[me] = 0: acknowledged
    la    t1, per_hart
    slli  t2, s0, 2
    add   t1, t1, t2
    lw    t2, 0(t1)                 # per_hart[me] += 1 (no other hart writes it)
    addi  t2, t2, 1
    sw    t2, 0(t1)
    la    t1, ipis
    li    t2, 1
    amoadd.w t2, t2, (t1)
    addi  t2, t2, 1                 # t2 = IPIs so far, this one included
    li    t1, IPI_ROUNDS
    mul   t1, t1, s1
    bgeu  t2, t1, 2f                # all done: stop here
    addi  t1, s0, 1                 # next hart, wrapping round to 0
    bltu  t1, s1, 1f
    li    t1, 0
1:  slli  t1, t1, 2
    add   t1, t0, t1
    li    t2, 1
    sw    t2, 0(t1)                 # msip[next] = 1
2:  mret

# " = <n> x 1000", n = s1.
harts_times:
    addi  sp, sp, -16
    sw    ra, 12(sp)
    la    a0, s_equals
    call  puts
    mv    a0, s1
    call  put_dec
    la    a0, s_times
    call  puts
    li    a0, ITERATIONS
    call  put_dec
    lw    ra, 12(sp)
    addi  sp, sp, 16
    ret

# putc(a0 = byte). Clobbers t0, t1 only.
putc:
    li    t0, UART_BASE
1:  lbu   t1, UART_LSR(t0)          # wait until the transmitter is free
    andi  t1, t1, LSR_THRE
    beqz  t1, 1b
    sb    a0, 0(t0)
    ret

# puts(a0 = NUL-terminated string)
puts:
    addi  sp, sp, -16
    sw    ra, 12(sp)
    mv    a1, a0
1:  lbu   a0, 0(a1)
    beqz  a0, 2f
    call  putc
    addi  a1, a1, 1
    j     1b
2:  lw    ra, 12(sp)
    addi  sp, sp, 16
    ret

# put_dec(a0 = unsigned value): decimal, no padding.
put_dec:
    addi  sp, sp, -32
    sw    ra, 28(sp)
    addi  a1, sp, 12                # digits go backwards into sp+2 .. sp+11
    li    a2, 10
1:  remu  a3, a0, a2
    divu  a0, a0, a2
    addi  a3, a3, '0'
    addi  a1, a1, -1
    sb    a3, 0(a1)
    bnez  a0, 1b
    addi  a2, sp, 12
2:  lbu   a0, 0(a1)
    call  putc
    addi  a1, a1, 1
    bne   a1, a2, 2b
    lw    ra, 28(sp)
    addi  sp, sp, 32
    ret

    .section .data
    .balign 4
counter:        .word 0             # amoadd's
lrsc_counter:   .word 0             # lr/sc's
retries:        .word 0             # failed sc.w, every hart's
ready:          .word 0             # harts ready for IPIs
ipis:           .word 0             # IPIs taken, every hart's
per_hart:       .word 0, 0, 0, 0, 0, 0, 0, 0    # IPIs taken by hart i (up to 8 harts)

    .section .rodata
s_title:        .asciz "harts: "
s_harts:        .asciz "\n"
s_amoadd:       .asciz "amoadd: "
s_lrsc:         .asciz "\nlr/sc: "
s_equals:       .asciz " = "
s_times:        .asciz " x "
s_comma:        .asciz ", "
s_retries:      .asciz " retries\n"
s_ipi:          .asciz "ipi: "
s_interrupts:   .asciz " interrupts, "
s_each:         .asciz " on each hart\n"
s_failed:       .asciz "FAILED\n"
