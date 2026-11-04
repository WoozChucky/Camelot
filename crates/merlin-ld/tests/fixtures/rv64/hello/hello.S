# Bare-metal hello world for Grail, built with our own tools:
#
#   cargo xtask examples hello
#
# which does the same as
#
#   merlin-as examples/hello/hello.S -o hello.o
#   merlin-ld -T examples/hello/link.ld hello.o -o examples/hello/hello.elf
#
# Reference build with LLVM, only for comparison (same loaded bytes; the
# differences elsewhere in the file are listed in docs/excalibur-spec.md
# section 10, "Examples"):
#
#   clang --target=riscv32 -march=rv32im_zicsr -mabi=ilp32 -mno-relax -nostdlib -static \
#         -fuse-ld=lld -Wl,--no-relax,--discard-none -include examples/xcal.inc \
#         -T examples/hello/link.ld examples/hello/hello.S -o hello.elf

    .equ UART_BASE, 0x10000000
    .equ UART_LSR,  5
    .equ LSR_THRE,  0x20

    .section .text.start
    .globl _start
_start:
    la    sp, __stack_top
    la    a0, message
    call  puts

    # Show how many bytes we printed (a0 holds the count).
    xcal.dbg a0

    # Stop the machine with exit code 0.
    xcal.halt zero

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
