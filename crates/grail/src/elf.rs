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
    let symbols = elf
        .symbols()?
        .into_iter()
        .filter(|s| !s.name.is_empty())
        .map(|s| (s.name, s.value))
        .collect();
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
