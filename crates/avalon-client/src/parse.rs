//! What people type, as protocol values. Every front end (Round Table's flags,
//! Pendragon's forms) parses with these, so they all accept the same syntax.

use avalon_proto::{validate_name, DiskSource, NetSpec, SECTOR_SIZE};
use grail::net::MacAddr;
use std::net::{IpAddr, SocketAddr};

/// Parses a `--clock` value: a number with an optional unit, `Hz`, `kHz` or `MHz` (any
/// case; no unit = Hz), as Grail's `--clock` does: `10MHz`, `500kHz`, `1.5MHz`, `5Hz`.
pub use grail::parse_clock;

/// Parses a `--ram` value, a whole number of MiB from 1 to 2048 (Grail's `--ram`).
/// Returns bytes.
pub fn parse_ram(s: &str) -> Result<u64, String> {
    grail::parse_ram_mib(s).map(|bytes| bytes as u64)
}

/// Parses a `--harts` value, a whole number from 1 to 8 (Grail's `--harts`).
pub fn parse_harts(s: &str) -> Result<u32, String> {
    grail::parse_harts(s).map(|n| n as u32)
}

/// Parses a server address: `host:port`, or a host or IP address alone for
/// `default_port`. IPv6 addresses with a port go in brackets (`[::1]:7400`). The host
/// is not looked up here, only when connecting. Port 0 is refused: nothing can be
/// reached there.
pub fn parse_addr(s: &str, default_port: u16) -> Result<String, String> {
    let bad = || {
        format!("'{s}' is not a server address (examples: 127.0.0.1:7400, localhost, 192.168.1.5:7500, [::1]:7400)")
    };
    if s.is_empty() || s.contains(|c: char| c.is_whitespace() || c == '/') {
        return Err(bad());
    }
    if let Ok(addr) = s.parse::<SocketAddr>() {
        if addr.port() == 0 {
            return Err(bad());
        }
        return Ok(addr.to_string());
    }
    let bare = s.strip_prefix('[').and_then(|s| s.strip_suffix(']')).unwrap_or(s);
    if let Ok(ip) = bare.parse::<IpAddr>() {
        return Ok(SocketAddr::new(ip, default_port).to_string());
    }
    // A host name, with or without a port. Anything else with brackets or several
    // colons is a broken IPv6 address.
    if s.contains(['[', ']']) {
        return Err(bad());
    }
    match s.split_once(':') {
        None => Ok(format!("{s}:{default_port}")),
        Some((host, port)) if !host.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) => {
            match port.parse::<u16>() {
                Ok(1..) => Ok(s.to_string()),
                Ok(0) | Err(_) => Err(bad()),
            }
        }
        Some(_) => Err(bad()),
    }
}

/// The port of one of the protocol's default addresses (`DEFAULT_AVALON_ADDR`,
/// `DEFAULT_LAKE_ADDR`).
pub use avalon_proto::default_port;

/// Parses a disk for a VM's hardware profile: `lake://ADDR/NAME` is the image `NAME`
/// on the Lake server at `ADDR` (port 7401 when it has none), anything else the path of
/// an image file on the Avalon host (relative to Avalon's working directory).
///
/// The protocol's own parser ([`DiskSource::parse`], which Grail's `--disk` and Avalon
/// use too) reads the argument; the address is then checked as [`parse_addr`] checks
/// any server address (no port 0), and written the same way.
pub fn parse_disk_source(s: &str) -> Result<DiskSource, String> {
    match DiskSource::parse(s)? {
        DiskSource::Lake { addr, name } => {
            let addr = parse_addr(&addr, default_port(avalon_proto::DEFAULT_LAKE_ADDR))
                .map_err(|e| format!("'{s}': {e}"))?;
            Ok(DiskSource::Lake { addr, name })
        }
        file => Ok(file),
    }
}

/// Parses a disk size: a whole number with an optional unit, any case: none or `B`
/// for bytes, `K`/`KiB`, `M`/`MiB`, `G`/`GiB` (powers of 1024), or `S`/`sector(s)` for
/// 512-byte sectors (`64MiB`, `1G`, `2048s`). The size must be a non-zero whole number
/// of sectors, at most what a Grail disk can have. Returns sectors.
pub fn parse_disk_size(s: &str) -> Result<u64, String> {
    let s = s.trim();
    let bad = || format!("'{s}' is not a disk size (examples: 64MiB, 512K, 1G, 2048s)");
    let split = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    let (number, unit) = s.split_at(split);
    let unit = unit.trim_start();
    let scale: u64 = match unit.to_ascii_lowercase().as_str() {
        "" | "b" => 1,
        "k" | "kib" => 1 << 10,
        "m" | "mib" => 1 << 20,
        "g" | "gib" => 1 << 30,
        "s" | "sector" | "sectors" => SECTOR_SIZE as u64,
        _ => return Err(bad()),
    };
    let n: u64 = number.parse().map_err(|_| bad())?;
    let too_big = || format!("{s} is too big (at most {} sectors)", grail::block::MAX_SECTORS);
    let bytes = n.checked_mul(scale).ok_or_else(too_big)?;
    if bytes == 0 || !bytes.is_multiple_of(SECTOR_SIZE as u64) {
        return Err(format!(
            "{s} is not a whole number of {SECTOR_SIZE}-byte sectors (at least one)"
        ));
    }
    let sectors = bytes / SECTOR_SIZE as u64;
    if sectors > grail::block::MAX_SECTORS {
        return Err(too_big());
    }
    Ok(sectors)
}

/// Parses a network card for a VM's hardware profile: `SWITCH[,mac=ADDR]`, the switch
/// on Avalon to plug it into (`-` for none: the card starts unplugged) and optionally
/// its MAC address (`02:00:00:00:00:01`, or with `-`; without one Avalon makes one up
/// from the VM's name). The address must be one a card may have: individual and
/// locally administered, as Grail requires.
pub fn parse_net(s: &str) -> Result<NetSpec, String> {
    let mut parts = s.split(',');
    let switch = match parts.next().unwrap_or_default() {
        "-" => None,
        name => {
            validate_name(name).map_err(|e| format!("'{s}': switch {e}"))?;
            Some(name.to_string())
        }
    };
    let mut mac = None;
    for option in parts {
        match option.split_once('=') {
            Some(("mac", text)) if mac.is_none() => {
                let addr = MacAddr::parse(text)?;
                addr.check_card()?;
                mac = Some(addr.0);
            }
            Some(("mac", _)) => return Err(format!("'{s}': mac= is given twice")),
            _ => return Err(format!("'{s}': unknown option '{option}' (there is mac=ADDR)")),
        }
    }
    Ok(NetSpec { switch, mac })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn network_cards() {
        let net = |switch: Option<&str>, mac: Option<[u8; 6]>| NetSpec {
            switch: switch.map(str::to_string),
            mac,
        };
        assert_eq!(parse_net("lan"), Ok(net(Some("lan"), None)));
        assert_eq!(parse_net("-"), Ok(net(None, None)));
        assert_eq!(
            parse_net("lan,mac=02:00:00:00:00:2a"),
            Ok(net(Some("lan"), Some([2, 0, 0, 0, 0, 0x2a])))
        );
        assert_eq!(
            parse_net("-,mac=0A-BC-00-00-00-01"),
            Ok(net(None, Some([0x0a, 0xbc, 0, 0, 0, 1])))
        );
        for (bad, why) in [
            ("", "'': switch name must be"),
            ("LAN", "'LAN': switch 'LAN' is not a valid name"),
            ("lan,", "'lan,': unknown option ''"),
            ("lan,mtu=1500", "'lan,mtu=1500': unknown option 'mtu=1500'"),
            ("lan,mac=x", "'x' is not a MAC address"),
            (
                "lan,mac=01:00:5e:00:00:01",
                "01:00:5e:00:00:01 is a group address",
            ),
            (
                "lan,mac=00:11:22:33:44:55",
                "00:11:22:33:44:55 is not locally administered",
            ),
            (
                "lan,mac=02:00:00:00:00:01,mac=02:00:00:00:00:02",
                "'lan,mac=02:00:00:00:00:01,mac=02:00:00:00:00:02': mac= is given twice",
            ),
        ] {
            let e = parse_net(bad).unwrap_err();
            assert!(e.starts_with(why), "{bad}: {e}");
        }
    }

    #[test]
    fn addresses() {
        let ok = |s: &str, want: &str| assert_eq!(parse_addr(s, 7400).as_deref(), Ok(want), "{s}");
        ok("127.0.0.1:7500", "127.0.0.1:7500");
        ok("127.0.0.1", "127.0.0.1:7400");
        ok("localhost", "localhost:7400");
        ok("localhost:9", "localhost:9");
        ok("avalon.lan:7400", "avalon.lan:7400");
        ok("::1", "[::1]:7400");
        ok("[::1]", "[::1]:7400");
        ok("[::1]:7500", "[::1]:7500");
        for bad in [
            "",
            " ",
            "a b",
            ":7400",
            "host:",
            "host:port",
            "host:70000",
            "host:1:2",
            "[::1",
            "[host]:1",
            "host/x",
            // Port 0 cannot be connected to.
            "host:0",
            "host:00",
            "127.0.0.1:0",
            "[::1]:0",
        ] {
            assert!(parse_addr(bad, 7400).is_err(), "{bad:?}");
        }
        assert_eq!(default_port(avalon_proto::DEFAULT_AVALON_ADDR), 7400);
        assert_eq!(default_port(avalon_proto::DEFAULT_LAKE_ADDR), 7401);
    }

    #[test]
    fn ram_and_clock_are_grails() {
        assert_eq!(parse_ram("64"), Ok(64 << 20));
        assert_eq!(parse_ram("2048"), Ok(2048 << 20));
        assert!(parse_ram("0").is_err());
        assert!(parse_ram("2049").is_err());
        assert!(parse_ram("64MiB").is_err());
        assert_eq!(parse_clock("10MHz"), Ok(10_000_000));
        assert_eq!(parse_clock("1.5mhz"), Ok(1_500_000));
        assert!(parse_clock("0").is_err());
        assert_eq!(parse_harts("4"), Ok(4));
        assert!(parse_harts("0").is_err());
        assert!(parse_harts("9").is_err());
    }

    #[test]
    fn disk_sources() {
        assert_eq!(
            parse_disk_source("disk.img"),
            Ok(DiskSource::File("disk.img".into()))
        );
        assert_eq!(
            parse_disk_source(r"C:\images\a b.img"),
            Ok(DiskSource::File(r"C:\images\a b.img".into()))
        );
        assert_eq!(
            parse_disk_source("lake://127.0.0.1:7401/data"),
            Ok(DiskSource::Lake {
                addr: "127.0.0.1:7401".into(),
                name: "data".into()
            })
        );
        assert_eq!(
            parse_disk_source("lake://storage/boot.img"),
            Ok(DiskSource::Lake {
                addr: "storage:7401".into(),
                name: "boot.img".into()
            })
        );
        for bad in [
            "",
            "lake://",
            "lake://host",
            "lake:///data",
            "lake://host/",
            "lake://host/Data",
            "lake://host/a/b",
            "lake://host/nul",
            "lake://h:0/x",
        ] {
            assert!(parse_disk_source(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn disk_sizes() {
        let ok = |s: &str, sectors: u64| assert_eq!(parse_disk_size(s), Ok(sectors), "{s}");
        ok("512", 1);
        ok("4096B", 8);
        ok("1K", 2);
        ok("1kib", 2);
        ok("64MiB", 131_072);
        ok("64 MiB", 131_072);
        ok("1G", 2_097_152);
        ok("2048s", 2048);
        ok("3 sectors", 3);
        ok("1 sector", 1);
        ok("1Sector", 1);
        ok("2047GiB", 2047 << 21);
        ok("4294967295s", grail::block::MAX_SECTORS);
        for bad in [
            "",
            "0",
            "0s",
            "100",
            "1.5M",
            "-1K",
            "1T",
            "M",
            "1 M B",
            "64MB",
            "2048G",
            "4294967296s",
            "99999999999999999999",
            "18446744073709551615G",
        ] {
            assert!(parse_disk_size(bad).is_err(), "{bad:?}");
        }
    }
}
