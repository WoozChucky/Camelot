//! Grail: an emulator for the Excalibur ISA (RV32IMA + Zicsr, M/S/U modes, Sv32).
//!
//! A [`Machine`] is built from a [`MachineConfig`] and owns all of its state, I/O
//! included (UART and debug output go to sinks the host hands it), so a process can run
//! several machines, each on its own thread.

pub mod block;
pub mod bus;
pub mod config;
pub mod console;
pub mod cpu;
pub mod devices;
pub mod display;
pub mod elf;
pub mod framebuffer;
pub mod keyboard;
pub mod sysinfo;
pub mod trap;
#[cfg(windows)]
mod win32;
#[cfg(windows)]
mod window;

use block::{BlockBackend, BlockDevice};
use bus::{Bus, Exit, MAX_DISKS};
pub use config::{parse_clock, parse_ram_mib, DiskConfig, MachineConfig, DEFAULT_RAM_SIZE};
use cpu::Cpu;
use keyboard::KeyEvent;
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
}

#[derive(Debug, PartialEq, Eq)]
pub enum RunResult {
    Exited(u32),
    StepLimit,
}

impl Machine {
    /// Builds a machine with the hardware `config` describes, opening its disks.
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
        config.validate()?;
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
        let mut cpu = Cpu::new(Bus::new(&config, disks));
        let pacer = config.clock_hz.map(Pacer::new);
        // With a real-time clock a wfi takes real time; see `run_steps`.
        cpu.wfi_skips_to_timer = pacer.is_none();
        Ok(Machine { cpu, config, pacer })
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

    /// Hands a key event to the keyboard device (spec section 9.4). This is the one way
    /// in for every host-side producer: the framebuffer window, tests, or a remote
    /// client. Returns false if the event was dropped: its code is not in the spec's key
    /// code table (see [`keyboard::code::is_valid`]), or the keyboard FIFO is full.
    pub fn key_event(&mut self, ev: KeyEvent) -> bool {
        self.cpu.bus.kbd.push(ev)
    }

    /// Exit code, once the program has asked to stop.
    pub fn exit_code(&self) -> Option<u32> {
        self.cpu.bus.exit.map(|Exit::Code(c)| c)
    }

    /// Runs at most `steps` steps and returns how many ran, without ever sleeping: the
    /// building block for a host that schedules machines itself. Stops early, right after
    /// the instruction that did it, when the program exits or writes `FB_PRESENT`, so the
    /// host can show or dump that exact frame.
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
        let mut n = 0;
        while n < budget && self.cpu.bus.exit.is_none() {
            if paced {
                // Parked in wfi: let the idle ticks of this batch pass at once, keeping one
                // step for the wake-up check. The pacer then sleeps them off in real time.
                n += self.cpu.idle(budget - n - 1);
            }
            trace(&self.cpu);
            self.cpu.step();
            n += 1;
            if STOP_ON_PRESENT && self.cpu.bus.fb.present_pending() {
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
