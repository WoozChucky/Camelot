//! A machine's hardware profile, plus the parsers for the command-line flags that set it.
//!
//! The profile is plain data so a host can build one per machine (the `grail` CLI from
//! its flags, a hypervisor from its own configuration). The parsers are public for the
//! same reason: every front end accepts the same `--ram` and `--clock` syntax.

use crate::block::{BlockBackend, FileDisk, MemDisk, MAX_SECTORS, SECTOR_SIZE};
use crate::bus::{MAX_DISKS, MAX_NETS};
use crate::lake::LakeDisk;
use crate::net::{Hub, MacAddr, NetBackend, Pcap};
use gateway::{Gateway, HostFwd};
use std::path::PathBuf;

pub const MIB: usize = 1024 * 1024;
pub const GIB: usize = 1024 * MIB;
/// RAM size when nothing else is asked for.
pub const DEFAULT_RAM_SIZE: usize = 4 * MIB;
/// The most RAM a machine has: 1 TiB, the memory controller's limit (spec section 7),
/// ending at 0x100_7FFF_FFFF, well inside the 48-bit physical address space. Backed
/// lazily ([`crate::ram::Ram`]), so it costs the host what the guest touches.
pub const MAX_RAM_SIZE: usize = 1 << 40;
/// RAM is handed out in whole 4 KiB pages.
pub const RAM_GRANULE: usize = 4096;
/// Clock frequency the machine reports when it is not paced to a real-time clock.
pub const DEFAULT_NOMINAL_HZ: u32 = 10_000_000;
/// Most harts a machine can have. Grail runs them one after the other on one host
/// thread, so each one more slows the others down; the CLINT and PLIC layouts have
/// room for many more.
pub const MAX_HARTS: usize = 8;

/// What a machine is built from. New hardware (disks, more devices) becomes new fields
/// with a default value, so build configs as
/// `MachineConfig { ram_size, ..MachineConfig::default() }`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MachineConfig {
    /// RAM size in bytes, mapped at `RAM_BASE`. A non-zero multiple of 4 KiB, at most
    /// [`MAX_RAM_SIZE`] (1 TiB).
    pub ram_size: usize,
    /// Real-time mode: run this many steps (instructions, and so `mtime` ticks) per
    /// wall-clock second. `None` runs at full speed, deterministically.
    pub clock_hz: Option<u32>,
    /// The CPU and timebase frequency reported to software when `clock_hz` is `None`,
    /// so it computes timer intervals the same way in both modes.
    pub nominal_hz: u32,
    /// Harts (cores), 1 to [`MAX_HARTS`]. Each runs at the clock: with `clock_hz`, `n`
    /// harts run `n * clock_hz` instructions per second in all.
    pub harts: usize,
    /// Block devices (spec section 9.6): disk `i` is mapped at `BLK_BASE + i * BLK_SIZE`
    /// and interrupts on PLIC source `BLK_IRQ + i`. At most [`MAX_DISKS`], counting any
    /// backends handed to [`crate::Machine::with_backends`].
    pub disks: Vec<DiskConfig>,
    /// Network cards (spec section 9.8): card `i` is mapped at `NET_BASE + i * NET_SIZE`
    /// and interrupts on PLIC source `NET_IRQ + i`. At most [`MAX_NETS`], counting any
    /// backends handed to [`crate::Machine::with_devices`].
    pub nets: Vec<NetConfig>,
    /// The paging modes the harts implement: Sv39 and Sv48 by default, or Sv39 only, as
    /// on most RV64 boards (`--paging sv39`, #172): `satp` then refuses a write of MODE
    /// 9 (WARL, spec section 5), which is how software finds out.
    pub paging: Paging,
    /// The firmware the machine boots, QEMU's `-bios` (`--bios`, #174): Camelot's
    /// built-in firmware or one from a file, which starts the program in S-mode, or
    /// none, so the program runs in M-mode. None in a `MachineConfig` built here (the
    /// bare machine a host builds for its own program); the `grail` command and the
    /// platform's front ends default to Camelot's firmware, as QEMU's `virt` machine
    /// boots OpenSBI unless told `-bios none` (spec section 7, "Firmware").
    pub bios: Bios,
}

/// What [`MachineConfig::paging`] lets a hart translate with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Paging {
    /// Sv48 and Sv39 (and Bare).
    #[default]
    Sv48,
    /// Sv39 (and Bare) only: no Sv48.
    Sv39,
}

/// What [`MachineConfig::bios`] loads before the program: QEMU's `-bios` (spec
/// section 7, "Firmware").
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Bios {
    /// No firmware (`--bios none`): the boot ROM starts the program, or its disk
    /// loader the disk's boot block, in M-mode, as riscv-tests and the bare-metal
    /// examples run. A [`MachineConfig`]'s default; the `grail` command's is
    /// [`Bios::Default`].
    #[default]
    None,
    /// Camelot's firmware, built into Grail (`--bios default`, the `grail` command's
    /// default,
    /// [`crate::firmware::FIRMWARE_ELF`]): it runs in M-mode, implements the SBI,
    /// and starts the program, or the disk's boot block, in S-mode.
    Default,
    /// The firmware in this file (`--bios FILE`): an ELF executable, or a raw
    /// binary loaded at the start of RAM, as OpenSBI's `fw_jump.bin`.
    File(PathBuf),
}

impl Bios {
    /// Whether the machine has firmware.
    pub fn is_some(&self) -> bool {
        *self != Bios::None
    }
}

/// One network card of a [`MachineConfig`]: what it is plugged into, its address, and
/// whether its frames are captured.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetConfig {
    pub source: NetSource,
    /// The card's MAC address (`NET_MAC`). `None` lets the backend pick one: a hub port
    /// gives `02:ca:00:00:00:NN`, with `NN` the port's number on the hub; user
    /// networking gives `52:54:00:12:34:56`.
    pub mac: Option<MacAddr>,
    /// Write every frame the card sends or receives to this file, in pcap format.
    pub pcap: Option<PathBuf>,
}

/// What a configured card is plugged into. A host with a backend of its own passes it
/// to [`crate::Machine::with_devices`] instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NetSource {
    /// A port of this in-process hub: the card sees every frame the other ports send.
    /// Machines whose configs name the same hub are on one network.
    Hub(Hub),
    /// User networking: a NAT gateway of its own (the `gateway` crate) that gives the
    /// card an address by DHCP, answers DNS, and reaches the internet and the host
    /// (10.0.2.2 is the host's 127.0.0.1) through ordinary sockets. Each host port in
    /// `hostfwd` is forwarded to a port of the guest.
    User { hostfwd: Vec<HostFwd> },
}

impl NetConfig {
    /// A card on a port of `hub`, with the port's default MAC address.
    pub fn hub(hub: &Hub) -> Self {
        NetConfig {
            source: NetSource::Hub(hub.clone()),
            mac: None,
            pcap: None,
        }
    }

    /// A card with user networking, forwarding these host ports to the guest.
    pub fn user(hostfwd: Vec<HostFwd>) -> Self {
        NetConfig {
            source: NetSource::User { hostfwd },
            mac: None,
            pcap: None,
        }
    }

    /// A card from a `--net` argument: a kind, then options after commas. The kind is
    /// `hub` (a port of `hub`, the process's hub) or `user` (user networking); the
    /// options are `mac=ADDR` and, for `user`, any number of
    /// `hostfwd=[tcp|udp]:[HOSTADDR]:HOSTPORT-[GUESTADDR]:GUESTPORT`.
    pub fn parse(arg: &str, hub: &Hub) -> Result<Self, String> {
        let mut parts = arg.split(',');
        let mut config = match parts.next() {
            Some("hub") => NetConfig::hub(hub),
            Some("user") => NetConfig::user(Vec::new()),
            _ => return Err(format!("'{arg}' is not a network: the kind is hub or user")),
        };
        for option in parts {
            match (option.split_once('='), &mut config.source) {
                (Some(("mac", mac)), _) => config.mac = Some(MacAddr::parse(mac)?),
                (Some(("hostfwd", rule)), NetSource::User { hostfwd }) => hostfwd.push(HostFwd::parse(rule)?),
                (_, NetSource::Hub(_)) => {
                    return Err(format!("unknown network option '{option}' (there is mac=ADDR)"))
                }
                (_, NetSource::User { .. }) => {
                    return Err(format!(
                        "unknown network option '{option}' (there are mac=ADDR and hostfwd=RULE)"
                    ))
                }
            }
        }
        Ok(config)
    }

    pub fn with_mac(self, mac: MacAddr) -> Self {
        NetConfig {
            mac: Some(mac),
            ..self
        }
    }

    pub fn with_pcap(self, path: impl Into<PathBuf>) -> Self {
        NetConfig {
            pcap: Some(path.into()),
            ..self
        }
    }

    /// Opens the backend this card describes, and picks its MAC address.
    pub fn open(&self) -> Result<(MacAddr, Box<dyn NetBackend>), String> {
        let (default_mac, backend): (MacAddr, Box<dyn NetBackend>) = match &self.source {
            NetSource::Hub(hub) => {
                let port = hub.attach();
                (port.default_mac(), Box::new(port))
            }
            NetSource::User { hostfwd } => {
                let config = gateway::Config {
                    hostfwd: hostfwd.clone(),
                    ..gateway::Config::default()
                };
                let gw = Gateway::new(config).map_err(|e| e.to_string())?;
                (MacAddr(gateway::DEFAULT_GUEST_MAC), Box::new(gw))
            }
        };
        let backend = match &self.pcap {
            None => backend,
            Some(path) => match Pcap::create(path, backend) {
                Ok(pcap) => Box::new(pcap),
                Err(e) => return Err(format!("{}: {e}", path.display())),
            },
        };
        Ok((self.mac.unwrap_or(default_mac), backend))
    }
}

/// One disk of a [`MachineConfig`]: where its sectors come from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiskConfig {
    pub source: DiskSource,
    /// Refuse writes (`BLK_FLAGS.0`); a file is then opened read-only.
    pub read_only: bool,
}

/// Where a configured disk's sectors live. A host with a backend of its own passes it
/// to [`crate::Machine::with_backends`] instead.
#[derive(Clone, PartialEq, Eq)]
pub enum DiskSource {
    /// An image file on the host, a whole number of 512-byte sectors. It is opened when
    /// the machine is built, and written in place.
    File(PathBuf),
    /// A disk in host memory that starts as a copy of these bytes (a whole number of
    /// sectors) and is gone when the machine is.
    Memory(Vec<u8>),
    /// Image `name` on the Lake storage server at `addr` (`host:port`): a network disk
    /// ([`LakeDisk`]), attached when the machine is built and detached when it is
    /// dropped.
    Lake { addr: String, name: String },
}

impl std::fmt::Debug for DiskSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DiskSource::File(path) => f.debug_tuple("File").field(path).finish(),
            // Not the bytes: an image can be megabytes long.
            DiskSource::Memory(image) => write!(f, "Memory({} bytes)", image.len()),
            DiskSource::Lake { addr, name } => write!(f, "Lake({}{addr}/{name})", avalon_proto::LAKE_SCHEME),
        }
    }
}

impl DiskConfig {
    /// A read-write disk backed by the image file at `path`.
    pub fn file(path: impl Into<PathBuf>) -> Self {
        DiskConfig {
            source: DiskSource::File(path.into()),
            read_only: false,
        }
    }

    /// A read-write disk in memory, starting as a copy of `image`.
    pub fn memory(image: Vec<u8>) -> Self {
        DiskConfig {
            source: DiskSource::Memory(image),
            read_only: false,
        }
    }

    /// A read-write disk on a Lake server: image `name` at `addr` (`host:port`).
    pub fn lake(addr: impl Into<String>, name: impl Into<String>) -> Self {
        DiskConfig {
            source: DiskSource::Lake {
                addr: addr.into(),
                name: name.into(),
            },
            read_only: false,
        }
    }

    /// A read-write disk from a command-line argument: `lake://addr/name` is a disk on
    /// a Lake server (`addr` is `host:port`, or a host alone for port 7401), anything
    /// else the path of an image file.
    pub fn parse(arg: &str) -> Result<Self, String> {
        Ok(match avalon_proto::DiskSource::parse(arg)? {
            avalon_proto::DiskSource::File(path) => DiskConfig::file(path),
            avalon_proto::DiskSource::Lake { addr, name } => DiskConfig::lake(addr, name),
        })
    }

    pub fn with_read_only(self, read_only: bool) -> Self {
        DiskConfig { read_only, ..self }
    }

    /// Opens the backend this disk describes.
    pub fn open(&self) -> Result<Box<dyn BlockBackend>, String> {
        match &self.source {
            DiskSource::File(path) => match FileDisk::open(path, self.read_only) {
                Ok(disk) => Ok(Box::new(disk)),
                Err(e) => Err(format!("{}: {e}", path.display())),
            },
            DiskSource::Memory(image) => match MemDisk::from_image(image.clone()) {
                Ok(disk) => Ok(Box::new(disk.with_read_only(self.read_only))),
                Err(e) => Err(format!("memory image: {e}")),
            },
            DiskSource::Lake { addr, name } => match LakeDisk::open(addr, name, self.read_only) {
                Ok(disk) => Ok(Box::new(disk)),
                Err(e) => Err(e),
            },
        }
    }
}

impl Default for MachineConfig {
    fn default() -> Self {
        MachineConfig {
            ram_size: DEFAULT_RAM_SIZE,
            clock_hz: None,
            nominal_hz: DEFAULT_NOMINAL_HZ,
            harts: 1,
            disks: Vec::new(),
            nets: Vec::new(),
            paging: Paging::Sv48,
            bios: Bios::None,
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
        let max = MAX_RAM_SIZE;
        if ram == 0 || ram > max || !ram.is_multiple_of(RAM_GRANULE) {
            return Err(format!(
                "RAM size {ram:#x} must be a non-zero multiple of 4 KiB, at most {max:#x} (a machine has at most 1 TiB)"
            ));
        }
        if self.clock_hz == Some(0) || self.nominal_hz == 0 {
            return Err("clock frequency must not be 0 Hz".into());
        }
        if !(1..=MAX_HARTS).contains(&self.harts) {
            return Err(format!("{} harts, must be 1 to {MAX_HARTS}", self.harts));
        }
        if self.disks.len() > MAX_DISKS {
            return Err(format!("{} disks, at most {MAX_DISKS}", self.disks.len()));
        }
        // Image files and Lake disks are checked when they are opened (`Machine::new`).
        for disk in &self.disks {
            match &disk.source {
                DiskSource::Memory(image) => {
                    let len = image.len();
                    if !len.is_multiple_of(SECTOR_SIZE) || (len / SECTOR_SIZE) as u64 > MAX_SECTORS {
                        return Err(format!(
                            "memory disk of {len} bytes must be a whole number of {SECTOR_SIZE}-byte sectors, at most {MAX_SECTORS}"
                        ));
                    }
                }
                DiskSource::Lake { addr, name } => {
                    avalon_proto::validate_name(name)
                        .map_err(|e| format!("disk {}{addr}/{name}: {e}", avalon_proto::LAKE_SCHEME))?;
                }
                DiskSource::File(_) => {}
            }
        }
        if self.nets.len() > MAX_NETS {
            return Err(format!("{} network cards, at most {MAX_NETS}", self.nets.len()));
        }
        for (i, net) in self.nets.iter().enumerate() {
            if let Some(mac) = net.mac {
                mac.check_card().map_err(|e| format!("network card {i}: {e}"))?;
            }
        }
        Ok(())
    }
}

/// A size of memory as people type it: a whole number of MiB, or a whole number with a
/// unit, `M`/`MiB`, `G`/`GiB` or `T`/`TiB` in any case (`64`, `64M`, `16G`, `2TiB`).
/// `Ok(None)` if it is one but does not fit in 64 bits.
fn size_in_bytes(s: &str) -> Result<Option<u64>, String> {
    let bad = || format!("'{s}' is not a RAM size (examples: 64, 64M, 512MiB, 16G, 16GiB)");
    let split = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    let (number, unit) = s.split_at(split);
    let scale: u64 = match unit.to_ascii_lowercase().as_str() {
        "" | "m" | "mib" => 1 << 20,
        "g" | "gib" => 1 << 30,
        "t" | "tib" => 1 << 40,
        _ => return Err(bad()),
    };
    if number.is_empty() {
        return Err(bad());
    }
    Ok(number.parse::<u64>().ok().and_then(|n| n.checked_mul(scale)))
}

/// Parses a size of memory with no limit but 64 bits (a RAM budget: Avalon's
/// `--max-ram`): the syntax of `--ram` ([`parse_ram_size`]), at least 1 MiB. Returns
/// bytes.
pub fn parse_size(s: &str) -> Result<u64, String> {
    let s = s.trim();
    match size_in_bytes(s)? {
        Some(0) => Err(format!("{s} is too small (at least 1 MiB)")),
        Some(size) => Ok(size),
        None => Err(format!("{s} does not fit in 64 bits")),
    }
}

/// Parses a `--ram` value (spec section 7, "RAM"): a whole number of MiB, or a whole
/// number with a unit, `M`/`MiB`, `G`/`GiB` or `T`/`TiB` in any case (`64`, `64M`,
/// `16G`, `16GiB`, `1T`). From 1 MiB to 1 TiB. Returns the size in bytes.
pub fn parse_ram_size(s: &str) -> Result<usize, String> {
    let s = s.trim();
    match size_in_bytes(s)? {
        Some(size) if (MIB as u64..=MAX_RAM_SIZE as u64).contains(&size) => Ok(size as usize),
        _ => Err(format!("{s} is out of range (1 MiB to 1 TiB)")),
    }
}

/// Parses a `--paging` value: `sv48` (Sv48 and Sv39) or `sv39` (Sv39 only).
pub fn parse_paging(s: &str) -> Result<Paging, String> {
    match s.trim() {
        "sv48" => Ok(Paging::Sv48),
        "sv39" => Ok(Paging::Sv39),
        s => Err(format!("'{s}' is not a paging mode (sv39 or sv48)")),
    }
}

/// Parses a `--bios` value, as QEMU's `-bios`: `none`, `default` (the built-in
/// firmware) or a file's path.
pub fn parse_bios(s: &str) -> Result<Bios, String> {
    match s {
        "" => Err("give none, default or a firmware file".into()),
        "none" => Ok(Bios::None),
        "default" => Ok(Bios::Default),
        path => Ok(Bios::File(PathBuf::from(path))),
    }
}

/// Parses a `--harts` value: a whole number from 1 to [`MAX_HARTS`].
pub fn parse_harts(s: &str) -> Result<usize, String> {
    let s = s.trim();
    let n: usize = match s.parse() {
        Ok(n) if s.bytes().all(|b| b.is_ascii_digit()) => n,
        _ => return Err(format!("'{s}' is not a whole number of harts")),
    };
    if !(1..=MAX_HARTS).contains(&n) {
        return Err(format!("{n} harts is out of range (1 to {MAX_HARTS})"));
    }
    Ok(n)
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
        assert_eq!(c.ram_size, 4 * MIB);
        assert_eq!(c.clock_hz, None);
        assert_eq!(c.cpu_hz(), 10_000_000);
        assert_eq!(c.harts, 1);
        assert_eq!(c.validate(), Ok(()));
        let paced = MachineConfig {
            clock_hz: Some(5),
            ..c
        };
        assert_eq!(paced.cpu_hz(), 5);
    }

    #[test]
    fn validate_rejects_bad_sizes_and_zero_clocks() {
        assert_eq!(MAX_RAM_SIZE, 0x100_0000_0000);
        // RAM ends at 0x100_7FFF_FFFF at most: 48-bit physical addresses.
        assert_eq!(crate::bus::RAM_BASE + MAX_RAM_SIZE as u64 - 1, 0x100_7fff_ffff);
        let config = MachineConfig::with_ram;
        for ram in [
            4096,
            64 * 1024,
            MIB,
            2 * GIB,
            2 * GIB + RAM_GRANULE,
            16 * GIB,
            MAX_RAM_SIZE,
        ] {
            assert_eq!(config(ram).validate(), Ok(()), "{ram:#x}");
        }
        for ram in [0, 1, 4095, 4097] {
            assert!(config(ram).validate().is_err(), "{ram:#x}");
        }
        let e = config(MAX_RAM_SIZE + RAM_GRANULE).validate().unwrap_err();
        assert!(e.contains("a machine has at most 1 TiB"), "{e}");
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
    fn validate_checks_disks() {
        assert!(MachineConfig::default().disks.is_empty());
        let disks = |disks: Vec<DiskConfig>| MachineConfig {
            disks,
            ..MachineConfig::default()
        };
        let sector = || DiskConfig::memory(vec![0; SECTOR_SIZE]);
        assert_eq!(disks(vec![sector(); MAX_DISKS]).validate(), Ok(()));
        let e = disks(vec![sector(); MAX_DISKS + 1]).validate().unwrap_err();
        assert!(e.contains("at most 8"), "{e}");
        assert_eq!(disks(vec![DiskConfig::memory(Vec::new())]).validate(), Ok(()));
        let e = disks(vec![DiskConfig::memory(vec![0; 100])])
            .validate()
            .unwrap_err();
        assert!(e.contains("whole number"), "{e}");
        // Files are only looked at when opened.
        assert_eq!(
            disks(vec![DiskConfig::file("no/such/file.img")]).validate(),
            Ok(())
        );
        let e = DiskConfig::file("no/such/file.img").open().err().unwrap();
        assert!(e.starts_with("no/such/file.img: "), "{e}");
    }

    #[test]
    fn disk_configs_open_their_backends() {
        let mut image = vec![0; 2 * SECTOR_SIZE];
        image[SECTOR_SIZE] = 0x42;
        let config = DiskConfig::memory(image.clone());
        assert!(!config.read_only);
        let mut disk = config.open().unwrap();
        assert_eq!(disk.capacity(), 2);
        assert!(!disk.read_only());
        let mut buf = [0; SECTOR_SIZE];
        disk.read(1, &mut buf).unwrap();
        assert_eq!(buf[0], 0x42);
        disk.write(1, &[0; SECTOR_SIZE]).unwrap();
        // The backend has its own copy: the config (and the next machine) are unchanged.
        assert_eq!(config.source, DiskSource::Memory(image));
        let ro = config.with_read_only(true).open().unwrap();
        assert!(ro.read_only());
        let debug = format!("{:?}", DiskConfig::memory(vec![0; 1024]).source);
        assert_eq!(debug, "Memory(1024 bytes)");
    }

    #[test]
    fn lake_disks() {
        assert_eq!(
            DiskConfig::parse("lake://127.0.0.1:7401/root").unwrap(),
            DiskConfig::lake("127.0.0.1:7401", "root")
        );
        assert_eq!(
            DiskConfig::parse("lake://storage/root").unwrap(),
            DiskConfig::lake("storage:7401", "root")
        );
        assert_eq!(
            DiskConfig::parse("images/root.img").unwrap(),
            DiskConfig::file("images/root.img")
        );
        assert!(DiskConfig::parse("lake://storage/Bad Name").is_err());
        let debug = format!("{:?}", DiskConfig::lake("h:1", "root").source);
        assert_eq!(debug, "Lake(lake://h:1/root)");

        // A bad name fails validation, before anything connects.
        let config = MachineConfig {
            disks: vec![DiskConfig::lake("127.0.0.1:7401", "../etc")],
            ..MachineConfig::default()
        };
        let e = config.validate().unwrap_err();
        assert!(e.starts_with("disk lake://127.0.0.1:7401/../etc: "), "{e}");

        // Nothing listening: opening fails, with the disk's address in the message.
        let port = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            listener.local_addr().unwrap().port()
        };
        let addr = format!("127.0.0.1:{port}");
        let e = DiskConfig::lake(&addr, "root").open().err().unwrap();
        assert!(e.starts_with(&format!("lake://{addr}/root: ")), "{e}");
        let e = DiskConfig::lake("no such host:1", "root").open().err().unwrap();
        assert!(e.starts_with("lake://no such host:1/root: "), "{e}");
    }

    #[test]
    fn net_configs() {
        let hub = Hub::new();
        assert!(MachineConfig::default().nets.is_empty());
        assert_eq!(NetConfig::parse("hub", &hub), Ok(NetConfig::hub(&hub)));
        let mac = MacAddr([2, 0, 0, 0, 0, 9]);
        assert_eq!(
            NetConfig::parse("hub,mac=02:00:00:00:00:09", &hub),
            Ok(NetConfig::hub(&hub).with_mac(mac))
        );
        assert_eq!(NetConfig::parse("user", &hub), Ok(NetConfig::user(Vec::new())));
        assert_eq!(
            NetConfig::parse(
                "user,hostfwd=tcp::8080-:80,mac=02:00:00:00:00:09,hostfwd=udp::5353-:53",
                &hub
            ),
            Ok(NetConfig::user(vec![HostFwd::tcp(8080, 80), HostFwd::udp(5353, 53)]).with_mac(mac))
        );
        for (bad, why) in [
            ("", "not a network"),
            ("users", "not a network"),
            ("Hub", "not a network"),
            ("hub,", "unknown network option ''"),
            ("hub,mtu=1500", "unknown network option 'mtu=1500'"),
            ("hub,mac=02:00", "not a MAC address"),
            (
                "hub,hostfwd=tcp::1-:2",
                "unknown network option 'hostfwd=tcp::1-:2' (there is mac=ADDR)",
            ),
            (
                "user,mtu=1500",
                "unknown network option 'mtu=1500' (there are mac=ADDR and hostfwd=RULE)",
            ),
            ("user,hostfwd=tcp::80", "not a forwarding rule"),
        ] {
            let e = NetConfig::parse(bad, &hub).unwrap_err();
            assert!(e.contains(why), "{bad:?}: {e}");
        }
        // Configs name hubs by identity.
        assert_ne!(NetConfig::hub(&hub), NetConfig::hub(&Hub::new()));

        let nets = |nets: Vec<NetConfig>| MachineConfig {
            nets,
            ..MachineConfig::default()
        };
        assert_eq!(nets(vec![NetConfig::hub(&hub); MAX_NETS]).validate(), Ok(()));
        let e = nets(vec![NetConfig::hub(&hub); MAX_NETS + 1])
            .validate()
            .unwrap_err();
        assert_eq!(e, "5 network cards, at most 4");
        let group = NetConfig::hub(&hub).with_mac(MacAddr([1, 0, 0x5e, 0, 0, 1]));
        let e = nets(vec![NetConfig::hub(&hub), group]).validate().unwrap_err();
        assert_eq!(
            e,
            "network card 1: 01:00:5e:00:00:01 is a group address, not a card's"
        );
        let universal = NetConfig::hub(&hub).with_mac(MacAddr([0, 0x11, 0x22, 0x33, 0x44, 0x55]));
        let e = nets(vec![universal]).validate().unwrap_err();
        assert_eq!(
            e,
            "network card 0: 00:11:22:33:44:55 is not locally administered (bit 1 of the first byte, as in 02:..)"
        );
    }

    #[test]
    fn net_configs_open_their_backends() {
        let hub = Hub::new();
        let (mac1, mut card1) = NetConfig::hub(&hub).open().unwrap();
        let (mac2, mut card2) = NetConfig::hub(&hub).open().unwrap();
        let mac = MacAddr([2, 0, 0, 0, 0, 9]);
        let (mac3, _card3) = NetConfig::hub(&hub).with_mac(mac).open().unwrap();
        assert_eq!([mac1, mac2, mac3], [MacAddr::local(1), MacAddr::local(2), mac]);
        assert_eq!(hub.ports(), 3);
        card1.send(&[7; 14]).unwrap();
        assert_eq!(card2.recv(), Some(vec![7; 14]));

        // A capture file that cannot be created fails the card, naming the file.
        let e = NetConfig::hub(&hub)
            .with_pcap("no/such/dir/x.pcap")
            .open()
            .err()
            .unwrap();
        assert!(e.starts_with("no/such/dir/x.pcap: "), "{e}");
        assert_eq!(hub.ports(), 3, "and leaves no port behind");
    }

    #[test]
    fn harts_1_to_max() {
        for n in 1..=MAX_HARTS {
            assert_eq!(parse_harts(&n.to_string()), Ok(n));
            let config = MachineConfig {
                harts: n,
                ..MachineConfig::default()
            };
            assert_eq!(config.validate(), Ok(()));
        }
        assert_eq!(parse_harts(" 4 "), Ok(4));
        for (bad, why) in [
            ("0", "out of range (1 to 8)"),
            ("9", "out of range"),
            ("99999999999999999999999", "not a whole number"),
            ("-1", "not a whole number"),
            ("+2", "not a whole number"),
            ("2.0", "not a whole number"),
            ("", "not a whole number"),
            ("two", "not a whole number"),
        ] {
            let e = parse_harts(bad).unwrap_err();
            assert!(e.contains(why), "{bad:?}: {e}");
        }
        for harts in [0, MAX_HARTS + 1] {
            let config = MachineConfig {
                harts,
                ..MachineConfig::default()
            };
            assert_eq!(config.validate(), Err(format!("{harts} harts, must be 1 to 8")));
        }
    }

    #[test]
    fn user_net_configs_open_a_gateway() {
        let (mac, mut card) = NetConfig::user(vec![HostFwd::tcp(0, 80)]).open().unwrap();
        assert_eq!(mac, MacAddr([0x52, 0x54, 0, 0x12, 0x34, 0x56]));
        assert!(card.link_up());
        // An ARP request for the gateway gets its answer.
        let mut arp = vec![0xff; 6];
        arp.extend_from_slice(&mac.0);
        arp.extend_from_slice(&[0x08, 0x06, 0, 1, 0x08, 0, 6, 4, 0, 1]);
        arp.extend_from_slice(&mac.0);
        arp.extend_from_slice(&[10, 0, 2, 15, 0, 0, 0, 0, 0, 0, 10, 0, 2, 2]);
        card.send(&arp).unwrap();
        let reply = card.recv().unwrap();
        assert_eq!(
            (&reply[..6], &reply[6..12]),
            (&mac.0[..], &gateway::GATEWAY_MAC[..])
        );

        // A forwarded port that is taken fails the card, naming the rule.
        let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = taken.local_addr().unwrap().port();
        let e = NetConfig::user(vec![HostFwd::tcp(port, 80)])
            .open()
            .err()
            .unwrap();
        assert!(
            e.starts_with(&format!("hostfwd tcp:127.0.0.1:{port}-10.0.2.15:80: ")),
            "{e}"
        );
    }

    #[test]
    fn ram_sizes_in_mib_or_gib() {
        for (s, size) in [
            ("1", MIB),
            ("128", 128 * MIB),
            ("2048", 2 * GIB),
            (" 64 ", 64 * MIB),
            ("64M", 64 * MIB),
            ("64m", 64 * MIB),
            ("64MiB", 64 * MIB),
            ("64mib", 64 * MIB),
            ("16G", 16 * GIB),
            ("16g", 16 * GIB),
            ("16GiB", 16 * GIB),
            ("16GIB", 16 * GIB),
            ("1024G", MAX_RAM_SIZE),
            ("1048576", MAX_RAM_SIZE),
            ("1T", MAX_RAM_SIZE),
            ("1tib", MAX_RAM_SIZE),
        ] {
            assert_eq!(parse_ram_size(s), Ok(size), "{s:?}");
        }
        for good in ["1", "16", "2048", "2G"] {
            let size = parse_ram_size(good).unwrap();
            assert_eq!(MachineConfig::with_ram(size).validate(), Ok(()));
        }
    }

    #[test]
    fn budgets_take_the_same_sizes_without_the_ram_limit() {
        assert_eq!(parse_size("4096"), Ok(4 << 30));
        assert_eq!(parse_size("2T"), Ok(2 << 40));
        assert_eq!(parse_size("16384G"), Ok(16 << 40));
        assert_eq!(parse_size(" 512MiB "), Ok(512 << 20));
        assert!(parse_size("0").unwrap_err().contains("at least 1 MiB"));
        assert!(parse_size("99999999999T").unwrap_err().contains("64 bits"));
        assert!(parse_size("64MB").unwrap_err().contains("is not a RAM size"));
    }

    #[test]
    fn ram_rejects_out_of_range_and_garbage() {
        for big in [
            "0",
            "0G",
            "1025G",
            "1048577",
            "2T",
            "99999999999999999999999",
            "99999999999999999G",
        ] {
            let e = parse_ram_size(big).unwrap_err();
            assert!(e.contains("out of range (1 MiB to 1 TiB)"), "{big:?}: {e}");
        }
        for bad in [
            "-1", "+5", "1.5", "1.5G", "12K", "12KiB", "12P", "G", "", "abc", "0x10", "64 M", "64MB",
        ] {
            let e = parse_ram_size(bad).unwrap_err();
            assert!(e.contains("is not a RAM size"), "{bad:?}: {e}");
        }
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
