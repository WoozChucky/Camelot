//! Writes the linked image as an ELF32 little-endian executable (`ET_EXEC`).
//!
//! ```text
//! ELF header (52 bytes)
//! program headers, one PT_LOAD per segment
//! segment contents, each at a file offset congruent to its address mod 4 KiB
//! .symtab, .strtab, the debug sections (.debug_line, ...), .shstrtab
//! section header table: null, output sections, .symtab, .strtab, debug sections, .shstrtab
//! ```
//!
//! The debug sections come after everything else but the section names, so
//! that taking them out ([`strip_debug`]) leaves exactly the file a link
//! without them writes: no loaded byte, file offset or section index moves.
//!
//! Segments group consecutive output sections with the same permissions (R,
//! RX, RW), as ld.lld does when there is a `SECTIONS` command (merlin-ld
//! always has one: the default layout is a script too). Like ld.lld, a
//! segment does not split at address gaps or after a NOBITS section: the file
//! holds zeros for those bytes and `p_filesz` runs to the last section with
//! file contents. A segment holding only empty sections is dropped.

use crate::layout::{Def, Layout};
use crate::object::{
    SHF_EXECINSTR, SHF_WRITE, SHN_ABS, STB_GLOBAL, STB_LOCAL, STT_FILE, STT_NOTYPE, STT_SECTION,
};
use crate::relocate::Images;

pub const PAGE: u32 = 0x1000;
const PT_LOAD: u32 = 1;
const PF_X: u32 = 1;
const PF_W: u32 = 2;
const PF_R: u32 = 4;
const SHT_PROGBITS: u32 = 1;
const SHT_SYMTAB: u32 = 2;
const SHT_STRTAB: u32 = 3;
const SHT_NOBITS: u32 = 8;
const ET_EXEC: u16 = 2;
const EM_RISCV: u16 = 243;

/// A `PT_LOAD` segment: a run of output sections.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    pub vaddr: u32,
    pub filesz: u32,
    pub memsz: u32,
    pub flags: u32,
    /// Indices into [`Layout::sections`].
    pub sections: Vec<usize>,
}

fn pflags(shflags: u32) -> u32 {
    let mut f = PF_R;
    if shflags & SHF_WRITE != 0 {
        f |= PF_W;
    }
    if shflags & SHF_EXECINSTR != 0 {
        f |= PF_X;
    }
    f
}

/// Groups the output sections into `PT_LOAD` segments.
pub fn segments(layout: &Layout) -> Vec<Segment> {
    let mut segs: Vec<Segment> = Vec::new();
    for (i, s) in layout.sections.iter().enumerate() {
        let flags = pflags(s.flags);
        let start_new = match segs.last() {
            None => true,
            Some(seg) => {
                let prev = &layout.sections[*seg.sections.last().unwrap()];
                let prev_end = prev.addr as u64 + prev.size as u64;
                // Going backward can't be one segment.
                seg.flags != flags || (s.addr as u64) < prev_end
            }
        };
        if start_new {
            segs.push(Segment {
                vaddr: s.addr,
                filesz: 0,
                memsz: 0,
                flags,
                sections: Vec::new(),
            });
        }
        let seg = segs.last_mut().unwrap();
        seg.sections.push(i);
        let end = s.addr - seg.vaddr + s.size;
        seg.memsz = end;
        if !s.nobits {
            seg.filesz = end;
        }
    }
    // E.g. an empty `.rodata` between `.text` and `.data` (ld.lld removes
    // empty PT_LOADs too).
    segs.retain(|s| s.memsz > 0);
    segs
}

fn put16(b: &mut Vec<u8>, v: u16) {
    b.extend_from_slice(&v.to_le_bytes());
}
fn put32(b: &mut Vec<u8>, v: u32) {
    b.extend_from_slice(&v.to_le_bytes());
}
fn pad_to(b: &mut Vec<u8>, len: usize) {
    if b.len() < len {
        b.resize(len, 0);
    }
}

struct StrTab(Vec<u8>);

impl StrTab {
    fn add(&mut self, s: &str) -> u32 {
        if s.is_empty() {
            return 0;
        }
        let off = self.0.len() as u32;
        self.0.extend_from_slice(s.as_bytes());
        self.0.push(0);
        off
    }
}

/// Symbol table entries: null, locals, then globals.
fn symtab(layout: &Layout, strtab: &mut StrTab) -> (Vec<u8>, u32) {
    let mut out = vec![0u8; 16];
    let shndx_of = |obj: usize, sec: u16| -> Option<u16> {
        if sec == SHN_ABS {
            return Some(SHN_ABS);
        }
        layout.input_addr(obj, sec as usize)?;
        // Placed but without an output section (only after a layout error).
        Some(
            layout
                .input_output_section(obj, sec as usize)
                .map_or(SHN_ABS, |o| o as u16 + 1),
        )
    };
    let mut entry = |out: &mut Vec<u8>, name: &str, value: u32, size: u32, info: u8, shndx: u16| {
        put32(out, strtab.add(name));
        put32(out, value);
        put32(out, size);
        out.push(info);
        out.push(0);
        put16(out, shndx);
    };

    for (oi, o) in layout.objects.iter().enumerate() {
        for (si, s) in o.symbols.iter().enumerate().skip(1) {
            if !s.is_local() || s.ty == STT_SECTION || s.ty == STT_FILE || s.is_undefined() {
                continue;
            }
            let (Some(shndx), Some(value)) = (shndx_of(oi, s.shndx), layout.object_symbol(oi, si)) else {
                continue; // in a section that isn't loaded (debug info, ...)
            };
            entry(&mut out, &s.name, value, s.size, (STB_LOCAL << 4) | s.ty, shndx);
        }
    }
    let first_global = (out.len() / 16) as u32;
    for (name, def) in &layout.globals {
        match *def {
            Def::Script(v) => entry(&mut out, name, v, 0, (STB_GLOBAL << 4) | STT_NOTYPE, SHN_ABS),
            Def::Object { obj, sym } => {
                let s = &layout.objects[obj].symbols[sym];
                // A global whose section /DISCARD/ threw away has no address
                // and is left out, as ld.lld does.
                let Some(value) = layout.object_symbol(obj, sym) else {
                    continue;
                };
                let shndx = shndx_of(obj, s.shndx).unwrap_or(SHN_ABS);
                entry(&mut out, name, value, s.size, (s.bind << 4) | s.ty, shndx);
            }
        }
    }
    (out, first_global)
}

/// Serializes the executable. `images` comes from [`crate::relocate::relocate`].
pub fn write(layout: &Layout, images: &Images) -> Vec<u8> {
    let debug = images.debug.as_slice();
    let images = images.sections.as_slice();
    let segs = segments(layout);
    let phoff = 52usize;
    let mut out = vec![0u8; phoff + 32 * segs.len()];

    // Segment contents.
    let mut sec_offset = vec![0u32; layout.sections.len()];
    let mut seg_offset = Vec::with_capacity(segs.len());
    for seg in &segs {
        // Smallest offset >= current end with offset ≡ vaddr (mod PAGE).
        let cur = out.len() as u32;
        let want = seg.vaddr % PAGE;
        let off = cur + (want + PAGE - cur % PAGE) % PAGE;
        pad_to(&mut out, off as usize);
        for &si in &seg.sections {
            let s = &layout.sections[si];
            let o = off + (s.addr - seg.vaddr);
            sec_offset[si] = o;
            if !s.nobits {
                pad_to(&mut out, o as usize);
                out.extend_from_slice(&images[si]);
            }
        }
        seg_offset.push(off);
    }
    for (i, seg) in segs.iter().enumerate() {
        let mut ph = Vec::with_capacity(32);
        for v in [
            PT_LOAD,
            seg_offset[i],
            seg.vaddr,
            seg.vaddr,
            seg.filesz,
            seg.memsz,
            seg.flags,
            PAGE,
        ] {
            put32(&mut ph, v);
        }
        out[phoff + 32 * i..phoff + 32 * (i + 1)].copy_from_slice(&ph);
    }

    // Symbol and string tables.
    let mut strtab = StrTab(vec![0]);
    let mut shstrtab = StrTab(vec![0]);
    let (syms, first_global) = symtab(layout, &mut strtab);
    out.resize(out.len().next_multiple_of(4), 0);
    let symtab_off = out.len() as u32;
    out.extend_from_slice(&syms);
    let strtab_off = out.len() as u32;
    out.extend_from_slice(&strtab.0);
    let mut debug_off = Vec::new();
    for (ds, img) in layout.debug.iter().zip(debug) {
        out.resize(out.len().next_multiple_of(ds.align as usize), 0);
        debug_off.push(out.len() as u32);
        out.extend_from_slice(img);
    }

    // Section headers.
    let mut shdrs: Vec<[u32; 10]> = vec![[0; 10]];
    for (i, s) in layout.sections.iter().enumerate() {
        shdrs.push([
            shstrtab.add(&s.name),
            if s.nobits { SHT_NOBITS } else { SHT_PROGBITS },
            s.flags,
            s.addr,
            sec_offset[i],
            s.size,
            0,
            0,
            s.align,
            0,
        ]);
    }
    let symtab_index = shdrs.len() as u32;
    shdrs.push([
        shstrtab.add(".symtab"),
        SHT_SYMTAB,
        0,
        0,
        symtab_off,
        syms.len() as u32,
        symtab_index + 1,
        first_global,
        4,
        16,
    ]);
    shdrs.push([
        shstrtab.add(".strtab"),
        SHT_STRTAB,
        0,
        0,
        strtab_off,
        strtab.0.len() as u32,
        0,
        0,
        1,
        0,
    ]);
    for (ds, off) in layout.debug.iter().zip(debug_off) {
        shdrs.push([
            shstrtab.add(&ds.name),
            SHT_PROGBITS,
            ds.flags,
            0,
            off,
            ds.size,
            0,
            0,
            ds.align,
            ds.entsize,
        ]);
    }
    let shstrtab_name = shstrtab.add(".shstrtab");
    shdrs.push([
        shstrtab_name,
        SHT_STRTAB,
        0,
        0,
        out.len() as u32,
        shstrtab.0.len() as u32,
        0,
        0,
        1,
        0,
    ]);
    out.extend_from_slice(&shstrtab.0);
    out.resize(out.len().next_multiple_of(4), 0);
    let shoff = out.len() as u32;
    for h in &shdrs {
        for v in h {
            put32(&mut out, *v);
        }
    }

    // ELF header.
    let mut hdr = Vec::with_capacity(52);
    hdr.extend_from_slice(b"\x7fELF");
    hdr.extend_from_slice(&[1, 1, 1, 0]); // ELFCLASS32, little-endian, version 1, SysV ABI
    hdr.extend_from_slice(&[0; 8]);
    put16(&mut hdr, ET_EXEC);
    put16(&mut hdr, EM_RISCV);
    put32(&mut hdr, 1); // e_version
    put32(&mut hdr, layout.entry);
    put32(&mut hdr, if segs.is_empty() { 0 } else { phoff as u32 });
    put32(&mut hdr, shoff);
    put32(&mut hdr, 0); // e_flags: ilp32 soft-float, no RVC
    put16(&mut hdr, 52); // e_ehsize
    put16(&mut hdr, 32); // e_phentsize
    put16(&mut hdr, segs.len() as u16);
    put16(&mut hdr, 40); // e_shentsize
    put16(&mut hdr, shdrs.len() as u16);
    put16(&mut hdr, shdrs.len() as u16 - 1); // e_shstrndx
    out[..52].copy_from_slice(&hdr);
    out
}

/// A little-endian `u32` of `b` at `at` (a header field).
fn get32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}

/// The executable `elf` without its debug sections (`.debug_*`, not
/// loaded), for a program that is installed rather than debugged. For a file
/// [`write`] made, the result is what it writes for the same link without
/// debug information: the bytes up to the debug sections stay as they are,
/// then come the section names without theirs and the section headers
/// without theirs. A file with no debug sections comes back unchanged.
/// Anything laid out otherwise is an error, and so is an ELF64 file: the
/// offsets below are ELF32's, the class [`write`] makes.
pub fn strip_debug(elf: &[u8]) -> Result<Vec<u8>, String> {
    let parsed = elf::Elf::parse(elf).map_err(|e| e.to_string())?;
    if parsed.class != elf::Class::Elf32 {
        return Err("strip_debug: ELF64 (RV64) files are not supported yet".into());
    }
    let is_debug = |s: &elf::Section| !s.is_alloc() && s.name.starts_with(".debug_");
    if !parsed.sections.iter().any(is_debug) {
        return Ok(elf.to_vec());
    }
    let shstrndx = parsed.header.shstrndx as usize;
    let n = parsed.sections.len();
    // The layout write() makes: debug sections last but for the names, and
    // their contents after every other section's but the names'.
    let first = parsed.sections.iter().position(is_debug).unwrap();
    let ordered = shstrndx == n - 1 && (first..n - 1).all(|i| is_debug(&parsed.sections[i]));
    // Up to the end of the last kept section (not the padding that may align
    // the first debug section).
    let ends = parsed.sections[1..first]
        .iter()
        .filter(|s| !s.is_nobits())
        .map(|s| (s.offset + s.size) as usize);
    let headers_end = 52 + 32 * parsed.segments.len();
    let cut = ends.max().unwrap_or(0).max(headers_end);
    if !ordered || cut > parsed.sections[first].offset as usize {
        return Err("the debug sections are not where merlin-ld puts them (last)".into());
    }
    let mut out = elf[..cut].to_vec();
    // The kept sections' names, in their order (offsets as write() gives them).
    let mut names = StrTab(vec![0]);
    let mut headers: Vec<[u32; 10]> = Vec::new();
    for (i, s) in parsed.sections[..first].iter().enumerate() {
        let at = parsed.header.shoff as usize + 40 * i;
        let mut h: [u32; 10] = std::array::from_fn(|k| get32(elf, at + 4 * k));
        if i > 0 {
            h[0] = names.add(&s.name);
        }
        headers.push(h);
    }
    let shstrtab = &parsed.sections[shstrndx];
    let at = parsed.header.shoff as usize + 40 * shstrndx;
    let mut h: [u32; 10] = std::array::from_fn(|k| get32(elf, at + 4 * k));
    h[0] = names.add(&shstrtab.name);
    h[4] = out.len() as u32;
    h[5] = names.0.len() as u32;
    headers.push(h);
    out.extend_from_slice(&names.0);
    out.resize(out.len().next_multiple_of(4), 0);
    let shoff = out.len() as u32;
    for h in &headers {
        for v in h {
            put32(&mut out, *v);
        }
    }
    out[32..36].copy_from_slice(&shoff.to_le_bytes());
    out[48..50].copy_from_slice(&(headers.len() as u16).to_le_bytes());
    out[50..52].copy_from_slice(&(headers.len() as u16 - 1).to_le_bytes());
    Ok(out)
}
