//! Protocol values as text for people: sizes, frequencies, device names, disk sources.
//! [`format_hz`] and [`disk_source_text`] give back what [`crate::parse`] accepts.

use avalon_proto::DiskSource;
use grail::net::MacAddr;
use grail::sysinfo::DeviceType;

/// A size in bytes in the largest binary unit that shows it exactly with at most two
/// decimals: `64 MiB`, `1.5 KiB`, `1024.5 KiB`, `700 bytes`.
pub fn format_size(bytes: u64) -> String {
    const UNITS: [(u64, &str); 4] = [
        (1 << 40, "TiB"),
        (1 << 30, "GiB"),
        (1 << 20, "MiB"),
        (1 << 10, "KiB"),
    ];
    for (unit, name) in UNITS {
        // Exact in hundredths of the unit?
        if bytes >= unit && (bytes as u128 * 100).is_multiple_of(unit as u128) {
            return format!("{} {name}", decimal(bytes as u128 * 100 / unit as u128, 2));
        }
    }
    if bytes == 1 {
        "1 byte".into()
    } else {
        format!("{bytes} bytes")
    }
}

/// A frequency in the largest of Hz, kHz and MHz that is not more than it, exactly:
/// `10 MHz`, `1.5 MHz`, `500 kHz`, `5 Hz`. Without the space it parses back with
/// [`crate::parse_clock`] to the same value.
pub fn format_hz(hz: u32) -> String {
    let (unit, places) = match hz {
        1_000_000.. => ("MHz", 6),
        1_000.. => ("kHz", 3),
        _ => ("Hz", 0),
    };
    format!("{} {unit}", decimal(hz as u128, places))
}

/// `value / 10^places` in decimal, trailing zeros (and a trailing point) dropped.
fn decimal(value: u128, places: u32) -> String {
    let scale = 10u128.pow(places);
    let (whole, frac) = (value / scale, value % scale);
    if frac == 0 {
        return whole.to_string();
    }
    let frac = format!("{frac:0width$}", width = places as usize);
    format!("{whole}.{}", frac.trim_end_matches('0'))
}

/// The name of a sysinfo device type code (Excalibur spec section 9.5), as the
/// `sysinfo` example prints it, or `None` for a code this build does not know.
pub fn device_name(kind: u32) -> Option<&'static str> {
    const NAMES: [(DeviceType, &str); 11] = [
        (DeviceType::Clint, "CLINT"),
        (DeviceType::Plic, "PLIC"),
        (DeviceType::Uart, "UART"),
        (DeviceType::TestFinisher, "test"),
        (DeviceType::FbCtrl, "fb-ctrl"),
        (DeviceType::FbVram, "fb-vram"),
        (DeviceType::Sysinfo, "sysinfo"),
        (DeviceType::Keyboard, "keyboard"),
        (DeviceType::Block, "block"),
        (DeviceType::Pointer, "pointer"),
        (DeviceType::Net, "net"),
    ];
    NAMES
        .iter()
        .find(|(t, _)| *t as u32 == kind)
        .map(|(_, name)| *name)
}

/// A MAC address as `02:ca:00:00:00:01`, which [`crate::parse_net`] reads back.
pub fn mac_text(mac: [u8; 6]) -> String {
    MacAddr(mac).to_string()
}

/// A disk source as it is typed: the file's path, or `lake://ADDR/NAME` (the
/// protocol's `Display`, which `DiskSource::parse` reads back).
pub fn disk_source_text(source: &DiskSource) -> String {
    source.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::{parse_clock, parse_disk_source};

    #[test]
    fn sizes() {
        assert_eq!(format_size(0), "0 bytes");
        assert_eq!(format_size(1), "1 byte");
        assert_eq!(format_size(512), "512 bytes");
        assert_eq!(format_size(1024), "1 KiB");
        assert_eq!(format_size(1536), "1.5 KiB");
        assert_eq!(format_size(4096), "4 KiB");
        assert_eq!(format_size(64 << 20), "64 MiB");
        assert_eq!(format_size((1 << 20) + 512), "1024.5 KiB");
        assert_eq!(format_size(3 << 29), "1.5 GiB");
        assert_eq!(format_size(2 << 30), "2 GiB");
        assert_eq!(format_size(1 << 40), "1 TiB");
        assert_eq!(format_size(u64::MAX), format!("{} bytes", u64::MAX));
    }

    #[test]
    fn frequencies_round_trip() {
        assert_eq!(format_hz(10_000_000), "10 MHz");
        assert_eq!(format_hz(1_500_000), "1.5 MHz");
        assert_eq!(format_hz(500_000), "500 kHz");
        assert_eq!(format_hz(1_234), "1.234 kHz");
        assert_eq!(format_hz(5), "5 Hz");
        assert_eq!(format_hz(999), "999 Hz");
        assert_eq!(format_hz(u32::MAX), "4294.967295 MHz");
        for hz in [
            1,
            5,
            999,
            1_000,
            1_001,
            12_345,
            1_000_000,
            1_500_000,
            10_000_000,
            33_333_333,
            u32::MAX,
        ] {
            assert_eq!(parse_clock(&format_hz(hz).replace(' ', "")), Ok(hz), "{hz}");
        }
    }

    #[test]
    fn device_names() {
        assert_eq!(device_name(1), Some("CLINT"));
        assert_eq!(device_name(3), Some("UART"));
        assert_eq!(device_name(4), Some("test"));
        assert_eq!(device_name(9), Some("block"));
        assert_eq!(device_name(10), Some("pointer"));
        assert_eq!(device_name(11), Some("net"));
        assert_eq!(device_name(0), None);
        assert_eq!(device_name(999), None);
    }

    #[test]
    fn macs_round_trip() {
        let mac = [0x02, 0xca, 0, 0x0f, 0xa0, 0xff];
        assert_eq!(mac_text(mac), "02:ca:00:0f:a0:ff");
        let back = crate::parse_net(&format!("-,mac={}", mac_text(mac))).unwrap();
        assert_eq!(back.mac, Some(mac));
    }

    #[test]
    fn disk_sources_round_trip() {
        for text in [
            "disk.img",
            "lake://127.0.0.1:7401/data",
            "lake://[::1]:7401/boot.img",
        ] {
            assert_eq!(disk_source_text(&parse_disk_source(text).unwrap()), text);
        }
    }
}
