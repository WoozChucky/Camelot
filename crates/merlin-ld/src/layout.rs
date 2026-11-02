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
//!    An allocated, non-empty input section no rule matches is an error.
//! 4. Walk `SECTIONS` with the location counter, placing each input section at
//!    its alignment and evaluating script assignments as they come.
//! 5. Report undefined globals (`a.o: undefined reference to 'foo'`).
//! 6. Pick the entry point.

use crate::object::{self, Object, SHF_ALLOC, SHF_EXECINSTR, SHF_WRITE, SHN_ABS, SHN_UNDEF, STT_SECTION};
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
    /// `SHF_ALLOC` plus the union of the inputs' `SHF_WRITE` / `SHF_EXECINSTR`.
    pub flags: u32,
    /// Occupies no file space (`.bss`): every input is `SHT_NOBITS`.
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
    /// discarded for being empty.
    out: Option<usize>,
}

#[derive(Debug)]
pub struct Layout {
    pub objects: Vec<Object>,
    pub sections: Vec<OutputSection>,
    /// Defined globals in first-definition order. Script assignments override
    /// object definitions of the same name.
    pub globals: Vec<(String, Def)>,
    pub entry: u32,
    /// The entry symbol's name, unless the entry is a number or a fallback.
    pub entry_symbol: Option<String>,
    pub warnings: Vec<String>,
    /// `[object][section]`.
    placement: Vec<Vec<Option<Placed>>>,
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
                .input_addr(obj, sec as usize)
                .map(|a| a.wrapping_add(s.value)),
        }
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
                return self.input_addr(obj, s.shndx as usize).ok_or_else(|| {
                    format!(
                        "refers to section '{}', which is not loaded",
                        o.symbol_display_name(sym as u32)
                    )
                });
            }
            return self
                .object_symbol(obj, sym)
                .ok_or_else(|| format!("refers to '{}', whose section is not loaded", s.name));
        }
        match self.global_index.get(&s.name) {
            Some(&i) => self
                .def_value(self.globals[i].1)
                .ok_or_else(|| format!("refers to '{}', whose section is not loaded", s.name)),
            None if s.is_weak() => Ok(0),
            None => Err(format!("undefined reference to '{}'", s.name)),
        }
    }
}

/// Mutable state while walking `SECTIONS`.
struct Walker<'a> {
    script_name: &'a str,
    script: &'a Script,
    objects: &'a [Object],
    placement: Vec<Vec<Option<Placed>>>,
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
                let new = if a.add { dot as i64 + v } else { v };
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
            if sec.is_alloc() && !assigned[oi][si] && sec.size > 0 {
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
        globals,
        global_index,
        sections: Vec::new(),
        errors: Vec::new(),
    };
    let region = script.memory.as_ref();
    let mut dot: u64 = region.map_or(0, |r| r.origin as u64);
    // With a MEMORY region, sections go at the region's next free address.
    let mut cursor: Option<u64> = region.map(|r| r.origin as u64);
    let mut last_dot_move: Option<usize> = None;
    for (ci, cmd) in script.commands.iter().enumerate() {
        match cmd {
            Command::Assign(a) => {
                let before = dot;
                dot = w.assign(a, dot);
                if a.symbol.is_none() && dot != before {
                    last_dot_move = Some(a.line);
                }
            }
            Command::Output(o) => {
                let all_inputs = rules[ci].iter().flatten();
                let align = all_inputs
                    .clone()
                    .map(|&(oi, si)| objects[oi].sections[si].align)
                    .max()
                    .unwrap_or(1);
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
                if d > 1 << 32 {
                    w.script_err(
                        o.line,
                        format!("section {} extends past the 4 GiB address space", o.name),
                    );
                    continue;
                }
                let size = (d - start) as u32;
                if size == 0 {
                    // Discarded, like ld.lld does with empty output sections.
                    continue;
                }
                if inputs.is_empty() {
                    // Only `. += n`: reserve space without file contents.
                    flags |= SHF_WRITE;
                }
                if let (Some(r), Some(c)) = (region, cursor) {
                    if dot != c {
                        let line = last_dot_move.unwrap_or(o.line);
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
            }
        }
    }
    errors.append(&mut w.errors);

    let mut layout = Layout {
        objects: Vec::new(),
        sections: w.sections,
        globals: w.globals,
        entry: 0,
        entry_symbol: None,
        warnings: Vec::new(),
        placement: w.placement,
        global_index: w.global_index,
    };
    layout.objects = objects;

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
