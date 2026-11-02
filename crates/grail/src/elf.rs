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
