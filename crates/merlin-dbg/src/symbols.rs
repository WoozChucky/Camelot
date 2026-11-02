//! The program's symbols, from its ELF symbol table (through `elf32`): names for
//! addresses (`break main`) and addresses for names (`0x80000024 <puts+4>`).

use std::collections::HashMap;

/// `STT_SECTION` and `STT_FILE`: symbols that name no place in the program.
const STT_SECTION: u8 = 3;
const STT_FILE: u8 = 4;
/// `STT_FUNC`.
const STT_FUNC: u8 = 2;
/// `STB_LOCAL`.
const STB_LOCAL: u8 = 0;

#[derive(Debug, Clone, PartialEq, Eq)]
struct Symbol {
    name: String,
    addr: u32,
    /// 0 when the symbol says nothing about its size (an assembly label).
    size: u32,
}

#[derive(Default)]
pub struct Symbols {
    /// By address; of several symbols at one address only the best name is kept.
    sorted: Vec<Symbol>,
    by_name: HashMap<String, u32>,
}

impl Symbols {
    /// The symbols of an ELF file. Only those defined in a section count: not absolute
    /// ones (`.equ` constants), not section or file symbols, not the assembler's local
    /// labels (`.L...`).
    pub fn from_elf(file: &[u8]) -> Result<Symbols, String> {
        let elf = elf32::Elf::parse(file).map_err(|e| e.to_string())?;
        let mut candidates = Vec::new();
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
            candidates.push((s.value, rank, s.name.clone(), s.size));
        }
        candidates.sort();
        let mut out = Symbols::default();
        for (addr, _, name, size) in candidates {
            out.by_name.entry(name.clone()).or_insert(addr);
            if out.sorted.last().is_none_or(|s| s.addr != addr) {
                out.sorted.push(Symbol { name, addr, size });
            }
        }
        Ok(out)
    }

    pub fn is_empty(&self) -> bool {
        self.by_name.is_empty()
    }

    /// The address of symbol `name`.
    pub fn lookup(&self, name: &str) -> Option<u32> {
        self.by_name.get(name).copied()
    }

    /// The symbol at or before `addr`, if `addr` can be in it: inside its size, or
    /// before the next symbol when it has none. Its name and address.
    pub fn containing(&self, addr: u32) -> Option<(&str, u32)> {
        let i = self.sorted.partition_point(|s| s.addr <= addr).checked_sub(1)?;
        let s = &self.sorted[i];
        if s.size != 0 && addr - s.addr >= s.size {
            return None;
        }
        Some((&s.name, s.addr))
    }

    /// `name` or `name+off` for `addr`, if a symbol holds it.
    pub fn describe(&self, addr: u32) -> Option<String> {
        let (name, start) = self.containing(addr)?;
        Some(match addr - start {
            0 => name.to_string(),
            off => format!("{name}+{off}"),
        })
    }

    /// `0x80000024 <puts+4>`, or the bare address when no symbol holds it.
    pub fn show(&self, addr: u32) -> String {
        match self.describe(addr) {
            Some(d) => format!("{addr:#010x} <{d}>"),
            None => format!("{addr:#010x}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hello() -> Symbols {
        let file = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../examples/hello/hello.elf"
        ))
        .unwrap();
        Symbols::from_elf(&file).unwrap()
    }

    #[test]
    fn names_and_addresses() {
        let s = hello();
        assert_eq!(s.lookup("_start"), Some(0x8000_0000));
        assert_eq!(s.lookup("puts"), Some(0x8000_0020));
        assert_eq!(s.lookup("message"), Some(0x8000_0050));
        assert_eq!(s.lookup("UART_BASE"), None, "absolute");
        assert_eq!(s.lookup(".Lpcrel_hi0"), None, "local label");
        assert_eq!(s.describe(0x8000_0000).as_deref(), Some("_start"));
        assert_eq!(s.describe(0x8000_0018).as_deref(), Some("_start+24"));
        assert_eq!(s.describe(0x8000_0024).as_deref(), Some("puts+4"));
        assert_eq!(s.describe(0x7fff_fffc), None);
        assert_eq!(s.show(0x8000_0024), "0x80000024 <puts+4>");
        assert_eq!(s.show(0x1000), "0x00001000");
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
