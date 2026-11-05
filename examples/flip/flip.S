# Page flipping on Grail's display controller (spec section 9.3): switches the display
# to 800x600, draws eight frames of horizontal colour bands that move down the screen,
# each into the buffer that is not shown, and shows each with a page flip, which takes
# effect at the next vertical blank. So no frame is ever seen half drawn: a display
# that scanned out the buffer being drawn would show the top of one frame over the
# bottom of the last ("tearing").
#
# - The mode is found by name, not by number: the program reads FB_MODES and each
#   mode's size through FB_MODE_SEL, and takes the one that is 800x600.
# - 800x600 does not fit in the built-in VRAM (640x480), so the two buffers are in RAM,
#   page-aligned, in .bss: 2 x 1.92 MB of the default 4 MiB.
# - Frame 0 is drawn into the first buffer before anything shows it. Writing FB_SCANOUT
#   then FB_MODE is a modeset: the mode and the buffer take effect together, at once.
# - Each later frame is drawn into the back buffer, named in FB_SCANOUT, and FB_FLIP
#   asks for it to be shown from the next vertical blank. The program then sleeps in
#   wfi until FB_STATUS.FLIP_PENDING clears, woken by the vertical-blank interrupt
#   (PLIC source 16), whose handler acknowledges it in FB_IRQ and counts it. Then the
#   two buffers swap roles.
#
# After the eighth frame it prints the number of frames shown with xcal.dbg and halts
# with exit code 0. No 800x600 mode, or a modeset or flip the display refuses, halt it
# with exit code 2 or 3, and any unexpected trap with exit code 1.
#
# Run it with the framebuffer window (Windows), at a real-time clock so that the frames
# go by at the speed the program draws them:
#
#   cargo run --release -p grail -- --bios none --fb --clock 50MHz examples/flip/flip.elf
#
# or headless, dumping each frame it shows:  grail --bios none --fb-dump out examples/flip/flip.elf
#
# Built with our own tools by `cargo xtask examples flip`, which does the same as
#
#   merlin -E -I include examples/flip/flip.S -o flip.s
#   merlin-as flip.s -o flip.o
#   merlin-ld -T examples/flip/link.ld flip.o -o examples/flip/flip.elf
#
# Reference build with LLVM, only for comparison:
#
#   clang --target=riscv64 -march=rv64ima_zicsr_zifencei -mabi=lp64 -mcmodel=medany -mno-relax \
#         -nostdlib -static -fuse-ld=lld -Wl,--no-relax,--discard-none -include examples/xcal.inc \
#         -I include -T examples/flip/link.ld examples/flip/flip.S -o flip.elf

#include <asm.h>

    .equ FB_BASE,       0x30000000      # framebuffer registers (spec section 9.3)
    .equ FB_CTRL,       0x10            # bit 0: display enable
    .equ FB_MODES,      0x1c            # how many modes there are
    .equ FB_MODE,       0x20            # the current mode: writing it is a modeset
    .equ FB_MODE_SEL,   0x24            # the mode FB_MODE_* describe
    .equ FB_MODE_WIDTH, 0x28
    .equ FB_MODE_HEIGHT, 0x2c
    .equ FB_MODE_STRIDE, 0x30           # bytes per row
    .equ FB_STATUS,     0x34            # bit 0: flip pending; bits 1, 2: refused
    .equ FB_SCANOUT,    0x38            # 64-bit: the buffer to show (0 = VRAM)
    .equ FB_FLIP,       0x40            # show FB_SCANOUT from the next vertical blank
    .equ FB_IRQ,        0x44            # bit 0: enable; bit 1: pending, write 1 to clear
    .equ FLIP_PENDING,  1
    .equ IRQ_ON_CLEAR,  3               # enable, and clear the pending bit
    .equ FB_SRC,        16              # PLIC source of the vertical blank

    .equ PLIC_BASE,     0x0c000000
    .equ PLIC_ENABLE0,  0x0c002000      # context 0 = hart 0 M-mode
    .equ PLIC_THRESH0,  0x0c200000
    .equ PLIC_CLAIM0,   0x0c200004

    .equ MIE_MEIE,      0x800           # mie bit 11
    .equ MSTATUS_MIE,   0x8
    .equ CAUSE_MEI,     0x800000000000000b # interrupt bit 63, cause 11

    .equ WIDTH,         800
    .equ HEIGHT,        600
    .equ BUF_SIZE,      469 * 4096      # 800 x 600 x 4 bytes, in whole pages
    .equ FRAMES,        8
    .equ BAND,          75              # rows of a colour band
    .equ STEP,          25              # rows the bands move down each frame

# Kept across interrupts:
#   s0 = FB_BASE        s1 = width          s2 = height         s3 = stride
#   s4 = frame number   s5 = front buffer   s6 = back buffer    s7 = `bands`
#   s8 = vertical-blank interrupts taken (the handler counts them)
#   s9, s10, s11: the handler's own

    .section .text.start
    .globl _start
_start:
    la    sp, __stack_top
    la    t0, trap
    csrw  mtvec, t0
    li    s0, FB_BASE
    li    s8, 0

    # Find the 800x600 mode: a7 = its number.
    lw    t0, FB_MODES(s0)
    li    a7, 0
    li    t3, WIDTH
    li    t4, HEIGHT
1:  beq   a7, t0, no_mode
    sw    a7, FB_MODE_SEL(s0)
    lw    t1, FB_MODE_WIDTH(s0)
    lw    t2, FB_MODE_HEIGHT(s0)
    bne   t1, t3, 2f
    beq   t2, t4, 3f
2:  addi  a7, a7, 1
    j     1b
3:  mv    s1, t1
    mv    s2, t2
    lw    s3, FB_MODE_STRIDE(s0)
    mul   t0, s3, s2                    # a frame's bytes
    li    t1, BUF_SIZE
    bltu  t1, t0, no_mode               # more than a buffer holds
    la    s5, fb0
    la    s6, fb1
    la    s7, bands

    # Frame 0 into the first buffer, then the display on and the modeset: 800x600
    # from that buffer, at once.
    li    s4, 0
    mv    a0, s5
    call  draw
    li    t0, 1
    sw    t0, FB_CTRL(s0)
    mv    a0, s5
    call  set_scanout
    sw    a7, FB_MODE(s0)
    lw    t0, FB_STATUS(s0)
    bnez  t0, refused

    # The vertical blank interrupts on PLIC source 16, priority 1, enabled for
    # context 0, threshold 0; then in the display controller, and on the hart.
    li    t0, PLIC_BASE
    li    t1, 1
    sw    t1, 4*FB_SRC(t0)
    li    t0, PLIC_ENABLE0
    li    t1, 1 << FB_SRC
    sw    t1, 0(t0)
    li    t0, PLIC_THRESH0
    sw    zero, 0(t0)
    li    t1, IRQ_ON_CLEAR
    sw    t1, FB_IRQ(s0)
    li    t0, MIE_MEIE
    csrs  mie, t0
    csrsi mstatus, MSTATUS_MIE

next:
    addi  s4, s4, 1
    li    t0, FRAMES
    beq   s4, t0, done
    # Draw the next frame where nobody looks, and flip to it.
    mv    a0, s6
    call  draw
    mv    a0, s6
    call  set_scanout
    li    t0, 1
    sw    t0, FB_FLIP(s0)
1:  lw    t0, FB_STATUS(s0)
    andi  t1, t0, ~FLIP_PENDING
    bnez  t1, refused
    andi  t0, t0, FLIP_PENDING
    beqz  t0, 2f                        # shown: the old front buffer is free
    wfi                                 # until the next vertical blank
    j     1b
2:  mv    t0, s5
    mv    s5, s6
    mv    s6, t0
    j     next

done:
    # How many frames were shown.
    xcal.dbg s4
    xcal.halt zero

no_mode:
    li    t0, 2
    xcal.halt t0

refused:
    # Show FB_STATUS, then halt with exit code 3.
    xcal.dbg t0
    li    t0, 3
    xcal.halt t0

# The vertical-blank interrupt: acknowledge it (which lowers the line), count it.
# Uses s9-s11 only, which the rest of the program leaves alone.
    .p2align 2
trap:
    csrr  s10, mcause
    li    s11, CAUSE_MEI
    bne   s10, s11, fail
    li    s9, PLIC_CLAIM0
    lw    s10, 0(s9)                    # claim
    li    s11, FB_SRC
    bne   s10, s11, fail
    li    s11, IRQ_ON_CLEAR
    sw    s11, FB_IRQ(s0)
    addi  s8, s8, 1
    sw    s10, 0(s9)                    # complete
    mret

fail:
    # Show the unexpected mcause or source, then halt with exit code 1.
    xcal.dbg s10
    li    s10, 1
    xcal.halt s10

# set_scanout(a0 = buffer): FB_SCANOUT = a0, a physical address below 4 GiB.
set_scanout:
    sd    a0, FB_SCANOUT(s0)
    ret

# draw(a0 = buffer): frame s4, bands of BAND rows, the colours of `bands` from the top
# down, moved down by STEP rows a frame: row y is bands[(y + STEP * s4) / BAND % 8].
# Clobbers t0-t6.
draw:
    li    t0, STEP
    mul   t0, t0, s4                    # how far the bands moved
    li    t1, 0                         # y
    mv    t2, a0                        # start of row y
    slli  t5, s1, 2                     # bytes of pixels in a row
1:  add   t3, t1, t0
    li    t4, BAND
    divu  t3, t3, t4
    andi  t3, t3, 7
    slli  t3, t3, 2
    add   t3, s7, t3
    lw    t3, 0(t3)                     # the row's colour
    mv    t4, t2
    add   t6, t2, t5                    # end of the row
2:  sw    t3, 0(t4)
    addi  t4, t4, 4
    bltu  t4, t6, 2b
    add   t2, t2, s3
    addi  t1, t1, 1
    bltu  t1, s2, 1b
    ret

    .section .rodata
    .balign 4
# 0x00RRGGBB: white, yellow, cyan, green, magenta, red, blue, black.
bands:
    .word 0x00ffffff, 0x00ffff00, 0x0000ffff, 0x0000ff00
    .word 0x00ff00ff, 0x00ff0000, 0x000000ff, 0x00000000

    .section .bss
    .balign 4096
fb0:    .space BUF_SIZE
fb1:    .space BUF_SIZE
