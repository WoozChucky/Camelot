//! Programs built entirely with our own tools: assembled by merlin-as, linked
//! by merlin-ld, run in Grail. (Every example is also built and run by
//! `crates/xtask/tests/examples.rs`.)

use grail::{Machine, RunResult};
use merlin_ld::{Input, Options};
use std::path::Path;
use std::process::Command;

const MULDIV: &str = include_str!("../../../examples/muldiv/muldiv.S");

/// Assembles, links and runs `src`; returns how it ended and what it printed.
fn build_and_run(src: &str) -> (RunResult, String) {
    let obj = merlin_as::assemble(src).unwrap_or_else(|e| panic!("merlin-as: {e:?}"));
    let input = Input {
        name: "prog.o".into(),
        data: merlin_as::elf::write(&obj),
    };
    let linked =
        merlin_ld::link(&[input], &Options::default()).unwrap_or_else(|e| panic!("merlin-ld: {e:?}"));
    let mut m = Machine::with_ram(grail::DEFAULT_RAM_SIZE);
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

/// Runs the merlin-ld binary in `dir`; returns its stderr.
fn merlin_ld(dir: &Path, args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_merlin-ld"))
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    let err = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(out.status.success(), "merlin-ld {args:?}: {err}");
    err
}

/// The commands at the top of `examples/multiobj/main.S`, with the merlin-ld
/// binary: two objects and a script in, the committed executable out (which
/// `cargo xtask examples` builds by linking in memory).
#[test]
fn multiobj_example_with_the_merlin_ld_binary() {
    let example = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/multiobj");
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("merlin-as-programs-multiobj");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for src in ["main", "print"] {
        let text = std::fs::read_to_string(example.join(format!("{src}.S"))).unwrap();
        let obj = merlin_as::assemble_to_elf(&text).unwrap_or_else(|e| panic!("{src}.S: {e:?}"));
        std::fs::write(dir.join(format!("{src}.o")), obj).unwrap();
    }
    std::fs::copy(example.join("link.ld"), dir.join("link.ld")).unwrap();

    let err = merlin_ld(
        &dir,
        &["-T", "link.ld", "main.o", "print.o", "-o", "multiobj.elf"],
    );
    assert_eq!(err, "");
    let ours = std::fs::read(dir.join("multiobj.elf")).unwrap();
    let committed = std::fs::read(example.join("multiobj.elf")).unwrap();
    assert!(
        ours == committed,
        "examples/multiobj/multiobj.elf is not the CLI's link"
    );

    // In the other order print.S's code comes first in .text, but _start is
    // in .text.start, which the script places before it: still a program.
    merlin_ld(&dir, &["-T", "link.ld", "print.o", "main.o", "-o", "swapped.elf"]);
    let swapped = std::fs::read(dir.join("swapped.elf")).unwrap();
    assert_ne!(swapped, committed);
    let mut m = Machine::with_ram(grail::DEFAULT_RAM_SIZE);
    m.cpu.bus.uart.capture = true;
    m.load_elf(&swapped).unwrap();
    assert_eq!(m.run(Some(100_000), |_| {}), RunResult::Exited(42));
    let out = String::from_utf8(std::mem::take(&mut m.cpu.bus.uart.captured)).unwrap();
    assert!(
        out.starts_with("Hello from two objects!\nputs lives in print.S.\n"),
        "{out}"
    );
}
