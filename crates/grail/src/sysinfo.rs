//! Xcal sysinfo (spec section 9.5): read-only registers that tell software what machine
//! it runs on (RAM, clock, harts) and a table of the devices it has, so a kernel
//! discovers its hardware instead of hard-coding it.
//!
//! The layout is version 2's (spec section 9.5): 64-bit registers for the RAM base and
//! size and for the firmware's reserved RAM, and 24-byte device entries whose base and
//! size are 64-bit. (Version 1, all 32-bit, was RV32's, and went with it.)

use crate::bus::{BusFault, RAM_BASE};
use crate::config::MachineConfig;
use crate::mmio::{self, Field};

/// "SYSI" when the register is read as big-endian hex: 53 59 53 49.
pub const SYSINFO_MAGIC: u32 = 0x5359_5349;
/// The layout's version, 2. A version changes only if existing registers change
/// meaning or place; new registers and new device types do not need one.
pub const SYSINFO_VERSION: u32 = 2;

// Register offsets from the start of the block.
pub const REG_MAGIC: u32 = 0x00;
pub const REG_VERSION: u32 = 0x04;
/// 64-bit.
pub const REG_RAM_BASE: u32 = 0x08;
/// 64-bit.
pub const REG_RAM_SIZE: u32 = 0x10;
pub const REG_TIMEBASE_HZ: u32 = 0x18;
pub const REG_CPU_HZ: u32 = 0x1c;
pub const REG_FLAGS: u32 = 0x20;
pub const REG_DEV_COUNT: u32 = 0x24;
/// Number of harts; their `mhartid`s are 0 to this minus 1.
pub const REG_HARTS: u32 = 0x28;
/// 64-bit: where the RAM the firmware keeps for itself starts (spec section 7,
/// "Firmware"), 0 without firmware.
pub const REG_RESERVED_BASE: u32 = 0x30;
/// 64-bit: its size, 0 without firmware.
pub const REG_RESERVED_SIZE: u32 = 0x38;
/// First device table entry.
pub const DEV_TABLE: u32 = 0x40;
/// Bytes per device table entry: type (+0), irq (+4), base (+8, 64-bit), size
/// (+0x10, 64-bit).
pub const DEV_ENTRY_SIZE: u32 = 24;
/// The fields of a device table entry, as offsets into it.
pub const DEV_TYPE: u32 = 0x00;
pub const DEV_IRQ: u32 = 0x04;
pub const DEV_BASE: u32 = 0x08;
pub const DEV_SIZE: u32 = 0x10;
/// Entries that fit in the 4 KiB block.
pub const DEV_MAX: usize = ((0x1000 - DEV_TABLE) / DEV_ENTRY_SIZE) as usize;

/// `SYS_FLAGS` bit 0: the clock is real time (`--clock`), not just nominal.
pub const FLAG_REAL_TIME: u32 = 1 << 0;

/// Device type codes in the device table. Codes are never reused; a new kind of device
/// gets the next free one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum DeviceType {
    /// Core-local interruptor (msip, mtimecmp, mtime).
    Clint = 1,
    /// Platform-level interrupt controller.
    Plic = 2,
    /// 16550-compatible UART.
    Uart = 3,
    /// SiFive-style test finisher (exit device).
    TestFinisher = 4,
    /// Framebuffer control registers.
    FbCtrl = 5,
    /// Framebuffer pixel memory.
    FbVram = 6,
    /// This block.
    Sysinfo = 7,
    /// Xcal keyboard (spec section 9.4).
    Keyboard = 8,
    /// Xcal block device, one entry per disk (spec section 9.6).
    Block = 9,
    /// Xcal pointer (mouse/tablet, spec section 9.7).
    Pointer = 10,
    /// Xcal network card, one entry per card (spec section 9.8).
    Net = 11,
    /// Real-time clock, the Goldfish RTC (spec section 9.9).
    Rtc = 12,
}

/// One device table entry. `irq` is the device's PLIC source, 0 if it has none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeviceEntry {
    pub kind: DeviceType,
    pub base: u64,
    pub size: u64,
    pub irq: u32,
}

pub struct Sysinfo {
    ram_size: u64,
    cpu_hz: u32,
    flags: u32,
    harts: u32,
    devices: Vec<DeviceEntry>,
    /// The RAM the firmware keeps, as (base, size): (0, 0) without firmware.
    reserved: (u64, u64),
}

impl Sysinfo {
    /// `config` must be valid (see [`MachineConfig::validate`]).
    pub fn new(config: &MachineConfig, devices: Vec<DeviceEntry>) -> Self {
        assert!(devices.len() <= DEV_MAX, "too many devices for the sysinfo table");
        Sysinfo {
            ram_size: config.ram_size as u64,
            cpu_hz: config.cpu_hz(),
            flags: if config.clock_hz.is_some() {
                FLAG_REAL_TIME
            } else {
                0
            },
            harts: u32::try_from(config.harts).expect("a valid hart count fits in 32 bits"),
            devices,
            reserved: (0, 0),
        }
    }

    /// The RAM the machine's firmware keeps for itself, `size` bytes from `base`
    /// (`SYS_RESERVED_BASE`, `SYS_RESERVED_SIZE`): set when the
    /// firmware is loaded ([`crate::Machine::load_elf`] with `--bios`).
    pub fn set_reserved(&mut self, base: u64, size: u64) {
        self.reserved = (base, size);
    }

    /// The RAM the firmware keeps, as (base, size).
    pub fn reserved(&self) -> (u64, u64) {
        self.reserved
    }

    pub fn devices(&self) -> &[DeviceEntry] {
        &self.devices
    }

    /// What `off` holds, with the register's value.
    fn field(&self, off: u32) -> Field<u64> {
        let word = match off {
            REG_MAGIC => SYSINFO_MAGIC,
            REG_VERSION => SYSINFO_VERSION,
            REG_RAM_BASE => return Field::Low(RAM_BASE),
            o if o == REG_RAM_BASE + 4 => return Field::High(RAM_BASE),
            REG_RAM_SIZE => return Field::Low(self.ram_size),
            o if o == REG_RAM_SIZE + 4 => return Field::High(self.ram_size),
            // One instruction = one cycle = one mtime tick: both run at the same rate.
            REG_TIMEBASE_HZ | REG_CPU_HZ => self.cpu_hz,
            REG_FLAGS => self.flags,
            REG_DEV_COUNT => self.devices.len() as u32,
            REG_HARTS => self.harts,
            REG_RESERVED_BASE => return Field::Low(self.reserved.0),
            o if o == REG_RESERVED_BASE + 4 => return Field::High(self.reserved.0),
            REG_RESERVED_SIZE => return Field::Low(self.reserved.1),
            o if o == REG_RESERVED_SIZE + 4 => return Field::High(self.reserved.1),
            o if o >= DEV_TABLE => {
                let index = ((o - DEV_TABLE) / DEV_ENTRY_SIZE) as usize;
                let Some(d) = self.devices.get(index) else {
                    return Field::Unused;
                };
                return match (o - DEV_TABLE) % DEV_ENTRY_SIZE {
                    DEV_TYPE => Field::Word(d.kind as u64),
                    DEV_IRQ => Field::Word(d.irq.into()),
                    DEV_BASE => Field::Low(d.base),
                    f if f == DEV_BASE + 4 => Field::High(d.base),
                    DEV_SIZE => Field::Low(d.size),
                    _ => Field::High(d.size),
                };
            }
            _ => return Field::Unused,
        };
        Field::Word(word.into())
    }

    /// Reads a register, at the widths of spec section 9 ([`mmio::access`]). Unused
    /// offsets, and entries past the device count, read as 0.
    pub fn read(&self, off: u32, size: u32) -> Result<u64, BusFault> {
        let access = mmio::access(off, size, |o| self.field(o))?;
        Ok(access.read(|value| value))
    }

    /// Everything is read-only: writes of a width [`Sysinfo::read`] takes are ignored,
    /// others fault.
    pub fn write(&self, off: u32, size: u32, _val: u64) -> Result<(), BusFault> {
        self.read(off, size).map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::MIB;

    fn devices() -> Vec<DeviceEntry> {
        vec![
            DeviceEntry {
                kind: DeviceType::Uart,
                base: 0x1000_0000,
                size: 0x100,
                irq: 1,
            },
            DeviceEntry {
                kind: DeviceType::Clint,
                base: 0x0200_0000,
                size: 0x1_0000,
                irq: 0,
            },
        ]
    }

    #[test]
    fn header_registers() {
        let s = Sysinfo::new(&MachineConfig::with_ram(64 * MIB), devices());
        assert_eq!(s.read(REG_MAGIC, 4), Ok(0x5359_5349));
        assert_eq!(s.read(REG_VERSION, 4), Ok(2));
        assert_eq!(s.read(REG_RAM_BASE, 4), Ok(0x8000_0000));
        assert_eq!(s.read(REG_RAM_SIZE, 4), Ok(0x0400_0000));
        assert_eq!(s.read(REG_TIMEBASE_HZ, 4), Ok(10_000_000));
        assert_eq!(s.read(REG_CPU_HZ, 4), Ok(10_000_000));
        assert_eq!(s.read(REG_FLAGS, 4), Ok(0), "nominal clock");
        assert_eq!(s.read(REG_DEV_COUNT, 4), Ok(2));
        assert_eq!(s.read(REG_HARTS, 4), Ok(1));
        assert_eq!(s.read(REG_RESERVED_BASE, 8), Ok(0), "no firmware");
        assert_eq!(s.read(REG_RESERVED_SIZE, 8), Ok(0));
        assert_eq!(s.read(0x2c, 4), Ok(0), "reserved");
    }

    #[test]
    fn hart_count() {
        let config = MachineConfig {
            harts: 4,
            ..MachineConfig::default()
        };
        let s = Sysinfo::new(&config, Vec::new());
        assert_eq!(s.read(REG_HARTS, 4), Ok(4));
        assert_eq!(s.read(REG_HARTS, 2), Err(BusFault));
    }

    #[test]
    fn real_time_clock_and_largest_ram() {
        let config = MachineConfig {
            ram_size: 0x8000_0000,
            clock_hz: Some(1_500_000),
            ..MachineConfig::default()
        };
        let s = Sysinfo::new(&config, Vec::new());
        assert_eq!(s.read(REG_RAM_SIZE, 8), Ok(0x8000_0000));
        assert_eq!(s.read(REG_TIMEBASE_HZ, 4), Ok(1_500_000));
        assert_eq!(s.read(REG_CPU_HZ, 4), Ok(1_500_000));
        assert_eq!(s.read(REG_FLAGS, 4), Ok(FLAG_REAL_TIME.into()));
        assert_eq!(s.read(REG_DEV_COUNT, 4), Ok(0));
        assert_eq!(s.read(DEV_TABLE, 4), Ok(0));
    }

    #[test]
    fn nominal_clock_is_configurable() {
        let config = MachineConfig {
            nominal_hz: 32_768,
            ..MachineConfig::default()
        };
        let s = Sysinfo::new(&config, Vec::new());
        assert_eq!(s.read(REG_TIMEBASE_HZ, 4), Ok(32_768));
        assert_eq!(s.read(REG_FLAGS, 4), Ok(0));
    }

    #[test]
    fn read_only_and_word_only() {
        let s = Sysinfo::new(&MachineConfig::default(), devices());
        for off in [REG_MAGIC, REG_RAM_SIZE, REG_CPU_HZ, REG_DEV_COUNT, DEV_TABLE] {
            assert_eq!(s.write(off, 4, 0xdead_beef), Ok(()));
        }
        assert_eq!(s.read(REG_RAM_SIZE, 4), Ok(4 * MIB as u64));
        assert_eq!(s.read(REG_DEV_COUNT, 4), Ok(2));
        assert_eq!(s.read(REG_MAGIC, 1), Err(BusFault));
        assert_eq!(s.read(REG_MAGIC, 2), Err(BusFault));
        assert_eq!(s.write(REG_MAGIC, 1, 0), Err(BusFault));
        assert_eq!(s.write(REG_MAGIC, 2, 0), Err(BusFault));
        assert_eq!(s.write(REG_MAGIC, 8, 0), Err(BusFault));
    }

    #[test]
    fn version_2_header() {
        let config = MachineConfig {
            ram_size: 0x100_0000_0000,
            clock_hz: Some(1_500_000),
            harts: 3,
            ..MachineConfig::default()
        };
        let s = Sysinfo::new(&config, devices());
        assert_eq!(s.read(REG_MAGIC, 4), Ok(0x5359_5349));
        assert_eq!(s.read(REG_VERSION, 4), Ok(2));
        // The 64-bit registers, whole and by halves.
        assert_eq!(s.read(REG_RAM_BASE, 8), Ok(0x8000_0000));
        assert_eq!(s.read(REG_RAM_BASE, 4), Ok(0x8000_0000));
        assert_eq!(s.read(REG_RAM_BASE + 4, 4), Ok(0));
        assert_eq!(s.read(REG_RAM_SIZE, 8), Ok(0x100_0000_0000), "1 TiB");
        assert_eq!(s.read(REG_RAM_SIZE, 4), Ok(0));
        assert_eq!(s.read(REG_RAM_SIZE + 4, 4), Ok(0x100));
        assert_eq!(s.read(REG_TIMEBASE_HZ, 4), Ok(1_500_000));
        assert_eq!(s.read(REG_CPU_HZ, 4), Ok(1_500_000));
        assert_eq!(s.read(REG_FLAGS, 4), Ok(FLAG_REAL_TIME.into()));
        assert_eq!(s.read(REG_DEV_COUNT, 4), Ok(2));
        assert_eq!(s.read(REG_HARTS, 4), Ok(3));
        for reserved in [0x2c, 0x30, 0x3c] {
            assert_eq!(s.read(reserved, 4), Ok(0));
        }
        assert_eq!(
            s.read(0x30, 8),
            Ok(0),
            "reserved space, at a width the block takes"
        );
        // 32-bit registers take no 8-byte reads, alone or in pairs.
        assert_eq!(s.read(REG_MAGIC, 8), Err(BusFault));
        assert_eq!(s.read(REG_TIMEBASE_HZ, 8), Err(BusFault));
        assert_eq!(s.read(REG_HARTS, 8), Err(BusFault), "harts and reserved");
        assert_eq!(s.read(REG_RAM_BASE, 2), Err(BusFault));
        // Read-only, at the widths it takes.
        assert_eq!(s.write(REG_RAM_SIZE, 8, 0), Ok(()));
        assert_eq!(s.read(REG_RAM_SIZE, 8), Ok(0x100_0000_0000));
        assert_eq!(s.write(REG_MAGIC, 8, 0), Err(BusFault));
    }

    #[test]
    fn version_2_device_table() {
        let mut devs = devices();
        devs[0].base = 0x12_3456_7000;
        devs[0].size = 0x1_0000_0000;
        let s = Sysinfo::new(&MachineConfig::default(), devs);
        let at = |i: u32, field: u32| DEV_TABLE + i * DEV_ENTRY_SIZE + field;
        assert_eq!(s.read(at(0, 0x00), 4), Ok(DeviceType::Uart as u64));
        assert_eq!(s.read(at(0, 0x04), 4), Ok(1), "irq");
        assert_eq!(s.read(at(0, 0x08), 8), Ok(0x12_3456_7000));
        assert_eq!(s.read(at(0, 0x08), 4), Ok(0x3456_7000));
        assert_eq!(s.read(at(0, 0x0c), 4), Ok(0x12));
        assert_eq!(s.read(at(0, 0x10), 8), Ok(0x1_0000_0000));
        assert_eq!(s.read(at(0, 0x14), 4), Ok(1));
        assert_eq!(s.read(at(1, 0x00), 4), Ok(DeviceType::Clint as u64));
        assert_eq!(s.read(at(1, 0x08), 8), Ok(0x0200_0000));
        assert_eq!(s.read(at(1, 0x10), 8), Ok(0x1_0000));
        // The type and irq words are 32-bit: not one 8-byte read.
        assert_eq!(s.read(at(1, 0x00), 8), Err(BusFault));
        // Past the count: zeros, at either width.
        assert_eq!(s.read(at(2, 0x08), 8), Ok(0));
        assert_eq!(s.read(0xff8, 8), Ok(0));
        assert_eq!(DEV_MAX, 168);
    }
}
