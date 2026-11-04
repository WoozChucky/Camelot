# Network card demo for Grail (spec section 9.8), the side that speaks first: finds
# the first network card, sends one Ethernet frame to everyone (the broadcast
# address) and waits for an answer, polling the receive ring (no interrupts), then
# prints it. examples/net/pong.S is the side that answers. Both use the driver in
# netlib.S.
#
# Two machines on one hub, as crates/grail/tests/net.rs runs them, print
#
#   net 0x10009000, irq 12, mac 02:ca:00:00:00:01, link up
#   sent 60 bytes to ff:ff:ff:ff:ff:ff
#   received 60 bytes from 02:ca:00:00:00:02, type 0x88b5: Hello back from pong!
#
# The grail command runs one machine, so nobody answers there; with a capture of the
# card, the frame it sent is in the file (Wireshark shows it as an "IEEE 802a" frame):
#
#   grail --net hub --pcap ping.pcap examples/net/net-ping.elf
#
# Exit code 0 when an answer came, 1 without a network card, 2 if sending failed or
# the answer's descriptor has an error status, 3 if no answer came in time.
#
# Built with our own tools by `cargo xtask examples net-ping`, which does the same as
#
#   merlin -E -I include examples/net/ping.S -o ping.s
#   merlin-as ping.s -o ping.o
#   merlin -E -I include examples/net/netlib.S -o netlib.s
#   merlin-as netlib.s -o netlib.o
#   merlin-ld -T examples/net/link.ld ping.o netlib.o -o examples/net/net-ping.elf
#
# and, for RV64 (the same code here; the driver has what differs), the same as
#
#   merlin -E -march=rv64ima_zicsr_zifencei -I include examples/net/ping.S -o ping.s
#   merlin-as -march=rv64ima_zicsr_zifencei ping.s -o ping.o
#   merlin -E -march=rv64ima_zicsr_zifencei -I include examples/net/netlib.S -o netlib.s
#   merlin-as -march=rv64ima_zicsr_zifencei netlib.s -o netlib.o
#   merlin-ld -T examples/net/link.ld ping.o netlib.o -o examples/net/net-ping-rv64.elf
#
# Reference build with LLVM, only for comparison:
#
#   clang --target=riscv32 -march=rv32im_zicsr -mabi=ilp32 -mno-relax -nostdlib -static \
#         -fuse-ld=lld -Wl,--no-relax,--discard-none -include examples/xcal.inc -I include \
#         -T examples/net/link.ld examples/net/ping.S examples/net/netlib.S -o net-ping.elf
#
# and for RV64:
#
#   clang --target=riscv64 -march=rv64ima_zicsr_zifencei -mabi=lp64 -mcmodel=medany -mno-relax \
#         -nostdlib -static -fuse-ld=lld -Wl,--no-relax,--discard-none -include examples/xcal.inc \
#         -I include -T examples/net/link.ld examples/net/ping.S examples/net/netlib.S \
#         -o net-ping-rv64.elf

    .equ FRAME_SIZE,      1536
    .equ TIMEOUT,         200000        # polls of the receive ring before giving up

    .section .text.start
    .globl _start
_start:
    la    sp, __stack_top
    call  net_init
    bnez  a0, no_card

    la    a0, frame                 # broadcast "Hello from ping!"
    la    a1, broadcast
    la    a2, s_hello
    call  make_frame
    mv    s1, a0
    mv    a1, a0
    la    a0, frame
    call  net_send
    bnez  a0, failed
    mv    a0, s1
    la    a1, broadcast
    call  print_sent

    li    s2, TIMEOUT
1:  call  net_poll
    bnez  a0, 2f
    addi  s2, s2, -1
    bnez  s2, 1b
    la    a0, s_no_answer
    call  puts
    li    a0, 3
    j     halt

2:  bnez  a2, failed                # a0 = the frame, a1 = its length
    call  print_frame
    call  net_release
    li    a0, 0
    j     halt

no_card:
    la    a0, s_no_card
    call  puts
    li    a0, 1
    j     halt

failed:
    la    a0, s_failed
    call  puts
    li    a0, 2

    # Stop the machine with exit code a0.
halt:
    xcal.halt a0

    .section .rodata
broadcast:      .byte 0xff, 0xff, 0xff, 0xff, 0xff, 0xff
s_hello:        .asciz "Hello from ping!"
s_no_answer:    .asciz "no answer\n"
s_no_card:      .asciz "no network card (try grail --net hub)\n"
s_failed:       .asciz "network error\n"

    .section .bss
    .balign 4
frame:  .space FRAME_SIZE
