//! Step-loop throughput guard. Timing depends on the host, so this is not part of the
//! default test run: `cargo test --release -p grail --test throughput -- --ignored --nocapture`.

use grail::bus::RAM_BASE;
use grail::{Machine, RunResult};
use std::time::Instant;

const STEPS: u64 = 50_000_000;

/// A tight `jal x0, 0` loop with a UART input channel attached and the PLIC idle: the
/// common case of a long run that never takes an external interrupt.
#[test]
#[ignore = "timing-dependent; run explicitly in release mode"]
fn idle_plic_and_uart_do_not_slow_the_step_loop() {
    let mut best = f64::MAX;
    for _ in 0..3 {
        let mut m = Machine::with_ram(64 * 1024);
        m.cpu.bus.load(RAM_BASE, &0x0000_006fu32.to_le_bytes()).unwrap();
        m.cpu.pc = RAM_BASE;
        let (_tx, rx) = std::sync::mpsc::channel::<u8>();
        m.cpu.bus.uart.input = Some(rx);
        let t = Instant::now();
        assert_eq!(m.run(Some(STEPS), |_| {}), RunResult::StepLimit);
        best = best.min(t.elapsed().as_secs_f64());
    }
    let mips = STEPS as f64 / best / 1e6;
    println!("{STEPS} steps in {best:.3}s ({mips:.0} MIPS)");
    // origin/main ran this at ~95 MIPS on a desktop; a per-step cost that is linear in
    // the PLIC's source count, or a host channel poll, drops it well below 50.
    assert!(mips > 50.0, "step loop too slow: {mips:.0} MIPS");
}
