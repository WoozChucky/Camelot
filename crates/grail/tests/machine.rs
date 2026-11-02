//! Machines built from a `MachineConfig`: the sysinfo example, several machines in one
//! process with their own consoles, real-time pacing, and the `--ram` / `--clock` flags.

use grail::bus::{CLINT_BASE, RAM_BASE};
use grail::config::MIB;
use grail::{Machine, MachineConfig, RunResult};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Generous ceiling for anything that should take well under a second, so a loaded CI
/// machine does not make the timing tests flaky.
const SLOW: Duration = Duration::from_secs(5);

fn example(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../examples/{name}/{name}.elf"))
}

fn sysinfo_elf() -> Vec<u8> {
    std::fs::read(example("sysinfo")).unwrap()
}

/// A `Write` the test can read back after giving it to a machine.
#[derive(Clone, Default)]
struct Shared(Arc<Mutex<Vec<u8>>>);

impl Shared {
    fn text(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
    }
}

impl Write for Shared {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// What `examples/sysinfo` prints on a machine with this RAM and clock.
fn sysinfo_output(ram_mib: u32, hz: u32, real_time: bool) -> String {
    let kind = if real_time { "real time" } else { "nominal" };
    format!(
        "sysinfo v1
RAM      {ram_mib} MiB at 0x80000000
CPU      {hz} Hz ({kind})
timebase {hz} Hz
9 devices:
  test      0x00100000 + 0x00001000
  CLINT     0x02000000 + 0x00010000
  PLIC      0x0c000000 + 0x04000000
  UART      0x10000000 + 0x00000100  irq 1
  fb-ctrl   0x30000000 + 0x00001000
  keyboard  0x30001000 + 0x00001000  irq 2
  sysinfo   0x30002000 + 0x00001000
  pointer   0x30003000 + 0x00001000  irq 11
  fb-vram   0x31000000 + 0x0012c000
"
    )
}

#[test]
fn machine_is_send() {
    fn assert_send<T: Send + 'static>() {}
    assert_send::<Machine>();
}

#[test]
fn sysinfo_example_on_the_default_machine() {
    let mut m = Machine::with_ram(grail::DEFAULT_RAM_SIZE);
    m.cpu.bus.uart.capture = true;
    m.load_elf(&sysinfo_elf()).unwrap();
    assert_eq!(m.run(Some(1_000_000), |_| {}), RunResult::Exited(0));
    let out = String::from_utf8(std::mem::take(&mut m.cpu.bus.uart.captured)).unwrap();
    assert_eq!(out, sysinfo_output(4, 10_000_000, false));
}

#[test]
fn sysinfo_reports_a_custom_nominal_clock() {
    let config = MachineConfig {
        ram_size: MIB,
        nominal_hz: 32_768,
        ..MachineConfig::default()
    };
    let mut m = Machine::new(config).unwrap();
    m.cpu.bus.uart.capture = true;
    m.load_elf(&sysinfo_elf()).unwrap();
    assert_eq!(m.run(Some(1_000_000), |_| {}), RunResult::Exited(0));
    assert_eq!(
        String::from_utf8_lossy(&m.cpu.bus.uart.captured),
        sysinfo_output(1, 32_768, false)
    );
}

#[test]
fn two_machines_on_two_threads_keep_their_own_config_and_console() {
    let configs = [
        MachineConfig {
            ram_size: 64 * MIB,
            ..MachineConfig::default()
        },
        MachineConfig {
            ram_size: 256 * MIB,
            clock_hz: Some(2_000_000),
            ..MachineConfig::default()
        },
    ];
    let handles: Vec<_> = configs
        .into_iter()
        .map(|config| {
            let (uart, debug) = (Shared::default(), Shared::default());
            let mut m = Machine::new(config).unwrap();
            m.set_uart_output(uart.clone());
            m.set_debug_output(debug.clone());
            m.load_elf(&sysinfo_elf()).unwrap();
            // The machine moves to its own thread.
            let thread = std::thread::spawn(move || m.run(Some(10_000_000), |_| {}));
            (thread, uart, debug)
        })
        .collect();
    let results: Vec<_> = handles
        .into_iter()
        .map(|(thread, uart, debug)| (thread.join().unwrap(), uart.text(), debug.text()))
        .collect();

    assert_eq!(results[0].0, RunResult::Exited(0));
    assert_eq!(results[0].1, sysinfo_output(64, 10_000_000, false));
    assert_eq!(results[1].0, RunResult::Exited(0));
    assert_eq!(results[1].1, sysinfo_output(256, 2_000_000, true));
    assert!(results.iter().all(|r| r.2.is_empty()), "sysinfo uses no xcal.dbg");
}

#[test]
fn uart_input_and_debug_output_are_per_machine() {
    // Two echo machines in one process, each fed and read through its own channel and sinks.
    let run = |input: &[u8]| {
        let mut m = Machine::with_ram(grail::DEFAULT_RAM_SIZE);
        let (tx, rx) = channel();
        let (uart, debug) = (Shared::default(), Shared::default());
        m.set_uart_input(rx);
        m.set_uart_output(uart.clone());
        m.set_debug_output(debug.clone());
        m.load_elf(&std::fs::read(example("uart-echo")).unwrap()).unwrap();
        for &b in input {
            tx.send(b).unwrap();
        }
        (m, uart, debug)
    };
    let (mut a, a_uart, a_debug) = run(b"first\x04");
    let (mut b, b_uart, b_debug) = run(b"second one\x04");
    assert_eq!(a.run(Some(1_000_000), |_| {}), RunResult::Exited(0));
    assert_eq!(b.run(Some(1_000_000), |_| {}), RunResult::Exited(0));
    assert_eq!(a_uart.text(), "first");
    assert_eq!(b_uart.text(), "second one");
    assert_eq!(a_debug.text(), "[dbg] pc=800000ac s0 = 0x00000006 (6)\n");
    assert_eq!(b_debug.text(), "[dbg] pc=800000ac s0 = 0x0000000b (11)\n");
}

#[test]
fn default_runs_are_deterministic() {
    let run = || {
        let mut m = Machine::with_ram(grail::DEFAULT_RAM_SIZE);
        m.cpu.bus.uart.capture = true;
        m.load_elf(&sysinfo_elf()).unwrap();
        let mut steps = 0u64;
        assert_eq!(m.run(None, |_| steps += 1), RunResult::Exited(0));
        (
            steps,
            m.cpu.bus.clint.mtime,
            m.cpu.csr.minstret,
            m.cpu.bus.uart.captured,
        )
    };
    let first = run();
    assert_eq!(first, run());
    assert_eq!(first.0, first.1, "one mtime tick per step");
}

/// A machine whose RAM holds `program` at `RAM_BASE`, with pc there.
fn bare_machine(config: MachineConfig, program: &[u32]) -> Machine {
    let mut m = Machine::new(config).unwrap();
    let code: Vec<u8> = program.iter().flat_map(|w| w.to_le_bytes()).collect();
    m.cpu.bus.load(RAM_BASE, &code).unwrap();
    m.cpu.pc = RAM_BASE;
    m
}

fn clock(hz: u32) -> MachineConfig {
    MachineConfig {
        ram_size: 64 * 1024,
        clock_hz: Some(hz),
        ..MachineConfig::default()
    }
}

const LOOP: u32 = 0x0000_006f; // jal x0, 0

#[test]
fn clock_paces_instructions() {
    // 300 instructions at 1 kHz take at least 0.3 s.
    let mut m = bare_machine(clock(1000), &[LOOP]);
    let t = Instant::now();
    assert_eq!(m.run(Some(300), |_| {}), RunResult::StepLimit);
    let elapsed = t.elapsed();
    assert!(elapsed >= Duration::from_millis(300), "too fast: {elapsed:?}");
    assert!(elapsed < SLOW, "too slow: {elapsed:?}");
    assert_eq!(m.cpu.bus.clint.mtime, 300);
}

/// `lui rd, %hi(v); addi rd, rd, %lo(v)`: loads any 32-bit `v` into `rd`.
fn li(rd: u32, v: u32) -> [u32; 2] {
    let hi = v.wrapping_add(0x800) >> 12;
    let lo = v.wrapping_sub(hi << 12) & 0xfff;
    [0x37 | rd << 7 | hi << 12, 0x13 | rd << 7 | rd << 15 | lo << 20]
}

const MTIE: u32 = 1 << 7;
const MEIE: u32 = 1 << 11;

/// Sets mtimecmp to `when`, sets mie to `mie` (with mstatus.MIE still clear, so an
/// enabled interrupt wakes wfi without trapping), waits, then halts.
fn wfi_program(when: u32, mie: u32) -> Vec<u32> {
    const T0: u32 = 5;
    const T1: u32 = 6;
    let mut p = Vec::new();
    p.extend(li(T0, CLINT_BASE + 0x4000)); // t0 = &mtimecmp
    p.extend(li(T1, when));
    p.push(0x0062_a023); // sw   t1, 0(t0)
    p.push(0x0002_a223); // sw   zero, 4(t0)
    p.extend(li(T1, mie));
    p.push(0x3043_1073); // csrw mie, t1
    p.push(0x1050_0073); // wfi
    p.push(0x0000_100b); // xcal.halt zero
    p
}

/// Sets mtimecmp to `when`, enables only the machine timer interrupt, waits, then halts.
fn wfi_until(when: u32) -> Vec<u32> {
    wfi_program(when, MTIE)
}

#[test]
fn unpaced_wfi_jumps_to_the_timer() {
    let mut m = bare_machine(MachineConfig::with_ram(64 * 1024), &wfi_until(200_000));
    let mut steps = 0;
    assert_eq!(m.run(Some(1000), |_| steps += 1), RunResult::Exited(0));
    assert!(steps < 20, "{steps} steps");
    assert_eq!(m.cpu.bus.clint.mtime, 200_001);
}

#[test]
fn paced_wfi_sleeps_until_the_timer() {
    // At 1 MHz, mtimecmp = 200 000 is 0.2 s away: wfi waits that long in real time, in
    // ~1 ms batches of skipped idle ticks, and wakes exactly at mtimecmp.
    let mut m = bare_machine(clock(1_000_000), &wfi_until(200_000));
    let t = Instant::now();
    let mut traced = 0;
    assert_eq!(m.run(Some(1_000_000), |_| traced += 1), RunResult::Exited(0));
    let elapsed = t.elapsed();
    assert!(elapsed >= Duration::from_millis(200), "too fast: {elapsed:?}");
    assert!(elapsed < SLOW, "too slow: {elapsed:?}");
    assert_eq!(
        m.cpu.bus.clint.mtime, 200_000,
        "woke on the tick mtime reached mtimecmp"
    );
    assert!(
        traced < 1000,
        "idle ticks were skipped in bulk ({traced} steps traced)"
    );
}

#[test]
fn paced_wfi_skips_past_a_masked_timer() {
    // Only MEIE is enabled, so the timer cannot end the wait: whether mtimecmp is already
    // behind mtime (0) or still ahead (1000), the idle ticks must keep being skipped in
    // bulk rather than one step per tick once mtime reaches mtimecmp.
    for when in [0, 1000] {
        let mut m = bare_machine(clock(10_000_000), &wfi_program(when, MEIE));
        let mut traced = 0u64;
        assert_eq!(m.run(Some(1_000_000), |_| traced += 1), RunResult::StepLimit);
        assert!(m.cpu.waiting, "still waiting (mtimecmp {when})");
        assert_eq!(m.cpu.bus.clint.mtime, 1_000_000);
        assert!(
            traced < 10_000,
            "mtimecmp {when}: {traced} steps traced for 1 000 000 idle ticks"
        );
    }
}

#[test]
fn paced_idle_stays_within_the_step_limit() {
    // Parked in wfi with the timer far away, a paced machine skips idle ticks in bulk,
    // but run_for(k) still runs exactly k ticks (well inside one 1000-tick batch) and
    // run(Some(n)) stops at n.
    let mut m = bare_machine(clock(1_000_000), &wfi_until(400_000));
    assert_eq!(m.run_for(20, &mut |_| {}), 20);
    assert!(m.cpu.waiting, "parked in wfi");
    for k in [1, 2, 5, 10, 100] {
        let before = m.cpu.bus.clint.mtime;
        assert_eq!(m.run_for(k, &mut |_| {}), k, "run_for({k})");
        assert_eq!(m.cpu.bus.clint.mtime - before, k, "mtime after run_for({k})");
    }

    let mut m = bare_machine(clock(1_000_000), &wfi_until(400_000));
    assert_eq!(m.run(Some(50_000), |_| {}), RunResult::StepLimit);
    assert!(m.cpu.waiting);
    assert_eq!(m.cpu.bus.clint.mtime, 50_000);
}

#[test]
fn paced_wfi_wakes_on_uart_input() {
    // The echo example sleeps in wfi with no timer set; at 100 kHz it must still notice
    // input promptly while the host sleeps between batches.
    let mut m = Machine::new(MachineConfig {
        clock_hz: Some(100_000),
        ..MachineConfig::default()
    })
    .unwrap();
    m.load_elf(&std::fs::read(example("uart-echo")).unwrap()).unwrap();
    let (tx, rx) = channel();
    let uart = Shared::default();
    m.set_uart_input(rx);
    m.set_uart_output(uart.clone());
    let start = Instant::now();
    let machine = std::thread::spawn(move || {
        let r = m.run(Some(100_000_000), |_| {});
        (r, Instant::now())
    });
    std::thread::sleep(Duration::from_millis(300));
    let sent = Instant::now();
    tx.send(b'k').unwrap();
    tx.send(4).unwrap();
    let (r, done) = machine.join().unwrap();
    assert_eq!(r, RunResult::Exited(0));
    assert_eq!(uart.text(), "k");
    assert!(done - start >= Duration::from_millis(300));
    assert!(done - sent < SLOW, "took {:?} to react", done - sent);
}

#[test]
fn run_for_never_sleeps() {
    // At 5 Hz a step is 200 ms of guest time. run_for runs what is due and returns at
    // once; pace_delay says how long until the next step.
    let mut m = bare_machine(clock(5), &[LOOP]);
    assert_eq!(m.pace_delay(), None, "nothing run yet");
    let t = Instant::now();
    assert_eq!(m.run_for(1000, &mut |_| {}), 1);
    assert_eq!(m.run_for(1000, &mut |_| {}), 0, "ahead of the wall clock");
    assert!(t.elapsed() < Duration::from_millis(150));
    let delay = m.pace_delay().expect("must wait");
    assert!(delay <= Duration::from_millis(200), "{delay:?}");
    std::thread::sleep(delay);
    assert_eq!(m.pace_delay(), None);
    // At least the step that just became due; more if this thread was held up long
    // enough for the next one to be due too.
    let ran = m.run_for(1000, &mut |_| {});
    assert!(ran >= 1, "{ran}");
    assert_eq!(m.cpu.bus.clint.mtime, 1 + ran);
}

#[test]
fn run_for_runs_everything_asked_without_a_clock() {
    let mut m = bare_machine(MachineConfig::with_ram(64 * 1024), &[LOOP]);
    assert_eq!(m.run_for(123_456, &mut |_| {}), 123_456);
    assert_eq!(m.pace_delay(), None);
}

fn grail(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_grail"))
        .args(args)
        .output()
        .expect("cannot run grail")
}

#[test]
fn cli_ram_and_clock_flags_reach_sysinfo() {
    let elf = example("sysinfo");
    let elf = elf.to_str().unwrap();
    let out = grail(&["--ram", "64", "--clock", "1.5MHz", elf]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(
        String::from_utf8(out.stdout).unwrap(),
        sysinfo_output(64, 1_500_000, true)
    );

    let out = grail(&["--ram", "2048", elf]);
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8(out.stdout).unwrap(),
        sysinfo_output(2048, 10_000_000, false)
    );
}

#[test]
fn cli_clock_paces_the_program() {
    // Count the example's steps, then run it through the binary at a clock that makes
    // them last at least 0.25 s.
    let mut m = Machine::with_ram(grail::DEFAULT_RAM_SIZE);
    m.load_elf(&sysinfo_elf()).unwrap();
    let mut steps = 0u64;
    assert_eq!(m.run(None, |_| steps += 1), RunResult::Exited(0));
    let hz = steps * 4;
    let elf = example("sysinfo");
    let t = Instant::now();
    let out = grail(&["--clock", &format!("{hz}Hz"), elf.to_str().unwrap()]);
    let elapsed = t.elapsed();
    assert!(out.status.success());
    assert!(
        elapsed >= Duration::from_millis(240),
        "{steps} steps at {hz} Hz in {elapsed:?}"
    );
    assert!(elapsed < SLOW, "{elapsed:?}");
}

#[test]
fn cli_rejects_bad_ram_and_clock_values() {
    let elf = example("sysinfo");
    let elf = elf.to_str().unwrap();
    for (args, message) in [
        (
            &["--ram", "0", elf][..],
            "--ram: 0 MiB is out of range (1 to 2048 MiB)",
        ),
        (&["--ram", "2049", elf], "--ram: 2049 MiB is out of range"),
        (
            &["--ram", "lots", elf],
            "--ram: 'lots' is not a whole number of MiB",
        ),
        (
            &["--clock", "0", elf],
            "--clock: the clock must be faster than 0 Hz",
        ),
        (&["--clock", "5GHz", elf], "--clock: '5GHz' is not a frequency"),
        (
            &["--clock", "1.5Hz", elf],
            "--clock: 1.5Hz is not a whole number of Hz",
        ),
        (
            &["--clock", "5000MHz", elf],
            "--clock: 5000MHz does not fit in 32 bits",
        ),
        (&["--max-steps", "x", elf], "--max-steps: 'x' is not a number"),
        (&[elf, "--clock"], "--clock needs a value"),
        (&[elf, "--ram"], "--ram needs a value"),
    ] {
        let out = grail(args);
        let err = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(2), "{args:?}: {err}");
        assert!(err.contains(message), "{args:?}: {err}");
        assert!(out.stdout.is_empty(), "{args:?} ran the program");
    }
}

/// `--harts N` builds a machine of N harts: `examples/smp` runs on all of them, and
/// the trace names the hart of each instruction.
#[test]
fn cli_harts_flag() {
    let elf = example("smp");
    let elf = elf.to_str().unwrap();
    let out = grail(&["--harts", "4", elf]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(
        stdout.starts_with("harts: 4\namoadd: 4000 = 4 x 1000\n"),
        "{stdout}"
    );

    let out = grail(&["--harts", "2", "--trace", "--max-steps", "3", elf]);
    assert_eq!(out.status.code(), Some(124));
    let trace = String::from_utf8(out.stderr).unwrap();
    let lines: Vec<&str> = trace.lines().take(4).collect();
    assert_eq!(
        lines,
        [
            "h0 M 00001000: auipc t0, 0x0",
            "h1 M 00001020: csrrs a0, mhartid, zero",
            "h0 M 00001004: lw t0, 16(t0)",
            "h1 M 00001024: slli t1, a0, 2",
        ]
    );

    for (args, message) in [
        (
            &["--harts", "0", elf][..],
            "--harts: 0 harts is out of range (1 to 8)",
        ),
        (
            &["--harts", "9", elf],
            "--harts: 9 harts is out of range (1 to 8)",
        ),
        (
            &["--harts", "two", elf],
            "--harts: 'two' is not a whole number of harts",
        ),
        (&[elf, "--harts"], "--harts needs a value"),
    ] {
        let out = grail(args);
        let err = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(2), "{args:?}: {err}");
        assert!(err.contains(message), "{args:?}: {err}");
    }
}

#[test]
fn usage_documents_clock_and_its_units() {
    let out = grail(&["--help"]);
    assert!(out.status.success());
    let usage = String::from_utf8(out.stdout).unwrap();
    for text in [
        "--clock FREQ",
        "Hz, kHz or MHz",
        "--clock 1.5MHz",
        "--ram MiB",
        "1 to 2048",
        "--harts N",
    ] {
        assert!(usage.contains(text), "usage lacks {text:?}:\n{usage}");
    }
    // The default RAM, as the machine has it: 4 MiB.
    assert_eq!(grail::DEFAULT_RAM_SIZE, 4 << 20);
    assert_eq!(grail::MachineConfig::default().ram_size, grail::DEFAULT_RAM_SIZE);
    assert!(
        usage.contains("--ram MiB       RAM size in MiB, 1 to 2048 (default 4)\n"),
        "{usage}"
    );
}
