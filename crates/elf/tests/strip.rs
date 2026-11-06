//! `elf::strip::strip_debug` against `llvm-strip -g`: tests/strip holds
//! objects merlin-as wrote for files of the C library and what llvm-strip
//! made of each (scripts/gen-strip-golden.sh). Stripped here, each is the
//! same bytes; stripped again, it does not change.

use std::path::{Path, PathBuf};

fn dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/strip")
}

fn read(name: &str) -> Vec<u8> {
    std::fs::read(dir().join(name)).unwrap_or_else(|e| panic!("{name}: {e}"))
}

fn debug_sections(obj: &[u8]) -> usize {
    let e = elf::Elf::parse(obj).unwrap();
    e.sections
        .iter()
        .filter(|s| elf::strip::is_debug_section(&s.name))
        .count()
}

#[test]
fn stripping_writes_what_llvm_strip_writes() {
    for name in ["getenv", "printf", "crt0", "memcpy"] {
        let obj = read(&format!("{name}.o"));
        let want = read(&format!("{name}.stripped.o"));
        let got = elf::strip::strip_debug(&obj).unwrap();
        assert!(
            got == want,
            "{name}.o: {} bytes, llvm-strip's {} (first difference at {:?})",
            got.len(),
            want.len(),
            got.iter().zip(&want).position(|(a, b)| a != b)
        );
        assert_eq!(debug_sections(&got), 0, "{name}");
        assert_eq!(elf::strip::strip_debug(&got).unwrap(), got, "{name}: again");
    }
    assert!(debug_sections(&read("getenv.o")) > 0);
}

#[test]
fn string_tables_are_llvms() {
    let (table, at) = elf::strip::string_table(["setenv", "unsetenv", "", "a", "setenv", "b"]);
    // Sorted by the bytes from the end, the greatest first; setenv shares
    // unsetenv's bytes.
    assert_eq!(table, b"\0unsetenv\0b\0a\0");
    assert_eq!(at["unsetenv"], 1);
    assert_eq!(at["setenv"], 3);
    assert_eq!(at["b"], 10);
    assert_eq!(at[""], 0);
}

#[test]
fn only_elf64_objects_are_stripped() {
    let exe = std::fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/llvm/rv64/prog.elf")).unwrap();
    assert!(elf::strip::strip_debug(&exe).is_err());
    let rv32 = std::fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/llvm/rv32/prog.o")).unwrap();
    assert!(elf::strip::strip_debug(&rv32).is_err());
    // clang's object has an address-significance table: refused.
    let clang = std::fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/llvm/rv64/prog.o")).unwrap();
    let err = elf::strip::strip_debug(&clang).unwrap_err().0;
    assert!(err.contains(".llvm_addrsig"), "{err}");
}
