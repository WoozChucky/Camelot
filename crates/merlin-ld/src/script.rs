//! The linker-script subset merlin-ld understands (spec §10, "Linker scripts").
//!
//! ```text
//! OUTPUT_ARCH(riscv)                       optional; must say riscv
//! OUTPUT_FORMAT("elf32-littleriscv")       optional; elf32-littleriscv or
//!                                          elf64-littleriscv, the objects' class
//! ENTRY(symbol)
//! MEMORY { RAM (rwx) : ORIGIN = expr, LENGTH = expr }      at most one region
//! SECTIONS
//! {
//!     . = expr;   . += expr;   symbol = expr;
//!     .name : { *(pattern pattern ...) ... . = ALIGN(n); . += n; symbol = expr; } > RAM
//!     .name (NOLOAD) : ALIGN(n) { ... } > RAM    both optional
//!     /DISCARD/ : { *(pattern ...) }
//! }
//! ```
//!
//! Expressions: numbers (`16`, `0x80000000`, `64K`, `128M`), `.`, symbols,
//! `ALIGN(n)`, `ORIGIN(region)`, `LENGTH(region)`, parentheses, `+` and `-`.
//! Comments are `/* ... */`. Anything else is an error naming what was found.
//!
//! A script is read for the class of the objects it links ([`parse`] takes
//! it). With ELF64 (RV64) objects, values are 64-bit addresses: numbers go up
//! to 2^64 - 1 and `+` and `-` wrap modulo 2^64, as in ld.lld. With ELF32
//! (RV32, until #159) they are exact `i64`s, as merlin-ld has always read
//! them: numbers up to 2^63 - 1, an overflow is an error, and the 4 GiB
//! address space is checked where a value is used (MEMORY here, the rest in
//! [`crate::layout`]).

use elf::Class;
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Script {
    pub entry: Option<String>,
    /// `OUTPUT_FORMAT(...)`'s first argument, which ld.lld goes by: the class
    /// the objects must have.
    pub format: Option<Class>,
    pub memory: Option<Region>,
    /// The contents of `SECTIONS`, in order.
    pub commands: Vec<Command>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Region {
    pub name: String,
    /// `origin + length` does not overflow: [`parse`] checks it (within
    /// 4 GiB on ELF32, within 2^64 on ELF64).
    pub origin: u64,
    pub length: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Assign(Assign),
    Output(OutputDesc),
}

/// `. = e`, `. += e` or `sym = e`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Assign {
    /// `None` is the location counter `.`.
    pub symbol: Option<String>,
    /// `+=` rather than `=`.
    pub add: bool,
    pub expr: Expr,
    pub line: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputDesc {
    /// `/DISCARD/` for the sections to throw away (see [`OutputDesc::is_discard`]).
    pub name: String,
    /// `(NOLOAD)`: the section takes no file space (`SHT_NOBITS`), whatever
    /// its inputs are.
    pub noload: bool,
    /// `ALIGN(n)` after the colon: the section starts at least this aligned.
    pub align: Option<Expr>,
    pub items: Vec<Item>,
    /// `> REGION`.
    pub region: Option<String>,
    pub line: usize,
}

impl OutputDesc {
    /// The name of the output section whose inputs are thrown away.
    pub const DISCARD: &'static str = "/DISCARD/";

    /// Whether this is `/DISCARD/`: the input sections it matches are not
    /// placed anywhere.
    pub fn is_discard(&self) -> bool {
        self.name == Self::DISCARD
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    /// `*(pattern ...)`: input sections from every file whose name matches a pattern.
    Input(Vec<String>),
    Assign(Assign),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expr {
    /// A number, as [`parse_number`] reads it for the script's class.
    Num(u64),
    Dot,
    Symbol(String),
    Align(Box<Expr>),
    Origin(String),
    Length(String),
    Add(Box<Expr>, Box<Expr>),
    Sub(Box<Expr>, Box<Expr>),
}

impl Expr {
    /// Whether the value depends on the location counter.
    pub fn uses_dot(&self) -> bool {
        match self {
            Expr::Dot | Expr::Align(_) => true,
            Expr::Num(_) | Expr::Symbol(_) | Expr::Origin(_) | Expr::Length(_) => false,
            Expr::Add(a, b) | Expr::Sub(a, b) => a.uses_dot() || b.uses_dot(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptError {
    pub line: usize,
    pub msg: String,
}

impl fmt::Display for ScriptError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.line, self.msg)
    }
}

fn error<T>(line: usize, msg: impl Into<String>) -> Result<T, ScriptError> {
    Err(ScriptError {
        line,
        msg: msg.into(),
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Tok {
    /// Names, numbers, section names and glob patterns (`.text*`, `*`).
    Word(String),
    Str(String),
    /// `{ } ( ) : ; , = > + -` and `+=`.
    Punct(&'static str),
}

impl fmt::Display for Tok {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Tok::Word(w) => write!(f, "'{w}'"),
            Tok::Str(s) => write!(f, "\"{s}\""),
            Tok::Punct(p) => write!(f, "'{p}'"),
        }
    }
}

fn is_word_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '$' | '*' | '?')
}

fn lex(src: &str) -> Result<Vec<(Tok, usize)>, ScriptError> {
    let mut toks = Vec::new();
    let mut line = 1;
    let mut chars = src.chars().peekable();
    while let Some(&c) = chars.peek() {
        match c {
            '\n' => {
                line += 1;
                chars.next();
            }
            c if c.is_whitespace() => {
                chars.next();
            }
            '/' => {
                chars.next();
                if chars.clone().take(8).eq("DISCARD/".chars()) {
                    chars.nth(7);
                    toks.push((Tok::Word(OutputDesc::DISCARD.into()), line));
                    continue;
                }
                if chars.peek() != Some(&'*') {
                    return error(line, "unexpected '/' (only /* */ comments are supported)");
                }
                chars.next();
                let start = line;
                let mut prev = ' ';
                loop {
                    match chars.next() {
                        Some('/') if prev == '*' => break,
                        Some(c) => {
                            if c == '\n' {
                                line += 1;
                            }
                            prev = c;
                        }
                        None => return error(start, "unterminated comment"),
                    }
                }
            }
            '"' => {
                chars.next();
                let mut s = String::new();
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\n') | None => return error(line, "unterminated string"),
                        Some(c) => s.push(c),
                    }
                }
                toks.push((Tok::Str(s), line));
            }
            '+' => {
                chars.next();
                if chars.peek() == Some(&'=') {
                    chars.next();
                    toks.push((Tok::Punct("+="), line));
                } else {
                    toks.push((Tok::Punct("+"), line));
                }
            }
            c if is_word_char(c) => {
                let mut w = String::new();
                while let Some(&c) = chars.peek() {
                    if !is_word_char(c) {
                        break;
                    }
                    w.push(c);
                    chars.next();
                }
                toks.push((Tok::Word(w), line));
            }
            _ => {
                let p = match c {
                    '{' => "{",
                    '}' => "}",
                    '(' => "(",
                    ')' => ")",
                    ':' => ":",
                    ';' => ";",
                    ',' => ",",
                    '=' => "=",
                    '>' => ">",
                    '-' => "-",
                    _ => return error(line, format!("unexpected character '{c}'")),
                };
                chars.next();
                toks.push((Tok::Punct(p), line));
            }
        }
    }
    Ok(toks)
}

/// The `OUTPUT_FORMAT` name of a class: `elf32-littleriscv` or `elf64-littleriscv`.
pub fn format_name(class: Class) -> &'static str {
    match class {
        Class::Elf32 => "elf32-littleriscv",
        Class::Elf64 => "elf64-littleriscv",
    }
}

/// The class an `OUTPUT_FORMAT` name stands for, if merlin-ld knows it.
fn format_class(name: &str) -> Option<Class> {
    [Class::Elf32, Class::Elf64]
        .into_iter()
        .find(|&c| format_name(c) == name)
}

/// The class `src`'s `OUTPUT_FORMAT` names (its first argument, the one
/// ld.lld goes by), found without reading the rest of the script: `None`
/// if it has none, or one [`parse`] will refuse.
///
/// The executable's class is the script's when it names one, else the
/// first object's, and ld.lld compares every object (library members too)
/// with it. So the layout and [`crate::library`] need it before the script
/// is parsed: the layout to know which class to parse the script for.
pub fn declared_format(src: &str) -> Option<Class> {
    let toks = lex(src).ok()?;
    // Commands are the words at the top level, outside every `( )` and
    // `{ }`; like the parser, take the first OUTPUT_FORMAT's first argument.
    let mut depth = 0usize;
    for (i, (t, _)) in toks.iter().enumerate() {
        match t {
            Tok::Punct("(" | "{") => depth += 1,
            Tok::Punct(")" | "}") => depth = depth.saturating_sub(1),
            Tok::Word(w) if depth == 0 && w == "OUTPUT_FORMAT" => {
                return match (toks.get(i + 1), toks.get(i + 2)) {
                    (Some((Tok::Punct("("), _)), Some((Tok::Word(f) | Tok::Str(f), _))) => format_class(f),
                    _ => None,
                };
            }
            _ => {}
        }
    }
    None
}

/// Parses a number: decimal or `0x` hex, with an optional `K` or `M` suffix
/// (ld.lld has no `G` either). ELF64 takes any `u64`; ELF32 (RV32, until
/// #159) only values up to `i64::MAX`, the type its scripts are evaluated in.
pub fn parse_number(w: &str, class: Class) -> Option<u64> {
    let (digits, mult) = match w.as_bytes().last()? {
        b'K' | b'k' => (&w[..w.len() - 1], 1024),
        b'M' | b'm' => (&w[..w.len() - 1], 1024 * 1024),
        _ => (w, 1),
    };
    let v = if let Some(h) = digits.strip_prefix("0x").or_else(|| digits.strip_prefix("0X")) {
        // Digits only: from_str_radix would take a sign too (`-e 0x+10`).
        if h.is_empty() || !h.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        u64::from_str_radix(h, 16).ok()?
    } else if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) {
        digits.parse::<u64>().ok()?
    } else {
        return None;
    };
    let v = v.checked_mul(mult)?;
    match class {
        Class::Elf32 => (v <= i64::MAX as u64).then_some(v),
        Class::Elf64 => Some(v),
    }
}

struct Parser {
    toks: Vec<(Tok, usize)>,
    pos: usize,
    class: Class,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos).map(|(t, _)| t)
    }
    fn line(&self) -> usize {
        match self.toks.get(self.pos) {
            Some((_, l)) => *l,
            None => self.toks.last().map(|(_, l)| *l).unwrap_or(1),
        }
    }
    fn next(&mut self) -> Result<Tok, ScriptError> {
        match self.toks.get(self.pos) {
            Some((t, _)) => {
                self.pos += 1;
                Ok(t.clone())
            }
            None => error(self.line(), "unexpected end of script"),
        }
    }
    fn is(&self, p: &str) -> bool {
        matches!(self.peek(), Some(Tok::Punct(q)) if *q == p)
    }
    fn eat(&mut self, p: &str) -> bool {
        if self.is(p) {
            self.pos += 1;
            true
        } else {
            false
        }
    }
    fn expect(&mut self, p: &str) -> Result<(), ScriptError> {
        let line = self.line();
        match self.next()? {
            Tok::Punct(q) if q == p => Ok(()),
            t => error(line, format!("expected '{p}', found {t}")),
        }
    }
    fn word(&mut self, what: &str) -> Result<String, ScriptError> {
        let line = self.line();
        match self.next()? {
            Tok::Word(w) => Ok(w),
            t => error(line, format!("expected {what}, found {t}")),
        }
    }
    /// A word or a quoted string, as in `OUTPUT_ARCH("riscv")`.
    fn word_or_str(&mut self, what: &str) -> Result<String, ScriptError> {
        let line = self.line();
        match self.next()? {
            Tok::Word(w) | Tok::Str(w) => Ok(w),
            t => error(line, format!("expected {what}, found {t}")),
        }
    }

    fn script(&mut self) -> Result<Script, ScriptError> {
        let mut script = Script {
            entry: None,
            format: None,
            memory: None,
            commands: Vec::new(),
        };
        let mut seen_sections = false;
        let mut seen_memory = false;
        while self.peek().is_some() {
            let line = self.line();
            let kw = self.word("a linker script command")?;
            match kw.as_str() {
                "ENTRY" => {
                    self.expect("(")?;
                    script.entry = Some(self.word("an entry symbol")?);
                    self.expect(")")?;
                }
                "OUTPUT_ARCH" => {
                    self.expect("(")?;
                    let arch = self.word_or_str("an architecture")?;
                    if !arch.eq_ignore_ascii_case("riscv") {
                        return error(line, format!("OUTPUT_ARCH({arch}): only riscv is supported"));
                    }
                    self.expect(")")?;
                }
                "OUTPUT_FORMAT" => {
                    self.expect("(")?;
                    // `OUTPUT_FORMAT(default, big, little)`: ld.lld goes by the
                    // first; merlin-ld also wants the others to be names it knows.
                    loop {
                        let fmt = self.word_or_str("an output format")?;
                        let Some(class) = format_class(&fmt) else {
                            return error(
                                line,
                                format!(
                                    "OUTPUT_FORMAT({fmt}): only elf32-littleriscv and elf64-littleriscv \
                                     are supported"
                                ),
                            );
                        };
                        script.format.get_or_insert(class);
                        if !self.eat(",") {
                            break;
                        }
                    }
                    self.expect(")")?;
                }
                "MEMORY" => {
                    if seen_memory {
                        return error(line, "only one MEMORY command is supported");
                    }
                    seen_memory = true;
                    script.memory = self.memory()?;
                }
                "SECTIONS" => {
                    if seen_sections {
                        return error(line, "only one SECTIONS command is supported");
                    }
                    seen_sections = true;
                    script.commands = self.sections()?;
                }
                _ => return error(line, format!("unsupported linker script command '{kw}'")),
            }
            self.eat(";");
        }
        Ok(script)
    }

    fn memory(&mut self) -> Result<Option<Region>, ScriptError> {
        self.expect("{")?;
        let mut region = None;
        while !self.eat("}") {
            let line = self.line();
            let name = self.word("a memory region name")?;
            if region.is_some() {
                return error(line, "only one MEMORY region is supported");
            }
            if self.eat("(") {
                let attrs = self.word("region attributes")?;
                if !attrs.chars().all(|c| "rwxaiRWXAI".contains(c)) {
                    return error(line, format!("unsupported region attributes '{attrs}'"));
                }
                self.expect(")")?;
            }
            self.expect(":")?;
            let origin = self.region_field(&["ORIGIN", "org", "o"])?;
            self.expect(",")?;
            let length = self.region_field(&["LENGTH", "len", "l"])?;
            match self.class {
                // ELF32 (RV32, until #159): the region is within 4 GiB, said
                // here with its line. The values are i64s (`o = 0 - 1` is -1).
                Class::Elf32 => {
                    let to_u32 = |v: u64, what: &str| {
                        let v = v as i64;
                        u32::try_from(v)
                            .or_else(|_| error(line, format!("MEMORY {what} {v:#x} does not fit in 32 bits")))
                    };
                    let (o, l) = (to_u32(origin, "ORIGIN")?, to_u32(length, "LENGTH")?);
                    if o as u64 + l as u64 > 1 << 32 {
                        return error(line, format!("MEMORY region {name} extends past 4 GiB"));
                    }
                }
                // ELF64: the region ends below 2^64. ld.lld computes its end
                // modulo 2^64, so one that reaches 2^64 ends at 0 and no
                // section fits in it; an error here says why.
                Class::Elf64 => {
                    if origin.checked_add(length).is_none() {
                        return error(
                            line,
                            format!("MEMORY region {name} extends past the 64-bit address space"),
                        );
                    }
                }
            }
            region = Some(Region { name, origin, length });
        }
        Ok(region)
    }

    /// `ORIGIN = constant-expr`.
    fn region_field(&mut self, names: &[&str]) -> Result<u64, ScriptError> {
        let line = self.line();
        let w = self.word(names[0])?;
        if !names.contains(&w.as_str()) {
            return error(line, format!("expected {}, found '{w}'", names[0]));
        }
        self.expect("=")?;
        let e = self.expr()?;
        const_eval(&e, self.class).ok_or_else(|| ScriptError {
            line,
            msg: format!("{} must be a constant expression", names[0]),
        })
    }

    fn sections(&mut self) -> Result<Vec<Command>, ScriptError> {
        self.expect("{")?;
        let mut cmds = Vec::new();
        while !self.eat("}") {
            let line = self.line();
            let name = self.word("a section name, '.' or a symbol")?;
            if self.is("=") || self.is("+=") {
                cmds.push(Command::Assign(self.assign(name, line)?));
                continue;
            }
            // `name (NOLOAD) :`, the only output section type supported.
            let noload = self.is("(")
                && matches!(self.toks.get(self.pos + 1), Some((Tok::Word(w), _)) if w == "NOLOAD");
            if noload {
                self.pos += 2;
                self.expect(")")?;
                if !self.is(":") {
                    return error(line, format!("expected ':' after '{name} (NOLOAD)'"));
                }
            }
            if self.eat(":") {
                cmds.push(Command::Output(self.output_section(name, noload, line)?));
                continue;
            }
            return match self.peek() {
                Some(Tok::Punct("(")) => error(line, format!("unsupported linker script command '{name}'")),
                Some(t) => error(
                    line,
                    format!(
                        "unsupported syntax after '{name}': found {t} (output sections are \
                         `name [(NOLOAD)] : [ALIGN(n)] {{ ... }}`, without an address or other attributes)"
                    ),
                ),
                None => error(line, "unexpected end of script"),
            };
        }
        Ok(cmds)
    }

    /// The rest of an assignment after its target name.
    fn assign(&mut self, name: String, line: usize) -> Result<Assign, ScriptError> {
        let add = self.eat("+=");
        if !add {
            self.expect("=")?;
        }
        let symbol = if name == "." { None } else { Some(name) };
        if add && symbol.is_some() {
            return error(line, "'+=' is only supported for the location counter '.'");
        }
        let expr = self.expr()?;
        self.expect(";")?;
        Ok(Assign {
            symbol,
            add,
            expr,
            line,
        })
    }

    /// The rest of an output section after its name, its `(NOLOAD)` and the
    /// colon: `[ALIGN(n)] { items } [> REGION]`.
    fn output_section(&mut self, name: String, noload: bool, line: usize) -> Result<OutputDesc, ScriptError> {
        if name == "." {
            return error(line, format!("'{name}' is not a valid output section name"));
        }
        let discard = name == OutputDesc::DISCARD;
        if discard && noload {
            return error(line, "/DISCARD/ cannot be NOLOAD");
        }
        let align = match self.peek() {
            Some(Tok::Word(w)) if w == "ALIGN" => {
                let aline = self.line();
                if discard {
                    return error(aline, "/DISCARD/ cannot have an ALIGN");
                }
                self.pos += 1;
                self.expect("(")?;
                let e = self.expr()?;
                if self.is(",") {
                    return error(aline, "only the one-argument form ALIGN(n) is supported");
                }
                self.expect(")")?;
                Some(e)
            }
            Some(Tok::Punct("{")) => None,
            Some(t) => {
                return error(
                    self.line(),
                    format!(
                        "unsupported syntax after '{name} :': found {t} (only ALIGN(n) may come before '{{')"
                    ),
                )
            }
            None => return error(self.line(), "unexpected end of script"),
        };
        self.expect("{")?;
        let mut items = Vec::new();
        while !self.eat("}") {
            let iline = self.line();
            let w = self.word("an input section list or an assignment")?;
            if self.is("=") || self.is("+=") {
                if discard {
                    return error(iline, "/DISCARD/ may only list input sections");
                }
                let a = self.assign(w, iline)?;
                if a.symbol.is_none() && !a.add && !a.expr.uses_dot() {
                    return error(
                        iline,
                        "inside an output section only '. = ALIGN(n)', '. += n' or expressions using '.' \
                         may move the location counter",
                    );
                }
                items.push(Item::Assign(a));
                continue;
            }
            if !self.is("(") {
                return error(iline, format!("expected '*(...)' or an assignment, found '{w}'"));
            }
            if w != "*" {
                return error(
                    iline,
                    format!("only '*(...)' input section lists are supported, not '{w}(...)'"),
                );
            }
            self.expect("(")?;
            let mut patterns = Vec::new();
            while !self.eat(")") {
                let pline = self.line();
                let p = self.word("a section name pattern")?;
                if self.is("(") {
                    return error(
                        pline,
                        format!("'{p}(...)' is not supported inside an input section list"),
                    );
                }
                patterns.push(p);
            }
            if patterns.is_empty() {
                return error(iline, "empty input section list '*()'");
            }
            items.push(Item::Input(patterns));
        }
        let region = if self.eat(">") {
            if discard {
                return error(self.line(), "/DISCARD/ cannot go in a memory region");
            }
            Some(self.word("a memory region name")?)
        } else {
            None
        };
        self.eat(";");
        Ok(OutputDesc {
            name,
            noload,
            align,
            items,
            region,
            line,
        })
    }

    fn expr(&mut self) -> Result<Expr, ScriptError> {
        let mut e = self.atom()?;
        loop {
            if self.eat("+") {
                e = Expr::Add(Box::new(e), Box::new(self.atom()?));
            } else if self.eat("-") {
                e = Expr::Sub(Box::new(e), Box::new(self.atom()?));
            } else {
                return Ok(e);
            }
        }
    }

    fn atom(&mut self) -> Result<Expr, ScriptError> {
        let line = self.line();
        if self.eat("(") {
            let e = self.expr()?;
            self.expect(")")?;
            return Ok(e);
        }
        let w = self.word("an expression")?;
        if w == "." {
            return Ok(Expr::Dot);
        }
        if w.as_bytes()[0].is_ascii_digit() {
            return parse_number(&w, self.class)
                .map(Expr::Num)
                .ok_or_else(|| ScriptError {
                    line,
                    msg: format!("bad number '{w}'"),
                });
        }
        if !self.is("(") {
            if w.contains(['*', '?']) {
                return error(line, format!("'{w}' is not a valid symbol name"));
            }
            return Ok(Expr::Symbol(w));
        }
        self.expect("(")?;
        let e = match w.as_str() {
            "ALIGN" => {
                let n = self.expr()?;
                if self.is(",") {
                    return error(line, "only the one-argument form ALIGN(n) is supported");
                }
                Expr::Align(Box::new(n))
            }
            "ORIGIN" => Expr::Origin(self.word("a memory region name")?),
            "LENGTH" => Expr::Length(self.word("a memory region name")?),
            _ => return error(line, format!("unsupported function '{w}' in expression")),
        };
        self.expect(")")?;
        Ok(e)
    }
}

/// Evaluates an expression made only of numbers, with the class's
/// arithmetic: modulo 2^64 on ELF64, exact `i64` on ELF32 (RV32, until #159),
/// where an overflow makes it no constant.
fn const_eval(e: &Expr, class: Class) -> Option<u64> {
    let (a, b) = match e {
        Expr::Num(n) => return Some(*n),
        Expr::Add(a, b) | Expr::Sub(a, b) => (const_eval(a, class)?, const_eval(b, class)?),
        _ => return None,
    };
    let add = matches!(e, Expr::Add(..));
    match class {
        Class::Elf32 => {
            let (a, b) = (a as i64, b as i64);
            let v = if add { a.checked_add(b) } else { a.checked_sub(b) };
            v.map(|v| v as u64)
        }
        Class::Elf64 => Some(if add { a.wrapping_add(b) } else { a.wrapping_sub(b) }),
    }
}

/// Parses a linker script for objects of class `class`.
pub fn parse(src: &str, class: Class) -> Result<Script, ScriptError> {
    let toks = lex(src)?;
    let script = Parser { toks, pos: 0, class }.script()?;
    // `> REGION` must name the region, and only when there is one.
    for c in &script.commands {
        if let Command::Output(o) = c {
            if let Some(r) = &o.region {
                match &script.memory {
                    Some(m) if &m.name == r => {}
                    _ => {
                        return error(
                            o.line,
                            format!("output section {} refers to unknown region '{r}'", o.name),
                        )
                    }
                }
            }
        }
    }
    Ok(script)
}

/// Glob match with `*` (any run of characters) and `?` (any one character).
pub fn glob_match(pattern: &str, name: &str) -> bool {
    let (p, n) = (pattern.as_bytes(), name.as_bytes());
    // Iterative matcher with single-star backtracking.
    let (mut pi, mut ni) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while ni < n.len() {
        if pi < p.len() && (p[pi] == b'?' || p[pi] == n[ni]) {
            pi += 1;
            ni += 1;
        } else if pi < p.len() && p[pi] == b'*' {
            star = Some((pi, ni));
            pi += 1;
        } else if let Some((sp, sn)) = star {
            pi = sp + 1;
            ni = sn + 1;
            star = Some((sp, sn + 1));
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|&c| c == b'*')
}

/// The layout used without `-T`: `.text` at 0x8000_0000, then `.rodata`,
/// `.data` and `.bss`, each aligned to its most-aligned input section. The
/// same for both classes (it was never ld.lld's script-less layout, whose
/// image base is 0x10000). On RV64 it takes only pc-relative code
/// (`-mcmodel=medany`): RAM at 0x8000_0000 is past what `lui %hi(x)` can
/// reach there, `-2^31 - 0x800 <= x < 2^31 - 0x800` (ld.lld's check, in
/// [`crate::reloc`]).
pub const DEFAULT_SCRIPT: &str = "
ENTRY(_start)
SECTIONS
{
    . = 0x80000000;
    .text : { *(.text.start) *(.text.init) *(.text .text.*) }
    .rodata : { *(.rodata .rodata.* .srodata .srodata.*) }
    .data : { *(.data .data.* .sdata .sdata.*) }
    .bss : { *(.bss .bss.* .sbss .sbss.*) }
}
";

#[cfg(test)]
mod tests {
    use super::*;

    fn err(src: &str) -> String {
        parse(src, Class::Elf32).unwrap_err().to_string()
    }

    #[test]
    fn parses_hello_link_ld() {
        let src = include_str!("../../../examples/hello/link.ld");
        let s = parse(src, Class::Elf32).unwrap();
        assert_eq!(s.entry.as_deref(), Some("_start"));
        assert_eq!(
            s.memory,
            Some(Region {
                name: "RAM".into(),
                origin: 0x8000_0000,
                length: 4 << 20
            })
        );
        assert_eq!(s.commands.len(), 7);
        let Command::Output(text) = &s.commands[0] else {
            panic!()
        };
        assert_eq!(text.name, ".text");
        assert_eq!(text.region.as_deref(), Some("RAM"));
        assert_eq!(
            text.items,
            vec![
                Item::Input(vec![".text.start".into()]),
                Item::Input(vec![".text*".into()])
            ]
        );
        let Command::Output(bss) = &s.commands[3] else {
            panic!()
        };
        assert_eq!(
            bss.items,
            vec![
                Item::Input(vec![".bss*".into()]),
                Item::Input(vec!["COMMON".into()])
            ]
        );
        assert_eq!(
            s.commands[4],
            Command::Assign(Assign {
                symbol: None,
                add: false,
                expr: Expr::Align(Box::new(Expr::Num(16))),
                line: 12
            })
        );
        assert_eq!(
            s.commands[5],
            Command::Assign(Assign {
                symbol: None,
                add: true,
                expr: Expr::Num(64 * 1024),
                line: 13
            })
        );
        assert_eq!(
            s.commands[6],
            Command::Assign(Assign {
                symbol: Some("__stack_top".into()),
                add: false,
                expr: Expr::Dot,
                line: 14
            })
        );
    }

    #[test]
    fn parses_the_kernels_script() {
        // kernel/kernel.ld: ALIGN(n) after the colon, NOLOAD, /DISCARD/.
        let s = parse(include_str!("../../../kernel/kernel.ld"), Class::Elf32).unwrap();
        let outs: Vec<&OutputDesc> = s
            .commands
            .iter()
            .filter_map(|c| match c {
                Command::Output(o) => Some(o),
                Command::Assign(_) => None,
            })
            .collect();
        let shape: Vec<(&str, bool, Option<&Expr>, bool)> = outs
            .iter()
            .map(|o| (o.name.as_str(), o.noload, o.align.as_ref(), o.is_discard()))
            .collect();
        let page = Expr::Num(4096);
        let sixteen = Expr::Num(16);
        assert_eq!(
            shape,
            [
                (".text", false, None, false),
                (".rodata", false, Some(&page), false),
                (".data", false, Some(&page), false),
                (".bss", true, Some(&sixteen), false),
                (".stack", true, Some(&sixteen), false),
                ("/DISCARD/", false, None, true),
            ]
        );
        assert_eq!(
            outs[5].items,
            [
                Item::Input(vec![".comment".into()]),
                Item::Input(vec![".note".into(), ".note.*".into()]),
                Item::Input(vec![".eh_frame".into()])
            ]
        );
        assert_eq!(outs[0].region.as_deref(), Some("RAM"));
        assert_eq!(outs[5].region, None);
        // `/DISCARD/` reads as one word wherever it is.
        let s = parse("SECTIONS{/DISCARD/:{*(.x)}}", Class::Elf32).unwrap();
        assert!(matches!(&s.commands[0], Command::Output(o) if o.is_discard()));
    }

    #[test]
    fn default_script_parses() {
        let s = parse(DEFAULT_SCRIPT, Class::Elf32).unwrap();
        assert_eq!(s.commands.len(), 5);
        assert!(s.memory.is_none());
    }

    #[test]
    fn numbers() {
        assert_eq!(parse_number("16", Class::Elf32), Some(16));
        assert_eq!(parse_number("0x80000000", Class::Elf32), Some(0x8000_0000));
        assert_eq!(parse_number("64K", Class::Elf32), Some(65536));
        assert_eq!(parse_number("128M", Class::Elf32), Some(128 << 20));
        assert_eq!(parse_number("0x10K", Class::Elf32), Some(16 * 1024));
        assert_eq!(parse_number("12a", Class::Elf32), None);
        assert_eq!(parse_number("K", Class::Elf32), None);
        // No sign after 0x (an -e argument can have one; script words can't).
        assert_eq!(parse_number("0x+10", Class::Elf32), None);
        assert_eq!(parse_number("0x-10", Class::Elf32), None);
        assert_eq!(parse_number("0x", Class::Elf32), None);
    }

    #[test]
    fn numbers_per_class() {
        // ELF64: every u64, and nothing past it.
        let n64 = |w| parse_number(w, Class::Elf64);
        assert_eq!(n64("0xffffffff80000000"), Some(0xffff_ffff_8000_0000));
        assert_eq!(n64("0xffffffffffffffff"), Some(u64::MAX));
        assert_eq!(n64("18446744073709551615"), Some(u64::MAX));
        assert_eq!(n64("0x10000000000000000"), None);
        assert_eq!(n64("0x4000000000000000K"), None);
        assert_eq!(n64("16M"), Some(16 << 20));
        // No G suffix, as in ld.lld (`malformed number: 1G`).
        assert_eq!(n64("1G"), None);
        // ELF32 (RV32): up to i64::MAX, the type its scripts compute in.
        assert_eq!(
            parse_number("0x7fffffffffffffff", Class::Elf32),
            Some(i64::MAX as u64)
        );
        assert_eq!(parse_number("0x8000000000000000", Class::Elf32), None);
        assert_eq!(parse_number("0x20000000000000K", Class::Elf32), None);
        let e = parse("SECTIONS { . = 0x10000000000000000; }", Class::Elf64).unwrap_err();
        assert_eq!(e.to_string(), "1: bad number '0x10000000000000000'");
        let e = parse("SECTIONS { . = 1G; }", Class::Elf64).unwrap_err();
        assert_eq!(e.to_string(), "1: bad number '1G'");
    }

    #[test]
    fn elf64_memory_regions() {
        // ELF64 constants wrap modulo 2^64, as ld.lld computes: 0 - 0x80000000
        // is the top 2 GiB, where a higher-half kernel links.
        let s = parse("MEMORY { RAM : o = 0 - 0x80000000, l = 128M }", Class::Elf64).unwrap();
        let m = s.memory.unwrap();
        assert_eq!((m.origin, m.length), (0xffff_ffff_8000_0000, 128 << 20));
        // Up to the last byte below 2^64, but not to 2^64 itself.
        parse(
            "MEMORY { RAM : o = 0xffffffff80000000, l = 0x7fffffff }",
            Class::Elf64,
        )
        .unwrap();
        let e = parse(
            "\nMEMORY { RAM : o = 0xffffffff80000000, l = 0x80000000 }",
            Class::Elf64,
        )
        .unwrap_err();
        assert_eq!(
            e.to_string(),
            "2: MEMORY region RAM extends past the 64-bit address space"
        );
        // No 4 GiB limit on ELF64.
        parse("MEMORY { RAM : o = 0x100000000, l = 0x100000000 }", Class::Elf64).unwrap();
        // ELF32 (RV32) keeps it, and reads 0 - 1 as -1.
        assert!(parse("MEMORY { RAM : o = 0x100000000, l = 1 }", Class::Elf32)
            .unwrap_err()
            .to_string()
            .contains("MEMORY ORIGIN 0x100000000 does not fit in 32 bits"));
        assert_eq!(
            parse("MEMORY { RAM : o = 0 - 1, l = 1 }", Class::Elf32)
                .unwrap_err()
                .to_string(),
            "1: MEMORY ORIGIN 0xffffffffffffffff does not fit in 32 bits"
        );
    }

    #[test]
    fn expressions_and_comments() {
        let s = parse(
            "/* header\n comment */ MEMORY { ram : org = 0x1000 + 0x10, len = 2K - 16 }\n\
             SECTIONS { top = ORIGIN(ram) + LENGTH(ram) - (4 + 4); . = ALIGN(8) ; }",
            Class::Elf32,
        )
        .unwrap();
        let m = s.memory.unwrap();
        assert_eq!((m.origin, m.length), (0x1010, 2048 - 16));
        assert_eq!(
            s.commands[0],
            Command::Assign(Assign {
                symbol: Some("top".into()),
                add: false,
                expr: Expr::Sub(
                    Box::new(Expr::Add(
                        Box::new(Expr::Origin("ram".into())),
                        Box::new(Expr::Length("ram".into()))
                    )),
                    Box::new(Expr::Add(Box::new(Expr::Num(4)), Box::new(Expr::Num(4))))
                ),
                line: 3
            })
        );
    }

    #[test]
    fn quoted_arch_and_format() {
        let s = parse(
            "OUTPUT_ARCH( \"riscv\" ) OUTPUT_FORMAT(\"elf32-littleriscv\")",
            Class::Elf32,
        )
        .unwrap();
        assert_eq!(s.format, Some(Class::Elf32));
        assert!(err("OUTPUT_ARCH(i386)").contains("only riscv"));
        // Either class is recorded, whatever the objects' (the layout
        // compares them); the first argument is the one that counts.
        let s = parse("OUTPUT_FORMAT(\"elf64-littleriscv\")", Class::Elf32).unwrap();
        assert_eq!(s.format, Some(Class::Elf64));
        let s = parse(
            "OUTPUT_FORMAT(\"elf64-littleriscv\", \"elf32-littleriscv\", \"elf64-littleriscv\")",
            Class::Elf64,
        )
        .unwrap();
        assert_eq!(s.format, Some(Class::Elf64));
        assert_eq!(parse("ENTRY(_start)", Class::Elf32).unwrap().format, None);
        // declared_format finds the same name without parsing the script.
        for (src, want) in [
            (r#"OUTPUT_FORMAT("elf64-littleriscv")"#, Some(Class::Elf64)),
            (
                r#"ENTRY(_start) OUTPUT_FORMAT("elf32-littleriscv", "elf64-littleriscv")"#,
                Some(Class::Elf32),
            ),
            (
                "MEMORY { RAM (rwx) : ORIGIN = 0, LENGTH = 4 }\nOUTPUT_FORMAT(\"elf64-littleriscv\")",
                Some(Class::Elf64),
            ),
            // The first command counts, as in the parser.
            (
                r#"OUTPUT_FORMAT("elf32-littleriscv") OUTPUT_FORMAT("elf64-littleriscv")"#,
                Some(Class::Elf32),
            ),
            ("ENTRY(_start)", None),
        ] {
            assert_eq!(declared_format(src), want, "{src}");
            assert_eq!(parse(src, Class::Elf64).unwrap().format, want, "{src}");
        }
        // And nothing where the parser finds no OUTPUT_FORMAT it knows.
        for src in [
            r#"OUTPUT_FORMAT("elf64-bigriscv")"#,
            "OUTPUT_FORMAT",
            // A word among a command's arguments, or in SECTIONS, is no command.
            "ENTRY(OUTPUT_FORMAT) SECTIONS { OUTPUT_FORMAT = 1; }",
            "/* unterminated",
        ] {
            assert_eq!(declared_format(src), None, "{src}");
        }
        assert_eq!(
            err("OUTPUT_FORMAT(\"elf32-littleriscv\", \"elf64-bigriscv\")"),
            "1: OUTPUT_FORMAT(elf64-bigriscv): only elf32-littleriscv and elf64-littleriscv are supported"
        );
    }

    #[test]
    fn rejects_everything_else() {
        let cases = [
            ("PROVIDE(foo = 1);", "unsupported linker script command 'PROVIDE'"),
            ("INCLUDE other.ld", "unsupported linker script command 'INCLUDE'"),
            (
                "SECTIONS { PROVIDE(x = .); }",
                "unsupported linker script command 'PROVIDE'",
            ),
            (
                "SECTIONS { .text : { KEEP(*(.init)) } }",
                "only '*(...)' input section lists",
            ),
            (
                "SECTIONS { .text : { crt0.o(.text) } }",
                "only '*(...)' input section lists",
            ),
            (
                "SECTIONS { .text : { *(SORT(.text*)) } }",
                "not supported inside an input section list",
            ),
            (
                "SECTIONS { .text 0x100 : { *(.text) } }",
                "without an address or other attributes",
            ),
            (
                "SECTIONS { .bss (COPY) : { *(.bss) } }",
                "unsupported linker script command '.bss'",
            ),
            (
                "SECTIONS { .bss (NOLOAD) { *(.bss) } }",
                "expected ':' after '.bss (NOLOAD)'",
            ),
            (
                "SECTIONS { .text : AT(0x100) { *(.text) } }",
                "only ALIGN(n) may come before '{'",
            ),
            ("SECTIONS { .text : ALIGN(4, 8) { } }", "one-argument form"),
            (
                "SECTIONS { /DISCARD/ (NOLOAD) : { *(.comment) } }",
                "/DISCARD/ cannot be NOLOAD",
            ),
            (
                "SECTIONS { /DISCARD/ : ALIGN(4) { *(.comment) } }",
                "/DISCARD/ cannot have an ALIGN",
            ),
            (
                "SECTIONS { /DISCARD/ : { x = .; } }",
                "/DISCARD/ may only list input sections",
            ),
            (
                "MEMORY { RAM : o = 0, l = 1M } SECTIONS { /DISCARD/ : { *(.note) } > RAM }",
                "/DISCARD/ cannot go in a memory region",
            ),
            ("SECTIONS { .text : { *(.text) } > ROM }", "unknown region 'ROM'"),
            (
                "SECTIONS { .text : { *(.text) } AT> ROM }",
                "unsupported syntax after 'AT'",
            ),
            ("SECTIONS { . = 4 / 2; }", "unexpected '/'"),
            ("SECTIONS { x += 4; }", "only supported for the location counter"),
            (
                "SECTIONS { .text : { . = 0x100; } }",
                "may move the location counter",
            ),
            ("SECTIONS { . = ALIGN(., 8); }", "one-argument form"),
            ("SECTIONS { . = SIZEOF(.text); }", "unsupported function 'SIZEOF'"),
            ("SECTIONS { . = 12x; }", "bad number '12x'"),
            (
                "MEMORY { a : o = 0, l = 1 b : o = 1, l = 1 }",
                "only one MEMORY region",
            ),
            ("MEMORY { a : o = 0x80000000, l = 0x80000001 }", "past 4 GiB"),
            ("MEMORY { a : o = ., l = 1 }", "constant expression"),
            ("MEMORY { a (!rx) : o = 0, l = 1 }", "unexpected character '!'"),
            ("SECTIONS { } SECTIONS { }", "only one SECTIONS"),
            ("SECTIONS { . = 1 }", "expected ';'"),
            ("ENTRY(", "unexpected end of script"),
            ("/* never closed", "unterminated comment"),
        ];
        for (src, want) in cases {
            let e = err(src);
            assert!(e.contains(want), "{src:?}: got {e:?}, want {want:?}");
        }
    }

    #[test]
    fn error_lines() {
        let e = parse(
            "ENTRY(_start)\n\nSECTIONS {\n  .text : { FOO(*) }\n}",
            Class::Elf32,
        )
        .unwrap_err();
        assert_eq!(e.line, 4);
    }

    #[test]
    fn globs() {
        assert!(glob_match(".text*", ".text"));
        assert!(glob_match(".text*", ".text.start"));
        assert!(!glob_match(".text*", ".rodata"));
        assert!(glob_match(".text.*", ".text.x"));
        assert!(!glob_match(".text.*", ".text"));
        assert!(glob_match("*", ".anything"));
        assert!(glob_match(".?data", ".sdata"));
        assert!(!glob_match(".?data", ".data"));
        assert!(glob_match("*a*b", ".xaxxb"));
        assert!(!glob_match("*a*b", ".xaxxbc"));
        assert!(glob_match(".bss", ".bss"));
        assert!(!glob_match(".bss", ".bss.x"));
    }
}
