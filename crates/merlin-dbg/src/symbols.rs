//! The program's symbols, from its ELF symbol table (through `elf`): names for
//! addresses (`break main`) and addresses for names (`0x80000024 <puts+4>`). The
//! programs are RV64's, ELF64 files ([`check_program`]).

use crate::remote::hex_value;
use std::collections::HashMap;

/// `STT_SECTION` and `STT_FILE`: symbols that name no place in the program.
const STT_SECTION: u8 = 3;
const STT_FILE: u8 = 4;
/// `STT_FUNC`.
const STT_FUNC: u8 = 2;
/// `STB_LOCAL`.
const STB_LOCAL: u8 = 0;

/// Checks that a program is one merlin-dbg debugs: an ELF64 file, for an RV64
/// machine. An ELF32 one is RV32 code, which no Excalibur machine runs.
pub fn check_program(file: &[u8]) -> Result<(), String> {
    let elf = elf::Elf::parse(file).map_err(|e| e.to_string())?;
    match elf.class {
        elf::Class::Elf64 => Ok(()),
        elf::Class::Elf32 => {
            Err("an RV32 program (ELF32): merlin-dbg debugs RV64 (ELF64) programs only".into())
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Symbol {
    addr: u64,
    /// Of several symbols at one address, the lowest is kept: a function before
    /// anything else, then a global name before a local one.
    rank: (bool, bool),
    name: String,
    /// 0 when the symbol says nothing about its size (an assembly label).
    size: u64,
    /// The end of the section the symbol is in: a symbol without a size covers
    /// the addresses up to the next symbol, but not past its section, as GDB's
    /// minimal symbols do (the stack beyond the last section has no name).
    section_end: u64,
    /// Whether the section is code (`SHF_EXECINSTR`): `break` skips the prologue
    /// of a symbol there, as GDB does of a text symbol.
    code: bool,
}

#[derive(Default)]
pub struct Symbols {
    /// By address; of several symbols at one address only the best name is kept.
    sorted: Vec<Symbol>,
    /// Each name's symbol, an index into `all`.
    by_name: HashMap<String, usize>,
    /// Every symbol of every file added.
    all: Vec<Symbol>,
}

impl Symbols {
    /// The symbols of an ELF file. Only those defined in a section count: not absolute
    /// ones (`.equ` constants), not section or file symbols, not the assembler's local
    /// labels (`.L...`).
    pub fn from_elf(file: &[u8]) -> Result<Symbols, String> {
        let mut out = Symbols::default();
        out.add_elf(file)?;
        Ok(out)
    }

    /// Adds the symbols of another ELF file (`add-symbol-file`): a user program
    /// next to the kernel. Of a name defined in both, the first file's is kept.
    pub fn add_elf(&mut self, file: &[u8]) -> Result<(), String> {
        let elf = elf::Elf::parse(file).map_err(|e| e.to_string())?;
        let mut found = Vec::new();
        for s in elf.symbols().map_err(|e| e.to_string())? {
            if s.name.is_empty()
                || s.name.starts_with(".L")
                || s.name.starts_with('$')
                || s.section().is_none()
                || matches!(s.ty(), STT_SECTION | STT_FILE)
            {
                continue;
            }
            let section = s.section().and_then(|i| elf.sections.get(i));
            found.push(Symbol {
                addr: s.value,
                // Of several at one address: a function first, then a global name.
                rank: (s.ty() != STT_FUNC, s.bind() == STB_LOCAL),
                name: s.name.clone(),
                size: s.size,
                section_end: section.map_or(u64::MAX, |sec| sec.addr.saturating_add(sec.size)),
                code: section.is_some_and(|sec| sec.is_exec()),
            });
        }
        // A name defined twice in the file (two files' `static` functions):
        // the lowest address.
        found.sort();
        for s in found {
            self.by_name.entry(s.name.clone()).or_insert(self.all.len());
            self.all.push(s);
        }
        let mut sorted = self.all.clone();
        sorted.sort();
        sorted.dedup_by_key(|s| s.addr);
        self.sorted = sorted;
        Ok(())
    }

    pub fn is_empty(&self) -> bool {
        self.by_name.is_empty()
    }

    /// The address of symbol `name`.
    pub fn lookup(&self, name: &str) -> Option<u64> {
        self.by_name.get(name).map(|&i| self.all[i].addr)
    }

    /// The size symbol `name` has in the symbol table, if it has one (not 0).
    pub fn size(&self, name: &str) -> Option<u64> {
        let s = &self.all[*self.by_name.get(name)?];
        (s.size != 0).then_some(s.size)
    }

    /// The address of symbol `name` if it is in code (a function, or an assembly
    /// label in `.text`): what `break NAME` skips the prologue of.
    pub fn code(&self, name: &str) -> Option<u64> {
        let s = &self.all[*self.by_name.get(name)?];
        s.code.then_some(s.addr)
    }

    /// The symbol at or before `addr` whose range ([`Symbols::range`]) holds it, and
    /// that range.
    fn holding(&self, addr: u64) -> Option<(&Symbol, u64)> {
        let i = self.sorted.partition_point(|s| s.addr <= addr).checked_sub(1)?;
        let s = &self.sorted[i];
        let end = match s.size {
            0 => {
                let next = self.sorted.get(i + 1).map_or(u64::MAX, |n| n.addr);
                // A symbol at the very end of its section (`_end`) still names its
                // own address.
                next.min(s.section_end.max(s.addr.saturating_add(1)))
            }
            size => s.addr.saturating_add(size),
        };
        (addr < end).then_some((s, end))
    }

    /// The symbol at or before `addr`, if `addr` can be in it: inside its size, or
    /// before the next symbol and the end of its section when it has none. Its name
    /// and address.
    pub fn containing(&self, addr: u64) -> Option<(&str, u64)> {
        self.holding(addr).map(|(s, _)| (s.name.as_str(), s.addr))
    }

    /// The address range of the symbol holding `addr`: its size, or up to the
    /// next symbol (or the end of its section) when it has none.
    pub fn range(&self, addr: u64) -> Option<(u64, u64)> {
        self.holding(addr).map(|(s, end)| (s.addr, end))
    }

    /// `name` or `name+off` for `addr`, if a symbol holds it.
    pub fn describe(&self, addr: u64) -> Option<String> {
        let (name, start) = self.containing(addr)?;
        Some(match addr - start {
            0 => name.to_string(),
            off => format!("{name}+{off}"),
        })
    }

    /// `0x0000000080000024 <puts+4>`, or the bare address when no symbol holds it,
    /// with all of a 64-bit address's digits, as GDB shows a 64-bit target's code
    /// addresses.
    pub fn show(&self, addr: u64) -> String {
        self.with_name(hex_value(addr), addr)
    }

    /// The same without leading zeros (`0x1000`, `0x80000024 <puts+4>`), as GDB
    /// shows an address outside frames and disassembly.
    pub fn show_plain(&self, addr: u64) -> String {
        self.with_name(format!("{addr:#x}"), addr)
    }

    fn with_name(&self, hex: String, addr: u64) -> String {
        match self.describe(addr) {
            Some(d) => format!("{hex} <{d}>"),
            None => hex,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(rel: &str) -> Vec<u8> {
        std::fs::read(format!("{}/../../{rel}", env!("CARGO_MANIFEST_DIR"))).unwrap()
    }

    /// hello's symbols.
    #[test]
    fn names_and_addresses() {
        let elf = read("examples/hello/hello.elf");
        assert_eq!(check_program(&elf), Ok(()));
        let s = Symbols::from_elf(&elf).unwrap();
        assert_eq!(s.lookup("_start"), Some(0x8000_0000));
        assert_eq!(s.lookup("puts"), Some(0x8000_0020));
        assert_eq!(s.lookup("message"), Some(0x8000_0050));
        assert_eq!(s.lookup("UART_BASE"), None, "absolute");
        assert_eq!(s.lookup(".Lpcrel_hi0"), None, "local label");
        assert_eq!(s.describe(0x8000_0000).as_deref(), Some("_start"));
        assert_eq!(s.describe(0x8000_0018).as_deref(), Some("_start+24"));
        assert_eq!(s.describe(0x8000_0024).as_deref(), Some("puts+4"));
        assert_eq!(s.describe(0x7fff_fffc), None);
        // message, a label without a size, ends with .rodata: past it (the
        // stack) has no name, as with GDB.
        assert_eq!(s.describe(0x8000_0064).as_deref(), Some("message+20"));
        assert_eq!(s.describe(0x8001_0000), None);
        assert_eq!(s.code("puts"), Some(0x8000_0020));
        assert_eq!(s.code("message"), None, "data");
        assert_eq!(s.code("nowhere"), None);
        assert_eq!(s.size("message"), None, "a label has no size");
        let s = Symbols::from_elf(&read("examples/hello/hello.elf")).unwrap();
        assert_eq!(s.show(0x8000_0024), "0x0000000080000024 <puts+4>");
        assert_eq!(s.show(0x1000), "0x0000000000001000");
        assert_eq!(s.show_plain(0x8000_0024), "0x80000024 <puts+4>");
        assert_eq!(s.show_plain(0x1000), "0x1000");
    }

    /// An RV32 program (clang's, `crates/elf/tests/llvm`) is refused.
    #[test]
    fn programs_are_rv64() {
        assert_eq!(check_program(&read("examples/hello/hello.elf")), Ok(()));
        assert_eq!(
            check_program(&read("crates/elf/tests/llvm/rv32/prog.elf")),
            Err("an RV32 program (ELF32): merlin-dbg debugs RV64 (ELF64) programs only".into())
        );
    }

    /// clang's RV64 program (`crates/elf/tests/llvm`): sized symbols, and the line
    /// tables, as llvm-dwarfdump reads them (`rv64/prog.elf.lines` there).
    #[test]
    fn an_rv64_program_from_clang() {
        let elf = read("crates/elf/tests/llvm/rv64/prog.elf");
        let s = Symbols::from_elf(&elf).unwrap();
        assert_eq!(s.lookup("sum"), Some(0x8000_0000));
        assert_eq!(s.lookup("twice"), Some(0x8000_008c));
        assert_eq!(s.describe(0x8000_0090).as_deref(), Some("twice+4"));
        assert_eq!(s.range(0x8000_0090), Some((0x8000_008c, 0x8000_00b8)));
        assert_eq!(s.lookup("counter"), Some(0x8000_2178));
        // An int's size, which `watch counter` watches; a function's.
        assert_eq!(s.size("counter"), Some(4));
        assert_eq!(s.size("twice"), Some(0x2c));
        let mut lines = crate::source::Lines::default();
        lines.add_elf(&elf).unwrap();
        let at = |a| lines.at(a).map(|p| p.to_string());
        assert_eq!(at(0x8000_0000).as_deref(), Some("/camelot/prog.c:15"));
        assert_eq!(at(0x8000_0024).as_deref(), Some("/camelot/prog.c:17"));
        assert_eq!(at(0x8000_00a4).as_deref(), Some("/camelot/prog.c:12"));
        assert!(lines.starts_statement(0x8000_0018));
        assert!(!lines.starts_statement(0x8000_0024));
        // sum's prologue ends at its prologue_end row; twice's too.
        assert_eq!(lines.after_prologue(0x8000_0000, 0x8000_008c), 0x8000_0018);
        assert_eq!(lines.after_prologue(0x8000_008c, 0x8000_00b8), 0x8000_00a0);
    }

    #[test]
    fn sized_symbols_end() {
        let file = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../../kernel/kernel.elf")).unwrap();
        let s = Symbols::from_elf(&file).unwrap();
        let kmain = s.lookup("kmain").unwrap();
        assert_eq!(s.describe(kmain + 8).unwrap(), "kmain+8");
        assert_eq!(s.containing(kmain + 4), Some(("kmain", kmain)));
    }
}
