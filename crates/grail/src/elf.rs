//! Minimal ELF32 little-endian reader: enough to load PT_LOAD segments and
//! look up symbols. merlin-ld will produce the same subset.

use std::fmt;

#[derive(Debug)]
pub struct ElfError(pub String);

impl fmt::Display for ElfError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "bad ELF: {}", self.0)
    }
}

fn err<T>(msg: &str) -> Result<T, ElfError> {
    Err(ElfError(msg.to_string()))
}

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

const PT_LOAD: u32 = 1;
const SHT_SYMTAB: u32 = 2;
const EM_RISCV: u16 = 243;

struct Reader<'a>(&'a [u8]);

impl Reader<'_> {
    fn u16(&self, off: usize) -> Result<u16, ElfError> {
        match self.0.get(off..off + 2) {
            Some(b) => Ok(u16::from_le_bytes([b[0], b[1]])),
            None => err("truncated file"),
        }
    }
    fn u32(&self, off: usize) -> Result<u32, ElfError> {
        match self.0.get(off..off + 4) {
            Some(b) => Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]])),
            None => err("truncated file"),
        }
    }
    fn bytes(&self, off: usize, len: usize) -> Result<&[u8], ElfError> {
        match self.0.get(off..off + len) {
            Some(b) => Ok(b),
            None => err("truncated file"),
        }
    }
    fn cstr(&self, off: usize) -> String {
        let s = self.0.get(off..).unwrap_or(&[]);
        let end = s.iter().position(|&b| b == 0).unwrap_or(s.len());
        String::from_utf8_lossy(&s[..end]).into_owned()
    }
}

pub fn parse(file: &[u8]) -> Result<Elf, ElfError> {
    let r = Reader(file);
    if r.bytes(0, 4)? != b"\x7fELF" {
        return err("missing magic");
    }
    if file[4] != 1 {
        return err("not 32-bit (EI_CLASS != ELFCLASS32)");
    }
    if file[5] != 1 {
        return err("not little-endian");
    }
    if r.u16(18)? != EM_RISCV {
        return err("e_machine is not RISC-V");
    }
    let entry = r.u32(24)?;
    let phoff = r.u32(28)? as usize;
    let shoff = r.u32(32)? as usize;
    let phentsize = r.u16(42)? as usize;
    let phnum = r.u16(44)? as usize;
    let shentsize = r.u16(46)? as usize;
    let shnum = r.u16(48)? as usize;

    let mut segments = Vec::new();
    for i in 0..phnum {
        let ph = phoff + i * phentsize;
        if r.u32(ph)? != PT_LOAD {
            continue;
        }
        let offset = r.u32(ph + 4)? as usize;
        let paddr = r.u32(ph + 12)?;
        let filesz = r.u32(ph + 16)? as usize;
        let memsz = r.u32(ph + 20)?;
        segments.push(Segment {
            paddr,
            data: r.bytes(offset, filesz)?.to_vec(),
            mem_size: memsz,
        });
    }

    let mut symbols = Vec::new();
    for i in 0..shnum {
        let sh = shoff + i * shentsize;
        if r.u32(sh + 4)? != SHT_SYMTAB {
            continue;
        }
        let off = r.u32(sh + 16)? as usize;
        let size = r.u32(sh + 20)? as usize;
        let link = r.u32(sh + 24)? as usize; // string table section index
        let entsize = (r.u32(sh + 36)? as usize).max(16);
        let strtab = r.u32(shoff + link * shentsize + 16)? as usize;
        for s in (off..off + size).step_by(entsize) {
            let name = r.cstr(strtab + r.u32(s)? as usize);
            if !name.is_empty() {
                symbols.push((name, r.u32(s + 4)?));
            }
        }
    }

    Ok(Elf {
        entry,
        segments,
        symbols,
    })
}

impl Elf {
    pub fn symbol(&self, name: &str) -> Option<u32> {
        self.symbols.iter().find(|(n, _)| n == name).map(|&(_, v)| v)
    }
}
