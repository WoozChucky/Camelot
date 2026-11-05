# The cursor plane of Grail's display controller (spec section 9.3): draws one frame,
# colour bars at 640x480 in VRAM, presents it once, then moves a cursor over it along
# a path, a place a frame, without ever drawing into the frame again. The display
# blends the cursor's image over the frame where FB_CURSOR_POS puts it, as KMS's
# cursor plane does, so moving the pointer costs a register write, not a redraw.
#
# - The image is 64x64 pixels of ARGB8888 with premultiplied alpha, in a page of .bss
#   (which starts clear: alpha 0, nothing shown). The program draws an arrow into its
#   top left corner, its tip at (0, 0): a black outline around white, both opaque, and
#   below it a tail half covered in blue (alpha 0x80 and blue 0x80, already multiplied
#   by alpha), so the bars show through it.
# - FB_CURSOR_ADDR, FB_CURSOR_HOT (the tip, (0, 0)), FB_CURSOR_POS (the path's first
#   place) and FB_CURSOR_CTRL (on) are written together; the cursor registers are
#   double-buffered, so the next vertical blank takes them all and the cursor shows
#   from that frame on. FB_STATUS.CURSOR_PENDING is set until then: the program sleeps
#   in wfi until it clears, woken by the vertical-blank interrupt (PLIC source 16),
#   whose handler acknowledges it in FB_IRQ and counts it.
# - Then each next place of the path: one write of FB_CURSOR_POS, and a sleep until
#   the vertical blank has taken it. The path goes off the top left corner (only the
#   end of the tail shows) and off the bottom right one (only the tip shows), as
#   FB_CURSOR_POS's signed halves allow.
#
# After the last place it prints how many it showed with xcal.dbg and halts with exit
# code 0. A cursor the display refuses (FB_STATUS.BAD_CURSOR) halts it with exit code
# 3, and any unexpected trap with exit code 1.
#
# Run it with the framebuffer window (Windows), at a real-time clock so that the
# cursor goes by at 60 places a second:
#
#   cargo run --release -p grail -- --fb --clock 50MHz examples/cursor/cursor.elf
#
# or headless, dumping a frame each time the cursor moves (one for the present first,
# one at exit last):  grail --fb-dump out examples/cursor/cursor.elf
# (cursor-rv64.elf on an RV64 machine).
#
# Built with our own tools by `cargo xtask examples cursor`, which does the same as
#
#   merlin -E -I include examples/cursor/cursor.S -o cursor.s
#   merlin-as cursor.s -o cursor.o
#   merlin-ld -T examples/cursor/link.ld cursor.o -o examples/cursor/cursor.elf
#
# and, for RV64 (the same code: mcause's interrupt bit, CAUSE_MEI, is bit 63, and
# FB_CURSOR_ADDR, a 64-bit register, is written whole with sd instead of by its
# halves), the same as
#
#   merlin -E -march=rv64ima_zicsr_zifencei -I include examples/cursor/cursor.S -o cursor.s
#   merlin-as -march=rv64ima_zicsr_zifencei cursor.s -o cursor.o
#   merlin-ld -T examples/cursor/link.ld cursor.o -o examples/cursor/cursor-rv64.elf
#
# Reference build with LLVM, only for comparison:
#
#   clang --target=riscv32 -march=rv32im_zicsr -mabi=ilp32 -mno-relax -nostdlib -static \
#         -fuse-ld=lld -Wl,--no-relax,--discard-none -include examples/xcal.inc -I include \
#         -T examples/cursor/link.ld examples/cursor/cursor.S -o cursor.elf
#
# and for RV64:
#
#   clang --target=riscv64 -march=rv64ima_zicsr_zifencei -mabi=lp64 -mcmodel=medany -mno-relax \
#         -nostdlib -static -fuse-ld=lld -Wl,--no-relax,--discard-none -include examples/xcal.inc \
#         -I include -T examples/cursor/link.ld examples/cursor/cursor.S -o cursor-rv64.elf

#include <asm.h>

    .equ FB_BASE,       0x30000000      # framebuffer registers (spec section 9.3)
    .equ FB_CTRL,       0x10            # bit 0: display enable
    .equ FB_PRESENT,    0x14            # write anything: a new frame
    .equ FB_STATUS,     0x34            # bit 3: cursor pending; bit 4: bad cursor
    .equ FB_IRQ,        0x44            # bit 0: enable; bit 1: pending, write 1 to clear
    .equ FB_CURSOR_CTRL, 0x4c           # bit 0: cursor plane enable
    .equ FB_CURSOR_ADDR, 0x50           # 64-bit: the image, page-aligned in RAM
    .equ FB_CURSOR_POS, 0x58            # x in bits 15:0, y in bits 31:16, signed
    .equ FB_CURSOR_HOT, 0x5c            # the hotspot: x in bits 5:0, y in bits 21:16
    .equ CURSOR_PENDING, 8
    .equ BAD_CURSOR,    16
    .equ IRQ_ON_CLEAR,  3               # enable, and clear the pending bit
    .equ FB_SRC,        16              # PLIC source of the vertical blank
    .equ VRAM,          0x31000000

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

    .equ WIDTH,         640
    .equ HEIGHT,        480
    .equ BAR,           80              # columns of a colour bar
    .equ CURSOR_ROW,    256             # bytes of a row of the image: 64 x 4
    .equ ARROW,         16              # rows of the arrow
    .equ TAIL,          8               # rows of the tail below it, and its width
    .equ OPAQUE_BLACK,  0xff000000
    .equ OPAQUE_WHITE,  0xffffffff
    .equ HALF_BLUE,     0x80000080      # alpha 0x80, blue 0x80: blue, half covered
    .equ PLACES,        6               # places in `path`

# Kept across interrupts:
#   s0 = FB_BASE        s1 = place in the path      s2 = `path`
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

    # The frame: eight bars of BAR columns, the colours of `bars` from the left. Row
    # by row, pixel by pixel: pixel x is bars[x / BAR].
    li    t0, VRAM
    la    t5, bars
    li    t1, 0                         # y
1:  li    t2, 0                         # x
2:  li    t3, BAR
    divu  t3, t2, t3
    slli  t3, t3, 2
    add   t3, t5, t3
    lw    t3, 0(t3)
    sw    t3, 0(t0)
    addi  t0, t0, 4
    addi  t2, t2, 1
    li    t3, WIDTH
    bltu  t2, t3, 2b
    addi  t1, t1, 1
    li    t3, HEIGHT
    bltu  t1, t3, 1b
    # Shown, once: the display on, and the frame presented. Nothing draws into it again.
    li    t0, 1
    sw    t0, FB_CTRL(s0)
    sw    t0, FB_PRESENT(s0)

    # The arrow: row y (0 to ARROW-1) has its pixels 0 to y, the first, the last and
    # all of the last row black, the others white.
    la    t0, cursor                    # start of row y
    li    t1, 0                         # y
    li    t4, OPAQUE_BLACK
    li    t5, OPAQUE_WHITE
1:  li    t2, 0                         # x
2:  mv    t3, t5
    beqz  t2, 3f
    beq   t2, t1, 3f
    li    t6, ARROW - 1
    beq   t1, t6, 3f
    j     4f
3:  mv    t3, t4
4:  slli  t6, t2, 2
    add   t6, t0, t6
    sw    t3, 0(t6)
    addi  t2, t2, 1
    bgeu  t1, t2, 2b                    # x <= y
    addi  t0, t0, CURSOR_ROW
    addi  t1, t1, 1
    li    t6, ARROW
    bltu  t1, t6, 1b
    # The tail: TAIL rows of TAIL pixels, half-covered blue.
    li    t4, HALF_BLUE
    li    t1, 0
1:  li    t2, 0
2:  slli  t6, t2, 2
    add   t6, t0, t6
    sw    t4, 0(t6)
    addi  t2, t2, 1
    li    t6, TAIL
    bltu  t2, t6, 2b
    addi  t0, t0, CURSOR_ROW
    addi  t1, t1, 1
    bltu  t1, t6, 1b

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

    # The cursor: its image, hotspot, first place and the plane on, all taken by the
    # next vertical blank.
    la    a0, cursor
#if __riscv_xlen == 64
    sd    a0, FB_CURSOR_ADDR(s0)
#else
    sw    a0, FB_CURSOR_ADDR(s0)        # the low half
    sw    zero, FB_CURSOR_ADDR+4(s0)    # and the high half
#endif
    sw    zero, FB_CURSOR_HOT(s0)       # the tip, (0, 0)
    la    s2, path
    li    s1, 0
    lw    t0, 0(s2)
    sw    t0, FB_CURSOR_POS(s0)
    li    t0, 1
    sw    t0, FB_CURSOR_CTRL(s0)
    call  wait

next:
    # The next place: one register write, then the vertical blank that takes it.
    addi  s1, s1, 1
    li    t0, PLACES
    beq   s1, t0, done
    slli  t0, s1, 2
    add   t0, s2, t0
    lw    t0, 0(t0)
    sw    t0, FB_CURSOR_POS(s0)
    call  wait
    j     next

done:
    # How many places the cursor was shown at.
    xcal.dbg s1
    xcal.halt zero

refused:
    # Show FB_STATUS, then halt with exit code 3.
    xcal.dbg t0
    li    t0, 3
    xcal.halt t0

# wait: sleeps until a vertical blank has taken the cursor registers written, then
# checks the display took the cursor. Clobbers t0, t1.
wait:
1:  lw    t0, FB_STATUS(s0)
    andi  t1, t0, BAD_CURSOR
    bnez  t1, refused
    andi  t1, t0, CURSOR_PENDING
    beqz  t1, 2f
    wfi                                 # until the next vertical blank
    j     1b
2:  ret

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

    .section .rodata
    .balign 4
# 0x00RRGGBB: white, yellow, cyan, green, magenta, red, blue, black.
bars:
    .word 0x00ffffff, 0x00ffff00, 0x0000ffff, 0x0000ff00
    .word 0x00ff00ff, 0x00ff0000, 0x000000ff, 0x00000000

# The places of the cursor's top left corner, as FB_CURSOR_POS takes them: x in the
# low half, y in the high half, each a signed 16-bit number. PLACES of them.
path:
    .word 0x00640064                    # (100, 100)
    .word 0x00c8012c                    # (300, 200)
    .word 0xfff4fffc                    # (-4, -12): off the top left corner
    .word 0x01d8027a                    # (634, 472): off the bottom right corner
    .word 0x00f00140                    # (320, 240)
    .word 0x00000000                    # (0, 0)

    .section .bss
    .balign 4096
cursor: .space 64 * CURSOR_ROW          # the image, 64 rows of 64 pixels
