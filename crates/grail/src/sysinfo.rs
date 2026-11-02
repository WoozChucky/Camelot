//! Xcal sysinfo (spec section 9.5): read-only registers that tell software what machine
//! it runs on (RAM, clock) and a table of the devices it has, so a kernel discovers
//! its hardware instead of hard-coding it.

use crate::bus::{BusFault, RAM_BASE};
use crate::config::MachineConfig;

/// "SYSI" when the register is read as big-endian hex: 53 59 53 49.
pub const SYSINFO_MAGIC: u32 = 0x5359_5349;
/// Layout version. Bumped only if existing registers change meaning; new registers
/// and new device types do not need a new version.
pub const SYSINFO_VERSION: u32 = 1;

// Register offsets from the start of the block.
pub const REG_MAGIC: u32 = 0x00;
pub const REG_VERSION: u32 = 0x04;
pub const REG_RAM_BASE: u32 = 0x08;
pub const REG_RAM_SIZE: u32 = 0x0c;
pub const REG_TIMEBASE_HZ: u32 = 0x10;
pub const REG_CPU_HZ: u32 = 0x14;
pub const REG_FLAGS: u32 = 0x18;
pub const REG_DEV_COUNT: u32 = 0x1c;
/// First device table entry.
pub const DEV_TABLE: u32 = 0x40;
/// Bytes per device table entry: type, base, size, irq.
pub const DEV_ENTRY_SIZE: u32 = 16;
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
}

/// One device table entry. `irq` is the device's PLIC source, 0 if it has none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeviceEntry {
    pub kind: DeviceType,
    pub base: u32,
    pub size: u32,
    pub irq: u32,
}

pub struct Sysinfo {
    ram_size: u32,
    cpu_hz: u32,
    flags: u32,
    devices: Vec<DeviceEntry>,
}

impl Sysinfo {
    /// `config` must be valid (see [`MachineConfig::validate`]).
    pub fn new(config: &MachineConfig, devices: Vec<DeviceEntry>) -> Self {
        assert!(devices.len() <= DEV_MAX, "too many devices for the sysinfo table");
        Sysinfo {
            ram_size: u32::try_from(config.ram_size).expect("RAM size fits in 32 bits"),
            cpu_hz: config.cpu_hz(),
            flags: if config.clock_hz.is_some() {
                FLAG_REAL_TIME
            } else {
                0
            },
            devices,
        }
    }

    pub fn devices(&self) -> &[DeviceEntry] {
        &self.devices
    }

    /// Registers are 32-bit only. Unused offsets, and entries past the device count, read as 0.
    pub fn read(&self, off: u32, size: u32) -> Result<u32, BusFault> {
        if size != 4 {
            return Err(BusFault);
        }
        Ok(match off {
            REG_MAGIC => SYSINFO_MAGIC,
            REG_VERSION => SYSINFO_VERSION,
            REG_RAM_BASE => RAM_BASE,
            REG_RAM_SIZE => self.ram_size,
            // One instruction = one cycle = one mtime tick: both run at the same rate.
            REG_TIMEBASE_HZ | REG_CPU_HZ => self.cpu_hz,
            REG_FLAGS => self.flags,
            REG_DEV_COUNT => self.devices.len() as u32,
            o if o >= DEV_TABLE => {
                let index = ((o - DEV_TABLE) / DEV_ENTRY_SIZE) as usize;
                match self.devices.get(index) {
                    Some(d) => match (o - DEV_TABLE) % DEV_ENTRY_SIZE {
                        0 => d.kind as u32,
                        4 => d.base,
                        8 => d.size,
                        12 => d.irq,
                        _ => 0,
                    },
                    None => 0,
                }
            }
            _ => 0,
        })
    }

    /// Everything is read-only: word writes are ignored, narrower ones fault.
    pub fn write(&self, _off: u32, size: u32, _val: u32) -> Result<(), BusFault> {
        if size != 4 {
            return Err(BusFault);
        }
        Ok(())
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
        assert_eq!(s.read(REG_VERSION, 4), Ok(1));
        assert_eq!(s.read(REG_RAM_BASE, 4), Ok(0x8000_0000));
        assert_eq!(s.read(REG_RAM_SIZE, 4), Ok(0x0400_0000));
        assert_eq!(s.read(REG_TIMEBASE_HZ, 4), Ok(10_000_000));
        assert_eq!(s.read(REG_CPU_HZ, 4), Ok(10_000_000));
        assert_eq!(s.read(REG_FLAGS, 4), Ok(0), "nominal clock");
        assert_eq!(s.read(REG_DEV_COUNT, 4), Ok(2));
        for reserved in [0x20, 0x24, 0x3c] {
            assert_eq!(s.read(reserved, 4), Ok(0));
        }
    }

    #[test]
    fn real_time_clock_and_largest_ram() {
        let config = MachineConfig {
            ram_size: 0x8000_0000,
            clock_hz: Some(1_500_000),
            ..MachineConfig::default()
        };
        let s = Sysinfo::new(&config, Vec::new());
        assert_eq!(s.read(REG_RAM_SIZE, 4), Ok(0x8000_0000));
        assert_eq!(s.read(REG_TIMEBASE_HZ, 4), Ok(1_500_000));
        assert_eq!(s.read(REG_CPU_HZ, 4), Ok(1_500_000));
        assert_eq!(s.read(REG_FLAGS, 4), Ok(FLAG_REAL_TIME));
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
    fn device_table_entries() {
        let s = Sysinfo::new(&MachineConfig::default(), devices());
        let entry = |i: u32, field: u32| s.read(DEV_TABLE + i * DEV_ENTRY_SIZE + field, 4).unwrap();
        assert_eq!(
            [entry(0, 0), entry(0, 4), entry(0, 8), entry(0, 12)],
            [3, 0x1000_0000, 0x100, 1]
        );
        assert_eq!(
            [entry(1, 0), entry(1, 4), entry(1, 8), entry(1, 12)],
            [1, 0x0200_0000, 0x1_0000, 0]
        );
        // Past the count, and up to the end of the block: zeros.
        assert_eq!([entry(2, 0), entry(2, 4)], [0, 0]);
        assert_eq!(s.read(0xffc, 4), Ok(0));
        assert_eq!(DEV_MAX, 252);
    }

    #[test]
    fn read_only_and_word_only() {
        let s = Sysinfo::new(&MachineConfig::default(), devices());
        for off in [REG_MAGIC, REG_RAM_SIZE, REG_CPU_HZ, REG_DEV_COUNT, DEV_TABLE] {
            assert_eq!(s.write(off, 4, 0xdead_beef), Ok(()));
        }
        assert_eq!(s.read(REG_RAM_SIZE, 4), Ok(128 * MIB as u32));
        assert_eq!(s.read(REG_DEV_COUNT, 4), Ok(2));
        assert_eq!(s.read(REG_MAGIC, 1), Err(BusFault));
        assert_eq!(s.read(REG_MAGIC, 2), Err(BusFault));
        assert_eq!(s.write(REG_MAGIC, 1, 0), Err(BusFault));
        assert_eq!(s.write(REG_MAGIC, 2, 0), Err(BusFault));
    }
}
