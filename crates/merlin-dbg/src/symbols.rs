//! The program's symbols, from its ELF symbol table (through `elf`): names for
//! addresses (`break main`) and addresses for names (`0x80000024 <puts+4>`). Programs
//! of either width: an ELF32 file's addresses are 32-bit, an ELF64 one's 64-bit.

use crate::remote::hex_value;
use excalibur::Xlen;
use std::collections::HashMap;

/// `STT_SECTION` and `STT_FILE`: symbols that name no place in the program.
const STT_SECTION: u8 = 3;
const STT_FILE: u8 = 4;
/// `STT_FUNC`.
const STT_FUNC: u8 = 2;
/// `STB_LOCAL`.
const STB_LOCAL: u8 = 0;

/// The width a program is for: its ELF class (ELF32 for RV32, ELF64 for RV64).
pub fn elf_xlen(file: &[u8]) -> Result<Xlen, String> {
    let elf = elf::Elf::parse(file).map_err(|e| e.to_string())?;
    Ok(match elf.class {
        elf::Class::Elf32 => Xlen::Rv32,
        elf::Class::Elf64 => Xlen::Rv64,
    })
}

/// The error for a program of the other width than the machine's.
pub fn check_xlen(file: &[u8], machine: Xlen) -> Result<(), String> {
    let program = elf_xlen(file)?;
    if program == machine {
        return Ok(());
    }
    let bits = program.bits();
    Err(format!(
        "an RV{bits} program (ELF{bits}), but the machine is RV{}",
        machine.bits()
    ))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Symbol {
    name: String,
    addr: u64,
    /// 0 when the symbol says nothing about its size (an assembly label).
    size: u64,
}

#[derive(Default)]
pub struct Symbols {
    /// By address; of several symbols at one address only the best name is kept.
    sorted: Vec<Symbol>,
    by_name: HashMap<String, u64>,
    /// Every symbol of every file added: (address, rank, name, size).
    candidates: Vec<(u64, (bool, bool), String, u64)>,
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
            // Of several at one address: a function first, then a global name.
            let rank = (s.ty() != STT_FUNC, s.bind() == STB_LOCAL);
            found.push((s.value, rank, s.name.clone(), s.size));
        }
        // A name defined twice in the file (two files' `static` functions):
        // the lowest address.
        found.sort();
        for (addr, _, name, _) in &found {
            self.by_name.entry(name.clone()).or_insert(*addr);
        }
        self.candidates.extend(found);
        let mut all = self.candidates.clone();
        all.sort();
        self.sorted.clear();
        for (addr, _, name, size) in all {
            if self.sorted.last().is_none_or(|s| s.addr != addr) {
                self.sorted.push(Symbol { name, addr, size });
            }
        }
        Ok(())
    }

    pub fn is_empty(&self) -> bool {
        self.by_name.is_empty()
    }

    /// The address of symbol `name`.
    pub fn lookup(&self, name: &str) -> Option<u64> {
        self.by_name.get(name).copied()
    }

    /// The symbol at or before `addr`, if `addr` can be in it: inside its size, or
    /// before the next symbol when it has none. Its name and address.
    pub fn containing(&self, addr: u64) -> Option<(&str, u64)> {
        let i = self.sorted.partition_point(|s| s.addr <= addr).checked_sub(1)?;
        let s = &self.sorted[i];
        if s.size != 0 && addr - s.addr >= s.size {
            return None;
        }
        Some((&s.name, s.addr))
    }

    /// The address range of the symbol holding `addr`: its size, or up to the
    /// next symbol when it has none (or the end of the address space).
    pub fn range(&self, addr: u64) -> Option<(u64, u64)> {
        let i = self.sorted.partition_point(|s| s.addr <= addr).checked_sub(1)?;
        let s = &self.sorted[i];
        let end = match s.size {
            0 => self.sorted.get(i + 1).map_or(u64::MAX, |n| n.addr),
            size => s.addr.saturating_add(size),
        };
        (addr < end).then_some((s.addr, end))
    }

    /// `name` or `name+off` for `addr`, if a symbol holds it.
    pub fn describe(&self, addr: u64) -> Option<String> {
        let (name, start) = self.containing(addr)?;
        Some(match addr - start {
            0 => name.to_string(),
            off => format!("{name}+{off}"),
        })
    }

    /// `0x80000024 <puts+4>`, or the bare address when no symbol holds it, with as
    /// many digits as an `xlen` address has (`0x0000000080000024 <puts+4>` on RV64,
    /// as GDB shows a 64-bit target's code addresses).
    pub fn show(&self, addr: u64, xlen: Xlen) -> String {
        self.with_name(hex_value(addr, xlen), addr)
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

    /// hello's symbols, RV32 (ELF32) and RV64 (ELF64): the same addresses.
    #[test]
    fn names_and_addresses() {
        for (file, xlen) in [
            ("examples/hello/hello.elf", Xlen::Rv32),
            ("examples/hello/hello-rv64.elf", Xlen::Rv64),
        ] {
            let elf = read(file);
            assert_eq!(elf_xlen(&elf), Ok(xlen));
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
        }
        let s = Symbols::from_elf(&read("examples/hello/hello.elf")).unwrap();
        assert_eq!(s.show(0x8000_0024, Xlen::Rv32), "0x80000024 <puts+4>");
        assert_eq!(s.show(0x1000, Xlen::Rv32), "0x00001000");
        assert_eq!(s.show(0x8000_0024, Xlen::Rv64), "0x0000000080000024 <puts+4>");
        assert_eq!(s.show(0x1000, Xlen::Rv64), "0x0000000000001000");
        assert_eq!(s.show_plain(0x8000_0024), "0x80000024 <puts+4>");
        assert_eq!(s.show_plain(0x1000), "0x1000");
    }

    /// A program of the other width is refused against the machine's.
    #[test]
    fn widths_must_agree() {
        let rv32 = read("examples/hello/hello.elf");
        let rv64 = read("examples/hello/hello-rv64.elf");
        assert_eq!(check_xlen(&rv32, Xlen::Rv32), Ok(()));
        assert_eq!(check_xlen(&rv64, Xlen::Rv64), Ok(()));
        assert_eq!(
            check_xlen(&rv32, Xlen::Rv64),
            Err("an RV32 program (ELF32), but the machine is RV64".into())
        );
        assert_eq!(
            check_xlen(&rv64, Xlen::Rv32),
            Err("an RV64 program (ELF64), but the machine is RV32".into())
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
