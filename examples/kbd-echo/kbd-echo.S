# Keyboard echo for Grail, polling: waits on KBD_STATUS, pops events from KBD_DATA
# (spec section 9.4) and echoes them to the UART. It starts in the keyboard's ASCII
# mode, where it echoes every key press (releases are skipped):
#
#   printable keys, Enter, Tab   the character itself
#   Backspace                    "\b \b" (erases the last character on a terminal)
#   other keys (arrows, F1-F11)  their code in hex, e.g. [80] for Up, [90] for F1
#   Ctrl held                    a '^' before the key, e.g. ^c for Ctrl+C
#   F12                          switches to raw mode (KBD_CTRL bit 1)
#
# In raw mode it prints every event, press and release, as the key's HID usage in hex
# (every usage a keyboard reports fits in two digits): "+04 " for a press of the key
# A, "-04 " for its release, so Shift+a is "+e1 +04 -04 -e1 ". Modifiers alone, Alt
# and the keys that type nothing show up too.
#
#   F12 (usage 45)               switches back to ASCII mode
#
# Switching prints a newline. The mode change empties the keyboard's FIFO, so the
# release of the F12 that switched comes in the new mode: in raw mode it shows as a
# "-45 " with no press before it.
#
# In both modes:
#
#   Esc (usage 29)               prints the number of presses seen (Esc included)
#                                with xcal.dbg, then halts with exit code 0
#
# Key events come from the framebuffer window, so run it with --fb (Windows) and type
# into the window; the echo appears in the terminal. Without a window the keyboard
# stays empty and the program waits forever.
#
#   cargo run --release -p grail -- --fb examples/kbd-echo/kbd-echo.elf
#
# (kbd-echo-rv64.elf on an RV64 machine).
#
# Built with our own tools by `cargo xtask examples kbd-echo`, which does the same as
#
#   merlin -E -I include examples/kbd-echo/kbd-echo.S -o kbd-echo.s
#   merlin-as kbd-echo.s -o kbd-echo.o
#   merlin-ld -T examples/kbd-echo/link.ld kbd-echo.o -o examples/kbd-echo/kbd-echo.elf
#
# and, for RV64 (the same code: putkey saves ra and its argument with REG_S, which is
# sd there; sw would keep only the low 32 bits of ra, and lw sign-extend them, which
# breaks a return address at 0x8000_0000 and up), the same as
#
#   merlin -E -march=rv64ima_zicsr_zifencei -I include examples/kbd-echo/kbd-echo.S -o kbd-echo.s
#   merlin-as -march=rv64ima_zicsr_zifencei kbd-echo.s -o kbd-echo.o
#   merlin-ld -T examples/kbd-echo/link.ld kbd-echo.o -o examples/kbd-echo/kbd-echo-rv64.elf
#
# Reference build with LLVM, only for comparison:
#
#   clang --target=riscv32 -march=rv32im_zicsr -mabi=ilp32 -mno-relax -nostdlib -static \
#         -fuse-ld=lld -Wl,--no-relax,--discard-none -include examples/xcal.inc -I include \
#         -T examples/kbd-echo/link.ld examples/kbd-echo/kbd-echo.S -o kbd-echo.elf
#
# and for RV64:
#
#   clang --target=riscv64 -march=rv64ima_zicsr_zifencei -mabi=lp64 -mcmodel=medany -mno-relax \
#         -nostdlib -static -fuse-ld=lld -Wl,--no-relax,--discard-none -include examples/xcal.inc \
#         -I include -T examples/kbd-echo/link.ld examples/kbd-echo/kbd-echo.S -o kbd-echo-rv64.elf

#include <asm.h>

    .equ KBD_BASE,      0x30001000
    .equ KBD_STATUS,    0               # bit 0: event available
    .equ KBD_DATA,      4               # pops one event
    .equ KBD_CTRL,      8
    .equ KBD_CTRL_RAW,  2               # KBD_CTRL bit 1: raw mode
    # ASCII mode
    .equ KBD_RELEASE,   0x100           # KBD_DATA bit 8
    .equ KBD_CTRL_HELD, 0x400           # KBD_DATA bit 10
    .equ KEY_BACKSPACE, 0x08
    .equ KEY_ESC,       0x1b
    .equ KEY_F12,       0x9b
    # Raw mode
    .equ RAW_RELEASE,   16              # KBD_DATA bit 16 (as a shift amount)
    .equ USAGE_ESCAPE,  0x29
    .equ USAGE_F12,     0x45

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
    lw    t0, KBD_CTRL(s0)
    andi  t0, t0, KBD_CTRL_RAW
    bnez  t0, raw
    andi  t0, s2, KBD_RELEASE
    bnez  t0, poll
    addi  s1, s1, 1
    andi  s3, s2, 0xff                  # s3 = key code
    li    t0, KEY_ESC
    beq   s3, t0, done
    li    t0, KEY_F12
    beq   s3, t0, switch
    andi  t0, s2, KBD_CTRL_HELD
    beqz  t0, 1f
    li    a0, '^'
    call  putc
1:  mv    a0, s3
    call  putkey
    j     poll

# A raw-mode event: s3 = usage (bits 15:0), s4 = 1 on release (bit 16).
raw:
    li    t0, 0xffff
    and   s3, s2, t0
    srli  s4, s2, RAW_RELEASE
    bnez  s4, 1f
    addi  s1, s1, 1                     # a press: count it, and maybe act on it
    li    t0, USAGE_ESCAPE
    beq   s3, t0, done
    li    t0, USAGE_F12
    beq   s3, t0, switch
1:  li    a0, '+'
    beqz  s4, 2f
    li    a0, '-'
2:  call  putc
    srli  a0, s3, 4
    call  puthex
    andi  a0, s3, 0xf
    call  puthex
    li    a0, ' '
    call  putc
    j     poll

# F12: the other mode. Changing KBD_CTRL bit 1 empties the FIFO.
switch:
    lw    t0, KBD_CTRL(s0)
    xori  t0, t0, KBD_CTRL_RAW
    sw    t0, KBD_CTRL(s0)
    li    a0, '\n'
    call  putc
    j     poll

done:
    # Show the number of presses, then halt with exit code 0.
    xcal.dbg s1
    xcal.halt zero

# putkey(a0 = key code): echoes one key as described at the top.
putkey:
    addi  sp, sp, -16
    REG_S ra, 16-SZREG(sp)
    REG_S a0, 16-2*SZREG(sp)
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
    REG_L a0, 16-2*SZREG(sp)
    srli  a0, a0, 4
    call  puthex
    REG_L a0, 16-2*SZREG(sp)
    andi  a0, a0, 0xf
    call  puthex
    li    a0, ']'
    call  putc
9:  REG_L ra, 16-SZREG(sp)
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
