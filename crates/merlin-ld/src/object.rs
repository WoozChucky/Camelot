//! Reads ELF32 little-endian RISC-V relocatable objects (`ET_REL`): the subset
//! that clang (`-c`) and merlin-as produce. Anything outside that subset is
//! rejected with a message saying what was found, never skipped silently.
//!
//! Sections and symbols keep their ELF indices, so a relocation's symbol index
//! and a symbol's `st_shndx` can be used directly.

// The ELF constants, the elf crate's (shared with merlin-objdump, and generated into
// the C linker's tables by crates/xtask/src/c_tables.rs).
pub use elf::{
    EM_RISCV, ET_REL, SHF_ALLOC, SHF_EXECINSTR, SHF_GROUP, SHF_MERGE, SHF_STRINGS, SHF_TLS, SHF_WRITE,
    SHN_ABS, SHN_COMMON, SHN_LORESERVE, SHN_UNDEF, SHT_GROUP, SHT_NOBITS, SHT_NULL, SHT_PROGBITS, SHT_REL,
    SHT_RELA, SHT_STRTAB, SHT_SYMTAB, SHT_SYMTAB_SHNDX, STB_GLOBAL, STB_LOCAL, STB_WEAK, STT_FILE,
    STT_NOTYPE, STT_SECTION, STT_TLS,
};

#[derive(Debug, Clone)]
pub struct Section {
    pub name: String,
    pub ty: u32,
    pub flags: u32,
    /// File contents; empty for `SHT_NOBITS`.
    pub data: Vec<u8>,
    /// Size in memory (`sh_size`), also for `SHT_NOBITS`.
    pub size: u32,
    /// Alignment in bytes, at least 1.
    pub align: u32,
    /// `sh_entsize`: the entry size of a `SHF_MERGE` section.
    pub entsize: u32,
    /// Relocations that patch this section (from the `.rela` section whose
    /// `sh_info` points here).
    pub relocs: Vec<Rela>,
}

impl Section {
    pub fn is_alloc(&self) -> bool {
        self.flags & SHF_ALLOC != 0
    }
    pub fn is_nobits(&self) -> bool {
        self.ty == SHT_NOBITS
    }
}

/// One `Elf32_Rela` entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rela {
    /// Offset of the patched bytes from the start of the section.
    pub offset: u32,
    /// `ELF32_R_TYPE(r_info)`, an `R_RISCV_*` number.
    pub ty: u32,
    /// `ELF32_R_SYM(r_info)`, an index into [`Object::symbols`].
    pub sym: u32,
    pub addend: i32,
}

#[derive(Debug, Clone)]
pub struct Symbol {
    pub name: String,
    pub value: u32,
    pub size: u32,
    pub bind: u8,
    pub ty: u8,
    pub shndx: u16,
}

impl Symbol {
    pub fn is_local(&self) -> bool {
        self.bind == STB_LOCAL
    }
    pub fn is_weak(&self) -> bool {
        self.bind == STB_WEAK
    }
    pub fn is_undefined(&self) -> bool {
        self.shndx == SHN_UNDEF
    }
}

#[derive(Debug, Clone)]
pub struct Object {
    /// File name, used as the prefix of every diagnostic about this object.
    pub name: String,
    /// Indexed by ELF section index; entry 0 is the null section.
    pub sections: Vec<Section>,
    /// Indexed by symbol table index; entry 0 is the null symbol.
    pub symbols: Vec<Symbol>,
}

impl Object {
    /// The name to show for a symbol in diagnostics: section symbols are
    /// shown as their section's name.
    pub fn symbol_display_name(&self, index: u32) -> &str {
        match self.symbols.get(index as usize) {
            Some(s) if s.ty == STT_SECTION => self
                .sections
                .get(s.shndx as usize)
                .map(|sec| sec.name.as_str())
                .unwrap_or(""),
            Some(s) => &s.name,
            None => "",
        }
    }
}

struct Reader<'a>(&'a [u8]);

impl Reader<'_> {
    fn bytes(&self, off: usize, len: usize) -> Result<&[u8], String> {
        off.checked_add(len)
            .and_then(|end| self.0.get(off..end))
            .ok_or_else(|| format!("truncated file (needs bytes {off:#x}..{:#x})", off + len))
    }
    fn u8(&self, off: usize) -> Result<u8, String> {
        Ok(self.bytes(off, 1)?[0])
    }
    fn u16(&self, off: usize) -> Result<u16, String> {
        let b = self.bytes(off, 2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }
    fn u32(&self, off: usize) -> Result<u32, String> {
        let b = self.bytes(off, 4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
}

fn cstr(table: &[u8], off: u32) -> Result<String, String> {
    let s = table
        .get(off as usize..)
        .ok_or_else(|| format!("string offset {off:#x} outside its string table"))?;
    let end = s.iter().position(|&b| b == 0).unwrap_or(s.len());
    Ok(String::from_utf8_lossy(&s[..end]).into_owned())
}

/// Raw section header, before contents are interpreted.
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

/// Parses a relocatable object. Errors don't include the file name; the
/// caller adds it.
pub fn parse(name: &str, file: &[u8]) -> Result<Object, String> {
    let r = Reader(file);
    if file.len() < 4 || &file[..4] != b"\x7fELF" {
        return Err("not an ELF file".into());
    }
    if r.u8(4)? != 1 {
        return Err("not a 32-bit ELF file (EI_CLASS is not ELFCLASS32)".into());
    }
    if r.u8(5)? != 1 {
        return Err("not a little-endian ELF file".into());
    }
    if r.u8(6)? != 1 {
        return Err("unknown ELF version".into());
    }
    let e_type = r.u16(16)?;
    if e_type != ET_REL {
        let what = match e_type {
            2 => "an executable",
            3 => "a shared object",
            4 => "a core file",
            _ => "not a relocatable object",
        };
        return Err(format!(
            "{what} (e_type {e_type}), expected a relocatable object (ET_REL)"
        ));
    }
    let machine = r.u16(18)?;
    if machine != EM_RISCV {
        return Err(format!("e_machine is {machine}, not RISC-V (243)"));
    }
    let flags = r.u32(36)?;
    if flags & 1 != 0 {
        return Err("built with compressed instructions (EF_RISCV_RVC); Excalibur has no C extension".into());
    }
    if flags != 0 {
        return Err(format!(
            "e_flags {flags:#x} not supported: Excalibur objects use the ilp32 soft-float ABI (e_flags 0)"
        ));
    }
    let shoff = r.u32(32)? as usize;
    let shentsize = r.u16(46)? as usize;
    let shnum = r.u16(48)? as usize;
    let shstrndx = r.u16(50)? as usize;
    if shnum == 0 {
        return Ok(Object {
            name: name.to_string(),
            sections: Vec::new(),
            symbols: Vec::new(),
        });
    }
    if shentsize != 40 {
        return Err(format!("e_shentsize is {shentsize}, expected 40"));
    }

    let mut hdrs = Vec::with_capacity(shnum);
    for i in 0..shnum {
        let h = shoff + i * 40;
        hdrs.push(Shdr {
            name: r.u32(h)?,
            ty: r.u32(h + 4)?,
            flags: r.u32(h + 8)?,
            offset: r.u32(h + 16)?,
            size: r.u32(h + 20)?,
            link: r.u32(h + 24)?,
            info: r.u32(h + 28)?,
            align: r.u32(h + 32)?,
            entsize: r.u32(h + 36)?,
        });
    }
    let contents = |h: &Shdr| -> Result<&[u8], String> {
        if h.ty == SHT_NOBITS {
            Ok(&[])
        } else {
            r.bytes(h.offset as usize, h.size as usize)
        }
    };
    let shstrtab = match hdrs.get(shstrndx) {
        Some(h) if h.ty == SHT_STRTAB => contents(h)?,
        _ => return Err(format!("e_shstrndx {shstrndx} is not a string table")),
    };

    let mut sections = Vec::with_capacity(shnum);
    for (i, h) in hdrs.iter().enumerate() {
        let sname = cstr(shstrtab, h.name)?;
        let alloc = h.flags & SHF_ALLOC != 0;
        match h.ty {
            SHT_REL => {
                return Err(format!(
                    "section '{sname}' is SHT_REL; RISC-V objects use SHT_RELA"
                ))
            }
            SHT_GROUP => return Err(format!("section group '{sname}' (COMDAT) is not supported")),
            SHT_SYMTAB_SHNDX => {
                return Err("extended section indices (SHT_SYMTAB_SHNDX) are not supported".into())
            }
            _ => {}
        }
        if alloc && h.ty != SHT_PROGBITS && h.ty != SHT_NOBITS {
            return Err(format!(
                "allocated section '{sname}' has type {:#x}; only PROGBITS and NOBITS are supported",
                h.ty
            ));
        }
        if h.flags & SHF_TLS != 0 {
            return Err(format!("thread-local section '{sname}' is not supported"));
        }
        if h.flags & SHF_GROUP != 0 {
            return Err(format!(
                "section '{sname}' is in a section group (COMDAT), not supported"
            ));
        }
        let data = if i == 0 { Vec::new() } else { contents(h)?.to_vec() };
        sections.push(Section {
            name: sname,
            ty: h.ty,
            flags: h.flags,
            data,
            size: h.size,
            align: h.align.max(1),
            entsize: h.entsize,
            relocs: Vec::new(),
        });
    }

    // Symbol table.
    let symtabs: Vec<usize> = (0..shnum).filter(|&i| hdrs[i].ty == SHT_SYMTAB).collect();
    if symtabs.len() > 1 {
        return Err("more than one symbol table".into());
    }
    let mut symbols = Vec::new();
    if let Some(&st) = symtabs.first() {
        let h = &hdrs[st];
        let strtab = match hdrs.get(h.link as usize) {
            Some(s) if s.ty == SHT_STRTAB => contents(s)?,
            _ => return Err("symbol table's sh_link is not a string table".into()),
        };
        if h.entsize != 16 {
            return Err(format!("symbol table entry size is {}, expected 16", h.entsize));
        }
        let data = contents(h)?;
        for (n, e) in data.as_chunks::<16>().0.iter().enumerate() {
            let e = Reader(e);
            let info = e.u8(12)?;
            let sym = Symbol {
                name: cstr(strtab, e.u32(0)?)?,
                value: e.u32(4)?,
                size: e.u32(8)?,
                bind: info >> 4,
                ty: info & 0xf,
                shndx: e.u16(14)?,
            };
            if n == 0 {
                symbols.push(sym);
                continue;
            }
            if sym.shndx == SHN_COMMON {
                return Err(format!(
                    "common symbol '{}' is not supported (define it in .bss, or compile with -fno-common)",
                    sym.name
                ));
            }
            if sym.ty == STT_TLS {
                return Err(format!("thread-local symbol '{}' is not supported", sym.name));
            }
            if sym.bind > STB_WEAK {
                return Err(format!(
                    "symbol '{}' has unsupported binding {}",
                    sym.name, sym.bind
                ));
            }
            if sym.shndx != SHN_ABS
                && sym.shndx != SHN_UNDEF
                && (sym.shndx >= SHN_LORESERVE || sym.shndx as usize >= shnum)
            {
                return Err(format!(
                    "symbol '{}' has unsupported section index {:#x}",
                    sym.name, sym.shndx
                ));
            }
            symbols.push(sym);
        }
    }

    // Relocations, attached to the section they patch.
    for (i, h) in hdrs.iter().enumerate() {
        if h.ty != SHT_RELA {
            continue;
        }
        let rname = &sections[i].name;
        if symtabs.first() != Some(&(h.link as usize)) {
            return Err(format!(
                "relocation section '{rname}' does not use the symbol table"
            ));
        }
        let target = h.info as usize;
        if target == 0 || target >= shnum {
            return Err(format!(
                "relocation section '{rname}' patches section index {target}"
            ));
        }
        if h.entsize != 12 {
            return Err(format!(
                "relocation section '{rname}' has entry size {}, expected 12",
                h.entsize
            ));
        }
        let mut relocs = Vec::new();
        for e in contents(h)?.as_chunks::<12>().0 {
            let e = Reader(e);
            let info = e.u32(4)?;
            let rela = Rela {
                offset: e.u32(0)?,
                ty: info & 0xff,
                sym: info >> 8,
                addend: e.u32(8)? as i32,
            };
            if rela.sym as usize >= symbols.len() {
                return Err(format!(
                    "relocation in '{rname}' refers to symbol index {}, past the symbol table",
                    rela.sym
                ));
            }
            relocs.push(rela);
        }
        sections[target].relocs.extend(relocs);
    }

    Ok(Object {
        name: name.to_string(),
        sections,
        symbols,
    })
}
