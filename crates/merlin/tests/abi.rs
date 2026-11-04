//! merlin's calling convention against clang's, on RV32 (ILP32) and RV64
//! (LP64): structs of every size class, the narrow scalars, `int`, `unsigned
//! int`, `long` and pointers mixed (32-bit values sign-extended on RV64),
//! arguments on the stack and variadic arguments, passed and returned both
//! ways between merlin-compiled and clang-compiled code (see
//! `tests/abi/clang_side.c` for the cases).
//!
//! Run by `cargo test`, without LLVM: the clang objects are committed,
//! `tests/abi/clang_side.o` for RV32 and `tests/abi/clang_side.rv64.o` for
//! RV64 (with their assembly, `.s`, for reading).
//!
//! * `merlin_and_clang_call_each_other`: merlin compiles
//!   `tests/abi/merlin_side.c`; merlin-ld links it with the committed clang
//!   object and `tests/crt0.s`; Grail runs it. Exit code 0 means every
//!   check passed; otherwise the code numbers the first that failed. Both
//!   widths, as are the tests below.
//! * `merlin_calls_merlin`: the same program with both sides compiled by
//!   merlin.
//!
//! Ignored, since they need clang on `PATH`:
//!
//! * `clang_side`: compiles `clang_side.c` again and checks that the
//!   committed `.s` and `.o` files match; with `MERLIN_BLESS=1` it rewrites
//!   them instead (after changing the source):
//!
//!       MERLIN_BLESS=1 cargo test -p merlin --test abi -- --ignored clang_side
//!
//! * `clang_calls_clang`: both sides compiled by clang, which checks the
//!   program itself.

use grail::{Machine, RunResult};
use merlin::types::Xlen;
use std::path::{Path, PathBuf};
use std::process::Command;

const WIDTHS: [Xlen; 2] = [Xlen::Rv32, Xlen::Rv64];

/// Checks the exit code of the round-trip program for `x`: 0, or the
/// number of the first check that failed.
fn assert_passed(code: u8, x: Xlen) {
    let which = match code {
        0 => return,
        1..=100 => format!("check {code} of main in merlin_side.c"),
        _ => format!("check {} of c_calls_merlin in clang_side.c", code - 100),
    };
    panic!("{x:?}: {which} failed (exit code {code})");
}

/// How the clang side is compiled (and the merlin side, for
/// `clang_calls_clang`), after the target's flags ([`target_flags`]).
const CLANG_FLAGS: &[&str] = &[
    "-std=c99",
    "-O1",
    "-mno-relax",
    "-msmall-data-limit=0",
    "-ffreestanding",
    "-fno-builtin",
    "-fno-addrsig",
];

/// clang's target flags for `x`: RV64 code is linked at 0x8000_0000, so
/// `-mcmodel=medany`.
fn target_flags(x: Xlen) -> &'static [&'static str] {
    match x {
        Xlen::Rv32 => &["--target=riscv32", "-march=rv32im", "-mabi=ilp32"],
        Xlen::Rv64 => &[
            "--target=riscv64",
            "-march=rv64ima_zicsr_zifencei",
            "-mabi=lp64",
            "-mcmodel=medany",
        ],
    }
}

/// The committed clang object for `x`, and its assembly: `clang_side.o` and
/// `clang_side.s` for RV32, `clang_side.rv64.o` and `.rv64.s` for RV64.
fn clang_side_files(x: Xlen) -> (&'static str, &'static str) {
    match x {
        Xlen::Rv32 => ("clang_side.o", "clang_side.s"),
        Xlen::Rv64 => ("clang_side.rv64.o", "clang_side.rv64.s"),
    }
}

fn tests_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests")
}

fn abi_dir() -> PathBuf {
    tests_dir().join("abi")
}

/// Assembles with merlin-as for `x`.
fn assemble(asm: &str, x: Xlen) -> Vec<u8> {
    merlin_as::assemble_to_elf_with(asm, &merlin_as::Options { xlen: x })
        .unwrap_or_else(|e| panic!("merlin-as: {e:?}"))
}

/// Compiles a file of `tests/abi` with merlin for `x`, then assembles it.
fn merlin_object(file: &str, x: Xlen) -> Vec<u8> {
    let src = std::fs::read_to_string(abi_dir().join(file)).unwrap();
    let opts = merlin::Options {
        xlen: x,
        ..merlin::Options::default()
    };
    let asm = merlin::compile_with(file, &src, &opts, &merlin::no_files).unwrap_or_else(|errs| {
        let msgs: Vec<String> = errs.iter().map(|e| e.render(file)).collect();
        panic!("{}", msgs.join("\n"))
    });
    assemble(&asm, x)
}

/// Links `tests/crt0.s` and the objects with merlin-ld, runs the program in
/// Grail and returns its exit code.
fn link_and_run(objs: Vec<(&str, Vec<u8>)>, x: Xlen) -> u8 {
    let crt0 = std::fs::read_to_string(tests_dir().join("crt0.s")).unwrap();
    let mut inputs = vec![merlin_ld::Input {
        name: "crt0.o".into(),
        data: assemble(&crt0, x),
    }];
    for (name, data) in objs {
        inputs.push(merlin_ld::Input {
            name: name.into(),
            data,
        });
    }
    let linked = merlin_ld::link(&inputs, &merlin_ld::Options::default())
        .unwrap_or_else(|e| panic!("merlin-ld: {e:?}"));
    let mut m = Machine::with_ram(grail::DEFAULT_RAM_SIZE);
    m.load_elf(&linked.elf).unwrap();
    match m.run(Some(10_000_000), |_| {}) {
        RunResult::Exited(code) => code as u8,
        RunResult::StepLimit => panic!("still running at pc={:#010x}", m.cpu.pc),
    }
}

#[test]
fn merlin_and_clang_call_each_other() {
    for x in WIDTHS {
        let clang_side = std::fs::read(abi_dir().join(clang_side_files(x).0)).unwrap();
        let code = link_and_run(
            vec![
                ("merlin_side.o", merlin_object("merlin_side.c", x)),
                ("clang_side.o", clang_side),
            ],
            x,
        );
        assert_passed(code, x);
    }
}

#[test]
fn merlin_calls_merlin() {
    for x in WIDTHS {
        let code = link_and_run(
            vec![
                ("merlin_side.o", merlin_object("merlin_side.c", x)),
                ("clang_side.o", merlin_object("clang_side.c", x)),
            ],
            x,
        );
        assert_passed(code, x);
    }
}

/// Runs clang for `x` in `tests/abi` on `file` (a relative name, so no
/// absolute path ends up in the output) with the extra arguments.
fn clang(file: &str, x: Xlen, extra: &[&str]) {
    let out = Command::new("clang")
        .current_dir(abi_dir())
        .args(target_flags(x))
        .args(CLANG_FLAGS)
        .args(extra)
        .arg(file)
        .output()
        .expect("running clang");
    assert!(
        out.status.success(),
        "clang failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A scratch directory, removed when dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Scratch {
        let dir = std::env::temp_dir().join(format!("merlin-abi-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        Scratch(dir)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
#[ignore = "needs clang on PATH"]
fn clang_side() {
    for x in WIDTHS {
        let (obj, asm_name) = clang_side_files(x);
        let scratch = Scratch::new("regen");
        let (s, o) = (scratch.0.join(asm_name), scratch.0.join(obj));
        clang("clang_side.c", x, &["-S", "-o", s.to_str().unwrap()]);
        clang("clang_side.c", x, &["-c", "-o", o.to_str().unwrap()]);
        let asm = std::fs::read_to_string(&s).unwrap();
        if std::env::var_os("MERLIN_BLESS").is_some() {
            std::fs::write(abi_dir().join(asm_name), &asm).unwrap();
            std::fs::copy(&o, abi_dir().join(obj)).unwrap();
            continue;
        }
        let committed = std::fs::read_to_string(abi_dir().join(asm_name)).unwrap();
        assert_eq!(
            committed.replace("\r\n", "\n"),
            asm.replace("\r\n", "\n"),
            "tests/abi/{asm_name} is out of date (MERLIN_BLESS=1 to regenerate)"
        );
        // The object is what the round trip links, so it must match too.
        assert!(
            std::fs::read(&o).unwrap() == std::fs::read(abi_dir().join(obj)).unwrap(),
            "tests/abi/{obj} is out of date (MERLIN_BLESS=1 to regenerate)"
        );
    }
}

#[test]
#[ignore = "needs clang on PATH"]
fn clang_calls_clang() {
    for x in WIDTHS {
        let scratch = Scratch::new("clang");
        let mut objs = Vec::new();
        for (c, o) in [
            ("clang_side.c", "clang_side.o"),
            ("merlin_side.c", "merlin_side.o"),
        ] {
            let path = scratch.0.join(o);
            clang(c, x, &["-c", "-o", path.to_str().unwrap()]);
            objs.push((o, std::fs::read(&path).unwrap()));
        }
        assert_passed(link_and_run(objs, x), x);
    }
}
