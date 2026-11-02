//! ELF32 little-endian reader for RISC-V files: the ELF header, program
//! headers, section headers, symbols and relocations.
//!
//! Shared by Grail (loads executables), merlin-objdump (prints everything)
//! and merlin-ld (reads `.o` files). It only reads; merlin-as and merlin-ld
//! write their own output. Every offset taken from the file is bounds
//! checked, so a truncated or corrupt file gives an [`Error`], never a panic.

use std::fmt;

// e_type
pub const ET_NONE: u16 = 0;
pub const ET_REL: u16 = 1;
pub const ET_EXEC: u16 = 2;
pub const ET_DYN: u16 = 3;

pub const EM_RISCV: u16 = 243;

// Section types.
pub const SHT_NULL: u32 = 0;
pub const SHT_PROGBITS: u32 = 1;
pub const SHT_SYMTAB: u32 = 2;
pub const SHT_STRTAB: u32 = 3;
pub const SHT_RELA: u32 = 4;
pub const SHT_NOBITS: u32 = 8;
pub const SHT_REL: u32 = 9;

// Section flags.
pub const SHF_WRITE: u32 = 0x1;
pub const SHF_ALLOC: u32 = 0x2;
pub const SHF_EXECINSTR: u32 = 0x4;
pub const SHF_INFO_LINK: u32 = 0x40;

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

/// Sizes of the fixed-layout records.
pub const EHDR_SIZE: usize = 52;
pub const PHDR_SIZE: usize = 32;
pub const SHDR_SIZE: usize = 40;
pub const SYM_SIZE: usize = 16;
pub const RELA_SIZE: usize = 12;
pub const REL_SIZE: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error(pub String);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "bad ELF: {}", self.0)
    }
}

impl std::error::Error for Error {}

fn err<T>(msg: impl Into<String>) -> Result<T, Error> {
    Err(Error(msg.into()))
}

/// The ELF file header, minus `e_ident` (which [`Elf::parse`] checks).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    pub ty: u16,
    pub machine: u16,
    pub version: u32,
    pub entry: u32,
    pub phoff: u32,
    pub shoff: u32,
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
    pub offset: u32,
    pub vaddr: u32,
    pub paddr: u32,
    pub filesz: u32,
    pub memsz: u32,
    pub flags: u32,
    pub align: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    /// Name from the section header string table (empty for the null section).
    pub name: String,
    pub ty: u32,
    pub flags: u32,
    pub addr: u32,
    pub offset: u32,
    pub size: u32,
    pub link: u32,
    pub info: u32,
    pub align: u32,
    pub entsize: u32,
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
    pub value: u32,
    pub size: u32,
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rela {
    /// Offset within the target section (`.o`) or virtual address (executables).
    pub offset: u32,
    /// Index into the symbol table named by the relocation section's `sh_link`.
    pub sym: u32,
    pub ty: u32,
    pub addend: i32,
}

/// A parsed ELF file. Section, segment and symbol contents stay in the
/// borrowed buffer and are read on demand.
pub struct Elf<'a> {
    pub data: &'a [u8],
    pub header: Header,
    pub segments: Vec<ProgramHeader>,
    pub sections: Vec<Section>,
}

fn u16_at(data: &[u8], off: usize) -> Result<u16, Error> {
    match data.get(off..off.saturating_add(2)) {
        Some(b) => Ok(u16::from_le_bytes([b[0], b[1]])),
        None => err("truncated file"),
    }
}

fn u32_at(data: &[u8], off: usize) -> Result<u32, Error> {
    match data.get(off..off.saturating_add(4)) {
        Some(b) => Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]])),
        None => err("truncated file"),
    }
}

fn bytes_at(data: &[u8], off: usize, len: usize) -> Result<&[u8], Error> {
    match off.checked_add(len).and_then(|end| data.get(off..end)) {
        Some(b) => Ok(b),
        None => err("truncated file"),
    }
}

/// Reads a NUL-terminated string at `off`; a missing terminator ends at the end of `data`.
fn cstr(data: &[u8], off: usize) -> Result<String, Error> {
    let Some(s) = data.get(off..) else {
        return err(format!("string offset {off:#x} out of range"));
    };
    let end = s.iter().position(|&b| b == 0).unwrap_or(s.len());
    Ok(String::from_utf8_lossy(&s[..end]).into_owned())
}

impl<'a> Elf<'a> {
    /// Parses the header, program headers and section headers of an ELF32
    /// little-endian RISC-V file.
    pub fn parse(data: &'a [u8]) -> Result<Elf<'a>, Error> {
        if bytes_at(data, 0, 4)? != b"\x7fELF" {
            return err("missing magic");
        }
        if bytes_at(data, 4, 2)? != [1, 1] {
            return err(if data[4] != 1 {
                "not 32-bit (EI_CLASS != ELFCLASS32)"
            } else {
                "not little-endian"
            });
        }
        let header = Header {
            ty: u16_at(data, 16)?,
            machine: u16_at(data, 18)?,
            version: u32_at(data, 20)?,
            entry: u32_at(data, 24)?,
            phoff: u32_at(data, 28)?,
            shoff: u32_at(data, 32)?,
            flags: u32_at(data, 36)?,
            ehsize: u16_at(data, 40)?,
            phentsize: u16_at(data, 42)?,
            phnum: u16_at(data, 44)?,
            shentsize: u16_at(data, 46)?,
            shnum: u16_at(data, 48)?,
            shstrndx: u16_at(data, 50)?,
        };
        if header.machine != EM_RISCV {
            return err("e_machine is not RISC-V");
        }

        let mut segments = Vec::new();
        if header.phnum > 0 {
            if (header.phentsize as usize) < PHDR_SIZE {
                return err("e_phentsize too small");
            }
            for i in 0..header.phnum as usize {
                let ph = (header.phoff as usize).saturating_add(i * header.phentsize as usize);
                bytes_at(data, ph, PHDR_SIZE)?;
                segments.push(ProgramHeader {
                    ty: u32_at(data, ph)?,
                    offset: u32_at(data, ph + 4)?,
                    vaddr: u32_at(data, ph + 8)?,
                    paddr: u32_at(data, ph + 12)?,
                    filesz: u32_at(data, ph + 16)?,
                    memsz: u32_at(data, ph + 20)?,
                    flags: u32_at(data, ph + 24)?,
                    align: u32_at(data, ph + 28)?,
                });
            }
        }

        let mut sections = Vec::new();
        if header.shnum > 0 {
            if (header.shentsize as usize) < SHDR_SIZE {
                return err("e_shentsize too small");
            }
            let mut name_offsets = Vec::new();
            for i in 0..header.shnum as usize {
                let sh = (header.shoff as usize).saturating_add(i * header.shentsize as usize);
                bytes_at(data, sh, SHDR_SIZE)?;
                name_offsets.push(u32_at(data, sh)? as usize);
                sections.push(Section {
                    name: String::new(),
                    ty: u32_at(data, sh + 4)?,
                    flags: u32_at(data, sh + 8)?,
                    addr: u32_at(data, sh + 12)?,
                    offset: u32_at(data, sh + 16)?,
                    size: u32_at(data, sh + 20)?,
                    link: u32_at(data, sh + 24)?,
                    info: u32_at(data, sh + 28)?,
                    align: u32_at(data, sh + 32)?,
                    entsize: u32_at(data, sh + 36)?,
                });
            }
            if let Some(strtab) = sections
                .get(header.shstrndx as usize)
                .filter(|_| header.shstrndx != 0)
            {
                let names = bytes_at(data, strtab.offset as usize, strtab.size as usize)?;
                for (s, &off) in sections.iter_mut().zip(&name_offsets) {
                    s.name = cstr(names, off)?;
                }
            }
        }

        Ok(Elf {
            data,
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
        bytes_at(self.data, s.offset as usize, s.size as usize)
            .map_err(|_| Error(format!("section {} extends past the end of the file", s.name)))
    }

    /// The bytes a segment loads from the file (`p_filesz` of them).
    pub fn segment_data(&self, p: &ProgramHeader) -> Result<&'a [u8], Error> {
        bytes_at(self.data, p.offset as usize, p.filesz as usize).map_err(|_| {
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
        let entsize = (symtab.entsize as usize).max(SYM_SIZE);
        let mut out = Vec::with_capacity(data.len() / entsize);
        for s in (0..data.len() / entsize).map(|i| i * entsize) {
            out.push(Symbol {
                name: cstr(strtab, u32_at(data, s)? as usize)?,
                value: u32_at(data, s + 4)?,
                size: u32_at(data, s + 8)?,
                info: data[s + 12],
                other: data[s + 13],
                shndx: u16_at(data, s + 14)?,
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
            SHT_RELA => (RELA_SIZE, true),
            SHT_REL => (REL_SIZE, false),
            _ => return err(format!("{} is not a relocation section", rel.name)),
        };
        let data = self.section_data(rel)?;
        let entsize = (rel.entsize as usize).max(min);
        let mut out = Vec::with_capacity(data.len() / entsize);
        for r in (0..data.len() / entsize).map(|i| i * entsize) {
            let info = u32_at(data, r + 4)?;
            out.push(Rela {
                offset: u32_at(data, r)?,
                sym: info >> 8,
                ty: info & 0xff,
                addend: if has_addend {
                    u32_at(data, r + 8)? as i32
                } else {
                    0
                },
            });
        }
        Ok(out)
    }

    /// Looks up a defined symbol's value by name.
    pub fn symbol(&self, name: &str) -> Option<u32> {
        let syms = self.symbols().ok()?;
        syms.iter()
            .find(|s| s.name == name && !s.is_undefined())
            .map(|s| s.value)
    }
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
