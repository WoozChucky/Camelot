//! Physical address space. See spec section 7 for the memory map.

use crate::config::MachineConfig;
use crate::devices::{Clint, Plic, TestDevice, Uart, KBD_IRQ, UART_IRQ};
use crate::framebuffer::{Framebuffer, VRAM_SIZE};
use crate::keyboard::Keyboard;
use crate::sysinfo::{DeviceEntry, DeviceType, Sysinfo};
use std::io::Write;

pub const BOOT_ROM_BASE: u32 = 0x0000_1000;
pub const TEST_DEV_BASE: u32 = 0x0010_0000;
pub const TEST_DEV_SIZE: u32 = 0x1000;
pub const CLINT_BASE: u32 = 0x0200_0000;
pub const CLINT_SIZE: u32 = 0x1_0000;
pub const PLIC_BASE: u32 = 0x0C00_0000;
pub const PLIC_SIZE: u32 = 0x0400_0000;
pub const UART_BASE: u32 = 0x1000_0000;
pub const UART_SIZE: u32 = 0x100;
pub const FB_CTRL_BASE: u32 = 0x3000_0000;
pub const FB_CTRL_SIZE: u32 = 0x1000;
pub const KBD_BASE: u32 = 0x3000_1000;
pub const KBD_SIZE: u32 = 0x1000;
pub const SYSINFO_BASE: u32 = 0x3000_2000;
pub const SYSINFO_SIZE: u32 = 0x1000;
pub const VRAM_BASE: u32 = 0x3100_0000;
pub const RAM_BASE: u32 = 0x8000_0000;

/// The devices a machine built from `_config` has, as the sysinfo device table lists
/// them: in address order. A new device is one more entry here (plus its decoding in
/// [`Bus::read`] and [`Bus::write`]); devices that only some machines have can look at
/// the config.
pub fn device_table(_config: &MachineConfig) -> Vec<DeviceEntry> {
    let dev = |kind, base, size, irq| DeviceEntry {
        kind,
        base,
        size,
        irq,
    };
    vec![
        dev(DeviceType::TestFinisher, TEST_DEV_BASE, TEST_DEV_SIZE, 0),
        dev(DeviceType::Clint, CLINT_BASE, CLINT_SIZE, 0),
        dev(DeviceType::Plic, PLIC_BASE, PLIC_SIZE, 0),
        dev(DeviceType::Uart, UART_BASE, UART_SIZE, UART_IRQ),
        dev(DeviceType::FbCtrl, FB_CTRL_BASE, FB_CTRL_SIZE, 0),
        dev(DeviceType::Keyboard, KBD_BASE, KBD_SIZE, KBD_IRQ),
        dev(DeviceType::Sysinfo, SYSINFO_BASE, SYSINFO_SIZE, 0),
        dev(DeviceType::FbVram, VRAM_BASE, VRAM_SIZE, 0),
    ]
}

/// Physical access failed: nothing mapped there, or the device refused the width.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BusFault;

/// Why the machine wants to stop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exit {
    /// Program asked to stop with this exit code (0 = success).
    Code(u32),
}

pub struct Bus {
    pub ram: Vec<u8>,
    pub rom: Vec<u8>,
    pub clint: Clint,
    pub plic: Plic,
    pub uart: Uart,
    pub fb: Framebuffer,
    pub kbd: Keyboard,
    pub test: TestDevice,
    pub sysinfo: Sysinfo,
    /// Where `xcal.dbg` output goes (the CLI's stderr). `None` drops it. With
    /// `uart.capture` on, it goes into the UART capture buffer instead.
    pub debug: Option<Box<dyn Write + Send>>,
    /// Address of the riscv-tests `tohost` symbol, if the loaded ELF has one.
    pub tohost: Option<u32>,
    /// Set when a device or `tohost` asks the machine to stop.
    pub exit: Option<Exit>,
}

impl Bus {
    /// The bus of a machine built from `config`, which must be valid
    /// ([`MachineConfig::validate`]).
    pub fn new(config: &MachineConfig) -> Self {
        Bus {
            ram: vec![0; config.ram_size],
            rom: Vec::new(),
            clint: Clint::default(),
            plic: Plic::default(),
            uart: Uart::default(),
            fb: Framebuffer::default(),
            kbd: Keyboard::default(),
            test: TestDevice,
            sysinfo: Sysinfo::new(config, device_table(config)),
            debug: None,
            tohost: None,
            exit: None,
        }
    }

    /// Fills the boot ROM with a stub that jumps to `entry` with a0 = hart id (0).
    pub fn set_boot_entry(&mut self, entry: u32) {
        let words: [u32; 5] = [
            0x0000_0297, // auipc t0, 0
            0x0102_a283, // lw    t0, 16(t0)
            0x0000_0513, // li    a0, 0
            0x0002_8067, // jr    t0
            entry,
        ];
        self.rom = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    }

    /// Samples every device's interrupt line into the PLIC. Called once per step,
    /// before the hart looks at `mip`.
    pub fn update_interrupts(&mut self) {
        let uart = self.uart.irq();
        self.plic.set_level(UART_IRQ, uart);
        self.plic.set_level(KBD_IRQ, self.kbd.irq());
    }

    fn ram_range(&self, addr: u32, size: u32) -> Option<usize> {
        let off = addr.wrapping_sub(RAM_BASE) as usize;
        (addr >= RAM_BASE && off + size as usize <= self.ram.len()).then_some(off)
    }

    fn vram_range(addr: u32, size: u32) -> Option<usize> {
        let off = addr.wrapping_sub(VRAM_BASE);
        (addr >= VRAM_BASE && off < VRAM_SIZE && VRAM_SIZE - off >= size).then_some(off as usize)
    }

    /// RAM and VRAM: plain memory, accessed directly instead of through a device.
    fn memory(&mut self, addr: u32, size: u32) -> Option<&mut [u8]> {
        if let Some(off) = self.ram_range(addr, size) {
            return Some(&mut self.ram[off..off + size as usize]);
        }
        Self::vram_range(addr, size).map(|off| &mut self.fb.vram[off..off + size as usize])
    }

    /// Reads `size` (1, 2 or 4) bytes of RAM or boot ROM, little-endian, zero-extended.
    /// Device registers fault here: instruction fetches and page-table walks use this, so
    /// they can never trigger a device's read side effect (such as a PLIC claim).
    pub fn read_mem(&self, addr: u32, size: u32) -> Result<u32, BusFault> {
        let bytes = if let Some(off) = self.ram_range(addr, size) {
            &self.ram[off..off + size as usize]
        } else if (BOOT_ROM_BASE..BOOT_ROM_BASE + 0x1000).contains(&addr) {
            // Past the end of the stub (but inside the ROM window) reads as 0.
            let off = (addr - BOOT_ROM_BASE) as usize;
            self.rom.get(off..).unwrap_or(&[])
        } else {
            return Err(BusFault);
        };
        let mut v = 0u32;
        for i in (0..size as usize).rev() {
            v = (v << 8) | *bytes.get(i).unwrap_or(&0) as u32;
        }
        Ok(v)
    }

    /// Reads `size` (1, 2 or 4) bytes, little-endian, zero-extended.
    pub fn read(&mut self, addr: u32, size: u32) -> Result<u32, BusFault> {
        if let Some(mem) = self.memory(addr, size) {
            return Ok(mem.iter().rev().fold(0, |v, &b| (v << 8) | b as u32));
        }
        if let Ok(v) = self.read_mem(addr, size) {
            return Ok(v);
        }
        match addr {
            a if (CLINT_BASE..CLINT_BASE + CLINT_SIZE).contains(&a) => self.clint.read(a - CLINT_BASE, size),
            a if (PLIC_BASE..PLIC_BASE + PLIC_SIZE).contains(&a) => self.plic.read(a - PLIC_BASE, size),
            a if (UART_BASE..UART_BASE + UART_SIZE).contains(&a) => self.uart.read(a - UART_BASE, size),
            a if (FB_CTRL_BASE..FB_CTRL_BASE + FB_CTRL_SIZE).contains(&a) => {
                self.fb.read(a - FB_CTRL_BASE, size)
            }
            a if (KBD_BASE..KBD_BASE + KBD_SIZE).contains(&a) => self.kbd.read(a - KBD_BASE, size),
            a if (SYSINFO_BASE..SYSINFO_BASE + SYSINFO_SIZE).contains(&a) => {
                self.sysinfo.read(a - SYSINFO_BASE, size)
            }
            a if (TEST_DEV_BASE..TEST_DEV_BASE + TEST_DEV_SIZE).contains(&a) => Ok(0),
            _ => Err(BusFault),
        }
    }

    pub fn write(&mut self, addr: u32, size: u32, val: u32) -> Result<(), BusFault> {
        if let Some(mem) = self.memory(addr, size) {
            mem.copy_from_slice(&val.to_le_bytes()[..size as usize]);
            if self.tohost == Some(addr) && val != 0 {
                // riscv-tests convention: 1 = pass, otherwise (test_number << 1) | 1.
                self.exit = Some(Exit::Code(if val == 1 { 0 } else { val >> 1 }));
            }
            return Ok(());
        }
        match addr {
            a if (CLINT_BASE..CLINT_BASE + CLINT_SIZE).contains(&a) => {
                self.clint.write(a - CLINT_BASE, size, val)
            }
            a if (PLIC_BASE..PLIC_BASE + PLIC_SIZE).contains(&a) => self.plic.write(a - PLIC_BASE, size, val),
            a if (UART_BASE..UART_BASE + UART_SIZE).contains(&a) => self.uart.write(a - UART_BASE, size, val),
            a if (FB_CTRL_BASE..FB_CTRL_BASE + FB_CTRL_SIZE).contains(&a) => {
                self.fb.write(a - FB_CTRL_BASE, size, val)
            }
            a if (KBD_BASE..KBD_BASE + KBD_SIZE).contains(&a) => self.kbd.write(a - KBD_BASE, size, val),
            a if (SYSINFO_BASE..SYSINFO_BASE + SYSINFO_SIZE).contains(&a) => {
                self.sysinfo.write(a - SYSINFO_BASE, size, val)
            }
            a if (TEST_DEV_BASE..TEST_DEV_BASE + TEST_DEV_SIZE).contains(&a) => {
                if let Some(e) = self.test.write(a - TEST_DEV_BASE, val) {
                    self.exit = Some(e);
                }
                Ok(())
            }
            _ => Err(BusFault),
        }
    }

    /// Debug console (xcal.dbg): the `debug` sink, or the UART capture buffer in tests.
    pub fn debug_out(&mut self, bytes: &[u8]) {
        if self.uart.capture {
            self.uart.captured.extend_from_slice(bytes);
        } else if let Some(out) = &mut self.debug {
            let _ = out.write_all(bytes);
            let _ = out.flush();
        }
    }

    /// Side-effect-free 32-bit read of RAM or ROM, for tracing and debugging.
    pub fn peek(&self, addr: u32) -> Option<u32> {
        let mem = if let Some(off) = self.ram_range(addr, 4) {
            &self.ram[off..off + 4]
        } else {
            let off = addr.checked_sub(BOOT_ROM_BASE)? as usize;
            self.rom.get(off..off + 4)?
        };
        Some(u32::from_le_bytes([mem[0], mem[1], mem[2], mem[3]]))
    }

    /// Copies bytes into physical memory (used by the loader).
    pub fn load(&mut self, addr: u32, data: &[u8]) -> Result<(), BusFault> {
        let off = self.ram_range(addr, data.len() as u32).ok_or(BusFault)?;
        self.ram[off..off + data.len()].copy_from_slice(data);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sysinfo::{self, DEV_ENTRY_SIZE, DEV_TABLE};
    use std::sync::{Arc, Mutex};

    fn small_bus() -> Bus {
        Bus::new(&MachineConfig::with_ram(4096))
    }

    #[test]
    fn sysinfo_on_the_bus() {
        let config = MachineConfig {
            ram_size: 32 * 1024 * 1024,
            clock_hz: Some(250_000),
            ..MachineConfig::default()
        };
        let mut bus = Bus::new(&config);
        let reg = |bus: &mut Bus, off| bus.read(SYSINFO_BASE + off, 4).unwrap();
        assert_eq!(reg(&mut bus, sysinfo::REG_MAGIC), sysinfo::SYSINFO_MAGIC);
        assert_eq!(reg(&mut bus, sysinfo::REG_RAM_BASE), RAM_BASE);
        assert_eq!(reg(&mut bus, sysinfo::REG_RAM_SIZE), 32 * 1024 * 1024);
        assert_eq!(reg(&mut bus, sysinfo::REG_TIMEBASE_HZ), 250_000);
        assert_eq!(reg(&mut bus, sysinfo::REG_CPU_HZ), 250_000);
        assert_eq!(reg(&mut bus, sysinfo::REG_FLAGS), sysinfo::FLAG_REAL_TIME);
        bus.write(SYSINFO_BASE + sysinfo::REG_RAM_SIZE, 4, 1).unwrap();
        assert_eq!(
            reg(&mut bus, sysinfo::REG_RAM_SIZE),
            32 * 1024 * 1024,
            "read-only"
        );
        assert_eq!(bus.read(SYSINFO_BASE, 1), Err(BusFault));
        assert_eq!(bus.write(SYSINFO_BASE, 2, 0), Err(BusFault));
        assert_eq!(bus.read(SYSINFO_BASE + SYSINFO_SIZE, 4), Err(BusFault));
        // Not memory: no instruction fetches from it.
        assert_eq!(bus.read_mem(SYSINFO_BASE, 4), Err(BusFault));
    }

    #[test]
    fn device_table_matches_the_bus() {
        let mut bus = small_bus();
        let count = bus.read(SYSINFO_BASE + sysinfo::REG_DEV_COUNT, 4).unwrap();
        let table = bus.sysinfo.devices().to_vec();
        assert_eq!(count as usize, table.len());
        assert_eq!(table, device_table(&MachineConfig::default()));
        let ram = RAM_BASE as u64..1 << 32;
        let rom = BOOT_ROM_BASE as u64..BOOT_ROM_BASE as u64 + 0x1000;
        assert!(table.is_sorted_by_key(|d| d.base), "address order");
        for (i, d) in table.iter().enumerate() {
            // The entry as software reads it.
            let at = |field: u32| SYSINFO_BASE + DEV_TABLE + i as u32 * DEV_ENTRY_SIZE + field;
            assert_eq!(bus.read(at(0), 4), Ok(d.kind as u32));
            assert_eq!(bus.read(at(4), 4), Ok(d.base));
            assert_eq!(bus.read(at(8), 4), Ok(d.size));
            assert_eq!(bus.read(at(12), 4), Ok(d.irq));

            // Both ends of the range are decoded (UART registers are bytes, the rest words),
            // and the byte past it is not this device.
            let width = if d.kind == DeviceType::Uart { 1 } else { 4 };
            let last = d.base + d.size - width;
            assert!(bus.read(d.base, width).is_ok(), "{:?} base", d.kind);
            assert!(bus.read(last, width).is_ok(), "{:?} last", d.kind);
            assert!(d.size > 0 && d.base.checked_add(d.size).is_some());
            let range = d.base as u64..(d.base + d.size) as u64;
            for other in &table[i + 1..] {
                let o = other.base as u64..(other.base + other.size) as u64;
                assert!(
                    range.end <= o.start || o.end <= range.start,
                    "{:?} overlaps {:?}",
                    d,
                    other
                );
            }
            for mem in [&ram, &rom] {
                assert!(
                    range.end <= mem.start || mem.end <= range.start,
                    "{d:?} overlaps memory"
                );
            }
        }
        // The UART's and keyboard's PLIC sources are the ones the bus wires them to.
        let uart = table.iter().find(|d| d.kind == DeviceType::Uart).unwrap();
        assert_eq!(uart.irq, UART_IRQ);
        let kbd = table.iter().find(|d| d.kind == DeviceType::Keyboard).unwrap();
        assert_eq!(kbd.irq, KBD_IRQ);
    }

    /// A `Write` whose bytes the test can still read after handing it to the bus.
    #[derive(Clone, Default)]
    struct Shared(Arc<Mutex<Vec<u8>>>);

    impl Write for Shared {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn debug_output_goes_to_the_machine_sink() {
        let mut bus = small_bus();
        bus.debug_out(b"dropped\n"); // no sink: discarded, not printed
        let sink = Shared::default();
        bus.debug = Some(Box::new(sink.clone()));
        bus.debug_out(b"[dbg] one\n");
        assert_eq!(&*sink.0.lock().unwrap(), b"[dbg] one\n");
        bus.uart.capture = true;
        bus.debug_out(b"[dbg] two\n");
        assert_eq!(bus.uart.captured, b"[dbg] two\n");
        assert_eq!(&*sink.0.lock().unwrap(), b"[dbg] one\n");
    }

    #[test]
    fn vram_is_plain_memory() {
        let mut bus = small_bus();
        bus.write(VRAM_BASE, 4, 0x00ff_8040).unwrap();
        assert_eq!(bus.read(VRAM_BASE, 4), Ok(0x00ff_8040));
        assert_eq!(bus.read(VRAM_BASE, 1), Ok(0x40));
        assert_eq!(bus.read(VRAM_BASE + 1, 1), Ok(0x80));
        assert_eq!(bus.read(VRAM_BASE + 2, 2), Ok(0x00ff));
        assert_eq!(&bus.fb.vram[..4], &[0x40, 0x80, 0xff, 0x00]);

        bus.write(VRAM_BASE + 5, 1, 0xabcd).unwrap();
        bus.write(VRAM_BASE + 6, 2, 0x1234_5678).unwrap();
        assert_eq!(bus.read(VRAM_BASE + 4, 4), Ok(0x5678_cd00));

        // Data accesses only: no instruction fetches or page-table walks from VRAM.
        assert_eq!(bus.read_mem(VRAM_BASE, 4), Err(BusFault));
    }

    #[test]
    fn vram_bounds() {
        let mut bus = small_bus();
        let last = VRAM_BASE + VRAM_SIZE - 4;
        assert_eq!(last, 0x3112_bffc);
        bus.write(last, 4, 0x0011_2233).unwrap();
        assert_eq!(bus.read(last, 4), Ok(0x0011_2233));
        assert_eq!(bus.fb.pixel(639, 479), 0x0011_2233);
        assert_eq!(bus.read(last + 3, 1), Ok(0));
        assert_eq!(bus.read(last + 4, 1), Err(BusFault));
        assert_eq!(bus.write(last + 4, 4, 0), Err(BusFault));
        assert_eq!(bus.read(VRAM_BASE - 4, 4), Err(BusFault));
    }

    #[test]
    fn framebuffer_registers_on_the_bus() {
        let mut bus = small_bus();
        assert_eq!(bus.read(FB_CTRL_BASE, 4), Ok(640));
        assert_eq!(bus.read(FB_CTRL_BASE + 4, 4), Ok(480));
        assert_eq!(bus.read(FB_CTRL_BASE + 8, 4), Ok(2560));
        assert_eq!(bus.read(FB_CTRL_BASE + 0xc, 4), Ok(0));
        bus.write(FB_CTRL_BASE + 0x10, 4, 1).unwrap();
        assert!(bus.fb.enabled());
        bus.write(FB_CTRL_BASE + 0x14, 4, 1).unwrap();
        assert!(bus.fb.take_present());
        assert_eq!(bus.read(FB_CTRL_BASE, 1), Err(BusFault));
        assert_eq!(bus.read(FB_CTRL_BASE + 0xffc, 4), Ok(0));
    }

    #[test]
    fn keyboard_registers_on_the_bus() {
        use crate::keyboard::KeyEvent;
        let mut bus = small_bus();
        assert_eq!(bus.read(KBD_BASE, 4), Ok(0));
        assert_eq!(bus.read(KBD_BASE + 4, 4), Ok(0));
        bus.kbd.push(KeyEvent::press(b'k').with_shift(true));
        assert_eq!(bus.read(KBD_BASE, 4), Ok(1));
        assert_eq!(bus.read(KBD_BASE + 4, 4), Ok(0x26b));
        assert_eq!(bus.read(KBD_BASE, 4), Ok(0));
        bus.write(KBD_BASE + 8, 4, 1).unwrap();
        assert_eq!(bus.read(KBD_BASE + 8, 4), Ok(1));
        assert_eq!(bus.read(KBD_BASE + 0xffc, 4), Ok(0));
        assert_eq!(bus.read(KBD_BASE, 1), Err(BusFault));
        assert_eq!(bus.write(KBD_BASE + 8, 2, 0), Err(BusFault));
        // The next page is sysinfo's, not the keyboard's.
        assert_eq!(bus.read(KBD_BASE + KBD_SIZE, 4), Ok(sysinfo::SYSINFO_MAGIC));
        // Data accesses only.
        assert_eq!(bus.read_mem(KBD_BASE, 4), Err(BusFault));
    }
}
