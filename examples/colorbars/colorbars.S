# Framebuffer test pattern for Grail: eight vertical color bars inside an 8-pixel
# grey border, presented once, then halt.
#
# Built with our own tools by `cargo xtask examples colorbars`, which does the same as
#
#   merlin -E -I include examples/colorbars/colorbars.S -o colorbars.s
#   merlin-as colorbars.s -o colorbars.o
#   merlin-ld -T examples/colorbars/link.ld colorbars.o -o examples/colorbars/colorbars.elf
#
# Reference build with LLVM, only for comparison:
#
#   clang --target=riscv64 -march=rv64ima_zicsr_zifencei -mabi=lp64 -mcmodel=medany -mno-relax \
#         -nostdlib -static -fuse-ld=lld -Wl,--no-relax,--discard-none -include examples/xcal.inc \
#         -T examples/colorbars/link.ld examples/colorbars/colorbars.S -o colorbars.elf
#
# Run it in a window (Windows):  grail --bios none --fb examples/colorbars/colorbars.elf
# or headless, dumping frames:   grail --bios none --fb-dump out examples/colorbars/colorbars.elf

    .equ FB_BASE,      0x30000000   # framebuffer registers (spec section 9.3)
    .equ FB_WIDTH,     0x00
    .equ FB_HEIGHT,    0x04
    .equ FB_STRIDE,    0x08
    .equ FB_CTRL,      0x10
    .equ FB_PRESENT,   0x14
    .equ VRAM_BASE,    0x31000000
    .equ BORDER,       8
    .equ BORDER_COLOR, 0x00808080

    .section .text.start
    .globl _start
_start:
    li    s0, FB_BASE
    lw    s1, FB_WIDTH(s0)          # 640
    lw    s2, FB_HEIGHT(s0)         # 480
    lw    s3, FB_STRIDE(s0)         # bytes per row
    srli  s4, s1, 3                 # bar width = width / 8
    li    s9, BORDER
    sub   s5, s1, s9                # first column of the right border
    sub   s6, s2, s9                # first row of the bottom border
    la    s7, bars
    li    s8, BORDER_COLOR

    li    a0, VRAM_BASE             # start of the current row
    li    t0, 0                     # y
row:
    li    t1, 0                     # x
    mv    a1, a0                    # current pixel
col:
    bltu  t1, s9, border
    bgeu  t1, s5, border
    bltu  t0, s9, border
    bgeu  t0, s6, border
    divu  t2, t1, s4                # bar index
    slli  t2, t2, 2
    add   t2, s7, t2
    lw    t2, 0(t2)
    j     store
border:
    mv    t2, s8
store:
    sw    t2, 0(a1)
    addi  a1, a1, 4
    addi  t1, t1, 1
    bltu  t1, s1, col
    add   a0, a0, s3
    addi  t0, t0, 1
    bltu  t0, s2, row

    li    t0, 1
    sw    t0, FB_CTRL(s0)           # display on
    sw    t0, FB_PRESENT(s0)        # show this frame now

    # Stop the machine with exit code 0.
    xcal.halt zero

    .section .rodata
    .balign 4
# 0x00RRGGBB: white, yellow, cyan, green, magenta, red, blue, black.
bars:
    .word 0x00ffffff, 0x00ffff00, 0x0000ffff, 0x0000ff00
    .word 0x00ff00ff, 0x00ff0000, 0x000000ff, 0x00000000
