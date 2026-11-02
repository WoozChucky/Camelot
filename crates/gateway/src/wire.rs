//! The packet formats the gateway speaks, read and written byte by byte: Ethernet II,
//! ARP, IPv4, ICMP, UDP and TCP. Everything on the wire is big-endian ("network byte
//! order").
//!
//! Parsers take untrusted bytes from the guest and return `None` for anything short,
//! inconsistent or with a wrong checksum: the gateway drops such packets, as a router
//! would, and never panics on them. Builders return finished packets with every
//! length and checksum filled in.

use std::net::Ipv4Addr;

/// A MAC address, in wire order.
pub type Mac = [u8; 6];

pub const BROADCAST_MAC: Mac = [0xff; 6];

/// Ethernet II header: destination, source, EtherType.
pub const ETH_HEADER: usize = 14;
/// Longest frame (without FCS): a 1500-byte payload behind the header.
pub const MAX_FRAME: usize = 1514;
/// Largest IPv4 packet in one frame (the Ethernet MTU).
pub const MTU: usize = 1500;
pub const IPV4_HEADER: usize = 20;
pub const UDP_HEADER: usize = 8;
/// TCP header without options; ours carry at most an MSS option (4 bytes more).
pub const TCP_HEADER: usize = 20;
/// Most TCP payload in one frame: 1500 - 20 - 20.
pub const TCP_MSS: usize = MTU - IPV4_HEADER - TCP_HEADER;
/// Most UDP payload in one frame: 1500 - 20 - 8.
pub const UDP_MAX_PAYLOAD: usize = MTU - IPV4_HEADER - UDP_HEADER;

pub const ETHERTYPE_IPV4: u16 = 0x0800;
pub const ETHERTYPE_ARP: u16 = 0x0806;

pub const PROTO_ICMP: u8 = 1;
pub const PROTO_TCP: u8 = 6;
pub const PROTO_UDP: u8 = 17;

/// TCP flag bits (the low byte of the flags field).
pub mod tcp_flags {
    pub const FIN: u8 = 0x01;
    pub const SYN: u8 = 0x02;
    pub const RST: u8 = 0x04;
    pub const PSH: u8 = 0x08;
    pub const ACK: u8 = 0x10;
}

pub fn be16(b: &[u8], at: usize) -> u16 {
    u16::from_be_bytes([b[at], b[at + 1]])
}

pub fn be32(b: &[u8], at: usize) -> u32 {
    u32::from_be_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

pub fn ipv4_at(b: &[u8], at: usize) -> Ipv4Addr {
    Ipv4Addr::new(b[at], b[at + 1], b[at + 2], b[at + 3])
}

/// The Internet checksum (RFC 1071): the one's complement sum of the data as 16-bit
/// big-endian words, an odd last byte padded with a zero. `sum` adds to a running
/// total; [`finish`] folds the carries and complements it.
pub fn sum(data: &[u8], mut acc: u32) -> u32 {
    let (words, rest) = data.as_chunks::<2>();
    for w in words {
        acc += u16::from_be_bytes(*w) as u32;
        // Fold now and then so a long packet cannot overflow the accumulator.
        if acc >= 0x8000_0000 {
            acc = (acc & 0xffff) + (acc >> 16);
        }
    }
    if let [last] = rest {
        acc += (*last as u32) << 8;
    }
    acc
}

/// Folds a running [`sum`] to 16 bits and complements it: the value that goes into a
/// checksum field. Data that includes its own correct checksum finishes to 0.
pub fn finish(mut acc: u32) -> u16 {
    while acc > 0xffff {
        acc = (acc & 0xffff) + (acc >> 16);
    }
    !(acc as u16)
}

/// The sum of the pseudo-header that UDP and TCP checksums cover besides the segment
/// itself: source and destination addresses, protocol and segment length.
fn pseudo_header(src: Ipv4Addr, dst: Ipv4Addr, proto: u8, len: usize) -> u32 {
    let acc = sum(&src.octets(), 0);
    let acc = sum(&dst.octets(), acc);
    acc + proto as u32 + len as u32
}

/// An Ethernet II frame, borrowed from the bytes it was parsed from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Eth<'a> {
    pub dst: Mac,
    pub src: Mac,
    pub ethertype: u16,
    pub payload: &'a [u8],
}

pub fn parse_eth(frame: &[u8]) -> Option<Eth<'_>> {
    if frame.len() < ETH_HEADER || frame.len() > MAX_FRAME {
        return None;
    }
    Some(Eth {
        dst: frame[0..6].try_into().unwrap(),
        src: frame[6..12].try_into().unwrap(),
        ethertype: be16(frame, 12),
        payload: &frame[ETH_HEADER..],
    })
}

pub fn eth_frame(dst: Mac, src: Mac, ethertype: u16, payload: &[u8]) -> Vec<u8> {
    let mut f = Vec::with_capacity(ETH_HEADER + payload.len());
    f.extend_from_slice(&dst);
    f.extend_from_slice(&src);
    f.extend_from_slice(&ethertype.to_be_bytes());
    f.extend_from_slice(payload);
    f
}

/// ARP for IPv4 over Ethernet (RFC 826).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Arp {
    /// 1 = request, 2 = reply.
    pub op: u16,
    pub sender_mac: Mac,
    pub sender_ip: Ipv4Addr,
    pub target_mac: Mac,
    pub target_ip: Ipv4Addr,
}

pub const ARP_REQUEST: u16 = 1;
pub const ARP_REPLY: u16 = 2;
const ARP_LEN: usize = 28;

pub fn parse_arp(p: &[u8]) -> Option<Arp> {
    // Hardware type 1 (Ethernet), protocol 0x0800, address sizes 6 and 4.
    if p.len() < ARP_LEN || be16(p, 0) != 1 || be16(p, 2) != ETHERTYPE_IPV4 || p[4] != 6 || p[5] != 4 {
        return None;
    }
    Some(Arp {
        op: be16(p, 6),
        sender_mac: p[8..14].try_into().unwrap(),
        sender_ip: ipv4_at(p, 14),
        target_mac: p[18..24].try_into().unwrap(),
        target_ip: ipv4_at(p, 24),
    })
}

pub fn arp_packet(arp: &Arp) -> Vec<u8> {
    let mut p = Vec::with_capacity(ARP_LEN);
    p.extend_from_slice(&1u16.to_be_bytes());
    p.extend_from_slice(&ETHERTYPE_IPV4.to_be_bytes());
    p.extend_from_slice(&[6, 4]);
    p.extend_from_slice(&arp.op.to_be_bytes());
    p.extend_from_slice(&arp.sender_mac);
    p.extend_from_slice(&arp.sender_ip.octets());
    p.extend_from_slice(&arp.target_mac);
    p.extend_from_slice(&arp.target_ip.octets());
    p
}

/// An IPv4 packet (RFC 791). Options are skipped; `payload` is cut to the packet's
/// total length, so Ethernet padding after it is gone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ipv4<'a> {
    pub src: Ipv4Addr,
    pub dst: Ipv4Addr,
    pub proto: u8,
    pub payload: &'a [u8],
}

/// Parses and checks an IPv4 packet: version 4, a header of 20 to 60 bytes, a total
/// length that fits, a correct header checksum and no fragmentation (the gateway does
/// not reassemble, and a guest has no reason to fragment toward it).
pub fn parse_ipv4(p: &[u8]) -> Option<Ipv4<'_>> {
    if p.len() < IPV4_HEADER || p[0] >> 4 != 4 {
        return None;
    }
    let ihl = (p[0] & 0x0f) as usize * 4;
    let total = be16(p, 2) as usize;
    if ihl < IPV4_HEADER || total < ihl || total > p.len() {
        return None;
    }
    // More-fragments flag, or a fragment offset.
    if be16(p, 6) & 0x3fff != 0 {
        return None;
    }
    if finish(sum(&p[..ihl], 0)) != 0 {
        return None;
    }
    Some(Ipv4 {
        src: ipv4_at(p, 12),
        dst: ipv4_at(p, 16),
        proto: p[9],
        payload: &p[ihl..total],
    })
}

/// An IPv4 packet with a 20-byte header: TTL 64, "don't fragment", the given
/// identification, and its header checksum.
pub fn ipv4_packet(src: Ipv4Addr, dst: Ipv4Addr, proto: u8, id: u16, payload: &[u8]) -> Vec<u8> {
    let total = IPV4_HEADER + payload.len();
    let mut p = Vec::with_capacity(total);
    p.extend_from_slice(&[0x45, 0]); // version 4, 5 words of header; TOS 0
    p.extend_from_slice(&(total as u16).to_be_bytes());
    p.extend_from_slice(&id.to_be_bytes());
    p.extend_from_slice(&0x4000u16.to_be_bytes()); // DF, offset 0
    p.extend_from_slice(&[64, proto, 0, 0]); // TTL, protocol, checksum (below)
    p.extend_from_slice(&src.octets());
    p.extend_from_slice(&dst.octets());
    let csum = finish(sum(&p, 0));
    p[10..12].copy_from_slice(&csum.to_be_bytes());
    p.extend_from_slice(payload);
    p
}

/// An ICMP echo request or reply (RFC 792).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Echo<'a> {
    /// 8 = request, 0 = reply.
    pub kind: u8,
    pub ident: u16,
    pub seq: u16,
    pub data: &'a [u8],
}

pub const ICMP_ECHO_REPLY: u8 = 0;
pub const ICMP_ECHO_REQUEST: u8 = 8;

/// An ICMP echo message with a correct checksum; other ICMP messages are `None`.
pub fn parse_echo(p: &[u8]) -> Option<Echo<'_>> {
    if p.len() < 8 || (p[0] != ICMP_ECHO_REQUEST && p[0] != ICMP_ECHO_REPLY) || p[1] != 0 {
        return None;
    }
    if finish(sum(p, 0)) != 0 {
        return None;
    }
    Some(Echo {
        kind: p[0],
        ident: be16(p, 4),
        seq: be16(p, 6),
        data: &p[8..],
    })
}

pub fn echo_message(echo: &Echo) -> Vec<u8> {
    let mut p = Vec::with_capacity(8 + echo.data.len());
    p.extend_from_slice(&[echo.kind, 0, 0, 0]);
    p.extend_from_slice(&echo.ident.to_be_bytes());
    p.extend_from_slice(&echo.seq.to_be_bytes());
    p.extend_from_slice(echo.data);
    let csum = finish(sum(&p, 0));
    p[2..4].copy_from_slice(&csum.to_be_bytes());
    p
}

/// A UDP datagram (RFC 768).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Udp<'a> {
    pub src_port: u16,
    pub dst_port: u16,
    pub payload: &'a [u8],
}

/// Parses the UDP datagram in `ip`. A checksum of 0 means none was computed (allowed
/// over IPv4); any other must be right.
pub fn parse_udp<'a>(ip: &Ipv4<'a>) -> Option<Udp<'a>> {
    let p = ip.payload;
    if p.len() < UDP_HEADER {
        return None;
    }
    let len = be16(p, 4) as usize;
    if len < UDP_HEADER || len > p.len() {
        return None;
    }
    let p = &p[..len];
    if be16(p, 6) != 0 && finish(sum(p, pseudo_header(ip.src, ip.dst, PROTO_UDP, len))) != 0 {
        return None;
    }
    Some(Udp {
        src_port: be16(p, 0),
        dst_port: be16(p, 2),
        payload: &p[UDP_HEADER..],
    })
}

/// A UDP datagram with its checksum (a computed 0 is sent as 0xffff, since 0 would
/// mean "no checksum").
pub fn udp_datagram(src: Ipv4Addr, dst: Ipv4Addr, src_port: u16, dst_port: u16, payload: &[u8]) -> Vec<u8> {
    let len = UDP_HEADER + payload.len();
    let mut p = Vec::with_capacity(len);
    p.extend_from_slice(&src_port.to_be_bytes());
    p.extend_from_slice(&dst_port.to_be_bytes());
    p.extend_from_slice(&(len as u16).to_be_bytes());
    p.extend_from_slice(&[0, 0]);
    p.extend_from_slice(payload);
    let csum = match finish(sum(&p, pseudo_header(src, dst, PROTO_UDP, len))) {
        0 => 0xffff,
        c => c,
    };
    p[6..8].copy_from_slice(&csum.to_be_bytes());
    p
}

/// A TCP segment (RFC 793). Of the options only the MSS (kind 2) is read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tcp<'a> {
    pub src_port: u16,
    pub dst_port: u16,
    pub seq: u32,
    pub ack: u32,
    /// [`tcp_flags`] bits.
    pub flags: u8,
    pub window: u16,
    /// The maximum segment size option, if the segment carries one (SYNs do).
    pub mss: Option<u16>,
    pub payload: &'a [u8],
}

impl Tcp<'_> {
    pub fn has(&self, flag: u8) -> bool {
        self.flags & flag != 0
    }

    /// Sequence numbers the segment occupies: its payload, plus one each for SYN and FIN.
    pub fn seq_len(&self) -> u32 {
        self.payload.len() as u32 + self.has(tcp_flags::SYN) as u32 + self.has(tcp_flags::FIN) as u32
    }
}

/// Parses the TCP segment in `ip`; the checksum must be right.
pub fn parse_tcp<'a>(ip: &Ipv4<'a>) -> Option<Tcp<'a>> {
    let p = ip.payload;
    if p.len() < TCP_HEADER {
        return None;
    }
    let off = (p[12] >> 4) as usize * 4;
    if off < TCP_HEADER || off > p.len() {
        return None;
    }
    if finish(sum(p, pseudo_header(ip.src, ip.dst, PROTO_TCP, p.len()))) != 0 {
        return None;
    }
    // Options: kind 0 ends them, kind 1 is one byte of padding, the rest are
    // kind, length (counting both), data.
    let mut mss = None;
    let mut opts = &p[TCP_HEADER..off];
    while let [kind, rest @ ..] = opts {
        match kind {
            0 => break,
            1 => opts = rest,
            _ => {
                let len = *rest.first()? as usize;
                if len < 2 || len > opts.len() {
                    break;
                }
                if *kind == 2 && len == 4 {
                    mss = Some(be16(opts, 2));
                }
                opts = &opts[len..];
            }
        }
    }
    Some(Tcp {
        src_port: be16(p, 0),
        dst_port: be16(p, 2),
        seq: be32(p, 4),
        ack: be32(p, 8),
        flags: p[13],
        window: be16(p, 14),
        mss,
        payload: &p[off..],
    })
}

/// The fields of a TCP segment to build; see [`tcp_segment`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TcpHeader {
    pub src_port: u16,
    pub dst_port: u16,
    pub seq: u32,
    pub ack: u32,
    pub flags: u8,
    pub window: u16,
    /// Sent as an option (only on SYNs).
    pub mss: Option<u16>,
}

/// A TCP segment with its checksum.
pub fn tcp_segment(src: Ipv4Addr, dst: Ipv4Addr, h: &TcpHeader, payload: &[u8]) -> Vec<u8> {
    let opt_len = if h.mss.is_some() { 4 } else { 0 };
    let len = TCP_HEADER + opt_len + payload.len();
    let mut p = Vec::with_capacity(len);
    p.extend_from_slice(&h.src_port.to_be_bytes());
    p.extend_from_slice(&h.dst_port.to_be_bytes());
    p.extend_from_slice(&h.seq.to_be_bytes());
    p.extend_from_slice(&h.ack.to_be_bytes());
    p.push((((TCP_HEADER + opt_len) / 4) as u8) << 4);
    p.push(h.flags);
    p.extend_from_slice(&h.window.to_be_bytes());
    p.extend_from_slice(&[0, 0, 0, 0]); // checksum (below), urgent pointer
    if let Some(mss) = h.mss {
        p.extend_from_slice(&[2, 4]);
        p.extend_from_slice(&mss.to_be_bytes());
    }
    p.extend_from_slice(payload);
    let csum = finish(sum(&p, pseudo_header(src, dst, PROTO_TCP, len)));
    p[16..18].copy_from_slice(&csum.to_be_bytes());
    p
}

/// Sequence number comparisons modulo 2^32 (RFC 793 section 3.3): `a` comes before
/// `b` when it is less than 2^31 behind it.
pub fn seq_lt(a: u32, b: u32) -> bool {
    (a.wrapping_sub(b) as i32) < 0
}

pub fn seq_le(a: u32, b: u32) -> bool {
    a == b || seq_lt(a, b)
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: Ipv4Addr = Ipv4Addr::new(10, 0, 2, 15);
    const B: Ipv4Addr = Ipv4Addr::new(10, 0, 2, 2);

    #[test]
    fn checksum_of_rfc1071_example() {
        // RFC 1071 section 3: 00 01 f2 03 f4 f5 f6 f7 sums to ddf2 (complement 220d).
        let data = [0x00, 0x01, 0xf2, 0x03, 0xf4, 0xf5, 0xf6, 0xf7];
        assert_eq!(finish(sum(&data, 0)), !0xddf2);
        // Odd lengths pad with a zero byte.
        assert_eq!(finish(sum(&[0x12], 0)), !0x1200);
        assert_eq!(finish(sum(&[], 0)), 0xffff);
    }

    #[test]
    fn ipv4_round_trip_and_checks() {
        let p = ipv4_packet(A, B, PROTO_UDP, 7, b"payload");
        let ip = parse_ipv4(&p).unwrap();
        assert_eq!(
            (ip.src, ip.dst, ip.proto, ip.payload),
            (A, B, PROTO_UDP, &b"payload"[..])
        );
        // Ethernet padding after the total length is cut off.
        let mut padded = p.clone();
        padded.extend_from_slice(&[0; 10]);
        assert_eq!(parse_ipv4(&padded).unwrap().payload, b"payload");
        // A flipped bit breaks the header checksum.
        let mut bad = p.clone();
        bad[8] ^= 1;
        assert_eq!(parse_ipv4(&bad), None);
        // Truncated anywhere: never a panic.
        for n in 0..p.len() {
            assert_eq!(parse_ipv4(&p[..n]), None, "{n} bytes");
        }
        // Fragments are refused.
        let mut frag = p.clone();
        frag[6] = 0x20; // more fragments
        frag[10..12].copy_from_slice(&[0, 0]);
        let c = finish(sum(&frag[..20], 0));
        frag[10..12].copy_from_slice(&c.to_be_bytes());
        assert_eq!(parse_ipv4(&frag), None);
    }

    #[test]
    fn udp_round_trip_and_checksum() {
        let d = udp_datagram(A, B, 1234, 53, b"query");
        let ipp = ipv4_packet(A, B, PROTO_UDP, 0, &d);
        let ip = parse_ipv4(&ipp).unwrap();
        let udp = parse_udp(&ip).unwrap();
        assert_eq!(
            (udp.src_port, udp.dst_port, udp.payload),
            (1234, 53, &b"query"[..])
        );
        // A wrong checksum is dropped; no checksum (0) is accepted.
        let mut bad = d.clone();
        bad[9] ^= 0xff;
        let ipp = ipv4_packet(A, B, PROTO_UDP, 0, &bad);
        assert_eq!(parse_udp(&parse_ipv4(&ipp).unwrap()), None);
        bad[6..8].copy_from_slice(&[0, 0]);
        let ipp = ipv4_packet(A, B, PROTO_UDP, 0, &bad);
        assert!(parse_udp(&parse_ipv4(&ipp).unwrap()).is_some());
    }

    #[test]
    fn tcp_round_trip_with_mss() {
        let h = TcpHeader {
            src_port: 80,
            dst_port: 40000,
            seq: 0xdead_beef,
            ack: 7,
            flags: tcp_flags::SYN | tcp_flags::ACK,
            window: 1000,
            mss: Some(1460),
        };
        let seg = tcp_segment(A, B, &h, b"");
        let ipp = ipv4_packet(A, B, PROTO_TCP, 0, &seg);
        let tcp = parse_tcp(&parse_ipv4(&ipp).unwrap()).unwrap();
        assert_eq!(
            (
                tcp.src_port,
                tcp.dst_port,
                tcp.seq,
                tcp.ack,
                tcp.flags,
                tcp.window,
                tcp.mss
            ),
            (80, 40000, 0xdead_beef, 7, h.flags, 1000, Some(1460))
        );
        assert_eq!(tcp.seq_len(), 1);
        // A wrong checksum is dropped.
        let mut bad = seg.clone();
        bad[4] ^= 1;
        let ipp = ipv4_packet(A, B, PROTO_TCP, 0, &bad);
        assert_eq!(parse_tcp(&parse_ipv4(&ipp).unwrap()), None);
        // An option running past the header ends the options, not a panic.
        let mut odd = seg.clone();
        odd[21] = 40;
        odd[16..18].copy_from_slice(&[0, 0]);
        let c = finish(sum(&odd, pseudo_header(A, B, PROTO_TCP, odd.len())));
        odd[16..18].copy_from_slice(&c.to_be_bytes());
        let ipp = ipv4_packet(A, B, PROTO_TCP, 0, &odd);
        assert_eq!(parse_tcp(&parse_ipv4(&ipp).unwrap()).unwrap().mss, None);
    }

    #[test]
    fn sequence_comparisons_wrap() {
        assert!(seq_lt(1, 2));
        assert!(seq_lt(u32::MAX, 0));
        assert!(!seq_lt(0, u32::MAX));
        assert!(seq_le(5, 5));
    }
}
