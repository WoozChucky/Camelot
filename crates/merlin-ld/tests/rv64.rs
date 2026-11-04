//! RV64: merlin-ld reading ELF64 objects (merlin-as's, assembled for
//! `rv64ima_zicsr_zifencei`) and laying them out with 64-bit addresses.
//!
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

use elf::Class;
use excalibur::Xlen;
use merlin_ld::{Input, Options, ScriptSource};

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

fn script(text: &str) -> Options {
    Options {
        script: Some(ScriptSource {
            name: "test.ld".into(),
            text: text.into(),
        }),
        entry: None,
    }
}

fn errors(r: Result<impl std::fmt::Debug, Vec<merlin_ld::Error>>) -> Vec<String> {
    r.expect_err("link should fail")
        .into_iter()
        .map(|e| e.0)
        .collect()
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
            "x.o: section '.text' has sh_addralign 0x100000000, too large (at most 2^32 - 1)".into(),
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

    // -e takes a 64-bit address.
    let opts = Options {
        entry: Some("0xffffffff80000000".into()),
        ..Options::default()
    };
    let layout = merlin_ld::layout(&[rv64("a.o", START)], &opts).unwrap();
    assert_eq!(layout.entry, 0xffff_ffff_8000_0000);
}

/// RV64 objects are read, laid out and relocated, but the ELF64 writer
/// comes in the next step of #149, which replaces this test.
#[test]
fn rv64_executables_are_not_written_yet() {
    assert_eq!(
        errors(merlin_ld::link(&[rv64("a.o", START)], &Options::default())),
        ["ELF64 (RV64) output is not supported yet"]
    );
}
