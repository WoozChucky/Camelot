# Keyboard echo for Grail, interrupt-driven: every key event raises a machine external
# interrupt through PLIC source 2 (spec sections 7.1 and 9.4), and the handler echoes
# key presses to the UART. The main loop only sleeps in wfi, so nothing is echoed
# unless the interrupt path works.
#
# One event per interrupt: claim, pop KBD_DATA, complete. If more events are waiting,
# the keyboard's line is still high after the complete, so the PLIC raises it again.
# Presses of printable keys, Enter, Tab and Backspace are echoed as is; other keys
# (arrows, F1-F12) as their code in hex, e.g. [80] for Up; releases are skipped.
# Pressing Esc prints the number of interrupts taken (one per event, Esc included)
# with xcal.dbg and halts with exit code 0. Any unexpected trap halts with exit code 1.
#
# Run it with the framebuffer window (Windows) and type into the window:
#
#   cargo run --release -p grail -- --fb examples/kbd-echo/kbd-echo-irq.elf
#
# kbd-echo-irq-rv64.S is the same program for RV64 machines.
#
# Built with our own tools by `cargo xtask examples kbd-echo-irq`, which does the same as
#
#   merlin-as examples/kbd-echo/kbd-echo-irq.S -o kbd-echo-irq.o
#   merlin-ld -T examples/kbd-echo/link.ld kbd-echo-irq.o -o examples/kbd-echo/kbd-echo-irq.elf
#
# Reference build with LLVM, only for comparison:
#
#   clang --target=riscv32 -march=rv32im_zicsr -mabi=ilp32 -mno-relax -nostdlib -static \
#         -fuse-ld=lld -Wl,--no-relax,--discard-none -include examples/xcal.inc \
#         -T examples/kbd-echo/link.ld examples/kbd-echo/kbd-echo-irq.S -o kbd-echo-irq.elf

    .equ KBD_BASE,      0x30001000
    .equ KBD_DATA,      4               # pops one event
    .equ KBD_CTRL,      8               # bit 0: interrupt enable
    .equ KBD_RELEASE,   0x100           # KBD_DATA bit 8
    .equ KEY_ESC,       0x1b

    .equ UART_BASE,     0x10000000
    .equ UART_THR,      0
    .equ UART_LSR,      5
    .equ LSR_THRE,      0x20

    .equ PLIC_BASE,     0x0c000000
    .equ PLIC_ENABLE0,  0x0c002000      # context 0 = hart 0 M-mode
    .equ PLIC_THRESH0,  0x0c200000
    .equ PLIC_CLAIM0,   0x0c200004
    .equ KBD_IRQ,       2               # PLIC source of the keyboard

    .equ MIE_MEIE,      0x800           # mie bit 11
    .equ MSTATUS_MIE,   0x8
    .equ CAUSE_MEI,     0x8000000b

    .section .text.start
    .globl _start
_start:
    la    sp, __stack_top
    la    t0, trap
    csrw  mtvec, t0
    li    s0, 0                         # s0 = interrupts taken

    # PLIC: source 2 priority 1, enabled for context 0, threshold 0.
    li    t0, PLIC_BASE
    li    t1, 1
    sw    t1, 4*KBD_IRQ(t0)
    li    t0, PLIC_ENABLE0
    li    t1, 1 << KBD_IRQ
    sw    t1, 0(t0)
    li    t0, PLIC_THRESH0
    sw    zero, 0(t0)

    # Keyboard: interrupt while an event is waiting.
    li    t0, KBD_BASE
    li    t1, 1
    sw    t1, KBD_CTRL(t0)

    # Hart: take machine external interrupts.
    li    t0, MIE_MEIE
    csrs  mie, t0
    csrsi mstatus, MSTATUS_MIE

idle:
    wfi
    j     idle

    .p2align 2
trap:
    csrr  t0, mcause
    li    t1, CAUSE_MEI
    bne   t0, t1, fail
    addi  s0, s0, 1

    li    t2, PLIC_CLAIM0
    lw    t3, 0(t2)                     # claim
    li    t1, KBD_IRQ
    bne   t3, t1, fail

    li    t4, KBD_BASE
    lw    a0, KBD_DATA(t4)              # pop one event
    sw    t3, 0(t2)                     # complete

    andi  t1, a0, KBD_RELEASE
    bnez  t1, 1f
    andi  a0, a0, 0xff
    li    t1, KEY_ESC
    beq   a0, t1, done
    call  putkey                        # the main loop keeps nothing in ra or t*
1:  mret

done:
    # How many interrupts were taken.
    xcal.dbg s0
    xcal.halt zero

fail:
    # Show the unexpected mcause, then halt with exit code 1.
    xcal.dbg t0
    li    t0, 1
    xcal.halt t0

# putkey(a0 = key code): ASCII as is, 0x80 and up as [hex].
putkey:
    addi  sp, sp, -16
    sw    ra, 12(sp)
    sw    a0, 8(sp)
    li    t0, 0x80
    bgeu  a0, t0, 1f
    call  putc
    j     9f
1:  li    a0, '['
    call  putc
    lw    a0, 8(sp)
    srli  a0, a0, 4
    call  puthex
    lw    a0, 8(sp)
    andi  a0, a0, 0xf
    call  puthex
    li    a0, ']'
    call  putc
9:  lw    ra, 12(sp)
    addi  sp, sp, 16
    ret

# puthex(a0 = 0..15): one lowercase hex digit.
puthex:
    li    t0, 10
    bltu  a0, t0, 1f
    addi  a0, a0, 'a' - '0' - 10
1:  addi  a0, a0, '0'
    # falls through to putc

# putc(a0 = byte): waits for THRE and sends it. Clobbers t0, t1.
putc:
    li    t0, UART_BASE
1:  lbu   t1, UART_LSR(t0)
    andi  t1, t1, LSR_THRE
    beqz  t1, 1b
    sb    a0, UART_THR(t0)
    ret
