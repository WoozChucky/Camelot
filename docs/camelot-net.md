# Camelot's network stack

The Camelot kernel's networking (issues #83 to #85): a driver for Grail's
network card (`docs/excalibur-spec.md` section 9.8), Ethernet, ARP, IPv4,
ICMP echo, UDP and TCP, an address from DHCP or a static configuration,
the socket system calls that give programs TCP and UDP behind file
descriptors ([Sockets](#sockets)), and the network programs on them:
`ifconfig`, `ping`, `nc`, `fetch` and `httpd`, with a DNS resolver in the
user library ([Network programs](#network-programs)).

The code is in `kernel/net/`, one file per layer, with the whole interface in
`kernel/include/net.h` and the driver's in `kernel/include/nic.h`:

| File | What it does |
|---|---|
| `nic.c` | the card: descriptor rings, the interrupt, the transmit queue |
| `pbuf.c` | the packet buffer pool |
| `ether.c` | Ethernet II: filtering by address, dispatch by EtherType |
| `arp.c` | the ARP cache: requests, replies, timeouts, the packets waiting for an answer |
| `ip.c` | IPv4: header checks, checksums, routing, loopback |
| `icmp.c` | echo requests and replies |
| `udp.c` | UDP: ports, checksums, the kernel-internal socket API, the echo service |
| `tcp.c` | TCP: the connection state machine, retransmission, the kernel-internal socket API |
| `dhcp.c` | the DHCP client |
| `net.c` | the interface's configuration, bringing it up at boot, the counters, the timer, waiting, the `netinfo`, `netconf` and `ping` system calls |
| `socket.c` | the socket system calls: sockets behind file descriptors, on UDP's and TCP's kernel-internal APIs |

It is a small host stack in the spirit of xv6's lab network code and lwIP's
raw API: one interface, no forwarding, no fragmentation, no options, and every
limit a constant in `net.h`.

## Execution model

One spinlock, `net_lock` (`kernel/include/net.h`; spec section 10, "Kernel
locking"), covers the whole stack: the network system calls run under it, and
so do the card's interrupt, the timer and reads and writes of a socket file.
A received frame is handled from start to end inside the card's interrupt: the driver passes it to
`ether_input()`, which passes it to `arp_input()` or `ip_input()`, and so on
up; an answer (an ARP reply, an echo reply, a UDP echo) is sent from there
too. Sending goes down the same way from whoever sends, a process in a
system call or the stack itself.

A process that waits for the network (`ping`, a socket, DHCP) calls
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
  times in all (`ARP_TRIES`). The packets sent meanwhile wait in the entry,
  in order, up to 8 (`ARP_QUEUE`), and all go when the answer comes, as
  Linux's neighbour entries queue them (a TCP retransmission's segments to a
  gateway whose entry expired go together, not only the last); with 8
  waiting the oldest is dropped for the next and counts in `arp-failed`;
- **failed**: three requests went unanswered: the waiting packets are dropped
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
thread waits in a slot (one per thread) for the reply with the same
identifier and sequence number. The `ping` system call uses the caller's tid
as the identifier (a single-threaded process's tid is its pid), so two
threads' pings never mix, and a one-second
timeout. The round-trip time comes from `mtime` (the `time` CSR) and is reported
in microseconds.

## UDP

RFC 768. Input checks the length field (at least 8, no more than the
packet: `udp`) and the checksum over the pseudo-header and the datagram
(`udp-checksum`; a checksum of 0 means the sender computed none, and is
accepted). Output always computes one, and sends 0xffff for a sum of 0.

The kernel-internal socket API, which DHCP and the socket system calls
(`socket.c`) use. Data is copied from and to a process's memory through a
page table, or kernel memory (a null page table):

| Function | |
|---|---|
| `udp_open(port, &err)` | a socket bound to `port`, or with 0 the next free port from 49152 up; `-EADDRINUSE`, or `-ENOBUFS` when all 8 (`UDP_SOCKETS`) are taken |
| `udp_close(s)` | unbinds, dropping what was not read |
| `udp_sendto(s, dst, port, pt, src, len)` | sends from the socket's port; `len`, or `-EMSGSIZE` over 1472 bytes, `-EFAULT`, or the routing errors |
| `udp_recvfrom(s, pt, dst, size, &from, &port, timeout)` | the oldest datagram (truncated to `size`, the rest lost), its sender, its whole length; sleeps up to `timeout` ticks (0: forever), then `-ETIMEDOUT`; `-EINTR` once the process is killed |
| `udp_pending(s)` | the bytes of the datagrams waiting |

A socket is bound to a port on every address of the host. Datagrams wait on
it in their buffers, at most 4 (`UDP_QUEUE_MAX`); more are dropped
(`queue-full`). A socket with a peer (`raddr` and `rport`, set by the
`connect` system call) takes datagrams from that peer only (any
127.0.0.0/8 address stands for 127.0.0.1, which loopback replies come
from); the others are dropped and count in `no-port`, and the echo service
does not answer them either, since the port is the socket's. A datagram to a
port with no socket counts in `no-port`,
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
(from `mtime`, through the `time` CSR) plus a keyed hash of the two addresses and ports,
so they are neither guessable nor reused by a quick reconnection. The tests
can set the next one (`tcp_isn_next`, written into the kernel's RAM) to see
sequence numbers wrap.

**The socket API** is kernel-internal; the socket system calls put it
behind file descriptors (`kernel/include/net.h` says what each returns). Every call but
`tcp_close()` is made by a process; those that wait sleep in `net_sleep()`
and give up with `-EINTR` as soon as the process is killed. Data is copied
from and to a process's memory through its page table, or kernel memory.

| Function | |
|---|---|
| `tcp_socket(&err)` | a new socket; `-ENOBUFS` when all 16 are taken |
| `tcp_bind(s, port)` | binds to `port` on every address (0: the next ephemeral port); `-EADDRINUSE`, `-EINVAL` |
| `tcp_listen(s, backlog)` | makes a bound socket a listener; up to `backlog` (taken as 1 to 8) connections wait for `tcp_accept`, and a SYN past them is dropped (`backlog`): the peer sends it again |
| `tcp_connect(s, addr, port)` | the handshake; `-ECONNREFUSED`, `-ETIMEDOUT`, `-ENETUNREACH`, `-EISCONN`, `-ENOBUFS`, `-EINVAL`, `-EINTR` |
| `tcp_accept(l, timeout, &err)` | the oldest connection established on the listener, a socket of its own; `-EAGAIN` after `timeout` ticks (0: no limit), `-EINTR` |
| `tcp_send(s, pt, src, n)` | queues `n` bytes, waiting for room as it goes; `-EPIPE` after `tcp_shutdown`, `-ENOTCONN`, the error that ended the connection |
| `tcp_recv(s, pt, dst, n, timeout)` | up to `n` bytes, waiting for some; 0 at the end of the peer's data; `-EAGAIN` after `timeout` ticks (0: no limit), `-EINTR` |
| `tcp_shutdown(s)` | no more sending: a FIN follows what is queued |
| `tcp_close(s)` | lets go of the socket (above); a listener resets the connections still waiting for `tcp_accept` |
| `tcp_ends(s, ...)` | the two ends' addresses and ports |

**Loopback** works like UDP's: a segment to 127.0.0.1 or the interface's
address is handled inside the `ip_output()` of the segment that caused it,
so the code that sends checks its connection again after each segment.
Like the rest of the stack, TCP needs a card to run (its timer runs only
then), even for loopback.

## Sockets

`socket.c` gives programs the stack through the socket system calls
(`docs/camelot-syscalls.md`, "Sockets": BSD's calls, Linux's numbers and
`struct sockaddr_in`). A socket is a `struct socket` (`net.h`): its type,
the TCP or UDP socket under it, whether each direction was shut down, and
its receive timeout. An open file of type `FD_SOCKET` points at it
(`kernel/file.c`), so a socket is a descriptor like a file or a pipe: `read`
and `write` are `recv` and `send`, `dup` and `spawn` share it, and the last
`close` closes it, at a process's exit too (`tcp_close()` then finishes a
TCP connection on its own, above). There is one `struct socket` per open
file the kernel can have (`NFILE`), so the open file always runs out first.

- **Making one.** `socket()` takes a TCP socket at once (16 in all); a UDP
  socket gets its port, and with it one of the 8 UDP sockets, at `bind`,
  or at its first `connect` or `sendto` (an ephemeral port), as on Linux. A
  `listen` without a `bind` takes an ephemeral port too.
- **Addresses.** A socket is bound to a port on every address of the host;
  `bind` checks that the address is one of the host's (0.0.0.0, the
  interface's, 127.0.0.0/8: `-EADDRNOTAVAIL` otherwise) but keeps only the
  port. TCP's ports and UDP's are separate: a TCP listener on 7 and the UDP
  echo service, say.
- **UDP with a peer.** `connect` on a UDP socket only remembers the peer:
  `send` goes there, and the stack drops datagrams from anyone else.
- **Shutting down.** `shutdown(SHUT_WR)` is `tcp_shutdown()` for TCP (the
  FIN) and stops `send` for UDP; `SHUT_RD` makes `recv` return 0 from then
  on, without telling the peer, as on Linux.
- **Timeouts.** `SO_RCVTIMEO` (`setsockopt`) is kept in ticks, rounded up,
  and passed to `tcp_recv()`, `tcp_accept()` and `udp_recvfrom()`; one that
  runs out is `-EAGAIN`, as on Linux.
- **Errors** come in the order the document gives: the descriptor
  (`-EBADF`, then `-ENOTSOCK`), the pointers (`-EFAULT`), then the rest.
  Without a card `socket()` is `-ENODEV`, so no socket exists without one.

Port 7's UDP echo service answers while no socket has the port, so a
program that binds 7 takes it over.

## Network programs

The programs are in `kernel/user/` (`kernel/user/README.md` lists them all),
on the socket calls and the `netinfo`, `netconf` and `ping` calls. Each
says what went wrong as `NAME: REASON` and exits with 1.

```
# ping 10.0.2.2 2
PING 10.0.2.2: 56 data bytes
64 bytes from 10.0.2.2: seq=1 ttl=64 time=1.211 ms
64 bytes from 10.0.2.2: seq=2 ttl=64 time=0.723 ms
2 sent, 2 received
# ifconfig
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

- **`ifconfig [ADDR [MASK [GATEWAY]]]`** prints the interface and its
  counters (`netinfo`), or gives it a static address (`netconf`, below).
- **`ping HOST [N]`** sends N echo requests (4 by default), a second apart,
  each waiting a second for its reply: `seq=N: timed out` when none comes,
  or the error (`host unreachable`, `network unreachable`) when the request
  couldn't be sent. After a reply or an error it sleeps a second before the
  next request; after a timeout it doesn't, the wait having taken the
  second. The sequence number wraps at 65536, as it does on the wire. HOST
  may be a name (`PING web.test (10.0.2.2): ...`). It exits 0 if any reply
  came.
- **`nc [-u] [-v] HOST PORT`** and **`nc -l [-u] [-v] PORT`**: netcat. It
  copies its input to the other end and what comes from there to its
  output, both at once: with no `fork` or `poll`, the sending half is a
  second `nc` process with the socket as its output. With TCP the end of
  the input sends a FIN and nc ends once the other side has ended its own;
  input from the console, which ends only with Ctrl-D, is not waited for.
  With UDP (`-u`), each read of the input is a datagram; a listener talks
  to whoever sent the first one, and nc ends a second after its input is
  over and nothing more came.
- **`fetch http://HOST[:PORT][/PATH]`** downloads with an HTTP/1.0 GET to
  its output (`fetch URL > FILE` saves it), and checks the status (a 404 is
  `fetch: URL: 404 Not Found`) and the Content-Length.
- **`httpd [-p PORT] [-n COUNT] [-t SECONDS] [DIR]`** serves DIR's files
  (index.html for a directory, else a listing) over HTTP/1.0 to one client
  at a time, with a log line per request, for COUNT connections or until it
  is killed.
- **`shd [-p PORT] [-n MAX] [-v]`** is the shell server: each TCP
  connection to PORT (23) gets a shell of its own, `/bin/sh` spawned with
  the connection as its descriptors 0, 1 and 2, so several people (or
  windows) each have one, and a busy one holds up nobody. shd keeps a
  descriptor for each connection and waits for the next one half a second
  at a time (`SO_RCVTIMEO`); in between it collects the shells that ended
  (`procs`, then `wait`) and ends their connections with `shutdown`, and
  hangs up the sessions whose client left: `fstat` on a socket gives
  `nlink` 0 once its other end has hung up (its FIN came, or a reset), and
  with nothing left unread shd kills what the session runs (a shell that
  reads the end of a connection kills its background programs itself
  before it ends: once it has, nothing tells whose they were). There is no
  line discipline on a socket, so the client edits lines (`herald --tcp`,
  on Windows too) and the shell reads its input a byte at a time.
  `kernel/user/README.md` ("shd") has the details, and the security
  caveat: no users yet, so its shells run as root; forward its port from
  127.0.0.1 only. `cargo xtask up --demo` starts it in its VM, with
  127.0.0.1:2323 forwarded to port 23.
- **`nettest`** moves known bytes through TCP (`nettest connect ADDR PORT
  BYTES [TIMES]` sends a pattern and reads to the end, `nettest serve PORT
  CONNECTIONS [BACKLOG]` echoes each connection, each printing the bytes'
  counts and FNV-1a hash), for the TCP tests and by hand; **`socktest`**
  checks the socket calls from user space over loopback.

**Names.** `resolve()` (`kernel/user/lib/resolve.c`) is the resolver every
program uses: an address as it is, any other name asked of the DNS server
DHCP gave (10.0.2.3 behind `--net user`) with an A query over a connected
UDP socket, three tries a second apart. `localhost` is asked too: the
gateway answers it. An unknown name is `unknown host`; with no DNS server
(a static configuration), `network unreachable`.

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

## Running it

```
grail --net user kernel/kernel.elf
```

gives the kernel user networking (spec section 9.8): DHCP from the gateway,
`ping 10.0.2.2` (the host), `fetch http://10.0.2.2:8000/file` from a server
on the host, names through the gateway's DNS server, and UDP to the echo
service through a forwarded port (`--net user,hostfwd=udp::5555-:7`, then
any UDP client to 127.0.0.1:5555). A web server for the root filesystem:

```
grail --disk target/camelot-fs.img --net user,hostfwd=tcp::8080-:80 kernel/kernel.elf
# httpd &
```

then `http://127.0.0.1:8080/` on the host. Machines on one hub (in one
process) or VMs on one Avalon switch are on one Ethernet: give them static
addresses on one subnet with `ifconfig` and they ping each other, and `nc
-l 5000` in one answers `nc ADDR 5000` in the other.

## Tests

`crates/grail/tests/kernel_net.rs` runs the committed kernel in-process on
Grail's cards, by step counts, so what the kernel does is deterministic,
through the `ifconfig` and `ping` programs:

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
  mask and gateway `netconf` refuses, and what `ifconfig` can't parse, leave
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
`nettest` (the helpers the files share are in `tests/common`):

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

`crates/grail/tests/kernel_sockets.rs` tests the socket calls and the
programs, through the gateway with a resolver of the test's (so no DNS on
the internet is needed):

- `socktest`, over loopback: bad descriptors (`-EBADF`, `-ENOTSOCK`), bad
  pointers (`-EFAULT`, nothing done), bad families, addresses and ports,
  ports in use, every call on the wrong kind of socket or in the wrong
  state, TCP and UDP between its own sockets (datagram boundaries,
  truncation, the largest datagram, a full queue, a connected socket's
  filter), receive timeouts, shutdown, a socket as another program's output,
  running out of TCP and UDP sockets, and calls that end with `-EINTR` when
  their process is killed;
- `httpd` serving files from the machine's disk (an image the test makes
  with mkfs) to clients on the host through a forwarded port: a 20 KB file
  byte for byte, a directory's `index.html`, a 404, and `fetch` in the
  machine over loopback into a file; its log has a line for each;
- `fetch` from a Rust server on the host, by address and by a name the test
  resolver knows, a 404, an unknown name and a refused connection; `ping`
  by name;
- `nc` sending to a host server and reading its answer, then listening for
  a host client through a forwarded port, each side getting the other's
  bytes.

`crates/avalon/tests/sockets.rs` runs two kernels on an Avalon switch with
static addresses, `nc -l` in one and `nc` in the other, over TCP and UDP,
and a connection to a port nobody listens on, refused. Both machines run in
turns by steps, with time following the steps even while idle (`wfi` does
not skip to the next tick), so their clocks stay together.

A DNS query, like a SYN, makes the gateway work on a thread (its
resolver), so the machine waits for the answer, a bounded wait of the wall
clock (`SteppedGateway` in `tests/common`).
