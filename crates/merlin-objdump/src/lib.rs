//! merlin-objdump: prints what is inside our ELF32 `.o` files and
//! executables, so merlin-as and merlin-ld output can be debugged without LLVM.
//!
//! The output follows llvm-objdump (LLVM 23) line for line, so the two can be
//! diffed: the tests compare against llvm-objdump output committed under
//! `tests/golden/`. Differences are deliberate and few:
//!
//! - Words that do not decode print as `.word 0x...` (one per 4 bytes) where
//!   llvm-objdump prints `<unknown>`, and the Xcal instructions print as
//!   `xcal.dbg` / `xcal.halt`. (Data the assembler marked with a `$d`
//!   mapping symbol prints as llvm-objdump prints it: `.word`, `.short` and
//!   `.byte` with their bytes.)
//! - Reserved fence encodings print as the `fence`/`fence.i` Grail executes
//!   them as, and `dret` prints as `.word`.
//! - Only the CSR names in `excalibur::csr` are known; others print as numbers.

pub mod cli;
mod disasm;

use elf::{Elf, Error, Section, Symbol};
use std::fmt::Write;

/// What to print, mirroring llvm-objdump's options.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Options {
    /// `-f`: architecture and entry point.
    pub file_headers: bool,
    /// `-p`: program headers.
    pub private_headers: bool,
    /// `-h`: section headers.
    pub section_headers: bool,
    /// `-t`: symbol table.
    pub symbols: bool,
    /// `-r`: relocations. With `-d` they are shown under the instruction they patch.
    pub relocs: bool,
    /// `-s`: hex dump of every section.
    pub contents: bool,
    /// `-d`: disassemble executable sections.
    pub disassemble: bool,
    /// Print pseudo-instructions (`mv`, `ret`, ...); `-M no-aliases` clears it.
    pub aliases: bool,
    /// Show the instruction word next to each instruction; `--no-show-raw-insn` clears it.
    pub raw_insn: bool,
    /// `-l`: with `-d`, the function and the source `file:line` each run of
    /// instructions comes from (the `.debug_line` line tables).
    pub line_numbers: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            file_headers: false,
            private_headers: false,
            section_headers: false,
            symbols: false,
            relocs: false,
            contents: false,
            disassemble: false,
            aliases: true,
            raw_insn: true,
            line_numbers: false,
        }
    }
}

impl Options {
    /// `-x`: everything but the disassembly and the contents.
    pub fn set_all_headers(&mut self) {
        self.file_headers = true;
        self.private_headers = true;
        self.section_headers = true;
        self.symbols = true;
        self.relocs = true;
    }

    /// True if at least one thing to print is selected.
    pub fn any(&self) -> bool {
        self.file_headers
            || self.private_headers
            || self.section_headers
            || self.symbols
            || self.relocs
            || self.contents
            || self.disassemble
    }
}

/// Dumps one file. `name` is printed in the header line, as given.
pub fn dump(name: &str, data: &[u8], opts: &Options) -> Result<String, Error> {
    dump_with_warnings(name, data, opts).map(|(text, _)| text)
}

/// [`dump`], and the warnings llvm-objdump would print on stderr for the
/// file (without its `llvm-objdump: warning: 'file': ` prefix), in order.
pub fn dump_with_warnings(name: &str, data: &[u8], opts: &Options) -> Result<(String, Vec<String>), Error> {
    let elf = Elf::parse(data)?;
    // RV64 (ELF64) comes with the RV64 migration's objdump step.
    if elf.class != elf::Class::Elf32 {
        return Err(Error("ELF64 (RV64) is not supported yet".into()));
    }
    let mut out = String::new();
    let mut warnings = Vec::new();
    let _ = writeln!(out, "\n{name}:\tfile format elf{}-littleriscv", bits(&elf));
    if opts.file_headers {
        file_headers(&mut out, &elf);
    }
    if opts.private_headers {
        program_headers(&mut out, &elf);
    }
    if opts.section_headers {
        section_headers(&mut out, &elf);
    }
    if opts.symbols {
        symbol_table(&mut out, &elf)?;
    }
    if opts.relocs && !opts.disassemble {
        relocations(&mut out, &elf)?;
    }
    if opts.contents {
        contents(&mut out, &elf)?;
    }
    if opts.disassemble {
        disasm::disassemble(&mut out, &mut warnings, &elf, opts)?;
    }
    Ok((out, warnings))
}

/// 32 for an ELF32 file (RV32 code), 64 for an ELF64 one (RV64).
fn bits(elf: &Elf) -> u32 {
    match elf.class {
        elf::Class::Elf32 => 32,
        elf::Class::Elf64 => 64,
    }
}

/// The width the file's code is for, from its class.
pub(crate) fn xlen(elf: &Elf) -> excalibur::Xlen {
    match elf.class {
        elf::Class::Elf32 => excalibur::Xlen::Rv32,
        elf::Class::Elf64 => excalibur::Xlen::Rv64,
    }
}

/// How many hex digits llvm-objdump gives an address (or a symbol's size)
/// in its columns: 8 in an ELF32 file, 16 in an ELF64 one.
pub(crate) fn addr_width(elf: &Elf) -> usize {
    bits(elf) as usize / 4
}

fn file_headers(out: &mut String, elf: &Elf) {
    let w = addr_width(elf);
    let _ = writeln!(out, "architecture: riscv{}", bits(elf));
    let _ = writeln!(out, "start address: 0x{:0w$x}", elf.header.entry);
}

fn program_headers(out: &mut String, elf: &Elf) {
    let w = addr_width(elf);
    let _ = writeln!(out, "\nProgram Header:");
    for p in &elf.segments {
        let ty = match p.ty {
            elf::PT_DYNAMIC => " DYNAMIC",
            elf::PT_GNU_EH_FRAME => "EH_FRAME",
            elf::PT_GNU_RELRO => "   RELRO",
            elf::PT_GNU_STACK => "   STACK",
            elf::PT_INTERP => "  INTERP",
            elf::PT_LOAD => "    LOAD",
            elf::PT_NOTE => "    NOTE",
            elf::PT_PHDR => "    PHDR",
            elf::PT_TLS => "     TLS",
            _ => " UNKNOWN",
        };
        // llvm-objdump prints log2 of the alignment; 0 has 64 trailing zero bits.
        let align = p.align.trailing_zeros();
        let flag = |bit, c| if p.flags & bit != 0 { c } else { '-' };
        let _ = writeln!(
            out,
            "{ty} off    0x{:0w$x} vaddr 0x{:0w$x} paddr 0x{:0w$x} align 2**{align}",
            p.offset, p.vaddr, p.paddr
        );
        let _ = writeln!(
            out,
            "         filesz 0x{:0w$x} memsz 0x{:0w$x} flags {}{}{}",
            p.filesz,
            p.memsz,
            flag(elf::PF_R, 'r'),
            flag(elf::PF_W, 'w'),
            flag(elf::PF_X, 'x')
        );
    }
    let _ = writeln!(out, "\nDynamic Section:");
}

/// The section-type column of `-h`.
fn section_kind(s: &Section) -> String {
    let mut kinds = Vec::new();
    if s.is_exec() {
        kinds.push("TEXT");
    }
    // Like LLVM's isSectionData: also .init_array, alloc notes and so on.
    if s.is_alloc() && !s.is_exec() && !s.is_nobits() {
        kinds.push("DATA");
    }
    if s.is_nobits() && s.flags & (elf::SHF_ALLOC | elf::SHF_WRITE) != 0 {
        kinds.push("BSS");
    }
    if s.name.starts_with(".debug") || s.name.starts_with(".zdebug") || s.name == ".gdb_index" {
        kinds.push("DEBUG");
    }
    kinds.join(", ")
}

fn section_headers(out: &mut String, elf: &Elf) {
    let width = elf
        .sections
        .iter()
        .map(|s| s.name.len())
        .max()
        .unwrap_or(0)
        .max(13);
    // The size takes 8 digits on both widths, the address the class's.
    let w = addr_width(elf);
    let _ = writeln!(out, "\nSections:");
    let _ = writeln!(out, "Idx {:<width$} Size     {:<w$} Type", "Name", "VMA");
    for (i, s) in elf.sections.iter().enumerate() {
        let _ = writeln!(
            out,
            "{i:3} {:<width$} {:08x} {:0w$x} {}",
            s.name,
            s.size,
            s.addr,
            section_kind(s)
        );
    }
}

/// Name of the section a symbol is defined in, or `*ABS*` / `*UND*` / `*COM*`.
fn symbol_section<'a>(elf: &'a Elf, s: &Symbol) -> &'a str {
    match s.shndx {
        elf::SHN_ABS => "*ABS*",
        elf::SHN_COMMON => "*COM*",
        elf::SHN_UNDEF => "*UND*",
        i => elf.sections.get(i as usize).map_or("*UND*", |s| s.name.as_str()),
    }
}

/// The name a symbol is shown with: section symbols take their section's name.
fn symbol_name<'a>(elf: &'a Elf, s: &'a Symbol) -> &'a str {
    match (s.ty(), s.section()) {
        (elf::STT_SECTION, Some(i)) => elf.sections.get(i).map_or("", |s| s.name.as_str()),
        _ => &s.name,
    }
}

fn symbol_table(out: &mut String, elf: &Elf) -> Result<(), Error> {
    let w = addr_width(elf);
    let _ = writeln!(out, "\nSYMBOL TABLE:");
    for s in elf.symbols()?.iter().skip(1) {
        let weak = s.bind() == elf::STB_WEAK;
        let common = s.shndx == elf::SHN_COMMON;
        let glob_loc = match s.bind() {
            10 => 'u', // STB_GNU_UNIQUE
            _ if weak || (s.section().is_none() && s.shndx != elf::SHN_ABS) => ' ',
            elf::STB_LOCAL => 'l',
            _ => 'g',
        };
        let ifunc = if s.ty() == 10 { 'i' } else { ' ' }; // STT_GNU_IFUNC
        let debug = if matches!(s.ty(), elf::STT_SECTION | elf::STT_FILE) {
            'd'
        } else {
            ' '
        };
        let kind = match s.ty() {
            elf::STT_FILE => 'f',
            elf::STT_FUNC => 'F',
            elf::STT_OBJECT | elf::STT_COMMON | elf::STT_TLS => 'O',
            _ => ' ',
        };
        let weak = if weak { 'w' } else { ' ' };
        let visibility = match s.other {
            0 => String::new(),
            1 => " .internal".to_string(),
            2 => " .hidden".to_string(),
            3 => " .protected".to_string(),
            o => format!(" 0x{o:02x}"),
        };
        let _ = writeln!(
            out,
            "{:0w$x} {glob_loc}{weak}  {ifunc}{debug}{kind} {}\t{:0w$x}{visibility} {}",
            // A common symbol's st_value is its alignment; llvm-objdump shows
            // its size in the first column and the alignment in the second.
            if common { s.size } else { s.value },
            symbol_section(elf, s),
            if common { s.value } else { s.size },
            symbol_name(elf, s)
        );
    }
    Ok(())
}

/// One `.rela.*` (or `.rel.*`) section, read.
pub(crate) struct RelocSection {
    /// Index of the section the relocations patch (`sh_info`).
    pub target: usize,
    /// The symbol table they refer to (`sh_link`).
    pub symbols: Vec<Symbol>,
    /// In file order, as `-r` prints them (`-dr` sorts its own copy).
    pub relocs: Vec<elf::Rela>,
}

/// Every relocation section in the file, in file order.
pub(crate) fn relocation_sections(elf: &Elf) -> Result<Vec<RelocSection>, Error> {
    let mut out = Vec::new();
    for s in &elf.sections {
        if s.ty == elf::SHT_RELA || s.ty == elf::SHT_REL {
            let relocs = elf.relocations(s)?;
            let symbols = match elf.sections.get(s.link as usize) {
                Some(symtab) if symtab.ty == elf::SHT_SYMTAB => elf.symbols_in(symtab)?,
                _ => elf.symbols()?,
            };
            out.push(RelocSection {
                target: s.info as usize,
                symbols,
                relocs,
            });
        }
    }
    Ok(out)
}

/// `R_RISCV_PCREL_HI20`, or `Unknown` for a type we have no name for.
pub(crate) fn reloc_type(ty: u32) -> &'static str {
    elf::reloc_name(ty).unwrap_or("Unknown")
}

/// What a relocation points at: `symbol`, `symbol+0x8`, `.rodata-0x4`, or `*ABS*`.
pub(crate) fn reloc_value(elf: &Elf, symbols: &[Symbol], r: &elf::Rela) -> String {
    let mut v = match symbols.get(r.sym as usize).filter(|_| r.sym != 0) {
        Some(s) => symbol_name(elf, s).to_string(),
        None => "*ABS*".to_string(),
    };
    if r.addend > 0 {
        let _ = write!(v, "+{:#x}", r.addend);
    } else if r.addend < 0 {
        let _ = write!(v, "-{:#x}", r.addend.unsigned_abs());
    }
    v
}

fn relocations(out: &mut String, elf: &Elf) -> Result<(), Error> {
    let w = addr_width(elf);
    for rs in relocation_sections(elf)? {
        let name = elf.sections.get(rs.target).map_or("", |s| s.name.as_str());
        let _ = writeln!(out, "\nRELOCATION RECORDS FOR [{name}]:");
        let _ = writeln!(out, "{:<w$} TYPE                     VALUE", "OFFSET");
        for r in &rs.relocs {
            let _ = writeln!(
                out,
                "{:0w$x} {:<24} {}",
                r.offset,
                reloc_type(r.ty),
                reloc_value(elf, &rs.symbols, r)
            );
        }
    }
    Ok(())
}

fn contents(out: &mut String, elf: &Elf) -> Result<(), Error> {
    for s in elf.sections.iter().filter(|s| s.size > 0) {
        let _ = writeln!(out, "Contents of section {}:", s.name);
        if s.is_nobits() && s.flags & (elf::SHF_ALLOC | elf::SHF_WRITE) != 0 {
            let end = s.addr + s.size;
            let _ = writeln!(
                out,
                "<skipping contents of bss section at [{:04x}, {end:04x})>",
                s.addr
            );
            continue;
        }
        let data = elf.section_data(s)?;
        for (row, chunk) in data.chunks(16).enumerate() {
            let _ = write!(out, " {:04x} ", s.addr + row as u64 * 16);
            for i in 0..16 {
                if i != 0 && i % 4 == 0 {
                    out.push(' ');
                }
                match chunk.get(i) {
                    Some(b) => {
                        let _ = write!(out, "{b:02x}");
                    }
                    None => out.push_str("  "),
                }
            }
            out.push_str("  ");
            out.extend(
                chunk
                    .iter()
                    .map(|&b| if (0x20..0x7f).contains(&b) { b as char } else { '.' }),
            );
            out.push('\n');
        }
    }
    Ok(())
}
