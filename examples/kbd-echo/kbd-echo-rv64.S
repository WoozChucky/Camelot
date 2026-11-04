# The RV64 build of kbd-echo (kbd-echo.S is the RV32 one). On RV64, putkey saves ra and its
# argument with sd: sw would keep only the low 32 bits of ra, and lw sign-extend
# them, which breaks a return address at 0x8000_0000 and up.
#
# Keyboard echo for Grail, polling: waits on KBD_STATUS, pops events from KBD_DATA
# (spec section 9.4) and echoes every key press to the UART. Releases are skipped.
#
#   printable keys, Enter, Tab   the character itself
#   Backspace                    "\b \b" (erases the last character on a terminal)
#   other keys (arrows, F1-F12)  their code in hex, e.g. [80] for Up, [90] for F1
#   Ctrl held                    a '^' before the key, e.g. ^c for Ctrl+C
#   Esc                          prints the number of presses seen (Esc included)
#                                with xcal.dbg, then halts with exit code 0
#
# Key events come from the framebuffer window, so run it with --fb (Windows) and type
# into the window; the echo appears in the terminal. Without a window the keyboard
# stays empty and the program waits forever.
#
#   cargo run --release -p grail -- --fb examples/kbd-echo/kbd-echo-rv64.elf
#
# Built with our own tools by `cargo xtask examples kbd-echo`, which does the same as
#
#   merlin-as -march=rv64ima_zicsr_zifencei examples/kbd-echo/kbd-echo-rv64.S -o kbd-echo-rv64.o
#   merlin-ld -T examples/kbd-echo/link.ld kbd-echo-rv64.o -o examples/kbd-echo/kbd-echo-rv64.elf
#
# Reference build with LLVM, only for comparison:
#
#   clang --target=riscv64 -march=rv64ima_zicsr_zifencei -mabi=lp64 -mcmodel=medany -mno-relax \
#         -nostdlib -static -fuse-ld=lld -Wl,--no-relax,--discard-none -include examples/xcal.inc \
#         -T examples/kbd-echo/link.ld examples/kbd-echo/kbd-echo-rv64.S -o kbd-echo-rv64.elf

    .equ KBD_BASE,      0x30001000
    .equ KBD_STATUS,    0               # bit 0: event available
    .equ KBD_DATA,      4               # pops one event
    .equ KBD_RELEASE,   0x100           # KBD_DATA bit 8
    .equ KBD_CTRL_HELD, 0x400           # KBD_DATA bit 10
    .equ KEY_BACKSPACE, 0x08
    .equ KEY_ESC,       0x1b

    .equ UART_BASE,     0x10000000
    .equ UART_THR,      0
    .equ UART_LSR,      5
    .equ LSR_THRE,      0x20

    .section .text.start
    .globl _start
_start:
    la    sp, __stack_top
    li    s0, KBD_BASE
    li    s1, 0                         # s1 = presses seen

poll:
    lw    t0, KBD_STATUS(s0)
    andi  t0, t0, 1
    beqz  t0, poll
    lw    s2, KBD_DATA(s0)              # s2 = event
    andi  t0, s2, KBD_RELEASE
    bnez  t0, poll
    addi  s1, s1, 1
    andi  s3, s2, 0xff                  # s3 = key code
    li    t0, KEY_ESC
    beq   s3, t0, done
    andi  t0, s2, KBD_CTRL_HELD
    beqz  t0, 1f
    li    a0, '^'
    call  putc
1:  mv    a0, s3
    call  putkey
    j     poll

done:
    # Show the number of presses, then halt with exit code 0.
    xcal.dbg s1
    xcal.halt zero

# putkey(a0 = key code): echoes one key as described at the top.
putkey:
    addi  sp, sp, -16
    sd    ra, 8(sp)
    sd    a0, 0(sp)
    li    t0, KEY_BACKSPACE
    bne   a0, t0, 1f
    call  putc                          # "\b \b"
    li    a0, ' '
    call  putc
    li    a0, KEY_BACKSPACE
    call  putc
    j     9f
1:  li    t0, 0x80
    bgeu  a0, t0, 2f
    call  putc                          # ASCII: as is
    j     9f
2:  li    a0, '['                       # 0x80 and up: [hex]
    call  putc
    ld    a0, 0(sp)
    srli  a0, a0, 4
    call  puthex
    ld    a0, 0(sp)
    andi  a0, a0, 0xf
    call  puthex
    li    a0, ']'
    call  putc
9:  ld    ra, 8(sp)
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
