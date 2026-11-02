//! The linker-script subset merlin-ld understands (spec §10, "Linker scripts").
//!
//! ```text
//! OUTPUT_ARCH(riscv)                       optional; must say riscv
//! OUTPUT_FORMAT("elf32-littleriscv")       optional; must say elf32-littleriscv
//! ENTRY(symbol)
//! MEMORY { RAM (rwx) : ORIGIN = expr, LENGTH = expr }      at most one region
//! SECTIONS
//! {
//!     . = expr;   . += expr;   symbol = expr;
//!     .name : { *(pattern pattern ...) ... . = ALIGN(n); . += n; symbol = expr; } > RAM
//! }
//! ```
//!
//! Expressions: numbers (`16`, `0x80000000`, `64K`, `128M`), `.`, symbols,
//! `ALIGN(n)`, `ORIGIN(region)`, `LENGTH(region)`, parentheses, `+` and `-`.
//! Comments are `/* ... */`. Anything else is an error naming what was found.

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Script {
    pub entry: Option<String>,
    pub memory: Option<Region>,
    /// The contents of `SECTIONS`, in order.
    pub commands: Vec<Command>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Region {
    pub name: String,
    pub origin: u32,
    pub length: u32,
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
    pub name: String,
    pub items: Vec<Item>,
    /// `> REGION`.
    pub region: Option<String>,
    pub line: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    /// `*(pattern ...)`: input sections from every file whose name matches a pattern.
    Input(Vec<String>),
    Assign(Assign),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expr {
    Num(i64),
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
                    return error(
                        line,
                        "/DISCARD/ is not supported (non-allocated sections are dropped without it)",
                    );
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

/// Parses a number: decimal or `0x` hex, with an optional `K` or `M` suffix.
pub fn parse_number(w: &str) -> Option<i64> {
    let (digits, mult) = match w.as_bytes().last()? {
        b'K' | b'k' => (&w[..w.len() - 1], 1024),
        b'M' | b'm' => (&w[..w.len() - 1], 1024 * 1024),
        _ => (w, 1),
    };
    let v = if let Some(h) = digits.strip_prefix("0x").or_else(|| digits.strip_prefix("0X")) {
        i64::from_str_radix(h, 16).ok()?
    } else if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) {
        digits.parse::<i64>().ok()?
    } else {
        return None;
    };
    v.checked_mul(mult)
}

struct Parser {
    toks: Vec<(Tok, usize)>,
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
                    loop {
                        let fmt = self.word_or_str("an output format")?;
                        if fmt != "elf32-littleriscv" {
                            return error(
                                line,
                                format!("OUTPUT_FORMAT({fmt}): only elf32-littleriscv is supported"),
                            );
                        }
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
            let to_u32 = |v: i64, what: &str| {
                u32::try_from(v)
                    .or_else(|_| error(line, format!("MEMORY {what} {v:#x} does not fit in 32 bits")))
            };
            let origin = to_u32(origin, "ORIGIN")?;
            let length = to_u32(length, "LENGTH")?;
            if origin as u64 + length as u64 > 1 << 32 {
                return error(line, format!("MEMORY region {name} extends past 4 GiB"));
            }
            region = Some(Region { name, origin, length });
        }
        Ok(region)
    }

    /// `ORIGIN = constant-expr`.
    fn region_field(&mut self, names: &[&str]) -> Result<i64, ScriptError> {
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
            let name = self.word("a section name, '.' or a symbol")?;
            if self.is("=") || self.is("+=") {
                cmds.push(Command::Assign(self.assign(name, line)?));
                continue;
            }
            if self.eat(":") {
                cmds.push(Command::Output(self.output_section(name, line)?));
                continue;
            }
            return match self.peek() {
                Some(Tok::Punct("(")) => error(line, format!("unsupported linker script command '{name}'")),
                Some(t) => error(
                    line,
                    format!(
                        "unsupported syntax after '{name}': found {t} (output sections are \
                         `name : {{ ... }}`, without address or attributes)"
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

    fn output_section(&mut self, name: String, line: usize) -> Result<OutputDesc, ScriptError> {
        if name == "." || name == "/DISCARD/" {
            return error(line, format!("'{name}' is not a valid output section name"));
        }
        self.expect("{")?;
        let mut items = Vec::new();
        while !self.eat("}") {
            let iline = self.line();
            let w = self.word("an input section list or an assignment")?;
            if self.is("=") || self.is("+=") {
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
            Some(self.word("a memory region name")?)
        } else {
            None
        };
        self.eat(";");
        Ok(OutputDesc {
            name,
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
            "ORIGIN" => Expr::Origin(self.word("a memory region name")?),
            "LENGTH" => Expr::Length(self.word("a memory region name")?),
            _ => return error(line, format!("unsupported function '{w}' in expression")),
        };
        self.expect(")")?;
        Ok(e)
    }
}

/// Evaluates an expression made only of numbers.
fn const_eval(e: &Expr) -> Option<i64> {
    match e {
        Expr::Num(n) => Some(*n),
        Expr::Add(a, b) => const_eval(a)?.checked_add(const_eval(b)?),
        Expr::Sub(a, b) => const_eval(a)?.checked_sub(const_eval(b)?),
        _ => None,
    }
}

pub fn parse(src: &str) -> Result<Script, ScriptError> {
    let toks = lex(src)?;
    let script = Parser { toks, pos: 0 }.script()?;
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
/// `.data` and `.bss`, each aligned to its most-aligned input section.
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
                length: 128 << 20
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
        parse("OUTPUT_ARCH( \"riscv\" ) OUTPUT_FORMAT(\"elf32-littleriscv\")").unwrap();
        assert!(err("OUTPUT_ARCH(i386)").contains("only riscv"));
        assert!(err("OUTPUT_FORMAT(\"elf64-littleriscv\")").contains("elf32-littleriscv"));
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
                "without address or attributes",
            ),
            ("SECTIONS { .text : { *(.text) } > ROM }", "unknown region 'ROM'"),
            (
                "SECTIONS { .text : { *(.text) } AT> ROM }",
                "unsupported syntax after 'AT'",
            ),
            (
                "SECTIONS { /DISCARD/ : { *(.comment) } }",
                "/DISCARD/ is not supported",
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
