//! Symbol resolution and section layout: everything up to (not including)
//! relocation. The result, [`Layout`], fixes every address in the output.
//!
//! Steps, in order:
//!
//! 1. Parse every object ([`crate::object`]).
//! 2. Build the global symbol table. Locals stay per object. Two strong
//!    definitions of a global are an error; a strong one beats a weak one.
//! 3. Assign input sections to output sections: script order, first matching
//!    `*(pattern)` wins, objects in command-line order, sections in file order.
//!    An allocated input section no rule matches is an error if it is
//!    non-empty or defines a global; an empty one is an orphan, which goes
//!    where ld.lld puts it ([`place_orphans`]).
//! 4. Walk `SECTIONS` with the location counter, placing each input section at
//!    its alignment and evaluating script assignments as they come. As in
//!    ld.lld, an output section is dropped only when no input section (not
//!    even an empty one) matched it, it has no assignments, no `ADDR` in
//!    the script names it and its `AT` and `ALIGN` use no symbol.
//! 5. Report undefined globals (`a.o: undefined reference to 'foo'`).
//! 6. Pick the entry point.
//!
//! Debug information is kept apart from all this: every `.debug_*` input
//! section (not loaded, so no rule places it) goes into the output section of
//! its name, the objects' in command-line order, as ld.lld does without a rule
//! for them. Those sections have no address; a symbol in one is its offset
//! there (see [`Layout::debug`]).
//!
//! ## Two widths
//!
//! The executable has the class of its objects ([`Layout::class`]): ELF32
//! for RV32, ELF64 for RV64. Addresses and sizes are `u64` for both. What
//! differs is how far they go and how the script computes:
//!
//! - ELF64 has the whole 64-bit address space, and script arithmetic is
//!   `u64` modulo 2^64, as ld.lld's (`0 - 1` is `0xffff_ffff_ffff_ffff`). A
//!   section may not run past 2^64 (ld.lld's `checkSections`: "exceeds
//!   available address space").
//! - ELF32 (RV32, until #159) has 4 GiB: the location counter, script
//!   symbols, section ends, the MEMORY region and the entry point must stay
//!   within it, and script arithmetic is exact (`i64`, an error on overflow).
//!   ld.lld truncates such values to 32 bits; merlin-ld says what is wrong.
//!
//! ## Load addresses (`AT`)
//!
//! A section runs at its address (VMA, [`OutputSection::addr`]) and is
//! loaded at its load address (LMA, [`OutputSection::lma`], its segment's
//! `p_paddr`). They are the same unless the script says otherwise with
//! `.name : AT(lma) { ... }`: a kernel linked in the top 2 GiB of the address
//! space, which paging maps there, has to be loaded somewhere in RAM first,
//! and Linux's riscv `vmlinux.lds` says where with
//! `AT(ADDR(.x) - LOAD_OFFSET)`. What ld.lld does, and merlin-ld with it:
//!
//! - `.` in the `AT` expression is the section's start, aligned
//!   (`AT(. - OFFSET)` is the same as `AT(ADDR(.x) - OFFSET)`).
//! - The difference LMA - VMA (modulo 2^64) carries on: a section without
//!   an `AT` is loaded at its address plus the difference of the last
//!   section that had one (NOLOAD ones too, empty ones too), 0 at first.
//!   So one `AT` on the first section moves the whole image.
//! - A section dropped for being empty evaluates nothing: its `AT` sets no
//!   difference.
//! - A section with an `AT` starts a segment of its own, even when it
//!   follows the previous one with the same permissions ([`OutputSection::at`],
//!   [`crate::write::segments`]). So every section of a segment has the same
//!   difference, and the segment's `p_paddr` is its first section's LMA.
//! - Two sections with file contents must not be loaded over each other
//!   (ld.lld's "load address range overlaps").
//! - File offsets follow the addresses, not the load addresses, and the
//!   entry point is an address.
//!
//! `ADDR(.x)` is the address of output section `.x`: one already placed,
//! or the one being placed (in its `AT` and its assignments). A section an
//! `ADDR` names is kept even if it is empty, as in ld.lld.

use crate::object::{
    self, Class, Object, SHF_ALLOC, SHF_EXECINSTR, SHF_MERGE, SHF_STRINGS, SHF_WRITE, SHN_ABS, SHN_LORESERVE,
    SHN_UNDEF, STT_SECTION,
};
use crate::script::{self, Assign, Command, Expr, Item, Script};
use crate::{Error, Input, Options};
use std::collections::{HashMap, HashSet};

/// The name diagnostics use for the built-in layout.
pub const DEFAULT_SCRIPT_NAME: &str = "<default layout>";

/// One section of the executable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputSection {
    pub name: String,
    /// The address the section runs at (its VMA).
    pub addr: u64,
    pub size: u64,
    /// The address it is loaded at (its LMA, the segment's `p_paddr`); the
    /// same as `addr` until a script sets one with `AT(...)`.
    pub lma: u64,
    /// The script gave this section its own `AT(...)`: it starts a segment.
    pub at: bool,
    /// Largest alignment of its input sections.
    pub align: u32,
    /// `SHF_ALLOC` plus the union of the inputs' `SHF_WRITE` / `SHF_EXECINSTR`;
    /// without inputs, the `SHF_ALLOC` and `SHF_WRITE` of the previous section
    /// that had some (never `SHF_EXECINSTR`, as in ld.lld).
    pub flags: u64,
    /// Occupies no file space (`.bss`): every input is `SHT_NOBITS`
    /// (and there is at least one), or the script says `(NOLOAD)`.
    pub nobits: bool,
    /// Input sections in address order.
    pub inputs: Vec<InputRef>,
}

impl OutputSection {
    /// ld.lld makes it RELRO ([`is_relro`]): a run of such sections ends its
    /// `PT_LOAD` and has a `PT_GNU_RELRO` ([`crate::write::segments`]).
    pub fn is_relro(&self) -> bool {
        is_relro(&self.name, self.flags)
    }
}

/// An input section placed in an output section.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputRef {
    /// Index into [`Layout::objects`].
    pub object: usize,
    /// ELF section index within that object.
    pub section: usize,
    pub addr: u64,
}

/// Where a global symbol's definition lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Def {
    /// `objects[obj].symbols[sym]`.
    Object { obj: usize, sym: usize },
    /// Assigned by the linker script.
    Script(u64),
}

/// Where an input section ended up.
#[derive(Debug, Clone, Copy)]
struct Placed {
    addr: u64,
    /// Index into [`Layout::sections`], or `None` if its output section was
    /// not created (a layout error).
    out: Option<usize>,
}

/// A debug section of the executable (`.debug_line`, ...): not loaded, it
/// goes after everything that is, and its input sections are concatenated,
/// each at its alignment. [`InputRef::addr`] is an input's offset in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DebugSection {
    pub name: String,
    pub size: u64,
    pub align: u32,
    /// `SHF_MERGE | SHF_STRINGS` when every input has them (string tables).
    pub flags: u64,
    pub entsize: u64,
    pub inputs: Vec<InputRef>,
}

/// Whether an input section is debug information merlin-ld keeps.
pub fn is_debug(sec: &object::Section) -> bool {
    !sec.is_alloc() && sec.ty == object::SHT_PROGBITS && sec.name.starts_with(".debug_")
}

#[derive(Debug)]
pub struct Layout {
    /// The class of the executable: its objects' (the first one's).
    pub class: Class,
    pub objects: Vec<Object>,
    pub sections: Vec<OutputSection>,
    /// The debug sections, by name in the order the inputs first have them.
    pub debug: Vec<DebugSection>,
    /// Defined globals in first-definition order. Script assignments override
    /// object definitions of the same name.
    pub globals: Vec<(String, Def)>,
    pub entry: u64,
    /// The entry symbol's name, unless the entry is a number or a fallback.
    pub entry_symbol: Option<String>,
    pub warnings: Vec<String>,
    /// The script has a `MEMORY` region, which every output section is in
    /// (ld.lld gives every section a region when there are any). Such a
    /// section never shares a segment with the ELF and program headers,
    /// which are in no region ([`crate::write::segments`]).
    pub memory: bool,
    /// ld.lld would write a `.riscv.attributes` section for these objects,
    /// and so a `PT_RISCV_ATTRIBUTES` program header: one of them has an
    /// `SHT_RISCV_ATTRIBUTES` section, and the script does not throw it away
    /// ([`riscv_attributes_kept`]). merlin-ld writes neither, but leaves room
    /// for that header, as it does for every one ld.lld counts
    /// ([`crate::write`], "The headers").
    pub riscv_attributes: bool,
    /// `[object][section]`.
    placement: Vec<Vec<Option<Placed>>>,
    /// The input sections `/DISCARD/` matched, as `(object, section)`.
    discarded: HashSet<(usize, usize)>,
    /// `(object, section)` of a debug input section: its offset in its
    /// output section.
    debug_offsets: HashMap<(usize, usize), u64>,
    global_index: HashMap<String, usize>,
}

fn err(msg: impl Into<String>) -> Error {
    Error(msg.into())
}

/// `v` rounded up to a multiple of `a`; `None` if that is past 2^64 (only
/// an ELF64 address near the top can get there).
fn align_up(v: u64, a: u64) -> Option<u64> {
    v.checked_next_multiple_of(a)
}

/// The address of a symbol `value` bytes into a section at `base`. ELF32
/// (RV32, until #159): addresses are 32 bits and wrap at 4 GiB, as they
/// always have in merlin-ld (a symbol defined far past its section's end).
fn symbol_address(class: Class, base: u64, value: u64) -> u64 {
    match class {
        Class::Elf32 => u64::from((base as u32).wrapping_add(value as u32)),
        Class::Elf64 => base.wrapping_add(value),
    }
}

impl Layout {
    /// The final address of a global symbol (object- or script-defined).
    pub fn symbol(&self, name: &str) -> Option<u64> {
        let &i = self.global_index.get(name)?;
        self.def_value(self.globals[i].1)
    }

    pub fn section(&self, name: &str) -> Option<&OutputSection> {
        self.sections.iter().find(|s| s.name == name)
    }

    pub fn def_value(&self, d: Def) -> Option<u64> {
        match d {
            Def::Script(v) => Some(v),
            Def::Object { obj, sym } => self.object_symbol(obj, sym),
        }
    }

    /// The address of a symbol defined in `obj`; `None` if it is undefined
    /// there or its section is not loaded.
    pub fn object_symbol(&self, obj: usize, sym: usize) -> Option<u64> {
        let s = &self.objects[obj].symbols[sym];
        match s.shndx {
            SHN_UNDEF => None,
            SHN_ABS => Some(s.value),
            sec => self
                .section_base(obj, sec as usize)
                .map(|a| symbol_address(self.class, a, s.value)),
        }
    }

    /// Where an input section starts: its address if it is loaded, its
    /// offset in its output section if it is debug information.
    fn section_base(&self, obj: usize, sec: usize) -> Option<u64> {
        self.input_addr(obj, sec)
            .or_else(|| self.debug_offsets.get(&(obj, sec)).copied())
    }

    /// Whether symbol `sym` of `obj`, as the link resolves it, has no place
    /// in the executable: it is defined in a section that is neither loaded
    /// nor debug information, or that `/DISCARD/` threw away (a global
    /// counts by its definition, in whichever object that is), or it is a
    /// weak reference nobody defines. Debug information may point at such
    /// symbols (the code of a discarded function); there the linker writes
    /// a tombstone, as ld.lld's `relocateNonAlloc` does for a symbol without
    /// an output section. Absolute and script symbols have their values.
    pub fn in_dropped_section(&self, obj: usize, sym: usize) -> bool {
        let mut s = &self.objects[obj].symbols[sym];
        let mut obj = obj;
        if sym != 0 && !s.is_local() {
            match self.global_index.get(&s.name).map(|&i| self.globals[i].1) {
                Some(Def::Object { obj: o, sym: d }) => {
                    obj = o;
                    s = &self.objects[o].symbols[d];
                }
                Some(Def::Script(_)) => return false,
                None => return s.is_weak(),
            }
        }
        s.shndx != SHN_UNDEF && s.shndx < SHN_LORESERVE && self.section_base(obj, s.shndx as usize).is_none()
    }

    /// Address of an input section, if it was placed.
    pub fn input_addr(&self, obj: usize, sec: usize) -> Option<u64> {
        self.placement.get(obj)?.get(sec)?.map(|p| p.addr)
    }

    /// Index into [`Layout::sections`] of the output section holding an input
    /// section; `None` if it isn't loaded or its output section was discarded.
    pub fn input_output_section(&self, obj: usize, sec: usize) -> Option<usize> {
        self.placement.get(obj)?.get(sec)?.and_then(|p| p.out)
    }

    /// S for a relocation in `obj` against its symbol `sym`.
    pub fn reloc_symbol(&self, obj: usize, sym: usize) -> Result<u64, String> {
        let o = &self.objects[obj];
        let s = &o.symbols[sym];
        if sym == 0 {
            return Ok(0);
        }
        if s.is_local() {
            if s.ty == STT_SECTION {
                return self.section_base(obj, s.shndx as usize).ok_or_else(|| {
                    let name = o.symbol_display_name(sym as u32);
                    format!(
                        "refers to section '{name}', which {}",
                        self.unplaced(obj, s.shndx)
                    )
                });
            }
            return self.object_symbol(obj, sym).ok_or_else(|| {
                format!(
                    "refers to '{}', whose section {}",
                    s.name,
                    self.unplaced(obj, s.shndx)
                )
            });
        }
        match self.global_index.get(&s.name) {
            Some(&i) => self.def_value(self.globals[i].1).ok_or_else(|| {
                let why = match self.globals[i].1 {
                    Def::Object { obj, sym } => self.unplaced(obj, self.objects[obj].symbols[sym].shndx),
                    Def::Script(_) => "is not loaded",
                };
                format!("refers to '{}', whose section {why}", s.name)
            }),
            None if s.is_weak() => Ok(0),
            None => Err(format!("undefined reference to '{}'", s.name)),
        }
    }

    /// Why input section `sec` of `obj` has no address, for an error
    /// message: `/DISCARD/` threw it away, or it is not loaded at all.
    fn unplaced(&self, obj: usize, sec: u16) -> &'static str {
        if self.discarded.contains(&(obj, sec as usize)) {
            "was discarded by /DISCARD/"
        } else {
            "is not loaded"
        }
    }

    /// Whether symbol `sym` of `obj` is a weak reference nobody defines (its
    /// [`Layout::reloc_symbol`] value is then 0).
    pub fn is_undefined_weak(&self, obj: usize, sym: usize) -> bool {
        let s = &self.objects[obj].symbols[sym];
        sym != 0 && s.is_weak() && s.is_undefined() && !self.global_index.contains_key(&s.name)
    }
}

/// Mutable state while walking `SECTIONS`.
struct Walker<'a> {
    class: Class,
    script_name: &'a str,
    script: &'a Script,
    objects: &'a [Object],
    placement: Vec<Vec<Option<Placed>>>,
    discarded: HashSet<(usize, usize)>,
    globals: Vec<(String, Def)>,
    global_index: HashMap<String, usize>,
    sections: Vec<OutputSection>,
    /// The output section being placed and its start, once that is known:
    /// what `ADDR` of it is, in its `AT` and among its assignments.
    current: Option<(&'a str, u64)>,
    /// The names of the script's output sections (not `/DISCARD/`): an
    /// `ADDR` of one not placed yet names a later one.
    script_sections: HashSet<&'a str>,
    errors: Vec<Error>,
}

impl Walker<'_> {
    fn script_err(&mut self, line: usize, msg: impl Into<String>) {
        self.errors
            .push(err(format!("{}:{line}: {}", self.script_name, msg.into())));
    }

    /// `a + b` in script arithmetic. ELF64: modulo 2^64, as ld.lld computes
    /// (`. = 0xffffffff80000000 + 0x80000000` is 0). ELF32 (RV32, until #159):
    /// the values are `i64`s (see [`Walker::eval`]) and the sum is exact, an
    /// overflow an error.
    fn add(&self, a: u64, b: u64) -> Result<u64, String> {
        match self.class {
            Class::Elf32 => (a as i64)
                .checked_add(b as i64)
                .map(|v| v as u64)
                .ok_or("arithmetic overflow".into()),
            Class::Elf64 => Ok(a.wrapping_add(b)),
        }
    }

    /// `a - b`, as [`Walker::add`].
    fn sub(&self, a: u64, b: u64) -> Result<u64, String> {
        match self.class {
            Class::Elf32 => (a as i64)
                .checked_sub(b as i64)
                .map(|v| v as u64)
                .ok_or("arithmetic overflow".into()),
            Class::Elf64 => Ok(a.wrapping_sub(b)),
        }
    }

    /// The value of a script expression with the location counter at `dot`.
    ///
    /// The result is a `u64` on both classes. ELF64 reads it as one: a 64-bit
    /// address, any of them. ELF32 (RV32, until #159) reads its bits as an
    /// `i64` (`v as i64`), the type merlin-ld's RV32 scripts have always been
    /// evaluated in: numbers up to `i64::MAX`, exact `+` and `-`, so `0 - 1`
    /// is -1, which the uses then refuse (a backward location counter, a
    /// symbol that does not fit in 32 bits) rather than wrap.
    fn eval(&self, e: &Expr, dot: u64) -> Result<u64, String> {
        Ok(match e {
            Expr::Num(n) => *n,
            Expr::Dot => dot,
            Expr::Symbol(name) => {
                let value = self
                    .global_index
                    .get(name)
                    .and_then(|&i| match self.globals[i].1 {
                        Def::Script(v) => Some(v),
                        Def::Object { obj, sym } => {
                            let s = &self.objects[obj].symbols[sym];
                            match s.shndx {
                                SHN_ABS => Some(s.value),
                                sec => self.placement[obj][sec as usize]
                                    .map(|p| symbol_address(self.class, p.addr, s.value)),
                            }
                        }
                    });
                match value {
                    Some(v) => v,
                    None if self.global_index.contains_key(name) => {
                        return Err(format!("symbol '{name}' is used before its section is placed"))
                    }
                    None => return Err(format!("undefined symbol '{name}' in expression")),
                }
            }
            Expr::Align(n) => {
                let n = self.eval(n, dot)?;
                // ELF32 (RV32, until #159): a negative i64 is not an alignment either.
                let positive = match self.class {
                    Class::Elf32 => (n as i64) > 0,
                    Class::Elf64 => n > 0,
                };
                if !positive {
                    return Err(format!("ALIGN({}): alignment must be positive", self.show(n)));
                }
                // Only ELF64 can get past 2^64 (`. = ALIGN(16)` at
                // 0xffff_ffff_ffff_fff8). ld.lld wraps to 0 and then reports a
                // backward move; saying so here is clearer.
                align_up(dot, n).ok_or("arithmetic overflow")?
            }
            Expr::Origin(r) | Expr::Length(r) => match &self.script.memory {
                Some(m) if &m.name == r => match e {
                    Expr::Origin(_) => m.origin,
                    _ => m.length,
                },
                _ => return Err(format!("unknown memory region '{r}'")),
            },
            // The address (VMA) of an output section: one already placed,
            // or the one being placed once its start is known (in its own
            // `AT`, not in its `ALIGN`, which that start depends on).
            // merlin-ld walks the script once, so a later section has no
            // address yet: an error, where ld.lld would go round again
            // with the addresses of the first pass, as for symbols.
            Expr::Addr(name) => match self.current {
                Some((cur, start)) if cur == name => start,
                _ => match self.sections.iter().find(|s| &s.name == name) {
                    Some(s) => s.addr,
                    None if self.script_sections.contains(name.as_str()) => {
                        return Err(format!("section '{name}' is used before it is placed"))
                    }
                    // ld.lld's words. (A section of the script that ADDR names
                    // is never dropped, so only an unknown name gets here.)
                    None => return Err(format!("undefined section {name}")),
                },
            },
            Expr::Add(a, b) => self.add(self.eval(a, dot)?, self.eval(b, dot)?)?,
            Expr::Sub(a, b) => self.sub(self.eval(a, dot)?, self.eval(b, dot)?)?,
        })
    }

    /// A script value as a message shows it in decimal: signed on ELF32 (RV32,
    /// until #159), where values are `i64`s, unsigned on ELF64.
    fn show(&self, v: u64) -> String {
        match self.class {
            Class::Elf32 => (v as i64).to_string(),
            Class::Elf64 => v.to_string(),
        }
    }

    /// Runs one assignment. Returns the new location counter.
    fn assign(&mut self, a: &Assign, dot: u64) -> u64 {
        let v = match self.eval(&a.expr, dot) {
            Ok(v) => v,
            Err(e) => {
                self.script_err(a.line, e);
                return dot;
            }
        };
        match &a.symbol {
            None => {
                let new = if a.add { self.add(dot, v) } else { Ok(v) };
                let new = match new {
                    Ok(new) => new,
                    Err(e) => {
                        self.script_err(a.line, e);
                        return dot;
                    }
                };
                // ELF32 (RV32, until #159): compared as i64s, so a negative
                // value (`. = 0 - 1`) is a move backward. On ELF64 a `. += n`
                // that wraps past 2^64 comes out below `dot`, backward too.
                let backward = match self.class {
                    Class::Elf32 => (new as i64) < dot as i64,
                    Class::Elf64 => new < dot,
                };
                if backward {
                    self.script_err(
                        a.line,
                        format!("cannot move the location counter backward (from {dot:#x} to {new:#x})"),
                    );
                    return dot;
                }
                // ELF32 (RV32, until #159): 4 GiB of address space. `.` may be
                // exactly 2^32, the end of a section that fills it.
                if self.class == Class::Elf32 && new > 1 << 32 {
                    self.script_err(
                        a.line,
                        format!("location counter {new:#x} is past the 4 GiB address space"),
                    );
                    return dot;
                }
                new
            }
            Some(name) => {
                // ELF32 (RV32, until #159): a symbol's value is 32 bits (ld.lld
                // would truncate it). Read as an i64, so -1 does not fit either.
                if self.class == Class::Elf32 && u32::try_from(v as i64).is_err() {
                    self.script_err(
                        a.line,
                        format!("value {v:#x} of '{name}' does not fit in 32 bits"),
                    );
                    return dot;
                }
                // A script assignment overrides an object's definition (as in ld.lld).
                match self.global_index.get(name) {
                    Some(&i) => self.globals[i].1 = Def::Script(v),
                    None => {
                        self.global_index.insert(name.clone(), self.globals.len());
                        self.globals.push((name.clone(), Def::Script(v)));
                    }
                }
                dot
            }
        }
    }
}

/// merlin-ld does not merge `SHF_MERGE` sections (clang's `.rodata.str1.1`
/// string literals, `.rodata.cst8` constants): it places them like any other
/// section. ld.lld keeps one copy of each identical string or constant, so
/// when an output section holds duplicates its size, and every address after
/// it, differ from ld.lld's. This says so, once per affected output section.
fn merge_warnings(layout: &Layout) -> Vec<String> {
    let mut warnings = Vec::new();
    for os in &layout.sections {
        // ld.lld merges within one output section, per (flags, entsize, align).
        let mut seen: HashSet<(u64, u64, u32, &[u8])> = HashSet::new();
        let mut dup_bytes = 0usize;
        for i in &os.inputs {
            let sec = &layout.objects[i.object].sections[i.section];
            if sec.flags & SHF_MERGE == 0 || sec.is_nobits() || sec.entsize == 0 {
                continue;
            }
            let e = sec.entsize as usize;
            let mut rest = &sec.data[..];
            while !rest.is_empty() {
                let len = if sec.flags & SHF_STRINGS != 0 {
                    // A string runs up to and including its all-zero entry.
                    rest.chunks(e)
                        .position(|c| c.iter().all(|&b| b == 0))
                        .map_or(rest.len(), |n| (n + 1) * e)
                        .min(rest.len())
                } else {
                    e.min(rest.len())
                };
                let (entry, tail) = rest.split_at(len);
                if !seen.insert((sec.flags, sec.entsize, sec.align, entry)) {
                    dup_bytes += len;
                }
                rest = tail;
            }
        }
        if dup_bytes > 0 {
            warnings.push(format!(
                "{}: mergeable (SHF_MERGE) input sections hold {dup_bytes} bytes of duplicate strings or \
                 constants; merlin-ld keeps them (it does not merge), so this section is larger than \
                 ld.lld's and the addresses after it differ",
                os.name
            ));
        }
    }
    warnings
}

/// Whether a section loaded at `lma` with `size` bytes fits the class's
/// address space, as its address range must. ELF32 (RV32, until #159):
/// within 4 GiB, which ld.lld does not check (it truncates `p_paddr` to 32
/// bits). ELF64: below 2^64, which ld.lld does not check either (`p_paddr +
/// p_memsz` would wrap); merlin-ld says so rather than write a segment that
/// ends before it starts.
fn load_range_fits(class: Class, lma: u64, size: u64) -> bool {
    let end = lma.checked_add(size);
    match class {
        Class::Elf32 => end.is_some_and(|end| end <= 1 << 32),
        Class::Elf64 => end.is_some(),
    }
}

/// Sections loaded over each other, as ld.lld's checkSections finds them
/// ("load address range overlaps"): with `AT`, two sections can be given
/// load addresses that overlap although their addresses do not, and a
/// loader would write one's bytes over the other's. Only sections with
/// bytes in the file count (not empty or NOBITS ones, which load nothing).
/// Like ld.lld, sort them by load address and compare each with the next
/// one, naming the lower one first (its range is the one the other starts
/// in).
fn load_overlaps(class: Class, sections: &[OutputSection]) -> Vec<Error> {
    // (One whose load range does not fit has been reported already.)
    let mut loaded: Vec<&OutputSection> = sections
        .iter()
        .filter(|s| !s.nobits && s.size > 0 && load_range_fits(class, s.lma, s.size))
        .collect();
    // Stable: sections at the same load address stay in script order.
    loaded.sort_by_key(|s| s.lma);
    // `lma + size` does not overflow: the load range fits.
    let range = |s: &OutputSection| format!("[{:#x}, {:#x}]", s.lma, s.lma + s.size - 1);
    loaded
        .windows(2)
        .filter(|w| w[1].lma < w[0].lma + w[0].size)
        .map(|w| {
            let (a, b) = (w[0], w[1]);
            err(format!(
                "section {} load address range overlaps with {} ({} is {}, {} is {})",
                a.name,
                b.name,
                a.name,
                range(a),
                b.name,
                range(b)
            ))
        })
        .collect()
}

/// Whether ld.lld would write a `.riscv.attributes` section: it merges every
/// input `SHT_RISCV_ATTRIBUTES` section (clang and merlin-as give each
/// object one) into one section of that name, which the script then places
/// like any other, by its name: the first output section whose pattern
/// matches `.riscv.attributes` gets it, and only a `/DISCARD/` there throws
/// it away. (The merged section belongs to no object, so a pattern that
/// names one would not match it; merlin-ld's patterns name none.)
fn riscv_attributes_kept(objects: &[Object], script: &Script) -> bool {
    let any = objects
        .iter()
        .any(|o| o.sections.iter().any(|s| s.ty == elf::SHT_RISCV_ATTRIBUTES));
    let first_rule = script.commands.iter().find_map(|c| match c {
        Command::Output(o) => o
            .items
            .iter()
            .any(|i| matches!(i, Item::Input(p) if p.iter().any(|p| script::glob_match(p, ".riscv.attributes"))))
            .then_some(o),
        Command::Assign(_) => None,
    });
    any && !first_rule.is_some_and(|o| o.is_discard())
}

/// The names of the writable sections ld.lld makes RELRO (`isRelroSection`,
/// on by default: `-z relro`): read-only once a dynamic loader has
/// relocated them. A static executable has no loader to do that, but the
/// sections still get a `PT_GNU_RELRO` and end their `PT_LOAD`
/// ([`crate::write::segments`]). ld.lld's other RELRO sections are ones
/// merlin-ld never makes: thread-local ones, `SHT_INIT_ARRAY` and the like
/// (an allocated input must be `PROGBITS` or `NOBITS`), and the `.got`
/// that ld.lld fills.
const RELRO_NAMES: [&str; 11] = [
    ".data.rel.ro",
    ".bss.rel.ro",
    ".ctors",
    ".dtors",
    ".jcr",
    ".eh_frame",
    ".fini_array",
    ".init_array",
    ".preinit_array",
    ".dynamic",
    ".toc",
];

/// Whether an output section of this name and these flags is RELRO in
/// ld.lld: writable and named in [`RELRO_NAMES`].
pub fn is_relro(name: &str, flags: u64) -> bool {
    flags & SHF_WRITE != 0 && RELRO_NAMES.contains(&name)
}

/// ld.lld's sort rank of an output section (`getSectionRank`), for the links
/// merlin-ld makes (RISC-V, no TLS, large or partitioned sections, no
/// `--section-start`): read-only, executable, writable and executable,
/// RELRO, writable, and within each `NOBITS` last. Only which bits two ranks
/// differ in matters here, for placing an orphan ([`orphan_position`]).
fn section_rank(name: &str, flags: u64, nobits: bool) -> u32 {
    const NOT_ADDR_SET: u32 = 1 << 27; // no --section-start for it
    const WRITE: u32 = 1 << 14;
    const EXEC_WRITE: u32 = 1 << 13;
    const EXEC: u32 = 1 << 12;
    const RODATA: u32 = 1 << 11;
    const NOT_LARGE: u32 = 1 << 9; // x86-64's large read-only data goes first
    const NOT_RELRO: u32 = 1 << 8;
    const NOT_TLS: u32 = 1 << 7;
    const BSS: u32 = 1 << 6;
    let mut rank = NOT_ADDR_SET;
    rank |= match (flags & SHF_WRITE != 0, flags & SHF_EXECINSTR != 0) {
        (false, false) => RODATA | NOT_LARGE,
        (false, true) => EXEC,
        (true, true) => EXEC_WRITE,
        (true, false) if is_relro(name, flags) => WRITE | NOT_TLS,
        (true, false) => WRITE | NOT_TLS | NOT_RELRO,
    };
    if nobits {
        rank |= BSS;
    }
    // RISC-V: `.sdata` and `.sbss` next to each other, between the other
    // data and the other NOBITS sections (for GP relaxation, as GNU ld).
    if name == ".sdata" || (nobits && name != ".sbss") {
        rank |= 1;
    }
    rank
}

/// The commands of `script` with what each `*(...)` item matched (`rules`),
/// and the empty orphans (`orphans`, input sections no rule matched) placed
/// among them as ld.lld places orphan sections (`addOrphanSections`,
/// `sortSections`). Orphans of one name make one output section, the
/// objects' in command-line order, or join the script's output section of
/// that name, at its end. A new one goes next to the output section most like
/// it ([`orphan_position`]), so that it shares its segment if it can; as
/// any output section with an input, it is kept although it is empty,
/// aligns the location counter and can make a segment of its own.
/// The orphans that are not empty are errors (see "Placement" in the spec).
#[allow(clippy::type_complexity)]
fn place_orphans(
    script: &Script,
    rules: Vec<Vec<Vec<(usize, usize)>>>,
    objects: &[Object],
    orphans: &[(usize, usize)],
) -> (Vec<Command>, Vec<Vec<Vec<(usize, usize)>>>) {
    let mut cmds = script.commands.clone();
    let mut rules = rules;
    let in_script = cmds.len();
    let mut new: Vec<(String, Vec<(usize, usize)>)> = Vec::new();
    for &(oi, si) in orphans {
        let name = &objects[oi].sections[si].name;
        let named = cmds[..in_script]
            .iter()
            .position(|c| matches!(c, Command::Output(o) if !o.is_discard() && o.name == *name));
        if let Some(ci) = named {
            // ld.lld's recordSection: into the last `*(...)` if the section
            // ends with one, else into a new one after its assignments.
            let Command::Output(o) = &mut cmds[ci] else {
                unreachable!()
            };
            if !matches!(o.items.last(), Some(Item::Input(_))) {
                o.items.push(Item::Input(Vec::new()));
                rules[ci].push(Vec::new());
            }
            rules[ci].last_mut().unwrap().push((oi, si));
        } else if let Some((_, inputs)) = new.iter_mut().find(|(n, _)| n == name) {
            inputs.push((oi, si));
        } else {
            new.push((name.clone(), vec![(oi, si)]));
        }
    }
    for (name, inputs) in new {
        cmds.push(Command::Output(script::OutputDesc {
            name,
            noload: false,
            at: None,
            align: None,
            items: vec![Item::Input(Vec::new())],
            region: None,
            line: 0,
        }));
        rules.push(vec![inputs]);
    }
    // The rank of each output section with inputs (ld.lld's
    // hasInputSections; not `/DISCARD/`, which is no output section).
    let mut ranks: Vec<Option<u32>> = cmds
        .iter()
        .zip(&rules)
        .map(|(c, r)| {
            let Command::Output(o) = c else { return None };
            let inputs: Vec<&(usize, usize)> = r.iter().flatten().collect();
            if o.is_discard() || inputs.is_empty() {
                return None;
            }
            let sec = |&&(oi, si): &&(usize, usize)| &objects[oi].sections[si];
            let flags = inputs
                .iter()
                .map(sec)
                .fold(SHF_ALLOC, |f, s| f | (s.flags & (SHF_WRITE | SHF_EXECINSTR)));
            let nobits = o.noload || inputs.iter().map(sec).all(|s| s.is_nobits());
            Some(section_rank(&o.name, flags, nobits))
        })
        .collect();
    // The orphans in rank order (stable), then each moved to its place, with
    // those of the same rank after it (ld.lld moves them in one go).
    let mut order: Vec<usize> = (in_script..cmds.len()).collect();
    order.sort_by_key(|&k| ranks[k]);
    let tail: Vec<(Command, Vec<Vec<(usize, usize)>>, Option<u32>)> = order
        .iter()
        .map(|&k| (cmds[k].clone(), rules[k].clone(), ranks[k]))
        .collect();
    cmds.truncate(in_script);
    rules.truncate(in_script);
    ranks.truncate(in_script);
    for (c, r, rank) in tail {
        cmds.push(c);
        rules.push(r);
        ranks.push(rank);
    }
    // ld.lld's "horrible special case": a first `. = ...` before every
    // section stays first, so that every section is after it.
    let skip = |c: &Command| matches!(c, Command::Assign(a) if a.symbol.is_some());
    let mut first = cmds.iter().position(|c| !skip(c)).unwrap_or(cmds.len());
    if matches!(cmds.get(first), Some(Command::Assign(_))) {
        first += 1;
    }
    let mut next = in_script;
    while next < cmds.len() {
        let rank = ranks[next];
        let end = (next + 1..cmds.len())
            .find(|&k| ranks[k] != rank)
            .unwrap_or(cmds.len());
        let pos = orphan_position(&cmds, &ranks, first, next, script.memory.is_some());
        cmds[pos..end].rotate_right(end - next);
        rules[pos..end].rotate_right(end - next);
        ranks[pos..end].rotate_right(end - next);
        next = end;
    }
    // An orphan's errors (an alignment past 2^64, an overflowed region) give
    // the line of the command before it.
    let mut line = 1;
    for c in &mut cmds {
        match c {
            Command::Output(o) if o.line == 0 => o.line = line,
            Command::Output(o) => line = o.line,
            Command::Assign(a) => line = a.line,
        }
    }
    (cmds, rules)
}

/// Where ld.lld's `findOrphanPos` puts the orphan `cmds[orphan]` among
/// `cmds[first..orphan]`, given each command's rank if it is an output
/// section with inputs: next to the one whose rank shares the most leading
/// bits with its own (the first such one if the orphan ranks lower, the last
/// one otherwise), after it if that one ranks no higher (always, with
/// MEMORY), else before it; past symbol assignments, and at the very end if
/// no output section with inputs follows.
fn orphan_position(
    cmds: &[Command],
    ranks: &[Option<u32>],
    first: usize,
    orphan: usize,
    memory: bool,
) -> usize {
    let sec = ranks[orphan].expect("an orphan has inputs");
    // -1 for a command that is no output section with inputs.
    let proximity = |rank: Option<u32>| rank.map_or(-1, |r| (r ^ sec).leading_zeros() as i32);
    let mut best = 0;
    let mut i = orphan;
    for (j, &rank) in ranks.iter().enumerate().take(orphan).skip(first) {
        let p = proximity(rank);
        if p > best || (p == best && rank.is_some_and(|r| r <= sec)) {
            best = p;
            i = j;
        }
    }
    if i == orphan {
        return orphan;
    }
    if ranks[i].unwrap() <= sec || memory {
        // After it, and after the sections that follow it as close.
        let after = i + 1;
        i = after;
        for (j, &rank) in ranks.iter().enumerate().take(orphan).skip(after) {
            if rank.is_none() {
                continue;
            }
            if proximity(rank) != best {
                break;
            }
            i = j + 1;
        }
    } else {
        // Before it, and before the commands without inputs before it.
        while i != first && ranks[i - 1].is_none() {
            i -= 1;
        }
    }
    if ranks[i..orphan].iter().all(Option::is_none) {
        return orphan;
    }
    while i != orphan && matches!(&cmds[i], Command::Assign(a) if a.symbol.is_some()) {
        i += 1;
    }
    i
}

pub fn build(inputs: &[Input], opts: &Options) -> Result<Layout, Vec<Error>> {
    // 1. Objects.
    let mut errors = Vec::new();
    let mut objects = Vec::new();
    for i in inputs {
        match object::parse(&i.name, &i.data) {
            Ok(o) => objects.push(o),
            Err(e) => errors.push(err(format!("{}: {e}", i.name))),
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    if objects.is_empty() {
        return Err(vec![err("no input files")]);
    }
    let (script_name, src) = match &opts.script {
        Some(s) => (s.name.as_str(), s.text.as_str()),
        None => (DEFAULT_SCRIPT_NAME, script::DEFAULT_SCRIPT),
    };
    // The class of the executable, as ld.lld picks it (it needs no option
    // either): the script's `OUTPUT_FORMAT` if it names one, else the first
    // object's. Every object must have it, or the link stops:
    // `b.o is incompatible with a.o`, or `a.o is incompatible with
    // elf32-littleriscv`.
    let declared = script::declared_format(src);
    let (class, against) = match declared {
        Some(format) => (format, script::format_name(format)),
        None => (objects[0].class, objects[0].name.as_str()),
    };
    let wrong = object::check_classes(class, against, &objects);

    // Script. Its numbers and arithmetic depend on the class. ld.lld reads
    // the script before it compares the classes, so a script's own errors
    // come first; but its numbers are 64-bit whatever the class. So when an
    // object is of the wrong class the script is read for ELF64, where no
    // value is refused for being past 4 GiB, and its errors are the ones
    // ld.lld gives before it refuses the objects.
    let read_for = if wrong.is_empty() { class } else { Class::Elf64 };
    let script = script::parse(src, read_for).map_err(|e| vec![err(format!("{script_name}:{e}"))])?;
    debug_assert_eq!(script.format, declared);
    if !wrong.is_empty() {
        return Err(wrong);
    }

    // 2. Globals.
    let mut globals: Vec<(String, Def)> = Vec::new();
    let mut global_index: HashMap<String, usize> = HashMap::new();
    for (oi, o) in objects.iter().enumerate() {
        for (si, s) in o.symbols.iter().enumerate().skip(1) {
            if s.is_local() || s.is_undefined() {
                continue;
            }
            if s.shndx != SHN_ABS && !o.sections[s.shndx as usize].is_alloc() {
                errors.push(err(format!(
                    "{}: global symbol '{}' is defined in section '{}', which is not loaded",
                    o.name, s.name, o.sections[s.shndx as usize].name
                )));
                continue;
            }
            match global_index.get(&s.name) {
                None => {
                    global_index.insert(s.name.clone(), globals.len());
                    globals.push((s.name.clone(), Def::Object { obj: oi, sym: si }));
                }
                Some(&gi) => {
                    let Def::Object { obj: fo, sym: fs } = globals[gi].1 else {
                        unreachable!("no script symbols yet")
                    };
                    let first_weak = objects[fo].symbols[fs].is_weak();
                    if s.is_weak() {
                        continue;
                    }
                    if first_weak {
                        globals[gi].1 = Def::Object { obj: oi, sym: si };
                    } else {
                        errors.push(err(format!(
                            "{}: duplicate symbol '{}' (first defined in {})",
                            o.name, s.name, objects[fo].name
                        )));
                    }
                }
            }
        }
    }

    // 3. Input sections → output sections.
    let mut assigned: Vec<Vec<bool>> = objects.iter().map(|o| vec![false; o.sections.len()]).collect();
    // rules[cmd][item] = inputs matched by that `*(...)`, in order.
    let mut rules: Vec<Vec<Vec<(usize, usize)>>> = Vec::new();
    for cmd in &script.commands {
        let mut per_item = Vec::new();
        if let Command::Output(o) = cmd {
            for item in &o.items {
                let mut matched = Vec::new();
                if let Item::Input(patterns) = item {
                    for (oi, obj) in objects.iter().enumerate() {
                        for (si, sec) in obj.sections.iter().enumerate() {
                            if !sec.is_alloc() || assigned[oi][si] {
                                continue;
                            }
                            if patterns.iter().any(|p| script::glob_match(p, &sec.name)) {
                                assigned[oi][si] = true;
                                matched.push((oi, si));
                            }
                        }
                    }
                }
                per_item.push(matched);
            }
        }
        rules.push(per_item);
    }
    // The empty sections no rule matches: orphans, placed as ld.lld places
    // them ([`place_orphans`]).
    let mut orphans = Vec::new();
    for (oi, obj) in objects.iter().enumerate() {
        for (si, sec) in obj.sections.iter().enumerate() {
            if !sec.is_alloc() || assigned[oi][si] {
                continue;
            }
            // An empty section is an orphan, unless a global lives in it.
            let defines_global = || {
                obj.symbols
                    .iter()
                    .skip(1)
                    .any(|s| !s.is_local() && s.shndx as usize == si && s.shndx < SHN_LORESERVE)
            };
            if sec.size == 0 && !defines_global() {
                orphans.push((oi, si));
            } else {
                errors.push(err(format!(
                    "{}: section '{}' is not placed by {}",
                    obj.name,
                    sec.name,
                    if opts.script.is_some() {
                        format!("linker script {script_name}")
                    } else {
                        "the default layout (use a linker script with -T)".to_string()
                    }
                )));
            }
        }
    }
    // The script's commands with the orphans in them, and what each one's
    // `*(...)` items matched.
    let (commands, rules) = place_orphans(&script, rules, &objects, &orphans);

    // 4. Addresses.
    let mut w = Walker {
        class,
        script_name,
        script: &script,
        objects: &objects,
        placement: objects.iter().map(|o| vec![None; o.sections.len()]).collect(),
        discarded: HashSet::new(),
        globals,
        global_index,
        sections: Vec::new(),
        current: None,
        script_sections: script
            .commands
            .iter()
            .filter_map(|c| match c {
                Command::Output(o) if !o.is_discard() => Some(o.name.as_str()),
                _ => None,
            })
            .collect(),
        errors: Vec::new(),
    };
    // The sections an `ADDR(...)` names somewhere in the script: kept even
    // without inputs (ld.lld's usedInExpression).
    let addr_named: HashSet<&str> = script.addr_names().into_iter().collect();
    let region = script.memory.as_ref();
    // The location counter starts at 0, also with MEMORY (as in ld.lld): only
    // output sections go at the region's next free address.
    let mut dot: u64 = 0;
    let mut cursor: Option<u64> = region.map(|r| r.origin);
    // Line of the last top-level `.` move since the previous output section.
    let mut dot_moved: Option<usize> = None;
    // Flags of the last output section that had input sections; an output
    // section without any inherits their SHF_ALLOC and SHF_WRITE (as in ld.lld).
    let mut prev_flags = SHF_ALLOC;
    // LMA - VMA (modulo 2^64): where sections are loaded relative to where
    // they run. 0 until a section has an `AT`; every section after it
    // without one keeps its difference (see "Load addresses" above).
    let mut lma_offset: u64 = 0;
    'commands: for (ci, cmd) in commands.iter().enumerate() {
        w.current = None;
        match cmd {
            Command::Assign(a) => {
                let before = dot;
                dot = w.assign(a, dot);
                if a.symbol.is_none() && dot != before {
                    dot_moved = Some(a.line);
                }
            }
            // What `/DISCARD/` matched goes nowhere: those sections have no
            // address, so a reference to them is an error (see
            // `Layout::reloc_symbol`).
            Command::Output(o) if o.is_discard() => {
                w.discarded.extend(rules[ci].iter().flatten().copied());
            }
            Command::Output(o) => {
                let all_inputs = rules[ci].iter().flatten();
                // ld.lld's rule: an output section is dropped only if no input
                // section matched it (not even an empty one) and it has no
                // assignments. Zero-size inputs still align it. Nor is one
                // dropped that an expression needs: one an `ADDR` names
                // (usedInExpression) or one whose `AT` or `ALIGN` uses a
                // symbol (expressionsUseSymbols). A dropped section
                // evaluates nothing, so its `AT` changes no load address.
                let has_assign = o.items.iter().any(|i| matches!(i, Item::Assign(_)));
                let needed = addr_named.contains(o.name.as_str())
                    || o.at.iter().chain(&o.align).any(Expr::uses_symbols);
                if all_inputs.clone().next().is_none() && !has_assign && !needed {
                    continue;
                }
                let mut align = all_inputs
                    .clone()
                    .map(|&(oi, si)| objects[oi].sections[si].align)
                    .max()
                    .unwrap_or(1);
                // `.name : ALIGN(n) { ... }`: at least n, like an input that
                // aligned (ld.lld's sh_addralign is the larger of the two).
                // At most 2^31 on both classes: ld.lld keeps a section's
                // alignment in 32 bits on RV64 too, and ignores a larger one.
                // (On ELF32 a negative i64 reads as a u64 far above 2^31.)
                if let Some(e) = &o.align {
                    match w.eval(e, dot) {
                        Ok(n) if n.is_power_of_two() && n <= 1 << 31 => align = align.max(n as u32),
                        Ok(n) => {
                            let shown = w.show(n);
                            w.script_err(
                                o.line,
                                format!(
                                    "ALIGN({shown}) of section {}: alignment must be a power of two",
                                    o.name
                                ),
                            )
                        }
                        Err(e) => w.script_err(o.line, e),
                    }
                }
                // A section, or an input in it, that would have to start past
                // 2^64 to be aligned (only ELF64 gets that high).
                let past_the_top = |at: u64, align: u32| {
                    format!(
                        "section {} exceeds available address space: {at:#x} aligned to {align:#x} is past 2^64",
                        o.name
                    )
                };
                let base = cursor.unwrap_or(dot);
                let Some(start) = align_up(base, align as u64) else {
                    w.script_err(o.line, past_the_top(base, align));
                    continue;
                };
                // From here on `ADDR` of this section is `start`.
                w.current = Some((&o.name, start));
                // `AT(lma)`, with `.` at the section's aligned start (ld.lld:
                // `AT(. - OFFSET)` is LMA = VMA - OFFSET). It sets the offset
                // the sections after this one keep.
                let mut at = false;
                if let Some(e) = &o.at {
                    match w.eval(e, start) {
                        Ok(lma) => {
                            lma_offset = lma.wrapping_sub(start);
                            at = true;
                        }
                        Err(e) => w.script_err(o.line, e),
                    }
                }
                let lma = start.wrapping_add(lma_offset);
                let mut d = start;
                let mut inputs = Vec::new();
                let mut flags = SHF_ALLOC;
                let mut nobits = true;
                for (ii, item) in o.items.iter().enumerate() {
                    match item {
                        Item::Input(_) => {
                            for &(oi, si) in &rules[ci][ii] {
                                let sec = &objects[oi].sections[si];
                                let Some(at) = align_up(d, sec.align as u64) else {
                                    w.script_err(o.line, past_the_top(d, sec.align));
                                    continue 'commands;
                                };
                                d = at;
                                w.placement[oi][si] = Some(Placed { addr: d, out: None });
                                inputs.push(InputRef {
                                    object: oi,
                                    section: si,
                                    addr: d,
                                });
                                // Wrapping, as ld.lld's location counter: an
                                // ELF64 section that runs past 2^64 is
                                // reported once its size is known, below.
                                // ELF32's `d` stays far below 2^64.
                                d = d.wrapping_add(sec.size);
                                flags |= sec.flags & (SHF_WRITE | SHF_EXECINSTR);
                                nobits &= sec.is_nobits();
                            }
                        }
                        Item::Assign(a) => d = w.assign(a, d),
                    }
                }
                let size = d.wrapping_sub(start);
                match class {
                    // ELF32 (RV32, until #159): `d` may be exactly 4 GiB (a
                    // section ending at the top of the address space), but
                    // the size must still fit in 32 bits.
                    Class::Elf32 => {
                        if size > u64::from(u32::MAX) || d > 1 << 32 {
                            w.script_err(
                                o.line,
                                format!("section {} extends past the 4 GiB address space", o.name),
                            );
                            continue;
                        }
                    }
                    // ELF64: the section must end below 2^64, even exactly at
                    // it is too far (ld.lld's checkSections).
                    Class::Elf64 => {
                        if start.checked_add(size).is_none() {
                            w.script_err(
                                o.line,
                                format!(
                                    "section {} at {start:#x} of size {size:#x} exceeds available address space",
                                    o.name
                                ),
                            );
                            continue;
                        }
                    }
                }
                // The load address range must fit as the address range does.
                // The section is still placed: its addresses are right, and
                // the sections after it go where they should.
                if !load_range_fits(class, lma, size) {
                    let space = match class {
                        Class::Elf32 => "the 4 GiB",
                        Class::Elf64 => "the 64-bit",
                    };
                    w.script_err(
                        o.line,
                        format!(
                            "section {} load address range extends past {space} address space",
                            o.name
                        ),
                    );
                }
                if inputs.is_empty() {
                    // Only assignments (`. += n`, symbols): like ld.lld, a
                    // PROGBITS section with the previous section's flags, but
                    // only SHF_ALLOC and SHF_WRITE of them (ld.lld's
                    // adjustOutputSections leaves SHF_EXECINSTR out, so that
                    // such a section after the code is not executable: it
                    // goes with the data that follows it).
                    flags = prev_flags & (SHF_ALLOC | SHF_WRITE);
                    nobits = false;
                } else {
                    prev_flags = flags;
                }
                // NOLOAD: memory only, whatever the inputs hold (as in ld.lld,
                // which gives the section SHT_NOBITS).
                if o.noload {
                    nobits = true;
                }
                if let (Some(r), Some(c)) = (region, cursor) {
                    if let (Some(line), true) = (dot_moved, dot != c) {
                        w.script_err(
                            line,
                            format!(
                                "'.' is moved before output section {}, which is placed at the next free \
                                 address of MEMORY region {} instead; move the assignment inside the section",
                                o.name, r.name
                            ),
                        );
                    }
                    // script::parse made sure this does not overflow.
                    let end = r.origin + r.length;
                    if d > end {
                        w.script_err(
                            o.line,
                            format!(
                                "section {} overflows MEMORY region {} by {} bytes",
                                o.name,
                                r.name,
                                d - end
                            ),
                        );
                    }
                    cursor = Some(d);
                }
                let out = w.sections.len();
                for i in &inputs {
                    w.placement[i.object][i.section].as_mut().unwrap().out = Some(out);
                }
                w.sections.push(OutputSection {
                    name: o.name.clone(),
                    addr: start,
                    size,
                    lma,
                    at,
                    align,
                    flags,
                    nobits,
                    inputs,
                });
                dot = d;
                dot_moved = None;
            }
        }
    }
    errors.append(&mut w.errors);
    errors.extend(load_overlaps(class, &w.sections));

    // Debug information, after everything else.
    let mut debug: Vec<DebugSection> = Vec::new();
    let mut debug_offsets = HashMap::new();
    for (oi, obj) in objects.iter().enumerate() {
        for (si, sec) in obj.sections.iter().enumerate() {
            if !is_debug(sec) {
                continue;
            }
            let merge = SHF_MERGE | SHF_STRINGS;
            let out = match debug.iter_mut().find(|d| d.name == sec.name) {
                Some(d) => d,
                None => {
                    debug.push(DebugSection {
                        name: sec.name.clone(),
                        size: 0,
                        align: 1,
                        flags: sec.flags & merge,
                        entsize: sec.entsize,
                        inputs: Vec::new(),
                    });
                    debug.last_mut().unwrap()
                }
            };
            if sec.flags & merge != out.flags || sec.entsize != out.entsize {
                (out.flags, out.entsize) = (0, 0);
            }
            // Debug sections hold file contents: far below 2^64.
            let at = align_up(out.size, sec.align as u64).expect("debug sections fit in 64 bits");
            out.align = out.align.max(sec.align);
            out.inputs.push(InputRef {
                object: oi,
                section: si,
                addr: at,
            });
            debug_offsets.insert((oi, si), at);
            out.size = at + sec.size;
        }
    }

    let mut layout = Layout {
        class,
        objects: Vec::new(),
        sections: w.sections,
        debug,
        globals: w.globals,
        entry: 0,
        entry_symbol: None,
        warnings: Vec::new(),
        memory: script.memory.is_some(),
        riscv_attributes: riscv_attributes_kept(&objects, &script),
        placement: w.placement,
        discarded: w.discarded,
        debug_offsets,
        global_index: w.global_index,
    };
    layout.objects = objects;
    let merge = merge_warnings(&layout);
    layout.warnings.extend(merge);

    // 5. Undefined references.
    for o in &layout.objects {
        let mut seen = HashSet::new();
        for s in o.symbols.iter().skip(1) {
            if !s.is_local()
                && s.is_undefined()
                && !s.is_weak()
                && !layout.global_index.contains_key(&s.name)
                && seen.insert(&s.name)
            {
                errors.push(err(format!("{}: undefined reference to '{}'", o.name, s.name)));
            }
        }
    }

    // 6. Entry point: -e, else ENTRY(...), else _start.
    let (entry, explicit) = match (&opts.entry, &script.entry) {
        (Some(e), _) => (e.clone(), true),
        (None, Some(e)) => (e.clone(), false),
        (None, None) => ("_start".to_string(), false),
    };
    if let Some(n) = script::parse_number(&entry, class) {
        // ELF32 (RV32, until #159): e_entry is 32 bits (ld.lld would truncate
        // it). ELF64 takes any address, `-e 0xffffffff80000000` too.
        if class == Class::Elf32 && u32::try_from(n).is_err() {
            errors.push(err(format!("entry address {entry} does not fit in 32 bits")));
        } else {
            layout.entry = n;
        }
    } else if let Some(v) = layout.symbol(&entry) {
        layout.entry = v;
        layout.entry_symbol = Some(entry);
    } else if explicit {
        errors.push(err(format!("entry symbol '{entry}' is not defined")));
    } else {
        let fallback = layout
            .sections
            .iter()
            .find(|s| s.flags & SHF_EXECINSTR != 0)
            .or(layout.sections.first())
            .map_or(0, |s| s.addr);
        layout.entry = fallback;
        // An address as wide as the class's: 8 or 16 hex digits.
        let shown = match class {
            Class::Elf32 => format!("{fallback:#010x}"),
            Class::Elf64 => format!("{fallback:#018x}"),
        };
        layout.warnings.push(format!(
            "cannot find entry symbol '{entry}'; defaulting to {shown}"
        ));
    }

    // 7. What the program headers would be: a placement without errors can
    // still give the headers a load address below 0 (see write::check).
    if errors.is_empty() {
        errors.extend(crate::write::check(&layout).into_iter().map(err));
    }

    if errors.is_empty() {
        Ok(layout)
    } else {
        Err(errors)
    }
}
