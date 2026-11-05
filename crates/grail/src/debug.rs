//! The debug interface of a [`Machine`]: what a debugger needs from the emulator, and
//! what Grail's GDB remote stub ([`crate::gdb`]) is built on (spec section 7,
//! "Debugging"; `docs/merlin-dbg.md`).
//!
//! - **Running**: [`Machine::debug_continue`] runs the machine until something stops
//!   it or a budget of steps is spent, so a host can look for a pause request (a GDB
//!   client's Ctrl-C) between two calls; [`Machine::debug_step`] runs one instruction
//!   of the selected hart. A machine the host does not run is paused.
//! - **Breakpoints** ([`Machine::set_breakpoint`]) are kept here, not written into guest
//!   memory: before each hart's turn, the machine checks whether that hart is about to
//!   execute the instruction at a breakpoint's address. The program never sees them.
//! - **Watchpoints** ([`Machine::set_watchpoint`]) stop the machine right after an
//!   instruction that reads or writes a watched range of virtual addresses.
//! - **State**: registers, pc, CSRs and memory of the selected hart
//!   ([`Machine::select_hart`]), memory through its address translation or physical
//!   ([`Space`]).
//!
//! Addresses and values are `u64`, as wide as an RV64 hart's. On an RV32 machine they
//! are its 32-bit values: registers, the pc and CSRs read as their low 32 bits
//! (zero-extended), and virtual addresses past 4 GiB are not in its address space.
//!
//! Debugging does not change what the machine does: a debugged run executes exactly the
//! instructions an undebugged one does, in the same order, with the same results. The
//! machine stops between two harts' turns ([`Cpu::turn_traced`]) and picks up where it
//! stopped, and nothing a debugger reads has a side effect (memory reads never reach a
//! device register, translation sets no A/D bits).

use crate::cpu::{Cpu, Mode};
use crate::Machine;
use excalibur::Xlen;
use std::collections::BTreeSet;
use std::time::Instant;

/// What a watchpoint watches for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchKind {
    /// Stores (and the write of an AMO or a successful `sc.w`).
    Write,
    /// Loads (and the read of an AMO or `lr.w`).
    Read,
    /// Either.
    Access,
}

impl WatchKind {
    fn matches(self, read: bool, write: bool) -> bool {
        match self {
            WatchKind::Write => write,
            WatchKind::Read => read,
            WatchKind::Access => read || write,
        }
    }
}

/// A watched range of virtual addresses, `[addr, addr + len)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Watchpoint {
    pub addr: u64,
    pub len: u64,
    pub kind: WatchKind,
}

/// A watchpoint an access hit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WatchHit {
    pub hart: usize,
    pub kind: WatchKind,
    /// The first watched byte the access touched.
    pub addr: u64,
}

/// The watchpoints, kept on the bus, where the harts' loads and stores report to them
/// ([`Watches::note`]), and the first hit since the debugger last looked.
#[derive(Default)]
pub struct Watches {
    list: Vec<Watchpoint>,
    hit: Option<WatchHit>,
}

impl Watches {
    /// A hart's load or store of `size` bytes at `vaddr` succeeded (`read` and `write`
    /// say which it was; an AMO is both). Cheap when there is no watchpoint: this runs on
    /// every access.
    #[inline]
    pub(crate) fn note(&mut self, hart: u32, vaddr: u64, size: u32, read: bool, write: bool) {
        if !self.list.is_empty() && self.hit.is_none() {
            self.check(hart, vaddr, size, read, write);
        }
    }

    fn check(&mut self, hart: u32, vaddr: u64, size: u32, read: bool, write: bool) {
        // In u128: a range may end at 2^64.
        let end = u128::from(vaddr) + u128::from(size);
        self.hit = self
            .list
            .iter()
            .find(|w| {
                let w_end = u128::from(w.addr) + u128::from(w.len);
                w.kind.matches(read, write) && u128::from(vaddr) < w_end && u128::from(w.addr) < end
            })
            .map(|w| WatchHit {
                hart: hart as usize,
                kind: w.kind,
                addr: vaddr.max(w.addr),
            });
    }
}

/// Why the machine stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stop {
    /// `hart` is about to execute the instruction at `pc`, a breakpoint.
    Breakpoint { hart: usize, pc: u64 },
    /// An instruction of `hart` hit a watchpoint; it has completed, and the hart's pc is
    /// the next instruction.
    Watchpoint(WatchHit),
    /// [`Machine::debug_step`] ran one instruction of `hart`.
    Stepped { hart: usize },
    /// The program stopped the machine with this exit code.
    Exited(u32),
}

/// Which addresses a debugger's memory access uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Space {
    /// Virtual addresses, translated as the selected hart's instruction fetches are now
    /// (physical in M-mode or with paging off), without permission checks.
    Virtual,
    /// Physical addresses.
    Physical,
}

/// A machine's debugger state: breakpoints and the selected hart.
pub struct Debug {
    breakpoints: BTreeSet<u64>,
    hart: usize,
    /// The machine has stopped (or not run yet) since it last ran a turn: the next turn
    /// is not stopped by a breakpoint at its hart's pc, so resuming from a breakpoint
    /// executes that instruction instead of stopping on it again.
    resuming: bool,
    /// Steps run by [`Machine::debug_continue`] and [`Machine::debug_step`].
    steps: u64,
}

impl Default for Debug {
    fn default() -> Self {
        Debug {
            breakpoints: BTreeSet::new(),
            hart: 0,
            resuming: true,
            steps: 0,
        }
    }
}

impl Machine {
    /// Sets a breakpoint at virtual address `pc`: a hart about to execute the
    /// instruction there stops the machine. Returns false if there was one already; an
    /// error for an address past the machine's address space (4 GiB on RV32), which no
    /// hart could ever reach.
    pub fn set_breakpoint(&mut self, pc: u64) -> Result<bool, String> {
        if pc & !self.width_mask() != 0 {
            return Err(format!("{pc:#x} is past the end of the address space"));
        }
        Ok(self.debug.breakpoints.insert(pc))
    }

    /// Removes the breakpoint at `pc`. Returns false if there was none.
    pub fn clear_breakpoint(&mut self, pc: u64) -> bool {
        self.debug.breakpoints.remove(&pc)
    }

    /// The breakpoints, in address order.
    pub fn breakpoints(&self) -> impl Iterator<Item = u64> + '_ {
        self.debug.breakpoints.iter().copied()
    }

    /// Watches `[addr, addr + len)` (virtual addresses) for `kind` accesses by any hart.
    /// An error for an empty range or one past the end of the machine's address space
    /// (2^64, or 2^32 on RV32).
    pub fn set_watchpoint(&mut self, addr: u64, len: u64, kind: WatchKind) -> Result<(), String> {
        let end = u128::from(addr) + u128::from(len);
        if len == 0 || end > 1u128 << self.config().xlen.bits() {
            return Err(format!("cannot watch {len} bytes at {addr:#x}"));
        }
        let w = Watchpoint { addr, len, kind };
        let list = &mut self.cpu.bus.watches.list;
        if !list.contains(&w) {
            list.push(w);
        }
        Ok(())
    }

    /// Removes a watchpoint set with the same range and kind. Returns false if there was
    /// none.
    pub fn clear_watchpoint(&mut self, addr: u64, len: u64, kind: WatchKind) -> bool {
        let list = &mut self.cpu.bus.watches.list;
        let before = list.len();
        list.retain(|w| *w != Watchpoint { addr, len, kind });
        list.len() != before
    }

    /// The watchpoints, in the order they were set.
    pub fn watchpoints(&self) -> &[Watchpoint] {
        &self.cpu.bus.watches.list
    }

    /// The steps [`Machine::debug_continue`] and [`Machine::debug_step`] have run, as
    /// [`Machine::run_for`] counts them (idle ticks of a real-time machine waiting in
    /// `wfi` included): a host that runs the machine both ways adds them up.
    pub fn debug_steps(&self) -> u64 {
        self.debug.steps
    }

    /// Whether the machine has a breakpoint or a watchpoint: a host runs it with
    /// [`Machine::debug_continue`] then, debugger or not, so that they stop it (QEMU
    /// keeps a debugger's breakpoints when its connection ends without `D`).
    pub fn has_debug_points(&self) -> bool {
        !self.debug.breakpoints.is_empty() || !self.cpu.bus.watches.list.is_empty()
    }

    /// Removes every breakpoint and watchpoint (a debugger detaching, or a new one
    /// asking why the machine stopped, `?`).
    pub fn clear_debug_points(&mut self) {
        self.debug.breakpoints.clear();
        self.cpu.bus.watches.list.clear();
        self.cpu.bus.watches.hit = None;
    }

    /// Selects the hart the register, CSR and memory accessors and
    /// [`Machine::debug_step`] use: hart 0 to begin with.
    pub fn select_hart(&mut self, hart: usize) -> Result<(), String> {
        if hart >= self.cpu.harts().len() {
            return Err(format!(
                "no hart {hart}: the machine has {}",
                self.cpu.harts().len()
            ));
        }
        self.debug.hart = hart;
        Ok(())
    }

    pub fn selected_hart(&self) -> usize {
        self.debug.hart
    }

    /// The machine's values as a debugger sees them: all 64 bits, or the low 32 on RV32
    /// (whose registers the harts keep sign-extended).
    fn width_mask(&self) -> u64 {
        match self.config().xlen {
            Xlen::Rv32 => u32::MAX.into(),
            Xlen::Rv64 => u64::MAX,
        }
    }

    /// Register `x<r>` of the selected hart (`r` < 32).
    pub fn reg(&self, r: usize) -> u64 {
        self.cpu.hart(self.debug.hart).x[r] & self.width_mask()
    }

    /// Writes register `x<r>` of the selected hart (its low 32 bits on RV32). Writes to
    /// `x0` are ignored.
    pub fn set_reg(&mut self, r: usize, v: u64) {
        self.cpu.hart_mut(self.debug.hart).set_reg(r as u8, v);
    }

    /// The selected hart's pc: the instruction it executes next.
    pub fn pc(&self) -> u64 {
        self.cpu.hart(self.debug.hart).pc & self.width_mask()
    }

    pub fn set_pc(&mut self, pc: u64) {
        let mask = self.width_mask();
        self.cpu.hart_mut(self.debug.hart).pc = pc & mask;
    }

    /// The selected hart's privilege mode.
    pub fn mode(&self) -> Mode {
        self.cpu.hart(self.debug.hart).mode
    }

    pub fn set_mode(&mut self, mode: Mode) {
        self.cpu.hart_mut(self.debug.hart).mode = mode;
    }

    /// CSR `addr` of the selected hart, whatever its privilege mode. `None` if Grail has
    /// no such CSR.
    pub fn csr(&self, addr: u16) -> Option<u64> {
        let mask = self.width_mask();
        self.cpu
            .hart(self.debug.hart)
            .csr_value(&self.cpu.bus, addr)
            .map(|v| v & mask)
    }

    /// Writes CSR `addr` of the selected hart, with the CSR's WARL rules. Returns false if
    /// Grail has no such CSR or it is read-only.
    pub fn set_csr(&mut self, addr: u16, v: u64) -> bool {
        self.cpu.hart_mut(self.debug.hart).csr_store(addr, v)
    }

    /// The physical address of `addr` in `space`, as [`Machine::read_memory`] sees it.
    /// A virtual address an RV32 hart cannot form (past 4 GiB) has none.
    pub fn translate(&self, addr: u64, space: Space) -> Option<u64> {
        match space {
            Space::Physical => Some(addr),
            Space::Virtual if addr & !self.width_mask() != 0 => None,
            Space::Virtual => self
                .cpu
                .hart(self.debug.hart)
                .debug_translate(&self.cpu.bus, addr),
        }
    }

    /// Reads memory at `addr` into `buf`, stopping at the first byte that cannot be read:
    /// unmapped, or not memory (RAM, VRAM or the boot ROM; device registers are never
    /// read, since reading one can change it). Returns the bytes read.
    pub fn read_memory(&self, addr: u64, buf: &mut [u8], space: Space) -> usize {
        for (i, b) in buf.iter_mut().enumerate() {
            let Some(a) = addr.checked_add(i as u64) else {
                return i;
            };
            match self.translate(a, space).and_then(|p| self.cpu.bus.debug_read(p)) {
                Some(v) => *b = v,
                None => return i,
            }
        }
        buf.len()
    }

    /// Writes `data` to memory at `addr`, stopping at the first byte that cannot be
    /// written: unmapped, or not RAM or VRAM. Returns the bytes written. A write breaks
    /// the LR/SC reservations on the words it changes, as a store would.
    pub fn write_memory(&mut self, addr: u64, data: &[u8], space: Space) -> usize {
        for (i, &b) in data.iter().enumerate() {
            let Some(a) = addr.checked_add(i as u64) else {
                return i;
            };
            match self.translate(a, space) {
                Some(p) if self.cpu.bus.debug_write(p, b) => {}
                _ => return i,
            }
        }
        data.len()
    }

    /// Runs the machine until a breakpoint or a watchpoint stops it, the program exits,
    /// or `steps` steps have run (`None`: the host gets control back, to look for a pause
    /// request, and calls again to carry on). With a real-time clock it also returns
    /// `None` once the machine is ahead of the wall clock ([`Machine::pace_delay`]).
    ///
    /// `trace` is called before each hart's turn, as [`Machine::run_for`]'s is.
    pub fn debug_continue(&mut self, steps: u64, trace: &mut impl FnMut(&Cpu)) -> Option<Stop> {
        let budget = match &mut self.pacer {
            None => steps,
            Some(p) => steps.min(p.allowance(Instant::now())),
        };
        let paced = self.pacer.is_some();
        let mut n = 0;
        let stop = loop {
            if let Some(code) = self.exit_code() {
                break Some(Stop::Exited(code));
            }
            if n >= budget {
                break None;
            }
            if paced && self.cpu.next_hart() == 0 {
                // As in `run_steps`: idle ticks pass at once, keeping one step.
                let skipped = self.cpu.idle(budget - n - 1);
                if skipped > 0 {
                    self.cpu.bus.skip(skipped);
                }
                n += skipped;
            }
            if let Some(stop) = self.breakpoint_hit() {
                break Some(stop);
            }
            n += self.debug_turn(trace) as u64;
            if let Some(hit) = self.cpu.bus.watches.hit.take() {
                break Some(Stop::Watchpoint(hit));
            }
        };
        if let Some(p) = &mut self.pacer {
            p.ticks += n;
        }
        self.debug.steps += n;
        if stop.is_some() {
            self.debug.resuming = true;
        }
        stop
    }

    /// Runs one instruction of the selected hart (or its one turn: it may take an
    /// interrupt instead, or wait in `wfi`). The harts whose turns come before its own
    /// run theirs first, as they would without a debugger, and may stop the machine at
    /// a breakpoint or a watchpoint before it.
    pub fn debug_step(&mut self, trace: &mut impl FnMut(&Cpu)) -> Stop {
        let stop = self.step_hart(trace);
        self.debug.resuming = true;
        stop
    }

    fn step_hart(&mut self, trace: &mut impl FnMut(&Cpu)) -> Stop {
        let hart = self.debug.hart;
        loop {
            if let Some(code) = self.exit_code() {
                return Stop::Exited(code);
            }
            if let Some(stop) = self.breakpoint_hit() {
                return stop;
            }
            let turn = self.cpu.next_hart();
            let ended_step = self.debug_turn(trace);
            if let Some(p) = &mut self.pacer {
                p.ticks += ended_step as u64;
            }
            self.debug.steps += ended_step as u64;
            if let Some(hit) = self.cpu.bus.watches.hit.take() {
                return Stop::Watchpoint(hit);
            }
            if turn == hart {
                return match self.exit_code() {
                    Some(code) => Stop::Exited(code),
                    None => Stop::Stepped { hart },
                };
            }
        }
    }

    /// The breakpoint the next turn's hart is about to execute, if any. Not on the first
    /// turn after a stop: that one executes the instruction the machine stopped at.
    fn breakpoint_hit(&self) -> Option<Stop> {
        if self.debug.resuming || self.debug.breakpoints.is_empty() {
            return None;
        }
        let hart = self.cpu.next_hart();
        let h = self.cpu.hart(hart);
        let pc = h.pc & self.width_mask();
        (self.debug.breakpoints.contains(&pc) && h.will_execute(&self.cpu.bus))
            .then_some(Stop::Breakpoint { hart, pc })
    }

    /// Runs the next hart's turn. Returns true if it ended a step.
    fn debug_turn(&mut self, trace: &mut impl FnMut(&Cpu)) -> bool {
        self.debug.resuming = false;
        self.cpu.turn_traced(trace)
    }

    /// Pauses the machine between two calls of [`Machine::debug_continue`] (the host
    /// handling a pause request): like a stop, the next turn steps over a breakpoint at
    /// its hart's pc, as resuming after any stop does.
    pub fn debug_pause(&mut self) {
        self.debug.resuming = true;
    }
}
