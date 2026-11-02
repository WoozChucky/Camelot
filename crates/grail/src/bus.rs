//! Physical address space. See spec section 7 for the memory map.

use crate::block::{self, BlockDevice};
use crate::boot;
use crate::config::MachineConfig;
use crate::cpu::Reservation;
use crate::devices::{Clint, Plic, TestDevice, Uart, BLK_IRQ, KBD_IRQ, NET_IRQ, PTR_IRQ, UART_IRQ};
use crate::framebuffer::{Framebuffer, VRAM_SIZE};
use crate::keyboard::Keyboard;
use crate::net::NetDevice;
use crate::pointer::Pointer;
use crate::sysinfo::{DeviceEntry, DeviceType, Sysinfo};
use std::io::Write;

pub const BOOT_ROM_BASE: u32 = 0x0000_1000;
/// Where harts other than hart 0 start: the boot ROM's park loop, which waits for an
/// IPI and then jumps to the entry point (spec section 7, "Boot").
pub const PARK_PC: u32 = BOOT_ROM_BASE + 0x20;
pub const TEST_DEV_BASE: u32 = 0x0010_0000;
pub const TEST_DEV_SIZE: u32 = 0x1000;
pub const CLINT_BASE: u32 = 0x0200_0000;
pub const CLINT_SIZE: u32 = 0x1_0000;
pub const PLIC_BASE: u32 = 0x0C00_0000;
pub const PLIC_SIZE: u32 = 0x0400_0000;
pub const UART_BASE: u32 = 0x1000_0000;
pub const UART_SIZE: u32 = 0x100;
/// Disk 0's registers (spec section 9.6); disk `i` is at `BLK_BASE + i * BLK_SIZE`.
pub const BLK_BASE: u32 = 0x1000_1000;
pub const BLK_SIZE: u32 = 0x1000;
/// Disks a machine can have: one page each in 0x1000_1000 - 0x1000_8FFF, PLIC sources
/// `BLK_IRQ` (3) to 10.
pub const MAX_DISKS: usize = 8;
/// Network card 0's registers (spec section 9.8), right after the disks' pages; card
/// `i` is at `NET_BASE + i * NET_SIZE`.
pub const NET_BASE: u32 = 0x1000_9000;
pub const NET_SIZE: u32 = 0x1000;
/// Network cards a machine can have: one page each in 0x1000_9000 - 0x1000_CFFF, PLIC
/// sources `NET_IRQ` (12) to 15.
pub const MAX_NETS: usize = 4;
pub const FB_CTRL_BASE: u32 = 0x3000_0000;
pub const FB_CTRL_SIZE: u32 = 0x1000;
pub const KBD_BASE: u32 = 0x3000_1000;
pub const KBD_SIZE: u32 = 0x1000;
pub const SYSINFO_BASE: u32 = 0x3000_2000;
pub const SYSINFO_SIZE: u32 = 0x1000;
pub const PTR_BASE: u32 = 0x3000_3000;
pub const PTR_SIZE: u32 = 0x1000;
pub const VRAM_BASE: u32 = 0x3100_0000;
pub const RAM_BASE: u32 = 0x8000_0000;

/// The devices a machine built from `config` with `disks` disks has, as the sysinfo
/// device table lists them: in address order. Its network cards are the ones
/// `config.nets` describes; [`device_table_with`] counts host-made ones too.
pub fn device_table(config: &MachineConfig, disks: usize) -> Vec<DeviceEntry> {
    device_table_with(config, disks, config.nets.len())
}

/// The device table of a machine built from `_config` with `disks` disks and `nets`
/// network cards. A new device is one more entry here (plus its decoding in
/// [`Bus::read`] and [`Bus::write`]); devices that only some machines have can look at
/// the config.
pub fn device_table_with(_config: &MachineConfig, disks: usize, nets: usize) -> Vec<DeviceEntry> {
    let dev = |kind, base, size, irq| DeviceEntry {
        kind,
        base,
        size,
        irq,
    };
    let mut table = vec![
        dev(DeviceType::TestFinisher, TEST_DEV_BASE, TEST_DEV_SIZE, 0),
        dev(DeviceType::Clint, CLINT_BASE, CLINT_SIZE, 0),
        dev(DeviceType::Plic, PLIC_BASE, PLIC_SIZE, 0),
        dev(DeviceType::Uart, UART_BASE, UART_SIZE, UART_IRQ),
    ];
    table.extend(
        (0..disks as u32).map(|i| dev(DeviceType::Block, BLK_BASE + i * BLK_SIZE, BLK_SIZE, BLK_IRQ + i)),
    );
    table.extend(
        (0..nets as u32).map(|i| dev(DeviceType::Net, NET_BASE + i * NET_SIZE, NET_SIZE, NET_IRQ + i)),
    );
    table.extend([
        dev(DeviceType::FbCtrl, FB_CTRL_BASE, FB_CTRL_SIZE, 0),
        dev(DeviceType::Keyboard, KBD_BASE, KBD_SIZE, KBD_IRQ),
        dev(DeviceType::Sysinfo, SYSINFO_BASE, SYSINFO_SIZE, 0),
        dev(DeviceType::Pointer, PTR_BASE, PTR_SIZE, PTR_IRQ),
        dev(DeviceType::FbVram, VRAM_BASE, VRAM_SIZE, 0),
    ]);
    table
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
    /// The boot ROM's bytes, from `BOOT_ROM_BASE` (the rest of its page reads as 0).
    pub rom: Vec<u8>,
    pub clint: Clint,
    pub plic: Plic,
    pub uart: Uart,
    pub fb: Framebuffer,
    pub kbd: Keyboard,
    pub ptr: Pointer,
    /// Disk `i` is at `BLK_BASE + i * BLK_SIZE`, on PLIC source `BLK_IRQ + i`.
    pub disks: Vec<BlockDevice>,
    /// Network card `i` is at `NET_BASE + i * NET_SIZE`, on PLIC source `NET_IRQ + i`.
    pub nets: Vec<NetDevice>,
    pub test: TestDevice,
    pub sysinfo: Sysinfo,
    /// Where `xcal.dbg` output goes (the CLI's stderr). `None` drops it. With
    /// `uart.capture` on, it goes into the UART capture buffer instead.
    pub debug: Option<Box<dyn Write + Send>>,
    /// Address of the riscv-tests `tohost` symbol, if the loaded ELF has one.
    pub tohost: Option<u32>,
    /// Set when a device or `tohost` asks the machine to stop.
    pub exit: Option<Exit>,
    /// Hart `i`'s LR/SC reservation. They live here, next to memory, because every
    /// write to memory goes through the bus: a hart's store, an AMO or a `sc.w`
    /// ([`Bus::snoop_store`]), and a device's DMA ([`Bus::tick`], [`Bus::write`]), so
    /// each one breaks the reservations it touches whichever hart holds them.
    reservations: Vec<Reservation>,
}

impl Bus {
    /// The bus of a machine built from `config`, which must be valid
    /// ([`MachineConfig::validate`]), with these disks (at most [`MAX_DISKS`]) and no
    /// network cards. The disks are not opened from `config.disks` here:
    /// [`crate::Machine::new`] does that.
    pub fn new(config: &MachineConfig, disks: Vec<BlockDevice>) -> Self {
        Bus::with_nets(config, disks, Vec::new())
    }

    /// [`Bus::new`] with these network cards too (at most [`MAX_NETS`]). They are not
    /// opened from `config.nets` here: [`crate::Machine::new`] does that.
    pub fn with_nets(config: &MachineConfig, disks: Vec<BlockDevice>, nets: Vec<NetDevice>) -> Self {
        assert!(disks.len() <= MAX_DISKS, "at most {MAX_DISKS} disks");
        assert!(nets.len() <= MAX_NETS, "at most {MAX_NETS} network cards");
        let harts = config.harts;
        assert!(harts >= 1, "a machine has at least one hart");
        let mut bus = Bus {
            ram: vec![0; config.ram_size],
            rom: Vec::new(),
            clint: Clint::new(harts),
            plic: Plic::new(harts),
            uart: Uart::default(),
            fb: Framebuffer::default(),
            kbd: Keyboard::default(),
            ptr: Pointer::default(),
            sysinfo: Sysinfo::new(config, device_table_with(config, disks.len(), nets.len())),
            disks,
            nets,
            test: TestDevice,
            debug: None,
            tohost: None,
            exit: None,
            reservations: vec![Reservation::default(); harts],
        };
        if harts > 1 {
            // The park loop must be there before any program is loaded: the other harts
            // run it from reset.
            bus.set_boot_entry(0);
        }
        bus
    }

    /// How many harts share this bus.
    pub fn harts(&self) -> usize {
        self.reservations.len()
    }

    /// Fills the boot ROM (spec section 7, "Boot"). Hart 0 starts at `BOOT_ROM_BASE`, a
    /// stub that jumps to `entry` with a0 = its hart id, 0. On a machine with more harts
    /// the others start at [`PARK_PC`], a loop that waits in `wfi` until the hart's
    /// `msip` is set (an IPI), clears it and jumps to `entry` too, with a0 = its hart id.
    /// A one-hart machine's ROM has only the stub.
    pub fn set_boot_entry(&mut self, entry: u32) {
        let mut words = vec![
            0x0000_0297, // 0x1000: auipc t0, 0
            0x0102_a283, //         lw    t0, 16(t0)     the entry word
            0x0000_0513, //         li    a0, 0
            0x0002_8067, //         jr    t0
            entry,       // 0x1010: the entry point
        ];
        if self.harts() > 1 {
            words.resize(((PARK_PC - BOOT_ROM_BASE) / 4) as usize, 0);
            words.extend([
                0xf140_2573, // 0x1020: csrr  a0, mhartid
                0x0025_1313, //         slli  t1, a0, 2
                0x0200_02b7, //         lui   t0, 0x2000      CLINT_BASE
                0x0062_8333, //         add   t1, t0, t1      t1 = &msip[hart]
                0x0080_0393, //         li    t2, 8           mie.MSIE: msip ends wfi
                0x3043_9073, //         csrw  mie, t2         (mstatus.MIE is 0: no trap)
                0x1050_0073, // 0x1038: wfi
                0x0003_2383, //         lw    t2, 0(t1)
                0xfe03_8ce3, //         beqz  t2, 0x1038      not woken by an IPI: wait
                0x0003_2023, //         sw    zero, 0(t1)     acknowledge the IPI
                0x3040_1073, //         csrw  mie, zero
                0x0000_12b7, //         lui   t0, 1           BOOT_ROM_BASE
                0x0102_a283, //         lw    t0, 16(t0)      the entry word
                0x0002_8067, //         jr    t0
            ]);
        }
        self.rom = words.iter().flat_map(|w: &u32| w.to_le_bytes()).collect();
    }

    /// Fills the boot ROM to boot from disk 0 (spec section 7, "Boot from disk"): the
    /// stub and the park loop of [`Bus::set_boot_entry`], with the entry word pointing
    /// at the disk loader ([`boot::DISK_BOOT_PC`]), which this adds to the ROM.
    pub fn set_disk_boot(&mut self) {
        self.set_boot_entry(boot::DISK_BOOT_PC);
        self.rom.resize((boot::DISK_BOOT_PC - BOOT_ROM_BASE) as usize, 0);
        self.rom.extend(boot::disk_loader());
    }

    /// Hart `hart`'s LR/SC reservation.
    pub fn reservation(&self, hart: usize) -> Reservation {
        self.reservations[hart]
    }

    pub fn reservation_mut(&mut self, hart: usize) -> &mut Reservation {
        &mut self.reservations[hart]
    }

    /// Hart `hart` wrote `size` bytes at physical address `paddr` (a store, an AMO, a
    /// successful `sc.w`, an A/D update): a write to memory breaks every reservation on
    /// the words it touched, whichever hart holds it; a store to a device breaks the
    /// storing hart's own reservation (the device may start a DMA into RAM).
    pub fn snoop_store(&mut self, hart: usize, paddr: u32, size: u32) {
        if self.reservations.iter().all(|r| r.held().is_none()) {
            return;
        }
        if self.is_memory(paddr, size) {
            for r in &mut self.reservations {
                r.snoop(paddr, size);
            }
        } else {
            self.reservations[hart].clear();
        }
    }

    /// The start of a step ([`crate::cpu::Cpu::step`]): `mtime` ticks, then every
    /// device's line is sampled ([`Bus::update_interrupts`]).
    pub fn tick(&mut self) {
        self.clint.mtime = self.clint.mtime.wrapping_add(1);
        self.update_interrupts();
    }

    /// Lets `ticks` idle ticks pass at once for the devices that count steps (a paced
    /// machine whose harts all wait in `wfi`, see [`crate::cpu::Cpu::idle`]): network
    /// cards still poll their backends on time, and what they receive breaks
    /// reservations as in [`Bus::update_interrupts`].
    pub fn skip(&mut self, ticks: u64) {
        let reservations = &mut self.reservations;
        for nic in &mut self.nets {
            nic.skip(ticks, &mut self.ram, &mut |paddr, len| {
                reservations.iter_mut().for_each(|r| r.snoop(paddr, len))
            });
        }
    }

    /// Samples every device's interrupt line into the PLIC. Called once per step,
    /// before the harts look at `mip`. Network cards poll their backends from here
    /// (every [`crate::net::POLL_STEPS`] steps), so received frames land in RAM between
    /// two instructions; each RAM range they write breaks the reservations on it, as a
    /// store would.
    pub fn update_interrupts(&mut self) {
        let reservations = &mut self.reservations;
        for (i, nic) in self.nets.iter_mut().enumerate() {
            nic.tick(&mut self.ram, &mut |paddr, len| {
                reservations.iter_mut().for_each(|r| r.snoop(paddr, len))
            });
            self.plic.set_level(NET_IRQ + i as u32, nic.irq());
        }
        let uart = self.uart.irq();
        self.plic.set_level(UART_IRQ, uart);
        self.plic.set_level(KBD_IRQ, self.kbd.irq());
        self.plic.set_level(PTR_IRQ, self.ptr.irq());
        for (i, disk) in self.disks.iter().enumerate() {
            self.plic.set_level(BLK_IRQ + i as u32, disk.irq());
        }
    }

    /// The disk whose register page holds `addr`, and the offset in it.
    fn disk_at(&self, addr: u32) -> Option<(usize, u32)> {
        let off = addr.wrapping_sub(BLK_BASE);
        let i = (off / BLK_SIZE) as usize;
        (addr >= BLK_BASE && i < self.disks.len()).then_some((i, off % BLK_SIZE))
    }

    /// The network card whose register page holds `addr`, and the offset in it.
    fn net_at(&self, addr: u32) -> Option<(usize, u32)> {
        let off = addr.wrapping_sub(NET_BASE);
        let i = (off / NET_SIZE) as usize;
        (addr >= NET_BASE && i < self.nets.len()).then_some((i, off % NET_SIZE))
    }

    fn ram_range(&self, addr: u32, size: u32) -> Option<usize> {
        let off = addr.wrapping_sub(RAM_BASE) as usize;
        let end = off.checked_add(size as usize)?;
        (addr >= RAM_BASE && end <= self.ram.len()).then_some(off)
    }

    fn vram_range(addr: u32, size: u32) -> Option<usize> {
        let off = addr.wrapping_sub(VRAM_BASE);
        (addr >= VRAM_BASE && off < VRAM_SIZE && VRAM_SIZE - off >= size).then_some(off as usize)
    }

    /// Whether `[addr, addr + size)` is plain memory (RAM or VRAM) rather than a device
    /// register or nothing. Atomics (LR/SC, AMOs) only work there.
    pub fn is_memory(&self, addr: u32, size: u32) -> bool {
        self.ram_range(addr, size).is_some() || Self::vram_range(addr, size).is_some()
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
        if let Some((i, off)) = self.disk_at(addr) {
            return self.disks[i].read(off, size);
        }
        if let Some((i, off)) = self.net_at(addr) {
            return self.nets[i].read(off, size);
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
            a if (PTR_BASE..PTR_BASE + PTR_SIZE).contains(&a) => self.ptr.read(a - PTR_BASE, size),
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
        if addr == boot::BOOT_ENTRY_ADDR && size == 4 {
            // The ROM's entry word, its one writable word: a bootloader points the
            // harts it has not started yet at its kernel.
            let at = (addr - BOOT_ROM_BASE) as usize;
            if self.rom.len() < at + 4 {
                self.rom.resize(at + 4, 0);
            }
            self.rom[at..at + 4].copy_from_slice(&val.to_le_bytes());
            return Ok(());
        }
        if let Some((i, off)) = self.disk_at(addr) {
            if off == block::REG_CMD {
                // The disk moves data between itself and RAM on its own (DMA), which
                // may write any hart's reserved word: no reservation survives a command.
                self.reservations.iter_mut().for_each(Reservation::clear);
            }
            return self.disks[i].write(off, size, val, &mut self.ram);
        }
        if let Some((i, off)) = self.net_at(addr) {
            // Writing `NET_TX_TAIL` sends frames at once: the card reads them from RAM
            // and writes each descriptor's length and status back (DMA), which breaks
            // the reservations on those words, whichever hart holds them.
            let reservations = &mut self.reservations;
            return self.nets[i].write(off, size, val, &mut self.ram, &mut |paddr, len| {
                reservations.iter_mut().for_each(|r| r.snoop(paddr, len))
            });
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
            a if (PTR_BASE..PTR_BASE + PTR_SIZE).contains(&a) => self.ptr.write(a - PTR_BASE, size, val),
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
        self.load_zeroed(addr, data, 0)
    }

    /// Copies `data` to `addr` and zeroes the bytes after it up to `size` in all (a
    /// loadable segment and its .bss). Fails, changing nothing, unless the whole range
    /// is RAM.
    pub fn load_zeroed(&mut self, addr: u32, data: &[u8], size: u32) -> Result<(), BusFault> {
        let size = size.max(u32::try_from(data.len()).map_err(|_| BusFault)?);
        let off = self.ram_range(addr, size).ok_or(BusFault)?;
        let (head, tail) = self.ram[off..off + size as usize].split_at_mut(data.len());
        head.copy_from_slice(data);
        tail.fill(0);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::{self, MemDisk, SECTOR_SIZE};
    use crate::net::{Hub, MacAddr, NetBackend};
    use crate::sysinfo::{self, DEV_ENTRY_SIZE, DEV_TABLE};
    use std::sync::{Arc, Mutex};

    fn small_bus() -> Bus {
        Bus::new(&MachineConfig::with_ram(4096), Vec::new())
    }

    #[test]
    fn load_zeroed_checks_the_whole_range_first() {
        let mut bus = small_bus();
        bus.ram.fill(0xaa);
        bus.load_zeroed(RAM_BASE + 8, &[1, 2], 6).unwrap();
        assert_eq!(&bus.ram[6..16], &[0xaa, 0xaa, 1, 2, 0, 0, 0, 0, 0xaa, 0xaa]);
        // Past the end of RAM, by data or by size: refused, nothing written.
        assert_eq!(bus.load_zeroed(RAM_BASE + 4094, &[3, 3, 3], 0), Err(BusFault));
        assert_eq!(bus.load_zeroed(RAM_BASE, &[3], u32::MAX), Err(BusFault));
        // VRAM is memory, but programs are loaded into RAM only (spec section 7).
        assert_eq!(bus.load(VRAM_BASE, &[3]), Err(BusFault));
        assert_eq!(bus.fb.vram[0], 0);
        assert_eq!(bus.load_zeroed(RAM_BASE + 4000, &[], 0x8000_0000), Err(BusFault));
        assert_eq!(bus.load_zeroed(RAM_BASE - 4, &[3], 8), Err(BusFault));
        assert_eq!(bus.ram[0], 0xaa);
        assert_eq!(bus.ram[4094], 0xaa);
        bus.load(RAM_BASE + 4095, &[9]).unwrap();
        assert_eq!(bus.ram[4095], 9);
    }

    #[test]
    fn the_rom_has_one_writable_word() {
        for harts in [1, 2] {
            let config = MachineConfig {
                harts,
                ..MachineConfig::with_ram(4096)
            };
            let mut bus = Bus::new(&config, Vec::new());
            bus.set_boot_entry(RAM_BASE);
            let stub = bus.peek(BOOT_ROM_BASE);
            // The entry word takes 32-bit stores; nothing else in the ROM does.
            assert_eq!(bus.write(BOOT_ROM_BASE + 16, 4, 0x8000_1234), Ok(()));
            assert_eq!(bus.peek(BOOT_ROM_BASE + 16), Some(0x8000_1234));
            assert_eq!(bus.read(BOOT_ROM_BASE + 16, 4), Ok(0x8000_1234));
            for (addr, size) in [
                (BOOT_ROM_BASE + 16, 1),
                (BOOT_ROM_BASE + 16, 2),
                (BOOT_ROM_BASE, 4),
            ] {
                assert_eq!(bus.write(addr, size, 0), Err(BusFault), "{addr:#x}/{size}");
            }
            assert_eq!(bus.write(BOOT_ROM_BASE + 0x200, 4, 0), Err(BusFault));
            assert_eq!(bus.peek(BOOT_ROM_BASE), stub);
            assert_eq!(bus.peek(BOOT_ROM_BASE + 16), Some(0x8000_1234));
        }
        // On an empty ROM too (no program yet): the word is there to write.
        let mut bus = Bus::new(&MachineConfig::with_ram(4096), Vec::new());
        assert_eq!(bus.write(BOOT_ROM_BASE + 16, 4, 7), Ok(()));
        assert_eq!(bus.peek(BOOT_ROM_BASE + 16), Some(7));
    }

    #[test]
    fn disk_boot_fills_the_rom() {
        let mut bus = Bus::new(&MachineConfig::with_ram(4096), Vec::new());
        bus.set_disk_boot();
        // The stub, its entry word pointing at the loader, and the loader.
        assert_eq!(bus.peek(BOOT_ROM_BASE), Some(0x0000_0297), "auipc t0, 0");
        assert_eq!(bus.peek(BOOT_ROM_BASE + 16), Some(boot::DISK_BOOT_PC));
        assert_eq!(
            bus.peek(boot::DISK_BOOT_PC),
            Some(0xf140_22f3),
            "csrr t0, mhartid"
        );
        let loader = boot::disk_loader();
        let at = (boot::DISK_BOOT_PC - BOOT_ROM_BASE) as usize;
        assert_eq!(&bus.rom[at..], &loader[..]);
        assert!(
            bus.rom[20..at].iter().all(|&b| b == 0),
            "no park loop on one hart"
        );

        let config = MachineConfig {
            harts: 2,
            ..MachineConfig::with_ram(4096)
        };
        let mut bus = Bus::new(&config, Vec::new());
        bus.set_disk_boot();
        assert_eq!(bus.peek(PARK_PC), Some(0xf140_2573), "the park loop");
        assert_eq!(&bus.rom[at..], &loader[..]);
    }

    #[test]
    fn harts_share_the_bus() {
        let config = MachineConfig {
            harts: 3,
            ..MachineConfig::with_ram(4096)
        };
        let mut bus = Bus::new(&config, Vec::new());
        assert_eq!(bus.harts(), 3);
        // The same devices as a one-hart machine: the CLINT and PLIC pages have room.
        assert_eq!(bus.sysinfo.devices(), device_table(&MachineConfig::default(), 0));
        assert_eq!(bus.read(SYSINFO_BASE + sysinfo::REG_HARTS, 4), Ok(3));
        // The park loop is there from the start; set_boot_entry only sets the entry.
        assert_eq!(bus.peek(PARK_PC), Some(0xf140_2573), "csrr a0, mhartid");
        assert_eq!(bus.peek(BOOT_ROM_BASE + 16), Some(0));
        bus.set_boot_entry(RAM_BASE);
        assert_eq!(bus.peek(BOOT_ROM_BASE + 16), Some(RAM_BASE));
        assert_eq!(bus.peek(PARK_PC), Some(0xf140_2573));

        // A store to memory breaks every hart's reservation on the word; a store to a
        // device only the storing hart's.
        let reserve = |bus: &mut Bus| {
            for hart in 0..3 {
                bus.reservation_mut(hart).reserve(RAM_BASE + 4 * hart as u32);
            }
        };
        let held = |bus: &Bus| -> Vec<bool> { (0..3).map(|h| bus.reservation(h).held().is_some()).collect() };
        reserve(&mut bus);
        bus.snoop_store(0, RAM_BASE + 4, 4);
        assert_eq!(held(&bus), [true, false, true]);
        bus.snoop_store(1, RAM_BASE, 8);
        assert_eq!(held(&bus), [false, false, true]);
        reserve(&mut bus);
        bus.snoop_store(2, UART_BASE, 1);
        assert_eq!(held(&bus), [true, true, false]);
    }

    #[test]
    fn sysinfo_on_the_bus() {
        let config = MachineConfig {
            ram_size: 32 * 1024 * 1024,
            clock_hz: Some(250_000),
            ..MachineConfig::default()
        };
        let mut bus = Bus::new(&config, Vec::new());
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
        // The next page is the pointer's.
        assert_eq!(bus.read(SYSINFO_BASE + SYSINFO_SIZE, 4), Ok(0));
        assert_eq!(bus.read(PTR_BASE + PTR_SIZE, 4), Err(BusFault));
        // Not memory: no instruction fetches from it.
        assert_eq!(bus.read_mem(SYSINFO_BASE, 4), Err(BusFault));
    }

    #[test]
    fn device_table_matches_the_bus() {
        let bus = small_bus();
        assert_eq!(bus.sysinfo.devices(), device_table(&MachineConfig::default(), 0));
        device_table_matches(bus);
    }

    /// Checks that `bus`'s sysinfo table describes the bus: every entry readable by
    /// software, decoded at both ends, no overlaps, and the right interrupt sources.
    fn device_table_matches(mut bus: Bus) {
        let count = bus.read(SYSINFO_BASE + sysinfo::REG_DEV_COUNT, 4).unwrap();
        let table = bus.sysinfo.devices().to_vec();
        assert_eq!(count as usize, table.len());
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
        // The UART's, keyboard's and pointer's PLIC sources are the ones the bus wires them to.
        let uart = table.iter().find(|d| d.kind == DeviceType::Uart).unwrap();
        assert_eq!(uart.irq, UART_IRQ);
        let kbd = table.iter().find(|d| d.kind == DeviceType::Keyboard).unwrap();
        assert_eq!(kbd.irq, KBD_IRQ);
        let ptr = table.iter().find(|d| d.kind == DeviceType::Pointer).unwrap();
        assert_eq!((ptr.base, ptr.irq), (PTR_BASE, PTR_IRQ));
        // Disks are listed in order, each on its own source.
        let disks: Vec<_> = table.iter().filter(|d| d.kind == DeviceType::Block).collect();
        assert_eq!(disks.len(), bus.disks.len());
        for (i, d) in disks.iter().enumerate() {
            assert_eq!(
                (d.base, d.irq),
                (BLK_BASE + i as u32 * BLK_SIZE, BLK_IRQ + i as u32)
            );
        }
        // So are network cards.
        let nets: Vec<_> = table.iter().filter(|d| d.kind == DeviceType::Net).collect();
        assert_eq!(nets.len(), bus.nets.len());
        for (i, d) in nets.iter().enumerate() {
            assert_eq!(
                (d.base, d.irq),
                (NET_BASE + i as u32 * NET_SIZE, NET_IRQ + i as u32)
            );
        }
        // All the sources are different and exist on the PLIC.
        let mut irqs: Vec<u32> = table.iter().map(|d| d.irq).filter(|&irq| irq != 0).collect();
        irqs.sort();
        irqs.dedup();
        assert_eq!(irqs.len(), 3 + bus.disks.len() + bus.nets.len());
        assert!(irqs.iter().all(|&irq| irq < crate::devices::PLIC_SOURCES));
    }

    /// A bus with `disks` disks of one sector and `nets` network cards, each on its own
    /// hub port; card `i` has MAC address `MacAddr::local(i + 1)`.
    fn bus_with_nets(disks: usize, nets: usize, hub: &Hub) -> Bus {
        let disks = (0..disks)
            .map(|_| BlockDevice::new(Box::new(MemDisk::new(1))).unwrap())
            .collect();
        let nets = (0..nets)
            .map(|i| NetDevice::new(MacAddr::local(i as u32 + 1), Box::new(hub.attach())))
            .collect();
        Bus::with_nets(&MachineConfig::with_ram(4096), disks, nets)
    }

    #[test]
    fn device_table_lists_every_network_card() {
        let hub = Hub::new();
        for (disks, nets) in [(0, 1), (2, 1), (0, MAX_NETS), (MAX_DISKS, MAX_NETS)] {
            let bus = bus_with_nets(disks, nets, &hub);
            let table = bus.sysinfo.devices();
            assert_eq!(table, device_table_with(&MachineConfig::default(), disks, nets));
            assert_eq!(table.len(), 9 + disks + nets);
            device_table_matches(bus);
        }
        // The highest PLIC source in use.
        assert_eq!(NET_IRQ + MAX_NETS as u32 - 1, 15);
        // device_table counts the config's cards.
        let config = MachineConfig {
            nets: vec![crate::NetConfig::hub(&hub); 2],
            ..MachineConfig::default()
        };
        assert_eq!(device_table(&config, 1), device_table_with(&config, 1, 2));
    }

    #[test]
    fn network_card_registers_on_the_bus() {
        use crate::net::{self, IRQ_TX};
        let hub = Hub::new();
        let mut bus = bus_with_nets(0, 2, &hub);
        let mut peer = hub.attach();
        let card1 = NET_BASE + NET_SIZE;
        assert_eq!(bus.read(NET_BASE + net::REG_MAC_LO, 4), Ok(0x0000_ca02));
        assert_eq!(bus.read(NET_BASE + net::REG_MAC_HI, 4), Ok(0x0100));
        assert_eq!(bus.read(card1 + net::REG_MAC_HI, 4), Ok(0x0200));

        // Card 1 sends a frame from RAM: a one-descriptor ring at RAM_BASE, the frame
        // at RAM_BASE + 64.
        let frame: Vec<u8> = (0..20).collect();
        bus.ram[64..84].copy_from_slice(&frame);
        bus.ram[..4].copy_from_slice(&(RAM_BASE + 64).to_le_bytes());
        bus.ram[4..6].copy_from_slice(&20u16.to_le_bytes());
        bus.write(card1 + net::REG_TX_BASE, 4, RAM_BASE).unwrap();
        bus.write(card1 + net::REG_TX_SIZE, 4, 1).unwrap();
        bus.write(card1 + net::REG_IRQ_ENABLE, 4, IRQ_TX).unwrap();
        bus.write(card1 + net::REG_TX_TAIL, 4, 1).unwrap();
        assert_eq!(bus.read(card1 + net::REG_TX_HEAD, 4), Ok(1));
        assert_eq!(peer.recv(), Some(frame.clone()));
        // Card 0 is on the same hub, and gets it at its next poll: no ring, so dropped.
        for _ in 0..net::POLL_STEPS {
            bus.update_interrupts();
        }
        assert_eq!(bus.read(NET_BASE + net::REG_RX_DROPPED, 4), Ok(1));
        // Card 1's line reaches the PLIC as source NET_IRQ + 1.
        assert_eq!(bus.read(PLIC_BASE + 0x1000, 4), Ok(1 << (NET_IRQ + 1)));

        // Words only; nothing past the last card; not memory.
        assert_eq!(bus.read(NET_BASE, 1), Err(BusFault));
        assert_eq!(bus.write(NET_BASE + net::REG_TX_TAIL, 2, 1), Err(BusFault));
        assert_eq!(bus.read(NET_BASE + 2 * NET_SIZE, 4), Err(BusFault));
        assert_eq!(bus.write(NET_BASE + 2 * NET_SIZE, 4, 0), Err(BusFault));
        assert_eq!(bus.read_mem(NET_BASE, 4), Err(BusFault));
        // Without cards the range is unmapped, even with every disk.
        let mut bus = bus_with_nets(MAX_DISKS, 0, &hub);
        assert_eq!(bus.read(NET_BASE, 4), Err(BusFault));
        assert_eq!(bus.write(NET_BASE + net::REG_TX_TAIL, 4, 1), Err(BusFault));
        assert!(bus.read(NET_BASE - 4, 4).is_ok(), "the last disk's page");
    }

    /// A bus with `n` disks of one sector each, disk `i` filled with byte `i`.
    fn bus_with_disks(n: usize) -> Bus {
        let disks = (0..n)
            .map(|i| {
                let disk = MemDisk::from_image(vec![i as u8; SECTOR_SIZE]).unwrap();
                BlockDevice::new(Box::new(disk)).unwrap()
            })
            .collect();
        Bus::new(&MachineConfig::with_ram(4096), disks)
    }

    #[test]
    fn device_table_lists_every_disk() {
        for n in [0, 1, 3, MAX_DISKS] {
            let bus = bus_with_disks(n);
            let table = bus.sysinfo.devices();
            assert_eq!(table, device_table(&MachineConfig::default(), n));
            assert_eq!(table.len(), 9 + n);
            device_table_matches(bus);
        }
    }

    #[test]
    fn disk_registers_on_the_bus() {
        let mut bus = bus_with_disks(2);
        let disk1 = BLK_BASE + BLK_SIZE;
        assert_eq!(bus.read(BLK_BASE + block::REG_CAPACITY, 4), Ok(1));
        assert_eq!(bus.read(disk1 + block::REG_CAPACITY, 4), Ok(1));
        // Disk 1, sector 0 -> the start of RAM, with DMA through the bus's own RAM.
        bus.write(disk1 + block::REG_DMA, 4, RAM_BASE + 8).unwrap();
        bus.write(disk1 + block::REG_COUNT, 4, 1).unwrap();
        bus.write(disk1 + block::REG_CTRL, 4, 1).unwrap();
        bus.write(disk1 + block::REG_CMD, 4, block::CMD_READ).unwrap();
        assert_eq!(bus.read(disk1 + block::REG_STATUS, 4), Ok(block::STATUS_DONE));
        assert_eq!(bus.read(disk1 + block::REG_ERROR, 4), Ok(0));
        assert_eq!(
            bus.read(BLK_BASE + block::REG_STATUS, 4),
            Ok(0),
            "disk 0 did nothing"
        );
        assert_eq!(&bus.ram[..8], &[0; 8]);
        assert_eq!(&bus.ram[8..8 + SECTOR_SIZE], &[1; SECTOR_SIZE]);
        assert_eq!(bus.ram[8 + SECTOR_SIZE], 0);

        // Its line reaches the PLIC as source BLK_IRQ + 1.
        bus.update_interrupts();
        assert_eq!(bus.read(PLIC_BASE + 0x1000, 4), Ok(1 << (BLK_IRQ + 1)));

        // Words only; nothing past the last disk; not memory.
        assert_eq!(bus.read(BLK_BASE, 1), Err(BusFault));
        assert_eq!(bus.write(BLK_BASE + block::REG_CMD, 2, 1), Err(BusFault));
        assert_eq!(bus.read(BLK_BASE + 2 * BLK_SIZE, 4), Err(BusFault));
        assert_eq!(bus.write(BLK_BASE + 2 * BLK_SIZE, 4, 0), Err(BusFault));
        assert_eq!(bus.read_mem(BLK_BASE, 4), Err(BusFault));
        // Without disks the range is unmapped.
        let mut bus = small_bus();
        assert_eq!(bus.read(BLK_BASE, 4), Err(BusFault));
        assert_eq!(bus.write(BLK_BASE + block::REG_CMD, 4, 1), Err(BusFault));
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

    #[test]
    fn pointer_registers_on_the_bus() {
        use crate::pointer::{self as ptr, PointerInput, BUTTON_LEFT};
        let mut bus = small_bus();
        assert_eq!(bus.read(PTR_BASE + ptr::REG_STATUS, 4), Ok(0));
        assert_eq!(bus.read(PTR_BASE + ptr::REG_EVENT, 4), Ok(0));
        bus.ptr
            .push(PointerInput::move_to(320, 240).with_buttons(BUTTON_LEFT));
        assert_eq!(bus.read(PTR_BASE + ptr::REG_STATUS, 4), Ok(ptr::STATUS_EVENT));
        assert_eq!(bus.read(PTR_BASE + ptr::REG_EVENT, 4), Ok(0x8000_0001));
        assert_eq!(bus.read(PTR_BASE + ptr::REG_EVENT_XY, 4), Ok(320 | 240 << 16));
        assert_eq!(bus.read(PTR_BASE + ptr::REG_POS, 4), Ok(320 | 240 << 16));
        assert_eq!(bus.read(PTR_BASE + ptr::REG_BUTTONS, 4), Ok(1));
        assert_eq!(bus.read(PTR_BASE + ptr::REG_STATUS, 4), Ok(0));

        // Its line reaches the PLIC as source PTR_IRQ once enabled.
        bus.ptr.push(PointerInput::move_to(1, 1));
        bus.update_interrupts();
        assert_eq!(bus.read(PLIC_BASE + 0x1000, 4), Ok(0));
        bus.write(PTR_BASE + ptr::REG_CTRL, 4, ptr::CTRL_IRQ_ENABLE)
            .unwrap();
        bus.update_interrupts();
        assert_eq!(bus.read(PLIC_BASE + 0x1000, 4), Ok(1 << PTR_IRQ));

        // Words only, nothing popped by a faulting access; the page ends the range.
        assert_eq!(bus.read(PTR_BASE + ptr::REG_EVENT, 1), Err(BusFault));
        assert_eq!(bus.read(PTR_BASE + ptr::REG_EVENT, 2), Err(BusFault));
        assert_eq!(bus.write(PTR_BASE + ptr::REG_CTRL, 2, 0), Err(BusFault));
        assert_eq!(bus.ptr.len(), 1);
        assert_eq!(bus.read(PTR_BASE + 0xffc, 4), Ok(0));
        assert_eq!(bus.read(PTR_BASE + PTR_SIZE, 4), Err(BusFault));
        // Data accesses only.
        assert_eq!(bus.read_mem(PTR_BASE, 4), Err(BusFault));
    }
}
