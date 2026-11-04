# User networking demo for Grail (spec section 9.8, "User networking"): a bare-metal
# program that gets an address by DHCP from the NAT gateway behind `grail --net user`
# and speaks just enough TCP to open a connection to the host and to accept one from
# it. It uses the card driver in netlib.S.
#
# After DHCP it reads commands from the UART, one per line:
#
#   c PORT   connect to 10.0.2.2:PORT (the host's 127.0.0.1), send a greeting, print
#            what comes back until the host closes, then close
#   l PORT   listen on PORT, accept one connection (a host port forwarded with
#            hostfwd), echo every byte back until the host closes, then close
#   q        exit
#
#   printf 'c 8000\nq\n' | grail --net user examples/net/net-user.elf
#
# with something listening on the host's port 8000 prints
#
#   net 0x10009000, irq 12, mac 52:54:00:12:34:56, link up
#   dhcp: address 10.0.2.15, server 10.0.2.2
#   connect 10.0.2.2:8000
#   received: (what the host sent)
#   closed
#
# and `printf 'l 7\n' | grail --net user,hostfwd=tcp::5555-:7 ...` echoes for a
# client of the host's port 5555 (crates/grail/tests/net_user.rs does both).
#
# The TCP here is the least that works on the gateway's link, which delivers the
# guest's frames in order and never loses them: one connection at a time, no
# retransmission of our own (the gateway retransmits its segments if one is dropped),
# a fixed window, segments taken only in order. Every frame goes to the gateway's MAC
# address (learned from its DHCP replies), so no ARP either: the gateway learned ours
# from DHCP too. Fields on the wire are big-endian, so they are built byte by byte
# (put16/put32, get16/get32), and the frames' 4-byte fields are not word-aligned.
#
# Exit code 0 after `q`, 1 without a network card, 2 on a DHCP NAK, a refused or reset
# connection or a send error.
#
# On RV64 machines (net-user-rv64.elf) the functions save ra and the s registers
# with REG_S (sd), in frames of SZREG-sized slots, and the TCP sequence numbers,
# 32-bit words, are added with addw and addiw (ADDW, ADDIW), modulo 2^32 like the
# fields they go to.
#
# Built with our own tools by `cargo xtask examples net-user`, which does the same as
#
#   merlin -E -I include examples/net/user.S -o user.s
#   merlin-as user.s -o user.o
#   merlin -E -I include examples/net/netlib.S -o netlib.s
#   merlin-as netlib.s -o netlib.o
#   merlin-ld -T examples/net/link.ld user.o netlib.o -o examples/net/net-user.elf
#
# and, for RV64, the same as
#
#   merlin -E -march=rv64ima_zicsr_zifencei -I include examples/net/user.S -o user.s
#   merlin-as -march=rv64ima_zicsr_zifencei user.s -o user.o
#   merlin -E -march=rv64ima_zicsr_zifencei -I include examples/net/netlib.S -o netlib.s
#   merlin-as -march=rv64ima_zicsr_zifencei netlib.s -o netlib.o
#   merlin-ld -T examples/net/link.ld user.o netlib.o -o examples/net/net-user-rv64.elf
#
# Reference build with LLVM, only for comparison:
#
#   clang --target=riscv32 -march=rv32im_zicsr -mabi=ilp32 -mno-relax -nostdlib -static \
#         -fuse-ld=lld -Wl,--no-relax,--discard-none -include examples/xcal.inc -I include \
#         -T examples/net/link.ld examples/net/user.S examples/net/netlib.S -o net-user.elf
#
# and for RV64:
#
#   clang --target=riscv64 -march=rv64ima_zicsr_zifencei -mabi=lp64 -mcmodel=medany -mno-relax \
#         -nostdlib -static -fuse-ld=lld -Wl,--no-relax,--discard-none -include examples/xcal.inc \
#         -I include -T examples/net/link.ld examples/net/user.S examples/net/netlib.S \
#         -o net-user-rv64.elf

#include <asm.h>

#if __riscv_xlen == 64
#define IP_FRAME 48     /* ip_header's frame: five registers, 16-byte aligned */
#else
#define IP_FRAME 32
#endif

    .equ UART_BASE,       0x10000000
    .equ UART_RBR,        0             # receive buffer
    .equ UART_LSR,        5
    .equ LSR_DR,          1             # data ready
    .equ LINE_MAX,        32

    # Offsets in the frames we build: Ethernet header, IPv4 header (no options), then
    # UDP or TCP.
    .equ ETH_HDR,         14
    .equ IP_HDR,          20
    .equ UDP_HDR,         8
    .equ TCP_HDR,         20
    .equ L4_OFF,          34            # ETH_HDR + IP_HDR
    .equ UDP_DATA,        42
    .equ TCP_DATA,        54
    .equ FRAME_SIZE,      1536

    .equ PROTO_TCP,       6
    .equ PROTO_UDP,       17

    # DHCP (RFC 2131): a BOOTP message, the magic cookie, options.
    .equ BOOTP_SIZE,      300           # BOOTP's minimum message
    .equ DHCP_OPTIONS,    240           # after the 236-byte fixed part and the cookie
    .equ DHCP_MAGIC,      0x63825363
    .equ XID,             0x43414d4c    # transaction id: "CAML"
    .equ DHCP_DISCOVER,   1
    .equ DHCP_OFFER,      2
    .equ DHCP_REQUEST,    3
    .equ DHCP_ACK,        5
    .equ DHCP_NAK,        6
    .equ OPT_TYPE,        53
    .equ OPT_REQUESTED,   50
    .equ OPT_SERVER,      54
    .equ OPT_END,         255

    # TCP flags.
    .equ TCP_FIN,         0x01
    .equ TCP_SYN,         0x02
    .equ TCP_RST,         0x04
    .equ TCP_ACK,         0x10
    .equ TCP_SYN_ACK,     0x12
    .equ TCP_FIN_ACK,     0x11
    .equ TCP_PSH_ACK,     0x18
    .equ WINDOW,          1024          # what we take at once: two of the gateway's
                                        # 536-byte segments, in a ring of 4 buffers
    .equ ISS,             0x1000        # our first sequence number
    .equ CLIENT_PORT,     40000         # our end of a connection we open
    .equ GREETING_LEN,    22            # s_greeting's length

    .section .text.start
    .globl _start
_start:
    la    sp, __stack_top
    call  net_init
    bnez  a0, no_card
    call  dhcp
    bnez  a0, failed

command:
    call  readline                  # a0 = the line
    lbu   t0, 0(a0)
    li    t1, 'c'
    beq   t0, t1, 1f
    li    t1, 'l'
    beq   t0, t1, 2f
    li    t1, 'q'
    beq   t0, t1, quit
    j     command                   # blank or unknown: ignored
1:  addi  a0, a0, 1
    call  parse_dec
    call  tcp_client
    bnez  a0, failed
    j     command
2:  addi  a0, a0, 1
    call  parse_dec
    call  tcp_server
    bnez  a0, failed
    j     command

quit:
    li    a0, 0
    j     halt

no_card:
    la    a0, s_no_card
    call  puts
    li    a0, 1
    j     halt

failed:
    li    a0, 2

    # Stop the machine with exit code a0.
halt:
    xcal.halt a0

    .text

# ---- DHCP ----

# dhcp() -> a0 = 0 once the server ACKed an address (now in my_ip), 1 on a NAK.
# DISCOVER, wait for an OFFER, REQUEST the offered address, wait for the ACK.
dhcp:
    addi  sp, sp, -16
    REG_S ra, 16-SZREG(sp)
    li    a0, DHCP_DISCOVER
    call  dhcp_message
    li    a0, DHCP_OFFER
    call  dhcp_reply
    li    t0, DHCP_OFFER
    bne   a0, t0, 1f
    li    a0, DHCP_REQUEST
    call  dhcp_message
    li    a0, DHCP_ACK
    call  dhcp_reply
    li    t0, DHCP_ACK
    bne   a0, t0, 1f
    la    a0, my_ip
    la    a1, offered
    li    a2, 4
    call  memcpy
    la    a0, s_dhcp                # "dhcp: address 10.0.2.15, server 10.0.2.2"
    call  puts
    la    a0, my_ip
    call  put_ip
    la    a0, s_server
    call  puts
    la    a0, server_id
    call  put_ip
    call  newline
    li    a0, 0
    j     2f
1:  la    a0, s_nak
    call  puts
    li    a0, 1
2:  REG_L ra, 16-SZREG(sp)
    addi  sp, sp, 16
    ret

# dhcp_message(a0 = message type): broadcasts a DISCOVER, or a REQUEST for the
# address in `offered` from the server in `server_id`.
dhcp_message:
    addi  sp, sp, -4*SZREG
    REG_S ra, 3*SZREG(sp)
    REG_S s1, 2*SZREG(sp)
    REG_S s2, SZREG(sp)
    mv    s2, a0
    la    s1, txbuf
    addi  s1, s1, UDP_DATA          # s1 = the BOOTP message
    mv    a0, s1
    li    a1, BOOTP_SIZE
    call  memzero
    li    t0, 1
    sb    t0, 0(s1)                 # op: request
    sb    t0, 1(s1)                 # hardware type: Ethernet
    li    t0, 6
    sb    t0, 2(s1)                 # hardware address length
    addi  a0, s1, 4
    li    a1, XID
    call  put32
    addi  a0, s1, 28                # chaddr: our MAC
    la    a1, net_mac
    li    a2, 6
    call  memcpy
    addi  a0, s1, 236
    li    a1, DHCP_MAGIC
    call  put32
    addi  t0, s1, DHCP_OPTIONS
    li    t1, OPT_TYPE              # option 53, message type
    sb    t1, 0(t0)
    li    t1, 1
    sb    t1, 1(t0)
    sb    s2, 2(t0)
    addi  t0, t0, 3
    li    t1, DHCP_REQUEST
    bne   s2, t1, 1f
    li    t1, OPT_REQUESTED         # option 50, the address we want
    sb    t1, 0(t0)
    li    t1, 4
    sb    t1, 1(t0)
    addi  a0, t0, 2
    la    a1, offered
    li    a2, 4
    call  memcpy                    # returns a0 = just past the copy
    li    t1, OPT_SERVER            # option 54, whose offer we take
    sb    t1, 0(a0)
    li    t1, 4
    sb    t1, 1(a0)
    addi  a0, a0, 2
    la    a1, server_id
    li    a2, 4
    call  memcpy
    mv    t0, a0
1:  li    t1, OPT_END
    sb    t1, 0(t0)

    # UDP from port 68 to 67, no checksum (0, which IPv4 allows), to everyone.
    la    s1, txbuf
    addi  a0, s1, L4_OFF
    li    a1, 68
    call  put16
    addi  a0, s1, L4_OFF + 2
    li    a1, 67
    call  put16
    addi  a0, s1, L4_OFF + 4
    li    a1, UDP_HDR + BOOTP_SIZE
    call  put16
    addi  t0, s1, L4_OFF + 6        # checksum
    sb    zero, 0(t0)
    sb    zero, 1(t0)
    li    a0, PROTO_UDP
    li    a1, UDP_HDR + BOOTP_SIZE
    la    a2, ip_broadcast
    la    a3, mac_broadcast
    call  ip_header
    mv    a1, a0
    mv    a0, s1
    call  net_send
    REG_L s2, SZREG(sp)
    REG_L s1, 2*SZREG(sp)
    REG_L ra, 3*SZREG(sp)
    addi  sp, sp, 4*SZREG
    ret

# dhcp_reply(a0 = message type wanted) -> a0 = that type, or DHCP_NAK. Waits for a
# server's reply to our transaction; from the wanted one it keeps the address
# offered (yiaddr), the server's identifier and its MAC address (the gateway's).
dhcp_reply:
    addi  sp, sp, -4*SZREG
    REG_S ra, 3*SZREG(sp)
    REG_S s1, 2*SZREG(sp)
    REG_S s2, SZREG(sp)
    REG_S s3, 0(sp)
    mv    s3, a0
1:  call  net_poll
    beqz  a0, 1b
    bnez  a2, 8f                    # a descriptor with an error
    mv    s1, a0                    # s1 = the frame
    addi  a0, s1, 12
    call  get16
    li    t0, 0x0800                # IPv4
    bne   a0, t0, 8f
    lbu   t0, 23(s1)
    li    t1, PROTO_UDP
    bne   t0, t1, 8f
    lbu   t0, 14(s1)                # IP header length, in words
    andi  t0, t0, 15
    slli  t0, t0, 2
    addi  t0, t0, ETH_HDR
    add   s2, s1, t0                # s2 = the UDP header
    addi  a0, s2, 2
    call  get16
    li    t0, 68                    # to the DHCP client port
    bne   a0, t0, 8f
    addi  s2, s2, UDP_HDR           # s2 = the BOOTP message
    lbu   t0, 0(s2)
    li    t1, 2                     # a reply
    bne   t0, t1, 8f
    addi  a0, s2, 4
    call  get32
    li    t0, XID
    bne   a0, t0, 8f
    mv    a0, s2
    li    a1, OPT_TYPE
    call  dhcp_option
    beqz  a0, 8f
    lbu   t0, 0(a0)
    li    t1, DHCP_NAK
    beq   t0, t1, 7f
    bne   t0, s3, 8f
    la    a0, offered
    addi  a1, s2, 16                # yiaddr
    li    a2, 4
    call  memcpy
    mv    a0, s2
    li    a1, OPT_SERVER
    call  dhcp_option
    beqz  a0, 2f
    mv    a1, a0
    la    a0, server_id
    li    a2, 4
    call  memcpy
2:  la    a0, gw_mac                # the frame's source
    addi  a1, s1, 6
    li    a2, 6
    call  memcpy
    call  net_release
    mv    a0, s3
    j     9f
7:  call  net_release
    li    a0, DHCP_NAK
    j     9f
8:  call  net_release               # not for us: next frame
    j     1b
9:  REG_L s3, 0(sp)
    REG_L s2, SZREG(sp)
    REG_L s1, 2*SZREG(sp)
    REG_L ra, 3*SZREG(sp)
    addi  sp, sp, 4*SZREG
    ret

# dhcp_option(a0 = BOOTP message, a1 = option code) -> a0 = the option's data, or
# 0 when the message does not have it. Clobbers t0-t2.
dhcp_option:
    addi  t2, a0, BOOTP_SIZE + 64   # look no further than this
    addi  a0, a0, DHCP_OPTIONS
1:  bgeu  a0, t2, 3f
    lbu   t0, 0(a0)
    li    t1, OPT_END
    beq   t0, t1, 3f
    bnez  t0, 2f
    addi  a0, a0, 1                 # option 0: one byte of padding
    j     1b
2:  lbu   t1, 1(a0)                 # code, length, data
    addi  a0, a0, 2
    beq   t0, a1, 4f
    add   a0, a0, t1
    j     1b
3:  li    a0, 0
4:  ret

# ---- IPv4 ----

# ip_header(a0 = protocol, a1 = length after the IP header, a2 = destination IP
# address (4 bytes), a3 = destination MAC address (6 bytes)) -> a0 = the frame's
# length. Fills txbuf's Ethernet header (from this card) and IPv4 header (from my_ip,
# TTL 64, with its checksum).
ip_header:
    addi  sp, sp, -IP_FRAME
    REG_S ra, IP_FRAME-SZREG(sp)
    REG_S s1, IP_FRAME-2*SZREG(sp)
    REG_S s2, IP_FRAME-3*SZREG(sp)
    REG_S s3, IP_FRAME-4*SZREG(sp)
    REG_S s4, IP_FRAME-5*SZREG(sp)
    mv    s1, a0
    mv    s2, a1
    mv    s3, a2
    la    s4, txbuf
    mv    a0, s4                    # Ethernet: destination, source, type
    mv    a1, a3
    li    a2, 6
    call  memcpy
    la    a1, net_mac
    li    a2, 6
    call  memcpy
    addi  a0, s4, 12
    li    a1, 0x0800
    call  put16
    li    t0, 0x45                  # version 4, 5 words of header
    sb    t0, 14(s4)
    sb    zero, 15(s4)              # type of service
    addi  a0, s4, 16
    addi  a1, s2, IP_HDR            # total length
    call  put16
    sb    zero, 18(s4)              # identification, flags, fragment offset
    sb    zero, 19(s4)
    sb    zero, 20(s4)
    sb    zero, 21(s4)
    li    t0, 64
    sb    t0, 22(s4)                # time to live
    sb    s1, 23(s4)                # protocol
    sb    zero, 24(s4)              # checksum, below
    sb    zero, 25(s4)
    addi  a0, s4, 26
    la    a1, my_ip
    li    a2, 4
    call  memcpy
    mv    a1, s3                    # a0 = s4 + 30 now
    li    a2, 4
    call  memcpy
    addi  a0, s4, 14
    li    a1, IP_HDR
    li    a2, 0
    call  csum
    call  csum_fold
    mv    a1, a0
    addi  a0, s4, 24
    call  put16
    addi  a0, s2, L4_OFF
    REG_L s4, IP_FRAME-5*SZREG(sp)
    REG_L s3, IP_FRAME-4*SZREG(sp)
    REG_L s2, IP_FRAME-3*SZREG(sp)
    REG_L s1, IP_FRAME-2*SZREG(sp)
    REG_L ra, IP_FRAME-SZREG(sp)
    addi  sp, sp, IP_FRAME
    ret

# csum(a0 = address, a1 = length, a2 = running sum) -> a0 = the sum plus the bytes
# read as big-endian 16-bit words (an odd last byte as a high half): the Internet
# checksum before folding. Clobbers t0, t1.
csum:
1:  li    t0, 2
    bltu  a1, t0, 2f
    lbu   t0, 0(a0)
    lbu   t1, 1(a0)
    slli  t0, t0, 8
    or    t0, t0, t1
    add   a2, a2, t0
    addi  a0, a0, 2
    addi  a1, a1, -2
    j     1b
2:  beqz  a1, 3f
    lbu   t0, 0(a0)
    slli  t0, t0, 8
    add   a2, a2, t0
3:  mv    a0, a2
    ret

# csum_fold(a0 = sum) -> a0 = the value of the checksum field: the carries added back
# in until it fits in 16 bits, complemented. Clobbers t0, t1.
csum_fold:
    li    t1, 0xffff
1:  srli  t0, a0, 16
    beqz  t0, 2f
    and   a0, a0, t1
    add   a0, a0, t0
    j     1b
2:  not   a0, a0
    and   a0, a0, t1
    ret

# ---- TCP ----

# tcp_client(a0 = port) -> a0 = 0, or 1 if the connection was refused or reset.
# Connects to 10.0.2.2:port, sends the greeting, prints what arrives, and closes
# after the host does.
tcp_client:
    addi  sp, sp, -4*SZREG
    REG_S ra, 3*SZREG(sp)
    REG_S s1, 2*SZREG(sp)
    REG_S s2, SZREG(sp)
    REG_S s3, 0(sp)
    la    t0, tcp_rport
    sh    a0, 0(t0)
    mv    s1, a0
    la    t0, tcp_lport
    li    t1, CLIENT_PORT
    sh    t1, 0(t0)
    la    a0, tcp_ip                # the host, as the guest sees it
    la    a1, host_ip
    li    a2, 4
    call  memcpy
    la    t0, snd_nxt
    li    t1, ISS
    sw    t1, 0(t0)
    la    a0, s_connect             # "connect 10.0.2.2:PORT"
    call  puts
    la    a0, tcp_ip
    call  put_ip
    li    a0, ':'
    call  putc
    mv    a0, s1
    call  put_dec
    call  newline

    li    a0, TCP_SYN               # SYN, then wait for SYN-ACK
    li    a2, 0
    call  tcp_send
1:  call  tcp_recv
    andi  t0, a0, TCP_RST
    bnez  t0, refused
    andi  t0, a0, TCP_SYN
    beqz  t0, 1b
    andi  t0, a0, TCP_ACK
    beqz  t0, 1b
    la    t0, seg_seq
    lw    t1, 0(t0)
    ADDIW t1, t1, 1                 # their SYN takes one sequence number
    la    t0, rcv_nxt
    sw    t1, 0(t0)
    li    a0, TCP_ACK               # connected
    li    a2, 0
    call  tcp_send
    li    a0, TCP_PSH_ACK
    la    a1, s_greeting
    li    a2, GREETING_LEN
    call  tcp_send

2:  call  tcp_take                  # print what comes, until the host's FIN
    andi  t0, a0, TCP_RST
    bnez  t0, reset
    mv    s2, a0
    mv    s3, a2
    beqz  a1, 3f
    mv    s1, a1
    la    a0, s_received
    call  puts
    la    a0, rxdata
    mv    a1, s1
    call  put_text
3:  beqz  s3, 2b                    # nothing to acknowledge
    li    a0, TCP_ACK
    li    a2, 0
    call  tcp_send
    andi  t0, s2, TCP_FIN
    beqz  t0, 2b
    li    a0, TCP_FIN_ACK           # the host closed: so do we
    li    a2, 0
    call  tcp_send
    call  tcp_wait_ack
    bnez  a0, reset
    la    a0, s_closed
    call  puts
    li    a0, 0
    j     tcp_return

refused:
    la    a0, s_refused
    call  puts
    li    a0, 1
    j     tcp_return
reset:
    la    a0, s_reset
    call  puts
    li    a0, 1
tcp_return:                         # the epilogue of tcp_client and tcp_server
    REG_L s3, 0(sp)
    REG_L s2, SZREG(sp)
    REG_L s1, 2*SZREG(sp)
    REG_L ra, 3*SZREG(sp)
    addi  sp, sp, 4*SZREG
    ret

# tcp_server(a0 = port) -> a0 = 0, or 1 if the connection was reset. Waits for a SYN
# to the port, accepts, echoes every byte back, and closes after the peer does.
tcp_server:
    addi  sp, sp, -4*SZREG
    REG_S ra, 3*SZREG(sp)
    REG_S s1, 2*SZREG(sp)
    REG_S s2, SZREG(sp)
    REG_S s3, 0(sp)
    la    t0, tcp_lport
    sh    a0, 0(t0)
    mv    s1, a0
    la    t0, tcp_rport             # from any port
    sh    zero, 0(t0)
    la    a0, s_listen              # "listen PORT"
    call  puts
    mv    a0, s1
    call  put_dec
    call  newline

1:  call  tcp_recv                  # a SYN (and not a SYN-ACK)
    andi  t0, a0, TCP_SYN
    beqz  t0, 1b
    andi  t0, a0, TCP_ACK
    bnez  t0, 1b
    la    t0, seg_sport             # the peer is whoever sent it
    lhu   t1, 0(t0)
    la    t0, tcp_rport
    sh    t1, 0(t0)
    la    a0, tcp_ip
    la    a1, seg_ip
    li    a2, 4
    call  memcpy
    la    t0, seg_seq
    lw    t1, 0(t0)
    ADDIW t1, t1, 1
    la    t0, rcv_nxt
    sw    t1, 0(t0)
    la    a0, s_accepted            # "accepted from 10.0.2.2:PORT"
    call  puts
    la    a0, tcp_ip
    call  put_ip
    li    a0, ':'
    call  putc
    la    t0, tcp_rport
    lhu   a0, 0(t0)
    call  put_dec
    call  newline

2:  la    t0, snd_nxt               # SYN-ACK (again, if the peer sent its SYN again)
    li    t1, ISS
    sw    t1, 0(t0)
    li    a0, TCP_SYN_ACK
    li    a2, 0
    call  tcp_send
3:  call  tcp_take
    andi  t0, a0, TCP_RST
    bnez  t0, reset
    andi  t0, a0, TCP_SYN
    bnez  t0, 2b
    mv    s2, a0
    mv    s3, a1
    beqz  s3, 4f
    li    a0, TCP_PSH_ACK           # echo: the same bytes back, acknowledging them
    la    a1, rxdata
    mv    a2, s3
    call  tcp_send
    la    a0, s_echoed              # "echoed N bytes"
    call  puts
    mv    a0, s3
    call  put_dec
    la    a0, s_bytes
    call  puts
    j     5f
4:  beqz  a2, 3b                    # a bare ACK: nothing to do
    andi  t0, s2, TCP_FIN
    bnez  t0, 5f
    li    a0, TCP_ACK               # old data again: say what we have
    li    a2, 0
    call  tcp_send
    j     3b
5:  andi  t0, s2, TCP_FIN
    beqz  t0, 3b
    li    a0, TCP_FIN_ACK           # the peer closed: so do we
    li    a2, 0
    call  tcp_send
    call  tcp_wait_ack
    bnez  a0, reset
    la    a0, s_closed
    call  puts
    li    a0, 0
    j     tcp_return

# tcp_send(a0 = flags, a1 = data, a2 = length): one segment of the connection
# (tcp_lport to tcp_ip:tcp_rport) with sequence number snd_nxt, acknowledging
# rcv_nxt; then snd_nxt moves past the data and a SYN or FIN. a1 is not read when
# a2 is 0.
tcp_send:
    addi  sp, sp, -32
    REG_S ra, 32-SZREG(sp)
    REG_S s1, 32-2*SZREG(sp)
    REG_S s2, 32-3*SZREG(sp)
    REG_S s3, 32-4*SZREG(sp)
    mv    s1, a0
    mv    s3, a2
    la    a0, txbuf                 # the data, after the headers
    addi  a0, a0, TCP_DATA
    call  memcpy
    la    s2, txbuf
    addi  s2, s2, L4_OFF            # s2 = the TCP header
    la    t0, tcp_lport             # ports
    lhu   a1, 0(t0)
    mv    a0, s2
    call  put16
    la    t0, tcp_rport
    lhu   a1, 0(t0)
    addi  a0, s2, 2
    call  put16
    la    t0, snd_nxt               # sequence and acknowledgment numbers
    lw    a1, 0(t0)
    addi  a0, s2, 4
    call  put32
    la    t0, rcv_nxt
    lw    a1, 0(t0)
    addi  a0, s2, 8
    call  put32
    li    t0, 0x50                  # 5 words of header, no options
    sb    t0, 12(s2)
    sb    s1, 13(s2)                # flags
    addi  a0, s2, 14
    li    a1, WINDOW
    call  put16
    sb    zero, 16(s2)              # checksum (below), urgent pointer
    sb    zero, 17(s2)
    sb    zero, 18(s2)
    sb    zero, 19(s2)
    # The checksum covers a pseudo-header (both addresses, the protocol and the
    # segment's length) and the segment.
    la    a0, my_ip
    li    a1, 4
    li    a2, PROTO_TCP
    call  csum
    mv    a2, a0
    la    a0, tcp_ip
    li    a1, 4
    call  csum
    addi  a2, a0, TCP_HDR
    add   a2, a2, s3
    mv    a0, s2
    addi  a1, s3, TCP_HDR
    call  csum
    call  csum_fold
    mv    a1, a0
    addi  a0, s2, 16
    call  put16
    li    a0, PROTO_TCP             # every frame goes to the gateway
    addi  a1, s3, TCP_HDR
    la    a2, tcp_ip
    la    a3, gw_mac
    call  ip_header
    mv    a1, a0
    la    a0, txbuf
    call  net_send
    bnez  a0, send_failed
    la    t0, snd_nxt               # the sequence numbers used
    lw    t1, 0(t0)
    ADDW  t1, t1, s3
    andi  t2, s1, TCP_FIN
    ADDW  t1, t1, t2
    srli  t2, s1, 1                 # TCP_SYN is bit 1
    andi  t2, t2, 1
    ADDW  t1, t1, t2
    sw    t1, 0(t0)
    REG_L s3, 32-4*SZREG(sp)
    REG_L s2, 32-3*SZREG(sp)
    REG_L s1, 32-2*SZREG(sp)
    REG_L ra, 32-SZREG(sp)
    addi  sp, sp, 32
    ret
send_failed:
    la    a0, s_send_failed
    call  puts
    j     failed

# tcp_recv() -> a0 = the flags of the next TCP segment for the connection: to
# tcp_lport, from tcp_rport (or any port while it is 0). Its sequence and
# acknowledgment numbers, source and payload length go to seg_*, the payload to
# rxdata. Other frames are dropped.
tcp_recv:
    addi  sp, sp, -4*SZREG
    REG_S ra, 3*SZREG(sp)
    REG_S s1, 2*SZREG(sp)
    REG_S s2, SZREG(sp)
    REG_S s3, 0(sp)
1:  call  net_poll
    beqz  a0, 1b
    bnez  a2, 8f
    mv    s1, a0                    # s1 = the frame
    addi  a0, s1, 12
    call  get16
    li    t0, 0x0800
    bne   a0, t0, 8f
    lbu   t0, 23(s1)
    li    t1, PROTO_TCP
    bne   t0, t1, 8f
    lbu   t0, 14(s1)
    andi  t0, t0, 15
    slli  s3, t0, 2                 # s3 = the IP header's length
    add   s2, s1, s3
    addi  s2, s2, ETH_HDR           # s2 = the TCP header
    addi  a0, s1, 16
    call  get16
    sub   s3, a0, s3                # s3 = the segment's length
    addi  a0, s2, 2
    call  get16
    la    t0, tcp_lport
    lhu   t0, 0(t0)
    bne   a0, t0, 8f
    mv    a0, s2
    call  get16
    la    t0, tcp_rport
    lhu   t1, 0(t0)
    beqz  t1, 2f
    bne   a0, t1, 8f
2:  la    t0, seg_sport
    sh    a0, 0(t0)
    la    a0, seg_ip
    addi  a1, s1, 26                # the IP source address
    li    a2, 4
    call  memcpy
    addi  a0, s2, 4
    call  get32
    la    t0, seg_seq
    sw    a0, 0(t0)
    addi  a0, s2, 8
    call  get32
    la    t0, seg_ack
    sw    a0, 0(t0)
    lbu   t0, 13(s2)
    la    t1, seg_flags
    sw    t0, 0(t1)
    lbu   t0, 12(s2)                # data offset, in words
    srli  t0, t0, 4
    slli  t0, t0, 2
    sub   a2, s3, t0                # the payload's length
    bltz  a2, 8f
    li    t1, FRAME_SIZE
    bgeu  a2, t1, 8f
    la    t1, seg_len
    sw    a2, 0(t1)
    add   a1, s2, t0
    la    a0, rxdata
    call  memcpy
    call  net_release
    la    t0, seg_flags
    lw    a0, 0(t0)
    REG_L s3, 0(sp)
    REG_L s2, SZREG(sp)
    REG_L s1, 2*SZREG(sp)
    REG_L ra, 3*SZREG(sp)
    addi  sp, sp, 4*SZREG
    ret
8:  call  net_release
    j     1b

# tcp_take() -> a0 = the next segment's flags, a1 = the length of the new data it
# brought (in rxdata; 0 if none), a2 = 1 if it calls for an acknowledgment (it had
# data or a FIN, new or not). Data or a FIN out of sequence is not taken: a1 = 0,
# and FIN is cleared from a0. rcv_nxt moves past what was taken.
tcp_take:
    addi  sp, sp, -16
    REG_S ra, 16-SZREG(sp)
    call  tcp_recv
    la    t0, seg_len
    lw    t1, 0(t0)
    andi  t0, a0, TCP_FIN
    snez  a2, t1
    or    a2, a2, t0
    li    a1, 0
    andi  t0, a0, TCP_SYN           # a SYN is the caller's business
    bnez  t0, 2f
    la    t0, seg_seq
    lw    t2, 0(t0)
    la    t0, rcv_nxt
    lw    t3, 0(t0)
    bne   t2, t3, 1f
    ADDW  t3, t3, t1                # past the data
    andi  t2, a0, TCP_FIN
    ADDW  t3, t3, t2                # and the FIN
    sw    t3, 0(t0)
    mv    a1, t1
    j     2f
1:  andi  a0, a0, -2                # out of sequence: not a FIN we can take
2:  REG_L ra, 16-SZREG(sp)
    addi  sp, sp, 16
    ret

# tcp_wait_ack() -> a0 = 0 once the peer has acknowledged everything we sent
# (snd_nxt), 1 if it reset the connection instead.
tcp_wait_ack:
    addi  sp, sp, -16
    REG_S ra, 16-SZREG(sp)
1:  call  tcp_recv
    andi  t0, a0, TCP_RST
    bnez  t0, 2f
    andi  t0, a0, TCP_ACK
    beqz  t0, 1b
    la    t0, seg_ack
    lw    t0, 0(t0)
    la    t1, snd_nxt
    lw    t1, 0(t1)
    bne   t0, t1, 1b
    li    a0, 0
    j     3f
2:  li    a0, 1
3:  REG_L ra, 16-SZREG(sp)
    addi  sp, sp, 16
    ret

# ---- Helpers ----

# readline() -> a0 = linebuf: the next line from the UART without its '\n' (a '\r'
# is dropped too), NUL-terminated, at most LINE_MAX - 1 characters kept.
readline:
    la    t2, linebuf
    li    t3, 0                     # characters so far
1:  li    t0, UART_BASE
2:  lbu   t1, UART_LSR(t0)          # wait for a byte
    andi  t1, t1, LSR_DR
    beqz  t1, 2b
    lbu   t1, UART_RBR(t0)
    li    t0, '\n'
    beq   t1, t0, 3f
    li    t0, '\r'
    beq   t1, t0, 1b
    li    t0, LINE_MAX - 1
    bgeu  t3, t0, 1b
    add   t0, t2, t3
    sb    t1, 0(t0)
    addi  t3, t3, 1
    j     1b
3:  add   t0, t2, t3
    sb    zero, 0(t0)
    mv    a0, t2
    ret

# parse_dec(a0 = text) -> a0 = the decimal number after any spaces (0 if none).
parse_dec:
    li    t1, ' '
1:  lbu   t0, 0(a0)
    bne   t0, t1, 2f
    addi  a0, a0, 1
    j     1b
2:  li    t2, 0
    li    t3, 10
3:  lbu   t0, 0(a0)
    addi  t0, t0, -48               # '0'
    bgeu  t0, t3, 4f                # not a digit (below '0' wraps around)
    mul   t2, t2, t3
    add   t2, t2, t0
    addi  a0, a0, 1
    j     3b
4:  mv    a0, t2
    ret

# put_ip(a0 = address of 4 bytes): "10.0.2.15".
put_ip:
    addi  sp, sp, -4*SZREG
    REG_S ra, 3*SZREG(sp)
    REG_S s1, 2*SZREG(sp)
    REG_S s2, SZREG(sp)
    mv    s1, a0
    addi  s2, a0, 4
    j     2f
1:  li    a0, '.'
    call  putc
2:  lbu   a0, 0(s1)
    call  put_dec
    addi  s1, s1, 1
    bne   s1, s2, 1b
    REG_L s2, SZREG(sp)
    REG_L s1, 2*SZREG(sp)
    REG_L ra, 3*SZREG(sp)
    addi  sp, sp, 4*SZREG
    ret

# put16(a0 = address, a1 = value): the low 16 bits, big-endian. Clobbers t0.
put16:
    srli  t0, a1, 8
    sb    t0, 0(a0)
    sb    a1, 1(a0)
    ret

# put32(a0 = address, a1 = value): big-endian. Clobbers t0.
put32:
    srli  t0, a1, 24
    sb    t0, 0(a0)
    srli  t0, a1, 16
    sb    t0, 1(a0)
    srli  t0, a1, 8
    sb    t0, 2(a0)
    sb    a1, 3(a0)
    ret

# get16(a0 = address) -> a0 = the big-endian halfword there. Clobbers t0, t1.
get16:
    lbu   t0, 0(a0)
    lbu   t1, 1(a0)
    slli  t0, t0, 8
    or    a0, t0, t1
    ret

# get32(a0 = address) -> a0 = the big-endian word there. Clobbers t0, t1.
get32:
    lbu   t0, 0(a0)
    lbu   t1, 1(a0)
    slli  t0, t0, 8
    or    t0, t0, t1
    lbu   t1, 2(a0)
    slli  t0, t0, 8
    or    t0, t0, t1
    lbu   t1, 3(a0)
    slli  t0, t0, 8
    or    a0, t0, t1
    ret

# memcpy(a0 = destination, a1 = source, a2 = length) -> a0 = just past the copy.
# Clobbers t0, a1, a2.
memcpy:
    beqz  a2, 2f
1:  lbu   t0, 0(a1)
    sb    t0, 0(a0)
    addi  a0, a0, 1
    addi  a1, a1, 1
    addi  a2, a2, -1
    bnez  a2, 1b
2:  ret

# memzero(a0 = destination, a1 = length). Clobbers a0, a1.
memzero:
    beqz  a1, 2f
1:  sb    zero, 0(a0)
    addi  a0, a0, 1
    addi  a1, a1, -1
    bnez  a1, 1b
2:  ret

    .section .rodata
host_ip:        .byte 10, 0, 2, 2   # the host, through the gateway
ip_broadcast:   .byte 255, 255, 255, 255
mac_broadcast:  .byte 0xff, 0xff, 0xff, 0xff, 0xff, 0xff
s_greeting:     .ascii "Hello from the guest!\n"
s_no_card:      .asciz "no network card (try grail --net user)\n"
s_dhcp:         .asciz "dhcp: address "
s_server:       .asciz ", server "
s_nak:          .asciz "dhcp: refused\n"
s_connect:      .asciz "connect "
s_listen:       .asciz "listen "
s_accepted:     .asciz "accepted from "
s_received:     .asciz "received: "
s_echoed:       .asciz "echoed "
s_bytes:        .asciz " bytes\n"
s_closed:       .asciz "closed\n"
s_refused:      .asciz "connection refused\n"
s_reset:        .asciz "connection reset\n"
s_send_failed:  .asciz "send failed\n"

    .section .bss
    .balign 4
txbuf:      .space FRAME_SIZE       # the frame being built
rxdata:     .space FRAME_SIZE       # the payload of the last TCP segment
linebuf:    .space LINE_MAX
my_ip:      .space 4                # our address, from DHCP (0.0.0.0 until then)
offered:    .space 4                # the address the server offered
server_id:  .space 4                # the DHCP server's identifier
tcp_ip:     .space 4                # the other end of the connection
seg_ip:     .space 4                # the source of the last segment
snd_nxt:    .space 4                # our next sequence number
rcv_nxt:    .space 4                # the next one we expect
seg_seq:    .space 4                # the last segment's sequence number,
seg_ack:    .space 4                # acknowledgment number,
seg_flags:  .space 4                # flags
seg_len:    .space 4                # and payload length
tcp_lport:  .space 2                # our port
tcp_rport:  .space 2                # theirs (0: any, while listening)
seg_sport:  .space 2                # the last segment's source port
gw_mac:     .space 6                # the gateway's MAC address
