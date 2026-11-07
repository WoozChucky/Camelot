//! The real-time clock (spec section 9.9): the Goldfish RTC of Android's emulator and
//! QEMU's `virt` machine, register for register, so Linux's `rtc-goldfish` driver and
//! OpenSBI's tree find it as they find QEMU's.
//!
//! It counts nanoseconds since the Unix epoch, 1970-01-01 00:00:00 UTC. Its time is
//! not read from the host at each access: it is the time it was started at plus the
//! machine's own time since, `mtime` ticks at the timebase, so a machine that runs at
//! full speed sees its clock run as fast as its timer does, and a machine started at a
//! fixed time ([`Rtc::At`]) reads the same times on every run.
//!
//! Registers, 32 bits each:
//!
//! | Offset | Register | |
//! |---|---|---|
//! | 0x00 | `TIME_LOW` | the time's low 32 bits; reading it latches the high 32 into `TIME_HIGH`; writing it sets the time, the written `TIME_HIGH` its high half and this its low |
//! | 0x04 | `TIME_HIGH` | the high 32 bits latched by the last `TIME_LOW` read; a write is kept for the next `TIME_LOW` write |
//! | 0x08 | `ALARM_LOW` | writing it arms the alarm, the written `ALARM_HIGH` its high half and this its low; reads the last alarm's low half |
//! | 0x0C | `ALARM_HIGH` | the last alarm's high half; a write is kept for the next `ALARM_LOW` write |
//! | 0x10 | `IRQ_ENABLED` | bit 0: the alarm raises the interrupt line |
//! | 0x14 | `CLEAR_ALARM` | a write disarms the alarm |
//! | 0x18 | `ALARM_STATUS` | 1 while the alarm is armed |
//! | 0x1C | `CLEAR_INTERRUPT` | a write lowers the line |
//!
//! Linux reads `TIME_LOW` then `TIME_HIGH`, so the two halves are of one time, and
//! writes `TIME_HIGH` then `TIME_LOW`.

use crate::bus::BusFault;
use std::time::{SystemTime, UNIX_EPOCH};

pub const REG_TIME_LOW: u32 = 0x00;
pub const REG_TIME_HIGH: u32 = 0x04;
pub const REG_ALARM_LOW: u32 = 0x08;
pub const REG_ALARM_HIGH: u32 = 0x0C;
pub const REG_IRQ_ENABLED: u32 = 0x10;
pub const REG_CLEAR_ALARM: u32 = 0x14;
pub const REG_ALARM_STATUS: u32 = 0x18;
pub const REG_CLEAR_INTERRUPT: u32 = 0x1C;

const NS_PER_SEC: u64 = 1_000_000_000;

/// Where a machine's clock starts ([`crate::MachineConfig::rtc`], `--rtc`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rtc {
    /// The host's time when the machine is built: the `grail` command's default, and
    /// the platform's.
    Host,
    /// This many seconds after the Unix epoch, UTC: the same time on every run, as the
    /// tests want. A `MachineConfig`'s default is [`DEFAULT_RTC_TIME`].
    At(u64),
}

/// 2026-01-01 00:00:00 UTC, where the clock of a [`crate::MachineConfig`] built in code
/// starts unless it says otherwise.
pub const DEFAULT_RTC_TIME: u64 = 1_767_225_600;

impl Rtc {
    /// Nanoseconds since the epoch at which the clock starts.
    fn start_ns(self) -> u64 {
        match self {
            Rtc::Host => SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos().min(u128::from(u64::MAX)) as u64),
            Rtc::At(secs) => secs.saturating_mul(NS_PER_SEC),
        }
    }
}

/// `--rtc`'s argument: `host`, seconds since the epoch, or a UTC date as
/// `YYYY-MM-DD` or `YYYY-MM-DDTHH:MM:SS` (QEMU's `-rtc base=` spelling).
pub fn parse_rtc(s: &str) -> Result<Rtc, String> {
    if s == "host" {
        return Ok(Rtc::Host);
    }
    if !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) {
        return s.parse().map(Rtc::At).map_err(|_| format!("bad time `{s}`"));
    }
    parse_date(s)
        .map(Rtc::At)
        .ok_or_else(|| format!("bad time `{s}`: host, seconds since 1970, YYYY-MM-DD or YYYY-MM-DDTHH:MM:SS"))
}

/// Seconds since the epoch of a UTC `YYYY-MM-DD[THH:MM:SS]`, from 1970 on.
fn parse_date(s: &str) -> Option<u64> {
    let (date, time) = s.split_once('T').unwrap_or((s, "00:00:00"));
    let num = |p: &str, len: usize| {
        (p.len() == len && p.bytes().all(|b| b.is_ascii_digit()))
            .then(|| p.parse::<u64>().ok())
            .flatten()
    };
    let mut d = date.split('-');
    let (y, m, day) = (num(d.next()?, 4)?, num(d.next()?, 2)?, num(d.next()?, 2)?);
    let mut t = time.split(':');
    let (h, min, sec) = (num(t.next()?, 2)?, num(t.next()?, 2)?, num(t.next()?, 2)?);
    if d.next().is_some() || t.next().is_some() || y < 1970 || !(1..=12).contains(&m) {
        return None;
    }
    if day == 0 || day > days_in_month(y, m) || h > 23 || min > 59 || sec > 59 {
        return None;
    }
    Some(days_from_civil(y, m, day) * 86_400 + h * 3600 + min * 60 + sec)
}

fn days_in_month(y: u64, m: u64) -> u64 {
    match m {
        2 if y.is_multiple_of(4) && (!y.is_multiple_of(100) || y.is_multiple_of(400)) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Days from 1970-01-01 to `y-m-d` (Howard Hinnant's `days_from_civil`, for years
/// from 1970 on): years counted from March, so the leap day ends one.
fn days_from_civil(y: u64, m: u64, d: u64) -> u64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The device: its time is `base_ns` plus the time `mtime` has counted.
pub struct RtcDevice {
    /// The time at `mtime` 0, in nanoseconds since the epoch (wrapping: a time set
    /// before the machine's own time has run wraps it).
    base_ns: u64,
    /// `mtime` ticks a second.
    hz: u64,
    /// `TIME_HIGH` as a read gives it, latched by the last `TIME_LOW` read.
    time_high: u32,
    /// `TIME_HIGH` as last written, for the next `TIME_LOW` write.
    set_high: u32,
    /// `ALARM_HIGH` as last written, for the next `ALARM_LOW` write.
    alarm_high: u32,
    /// The armed alarm's time, if one is.
    alarm: Option<u64>,
    /// The last alarm set, armed or not, as `ALARM_LOW` and `ALARM_HIGH` read.
    alarm_last: u64,
    irq_enabled: bool,
    /// The alarm went off and its interrupt has not been cleared.
    pending: bool,
}

impl RtcDevice {
    /// A clock started at `start` on a machine whose timebase is `hz`.
    pub fn new(start: Rtc, hz: u32) -> Self {
        RtcDevice {
            base_ns: start.start_ns(),
            hz: u64::from(hz.max(1)),
            time_high: 0,
            set_high: 0,
            alarm_high: 0,
            alarm: None,
            alarm_last: 0,
            irq_enabled: false,
            pending: false,
        }
    }

    /// Nanoseconds `ticks` of `mtime` make.
    fn ticks_to_ns(&self, ticks: u64) -> u64 {
        (u128::from(ticks) * u128::from(NS_PER_SEC) / u128::from(self.hz)) as u64
    }

    /// The time, in nanoseconds since the epoch, when `mtime` reads `mtime`.
    pub fn now_ns(&self, mtime: u64) -> u64 {
        self.base_ns.wrapping_add(self.ticks_to_ns(mtime))
    }

    /// Reads a register at `mtime`: 32-bit ones only.
    pub fn read(&mut self, off: u32, size: u32, mtime: u64) -> Result<u32, BusFault> {
        if size != 4 {
            return Err(BusFault);
        }
        Ok(match off {
            REG_TIME_LOW => {
                let now = self.now_ns(mtime);
                self.time_high = (now >> 32) as u32;
                now as u32
            }
            REG_TIME_HIGH => self.time_high,
            REG_ALARM_LOW => self.alarm_last as u32,
            REG_ALARM_HIGH => (self.alarm_last >> 32) as u32,
            REG_IRQ_ENABLED => self.irq_enabled.into(),
            REG_ALARM_STATUS => self.alarm.is_some().into(),
            _ => 0,
        })
    }

    /// Writes a register at `mtime`.
    pub fn write(&mut self, off: u32, size: u32, val: u32, mtime: u64) -> Result<(), BusFault> {
        if size != 4 {
            return Err(BusFault);
        }
        match off {
            REG_TIME_LOW => {
                let set = u64::from(self.set_high) << 32 | u64::from(val);
                self.base_ns = set.wrapping_sub(self.ticks_to_ns(mtime));
            }
            REG_TIME_HIGH => self.set_high = val,
            REG_ALARM_LOW => {
                let at = u64::from(self.alarm_high) << 32 | u64::from(val);
                self.alarm_last = at;
                self.alarm = Some(at);
                self.check_alarm(mtime);
            }
            REG_ALARM_HIGH => self.alarm_high = val,
            REG_IRQ_ENABLED => self.irq_enabled = val & 1 != 0,
            REG_CLEAR_ALARM => self.alarm = None,
            REG_CLEAR_INTERRUPT => self.pending = false,
            _ => {}
        }
        Ok(())
    }

    /// Sets off the alarm if its time has come at `mtime`.
    fn check_alarm(&mut self, mtime: u64) {
        if self.alarm.is_some_and(|at| self.now_ns(mtime) >= at) {
            self.alarm = None;
            self.pending = true;
        }
    }

    /// The interrupt line at `mtime`: an alarm that went off, with the interrupt
    /// enabled.
    pub fn irq(&mut self, mtime: u64) -> bool {
        self.check_alarm(mtime);
        self.pending && self.irq_enabled
    }

    /// `mtime` ticks from `mtime` until the armed alarm raises the line, one short of
    /// it ([`crate::bus::Bus::device_deadline`]); `None` with nothing to raise it.
    pub fn deadline(&self, mtime: u64) -> Option<u64> {
        let at = self.alarm.filter(|_| self.irq_enabled)?;
        let ns = at.saturating_sub(self.now_ns(mtime));
        let ticks = (u128::from(ns) * u128::from(self.hz)).div_ceil(u128::from(NS_PER_SEC));
        Some((ticks.min(u128::from(u64::MAX)) as u64).saturating_sub(1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_parse_to_seconds_since_the_epoch() {
        assert_eq!(parse_rtc("host"), Ok(Rtc::Host));
        assert_eq!(parse_rtc("0"), Ok(Rtc::At(0)));
        assert_eq!(parse_rtc("1970-01-01"), Ok(Rtc::At(0)));
        assert_eq!(parse_rtc("2026-01-01"), Ok(Rtc::At(DEFAULT_RTC_TIME)));
        assert_eq!(parse_rtc("2000-02-29T12:34:56"), Ok(Rtc::At(951_827_696)));
        assert_eq!(parse_rtc("2038-01-19T03:14:08"), Ok(Rtc::At(1 << 31)));
        for bad in [
            "",
            "now",
            "1969-12-31",
            "2001-02-29",
            "2026-13-01",
            "2026-1-1",
            "2026-01-01T24:00:00",
        ] {
            assert!(parse_rtc(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_time_is_the_start_plus_mtime() {
        let mut rtc = RtcDevice::new(Rtc::At(DEFAULT_RTC_TIME), 10_000_000);
        let start = DEFAULT_RTC_TIME * NS_PER_SEC;
        let time = |rtc: &mut RtcDevice, mtime| {
            let low = rtc.read(REG_TIME_LOW, 4, mtime).unwrap();
            let high = rtc.read(REG_TIME_HIGH, 4, mtime + 1_000_000_000).unwrap();
            u64::from(high) << 32 | u64::from(low)
        };
        assert_eq!(time(&mut rtc, 0), start);
        // 2.5 s of a 10 MHz timebase; the high half is the one latched with the low.
        assert_eq!(time(&mut rtc, 25_000_000), start + 2_500_000_000);
        assert_eq!(rtc.read(REG_TIME_LOW, 8, 0), Err(BusFault));
        assert_eq!(rtc.write(REG_TIME_LOW, 2, 0, 0), Err(BusFault));

        // Set to 1 s after the epoch at mtime 10^7 (1 s in): 1.5 s later it reads 2.5 s.
        rtc.write(REG_TIME_HIGH, 4, 0, 10_000_000).unwrap();
        rtc.write(REG_TIME_LOW, 4, NS_PER_SEC as u32, 10_000_000).unwrap();
        assert_eq!(time(&mut rtc, 25_000_000), 2_500_000_000);
    }

    #[test]
    fn an_armed_alarm_raises_the_line_once() {
        let mut rtc = RtcDevice::new(Rtc::At(0), 1000);
        // Due at 2 s: 2000 ticks of a 1 kHz timebase.
        rtc.write(REG_ALARM_HIGH, 4, 0, 0).unwrap();
        rtc.write(REG_ALARM_LOW, 4, 2 * NS_PER_SEC as u32, 0).unwrap();
        assert_eq!(rtc.read(REG_ALARM_STATUS, 4, 0), Ok(1));
        assert_eq!(rtc.deadline(0), None, "the interrupt is off");
        rtc.write(REG_IRQ_ENABLED, 4, 1, 0).unwrap();
        assert_eq!(rtc.deadline(0), Some(1999));
        assert_eq!(rtc.deadline(1500), Some(499));
        assert!(!rtc.irq(1999));
        assert!(rtc.irq(2000));
        assert_eq!(rtc.read(REG_ALARM_STATUS, 4, 2000), Ok(0));
        assert_eq!(rtc.deadline(2000), None);
        assert!(rtc.irq(3000), "until cleared");
        rtc.write(REG_CLEAR_INTERRUPT, 4, 1, 3000).unwrap();
        assert!(!rtc.irq(3000));

        // Disarmed, it never goes off; armed in the past, at once.
        rtc.write(REG_ALARM_LOW, 4, 4 * NS_PER_SEC as u32, 3000).unwrap();
        rtc.write(REG_CLEAR_ALARM, 4, 1, 3000).unwrap();
        assert!(!rtc.irq(5000));
        rtc.write(REG_ALARM_LOW, 4, 1, 5000).unwrap();
        assert!(rtc.irq(5000));
        assert_eq!(rtc.read(REG_ALARM_LOW, 4, 5000), Ok(1));
    }

    #[test]
    fn the_host_clock_starts_near_now() {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
        let rtc = RtcDevice::new(Rtc::Host, 1000);
        assert!(rtc.now_ns(0) / NS_PER_SEC >= now);
    }
}
