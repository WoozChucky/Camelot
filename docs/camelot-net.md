# Camelot's network stack

The Camelot kernel's networking, as far as issue #83 takes it: a driver for
Grail's network card (`docs/excalibur-spec.md` section 9.8), Ethernet, ARP,
IPv4, ICMP echo and UDP, an address from DHCP or a static configuration, and
two shell commands, `ifconfig` and `ping`. TCP (#84) and the socket system
calls and network tools (#85) build on it.

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
| `dhcp.c` | the DHCP client |
| `net.c` | the interface's configuration, bringing it up at boot, the counters, the timer, waiting, the system calls |

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
shape is always Ethernet, an IPv4 header without options, then ICMP or UDP).

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
4. for this host: the interface's address, 255.255.255.255 or the subnet's
   broadcast address; while the interface has no address, any UDP (a DHCP
   offer may come to the address offered). 127.0.0.0/8 never comes from the
   wire. Otherwise `filtered`;
5. ICMP (1) or UDP (17): otherwise `protocol`.

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
| `ip` | `rx_bad_ip` | a malformed IPv4 header or length |
| `ip-checksum` | `rx_bad_ip_sum` | a wrong header checksum |
| `fragment` | `rx_fragment` | a fragment |
| `protocol` | `rx_bad_proto` | neither ICMP nor UDP |
| `icmp` | `rx_bad_icmp` | a short ICMP message or a wrong checksum |
| `udp` | `rx_bad_udp` | a short datagram or a wrong length field |
| `udp-checksum` | `rx_bad_udp_sum` | a wrong UDP checksum |
| `no-port` | `rx_no_port` | UDP to a port with no socket (and not echoed) |
| `queue-full` | `rx_queue_full` | UDP to a socket holding 4 datagrams |
| `no-buffer` | `no_buffer` | the pool was empty |
| `arp-failed` | `arp_failed` | packets dropped for want of an ARP answer |
| `tx-error` | `tx_errors` | the transmit queue was full, or the card failed a send |

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
  rx dropped: icmp 0, udp 0, udp-checksum 0, no-port 0, queue-full 0
  other: no-buffer 0, arp-failed 0, tx-error 0
lo: inet 127.0.0.1 netmask 255.0.0.0
```

Five frames each way: the DHCP DISCOVER and REQUEST and their answers, an
ARP request for the gateway and its reply, and the two echoes. The times
depend on how fast the host runs Grail.

`ping ADDR [N]` sends N echo requests (4 by default), a second apart, each
waiting a second for its reply: `seq=N: timed out` when none comes, or the
error (`host unreachable`, `network unreachable`) when the request couldn't
be sent. `ping` alone still runs the `/bin/ping` demo program.

## Running it

```
grail --net user kernel/kernel.elf
```

gives the kernel user networking (spec section 9.8): DHCP from the gateway,
`ping 10.0.2.2` (the host), and UDP to the echo service through a forwarded
port (`--net user,hostfwd=udp::5555-:7`, then any UDP client to
127.0.0.1:5555). Machines on one hub (in one process) or VMs on one Avalon
switch are on one Ethernet: give them static addresses on one subnet with
`ifconfig` and they ping each other.

## Tests

`crates/grail/tests/kernel_net.rs` runs the committed kernel in-process on
Grail's cards, by step counts, so everything but the forwarded port is
deterministic:

- `--net user`: the boot lines, the DHCP lease (10.0.2.15), `ping 10.0.2.2`,
  loopback (127.0.0.1 and 10.0.2.15) and no counter but traffic; the same
  through `grail --net user` on the command line;
- UDP end to end: a host socket sends to a forwarded port, the gateway (which
  checks every checksum) passes it to the echo service, and the echo comes
  back;
- two kernels on a hub: one with a patched static configuration, the other
  falling back from DHCP and then given its address with `ifconfig`; they
  ping each other, and a ping off the subnet has no route;
- the test as the other host on a hub, sending crafted frames: every kind of
  malformed or unwanted frame above (bad checksums, truncated headers, wrong
  lengths, fragments, unknown EtherTypes and protocols, ports nobody has,
  frames the card refuses) is dropped and counted under its reason, the
  stack still answers ARP, echo and UDP afterwards with correct checksums,
  and the frame counts add up;
- ARP giving up: three requests a second apart, the pings time out, the next
  one fails at once.
