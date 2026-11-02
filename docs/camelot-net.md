# Camelot's network stack

The Camelot kernel's networking, as far as issues #83 and #84 take it: a
driver for Grail's network card (`docs/excalibur-spec.md` section 9.8),
Ethernet, ARP, IPv4, ICMP echo, UDP and TCP, an address from DHCP or a static
configuration, and two shell commands, `ifconfig` and `ping`. The socket
system calls and network tools (#85) build on it; until then a temporary
system call, `tcptest`, drives TCP from a program
([Testing TCP until #85](#testing-tcp-until-85)).

The code is in `kernel/net/`, one file per layer, with the whole interface in
`kernel/include/net.h` and the driver's in `kernel/include/nic.h`:

| File | What it does |
|---|---|
| `nic.c` | the card: descriptor rings, the interrupt, the transmit queue |
| `pbuf.c` | the packet buffer pool |
| `ether.c` | Ethernet II: filtering by address, dispatch by EtherType |
| `arp.c` | the ARP cache: requests, replies, timeouts, the packet waiting for an answer |
| `ip.c` | IPv4: header checks, checksums, routing, loopback |
| `icmp.c` | echo requests and replies |
| `udp.c` | UDP: ports, checksums, the kernel-internal socket API, the echo service |
| `tcp.c` | TCP: the connection state machine, retransmission, the kernel-internal socket API |
| `dhcp.c` | the DHCP client |
| `net.c` | the interface's configuration, bringing it up at boot, the counters, the timer, waiting, the system calls (and `tcptest`) |

It is a small host stack in the spirit of xv6's lab network code and lwIP's
raw API: one interface, no forwarding, no fragmentation, no options, and every
limit a constant in `net.h`.

## Execution model

Kernel code runs with interrupts off and on one hart (`kernel/proc.c`), and
the stack relies on it: there are no locks. A received frame is handled from
start to end inside the card's interrupt: the driver passes it to
`ether_input()`, which passes it to `arp_input()` or `ip_input()`, and so on
up; an answer (an ARP reply, an echo reply, a UDP echo) is sent from there
too. Sending goes down the same way from whoever sends, a process in a
system call or the stack itself.

A process that waits for the network (`ping`, a UDP socket, DHCP) calls
`net_sleep()`, which sleeps on the interface; `net_wakeup()` wakes every such
process when something arrives for one of them, and `net_timer()`, called on
every timer tick, wakes them too, so each checks its own condition and
timeout in a loop around the sleep, as every sleep in the kernel does.
Timeouts are in ticks (10 a second).

Addresses are `u32` in host order (10.0.2.15 is `0x0a00020f`) and ports
`u16`; packets hold them big-endian, and `get16`/`get32`/`put16`/`put32`
convert.

## Buffers

Every frame lives in a packet buffer (`struct pbuf`): 2048 bytes
(`PBUF_SIZE`, more than the largest frame, 1514 bytes, and two to a page),
its length, and a link for the queues it waits in. The pool holds 64
(`NPBUF`), on 32 pages taken from `kalloc` when a card is found (none
without one). A frame stays in its buffer from the card's DMA to whoever
consumes it, a socket or the card that sends it, so nothing is copied between
layers, and each layer that sends writes its header at a fixed offset (the
shape is always Ethernet, an IPv4 header without options, then ICMP, UDP or
TCP).

The pool is fixed, so the network can never eat the rest of memory. When it
is empty, a received frame is dropped (its buffer goes straight back to the
card) and sending fails with `-ENOBUFS`; both count in `no-buffer`. Of the 64,
16 always belong to the card's receive ring.

## The card

`nic_init()` walks the sysinfo device table (spec section 9.5) for entries of
type 11 (`DEV_NET`) and prints a line for each:

```
net 0: mac 52:54:00:12:34:56, irq 12, link up
```

The first card is the interface; the stack has one, and any other is listed
with `, not used`. A card without an interrupt (source 0, or above 31) is
not used either: the driver doesn't poll. Without a card nothing is printed,
no memory is taken and the kernel boots as before.

The card moves frames through two rings of 8-byte descriptors (`struct
nic_desc`: buffer address, length, status), one page for both: 16 receive
and 16 transmit descriptors (`NIC_RX_RING`, `NIC_TX_RING`). Each ring has two
free-running counters, the card's HEAD and the driver's TAIL.

- **Receive.** Every receive descriptor holds an empty buffer. On the
  interrupt the driver walks from its own counter to `RX_HEAD`: each good
  frame's buffer goes up the stack and a fresh one from the pool takes its
  place, then the descriptor goes back to the card (`RX_TAIL` moves on). With
  the pool empty, the frame is dropped and its own buffer re-armed, so the
  ring never shrinks. A descriptor with an error status holds no frame (the
  card counted it in `RX_DROPPED`).
- **Transmit.** A frame goes in the next free descriptor and `TX_TAIL` moves
  past it. Its buffer is freed once the card has handed the descriptor back
  (`TX_HEAD` passed it); a status other than OK counts in `tx-error`. While all
  16 are in flight, frames wait in a software queue of at most 32
  (`NIC_TXQ_MAX`) and go out as descriptors free up; one more is dropped and
  counted in `tx-error` (`-ENOBUFS`). Grail sends during the `TX_TAIL` store,
  so the queue only fills with a card that takes its time.
- **Interrupt.** `nic_intr()` acknowledges `IRQ_STATUS` first (a frame that
  lands while the ring is read raises it again rather than being missed),
  reports and clears a ring error (a bug, since the rings never change),
  receives, then reclaims sent descriptors. The PLIC source is enabled only
  once the interface is set up (`nic_start()`, from `net_init()`).

The card's own drops (frames shorter than 14 or longer than 1514 bytes, or
with no free descriptor) are its `RX_DROPPED` register, shown as `card`.

## Ethernet

Ethernet II: destination and source MAC, EtherType, payload; no FCS and no
padding to 60 bytes, which Grail's cards neither need nor check. A hub
repeats every frame to every port, so `ether_input()` first does what a real
card does in hardware: frames not to the interface's MAC or to broadcast are
dropped (`filtered`). IPv4 (`0x0800`) and ARP (`0x0806`) go up; any other
EtherType, or a frame shorter than the header, counts in `ether`.

## ARP

RFC 826 for Ethernet and IPv4: 28-byte packets, requests broadcast, replies
to the asker. The cache has 16 entries (`ARP_ENTRIES`), each:

- **resolved**: good for 60 s (`ARP_TIMEOUT`), then forgotten;
- **pending**: a request is out, repeated every second (`ARP_RETRY`), three
  times in all (`ARP_TRIES`). One packet waits in the entry for the answer;
  a newer one replaces it, and the replaced one counts in `arp-failed`;
- **failed**: three requests went unanswered: the waiting packet is dropped
  (`arp-failed`), and for 3 s (`ARP_HOLD`) sending to the address fails at
  once with `-EHOSTUNREACH` (also `arp-failed`) rather than asking again.

When the cache is full, a new address takes the entry used longest ago.
Received packets follow RFC 826's merge rule: an entry for the sender is
updated, and the target of a request adds one for the sender (it is about to
answer), so a host that is pinged answers without asking. A request for the
interface's address is answered. Nothing is learned from a probe (sender
0.0.0.0), a group MAC address, a packet claiming the interface's own
address, or while the interface has no address (`filtered`). Packets that are
not Ethernet/IPv4, are shorter than 28 bytes or have an operation other than
1 or 2 count in `arp`. Changing the interface's address empties the cache.

## IPv4

RFC 791, without options on the way out, fragmentation or forwarding.

**Input** checks everything a packet says about itself before believing it,
in this order:

1. at least 20 bytes, version 4, a header length (IHL) of 5 to 15 words that
   fits the frame, a total length no shorter than the header and no longer
   than the frame: otherwise `ip`. Bytes past the total length are padding;
2. the header checksum: `ip-checksum`;
3. not a fragment (MF set, or an offset): `fragment`. Reassembly is not
   supported, and nothing this stack sends needs it;
4. from the wire, a source address a host can have (RFC 1122 3.2.1.3): not
   255.255.255.255 or the subnet's broadcast address, not multicast or
   reserved (224.0.0.0 and up), not 127.0.0.0/8, not the interface's own
   address; 0.0.0.0 is allowed (a host with no address yet), though nothing
   answers it. Otherwise `ip`: answering a broadcast source would send the
   answer to the whole segment;
5. for this host: the interface's address, 255.255.255.255 or the subnet's
   broadcast address; while the interface has no address, any UDP (a DHCP
   offer may come to the address offered). 127.0.0.0/8 never comes from the
   wire. Otherwise `filtered`;
6. ICMP (1), TCP (6) or UDP (17): otherwise `protocol`.

**Output** writes a 20-byte header: TTL 64 (`IP_TTL`), DF set, an
identification counter, the checksum. A packet over 1500 bytes (`NET_MTU`)
is refused with `-EMSGSIZE`: nothing is fragmented. Routing is a host's:

- 127.0.0.0/8 and the interface's own address loop back (below);
- 255.255.255.255 and the subnet's broadcast address go to the Ethernet
  broadcast address, without ARP;
- an address on the subnet goes to its own MAC, through ARP;
- anything else to the default gateway's MAC, or fails with `-ENETUNREACH`
  without a gateway (so does everything but a broadcast while the interface
  has no address, and multicast).

**Loopback.** A packet to this host never touches the card: it goes to a
queue that `ip_input()` drains, so an echo request to 127.0.0.1 answered
from inside its own sending is a loop, not a recursion. Packets to
127.0.0.0/8 come from 127.0.0.1. `ifconfig` shows loopback as `lo`.

## ICMP

RFC 792's echo: type 8 (request) and 0 (reply), code 0, a checksum over the
whole message, an identifier, a sequence number, and data sent back
unchanged. Every echo request to one of the host's own addresses is
answered, but not those to a broadcast address (everyone on the network
would answer at once). Other ICMP types are taken and ignored. A wrong
checksum, or fewer than 8 bytes, counts in `icmp`.

The sending side is `icmp_ping(dst, id, seq, timeout, reply)`: an echo
request with 56 bytes of data (`PING_DATA`, 64 bytes of ICMP) and the
process waits in a slot (one per process) for the reply with the same
identifier and sequence number. The `ping` system call uses the caller's pid
as the identifier, so two processes' pings never mix, and a one-second
timeout. The round-trip time comes from the CLINT's `mtime` and is reported
in microseconds.

## UDP

RFC 768. Input checks the length field (at least 8, no more than the
packet: `udp`) and the checksum over the pseudo-header and the datagram
(`udp-checksum`; a checksum of 0 means the sender computed none, and is
accepted). Output always computes one, and sends 0xffff for a sum of 0.

The kernel-internal socket API, which DHCP uses now and #85's socket system
calls will put behind file descriptors:

| Function | |
|---|---|
| `udp_open(port, &err)` | a socket bound to `port`, or with 0 the next free port from 49152 up; `-EADDRINUSE`, or `-ENOBUFS` when all 8 (`UDP_SOCKETS`) are taken |
| `udp_close(s)` | unbinds, dropping what was not read |
| `udp_sendto(s, dst, port, buf, len)` | sends from the socket's port; `len`, or `-EMSGSIZE` over 1472 bytes, or the routing errors |
| `udp_recvfrom(s, buf, size, &from, &port, timeout)` | the oldest datagram (truncated to `size`, the rest lost), its sender, its length; sleeps up to `timeout` ticks (0: forever), then `-ETIMEDOUT` |

A socket is bound to a port on every address of the host. Datagrams wait on
it in their buffers, at most 4 (`UDP_QUEUE_MAX`); more are dropped
(`queue-full`). A datagram to a port with no socket counts in `no-port`,
except for the echo service (RFC 862): on port 7 with no socket bound, a
datagram comes back to its sender, from the address it was sent to, unless
it came from a port below 1024 (two echo services would bounce it forever)
or to a broadcast address (everyone would answer).

## TCP

RFC 9293, in `tcp.c`, with RFC 5961's defenses, RFC 6298's retransmission
timer and RFC 6528's initial sequence numbers. It is a small TCP: correct on
a network that loses, duplicates and reorders segments, but with none of the
machinery that only buys speed.

**Connections.** 16 sockets (`TCP_SOCKETS`), each a listener or one
connection, in a static table. A connection has two 4 KiB buffers
(`TCP_BUF`, a page each from `kalloc`, taken when it starts and given back
when it ends): what was sent stays in the send buffer until it is
acknowledged, what arrived in order stays in the receive buffer until it is
read, and the window offered is never more than the receive buffer's free
room, so the peer can never send more than fits. The MSS option on the SYN asks for 1460 bytes
(`TCP_MSS`: an Ethernet frame); a peer that sends none gets 536. Ephemeral
ports go round from 49152.

**Input.** A segment shorter than 20 bytes, or with a header length under 20
or past the segment, counts in `tcp`; a wrong checksum (over the same
pseudo-header as UDP's, protocol 6) in `tcp-checksum`; one to a broadcast
address, or from 0.0.0.0, in `filtered` (TCP is between two hosts). Of the options, the MSS
is read, NOP and the end of the list are understood and any other is
stepped over by its length. A segment for no socket counts in `no-socket`
and is answered with a reset (one that acknowledges it, if it had no ACK),
never a reset for a reset. Then RFC 9293 3.10.7's steps, in order:

1. **The sequence number.** A segment outside the window is answered with an
   ACK (the answer to a duplicate) and dropped. With the window closed, a
   segment at the next byte expected, or at most an MSS past the window's
   edge (a peer that probed with new data counts those bytes as sent), still
   has its ACK and window read, but not its data: RFC 9293's "special
   allowance" for ACKs. An empty segment may sit right at the window's edge,
   as Linux allows: that is where a peer that filled the window sends its
   ACKs from, and dropping them when one of its segments was lost would drop
   the acknowledgments of this end's data until the peer resent it.
2. **A reset** is believed only at exactly the next sequence number expected;
   elsewhere in the window it gets a "challenge" ACK (RFC 5961 3.2), which a
   real peer answers with a reset at the right number and a blind attacker
   cannot. It counts in `resets` and ends the connection: `-ECONNREFUSED` for
   a connection being opened, `-ECONNRESET` for one established.
3. **A SYN** on a synchronized connection gets a challenge ACK too (RFC 5961
   4.2). A SYN that is the peer's first one again (in SYN-RECEIVED) gets the
   SYN-ACK again: the first was lost.
4. **The ACK** frees what it covers from the send buffer, takes a round-trip
   sample, and updates the peer's window unless the segment is older than
   the last one that did.
5. **Data** is taken only in order: a segment past a gap is dropped
   (`out-of-order`) and the ACK says what is missing; the peer sends it
   again. There is no reassembly queue: with a 4 KiB window, at most two
   segments are ever lost to one gap.
6. **The FIN**, once everything before it is in: end of file for the reader.
   A FIN past a gap is dropped like data (`out-of-order`, and an ACK).

Every segment with data is acknowledged at once; there are no delayed ACKs.

**Output** sends what the peer's window allows, in segments of at most the
peer's MSS, then the FIN once the owner is done and everything before it is
out. Silly window avoidance both ways (RFC 9293 3.8.6.2): a short segment
goes only if it ends the data queued or is at least half the largest window
the peer offered; and the window's right edge, as every segment offers it,
moves on only once the free room reaches past it by an MSS or half the
buffer, whichever is less, so the peer never hears of a few bytes freed at a
time. When a read frees that much, an ACK says so at once: the peer, with
nothing it may send, would otherwise only find out by probing. There is no
slow start or congestion window: the send buffer is under three full
segments, so what a congestion window would limit hardly exists here.

**Timers**, in ticks, run from `net_timer()`:

- **Retransmission** (RFC 6298): the timeout follows the measured round trip
  (smoothed RTT plus four deviations, one tick at least), one segment timed
  at a time and never one that was sent twice (Karn), within 200 ms and
  60 s (`TCP_RTO_MIN`, `TCP_RTO_MAX`), 1 s before the first sample. When it
  goes off, everything not acknowledged is sent again from the oldest byte
  ("go back N", which a receiver that drops out-of-order data needs), the
  timeout doubles and `retransmits` counts it. A SYN is sent 5 more times
  (`TCP_SYN_RETRIES`), anything else 8 (`TCP_RETRIES`); then the connection
  is given up with `-ETIMEDOUT` (an established one sends a reset first).
- **Persist**: with data waiting and the peer's window closed, one byte is
  sent past it on the same doubling timeout, until the window opens. An ACK
  that leaves the probe's byte untaken, the window still closed, puts the
  persist timer back on, so a probe is never a retransmission timeout too.
  While a process holds the socket, probes are not retries: a peer whose
  buffer stays full is never given up on. Once its owner closed it, they
  are: after 8 (`TCP_RETRIES`) the connection is reset and given up, and
  only an acknowledgment of new data starts the count over, so a peer that
  keeps its window shut can't keep the slot (Linux bounds such orphans the
  same way).
- **TIME-WAIT** lasts 60 s (`TCP_TIME_WAIT`); the peer's FIN again in it is
  acknowledged again and starts it over. A connection in FIN-WAIT-2 that
  nobody holds waits 60 s (`TCP_FIN_TIMEOUT`) for the peer's FIN.

**Closing.** `tcp_close()` hands the socket to the stack, which finishes the
close on its own (FIN-WAIT, LAST-ACK, TIME-WAIT) and frees the slot at the
end, or sooner: when the retries or the probes run out (above), or the
peer's FIN does not come within `TCP_FIN_TIMEOUT`. A socket closed with data still unread is reset instead (RFC 9293
3.10.4's ABORT, as RFC 1122 4.2.2.13 allows), and so is one that gets data
after its owner closed it: nobody will ever read it. A connection in
TIME-WAIT keeps its buffers only while its owner holds it, and when the
table is full the one nearest its end gives up its slot to a new socket:
TIME-WAIT only guards against old duplicates, and a slot is worth more.

**Initial sequence numbers** follow RFC 6528: a clock ticking every 4 µs
(from the CLINT's `mtime`) plus a keyed hash of the two addresses and ports,
so they are neither guessable nor reused by a quick reconnection. The tests
can set the next one (`tcp_isn_next`, written into the kernel's RAM) to see
sequence numbers wrap.

**The socket API** is kernel-internal until #85 puts it behind file
descriptors (`kernel/include/net.h` says what each returns). Every call but
`tcp_close()` is made by a process; those that wait sleep in `net_sleep()`
and give up with `-EINTR` as soon as the process is killed. Data is copied
from and to a process's memory through its page table, or kernel memory.

| Function | |
|---|---|
| `tcp_socket(&err)` | a new socket; `-ENOBUFS` when all 16 are taken |
| `tcp_bind(s, port)` | binds to `port` on every address (0: the next ephemeral port); `-EADDRINUSE`, `-EINVAL` |
| `tcp_listen(s, backlog)` | makes a bound socket a listener; up to `backlog` (taken as 1 to 8) connections wait for `tcp_accept`, and a SYN past them is dropped (`backlog`): the peer sends it again |
| `tcp_connect(s, addr, port)` | the handshake; `-ECONNREFUSED`, `-ETIMEDOUT`, `-ENETUNREACH`, `-EISCONN`, `-ENOBUFS`, `-EINVAL`, `-EINTR` |
| `tcp_accept(l, &err)` | the oldest connection established on the listener, a socket of its own |
| `tcp_send(s, pt, src, n)` | queues `n` bytes, waiting for room as it goes; `-EPIPE` after `tcp_shutdown`, `-ENOTCONN`, the error that ended the connection |
| `tcp_recv(s, pt, dst, n)` | up to `n` bytes, waiting for some; 0 at the end of the peer's data |
| `tcp_shutdown(s)` | no more sending: a FIN follows what is queued |
| `tcp_close(s)` | lets go of the socket (above); a listener resets the connections still waiting for `tcp_accept` |
| `tcp_ends(s, ...)` | the two ends' addresses and ports |

**Loopback** works like UDP's: a segment to 127.0.0.1 or the interface's
address is handled inside the `ip_output()` of the segment that caused it,
so the code that sends checks its connection again after each segment.
Like the rest of the stack, TCP needs a card to run (its timer runs only
then), even for loopback.

### Testing TCP until #85

Programs get sockets with #85. Until then one temporary system call,
`tcptest` (27, `docs/camelot-syscalls.md`), drives the socket API from a
process, and a program of the same name runs it:

```
tcptest connect ADDR PORT BYTES [TIMES]
tcptest serve PORT CONNECTIONS [BACKLOG]
```

- `connect` opens a connection, sends BYTES bytes of a fixed pattern (byte
  `i` is `i ^ i >> 8 ^ i >> 16`, so a byte out of place shows) in 512-byte
  calls, ends its side, reads until the other side ends its own, and prints
  `10.0.2.15:49152 > 10.0.2.2:8000: sent N, received N, hash H`, H being the
  FNV-1a hash of what it received; TIMES times.
- `serve` listens on PORT (backlog 4 by default), prints `listening on port
  PORT`, then accepts CONNECTIONS connections one after the other, sends
  back what each sends until it ends its side, and prints
  `10.0.2.2:49152 > 10.0.2.15:7000: echoed N, hash H` for each.

A connection that fails prints what it did so far, then `tcptest: REASON`
(`connection refused`, `connection reset`, `timed out`), and the program
exits with 1. With `--net user,hostfwd=tcp::7000-:7000`, `tcptest serve 7000
1` answers any TCP client on the host's port 7000; `tcptest connect 10.0.2.2
PORT 1000` talks to a server on the host.

The client reads only once it has sent everything, so an echo through
loopback larger than the buffers on the way (about 16 KiB: the four 4 KiB
buffers of the connection's two ends) stops both, each waiting for the other
to read: the program's design, not the stack's. Through the gateway the
host's buffers take up the difference.

The call, the program and the listeners a process holds by handle (closed
when it exits) all go when #85's socket calls replace them.

## DHCP

RFC 2131 with RFC 2132's options, in `dhcp.c`, on the socket API (port 68):
DISCOVER, OFFER, REQUEST, ACK, with the broadcast flag set since the client
has no address to be answered at, a transaction number checked on every
answer, and a NAK starting over. Each try waits a second (`DHCP_TIMEOUT`) for
an answer, four tries in all (`DHCP_TRIES`). The lease's address, mask,
router and first DNS server configure the interface. The lease is not
renewed: the gateway's (`crates/gateway`, behind `--net user`) never expire
while it runs.

## Configuration

At boot, after the filesystem, `net_boot()` gives the interface its address
(nothing happens without a card). Four variables in `kernel/net/net.c`, in
`.data` so they can be patched in `kernel.elf` (the tests do), decide how:

| Variable | Default | |
|---|---|---|
| `net_dhcp` | 1 | ask DHCP first |
| `net_static_addr` | `0x0a00020f` | 10.0.2.15 |
| `net_static_mask` | `0xffffff00` | 255.255.255.0 |
| `net_static_gateway` | `0x0a000202` | 10.0.2.2 (0 for none) |

With `net_dhcp` set, a kernel process `dhcp` runs the client:

```
net: 10.0.2.15/24, gateway 10.0.2.2, dns 10.0.2.3 (dhcp)
```

If no server answers, or with `net_dhcp` 0, the static configuration is used
(the defaults are what `--net user` would give the first card):

```
net: no answer from DHCP
net: 10.0.2.15/24, gateway 10.0.2.2 (static)
```

`ifconfig ADDR [MASK [GATEWAY]]` changes it later (the `netconf` system
call): an ordinary unicast address that is not its subnet's network or
broadcast address, a contiguous mask (255.255.255.0 by default), and a
gateway on the subnet or none. The ARP cache is emptied.

## Counters

Every frame dropped is counted under its reason, never a panic, whatever the
frame says about itself. The kernel keeps them in `struct net_stats`
(`kernel/include/syscall.h`), which the `netinfo` system call returns with
the configuration, and `ifconfig` prints them:

| `ifconfig` | Field | Counts |
|---|---|---|
| `rx N frames, N bytes` | `rx_frames`, `rx_bytes` | frames the card delivered |
| `tx N frames, N bytes` | `tx_frames`, `tx_bytes` | frames handed to the card |
| `card` | `rx_card_dropped` | the card's `RX_DROPPED`: bad lengths, no free descriptor |
| `filtered` | `rx_filtered` | not for this host: another MAC or address, ARP not about it |
| `ether` | `rx_bad_ether` | an unknown EtherType or a short frame |
| `arp` | `rx_bad_arp` | a malformed ARP packet |
| `ip` | `rx_bad_ip` | a malformed IPv4 header or length, or a source no host can have |
| `ip-checksum` | `rx_bad_ip_sum` | a wrong header checksum |
| `fragment` | `rx_fragment` | a fragment |
| `protocol` | `rx_bad_proto` | not ICMP, TCP or UDP |
| `icmp` | `rx_bad_icmp` | a short ICMP message or a wrong checksum |
| `udp` | `rx_bad_udp` | a short datagram or a wrong length field |
| `udp-checksum` | `rx_bad_udp_sum` | a wrong UDP checksum |
| `no-port` | `rx_no_port` | UDP to a port with no socket (and not echoed) |
| `queue-full` | `rx_queue_full` | UDP to a socket holding 4 datagrams |
| `tcp` | `rx_bad_tcp` | a TCP segment shorter than its header, or a header length under 20 bytes or past the segment |
| `tcp-checksum` | `rx_bad_tcp_sum` | a wrong TCP checksum |
| `no-buffer` | `no_buffer` | the pool was empty |
| `arp-failed` | `arp_failed` | packets dropped for want of an ARP answer |
| `tx-error` | `tx_errors` | the transmit queue was full, or the card failed a send |
| `no-socket` | `tcp_no_socket` | TCP for a port with no connection or listener (answered with a reset) |
| `backlog` | `tcp_backlog` | a SYN dropped: its listener's backlog was full, or no slot or memory was left |
| `out-of-order` | `tcp_out_of_order` | data (or a FIN) past a gap, dropped (the peer resends) |
| `retransmits` | `tcp_retransmits` | retransmission timeouts (each sends everything not acknowledged again) |
| `resets` | `tcp_resets` | connections ended by a reset from the peer |

TCP's last five are not drops of malformed frames but what its connections
went through; `ifconfig` prints them on a line of their own.

## Shell commands

Until #85 makes them programs, the shell has two built-ins on the system
calls `netinfo`, `netconf` and `ping` (`docs/camelot-syscalls.md`):

```
$ ping 10.0.2.2 2
PING 10.0.2.2: 56 data bytes
64 bytes from 10.0.2.2: seq=1 ttl=64 time=1.211 ms
64 bytes from 10.0.2.2: seq=2 ttl=64 time=0.723 ms
2 sent, 2 received
$ ifconfig
net0: mac 52:54:00:12:34:56, link up
  inet 10.0.2.15 netmask 255.255.255.0 gateway 10.0.2.2 dns 10.0.2.3 (dhcp)
  rx 5 frames, 922 bytes; tx 5 frames, 922 bytes
  rx dropped: card 0, filtered 0, ether 0, arp 0, ip 0, ip-checksum 0, fragment 0, protocol 0
  rx dropped: icmp 0, udp 0, udp-checksum 0, no-port 0, queue-full 0, tcp 0, tcp-checksum 0
  other: no-buffer 0, arp-failed 0, tx-error 0
  tcp: no-socket 0, backlog 0, out-of-order 0, retransmits 0, resets 0
lo: inet 127.0.0.1 netmask 255.0.0.0
```

Five frames each way: the DHCP DISCOVER and REQUEST and their answers, an
ARP request for the gateway and its reply, and the two echoes. The times
depend on how fast the host runs Grail.

`ping ADDR [N]` sends N echo requests (4 by default), a second apart, each
waiting a second for its reply: `seq=N: timed out` when none comes, or the
error (`host unreachable`, `network unreachable`) when the request couldn't
be sent. After a reply or an error the shell sleeps a second before the next
request; after a timeout it doesn't, the wait having taken the second. The
sequence number wraps at 65536, as it does on the wire. A process killed
while it waits for a reply stops waiting at once. `ping` alone still runs the `/bin/ping` demo program.

## Running it

```
grail --net user kernel/kernel.elf
```

gives the kernel user networking (spec section 9.8): DHCP from the gateway,
`ping 10.0.2.2` (the host), and UDP to the echo service through a forwarded
port (`--net user,hostfwd=udp::5555-:7`, then any UDP client to
127.0.0.1:5555), and TCP both ways with `tcptest` (above). Machines on one hub (in one process) or VMs on one Avalon
switch are on one Ethernet: give them static addresses on one subnet with
`ifconfig` and they ping each other.

## Tests

`crates/grail/tests/kernel_net.rs` runs the committed kernel in-process on
Grail's cards, by step counts, so what the kernel does is deterministic:

- `--net user`: the boot lines, the DHCP lease (10.0.2.15), `ping 10.0.2.2`,
  loopback (127.0.0.1 and 10.0.2.15) and no counter but traffic; the same
  through `grail --net user` on the command line;
- UDP end to end: a host socket sends to a forwarded port, the gateway (which
  checks every checksum) passes it to the echo service, and the echo comes
  back. The gateway runs on a clock driven by the machine's steps, not the
  wall clock (it services its host sockets on every poll, so waiting for the
  datagram doesn't move its clock); the two hops through host sockets are bounded waits (the
  datagram is in the gateway's queue before the machine runs on, and the
  echo has been handed to the host before the test reads it), never sleeps;
- two kernels on a hub: one with a patched static configuration, the other
  falling back from DHCP and then given its address with `ifconfig`; they
  ping each other, and a ping off the subnet has no route. Every address,
  mask and gateway `netconf` refuses, and what the shell can't parse, leave
  the configuration as it was;
- the test as the other host on a hub, sending crafted frames: every kind of
  malformed or unwanted frame above (bad checksums, truncated headers, wrong
  lengths, fragments, unknown EtherTypes and protocols, ports nobody has,
  frames the card refuses, sources no host can have) is dropped and counted
  under its reason, the stack still answers ARP, echo and UDP afterwards with
  correct checksums, and the frame counts add up;
- ARP giving up: three requests a second apart, the pings time out, the next
  one fails at once; and an answered entry expiring after a minute (the
  machine sleeps through it: idle, it skips ahead to its next tick).

At the end of each, with the machine idle, every packet buffer but the
receive ring's 16 is back in the pool (the test reads `free_count` from the
kernel's RAM), so a drop path that forgets to free its buffer fails them.

`crates/grail/tests/kernel_tcp.rs` tests TCP the same way, through
`tcptest` (the helpers both files share are in `tests/common`):

- through the gateway: the guest's client and a Rust server on the host
  (10.0.2.2) send each other 192 KiB at once, byte for byte, with the
  guest's window closing while it still sends; the guest's echo server
  answers host clients through a forwarded port, one large, then two at once
  with the second waiting in the backlog;
- both again through a backend between the card and the gateway that drops,
  duplicates and reorders TCP frames both ways by a fixed pattern over the
  frames' count, with no randomness (which frames that hits still varies a
  little from run to run, since the host's sockets follow the wall clock);
  the transfers are still exact, and `retransmits` and `out-of-order` show
  what it took;
- 40 connections in a row through the gateway and 20 over loopback, more
  than there are sockets; afterwards, once TIME-WAIT is over, every socket
  is free (`tcp_in_use`), every page of their buffers is back (`nfree`) and
  so is every packet buffer;
- the test as the other host on a hub, segment by segment: resets for ports
  nobody listens on, malformed segments counted; the handshake and its MSS
  option, a lost SYN-ACK, challenge ACKs for resets and SYNs in the window,
  duplicates, data past a gap, a full backlog, a reset ending a connection
  and a listener's close resetting the one waiting; closing first, closing
  at once (CLOSING), TIME-WAIT acknowledging a FIN again, a refused and a
  reset connection; a SYN and data sent again until the connection is given
  up, the timeout doubling each time (the test reads the kernel's `ticks`
  at every step, as each segment comes); a zero window probed until it
  opens, with an MSS of 1000 among options the stack skips, the probes
  doubling too; a socket closed into a window that stays shut, given up
  after its probes; sequence numbers wrapping past 2^32 both ways (8 KiB
  echoed, both initial numbers just below it); silly window avoidance both
  ways; closing with data unread, and data after closing, answered with a
  reset, and a closed socket's FIN-WAIT-2 timing out; and processes killed
  while they wait in `accept`, `connect`, `send` and `recv`, ending at once.

The gateway runs on the machine's clock, but the host's sockets don't, so
a SYN to the gateway waits (a bounded wait of the wall clock) for the host
side to connect, and a host program is waited for with the machine running,
since the kernel may still have something to send again.

UDP's socket API is only exercised by DHCP and the echo service until #85
gives programs sockets; its tests (a full queue counted in `queue-full`,
`-EADDRINUSE`, ephemeral ports, running out of sockets, `udp_recvfrom`'s
timeout) come with the socket calls.
