//! Host port forwarding rules: `hostfwd=[tcp|udp]:[HOSTADDR]:HOSTPORT-[GUESTADDR]:GUESTPORT`,
//! QEMU's syntax, so `hostfwd=tcp::8080-:80` (or `tcp:8080-:80`) lets a browser on the
//! host open `http://127.0.0.1:8080` and reach a web server on the guest's port 80.

use crate::{is_guest_addr, FIRST_LEASE};
use std::fmt;
use std::net::{Ipv4Addr, SocketAddrV4};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Proto {
    Tcp,
    Udp,
}

impl fmt::Display for Proto {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Proto::Tcp => "tcp",
            Proto::Udp => "udp",
        })
    }
}

/// One forwarded port: the gateway listens on `host` and passes each connection (TCP)
/// or datagram (UDP) to `guest`, from the gateway's address.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct HostFwd {
    pub proto: Proto,
    /// Where the host side listens: 127.0.0.1 unless an address is given, so only
    /// programs on the host itself can connect. Port 0 lets the system pick one
    /// ([`crate::Gateway::hostfwd`] says which).
    pub host: SocketAddrV4,
    /// The guest's address (the first DHCP lease, 10.0.2.15, unless one is given)
    /// and port.
    pub guest: SocketAddrV4,
}

impl HostFwd {
    pub fn tcp(host_port: u16, guest_port: u16) -> HostFwd {
        HostFwd {
            proto: Proto::Tcp,
            host: SocketAddrV4::new(Ipv4Addr::LOCALHOST, host_port),
            guest: SocketAddrV4::new(FIRST_LEASE, guest_port),
        }
    }

    pub fn udp(host_port: u16, guest_port: u16) -> HostFwd {
        HostFwd {
            proto: Proto::Udp,
            ..HostFwd::tcp(host_port, guest_port)
        }
    }

    /// Parses `[tcp|udp]:[HOSTADDR]:HOSTPORT-[GUESTADDR]:GUESTPORT`; the protocol may be
    /// left out (TCP), and so may the colon before a host port without an address.
    pub fn parse(s: &str) -> Result<HostFwd, String> {
        let bad = |why: &str| {
            format!("'{s}' is not a forwarding rule ({why}; like tcp::8080-:80 or udp:127.0.0.1:5353-:53)")
        };
        let (proto, rest) = match s.split_once(':') {
            Some(("tcp", rest)) => (Proto::Tcp, rest),
            Some(("udp", rest)) => (Proto::Udp, rest),
            _ => (Proto::Tcp, s),
        };
        let (host, guest) = rest.split_once('-').ok_or_else(|| bad("no '-'"))?;
        let endpoint = |part: &str, default: Ipv4Addr, what: &str| -> Result<SocketAddrV4, String> {
            let (addr, port) = part.rsplit_once(':').unwrap_or(("", part));
            let addr = match addr {
                "" => default,
                a => a.parse().map_err(|_| bad(&format!("bad {what} address '{a}'")))?,
            };
            let port = port
                .parse()
                .map_err(|_| bad(&format!("bad {what} port '{port}'")))?;
            Ok(SocketAddrV4::new(addr, port))
        };
        let host = endpoint(host, Ipv4Addr::LOCALHOST, "host")?;
        let guest = endpoint(guest, FIRST_LEASE, "guest")?;
        if guest.port() == 0 {
            return Err(bad("the guest port cannot be 0"));
        }
        if !is_guest_addr(*guest.ip()) {
            return Err(bad(&format!(
                "{} is not a guest address in 10.0.2.0/24",
                guest.ip()
            )));
        }
        Ok(HostFwd { proto, host, guest })
    }
}

impl fmt::Display for HostFwd {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}-{}", self.proto, self.host, self.guest)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_qemu_forms() {
        let rule = |p, h: [u8; 4], hp, g: [u8; 4], gp| HostFwd {
            proto: p,
            host: SocketAddrV4::new(h.into(), hp),
            guest: SocketAddrV4::new(g.into(), gp),
        };
        let lo = [127, 0, 0, 1];
        let g15 = [10, 0, 2, 15];
        for (text, want) in [
            ("tcp::8080-:80", rule(Proto::Tcp, lo, 8080, g15, 80)),
            ("tcp:8080-:80", rule(Proto::Tcp, lo, 8080, g15, 80)),
            ("8080-:80", rule(Proto::Tcp, lo, 8080, g15, 80)),
            ("udp::5353-:53", rule(Proto::Udp, lo, 5353, g15, 53)),
            (
                "tcp:0.0.0.0:2222-10.0.2.16:22",
                rule(Proto::Tcp, [0, 0, 0, 0], 2222, [10, 0, 2, 16], 22),
            ),
            ("tcp::0-:7", rule(Proto::Tcp, lo, 0, g15, 7)),
            ("tcp::8080-80", rule(Proto::Tcp, lo, 8080, g15, 80)),
        ] {
            assert_eq!(HostFwd::parse(text), Ok(want), "{text}");
        }
        assert_eq!(HostFwd::tcp(8080, 80), rule(Proto::Tcp, lo, 8080, g15, 80));
        assert_eq!(
            HostFwd::tcp(8080, 80).to_string(),
            "tcp:127.0.0.1:8080-10.0.2.15:80"
        );
        assert_eq!(
            HostFwd::parse(&HostFwd::udp(1, 2).to_string()),
            Ok(HostFwd::udp(1, 2))
        );
    }

    #[test]
    fn rejects_bad_rules() {
        for (text, why) in [
            ("tcp:8080", "no '-'"),
            ("tcp::80x-:80", "bad host port '80x'"),
            ("tcp::8080-:", "bad guest port ''"),
            ("tcp::8080-:0", "the guest port cannot be 0"),
            ("tcp:localhost:8080-:80", "bad host address 'localhost'"),
            ("tcp::8080-192.168.1.5:80", "192.168.1.5 is not a guest address"),
            ("tcp::8080-10.0.2.2:80", "10.0.2.2 is not a guest address"),
            ("sctp::1-:2", "bad host address 'sctp:'"),
            ("tcp::70000-:80", "bad host port '70000'"),
        ] {
            let e = HostFwd::parse(text).unwrap_err();
            assert!(e.contains(why), "{text}: {e}");
        }
    }
}
