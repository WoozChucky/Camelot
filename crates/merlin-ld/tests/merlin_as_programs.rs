//! Programs built entirely with our own tools: assembled by merlin-as, linked
//! by merlin-ld, run in Grail, for RV32 and RV64. (Every example is also
//! built and run by `crates/xtask/tests/examples.rs`.)

use excalibur::Xlen;
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
    // (No output section of multiobj takes input from both objects, so the
    // object order does not move anything it loads; concatenating the same
    // section from several objects is covered by the fixtures in
    // tests/link.rs.)
}

// ---------------------------------------------------------------------------
// RV64: merlin-as objects assembled for `rv64ima_zicsr_zifencei`, linked
// into ELF64 executables with the default layout (pc-relative code at
// 0x8000_0000) and run on an RV64 Grail machine, which an ELF64 file makes.
// The sources are here until #151 brings RV64 examples.

/// Assembles `src` for RV64 into an object named `name`.
fn rv64_object(name: &str, src: &str) -> Input {
    let opts = merlin_as::Options { xlen: Xlen::Rv64 };
    let data =
        merlin_as::assemble_to_elf_with(src, &opts).unwrap_or_else(|e| panic!("merlin-as {name}: {e:?}"));
    Input {
        name: name.into(),
        data,
    }
}

/// Links `objs` with the default layout and runs the result; returns the
/// executable, how it ended and what it printed.
fn link_and_run(objs: &[Input]) -> (Vec<u8>, RunResult, String) {
    let linked = merlin_ld::link(objs, &Options::default()).unwrap_or_else(|e| panic!("merlin-ld: {e:?}"));
    assert_eq!(linked.layout.class, elf::Class::Elf64);
    let mut m = Machine::with_ram(grail::DEFAULT_RAM_SIZE);
    m.cpu.bus.uart.capture = true;
    m.load_elf(&linked.elf).unwrap();
    assert_eq!(m.cpu.xlen(), Xlen::Rv64, "an ELF64 file makes an RV64 machine");
    let r = m.run(Some(100_000), |_| {});
    let out = String::from_utf8(std::mem::take(&mut m.cpu.bus.uart.captured)).unwrap();
    (linked.elf, r, out)
}

/// One object: a pointer to 64-bit data (`.dword`, R_RISCV_64) reached
/// pc-relatively (`la`) and loaded with `ld`, 64-bit `li`, the W
/// instructions (32-bit results, sign-extended to 64 bits), branches. Each
/// check halts with its number on a mismatch; at the end xcal.dbg shows how
/// many ran.
const RV64_ONE: &str = "
    .text
    .globl _start
_start:
    la    t0, ptr
    ld    t1, 0(t0)                 # ptr: .dword value
    ld    a0, 0(t1)
    li    t2, 0x123456789abcdef0
    li    a7, 1
    bne   a0, t2, fail
    li    a1, 0x7fffffff
    addw  a2, a1, a1                # 0xfffffffe, sign-extended
    li    t2, -2
    li    a7, 2
    bne   a2, t2, fail
    li    a1, 1
    li    a3, 31
    sllw  a2, a1, a3                # 0x80000000, sign-extended
    li    t2, 0xffffffff80000000
    li    a7, 3
    bne   a2, t2, fail
    li    a1, 0x180000000
    sext.w a2, a1                   # the low word, 0x80000000, sign-extended
    li    a7, 4
    bne   a2, t2, fail
    li    a0, 4
    xcal.dbg a0
    xcal.halt zero
fail:
    xcal.halt a7

    .data
    .balign 8
value:
    .dword 0x123456789abcdef0
ptr:
    .dword value
";

#[test]
fn rv64_program_runs_in_grail() {
    let (_, r, out) = link_and_run(&[rv64_object("one.o", RV64_ONE)]);
    assert_eq!(r, RunResult::Exited(0), "output: {out}");
    // xcal.dbg prints 64-bit registers: 16 digits.
    assert!(out.ends_with("a0 = 0x0000000000000004 (4)\n"), "{out}");
    // The checks check: addw's result is sign-extended, not 0xfffffffe.
    let wrong = RV64_ONE.replace("li    t2, -2", "li    t2, 0xfffffffe");
    assert_ne!(wrong, RV64_ONE);
    let (_, r, _) = link_and_run(&[rv64_object("one.o", &wrong)]);
    assert_eq!(r, RunResult::Exited(2));
}

/// Two objects: calls both ways, a store into and loads from the other
/// object's data (pc-relative `sd`/`ld`), and `.dword` pointers into each
/// other (R_RISCV_64).
const RV64_MAIN: &str = "
    .text
    .globl _start
_start:
    li    a0, 20
    call  twice                     # in other.o
    li    t0, 40
    li    a7, 1
    bne   a0, t0, fail
    li    t1, 0x0123456789abcdef
    sd    t1, shared, t2            # other.o's .data
    call  read_shared
    li    a7, 2
    bne   a0, t1, fail
    ld    t0, twice_ptr             # .dword twice
    li    a0, 5
    jalr  t0
    li    t0, 10
    li    a7, 3
    bne   a0, t0, fail
    ld    t0, main_ptr              # other.o's .dword main_value
    ld    a0, 0(t0)
    li    t1, 0x1111222233334444
    li    a7, 4
    bne   a0, t1, fail
    li    a0, 4
    xcal.dbg a0
    xcal.halt zero
fail:
    xcal.halt a7

    .data
    .balign 8
    .globl main_value
main_value:
    .dword 0x1111222233334444
twice_ptr:
    .dword twice
";

const RV64_OTHER: &str = "
    .text
    .globl twice, read_shared
twice:                              # a0 * 2
    slli  a0, a0, 1
    ret
read_shared:                        # a0 = shared
    ld    a0, shared
    ret

    .data
    .balign 8
    .globl shared, main_ptr
shared:
    .dword 0
main_ptr:
    .dword main_value
";

#[test]
fn rv64_objects_call_and_point_into_each_other() {
    let objs = [
        rv64_object("main.o", RV64_MAIN),
        rv64_object("other.o", RV64_OTHER),
    ];
    let (_, r, out) = link_and_run(&objs);
    assert_eq!(r, RunResult::Exited(0), "output: {out}");
    assert!(out.ends_with("a0 = 0x0000000000000004 (4)\n"), "{out}");
}

/// The merlin-ld binary links the two RV64 objects into the file
/// `merlin_ld::link` makes in memory, and says nothing.
#[test]
fn rv64_objects_with_the_merlin_ld_binary() {
    let objs = [
        rv64_object("main.o", RV64_MAIN),
        rv64_object("other.o", RV64_OTHER),
    ];
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("merlin-as-programs-rv64");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for o in &objs {
        std::fs::write(dir.join(&o.name), &o.data).unwrap();
    }
    let err = merlin_ld(&dir, &["main.o", "other.o", "-o", "two.elf"]);
    assert_eq!(err, "");
    let ours = std::fs::read(dir.join("two.elf")).unwrap();
    let (elf, r, _) = link_and_run(&objs);
    assert!(ours == elf, "the CLI's link is not merlin_ld::link's");
    assert_eq!(r, RunResult::Exited(0));
}
