//! The DHCP server (RFC 2131, options from RFC 2132): one lease per MAC address, from
//! [`crate::FIRST_LEASE`] up, that never expires while the gateway lives.
//!
//! A client broadcasts DISCOVER, the server OFFERs an address, the client REQUESTs it
//! and the server ACKs; each message is a BOOTP packet (a fixed 236-byte part, the
//! "magic cookie" 99.130.83.99, then options as code, length, data). Every reply
//! carries the same configuration: subnet mask, router (the gateway), DNS server and a
//! lease time of a day.

use crate::wire::{ipv4_at, Mac};
use crate::{DNS_ADDR, GATEWAY_ADDR, NETMASK};
use std::collections::HashMap;
use std::net::Ipv4Addr;

pub const SERVER_PORT: u16 = 67;
pub const CLIENT_PORT: u16 = 68;

/// The fixed part of a BOOTP packet, before the magic cookie.
const FIXED: usize = 236;
const MAGIC: [u8; 4] = [99, 130, 83, 99];
/// BOOTP's minimum message size; some clients drop shorter replies.
const MIN_MESSAGE: usize = 300;
/// What the lease time option says: one day. The gateway keeps the address for the
/// client's MAC regardless, so a client that renews always gets the same one.
pub const LEASE_SECS: u32 = 86_400;

// Message types (option 53).
pub const DISCOVER: u8 = 1;
pub const OFFER: u8 = 2;
pub const REQUEST: u8 = 3;
pub const DECLINE: u8 = 4;
pub const ACK: u8 = 5;
pub const NAK: u8 = 6;
pub const RELEASE: u8 = 7;
pub const INFORM: u8 = 8;

// Option codes.
const OPT_PAD: u8 = 0;
const OPT_SUBNET_MASK: u8 = 1;
const OPT_ROUTER: u8 = 3;
const OPT_DNS: u8 = 6;
const OPT_REQUESTED_IP: u8 = 50;
const OPT_LEASE_TIME: u8 = 51;
const OPT_MESSAGE_TYPE: u8 = 53;
const OPT_SERVER_ID: u8 = 54;
const OPT_END: u8 = 255;

/// The parts of a client's message the server looks at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub kind: u8,
    pub xid: u32,
    /// Bit 15 asks for broadcast replies.
    pub flags: u16,
    pub ciaddr: Ipv4Addr,
    pub giaddr: Ipv4Addr,
    pub chaddr: Mac,
    pub requested: Option<Ipv4Addr>,
    pub server_id: Option<Ipv4Addr>,
}

/// Parses a BOOTREQUEST from a DHCP client; `None` for anything else.
pub fn parse(p: &[u8]) -> Option<Request> {
    // op 1 (request), htype 1 (Ethernet), hlen 6.
    if p.len() < FIXED + MAGIC.len() || p[0] != 1 || p[1] != 1 || p[2] != 6 || p[FIXED..FIXED + 4] != MAGIC {
        return None;
    }
    let mut req = Request {
        kind: 0,
        xid: crate::wire::be32(p, 4),
        flags: crate::wire::be16(p, 10),
        ciaddr: ipv4_at(p, 12),
        giaddr: ipv4_at(p, 24),
        chaddr: p[28..34].try_into().unwrap(),
        requested: None,
        server_id: None,
    };
    let mut opts = &p[FIXED + 4..];
    while let [code, rest @ ..] = opts {
        match *code {
            OPT_END => break,
            OPT_PAD => opts = rest,
            code => {
                let (&len, rest) = rest.split_first()?;
                let data = rest.get(..len as usize)?;
                match (code, data) {
                    (OPT_MESSAGE_TYPE, &[t]) => req.kind = t,
                    (OPT_REQUESTED_IP, &[a, b, c, d]) => req.requested = Some(Ipv4Addr::new(a, b, c, d)),
                    (OPT_SERVER_ID, &[a, b, c, d]) => req.server_id = Some(Ipv4Addr::new(a, b, c, d)),
                    _ => {}
                }
                opts = &rest[len as usize..];
            }
        }
    }
    (req.kind != 0).then_some(req)
}

/// A reply and where it goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reply {
    pub message: Vec<u8>,
    /// Ethernet destination: the client, or broadcast.
    pub mac: Mac,
    /// IP destination: the offered address, or 255.255.255.255.
    pub ip: Ipv4Addr,
}

/// The leases, and the server's answers.
#[derive(Debug, Default)]
pub struct Server {
    leases: HashMap<Mac, Ipv4Addr>,
}

impl Server {
    /// The address leased to `mac`, if any.
    pub fn lease_of(&self, mac: &Mac) -> Option<Ipv4Addr> {
        self.leases.get(mac).copied()
    }

    /// Every lease, in address order.
    pub fn leases(&self) -> Vec<(Mac, Ipv4Addr)> {
        let mut all: Vec<(Mac, Ipv4Addr)> = self.leases.iter().map(|(m, ip)| (*m, *ip)).collect();
        all.sort_by_key(|(_, ip)| *ip);
        all
    }

    /// `mac`'s lease, or the address it would get. `None` when the pool is used up.
    fn lease_for(&self, mac: Mac) -> Option<Ipv4Addr> {
        if let Some(ip) = self.leases.get(&mac) {
            return Some(*ip);
        }
        let first = u32::from(crate::FIRST_LEASE);
        let last = u32::from(crate::LAST_LEASE);
        (first..=last)
            .map(Ipv4Addr::from)
            .find(|ip| !self.leases.values().any(|a| a == ip))
    }

    /// The answer to a client's message, if it gets one. A group address (multicast or
    /// broadcast) is no card's, so it gets nothing.
    pub fn handle(&mut self, req: &Request) -> Option<Reply> {
        if req.chaddr[0] & 1 != 0 {
            return None;
        }
        match req.kind {
            DISCOVER => {
                let ip = self.lease_for(req.chaddr)?;
                self.leases.insert(req.chaddr, ip);
                Some(self.reply(req, OFFER, ip))
            }
            REQUEST => {
                // A REQUEST naming another server's offer is not for us.
                if req.server_id.is_some_and(|s| s != GATEWAY_ADDR) {
                    return None;
                }
                let Some(ip) = self.lease_for(req.chaddr) else {
                    return Some(self.reply(req, NAK, Ipv4Addr::UNSPECIFIED));
                };
                // The address asked for: option 50 while selecting or rebooting,
                // `ciaddr` while renewing. A NAK leaves the pool as it was.
                let wanted = req.requested.unwrap_or(req.ciaddr);
                if wanted != ip && wanted != Ipv4Addr::UNSPECIFIED {
                    return Some(self.reply(req, NAK, Ipv4Addr::UNSPECIFIED));
                }
                self.leases.insert(req.chaddr, ip);
                Some(self.reply(req, ACK, ip))
            }
            // Configuration only, for a client that has its address already.
            INFORM => Some(self.reply(req, ACK, Ipv4Addr::UNSPECIFIED)),
            // RELEASE and DECLINE need no answer; the lease stays the client's.
            _ => None,
        }
    }

    fn reply(&self, req: &Request, kind: u8, yiaddr: Ipv4Addr) -> Reply {
        let mut m = vec![0u8; FIXED];
        m[0] = 2; // BOOTREPLY
        m[1] = 1;
        m[2] = 6;
        m[4..8].copy_from_slice(&req.xid.to_be_bytes());
        m[10..12].copy_from_slice(&req.flags.to_be_bytes());
        // An INFORM's ACK (no address handed out) echoes the client's own.
        if kind == ACK && yiaddr.is_unspecified() {
            m[12..16].copy_from_slice(&req.ciaddr.octets());
        }
        m[16..20].copy_from_slice(&yiaddr.octets());
        if kind != NAK {
            m[20..24].copy_from_slice(&GATEWAY_ADDR.octets()); // siaddr
        }
        m[24..28].copy_from_slice(&req.giaddr.octets());
        m[28..34].copy_from_slice(&req.chaddr);
        m.extend_from_slice(&MAGIC);
        let mut opt = |code: u8, data: &[u8]| {
            m.push(code);
            m.push(data.len() as u8);
            m.extend_from_slice(data);
        };
        opt(OPT_MESSAGE_TYPE, &[kind]);
        opt(OPT_SERVER_ID, &GATEWAY_ADDR.octets());
        if kind != NAK {
            if !yiaddr.is_unspecified() {
                opt(OPT_LEASE_TIME, &LEASE_SECS.to_be_bytes());
            }
            opt(OPT_SUBNET_MASK, &NETMASK.octets());
            opt(OPT_ROUTER, &GATEWAY_ADDR.octets());
            opt(OPT_DNS, &DNS_ADDR.octets());
        }
        m.push(OPT_END);
        if m.len() < MIN_MESSAGE {
            m.resize(MIN_MESSAGE, OPT_PAD);
        }
        // RFC 2131 section 4.1: broadcast when the client asks for it (it cannot
        // receive unicast before it has an address) and for NAKs; otherwise unicast
        // to the client's MAC and the address it is getting.
        let broadcast = req.flags & 0x8000 != 0 || kind == NAK || yiaddr.is_unspecified();
        let (mac, ip) = if broadcast {
            if kind == ACK && yiaddr.is_unspecified() && !req.ciaddr.is_unspecified() {
                (req.chaddr, req.ciaddr) // an INFORM's answer goes to the address it has
            } else {
                (crate::wire::BROADCAST_MAC, Ipv4Addr::BROADCAST)
            }
        } else {
            (req.chaddr, yiaddr)
        };
        Reply { message: m, mac, ip }
    }
}

/// Builds a client message, for tests and for anyone who wants to drive the server:
/// `kind` from `chaddr`, transaction `xid`, asking for `requested` if given.
pub fn client_message(kind: u8, xid: u32, chaddr: Mac, requested: Option<Ipv4Addr>) -> Vec<u8> {
    let mut m = vec![0u8; FIXED];
    m[0] = 1;
    m[1] = 1;
    m[2] = 6;
    m[4..8].copy_from_slice(&xid.to_be_bytes());
    m[28..34].copy_from_slice(&chaddr);
    m.extend_from_slice(&MAGIC);
    m.extend_from_slice(&[OPT_MESSAGE_TYPE, 1, kind]);
    if let Some(ip) = requested {
        m.extend_from_slice(&[OPT_REQUESTED_IP, 4]);
        m.extend_from_slice(&ip.octets());
    }
    m.push(OPT_END);
    m
}

/// The options of a server's reply, by code, for tests and clients.
pub fn reply_options(m: &[u8]) -> Option<HashMap<u8, Vec<u8>>> {
    if m.len() < FIXED + 4 || m[FIXED..FIXED + 4] != MAGIC {
        return None;
    }
    let mut out = HashMap::new();
    let mut opts = &m[FIXED + 4..];
    while let [code, rest @ ..] = opts {
        match *code {
            OPT_END => break,
            OPT_PAD => opts = rest,
            code => {
                let (&len, rest) = rest.split_first()?;
                out.insert(code, rest.get(..len as usize)?.to_vec());
                opts = &rest[len as usize..];
            }
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAC1: Mac = [0x52, 0x54, 0, 0x12, 0x34, 0x56];
    const MAC2: Mac = [0x02, 0xca, 0, 0, 0, 2];

    #[test]
    fn discover_request_ack() {
        let mut s = Server::default();
        let req = parse(&client_message(DISCOVER, 0x1234, MAC1, None)).unwrap();
        let offer = s.handle(&req).unwrap();
        assert_eq!((offer.mac, offer.ip), (MAC1, crate::FIRST_LEASE));
        assert!(offer.message.len() >= MIN_MESSAGE);
        assert_eq!(&offer.message[4..8], &0x1234u32.to_be_bytes());
        assert_eq!(ipv4_at(&offer.message, 16), crate::FIRST_LEASE);
        let o = reply_options(&offer.message).unwrap();
        assert_eq!(o[&OPT_MESSAGE_TYPE], [OFFER]);
        assert_eq!(o[&OPT_SERVER_ID], GATEWAY_ADDR.octets());
        assert_eq!(o[&OPT_ROUTER], GATEWAY_ADDR.octets());
        assert_eq!(o[&OPT_DNS], DNS_ADDR.octets());
        assert_eq!(o[&OPT_SUBNET_MASK], [255, 255, 255, 0]);
        assert_eq!(o[&OPT_LEASE_TIME], LEASE_SECS.to_be_bytes());

        let req = parse(&client_message(REQUEST, 0x1234, MAC1, Some(crate::FIRST_LEASE))).unwrap();
        let ack = s.handle(&req).unwrap();
        assert_eq!(reply_options(&ack.message).unwrap()[&OPT_MESSAGE_TYPE], [ACK]);
        assert_eq!(s.lease_of(&MAC1), Some(crate::FIRST_LEASE));

        // Another client gets the next address; the first keeps its own.
        let req = parse(&client_message(DISCOVER, 1, MAC2, None)).unwrap();
        assert_eq!(s.handle(&req).unwrap().ip, Ipv4Addr::new(10, 0, 2, 16));
        let req = parse(&client_message(DISCOVER, 2, MAC1, None)).unwrap();
        assert_eq!(s.handle(&req).unwrap().ip, crate::FIRST_LEASE);
        // An offer holds its address for the card, in address order.
        let second = Ipv4Addr::new(10, 0, 2, 16);
        assert_eq!(s.leases(), [(MAC1, crate::FIRST_LEASE), (MAC2, second)]);
    }

    #[test]
    fn wrong_address_is_naked_and_other_servers_ignored() {
        let mut s = Server::default();
        let req = parse(&client_message(
            REQUEST,
            9,
            MAC1,
            Some(Ipv4Addr::new(10, 0, 2, 99)),
        ))
        .unwrap();
        let nak = s.handle(&req).unwrap();
        assert_eq!(reply_options(&nak.message).unwrap()[&OPT_MESSAGE_TYPE], [NAK]);
        assert_eq!(
            (nak.mac, nak.ip),
            (crate::wire::BROADCAST_MAC, Ipv4Addr::BROADCAST)
        );
        assert_eq!(s.lease_of(&MAC1), None, "a NAK takes no address from the pool");

        let mut m = client_message(REQUEST, 9, MAC1, Some(crate::FIRST_LEASE));
        m.pop();
        m.extend_from_slice(&[OPT_SERVER_ID, 4, 192, 168, 1, 1, OPT_END]);
        assert_eq!(s.handle(&parse(&m).unwrap()), None);
    }

    #[test]
    fn broadcast_flag_is_honoured() {
        let mut s = Server::default();
        let mut m = client_message(DISCOVER, 5, MAC1, None);
        m[10] = 0x80;
        let offer = s.handle(&parse(&m).unwrap()).unwrap();
        assert_eq!(
            (offer.mac, offer.ip),
            (crate::wire::BROADCAST_MAC, Ipv4Addr::BROADCAST)
        );
    }

    #[test]
    fn malformed_messages_are_ignored() {
        let good = client_message(DISCOVER, 1, MAC1, None);
        for n in 0..good.len() - 1 {
            // Cut short anywhere: no message type, or a truncated option.
            assert_eq!(parse(&good[..n]), None, "{n} bytes");
        }
        let mut bad = good.clone();
        bad[FIXED] = 0; // magic cookie
        assert_eq!(parse(&bad), None);
        let mut reply = good.clone();
        reply[0] = 2; // a BOOTREPLY
        assert_eq!(parse(&reply), None);
        // An option whose length runs past the end.
        let mut long = good[..FIXED + 4].to_vec();
        long.extend_from_slice(&[OPT_MESSAGE_TYPE, 9, DISCOVER]);
        assert_eq!(parse(&long), None);
    }

    #[test]
    fn group_addresses_get_no_lease() {
        let mut s = Server::default();
        for mac in [crate::wire::BROADCAST_MAC, [0x01, 0, 0x5e, 0, 0, 1]] {
            for kind in [DISCOVER, REQUEST] {
                let req = parse(&client_message(kind, 1, mac, None)).unwrap();
                assert_eq!(s.handle(&req), None);
            }
            assert_eq!(s.lease_of(&mac), None);
        }
    }

    #[test]
    fn pool_runs_out() {
        let mut s = Server::default();
        let size = u32::from(crate::LAST_LEASE) - u32::from(crate::FIRST_LEASE) + 1;
        for i in 0..size {
            let mac = [2, 0, 0, 0, (i >> 8) as u8, i as u8];
            assert!(s
                .handle(&parse(&client_message(DISCOVER, i, mac, None)).unwrap())
                .is_some());
        }
        let late = parse(&client_message(DISCOVER, 0, [2, 9, 9, 9, 9, 9], None)).unwrap();
        assert_eq!(s.handle(&late), None);
    }
}
