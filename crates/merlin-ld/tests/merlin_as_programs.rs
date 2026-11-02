//! Programs built entirely with our own tools: assembled by merlin-as, linked
//! by merlin-ld (default layout), run in Grail.
//!
//! Until merlin-ld applies relocations (#7) the programs must not need any:
//! merlin-as resolves references to local labels in the same section itself,
//! so code that stays in one section with local labels links already.

use grail::{Machine, RunResult};
use merlin_ld::{Input, Options};

const MULDIV: &str = include_str!("../../../examples/muldiv/muldiv.S");

/// Assembles, links and runs `src`; returns how it ended and what it printed.
fn build_and_run(src: &str) -> (RunResult, String) {
    let obj = merlin_as::assemble(src).unwrap_or_else(|e| panic!("merlin-as: {e:?}"));
    let relocs: usize = obj.sections.iter().map(|s| s.relocs.len()).sum();
    assert_eq!(
        relocs, 0,
        "the program needs relocations, which merlin-ld can't apply yet"
    );
    let input = Input {
        name: "prog.o".into(),
        data: merlin_as::elf::write(&obj),
    };
    let linked =
        merlin_ld::link(&[input], &Options::default()).unwrap_or_else(|e| panic!("merlin-ld: {e:?}"));
    let mut m = Machine::new(grail::DEFAULT_RAM_SIZE);
    m.cpu.bus.uart.capture = true;
    m.load_elf(&linked.elf).unwrap();
    let r = m.run(Some(100_000), |_| {});
    let out = String::from_utf8(std::mem::take(&mut m.cpu.bus.uart.captured)).unwrap();
    (r, out)
}

/// Issue #9: every M instruction, including division by zero and
/// `INT_MIN / -1`, gives the result the RISC-V spec requires.
#[test]
fn muldiv_example_passes_in_grail() {
    let (r, out) = build_and_run(MULDIV);
    assert_eq!(r, RunResult::Exited(0), "output: {out}");
    // All 17 cases ran (xcal.dbg prints the count just before halting).
    assert!(out.ends_with("a0 = 0x00000011 (17)\n"), "{out}");
}

/// The self-check really checks: a wrong expectation stops at that case.
#[test]
fn muldiv_example_reports_the_failing_case() {
    let wrong = MULDIV.replace("li    t0, -5", "li    t0, -4"); // case 8: -21 / 4
    assert_ne!(wrong, MULDIV);
    let (r, out) = build_and_run(&wrong);
    assert_eq!(r, RunResult::Exited(8), "output: {out}");

    // Division by zero must not trap: the quotient is all ones.
    let wrong = MULDIV.replacen("li    t0, 0xffffffff", "li    t0, 0", 1); // case 13
    let (r, _) = build_and_run(&wrong);
    assert_eq!(r, RunResult::Exited(13));
}
