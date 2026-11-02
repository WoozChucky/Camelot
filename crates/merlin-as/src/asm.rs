//! The assembler driver: walks statements, keeps sections and symbols, runs
//! directives, and asks the encoder for instruction bytes.
//!
//! It is single pass. Every reference to a label becomes a relocation (the
//! linker resolves them, even within one section), so no instruction's size
//! ever depends on a label's address. `.equ`/`.set` constants must be defined
//! before use.

use crate::ast::*;
use crate::reloc::{Emit, RelocKind, Target};
use crate::{encode, Error};
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SectionKind {
    /// Code: allocated + executable.
    Text,
    /// Writable data.
    Data,
    /// Read-only data.
    Rodata,
    /// Zero-initialised, no file contents.
    Bss,
}

#[derive(Debug)]
pub struct Section {
    pub name: String,
    pub kind: SectionKind,
    /// Contents. For `.bss` this stays empty and `size` grows instead.
    pub data: Vec<u8>,
    pub size: u32,
    pub align: u32,
    pub relocs: Vec<Reloc>,
}

#[derive(Debug, Clone)]
pub struct Reloc {
    pub offset: u32,
    pub kind: RelocKind,
    pub symbol: String,
    pub addend: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymType {
    NoType,
    Func,
    Object,
}

#[derive(Debug, Clone)]
pub struct Symbol {
    pub name: String,
    /// Index into `sections`, or None if undefined (or an absolute constant).
    pub section: Option<usize>,
    pub value: u32,
    pub global: bool,
    pub defined: bool,
    /// Set for `.equ`/`.set` constants.
    pub constant: Option<i64>,
    pub ty: SymType,
    pub size: u32,
}

/// The assembled result, ready for [`crate::elf::write`].
#[derive(Debug)]
pub struct Object {
    pub sections: Vec<Section>,
    pub symbols: Vec<Symbol>,
}

pub struct Assembler {
    sections: Vec<Section>,
    current: usize,
    symbols: Vec<Symbol>,
    by_name: HashMap<String, usize>,
    /// How many times each numeric label (`1:`) has been defined so far.
    local_counts: HashMap<u32, u32>,
    hidden_labels: u32,
    errors: Vec<Error>,
    line: usize,
}

/// What the encoder can ask the assembler about operands.
pub struct Ctx<'a> {
    asm: &'a Assembler,
}

impl Ctx<'_> {
    /// Evaluates an expression made only of numbers and `.equ` constants.
    /// Returns None if it mentions a label or a `%modifier`.
    pub fn constant(&self, e: &Expr) -> Option<i64> {
        self.asm.eval(e)
    }

    /// Splits `sym`, `sym + c` or `sym - c` into a resolved symbol name and addend.
    /// Numeric label references (`1b`, `1f`) come back as their internal names.
    /// Returns None for anything else (including plain constants).
    pub fn symbol(&self, e: &Expr) -> Option<(String, i64)> {
        match e {
            Expr::Sym(name) if self.asm.eval(e).is_none() => Some((self.asm.resolve_name(name), 0)),
            // `c + sym`
            Expr::Bin(BinOp::Add, a, b) if self.asm.eval(a).is_some() => {
                let (name, addend) = self.symbol(b)?;
                Some((name, addend + self.asm.eval(a)?))
            }
            // `sym + c`, `sym - c`
            Expr::Bin(op @ (BinOp::Add | BinOp::Sub), a, b) => {
                let (name, addend) = self.symbol(a)?;
                let c = self.asm.eval(b)?;
                Some((name, if *op == BinOp::Add { addend + c } else { addend - c }))
            }
            _ => None,
        }
    }

    /// A CSR operand: a name like `mstatus` or a number like `0x300`.
    pub fn csr(&self, e: &Expr) -> Option<u16> {
        if let Expr::Sym(name) = e {
            if let Some(addr) = excalibur::csr::parse(name) {
                return Some(addr);
            }
        }
        let v = self.constant(e)?;
        (0..4096).contains(&v).then_some(v as u16)
    }
}

impl Assembler {
    pub fn new() -> Self {
        let mut a = Assembler {
            sections: Vec::new(),
            current: 0,
            symbols: Vec::new(),
            by_name: HashMap::new(),
            local_counts: HashMap::new(),
            hidden_labels: 0,
            errors: Vec::new(),
            line: 0,
        };
        a.switch_section(".text");
        a
    }

    fn error(&mut self, msg: impl Into<String>) {
        self.errors.push(Error::new(self.line, msg));
    }

    pub fn run(mut self, stmts: &[Stmt]) -> Result<Object, Vec<Error>> {
        for s in stmts {
            self.line = s.line;
            match &s.kind {
                StmtKind::Label(name) => self.label(name),
                StmtKind::Directive(name, args) => self.directive(name, args),
                StmtKind::Instruction(m, ops) => self.instruction(m, ops),
            }
        }
        self.finish()
    }

    fn finish(mut self) -> Result<Object, Vec<Error>> {
        for s in &self.symbols {
            if !s.defined && s.name.starts_with(".L") {
                let shown = s
                    .name
                    .split('$')
                    .next()
                    .unwrap_or(&s.name)
                    .trim_start_matches(".L");
                self.errors
                    .push(Error::new(0, format!("undefined local label '{shown}'")));
            }
        }
        if !self.errors.is_empty() {
            self.errors.sort_by_key(|e| e.line);
            return Err(self.errors);
        }
        Ok(Object {
            sections: self.sections,
            symbols: self.symbols,
        })
    }

    // ----- symbols -----

    /// Maps `1b`/`1f` to the internal name of that numeric label instance.
    fn resolve_name(&self, name: &str) -> String {
        let (digits, dir) = name.split_at(name.len().saturating_sub(1));
        if let (Ok(n), "b" | "f") = (digits.parse::<u32>(), dir) {
            let count = self.local_counts.get(&n).copied().unwrap_or(0);
            let k = if dir == "b" { count.wrapping_sub(1) } else { count };
            return format!(".L{n}${k}");
        }
        name.to_string()
    }

    fn symbol_index(&mut self, name: &str) -> usize {
        if let Some(&i) = self.by_name.get(name) {
            return i;
        }
        self.symbols.push(Symbol {
            name: name.to_string(),
            section: None,
            value: 0,
            global: false,
            defined: false,
            constant: None,
            ty: SymType::NoType,
            size: 0,
        });
        self.by_name.insert(name.to_string(), self.symbols.len() - 1);
        self.symbols.len() - 1
    }

    fn define(&mut self, name: &str, section: usize, value: u32) {
        let i = self.symbol_index(name);
        if self.symbols[i].defined {
            self.error(format!("symbol '{name}' is already defined"));
            return;
        }
        let s = &mut self.symbols[i];
        s.section = Some(section);
        s.value = value;
        s.defined = true;
    }

    fn label(&mut self, name: &str) {
        let name = match name.parse::<u32>() {
            Ok(n) => {
                let k = self.local_counts.entry(n).or_insert(0);
                let internal = format!(".L{n}${k}");
                *k += 1;
                internal
            }
            Err(_) => name.to_string(),
        };
        let off = self.sections[self.current].size;
        self.define(&name, self.current, off);
    }

    fn eval(&self, e: &Expr) -> Option<i64> {
        Some(match e {
            Expr::Num(n) => *n,
            Expr::Sym(name) => self.symbols[*self.by_name.get(name)?].constant?,
            Expr::Neg(a) => self.eval(a)?.wrapping_neg(),
            Expr::Not(a) => !self.eval(a)?,
            Expr::Bin(op, a, b) => {
                let (a, b) = (self.eval(a)?, self.eval(b)?);
                match op {
                    BinOp::Add => a.wrapping_add(b),
                    BinOp::Sub => a.wrapping_sub(b),
                    BinOp::Mul => a.wrapping_mul(b),
                    BinOp::Div => a.checked_div(b)?,
                    BinOp::Rem => a.checked_rem(b)?,
                    BinOp::Shl => a.wrapping_shl(b as u32),
                    BinOp::Shr => a.wrapping_shr(b as u32),
                    BinOp::And => a & b,
                    BinOp::Or => a | b,
                    BinOp::Xor => a ^ b,
                }
            }
            Expr::Mod(..) => return None,
        })
    }

    // ----- sections and emission -----

    fn switch_section(&mut self, name: &str) {
        if let Some(i) = self.sections.iter().position(|s| s.name == name) {
            self.current = i;
            return;
        }
        let kind = if name == ".text" || name.starts_with(".text.") || name.starts_with(".init") {
            SectionKind::Text
        } else if name == ".bss" || name.starts_with(".bss.") || name.starts_with(".sbss") {
            SectionKind::Bss
        } else if name.starts_with(".rodata") || name.starts_with(".srodata") {
            SectionKind::Rodata
        } else {
            SectionKind::Data
        };
        let align = if kind == SectionKind::Text { 4 } else { 1 };
        self.sections.push(Section {
            name: name.to_string(),
            kind,
            data: Vec::new(),
            size: 0,
            align,
            relocs: Vec::new(),
        });
        self.current = self.sections.len() - 1;
    }

    fn emit_bytes(&mut self, bytes: &[u8]) {
        let sec = &mut self.sections[self.current];
        if sec.kind == SectionKind::Bss {
            if bytes.iter().any(|&b| b != 0) {
                self.error(format!("non-zero data in {}", self.sections[self.current].name));
                return;
            }
        } else {
            sec.data.extend_from_slice(bytes);
        }
        sec.size += bytes.len() as u32;
    }

    fn emit_zeros(&mut self, n: u32) {
        let sec = &mut self.sections[self.current];
        if sec.kind != SectionKind::Bss {
            sec.data.resize(sec.data.len() + n as usize, 0);
        }
        sec.size += n;
    }

    fn add_reloc(&mut self, offset: u32, kind: RelocKind, symbol: String, addend: i64) {
        self.symbol_index(&symbol);
        self.sections[self.current].relocs.push(Reloc {
            offset,
            kind,
            symbol,
            addend,
        });
    }

    fn align_to(&mut self, align: u32) {
        if align <= 1 {
            return;
        }
        let sec = &mut self.sections[self.current];
        sec.align = sec.align.max(align);
        let pad = (align - sec.size % align) % align;
        if sec.kind == SectionKind::Text && pad.is_multiple_of(4) && sec.size.is_multiple_of(4) {
            for _ in 0..pad / 4 {
                self.emit_bytes(&0x0000_0013u32.to_le_bytes()); // nop
            }
        } else {
            self.emit_zeros(pad);
        }
    }

    fn instruction(&mut self, mnemonic: &str, ops: &[Operand]) {
        let emits = match encode::encode(mnemonic, ops, &Ctx { asm: self }) {
            Ok(e) => e,
            Err(msg) => return self.error(msg),
        };
        if self.sections[self.current].kind == SectionKind::Bss {
            return self.error("instructions are not allowed in .bss");
        }
        if !self.sections[self.current].size.is_multiple_of(4) {
            return self.error("instruction is not 4-byte aligned (add .align 2)");
        }
        let base = self.sections[self.current].size;
        // Hidden labels on auipcs that a later %pcrel_lo in this expansion refers to.
        let mut auipc_labels: HashMap<usize, String> = HashMap::new();
        for (i, e) in emits.iter().enumerate() {
            if let Some(f) = &e.fixup {
                if let Target::Auipc(j) = f.target {
                    if j >= i {
                        return self.error(format!(
                            "internal encoder error: '{mnemonic}' refers to a later auipc"
                        ));
                    }
                    if let std::collections::hash_map::Entry::Vacant(e) = auipc_labels.entry(j) {
                        let name = format!(".Lpcrel_hi{}", self.hidden_labels);
                        self.hidden_labels += 1;
                        self.define(&name, self.current, base + 4 * j as u32);
                        e.insert(name);
                    }
                }
            }
        }
        for (i, Emit { word, fixup }) in emits.into_iter().enumerate() {
            let offset = base + 4 * i as u32;
            if let Some(f) = fixup {
                let (symbol, addend) = match f.target {
                    Target::Symbol { name, addend } => (name, addend),
                    Target::Auipc(j) => (auipc_labels[&j].clone(), 0),
                };
                self.add_reloc(offset, f.kind, symbol, addend);
            }
            self.emit_bytes(&word.to_le_bytes());
        }
    }

    // ----- directives -----

    fn arg_const(&mut self, args: &[DirArg], i: usize, what: &str) -> Option<i64> {
        match args.get(i) {
            Some(DirArg::Expr(e)) => match self.eval(e) {
                Some(v) => Some(v),
                None => {
                    self.error(format!("{what}: expected a constant expression"));
                    None
                }
            },
            _ => {
                self.error(format!("{what}: missing argument"));
                None
            }
        }
    }

    fn arg_name(&mut self, args: &[DirArg], i: usize, what: &str) -> Option<String> {
        match args.get(i) {
            Some(DirArg::Expr(Expr::Sym(n))) => Some(n.clone()),
            Some(DirArg::Str(s)) => Some(String::from_utf8_lossy(s).into_owned()),
            _ => {
                self.error(format!("{what}: expected a name"));
                None
            }
        }
    }

    fn data(&mut self, args: &[DirArg], size: u32) {
        for a in args {
            let DirArg::Expr(e) = a else {
                self.error("expected a number, not a string");
                continue;
            };
            if let Some(v) = self.eval(e) {
                let bytes = (v as u64).to_le_bytes();
                self.emit_bytes(&bytes[..size as usize]);
                continue;
            }
            let ctx = Ctx { asm: self };
            match (size, ctx.symbol(e)) {
                (4, Some((name, addend))) => {
                    let off = self.sections[self.current].size;
                    self.add_reloc(off, RelocKind::Abs32, name, addend);
                    self.emit_bytes(&[0; 4]);
                }
                (4, None) => self.error("expected a constant or symbol(+offset)"),
                _ => self.error("only .word can hold a symbol address"),
            }
        }
    }

    fn directive(&mut self, name: &str, args: &[DirArg]) {
        match name {
            ".text" | ".data" | ".rodata" | ".bss" => self.switch_section(name),
            ".section" => {
                if let Some(n) = self.arg_name(args, 0, ".section") {
                    self.switch_section(&n);
                }
            }
            ".globl" | ".global" => {
                for i in 0..args.len() {
                    if let Some(n) = self.arg_name(args, i, name) {
                        let s = self.symbol_index(&n);
                        self.symbols[s].global = true;
                    }
                }
            }
            ".local" => {}
            ".equ" | ".set" => {
                let (Some(n), Some(v)) = (self.arg_name(args, 0, name), self.arg_const(args, 1, name)) else {
                    return;
                };
                let s = self.symbol_index(&n);
                let sym = &mut self.symbols[s];
                if sym.defined && sym.constant.is_none() {
                    return self.error(format!("'{n}' is already a label"));
                }
                sym.defined = true;
                sym.constant = Some(v);
            }
            ".byte" => self.data(args, 1),
            ".half" | ".short" | ".2byte" => self.data(args, 2),
            ".word" | ".long" | ".4byte" => self.data(args, 4),
            ".ascii" | ".asciz" | ".string" => {
                for a in args {
                    match a {
                        DirArg::Str(s) => {
                            let s = s.clone();
                            self.emit_bytes(&s);
                            if name != ".ascii" {
                                self.emit_bytes(&[0]);
                            }
                        }
                        _ => self.error(format!("{name}: expected a string")),
                    }
                }
            }
            ".zero" | ".space" | ".skip" => {
                if let Some(n) = self.arg_const(args, 0, name) {
                    if args.len() > 1 {
                        let fill = self.arg_const(args, 1, name).unwrap_or(0) as u8;
                        self.emit_bytes(&vec![fill; n as usize]);
                    } else {
                        self.emit_zeros(n as u32);
                    }
                }
            }
            // RISC-V `.align n` means 2^n bytes, same as `.p2align`.
            ".align" | ".p2align" => {
                if let Some(n) = self.arg_const(args, 0, name) {
                    if !(0..=16).contains(&n) {
                        return self.error(format!("{name}: power of two must be 0..16"));
                    }
                    self.align_to(1 << n);
                }
            }
            ".balign" => {
                if let Some(n) = self.arg_const(args, 0, name) {
                    if n <= 0 || n & (n - 1) != 0 {
                        return self.error(".balign: alignment must be a power of two");
                    }
                    self.align_to(n as u32);
                }
            }
            ".type" => {
                if let Some(n) = self.arg_name(args, 0, ".type") {
                    let ty = match args.get(1) {
                        Some(DirArg::Expr(Expr::Sym(t))) if t.contains("function") => SymType::Func,
                        Some(DirArg::Expr(Expr::Sym(t))) if t.contains("object") => SymType::Object,
                        _ => SymType::NoType,
                    };
                    let s = self.symbol_index(&n);
                    self.symbols[s].ty = ty;
                }
            }
            ".size" => {
                // `.size sym, . - sym` needs `.`; accept a constant only for now.
                if let (Some(n), Some(DirArg::Expr(e))) = (self.arg_name(args, 0, ".size"), args.get(1)) {
                    if let Some(v) = self.eval(e) {
                        let s = self.symbol_index(&n);
                        self.symbols[s].size = v as u32;
                    }
                }
            }
            // Accepted and ignored: metadata that does not change the output.
            ".file" | ".ident" | ".option" | ".attribute" | ".addrsig" | ".cfi_startproc"
            | ".cfi_endproc" => {}
            _ => self.error(format!("unknown directive '{name}'")),
        }
    }
}

impl Default for Assembler {
    fn default() -> Self {
        Self::new()
    }
}
