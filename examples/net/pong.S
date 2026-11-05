# Network card demo for Grail (spec section 9.8), the side that answers: finds the
# first network card, sleeps in wfi until its receive interrupt says a frame came
# in, prints the frame, and sends a frame back to its sender. examples/net/ping.S is
# the side that speaks first. Both use the driver in netlib.S.
#
# Two machines on one hub, as crates/grail/tests/net.rs runs them, print
#
#   net 0x10009000, irq 12, mac 02:ca:00:00:00:02, link up
#   received 60 bytes from 02:ca:00:00:00:01, type 0x88b5: Hello from ping!
#   sent 60 bytes to 02:ca:00:00:00:01
#
# pong must be listening (its receive ring set up) before ping speaks: a frame that
# arrives while the card has no buffer is dropped, as on a real network.
#
# Exit code 0 after answering, 1 without a network card, 2 if the frame's descriptor
# has an error status or sending failed, 4 on an unexpected PLIC source.
#
# Built with our own tools by `cargo xtask examples net-pong`, which does the same as
#
#   merlin -E -I include examples/net/pong.S -o pong.s
#   merlin-as pong.s -o pong.o
#   merlin -E -I include examples/net/netlib.S -o netlib.s
#   merlin-as netlib.s -o netlib.o
#   merlin-ld -T examples/net/link.ld pong.o netlib.o -o examples/net/net-pong.elf
#
# Reference build with LLVM, only for comparison:
#
#   clang --target=riscv64 -march=rv64ima_zicsr_zifencei -mabi=lp64 -mcmodel=medany -mno-relax \
#         -nostdlib -static -fuse-ld=lld -Wl,--no-relax,--discard-none -include examples/xcal.inc \
#         -I include -T examples/net/link.ld examples/net/pong.S examples/net/netlib.S \
#         -o net-pong.elf

    .equ FRAME_SIZE,      1536

    .section .text.start
    .globl _start
_start:
    la    sp, __stack_top
    call  net_init
    bnez  a0, no_card

1:  call  net_wait                  # sleep until a frame is in
    bnez  a0, wrong_irq
    call  net_poll
    beqz  a0, 1b
    bnez  a2, failed
    mv    s1, a0                    # s1 = the frame
    call  print_frame

    la    a0, reply                 # "Hello back from pong!" to its sender
    addi  a1, s1, 6
    la    a2, s_hello_back
    call  make_frame
    mv    s2, a0
    call  net_release               # done with the received frame
    la    a0, reply
    mv    a1, s2
    call  net_send
    bnez  a0, failed
    mv    a0, s2
    la    a1, reply                 # its destination, as it went out
    call  print_sent
    li    a0, 0
    j     halt

no_card:
    la    a0, s_no_card
    call  puts
    li    a0, 1
    j     halt

wrong_irq:
    la    a0, s_wrong_irq
    call  puts
    li    a0, 4
    j     halt

failed:
    la    a0, s_failed
    call  puts
    li    a0, 2

    # Stop the machine with exit code a0.
halt:
    xcal.halt a0

    .section .rodata
s_hello_back:   .asciz "Hello back from pong!"
s_no_card:      .asciz "no network card (try grail --bios none --net hub)\n"
s_wrong_irq:    .asciz "unexpected PLIC source\n"
s_failed:       .asciz "network error\n"

    .section .bss
    .balign 4
reply:  .space FRAME_SIZE
