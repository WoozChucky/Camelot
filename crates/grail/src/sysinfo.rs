//! Xcal sysinfo (spec section 9.5): read-only registers that tell software what machine
//! it runs on (RAM, clock, harts) and a table of the devices it has, so a kernel
//! discovers its hardware instead of hard-coding it.
//!
//! Two layouts: **version 2**, which every RV64 machine presents, with 64-bit registers
//! for the RAM base and size and 24-byte device entries whose base and size are 64-bit;
//! and **version 1**, all 32-bit, which RV32 machines keep until RV32 is removed. The
//! constants at the top of this module are version 1's, [`v2`]'s the new layout's.

use crate::bus::{BusFault, RAM_BASE};
use crate::config::MachineConfig;
use excalibur::Xlen;

/// "SYSI" when the register is read as big-endian hex: 53 59 53 49.
pub const SYSINFO_MAGIC: u32 = 0x5359_5349;
/// Version 1's layout (RV32 machines). A version changes only if existing registers
/// change meaning or place; new registers and new device types do not need one.
pub const SYSINFO_VERSION: u32 = 1;

// Version 1's register offsets from the start of the block, all 32-bit.
pub const REG_MAGIC: u32 = 0x00;
pub const REG_VERSION: u32 = 0x04;
pub const REG_RAM_BASE: u32 = 0x08;
pub const REG_RAM_SIZE: u32 = 0x0c;
pub const REG_TIMEBASE_HZ: u32 = 0x10;
pub const REG_CPU_HZ: u32 = 0x14;
pub const REG_FLAGS: u32 = 0x18;
pub const REG_DEV_COUNT: u32 = 0x1c;
/// Number of harts; their `mhartid`s are 0 to this minus 1.
pub const REG_HARTS: u32 = 0x20;
/// First device table entry (both versions).
pub const DEV_TABLE: u32 = 0x40;
/// Bytes per version 1 device table entry: type, base, size, irq.
pub const DEV_ENTRY_SIZE: u32 = 16;
/// Version 1 entries that fit in the 4 KiB block.
pub const DEV_MAX: usize = ((0x1000 - DEV_TABLE) / DEV_ENTRY_SIZE) as usize;

/// Version 2, the layout of RV64 machines (spec section 9.5): the RAM base and size
/// are 64-bit, and so are each device entry's base and size.
pub mod v2 {
    use super::DEV_TABLE;

    pub const VERSION: u32 = 2;
    // `REG_MAGIC` and `REG_VERSION` are where version 1 has them.
    /// 64-bit.
    pub const REG_RAM_BASE: u32 = 0x08;
    /// 64-bit.
    pub const REG_RAM_SIZE: u32 = 0x10;
    pub const REG_TIMEBASE_HZ: u32 = 0x18;
    pub const REG_CPU_HZ: u32 = 0x1c;
    pub const REG_FLAGS: u32 = 0x20;
    pub const REG_DEV_COUNT: u32 = 0x24;
    pub const REG_HARTS: u32 = 0x28;
    /// Bytes per device table entry: type (+0), irq (+4), base (+8, 64-bit), size
    /// (+0x10, 64-bit).
    pub const DEV_ENTRY_SIZE: u32 = 24;
    /// Entries that fit in the 4 KiB block.
    pub const DEV_MAX: usize = ((0x1000 - DEV_TABLE) / DEV_ENTRY_SIZE) as usize;
}

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
}

/// One device table entry. `irq` is the device's PLIC source, 0 if it has none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeviceEntry {
    pub kind: DeviceType,
    pub base: u64,
    pub size: u64,
    pub irq: u32,
}

/// What a 4-byte-aligned offset of the block holds.
enum Slot {
    /// A 32-bit register.
    Word(u32),
    /// The low half (at the lower address) of a 64-bit register.
    Low(u64),
    /// Its high half.
    High(u64),
    /// Nothing: reads as 0.
    Reserved,
}

pub struct Sysinfo {
    ram_size: u64,
    cpu_hz: u32,
    flags: u32,
    harts: u32,
    devices: Vec<DeviceEntry>,
    /// Which layout the block presents: version 2 on RV64, 1 on RV32.
    xlen: Xlen,
}

impl Sysinfo {
    /// `config` must be valid (see [`MachineConfig::validate`]). The layout is the one
    /// of `config.xlen`'s machines.
    pub fn new(config: &MachineConfig, devices: Vec<DeviceEntry>) -> Self {
        assert!(
            devices.len() <= v2::DEV_MAX,
            "too many devices for the sysinfo table"
        );
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
            xlen: config.xlen,
        }
    }

    /// Presents the layout of an `xlen` machine (an ELF64 program made the machine
    /// RV64, [`crate::Machine::load_elf`]).
    pub fn set_xlen(&mut self, xlen: Xlen) {
        self.xlen = xlen;
    }

    pub fn devices(&self) -> &[DeviceEntry] {
        &self.devices
    }

    /// The version this block presents.
    pub fn version(&self) -> u32 {
        match self.xlen {
            Xlen::Rv32 => SYSINFO_VERSION,
            Xlen::Rv64 => v2::VERSION,
        }
    }

    /// What `off` holds in this block's layout.
    fn slot(&self, off: u32) -> Slot {
        if off % 4 != 0 {
            return Slot::Reserved;
        }
        match self.xlen {
            Xlen::Rv32 => self.slot_v1(off),
            Xlen::Rv64 => self.slot_v2(off),
        }
    }

    fn slot_v1(&self, off: u32) -> Slot {
        let word = match off {
            REG_MAGIC => SYSINFO_MAGIC,
            REG_VERSION => SYSINFO_VERSION,
            REG_RAM_BASE => RAM_BASE as u32,
            // At most 2 GiB on RV32.
            REG_RAM_SIZE => self.ram_size as u32,
            // One instruction = one cycle = one mtime tick: both run at the same rate.
            REG_TIMEBASE_HZ | REG_CPU_HZ => self.cpu_hz,
            REG_FLAGS => self.flags,
            REG_DEV_COUNT => self.devices.len() as u32,
            REG_HARTS => self.harts,
            o if o >= DEV_TABLE => {
                let index = ((o - DEV_TABLE) / DEV_ENTRY_SIZE) as usize;
                let Some(d) = self.devices.get(index) else {
                    return Slot::Reserved;
                };
                // Every device is below 4 GiB.
                match (o - DEV_TABLE) % DEV_ENTRY_SIZE {
                    0 => d.kind as u32,
                    4 => d.base as u32,
                    8 => d.size as u32,
                    _ => d.irq,
                }
            }
            _ => return Slot::Reserved,
        };
        Slot::Word(word)
    }

    fn slot_v2(&self, off: u32) -> Slot {
        let word = match off {
            REG_MAGIC => SYSINFO_MAGIC,
            REG_VERSION => v2::VERSION,
            v2::REG_RAM_BASE => return Slot::Low(RAM_BASE),
            o if o == v2::REG_RAM_BASE + 4 => return Slot::High(RAM_BASE),
            v2::REG_RAM_SIZE => return Slot::Low(self.ram_size),
            o if o == v2::REG_RAM_SIZE + 4 => return Slot::High(self.ram_size),
            v2::REG_TIMEBASE_HZ | v2::REG_CPU_HZ => self.cpu_hz,
            v2::REG_FLAGS => self.flags,
            v2::REG_DEV_COUNT => self.devices.len() as u32,
            v2::REG_HARTS => self.harts,
            o if o >= DEV_TABLE => {
                let index = ((o - DEV_TABLE) / v2::DEV_ENTRY_SIZE) as usize;
                let Some(d) = self.devices.get(index) else {
                    return Slot::Reserved;
                };
                return match (o - DEV_TABLE) % v2::DEV_ENTRY_SIZE {
                    0x00 => Slot::Word(d.kind as u32),
                    0x04 => Slot::Word(d.irq),
                    0x08 => Slot::Low(d.base),
                    0x0c => Slot::High(d.base),
                    0x10 => Slot::Low(d.size),
                    _ => Slot::High(d.size),
                };
            }
            _ => return Slot::Reserved,
        };
        Slot::Word(word)
    }

    /// Reads a register. Every version takes 4-byte reads, of a 32-bit register or one
    /// half of a 64-bit one; version 2 also takes 8-byte reads of a whole 64-bit
    /// register (or of reserved space). Unused offsets, and entries past the device
    /// count, read as 0.
    pub fn read(&self, off: u32, size: u32) -> Result<u64, BusFault> {
        match size {
            4 => Ok(match self.slot(off) {
                Slot::Word(v) => v.into(),
                Slot::Low(v) => v & 0xffff_ffff,
                Slot::High(v) => v >> 32,
                Slot::Reserved => 0,
            }),
            8 if self.xlen == Xlen::Rv64 && off % 8 == 0 => match (self.slot(off), self.slot(off + 4)) {
                (Slot::Low(v), _) => Ok(v),
                (Slot::Reserved, Slot::Reserved) => Ok(0),
                // Two 32-bit registers, or one and reserved space: a width it does not
                // take.
                _ => Err(BusFault),
            },
            _ => Err(BusFault),
        }
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

    fn rv64(config: MachineConfig) -> MachineConfig {
        MachineConfig {
            xlen: Xlen::Rv64,
            ..config
        }
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
        assert_eq!(s.read(REG_HARTS, 4), Ok(1));
        for reserved in [0x24, 0x28, 0x3c] {
            assert_eq!(s.read(reserved, 4), Ok(0));
        }
        // Version 1 has no 64-bit registers: no 8-byte reads.
        assert_eq!(s.read(REG_RAM_BASE, 8), Err(BusFault));
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
        assert_eq!(s.read(REG_RAM_SIZE, 4), Ok(0x8000_0000));
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
        assert_eq!(s.read(REG_RAM_SIZE, 4), Ok(4 * MIB as u64));
        assert_eq!(s.read(REG_DEV_COUNT, 4), Ok(2));
        assert_eq!(s.read(REG_MAGIC, 1), Err(BusFault));
        assert_eq!(s.read(REG_MAGIC, 2), Err(BusFault));
        assert_eq!(s.write(REG_MAGIC, 1, 0), Err(BusFault));
        assert_eq!(s.write(REG_MAGIC, 2, 0), Err(BusFault));
        assert_eq!(s.write(REG_RAM_BASE, 8, 0), Err(BusFault));
    }

    #[test]
    fn version_2_header() {
        let config = rv64(MachineConfig {
            ram_size: 0x100_0000_0000,
            clock_hz: Some(1_500_000),
            harts: 3,
            ..MachineConfig::default()
        });
        let s = Sysinfo::new(&config, devices());
        assert_eq!(s.read(REG_MAGIC, 4), Ok(0x5359_5349));
        assert_eq!(s.read(REG_VERSION, 4), Ok(2));
        // The 64-bit registers, whole and by halves.
        assert_eq!(s.read(v2::REG_RAM_BASE, 8), Ok(0x8000_0000));
        assert_eq!(s.read(v2::REG_RAM_BASE, 4), Ok(0x8000_0000));
        assert_eq!(s.read(v2::REG_RAM_BASE + 4, 4), Ok(0));
        assert_eq!(s.read(v2::REG_RAM_SIZE, 8), Ok(0x100_0000_0000), "1 TiB");
        assert_eq!(s.read(v2::REG_RAM_SIZE, 4), Ok(0));
        assert_eq!(s.read(v2::REG_RAM_SIZE + 4, 4), Ok(0x100));
        assert_eq!(s.read(v2::REG_TIMEBASE_HZ, 4), Ok(1_500_000));
        assert_eq!(s.read(v2::REG_CPU_HZ, 4), Ok(1_500_000));
        assert_eq!(s.read(v2::REG_FLAGS, 4), Ok(FLAG_REAL_TIME.into()));
        assert_eq!(s.read(v2::REG_DEV_COUNT, 4), Ok(2));
        assert_eq!(s.read(v2::REG_HARTS, 4), Ok(3));
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
        assert_eq!(s.read(v2::REG_TIMEBASE_HZ, 8), Err(BusFault));
        assert_eq!(s.read(v2::REG_HARTS, 8), Err(BusFault), "harts and reserved");
        assert_eq!(s.read(v2::REG_RAM_BASE, 2), Err(BusFault));
        // Read-only, at the widths it takes.
        assert_eq!(s.write(v2::REG_RAM_SIZE, 8, 0), Ok(()));
        assert_eq!(s.read(v2::REG_RAM_SIZE, 8), Ok(0x100_0000_0000));
        assert_eq!(s.write(REG_MAGIC, 8, 0), Err(BusFault));
    }

    #[test]
    fn version_2_device_table() {
        let mut devs = devices();
        devs[0].base = 0x12_3456_7000;
        devs[0].size = 0x1_0000_0000;
        let s = Sysinfo::new(&rv64(MachineConfig::default()), devs);
        let at = |i: u32, field: u32| DEV_TABLE + i * v2::DEV_ENTRY_SIZE + field;
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
        assert_eq!(v2::DEV_MAX, 168);
    }

    #[test]
    fn the_layout_follows_the_machines_width() {
        let mut s = Sysinfo::new(&MachineConfig::default(), devices());
        assert_eq!(s.version(), 1);
        s.set_xlen(Xlen::Rv64);
        assert_eq!(s.read(REG_VERSION, 4), Ok(2));
        assert_eq!(s.read(v2::REG_RAM_SIZE, 8), Ok(4 * MIB as u64));
    }
}
