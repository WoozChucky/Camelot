//! The DNS server at [`crate::DNS_ADDR`] (RFC 1035): it answers A queries itself, with
//! the IPv4 addresses the host's resolver gives for the name (`std::net::ToSocketAddrs`,
//! the same lookup any program on the host makes), rather than forwarding packets to
//! the host's DNS servers. That keeps it to std (there is no portable way to learn the
//! host's DNS servers) and gives the guest whatever the host sees, hosts-file entries
//! included.
//!
//! A message is a 12-byte header (id, flags, four counts), then the questions and
//! the records. A query has one question: a name as length-prefixed labels ending in
//! a zero byte, then type and class.

use crate::wire::be16;
use std::net::Ipv4Addr;

pub const PORT: u16 = 53;
const HEADER: usize = 12;
/// A DNS message over UDP is at most 512 bytes unless both ends agree on more (EDNS),
/// which this server does not offer.
pub const MAX_MESSAGE: usize = 512;
/// The time to live given with every answer, in seconds.
pub const TTL: u32 = 60;

pub const TYPE_A: u16 = 1;
pub const TYPE_AAAA: u16 = 28;
pub const CLASS_IN: u16 = 1;

// Response codes.
pub const NOERROR: u8 = 0;
pub const FORMERR: u8 = 1;
pub const NXDOMAIN: u8 = 3;
pub const NOTIMP: u8 = 4;

/// A parsed query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Query {
    pub id: u16,
    /// Recursion desired, copied into the answer.
    pub rd: bool,
    /// The question as it came, name, type and class, to repeat in the answer.
    pub question: Vec<u8>,
    pub qtype: u16,
    pub qclass: u16,
    /// The name as text (`example.com`), when it is one a resolver can take: labels of
    /// letters, digits, `-` and `_`. `None` for anything else.
    pub name: Option<String>,
}

/// What to do with a message sent to the server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Parsed {
    /// A standard query with one question.
    Query(Query),
    /// Answer at once with this message (an error).
    Reply(Vec<u8>),
    /// Not a query, or too broken to answer: drop it.
    Drop,
}

pub fn parse(m: &[u8]) -> Parsed {
    if m.len() < HEADER {
        return Parsed::Drop;
    }
    let id = be16(m, 0);
    let flags = be16(m, 2);
    // QR set: a response, not for a server.
    if flags & 0x8000 != 0 {
        return Parsed::Drop;
    }
    let rd = flags & 0x0100 != 0;
    let error = |rcode| Parsed::Reply(header(id, rd, rcode, 0, 0).to_vec());
    // Opcode 0 is a standard query; inverse queries and status are not served.
    if (flags >> 11) & 0xf != 0 {
        return error(NOTIMP);
    }
    if be16(m, 4) != 1 {
        return error(FORMERR);
    }
    // The name: labels of 1 to 63 bytes, at most 255 bytes in all with the lengths.
    // Compression pointers have no place in a query's only question.
    let mut at = HEADER;
    let mut labels: Vec<&[u8]> = Vec::new();
    loop {
        let Some(&len) = m.get(at) else {
            return error(FORMERR);
        };
        at += 1;
        if len == 0 {
            break;
        }
        if len > 63 || at + len as usize > m.len() || at + len as usize - HEADER > 255 {
            return error(FORMERR);
        }
        labels.push(&m[at..at + len as usize]);
        at += len as usize;
    }
    if at + 4 > m.len() {
        return error(FORMERR);
    }
    let qtype = be16(m, at);
    let qclass = be16(m, at + 2);
    at += 4;
    let ok_label = |l: &&[u8]| {
        l.iter()
            .all(|b| b.is_ascii_alphanumeric() || *b == b'-' || *b == b'_')
    };
    let name = (!labels.is_empty() && labels.iter().all(ok_label)).then(|| {
        let parts: Vec<String> = labels
            .iter()
            .map(|l| String::from_utf8_lossy(l).into_owned())
            .collect();
        parts.join(".")
    });
    Parsed::Query(Query {
        id,
        rd,
        question: m[HEADER..at].to_vec(),
        qtype,
        qclass,
        name,
    })
}

/// A header: QR (a response), RD as asked, RA (recursion available: the host's
/// resolver does it), the response code and the counts.
fn header(id: u16, rd: bool, rcode: u8, qdcount: u16, ancount: u16) -> [u8; HEADER] {
    let flags: u16 = 0x8000 | (rd as u16) << 8 | 0x0080 | rcode as u16;
    let mut h = [0; HEADER];
    h[0..2].copy_from_slice(&id.to_be_bytes());
    h[2..4].copy_from_slice(&flags.to_be_bytes());
    h[4..6].copy_from_slice(&qdcount.to_be_bytes());
    h[6..8].copy_from_slice(&ancount.to_be_bytes());
    h
}

/// The answer to `q`: the question again, then an A record for each address (as many
/// as fit in [`MAX_MESSAGE`] bytes), each naming the question's name by a pointer to
/// it (offset 12).
pub fn answer(q: &Query, rcode: u8, addrs: &[Ipv4Addr]) -> Vec<u8> {
    const RECORD: usize = 16; // pointer 2, type 2, class 2, TTL 4, length 2, address 4
    let room = MAX_MESSAGE.saturating_sub(HEADER + q.question.len()) / RECORD;
    let addrs = &addrs[..addrs.len().min(room)];
    let mut m = header(q.id, q.rd, rcode, 1, addrs.len() as u16).to_vec();
    m.extend_from_slice(&q.question);
    for a in addrs {
        m.extend_from_slice(&[0xc0, HEADER as u8]);
        m.extend_from_slice(&TYPE_A.to_be_bytes());
        m.extend_from_slice(&CLASS_IN.to_be_bytes());
        m.extend_from_slice(&TTL.to_be_bytes());
        m.extend_from_slice(&4u16.to_be_bytes());
        m.extend_from_slice(&a.octets());
    }
    m
}

/// How the server deals with a query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Plan {
    /// Answer now with this message.
    Reply(Vec<u8>),
    /// Look the name up (A records), then [`answer`].
    Resolve(String),
}

/// What a well-formed query needs: an A query for a usable name is looked up; an
/// AAAA or any other type of record gets an empty answer (the guest network is IPv4
/// only, and the host's resolver gives addresses only); a name a resolver cannot take
/// does not exist; a class other than IN is not served.
pub fn plan(q: &Query) -> Plan {
    if q.qclass != CLASS_IN {
        return Plan::Reply(answer(q, NOTIMP, &[]));
    }
    match (&q.name, q.qtype) {
        (None, _) => Plan::Reply(answer(q, NXDOMAIN, &[])),
        (Some(name), TYPE_A) => Plan::Resolve(name.clone()),
        _ => Plan::Reply(answer(q, NOERROR, &[])),
    }
}

/// Builds a query for `name` (dotted), for tests and clients.
pub fn query(id: u16, name: &str, qtype: u16) -> Vec<u8> {
    let mut m = vec![0u8; HEADER];
    m[0..2].copy_from_slice(&id.to_be_bytes());
    m[2..4].copy_from_slice(&0x0100u16.to_be_bytes()); // RD
    m[4..6].copy_from_slice(&1u16.to_be_bytes());
    for label in name.split('.').filter(|l| !l.is_empty()) {
        m.push(label.len() as u8);
        m.extend_from_slice(label.as_bytes());
    }
    m.push(0);
    m.extend_from_slice(&qtype.to_be_bytes());
    m.extend_from_slice(&CLASS_IN.to_be_bytes());
    m
}

/// The response code and A records of an answer, for tests and clients. Expects the
/// layout [`answer`] writes (one question, records naming it by pointer).
pub fn read_answer(m: &[u8]) -> Option<(u16, u8, Vec<Ipv4Addr>)> {
    if m.len() < HEADER || be16(m, 2) & 0x8000 == 0 {
        return None;
    }
    let rcode = (be16(m, 2) & 0xf) as u8;
    let ancount = be16(m, 6) as usize;
    let mut at = HEADER;
    if be16(m, 4) == 1 {
        while *m.get(at)? != 0 {
            at += 1 + m[at] as usize;
        }
        at += 5;
    }
    let mut addrs = Vec::new();
    for _ in 0..ancount {
        let rec = m.get(at..at + 16)?;
        if be16(rec, 2) == TYPE_A && be16(rec, 10) == 4 {
            addrs.push(Ipv4Addr::new(rec[12], rec[13], rec[14], rec[15]));
        }
        at += 12 + be16(rec, 10) as usize;
    }
    Some((be16(m, 0), rcode, addrs))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(m: &[u8]) -> Query {
        match parse(m) {
            Parsed::Query(q) => q,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_query_round_trip() {
        let q = parsed(&query(0x4242, "example.com", TYPE_A));
        assert_eq!((q.id, q.rd, q.qtype, q.qclass), (0x4242, true, TYPE_A, CLASS_IN));
        assert_eq!(q.name.as_deref(), Some("example.com"));
        assert_eq!(plan(&q), Plan::Resolve("example.com".into()));
        let a = answer(
            &q,
            NOERROR,
            &[Ipv4Addr::new(93, 184, 216, 34), Ipv4Addr::new(1, 2, 3, 4)],
        );
        assert_eq!(
            read_answer(&a),
            Some((
                0x4242,
                NOERROR,
                vec![Ipv4Addr::new(93, 184, 216, 34), Ipv4Addr::new(1, 2, 3, 4)]
            ))
        );
        // RD copied, RA set.
        assert_eq!(be16(&a, 2), 0x8180);
    }

    #[test]
    fn other_types_and_names() {
        let q = parsed(&query(1, "example.com", TYPE_AAAA));
        assert_eq!(plan(&q), Plan::Reply(answer(&q, NOERROR, &[])));
        let q = parsed(&query(1, "bad name.com", TYPE_A));
        assert_eq!(q.name, None);
        match plan(&q) {
            Plan::Reply(m) => assert_eq!(read_answer(&m).unwrap().1, NXDOMAIN),
            other => panic!("{other:?}"),
        }
        let mut chaos = query(1, "version.bind", 16);
        let n = chaos.len();
        chaos[n - 1] = 3; // class CH
        match plan(&parsed(&chaos)) {
            Plan::Reply(m) => assert_eq!(read_answer(&m).unwrap().1, NOTIMP),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn answers_fit_in_512_bytes() {
        let q = parsed(&query(1, "many.example", TYPE_A));
        let addrs: Vec<Ipv4Addr> = (0..100).map(|i| Ipv4Addr::new(10, 0, 0, i)).collect();
        let a = answer(&q, NOERROR, &addrs);
        assert!(a.len() <= MAX_MESSAGE);
        assert_eq!(
            read_answer(&a).unwrap().2.len(),
            (MAX_MESSAGE - 12 - q.question.len()) / 16
        );
    }

    #[test]
    fn broken_queries() {
        let good = query(7, "a.example", TYPE_A);
        for n in 0..good.len() {
            match parse(&good[..n]) {
                Parsed::Drop => assert!(n < HEADER),
                Parsed::Reply(m) => assert_eq!(read_answer(&m).unwrap().1, FORMERR, "{n} bytes"),
                Parsed::Query(q) => panic!("{n} bytes parsed: {q:?}"),
            }
        }
        // A response is not answered.
        let mut resp = good.clone();
        resp[2] |= 0x80;
        assert_eq!(parse(&resp), Parsed::Drop);
        // Two questions; an inverse query; a label over 63 bytes.
        let mut two = good.clone();
        two[5] = 2;
        assert!(matches!(parse(&two), Parsed::Reply(m) if read_answer(&m).unwrap().1 == FORMERR));
        let mut iquery = good.clone();
        iquery[2] |= 1 << 3;
        assert!(matches!(parse(&iquery), Parsed::Reply(m) if read_answer(&m).unwrap().1 == NOTIMP));
        let long = query(7, &"x".repeat(64), TYPE_A);
        assert!(matches!(parse(&long), Parsed::Reply(m) if read_answer(&m).unwrap().1 == FORMERR));
        // A compression pointer as a label length.
        let mut ptr = good[..HEADER].to_vec();
        ptr.extend_from_slice(&[0xc0, 0x0c, 0, 1, 0, 1]);
        assert!(matches!(parse(&ptr), Parsed::Reply(_)));
    }
}
