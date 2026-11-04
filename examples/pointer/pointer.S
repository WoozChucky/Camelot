# Pointer demo for Grail: an arrow cursor on the framebuffer that follows the mouse
# (the Xcal pointer, spec section 9.7) and changes colour while buttons are held:
# white with none, red with the left one, green with the right one, blue with the
# middle one, and a mix when several are held (the `colours` table below).
#
# Interrupt-driven: the main loop only sleeps in wfi. The pointer interrupts on PLIC
# source 11 and the keyboard on source 2 (spec sections 7.1, 9.4). For the pointer,
# the handler erases the cursor, reads every waiting event (absolute mode, the
# default: each one is a position in framebuffer pixels and the buttons held), draws
# the cursor where the last one says and presents the frame. If events were lost
# (PTR_STATUS.OVERFLOW) it catches up from PTR_POS and PTR_BUTTONS. A press of Esc
# prints the number of pointer events read with xcal.dbg and halts with exit code 0;
# other keys are ignored. Any unexpected trap halts with exit code 1.
#
# Run it with the framebuffer window (Windows) and move the mouse over the window:
#
#   cargo run --release -p grail -- --fb examples/pointer/pointer.elf
#
# (pointer-rv64.elf on an RV64 machine).
#
# Built with our own tools by `cargo xtask examples pointer`, which does the same as
#
#   merlin -E -I include examples/pointer/pointer.S -o pointer.s
#   merlin-as pointer.s -o pointer.o
#   merlin-ld -T examples/pointer/link.ld pointer.o -o examples/pointer/pointer.elf
#
# and, for RV64 (the same code: mcause's interrupt bit, CAUSE_MEI, is bit 63, a
# position word is read zero-extended, with lwu, and its x taken from bits 15:0 with
# shifts of 48, XLEN - 16: shifts of 16 would keep bits 47:0), the same as
#
#   merlin -E -march=rv64ima_zicsr_zifencei -I include examples/pointer/pointer.S -o pointer.s
#   merlin-as -march=rv64ima_zicsr_zifencei pointer.s -o pointer.o
#   merlin-ld -T examples/pointer/link.ld pointer.o -o examples/pointer/pointer-rv64.elf
#
# Reference build with LLVM, only for comparison:
#
#   clang --target=riscv32 -march=rv32im_zicsr -mabi=ilp32 -mno-relax -nostdlib -static \
#         -fuse-ld=lld -Wl,--no-relax,--discard-none -include examples/xcal.inc \
#         -T examples/pointer/link.ld examples/pointer/pointer.S -o pointer.elf
#
# and for RV64:
#
#   clang --target=riscv64 -march=rv64ima_zicsr_zifencei -mabi=lp64 -mcmodel=medany -mno-relax \
#         -nostdlib -static -fuse-ld=lld -Wl,--no-relax,--discard-none -include examples/xcal.inc \
#         -T examples/pointer/link.ld examples/pointer/pointer.S -o pointer-rv64.elf

#if __riscv_xlen == 64
#define LWU lwu         /* a 32-bit word, zero-extended */
#else
#define LWU lw
#endif

    .equ FB_BASE,       0x30000000      # framebuffer registers (spec section 9.3)
    .equ FB_WIDTH,      0x00
    .equ FB_HEIGHT,     0x04
    .equ FB_STRIDE,     0x08
    .equ FB_CTRL,       0x10
    .equ FB_PRESENT,    0x14
    .equ VRAM_BASE,     0x31000000
    .equ BACKGROUND,    0x00203048

    .equ PTR_BASE,      0x30003000      # pointer registers (spec section 9.7)
    .equ PTR_STATUS,    0x00            # bit 0: event waiting, bit 1: overflow
    .equ PTR_EVENT,     0x04            # pops one event: buttons in bits 2:0
    .equ PTR_EVENT_XY,  0x08            # its position: x in bits 15:0, y in 31:16
    .equ PTR_CTRL,      0x0c            # bit 0: interrupt enable, bit 1: relative
    .equ PTR_POS,       0x10            # where the pointer is now
    .equ PTR_BUTTONS,   0x14            # the buttons held now
    .equ PTR_OVERFLOW,  2
    .equ PTR_IRQ,       11              # PLIC source of the pointer

    .equ KBD_BASE,      0x30001000
    .equ KBD_DATA,      4               # pops one event
    .equ KBD_CTRL,      8               # bit 0: interrupt enable
    .equ KBD_IRQ,       2               # PLIC source of the keyboard
    .equ KEY_MASK,      0x1ff           # code and release bit, without Shift/Ctrl
    .equ KEY_ESC,       0x1b

    .equ PLIC_BASE,     0x0c000000
    .equ PLIC_ENABLE0,  0x0c002000      # context 0 = hart 0 M-mode
    .equ PLIC_THRESH0,  0x0c200000
    .equ PLIC_CLAIM0,   0x0c200004

    .equ MIE_MEIE,      0x800           # mie bit 11
    .equ MSTATUS_MIE,   0x8
#if __riscv_xlen == 64
    .equ CAUSE_MEI,     0x800000000000000b # interrupt bit 63, cause 11
#else
    .equ CAUSE_MEI,     0x8000000b
#endif

    .equ CURSOR_ROWS,   12              # rows in `arrow`, 8 pixels each

# Kept across interrupts (the main loop touches no register):
#   s0 = FB_BASE        s1 = cursor x       s2 = cursor y       s3 = buttons
#   s4 = pointer events read
#   s6 = width          s7 = height         s8 = stride (bytes per row)

    .section .text.start
    .globl _start
_start:
    la    sp, __stack_top
    la    t0, trap
    csrw  mtvec, t0
    li    s0, FB_BASE
    lw    s6, FB_WIDTH(s0)              # 640
    lw    s7, FB_HEIGHT(s0)             # 480
    lw    s8, FB_STRIDE(s0)             # 2560
    li    s4, 0

    # Fill the screen with the background colour.
    li    a0, VRAM_BASE
    mul   t0, s7, s8
    add   t0, a0, t0                    # end of VRAM
    li    t1, BACKGROUND
1:  sw    t1, 0(a0)
    addi  a0, a0, 4
    bltu  a0, t0, 1b

    # The cursor where the pointer is now, then the display on.
    li    t0, PTR_BASE
    LWU   t1, PTR_POS(t0)
    call  set_position
    lw    s3, PTR_BUTTONS(t0)
    call  draw
    li    t0, 1
    sw    t0, FB_CTRL(s0)
    sw    t0, FB_PRESENT(s0)

    # PLIC: keyboard at priority 1, pointer at 2 (so motion that is already waiting
    # is drawn before a key is handled), both enabled for context 0, threshold 0.
    li    t0, PLIC_BASE
    li    t1, 1
    sw    t1, 4*KBD_IRQ(t0)
    li    t1, 2
    sw    t1, 4*PTR_IRQ(t0)
    li    t0, PLIC_ENABLE0
    li    t1, (1 << KBD_IRQ) | (1 << PTR_IRQ)
    sw    t1, 0(t0)
    li    t0, PLIC_THRESH0
    sw    zero, 0(t0)

    # Both devices interrupt while an event is waiting. The pointer stays in absolute
    # mode (PTR_CTRL bit 1 clear).
    li    t1, 1
    li    t0, PTR_BASE
    sw    t1, PTR_CTRL(t0)
    li    t0, KBD_BASE
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
    li    s9, PLIC_CLAIM0
    lw    s10, 0(s9)                    # claim
    li    t1, PTR_IRQ
    beq   s10, t1, pointer
    li    t1, KBD_IRQ
    bne   s10, t1, fail

    # Keyboard: one event per interrupt. A press of Esc ends the program.
    li    t0, KBD_BASE
    lw    a0, KBD_DATA(t0)
    andi  a0, a0, KEY_MASK
    li    t1, KEY_ESC
    beq   a0, t1, done
    j     complete

pointer:
    li    a0, BACKGROUND                # erase the cursor
    call  draw_in
    li    t0, PTR_BASE
1:  lw    t1, PTR_STATUS(t0)
    andi  t2, t1, 1
    beqz  t2, 2f
    lw    t2, PTR_EVENT(t0)             # pops one event...
    andi  s3, t2, 7
    LWU   t1, PTR_EVENT_XY(t0)          # ...and this is its position
    call  set_position
    addi  s4, s4, 1
    j     1b
2:  andi  t1, t1, PTR_OVERFLOW          # t1 = the last status read
    beqz  t1, 3f
    # Events were dropped: the current state says where the pointer ended up.
    LWU   t1, PTR_POS(t0)
    call  set_position
    lw    s3, PTR_BUTTONS(t0)
    li    t1, PTR_OVERFLOW
    sw    t1, PTR_STATUS(t0)            # write 1 to clear
3:  call  draw
    sw    zero, FB_PRESENT(s0)          # show the frame now

complete:
    sw    s10, 0(s9)                    # complete
    mret

done:
    # How many pointer events were read.
    xcal.dbg s4
    xcal.halt zero

fail:
    # Show the unexpected mcause, then halt with exit code 1.
    xcal.dbg t0
    li    t0, 1
    xcal.halt t0

# set_position(t1 = position word, zero-extended): s1 = x (bits 15:0), s2 = y
# (bits 31:16).
set_position:
    slli  s1, t1, __riscv_xlen - 16
    srli  s1, s1, __riscv_xlen - 16
    srli  s2, t1, 16
    ret

# draw(): the cursor at (s1, s2) in the colour of the buttons s3.
draw:
    la    a0, colours
    slli  t1, s3, 2
    add   a0, a0, t1
    lw    a0, 0(a0)
    # falls through to draw_in

# draw_in(a0 = colour): the arrow with its tip at (s1, s2), clipped at the right and
# bottom edges. Clobbers t1 and a1-a6.
draw_in:
    la    a1, arrow                     # current row's bits
    li    a2, 0                         # row
    mul   a3, s2, s8
    li    t1, VRAM_BASE
    add   a3, a3, t1
    slli  t1, s1, 2
    add   a3, a3, t1                    # a3 = pixel (x, y + row)
1:  add   t1, s2, a2
    bgeu  t1, s7, 9f                    # below the screen: done
    lbu   a4, 0(a1)                     # bit 7 is the leftmost column
    li    a5, 0                         # column
    mv    a6, a3
2:  add   t1, s1, a5
    bgeu  t1, s6, 4f                    # past the right edge: next row
    li    t1, 0x80
    srl   t1, t1, a5
    and   t1, t1, a4
    beqz  t1, 3f
    sw    a0, 0(a6)
3:  addi  a6, a6, 4
    addi  a5, a5, 1
    li    t1, 8
    bltu  a5, t1, 2b
4:  addi  a1, a1, 1
    add   a3, a3, s8
    addi  a2, a2, 1
    li    t1, CURSOR_ROWS
    bltu  a2, t1, 1b
9:  ret

    .section .rodata
    .balign 4
# Cursor colour (0x00RRGGBB) by the buttons held (bit 0 left, 1 right, 2 middle).
colours:
    .word 0x00ffffff                    # none: white
    .word 0x00ff0000                    # left: red
    .word 0x0000ff00                    # right: green
    .word 0x00ffff00                    # left + right: yellow
    .word 0x000000ff                    # middle: blue
    .word 0x00ff00ff                    # left + middle: magenta
    .word 0x0000ffff                    # right + middle: cyan
    .word 0x00ff8000                    # all three: orange

# The arrow, one byte per row, bit 7 = leftmost pixel; the tip is the top-left pixel.
arrow:
    .byte 0x80                          # X.......
    .byte 0xc0                          # XX......
    .byte 0xe0                          # XXX.....
    .byte 0xf0                          # XXXX....
    .byte 0xf8                          # XXXXX...
    .byte 0xfc                          # XXXXXX..
    .byte 0xfe                          # XXXXXXX.
    .byte 0xff                          # XXXXXXXX
    .byte 0xf8                          # XXXXX...
    .byte 0xd8                          # XX.XX...
    .byte 0x8c                          # X...XX..
    .byte 0x0c                          # ....XX..
