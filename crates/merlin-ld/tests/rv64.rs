//! RV64: merlin-ld linking ELF64 objects into ELF64 executables, against
//! ld.lld's links of the same objects (the RV64 set of fixtures,
//! `tests/fixtures/rv64`, made by clang and LLD 23.1.4 with
//! `cargo xtask ld-fixtures`) and against merlin-as's RV64 objects.
//!
//! * The fixtures: every one of them gives ld.lld's executable (entry,
//!   allocated section headers, PT_LOADs with their offsets and load
//!   addresses, the PT_PHDR of loaded headers, symbols, every loaded byte)
//!   and runs in Grail, an RV64 machine for an ELF64 file. `rv64/relocs` and
//!   `rv64/at` are RV64's own (what RV64 code reaches is not what RV32 code
//!   does: the +-2 GiB window of lui, auipc and call, R_RISCV_64 data; and a
//!   higher-half program, linked at 0xffffffff80000000 and loaded in RAM with
//!   `AT(ADDR(.x) - OFFSET)`); the others are copies of the RV32 set's
//!   sources. Their `.objdump.txt` files are compared from #150, when
//!   merlin-objdump reads ELF64.
//! * The ELF64 file merlin-ld writes, its map, its line tables (R_RISCV_64
//!   addresses) and `strip_debug` of it.
//! * Every check of the object reader on ELF64's layout: its record sizes
//!   (64-byte section headers, 24-byte symbols and relocations), e_flags at
//!   offset 48 and the lp64 ABI, `r_info = sym << 32 | type`.
//! * Classes are not mixed, in ld.lld's words: `b.o is incompatible with
//!   a.o`, library members included, and a script's `OUTPUT_FORMAT` decides
//!   the class when it names one (`a.o is incompatible with
//!   elf32-littleriscv`).
//! * ELF64 layouts: addresses and symbols past 4 GiB, the end of the 64-bit
//!   address space, a 64-bit entry address.
//!
//! The ELF32 twins of these are in `link.rs`.

mod common;

use common::{errors, input, read_ours, run, script, RV64};
use elf::Class;
use excalibur::Xlen;
use grail::RunResult;
use merlin_ld::{Input, Options};
use std::path::PathBuf;

fn assemble(name: &str, src: &str, xlen: Xlen) -> Input {
    let data = merlin_as::assemble_to_elf_with(src, &merlin_as::Options { xlen })
        .unwrap_or_else(|e| panic!("{name}: {e:?}\n{src}"));
    Input {
        name: name.into(),
        data,
    }
}

fn rv64(name: &str, src: &str) -> Input {
    assemble(name, src, Xlen::Rv64)
}

fn rv32(name: &str, src: &str) -> Input {
    assemble(name, src, Xlen::Rv32)
}

// The RV64 set (`tests/fixtures/rv64`) of the helpers in common/mod.rs.

fn fixtures() -> PathBuf {
    common::fixtures(RV64)
}

fn inputs(dir: &str, names: &[&str]) -> Vec<Input> {
    common::inputs(RV64, dir, names)
}

fn case(script: &str) -> (Vec<Input>, Options) {
    common::case(RV64, script)
}

fn expected(name: &str) -> common::Readelf {
    common::expected(RV64, name)
}

fn assert_matches_ld_lld(name: &str, output: &str) -> merlin_ld::Linked {
    common::assert_matches_ld_lld_exit(RV64, name, output, 0)
}

fn reloc_types(path: &str) -> std::collections::HashMap<String, usize> {
    common::reloc_types(RV64, path)
}

fn norelocs() -> Vec<Input> {
    inputs("norelocs", &["start.o", "data.o", "more.o"])
}

/// A program with a pc-relative `la`, a load through a `.dword` pointer
/// (R_RISCV_64) and a jump: what an RV64 object at 0x8000_0000 can hold.
const START: &str = "    .text
    .globl _start
_start:
    la a0, value
    ld a1, 0(a0)
    j _start
    .data
    .globl value
value:
    .dword value
";

/// An object defining `name` in `.text`, 8 bytes of code.
fn defines(name: &str) -> String {
    format!("    .text\n    .globl {name}\n{name}:\n    nop\n    ret\n")
}

#[test]
fn reads_rv64_objects() {
    let o = merlin_ld::object::parse("a.o", &rv64("a.o", START).data).unwrap();
    assert_eq!(o.class, Class::Elf64);
    let section = |name: &str| o.sections.iter().find(|s| s.name == name).unwrap();
    assert_eq!(section(".text").size, 16);
    assert_eq!(section(".data").data, [0; 8]);
    // r_info's symbol index is its upper 32 bits: the relocations name the
    // symbols they use.
    let relocs: Vec<(u32, &str)> = [".text", ".data"]
        .iter()
        .flat_map(|s| &section(s).relocs)
        .map(|r| (r.ty, o.symbol_display_name(r.sym)))
        .filter(|(ty, _)| *ty != 51) // R_RISCV_RELAX
        .collect();
    let value = o.symbols.iter().find(|s| s.name == "value").unwrap();
    assert_eq!(value.shndx as usize, section_index(&o, ".data"));
    assert!(relocs.contains(&(23, "value")), "{relocs:?}"); // R_RISCV_PCREL_HI20
    assert!(relocs.contains(&(2, "value")), "{relocs:?}"); // R_RISCV_64
}

fn section_index(o: &merlin_ld::object::Object, name: &str) -> usize {
    o.sections.iter().position(|s| s.name == name).unwrap()
}

/// File offset of the ELF64 section header named `name`.
fn shdr(elf: &[u8], name: &str) -> usize {
    let parsed = elf::Elf::parse(elf).unwrap();
    let i = parsed
        .sections
        .iter()
        .position(|s| s.name == name)
        .unwrap_or_else(|| panic!("no section {name}"));
    parsed.header.shoff as usize + i * elf::SHDR64_SIZE
}

#[test]
fn rejects_bad_rv64_objects() {
    let good = rv64("x.o", START).data;
    let patched = |off: usize, bytes: &[u8]| {
        let mut b = good.clone();
        b[off..off + bytes.len()].copy_from_slice(bytes);
        b
    };
    // ELF64 section header fields: sh_offset at 24, sh_addralign at 48,
    // sh_entsize at 56.
    let in_shdr = |name: &str, field: usize, v: u64| patched(shdr(&good, name) + field, &v.to_le_bytes());
    let shoff = elf::Elf::parse(&good).unwrap().header.shoff as usize;
    let rela = {
        let h = shdr(&good, ".rela.text");
        u64::from_le_bytes(good[h + 24..h + 32].try_into().unwrap()) as usize
    };
    let not_strtab = elf::Elf::parse(&good)
        .unwrap()
        .sections
        .iter()
        .position(|s| s.ty == elf::SHT_PROGBITS)
        .unwrap();
    let cases: Vec<(Vec<u8>, String)> = vec![
        (
            patched(4, &[3]),
            "x.o: not a 32- or 64-bit ELF file (EI_CLASS is 3)".into(),
        ),
        // e_flags is at 48 in ELF64 (36 in ELF32).
        (
            patched(48, &[1, 0, 0, 0]),
            "x.o: built with compressed instructions (EF_RISCV_RVC); Excalibur has no C extension".into(),
        ),
        (
            patched(48, &[4, 0, 0, 0]),
            "x.o: e_flags 0x4 not supported: Excalibur objects use the lp64 soft-float ABI (e_flags 0)"
                .into(),
        ),
        (
            patched(58, &40u16.to_le_bytes()),
            "x.o: e_shentsize is 40, expected 64".into(),
        ),
        (
            patched(62, &(not_strtab as u16).to_le_bytes()),
            format!("x.o: e_shstrndx {not_strtab} is not a string table"),
        ),
        (
            in_shdr(".symtab", 56, 16),
            "x.o: symbol table entry size is 16, expected 24".into(),
        ),
        (
            in_shdr(".rela.text", 56, 12),
            "x.o: relocation section '.rela.text' has entry size 12, expected 24".into(),
        ),
        // r_info of the first relocation, symbol 999 in its upper half.
        (
            patched(rela + 8, &((999u64 << 32) | 23).to_le_bytes()),
            "x.o: relocation in '.rela.text' refers to symbol index 999, past the symbol table".into(),
        ),
        // ld.lld: "section sh_addralign is too large".
        (
            in_shdr(".text", 48, 1 << 32),
            "x.o: section '.text' has sh_addralign 0x100000000, too large".into(),
        ),
        // ld.lld: "x.o:(.text): sh_addralign is not a power of 2", also for
        // 2^32 - 1, which fits in 32 bits.
        (
            in_shdr(".text", 48, 0xffff_ffff),
            "x.o: section '.text' has sh_addralign 0xffffffff, not a power of 2".into(),
        ),
        // ld.lld: "section [index N] has an invalid sh_size (8) which is not
        // a multiple of its sh_entsize (24)" (ELF64's sh_size at 32).
        (
            in_shdr(".symtab", 32, 8),
            "x.o: section '.symtab' has sh_size 0x8, not a multiple of its sh_entsize (24)".into(),
        ),
        // A section header is read whole: a cut inside the third names its
        // 64 bytes.
        (
            good[..shoff + 2 * 64 + 10].to_vec(),
            format!(
                "x.o: truncated file (needs bytes {:#x}..{:#x})",
                shoff + 128,
                shoff + 192
            ),
        ),
        // Contents past the end of the file, with a 64-bit offset.
        (
            in_shdr(".text", 24, 1 << 40),
            "x.o: truncated file (needs bytes 0x10000000000..0x10000000010)".into(),
        ),
    ];
    for (data, want) in cases {
        let errs = errors(merlin_ld::layout(
            &[Input {
                name: "x.o".into(),
                data,
            }],
            &Options::default(),
        ));
        assert_eq!(errs, [want]);
    }

    // What ld.lld accepts: an alignment of 2^31 (the largest power of two
    // in 32 bits), and program headers, which mean nothing in an object
    // (here 3 of them, past the end of the file: e_phoff at 32, e_phnum at
    // 56).
    let aligned = merlin_ld::layout(
        &[Input {
            name: "x.o".into(),
            data: in_shdr(".text", 48, 1 << 31),
        }],
        &Options::default(),
    )
    .unwrap();
    assert_eq!(aligned.section(".text").unwrap().align, 1 << 31);
    let mut stray = patched(32, &(1u64 << 40).to_le_bytes());
    stray[56..58].copy_from_slice(&3u16.to_le_bytes());
    assert!(merlin_ld::layout(
        &[Input {
            name: "x.o".into(),
            data: stray,
        }],
        &Options::default(),
    )
    .is_ok());
}

#[test]
fn rv32_and_rv64_objects_are_not_mixed() {
    // Every object of the other class is reported, in order, against the
    // first one (ld.lld 23: "b.o is incompatible with a.o").
    let objs = [
        rv64("a.o", START),
        rv32("b.o", &defines("b")),
        rv64("c.o", &defines("c")),
        rv32("d.o", &defines("d")),
    ];
    assert_eq!(
        errors(merlin_ld::layout(&objs, &Options::default())),
        ["b.o is incompatible with a.o", "d.o is incompatible with a.o"]
    );
    let objs = [rv32("a.o", &defines("_start")), rv64("b.o", &defines("b"))];
    assert_eq!(
        errors(merlin_ld::link(&objs, &Options::default())),
        ["b.o is incompatible with a.o"]
    );
    // A script's own errors come first, as in ld.lld, which reads the
    // script before it compares the objects.
    assert_eq!(
        errors(merlin_ld::layout(
            &objs,
            &script("SECTIONS { .text : { *(.text) } } BOGUS")
        )),
        ["test.ld:1: unsupported linker script command 'BOGUS'"]
    );
}

#[test]
fn library_members_of_the_other_class_are_errors_even_when_not_needed() {
    let program = [rv64("a.o", START)];
    let library = [rv32("b32.o", &defines("b")), rv64("c64.o", &defines("c"))];
    assert_eq!(
        errors(merlin_ld::link_with_library(
            &program,
            &library,
            &Options::default()
        )),
        ["b32.o is incompatible with a.o"]
    );
    // With an OUTPUT_FORMAT the members are compared with it, as the
    // program's objects are.
    let elf32 = script("OUTPUT_FORMAT(\"elf32-littleriscv\") SECTIONS { .text : { *(.text) } }");
    assert_eq!(
        errors(merlin_ld::link_with_library(&program, &library, &elf32)),
        [
            "a.o is incompatible with elf32-littleriscv",
            "c64.o is incompatible with elf32-littleriscv"
        ]
    );
}

/// merlin-ld's command line: ld.lld reads the script first, then every
/// file in command-line order, library members among them, and compares
/// the classes only when all of that read.
#[test]
fn the_command_line_is_read_in_ld_llds_order() {
    let a64 = rv64("a64.o", START);
    let b32 = rv32("b32.o", &defines("b"));
    let c32 = rv32("c32.o", &defines("c"));
    let b64 = rv64("b64.o", &defines("b"));
    let link = |files: &[(&Input, bool)], opts: &Options| errors(merlin_ld::link_files(files, opts));
    let ok = Options::default();
    // `a64.o --start-lib b32.o --end-lib c32.o`: in that order.
    assert_eq!(
        link(&[(&a64, false), (&b32, true), (&c32, false)], &ok),
        [
            "b32.o is incompatible with a64.o",
            "c32.o is incompatible with a64.o"
        ]
    );
    // `--start-lib b32.o --end-lib a64.o b64.o`: the class is the member's,
    // and ld.lld names nothing for it.
    assert_eq!(
        link(&[(&b32, true), (&a64, false), (&b64, false)], &ok),
        ["a64.o is incompatible", "b64.o is incompatible"]
    );
    // A script's errors come first and hide the classes', also through a
    // library, and also with an OUTPUT_FORMAT merlin-ld does not know.
    for (src, want) in [
        ("SECTIONS { .text : {", "test.ld:1: unexpected end of script"),
        (
            "OUTPUT_FORMAT(foo)",
            "test.ld:1: OUTPUT_FORMAT(foo): only elf32-littleriscv and elf64-littleriscv are supported",
        ),
    ] {
        assert_eq!(
            link(&[(&a64, false), (&b32, true)], &script(src)),
            [want],
            "{src}"
        );
        assert_eq!(
            errors(merlin_ld::link_with_library(
                &[a64.clone(), b32.clone()],
                &[],
                &script(src)
            )),
            [want],
            "{src}"
        );
    }
    // Then the objects' errors, all of them, before the classes.
    let cut = Input {
        name: "cut.o".into(),
        data: a64.data[..64].to_vec(),
    };
    let errs = link(&[(&cut, false), (&b32, false)], &script("SECTIONS {"));
    assert_eq!(errs.len(), 2, "{errs:?}");
    assert_eq!(errs[0], "test.ld:1: unexpected end of script");
    assert!(errs[1].starts_with("cut.o: "), "{errs:?}");
}

#[test]
fn output_format_decides_the_class() {
    let sections = "SECTIONS { . = 0x80000000; .text : { *(.text) } .data : { *(.data) } }";
    let objs = [rv64("a.o", START), rv64("b.o", &defines("b"))];
    assert_eq!(
        errors(merlin_ld::layout(
            &objs,
            &script(&format!("OUTPUT_FORMAT(\"elf32-littleriscv\") {sections}"))
        )),
        [
            "a.o is incompatible with elf32-littleriscv",
            "b.o is incompatible with elf32-littleriscv"
        ]
    );
    let layout = merlin_ld::layout(
        &objs,
        &script(&format!("OUTPUT_FORMAT(\"elf64-littleriscv\") {sections}")),
    )
    .unwrap();
    assert_eq!(layout.class, Class::Elf64);
    // With a format, the first object is compared with it too, and the
    // others are not compared with the first.
    let mixed = [rv32("a.o", &defines("_start")), rv64("b.o", &defines("b"))];
    assert_eq!(
        errors(merlin_ld::layout(
            &mixed,
            &script(&format!("OUTPUT_FORMAT(\"elf64-littleriscv\") {sections}"))
        )),
        ["a.o is incompatible with elf64-littleriscv"]
    );
    // A higher-half RV64 script given RV32 objects: the objects are the
    // error, not the script's addresses past 4 GiB (ld.lld reads them as
    // 64-bit numbers before it looks at the objects), wherever the
    // OUTPUT_FORMAT is.
    let rv32_objs = [rv32("a.o", &defines("_start"))];
    let higher_half = "MEMORY { RAM (rwx) : ORIGIN = 0xffffffff80000000, LENGTH = 128M }
        OUTPUT_FORMAT(\"elf64-littleriscv\")
        SECTIONS { .text : { *(.text) } > RAM }";
    assert_eq!(
        errors(merlin_ld::layout(&rv32_objs, &script(higher_half))),
        ["a.o is incompatible with elf64-littleriscv"]
    );
    // And RV64 objects given an RV32 script with a 64-bit number.
    let big =
        "OUTPUT_FORMAT(\"elf32-littleriscv\") SECTIONS { . = 0xffffffff80000000; .text : { *(.text) } }";
    assert_eq!(
        errors(merlin_ld::layout(&objs[..1], &script(big))),
        ["a.o is incompatible with elf32-littleriscv"]
    );
}

#[test]
fn rv64_layouts_have_64_bit_addresses() {
    // The default layout, for pc-relative code: .text at 0x8000_0000.
    let layout = merlin_ld::layout(&[rv64("a.o", START)], &Options::default()).unwrap();
    assert_eq!(layout.class, Class::Elf64);
    assert_eq!(layout.entry, 0x8000_0000);
    assert_eq!(layout.section(".text").unwrap().addr, 0x8000_0000);

    // Past 4 GiB: an ELF32 error (link.rs's layout_errors), an address here.
    let layout = merlin_ld::layout(
        &[rv64("a.o", START)],
        &script("SECTIONS { . = 0x100000000; .text : { *(.text) } .data : { *(.data) } x = 0x100000000; }"),
    )
    .unwrap();
    assert_eq!(layout.section(".text").unwrap().addr, 0x1_0000_0000);
    assert_eq!(layout.symbol("_start"), Some(0x1_0000_0000));
    assert_eq!(layout.symbol("x"), Some(0x1_0000_0000));

    // A section must end below 2^64: ld.lld's text, also for one that
    // would end exactly there.
    assert_eq!(
        errors(merlin_ld::layout(
            &[rv64("a.o", &defines("_start"))],
            &script("SECTIONS { . = 0xfffffffffffffff8; .text : { *(.text) } }"),
        )),
        ["test.ld:1: section .text at 0xfffffffffffffff8 of size 0x8 exceeds available address space"]
    );
    let layout = merlin_ld::layout(
        &[rv64("a.o", &defines("_start"))],
        &script("SECTIONS { . = 0xfffffffffffffff0; .text : { *(.text) } }"),
    )
    .unwrap();
    assert_eq!(layout.section(".text").unwrap().addr, 0xffff_ffff_ffff_fff0);

    // As in ld.lld, the location counter goes on from the end of a section
    // past 2^64, wrapped: only that one is reported, and the next is at 0.
    let two = rv64("a.o", ".text\n.globl _start\n_start: nop\nnop\n.data\n.dword 1\n");
    let both = ".text : { *(.text) } .data : { *(.data) }";
    assert_eq!(
        errors(merlin_ld::layout(
            std::slice::from_ref(&two),
            &script(&format!("SECTIONS {{ . = 0xfffffffffffffff8; {both} }}")),
        )),
        ["test.ld:1: section .text at 0xfffffffffffffff8 of size 0x8 exceeds available address space"]
    );
    // At the top level `.` wraps round 2^64, by `+=` and by ALIGN, and a
    // section's alignment wraps to 0 too; all of these link, as in ld.lld.
    for (start, at) in [
        (format!(". = 0xfffffffffffffff0; . += 0x20; {both}"), 0x10),
        (format!(". = 0xfffffffffffffff8; . = ALIGN(16); {both}"), 0),
        (
            ". = 0xfffffffffffffff8; .text : ALIGN(16) { *(.text) } .data : { *(.data) }".into(),
            0,
        ),
    ] {
        let src = format!("SECTIONS {{ {start} }}");
        let layout = merlin_ld::layout(std::slice::from_ref(&two), &script(&src))
            .unwrap_or_else(|e| panic!("{src}: {e:?}"));
        assert_eq!(layout.section(".text").unwrap().addr, at, "{src}");
    }
    // Inside a section it is a move backward; `.` still takes the value, so
    // that section is past 2^64 and the next ones are fine at 0.
    assert_eq!(
        errors(merlin_ld::layout(
            std::slice::from_ref(&two),
            &script(&format!(
                "SECTIONS {{ . = 0xfffffffffffffff8; .s : {{ . = ALIGN(16); }} {both} }}"
            )),
        )),
        [
            "test.ld:1: cannot move the location counter backward (from 0xfffffffffffffff8 to 0x0)",
            "test.ld:1: section .s at 0xfffffffffffffff8 of size 0x8 exceeds available address space"
        ]
    );

    // -e takes a 64-bit address.
    let opts = Options {
        entry: Some("0xffffffff80000000".into()),
        ..Options::default()
    };
    let layout = merlin_ld::layout(&[rv64("a.o", START)], &opts).unwrap();
    assert_eq!(layout.entry, 0xffff_ffff_8000_0000);
}

// ---------------------------------------------------------------------------
// The ELF64 executable.

/// What makes the file ELF64, ld.lld's layout of it: a 64-byte header with
/// 56-byte program headers and 64-byte section headers, 24-byte symbols,
/// `.symtab` and the section header table 8-aligned, e_flags 0 (lp64
/// soft-float), 4 KiB pages as on RV32.
#[test]
fn rv64_executables_are_elf64() {
    let linked = merlin_ld::link(&[rv64("a.o", START)], &Options::default()).unwrap();
    let file = &linked.elf;
    assert_eq!(
        &file[..8],
        b"\x7fELF\x02\x01\x01\x00",
        "ELFCLASS64, little-endian, version 1"
    );
    let e = elf::Elf::parse(file).unwrap();
    assert_eq!(e.class, Class::Elf64);
    let h = &e.header;
    assert_eq!(
        (h.ty, h.machine, h.version, h.flags),
        (elf::ET_EXEC, elf::EM_RISCV, 1, 0)
    );
    assert_eq!((h.ehsize, h.phentsize, h.shentsize), (64, 56, 64));
    assert_eq!((h.entry, h.phoff, h.phnum), (0x8000_0000, 64, 2));
    assert_eq!(h.shoff % 8, 0);
    assert_eq!(e.sections[h.shstrndx as usize].name, ".shstrtab");
    let symtab = e.sections.iter().find(|s| s.name == ".symtab").unwrap();
    assert_eq!((symtab.entsize, symtab.align, symtab.offset % 8), (24, 8, 0));
    assert_eq!(symtab.size % 24, 0);
    assert_eq!(e.sections[symtab.link as usize].name, ".strtab");
    // sh_info is the first global: the locals come before it.
    let syms = e.symbols_in(symtab).unwrap();
    let first_global = symtab.info as usize;
    assert!(syms[1..first_global].iter().all(|s| s.bind() == elf::STB_LOCAL));
    assert!(syms[first_global..].iter().all(|s| s.bind() == elf::STB_GLOBAL));
    let value = linked.layout.symbol("value").unwrap();
    assert_eq!(elf::find_symbol(&syms, "value").map(|s| s.value), Some(value));
    // An RX and an RW segment; `.dword value` is R_RISCV_64, 8 bytes.
    let loads: Vec<(u64, u64, &str, u64)> = read_ours(file)
        .loads
        .iter()
        .map(|l| {
            (
                l.vaddr,
                l.paddr,
                if l.flags == "RE" { "RE" } else { "RW" },
                l.align,
            )
        })
        .collect();
    assert_eq!(
        loads,
        [
            (0x8000_0000, 0x8000_0000, "RE", 0x1000),
            (value, value, "RW", 0x1000)
        ]
    );
    let rw = &e.segments[1];
    assert_eq!(rw.offset % 0x1000, rw.vaddr % 0x1000);
    assert_eq!(e.segment_data(rw).unwrap(), value.to_le_bytes());
}

// ---------------------------------------------------------------------------
// The fixtures, against ld.lld.

/// Acceptance: relocation-free clang objects link to the same PT_LOADs,
/// sections and symbol addresses as ld.lld, and run.
#[test]
fn norelocs_matches_ld_lld() {
    let lld = expected("norelocs/link");
    assert_eq!(lld.loads.len(), 3, "fixture sanity: RX, R and RW segments");
    // The first segment one page in, as for RV32 (ld.lld's page rule, the
    // same page size).
    assert_eq!((lld.loads[0].offset, lld.loads[0].vaddr), (0x1000, 0x8000_0000));
    assert_matches_ld_lld("norelocs/link", "OK\n");
}

/// The RV32 set's layout edge cases (link.rs), on RV64: the same sources,
/// the same results.
#[test]
fn layout_edge_cases_match_ld_lld() {
    let mut names: Vec<String> = std::fs::read_dir(fixtures().join("layouts"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "ld"))
        .map(|p| format!("layouts/{}", p.file_stem().unwrap().to_string_lossy()))
        .collect();
    names.sort();
    assert_eq!(names.len(), 10, "{names:?}");
    for name in &names {
        assert_matches_ld_lld(name, "OK\n");
    }
    // Spot checks, so a broken fixture can't make both sides agree on nonsense.
    let lld = expected("layouts/empty");
    assert_eq!(lld.globals["__image_start"], 0);
    assert_eq!(lld.globals["wk"], 0x8000_0024, "weak definition");
    assert_eq!(expected("layouts/gap").loads.len(), 2);
    let attrs = expected("layouts/attrs");
    assert_eq!(attrs.globals["__end"], 0x8000_2000);
    assert_eq!(attrs.loads.len(), 3);
    let marker = expected("layouts/no-inputs");
    let marker = marker.sections.iter().find(|s| s.name == ".marker").unwrap();
    assert_eq!(marker.flags, "A", "no inputs after the code: not executable");
    // ELF64's headers take 0x1c8 bytes (64 + 7 * 56) where ELF32's take
    // 0x114: not loaded below a section at 0x100 into its page, loaded below
    // one at 0x400.
    let unaligned = expected("layouts/unaligned");
    assert_eq!(unaligned.phdr, None);
    assert_eq!(unaligned.loads[0].offset, 0x1100);
    let headers = expected("layouts/headers");
    assert_eq!(
        (
            headers.loads[0].offset,
            headers.loads[0].vaddr,
            headers.loads[0].filesz
        ),
        (0, 0x8000_0000, 0x1c8)
    );
    let phdr = headers.phdr.unwrap();
    assert_eq!((phdr.offset, phdr.filesz, phdr.align), (64, 0x188, 8));
}

/// examples/hello assembled for RV64 by clang: xcal.dbg prints 64-bit
/// registers, 16 digits.
#[test]
fn hello_matches_ld_lld_and_runs() {
    assert_matches_ld_lld(
        "hello/link",
        "Hello from Camelot!\n[dbg] pc=0000000080000018 a0 = 0x0000000000000014 (20)\n",
    );
}

/// examples/multiobj's RV64 build, two clang objects (`main-rv64.S`,
/// `print.S`): calls, pc-relative addresses and `.dword` pointers from one into
/// the other (R_RISCV_64), `.data` written at run time and `.bss` (link.rs's
/// twin).
#[test]
fn multiobj_matches_ld_lld_and_runs() {
    for (obj, ty) in [
        ("multiobj/main-rv64.o", "R_RISCV_CALL_PLT"),
        ("multiobj/main-rv64.o", "R_RISCV_64"),
        ("multiobj/main-rv64.o", "R_RISCV_PCREL_HI20"),
        ("multiobj/print.o", "R_RISCV_PCREL_LO12_I"),
    ] {
        assert!(reloc_types(obj).contains_key(ty), "{obj} has no {ty}");
    }
    let linked = common::assert_matches_ld_lld_exit(
        RV64,
        "multiobj/link",
        "Hello from two objects!\nputs lives in print.S.\n\
         [dbg] pc=0000000080000034 a0 = 0x000000000000002f (47)\n",
        42,
    );
    let l = &linked.layout;
    assert_eq!(l.class, Class::Elf64);
    let bss = l.section(".bss").unwrap();
    assert_eq!(l.symbol("bytes_written"), Some(bss.addr));
    assert!(l.symbol("puts").unwrap() > l.symbol("_start").unwrap());
}

/// Spec §10: merlin-ld does not merge SHF_MERGE strings (link.rs's twin).
#[test]
fn mergeable_strings_are_not_merged() {
    let (objs, opts) = case("merge/link.ld");
    let linked = merlin_ld::link(&objs, &opts).unwrap();
    assert_eq!(linked.layout.warnings.len(), 1, "{:?}", linked.layout.warnings);
    let lld = expected("merge/link");
    let ours = read_ours(&linked.elf);
    assert_eq!(ours.sections[0], lld.sections[0], ".text is unaffected");
    assert_eq!(
        ours.sections[1].size,
        lld.sections[1].size + 6,
        "one extra \"hello\\0\""
    );
    assert_eq!(lld.globals["s1"], lld.globals["s2"], "ld.lld merged them");
    assert_eq!(ours.globals["s2"], ours.globals["after_a"] + 1);
}

/// Acceptance: every relocation type in RV64 code (rv64/relocs/main.S),
/// at the edges of the BRANCH and JAL ranges and of the +-2 GiB window of
/// HI20, PCREL_HI20 and CALL_PLT, with R_RISCV_64 and R_RISCV_32 data, gives
/// ld.lld's bytes. The program also checks its own relocated values.
#[test]
fn relocs_match_ld_lld() {
    let main = reloc_types("relocs/main.o");
    for ty in [
        "R_RISCV_64",
        "R_RISCV_32",
        "R_RISCV_BRANCH",
        "R_RISCV_JAL",
        "R_RISCV_CALL",
        "R_RISCV_CALL_PLT",
        "R_RISCV_PCREL_HI20",
        "R_RISCV_PCREL_LO12_I",
        "R_RISCV_PCREL_LO12_S",
        "R_RISCV_HI20",
        "R_RISCV_LO12_I",
        "R_RISCV_LO12_S",
    ] {
        assert!(main.contains_key(ty), "main.o has no {ty}: {main:?}");
    }
    assert!(
        !main.contains_key("R_RISCV_RELAX"),
        "main.o is built with -mno-relax"
    );
    assert!(reloc_types("relocs/lib.o").contains_key("R_RISCV_64"));
    let relax = reloc_types("relocs/relax.o");
    // A hint for each relaxable instruction: the call, both halves of the
    // ld, of the sd and of the lui/addi pair, and the loop's j.
    assert_eq!(relax.get("R_RISCV_RELAX"), Some(&8), "{relax:?}");
    for ty in [
        "R_RISCV_BRANCH",
        "R_RISCV_JAL",
        "R_RISCV_HI20",
        "R_RISCV_PCREL_LO12_S",
    ] {
        assert_eq!(relax.get(ty), Some(&1), "{ty}: {relax:?}");
    }

    let linked = assert_matches_ld_lld("relocs/link", "OK\n");
    let l = &linked.layout;
    assert_eq!(l.class, Class::Elf64);
    // The edges main.S aims at, so a changed fixture can't silently lose them.
    let sym = |n: &str| l.symbol(n).unwrap_or_else(|| panic!("{n}"));
    let start = sym("_start");
    assert_eq!(start, 0x8010_0000);
    assert_eq!(sym("high_target") - (start + 4), 0xf_fffc);
    assert_eq!((start + 8) - sym("low_target"), 0x10_0000);
    let edges = sym("branch_edges");
    assert_eq!(edges - sym("branch_back"), 4096);
    assert_eq!(sym("branch_fwd") - (edges + 4), 4092);
    for site in ["pc", "call"] {
        let at = sym(&format!("{site}_max_site"));
        assert_eq!(sym(&format!("{site}_max")) - at, 0x7fff_f7ff, "{site}");
        let at = sym(&format!("{site}_min_site"));
        assert_eq!(at - sym(&format!("{site}_min")), 0x8000_0800, "{site}");
    }
    assert_eq!(
        (sym("hi_max"), sym("hi_min")),
        (0x7fff_f7ff, 0xffff_ffff_7fff_f800)
    );
    // The `la` of the undefined weak symbol reaches 0 from there only.
    assert!(sym("low_weak") < 0x8000_0800);
}

/// One byte past each edge of the window (rv64/relocs/link.ld, moved): the
/// same errors as ld.lld's, with the 20-bit value it prints, for lui, auipc
/// and call alike.
#[test]
fn relocation_window_errors() {
    let (objs, opts) = case("relocs/link.ld");
    assert_eq!(
        objs.iter().map(|o| o.name.as_str()).collect::<Vec<_>>(),
        ["main.o", "lib.o", "relax.o"]
    );
    let text = &opts.script.as_ref().unwrap().text;
    let moved = text
        .replace("hi_max = 0x7ffff7ff", "hi_max = 0x7ffff800")
        .replace("hi_min = 0xffffffff7ffff800", "hi_min = 0xffffffff7ffff7ff")
        .replace("pc_max_site + 0x7ffff7ff", "pc_max_site + 0x7ffff800")
        .replace("call_min_site - 0x80000800", "call_min_site - 0x80000801");
    assert_ne!(&moved, text);
    let errs = errors(merlin_ld::link(&objs, &script(&moved)));
    // `obj: relocation TY out of range for 'sym' at SEC+OFF: V is not in
    // [-524288, 524287]`, at each relocation against `sym`.
    let error = |obj: usize, sec: &str, sym: &str, value: i64| -> Vec<String> {
        let name = &objs[obj].name;
        let parsed = merlin_ld::object::parse(name, &objs[obj].data).unwrap();
        let i = parsed.symbols.iter().position(|s| s.name == sym).unwrap();
        let s = parsed.sections.iter().find(|s| s.name == sec).unwrap();
        s.relocs
            .iter()
            .filter(|r| r.sym as usize == i && !merlin_ld::reloc::name(r.ty).contains("LO12"))
            .map(|r| {
                let ty = merlin_ld::reloc::name(r.ty);
                format!(
                    "{name}: relocation {ty} out of range for '{sym}' at {sec}+{:#x}: {value} is not in \
                     [-524288, 524287]",
                    r.offset
                )
            })
            .collect()
    };
    let want: Vec<String> = [
        error(0, ".text.start", "hi_max", 524288),
        error(0, ".text.start", "hi_min", -524289),
        error(0, ".text.start", "pc_max", 524288),
        error(0, ".text.start", "call_min", -524289),
        error(2, ".text", "hi_max", 524288),
    ]
    .concat();
    assert_eq!(want.len(), 5, "{want:?}");
    assert!(want[0].contains("R_RISCV_HI20") && want[3].contains("R_RISCV_CALL_PLT"));
    assert_eq!(errs, want);
}

/// A jump or call to an undefined weak symbol aims at itself (as in
/// ld.lld), and the `la` of it, 0x10 bytes past 0x8000_0000, is
/// `auipc a0, 0x80000; addi a0, a0, -16`: 0.
#[test]
fn undefined_weak_symbols() {
    let (objs, opts) = case("relocs/link.ld");
    let linked = merlin_ld::link(&objs, &opts).unwrap();
    let e = elf::Elf::parse(&linked.elf).unwrap();
    let words = |vaddr: u64| -> Vec<u32> {
        let seg = e.segments.iter().find(|s| s.vaddr == vaddr).unwrap();
        e.segment_data(seg)
            .unwrap()
            .chunks(4)
            .map(|c| u32::from_le_bytes(c.try_into().unwrap()))
            .collect()
    };
    // `call missing` (auipc ra, 0; jalr ra, 0(ra)), `jal missing`, `beq a0, a1, missing`.
    assert!(words(0x8010_0000)
        .windows(4)
        .any(|w| w == [0x0000_0097, 0x0000_80e7, 0x0000_00ef, 0x00b5_0063]));
    let low_weak = linked.layout.symbol("low_weak").unwrap();
    assert_eq!(low_weak, 0x8000_0010);
    let low = words(0x8000_0000);
    let at = ((low_weak - 0x8000_0000) / 4) as usize;
    let (hi, lo) = match (
        excalibur::decode(low[at], Xlen::Rv64),
        excalibur::decode(low[at + 1], Xlen::Rv64),
    ) {
        (excalibur::Inst::Auipc { imm: hi, .. }, excalibur::Inst::OpImm { imm: lo, .. }) => (hi, lo),
        other => panic!("not auipc/addi: {other:?}"),
    };
    assert_eq!((hi, lo), (-0x8000_0000, -16));
    assert_eq!(low_weak.wrapping_add(hi as u64).wrapping_add(lo as u64), 0);
}

/// symbols/weak.o (a copy of the RV32 set's) on RV64: weak definitions and a
/// weak undefined reference (link.rs's `weak_symbols`).
#[test]
fn weak_symbols() {
    let objs = [
        input(&fixtures().join("norelocs/data.o")),
        input(&fixtures().join("symbols/weak.o")),
    ];
    let l = merlin_ld::layout(&objs, &Options::default()).unwrap();
    let text = l.section(".text").unwrap();
    assert!((text.addr..text.addr + text.size).contains(&l.symbol("helper").unwrap()));
    let data = l.section(".data").unwrap();
    assert!((data.addr..data.addr + data.size).contains(&l.symbol("weak_only").unwrap()));
    assert_eq!(l.symbol("missing"), None);
    let missing = l.objects[1]
        .symbols
        .iter()
        .position(|s| s.name == "missing")
        .unwrap();
    assert_eq!(l.reloc_symbol(1, missing), Ok(0));
}

/// rejects/*.o (copies of the RV32 set's) built for RV64: the same errors.
#[test]
fn rejects_unsupported_rv64_objects() {
    for (obj, want) in [
        ("common.o", "x.o: common symbol 'foo' is not supported"),
        (
            "comdat.o",
            "x.o: section group '.group' (COMDAT) is not supported",
        ),
        ("tls.o", "x.o: thread-local section '.tdata' is not supported"),
        (
            "nonalloc.o",
            "x.o: global symbol 'g' is defined in section '.note.camelot', which is not loaded",
        ),
    ] {
        let data = std::fs::read(fixtures().join("rejects").join(obj)).unwrap();
        assert_eq!(elf::Elf::parse(&data).unwrap().class, Class::Elf64, "{obj}");
        let errs = errors(merlin_ld::link(
            &[Input {
                name: "x.o".into(),
                data,
            }],
            &Options::default(),
        ));
        assert_eq!(errs.len(), 1, "{obj}: {errs:?}");
        assert!(errs[0].starts_with(want), "{obj}: {errs:?}");
    }
    let errs = errors(merlin_ld::link(
        &inputs("rejects", &["align.o"]),
        &Options::default(),
    ));
    assert_eq!(
        errs,
        [
            "align.o: R_RISCV_ALIGN at .text+0xc: merlin-ld does not delete alignment padding as ld.lld \
             does; assemble with -mno-relax"
        ]
    );
}

/// link.rs's `layout_errors`, on RV64: what is an error on both widths is
/// the same error, and what is RV32's 4 GiB limit links.
#[test]
fn layout_errors_and_the_4_gib_limits_that_are_rv32s() {
    let cases: &[(&str, &str)] = &[
        (
            "SECTIONS { .text : { *(.text*) } .data : { *(.data*) } .bss : { *(.bss*) } }",
            "data.o: section '.rodata' is not placed by linker script test.ld",
        ),
        (
            "MEMORY { RAM : ORIGIN = 0x80000000, LENGTH = 0x40 } SECTIONS { .text : { *(.text*) } > RAM \
             .rodata : { *(.rodata*) } > RAM .data : { *(.data*) } > RAM .bss : { *(.bss*) } > RAM }",
            "test.ld:1: section .data overflows MEMORY region RAM by 36 bytes",
        ),
        (
            "SECTIONS { . = 0x80000000; .text : { *(.text*) . = . - 0x1000; } }",
            "test.ld:1: cannot move the location counter backward (from 0x80000",
        ),
        (
            "SECTIONS { x = ORIGIN(RAM); }",
            "test.ld:1: unknown memory region 'RAM'",
        ),
        (
            "SECTIONS { . = 0x80000001; . = ALIGN(3); }",
            "test.ld:1: ALIGN(3): alignment must be power of 2",
        ),
        (
            "SECTIONS { . = ALIGN(0 - 16); }",
            "test.ld:1: ALIGN(18446744073709551600): alignment must be power of 2",
        ),
        (
            "SECTIONS { .text : ALIGN(3) { *(.text*) } }",
            "test.ld:1: ALIGN(3) of section .text: alignment must be power of 2",
        ),
        // A section's alignment is 32 bits in ld.lld on RV64 too.
        (
            "SECTIONS { .text : ALIGN(0x200000000) { *(.text*) } }",
            "test.ld:1: ALIGN(8589934592) of section .text: alignment must be at most 2^31 (ld.lld keeps 32 \
             bits of a section's alignment and would ignore this one)",
        ),
        (
            "SECTIONS { .text : { KEEP(*(.text)) } }",
            "test.ld:1: only '*(...)' input section lists are supported",
        ),
    ];
    for (src, want) in cases {
        let errs = errors(merlin_ld::layout(&norelocs(), &script(src)));
        assert!(
            errs.iter().any(|e| e.starts_with(want)),
            "{src}\n got {errs:?}\nwant {want}"
        );
    }

    // RV32's errors, the 4 GiB address space: on RV64, a link across 4 GiB.
    let all = ".text : { *(.text*) } .rodata : { *(.rodata*) } .data : { *(.data*) } .bss : { *(.bss*) }";
    let linked = merlin_ld::link(
        &norelocs(),
        &script(&format!("SECTIONS {{ . = 0xfffffff0; {all} x = 0x100000000; }}")),
    )
    .unwrap();
    let ours = read_ours(&linked.elf);
    let text = &ours.sections[0];
    assert_eq!(
        (text.name.as_str(), text.addr, text.size),
        (".text", 0xffff_fff0, 0x28)
    );
    assert!(ours.sections[1].addr > 0xffff_ffff);
    assert_eq!(ours.globals["x"], 0x1_0000_0000);
    // The headers (0x1c8 bytes) fit below .text in its page, so they are
    // loaded at the page's start, as ld.lld loads them (write.rs, "The
    // headers").
    assert_eq!((ours.loads[0].offset, ours.loads[0].vaddr), (0, 0xffff_f000));
    assert_eq!((ours.loads[1].offset, ours.loads[1].vaddr), (0xff0, 0xffff_fff0));
    // A section 4 GiB long, and `+` past 2^63 (exact on RV32 and an error
    // there, a 64-bit address here).
    let l = merlin_ld::layout(
        &norelocs(),
        &script(&format!("SECTIONS {{ {all} .s : {{ . += 0x100000000; }} }}")),
    )
    .unwrap();
    assert_eq!(l.section(".s").unwrap().size, 0x1_0000_0000);
    let l = merlin_ld::layout(
        &norelocs(),
        &script(&format!(
            "SECTIONS {{ . = 0x80000000; {all} . += 0x7fffffffffffffff; y = .; }}"
        )),
    )
    .unwrap();
    assert!(l.symbol("y").unwrap() > 0x8000_0000_0000_0000);
}

/// The `--Map` file of an RV64 link: 16-digit addresses and sizes (link.rs's
/// `map_file` has the same lines with 8).
#[test]
fn map_file() {
    let (objs, opts) = case("norelocs/link.ld");
    let linked = merlin_ld::link(&objs, &opts).unwrap();
    let map = merlin_ld::map::map(&linked.layout);
    for want in [
        "Entry point: 0x0000000080000000 (_start)",
        "Section              Address            Size               Align",
        ".text                0x0000000080000000 0x0000000000000028 4",
        "    0x0000000080000020 0x0000000000000008 data.o(.text.helper)",
        "        0x0000000080000020 helper",
        ".bss                 0x0000000080000070 0x0000000000000118 16  (nobits)",
        "    0x0000000080001190 __stack_top",
    ] {
        assert!(map.contains(want), "map lacks {want:?}:\n{map}");
    }
    assert!(!map.contains("(load"), "every section is loaded where it runs");
    let linked = merlin_ld::link(
        &[rv64("a.o", START)],
        &script("SECTIONS { . = 0xffffffff80000000; .text : { *(.text) } .data : { *(.data) } top = .; }"),
    )
    .unwrap();
    let map = merlin_ld::map::map(&linked.layout);
    assert!(
        map.contains(".text                0xffffffff80000000 0x0000000000000010 4"),
        "{map}"
    );
    assert!(map.contains("    0xffffffff80000018 top"), "{map}");
}

/// A higher-half program against ld.lld (rv64/at): linked to run at
/// 0xffffffff80000000 and loaded at 0x8000_0000 by AT, in the form of
/// Linux's riscv vmlinux.lds (`AT(ADDR(.x) - LOAD_OFFSET)`) and with
/// `AT(. - OFFSET)`. Grail loads it at its load addresses and starts it at
/// its entry point's, paging off; the program checks that it runs
/// 0xffffffff00000000 below its link address, with `.dword _start`
/// (R_RISCV_64), `lui %hi(_start)` and `.word _start` (R_RISCV_32, the
/// address being a sign-extended 32-bit one) holding that address.
#[test]
fn higher_half_load_addresses_match_ld_lld() {
    let mut names: Vec<String> = std::fs::read_dir(fixtures().join("at"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "ld"))
        .map(|p| format!("at/{}", p.file_stem().unwrap().to_string_lossy()))
        .collect();
    names.sort();
    assert_eq!(names, ["at/kernel", "at/split"]);
    for name in &names {
        assert_matches_ld_lld(name, "OK\n");
    }
    let types = reloc_types("at/kernel.o");
    for t in [
        "R_RISCV_64",
        "R_RISCV_32",
        "R_RISCV_HI20",
        "R_RISCV_LO12_I",
        "R_RISCV_PCREL_HI20",
    ] {
        assert!(types.contains_key(t), "{t}: {types:?}");
    }
    // Spot checks: (flags, vaddr, paddr) of each PT_LOAD.
    let loads = |name: &str| -> Vec<(String, u64, u64)> {
        expected(name)
            .loads
            .iter()
            .map(|l| (l.flags.clone(), l.vaddr, l.paddr))
            .collect()
    };
    let load = |f: &str, vaddr, paddr| (f.to_string(), vaddr, paddr);
    assert_eq!(
        loads("at/kernel"),
        [
            load("RE", 0xffff_ffff_8000_0000, 0x8000_0000),
            load("R", 0xffff_ffff_8000_1000, 0x8000_1000),
            load("RW", 0xffff_ffff_8000_2000, 0x8000_2000)
        ]
    );
    assert_eq!(expected("at/kernel").entry, 0xffff_ffff_8000_0000);
    // .text2 is 64-aligned: `.` in its AT is its start once aligned.
    assert_eq!(
        loads("at/split"),
        [
            load("RE", 0xffff_ffff_8000_0000, 0x8000_0000),
            load("RE", 0xffff_ffff_8000_0080, 0x8000_0080),
            load("R", 0xffff_ffff_8000_012c, 0x8000_012c),
            load("RW", 0xffff_ffff_8000_0130, 0x8000_0130)
        ]
    );
    // The map gives load addresses 16 digits wide.
    let (objs, opts) = case("at/kernel.ld");
    let linked = merlin_ld::link(&objs, &opts).unwrap();
    let map = merlin_ld::map::map(&linked.layout);
    let want = ".text                0xffffffff80000000 0x0000000000000104 4  (load 0x0000000080000000)";
    assert!(map.contains(want), "map lacks {want:?}:\n{map}");
}

/// ELF64 load addresses wrap modulo 2^64 as addresses do (`AT(. - OFFSET)`
/// is an ordinary higher-half form), but a load range may not run past
/// 2^64: ld.lld writes such a segment (p_paddr + p_memsz wraps), merlin-ld
/// reports it. No 4 GiB limit, which is RV32's.
#[test]
fn rv64_load_address_limits() {
    let objs = norelocs();
    let errs = errors(merlin_ld::layout(
        &objs,
        &script(
            "SECTIONS { . = 0x80000000; .text : AT(0xfffffffffffffff0) { *(.text*) } \
             .rodata : AT(0x1000) { *(.rodata*) } .data : { *(.data*) } .bss : { *(.bss*) } }",
        ),
    ));
    assert_eq!(
        errs,
        ["test.ld:1: section .text load address range extends past the 64-bit address space"]
    );
    // Its end must be below 2^64, as a section's end address must (ld.lld's
    // checkSections): .text (0x28 bytes) may be loaded 8 bytes below the
    // top but not 0x28 below. Past 4 GiB is fine.
    let at = |lma: &str| {
        merlin_ld::layout(
            &objs,
            &script(&format!(
                "SECTIONS {{ . = 0x80000000; .text : AT({lma}) {{ *(.text*) }} \
                 .rodata : AT(0x100000000) {{ *(.rodata*) }} .data : {{ *(.data*) }} .bss : {{ *(.bss*) }} }}"
            )),
        )
    };
    let l = at("0xffffffffffffffd0").unwrap();
    let (text, bss) = (l.section(".text").unwrap(), l.section(".bss").unwrap());
    assert_eq!(text.lma + text.size, 0xffff_ffff_ffff_fff8);
    assert_eq!(bss.lma, bss.addr - 0x8000_0028 + 0x1_0000_0000);
    assert_eq!(
        errors(at("0xffffffffffffffd8")),
        ["test.ld:1: section .text load address range extends past the 64-bit address space"]
    );
    // A negative difference wraps: the higher-half form, with ADDR.
    let l = merlin_ld::layout(
        &objs,
        &script(
            "SECTIONS { . = 0xffffffff80000000; .text : AT(ADDR(.text) - 0xffffffff00000000) { *(.text*) } \
             .rodata : { *(.rodata*) } .data : { *(.data*) } .bss : { *(.bss*) } }",
        ),
    )
    .unwrap();
    assert!(l.sections.iter().all(|s| s.lma == s.addr - 0xffff_ffff_0000_0000));
}

/// link.rs's twin: the members of a library are linked only when needed.
/// rv64/relocs' main.o and relax.o are the program; the library holds a
/// second main.o, norelocs' more.o (which nothing needs) and lib.o.
#[test]
fn library_members_are_linked_only_when_needed() {
    let main = input(&fixtures().join("relocs/main.o"));
    let relax = input(&fixtures().join("relocs/relax.o"));
    let lib = input(&fixtures().join("relocs/lib.o"));
    let library = [
        Input {
            name: "again.o".into(),
            data: main.data.clone(),
        },
        input(&fixtures().join("norelocs/more.o")),
        lib.clone(),
    ];
    let program = [main.clone(), relax.clone()];
    assert_eq!(merlin_ld::library::needed(&program, &library).unwrap(), [2]);
    let (_, opts) = case("relocs/link.ld");
    let linked = merlin_ld::link_with_library(&program, &library, &opts).unwrap();
    let plain = merlin_ld::link(&[main, relax, lib], &opts).unwrap();
    assert_eq!(linked.elf, plain.elf);
    let (_, r, out) = run(&linked.elf);
    assert_eq!((r, out.as_str()), (RunResult::Exited(0), "OK\n"));
    let errs = errors(merlin_ld::link_with_library(&program, &library[..2], &opts));
    assert!(
        errs.iter()
            .any(|e| e.contains("undefined reference to 'lib_double'")),
        "{errs:?}"
    );
}

// ---------------------------------------------------------------------------
// Debug information.

/// link.rs's `with_and_without_lines`, for RV64: two objects with line
/// tables (`.file`/`.loc`) and the same two without.
fn with_and_without_lines() -> (Vec<Input>, Vec<Input>) {
    let a = "\
        .file 0 \"src\" \"a.c\"
        .text
        .globl _start
_start:
        .loc 0 3 0
        call f
        .loc 0 4 5 prologue_end
        j _start
        .data
        .dword 7
";
    let b = "\
        .file 0 \"src\" \"b.c\"
        .file 1 \"src\" \"include/b.h\"
        .text
        nop
        .globl f
f:
        .loc 0 10 0
        addiw a0, a0, 1
        .loc 1 2 0
        ret
";
    let strip = |s: &str| -> String {
        s.lines()
            .filter(|l| !l.trim_start().starts_with(".file") && !l.trim_start().starts_with(".loc"))
            .map(|l| format!("{l}\n"))
            .collect()
    };
    let objs = |srcs: [&str; 2]| -> Vec<Input> {
        srcs.iter().zip(["a.o", "b.o"]).map(|(s, n)| rv64(n, s)).collect()
    };
    (objs([a, b]), objs([&strip(a), &strip(b)]))
}

/// The line tables of RV64 objects, whose addresses are R_RISCV_64 and
/// whose string offsets are R_RISCV_32, are kept and relocated; stripped,
/// the ELF64 file is the link without them, byte for byte.
#[test]
fn line_tables_are_kept_and_relocated() {
    let (debug, plain) = with_and_without_lines();
    let a = merlin_ld::object::parse("a.o", &debug[0].data).unwrap();
    let line = a.sections.iter().find(|s| s.name == ".debug_line").unwrap();
    let types: Vec<String> = line.relocs.iter().map(|r| merlin_ld::reloc::name(r.ty)).collect();
    assert!(
        types.iter().any(|t| t == "R_RISCV_64") && types.iter().any(|t| t == "R_RISCV_32"),
        "{types:?}"
    );

    let opts = Options::default();
    let linked = merlin_ld::link(&debug, &opts).unwrap();
    let without = merlin_ld::link(&plain, &opts).unwrap();
    let elf = elf::Elf::parse(&linked.elf).unwrap();
    assert_eq!(elf.class, Class::Elf64);
    let names: Vec<&str> = elf.sections.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "",
            ".text",
            ".data",
            ".symtab",
            ".strtab",
            ".debug_line",
            ".debug_line_str",
            ".shstrtab"
        ]
    );
    let segments = |file: &[u8]| -> Vec<(u64, Vec<u8>)> {
        let e = elf::Elf::parse(file).unwrap();
        e.segments
            .iter()
            .map(|p| (p.vaddr, e.segment_data(p).unwrap().to_vec()))
            .collect()
    };
    assert_eq!(segments(&linked.elf), segments(&without.elf));
    let text = elf.sections.iter().find(|s| s.name == ".text").unwrap().addr;
    let tables = elf.line_tables().unwrap();
    let rows: Vec<Vec<(u64, &str, u32, bool)>> = tables
        .iter()
        .map(|t| {
            assert_eq!(t.sequences.len(), 1);
            t.sequences[0]
                .rows
                .iter()
                .map(|r| (r.addr - text, t.path(r.file), r.line, r.end_sequence))
                .collect()
        })
        .collect();
    assert_eq!(
        rows,
        [
            vec![
                (0, "src/a.c", 3, false),
                (8, "src/a.c", 4, false),
                (12, "src/a.c", 4, true)
            ],
            vec![
                (16, "src/b.c", 10, false),
                (20, "src/include/b.h", 2, false),
                (24, "src/include/b.h", 2, true)
            ],
        ]
    );
    assert_eq!(merlin_ld::strip_debug(&linked.elf).unwrap(), without.elf);
    assert_eq!(merlin_ld::strip_debug(&without.elf).unwrap(), without.elf);
    // ld.lld puts .debug_* before .comment and .symtab: not merlin-ld's
    // layout, which strip_debug says rather than cutting it wrong.
    let lld = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../elf/tests/llvm/rv64/prog.elf"
    ))
    .unwrap();
    assert_eq!(elf::Elf::parse(&lld).unwrap().class, Class::Elf64);
    assert_eq!(
        merlin_ld::strip_debug(&lld).unwrap_err().to_string(),
        "the debug sections are not where merlin-ld puts them (last)"
    );
}

/// Debug information about code /DISCARD/ threw away points nowhere: 0, for
/// an R_RISCV_64 address (the line table's, with an addend) and an
/// R_RISCV_32 one alike, whatever the addend, as ld.lld writes them.
#[test]
fn debug_information_of_discarded_code_points_nowhere() {
    let src = "        .file 0 \"src\" \"a.c\"
        .text
        .globl _start
_start:
        .loc 0 1 0
        nop
        .section .text.gone, \"ax\"
        nop
gone:
        .loc 0 7 0
        nop
        .section .debug_info, \"\", @progbits
        .word gone + 4
        .dword gone + 4
        .word _start + 4
        .dword _start + 4
";
    let obj = rv64("a.o", src);
    let parsed = merlin_ld::object::parse("a.o", &obj.data).unwrap();
    let line = parsed.sections.iter().find(|s| s.name == ".debug_line").unwrap();
    assert!(
        line.relocs.iter().any(|r| r.addend == 4 && r.ty == 2),
        "an R_RISCV_64 with addend 4: {:?}",
        line.relocs
    );
    let opts = script("SECTIONS { . = 0x1000; .text : { *(.text) } /DISCARD/ : { *(.text.gone) } }");
    let linked = merlin_ld::link(&[obj], &opts).unwrap();
    let elf = elf::Elf::parse(&linked.elf).unwrap();
    let seqs = &elf.line_tables().unwrap()[0].sequences;
    let starts: Vec<(u64, u32)> = seqs.iter().map(|s| (s.rows[0].addr, s.rows[0].line)).collect();
    assert_eq!(starts, [(0x1000, 1), (0, 7)]);
    let info = elf.sections.iter().find(|s| s.name == ".debug_info").unwrap();
    let mut want = Vec::new();
    want.extend_from_slice(&0u32.to_le_bytes());
    want.extend_from_slice(&0u64.to_le_bytes());
    want.extend_from_slice(&0x1004u32.to_le_bytes());
    want.extend_from_slice(&0x1004u64.to_le_bytes());
    assert_eq!(elf.section_data(info).unwrap(), want);
}

/// link.rs's `label_differences_and_sets` on RV64: the same `.data`, and
/// `.dword gone - _start` 0 - 0x8000_0000 in 64 bits, as ld.lld writes it.
#[test]
fn label_differences_and_sets() {
    let ranges = common::assert_diffs(rv64("a.o", common::DIFFS).data);
    assert_eq!(ranges, [0, 0, 0, 0x80, 0xff, 0xff, 0xff, 0xff, 4, 0, 0, 0]);
}

/// link.rs's test of a global defined in discarded code and a weak symbol
/// nobody defines, on RV64: the same tombstones as ld.lld's, 0 in
/// `.debug_info` and 1 in `.debug_ranges` and `.debug_loc`.
#[test]
fn debug_information_of_a_discarded_global_points_nowhere() {
    common::assert_tombstones(
        rv64("a.o", common::TOMBSTONES).data,
        rv64("b.o", common::TOMBSTONES_GONE).data,
    );
}
