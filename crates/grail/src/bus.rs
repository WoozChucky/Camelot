//! Physical address space. See spec section 7 for the memory map.

use crate::devices::{Clint, Plic, TestDevice, Uart, UART_IRQ};

pub const BOOT_ROM_BASE: u32 = 0x0000_1000;
pub const TEST_DEV_BASE: u32 = 0x0010_0000;
pub const CLINT_BASE: u32 = 0x0200_0000;
pub const CLINT_SIZE: u32 = 0x1_0000;
pub const PLIC_BASE: u32 = 0x0C00_0000;
pub const PLIC_SIZE: u32 = 0x0400_0000;
pub const UART_BASE: u32 = 0x1000_0000;
pub const UART_SIZE: u32 = 0x100;
pub const RAM_BASE: u32 = 0x8000_0000;

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
    pub test: TestDevice,
    /// Address of the riscv-tests `tohost` symbol, if the loaded ELF has one.
    pub tohost: Option<u32>,
    /// Set when a device or `tohost` asks the machine to stop.
    pub exit: Option<Exit>,
}

impl Bus {
    pub fn new(ram_size: usize) -> Self {
        Bus {
            ram: vec![0; ram_size],
            rom: Vec::new(),
            clint: Clint::default(),
            plic: Plic::default(),
            uart: Uart::default(),
            test: TestDevice,
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
    }

    fn ram_range(&self, addr: u32, size: u32) -> Option<usize> {
        let off = addr.wrapping_sub(RAM_BASE) as usize;
        (addr >= RAM_BASE && off + size as usize <= self.ram.len()).then_some(off)
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
        if let Ok(v) = self.read_mem(addr, size) {
            return Ok(v);
        }
        match addr {
            a if (CLINT_BASE..CLINT_BASE + CLINT_SIZE).contains(&a) => self.clint.read(a - CLINT_BASE, size),
            a if (PLIC_BASE..PLIC_BASE + PLIC_SIZE).contains(&a) => self.plic.read(a - PLIC_BASE, size),
            a if (UART_BASE..UART_BASE + UART_SIZE).contains(&a) => self.uart.read(a - UART_BASE, size),
            a if (TEST_DEV_BASE..TEST_DEV_BASE + 0x1000).contains(&a) => Ok(0),
            _ => Err(BusFault),
        }
    }

    pub fn write(&mut self, addr: u32, size: u32, val: u32) -> Result<(), BusFault> {
        if let Some(off) = self.ram_range(addr, size) {
            for i in 0..size as usize {
                self.ram[off + i] = (val >> (8 * i)) as u8;
            }
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
            a if (TEST_DEV_BASE..TEST_DEV_BASE + 0x1000).contains(&a) => {
                if let Some(e) = self.test.write(a - TEST_DEV_BASE, val) {
                    self.exit = Some(e);
                }
                Ok(())
            }
            _ => Err(BusFault),
        }
    }

    /// Debug console (xcal.dbg). Goes to stderr, or into the UART capture buffer in tests.
    pub fn debug_out(&mut self, bytes: &[u8]) {
        if self.uart.capture {
            self.uart.captured.extend_from_slice(bytes);
        } else {
            use std::io::Write;
            let _ = std::io::stderr().write_all(bytes);
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
