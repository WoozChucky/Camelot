# data: checks the memory the kernel gives a program. Before anything is
# pushed, its whole stack page must read zero, and so must all of .bss, which
# spans two pages (the second has no bytes in the file); .data must hold its
# initial value; and both must be writable. Exits with status 0 if all of
# that holds, else with the number of the check that failed. On the way out
# it fills its stack and .bss with non-zero bytes, so that the pages it gives
# back are dirty for whoever gets them next.

    .equ USER_TOP, 0x00400000
    .equ STACK_SIZE, 0x1000
    .equ BSS_SIZE, 0x1800
    .equ INITIAL, 0x600df00d
    .equ DIRT, 0xa5a5a5a5

    .section .text.start
    .globl _start
_start:
    li    s0, 1                 # the stack page reads zero
    li    a0, USER_TOP - STACK_SIZE
    li    a1, STACK_SIZE
    call  all_zero
    bnez  a0, fail

    li    s0, 2                 # so does .bss
    la    a0, buf
    li    a1, BSS_SIZE
    call  all_zero
    bnez  a0, fail

    li    s0, 3                 # .data holds its initial value
    la    t0, word
    lw    t1, 0(t0)
    li    t2, INITIAL
    bne   t1, t2, fail

    li    s0, 4                 # .data is writable
    li    t2, DIRT
    sw    t2, 0(t0)
    lw    t1, 0(t0)
    bne   t1, t2, fail

    li    s0, 5                 # and so is .bss, to its last word
    la    a0, buf
    li    a1, BSS_SIZE
    call  fill
    la    t0, buf
    li    t1, BSS_SIZE - 4
    add   t0, t0, t1
    lw    t1, 0(t0)
    li    t2, DIRT
    bne   t1, t2, fail

    li    a0, USER_TOP - STACK_SIZE # dirty the stack too, then exit
    li    a1, STACK_SIZE
    call  fill
    li    a0, 0
    call  exit

fail:
    mv    a0, s0
    call  exit

# u32 all_zero(const u32 *p, u32 len): 0 if the len bytes at p (a multiple of
# 4) are all zero, else non-zero. Uses no stack.
all_zero:
    add   a1, a0, a1
1:  bgeu  a0, a1, 2f
    lw    t0, 0(a0)
    bnez  t0, 3f
    addi  a0, a0, 4
    j     1b
2:  li    a0, 0
    ret
3:  li    a0, 1
    ret

# void fill(u32 *p, u32 len): stores DIRT in the len bytes at p (a multiple of
# 4). Uses no stack.
fill:
    li    t0, DIRT
    add   a1, a0, a1
1:  bgeu  a0, a1, 2f
    sw    t0, 0(a0)
    addi  a0, a0, 4
    j     1b
2:  ret

    .data
    .align 2
word:
    .word INITIAL

    .bss
    .align 2
buf:
    .zero BSS_SIZE
