//! Grail: an emulator for the Excalibur ISA (RV32IM + Zicsr, M/S/U modes, Sv32).

pub mod bus;
pub mod cpu;
pub mod devices;
pub mod display;
pub mod elf;
pub mod framebuffer;
pub mod trap;
#[cfg(windows)]
mod win32;
#[cfg(windows)]
mod window;

use bus::{Bus, Exit};
use cpu::Cpu;

pub const DEFAULT_RAM_SIZE: usize = 128 * 1024 * 1024;

pub struct Machine {
    pub cpu: Cpu,
}

#[derive(Debug, PartialEq, Eq)]
pub enum RunResult {
    Exited(u32),
    StepLimit,
}

impl Machine {
    pub fn new(ram_size: usize) -> Self {
        Machine {
            cpu: Cpu::new(Bus::new(ram_size)),
        }
    }

    /// Loads an ELF image into RAM and points the boot ROM at its entry.
    pub fn load_elf(&mut self, file: &[u8]) -> Result<(), String> {
        let elf = elf::parse(file).map_err(|e| e.to_string())?;
        let bus = &mut self.cpu.bus;
        for seg in &elf.segments {
            bus.load(seg.paddr, &seg.data)
                .map_err(|_| format!("segment at {:#x} is outside RAM", seg.paddr))?;
            let zeros = vec![0; (seg.mem_size as usize).saturating_sub(seg.data.len())];
            bus.load(seg.paddr + seg.data.len() as u32, &zeros)
                .map_err(|_| format!("segment at {:#x} is outside RAM", seg.paddr))?;
        }
        bus.tohost = elf.symbol("tohost");
        bus.set_boot_entry(elf.entry);
        Ok(())
    }

    /// Exit code, once the program has asked to stop.
    pub fn exit_code(&self) -> Option<u32> {
        self.cpu.bus.exit.map(|Exit::Code(c)| c)
    }

    /// Runs at most `steps` steps. Stops early, right after the instruction that did
    /// it, when the program exits or writes `FB_PRESENT`, so the host can show or dump
    /// that exact frame. Returns how many steps ran.
    pub fn run_slice(&mut self, steps: u64, trace: &mut impl FnMut(&Cpu)) -> u64 {
        let mut n = 0;
        while n < steps && self.cpu.bus.exit.is_none() {
            trace(&self.cpu);
            self.cpu.step();
            n += 1;
            if self.cpu.bus.fb.present_pending() {
                break;
            }
        }
        n
    }

    /// Runs until the program exits or `max_steps` steps have run.
    pub fn run(&mut self, max_steps: Option<u64>, mut trace: impl FnMut(&Cpu)) -> RunResult {
        let mut steps = 0u64;
        loop {
            if let Some(Exit::Code(c)) = self.cpu.bus.exit {
                return RunResult::Exited(c);
            }
            if max_steps.is_some_and(|m| steps >= m) {
                return RunResult::StepLimit;
            }
            trace(&self.cpu);
            self.cpu.step();
            steps += 1;
        }
    }
}
