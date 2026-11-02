//! A machine's hardware profile, plus the parsers for the command-line flags that set it.
//!
//! The profile is plain data so a host can build one per machine (the `grail` CLI from
//! its flags, a hypervisor from its own configuration). The parsers are public for the
//! same reason: every front end accepts the same `--ram` and `--clock` syntax.

use crate::bus::RAM_BASE;

pub const MIB: usize = 1024 * 1024;
/// RAM size when nothing else is asked for.
pub const DEFAULT_RAM_SIZE: usize = 128 * MIB;
/// RAM must end at or below 4 GiB: everything from `RAM_BASE` to the top of the address
/// space, 2 GiB.
pub const MAX_RAM_SIZE: usize = (u32::MAX - RAM_BASE) as usize + 1;
/// RAM is handed out in whole 4 KiB pages.
pub const RAM_GRANULE: usize = 4096;
/// Clock frequency the machine reports when it is not paced to a real-time clock.
pub const DEFAULT_NOMINAL_HZ: u32 = 10_000_000;

/// What a machine is built from. New hardware (disks, more devices) becomes new fields
/// with a default value, so build configs as
/// `MachineConfig { ram_size, ..MachineConfig::default() }`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MachineConfig {
    /// RAM size in bytes, mapped at `RAM_BASE`. A non-zero multiple of 4 KiB, at most
    /// [`MAX_RAM_SIZE`].
    pub ram_size: usize,
    /// Real-time mode: run this many steps (instructions, and so `mtime` ticks) per
    /// wall-clock second. `None` runs at full speed, deterministically.
    pub clock_hz: Option<u32>,
    /// The CPU and timebase frequency reported to software when `clock_hz` is `None`,
    /// so it computes timer intervals the same way in both modes.
    pub nominal_hz: u32,
}

impl Default for MachineConfig {
    fn default() -> Self {
        MachineConfig {
            ram_size: DEFAULT_RAM_SIZE,
            clock_hz: None,
            nominal_hz: DEFAULT_NOMINAL_HZ,
        }
    }
}

impl MachineConfig {
    /// The default machine with `ram_size` bytes of RAM.
    pub fn with_ram(ram_size: usize) -> Self {
        MachineConfig {
            ram_size,
            ..MachineConfig::default()
        }
    }

    /// CPU clock in Hz: the real-time clock if there is one, else the nominal one. One
    /// instruction is one cycle is one `mtime` tick, so this is also the timebase.
    pub fn cpu_hz(&self) -> u32 {
        self.clock_hz.unwrap_or(self.nominal_hz)
    }

    pub fn validate(&self) -> Result<(), String> {
        let ram = self.ram_size;
        if ram == 0 || ram > MAX_RAM_SIZE || !ram.is_multiple_of(RAM_GRANULE) {
            return Err(format!(
                "RAM size {ram:#x} must be a non-zero multiple of 4 KiB, at most {MAX_RAM_SIZE:#x} (2 GiB)"
            ));
        }
        if self.clock_hz == Some(0) || self.nominal_hz == 0 {
            return Err("clock frequency must not be 0 Hz".into());
        }
        Ok(())
    }
}

/// Parses a `--ram` value: a whole number of MiB, from 1 up to what fits between
/// `RAM_BASE` and the top of the address space (2048). Returns the size in bytes.
pub fn parse_ram_mib(s: &str) -> Result<usize, String> {
    const MAX_MIB: usize = MAX_RAM_SIZE / MIB;
    let s = s.trim();
    let mib: usize = match s.parse() {
        Ok(n) if s.bytes().all(|b| b.is_ascii_digit()) => n,
        _ => return Err(format!("'{s}' is not a whole number of MiB")),
    };
    if !(1..=MAX_MIB).contains(&mib) {
        return Err(format!("{mib} MiB is out of range (1 to {MAX_MIB} MiB)"));
    }
    Ok(mib * MIB)
}

/// Parses a `--clock` value: a number with an optional unit, case-insensitive: `Hz`,
/// `kHz` or `MHz` (a bare number is Hz). Fractions are fine as long as the result is a
/// whole number of Hz (`1.5MHz`, not `1.5Hz`). The result must be 1 Hz to `u32::MAX` Hz.
pub fn parse_clock(s: &str) -> Result<u32, String> {
    let s = s.trim();
    let bad = || format!("'{s}' is not a frequency (examples: 10MHz, 500kHz, 1.5MHz, 5Hz)");
    let split = s
        .find(|c: char| !c.is_ascii_digit() && c != '.')
        .unwrap_or(s.len());
    let (number, unit) = s.split_at(split);
    let scale: u128 = match unit.to_ascii_lowercase().as_str() {
        "" | "hz" => 1,
        "khz" => 1_000,
        "mhz" => 1_000_000,
        _ => return Err(bad()),
    };
    let (whole, frac) = number.split_once('.').unwrap_or((number, ""));
    let digits_only = |p: &str| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit());
    if !digits_only(whole) || (number.contains('.') && !digits_only(frac)) {
        return Err(bad());
    }
    // Trailing zeros change nothing. A non-zero digit past the 6th decimal place (the
    // largest scale is 10^6) leaves a fraction of a Hz, however long the number is.
    let frac = frac.trim_end_matches('0');
    if frac.len() > 6 {
        return Err(format!("{s} is not a whole number of Hz"));
    }
    // value = whole.frac * scale, in exact integer arithmetic: (whole.frac * 10^k) * scale / 10^k.
    let too_big = || format!("{s} does not fit in 32 bits (at most {} Hz)", u32::MAX);
    let mut mantissa: u128 = 0;
    for b in whole.bytes().chain(frac.bytes()) {
        mantissa = mantissa
            .checked_mul(10)
            .and_then(|m| m.checked_add((b - b'0') as u128))
            .ok_or_else(too_big)?;
    }
    let divisor = 10u128.checked_pow(frac.len() as u32).ok_or_else(too_big)?;
    let scaled = mantissa.checked_mul(scale).ok_or_else(too_big)?;
    if scaled % divisor != 0 {
        return Err(format!("{s} is not a whole number of Hz"));
    }
    match u32::try_from(scaled / divisor) {
        Ok(0) => Err("the clock must be faster than 0 Hz".into()),
        Ok(hz) => Ok(hz),
        Err(_) => Err(too_big()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config() {
        let c = MachineConfig::default();
        assert_eq!(c.ram_size, 128 * MIB);
        assert_eq!(c.clock_hz, None);
        assert_eq!(c.cpu_hz(), 10_000_000);
        assert_eq!(c.validate(), Ok(()));
        let paced = MachineConfig {
            clock_hz: Some(5),
            ..c
        };
        assert_eq!(paced.cpu_hz(), 5);
    }

    #[test]
    fn validate_rejects_bad_sizes_and_zero_clocks() {
        assert_eq!(MAX_RAM_SIZE, 0x8000_0000);
        for ram in [4096, 64 * 1024, MIB, MAX_RAM_SIZE] {
            assert_eq!(MachineConfig::with_ram(ram).validate(), Ok(()), "{ram:#x}");
        }
        for ram in [0, 1, 4095, 4097, MAX_RAM_SIZE + RAM_GRANULE] {
            assert!(MachineConfig::with_ram(ram).validate().is_err(), "{ram:#x}");
        }
        let zero_clock = MachineConfig {
            clock_hz: Some(0),
            ..MachineConfig::default()
        };
        assert!(zero_clock.validate().is_err());
        let zero_nominal = MachineConfig {
            nominal_hz: 0,
            ..MachineConfig::default()
        };
        assert!(zero_nominal.validate().is_err());
    }

    #[test]
    fn ram_accepts_1_to_2048_mib() {
        assert_eq!(parse_ram_mib("1"), Ok(MIB));
        assert_eq!(parse_ram_mib("128"), Ok(128 * MIB));
        assert_eq!(parse_ram_mib("2048"), Ok(0x8000_0000));
        assert_eq!(parse_ram_mib(" 64 "), Ok(64 * MIB));
        for good in ["1", "16", "2048"] {
            let size = parse_ram_mib(good).unwrap();
            assert_eq!(MachineConfig::with_ram(size).validate(), Ok(()));
        }
    }

    #[test]
    fn ram_rejects_out_of_range_and_garbage() {
        for bad in [
            "0",
            "2049",
            "4096",
            "99999999999999999999999",
            "-1",
            "+5",
            "1.5",
            "12M",
            "128MiB",
            "",
            "abc",
            "0x10",
        ] {
            assert!(parse_ram_mib(bad).is_err(), "{bad:?} accepted");
        }
        assert!(parse_ram_mib("0").unwrap_err().contains("1 to 2048 MiB"));
        assert!(parse_ram_mib("abc").unwrap_err().contains("not a whole number"));
    }

    #[test]
    fn clock_units_and_case() {
        let cases = [
            ("10MHz", 10_000_000),
            ("10mhz", 10_000_000),
            ("10MHZ", 10_000_000),
            ("10Mhz", 10_000_000),
            ("500kHz", 500_000),
            ("500KHZ", 500_000),
            ("500khz", 500_000),
            ("5Hz", 5),
            ("5hz", 5),
            ("5HZ", 5),
            ("5", 5),
            ("1", 1),
            ("4294967295", u32::MAX),
            ("4294967295Hz", u32::MAX),
            ("4294967.295kHz", u32::MAX),
            ("4294.967295MHz", u32::MAX),
            (" 2MHz ", 2_000_000),
            ("007kHz", 7_000),
        ];
        for (s, hz) in cases {
            assert_eq!(parse_clock(s), Ok(hz), "{s:?}");
        }
    }

    #[test]
    fn clock_fractions_must_give_whole_hz() {
        assert_eq!(parse_clock("1.5MHz"), Ok(1_500_000));
        assert_eq!(parse_clock("0.5kHz"), Ok(500));
        assert_eq!(parse_clock("2.25kHz"), Ok(2_250));
        assert_eq!(parse_clock("0.000001MHz"), Ok(1));
        assert_eq!(parse_clock("3.0"), Ok(3));
        assert_eq!(parse_clock("3.000Hz"), Ok(3));
        assert_eq!(parse_clock("1.00000000000000000000kHz"), Ok(1000));
        // Long runs of trailing zeros used to overflow the mantissa and read as too big.
        let zeros = "0".repeat(60);
        assert_eq!(parse_clock(&format!("1.{zeros}kHz")), Ok(1000));
        assert_eq!(parse_clock(&format!("4294.967295{zeros}MHz")), Ok(u32::MAX));
        let tiny = format!("0.{zeros}1MHz");
        for bad in ["1.5Hz", "1.5", "0.0005kHz", "0.0000001MHz", "1.0000001MHz", &tiny] {
            let e = parse_clock(bad).unwrap_err();
            assert!(e.contains("whole number of Hz"), "{bad:?}: {e}");
        }
    }

    #[test]
    fn clock_rejects_zero_overflow_and_garbage() {
        for zero in ["0", "0Hz", "0kHz", "0.0MHz", "000"] {
            assert!(parse_clock(zero).unwrap_err().contains("0 Hz"), "{zero:?}");
        }
        for big in [
            "4294967296",
            "4294967.296kHz",
            "4295MHz",
            "5000MHz",
            "99999999999999999999999999999999999999999999MHz",
        ] {
            assert!(parse_clock(big).unwrap_err().contains("32 bits"), "{big:?}");
        }
        for garbage in [
            "", "MHz", "Hz", "fast", "10 MHz", "10GHz", "10MHzz", "10M", "10k", "1e6", "-5", "+5", "1.",
            ".5MHz", "1..5MHz", "1.2.3", "0x10", "10MHz5", "١٠",
        ] {
            let e = parse_clock(garbage).unwrap_err();
            assert!(e.contains("is not a frequency"), "{garbage:?}: {e}");
        }
    }
}
