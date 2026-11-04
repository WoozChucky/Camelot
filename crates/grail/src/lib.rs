//! Grail: an emulator for the Excalibur ISA (RV64IMA + Zicsr, M/S/U modes, Sv39 and
//! Sv48; RV32 with Sv32 while both widths exist).
//!
//! A [`Machine`] is built from a [`MachineConfig`] and owns all of its state, I/O
//! included (UART and debug output go to sinks the host hands it), so a process can run
//! several machines, each on its own thread.

pub mod block;
pub mod boot;
pub mod bus;
pub mod cli;
pub mod config;
pub mod console;
pub mod cpu;
pub mod debug;
pub mod devices;
pub mod display;
pub mod elf;
pub mod framebuffer;
pub mod gdb;
pub mod keyboard;
pub mod lake;
pub mod mmio;
pub mod net;
pub mod pointer;
pub mod ram;
pub mod sysinfo;
pub mod trap;
/// Hand-written Win32 declarations, shared with the other Windows programs of the
/// workspace (Herald's screen window and its tests) rather than written twice.
#[cfg(windows)]
pub mod win32;
/// The framebuffer window, also Herald's window onto a remote VM's screen.
#[cfg(windows)]
pub mod window;

use block::{BlockBackend, BlockDevice};
use bus::{Bus, Exit, MAX_DISKS, MAX_NETS};
pub use config::{
    max_ram_size, parse_clock, parse_harts, parse_paging, parse_ram_size, parse_size, parse_xlen, DiskConfig,
    MachineConfig, NetConfig, Paging, DEFAULT_RAM_SIZE, MAX_HARTS,
};
use cpu::Cpu;
pub use excalibur::Xlen;
use keyboard::KeyEvent;
use net::{MacAddr, NetBackend, NetDevice};
use pointer::PointerInput;
use std::io::Write;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

/// Steps [`Machine::run`] runs between checks of the step limit and the pacing clock.
const RUN_SLICE: u64 = 100_000;

pub struct Machine {
    pub cpu: Cpu,
    config: MachineConfig,
    /// Real-time pacing, when the config has a clock.
    pacer: Option<Pacer>,
    /// Breakpoints and the hart a debugger looks at ([`debug`]).
    debug: debug::Debug,
}

#[derive(Debug, PartialEq, Eq)]
pub enum RunResult {
    Exited(u32),
    StepLimit,
}

impl Machine {
    /// Builds a machine with the hardware `config` describes, opening its disks and
    /// network backends.
    pub fn new(config: MachineConfig) -> Result<Self, String> {
        Machine::with_backends(config, Vec::new())
    }

    /// Like [`Machine::new`], with more disks whose backends the host made itself (a
    /// network disk, say). They come after the disks `config.disks` describes: with two
    /// there, `backends[0]` is disk 2. Up to [`MAX_DISKS`] in all.
    pub fn with_backends(
        config: MachineConfig,
        backends: Vec<Box<dyn BlockBackend>>,
    ) -> Result<Self, String> {
        Machine::with_devices(config, backends, Vec::new())
    }

    /// Like [`Machine::with_backends`], with network cards whose backends the host made
    /// itself too (a switch port, a gateway), each with its MAC address. They come after
    /// the cards `config.nets` describes, up to [`MAX_NETS`] in all.
    pub fn with_devices(
        config: MachineConfig,
        backends: Vec<Box<dyn BlockBackend>>,
        net_backends: Vec<(MacAddr, Box<dyn NetBackend>)>,
    ) -> Result<Self, String> {
        config.validate()?;
        if config.nets.len() + net_backends.len() > MAX_NETS {
            return Err(format!(
                "{} network cards, at most {MAX_NETS}",
                config.nets.len() + net_backends.len()
            ));
        }
        for (mac, _) in &net_backends {
            mac.check_card()?;
        }
        if config.disks.len() + backends.len() > MAX_DISKS {
            return Err(format!(
                "{} disks, at most {MAX_DISKS}",
                config.disks.len() + backends.len()
            ));
        }
        let mut disks = Vec::new();
        for backend in config
            .disks
            .iter()
            .map(DiskConfig::open)
            .chain(backends.into_iter().map(Ok))
        {
            let i = disks.len();
            disks.push(
                backend
                    .and_then(BlockDevice::new)
                    .map_err(|e| format!("disk {i}: {e}"))?,
            );
        }
        let mut nets = Vec::new();
        for card in config
            .nets
            .iter()
            .map(NetConfig::open)
            .chain(net_backends.into_iter().map(Ok))
        {
            let i = nets.len();
            let (mac, backend) = card.map_err(|e| format!("network card {i}: {e}"))?;
            nets.push(NetDevice::new(mac, backend));
        }
        let mut cpu = Cpu::new(Bus::with_nets(&config, disks, nets)?);
        cpu.set_paging(config.paging);
        let pacer = config.clock_hz.map(Pacer::new);
        // With a real-time clock a wfi takes real time; see `run_steps`.
        cpu.wfi_skips_to_timer = pacer.is_none();
        Ok(Machine {
            cpu,
            config,
            pacer,
            debug: debug::Debug::default(),
        })
    }

    /// The default machine with `ram_size` bytes of RAM. Panics if that is not a valid
    /// size (see [`MachineConfig::validate`]).
    pub fn with_ram(ram_size: usize) -> Self {
        Machine::new(MachineConfig::with_ram(ram_size)).unwrap_or_else(|e| panic!("{e}"))
    }

    pub fn config(&self) -> &MachineConfig {
        &self.config
    }

    /// Feeds the UART's receive side from `input`.
    pub fn set_uart_input(&mut self, input: Receiver<u8>) {
        self.cpu.bus.uart.input = Some(input);
    }

    /// Sends what the program writes to the UART to `output` (by default it is dropped).
    pub fn set_uart_output(&mut self, output: impl Write + Send + 'static) {
        self.cpu.bus.uart.output = Some(Box::new(output));
    }

    /// Sends `xcal.dbg` output to `output` (by default it is dropped).
    pub fn set_debug_output(&mut self, output: impl Write + Send + 'static) {
        self.cpu.bus.debug = Some(Box::new(output));
    }

    /// Loads an ELF image into RAM and points the boot ROM at its entry. Each `PT_LOAD`
    /// segment goes to its physical address (`p_paddr`), and the program starts at the
    /// physical address of its entry point ([`elf::Elf::physical_entry`]), since paging
    /// is off at reset. A segment that does not fit in RAM is an error, found before
    /// anything is allocated for it, so an image from an untrusted source (a remote
    /// client) cannot make the host allocate more than its size.
    ///
    /// An ELF64 program makes an RV32 machine RV64 (it must not have run yet); an ELF32
    /// program on an RV64 machine is an error.
    pub fn load_elf(&mut self, file: &[u8]) -> Result<(), String> {
        let elf = elf::parse(file).map_err(|e| e.to_string())?;
        if elf.xlen == Xlen::Rv32 && self.config.xlen == Xlen::Rv64 {
            return Err("an RV32 program (ELF32) cannot run on an RV64 machine".into());
        }
        let entry = elf.physical_entry().map_err(|e| e.to_string())?;
        let bus = &mut self.cpu.bus;
        for seg in &elf.segments {
            bus.load_zeroed(seg.paddr, &seg.data, seg.mem_size)
                .map_err(|_| format!("segment at {:#x} is outside RAM", seg.paddr))?;
        }
        if elf.xlen != self.config.xlen {
            self.config.xlen = elf.xlen;
            self.cpu.set_xlen(elf.xlen);
        }
        let bus = &mut self.cpu.bus;
        bus.tohost = elf.symbol("tohost");
        bus.set_boot_entry(entry);
        Ok(())
    }

    /// Boots from disk 0 instead of a program: the boot ROM's disk loader reads the
    /// disk's boot block into RAM and jumps to it (spec section 7, "Boot from disk";
    /// [`boot`]). The checks happen when the machine runs: a disk with no valid boot
    /// block stops it with [`boot::EXIT_BOOT_FAILED`] and a message on the UART. An
    /// error here only if the machine has no disk, or has less RAM than the boot area
    /// ([`boot::BOOT_AREA_SIZE`], 64 KiB) the boot block is loaded into. The ROM's
    /// loader is the one of the machine's width (RV32 or RV64), and so must be the boot
    /// block's code.
    pub fn boot_from_disk(&mut self) -> Result<(), String> {
        if self.cpu.bus.disks.is_empty() {
            return Err("no disk to boot from".into());
        }
        if boot::boot_block_addr(self.cpu.bus.ram.len()).is_none() {
            return Err(format!(
                "booting from disk takes at least {} KiB of RAM",
                boot::BOOT_AREA_SIZE >> 10
            ));
        }
        self.cpu.bus.tohost = None;
        self.cpu.bus.set_disk_boot();
        Ok(())
    }

    /// Hands a key event to the keyboard device (spec section 9.4). This is the one way
    /// in for every host-side producer: the framebuffer window, tests, or a remote
    /// client. Returns false if the event was dropped: its code is not in the spec's key
    /// code table (see [`keyboard::code::is_valid`]), or the keyboard FIFO is full.
    pub fn key_event(&mut self, ev: KeyEvent) -> bool {
        self.cpu.bus.kbd.push(ev)
    }

    /// Hands a change of the host's pointer (mouse) to the pointer device (spec section
    /// 9.7): where it is now, in framebuffer pixels, or how far it moved; the buttons
    /// held; the wheel notches turned. Like [`Machine::key_event`], this is the one way in
    /// for every producer: the framebuffer window, tests, or a remote viewer. Returns
    /// true if it queued an event; false if the change is invisible to the program in
    /// the mode it chose (see [`pointer::Pointer::push`]) or the FIFO is full.
    pub fn pointer_event(&mut self, input: PointerInput) -> bool {
        self.cpu.bus.ptr.push(input)
    }

    /// Exit code, once the program has asked to stop.
    pub fn exit_code(&self) -> Option<u32> {
        self.cpu.bus.exit.map(|Exit::Code(c)| c)
    }

    /// Runs at most `steps` steps and returns how many ran, without ever sleeping: the
    /// building block for a host that schedules machines itself. A step runs one
    /// instruction on every hart ([`Cpu::step`]). Stops early when the program exits, or
    /// at the end of a step that wrote `FB_PRESENT` (right after the instruction that
    /// did it, on a one-hart machine), so the host can show or dump that frame. Only a
    /// write during this call counts: a present the host has not taken yet
    /// ([`framebuffer::Framebuffer::take_present`]) does not stop the next call.
    ///
    /// With a real-time clock it also stops once the machine is ~1 ms ahead of the wall
    /// clock (it may then run 0 steps); [`Machine::pace_delay`] says how long to wait
    /// before calling again.
    pub fn run_for(&mut self, steps: u64, trace: &mut impl FnMut(&Cpu)) -> u64 {
        self.run_steps::<true>(steps, trace)
    }

    /// With a real-time clock: how long the host should wait before the machine has more
    /// steps to run. `None` when it can run now, and always without a clock.
    pub fn pace_delay(&self) -> Option<Duration> {
        self.pacer.as_ref()?.delay(Instant::now())
    }

    /// Runs until the program exits or `max_steps` steps have run. `FB_PRESENT` does not
    /// stop it. With a real-time clock, sleeps whenever the machine gets ahead of the
    /// wall clock.
    pub fn run(&mut self, max_steps: Option<u64>, mut trace: impl FnMut(&Cpu)) -> RunResult {
        let mut steps = 0u64;
        loop {
            if let Some(Exit::Code(c)) = self.cpu.bus.exit {
                return RunResult::Exited(c);
            }
            let budget = match max_steps {
                Some(max) if steps >= max => return RunResult::StepLimit,
                Some(max) => (max - steps).min(RUN_SLICE),
                None => RUN_SLICE,
            };
            steps += self.run_steps::<false>(budget, &mut trace);
            if let Some(d) = self.pace_delay() {
                std::thread::sleep(d);
            }
        }
    }

    /// Runs at most `steps` steps (fewer when paced). Stops at exit, and right after a
    /// `FB_PRESENT` write when `STOP_ON_PRESENT`.
    fn run_steps<const STOP_ON_PRESENT: bool>(&mut self, steps: u64, trace: &mut impl FnMut(&Cpu)) -> u64 {
        let budget = match &mut self.pacer {
            None => steps,
            Some(p) => steps.min(p.allowance(Instant::now())),
        };
        let paced = self.pacer.is_some();
        let presents = self.cpu.bus.fb.presents;
        let mut n = 0;
        while n < budget && self.cpu.bus.exit.is_none() {
            if paced {
                // Every hart parked in wfi: let the idle ticks of this batch pass at once,
                // keeping one step for the wake-up check. The pacer then sleeps them off
                // in real time.
                let skipped = self.cpu.idle(budget - n - 1);
                if skipped > 0 {
                    // Network cards poll by the step: the skipped ones count too, or a
                    // card would poll only every few hundred batches while the guest idles.
                    self.cpu.bus.skip(skipped);
                }
                n += skipped;
            }
            self.cpu.step_traced(trace);
            n += 1;
            if STOP_ON_PRESENT && self.cpu.bus.fb.presents != presents {
                break;
            }
        }
        if let Some(p) = &mut self.pacer {
            p.ticks += n;
        }
        n
    }
}

/// Keeps a machine's steps in line with the wall clock: tick `n` may not run before
/// `start + n / hz`, give or take one batch.
///
/// The machine runs in batches of ~1 ms of guest time and the host sleeps between them,
/// rather than sleeping per instruction. Without a clock, `mtime` jumps over a `wfi`;
/// here the idle ticks are skipped in bulk too (`Cpu::idle`), but still counted, so the
/// wait lasts the real time until `mtimecmp`.
struct Pacer {
    hz: u32,
    /// Wall-clock time of tick 0, set by the first run (so loading a program or a host
    /// that is slow to start does not count as lag).
    start: Option<Instant>,
    /// Ticks run since `start`.
    ticks: u64,
}

impl Pacer {
    fn new(hz: u32) -> Self {
        Pacer {
            hz,
            start: None,
            ticks: 0,
        }
    }

    /// How far (in ticks, ~1 ms) the machine may run ahead of the wall clock: the batch size.
    fn lead(&self) -> u64 {
        (self.hz / 1000).max(1) as u64
    }

    /// Lag beyond this (~100 ms) is dropped instead of caught up with at full speed.
    fn max_lag(&self) -> u64 {
        (self.hz / 10).max(1) as u64
    }

    /// Ticks that should have run between `start` and `now`.
    fn due(&self, start: Instant, now: Instant) -> u64 {
        let nanos = now.saturating_duration_since(start).as_nanos();
        (nanos * self.hz as u128 / 1_000_000_000) as u64
    }

    /// Ticks that may run at `now`.
    fn allowance(&mut self, now: Instant) -> u64 {
        let start = *self.start.get_or_insert(now);
        let due = self.due(start, now);
        if due > self.ticks + self.max_lag() {
            // Far behind: the host is slower than the clock, or the machine was not run
            // for a while. Start counting again from now rather than racing to catch up.
            self.start = Some(now);
            self.ticks = 0;
            return self.lead();
        }
        (due + self.lead()).saturating_sub(self.ticks)
    }

    /// Time from `now` until every tick run so far is due, if that is in the future.
    fn delay(&self, now: Instant) -> Option<Duration> {
        let start = self.start?;
        // Rounded up, so that once it has passed `due` has caught up with `ticks`.
        let nanos = (self.ticks as u128 * 1_000_000_000).div_ceil(self.hz as u128);
        let target = start + Duration::from_nanos(nanos as u64);
        target.checked_duration_since(now).filter(|d| !d.is_zero())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn machine_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<Machine>();
    }

    #[test]
    fn new_rejects_invalid_configs() {
        assert!(Machine::new(MachineConfig::with_ram(0)).is_err());
        assert!(Machine::new(MachineConfig::with_ram(1000)).is_err());
        let zero = MachineConfig {
            clock_hz: Some(0),
            ..MachineConfig::default()
        };
        assert!(Machine::new(zero).is_err());
        let m = Machine::new(MachineConfig::with_ram(64 * 1024)).unwrap();
        assert_eq!(m.cpu.bus.ram.len(), 64 * 1024);
        assert_eq!(m.config().ram_size, 64 * 1024);
        assert!(m.cpu.wfi_skips_to_timer);
        assert_eq!(m.pace_delay(), None);
    }

    #[test]
    #[should_panic(expected = "multiple of 4 KiB")]
    fn with_ram_panics_on_a_bad_size() {
        Machine::with_ram(12345);
    }

    #[test]
    fn pacer_batches_and_delays() {
        let mut p = Pacer::new(1000); // lead 1 tick, max lag 100 ticks
        let t0 = Instant::now();
        assert_eq!(p.delay(t0), None, "not started");
        assert_eq!(p.allowance(t0), 1);
        p.ticks += 1;
        assert_eq!(p.delay(t0), Some(Duration::from_millis(1)));
        assert_eq!(p.allowance(t0), 0);
        let t = t0 + Duration::from_micros(1500);
        assert_eq!(p.allowance(t), 1, "tick 1 is due, so one more may run");
        assert_eq!(p.delay(t), None);
        p.ticks += 1;
        assert_eq!(p.delay(t), Some(Duration::from_micros(500)));

        // A 10 MHz clock runs in 10 000-tick (1 ms) batches.
        let mut fast = Pacer::new(10_000_000);
        assert_eq!(fast.allowance(t0), 10_000);
        fast.ticks = 10_000;
        assert_eq!(fast.allowance(t0 + Duration::from_micros(500)), 5_000);
        assert_eq!(
            fast.delay(t0 + Duration::from_micros(500)),
            Some(Duration::from_micros(500))
        );
    }

    #[test]
    fn pacer_drops_long_lag() {
        let mut p = Pacer::new(1000);
        let t0 = Instant::now();
        p.allowance(t0);
        // 50 ms behind: caught up with.
        assert_eq!(p.allowance(t0 + Duration::from_millis(50)), 51);
        // 10 s behind: start over from now with one batch.
        let late = t0 + Duration::from_secs(10);
        assert_eq!(p.allowance(late), 1);
        assert_eq!(p.start, Some(late));
        assert_eq!(p.ticks, 0);
    }

    #[test]
    fn delay_is_rounded_up() {
        // 3 ticks at 7 Hz = 428 571 428.57 ns: waiting the delay always makes them due.
        let mut p = Pacer::new(7);
        let t0 = Instant::now();
        p.allowance(t0);
        p.ticks = 3;
        let d = p.delay(t0).unwrap();
        assert_eq!(d, Duration::from_nanos(428_571_429));
        assert!(p.due(t0, t0 + d) >= 3);
        assert_eq!(p.allowance(t0 + d), 1);
    }
}
