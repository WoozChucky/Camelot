//! A user-mode NAT gateway: internet and host access for emulated machines with no
//! driver on the host, what QEMU calls "slirp" user networking (spec section 9.8,
//! "User networking").
//!
//! The gateway is a tiny router that lives in the emulator's process. On one side it
//! takes and gives raw Ethernet frames, as a network card's backend does; on the other
//! it uses ordinary `std::net` sockets. The guest sees a small network:
//!
//! | Address | What |
//! |---|---|
//! | 10.0.2.0/24 | the network |
//! | 10.0.2.2 | the gateway (router, DHCP server), and the host: connecting to it reaches the host's 127.0.0.1 |
//! | 10.0.2.3 | the DNS server, answering from the host's resolver |
//! | 10.0.2.15 and up | DHCP leases, one per MAC address |
//!
//! the same plan as QEMU's, so its documentation applies. What the gateway does with
//! each frame from the guest:
//!
//! - **ARP** requests for 10.0.2.2 and 10.0.2.3 are answered with [`GATEWAY_MAC`].
//! - **DHCP** (UDP port 67, [`dhcp`]) gives out the leases.
//! - **ICMP** echo requests to 10.0.2.2 and 10.0.2.3 are answered. ICMP to anywhere
//!   else is dropped: forwarding it would need raw sockets, which need privileges.
//! - **DNS** queries to 10.0.2.3 port 53 ([`dns`]) are answered with the A records the
//!   host's resolver gives.
//! - **UDP** to any other address goes out of a host `UdpSocket`, one per guest
//!   port; answers come back to the guest from the address they came from.
//! - **TCP** connections are made by the gateway on the guest's behalf with a host
//!   `TcpStream`; the guest talks TCP to the gateway ([`tcp`]), which copies the bytes.
//! - Port forwarding ([`HostFwd`]) does the opposite: the gateway listens on a host
//!   port and passes connections and datagrams to a guest's port, from 10.0.2.2.
//!
//! Everything is non-blocking and driven by the two calls a network card makes,
//! [`Gateway::send`] (a frame from the guest) and [`Gateway::recv`] (the next frame for
//! it), which also service the host sockets (at most every
//! [`Limits::service_interval`]). The only threads are short-lived helpers for the two
//! things `std::net` can only do blocking: connecting a `TcpStream` and resolving a
//! name. Their answers come back over a channel.
//!
//! A guest cannot exhaust the host: a guest machine (a MAC address, whatever source
//! addresses it makes up) has at most [`Limits::flows_per_guest`] flows (TCP
//! connections, UDP ports and forwarded UDP clients) and the gateway at most
//! [`Limits::flows`] in all, which also bounds the connect threads; each TCP flow
//! buffers at most [`Limits::tcp_buffer`] bytes each way, the queue toward the guest
//! holds at most [`Limits::queue`] frames, idle flows are closed, and no raw sockets
//! are used.
//!
//! The crate depends on nothing but std, and not on Grail: Grail wraps a [`Gateway`]
//! as a network card's backend (`grail --net user`), and anything else that moves
//! Ethernet frames (a switch) can do the same.

pub mod dhcp;
pub mod dns;
mod hostfwd;
pub mod tcp;
pub mod wire;

pub use hostfwd::{HostFwd, Proto};

use std::collections::hash_map::RandomState;
use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::hash::BuildHasher;
use std::io;
use std::net::{
    IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4, TcpListener, TcpStream, ToSocketAddrs, UdpSocket,
};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};
use wire::{tcp_flags::*, Mac, Tcp, TcpHeader, BROADCAST_MAC};

/// The guest network, 10.0.2.0/24.
pub const NETWORK: Ipv4Addr = Ipv4Addr::new(10, 0, 2, 0);
pub const NETMASK: Ipv4Addr = Ipv4Addr::new(255, 255, 255, 0);
/// The gateway: router, DHCP server, and the host's loopback as the guest sees it.
pub const GATEWAY_ADDR: Ipv4Addr = Ipv4Addr::new(10, 0, 2, 2);
/// The DNS server.
pub const DNS_ADDR: Ipv4Addr = Ipv4Addr::new(10, 0, 2, 3);
/// The first DHCP lease, and where forwarded ports go unless a rule says otherwise.
pub const FIRST_LEASE: Ipv4Addr = Ipv4Addr::new(10, 0, 2, 15);
/// The last DHCP lease: 240 machines in all.
pub const LAST_LEASE: Ipv4Addr = Ipv4Addr::new(10, 0, 2, 254);
/// The gateway's MAC address, for 10.0.2.2 and 10.0.2.3 (QEMU's): locally
/// administered, `0a:00:02:02` spelling the address.
pub const GATEWAY_MAC: Mac = [0x52, 0x55, 0x0a, 0x00, 0x02, 0x02];
/// A guest card's MAC address when nothing else picks one (QEMU's default too).
pub const DEFAULT_GUEST_MAC: Mac = [0x52, 0x54, 0x00, 0x12, 0x34, 0x56];
/// First port the gateway uses as its own end of forwarded connections.
pub const FIRST_PORT: u16 = 49152;

/// Whether a guest may have `ip`: in 10.0.2.0/24, and not the network, broadcast,
/// gateway or DNS address.
pub fn is_guest_addr(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    o[..3] == NETWORK.octets()[..3] && ![0, 2, 3, 255].contains(&o[3])
}

/// Looks a name up, giving its IPv4 addresses (the DNS server's A records).
pub type Resolver = Arc<dyn Fn(&str) -> io::Result<Vec<Ipv4Addr>> + Send + Sync>;

/// The host's resolver, through `ToSocketAddrs`: what any program on the host gets.
pub fn host_resolver() -> Resolver {
    Arc::new(|name: &str| {
        let mut addrs = Vec::new();
        for a in (name, 0).to_socket_addrs()? {
            if let IpAddr::V4(v4) = a.ip() {
                if !addrs.contains(&v4) {
                    addrs.push(v4);
                }
            }
        }
        Ok(addrs)
    })
}

/// What keeps one guest from using up the host, and the timeouts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Limits {
    /// Flows one guest machine may have at once: TCP connections (either way), UDP
    /// ports it sends from, and host clients of forwarded UDP ports. A machine is a
    /// MAC address: each flow counts against the MAC address that opened it (for a
    /// forwarded port, the one its guest address was learned from), so a guest that
    /// sends from made-up source addresses gains nothing. A SYN past the limit is
    /// reset, a datagram dropped, a forwarded connection closed.
    pub flows_per_guest: usize,
    /// Flows the gateway has at once, all guests together, refused the same way: the
    /// bound for a guest that makes up MAC addresses too. Every host socket and every
    /// connect thread belongs to a flow, so this bounds them as well.
    pub flows: usize,
    /// Bytes a TCP flow holds each way (toward the host, and toward the guest).
    pub tcp_buffer: usize,
    /// Frames waiting for the guest; more are dropped. While it is half full no more is
    /// read from host sockets (TCP streams and UDP sockets) and no forwarded connection
    /// is accepted: what the host sends waits in the host's socket buffers.
    pub queue: usize,
    /// DNS lookups running at once; queries past it are dropped (the guest retries).
    pub dns_lookups: usize,
    /// A UDP flow (or forwarded UDP client) with no datagram either way for this long
    /// is closed.
    pub udp_idle: Duration,
    /// An established TCP connection with no traffic for this long is reset.
    pub tcp_idle: Duration,
    /// A TCP connection being opened or closed (a FIN sent either way) with no traffic
    /// for this long is reset.
    pub tcp_closing: Duration,
    /// How long connecting to the host side of a guest's TCP connection may take.
    pub connect_timeout: Duration,
    /// The first retransmission timeout toward the guest; it doubles at each retry,
    /// up to [`tcp::MAX_RTO`].
    pub tcp_rto: Duration,
    /// Retransmissions of one segment before the connection is reset.
    pub tcp_retries: u32,
    /// Host sockets are serviced at most this often, by [`Gateway::recv`].
    pub service_interval: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            flows_per_guest: 256,
            flows: 1024,
            tcp_buffer: 64 * 1024,
            queue: 512,
            dns_lookups: 8,
            udp_idle: Duration::from_secs(60),
            tcp_idle: Duration::from_secs(2 * 60 * 60),
            tcp_closing: Duration::from_secs(60),
            connect_timeout: Duration::from_secs(10),
            tcp_rto: Duration::from_millis(200),
            tcp_retries: 8,
            service_interval: Duration::from_millis(1),
        }
    }
}

/// How to build a [`Gateway`].
#[derive(Clone)]
pub struct Config {
    pub hostfwd: Vec<HostFwd>,
    pub limits: Limits,
    /// Answers the DNS server's lookups; [`host_resolver`] by default.
    pub resolver: Resolver,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            hostfwd: Vec::new(),
            limits: Limits::default(),
            resolver: host_resolver(),
        }
    }
}

impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Config")
            .field("hostfwd", &self.hostfwd)
            .field("limits", &self.limits)
            .finish_non_exhaustive()
    }
}

/// Counters, for tests and curious hosts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Stats {
    /// Frames from the guest dropped as malformed: truncated, bad checksums, unknown
    /// layouts.
    pub malformed: u64,
    /// Frames for the guest dropped because the queue was full.
    pub queue_full: u64,
    /// Flows refused because the guest had too many.
    pub refused: u64,
}

/// What helper threads send back.
enum Event {
    Connected {
        key: (SocketAddrV4, SocketAddrV4),
        id: u64,
        result: io::Result<TcpStream>,
    },
    Resolved {
        guest: SocketAddrV4,
        query: dns::Query,
        result: io::Result<Vec<Ipv4Addr>>,
    },
}

/// A guest UDP port's host socket.
struct UdpFlow {
    socket: UdpSocket,
    last_active: Instant,
    /// The MAC address that opened it, for [`Limits::flows_per_guest`].
    owner: Mac,
}

/// A forwarded UDP port: its socket, and the host clients using it, each seen by the
/// guest as a port of 10.0.2.2 (the key).
struct UdpForward {
    rule: HostFwd,
    socket: UdpSocket,
    clients: HashMap<u16, FwdClient>,
}

struct FwdClient {
    addr: SocketAddr,
    last_active: Instant,
    /// The guest's MAC address when the client came, if it was known.
    owner: Option<Mac>,
}

/// An IP packet waiting for the guest's MAC address (asked for with ARP).
struct Unresolved {
    ip: Ipv4Addr,
    packet: Vec<u8>,
    since: Instant,
}

/// How long a packet waits for an ARP reply before it is dropped.
const ARP_WAIT: Duration = Duration::from_secs(1);
/// Packets waiting for ARP replies, at most.
const MAX_UNRESOLVED: usize = 64;
/// Datagrams or connections taken from one host socket per service, so one busy
/// socket cannot hold up the rest.
const SOCKET_BUDGET: usize = 64;

pub struct Gateway {
    limits: Limits,
    resolver: Resolver,
    dhcp: dhcp::Server,
    /// Frames for the guest, oldest first.
    out: VecDeque<Vec<u8>>,
    /// Guests' MAC addresses, learned from what they send.
    neighbors: HashMap<Ipv4Addr, Mac>,
    unresolved: VecDeque<Unresolved>,
    /// TCP flows by (guest end, the end the guest sees).
    tcp: HashMap<(SocketAddrV4, SocketAddrV4), tcp::Flow>,
    /// UDP flows by guest end.
    udp: HashMap<SocketAddrV4, UdpFlow>,
    tcp_fwd: Vec<(HostFwd, TcpListener)>,
    udp_fwd: Vec<UdpForward>,
    /// The forwarding rules in the order given, with the host ports bound.
    bound: Vec<HostFwd>,
    events: Receiver<Event>,
    events_tx: Sender<Event>,
    dns_running: usize,
    ip_id: u16,
    next_port: u16,
    next_id: u64,
    /// Keyed hashing seeded by the system: initial sequence numbers nobody can guess.
    iss_key: RandomState,
    last_service: Option<Instant>,
    stats: Stats,
}

impl fmt::Debug for Gateway {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Gateway")
            .field("hostfwd", &self.hostfwd())
            .field("flows", &self.flows())
            .field("stats", &self.stats)
            .finish_non_exhaustive()
    }
}

impl Gateway {
    /// A gateway with its forwarded ports bound (an error if one cannot be).
    pub fn new(config: Config) -> io::Result<Gateway> {
        let mut tcp_fwd = Vec::new();
        let mut udp_fwd = Vec::new();
        let mut bound = Vec::new();
        let context = |rule: &HostFwd, e: io::Error| io::Error::new(e.kind(), format!("hostfwd {rule}: {e}"));
        // A host port 0 shows the port the system picked.
        let bound_to = |rule: &HostFwd, addr: io::Result<SocketAddr>| match addr {
            Ok(SocketAddr::V4(host)) => HostFwd { host, ..*rule },
            _ => *rule,
        };
        for rule in &config.hostfwd {
            match rule.proto {
                Proto::Tcp => {
                    let l = TcpListener::bind(rule.host).map_err(|e| context(rule, e))?;
                    l.set_nonblocking(true)?;
                    bound.push(bound_to(rule, l.local_addr()));
                    tcp_fwd.push((*rule, l));
                }
                Proto::Udp => {
                    let s = UdpSocket::bind(rule.host).map_err(|e| context(rule, e))?;
                    s.set_nonblocking(true)?;
                    bound.push(bound_to(rule, s.local_addr()));
                    udp_fwd.push(UdpForward {
                        rule: *rule,
                        socket: s,
                        clients: HashMap::new(),
                    });
                }
            }
        }
        let (events_tx, events) = channel();
        Ok(Gateway {
            limits: config.limits,
            resolver: config.resolver,
            dhcp: dhcp::Server::default(),
            out: VecDeque::new(),
            neighbors: HashMap::new(),
            unresolved: VecDeque::new(),
            tcp: HashMap::new(),
            udp: HashMap::new(),
            tcp_fwd,
            udp_fwd,
            bound,
            events,
            events_tx,
            dns_running: 0,
            ip_id: 0,
            next_port: FIRST_PORT,
            next_id: 0,
            iss_key: RandomState::new(),
            last_service: None,
            stats: Stats::default(),
        })
    }

    /// The forwarding rules as bound, in the order [`Config::hostfwd`] gave them: a
    /// host port 0 shows the port the system picked.
    pub fn hostfwd(&self) -> Vec<HostFwd> {
        self.bound.clone()
    }

    /// TCP and UDP flows open now.
    pub fn flows(&self) -> usize {
        self.tcp.len() + self.udp.len() + self.udp_fwd.iter().map(|f| f.clients.len()).sum::<usize>()
    }

    /// The address leased to a card, if it has one.
    pub fn lease_of(&self, mac: &Mac) -> Option<Ipv4Addr> {
        self.dhcp.lease_of(mac)
    }

    /// Every card that has a lease, and its address, in address order.
    pub fn leases(&self) -> Vec<(Mac, Ipv4Addr)> {
        self.dhcp.leases()
    }

    pub fn stats(&self) -> Stats {
        self.stats
    }

    /// A frame from the guest network.
    pub fn send(&mut self, frame: &[u8]) {
        self.send_at(frame, Instant::now());
    }

    /// The next frame for the guest network, if any. Services the host sockets first
    /// when nothing is waiting (at most every [`Limits::service_interval`]). Never blocks.
    pub fn recv(&mut self) -> Option<Vec<u8>> {
        self.recv_at(Instant::now())
    }

    /// [`Gateway::send`] at a given time (tests drive the timers this way).
    pub fn send_at(&mut self, frame: &[u8], now: Instant) {
        let Some(eth) = wire::parse_eth(frame) else {
            self.stats.malformed += 1;
            return;
        };
        // Only frames to the gateway or broadcast; on a shared segment the others are
        // for other machines. A group source address is never a real sender.
        if (eth.dst != GATEWAY_MAC && eth.dst != BROADCAST_MAC)
            || eth.src[0] & 1 != 0
            || eth.src == GATEWAY_MAC
        {
            return;
        }
        match eth.ethertype {
            wire::ETHERTYPE_ARP => self.on_arp(eth.payload),
            wire::ETHERTYPE_IPV4 => self.on_ipv4(eth.src, eth.payload, now),
            _ => {}
        }
    }

    /// [`Gateway::recv`] at a given time.
    pub fn recv_at(&mut self, now: Instant) -> Option<Vec<u8>> {
        let due = self
            .last_service
            .is_none_or(|t| now.saturating_duration_since(t) >= self.limits.service_interval);
        if self.out.is_empty() && due {
            self.service(now);
        }
        self.out.pop_front()
    }

    // ---- From the guest ----

    fn on_arp(&mut self, p: &[u8]) {
        let Some(arp) = wire::parse_arp(p) else {
            self.stats.malformed += 1;
            return;
        };
        if is_guest_addr(arp.sender_ip) && arp.sender_mac[0] & 1 == 0 {
            self.learn(arp.sender_ip, arp.sender_mac);
        }
        if arp.op == wire::ARP_REQUEST && (arp.target_ip == GATEWAY_ADDR || arp.target_ip == DNS_ADDR) {
            let reply = wire::arp_packet(&wire::Arp {
                op: wire::ARP_REPLY,
                sender_mac: GATEWAY_MAC,
                sender_ip: arp.target_ip,
                target_mac: arp.sender_mac,
                target_ip: arp.sender_ip,
            });
            self.push(wire::eth_frame(
                arp.sender_mac,
                GATEWAY_MAC,
                wire::ETHERTYPE_ARP,
                &reply,
            ));
        }
    }

    /// Remembers `ip`'s MAC address, and sends what waited for it.
    fn learn(&mut self, ip: Ipv4Addr, mac: Mac) {
        if self.neighbors.insert(ip, mac) == Some(mac) {
            return;
        }
        let (ready, waiting) = std::mem::take(&mut self.unresolved)
            .into_iter()
            .partition(|u| u.ip == ip);
        self.unresolved = waiting;
        for u in ready {
            self.push(wire::eth_frame(mac, GATEWAY_MAC, wire::ETHERTYPE_IPV4, &u.packet));
        }
    }

    fn on_ipv4(&mut self, src_mac: Mac, p: &[u8], now: Instant) {
        let Some(ip) = wire::parse_ipv4(p) else {
            self.stats.malformed += 1;
            return;
        };
        // DHCP comes before the guest has an address (from 0.0.0.0).
        if ip.proto == wire::PROTO_UDP && (ip.dst == Ipv4Addr::BROADCAST || ip.dst == GATEWAY_ADDR) {
            if let Some(udp) = wire::parse_udp(&ip) {
                if udp.dst_port == dhcp::SERVER_PORT {
                    self.on_dhcp(src_mac, udp.payload);
                    return;
                }
            }
        }
        if !is_guest_addr(ip.src) {
            return;
        }
        self.learn(ip.src, src_mac);
        match ip.proto {
            wire::PROTO_ICMP => self.on_icmp(&ip, now),
            wire::PROTO_UDP => match wire::parse_udp(&ip) {
                Some(udp) => self.on_udp(src_mac, &ip, &udp, now),
                None => self.stats.malformed += 1,
            },
            wire::PROTO_TCP => match wire::parse_tcp(&ip) {
                Some(seg) => self.on_tcp(src_mac, &ip, &seg, now),
                None => self.stats.malformed += 1,
            },
            _ => {}
        }
    }

    fn on_dhcp(&mut self, src_mac: Mac, msg: &[u8]) {
        let Some(req) = dhcp::parse(msg) else {
            self.stats.malformed += 1;
            return;
        };
        // A client asks for its own card's address (there are no relays on the guest
        // network): a card cannot take leases for made-up addresses and drain the pool.
        if req.chaddr != src_mac {
            return;
        }
        let Some(reply) = self.dhcp.handle(&req) else {
            return;
        };
        let udp = wire::udp_datagram(
            GATEWAY_ADDR,
            reply.ip,
            dhcp::SERVER_PORT,
            dhcp::CLIENT_PORT,
            &reply.message,
        );
        let packet = self.ip_packet(GATEWAY_ADDR, reply.ip, wire::PROTO_UDP, &udp);
        self.push(wire::eth_frame(
            reply.mac,
            GATEWAY_MAC,
            wire::ETHERTYPE_IPV4,
            &packet,
        ));
        if let Some(ip) = self.dhcp.lease_of(&req.chaddr) {
            self.learn(ip, req.chaddr);
        }
    }

    fn on_icmp(&mut self, ip: &wire::Ipv4, now: Instant) {
        let Some(echo) = wire::parse_echo(ip.payload) else {
            return;
        };
        if echo.kind != wire::ICMP_ECHO_REQUEST || (ip.dst != GATEWAY_ADDR && ip.dst != DNS_ADDR) {
            return;
        }
        let reply = wire::echo_message(&wire::Echo {
            kind: wire::ICMP_ECHO_REPLY,
            ..echo
        });
        let packet = self.ip_packet(ip.dst, ip.src, wire::PROTO_ICMP, &reply);
        self.deliver(ip.src, packet, now);
    }

    fn on_udp(&mut self, src_mac: Mac, ip: &wire::Ipv4, udp: &wire::Udp, now: Instant) {
        let guest = SocketAddrV4::new(ip.src, udp.src_port);
        if ip.dst == DNS_ADDR {
            if udp.dst_port == dns::PORT {
                self.on_dns(guest, udp.payload, now);
            }
            return;
        }
        if ip.dst == GATEWAY_ADDR {
            // An answer to a host client of a forwarded port?
            for fwd in &mut self.udp_fwd {
                if fwd.rule.guest == guest {
                    if let Some(client) = fwd.clients.get_mut(&udp.dst_port) {
                        client.last_active = now;
                        let _ = fwd.socket.send_to(udp.payload, client.addr);
                        return;
                    }
                }
            }
        }
        let Some(host_ip) = host_addr(ip.dst) else {
            return;
        };
        if !self.udp.contains_key(&guest) {
            if !self.admit(Some(src_mac)) {
                return;
            }
            let Ok(socket) = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)) else {
                return;
            };
            if socket.set_nonblocking(true).is_err() {
                return;
            }
            self.udp.insert(
                guest,
                UdpFlow {
                    socket,
                    last_active: now,
                    owner: src_mac,
                },
            );
        }
        let flow = self.udp.get_mut(&guest).unwrap();
        flow.last_active = now;
        // A failed send is a lost datagram, as on any network.
        let _ = flow
            .socket
            .send_to(udp.payload, SocketAddrV4::new(host_ip, udp.dst_port));
    }

    fn on_dns(&mut self, guest: SocketAddrV4, msg: &[u8], now: Instant) {
        let query = match dns::parse(msg) {
            dns::Parsed::Query(q) => q,
            dns::Parsed::Reply(m) => return self.udp_to_guest(DNS_ADDR, dns::PORT, guest, &m, now),
            dns::Parsed::Drop => {
                self.stats.malformed += 1;
                return;
            }
        };
        match dns::plan(&query) {
            dns::Plan::Reply(m) => self.udp_to_guest(DNS_ADDR, dns::PORT, guest, &m, now),
            dns::Plan::Resolve(name) => {
                if self.dns_running >= self.limits.dns_lookups {
                    return;
                }
                let resolver = self.resolver.clone();
                let tx = self.events_tx.clone();
                let spawned = std::thread::Builder::new()
                    .name("gateway-dns".into())
                    .spawn(move || {
                        let result = resolver(&name);
                        let _ = tx.send(Event::Resolved { guest, query, result });
                    });
                if spawned.is_ok() {
                    self.dns_running += 1;
                }
            }
        }
    }

    fn on_tcp(&mut self, src_mac: Mac, ip: &wire::Ipv4, seg: &Tcp, now: Instant) {
        let guest = SocketAddrV4::new(ip.src, seg.src_port);
        let peer = SocketAddrV4::new(ip.dst, seg.dst_port);
        let key = (guest, peer);
        if let Some(flow) = self.tcp.get_mut(&key) {
            let mut segs = Vec::new();
            flow.on_segment(seg, now, &mut segs);
            let done = flow.done;
            self.tcp_to_guest(guest, peer, segs, now);
            if done {
                self.tcp.remove(&key);
            }
            return;
        }
        if seg.has(RST) {
            return;
        }
        // Anything but a fresh SYN for a connection we do not know gets a reset.
        let refuse = |gw: &mut Gateway| {
            let reset = if seg.has(ACK) {
                (RST, seg.ack, 0)
            } else {
                (RST | ACK, 0, seg.seq.wrapping_add(seg.seq_len()))
            };
            let s = tcp::Seg {
                flags: reset.0,
                seq: reset.1,
                ack: reset.2,
                window: 0,
                mss: None,
                payload: Vec::new(),
            };
            gw.tcp_to_guest(guest, peer, vec![s], now);
        };
        if !seg.has(SYN) || seg.has(ACK) {
            return refuse(self);
        }
        let Some(host_ip) = host_addr(ip.dst) else {
            return refuse(self);
        };
        if !self.admit(Some(src_mac)) {
            return refuse(self);
        }
        let id = self.new_id();
        let iss = self.new_iss(guest, peer);
        let mut flow = tcp::Flow::from_guest(id, guest, peer, seg, iss, now, &self.limits);
        flow.owner = Some(src_mac);
        let tx = self.events_tx.clone();
        let target = SocketAddr::V4(SocketAddrV4::new(host_ip, seg.dst_port));
        let timeout = self.limits.connect_timeout;
        let spawned = std::thread::Builder::new()
            .name("gateway-connect".into())
            .spawn(move || {
                let result = TcpStream::connect_timeout(&target, timeout);
                let _ = tx.send(Event::Connected { key, id, result });
            });
        match spawned {
            Ok(_) => {
                self.tcp.insert(key, flow);
            }
            Err(_) => refuse(self),
        }
    }

    // ---- Host side ----

    /// Answers from helper threads, forwarded ports, UDP sockets and TCP streams; timers.
    fn service(&mut self, now: Instant) {
        self.last_service = Some(now);
        while let Ok(event) = self.events.try_recv() {
            self.on_event(event, now);
        }
        self.accept_forwarded(now);
        self.recv_forwarded_udp(now);
        self.recv_udp(now);

        // Segments wait in `sends` until every flow had its turn; they count toward
        // the queue already.
        let mut sends = Vec::new();
        let mut pending = 0;
        let half = self.half_queue();
        for (key, flow) in &mut self.tcp {
            let may_read = self.out.len() + pending < half;
            let mut segs = Vec::new();
            flow.service(now, &self.limits, may_read, &mut segs);
            if !segs.is_empty() {
                pending += segs.len();
                sends.push((*key, segs));
            }
        }
        for ((guest, peer), segs) in sends {
            self.tcp_to_guest(guest, peer, segs, now);
        }
        self.tcp.retain(|_, f| !f.done);

        let idle = self.limits.udp_idle;
        self.udp
            .retain(|_, f| now.saturating_duration_since(f.last_active) < idle);
        for fwd in &mut self.udp_fwd {
            fwd.clients
                .retain(|_, c| now.saturating_duration_since(c.last_active) < idle);
        }
        self.unresolved
            .retain(|u| now.saturating_duration_since(u.since) < ARP_WAIT);
    }

    fn on_event(&mut self, event: Event, now: Instant) {
        match event {
            Event::Connected { key, id, result } => {
                let Some(flow) = self.tcp.get_mut(&key).filter(|f| f.id == id) else {
                    return; // the guest gave up meanwhile; the stream is dropped
                };
                let result = result.and_then(|s| {
                    s.set_nonblocking(true)?;
                    let _ = s.set_nodelay(true);
                    Ok(s)
                });
                let mut segs = Vec::new();
                flow.connected(result, now, &mut segs);
                let done = flow.done;
                self.tcp_to_guest(key.0, key.1, segs, now);
                if done {
                    self.tcp.remove(&key);
                }
            }
            Event::Resolved { guest, query, result } => {
                self.dns_running -= 1;
                let m = match result {
                    Ok(addrs) => dns::answer(&query, dns::NOERROR, &addrs),
                    Err(_) => dns::answer(&query, dns::NXDOMAIN, &[]),
                };
                self.udp_to_guest(DNS_ADDR, dns::PORT, guest, &m, now);
            }
        }
    }

    fn accept_forwarded(&mut self, now: Instant) {
        for i in 0..self.tcp_fwd.len() {
            for _ in 0..SOCKET_BUDGET {
                if !self.may_read() {
                    return;
                }
                let (rule, listener) = &self.tcp_fwd[i];
                let rule = *rule;
                let stream = match listener.accept() {
                    Ok((stream, _)) => stream,
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    Err(_) => break,
                };
                // Dropping the stream closes the host's connection at once.
                let owner = self.neighbors.get(rule.guest.ip()).copied();
                if !self.admit(owner) {
                    continue;
                }
                if stream.set_nonblocking(true).is_err() {
                    continue;
                }
                let _ = stream.set_nodelay(true);
                let Some(port) = self.new_port() else {
                    continue;
                };
                let peer = SocketAddrV4::new(GATEWAY_ADDR, port);
                let id = self.new_id();
                let iss = self.new_iss(rule.guest, peer);
                let mut segs = Vec::new();
                let mut flow =
                    tcp::Flow::to_guest(id, rule.guest, peer, stream, iss, now, &self.limits, &mut segs);
                flow.owner = owner;
                self.tcp.insert((rule.guest, peer), flow);
                self.tcp_to_guest(rule.guest, peer, segs, now);
            }
        }
    }

    fn recv_forwarded_udp(&mut self, now: Instant) {
        let mut buf = vec![0u8; 64 * 1024];
        for i in 0..self.udp_fwd.len() {
            for _ in 0..SOCKET_BUDGET {
                if !self.may_read() {
                    return;
                }
                let (n, from) = match self.udp_fwd[i].socket.recv_from(&mut buf) {
                    Ok(r) => r,
                    Err(e) if retry_udp(&e) => continue,
                    Err(_) => break,
                };
                if n > wire::UDP_MAX_PAYLOAD {
                    continue;
                }
                let guest = self.udp_fwd[i].rule.guest;
                let known = self.udp_fwd[i].clients.iter_mut().find(|(_, c)| c.addr == from);
                let port = match known {
                    Some((p, client)) => {
                        client.last_active = now;
                        *p
                    }
                    None => {
                        let owner = self.neighbors.get(guest.ip()).copied();
                        if !self.admit(owner) {
                            continue;
                        }
                        let Some(p) = self.new_port() else {
                            continue;
                        };
                        let client = FwdClient {
                            addr: from,
                            last_active: now,
                            owner,
                        };
                        self.udp_fwd[i].clients.insert(p, client);
                        p
                    }
                };
                self.udp_to_guest(GATEWAY_ADDR, port, guest, &buf[..n], now);
            }
        }
    }

    fn recv_udp(&mut self, now: Instant) {
        let mut buf = vec![0u8; 64 * 1024];
        let mut got = Vec::new();
        let room = self.half_queue().saturating_sub(self.out.len());
        for (guest, flow) in &mut self.udp {
            for _ in 0..SOCKET_BUDGET {
                if got.len() >= room {
                    break;
                }
                match flow.socket.recv_from(&mut buf) {
                    Ok((n, SocketAddr::V4(from))) if n <= wire::UDP_MAX_PAYLOAD => {
                        flow.last_active = now;
                        // The host's loopback is 10.0.2.2 to the guest.
                        let ip = if from.ip().is_loopback() {
                            GATEWAY_ADDR
                        } else {
                            *from.ip()
                        };
                        got.push((ip, from.port(), *guest, buf[..n].to_vec()));
                    }
                    Ok(_) => {}
                    Err(e) if retry_udp(&e) => {}
                    Err(_) => break,
                }
            }
        }
        for (ip, port, guest, data) in got {
            self.udp_to_guest(ip, port, guest, &data, now);
        }
    }

    // ---- Toward the guest ----

    fn push(&mut self, frame: Vec<u8>) {
        if self.out.len() >= self.limits.queue {
            self.stats.queue_full += 1;
        } else {
            self.out.push_back(frame);
        }
    }

    fn ip_packet(&mut self, src: Ipv4Addr, dst: Ipv4Addr, proto: u8, payload: &[u8]) -> Vec<u8> {
        self.ip_id = self.ip_id.wrapping_add(1);
        wire::ipv4_packet(src, dst, proto, self.ip_id, payload)
    }

    /// Sends an IP packet to guest address `dst`, asking its MAC address with ARP if
    /// it is not known yet (the packet waits for the answer).
    fn deliver(&mut self, dst: Ipv4Addr, packet: Vec<u8>, now: Instant) {
        if let Some(mac) = self.neighbors.get(&dst) {
            let frame = wire::eth_frame(*mac, GATEWAY_MAC, wire::ETHERTYPE_IPV4, &packet);
            return self.push(frame);
        }
        if self.unresolved.len() >= MAX_UNRESOLVED {
            return;
        }
        let asked = self.unresolved.iter().any(|u| u.ip == dst);
        self.unresolved.push_back(Unresolved {
            ip: dst,
            packet,
            since: now,
        });
        if !asked {
            let req = wire::arp_packet(&wire::Arp {
                op: wire::ARP_REQUEST,
                sender_mac: GATEWAY_MAC,
                sender_ip: GATEWAY_ADDR,
                target_mac: [0; 6],
                target_ip: dst,
            });
            self.push(wire::eth_frame(
                BROADCAST_MAC,
                GATEWAY_MAC,
                wire::ETHERTYPE_ARP,
                &req,
            ));
        }
    }

    fn udp_to_guest(&mut self, src: Ipv4Addr, src_port: u16, guest: SocketAddrV4, data: &[u8], now: Instant) {
        let udp = wire::udp_datagram(src, *guest.ip(), src_port, guest.port(), data);
        let packet = self.ip_packet(src, *guest.ip(), wire::PROTO_UDP, &udp);
        self.deliver(*guest.ip(), packet, now);
    }

    fn tcp_to_guest(&mut self, guest: SocketAddrV4, peer: SocketAddrV4, segs: Vec<tcp::Seg>, now: Instant) {
        for s in segs {
            let h = TcpHeader {
                src_port: peer.port(),
                dst_port: guest.port(),
                seq: s.seq,
                ack: s.ack,
                flags: s.flags,
                window: s.window,
                mss: s.mss,
            };
            let seg = wire::tcp_segment(*peer.ip(), *guest.ip(), &h, &s.payload);
            let packet = self.ip_packet(*peer.ip(), *guest.ip(), wire::PROTO_TCP, &seg);
            self.deliver(*guest.ip(), packet, now);
        }
    }

    // ---- Bookkeeping ----

    /// Flows the guest machine with MAC address `mac` has.
    fn flows_of(&self, mac: Mac) -> usize {
        let tcp = self.tcp.values().filter(|f| f.owner == Some(mac)).count();
        let udp = self.udp.values().filter(|f| f.owner == mac).count();
        let fwd = self.udp_fwd.iter().flat_map(|f| f.clients.values());
        tcp + udp + fwd.filter(|c| c.owner == Some(mac)).count()
    }

    /// Whether a new flow of `owner` (a guest's MAC address; `None` for a forwarded
    /// port's guest not heard from yet) fits the limits; counts the refusal if not.
    fn admit(&mut self, owner: Option<Mac>) -> bool {
        let full = self.flows() >= self.limits.flows
            || owner.is_some_and(|mac| self.flows_of(mac) >= self.limits.flows_per_guest);
        if full {
            self.stats.refused += 1;
        }
        !full
    }

    /// Whether host sockets may be read: not while the queue toward the guest is half
    /// full (what the host sends waits in its socket buffers meanwhile).
    fn may_read(&self) -> bool {
        self.out.len() < self.half_queue()
    }

    /// Frames queued for the guest past which host sockets are not read (at least one,
    /// so even a queue of one frame lets something through).
    fn half_queue(&self) -> usize {
        (self.limits.queue / 2).max(1)
    }

    fn new_id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    /// An initial sequence number: a keyed hash of the connection and a counter.
    fn new_iss(&mut self, guest: SocketAddrV4, peer: SocketAddrV4) -> u32 {
        self.iss_key.hash_one((guest, peer, self.next_id)) as u32
    }

    /// A port of 10.0.2.2 for the gateway's end of a forwarded connection or client,
    /// from [`FIRST_PORT`] up, not in use by another.
    fn new_port(&mut self) -> Option<u16> {
        for _ in FIRST_PORT..=u16::MAX {
            let p = self.next_port;
            self.next_port = if p == u16::MAX { FIRST_PORT } else { p + 1 };
            let in_use = self
                .tcp
                .keys()
                .any(|(_, peer)| *peer.ip() == GATEWAY_ADDR && peer.port() == p)
                || self.udp_fwd.iter().any(|f| f.clients.contains_key(&p));
            if !in_use {
                return Some(p);
            }
        }
        None
    }
}

/// Where a guest's packet for `ip` goes on the host: 10.0.2.2 is the host's
/// loopback; other addresses of the guest network, and loopback, broadcast,
/// multicast or unspecified addresses, go nowhere.
pub fn host_addr(ip: Ipv4Addr) -> Option<Ipv4Addr> {
    if ip == GATEWAY_ADDR {
        return Some(Ipv4Addr::LOCALHOST);
    }
    let local = ip.octets()[..3] == NETWORK.octets()[..3];
    if local || ip.is_loopback() || ip.is_broadcast() || ip.is_multicast() || ip.is_unspecified() {
        return None;
    }
    Some(ip)
}

/// Errors after which a UDP socket can be read again: on Windows a datagram that met
/// a closed port makes the next receive fail with "connection reset".
fn retry_udp(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::ConnectionReset | io::ErrorKind::Interrupted
    )
}

#[cfg(test)]
mod tests;
