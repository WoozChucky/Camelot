//! Writes an [`Object`] as an ELF32 little-endian relocatable file (ET_REL).
//!
//! Not the `elf` crate, which reads ELF files: merlin-as only uses that one
//! in its tests (a dev-dependency), where `elf::` names the crate. In this
//! crate's own code `elf::` is this module (`crate::elf`).
//!
//! Layout:
//!
//! ```text
//! ELF header (52 bytes)
//! section contents (.text, .data, ... then .rela.*, .symtab, .strtab, .shstrtab)
//! section header table
//! ```
//!
//! Section header order: null, user sections, .riscv.attributes, one .rela.X
//! per section that has relocations, .symtab, .strtab, .shstrtab.

use crate::asm::{Object, SectionKind, SymType};

/// The ISA merlin-as assembles, in the psABI's `Tag_RISCV_arch` spelling:
/// exactly what clang writes for `-march=rv32ima_zicsr_zifencei` (M implies
/// Zmmul, A implies Zaamo and Zalrsc). Without it llvm-objdump assumes plain
/// RV32I and prints the M and A instructions as `<unknown>`.
pub const ARCH: &str = "rv32i2p1_m2p0_a2p1_zicsr2p0_zifencei2p0_zmmul1p0_zaamo1p0_zalrsc1p0";

const SHT_RISCV_ATTRIBUTES: u32 = 0x7000_0003;
const SHT_PROGBITS: u32 = 1;
const SHT_SYMTAB: u32 = 2;
const SHT_STRTAB: u32 = 3;
const SHT_RELA: u32 = 4;
const SHT_NOBITS: u32 = 8;
const SHF_WRITE: u32 = 1;
const SHF_ALLOC: u32 = 2;
const SHF_EXECINSTR: u32 = 4;
const SHF_MERGE: u32 = 0x10;
const SHF_STRINGS: u32 = 0x20;
const SHF_INFO_LINK: u32 = 0x40;
const SHN_UNDEF: u16 = 0;
const SHN_ABS: u16 = 0xfff1;
const STB_LOCAL: u8 = 0;
const STB_GLOBAL: u8 = 1;
const STT_NOTYPE: u8 = 0;
const STT_OBJECT: u8 = 1;
const STT_FUNC: u8 = 2;
const STT_SECTION: u8 = 3;
const EM_RISCV: u16 = 243;

/// A string table under construction.
#[derive(Default)]
struct StrTab(Vec<u8>);

impl StrTab {
    fn new() -> Self {
        StrTab(vec![0])
    }
    fn add(&mut self, s: &str) -> u32 {
        if s.is_empty() {
            return 0;
        }
        let off = self.0.len() as u32;
        self.0.extend_from_slice(s.as_bytes());
        self.0.push(0);
        off
    }
}

struct Shdr {
    name: u32,
    ty: u32,
    flags: u32,
    offset: u32,
    size: u32,
    link: u32,
    info: u32,
    align: u32,
    entsize: u32,
}

fn put16(b: &mut Vec<u8>, v: u16) {
    b.extend_from_slice(&v.to_le_bytes());
}
fn put32(b: &mut Vec<u8>, v: u32) {
    b.extend_from_slice(&v.to_le_bytes());
}
fn align(b: &mut Vec<u8>, a: usize) {
    while !b.len().is_multiple_of(a) {
        b.push(0);
    }
}

/// Contents of `.riscv.attributes` (psABI "Attributes"): format version `A`,
/// then one vendor subsection `riscv` holding one `Tag_File` subsubsection
/// with `Tag_RISCV_arch` = [`ARCH`]. Lengths count themselves.
pub fn riscv_attributes() -> Vec<u8> {
    const TAG_FILE: u8 = 1;
    const TAG_RISCV_ARCH: u8 = 5;
    let mut file = vec![TAG_FILE];
    put32(&mut file, (1 + 4 + 1 + ARCH.len() + 1) as u32);
    file.push(TAG_RISCV_ARCH);
    file.extend_from_slice(ARCH.as_bytes());
    file.push(0);

    let mut out = vec![b'A'];
    put32(&mut out, (4 + b"riscv\0".len() + file.len()) as u32);
    out.extend_from_slice(b"riscv\0");
    out.extend_from_slice(&file);
    out
}

/// Debug sections of strings (`SHF_MERGE | SHF_STRINGS`).
fn is_string_section(name: &str) -> bool {
    matches!(name, ".debug_str" | ".debug_line_str")
}

pub fn write(obj: &Object) -> Vec<u8> {
    let mut shstr = StrTab::new();
    let mut strtab = StrTab::new();
    let mut out = vec![0u8; 52]; // header, filled in at the end
    let mut shdrs = vec![Shdr {
        name: 0,
        ty: 0,
        flags: 0,
        offset: 0,
        size: 0,
        link: 0,
        info: 0,
        align: 0,
        entsize: 0,
    }];

    // User sections: header index = position + 1.
    for s in &obj.sections {
        let (ty, flags) = match s.kind {
            SectionKind::Text => (SHT_PROGBITS, SHF_ALLOC | SHF_EXECINSTR),
            SectionKind::Data => (SHT_PROGBITS, SHF_ALLOC | SHF_WRITE),
            SectionKind::Rodata => (SHT_PROGBITS, SHF_ALLOC),
            SectionKind::Bss => (SHT_NOBITS, SHF_ALLOC | SHF_WRITE),
            // Strings a debugger reads by offset, which a linker may merge.
            SectionKind::Debug if is_string_section(&s.name) => (SHT_PROGBITS, SHF_MERGE | SHF_STRINGS),
            SectionKind::Debug => (SHT_PROGBITS, 0),
        };
        let entsize = if flags & SHF_STRINGS != 0 { 1 } else { 0 };
        align(&mut out, s.align.max(1) as usize);
        let offset = out.len() as u32;
        out.extend_from_slice(&s.data);
        shdrs.push(Shdr {
            name: shstr.add(&s.name),
            ty,
            flags,
            offset,
            size: s.size,
            link: 0,
            info: 0,
            align: s.align.max(1),
            entsize,
        });
    }

    let attributes = riscv_attributes();
    shdrs.push(Shdr {
        name: shstr.add(".riscv.attributes"),
        ty: SHT_RISCV_ATTRIBUTES,
        flags: 0,
        offset: out.len() as u32,
        size: attributes.len() as u32,
        link: 0,
        info: 0,
        align: 1,
        entsize: 0,
    });
    out.extend_from_slice(&attributes);

    // Symbol table: null, locals, then globals (ELF requires locals first).
    // Undefined symbols are always global.
    let is_global = |s: &crate::asm::Symbol| s.global || (!s.defined && s.constant.is_none());
    let mut order: Vec<usize> = (0..obj.symbols.len())
        .filter(|&i| !is_global(&obj.symbols[i]))
        .collect();
    let first_global = order.len() + 1;
    order.extend((0..obj.symbols.len()).filter(|&i| is_global(&obj.symbols[i])));
    let mut sym_index = vec![0u32; obj.symbols.len()];
    let mut symtab = vec![0u8; 16];
    for (n, &i) in order.iter().enumerate() {
        let s = &obj.symbols[i];
        sym_index[i] = n as u32 + 1;
        let bind = if is_global(s) { STB_GLOBAL } else { STB_LOCAL };
        let ty = match s.ty {
            SymType::Func => STT_FUNC,
            SymType::Object => STT_OBJECT,
            SymType::NoType => STT_NOTYPE,
            SymType::Section => STT_SECTION,
        };
        let (value, shndx) = match (s.constant, s.section) {
            (Some(c), _) => (c as u32, SHN_ABS),
            (None, Some(sec)) => (s.value, sec as u16 + 1),
            (None, None) => (0, SHN_UNDEF),
        };
        // A section symbol has no name: it stands for its section.
        let name = if s.ty == SymType::Section {
            0
        } else {
            strtab.add(&s.name)
        };
        put32(&mut symtab, name);
        put32(&mut symtab, value);
        put32(&mut symtab, s.size);
        symtab.push((bind << 4) | ty);
        symtab.push(0); // st_other
        put16(&mut symtab, shndx);
    }
    let symtab_index = (shdrs.len() + obj.sections.iter().filter(|s| !s.relocs.is_empty()).count()) as u32;

    // Relocation sections.
    for (i, s) in obj.sections.iter().enumerate() {
        if s.relocs.is_empty() {
            continue;
        }
        align(&mut out, 4);
        let offset = out.len() as u32;
        for r in &s.relocs {
            let sym = sym_index[obj
                .symbols
                .iter()
                .position(|x| x.name == r.symbol)
                .expect("reloc symbol exists")];
            put32(&mut out, r.offset);
            put32(&mut out, (sym << 8) | r.kind.elf_type());
            put32(&mut out, r.addend as i32 as u32);
        }
        shdrs.push(Shdr {
            name: shstr.add(&format!(".rela{}", s.name)),
            ty: SHT_RELA,
            flags: SHF_INFO_LINK,
            offset,
            size: 12 * s.relocs.len() as u32,
            link: symtab_index,
            info: i as u32 + 1,
            align: 4,
            entsize: 12,
        });
    }

    align(&mut out, 4);
    let symtab_name = shstr.add(".symtab");
    shdrs.push(Shdr {
        name: symtab_name,
        ty: SHT_SYMTAB,
        flags: 0,
        offset: out.len() as u32,
        size: symtab.len() as u32,
        link: symtab_index + 1,
        info: first_global as u32,
        align: 4,
        entsize: 16,
    });
    out.extend_from_slice(&symtab);

    let strtab_name = shstr.add(".strtab");
    shdrs.push(Shdr {
        name: strtab_name,
        ty: SHT_STRTAB,
        flags: 0,
        offset: out.len() as u32,
        size: strtab.0.len() as u32,
        link: 0,
        info: 0,
        align: 1,
        entsize: 0,
    });
    out.extend_from_slice(&strtab.0);

    let shstrtab_name = shstr.add(".shstrtab");
    shdrs.push(Shdr {
        name: shstrtab_name,
        ty: SHT_STRTAB,
        flags: 0,
        offset: out.len() as u32,
        size: shstr.0.len() as u32,
        link: 0,
        info: 0,
        align: 1,
        entsize: 0,
    });
    out.extend_from_slice(&shstr.0);

    align(&mut out, 4);
    let shoff = out.len() as u32;
    for h in &shdrs {
        for v in [
            h.name, h.ty, h.flags, 0, h.offset, h.size, h.link, h.info, h.align, h.entsize,
        ] {
            put32(&mut out, v);
        }
    }

    // ELF header.
    let mut hdr = Vec::with_capacity(52);
    hdr.extend_from_slice(b"\x7fELF");
    hdr.extend_from_slice(&[1, 1, 1, 0]); // ELFCLASS32, little-endian, version 1, SysV ABI
    hdr.extend_from_slice(&[0; 8]);
    put16(&mut hdr, 1); // ET_REL
    put16(&mut hdr, EM_RISCV);
    put32(&mut hdr, 1); // e_version
    put32(&mut hdr, 0); // e_entry
    put32(&mut hdr, 0); // e_phoff
    put32(&mut hdr, shoff);
    put32(&mut hdr, 0); // e_flags: soft-float ABI, no RVC
    put16(&mut hdr, 52); // e_ehsize
    put16(&mut hdr, 0); // e_phentsize
    put16(&mut hdr, 0); // e_phnum
    put16(&mut hdr, 40); // e_shentsize
    put16(&mut hdr, shdrs.len() as u16);
    put16(&mut hdr, shdrs.len() as u16 - 1); // e_shstrndx
    out[..52].copy_from_slice(&hdr);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attributes_match_clang() {
        // `llvm-objdump -s -j .riscv.attributes` of examples/hello/hello.S
        // built by clang 21 with -march=rv32ima_zicsr_zifencei.
        let clang = "41540000 00726973 63760001 4a000000 05727633 32693270 315f6d32 70305f61 \
                     3270315f 7a696373 72327030 5f7a6966 656e6365 69327030 5f7a6d6d 756c3170 \
                     305f7a61 616d6f31 70305f7a 616c7273 63317030 00";
        let hex: String = clang.split_whitespace().collect();
        let bytes: Vec<u8> = (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
            .collect();
        assert_eq!(riscv_attributes(), bytes);
    }
}
