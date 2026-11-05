//! The linker-script subset merlin-ld understands (spec §10, "Linker scripts").
//!
//! ```text
//! OUTPUT_ARCH(riscv)                       optional; must say riscv
//! OUTPUT_FORMAT("elf64-littleriscv")       optional; must say elf64-littleriscv
//! ENTRY(symbol)
//! MEMORY { RAM (rwx) : ORIGIN = expr, LENGTH = expr }      at most one region
//! SECTIONS
//! {
//!     . = expr;   . += expr;   symbol = expr;
//!     .name : { *(pattern pattern ...) ... . = ALIGN(n); . += n; symbol = expr; } > RAM
//!     .name (NOLOAD) : AT(lma) ALIGN(n) { ... } > RAM    each optional, in this order
//!     /DISCARD/ : { *(pattern ...) }
//! }
//! ```
//!
//! Expressions: numbers (`16`, `0x80000000`, `64K`, `128M`), `.`, symbols,
//! `ALIGN(n)`, `ORIGIN(region)`, `LENGTH(region)`, `ADDR(section)`,
//! parentheses, `+` and `-`. Comments are `/* ... */`. Anything else is an
//! error naming what was found.
//!
//! Names (section names, patterns, the symbols assignments set, regions,
//! formats) take `-` as ld.lld's lexer does outside expressions
//! (`OUTPUT_FORMAT(elf64-littleriscv)`, `*(.text-x)`); in an expression
//! `a-b` is `a - b` ([`Parser::name`]).
//!
//! `AT(lma)` gives an output section a load address (LMA) other than the
//! address it runs at (its VMA): the `p_paddr` of its segment, where a
//! loader puts its bytes. Linux's riscv `vmlinux.lds` writes it
//! `AT(ADDR(.text) - LOAD_OFFSET)`, so `ADDR(section)`, an output section's
//! address, comes with it. [`crate::layout`] explains what they mean.
//!
//! Values are 64-bit addresses, as the ELF64 (RV64) objects' are: numbers go
//! up to 2^64 - 1 and `+` and `-` wrap modulo 2^64, as in ld.lld. (Until #159
//! a script linking ELF32 objects was read with exact `i64`s and a 4 GiB
//! address space.)

use elf::Class;
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Script {
    pub entry: Option<String>,
    /// `OUTPUT_FORMAT(...)`'s first argument, which ld.lld goes by: ELF64,
    /// the only class merlin-ld writes (any other name is refused).
    pub format: Option<Class>,
    pub memory: Option<Region>,
    /// The contents of `SECTIONS`, in order.
    pub commands: Vec<Command>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Region {
    pub name: String,
    /// `origin + length` does not overflow: [`parse`] checks it (within
    /// 2^64).
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
    /// `AT(lma)` after the colon: the section is loaded at `lma` (its load
    /// address, `p_paddr`) rather than where it runs.
    pub at: Option<Expr>,
    /// `ALIGN(n)` after the colon (after `AT`): the section starts at least
    /// this aligned.
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
    /// `ADDR(section)`: the address of an output section.
    Addr(String),
    Add(Box<Expr>, Box<Expr>),
    Sub(Box<Expr>, Box<Expr>),
}

impl Expr {
    /// Whether the value depends on the location counter.
    pub fn uses_dot(&self) -> bool {
        match self {
            Expr::Dot | Expr::Align(_) => true,
            Expr::Num(_) | Expr::Symbol(_) | Expr::Origin(_) | Expr::Length(_) | Expr::Addr(_) => false,
            Expr::Add(a, b) | Expr::Sub(a, b) => a.uses_dot() || b.uses_dot(),
        }
    }

    /// Whether the value depends on a symbol. ld.lld keeps an output section
    /// without inputs if its `AT` or `ALIGN` does (its
    /// `expressionsUseSymbols`): the expression must still be evaluated, and
    /// an error reported if it cannot be. `ADDR` and `ORIGIN` name no symbol.
    pub fn uses_symbols(&self) -> bool {
        match self {
            Expr::Symbol(_) => true,
            Expr::Align(n) => n.uses_symbols(),
            Expr::Num(_) | Expr::Dot | Expr::Origin(_) | Expr::Length(_) | Expr::Addr(_) => false,
            Expr::Add(a, b) | Expr::Sub(a, b) => a.uses_symbols() || b.uses_symbols(),
        }
    }

    /// Appends the output sections the expression names with `ADDR(...)`.
    fn addr_names<'a>(&'a self, out: &mut Vec<&'a str>) {
        match self {
            Expr::Addr(name) => out.push(name),
            Expr::Align(n) => n.addr_names(out),
            Expr::Add(a, b) | Expr::Sub(a, b) => {
                a.addr_names(out);
                b.addr_names(out);
            }
            Expr::Num(_) | Expr::Dot | Expr::Symbol(_) | Expr::Origin(_) | Expr::Length(_) => {}
        }
    }
}

impl Script {
    /// The output sections some expression of the script names with
    /// `ADDR(...)`, anywhere in it. ld.lld keeps such a section even when
    /// nothing goes in it (its `usedInExpression`): an empty section still
    /// has an address, which the expression needs.
    pub fn addr_names(&self) -> Vec<&str> {
        let mut out = Vec::new();
        for c in &self.commands {
            match c {
                Command::Assign(a) => a.expr.addr_names(&mut out),
                Command::Output(o) => {
                    for e in o.at.iter().chain(&o.align) {
                        e.addr_names(&mut out);
                    }
                    for i in &o.items {
                        if let Item::Assign(a) = i {
                            a.expr.addr_names(&mut out);
                        }
                    }
                }
            }
        }
        out
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

/// The tokens of a script, each with its line, and for each whether it
/// follows the one before with nothing between them, no space and no
/// comment ([`Parser::name`] joins such runs).
type Lexed = (Vec<(Tok, usize)>, Vec<bool>);

fn lex(src: &str) -> Result<Lexed, ScriptError> {
    let mut toks = Vec::new();
    let mut glued = Vec::new();
    let mut line = 1;
    let mut chars = src.chars().peekable();
    // Whether the next token follows the last one directly.
    let mut adjacent = false;
    while let Some(&c) = chars.peek() {
        let before = toks.len();
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
                } else if chars.peek() != Some(&'*') {
                    return error(line, "unexpected '/' (only /* */ comments are supported)");
                } else {
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
        // White space and comments push nothing, and separate tokens.
        if toks.len() > before {
            glued.push(adjacent);
            adjacent = true;
        } else {
            adjacent = false;
        }
    }
    Ok((toks, glued))
}

/// The `OUTPUT_FORMAT` name of the class merlin-ld writes, ELF64.
pub const FORMAT_NAME: &str = "elf64-littleriscv";

/// The class an `OUTPUT_FORMAT` name stands for, if merlin-ld writes it:
/// ELF64. ld.lld takes a `-freebsd` suffix too (`elf64-littleriscv-freebsd`).
fn format_class(name: &str) -> Option<Class> {
    let name = name.strip_suffix("-freebsd").unwrap_or(name);
    (name == FORMAT_NAME).then_some(Class::Elf64)
}

/// Parses a number: decimal or `0x` hex, with an optional `K` or `M` suffix
/// (ld.lld has no `G` either): any `u64`.
pub fn parse_number(w: &str) -> Option<u64> {
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
    v.checked_mul(mult)
}

struct Parser {
    toks: Vec<(Tok, usize)>,
    /// [`Lexed`]'s: whether each token follows the one before directly.
    glued: Vec<bool>,
    pos: usize,
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
    /// A name: a section name, a pattern, a symbol an assignment sets, a
    /// region, a format. ld.lld's lexer reads these outside expressions,
    /// where `-` is a character of a name as letters are, so
    /// `elf64-littleriscv` and `*(.text-x)` are one token each, while
    /// inside an expression `a-b` is `a - b`. merlin-ld lexes `-` apart
    /// and joins it back here: the name is the word or `-` there and every
    /// word and `-` that follows it with nothing between them.
    fn name(&mut self, what: &str) -> Result<String, ScriptError> {
        let line = self.line();
        let mut name = match self.next()? {
            Tok::Word(w) => w,
            Tok::Punct("-") => "-".to_string(),
            t => return error(line, format!("expected {what}, found {t}")),
        };
        while self.glued.get(self.pos) == Some(&true) {
            match self.peek() {
                Some(Tok::Word(w)) => name.push_str(w),
                Some(Tok::Punct("-")) => name.push('-'),
                _ => break,
            }
            self.pos += 1;
        }
        Ok(name)
    }
    /// A [`name`](Self::name) or a quoted string, as in
    /// `OUTPUT_ARCH("riscv")`.
    fn name_or_str(&mut self, what: &str) -> Result<String, ScriptError> {
        if let Some(Tok::Str(s)) = self.peek() {
            let s = s.clone();
            self.pos += 1;
            return Ok(s);
        }
        self.name(what)
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
                    script.entry = Some(self.name("an entry symbol")?);
                    self.expect(")")?;
                }
                "OUTPUT_ARCH" => {
                    self.expect("(")?;
                    let arch = self.name_or_str("an architecture")?;
                    if !arch.eq_ignore_ascii_case("riscv") {
                        return error(line, format!("OUTPUT_ARCH({arch}): only riscv is supported"));
                    }
                    self.expect(")")?;
                }
                "OUTPUT_FORMAT" => {
                    self.expect("(")?;
                    // `OUTPUT_FORMAT(name)` or `OUTPUT_FORMAT(default, big,
                    // little)`, one argument or three, as ld.lld takes them:
                    // it goes by the first; merlin-ld also wants the others
                    // to be names it knows.
                    let first = self.format_arg(line)?;
                    if self.eat(",") {
                        self.format_arg(line)?;
                        self.expect(",")?;
                        self.format_arg(line)?;
                    }
                    self.expect(")")?;
                    script.format.get_or_insert(first);
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

    /// One argument of `OUTPUT_FORMAT`, a name merlin-ld knows.
    fn format_arg(&mut self, line: usize) -> Result<Class, ScriptError> {
        let fmt = self.name_or_str("an output format")?;
        format_class(&fmt).ok_or_else(|| ScriptError {
            line,
            msg: format!("OUTPUT_FORMAT({fmt}): only {FORMAT_NAME} is supported"),
        })
    }

    fn memory(&mut self) -> Result<Option<Region>, ScriptError> {
        self.expect("{")?;
        let mut region = None;
        while !self.eat("}") {
            let line = self.line();
            let name = self.name("a memory region name")?;
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
            // The region ends below 2^64. ld.lld computes its end modulo 2^64,
            // so one that reaches 2^64 ends at 0 and no section fits in it; an
            // error here says why.
            if origin.checked_add(length).is_none() {
                return error(
                    line,
                    format!("MEMORY region {name} extends past the 64-bit address space"),
                );
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
        const_eval(&e).ok_or_else(|| ScriptError {
            line,
            msg: format!("{} must be a constant expression", names[0]),
        })
    }

    fn sections(&mut self) -> Result<Vec<Command>, ScriptError> {
        self.expect("{")?;
        let mut cmds = Vec::new();
        while !self.eat("}") {
            let line = self.line();
            let name = self.name("a section name, '.' or a symbol")?;
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
                         `name [(NOLOAD)] : [AT(lma)] [ALIGN(n)] {{ ... }}`, without an address or other \
                         attributes)"
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
    /// colon: `[AT(lma)] [ALIGN(n)] { items } [> REGION]`. ld.lld reads `AT`
    /// and `ALIGN` in that order only (`ALIGN(n) AT(lma)` is its
    /// "{ expected, but got AT").
    fn output_section(&mut self, name: String, noload: bool, line: usize) -> Result<OutputDesc, ScriptError> {
        if name == "." {
            return error(line, format!("'{name}' is not a valid output section name"));
        }
        // A /DISCARD/'s NOLOAD and AT are read, as ld.lld reads them, and
        // mean nothing; its ALIGN is checked ([`crate::layout`]).
        let discard = name == OutputDesc::DISCARD;
        let at = match self.peek() {
            Some(Tok::Word(w)) if w == "AT" => {
                self.pos += 1;
                self.expect("(")?;
                let e = self.expr()?;
                self.expect(")")?;
                Some(e)
            }
            _ => None,
        };
        let align = match self.peek() {
            Some(Tok::Word(w)) if w == "ALIGN" => {
                let aline = self.line();
                self.pos += 1;
                self.expect("(")?;
                let e = self.expr()?;
                if self.is(",") {
                    return error(aline, "only the one-argument form ALIGN(n) is supported");
                }
                self.expect(")")?;
                Some(e)
            }
            _ => None,
        };
        match self.peek() {
            Some(Tok::Punct("{")) => {}
            Some(t) => {
                return error(
                    self.line(),
                    format!(
                        "unsupported syntax after '{name} :': found {t} (only AT(lma) and then ALIGN(n) may \
                         come before '{{')"
                    ),
                )
            }
            None => return error(self.line(), "unexpected end of script"),
        }
        self.expect("{")?;
        let mut items = Vec::new();
        while !self.eat("}") {
            let iline = self.line();
            let w = self.name("an input section list or an assignment")?;
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
                let p = self.name("a section name pattern")?;
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
            Some(self.name("a memory region name")?)
        } else {
            None
        };
        self.eat(";");
        Ok(OutputDesc {
            name,
            noload,
            at,
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
            return parse_number(&w).map(Expr::Num).ok_or_else(|| ScriptError {
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
            "ORIGIN" => Expr::Origin(self.name("a memory region name")?),
            "LENGTH" => Expr::Length(self.name("a memory region name")?),
            "ADDR" => Expr::Addr(self.name("a section name")?),
            _ => return error(line, format!("unsupported function '{w}' in expression")),
        };
        self.expect(")")?;
        Ok(e)
    }
}

/// Evaluates an expression made only of numbers, modulo 2^64.
fn const_eval(e: &Expr) -> Option<u64> {
    let (a, b) = match e {
        Expr::Num(n) => return Some(*n),
        Expr::Add(a, b) | Expr::Sub(a, b) => (const_eval(a)?, const_eval(b)?),
        _ => return None,
    };
    Some(if matches!(e, Expr::Add(..)) {
        a.wrapping_add(b)
    } else {
        a.wrapping_sub(b)
    })
}

/// Parses a linker script.
pub fn parse(src: &str) -> Result<Script, ScriptError> {
    let (toks, glued) = lex(src)?;
    let script = Parser { toks, glued, pos: 0 }.script()?;
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
        parse(src).unwrap_err().to_string()
    }

    #[test]
    fn parses_hello_link_ld() {
        let src = include_str!("../../../examples/hello/link.ld");
        let s = parse(src).unwrap();
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

    /// kernel/kernel.lds.S for `xlen`, preprocessed as `cargo xtask kernel`
    /// does (`merlin -E -P`, with the assembly's include directories).
    fn kernel_script(xlen: merlin::types::Xlen) -> String {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let kernel = root.join("kernel");
        let src = std::fs::read_to_string(kernel.join("kernel.lds.S")).unwrap();
        let opts = merlin::Options {
            xlen,
            include_dirs: vec!["../include".into(), "include".into()],
            line_markers: false,
            ..merlin::Options::default()
        };
        let load = |p: &std::path::Path| std::fs::read_to_string(kernel.join(p)).ok();
        merlin::preprocess_asm("kernel.lds.S", &src, &opts, &load).unwrap()
    }

    #[test]
    fn parses_the_kernels_script() {
        // kernel/kernel.lds.S: ALIGN(n) after the colon, NOLOAD, /DISCARD/;
        // linked high, loaded at 0x8020_0000, where the firmware starts its
        // next stage (OpenSBI's FW_JUMP_ADDR), no region.
        let s = parse(&kernel_script(merlin::types::Xlen::Rv64)).unwrap();
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
        // The boot stack, aligned to twice its size as every kernel stack is.
        let stack = Expr::Num(32 << 10);
        assert_eq!(
            shape,
            [
                (".text", false, None, false),
                (".rodata", false, Some(&page), false),
                (".data", false, Some(&page), false),
                (".bss", true, Some(&sixteen), false),
                (".stack", true, Some(&stack), false),
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
        assert_eq!(outs[0].at, Some(Expr::Num(0x8020_0000)));
        assert!(outs.iter().all(|o| o.region.is_none()));
        // `/DISCARD/` reads as one word wherever it is.
        let s = parse("SECTIONS{/DISCARD/:{*(.x)}}").unwrap();
        assert!(matches!(&s.commands[0], Command::Output(o) if o.is_discard()));
    }

    /// Linux's form (riscv vmlinux.lds): `AT(ADDR(.x) - LOAD_OFFSET)`, then
    /// `ALIGN(n)`, with or without `(NOLOAD)`.
    #[test]
    fn parses_at_and_addr() {
        let s = parse(
            "SECTIONS {\n\
             . = 0xffffffff80000000;\n\
             .text : AT(ADDR(.text) - 0xffffffff00000000) { *(.text) }\n\
             .data : AT(. - 0xffffffff00000000) ALIGN(4096) { x = ADDR(.text); *(.data) }\n\
             .bss (NOLOAD) : AT(lma) { *(.bss) }\n\
             .rodata : ALIGN(16) { *(.rodata) }\n\
             end = ADDR(.bss) + 4;\n\
             }",
        )
        .unwrap();
        let outs: Vec<&OutputDesc> = s
            .commands
            .iter()
            .filter_map(|c| match c {
                Command::Output(o) => Some(o),
                Command::Assign(_) => None,
            })
            .collect();
        let offset = || Box::new(Expr::Num(0xffff_ffff_0000_0000));
        assert_eq!(
            outs[0].at,
            Some(Expr::Sub(Box::new(Expr::Addr(".text".into())), offset()))
        );
        assert_eq!(outs[0].align, None);
        assert_eq!(outs[1].at, Some(Expr::Sub(Box::new(Expr::Dot), offset())));
        assert_eq!(outs[1].align, Some(Expr::Num(4096)));
        assert_eq!(
            outs[1].items[0],
            Item::Assign(Assign {
                symbol: Some("x".into()),
                add: false,
                expr: Expr::Addr(".text".into()),
                line: 4
            })
        );
        assert!(outs[2].noload);
        assert_eq!(outs[2].at, Some(Expr::Symbol("lma".into())));
        assert_eq!(
            (outs[3].at.as_ref(), outs[3].align.as_ref()),
            (None, Some(&Expr::Num(16)))
        );
        // Every section an ADDR names, wherever it is: in an AT, in an
        // assignment inside a section and at the top level.
        assert_eq!(s.addr_names(), [".text", ".text", ".bss"]);
        // ADDR does not use the location counter or a symbol; `.` and a
        // symbol do, also inside ALIGN.
        let at = |i: usize| outs[i].at.as_ref().unwrap();
        assert!(!at(0).uses_dot() && !at(0).uses_symbols());
        assert!(at(1).uses_dot() && !at(1).uses_symbols());
        assert!(at(2).uses_symbols());
        let align_of_sym = Expr::Align(Box::new(Expr::Symbol("n".into())));
        assert!(align_of_sym.uses_symbols());
        assert!(!Expr::Origin("RAM".into()).uses_symbols());
    }

    #[test]
    fn default_script_parses() {
        let s = parse(DEFAULT_SCRIPT).unwrap();
        assert_eq!(s.commands.len(), 5);
        assert!(s.memory.is_none());
    }

    #[test]
    fn numbers() {
        assert_eq!(parse_number("16"), Some(16));
        assert_eq!(parse_number("0x80000000"), Some(0x8000_0000));
        assert_eq!(parse_number("64K"), Some(65536));
        assert_eq!(parse_number("128M"), Some(128 << 20));
        assert_eq!(parse_number("0x10K"), Some(16 * 1024));
        assert_eq!(parse_number("12a"), None);
        assert_eq!(parse_number("K"), None);
        // No sign after 0x (an -e argument can have one; script words can't).
        assert_eq!(parse_number("0x+10"), None);
        assert_eq!(parse_number("0x-10"), None);
        assert_eq!(parse_number("0x"), None);
    }

    #[test]
    fn numbers_fit_in_64_bits() {
        // Every u64, and nothing past it.
        assert_eq!(parse_number("0xffffffff80000000"), Some(0xffff_ffff_8000_0000));
        assert_eq!(parse_number("0xffffffffffffffff"), Some(u64::MAX));
        assert_eq!(parse_number("18446744073709551615"), Some(u64::MAX));
        assert_eq!(parse_number("0x10000000000000000"), None);
        assert_eq!(parse_number("0x4000000000000000K"), None);
        assert_eq!(parse_number("16M"), Some(16 << 20));
        // No G suffix, as in ld.lld (`malformed number: 1G`).
        assert_eq!(parse_number("1G"), None);
        let e = parse("SECTIONS { . = 0x10000000000000000; }").unwrap_err();
        assert_eq!(e.to_string(), "1: bad number '0x10000000000000000'");
        let e = parse("SECTIONS { . = 1G; }").unwrap_err();
        assert_eq!(e.to_string(), "1: bad number '1G'");
    }

    #[test]
    fn memory_regions_wrap_modulo_2_64() {
        // ELF64 constants wrap modulo 2^64, as ld.lld computes: 0 - 0x80000000
        // is the top 2 GiB, where a higher-half kernel links.
        let s = parse("MEMORY { RAM : o = 0 - 0x80000000, l = 128M }").unwrap();
        let m = s.memory.unwrap();
        assert_eq!((m.origin, m.length), (0xffff_ffff_8000_0000, 128 << 20));
        // Up to the last byte below 2^64, but not to 2^64 itself.
        parse("MEMORY { RAM : o = 0xffffffff80000000, l = 0x7fffffff }").unwrap();
        let e = parse("\nMEMORY { RAM : o = 0xffffffff80000000, l = 0x80000000 }").unwrap_err();
        assert_eq!(
            e.to_string(),
            "2: MEMORY region RAM extends past the 64-bit address space"
        );
        // No 4 GiB limit: a region may start or end above it.
        parse("MEMORY { RAM : o = 0x100000000, l = 0x100000000 }").unwrap();
    }

    #[test]
    fn expressions_and_comments() {
        let s = parse(
            "/* header\n comment */ MEMORY { ram : org = 0x1000 + 0x10, len = 2K - 16 }\n\
             SECTIONS { top = ORIGIN(ram) + LENGTH(ram) - (4 + 4); . = ALIGN(8) ; }",
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
        let s = parse("OUTPUT_ARCH( \"riscv\" ) OUTPUT_FORMAT(\"elf64-littleriscv\")").unwrap();
        assert_eq!(s.format, Some(Class::Elf64));
        assert!(err("OUTPUT_ARCH(i386)").contains("only riscv"));
        // Three arguments, as ld.lld takes them: every one must be ELF64,
        // the first is the one that counts.
        let s = parse("OUTPUT_FORMAT(\"elf64-littleriscv\", \"elf64-littleriscv\", \"elf64-littleriscv\")")
            .unwrap();
        assert_eq!(s.format, Some(Class::Elf64));
        assert_eq!(parse("ENTRY(_start)").unwrap().format, None);
        for (src, want) in [
            (r#"OUTPUT_FORMAT("elf64-littleriscv")"#, Some(Class::Elf64)),
            // ld.lld takes the -freebsd names too.
            (
                r#"OUTPUT_FORMAT("elf64-littleriscv-freebsd")"#,
                Some(Class::Elf64),
            ),
            (
                "MEMORY { RAM (rwx) : ORIGIN = 0, LENGTH = 4 }\nOUTPUT_FORMAT(\"elf64-littleriscv\")",
                Some(Class::Elf64),
            ),
            ("ENTRY(_start)", None),
            // A word among a command's arguments, or in SECTIONS, is no command.
            ("ENTRY(OUTPUT_FORMAT) SECTIONS { OUTPUT_FORMAT = 1; }", None),
        ] {
            assert_eq!(parse(src).unwrap().format, want, "{src}");
        }
        // An RV32 (ELF32) format is refused as any other name is, in any
        // position.
        for src in [
            r#"OUTPUT_FORMAT("elf32-littleriscv")"#,
            "OUTPUT_FORMAT(elf32-littleriscv-freebsd)",
            r#"OUTPUT_FORMAT("elf64-littleriscv", "elf32-littleriscv", "elf64-littleriscv")"#,
        ] {
            assert!(
                err(src).ends_with(": only elf64-littleriscv is supported"),
                "{src}: {}",
                err(src)
            );
        }
        assert_eq!(
            err("OUTPUT_FORMAT(\"elf64-littleriscv\", \"elf64-bigriscv\", \"elf64-littleriscv\")"),
            "1: OUTPUT_FORMAT(elf64-bigriscv): only elf64-littleriscv is supported"
        );
        // A /DISCARD/'s NOLOAD, AT and ALIGN are read, as ld.lld reads them.
        let s = parse("SECTIONS { /DISCARD/ (NOLOAD) : AT(nowhere) ALIGN(16) { *(.comment) } }").unwrap();
        let Command::Output(o) = &s.commands[0] else {
            panic!()
        };
        assert!(o.is_discard() && o.at.is_some() && o.align.is_some());
    }

    #[test]
    fn names_take_dashes_and_expressions_do_not() {
        // Outside expressions ld.lld's tokens take `-` as they take letters,
        // so an unquoted format name is one token.
        assert_eq!(
            parse("OUTPUT_FORMAT(elf64-littleriscv)").unwrap().format,
            Some(Class::Elf64)
        );
        assert_eq!(
            err("OUTPUT_FORMAT(elf32-littleriscv)"),
            "1: OUTPUT_FORMAT(elf32-littleriscv): only elf64-littleriscv is supported"
        );
        assert_eq!(
            err("OUTPUT_FORMAT(elf64-bigriscv)"),
            "1: OUTPUT_FORMAT(elf64-bigriscv): only elf64-littleriscv is supported"
        );
        assert_eq!(
            err("OUTPUT_ARCH(riscv-x)"),
            "1: OUTPUT_ARCH(riscv-x): only riscv is supported"
        );
        // Section names, patterns, symbols an assignment sets, regions and
        // the names ADDR, ORIGIN and LENGTH take.
        let s = parse(
            "ENTRY(my-start) MEMORY { R-AM : ORIGIN = 0x1000, LENGTH = 4K }\n\
             SECTIONS { a-b = 1; -c = 2; .text-x : { *(.text-x .t-*) d-e = .; } > R-AM\n\
             f = ADDR(.text-x) + ORIGIN(R-AM) + LENGTH(R-AM); }",
        )
        .unwrap();
        assert_eq!(s.entry.as_deref(), Some("my-start"));
        assert_eq!(s.memory.as_ref().unwrap().name, "R-AM");
        let Command::Assign(a) = &s.commands[0] else {
            panic!()
        };
        assert_eq!(a.symbol.as_deref(), Some("a-b"));
        let Command::Assign(c) = &s.commands[1] else {
            panic!()
        };
        assert_eq!(c.symbol.as_deref(), Some("-c"));
        let Command::Output(o) = &s.commands[2] else {
            panic!()
        };
        assert_eq!(o.name, ".text-x");
        assert_eq!(o.items[0], Item::Input(vec![".text-x".into(), ".t-*".into()]));
        let Item::Assign(d) = &o.items[1] else { panic!() };
        assert_eq!(d.symbol.as_deref(), Some("d-e"));
        assert_eq!(o.region.as_deref(), Some("R-AM"));
        let Command::Assign(f) = &s.commands[3] else {
            panic!()
        };
        let addr = |n: &str| Box::new(Expr::Addr(n.into()));
        let (origin, length) = (
            Box::new(Expr::Origin("R-AM".into())),
            Box::new(Expr::Length("R-AM".into())),
        );
        assert_eq!(
            f.expr,
            Expr::Add(Box::new(Expr::Add(addr(".text-x"), origin)), length)
        );
        // A space or a comment ends a name, and in an expression `-` is
        // always the operator, glued or not.
        let s = parse("SECTIONS { x = a-b; y = 3-1; .t /**/-x : { } }");
        assert!(s.is_err());
        let s = parse("SECTIONS { x = a-b; y = 3-1; }").unwrap();
        let Command::Assign(x) = &s.commands[0] else {
            panic!()
        };
        assert_eq!(
            x.expr,
            Expr::Sub(
                Box::new(Expr::Symbol("a".into())),
                Box::new(Expr::Symbol("b".into()))
            )
        );
        let Command::Assign(y) = &s.commands[1] else {
            panic!()
        };
        assert_eq!(const_eval(&y.expr), Some(2));
        assert_eq!(
            err("SECTIONS { .t - x : { } }"),
            "1: unsupported syntax after '.t': found '-' (output sections are `name [(NOLOAD)] : \
             [AT(lma)] [ALIGN(n)] { ... }`, without an address or other attributes)"
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
                "1: unsupported syntax after '.text': found '0x100' (output sections are \
                 `name [(NOLOAD)] : [AT(lma)] [ALIGN(n)] { ... }`, without an address or other attributes)",
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
                "SECTIONS { .text : ALIGN(4) AT(0x100) { *(.text) } }",
                "1: unsupported syntax after '.text :': found 'AT' (only AT(lma) and then ALIGN(n) may come \
                 before '{')",
            ),
            (
                "SECTIONS { .text : AT(0x100) AT(0x200) { *(.text) } }",
                "found 'AT' (only AT(lma) and then ALIGN(n)",
            ),
            (
                "SECTIONS { .text : AT(0x100) FOO { *(.text) } }",
                "found 'FOO' (only AT(lma) and then ALIGN(n)",
            ),
            ("SECTIONS { .text : AT(0x100, 4) { } }", "expected ')', found ','"),
            ("SECTIONS { .text : AT 0x100 { } }", "expected '(', found '0x100'"),
            ("SECTIONS { x = ADDR(); }", "expected a section name, found ')'"),
            ("SECTIONS { x = ADDR(.text, 4); }", "expected ')', found ','"),
            (
                "SECTIONS { x = LOADADDR(.text); }",
                "unsupported function 'LOADADDR'",
            ),
            ("MEMORY { a : o = ADDR(.text), l = 1 }", "constant expression"),
            ("SECTIONS { .text : ALIGN(4, 8) { } }", "one-argument form"),
            // OUTPUT_FORMAT takes one argument or three, as in ld.lld.
            (
                r#"OUTPUT_FORMAT("elf64-littleriscv", "elf64-littleriscv")"#,
                "expected ',', found ')'",
            ),
            (
                r#"OUTPUT_FORMAT("elf64-littleriscv", "elf64-littleriscv", "elf64-littleriscv", "x")"#,
                "expected ')', found ','",
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
            (
                "MEMORY { a : o = 0xffffffff80000000, l = 0x80000001 }",
                "past the 64-bit address space",
            ),
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
        let e = parse("ENTRY(_start)\n\nSECTIONS {\n  .text : { FOO(*) }\n}").unwrap_err();
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
