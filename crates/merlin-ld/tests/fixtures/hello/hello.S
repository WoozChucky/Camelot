# Bare-metal hello world for Grail. Built with clang for now; merlin-as will
# assemble this same file once it exists.
#
#   clang --target=riscv32 -march=rv32im_zicsr -mabi=ilp32 -nostdlib -static \
#         -fuse-ld=lld -T examples/hello/link.ld examples/hello/hello.S -o hello.elf

    .equ UART_BASE, 0x10000000
    .equ UART_LSR,  5
    .equ LSR_THRE,  0x20

    .section .text.start
    .globl _start
_start:
    la    sp, __stack_top
    la    a0, message
    call  puts

    # xcal.dbg a0: show how many bytes we printed (a0 holds the count).
    .insn i 0x0b, 0, x0, a0, 0

    # xcal.halt zero: stop the machine with exit code 0.
    .insn i 0x0b, 1, x0, zero, 0

# puts(a0 = NUL-terminated string) -> a0 = bytes written
puts:
    li    t0, UART_BASE
    mv    t1, a0
1:  lbu   t2, 0(t1)
    beqz  t2, 3f
2:  lbu   t3, UART_LSR(t0)      # wait until the transmitter is free
    andi  t3, t3, LSR_THRE
    beqz  t3, 2b
    sb    t2, 0(t0)
    addi  t1, t1, 1
    j     1b
3:  sub   a0, t1, a0
    ret

    .section .rodata
message:
    .asciz "Hello from Camelot!\n"
