//! ELF reader for little-endian RISC-V files of either class, ELF32 (RV32)
//! and ELF64 (RV64): the ELF header, program headers, section headers,
//! symbols and relocations, and the DWARF line tables of `.debug_line`
//! ([`lines`]), and the entries of `.debug_info` ([`units`], [`dies`]).
//!
//! The two classes hold the same records. ELF64 widens addresses, offsets and
//! sizes to 8 bytes and moves a few narrow fields so that the wide ones stay
//! naturally aligned. [`Elf::parse`] takes the class from `e_ident[EI_CLASS]`
//! and reads either layout into the same types, whose fields are wide enough
//! for both (`u64`, an `i64` addend); [`Elf::class`] says which one it was.
//!
//! Shared by Grail (loads executables), merlin-objdump (prints everything),
//! merlin-ld (reads `.o` files and archives) and merlin-dbg. It reads ELF
//! files, and writes no executable: merlin-as and merlin-ld write theirs. [`archive`]
//! reads `ar` archives and writes them too, as `cargo xtask fs` writes the
//! root image's `libc.a`, and [`strip`] writes an object without its debug
//! information, as `llvm-strip -g` does. Every offset taken from the file
//! is bounds checked, so a truncated or corrupt file gives an [`Error`],
//! never a panic.

use std::fmt;

pub mod archive;
pub mod dies;
pub mod lines;
pub mod strip;
pub mod units;

// e_type
pub const ET_NONE: u16 = 0;
pub const ET_REL: u16 = 1;
pub const ET_EXEC: u16 = 2;
pub const ET_DYN: u16 = 3;

pub const EM_RISCV: u16 = 243;

// e_ident[EI_CLASS] and e_ident[EI_DATA].
pub const ELFCLASS32: u8 = 1;
pub const ELFCLASS64: u8 = 2;
pub const ELFDATA2LSB: u8 = 1;

// Section types.
pub const SHT_NULL: u32 = 0;
pub const SHT_PROGBITS: u32 = 1;
pub const SHT_SYMTAB: u32 = 2;
pub const SHT_STRTAB: u32 = 3;
pub const SHT_RELA: u32 = 4;
pub const SHT_NOBITS: u32 = 8;
pub const SHT_REL: u32 = 9;
pub const SHT_GROUP: u32 = 17;
pub const SHT_SYMTAB_SHNDX: u32 = 18;
/// `.riscv.attributes` (the psABI's build attributes: `Tag_RISCV_arch`, ...).
pub const SHT_RISCV_ATTRIBUTES: u32 = 0x7000_0003;

// Section flags (`sh_flags` is 4 bytes in ELF32 and 8 in ELF64).
pub const SHF_WRITE: u64 = 0x1;
pub const SHF_ALLOC: u64 = 0x2;
pub const SHF_EXECINSTR: u64 = 0x4;
pub const SHF_MERGE: u64 = 0x10;
pub const SHF_STRINGS: u64 = 0x20;
pub const SHF_INFO_LINK: u64 = 0x40;
pub const SHF_GROUP: u64 = 0x200;
pub const SHF_TLS: u64 = 0x400;

// Special section indices.
pub const SHN_UNDEF: u16 = 0;
pub const SHN_LORESERVE: u16 = 0xff00;
pub const SHN_ABS: u16 = 0xfff1;
pub const SHN_COMMON: u16 = 0xfff2;

// Program header types and flags.
pub const PT_NULL: u32 = 0;
pub const PT_LOAD: u32 = 1;
pub const PT_DYNAMIC: u32 = 2;
pub const PT_INTERP: u32 = 3;
pub const PT_NOTE: u32 = 4;
pub const PT_PHDR: u32 = 6;
pub const PT_TLS: u32 = 7;
pub const PT_GNU_EH_FRAME: u32 = 0x6474_e550;
pub const PT_GNU_STACK: u32 = 0x6474_e551;
pub const PT_GNU_RELRO: u32 = 0x6474_e552;
pub const PT_RISCV_ATTRIBUTES: u32 = 0x7000_0003;
pub const PF_X: u32 = 1;
pub const PF_W: u32 = 2;
pub const PF_R: u32 = 4;

// Symbol binding and type (st_info = bind << 4 | type).
pub const STB_LOCAL: u8 = 0;
pub const STB_GLOBAL: u8 = 1;
pub const STB_WEAK: u8 = 2;
pub const STT_NOTYPE: u8 = 0;
pub const STT_OBJECT: u8 = 1;
pub const STT_FUNC: u8 = 2;
pub const STT_SECTION: u8 = 3;
pub const STT_FILE: u8 = 4;
pub const STT_COMMON: u8 = 5;
pub const STT_TLS: u8 = 6;
pub const STT_GNU_IFUNC: u8 = 10;

/// Sizes of the fixed-layout records of ELF32 (the ones merlin-as and
/// merlin-ld write).
pub const EHDR_SIZE: usize = 52;
pub const PHDR_SIZE: usize = 32;
pub const SHDR_SIZE: usize = 40;
pub const SYM_SIZE: usize = 16;
pub const RELA_SIZE: usize = 12;
pub const REL_SIZE: usize = 8;

/// Sizes of the fixed-layout records of ELF64.
pub const EHDR64_SIZE: usize = 64;
pub const PHDR64_SIZE: usize = 56;
pub const SHDR64_SIZE: usize = 64;
pub const SYM64_SIZE: usize = 24;
pub const RELA64_SIZE: usize = 24;
pub const REL64_SIZE: usize = 16;

/// The class of a file, from `e_ident[EI_CLASS]`: the size of its addresses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    Elf32,
    Elf64,
}

impl Class {
    /// The size of an address, an offset or a size in the file's records, and
    /// of a pointer in its program: 4 or 8 bytes.
    pub fn addr_size(self) -> usize {
        self.pick(4, 8)
    }
    pub fn ehdr_size(self) -> usize {
        self.pick(EHDR_SIZE, EHDR64_SIZE)
    }
    pub fn phdr_size(self) -> usize {
        self.pick(PHDR_SIZE, PHDR64_SIZE)
    }
    pub fn shdr_size(self) -> usize {
        self.pick(SHDR_SIZE, SHDR64_SIZE)
    }
    pub fn sym_size(self) -> usize {
        self.pick(SYM_SIZE, SYM64_SIZE)
    }
    pub fn rela_size(self) -> usize {
        self.pick(RELA_SIZE, RELA64_SIZE)
    }
    pub fn rel_size(self) -> usize {
        self.pick(REL_SIZE, REL64_SIZE)
    }
    fn pick(self, elf32: usize, elf64: usize) -> usize {
        match self {
            Class::Elf32 => elf32,
            Class::Elf64 => elf64,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error(pub String);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "bad ELF: {}", self.0)
    }
}

impl std::error::Error for Error {}

pub(crate) fn err<T>(msg: impl Into<String>) -> Result<T, Error> {
    Err(Error(msg.into()))
}

/// The ELF file header, minus `e_ident` (which [`Elf::parse`] checks; its
/// class is [`Elf::class`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    pub ty: u16,
    pub machine: u16,
    pub version: u32,
    pub entry: u64,
    pub phoff: u64,
    pub shoff: u64,
    pub flags: u32,
    pub ehsize: u16,
    pub phentsize: u16,
    pub phnum: u16,
    pub shentsize: u16,
    pub shnum: u16,
    pub shstrndx: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgramHeader {
    pub ty: u32,
    pub offset: u64,
    pub vaddr: u64,
    pub paddr: u64,
    pub filesz: u64,
    pub memsz: u64,
    pub flags: u32,
    pub align: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    /// Name from the section header string table (empty for the null section).
    pub name: String,
    pub ty: u32,
    pub flags: u64,
    pub addr: u64,
    pub offset: u64,
    pub size: u64,
    pub link: u32,
    pub info: u32,
    pub align: u64,
    pub entsize: u64,
}

impl Section {
    pub fn is_alloc(&self) -> bool {
        self.flags & SHF_ALLOC != 0
    }
    pub fn is_exec(&self) -> bool {
        self.flags & SHF_EXECINSTR != 0
    }
    pub fn is_nobits(&self) -> bool {
        self.ty == SHT_NOBITS
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Symbol {
    pub name: String,
    pub value: u64,
    pub size: u64,
    pub info: u8,
    pub other: u8,
    /// Section index, or one of `SHN_UNDEF`, `SHN_ABS`, `SHN_COMMON`.
    pub shndx: u16,
}

impl Symbol {
    pub fn bind(&self) -> u8 {
        self.info >> 4
    }
    pub fn ty(&self) -> u8 {
        self.info & 0xf
    }
    pub fn is_undefined(&self) -> bool {
        self.shndx == SHN_UNDEF
    }
    /// The section this symbol is defined in, if it is an ordinary section index.
    pub fn section(&self) -> Option<usize> {
        (self.shndx != SHN_UNDEF && self.shndx < SHN_LORESERVE).then_some(self.shndx as usize)
    }
}

/// One relocation. `SHT_REL` entries (no explicit addend) read as addend 0.
///
/// `r_info` holds the symbol index and the type: `sym << 8 | type` in ELF32
/// (`ELF32_R_SYM`, `ELF32_R_TYPE`), `sym << 32 | type` in ELF64.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rela {
    /// Offset within the target section (`.o`) or virtual address (executables).
    pub offset: u64,
    /// Index into the symbol table named by the relocation section's `sh_link`.
    pub sym: u32,
    pub ty: u32,
    pub addend: i64,
}

/// A parsed ELF file. Section, segment and symbol contents stay in the
/// borrowed buffer and are read on demand.
pub struct Elf<'a> {
    pub data: &'a [u8],
    /// ELF32 or ELF64, from `e_ident[EI_CLASS]`.
    pub class: Class,
    pub header: Header,
    pub segments: Vec<ProgramHeader>,
    pub sections: Vec<Section>,
}

/// The error of a read past the end of the file: which bytes it needed,
/// `truncated file (needs bytes 0x18c..0x1b4)` (as merlin-ld and the C
/// linker on Camelot word it). The end is computed in 128 bits, so it is the
/// true one even when `off + len` does not fit a `usize`.
fn truncated(off: usize, len: usize) -> Error {
    let end = off as u128 + len as u128;
    Error(format!("truncated file (needs bytes {off:#x}..{end:#x})"))
}

fn u16_at(data: &[u8], off: usize) -> Result<u16, Error> {
    let b = bytes_at(data, off, 2)?;
    Ok(u16::from_le_bytes([b[0], b[1]]))
}

fn u32_at(data: &[u8], off: usize) -> Result<u32, Error> {
    let b = bytes_at(data, off, 4)?;
    Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

fn u64_at(data: &[u8], off: usize) -> Result<u64, Error> {
    let b = bytes_at(data, off, 8)?;
    Ok(u64::from_le_bytes(b.try_into().unwrap()))
}

/// An address-sized field (`Elf32_Addr`/`Elf64_Addr`, `_Off`, `_Xword`): 4
/// bytes in ELF32, 8 in ELF64.
fn addr_at(class: Class, data: &[u8], off: usize) -> Result<u64, Error> {
    match class {
        Class::Elf32 => u32_at(data, off).map(u64::from),
        Class::Elf64 => u64_at(data, off),
    }
}

/// An offset or a size read from the file, as an index into it. One that does
/// not fit a `usize` becomes `usize::MAX`, which no bounds check passes.
fn idx(v: u64) -> usize {
    usize::try_from(v).unwrap_or(usize::MAX)
}

fn bytes_at(data: &[u8], off: usize, len: usize) -> Result<&[u8], Error> {
    off.checked_add(len)
        .and_then(|end| data.get(off..end))
        .ok_or_else(|| truncated(off, len))
}

/// Reads a NUL-terminated string at `off`; a missing terminator ends at the end of `data`.
fn cstr(data: &[u8], off: usize) -> Result<String, Error> {
    let Some(s) = data.get(off..) else {
        return err(format!("string offset {off:#x} outside its string table"));
    };
    let end = s.iter().position(|&b| b == 0).unwrap_or(s.len());
    Ok(String::from_utf8_lossy(&s[..end]).into_owned())
}

impl<'a> Elf<'a> {
    /// Parses the header, program headers and section headers of a
    /// little-endian RISC-V file of either class.
    pub fn parse(data: &'a [u8]) -> Result<Elf<'a>, Error> {
        Elf::parse_with(data, true)
    }

    /// Like [`Elf::parse`], but reads no program headers ([`Elf::segments`]
    /// stays empty): how a linker reads a relocatable object (`.o`), whose
    /// program header table, if it has one, means nothing. ld.lld does not
    /// read it either, so a stray `e_phnum` is no error.
    pub fn parse_sections_only(data: &'a [u8]) -> Result<Elf<'a>, Error> {
        Elf::parse_with(data, false)
    }

    fn parse_with(data: &'a [u8], with_segments: bool) -> Result<Elf<'a>, Error> {
        if bytes_at(data, 0, 4)? != b"\x7fELF" {
            return err("missing magic");
        }
        let class = match bytes_at(data, 4, 1)?[0] {
            ELFCLASS32 => Class::Elf32,
            ELFCLASS64 => Class::Elf64,
            c => return err(format!("EI_CLASS {c} is neither ELFCLASS32 nor ELFCLASS64")),
        };
        if bytes_at(data, 5, 1)?[0] != ELFDATA2LSB {
            return err("not little-endian");
        }
        // e_entry, e_phoff and e_shoff are address-sized; the fields after
        // them move up by 12 bytes in ELF64.
        let a = class.addr_size();
        let rest = 24 + 3 * a;
        let header = Header {
            ty: u16_at(data, 16)?,
            machine: u16_at(data, 18)?,
            version: u32_at(data, 20)?,
            entry: addr_at(class, data, 24)?,
            phoff: addr_at(class, data, 24 + a)?,
            shoff: addr_at(class, data, 24 + 2 * a)?,
            flags: u32_at(data, rest)?,
            ehsize: u16_at(data, rest + 4)?,
            phentsize: u16_at(data, rest + 6)?,
            phnum: u16_at(data, rest + 8)?,
            shentsize: u16_at(data, rest + 10)?,
            shnum: u16_at(data, rest + 12)?,
            shstrndx: u16_at(data, rest + 14)?,
        };
        if header.machine != EM_RISCV {
            return err("e_machine is not RISC-V");
        }

        let mut segments = Vec::new();
        if with_segments && header.phnum > 0 {
            if (header.phentsize as usize) < class.phdr_size() {
                return err("e_phentsize too small");
            }
            for i in 0..header.phnum as usize {
                let ph = idx(header.phoff).saturating_add(i.saturating_mul(header.phentsize as usize));
                bytes_at(data, ph, class.phdr_size())?;
                segments.push(match class {
                    Class::Elf32 => ProgramHeader {
                        ty: u32_at(data, ph)?,
                        offset: u32_at(data, ph + 4)?.into(),
                        vaddr: u32_at(data, ph + 8)?.into(),
                        paddr: u32_at(data, ph + 12)?.into(),
                        filesz: u32_at(data, ph + 16)?.into(),
                        memsz: u32_at(data, ph + 20)?.into(),
                        flags: u32_at(data, ph + 24)?,
                        align: u32_at(data, ph + 28)?.into(),
                    },
                    // p_flags moves up to second place, which keeps the
                    // 8-byte fields after it aligned.
                    Class::Elf64 => ProgramHeader {
                        ty: u32_at(data, ph)?,
                        flags: u32_at(data, ph + 4)?,
                        offset: u64_at(data, ph + 8)?,
                        vaddr: u64_at(data, ph + 16)?,
                        paddr: u64_at(data, ph + 24)?,
                        filesz: u64_at(data, ph + 32)?,
                        memsz: u64_at(data, ph + 40)?,
                        align: u64_at(data, ph + 48)?,
                    },
                });
            }
        }

        let mut sections = Vec::new();
        if header.shnum > 0 {
            if (header.shentsize as usize) < class.shdr_size() {
                return err("e_shentsize too small");
            }
            let mut name_offsets = Vec::new();
            for i in 0..header.shnum as usize {
                let sh = idx(header.shoff).saturating_add(i.saturating_mul(header.shentsize as usize));
                bytes_at(data, sh, class.shdr_size())?;
                name_offsets.push(u32_at(data, sh)? as usize);
                // The same order in both classes: sh_flags, sh_addr,
                // sh_offset, sh_size, sh_addralign and sh_entsize are
                // address-sized, sh_name, sh_type, sh_link and sh_info 4 bytes.
                let field = |n: usize| addr_at(class, data, sh + 8 + n * a);
                let after = sh + 8 + 4 * a; // sh_link
                sections.push(Section {
                    name: String::new(),
                    ty: u32_at(data, sh + 4)?,
                    flags: field(0)?,
                    addr: field(1)?,
                    offset: field(2)?,
                    size: field(3)?,
                    link: u32_at(data, after)?,
                    info: u32_at(data, after + 4)?,
                    align: addr_at(class, data, after + 8)?,
                    entsize: addr_at(class, data, after + 8 + a)?,
                });
            }
            // Names come from the section header string table, when
            // e_shstrndx names one: SHN_UNDEF (0) means there is none, and
            // a section of another type is not read as one (the names stay
            // empty, and a reader that requires them, such as merlin-ld,
            // says what e_shstrndx points at).
            if let Some(strtab) = sections
                .get(header.shstrndx as usize)
                .filter(|s| header.shstrndx != 0 && s.ty == SHT_STRTAB)
            {
                let names = bytes_at(data, idx(strtab.offset), idx(strtab.size))?;
                for (s, &off) in sections.iter_mut().zip(&name_offsets) {
                    s.name = cstr(names, off)?;
                }
            }
        }

        Ok(Elf {
            data,
            class,
            header,
            segments,
            sections,
        })
    }

    /// The file contents of a section (empty for `SHT_NOBITS`).
    pub fn section_data(&self, s: &Section) -> Result<&'a [u8], Error> {
        if s.is_nobits() || s.ty == SHT_NULL {
            return Ok(&[]);
        }
        bytes_at(self.data, idx(s.offset), idx(s.size))
            .map_err(|_| Error(format!("section {} extends past the end of the file", s.name)))
    }

    /// The bytes a segment loads from the file (`p_filesz` of them).
    pub fn segment_data(&self, p: &ProgramHeader) -> Result<&'a [u8], Error> {
        bytes_at(self.data, idx(p.offset), idx(p.filesz)).map_err(|_| {
            Error(format!(
                "segment at {:#x} extends past the end of the file",
                p.vaddr
            ))
        })
    }

    /// Index of the (first) `SHT_SYMTAB` section.
    pub fn symtab_index(&self) -> Option<usize> {
        self.sections.iter().position(|s| s.ty == SHT_SYMTAB)
    }

    /// Reads the symbols of a symbol table section, including the null symbol
    /// at index 0, so indices match relocation `sym` fields.
    pub fn symbols_in(&self, symtab: &Section) -> Result<Vec<Symbol>, Error> {
        let data = self.section_data(symtab)?;
        let strtab = match self.sections.get(symtab.link as usize) {
            Some(s) => self.section_data(s)?,
            None => return err(format!("{}: bad string table index {}", symtab.name, symtab.link)),
        };
        let entsize = idx(symtab.entsize).max(self.class.sym_size());
        let mut out = Vec::with_capacity(data.len() / entsize);
        for s in (0..data.len() / entsize).map(|i| i * entsize) {
            let name = cstr(strtab, u32_at(data, s)? as usize)?;
            out.push(match self.class {
                Class::Elf32 => Symbol {
                    name,
                    value: u32_at(data, s + 4)?.into(),
                    size: u32_at(data, s + 8)?.into(),
                    info: data[s + 12],
                    other: data[s + 13],
                    shndx: u16_at(data, s + 14)?,
                },
                // st_info, st_other and st_shndx come first, then the
                // 8-byte st_value and st_size.
                Class::Elf64 => Symbol {
                    name,
                    info: data[s + 4],
                    other: data[s + 5],
                    shndx: u16_at(data, s + 6)?,
                    value: u64_at(data, s + 8)?,
                    size: u64_at(data, s + 16)?,
                },
            });
        }
        Ok(out)
    }

    /// The symbols of the file's symbol table (empty if it has none).
    pub fn symbols(&self) -> Result<Vec<Symbol>, Error> {
        match self.symtab_index() {
            Some(i) => self.symbols_in(&self.sections[i]),
            None => Ok(Vec::new()),
        }
    }

    /// Reads the entries of an `SHT_RELA` or `SHT_REL` section.
    pub fn relocations(&self, rel: &Section) -> Result<Vec<Rela>, Error> {
        let (min, has_addend) = match rel.ty {
            SHT_RELA => (self.class.rela_size(), true),
            SHT_REL => (self.class.rel_size(), false),
            _ => return err(format!("{} is not a relocation section", rel.name)),
        };
        let data = self.section_data(rel)?;
        let entsize = idx(rel.entsize).max(min);
        let mut out = Vec::with_capacity(data.len() / entsize);
        for r in (0..data.len() / entsize).map(|i| i * entsize) {
            out.push(match self.class {
                Class::Elf32 => {
                    let info = u32_at(data, r + 4)?;
                    Rela {
                        offset: u32_at(data, r)?.into(),
                        sym: info >> 8,
                        ty: info & 0xff,
                        addend: if has_addend {
                            (u32_at(data, r + 8)? as i32).into()
                        } else {
                            0
                        },
                    }
                }
                Class::Elf64 => {
                    let info = u64_at(data, r + 8)?;
                    Rela {
                        offset: u64_at(data, r)?,
                        sym: (info >> 32) as u32,
                        ty: info as u32,
                        addend: if has_addend {
                            u64_at(data, r + 16)? as i64
                        } else {
                            0
                        },
                    }
                }
            });
        }
        Ok(out)
    }

    /// Looks up a defined symbol's value by name (see [`find_symbol`]).
    pub fn symbol(&self, name: &str) -> Option<u64> {
        let syms = self.symbols().ok()?;
        find_symbol(&syms, name).map(|s| s.value)
    }
}

/// The definition of `name` among `syms`: the global (or weak) one if there
/// is one, which is what the name means to the whole program, else the first
/// local one. Several files may each have a local (`static`) symbol of the
/// same name, and an executable lists locals first.
pub fn find_symbol<'a>(syms: &'a [Symbol], name: &str) -> Option<&'a Symbol> {
    let mut defs = syms.iter().filter(|s| s.name == name && !s.is_undefined());
    let first = defs.clone().next()?;
    Some(defs.find(|s| s.bind() != STB_LOCAL).unwrap_or(first))
}

/// Name of a RISC-V relocation type (psABI numbering), as llvm-objdump and
/// llvm-readelf print it.
pub fn reloc_name(ty: u32) -> Option<&'static str> {
    Some(match ty {
        0 => "R_RISCV_NONE",
        1 => "R_RISCV_32",
        2 => "R_RISCV_64",
        3 => "R_RISCV_RELATIVE",
        4 => "R_RISCV_COPY",
        5 => "R_RISCV_JUMP_SLOT",
        6 => "R_RISCV_TLS_DTPMOD32",
        7 => "R_RISCV_TLS_DTPMOD64",
        8 => "R_RISCV_TLS_DTPREL32",
        9 => "R_RISCV_TLS_DTPREL64",
        10 => "R_RISCV_TLS_TPREL32",
        11 => "R_RISCV_TLS_TPREL64",
        12 => "R_RISCV_TLSDESC",
        16 => "R_RISCV_BRANCH",
        17 => "R_RISCV_JAL",
        18 => "R_RISCV_CALL",
        19 => "R_RISCV_CALL_PLT",
        20 => "R_RISCV_GOT_HI20",
        21 => "R_RISCV_TLS_GOT_HI20",
        22 => "R_RISCV_TLS_GD_HI20",
        23 => "R_RISCV_PCREL_HI20",
        24 => "R_RISCV_PCREL_LO12_I",
        25 => "R_RISCV_PCREL_LO12_S",
        26 => "R_RISCV_HI20",
        27 => "R_RISCV_LO12_I",
        28 => "R_RISCV_LO12_S",
        29 => "R_RISCV_TPREL_HI20",
        30 => "R_RISCV_TPREL_LO12_I",
        31 => "R_RISCV_TPREL_LO12_S",
        32 => "R_RISCV_TPREL_ADD",
        33 => "R_RISCV_ADD8",
        34 => "R_RISCV_ADD16",
        35 => "R_RISCV_ADD32",
        36 => "R_RISCV_ADD64",
        37 => "R_RISCV_SUB8",
        38 => "R_RISCV_SUB16",
        39 => "R_RISCV_SUB32",
        40 => "R_RISCV_SUB64",
        41 => "R_RISCV_GOT32_PCREL",
        43 => "R_RISCV_ALIGN",
        44 => "R_RISCV_RVC_BRANCH",
        45 => "R_RISCV_RVC_JUMP",
        51 => "R_RISCV_RELAX",
        52 => "R_RISCV_SUB6",
        53 => "R_RISCV_SET6",
        54 => "R_RISCV_SET8",
        55 => "R_RISCV_SET16",
        56 => "R_RISCV_SET32",
        57 => "R_RISCV_32_PCREL",
        58 => "R_RISCV_IRELATIVE",
        59 => "R_RISCV_PLT32",
        60 => "R_RISCV_SET_ULEB128",
        61 => "R_RISCV_SUB_ULEB128",
        62 => "R_RISCV_TLSDESC_HI20",
        63 => "R_RISCV_TLSDESC_LOAD_LO12",
        64 => "R_RISCV_TLSDESC_ADD_LO12",
        65 => "R_RISCV_TLSDESC_CALL",
        _ => return None,
    })
}

#[cfg(test)]
mod tests;
