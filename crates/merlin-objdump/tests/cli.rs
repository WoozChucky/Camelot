//! The merlin-objdump binary: arguments, exit codes, error messages, and
//! robustness against broken input.

use merlin_objdump::{dump, Options};
use std::path::{Path, PathBuf};
use std::process::Command;

fn root() -> &'static Path {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
}

fn objdump(args: &[&str]) -> (i32, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_merlin-objdump"))
        .args(args)
        .current_dir(root())
        .output()
        .unwrap();
    (
        out.status.code().unwrap(),
        String::from_utf8(out.stdout).unwrap(),
        String::from_utf8(out.stderr).unwrap(),
    )
}

#[test]
fn disassembles_hello() {
    let (code, out, err) = objdump(&["-d", "examples/hello/hello.elf"]);
    assert_eq!((code, err.as_str()), (0, ""));
    assert!(out.starts_with("\nexamples/hello/hello.elf:\tfile format elf32-littleriscv\n"));
    assert!(out.contains("\n80000000 <_start>:\n"));
    // Built by merlin-ld, which keeps `call` as auipc + jalr (no relaxation).
    assert!(out.contains("80000010: 00000097     \tauipc\tra, 0x0\n"));
    assert!(out.contains("80000014: 010080e7     \tjalr\t0x10(ra) <puts>\n"));
    assert!(out.contains("\tret\n"));
}

#[test]
fn several_files_and_options() {
    let (code, out, _) = objdump(&[
        "-h",
        "-r",
        "examples/hello/hello.elf",
        "crates/merlin-objdump/tests/fixtures/hello.o",
    ]);
    assert_eq!(code, 0);
    assert_eq!(out.matches("Sections:").count(), 2);
    assert!(out.contains("RELOCATION RECORDS FOR [.text.start]:"));
    assert!(out.contains("R_RISCV_CALL_PLT         puts"));
}

#[test]
fn errors_and_exit_codes() {
    let (code, out, _) = objdump(&["--help"]);
    assert_eq!(code, 0);
    assert!(out.contains("usage: merlin-objdump"));

    let (code, _, err) = objdump(&["-q", "x.o"]);
    assert_eq!(code, 2);
    assert!(err.contains("unknown option -q"));

    let (code, _, err) = objdump(&["examples/hello/hello.elf"]);
    assert_eq!(code, 2);
    assert!(err.contains("no command specified"));

    // A bad file is reported, the good one after it still printed, exit code 1.
    let (code, out, err) = objdump(&[
        "-f",
        "no-such-file.o",
        "examples/hello/hello.S",
        "examples/hello/hello.elf",
    ]);
    assert_eq!(code, 1);
    assert!(err.contains("error: 'no-such-file.o': "), "{err}");
    assert!(
        err.contains("error: 'examples/hello/hello.S': bad ELF: missing magic"),
        "{err}"
    );
    assert!(out.contains("start address: 0x80000000"));
}

#[test]
fn both_widths() {
    // An ELF64 file is RV64 code, with 16-digit addresses; an ELF32 one
    // next to it keeps its 8 digits.
    let (code, out, err) = objdump(&[
        "-d",
        "-f",
        "crates/elf/tests/llvm/rv64/prog.elf",
        "crates/elf/tests/llvm/rv32/prog.elf",
    ]);
    assert_eq!((code, err.as_str()), (0, ""));
    assert!(
        out.contains("rv64/prog.elf:\tfile format elf64-littleriscv\narchitecture: riscv64\n"),
        "{out}"
    );
    assert!(out.contains("start address: 0x00000000800000b8\n"), "{out}");
    assert!(
        out.contains("rv32/prog.elf:\tfile format elf32-littleriscv\narchitecture: riscv32\n"),
        "{out}"
    );
}

/// A mapping symbol whose ISA LLVM cannot parse gets llvm-objdump's warning
/// on stderr, once per name; the listing is the same. The riscv-tests ELFs,
/// built by GNU's assembler, have `$x.0`.
#[test]
fn unparsable_isa_mapping_symbols_warn() {
    let (code, out, err) = objdump(&["-d", "tests/riscv-tests/rv32ui-p-add", "-t"]);
    assert_eq!(code, 0);
    assert_eq!(
        err,
        "merlin-objdump: warning: 'tests/riscv-tests/rv32ui-p-add': could not parse ISA mapping \
         symbol '$x.0': string may only contain [a-z0-9_]; falling back to default disassembler\n"
    );
    assert!(out.contains("<test_2>:"), "{out}");
    // Only the disassembly reads them.
    let (code, _, err) = objdump(&["-t", "tests/riscv-tests/rv32ui-p-add"]);
    assert_eq!((code, err.as_str()), (0, ""));
}

/// `-l` reads `.debug_aranges` as LLVM does: in an object built with
/// relaxation a length there is a relocation pair over a 0, which LLVM reads
/// unrelocated, as the list's end, and warns of (its message, with our
/// prefix). Without `-l` nothing reads it.
#[test]
fn a_premature_end_of_debug_aranges_warns() {
    let file = "crates/merlin-objdump/tests/fixtures/rv64/units-aranges.o";
    let (code, out, err) = objdump(&["-d", "-l", file]);
    assert_eq!(code, 0);
    let want = "address range table at offset 0x0 has a premature terminator entry at offset 0x20";
    assert_eq!(err, format!("merlin-objdump: warning: '{file}': {want}\n"));
    assert!(out.contains("; g():\n"), "{out}");
    let (code, _, err) = objdump(&["-d", file]);
    assert_eq!((code, err.as_str()), (0, ""));
}

fn fixtures() -> Vec<PathBuf> {
    let mut files = vec![root().join("examples/hello/hello.elf")];
    for f in [
        "hello.elf",
        "hello.o",
        "isa.o",
        "relocs.o",
        "rv64/hello.elf",
        "rv64/isa.o",
        "rv64/relocs.o",
        "rv64/linked.elf",
        "debug.o",
        "rv64/units.o",
        "rv64/units-dwarf4.o",
        "rv64/units-aranges.o",
    ] {
        files.push(root().join("crates/merlin-objdump/tests/fixtures").join(f));
    }
    files
}

/// Truncated and corrupted files give errors or output, never a panic.
#[test]
fn broken_files_do_not_panic() {
    let mut opts = Options {
        disassemble: true,
        contents: true,
        line_numbers: true,
        ..Options::default()
    };
    opts.set_all_headers();
    let mut seed = 0x9e37_79b9u32;
    for path in fixtures() {
        let file = std::fs::read(&path).unwrap();
        for len in (0..file.len()).step_by(7) {
            let _ = dump("t", &file[..len], &opts);
        }
        for _ in 0..500 {
            let mut f = file.clone();
            for _ in 0..3 {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let i = (seed >> 8) as usize % f.len();
                f[i] = (seed >> 24) as u8;
            }
            let _ = dump("t", &f, &opts);
        }
    }
}
