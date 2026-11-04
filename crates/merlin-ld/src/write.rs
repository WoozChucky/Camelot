//! Writes the linked image as an ELF executable (`ET_EXEC`), little-endian,
//! of the layout's class ([`Layout::class`], its objects'): ELF32 for RV32,
//! ELF64 for RV64.
//!
//! ```text
//! ELF header (52 bytes ELF32, 64 bytes ELF64)
//! program headers (32 or 56 bytes each): PT_PHDR if the headers are loaded,
//!   one PT_LOAD per segment, in the room ld.lld leaves for its own (see "The headers")
//! segment contents, each at a file offset congruent to its address mod its p_align
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
//! file contents. A segment holding only empty sections is dropped. A
//! section the script gives a load address (`AT(lma)`) always starts a
//! segment of its own, as in ld.lld, and a segment's `p_paddr` is the load
//! address of its first section (the same as `p_vaddr` without `AT`).
//!
//! A segment's alignment (`p_align`) is the page size, 4 KiB, or more if one
//! of its sections asks for more (`.p2align 13`, `ALIGN(8K)`): ld.lld's
//! `PhdrEntry::add` raises it to every section's, so that the loader can map
//! the segment with the section aligned. Its file offset is congruent to its
//! address modulo that alignment (ld.lld's `computeFileOffset`), and the
//! sections after the first follow at their distance from it.
//!
//! ## The headers
//!
//! The file starts with the ELF header and the program header table. How
//! much room they take, where the first segment goes after them, and
//! whether they are loaded too, is ld.lld's rule for a link with a
//! `SECTIONS` command and no `PHDRS` (`Writer::createPhdrs`,
//! `LinkerScript::allocateHeaders`, `Writer::assignFileOffsets`):
//!
//! 1. ld.lld makes its program headers before it knows which of them it
//!    keeps, and sizes the table for all of them: `PT_PHDR`, a `PT_LOAD` for
//!    the headers, one `PT_LOAD` per run of sections (an empty one too,
//!    which it removes later), `PT_GNU_STACK`, and `PT_RISCV_ATTRIBUTES` if
//!    it writes a `.riscv.attributes` section ([`Layout::riscv_attributes`]).
//!    The first section joins the headers' `PT_LOAD` if it is read-only and
//!    in no MEMORY region ([`Layout::memory`]): one `PT_LOAD` fewer.
//! 2. The headers are loaded if they fit below the lowest section in its
//!    page: then they are at the start of that page and of the file, in a
//!    read-only `PT_LOAD` of their own, or in the first section's, which then
//!    starts at the page; and a `PT_PHDR` describes the table. If they don't
//!    fit, neither of those two program headers is written.
//! 3. Either way the contents of the segments come after the room the table
//!    was sized for: the first segment at the first offset past it that is
//!    congruent to its address.
//!
//! So with three segments of ELF32 (0x114 bytes: 52 for the ELF header, 7
//! program headers of 32), a first section at 0x8000_0000 is at file offset
//! 0x1000, one at 0x8000_0100 at 0x1100 (no room below it for 0x114 bytes),
//! and one at 0x8000_0400 at 0x400, after headers loaded at 0x8000_0000.
//! ELF64 needs 0x1c8 bytes (64 + 7 × 56) for the same.
//!
//! merlin-ld writes only the `PT_PHDR` and the `PT_LOAD`s, but it leaves the
//! same room ([`Headers`]), so that every segment is where ld.lld puts it:
//! after the table come zeros where ld.lld has `PT_GNU_STACK` and
//! `PT_RISCV_ATTRIBUTES` (and the headers it counted but removed). The
//! `PT_PHDR` and the headers' `PT_LOAD` cover the whole room, as ld.lld's
//! do. A program loaded from its segments (Grail, a bootloader) gets its
//! headers too, read-only, when they are loaded: a link script keeps them
//! out with a first section at the start of a page.
//!
//! ## The two classes
//!
//! ELF32 and ELF64 hold the same records; ELF64 widens every address, offset
//! and size to 8 bytes and reorders two records so that those stay aligned:
//!
//! | record | ELF32 | ELF64 |
//! |---|---|---|
//! | ELF header | 52 bytes | 64 bytes: `e_entry`, `e_phoff`, `e_shoff` 8 bytes each |
//! | program header | 32 bytes | 56 bytes: `p_flags` moves up, after `p_type` |
//! | section header | 40 bytes | 64 bytes: `sh_flags`, `sh_addr`, `sh_offset`, `sh_size`, `sh_addralign`, `sh_entsize` 8 bytes |
//! | symbol | 16 bytes | 24 bytes: `st_info`, `st_other`, `st_shndx` move before `st_value` and `st_size` |
//!
//! `.symtab` and the section header table are aligned to the size of an
//! address (4 or 8 bytes), as ld.lld aligns them. Everything else is the
//! same for both, the 4 KiB page included: ld.lld's RISC-V maximum page
//! size is 0x1000 for RV64 as for RV32, so `p_align` is at least 0x1000 for
//! both. `e_flags` is 0 for both: the soft-float ABI (ilp32 or lp64)
//! without compressed instructions, the only one merlin-ld links.
//!
//! The layout's addresses and sizes are `u64`. An ELF32 link stays below
//! 4 GiB (the layout checks it), so narrowing them to ELF32's 4-byte fields
//! ([`put_word`]) loses nothing.

use crate::layout::{Def, Layout};
use crate::object::{
    Class, SHF_EXECINSTR, SHF_WRITE, SHN_ABS, STB_GLOBAL, STB_LOCAL, STT_FILE, STT_NOTYPE, STT_SECTION,
};
use crate::relocate::Images;
use elf::{
    EM_RISCV, ET_EXEC, PF_R, PF_W, PF_X, PT_LOAD, PT_PHDR, SHT_NOBITS, SHT_PROGBITS, SHT_STRTAB, SHT_SYMTAB,
};

/// The page size segments are aligned to: ld.lld's for RISC-V, both classes.
pub const PAGE: u64 = 0x1000;

/// A `PT_LOAD` segment: a run of output sections.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    pub vaddr: u64,
    /// The load address (`p_paddr`): its first section's LMA.
    pub paddr: u64,
    pub filesz: u64,
    pub memsz: u64,
    pub flags: u32,
    /// `p_align`: [`PAGE`], or the largest alignment of its sections if that
    /// is more. Its file offset is congruent to `vaddr` modulo this.
    pub align: u64,
    /// Indices into [`Layout::sections`].
    pub sections: Vec<usize>,
    /// The segment loads the ELF header and the program headers: it starts
    /// at [`Headers::addr`], at file offset 0, and runs to the end of the
    /// headers' room or over its sections, if it has any (see "The headers").
    pub headers: bool,
}

/// The room the ELF header and the program header table take at the start
/// of the file, and whether they are loaded (see "The headers").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Headers {
    /// The ELF header and as many program headers as ld.lld sizes its table
    /// for. The segment contents come after it.
    pub size: u64,
    /// Where they are loaded, the start of the page of the lowest section,
    /// if they fit below it there; `None` if they are not loaded.
    pub addr: Option<u64>,
}

fn pflags(shflags: u64) -> u32 {
    let mut f = PF_R;
    if shflags & SHF_WRITE != 0 {
        f |= PF_W;
    }
    if shflags & SHF_EXECINSTR != 0 {
        f |= PF_X;
    }
    f
}

/// The `PT_LOAD` segments, the headers' one first if they are loaded, and
/// where the headers go (see "The headers").
pub fn segments(layout: &Layout) -> (Headers, Vec<Segment>) {
    let class = layout.class;
    let mut segs = runs(layout);

    // 1. ld.lld's program header table, sized for every header it makes:
    // PT_PHDR, the headers' PT_LOAD, one PT_LOAD per run (`runs` keeps the
    // empty ones ld.lld still counts here), PT_GNU_STACK, and
    // PT_RISCV_ATTRIBUTES. A read-only first section that is in no MEMORY
    // region goes into the headers' PT_LOAD instead of one of its own
    // (createPhdrs: a section joins a PT_LOAD only if it has the same flags
    // and MEMORY region, and the headers are in none; an `AT` on the first
    // section does not keep it out).
    let joins = !layout.memory && segs.first().is_some_and(|s| s.flags == PF_R);
    let count = 2 + segs.len() - usize::from(joins) + 1 + usize::from(layout.riscv_attributes);
    let size = (class.ehdr_size() + class.phdr_size() * count) as u64;

    // 2. Are they loaded? Only if they fit between the start of the page of
    // the lowest section and that section (allocateHeaders: "only allocate
    // program headers if that would not add a page"). Without sections
    // ld.lld's lowest address is the top of the address space (its `min`
    // starts at UINT64_MAX, and an ELF32 field keeps the low 32 bits).
    let top = match class {
        Class::Elf32 => u32::MAX.into(),
        Class::Elf64 => u64::MAX,
    };
    let lowest = layout.sections.iter().map(|s| s.addr).min().unwrap_or(top);
    let page = lowest - lowest % PAGE;
    let addr = (size <= lowest - page).then_some(page);
    if let Some(page) = addr {
        if joins {
            // The first segment, moved down to the page: the headers, then
            // the gap up to its first section, then its sections. Its file
            // size runs to its last section with file contents, or to the
            // end of the headers if none has any (ld.lld's p_filesz is the
            // offset of its last section, where a NOBITS one is at the file
            // end so far). Its load address moves down with it: every
            // section of a segment has the same LMA - VMA.
            let first = &mut segs[0];
            let below = first.vaddr - page;
            let file = first.sections.iter().any(|&i| !layout.sections[i].nobits);
            first.vaddr = page;
            first.paddr = first.paddr.wrapping_sub(below);
            first.filesz = if file { below + first.filesz } else { size };
            first.memsz += below;
            first.headers = true;
        } else {
            // A read-only segment of the headers alone, at its address
            // (ld.lld takes the LMA - VMA of the headers' PT_LOAD from its
            // first section, and this one has none).
            segs.insert(
                0,
                Segment {
                    vaddr: page,
                    paddr: page,
                    filesz: size,
                    memsz: size,
                    flags: PF_R,
                    align: PAGE,
                    sections: Vec::new(),
                    headers: true,
                },
            );
        }
    }
    // E.g. an empty `.rodata` between `.text` and `.data` (ld.lld removes
    // empty PT_LOADs too, after it has sized the table). The headers'
    // segment is never empty.
    segs.retain(|s| s.memsz > 0);
    (Headers { size, addr }, segs)
}

/// Groups the output sections into runs, the `PT_LOAD`s they need, empty
/// ones included.
fn runs(layout: &Layout) -> Vec<Segment> {
    let mut segs: Vec<Segment> = Vec::new();
    for (i, s) in layout.sections.iter().enumerate() {
        let flags = pflags(s.flags);
        let start_new = match segs.last() {
            None => true,
            Some(seg) => {
                let prev = &layout.sections[*seg.sections.last().unwrap()];
                // No overflow: every section ends below 2^64 (the layout
                // refuses one that would not, ld.lld's checkSections).
                let prev_end = prev.addr + prev.size;
                // Going backward can't be one segment. A section with its
                // own load address starts one, as in ld.lld: one segment has
                // one p_paddr, and the sections in it are loaded contiguously
                // from there.
                seg.flags != flags || s.addr < prev_end || s.at
            }
        };
        if start_new {
            segs.push(Segment {
                vaddr: s.addr,
                paddr: s.lma,
                filesz: 0,
                memsz: 0,
                flags,
                align: PAGE,
                sections: Vec::new(),
                headers: false,
            });
        }
        let seg = segs.last_mut().unwrap();
        seg.sections.push(i);
        // Every section counts, an empty or NOBITS one too (ld.lld).
        seg.align = seg.align.max(s.align.into());
        let end = s.addr - seg.vaddr + s.size;
        seg.memsz = end;
        if !s.nobits {
            seg.filesz = end;
        }
    }
    segs
}

fn put16(b: &mut Vec<u8>, v: u16) {
    b.extend_from_slice(&v.to_le_bytes());
}
fn put32(b: &mut Vec<u8>, v: u32) {
    b.extend_from_slice(&v.to_le_bytes());
}
fn put64(b: &mut Vec<u8>, v: u64) {
    b.extend_from_slice(&v.to_le_bytes());
}

/// Appends an address, offset or size field: 4 bytes in ELF32, 8 in ELF64.
/// An ELF32 value fits in 4 bytes: the layout keeps an ELF32 link below
/// 4 GiB, and its file is smaller than that.
fn put_word(class: Class, b: &mut Vec<u8>, v: u64) {
    match class {
        Class::Elf32 => {
            debug_assert!(u32::try_from(v).is_ok(), "{v:#x} in a 4-byte ELF32 field");
            put32(b, v as u32);
        }
        Class::Elf64 => put64(b, v),
    }
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

/// A section header, its fields as wide as ELF64's ([`Shdr::put`] narrows
/// the address-sized ones for ELF32).
#[derive(Debug, Clone, Default)]
struct Shdr {
    name: u32,
    ty: u32,
    flags: u64,
    addr: u64,
    offset: u64,
    size: u64,
    link: u32,
    info: u32,
    align: u64,
    entsize: u64,
}

impl Shdr {
    /// Appends the header in `class`'s layout. The order of the fields is
    /// the same in both; `sh_flags`, `sh_addr`, `sh_offset`, `sh_size`,
    /// `sh_addralign` and `sh_entsize` are address-sized.
    fn put(&self, class: Class, out: &mut Vec<u8>) {
        put32(out, self.name);
        put32(out, self.ty);
        put_word(class, out, self.flags);
        put_word(class, out, self.addr);
        put_word(class, out, self.offset);
        put_word(class, out, self.size);
        put32(out, self.link);
        put32(out, self.info);
        put_word(class, out, self.align);
        put_word(class, out, self.entsize);
    }

    /// The header as the elf crate read it (`name` is set by the caller:
    /// the offset in a new string table).
    fn from_section(s: &elf::Section) -> Shdr {
        Shdr {
            name: 0,
            ty: s.ty,
            flags: s.flags,
            addr: s.addr,
            offset: s.offset,
            size: s.size,
            link: s.link,
            info: s.info,
            align: s.align,
            entsize: s.entsize,
        }
    }
}

/// Appends a symbol table entry in `class`'s layout: ELF32 has
/// `name, value, size, info, other, shndx`; ELF64 moves the three narrow
/// fields before the two 8-byte ones, `name, info, other, shndx, value, size`.
fn put_symbol(class: Class, out: &mut Vec<u8>, name: u32, value: u64, size: u64, info: u8, shndx: u16) {
    put32(out, name);
    match class {
        Class::Elf32 => {
            put_word(class, out, value);
            put_word(class, out, size);
            out.push(info);
            out.push(0); // st_other: STV_DEFAULT
            put16(out, shndx);
        }
        Class::Elf64 => {
            out.push(info);
            out.push(0); // st_other: STV_DEFAULT
            put16(out, shndx);
            put64(out, value);
            put64(out, size);
        }
    }
}

/// A program header: `PT_LOAD` or `PT_PHDR`.
struct Phdr {
    ty: u32,
    flags: u32,
    offset: u64,
    vaddr: u64,
    paddr: u64,
    filesz: u64,
    memsz: u64,
    align: u64,
}

impl Phdr {
    /// The `PT_LOAD` of `seg`, at file offset `offset`.
    fn load(seg: &Segment, offset: u64) -> Phdr {
        Phdr {
            ty: PT_LOAD,
            flags: seg.flags,
            offset,
            vaddr: seg.vaddr,
            paddr: seg.paddr,
            filesz: seg.filesz,
            memsz: seg.memsz,
            align: seg.align,
        }
    }

    /// Appends the header in `class`'s layout. ELF64 moves `p_flags` up to
    /// follow `p_type`, so that the six address-sized fields after it are
    /// 8-aligned.
    fn put(&self, class: Class, out: &mut Vec<u8>) {
        put32(out, self.ty);
        if class == Class::Elf64 {
            put32(out, self.flags);
        }
        for v in [self.offset, self.vaddr, self.paddr, self.filesz, self.memsz] {
            put_word(class, out, v);
        }
        if class == Class::Elf32 {
            put32(out, self.flags);
        }
        put_word(class, out, self.align);
    }
}

/// Where `e_shoff` and `e_shnum` are in the ELF header (`e_shstrndx`
/// follows `e_shnum`): after the three address-sized fields `e_entry`,
/// `e_phoff` and `e_shoff`, which are 4 or 8 bytes.
fn shoff_and_shnum_at(class: Class) -> (usize, usize) {
    match class {
        Class::Elf32 => (32, 48),
        Class::Elf64 => (40, 60),
    }
}

/// Symbol table entries: null, locals, then globals. Returns the table and
/// the index of its first global (`sh_info`).
fn symtab(layout: &Layout, strtab: &mut StrTab) -> (Vec<u8>, u32) {
    let class = layout.class;
    let mut out = vec![0u8; class.sym_size()];
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
    let mut entry = |out: &mut Vec<u8>, name: &str, value: u64, size: u64, info: u8, shndx: u16| {
        let name = strtab.add(name);
        put_symbol(class, out, name, value, size, info, shndx);
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
    let first_global = (out.len() / class.sym_size()) as u32;
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
    let class = layout.class;
    let debug = images.debug.as_slice();
    let images = images.sections.as_slice();
    let (headers, segs) = segments(layout);
    let phoff = class.ehdr_size();
    // The room ld.lld leaves for the headers (see "The headers"), which
    // holds ours: PT_PHDR if they are loaded, then the PT_LOADs.
    let mut out = vec![0u8; headers.size as usize];

    // Segment contents.
    let mut sec_offset = vec![0u64; layout.sections.len()];
    let mut seg_offset = Vec::with_capacity(segs.len());
    for seg in &segs {
        let off = if seg.headers {
            // The headers' segment maps the start of the file. Its
            // sections, if any, are past the headers' room: the lowest
            // section is at least that far into the page.
            0
        } else {
            // Smallest offset >= current end with offset ≡ vaddr (mod p_align).
            let (cur, align) = (out.len() as u64, seg.align);
            let want = seg.vaddr % align;
            cur + (want + align - cur % align) % align
        };
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
    // Program headers. With the headers loaded, a PT_PHDR first: the table
    // in the headers' segment (the first one), all of the room after the
    // ELF header, at the address and load address it has there.
    let mut table: Vec<Phdr> = Vec::with_capacity(segs.len() + 1);
    if let Some(addr) = headers.addr {
        let room = headers.size - phoff as u64;
        table.push(Phdr {
            ty: PT_PHDR,
            flags: PF_R,
            offset: phoff as u64,
            vaddr: addr + phoff as u64,
            paddr: segs[0].paddr.wrapping_add(phoff as u64),
            filesz: room,
            memsz: room,
            align: class.addr_size() as u64,
        });
    }
    table.extend(
        segs.iter()
            .zip(&seg_offset)
            .map(|(seg, &off)| Phdr::load(seg, off)),
    );
    let mut phdrs = Vec::with_capacity(class.phdr_size() * table.len());
    for p in &table {
        p.put(class, &mut phdrs);
    }
    debug_assert!(
        phoff + phdrs.len() <= headers.size as usize,
        "the table fits its room"
    );
    out[phoff..phoff + phdrs.len()].copy_from_slice(&phdrs);

    // Symbol and string tables. `.symtab` is aligned to its entries'
    // widest field, an address (ld.lld: sh_addralign 4 or 8).
    let word = class.addr_size();
    let mut strtab = StrTab(vec![0]);
    let mut shstrtab = StrTab(vec![0]);
    let (syms, first_global) = symtab(layout, &mut strtab);
    out.resize(out.len().next_multiple_of(word), 0);
    let symtab_off = out.len() as u64;
    out.extend_from_slice(&syms);
    let strtab_off = out.len() as u64;
    out.extend_from_slice(&strtab.0);
    let mut debug_off = Vec::new();
    for (ds, img) in layout.debug.iter().zip(debug) {
        out.resize(out.len().next_multiple_of(ds.align as usize), 0);
        debug_off.push(out.len() as u64);
        out.extend_from_slice(img);
    }

    // Section headers.
    let mut shdrs: Vec<Shdr> = vec![Shdr::default()];
    for (i, s) in layout.sections.iter().enumerate() {
        shdrs.push(Shdr {
            name: shstrtab.add(&s.name),
            ty: if s.nobits { SHT_NOBITS } else { SHT_PROGBITS },
            flags: s.flags,
            addr: s.addr,
            offset: sec_offset[i],
            size: s.size,
            align: s.align.into(),
            ..Shdr::default()
        });
    }
    let symtab_index = shdrs.len() as u32;
    shdrs.push(Shdr {
        name: shstrtab.add(".symtab"),
        ty: SHT_SYMTAB,
        offset: symtab_off,
        size: syms.len() as u64,
        link: symtab_index + 1, // .strtab, next
        info: first_global,
        align: word as u64,
        entsize: class.sym_size() as u64,
        ..Shdr::default()
    });
    shdrs.push(Shdr {
        name: shstrtab.add(".strtab"),
        ty: SHT_STRTAB,
        offset: strtab_off,
        size: strtab.0.len() as u64,
        align: 1,
        ..Shdr::default()
    });
    for (ds, off) in layout.debug.iter().zip(debug_off) {
        shdrs.push(Shdr {
            name: shstrtab.add(&ds.name),
            ty: SHT_PROGBITS,
            flags: ds.flags,
            offset: off,
            size: ds.size,
            align: ds.align.into(),
            entsize: ds.entsize,
            ..Shdr::default()
        });
    }
    let shstrtab_name = shstrtab.add(".shstrtab");
    shdrs.push(Shdr {
        name: shstrtab_name,
        ty: SHT_STRTAB,
        offset: out.len() as u64,
        size: shstrtab.0.len() as u64,
        align: 1,
        ..Shdr::default()
    });
    out.extend_from_slice(&shstrtab.0);
    out.resize(out.len().next_multiple_of(word), 0);
    let shoff = out.len() as u64;
    for h in &shdrs {
        h.put(class, &mut out);
    }

    // ELF header.
    let mut hdr = Vec::with_capacity(class.ehdr_size());
    hdr.extend_from_slice(b"\x7fELF");
    let ident_class = match class {
        Class::Elf32 => elf::ELFCLASS32,
        Class::Elf64 => elf::ELFCLASS64,
    };
    // Class, little-endian, version 1, System V ABI.
    hdr.extend_from_slice(&[ident_class, elf::ELFDATA2LSB, 1, 0]);
    hdr.extend_from_slice(&[0; 8]);
    put16(&mut hdr, ET_EXEC);
    put16(&mut hdr, EM_RISCV);
    put32(&mut hdr, 1); // e_version
    put_word(class, &mut hdr, layout.entry);
    put_word(class, &mut hdr, if table.is_empty() { 0 } else { phoff as u64 });
    put_word(class, &mut hdr, shoff);
    put32(&mut hdr, 0); // e_flags: ilp32 or lp64 soft-float, no RVC
    put16(&mut hdr, class.ehdr_size() as u16);
    put16(&mut hdr, class.phdr_size() as u16);
    put16(&mut hdr, table.len() as u16);
    put16(&mut hdr, class.shdr_size() as u16);
    put16(&mut hdr, shdrs.len() as u16);
    put16(&mut hdr, shdrs.len() as u16 - 1); // e_shstrndx: .shstrtab, last
    debug_assert_eq!(hdr.len(), class.ehdr_size());
    out[..hdr.len()].copy_from_slice(&hdr);
    out
}

/// The executable `elf` without its debug sections (`.debug_*`, not
/// loaded), for a program that is installed rather than debugged. For a file
/// [`write`] made, of either class, the result is what it writes for the
/// same link without debug information: the bytes up to the debug sections
/// stay as they are (the program headers among them), then come the section
/// names without theirs and the section headers without theirs. A file with
/// no debug sections comes back unchanged. Anything laid out otherwise is an
/// error.
pub fn strip_debug(elf: &[u8]) -> Result<Vec<u8>, String> {
    let parsed = elf::Elf::parse(elf).map_err(|e| e.to_string())?;
    let class = parsed.class;
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
    let headers_end = class.ehdr_size() + class.phdr_size() * parsed.segments.len();
    let cut = ends.max().unwrap_or(0).max(headers_end);
    if !ordered || cut > parsed.sections[first].offset as usize {
        return Err("the debug sections are not where merlin-ld puts them (last)".into());
    }
    let mut out = elf[..cut].to_vec();
    // The kept sections' headers as they are, but for the names: a new
    // string table of theirs, in their order (offsets as write() gives them).
    let mut names = StrTab(vec![0]);
    let mut headers: Vec<Shdr> = Vec::new();
    for (i, s) in parsed.sections[..first].iter().enumerate() {
        let mut h = Shdr::from_section(s);
        if i > 0 {
            h.name = names.add(&s.name);
        }
        headers.push(h);
    }
    let shstrtab = &parsed.sections[shstrndx];
    let mut h = Shdr::from_section(shstrtab);
    h.name = names.add(&shstrtab.name);
    h.offset = out.len() as u64;
    h.size = names.0.len() as u64;
    headers.push(h);
    out.extend_from_slice(&names.0);
    out.resize(out.len().next_multiple_of(class.addr_size()), 0);
    let shoff = out.len() as u64;
    for h in &headers {
        h.put(class, &mut out);
    }
    let (shoff_at, shnum_at) = shoff_and_shnum_at(class);
    let mut field = Vec::new();
    put_word(class, &mut field, shoff);
    out[shoff_at..shoff_at + field.len()].copy_from_slice(&field);
    out[shnum_at..shnum_at + 2].copy_from_slice(&(headers.len() as u16).to_le_bytes());
    out[shnum_at + 2..shnum_at + 4].copy_from_slice(&(headers.len() as u16 - 1).to_le_bytes());
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Input, Options, ScriptSource};

    /// Code, read-only data and data, 4 bytes each (three runs: R E, R,
    /// RW), and an empty read-only section, `.empty`.
    const PROGRAM: &str = "    .text
    .globl _start
_start:
    nop
    .section .rodata, \"a\"
    .word 1
    .section .empty, \"a\"
    .data
    .word 2
";

    /// A program header as `llvm-readelf -l` lists it: type, offset,
    /// address, load address, file size, size in memory, permissions.
    type Ph = (&'static str, u64, u64, u64, u64, u64, &'static str);

    /// The program headers of `src`, assembled by merlin-as for `class`
    /// and linked with `script`.
    fn program_headers(class: Class, src: &str, script: &str) -> Vec<Ph> {
        let xlen = match class {
            Class::Elf32 => excalibur::Xlen::Rv32,
            Class::Elf64 => excalibur::Xlen::Rv64,
        };
        let data = merlin_as::assemble_to_elf_with(src, &merlin_as::Options { xlen }).unwrap();
        let opts = Options {
            script: Some(ScriptSource {
                name: "test.ld".into(),
                text: script.into(),
            }),
            entry: None,
        };
        let input = Input {
            name: "a.o".into(),
            data,
        };
        let linked = crate::link(&[input], &opts).unwrap();
        let elf = elf::Elf::parse(&linked.elf).unwrap();
        let phdrs = elf.segments.iter().map(|p| {
            let ty = match p.ty {
                PT_PHDR => "PHDR",
                PT_LOAD => "LOAD",
                t => panic!("program header type {t:#x}"),
            };
            let flags = match p.flags {
                PF_R => "R",
                f if f == PF_R | PF_X => "R E",
                f if f == PF_R | PF_W => "RW",
                f => panic!("p_flags {f:#x}"),
            };
            (ty, p.offset, p.vaddr, p.paddr, p.filesz, p.memsz, flags)
        });
        phdrs.collect()
    }

    /// `PROGRAM` linked from `start`, its three sections in a row, and
    /// `more` at the end of `SECTIONS`.
    fn at(class: Class, start: u64, more: &str) -> Vec<Ph> {
        let script = format!(
            "SECTIONS {{ . = {start:#x}; .text : {{ *(.text) }} .rodata : {{ *(.rodata) }} \
             .data : {{ *(.data) }} {more} }}"
        );
        program_headers(class, PROGRAM, &script)
    }

    /// ld.lld's PT_PHDR and the headers' PT_LOAD at 0x8000_0000, for
    /// `room` bytes of headers.
    fn loaded(class: Class, room: u64) -> [Ph; 2] {
        let ehdr = class.ehdr_size() as u64;
        let table = room - ehdr;
        [
            (
                "PHDR",
                ehdr,
                0x8000_0000 + ehdr,
                0x8000_0000 + ehdr,
                table,
                table,
                "R",
            ),
            ("LOAD", 0, 0x8000_0000, 0x8000_0000, room, room, "R"),
        ]
    }

    /// `loaded`, then `segments`.
    fn after_headers(class: Class, room: u64, segments: &[Ph]) -> Vec<Ph> {
        [&loaded(class, room)[..], segments].concat()
    }

    /// The three segments of `PROGRAM` from `start`, the first at file
    /// offset `off`.
    fn three(start: u64, off: u64) -> [Ph; 3] {
        [
            ("LOAD", off, start, start, 4, 4, "R E"),
            ("LOAD", off + 4, start + 4, start + 4, 4, 4, "R"),
            ("LOAD", off + 8, start + 8, start + 8, 4, 4, "RW"),
        ]
    }

    /// Where the first segment goes and whether the headers are loaded, on
    /// either side of their size: the ELF header and the 7 program headers
    /// ld.lld counts (PT_PHDR, the headers' PT_LOAD, the three PT_LOADs,
    /// PT_GNU_STACK and PT_RISCV_ATTRIBUTES), 0x114 bytes for ELF32 and
    /// 0x1c8 for ELF64. Every number in these tests is ld.lld's for the same
    /// object and script.
    #[test]
    fn the_headers_are_loaded_when_they_fit_below_the_first_section() {
        let class = Class::Elf32;
        assert_eq!(at(class, 0x8000_0110, ""), three(0x8000_0110, 0x1110));
        assert_eq!(
            at(class, 0x8000_0114, ""),
            after_headers(class, 0x114, &three(0x8000_0114, 0x114))
        );
        let class = Class::Elf64;
        assert_eq!(at(class, 0x8000_0114, ""), three(0x8000_0114, 0x1114));
        assert_eq!(at(class, 0x8000_01c4, ""), three(0x8000_01c4, 0x11c4));
        assert_eq!(
            at(class, 0x8000_01c8, ""),
            after_headers(class, 0x1c8, &three(0x8000_01c8, 0x1c8))
        );
        // At the start of a page there is no room below: the first segment
        // goes a page in.
        assert_eq!(at(class, 0x8000_0000, ""), three(0x8000_0000, 0x1000));
    }

    /// The room is for what ld.lld counts, not for what merlin-ld writes: no
    /// PT_RISCV_ATTRIBUTES without a `.riscv.attributes` (thrown away here),
    /// and a PT_LOAD for an empty run of sections, which ld.lld removes after
    /// it has sized the table (so at 0x8000_0190 the table has room for 8
    /// program headers, where ld.lld lists 7: PT_PHDR, the headers'
    /// PT_LOAD, three more PT_LOADs, PT_GNU_STACK and PT_RISCV_ATTRIBUTES;
    /// merlin-ld writes 5 of them).
    #[test]
    fn the_room_is_for_the_headers_ld_lld_counts() {
        let discard = "/DISCARD/ : { *(.riscv.attributes) }";
        let class = Class::Elf32;
        assert_eq!(at(class, 0x8000_00f0, discard), three(0x8000_00f0, 0x10f0));
        assert_eq!(
            at(class, 0x8000_00f4, discard),
            after_headers(class, 0xf4, &three(0x8000_00f4, 0xf4))
        );
        let class = Class::Elf64;
        assert_eq!(at(class, 0x8000_0150, discard), three(0x8000_0150, 0x1150));
        assert_eq!(
            at(class, 0x8000_0190, discard),
            after_headers(class, 0x190, &three(0x8000_0190, 0x190))
        );

        // .empty, read-only between the code and the data, is a run of its
        // own; .rodata after the data another.
        let script = |start: u64| {
            format!(
                "SECTIONS {{ . = {start:#x}; .text : {{ *(.text) }} .empty : {{ *(.empty) }} \
                 .data : {{ *(.data) }} .rodata : {{ *(.rodata) }} }}"
            )
        };
        let runs = |start: u64, off: u64| {
            [
                ("LOAD", off, start, start, 4, 4, "R E"),
                ("LOAD", off + 4, start + 4, start + 4, 4, 4, "RW"),
                ("LOAD", off + 8, start + 8, start + 8, 4, 4, "R"),
            ]
        };
        let class = Class::Elf32;
        assert_eq!(
            program_headers(class, PROGRAM, &script(0x8000_0114)),
            runs(0x8000_0114, 0x1114)
        );
        assert_eq!(
            program_headers(class, PROGRAM, &script(0x8000_0190)),
            after_headers(class, 52 + 8 * 32, &runs(0x8000_0190, 0x190))
        );
        let class = Class::Elf64;
        assert_eq!(
            program_headers(class, PROGRAM, &script(0x8000_01c8)),
            runs(0x8000_01c8, 0x11c8)
        );
    }

    /// A read-only first section shares the headers' PT_LOAD, which then
    /// starts at the page: the headers, the gap, the section. Its file size
    /// is the headers' alone if the section has no file contents (NOLOAD).
    /// In a MEMORY region the section has a PT_LOAD of its own.
    #[test]
    fn a_read_only_first_section_shares_the_headers_segment() {
        for class in [Class::Elf32, Class::Elf64] {
            let room = match class {
                Class::Elf32 => 0x114,
                Class::Elf64 => 0x1c8,
            };
            let [phdr, _] = loaded(class, room);
            let script = |noload: &str| {
                format!(
                    "SECTIONS {{ . = 0x80000400; .ro {noload} : {{ . += 0x10; }} .text : {{ *(.text) }} \
                     .rodata : {{ *(.rodata) }} .data : {{ *(.data) }} }}"
                )
            };
            let with = |first: Ph| {
                vec![
                    phdr,
                    first,
                    ("LOAD", 0x410, 0x8000_0410, 0x8000_0410, 4, 4, "R E"),
                    ("LOAD", 0x414, 0x8000_0414, 0x8000_0414, 4, 4, "R"),
                    ("LOAD", 0x418, 0x8000_0418, 0x8000_0418, 4, 4, "RW"),
                ]
            };
            assert_eq!(
                program_headers(class, PROGRAM, &script("")),
                with(("LOAD", 0, 0x8000_0000, 0x8000_0000, 0x410, 0x410, "R"))
            );
            assert_eq!(
                program_headers(class, PROGRAM, &script("(NOLOAD)")),
                with(("LOAD", 0, 0x8000_0000, 0x8000_0000, room, 0x410, "R"))
            );
            let memory = "MEMORY { RAM (rwx) : ORIGIN = 0x80000400, LENGTH = 1M } SECTIONS { \
                          .rodata : { *(.rodata) } > RAM .text : { *(.text) } > RAM \
                          .data : { *(.data) } > RAM }";
            let own = [
                ("LOAD", 0x400, 0x8000_0400, 0x8000_0400, 4, 4, "R"),
                ("LOAD", 0x404, 0x8000_0404, 0x8000_0404, 4, 4, "R E"),
                ("LOAD", 0x408, 0x8000_0408, 0x8000_0408, 4, 4, "RW"),
            ];
            assert_eq!(
                program_headers(class, PROGRAM, memory),
                after_headers(class, room, &own)
            );
        }
    }

    /// Without any section ld.lld's lowest address is the top of the
    /// address space, and the headers fit below it, in the last page.
    #[test]
    fn without_sections_the_headers_go_in_the_last_page() {
        let src = "    .globl _start\n    .set _start, 0x80000000\n";
        let script = "SECTIONS { /DISCARD/ : { *(.text) } }";
        assert_eq!(
            program_headers(Class::Elf32, src, script),
            [
                ("PHDR", 52, 0xffff_f034, 0xffff_f034, 0x80, 0x80, "R"),
                ("LOAD", 0, 0xffff_f000, 0xffff_f000, 0xb4, 0xb4, "R"),
            ]
        );
        let page = 0xffff_ffff_ffff_f000;
        assert_eq!(
            program_headers(Class::Elf64, src, script),
            [
                ("PHDR", 64, page + 64, page + 64, 0xe0, 0xe0, "R"),
                ("LOAD", 0, page, page, 0x120, 0x120, "R"),
            ]
        );
    }
}
