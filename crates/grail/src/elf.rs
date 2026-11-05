//! Loads executables: PT_LOAD segments, the entry point and symbol values.
//! The parsing lives in the shared `elf` crate, which reads ELF32 too; a machine runs
//! ELF64 (RV64) programs only, as an RV64 hart has no 32-bit mode (`mstatus.UXL` is
//! hard-wired to 64).
//!
//! This module is `grail::elf`; inside it, `elf::...` names the crate (a
//! module's own name is not in scope within it), as in `elf::Elf::parse`.

pub use elf::Error as ElfError;

/// A `PT_LOAD` segment.
pub struct Segment {
    /// Where it runs (`p_vaddr`).
    pub vaddr: u64,
    /// Where it is loaded (`p_paddr`).
    pub paddr: u64,
    pub data: Vec<u8>,
    /// Size in memory; bytes past `data.len()` are zero (.bss).
    pub mem_size: u64,
}

pub struct Elf {
    /// `e_entry`, a virtual address (see [`Elf::physical_entry`]).
    pub entry: u64,
    pub segments: Vec<Segment>,
    /// Defined symbols: the global and weak ones, then the locals.
    pub symbols: Vec<(String, u64)>,
}

/// Why an ELF32 program does not run.
pub const NO_ELF32: &str =
    "an ELF32 (RV32) program: Excalibur is RV64, and a machine runs ELF64 programs only";

/// Reads an executable for the machine: an ELF64 file ([`NO_ELF32`] for an ELF32 one).
pub fn parse(file: &[u8]) -> Result<Elf, ElfError> {
    let elf = elf::Elf::parse(file)?;
    if elf.class == elf::Class::Elf32 {
        return Err(ElfError(NO_ELF32.into()));
    }
    let mut segments = Vec::new();
    for p in elf.segments.iter().filter(|p| p.ty == elf::PT_LOAD) {
        segments.push(Segment {
            vaddr: p.vaddr,
            paddr: p.paddr,
            data: elf.segment_data(p)?.to_vec(),
            mem_size: p.memsz,
        });
    }
    // Global and weak symbols first, then the locals: a name defined by
    // both (a `static` function in one file named like a global one in
    // another) means the global one, as in `elf::find_symbol`.
    let mut defined: Vec<elf::Symbol> = elf
        .symbols()?
        .into_iter()
        .filter(|s| !s.name.is_empty() && !s.is_undefined())
        .collect();
    defined.sort_by_key(|s| s.bind() == elf::STB_LOCAL);
    let symbols = defined.into_iter().map(|s| (s.name, s.value)).collect();
    Ok(Elf {
        entry: elf.header.entry,
        segments,
        symbols,
    })
}

impl Elf {
    pub fn symbol(&self, name: &str) -> Option<u64> {
        self.symbols.iter().find(|(n, _)| n == name).map(|&(_, v)| v)
    }

    /// Where the program starts with paging off, as at reset: `e_entry` translated
    /// through the `PT_LOAD` segment whose virtual range holds it, so a kernel linked
    /// high and loaded low starts at its load address (spec section 7). An error if no
    /// segment holds it.
    pub fn physical_entry(&self) -> Result<u64, ElfError> {
        self.segments
            .iter()
            .find(|s| self.entry.wrapping_sub(s.vaddr) < s.mem_size)
            .map(|s| s.paddr.wrapping_add(self.entry - s.vaddr))
            .ok_or_else(|| ElfError("entry point outside the program's segments".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use elf::*;

    /// A section-header-only ELF64 file whose .symtab holds `syms` (name, value,
    /// binding), every one defined (SHN_ABS), after the null symbol.
    fn elf_with_symbols(syms: &[(&str, u64, u8)]) -> Vec<u8> {
        let mut strtab = vec![0u8];
        let mut symtab = vec![0u8; SYM64_SIZE];
        for &(name, value, bind) in syms {
            // An ELF64 symbol: name, info, other, shndx, value, size.
            symtab.extend_from_slice(&(strtab.len() as u32).to_le_bytes());
            strtab.extend_from_slice(name.as_bytes());
            strtab.push(0);
            symtab.extend_from_slice(&[(bind << 4) | STT_FUNC, 0]);
            symtab.extend_from_slice(&SHN_ABS.to_le_bytes());
            symtab.extend_from_slice(&value.to_le_bytes());
            symtab.extend_from_slice(&0u64.to_le_bytes());
        }
        let shstrtab = b"\0.symtab\0.strtab\0.shstrtab\0";
        let mut out = vec![0u8; EHDR64_SIZE];
        let symtab_off = out.len() as u64;
        out.extend_from_slice(&symtab);
        let strtab_off = out.len() as u64;
        out.extend_from_slice(&strtab);
        let shstrtab_off = out.len() as u64;
        out.extend_from_slice(shstrtab);
        let shoff = out.len() as u64;
        // sh_info of a .symtab: one past the last local symbol.
        let locals = 1 + syms.iter().take_while(|s| s.2 == STB_LOCAL).count() as u32;
        // (name, type, offset, size, link, info, entsize)
        let shdrs = [
            (0, SHT_NULL, 0, 0, 0, 0, 0),
            (
                1,
                SHT_SYMTAB,
                symtab_off,
                symtab.len() as u64,
                2,
                locals,
                SYM64_SIZE as u64,
            ),
            (9, SHT_STRTAB, strtab_off, strtab.len() as u64, 0, 0, 0),
            (17, SHT_STRTAB, shstrtab_off, shstrtab.len() as u64, 0, 0, 0),
        ];
        for (name, ty, off, size, link, info, entsize) in shdrs {
            // An ELF64 section header: name, type (4 bytes each), flags, addr, offset,
            // size (8), link, info (4), addralign, entsize (8).
            for v in [name, ty] {
                out.extend_from_slice(&v.to_le_bytes());
            }
            for v in [0, 0, off, size] {
                out.extend_from_slice(&u64::to_le_bytes(v));
            }
            for v in [link, info] {
                out.extend_from_slice(&u32::to_le_bytes(v));
            }
            for v in [8, entsize] {
                out.extend_from_slice(&u64::to_le_bytes(v));
            }
        }
        let mut h = b"\x7fELF\x02\x01\x01\0\0\0\0\0\0\0\0\0".to_vec();
        for v in [ET_EXEC, EM_RISCV] {
            h.extend_from_slice(&v.to_le_bytes());
        }
        h.extend_from_slice(&1u32.to_le_bytes());
        for v in [0x8000_0000u64, 0, shoff] {
            h.extend_from_slice(&v.to_le_bytes());
        }
        h.extend_from_slice(&0u32.to_le_bytes());
        for v in [EHDR64_SIZE, PHDR64_SIZE, 0, SHDR64_SIZE, shdrs.len(), 3] {
            h.extend_from_slice(&(v as u16).to_le_bytes());
        }
        out[..EHDR64_SIZE].copy_from_slice(&h);
        out
    }

    /// A C program built by clang and linked at 0x8000_0000 loads there; the same
    /// program built for RV32 (ELF32) is refused.
    #[test]
    fn elf64_programs_load_and_elf32_ones_are_refused() {
        let read = |path: &str| parse(&std::fs::read(path).unwrap());
        let rv64 = read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../elf/tests/llvm/rv64/prog.elf"
        ))
        .unwrap();
        assert!(rv64.segments.iter().any(|s| s.paddr == 0x8000_0000));
        let entry = rv64.physical_entry().unwrap();
        assert_eq!(entry, rv64.entry, "linked where it loads");
        let rv32 = read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../elf/tests/llvm/rv32/prog.elf"
        ));
        assert_eq!(rv32.err().unwrap().0, NO_ELF32);
    }

    fn segment(vaddr: u64, paddr: u64, mem_size: u64) -> Segment {
        Segment {
            vaddr,
            paddr,
            data: Vec::new(),
            mem_size,
        }
    }

    /// A kernel linked high and loaded low starts at its load address: paging is off
    /// at reset (spec section 7).
    #[test]
    fn the_entry_point_is_translated_through_its_segment() {
        let high = 0xffff_ffc0_0000_0000;
        let mut elf = super::Elf {
            entry: high + 0x1010,
            segments: vec![
                segment(0x1000, 0x1000, 0x10),
                segment(high, 0x8000_0000, 0x2000),
                segment(high + 0x2000, 0x8000_2000, 0x1000),
            ],
            symbols: Vec::new(),
        };
        assert_eq!(elf.physical_entry().unwrap(), 0x8000_1010);
        // .bss counts: the range is the memory size.
        elf.entry = high + 0x2fff;
        assert_eq!(elf.physical_entry().unwrap(), 0x8000_2fff);
        for entry in [high + 0x3000, high - 1, 0x1010, 0] {
            elf.entry = entry;
            let err = elf.physical_entry().expect_err("outside");
            assert_eq!(err.0, "entry point outside the program's segments", "{entry:#x}");
        }
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
