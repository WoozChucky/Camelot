# The network card driver and console helpers shared by examples/net/ping.S, pong.S
# and user.S (spec section 9.8). Linked into each of them.
#
# The card keeps two rings of descriptors in RAM. On RV32 a descriptor is 8 bytes,
# { u32 buffer address, u16 length, u16 status }; on RV64, whose card has the
# registers of version 2 of its interface, 16 bytes, { u64 buffer address, u16
# length, u16 status, u32 reserved }, naturally aligned (a ring's base is a multiple
# of 16). Each ring has a BASE (64-bit on RV64, written with REG_S, sd there), a SIZE
# (a power of two) and two free-running 32-bit counters: the descriptors from HEAD up
# to (not including) TAIL belong to the card, and counter n is slot n % SIZE. The
# driver hands descriptors over by advancing TAIL, the card hands them back by
# advancing HEAD.
#
#   transmit: fill the descriptor at TX_TAIL, then TX_TAIL += 1; the card sends it
#             during that store, writes its status and moves TX_HEAD past it.
#   receive:  give the card empty buffers by advancing RX_TAIL; the card fills the
#             one at RX_HEAD with the next frame, writes the frame's length and a
#             status into its descriptor, and moves RX_HEAD past it. When no buffer
#             is free, the frame is dropped and counted in NET_RX_DROPPED.
#
# On RV64 a counter is a 32-bit value that lw sign-extends: one the driver computes
# and compares with the card's register is kept in that form with addiw (ADDIW), so
# that the two are equal across 2^31; one it only stores with sw (the low 32 bits)
# and reads back with lw may be computed with addi. The card's address and the
# buffers' are XLEN-wide (REG_L, REG_S), and the functions save ra and the s
# registers with REG_S, in frames of SZREG-sized slots.

#include <asm.h>

#if __riscv_xlen == 64
#define LWU lwu         /* a 32-bit register, zero-extended */
#define ADDIW addiw     /* a 32-bit counter, kept sign-extended */
#define DEC_END 20      /* put_dec's digits: sp+0 .. sp+19, the 20 of 2^64 - 1 */
#else
#define LWU lw
#define ADDIW addi
#define DEC_END 12      /* sp+2 .. sp+11, the 10 of 2^32 - 1 */
#endif

    .equ UART_BASE,       0x10000000
    .equ UART_LSR,        5
    .equ LSR_THRE,        0x20

    .equ SYSINFO_BASE,    0x30002000
#if __riscv_xlen == 64
    .equ SYS_DEV_COUNT,   0x24
    .equ SYS_DEV_TABLE,   0x40
    .equ DEV_ENTRY_SIZE,  24            # type, irq, base (64-bit), size (64-bit)
    .equ DEV_TYPE,        0x00
    .equ DEV_IRQ,         0x04
    .equ DEV_BASE,        0x08
#else
    .equ SYS_DEV_COUNT,   0x1c
    .equ SYS_DEV_TABLE,   0x40
    .equ DEV_ENTRY_SIZE,  16            # type, base, size, irq
#endif
    .equ DEV_NET,         11            # device type code of a network card

    .equ NET_MAC_LO,      0x00          # MAC bytes 0-3 (byte 0 in bits 7:0)
    .equ NET_MAC_HI,      0x04          # MAC bytes 4-5
    .equ NET_STATUS,      0x08          # bit 0: link up
    .equ NET_IRQ_ENABLE,  0x10
    .equ NET_IRQ_STATUS,  0x14          # write 1s to clear
#if __riscv_xlen == 64
    .equ NET_TX_BASE,     0x20          # 64-bit
    .equ NET_TX_SIZE,     0x28
    .equ NET_TX_HEAD,     0x2c
    .equ NET_TX_TAIL,     0x30
    .equ NET_RX_BASE,     0x38          # 64-bit
    .equ NET_RX_SIZE,     0x40
    .equ NET_RX_HEAD,     0x44
    .equ NET_RX_TAIL,     0x48
#else
    .equ NET_TX_BASE,     0x20
    .equ NET_TX_SIZE,     0x24
    .equ NET_TX_HEAD,     0x28
    .equ NET_TX_TAIL,     0x2c
    .equ NET_RX_BASE,     0x30
    .equ NET_RX_SIZE,     0x34
    .equ NET_RX_HEAD,     0x38
    .equ NET_RX_TAIL,     0x3c
#endif
    .equ IRQ_RX,          1
    .equ IRQ_ALL,         7             # RX, TX, ERROR

    .equ RING_SIZE,       4             # descriptors per ring
    .equ RING_MASK,       3
#if __riscv_xlen == 64
    .equ DESC_SIZE,       16
    .equ DESC_SHIFT,      4             # log2(DESC_SIZE)
    .equ DESC_ADDR,       0             # u64
    .equ DESC_LEN,        8             # u16
    .equ DESC_STATUS,     10            # u16
#else
    .equ DESC_SIZE,       8
/* The rest of the layout as macros, not symbols: RV32's program had the numbers
   written out, and its executable (and symbol table) stays what it was. */
#define DESC_SHIFT 3
#define DESC_ADDR 0
#define DESC_LEN 4
#define DESC_STATUS 6
#endif
    .equ BUF_SIZE,        1536          # room for the longest frame, 1514 bytes
    .equ ETHERTYPE,       0x88b5        # IEEE "local experimental" EtherType
    .equ MIN_FRAME,       60            # what Ethernet pads short frames to

    .equ PLIC_BASE,       0x0c000000    # + 4 * source: priority
    .equ PLIC_ENABLE0,    0x0c002000    # context 0 = hart 0 M-mode
    .equ PLIC_CLAIM0,     0x0c200004
    .equ MIE_MEIE,        0x800         # mie bit 11
    .equ MIP_MEIP,        0x800         # mip bit 11

    .text

# net_init() -> a0 = 0, or 1 if the machine has no network card. Finds the first
# card in the sysinfo device table, prints "net BASE, irq N, mac ADDR, link up",
# and sets up both rings, every receive buffer given to the card. It also routes
# the card's receive interrupt to mip.MEIP (PLIC context 0, mie.MEIE; mstatus.MIE
# stays off, so no trap handler is needed): net_wait waits for it.
    .globl net_init
net_init:
    addi  sp, sp, -16
    REG_S ra, 16-SZREG(sp)
    REG_S s1, 16-2*SZREG(sp)

    li    t0, SYSINFO_BASE
    LWU   t1, SYS_DEV_COUNT(t0)
    addi  t0, t0, SYS_DEV_TABLE
    li    t2, DEV_NET
1:  beqz  t1, no_card
#if __riscv_xlen == 64
    lwu   t3, DEV_TYPE(t0)
#else
    lw    t3, 0(t0)
#endif
    beq   t3, t2, 2f
    addi  t0, t0, DEV_ENTRY_SIZE
    addi  t1, t1, -1
    j     1b
#if __riscv_xlen == 64
2:  ld    s1, DEV_BASE(t0)          # s1 = the card's registers
    lwu   t1, DEV_IRQ(t0)
#else
2:  lw    s1, 4(t0)                 # s1 = the card's registers
    lw    t1, 12(t0)
#endif
    la    t2, net_base
    REG_S s1, 0(t2)
    la    t2, net_irq
    sw    t1, 0(t2)
    la    t2, net_mac               # the MAC, in wire order
    lw    t3, NET_MAC_LO(s1)        # sw stores its 32 bits as they are
    sw    t3, 0(t2)
    lw    t3, NET_MAC_HI(s1)
    sw    t3, 4(t2)

    la    a0, s_net                 # "net 0x10009000, irq 12, mac ..., link up"
    call  puts
    mv    a0, s1
    call  put_hex
    la    a0, s_irq
    call  puts
    la    a0, net_irq
    lw    a0, 0(a0)
    call  put_dec
    la    a0, s_mac
    call  puts
    la    a0, net_mac
    call  put_mac
    lw    t0, NET_STATUS(s1)
    la    a0, s_link_up
    andi  t0, t0, 1
    bnez  t0, 3f
    la    a0, s_link_down
3:  call  puts

    # Transmit ring: empty (TX_HEAD = TX_TAIL = 0 after writing BASE and SIZE).
    la    t0, tx_ring
    REG_S t0, NET_TX_BASE(s1)
    li    t0, RING_SIZE
    sw    t0, NET_TX_SIZE(s1)

    # Receive ring: one buffer per descriptor, all handed to the card.
    la    t0, rx_ring
    REG_S t0, NET_RX_BASE(s1)
    li    t1, RING_SIZE
    sw    t1, NET_RX_SIZE(s1)
    la    t2, rx_bufs
    li    t3, BUF_SIZE
4:  REG_S t2, DESC_ADDR(t0)         # address
    sh    t3, DESC_LEN(t0)          # length: the buffer's size
    sh    zero, DESC_STATUS(t0)
    add   t2, t2, t3
    addi  t0, t0, DESC_SIZE
    addi  t1, t1, -1
    bnez  t1, 4b
    li    t0, RING_SIZE
    sw    t0, NET_RX_TAIL(s1)

    # The receive interrupt: priority 1, enabled for context 0, mie.MEIE on.
    la    t0, net_irq
    lw    t0, 0(t0)
    li    t1, PLIC_BASE
    slli  t2, t0, 2
    add   t1, t1, t2
    li    t2, 1
    sw    t2, 0(t1)
    sll   t2, t2, t0
    li    t1, PLIC_ENABLE0
    sw    t2, 0(t1)
    li    t0, MIE_MEIE
    csrs  mie, t0
    li    t0, IRQ_RX
    sw    t0, NET_IRQ_ENABLE(s1)

    li    a0, 0
    j     5f
no_card:
    li    a0, 1
5:  REG_L s1, 16-2*SZREG(sp)
    REG_L ra, 16-SZREG(sp)
    addi  sp, sp, 16
    ret

# net_send(a0 = frame, a1 = length) -> a0 = the descriptor's status (0 = sent).
# Clobbers t0-t3.
    .globl net_send
net_send:
    la    t0, net_base
    REG_L t0, 0(t0)
    lw    t1, NET_TX_TAIL(t0)
    andi  t2, t1, RING_MASK         # the slot of counter TX_TAIL
    slli  t2, t2, DESC_SHIFT
    la    t3, tx_ring
    add   t2, t2, t3
    REG_S a0, DESC_ADDR(t2)
    sh    a1, DESC_LEN(t2)
    sh    zero, DESC_STATUS(t2)
    ADDIW t1, t1, 1                 # sign-extended (RV64), as lw reads TX_HEAD
    sw    t1, NET_TX_TAIL(t0)       # the doorbell: the card sends it now
1:  lw    t3, NET_TX_HEAD(t0)       # done once HEAD has moved past it
    bne   t3, t1, 1b
    lhu   a0, DESC_STATUS(t2)
    ret

# net_poll() -> a0 = the next received frame's buffer, or 0 if none has arrived;
# a1 = its length, a2 = its descriptor's status (0 = ok). Pass the buffer back to
# the card with net_release when done with it. Clobbers t0-t3.
    .globl net_poll
net_poll:
    la    t0, net_base
    REG_L t0, 0(t0)
    la    t1, rx_next
    lw    t1, 0(t1)
    lw    t2, NET_RX_HEAD(t0)
    li    a0, 0
    beq   t1, t2, 1f                # the card has filled nothing new
    andi  t2, t1, RING_MASK
    slli  t2, t2, DESC_SHIFT
    la    t3, rx_ring
    add   t2, t2, t3
    REG_L a0, DESC_ADDR(t2)
    lhu   a1, DESC_LEN(t2)
    lhu   a2, DESC_STATUS(t2)
1:  ret

# net_release(): gives the buffer net_poll returned back to the card. The card wrote
# the frame's length over the buffer's, so it is set again first. Clobbers t0-t3.
    .globl net_release
net_release:
    la    t1, rx_next
    lw    t0, 0(t1)
    addi  t2, t0, 1
    sw    t2, 0(t1)
    andi  t0, t0, RING_MASK
    slli  t0, t0, DESC_SHIFT
    la    t1, rx_ring
    add   t0, t0, t1
    li    t1, BUF_SIZE
    sh    t1, DESC_LEN(t0)
    la    t0, net_base
    REG_L t0, 0(t0)
    lw    t1, NET_RX_TAIL(t0)
    addi  t1, t1, 1
    sw    t1, NET_RX_TAIL(t0)
    ret

# net_wait(): sleeps in wfi until the card raises its receive interrupt, then
# claims it, acknowledges it at the card (which lowers the line) and completes it.
# Returns a0 = 0, or 1 if the PLIC delivered another source. Clobbers t0-t3.
    .globl net_wait
net_wait:
    li    t1, MIP_MEIP
1:  csrr  t0, mip
    and   t0, t0, t1
    bnez  t0, 2f
    wfi
    j     1b
2:  li    t0, PLIC_CLAIM0
    lw    t1, 0(t0)                 # claim
    la    t2, net_irq
    lw    t2, 0(t2)
    li    a0, 1
    bne   t1, t2, 3f
    la    t2, net_base
    REG_L t2, 0(t2)
    li    t3, IRQ_ALL
    sw    t3, NET_IRQ_STATUS(t2)    # acknowledge
    li    a0, 0
3:  sw    t1, 0(t0)                 # complete
    ret

# make_frame(a0 = buffer, a1 = destination MAC, a2 = NUL-terminated text) -> a0 =
# its length. An Ethernet II frame from this card's MAC with EtherType 0x88b5 and
# the text (with its NUL) as payload, zero-padded to 60 bytes. Clobbers t0-t3.
    .globl make_frame
make_frame:
    mv    t0, a0
    li    t1, 6
1:  lbu   t2, 0(a1)                 # destination
    sb    t2, 0(t0)
    addi  a1, a1, 1
    addi  t0, t0, 1
    addi  t1, t1, -1
    bnez  t1, 1b
    la    t3, net_mac
    li    t1, 6
2:  lbu   t2, 0(t3)                 # source: this card
    sb    t2, 0(t0)
    addi  t3, t3, 1
    addi  t0, t0, 1
    addi  t1, t1, -1
    bnez  t1, 2b
    li    t2, ETHERTYPE >> 8        # EtherType, big-endian
    sb    t2, 0(t0)
    li    t2, ETHERTYPE & 0xff
    sb    t2, 1(t0)
    addi  t0, t0, 2
3:  lbu   t2, 0(a2)                 # payload
    sb    t2, 0(t0)
    addi  a2, a2, 1
    addi  t0, t0, 1
    bnez  t2, 3b
    addi  t1, a0, MIN_FRAME
4:  bgeu  t0, t1, 5f                # padding
    sb    zero, 0(t0)
    addi  t0, t0, 1
    j     4b
5:  sub   a0, t0, a0
    ret

# print_frame(a0 = frame, a1 = length): "received 60 bytes from ADDR, type 0x88b5:
# TEXT", TEXT being the payload up to its first NUL.
    .globl print_frame
print_frame:
    addi  sp, sp, -4*SZREG
    REG_S ra, 3*SZREG(sp)
    REG_S s1, 2*SZREG(sp)
    REG_S s2, SZREG(sp)
    mv    s1, a0
    mv    s2, a1
    la    a0, s_received
    call  puts
    mv    a0, s2
    call  put_dec
    la    a0, s_bytes_from
    call  puts
    addi  a0, s1, 6
    call  put_mac
    la    a0, s_type
    call  puts
    lbu   a0, 12(s1)
    call  put_byte
    lbu   a0, 13(s1)
    call  put_byte
    la    a0, s_colon
    call  puts
    addi  a0, s1, 14
    addi  a1, s2, -14
    call  put_text
    call  newline
    REG_L s2, SZREG(sp)
    REG_L s1, 2*SZREG(sp)
    REG_L ra, 3*SZREG(sp)
    addi  sp, sp, 4*SZREG
    ret

# print_sent(a0 = length, a1 = destination MAC): "sent 60 bytes to ADDR".
    .globl print_sent
print_sent:
    addi  sp, sp, -4*SZREG
    REG_S ra, 3*SZREG(sp)
    REG_S s1, 2*SZREG(sp)
    REG_S s2, SZREG(sp)
    mv    s1, a0
    mv    s2, a1
    la    a0, s_sent
    call  puts
    mv    a0, s1
    call  put_dec
    la    a0, s_bytes_to
    call  puts
    mv    a0, s2
    call  put_mac
    call  newline
    REG_L s2, SZREG(sp)
    REG_L s1, 2*SZREG(sp)
    REG_L ra, 3*SZREG(sp)
    addi  sp, sp, 4*SZREG
    ret

# putc(a0 = byte). Clobbers t0, t1 only.
    .globl putc
putc:
    li    t0, UART_BASE
1:  lbu   t1, UART_LSR(t0)          # wait until the transmitter is free
    andi  t1, t1, LSR_THRE
    beqz  t1, 1b
    sb    a0, 0(t0)
    ret

    .globl newline
newline:
    li    a0, '\n'
    j     putc

# puts(a0 = NUL-terminated string)
    .globl puts
puts:
    addi  sp, sp, -16
    REG_S ra, 16-SZREG(sp)
    mv    a1, a0
1:  lbu   a0, 0(a1)
    beqz  a0, 2f
    call  putc
    addi  a1, a1, 1
    j     1b
2:  REG_L ra, 16-SZREG(sp)
    addi  sp, sp, 16
    ret

# put_text(a0 = buffer, a1 = length): the bytes up to the first NUL or the end.
    .globl put_text
put_text:
    addi  sp, sp, -16
    REG_S ra, 16-SZREG(sp)
    mv    a2, a0
    add   a3, a0, a1
1:  bgeu  a2, a3, 2f
    lbu   a0, 0(a2)
    beqz  a0, 2f
    call  putc
    addi  a2, a2, 1
    j     1b
2:  REG_L ra, 16-SZREG(sp)
    addi  sp, sp, 16
    ret

# put_dec(a0 = unsigned value): decimal, no padding, up to the 10 digits of
# 2^32 - 1, or the 20 of 2^64 - 1 on RV64.
    .globl put_dec
put_dec:
    addi  sp, sp, -32
    REG_S ra, 32-SZREG(sp)
    addi  a1, sp, DEC_END           # digits go backwards into the bytes below sp+DEC_END
    li    a2, 10
1:  remu  a3, a0, a2
    divu  a0, a0, a2
    addi  a3, a3, '0'
    addi  a1, a1, -1
    sb    a3, 0(a1)
    bnez  a0, 1b
    addi  a2, sp, DEC_END
2:  lbu   a0, 0(a1)
    call  putc
    addi  a1, a1, 1
    bne   a1, a2, 2b
    REG_L ra, 32-SZREG(sp)
    addi  sp, sp, 32
    ret

# put_hex(a0 = value): "0x" and XLEN/4 lowercase hex digits (8, or 16 on RV64).
put_hex:
    addi  sp, sp, -16
    REG_S ra, 16-SZREG(sp)
    mv    a1, a0
    li    a0, '0'
    call  putc
    li    a0, 'x'
    call  putc
    li    a2, __riscv_xlen - 4      # shift of the next digit
1:  srl   a0, a1, a2
    call  put_digit
    addi  a2, a2, -4
    bgez  a2, 1b
    REG_L ra, 16-SZREG(sp)
    addi  sp, sp, 16
    ret

# put_byte(a0 = byte): two lowercase hex digits. Clobbers a0, a3, t0, t1.
put_byte:
    addi  sp, sp, -16
    REG_S ra, 16-SZREG(sp)
    REG_S a0, 16-2*SZREG(sp)
    srli  a0, a0, 4
    call  put_digit
    REG_L a0, 16-2*SZREG(sp)
    call  put_digit
    REG_L ra, 16-SZREG(sp)
    addi  sp, sp, 16
    ret

# put_digit(a0): the hex digit of a0's low 4 bits. Clobbers a0, a3, t0, t1.
put_digit:
    andi  a0, a0, 15
    li    a3, 10
    blt   a0, a3, 1f
    addi  a0, a0, 'a' - 10 - '0'
1:  addi  a0, a0, '0'
    j     putc

# put_mac(a0 = address of 6 bytes): "02:ca:00:00:00:01".
    .globl put_mac
put_mac:
    addi  sp, sp, -4*SZREG
    REG_S ra, 3*SZREG(sp)
    REG_S s1, 2*SZREG(sp)
    REG_S s2, SZREG(sp)
    mv    s1, a0
    addi  s2, a0, 6
    j     2f
1:  li    a0, ':'
    call  putc
2:  lbu   a0, 0(s1)
    call  put_byte
    addi  s1, s1, 1
    bne   s1, s2, 1b
    REG_L s2, SZREG(sp)
    REG_L s1, 2*SZREG(sp)
    REG_L ra, 3*SZREG(sp)
    addi  sp, sp, 4*SZREG
    ret

    .section .rodata
s_net:          .asciz "net "
s_irq:          .asciz ", irq "
s_mac:          .asciz ", mac "
s_link_up:      .asciz ", link up\n"
s_link_down:    .asciz ", link down\n"
s_received:     .asciz "received "
s_bytes_from:   .asciz " bytes from "
s_type:         .asciz ", type 0x"
s_colon:        .asciz ": "
s_sent:         .asciz "sent "
s_bytes_to:     .asciz " bytes to "

    .section .bss
#if __riscv_xlen == 64
    .balign 16                      # what the card requires of a ring's base
#else
    .balign 8
#endif
tx_ring:    .space RING_SIZE * DESC_SIZE
rx_ring:    .space RING_SIZE * DESC_SIZE
rx_bufs:    .space RING_SIZE * BUF_SIZE
    .balign SZREG
net_base:   .space SZREG            # the card's registers
net_irq:    .space 4                # its PLIC source
rx_next:    .space 4                # counter of the next receive descriptor to read
    .globl net_mac
net_mac:    .space 8                # its MAC address (6 bytes, then 2 of padding)
