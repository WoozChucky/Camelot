//! Writes an [`Object`] as a little-endian relocatable ELF64 file (ET_REL).
//!
//! Not the `elf` crate, which reads ELF files: merlin-as only uses that one
//! in its tests (a dev-dependency), where `elf::` names the crate. In this
//! crate's own code `elf::` is this module (`crate::elf`).
//!
//! Layout:
//!
//! ```text
//! ELF header (64 bytes)
//! section contents (.text, .data, ... then .rela.*, .symtab, .strtab, .shstrtab)
//! section header table
//! ```
//!
//! Section header order: null, the object's sections (the source's, then
//! `.riscv.attributes`, then the line table's if the source did not make
//! them, as clang orders them), one .rela.X per section that has
//! relocations, .symtab, .strtab, .shstrtab. (clang puts `.strtab` first,
//! with the section names in it, and each `.rela.X` after its section; no
//! tool cares, and the tests compare sections, not whole files.)
//!
//! # ELF64
//!
//! The fields that hold an address, an offset or a size are 8 bytes (the
//! gABI's `Elf64_Addr`, `Elf64_Off`, `Elf64_Xword`); the counts, indices and
//! types 4 or 2 bytes. Two records are laid out for that:
//!
//! * A symbol (`Elf64_Sym`) puts `st_info`, `st_other` and `st_shndx` before
//!   `st_value` and `st_size`, so the 8-byte fields are 8-aligned.
//! * A relocation (`Elf64_Rela`) packs the symbol index and the type into
//!   `r_info` as `sym << 32 | type`, and keeps the whole 64-bit addend.
//!
//! The tables of 8-byte fields (`.rela.*`, `.symtab`, the section headers)
//! start at 8-byte offsets, as clang writes them. (Until #159 this writer made
//! ELF32 objects for RV32 too.)

use crate::asm::{Object, SymType};

/// The ISA merlin-as assembles, in the psABI's `Tag_RISCV_arch` spelling:
/// exactly what clang writes for `-march=rv64ima_zicsr_zifencei` (M implies
/// Zmmul, A implies Zaamo and Zalrsc). Without it llvm-objdump assumes plain
/// RV64I and prints the M and A instructions as `<unknown>`.
pub const ARCH: &str = "rv64i2p1_m2p0_a2p1_zicsr2p0_zifencei2p0_zmmul1p0_zaamo1p0_zalrsc1p0";

// The types and flags of the object's sections are crate::section's; these
// are the writer's own sections'.
const SHT_SYMTAB: u32 = 2;
const SHT_STRTAB: u32 = 3;
const SHT_RELA: u32 = 4;
const SHF_INFO_LINK: u64 = 0x40;
const SHN_UNDEF: u16 = 0;
const SHN_ABS: u16 = 0xfff1;
const STB_LOCAL: u8 = 0;
const STB_GLOBAL: u8 = 1;
const STT_NOTYPE: u8 = 0;
const STT_OBJECT: u8 = 1;
const STT_FUNC: u8 = 2;
const STT_SECTION: u8 = 3;
const STT_FILE: u8 = 4;
const EM_RISCV: u16 = 243;
const ELFCLASS64: u8 = 2;

/// Bytes in an address, an offset or a size; also the alignment of the tables
/// made of them.
const WORD: usize = 8;
/// `e_ehsize`: the ELF header.
const EHSIZE: u16 = 64;
/// `e_shentsize`: a section header.
const SHENTSIZE: u16 = 64;
/// A symbol table entry (`Elf64_Sym`).
const SYM: usize = 24;
/// A relocation with addend (`Elf64_Rela`).
const RELA: usize = 24;

/// A string table under construction. A string already in it is not added
/// again: the mapping symbols share a few names (`$d`, `$x<isa>`) among
/// many symbols, which LLVM's string table writes once each too.
#[derive(Default)]
struct StrTab(Vec<u8>, std::collections::HashMap<String, u32>);

impl StrTab {
    fn new() -> Self {
        StrTab(vec![0], Default::default())
    }
    fn add(&mut self, s: &str) -> u32 {
        if s.is_empty() {
            return 0;
        }
        if let Some(&off) = self.1.get(s) {
            return off;
        }
        let off = self.0.len() as u32;
        self.0.extend_from_slice(s.as_bytes());
        self.0.push(0);
        self.1.insert(s.to_string(), off);
        off
    }
}

/// A section header. (`sh_addr` is always 0 in a relocatable file, so it has
/// no field here.)
struct Shdr {
    name: u32,
    ty: u32,
    flags: u64,
    offset: u64,
    size: u64,
    link: u32,
    info: u32,
    align: u64,
    entsize: u64,
}

fn put16(b: &mut Vec<u8>, v: u16) {
    b.extend_from_slice(&v.to_le_bytes());
}
fn put32(b: &mut Vec<u8>, v: u32) {
    b.extend_from_slice(&v.to_le_bytes());
}
fn put64(b: &mut Vec<u8>, v: u64) {
    b.extend_from_slice(&v.to_le_bytes());
}
fn align(b: &mut Vec<u8>, a: usize) {
    while !b.len().is_multiple_of(a) {
        b.push(0);
    }
}

/// Contents of `.riscv.attributes` (psABI "Attributes"): format version `A`,
/// then one vendor subsection `riscv` holding one `Tag_File` subsubsection
/// with `Tag_RISCV_arch` = `arch` ([`ARCH`]). Lengths count themselves.
pub fn riscv_attributes(arch: &str) -> Vec<u8> {
    const TAG_FILE: u8 = 1;
    const TAG_RISCV_ARCH: u8 = 5;
    let mut file = vec![TAG_FILE];
    put32(&mut file, (1 + 4 + 1 + arch.len() + 1) as u32);
    file.push(TAG_RISCV_ARCH);
    file.extend_from_slice(arch.as_bytes());
    file.push(0);

    let mut out = vec![b'A'];
    put32(&mut out, (4 + b"riscv\0".len() + file.len()) as u32);
    out.extend_from_slice(b"riscv\0");
    out.extend_from_slice(&file);
    out
}

pub fn write(obj: &Object) -> Vec<u8> {
    let mut shstr = StrTab::new();
    let mut strtab = StrTab::new();
    let mut out = vec![0u8; EHSIZE.into()]; // header, filled in at the end
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

    // User sections: header index = position + 1, with the type, flags and
    // entry size the assembler gave them (crate::section).
    for s in &obj.sections {
        align(&mut out, s.align.max(1) as usize);
        let offset = out.len() as u64;
        out.extend_from_slice(&s.data);
        shdrs.push(Shdr {
            name: shstr.add(&s.name),
            ty: s.attrs.ty,
            flags: s.attrs.flags,
            offset,
            size: s.size.into(),
            link: 0,
            info: 0,
            align: s.align.max(1).into(),
            entsize: s.attrs.entsize,
        });
    }

    // Symbol table: null, locals, then globals (ELF requires locals first).
    // Undefined symbols are global, but those `.local` names.
    // (`.local` keeps an undefined symbol local, as clang writes it.)
    let is_global = |s: &crate::asm::Symbol| s.global || (!s.defined && s.constant.is_none() && !s.local);
    let mut order: Vec<usize> = (0..obj.symbols.len())
        .filter(|&i| !is_global(&obj.symbols[i]))
        .collect();
    // The first `.file "name"`'s symbol comes before every other local, the
    // others where the source has them: LLVM's ELFObjectWriter ("Make the
    // first STT_FILE precede previous local symbols").
    if let Some(k) = order.iter().position(|&i| obj.symbols[i].ty == SymType::File) {
        let file = order.remove(k);
        order.insert(0, file);
    }
    let first_global = order.len() + 1;
    order.extend((0..obj.symbols.len()).filter(|&i| is_global(&obj.symbols[i])));
    let mut sym_index = vec![0u32; obj.symbols.len()];
    let mut symtab = vec![0u8; SYM];
    for (n, &i) in order.iter().enumerate() {
        let s = &obj.symbols[i];
        sym_index[i] = n as u32 + 1;
        let bind = if is_global(s) { STB_GLOBAL } else { STB_LOCAL };
        let ty = match s.ty {
            SymType::Func => STT_FUNC,
            SymType::Object => STT_OBJECT,
            SymType::NoType | SymType::Mapping => STT_NOTYPE,
            SymType::Section => STT_SECTION,
            SymType::File => STT_FILE,
        };
        // An `.equ` constant is an absolute symbol whose value is the
        // constant, all 64 bits: `.equ K, -1` is 0xffffffffffffffff.
        let (value, shndx) = match (s.constant, s.section) {
            _ if s.ty == SymType::File => (0, SHN_ABS),
            (Some(c), _) => (c as u64, SHN_ABS),
            (None, Some(sec)) => (s.value.into(), sec as u16 + 1),
            (None, None) => (0, SHN_UNDEF),
        };
        // A section symbol has no name: it stands for its section.
        let name = if s.ty == SymType::Section {
            0
        } else {
            strtab.add(&s.name)
        };
        let info = (bind << 4) | ty;
        let other = 0; // st_other: default visibility
                       // Elf64_Sym: st_name, st_info, st_other, st_shndx, st_value, st_size.
        put32(&mut symtab, name);
        symtab.extend_from_slice(&[info, other]);
        put16(&mut symtab, shndx);
        put64(&mut symtab, value);
        put64(&mut symtab, s.size);
    }
    let symtab_index = (shdrs.len() + obj.sections.iter().filter(|s| !s.relocs.is_empty()).count()) as u32;

    // Relocation sections.
    for (i, s) in obj.sections.iter().enumerate() {
        if s.relocs.is_empty() {
            continue;
        }
        align(&mut out, WORD);
        let offset = out.len() as u64;
        for r in &s.relocs {
            let sym = sym_index[obj
                .symbols
                .iter()
                .position(|x| x.name == r.symbol && !matches!(x.ty, SymType::Mapping | SymType::File))
                .expect("reloc symbol exists")];
            let ty = r.kind.elf_type();
            // r_offset, ELF64_R_INFO(sym, type), and the whole 64-bit addend.
            put64(&mut out, r.offset.into());
            put64(&mut out, (u64::from(sym) << 32) | u64::from(ty));
            put64(&mut out, r.addend as u64);
        }
        shdrs.push(Shdr {
            name: shstr.add(&format!(".rela{}", s.name)),
            ty: SHT_RELA,
            flags: SHF_INFO_LINK,
            offset,
            size: (RELA * s.relocs.len()) as u64,
            link: symtab_index,
            info: i as u32 + 1,
            align: WORD as u64,
            entsize: RELA as u64,
        });
    }

    align(&mut out, WORD);
    let symtab_name = shstr.add(".symtab");
    shdrs.push(Shdr {
        name: symtab_name,
        ty: SHT_SYMTAB,
        flags: 0,
        offset: out.len() as u64,
        size: symtab.len() as u64,
        link: symtab_index + 1,
        info: first_global as u32,
        align: WORD as u64,
        entsize: SYM as u64,
    });
    out.extend_from_slice(&symtab);

    let strtab_name = shstr.add(".strtab");
    shdrs.push(Shdr {
        name: strtab_name,
        ty: SHT_STRTAB,
        flags: 0,
        offset: out.len() as u64,
        size: strtab.0.len() as u64,
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
        offset: out.len() as u64,
        size: shstr.0.len() as u64,
        link: 0,
        info: 0,
        align: 1,
        entsize: 0,
    });
    out.extend_from_slice(&shstr.0);

    // Section headers, the address-sized fields (flags, addr, offset, size,
    // addralign, entsize) 8 bytes.
    align(&mut out, WORD);
    let shoff = out.len() as u64;
    for h in &shdrs {
        put32(&mut out, h.name);
        put32(&mut out, h.ty);
        put64(&mut out, h.flags);
        put64(&mut out, 0); // sh_addr
        put64(&mut out, h.offset);
        put64(&mut out, h.size);
        put32(&mut out, h.link);
        put32(&mut out, h.info);
        put64(&mut out, h.align);
        put64(&mut out, h.entsize);
    }

    // ELF header.
    let mut hdr = Vec::with_capacity(EHSIZE.into());
    hdr.extend_from_slice(b"\x7fELF");
    hdr.extend_from_slice(&[ELFCLASS64, 1, 1, 0]); // class, little-endian, version 1, SysV ABI
    hdr.extend_from_slice(&[0; 8]);
    put16(&mut hdr, 1); // ET_REL
    put16(&mut hdr, EM_RISCV);
    put32(&mut hdr, 1); // e_version
    put64(&mut hdr, 0); // e_entry
    put64(&mut hdr, 0); // e_phoff
    put64(&mut hdr, shoff);
    // e_flags: the soft-float ABI (lp64) and no RVC are both 0.
    put32(&mut hdr, 0);
    put16(&mut hdr, EHSIZE);
    put16(&mut hdr, 0); // e_phentsize
    put16(&mut hdr, 0); // e_phnum
    put16(&mut hdr, SHENTSIZE);
    put16(&mut hdr, shdrs.len() as u16);
    put16(&mut hdr, shdrs.len() as u16 - 1); // e_shstrndx
    out[..hdr.len()].copy_from_slice(&hdr);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(hex: &str) -> Vec<u8> {
        let hex: String = hex.split_whitespace().collect();
        (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn attributes_match_clang() {
        // `llvm-objdump -s -j .riscv.attributes` of a `nop` built by clang 23
        // with --target=riscv64 -march=rv64ima_zicsr_zifencei -mabi=lp64.
        let clang = "41540000 00726973 63760001 4a000000 05727636 34693270 315f6d32 70305f61 \
                     3270315f 7a696373 72327030 5f7a6966 656e6365 69327030 5f7a6d6d 756c3170 \
                     305f7a61 616d6f31 70305f7a 616c7273 63317030 00";
        assert_eq!(riscv_attributes(ARCH), bytes(clang));
    }
}
