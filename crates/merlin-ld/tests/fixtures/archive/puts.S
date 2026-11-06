# puts: writes a string with putc, a byte at a time, and returns its
# length (strlen). Its references extract strlen.o, which comes before it
# in libdemo.a, at once, and putc.o, after it, when ld.lld reads that.
    .text
    .globl puts
puts:
    addi  sp, sp, -32
    sd    ra, 24(sp)
    sd    s0, 16(sp)
    sd    s1, 8(sp)
    mv    s0, a0
    call  strlen
    mv    s1, a0
1:  lbu   a0, 0(s0)
    beqz  a0, 2f
    call  putc
    addi  s0, s0, 1
    j     1b
2:  mv    a0, s1
    ld    ra, 24(sp)
    ld    s0, 16(sp)
    ld    s1, 8(sp)
    addi  sp, sp, 32
    ret
