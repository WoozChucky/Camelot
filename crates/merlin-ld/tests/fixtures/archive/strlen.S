# libdemo.a's first member: strlen. puts.o needs it, and comes after it in
# the archive, so ld.lld links it right after puts.o, when puts.o's
# reference to it is resolved.
    .text
    .globl strlen
strlen:
    mv    t0, a0
1:  lbu   t1, 0(t0)
    beqz  t1, 2f
    addi  t0, t0, 1
    j     1b
2:  sub   a0, t0, a0
    ret
