//! Reads little-endian RISC-V relocatable objects (`ET_REL`), ELF64 (RV64):
//! the subset that clang (`-c`) and merlin-as produce. Anything outside that
//! subset is rejected with a message saying what was found, never skipped
//! silently: an ELF32 (RV32) object among them.
//!
//! The records themselves are read by the `elf` crate (shared with Grail,
//! merlin-objdump and merlin-dbg), which reads ELF32 too. This module is
//! merlin-ld's side of it: it checks what merlin-ld accepts, in a fixed
//! order, before and after the crate reads each part:
//!
//! 1. `e_ident` and the file header, read here: an ELF64 file, little-endian,
//!    version 1, `ET_REL`, RISC-V, `e_flags` 0 (the soft-float ABI, no
//!    compressed instructions), section headers of ELF64's size;
//! 2. the section headers and their names ([`elf::Elf::parse_sections_only`]);
//! 3. the sections, the symbol table and the relocation sections, each
//!    checked (types, links, entry sizes, contents inside the file) before
//!    the crate reads its entries, and the entries after.
//!
//! Sections and symbols keep their ELF indices, so a relocation's symbol index
//! and a symbol's `st_shndx` can be used directly.
//!
//! Sizes, flags, symbol values, relocation offsets and addends are held at
//! ELF64's widths (`u64`, `i64`).

// The ELF constants, the elf crate's (shared with merlin-objdump, and generated into
// the C linker's tables by crates/xtask/src/c_tables.rs). Its section flags are 8
// bytes, the size of ELF64's sh_flags, as Section::flags is.
pub use elf::{
    Class, EM_RISCV, ET_REL, SHF_ALLOC, SHF_EXECINSTR, SHF_GROUP, SHF_MERGE, SHF_STRINGS, SHF_TLS, SHF_WRITE,
    SHN_ABS, SHN_COMMON, SHN_LORESERVE, SHN_UNDEF, SHT_GROUP, SHT_NOBITS, SHT_NULL, SHT_PROGBITS, SHT_REL,
    SHT_RELA, SHT_STRTAB, SHT_SYMTAB, SHT_SYMTAB_SHNDX, STB_GLOBAL, STB_LOCAL, STB_WEAK, STT_FILE,
    STT_NOTYPE, STT_SECTION, STT_TLS,
};

/// One `Elf64_Rela` entry, the elf crate's: `offset` is the
/// offset of the patched bytes from the start of the section, `ty` an
/// `R_RISCV_*` number, `sym` an index into [`Object::symbols`].
pub use elf::Rela;

#[derive(Debug, Clone)]
pub struct Section {
    pub name: String,
    pub ty: u32,
    pub flags: u64,
    /// File contents; empty for `SHT_NOBITS`.
    pub data: Vec<u8>,
    /// Size in memory (`sh_size`), also for `SHT_NOBITS`.
    pub size: u64,
    /// Alignment in bytes, a power of two (`sh_addralign` 0 is 1). 32 bits:
    /// ld.lld keeps an input section's alignment in 32 bits too, and refuses
    /// an `sh_addralign` above 2^32 - 1 ("section
    /// sh_addralign is too large") and one that is not a power of two, as
    /// [`parse`] does. 1 for the symbol and string tables and relocations,
    /// whose alignment neither linker looks at.
    pub align: u32,
    /// `sh_entsize`: the entry size of a `SHF_MERGE` section.
    pub entsize: u64,
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

#[derive(Debug, Clone)]
pub struct Symbol {
    pub name: String,
    pub value: u64,
    pub size: u64,
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

/// Bounds-checked reads of the file, for the header fields this module
/// checks itself and for section contents. A read past the end says which
/// bytes it needed, in the elf crate's words.
struct Reader<'a>(&'a [u8]);

impl Reader<'_> {
    fn bytes(&self, off: usize, len: usize) -> Result<&[u8], String> {
        off.checked_add(len)
            .and_then(|end| self.0.get(off..end))
            // The end in 128 bits: an ELF64 offset plus size may not fit.
            .ok_or_else(|| {
                format!(
                    "truncated file (needs bytes {off:#x}..{:#x})",
                    off as u128 + len as u128
                )
            })
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
    /// A section's file contents: none for `SHT_NOBITS`.
    fn contents(&self, s: &elf::Section) -> Result<&[u8], String> {
        if s.ty == SHT_NOBITS {
            Ok(&[])
        } else {
            self.bytes(idx(s.offset), idx(s.size))
        }
    }
}

/// An offset or a size from the file, as an index into it. One that does not
/// fit a `usize` (on a 32-bit host) becomes `usize::MAX`, past every file.
fn idx(v: u64) -> usize {
    usize::try_from(v).unwrap_or(usize::MAX)
}

/// Parses a relocatable object. Errors don't include the file name; the
/// caller adds it.
pub fn parse(name: &str, file: &[u8]) -> Result<Object, String> {
    let r = Reader(file);

    // 1. e_ident and the file header. The elf crate checks less than
    // merlin-ld wants (no EI_VERSION, no e_type, any e_flags), and in another
    // order, so these come first, each with its own message.
    if file.len() < 4 || &file[..4] != b"\x7fELF" {
        return Err("not an ELF file".into());
    }
    // ELF64 only: RV64's objects. An ELF32 one is RV32 code, which no
    // Excalibur hart runs (merlin-ld linked RV32 until #159).
    let class = match r.u8(4)? {
        elf::ELFCLASS64 => Class::Elf64,
        elf::ELFCLASS32 => {
            return Err("an ELF32 (RV32) object: merlin-ld links RV64 (ELF64) objects only".into())
        }
        c => return Err(format!("not a 64-bit ELF file (EI_CLASS is {c})")),
    };
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
    // e_entry, e_phoff and e_shoff are address-sized, so e_flags and the
    // fields after it are 12 bytes further on in ELF64: e_flags at 36 or 48.
    let rest = 24 + 3 * class.addr_size();
    let flags = r.u32(rest)?;
    if flags & 1 != 0 {
        return Err("built with compressed instructions (EF_RISCV_RVC); Excalibur has no C extension".into());
    }
    if flags != 0 {
        // clang's lp64 (and merlin-as's objects) have e_flags 0:
        // no float ABI bits, no RVC, no TSO.
        return Err(format!(
            "e_flags {flags:#x} not supported: Excalibur objects use the lp64 soft-float ABI (e_flags 0)"
        ));
    }
    let shentsize = r.u16(rest + 10)? as usize;
    let shnum = r.u16(rest + 12)? as usize;
    let shstrndx = r.u16(rest + 14)? as usize;
    if shnum == 0 {
        return Ok(Object {
            name: name.to_string(),
            sections: Vec::new(),
            symbols: Vec::new(),
        });
    }
    if shentsize != class.shdr_size() {
        return Err(format!(
            "e_shentsize is {shentsize}, expected {}",
            class.shdr_size()
        ));
    }

    // 2. The section headers, each read whole, and their names. An object's
    // program headers, if it has any, mean nothing to a linker.
    let elf = elf::Elf::parse_sections_only(file).map_err(|e| e.0)?;
    let hdrs = &elf.sections;
    match hdrs.get(shstrndx) {
        // 0 is SHN_UNDEF, "no section name table": merlin-ld needs the names.
        Some(h) if shstrndx != 0 && h.ty == SHT_STRTAB => {}
        _ => return Err(format!("e_shstrndx {shstrndx} is not a string table")),
    }

    // 3. Sections.
    let mut sections = Vec::with_capacity(shnum);
    for (i, h) in hdrs.iter().enumerate() {
        let sname = &h.name;
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
        if h.is_alloc() && h.ty != SHT_PROGBITS && h.ty != SHT_NOBITS {
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
        // The alignment of a section that holds code or data (ld.lld checks
        // no other: not the null section, the symbol and string tables or the
        // relocations). Only ELF64's sh_addralign can be too large for 32
        // bits (ld.lld: "section sh_addralign is too large"). 0 means no
        // constraint, as 1 does; any other value must be a power of two, so
        // at most 2^31 (ld.lld: "sh_addralign is not a power of 2").
        let align = if matches!(h.ty, SHT_NULL | SHT_SYMTAB | SHT_STRTAB | SHT_RELA) {
            1
        } else {
            let Ok(align) = u32::try_from(h.align) else {
                return Err(format!(
                    "section '{sname}' has sh_addralign {:#x}, too large",
                    h.align
                ));
            };
            if align > 1 && !align.is_power_of_two() {
                return Err(format!(
                    "section '{sname}' has sh_addralign {align:#x}, not a power of 2"
                ));
            }
            align.max(1)
        };
        let data = if i == 0 {
            Vec::new()
        } else {
            r.contents(h)?.to_vec()
        };
        sections.push(Section {
            name: sname.clone(),
            ty: h.ty,
            flags: h.flags,
            data,
            size: h.size,
            align,
            entsize: h.entsize,
            relocs: Vec::new(),
        });
    }

    // Symbol table: its header is checked here, its entries read by the elf
    // crate (names first, then each entry checked in order).
    let symtabs: Vec<usize> = (0..shnum).filter(|&i| hdrs[i].ty == SHT_SYMTAB).collect();
    if symtabs.len() > 1 {
        return Err("more than one symbol table".into());
    }
    let mut symbols = Vec::new();
    if let Some(&st) = symtabs.first() {
        let h = &hdrs[st];
        let Some(strtab) = hdrs.get(h.link as usize).filter(|s| s.ty == SHT_STRTAB) else {
            return Err("symbol table's sh_link is not a string table".into());
        };
        // Both tables must lie inside the file before the crate reads them.
        r.contents(strtab)?;
        if h.entsize != class.sym_size() as u64 {
            return Err(format!(
                "symbol table entry size is {}, expected {}",
                h.entsize,
                class.sym_size()
            ));
        }
        // ld.lld: "has an invalid sh_size (N) which is not a multiple of its
        // sh_entsize (M)". The relocation sections' sizes it does not check.
        if h.size % h.entsize != 0 {
            return Err(format!(
                "section '{}' has sh_size {:#x}, not a multiple of its sh_entsize ({})",
                h.name, h.size, h.entsize
            ));
        }
        r.contents(h)?;
        for (n, s) in elf.symbols_in(h).map_err(|e| e.0)?.into_iter().enumerate() {
            let sym = Symbol {
                bind: s.bind(),
                ty: s.ty(),
                name: s.name,
                value: s.value,
                size: s.size,
                shndx: s.shndx,
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
        if h.entsize != class.rela_size() as u64 {
            return Err(format!(
                "relocation section '{rname}' has entry size {}, expected {}",
                h.entsize,
                class.rela_size()
            ));
        }
        r.contents(h)?;
        // r_info is `sym << 32 | type`; the crate splits it.
        let relocs = elf.relocations(h).map_err(|e| e.0)?;
        if let Some(bad) = relocs.iter().find(|rela| rela.sym as usize >= symbols.len()) {
            return Err(format!(
                "relocation in '{rname}' refers to symbol index {}, past the symbol table",
                bad.sym
            ));
        }
        sections[target].relocs.extend(relocs);
    }

    Ok(Object {
        name: name.to_string(),
        sections,
        symbols,
    })
}
