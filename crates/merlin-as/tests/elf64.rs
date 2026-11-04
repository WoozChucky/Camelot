//! An RV64 object read back with the `elf` crate, field by field, against
//! what clang writes for the same source.
//!
//! merlin-as lays out its sections in its own order (clang puts `.strtab`
//! first and each `.rela.X` after its X) and writes no `$x`/`$d` mapping
//! symbols, so the files differ as wholes; everything else here is clang's.
//! The expectations come from `llvm-readelf -h -S -s -r` and
//! `llvm-objdump -s` of the source assembled by clang 23 with
//!
//! ```text
//! clang --target=riscv64 -march=rv64ima_zicsr_zifencei -mabi=lp64 -mno-relax -c
//! ```

use merlin_as::{Options, Xlen};

const RV64: Options = Options { xlen: Xlen::Rv64 };

/// Code with pc-relative and call relocations, data with 64-bit addresses
/// and addends (and a 32-bit one), and two absolute constants whose 64-bit
/// values a 32-bit symbol would cut.
const SRC: &str = "
    .equ K, -1
    .equ N, -0x80000000
    .globl f
    .text
f:
    call ext
    la a0, var
    ld a1, 0(a0)
    ret
    .data
    .p2align 3
var:
    .dword ext+0x100000000
    .dword ext-1
    .word ext
    .word 0
    .dword f
";

fn hex(s: &str) -> Vec<u8> {
    let s: String = s.split_whitespace().collect();
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

fn object(src: &str) -> Vec<u8> {
    merlin_as::assemble_to_elf_with(src, &RV64).expect("assembles")
}

fn section<'a>(file: &'a elf::Elf, name: &str) -> &'a elf::Section {
    file.sections
        .iter()
        .find(|s| s.name == name)
        .unwrap_or_else(|| panic!("no section {name}"))
}

#[test]
fn the_header_is_elf64() {
    let bytes = object(SRC);
    let file = elf::Elf::parse(&bytes).expect("parses");
    assert_eq!(file.class, elf::Class::Elf64);
    // ELFCLASS64, little-endian, version 1, System V ABI, no padding bytes set.
    assert_eq!(bytes[..16], hex("7f454c46 02010100 00000000 00000000"));
    let h = &file.header;
    assert_eq!((h.ty, h.machine, h.version), (elf::ET_REL, elf::EM_RISCV, 1));
    assert_eq!((h.entry, h.phoff, h.phentsize, h.phnum), (0, 0, 0, 0));
    // lp64 (soft float) and no compressed instructions: no flag set.
    assert_eq!(h.flags, 0);
    assert_eq!((h.ehsize, h.shentsize), (64, 64));
    assert_eq!(h.shoff % 8, 0, "the section headers are 8-aligned");
    assert_eq!(file.sections[h.shstrndx as usize].name, ".shstrtab");
}

#[test]
fn sections_are_clangs() {
    let bytes = object(SRC);
    let file = elf::Elf::parse(&bytes).expect("parses");
    const AX: u64 = elf::SHF_ALLOC | elf::SHF_EXECINSTR;
    const WA: u64 = elf::SHF_WRITE | elf::SHF_ALLOC;
    // name, type, flags, size, addralign, entsize (llvm-readelf -S).
    let expected: [(&str, u32, u64, u64, u64, u64); 6] = [
        (".text", elf::SHT_PROGBITS, AX, 0x18, 4, 0),
        (".data", elf::SHT_PROGBITS, WA, 0x20, 8, 0),
        (".riscv.attributes", 0x7000_0003, 0, 0x55, 1, 0),
        (".rela.text", elf::SHT_RELA, elf::SHF_INFO_LINK, 0x48, 8, 0x18),
        (".rela.data", elf::SHT_RELA, elf::SHF_INFO_LINK, 0x60, 8, 0x18),
        (".symtab", elf::SHT_SYMTAB, 0, 6 * 0x18 + 0x18, 8, 0x18),
    ];
    for (name, ty, flags, size, align, entsize) in expected {
        let s = section(&file, name);
        assert_eq!(
            (s.ty, s.flags, s.size, s.align, s.entsize),
            (ty, flags, size, align, entsize),
            "{name}"
        );
        assert_eq!(s.offset % align, 0, "{name} at {:#x}", s.offset);
    }
    // A relocation section links to the symbol table and names the section
    // it applies to.
    let symtab = file.symtab_index().unwrap() as u32;
    for (rela, target) in [(".rela.text", ".text"), (".rela.data", ".data")] {
        let s = section(&file, rela);
        assert_eq!(s.link, symtab, "{rela}");
        assert_eq!(file.sections[s.info as usize].name, target, "{rela}");
    }

    // The contents (llvm-objdump -s); .data is all relocations.
    let text = hex("97000000 e7800000 17050000 13050500 83350500 67800000");
    assert_eq!(file.section_data(section(&file, ".text")).unwrap(), text);
    assert_eq!(file.section_data(section(&file, ".data")).unwrap(), [0; 0x20]);
    let attributes = file.section_data(section(&file, ".riscv.attributes")).unwrap();
    assert_eq!(
        attributes,
        merlin_as::elf::riscv_attributes(merlin_as::elf::ARCH_RV64)
    );
}

#[test]
fn symbols_are_clangs() {
    let bytes = object(SRC);
    let file = elf::Elf::parse(&bytes).expect("parses");
    let symbols = file.symbols().unwrap();
    let where_ = |s: &elf::Symbol| match s.shndx {
        elf::SHN_ABS => "ABS".to_string(),
        elf::SHN_UNDEF => "UND".to_string(),
        i => file.sections[i as usize].name.clone(),
    };
    let ours: Vec<(&str, u64, u64, u8, u8, String)> = symbols[1..]
        .iter()
        .map(|s| (s.name.as_str(), s.value, s.size, s.bind(), s.ty(), where_(s)))
        .collect();
    // llvm-readelf -s, without clang's mapping symbols ($xrv64i2p1_..., $d).
    // The constants are their whole 64-bit values, sign-extended: an ELF32
    // object holds 0xffffffff and 0x80000000.
    use elf::{STB_GLOBAL as GLOBAL, STB_LOCAL as LOCAL, STT_NOTYPE as NOTYPE};
    let clang = [
        ("K", 0xffff_ffff_ffff_ffff, 0, LOCAL, NOTYPE, "ABS"),
        ("N", 0xffff_ffff_8000_0000, 0, LOCAL, NOTYPE, "ABS"),
        (".Lpcrel_hi0", 8, 0, LOCAL, NOTYPE, ".text"),
        ("var", 0, 0, LOCAL, NOTYPE, ".data"),
        ("f", 0, 0, GLOBAL, NOTYPE, ".text"),
        ("ext", 0, 0, GLOBAL, NOTYPE, "UND"),
    ];
    let clang: Vec<_> = clang
        .iter()
        .map(|&(n, v, z, b, t, w)| (n, v, z, b, t, w.to_string()))
        .collect();
    assert_eq!(ours, clang);
    // sh_info: the index of the first global, after the null symbol and the
    // four locals.
    assert_eq!(section(&file, ".symtab").info, 5);

    // The raw Elf64_Sym of K: st_name, then st_info, st_other, st_shndx
    // (SHN_ABS), then the 8-byte st_value and st_size.
    let symtab = file.section_data(section(&file, ".symtab")).unwrap();
    let k = &symtab[0x18..0x30];
    assert_eq!(k[4..], hex("0000f1ff ffffffff ffffffff 00000000 00000000"));
}

#[test]
fn relocations_are_clangs() {
    let bytes = object(SRC);
    let file = elf::Elf::parse(&bytes).expect("parses");
    let symbols = file.symbols().unwrap();
    let relocs = |name: &str| -> Vec<(u64, &str, String, i64)> {
        file.relocations(section(&file, name))
            .unwrap()
            .iter()
            .map(|r| {
                let ty = elf::reloc_name(r.ty).unwrap();
                (r.offset, ty, symbols[r.sym as usize].name.clone(), r.addend)
            })
            .collect()
    };
    let expected = |list: &[(u64, &'static str, &str, i64)]| -> Vec<(u64, &'static str, String, i64)> {
        list.iter()
            .map(|&(o, t, s, a)| (o, t, s.to_string(), a))
            .collect()
    };
    // llvm-readelf -r.
    assert_eq!(
        relocs(".rela.text"),
        expected(&[
            (0x0, "R_RISCV_CALL_PLT", "ext", 0),
            (0x8, "R_RISCV_PCREL_HI20", "var", 0),
            (0xc, "R_RISCV_PCREL_LO12_I", ".Lpcrel_hi0", 0),
        ])
    );
    // A 64-bit address with a 64-bit addend, which an ELF32 Rela would cut
    // to 32 bits; a .word stays R_RISCV_32 on RV64.
    assert_eq!(
        relocs(".rela.data"),
        expected(&[
            (0x00, "R_RISCV_64", "ext", 0x1_0000_0000),
            (0x08, "R_RISCV_64", "ext", -1),
            (0x10, "R_RISCV_32", "ext", 0),
            (0x18, "R_RISCV_64", "f", 0),
        ])
    );

    // The raw Elf64_Rela of `.dword ext+0x100000000`: r_offset, r_info =
    // ext's index << 32 | R_RISCV_64 (2), r_addend.
    let ext = symbols.iter().position(|s| s.name == "ext").unwrap() as u64;
    let data = file.section_data(section(&file, ".rela.data")).unwrap();
    let field = |at: usize| u64::from_le_bytes(data[at..at + 8].try_into().unwrap());
    assert_eq!(field(0), 0);
    assert_eq!(field(8), ext << 32 | 2);
    assert_eq!(field(16), 0x1_0000_0000);
}

/// The line table of an RV64 object: an 8-byte address size and
/// `DW_LNE_set_address`, which gets an R_RISCV_64; the string offsets stay
/// R_RISCV_32 (DWARF32). clang relocates the address against a `.L0` label
/// at the start of `.text`, merlin-as against `.text`'s section symbol: the
/// same place.
#[test]
fn line_tables_have_8_byte_addresses() {
    let src = "
    .file 0 \"/src\" \"t.c\"
    .text
    .loc 0 3 5
    nop
    .loc 0 4 1 prologue_end
    nop
";
    let bytes = object(src);
    let file = elf::Elf::parse(&bytes).expect("parses");
    let line = file.section_data(section(&file, ".debug_line")).unwrap();
    let clang = "46000000 05000800 25000000 010101fb 0e0d0001 01010100 00000100 00010101 \
                 1f010000 00000201 1f020f01 00000000 00040005 05000902 00000000 00000000 \
                 1405010a 4b020400 0101";
    assert_eq!(line, hex(clang));
    let symbols = file.symbols().unwrap();
    let relocs: Vec<(u64, &str, &str, i64)> = file
        .relocations(section(&file, ".rela.debug_line"))
        .unwrap()
        .iter()
        .map(|r| {
            let s = &symbols[r.sym as usize];
            assert_eq!(s.ty(), elf::STT_SECTION);
            let target = file.sections[s.shndx as usize].name.as_str();
            (r.offset, elf::reloc_name(r.ty).unwrap(), target, r.addend)
        })
        .collect();
    assert_eq!(
        relocs,
        [
            (0x22, "R_RISCV_32", ".debug_line_str", 0),
            (0x2c, "R_RISCV_32", ".debug_line_str", 5),
            (0x38, "R_RISCV_64", ".text", 0),
        ]
    );
    // The elf crate reads it back (llvm-dwarfdump --debug-line prints the
    // same rows for clang's object): lines 3 and 4 at 0 and 4, the end at 8.
    let tables = file.line_tables().expect("line tables");
    let seq = &tables[0].sequences[0];
    assert_eq!(file.sections[seq.section.unwrap()].name, ".text");
    let rows: Vec<(u64, u32, u32, bool)> = seq
        .rows
        .iter()
        .map(|r| (r.addr, r.line, r.column, r.end_sequence))
        .collect();
    assert_eq!(rows, [(0, 3, 5, false), (4, 4, 1, false), (8, 4, 1, true)]);
}
