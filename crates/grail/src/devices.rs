//! Memory-mapped devices.

use crate::bus::{BusFault, Exit};
use std::collections::VecDeque;
use std::io::Write;
use std::sync::mpsc::Receiver;

/// Core-local interruptor: software interrupt (msip) and timer (mtime/mtimecmp).
pub struct Clint {
    pub msip: bool,
    pub mtimecmp: u64,
    pub mtime: u64,
}

impl Default for Clint {
    fn default() -> Self {
        // mtimecmp resets to "never" so the timer does not fire before software sets it.
        Clint {
            msip: false,
            mtimecmp: u64::MAX,
            mtime: 0,
        }
    }
}

impl Clint {
    pub fn timer_pending(&self) -> bool {
        self.mtime >= self.mtimecmp
    }

    pub fn read(&mut self, off: u32, size: u32) -> Result<u32, BusFault> {
        if size != 4 {
            return Err(BusFault);
        }
        Ok(match off {
            0x0 => self.msip as u32,
            0x4000 => self.mtimecmp as u32,
            0x4004 => (self.mtimecmp >> 32) as u32,
            0xbff8 => self.mtime as u32,
            0xbffc => (self.mtime >> 32) as u32,
            _ => 0,
        })
    }

    pub fn write(&mut self, off: u32, size: u32, val: u32) -> Result<(), BusFault> {
        if size != 4 {
            return Err(BusFault);
        }
        let lo = |old: u64| (old & !0xffff_ffff) | val as u64;
        let hi = |old: u64| (old & 0xffff_ffff) | ((val as u64) << 32);
        match off {
            0x0 => self.msip = val & 1 != 0,
            0x4000 => self.mtimecmp = lo(self.mtimecmp),
            0x4004 => self.mtimecmp = hi(self.mtimecmp),
            0xbff8 => self.mtime = lo(self.mtime),
            0xbffc => self.mtime = hi(self.mtime),
            _ => {}
        }
        Ok(())
    }
}

/// A small subset of a 16550 UART. Transmit goes straight to the host's stdout,
/// receive comes from a host input channel. No interrupts yet (needs the PLIC).
#[derive(Default)]
pub struct Uart {
    pub input: Option<Receiver<u8>>,
    rx: VecDeque<u8>,
    ier: u8,
    lcr: u8,
    mcr: u8,
    scr: u8,
    /// Everything transmitted, kept when `capture` is on (used by tests).
    pub captured: Vec<u8>,
    pub capture: bool,
}

const LSR_DR: u8 = 1 << 0; // data ready
const LSR_THRE: u8 = 1 << 5; // transmit holding register empty
const LSR_TEMT: u8 = 1 << 6; // transmitter empty

impl Uart {
    fn poll_input(&mut self) {
        if let Some(rx) = &self.input {
            while let Ok(b) = rx.try_recv() {
                self.rx.push_back(b);
            }
        }
    }

    pub fn read(&mut self, off: u32, size: u32) -> Result<u32, BusFault> {
        if size != 1 {
            return Err(BusFault);
        }
        self.poll_input();
        let dlab = self.lcr & 0x80 != 0;
        Ok(match off {
            0 if !dlab => self.rx.pop_front().unwrap_or(0) as u32,
            1 if !dlab => self.ier as u32,
            2 => 0x01, // IIR: no interrupt pending
            3 => self.lcr as u32,
            4 => self.mcr as u32,
            5 => (LSR_THRE | LSR_TEMT | if self.rx.is_empty() { 0 } else { LSR_DR }) as u32,
            7 => self.scr as u32,
            _ => 0,
        })
    }

    pub fn write(&mut self, off: u32, size: u32, val: u32) -> Result<(), BusFault> {
        if size != 1 {
            return Err(BusFault);
        }
        let b = val as u8;
        let dlab = self.lcr & 0x80 != 0;
        match off {
            0 if !dlab => {
                if self.capture {
                    self.captured.push(b);
                } else {
                    let mut out = std::io::stdout().lock();
                    let _ = out.write_all(&[b]);
                    let _ = out.flush();
                }
            }
            1 if !dlab => self.ier = b & 0x0f,
            3 => self.lcr = b,
            4 => self.mcr = b,
            7 => self.scr = b,
            _ => {} // divisor latch, FCR: accepted and ignored
        }
        Ok(())
    }
}

/// SiFive-style test finisher: write 0x5555 to pass, 0x3333 | (code << 16) to fail.
pub struct TestDevice;

impl TestDevice {
    pub fn write(&mut self, off: u32, val: u32) -> Option<Exit> {
        if off != 0 {
            return None;
        }
        match val & 0xffff {
            0x5555 => Some(Exit::Code(0)),
            0x3333 => Some(Exit::Code(val >> 16)),
            _ => None,
        }
    }
}
