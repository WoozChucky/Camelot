//! The section headers `.section` makes, read back with the `elf` crate and
//! compared with clang's for the same source, on both widths: the type,
//! flags, entry size, alignment and size of every section (see
//! `crates/merlin-as/src/section.rs` for LLVM's rules).
//!
//! The expectations are `llvm-readelf -S` of the source assembled by
//! clang 23 with
//!
//! ```text
//! clang --target=riscv32 -march=rv32ima_zicsr_zifencei -mno-relax -c
//! clang --target=riscv64 -march=rv64ima_zicsr_zifencei -mabi=lp64 -mno-relax -c
//! ```
//!
//! which give the same rows for both. Only the order of the sections that
//! are not the source's differs (clang puts `.strtab` first).

use merlin_as::{Options, Xlen};

/// Names LLVM gives attributes to, flags, every way of writing the type,
/// mergeable sections with their entry sizes, names with no flags at all,
/// LLVM's own sections, code sections with and without instructions, and
/// going back to a section with and without its flags.
const SRC: &str = r#"
    .text
    nop
    .section .text.start
    nop
    .section .text.f, "ax", @progbits
    nop
    .section .myx, "ax", @progbits
    nop
    .p2align 3
    nop
    .section .text.empty, "ax", @progbits
    .section .rodata.str1.1, "aMS", @progbits, 1
    .asciz "hi"
    .section .rodata.cst4x, "aM", @progbits, 4
    .word 1
    .section .rodata.cst8x, "aM", %progbits, 8
    .dword 1
    .section .mystuff
    .word 1
    .section .sdata
    .word 1
    .section .sbss
    .word 0
    .section .bss.x
    .word 0
    .section .mybss, "aw", @nobits
    .space 12
    .section .init_array, "aw"
    .word 0
    .section .note.camelot, "a", "note"
    .word 4
    .section .flags, "0x3"
    .word 5
    .section .debug_str
    .asciz "s"
    .section .data.rel.ro
    .word 6
    .section .tdata, "awT", @progbits
    .word 7
    .section .text.f
    nop
    .section .rodata.str1.1, "aMS", @progbits, 1
    .asciz "x"
"#;

const PROGBITS: u32 = 1;
const NOTE: u32 = 7;
const NOBITS: u32 = 8;
const INIT_ARRAY: u32 = 14;

const W: u64 = 0x1;
const A: u64 = 0x2;
const X: u64 = 0x4;
const M: u64 = 0x10;
const S: u64 = 0x20;
const T: u64 = 0x400;

/// clang's rows, in the source's order: (name, type, flags, entry size,
/// alignment, size).
const CLANG: &[(&str, u32, u64, u64, u64, u64)] = &[
    // `.text` is the one section LLVM makes 4-aligned before the first
    // line; every other code section starts at 1 and grows only by
    // `.p2align`, instructions or not.
    (".text", PROGBITS, A | X, 0, 4, 4),
    (".text.start", PROGBITS, A | X, 0, 1, 4),
    (".text.f", PROGBITS, A | X, 0, 1, 8),
    (".myx", PROGBITS, A | X, 0, 8, 0xc),
    (".text.empty", PROGBITS, A | X, 0, 1, 0),
    (".rodata.str1.1", PROGBITS, A | M | S, 1, 1, 5),
    (".rodata.cst4x", PROGBITS, A | M, 4, 1, 4),
    (".rodata.cst8x", PROGBITS, A | M, 8, 1, 8),
    // Names LLVM gives nothing: not even loaded.
    (".mystuff", PROGBITS, 0, 0, 1, 4),
    (".sdata", PROGBITS, 0, 0, 1, 4),
    (".sbss", PROGBITS, 0, 0, 1, 4),
    (".bss.x", NOBITS, W | A, 0, 1, 4),
    (".mybss", NOBITS, W | A, 0, 1, 0xc),
    (".init_array", INIT_ARRAY, W | A, 0, 1, 4),
    (".note.camelot", NOTE, A, 0, 1, 4),
    (".flags", PROGBITS, W | A, 0, 1, 4),
    (".debug_str", PROGBITS, M | S, 1, 1, 2),
    (".data.rel.ro", PROGBITS, W | A, 0, 1, 4),
    (".tdata", PROGBITS, W | A | T, 0, 1, 4),
];

#[test]
fn section_headers_are_clangs() {
    for xlen in [Xlen::Rv32, Xlen::Rv64] {
        let bytes = merlin_as::assemble_to_elf_with(SRC, &Options { xlen }).expect("assembles");
        let file = elf::Elf::parse(&bytes).unwrap();
        // The source's sections come first, after the null one.
        let rows: Vec<_> = file.sections[1..=CLANG.len()]
            .iter()
            .map(|s| (s.name.as_str(), s.ty, s.flags, s.entsize, s.align, s.size))
            .collect();
        assert_eq!(rows, CLANG, "{xlen:?}");
        // A NOBITS section takes no room in the file.
        let bss = &file.sections[1 + CLANG.iter().position(|r| r.0 == ".mybss").unwrap()];
        let next = &file.sections[2 + CLANG.iter().position(|r| r.0 == ".mybss").unwrap()];
        assert_eq!(bss.offset, next.offset, "{xlen:?}");
    }
}
