# data: checks the memory the kernel gives a program. Before anything is
# pushed, its stack page must read zero below sp (above it are its arguments,
# which the kernel put there), and so must all of .bss, which spans two pages
# (the second has no bytes in the file); .data must hold its initial value;
# and both must be writable. Exits with status 0 if all of
# that holds, else with the number of the check that failed. On the way out
# it fills its stack and .bss with non-zero bytes, so that the pages it gives
# back are dirty for whoever gets them next.
#
# The same code for RV32 and RV64: the stack page is the one sp is in (its
# address depends on USER_TOP, which on RV64 depends on the paging mode),
# and the words are 32-bit, compared as lw reads them, sign-extended.

    .equ STACK_SIZE, 0x1000
    .equ BSS_SIZE, 0x1800
    .equ INITIAL, 0x600df00d
    # 0xa5a5a5a5 as a 32-bit word: negative, so that li makes the value lw
    # reads back on RV64 too.
    .equ DIRT, -0x5a5a5a5b

    .text
    .globl main
main:
    li    s0, 1                 # the stack page reads zero below sp
    srli  a0, sp, 12            # its start: sp with the low 12 bits clear
    slli  a0, a0, 12
    sub   a1, sp, a0
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

    srli  a0, sp, 12            # dirty the stack too, then exit
    slli  a0, a0, 12
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
