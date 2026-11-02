//! Addresses as the platform's command lines write them: where a server listens
//! (`--listen`), and where a VM's disk lives (a path, or `lake://addr/name`).

use crate::{validate_name, DiskSource, DEFAULT_LAKE_ADDR};
use std::fmt;
use std::io;
use std::net::{IpAddr, SocketAddr, ToSocketAddrs};

/// The port of a `host:port` default address such as [`DEFAULT_LAKE_ADDR`].
pub fn default_port(addr: &str) -> u16 {
    addr.parse::<SocketAddr>()
        .expect("default addresses parse")
        .port()
}

/// Parses a server's `--listen` value: `host:port`, or just a host or IP address (then
/// `default_port`). IPv6 addresses with a port go in brackets: `[::1]:7400`.
pub fn parse_listen(s: &str, default_port: u16) -> Result<SocketAddr, String> {
    if let Ok(addr) = s.parse::<SocketAddr>() {
        return Ok(addr);
    }
    if let Ok(ip) = s.parse::<IpAddr>() {
        return Ok(SocketAddr::new(ip, default_port));
    }
    let resolved = if s.is_empty() || s.contains(char::is_whitespace) {
        Err(io::ErrorKind::InvalidInput.into())
    } else if s.contains(':') {
        s.to_socket_addrs()
    } else {
        (s, default_port).to_socket_addrs()
    };
    match resolved.map(|mut addrs| addrs.next()) {
        Ok(Some(addr)) => Ok(addr),
        _ => Err(format!(
            "'{s}' is not an address to listen on (examples: 127.0.0.1:{default_port}, 0.0.0.0, \
             localhost:{})",
            default_port.wrapping_add(100)
        )),
    }
}

/// What a disk argument starts with when the disk is an image on a Lake server.
pub const LAKE_SCHEME: &str = "lake://";

impl DiskSource {
    /// Parses a disk argument: `lake://addr/name` is image `name` on the Lake server at
    /// `addr` (`host:port`, or a host alone for Lake's default port 7401; IPv6 in
    /// brackets), anything else the path of an image file. The name must be a valid
    /// disk name ([`validate_name`]); the address is not resolved here.
    pub fn parse(s: &str) -> Result<DiskSource, String> {
        let Some(rest) = s.strip_prefix(LAKE_SCHEME) else {
            if s.is_empty() {
                return Err("empty disk path".into());
            }
            return Ok(DiskSource::File(s.to_string()));
        };
        let bad =
            |why: &str| format!("'{s}' is not a Lake disk ({why}; example: lake://127.0.0.1:7401/root)");
        let Some((host, name)) = rest.rsplit_once('/') else {
            return Err(bad("no '/name' after the address"));
        };
        validate_name(name).map_err(|e| bad(&e))?;
        let port_ok =
            |p: &str| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()) && p.parse::<u16>().is_ok();
        let default = default_port(DEFAULT_LAKE_ADDR);
        let addr = if let Some(v6) = host.strip_prefix('[') {
            match v6.split_once(']') {
                Some((ip, "")) if !ip.is_empty() => format!("{host}:{default}"),
                Some((ip, port)) if !ip.is_empty() && port.strip_prefix(':').is_some_and(port_ok) => {
                    host.to_string()
                }
                _ => return Err(bad("a bad address")),
            }
        } else {
            match host.split_once(':') {
                None if !host.is_empty() => format!("{host}:{default}"),
                Some((h, port)) if !h.is_empty() && port_ok(port) => host.to_string(),
                _ => return Err(bad("a bad address; IPv6 addresses go in brackets")),
            }
        };
        if addr.contains(['/', '\\']) || addr.contains(char::is_whitespace) {
            return Err(bad("a bad address"));
        }
        Ok(DiskSource::Lake {
            addr,
            name: name.to_string(),
        })
    }
}

/// The disk argument [`DiskSource::parse`] reads back: the path, or `lake://addr/name`.
impl fmt::Display for DiskSource {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            DiskSource::File(path) => write!(f, "{path}"),
            DiskSource::Lake { addr, name } => write!(f, "{LAKE_SCHEME}{addr}/{name}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listen_addresses() {
        let p = |s| parse_listen(s, 7400);
        assert_eq!(p("127.0.0.1:7400").unwrap(), "127.0.0.1:7400".parse().unwrap());
        assert_eq!(p("0.0.0.0").unwrap(), "0.0.0.0:7400".parse().unwrap());
        assert_eq!(parse_listen("::1", 7401).unwrap(), "[::1]:7401".parse().unwrap());
        assert_eq!(p("[::]:9000").unwrap(), "[::]:9000".parse().unwrap());
        assert_eq!(p("localhost:7500").unwrap().port(), 7500);
        assert!(p("localhost").unwrap().ip().is_loopback());
        assert_eq!(parse_listen("localhost", 7401).unwrap().port(), 7401);
        for bad in ["", "127.0.0.1:", "127.0.0.1:99999", "host name with spaces", ":"] {
            assert!(p(bad).is_err(), "{bad}");
        }
        assert!(p("").unwrap_err().contains("127.0.0.1:7400"));
    }

    #[test]
    fn disk_arguments() {
        let lake = |addr: &str, name: &str| DiskSource::Lake {
            addr: addr.into(),
            name: name.into(),
        };
        for (arg, source) in [
            ("disk.img", DiskSource::File("disk.img".into())),
            (
                "C:\\images\\root.img",
                DiskSource::File("C:\\images\\root.img".into()),
            ),
            ("lake.img", DiskSource::File("lake.img".into())),
            ("lake://127.0.0.1:7401/root", lake("127.0.0.1:7401", "root")),
            ("lake://storage:9000/vm-1.img", lake("storage:9000", "vm-1.img")),
            ("lake://storage/root", lake("storage:7401", "root")),
            ("lake://[::1]:7500/root", lake("[::1]:7500", "root")),
            ("lake://[::1]/root", lake("[::1]:7401", "root")),
        ] {
            assert_eq!(DiskSource::parse(arg).unwrap(), source, "{arg}");
        }
        // Display gives back what parses to the same source.
        for arg in ["disk.img", "lake://127.0.0.1:7401/root", "lake://[::1]:7500/a_b"] {
            assert_eq!(DiskSource::parse(arg).unwrap().to_string(), arg);
        }
        for bad in [
            "",
            "lake://",
            "lake://host",
            "lake:///root",
            "lake://host/",
            "lake://host/Root",
            "lake://host/../etc",
            "lake://host/con",
            "lake://host/123",
            "lake://host:/root",
            "lake://host:port/root",
            "lake://host:99999/root",
            "lake://::1/root",
            "lake://[::1/root",
            "lake://[]/root",
            "lake://[::1]x/root",
            "lake://a/b/root",
            "lake://a b/root",
        ] {
            assert!(DiskSource::parse(bad).is_err(), "{bad}");
        }
    }
}
