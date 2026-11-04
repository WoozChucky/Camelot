//! Writes an [`Object`] as a little-endian relocatable ELF file (ET_REL):
//! ELF32 for an RV32 object, ELF64 for an RV64 one ([`Object::xlen`]).
//!
//! Not the `elf` crate, which reads ELF files: merlin-as only uses that one
//! in its tests (a dev-dependency), where `elf::` names the crate. In this
//! crate's own code `elf::` is this module (`crate::elf`).
//!
//! Layout:
//!
//! ```text
//! ELF header (52 or 64 bytes)
//! section contents (.text, .data, ... then .rela.*, .symtab, .strtab, .shstrtab)
//! section header table
//! ```
//!
//! Section header order: null, user sections, .riscv.attributes, one .rela.X
//! per section that has relocations, .symtab, .strtab, .shstrtab. (clang puts
//! them in another order; no tool cares, and the tests compare sections, not
//! whole files.)
//!
//! # The two classes
//!
//! ELF64 is ELF32 with the fields that hold an address, an offset or a size
//! widened to 8 bytes (the gABI's `Elf64_Addr`, `Elf64_Off`, `Elf64_Xword`);
//! the counts, indices and types stay 4 or 2 bytes. So one writer serves both,
//! with [`Class`] saying how wide those fields are, and only two records
//! change shape beyond that:
//!
//! * A symbol (`Elf64_Sym`) puts `st_info`, `st_other` and `st_shndx` before
//!   `st_value` and `st_size`, so the 8-byte fields are 8-aligned.
//! * A relocation (`Elf64_Rela`) packs the symbol index and the type into
//!   `r_info` as `sym << 32 | type`, where ELF32 has `sym << 8 | type`, and
//!   keeps the whole 64-bit addend.
//!
//! The tables of 8-byte fields (`.rela.*`, `.symtab`, the section headers)
//! start at 8-byte offsets in an ELF64 file, as clang writes them.

use crate::asm::{Object, SectionKind, SymType};
use crate::Xlen;

/// The ISA merlin-as assembles for RV32, in the psABI's `Tag_RISCV_arch`
/// spelling: exactly what clang writes for `-march=rv32ima_zicsr_zifencei`
/// (M implies Zmmul, A implies Zaamo and Zalrsc). Without it llvm-objdump
/// assumes plain RV32I and prints the M and A instructions as `<unknown>`.
/// The C assembler, which writes RV32 until #157, takes this one.
pub const ARCH: &str = "rv32i2p1_m2p0_a2p1_zicsr2p0_zifencei2p0_zmmul1p0_zaamo1p0_zalrsc1p0";

/// The same for RV64: what clang writes for `-march=rv64ima_zicsr_zifencei`.
pub const ARCH_RV64: &str = "rv64i2p1_m2p0_a2p1_zicsr2p0_zifencei2p0_zmmul1p0_zaamo1p0_zalrsc1p0";

/// The `Tag_RISCV_arch` string of an object of this width.
pub fn arch(xlen: Xlen) -> &'static str {
    match xlen {
        Xlen::Rv32 => ARCH,
        Xlen::Rv64 => ARCH_RV64,
    }
}

const SHT_RISCV_ATTRIBUTES: u32 = 0x7000_0003;
const SHT_PROGBITS: u32 = 1;
const SHT_SYMTAB: u32 = 2;
const SHT_STRTAB: u32 = 3;
const SHT_RELA: u32 = 4;
const SHT_NOBITS: u32 = 8;
const SHF_WRITE: u64 = 1;
const SHF_ALLOC: u64 = 2;
const SHF_EXECINSTR: u64 = 4;
const SHF_MERGE: u64 = 0x10;
const SHF_STRINGS: u64 = 0x20;
const SHF_INFO_LINK: u64 = 0x40;
const SHN_UNDEF: u16 = 0;
const SHN_ABS: u16 = 0xfff1;
const STB_LOCAL: u8 = 0;
const STB_GLOBAL: u8 = 1;
const STT_NOTYPE: u8 = 0;
const STT_OBJECT: u8 = 1;
const STT_FUNC: u8 = 2;
const STT_SECTION: u8 = 3;
const EM_RISCV: u16 = 243;
const ELFCLASS32: u8 = 1;
const ELFCLASS64: u8 = 2;

/// What differs between ELF32 and ELF64 for this writer: the width of an
/// address, offset or size field (4 or 8 bytes) and the sizes of the records
/// built from them.
struct Class {
    /// `e_ident[EI_CLASS]`.
    ident: u8,
    /// Bytes in an address, an offset or a size; also the alignment of the
    /// tables made of them.
    word: usize,
    /// `e_ehsize`: the ELF header.
    ehsize: u16,
    /// `e_shentsize`: a section header.
    shentsize: u16,
    /// A symbol table entry (`Elf32_Sym` / `Elf64_Sym`).
    sym: usize,
    /// A relocation with addend (`Elf32_Rela` / `Elf64_Rela`).
    rela: usize,
}

const ELF32: Class = Class {
    ident: ELFCLASS32,
    word: 4,
    ehsize: 52,
    shentsize: 40,
    sym: 16,
    rela: 12,
};

const ELF64: Class = Class {
    ident: ELFCLASS64,
    word: 8,
    ehsize: 64,
    shentsize: 64,
    sym: 24,
    rela: 24,
};

impl Class {
    fn of(xlen: Xlen) -> &'static Class {
        match xlen {
            Xlen::Rv32 => &ELF32,
            Xlen::Rv64 => &ELF64,
        }
    }

    /// Appends an address, offset or size field. In ELF32 the value must fit
    /// in 4 bytes: everything this writer puts there does (file offsets and
    /// section sizes are 32-bit in [`Object`], and an RV32 constant is cut to
    /// 32 bits before it gets here).
    fn put_word(&self, b: &mut Vec<u8>, v: u64) {
        if self.word == 4 {
            debug_assert!(u32::try_from(v).is_ok(), "{v:#x} in a 4-byte ELF32 field");
            put32(b, v as u32);
        } else {
            put64(b, v);
        }
    }
}

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

/// A section header, its fields as wide as ELF64's; [`Class::put_word`]
/// narrows the address-sized ones for ELF32. (`sh_addr` is always 0 in a
/// relocatable file, so it has no field here.)
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
/// with `Tag_RISCV_arch` = `arch` ([`ARCH`] or [`ARCH_RV64`], the same length,
/// so the two sections differ only in "32" / "64"). Lengths count themselves.
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

/// Debug sections of strings (`SHF_MERGE | SHF_STRINGS`).
fn is_string_section(name: &str) -> bool {
    matches!(name, ".debug_str" | ".debug_line_str")
}

pub fn write(obj: &Object) -> Vec<u8> {
    let class = Class::of(obj.xlen);
    let mut shstr = StrTab::new();
    let mut strtab = StrTab::new();
    let mut out = vec![0u8; class.ehsize.into()]; // header, filled in at the end
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
        let offset = out.len() as u64;
        out.extend_from_slice(&s.data);
        shdrs.push(Shdr {
            name: shstr.add(&s.name),
            ty,
            flags,
            offset,
            size: s.size.into(),
            link: 0,
            info: 0,
            align: s.align.max(1).into(),
            entsize,
        });
    }

    let attributes = riscv_attributes(arch(obj.xlen));
    shdrs.push(Shdr {
        name: shstr.add(".riscv.attributes"),
        ty: SHT_RISCV_ATTRIBUTES,
        flags: 0,
        offset: out.len() as u64,
        size: attributes.len() as u64,
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
    let mut symtab = vec![0u8; class.sym];
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
        // An `.equ` constant is an absolute symbol whose value is the
        // constant: cut to 32 bits in ELF32 (as LLVM does on riscv32), whole
        // in ELF64, so `.equ K, -1` is 0xffffffff or 0xffffffffffffffff.
        let (value, shndx) = match (s.constant, s.section) {
            (Some(c), _) => match obj.xlen {
                Xlen::Rv32 => (u64::from(c as u32), SHN_ABS),
                Xlen::Rv64 => (c as u64, SHN_ABS),
            },
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
        match obj.xlen {
            // Elf32_Sym: st_name, st_value, st_size, st_info, st_other, st_shndx.
            Xlen::Rv32 => {
                put32(&mut symtab, name);
                class.put_word(&mut symtab, value);
                class.put_word(&mut symtab, s.size.into());
                symtab.extend_from_slice(&[info, other]);
                put16(&mut symtab, shndx);
            }
            // Elf64_Sym: st_name, st_info, st_other, st_shndx, st_value, st_size.
            Xlen::Rv64 => {
                put32(&mut symtab, name);
                symtab.extend_from_slice(&[info, other]);
                put16(&mut symtab, shndx);
                class.put_word(&mut symtab, value);
                class.put_word(&mut symtab, s.size.into());
            }
        }
    }
    let symtab_index = (shdrs.len() + obj.sections.iter().filter(|s| !s.relocs.is_empty()).count()) as u32;

    // Relocation sections.
    for (i, s) in obj.sections.iter().enumerate() {
        if s.relocs.is_empty() {
            continue;
        }
        align(&mut out, class.word);
        let offset = out.len() as u64;
        for r in &s.relocs {
            let sym = sym_index[obj
                .symbols
                .iter()
                .position(|x| x.name == r.symbol)
                .expect("reloc symbol exists")];
            let ty = r.kind.elf_type();
            class.put_word(&mut out, r.offset.into());
            match obj.xlen {
                // ELF32_R_INFO(sym, type); the addend is a 32-bit field, and
                // an RV32 address wraps at 32 bits, so cutting it changes
                // nothing the linker computes (LLVM writes the same).
                Xlen::Rv32 => {
                    put32(&mut out, (sym << 8) | ty);
                    put32(&mut out, r.addend as i32 as u32);
                }
                // ELF64_R_INFO(sym, type), and the whole 64-bit addend.
                Xlen::Rv64 => {
                    put64(&mut out, (u64::from(sym) << 32) | u64::from(ty));
                    put64(&mut out, r.addend as u64);
                }
            }
        }
        shdrs.push(Shdr {
            name: shstr.add(&format!(".rela{}", s.name)),
            ty: SHT_RELA,
            flags: SHF_INFO_LINK,
            offset,
            size: (class.rela * s.relocs.len()) as u64,
            link: symtab_index,
            info: i as u32 + 1,
            align: class.word as u64,
            entsize: class.rela as u64,
        });
    }

    align(&mut out, class.word);
    let symtab_name = shstr.add(".symtab");
    shdrs.push(Shdr {
        name: symtab_name,
        ty: SHT_SYMTAB,
        flags: 0,
        offset: out.len() as u64,
        size: symtab.len() as u64,
        link: symtab_index + 1,
        info: first_global as u32,
        align: class.word as u64,
        entsize: class.sym as u64,
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

    // Section headers: the same fields in the same order in both classes,
    // the address-sized ones (flags, addr, offset, size, addralign, entsize)
    // 4 or 8 bytes.
    align(&mut out, class.word);
    let shoff = out.len() as u64;
    for h in &shdrs {
        put32(&mut out, h.name);
        put32(&mut out, h.ty);
        class.put_word(&mut out, h.flags);
        class.put_word(&mut out, 0); // sh_addr
        class.put_word(&mut out, h.offset);
        class.put_word(&mut out, h.size);
        put32(&mut out, h.link);
        put32(&mut out, h.info);
        class.put_word(&mut out, h.align);
        class.put_word(&mut out, h.entsize);
    }

    // ELF header.
    let mut hdr = Vec::with_capacity(class.ehsize.into());
    hdr.extend_from_slice(b"\x7fELF");
    hdr.extend_from_slice(&[class.ident, 1, 1, 0]); // class, little-endian, version 1, SysV ABI
    hdr.extend_from_slice(&[0; 8]);
    put16(&mut hdr, 1); // ET_REL
    put16(&mut hdr, EM_RISCV);
    put32(&mut hdr, 1); // e_version
    class.put_word(&mut hdr, 0); // e_entry
    class.put_word(&mut hdr, 0); // e_phoff
    class.put_word(&mut hdr, shoff);
    // e_flags: the soft-float ABI (ilp32 or lp64) and no RVC are both 0.
    put32(&mut hdr, 0);
    put16(&mut hdr, class.ehsize);
    put16(&mut hdr, 0); // e_phentsize
    put16(&mut hdr, 0); // e_phnum
    put16(&mut hdr, class.shentsize);
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
        // `llvm-objdump -s -j .riscv.attributes` of examples/hello/hello.S
        // built by clang 21 with -march=rv32ima_zicsr_zifencei.
        let clang = "41540000 00726973 63760001 4a000000 05727633 32693270 315f6d32 70305f61 \
                     3270315f 7a696373 72327030 5f7a6966 656e6365 69327030 5f7a6d6d 756c3170 \
                     305f7a61 616d6f31 70305f7a 616c7273 63317030 00";
        assert_eq!(riscv_attributes(ARCH), bytes(clang));
        assert_eq!(arch(Xlen::Rv32), ARCH);
    }

    #[test]
    fn rv64_attributes_match_clang() {
        // `llvm-objdump -s -j .riscv.attributes` of a `nop` built by clang 23
        // with --target=riscv64 -march=rv64ima_zicsr_zifencei -mabi=lp64.
        let clang = "41540000 00726973 63760001 4a000000 05727636 34693270 315f6d32 70305f61 \
                     3270315f 7a696373 72327030 5f7a6966 656e6365 69327030 5f7a6d6d 756c3170 \
                     305f7a61 616d6f31 70305f7a 616c7273 63317030 00";
        assert_eq!(riscv_attributes(ARCH_RV64), bytes(clang));
        assert_eq!(arch(Xlen::Rv64), ARCH_RV64);
    }
}
