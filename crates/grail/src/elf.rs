//! Loads executables: PT_LOAD segments, the entry point and symbol values.
//! The parsing lives in the shared `elf32` crate.

pub use elf32::Error as ElfError;

pub struct Segment {
    pub paddr: u32,
    pub data: Vec<u8>,
    /// Size in memory; bytes past `data.len()` are zero (.bss).
    pub mem_size: u32,
}

pub struct Elf {
    pub entry: u32,
    pub segments: Vec<Segment>,
    /// Defined symbols: the global and weak ones, then the locals.
    pub symbols: Vec<(String, u32)>,
}

pub fn parse(file: &[u8]) -> Result<Elf, ElfError> {
    let elf = elf32::Elf::parse(file)?;
    let mut segments = Vec::new();
    for p in elf.segments.iter().filter(|p| p.ty == elf32::PT_LOAD) {
        segments.push(Segment {
            paddr: p.paddr,
            data: elf.segment_data(p)?.to_vec(),
            mem_size: p.memsz,
        });
    }
    // Global and weak symbols first, then the locals: a name defined by
    // both (a `static` function in one file named like a global one in
    // another) means the global one, as in `elf32::find_symbol`.
    let mut defined: Vec<elf32::Symbol> = elf
        .symbols()?
        .into_iter()
        .filter(|s| !s.name.is_empty() && !s.is_undefined())
        .collect();
    defined.sort_by_key(|s| s.bind() == elf32::STB_LOCAL);
    let symbols = defined.into_iter().map(|s| (s.name, s.value)).collect();
    Ok(Elf {
        entry: elf.header.entry,
        segments,
        symbols,
    })
}

impl Elf {
    pub fn symbol(&self, name: &str) -> Option<u32> {
        self.symbols.iter().find(|(n, _)| n == name).map(|&(_, v)| v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use elf32::*;

    /// A section-header-only ELF whose .symtab holds `syms` (name, value,
    /// binding), every one defined (SHN_ABS), after the null symbol.
    fn elf_with_symbols(syms: &[(&str, u32, u8)]) -> Vec<u8> {
        let mut strtab = vec![0u8];
        let mut symtab = vec![0u8; SYM_SIZE];
        for &(name, value, bind) in syms {
            symtab.extend_from_slice(&(strtab.len() as u32).to_le_bytes());
            strtab.extend_from_slice(name.as_bytes());
            strtab.push(0);
            symtab.extend_from_slice(&value.to_le_bytes());
            symtab.extend_from_slice(&0u32.to_le_bytes());
            symtab.extend_from_slice(&[(bind << 4) | STT_FUNC, 0]);
            symtab.extend_from_slice(&SHN_ABS.to_le_bytes());
        }
        let shstrtab = b"\0.symtab\0.strtab\0.shstrtab\0";
        let mut out = vec![0u8; EHDR_SIZE];
        let symtab_off = out.len() as u32;
        out.extend_from_slice(&symtab);
        let strtab_off = out.len() as u32;
        out.extend_from_slice(&strtab);
        let shstrtab_off = out.len() as u32;
        out.extend_from_slice(shstrtab);
        let shoff = out.len() as u32;
        // sh_info of a .symtab: one past the last local symbol.
        let locals = 1 + syms.iter().take_while(|s| s.2 == STB_LOCAL).count() as u32;
        // (name, type, offset, size, link, info, entsize)
        let shdrs = [
            (0, SHT_NULL, 0, 0, 0, 0, 0),
            (
                1,
                SHT_SYMTAB,
                symtab_off,
                symtab.len() as u32,
                2,
                locals,
                SYM_SIZE as u32,
            ),
            (9, SHT_STRTAB, strtab_off, strtab.len() as u32, 0, 0, 0),
            (17, SHT_STRTAB, shstrtab_off, shstrtab.len() as u32, 0, 0, 0),
        ];
        for (name, ty, off, size, link, info, entsize) in shdrs {
            for v in [name, ty, 0, 0, off, size, link, info, 4, entsize] {
                out.extend_from_slice(&v.to_le_bytes());
            }
        }
        let mut h = b"\x7fELF\x01\x01\x01\0\0\0\0\0\0\0\0\0".to_vec();
        for v in [ET_EXEC, EM_RISCV] {
            h.extend_from_slice(&v.to_le_bytes());
        }
        for v in [1u32, 0x8000_0000, 0, shoff, 0] {
            h.extend_from_slice(&v.to_le_bytes());
        }
        for v in [EHDR_SIZE, PHDR_SIZE, 0, SHDR_SIZE, shdrs.len(), 3] {
            h.extend_from_slice(&(v as u16).to_le_bytes());
        }
        out[..EHDR_SIZE].copy_from_slice(&h);
        out
    }

    #[test]
    fn a_global_symbol_wins_over_a_local_of_the_same_name() {
        // A `static void start(...)` in one file and the global `start` in
        // another: an executable's .symtab lists the locals first.
        let elf = parse(&elf_with_symbols(&[
            ("start", 0x10, STB_LOCAL),
            ("only_local", 0x14, STB_LOCAL),
            ("start", 0x20, STB_GLOBAL),
            ("weak", 0x30, STB_WEAK),
        ]))
        .unwrap();
        assert_eq!(elf.entry, 0x8000_0000);
        assert_eq!(elf.symbol("start"), Some(0x20));
        assert_eq!(elf.symbol("only_local"), Some(0x14));
        assert_eq!(elf.symbol("weak"), Some(0x30));
        assert_eq!(elf.symbol("nothing"), None);
    }
}
