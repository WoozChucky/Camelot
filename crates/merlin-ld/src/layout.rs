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
//!    non-empty or defines a global.
//! 4. Walk `SECTIONS` with the location counter, placing each input section at
//!    its alignment and evaluating script assignments as they come. As in
//!    ld.lld, an output section is dropped only when no input section (not
//!    even an empty one) matched it and it has no assignments.
//! 5. Report undefined globals (`a.o: undefined reference to 'foo'`).
//! 6. Pick the entry point.
//!
//! Debug information is kept apart from all this: every `.debug_*` input
//! section (not loaded, so no rule places it) goes into the output section of
//! its name, the objects' in command-line order, as ld.lld does without a rule
//! for them. Those sections have no address; a symbol in one is its offset
//! there (see [`Layout::debug`]).

use crate::object::{
    self, Object, SHF_ALLOC, SHF_EXECINSTR, SHF_MERGE, SHF_STRINGS, SHF_WRITE, SHN_ABS, SHN_LORESERVE,
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
    pub addr: u32,
    pub size: u32,
    /// Largest alignment of its input sections.
    pub align: u32,
    /// `SHF_ALLOC` plus the union of the inputs' `SHF_WRITE` / `SHF_EXECINSTR`;
    /// without inputs, the flags of the previous section that had some.
    pub flags: u32,
    /// Occupies no file space (`.bss`): every input is `SHT_NOBITS`
    /// (and there is at least one), or the script says `(NOLOAD)`.
    pub nobits: bool,
    /// Input sections in address order.
    pub inputs: Vec<InputRef>,
}

/// An input section placed in an output section.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputRef {
    /// Index into [`Layout::objects`].
    pub object: usize,
    /// ELF section index within that object.
    pub section: usize,
    pub addr: u32,
}

/// Where a global symbol's definition lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Def {
    /// `objects[obj].symbols[sym]`.
    Object { obj: usize, sym: usize },
    /// Assigned by the linker script.
    Script(u32),
}

/// Where an input section ended up.
#[derive(Debug, Clone, Copy)]
struct Placed {
    addr: u32,
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
    pub size: u32,
    pub align: u32,
    /// `SHF_MERGE | SHF_STRINGS` when every input has them (string tables).
    pub flags: u32,
    pub entsize: u32,
    pub inputs: Vec<InputRef>,
}

/// Whether an input section is debug information merlin-ld keeps.
pub fn is_debug(sec: &object::Section) -> bool {
    !sec.is_alloc() && sec.ty == object::SHT_PROGBITS && sec.name.starts_with(".debug_")
}

#[derive(Debug)]
pub struct Layout {
    pub objects: Vec<Object>,
    pub sections: Vec<OutputSection>,
    /// The debug sections, by name in the order the inputs first have them.
    pub debug: Vec<DebugSection>,
    /// Defined globals in first-definition order. Script assignments override
    /// object definitions of the same name.
    pub globals: Vec<(String, Def)>,
    pub entry: u32,
    /// The entry symbol's name, unless the entry is a number or a fallback.
    pub entry_symbol: Option<String>,
    pub warnings: Vec<String>,
    /// `[object][section]`.
    placement: Vec<Vec<Option<Placed>>>,
    /// The input sections `/DISCARD/` matched, as `(object, section)`.
    discarded: HashSet<(usize, usize)>,
    /// `(object, section)` of a debug input section: its offset in its
    /// output section.
    debug_offsets: HashMap<(usize, usize), u32>,
    global_index: HashMap<String, usize>,
}

fn err(msg: impl Into<String>) -> Error {
    Error(msg.into())
}

fn align_up(v: u64, a: u64) -> u64 {
    v.div_ceil(a) * a
}

impl Layout {
    /// The final address of a global symbol (object- or script-defined).
    pub fn symbol(&self, name: &str) -> Option<u32> {
        let &i = self.global_index.get(name)?;
        self.def_value(self.globals[i].1)
    }

    pub fn section(&self, name: &str) -> Option<&OutputSection> {
        self.sections.iter().find(|s| s.name == name)
    }

    pub fn def_value(&self, d: Def) -> Option<u32> {
        match d {
            Def::Script(v) => Some(v),
            Def::Object { obj, sym } => self.object_symbol(obj, sym),
        }
    }

    /// The address of a symbol defined in `obj`; `None` if it is undefined
    /// there or its section is not loaded.
    pub fn object_symbol(&self, obj: usize, sym: usize) -> Option<u32> {
        let s = &self.objects[obj].symbols[sym];
        match s.shndx {
            SHN_UNDEF => None,
            SHN_ABS => Some(s.value),
            sec => self
                .section_base(obj, sec as usize)
                .map(|a| a.wrapping_add(s.value)),
        }
    }

    /// Where an input section starts: its address if it is loaded, its
    /// offset in its output section if it is debug information.
    fn section_base(&self, obj: usize, sec: usize) -> Option<u32> {
        self.input_addr(obj, sec)
            .or_else(|| self.debug_offsets.get(&(obj, sec)).copied())
    }

    /// Whether `sym` of `obj` is in a section that is neither loaded nor
    /// debug information, or that `/DISCARD/` threw away. Debug information
    /// may point at such places (the code of a discarded function); there
    /// the linker writes 0, as ld.lld does.
    pub fn in_dropped_section(&self, obj: usize, sym: usize) -> bool {
        let s = &self.objects[obj].symbols[sym];
        s.shndx != SHN_UNDEF && s.shndx < SHN_LORESERVE && self.section_base(obj, s.shndx as usize).is_none()
    }

    /// Address of an input section, if it was placed.
    pub fn input_addr(&self, obj: usize, sec: usize) -> Option<u32> {
        self.placement.get(obj)?.get(sec)?.map(|p| p.addr)
    }

    /// Index into [`Layout::sections`] of the output section holding an input
    /// section; `None` if it isn't loaded or its output section was discarded.
    pub fn input_output_section(&self, obj: usize, sec: usize) -> Option<usize> {
        self.placement.get(obj)?.get(sec)?.and_then(|p| p.out)
    }

    /// S for a relocation in `obj` against its symbol `sym`.
    pub fn reloc_symbol(&self, obj: usize, sym: usize) -> Result<u32, String> {
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
    script_name: &'a str,
    script: &'a Script,
    objects: &'a [Object],
    placement: Vec<Vec<Option<Placed>>>,
    discarded: HashSet<(usize, usize)>,
    globals: Vec<(String, Def)>,
    global_index: HashMap<String, usize>,
    sections: Vec<OutputSection>,
    errors: Vec<Error>,
}

impl Walker<'_> {
    fn script_err(&mut self, line: usize, msg: impl Into<String>) {
        self.errors
            .push(err(format!("{}:{line}: {}", self.script_name, msg.into())));
    }

    fn eval(&self, e: &Expr, dot: u64) -> Result<i64, String> {
        Ok(match e {
            Expr::Num(n) => *n,
            Expr::Dot => dot as i64,
            Expr::Symbol(name) => {
                let value =
                    self.global_index
                        .get(name)
                        .and_then(|&i| match self.globals[i].1 {
                            Def::Script(v) => Some(v),
                            Def::Object { obj, sym } => {
                                let s = &self.objects[obj].symbols[sym];
                                match s.shndx {
                                    SHN_ABS => Some(s.value),
                                    sec => self.placement[obj][sec as usize]
                                        .map(|p| p.addr.wrapping_add(s.value)),
                                }
                            }
                        });
                match value {
                    Some(v) => v as i64,
                    None if self.global_index.contains_key(name) => {
                        return Err(format!("symbol '{name}' is used before its section is placed"))
                    }
                    None => return Err(format!("undefined symbol '{name}' in expression")),
                }
            }
            Expr::Align(n) => {
                let n = self.eval(n, dot)?;
                if n <= 0 {
                    return Err(format!("ALIGN({n}): alignment must be positive"));
                }
                align_up(dot, n as u64) as i64
            }
            Expr::Origin(r) | Expr::Length(r) => match &self.script.memory {
                Some(m) if &m.name == r => match e {
                    Expr::Origin(_) => m.origin as i64,
                    _ => m.length as i64,
                },
                _ => return Err(format!("unknown memory region '{r}'")),
            },
            Expr::Add(a, b) => self
                .eval(a, dot)?
                .checked_add(self.eval(b, dot)?)
                .ok_or("arithmetic overflow")?,
            Expr::Sub(a, b) => self
                .eval(a, dot)?
                .checked_sub(self.eval(b, dot)?)
                .ok_or("arithmetic overflow")?,
        })
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
                let new = if a.add {
                    (dot as i64).checked_add(v)
                } else {
                    Some(v)
                };
                let Some(new) = new else {
                    self.script_err(a.line, "arithmetic overflow");
                    return dot;
                };
                if new < dot as i64 {
                    self.script_err(
                        a.line,
                        format!("cannot move the location counter backward (from {dot:#x} to {new:#x})"),
                    );
                    return dot;
                }
                if new > 1 << 32 {
                    self.script_err(
                        a.line,
                        format!("location counter {new:#x} is past the 4 GiB address space"),
                    );
                    return dot;
                }
                new as u64
            }
            Some(name) => {
                let Ok(v) = u32::try_from(v) else {
                    self.script_err(
                        a.line,
                        format!("value {v:#x} of '{name}' does not fit in 32 bits"),
                    );
                    return dot;
                };
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
        let mut seen: HashSet<(u32, u32, u32, &[u8])> = HashSet::new();
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

    // Script.
    let (script_name, src) = match &opts.script {
        Some(s) => (s.name.as_str(), s.text.as_str()),
        None => (DEFAULT_SCRIPT_NAME, script::DEFAULT_SCRIPT),
    };
    let script = script::parse(src).map_err(|e| vec![err(format!("{script_name}:{e}"))])?;

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
    for (oi, obj) in objects.iter().enumerate() {
        for (si, sec) in obj.sections.iter().enumerate() {
            // An empty section can be left out, unless a global lives in it
            // (that symbol would otherwise have no address).
            let defines_global = || {
                obj.symbols
                    .iter()
                    .skip(1)
                    .any(|s| !s.is_local() && s.shndx as usize == si && s.shndx < SHN_LORESERVE)
            };
            if sec.is_alloc() && !assigned[oi][si] && (sec.size > 0 || defines_global()) {
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

    // 4. Addresses.
    let mut w = Walker {
        script_name,
        script: &script,
        objects: &objects,
        placement: objects.iter().map(|o| vec![None; o.sections.len()]).collect(),
        discarded: HashSet::new(),
        globals,
        global_index,
        sections: Vec::new(),
        errors: Vec::new(),
    };
    let region = script.memory.as_ref();
    // The location counter starts at 0, also with MEMORY (as in ld.lld): only
    // output sections go at the region's next free address.
    let mut dot: u64 = 0;
    let mut cursor: Option<u64> = region.map(|r| r.origin as u64);
    // Line of the last top-level `.` move since the previous output section.
    let mut dot_moved: Option<usize> = None;
    // Flags of the last output section that had input sections; an output
    // section without any inherits them (as in ld.lld).
    let mut prev_flags = SHF_ALLOC;
    for (ci, cmd) in script.commands.iter().enumerate() {
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
                // assignments. Zero-size inputs still align it.
                let has_assign = o.items.iter().any(|i| matches!(i, Item::Assign(_)));
                if all_inputs.clone().next().is_none() && !has_assign {
                    continue;
                }
                let mut align = all_inputs
                    .clone()
                    .map(|&(oi, si)| objects[oi].sections[si].align)
                    .max()
                    .unwrap_or(1);
                // `.name : ALIGN(n) { ... }`: at least n, like an input that
                // aligned (ld.lld's sh_addralign is the larger of the two).
                if let Some(e) = &o.align {
                    match w.eval(e, dot) {
                        Ok(n) if n > 0 && (n as u64).is_power_of_two() && n <= 1 << 31 => {
                            align = align.max(n as u32)
                        }
                        Ok(n) => w.script_err(
                            o.line,
                            format!(
                                "ALIGN({n}) of section {}: alignment must be a power of two",
                                o.name
                            ),
                        ),
                        Err(e) => w.script_err(o.line, e),
                    }
                }
                let base = cursor.unwrap_or(dot);
                let start = align_up(base, align as u64);
                let mut d = start;
                let mut inputs = Vec::new();
                let mut flags = SHF_ALLOC;
                let mut nobits = true;
                for (ii, item) in o.items.iter().enumerate() {
                    match item {
                        Item::Input(_) => {
                            for &(oi, si) in &rules[ci][ii] {
                                let sec = &objects[oi].sections[si];
                                d = align_up(d, sec.align as u64);
                                w.placement[oi][si] = Some(Placed {
                                    addr: d as u32,
                                    out: None,
                                });
                                inputs.push(InputRef {
                                    object: oi,
                                    section: si,
                                    addr: d as u32,
                                });
                                d += sec.size as u64;
                                flags |= sec.flags & (SHF_WRITE | SHF_EXECINSTR);
                                nobits &= sec.is_nobits();
                            }
                        }
                        Item::Assign(a) => d = w.assign(a, d),
                    }
                }
                // `d` may be exactly 4 GiB (a section ending at the top of the
                // address space), but the size must still fit in 32 bits.
                let size = match u32::try_from(d - start) {
                    Ok(size) if d <= 1 << 32 => size,
                    _ => {
                        w.script_err(
                            o.line,
                            format!("section {} extends past the 4 GiB address space", o.name),
                        );
                        continue;
                    }
                };
                if inputs.is_empty() {
                    // Only assignments (`. += n`, symbols): like ld.lld, a
                    // PROGBITS section with the previous section's flags.
                    flags = prev_flags;
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
                    let end = r.origin as u64 + r.length as u64;
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
                    addr: start as u32,
                    size,
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
            let at = align_up(out.size as u64, sec.align as u64) as u32;
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
        objects: Vec::new(),
        sections: w.sections,
        debug,
        globals: w.globals,
        entry: 0,
        entry_symbol: None,
        warnings: Vec::new(),
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
    if let Some(n) = script::parse_number(&entry) {
        match u32::try_from(n) {
            Ok(v) => layout.entry = v,
            Err(_) => errors.push(err(format!("entry address {entry} does not fit in 32 bits"))),
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
        layout.warnings.push(format!(
            "cannot find entry symbol '{entry}'; defaulting to {fallback:#010x}"
        ));
    }

    if errors.is_empty() {
        Ok(layout)
    } else {
        Err(errors)
    }
}
