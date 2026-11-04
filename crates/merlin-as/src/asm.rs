//! The assembler driver: walks statements, keeps sections and symbols, runs
//! directives, and asks the encoder for instruction bytes.
//!
//! It is single pass: no instruction's size ever depends on a label's address,
//! so every label reference is first recorded as a fixup (a would-be
//! relocation, with its source line) and the immediate is left zero.
//! `.equ`/`.set` constants must be defined before use.
//!
//! `.`, the location counter, is the address of the instruction (its first
//! word, for a pseudo-instruction) or data word it appears in. Each use
//! becomes a hidden local label there, so `j .` and `beq a0, a1, .-8`
//! resolve like any local label.
//!
//! When the whole file has been read, `Assembler::finish` does what clang's
//! assembler does with `-mno-relax`: a pc-relative fixup whose label is local
//! (not `.globl`) and defined in the same section as the instruction is
//! resolved on the spot, since the distance can't change at link time. That
//! covers branches (`R_RISCV_BRANCH`), `jal`/`j` (`R_RISCV_JAL`), `call`/`tail`
//! (`R_RISCV_CALL_PLT`, both words of the pair) and `auipc` + `%pcrel_lo`
//! pairs (`la`, `lla`, `lw rd, sym`, `sw rs, sym, rt`, explicit
//! `%pcrel_hi`/`%pcrel_lo`); the immediate is patched and no relocation is
//! written. An out-of-range result is an error on the instruction's line.
//! Everything else stays a relocation for the linker: references to global or
//! undefined symbols or to labels in other sections, and absolute ones
//! (`%hi`, `%lo`, `.word sym`, `.dword sym`), which depend on where the
//! section is loaded.
//!
//! Unlike clang, merlin-as never rewrites a branch it cannot resolve (to a
//! global, or out of range) into an inverted branch over a `jal`: the branch
//! keeps its `R_RISCV_BRANCH`, or is an error when out of range (spec section 10).
//!
//! Also as in clang, `.L` labels (including numeric labels like `1:` and the
//! hidden `.Lpcrel_hi` labels on `auipc`s) only go into the symbol table when
//! a relocation still refers to them.
//!
//! Debug information: `.debug_*` sections are not loaded (no `SHF_ALLOC`) and
//! hold data only. A `.word` or `.dword` there that names a `.L` label (the
//! 8-byte addresses of RV64's DWARF) becomes a relocation against the label's
//! section symbol, plus the label's offset, so that compiler-made labels
//! (`.Lfunc_begin0`) stay out of the symbol table and out of the linked
//! program's. `.file` and `.loc` become the `.debug_line` line table
//! ([`crate::dwarf`]).

use crate::ast::*;
use crate::reloc::{Emit, RelocKind, Target};
use crate::{dwarf, encode, Error};
use excalibur::Xlen;
use std::collections::{HashMap, HashSet};

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
    /// Debug information (`.debug_*`): data that is not loaded.
    Debug,
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
    /// Source line of the instruction or directive, for errors found later.
    pub line: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymType {
    NoType,
    Func,
    Object,
    /// A section's own symbol (`STT_SECTION`), at its start, for relocations
    /// from debug information. Named by [`section_symbol_name`].
    Section,
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
    /// The width it was assembled for: the ELF writer makes an ELF32 object
    /// for RV32 and an ELF64 one for RV64.
    pub xlen: Xlen,
}

pub struct Assembler {
    /// RV32 or RV64, for the whole file (`-march=`).
    xlen: Xlen,
    sections: Vec<Section>,
    current: usize,
    symbols: Vec<Symbol>,
    by_name: HashMap<String, usize>,
    /// How many times each numeric label (`1:`) has been defined so far.
    local_counts: HashMap<u32, u32>,
    hidden_labels: u32,
    /// How many `.Ldot` labels (see [`Assembler::dot_label`]) have been made.
    dot_labels: u32,
    /// `%pcrel_lo(label)` relocations to check once every label is known:
    /// (section, label, source line). See [`Assembler::check_pcrel_lo`].
    pcrel_lo: Vec<(usize, String, usize)>,
    /// Names given to `.local`, so that a later `.comm` allocates them in
    /// `.bss` (see the `.comm` directive).
    locals: HashSet<String>,
    /// Line information: `.file 0` (the compilation directory and the main
    /// file) and the other `.file`s by number.
    file0: Option<dwarf::File>,
    files: Vec<Option<dwarf::File>>,
    /// The last `.loc`, until an instruction (or data) takes it.
    loc: Option<dwarf::Loc>,
    /// `.loc`'s `is_stmt`, which carries over from one to the next.
    is_stmt: bool,
    /// Line table rows: (section, offset, where it comes from).
    rows: Vec<(usize, u32, dwarf::Loc)>,
    /// The source line of the first `.loc`, for an error about the line table.
    first_loc_line: usize,
    errors: Vec<Error>,
    line: usize,
}

/// What the encoder can ask the assembler about operands.
pub struct Ctx<'a> {
    asm: &'a Assembler,
}

impl Ctx<'_> {
    /// The width being assembled for. The encoder asks it to choose between
    /// the RV32 and RV64 forms of an instruction (shift amounts, `li`) and
    /// to refuse the instructions of the other width.
    pub fn xlen(&self) -> Xlen {
        self.asm.xlen
    }

    /// Evaluates an expression made only of numbers and `.equ` constants.
    /// Returns None if it mentions a label or a `%modifier`.
    pub fn constant(&self, e: &Expr) -> Option<i64> {
        self.asm.eval(e)
    }

    /// Splits `sym`, `sym + c` or `sym - c` into a resolved symbol name and addend.
    /// Numeric label references (`1b`, `1f`) come back as their internal names.
    /// Returns None for anything else (including plain constants).
    ///
    /// The addend wraps around in 64 bits, as every other sum of constants
    /// does (`eval`): with 64-bit `.equ` values, `sym + BIG + BIG` must not
    /// overflow.
    pub fn symbol(&self, e: &Expr) -> Option<(String, i64)> {
        match e {
            Expr::Sym(name) if self.asm.eval(e).is_none() => Some((self.asm.resolve_name(name), 0)),
            // `c + sym`
            Expr::Bin(BinOp::Add, a, b) if self.asm.eval(a).is_some() => {
                let (name, addend) = self.symbol(b)?;
                Some((name, addend.wrapping_add(self.asm.eval(a)?)))
            }
            // `sym + c`, `sym - c`
            Expr::Bin(op @ (BinOp::Add | BinOp::Sub), a, b) => {
                let (name, addend) = self.symbol(a)?;
                let c = self.asm.eval(b)?;
                let addend = if *op == BinOp::Add {
                    addend.wrapping_add(c)
                } else {
                    addend.wrapping_sub(c)
                };
                Some((name, addend))
            }
            _ => None,
        }
    }

    /// True if `name` (as [`Ctx::symbol`] returns it) is neither defined yet
    /// nor `.`: a label further down, a constant whose `.equ` comes later,
    /// or an external symbol.
    pub fn undefined(&self, name: &str) -> bool {
        name != DOT
            && !self
                .asm
                .by_name
                .get(name)
                .is_some_and(|&i| self.asm.symbols[i].defined)
    }

    /// A CSR operand: a name like `mstatus` or a number like `0x300`.
    ///
    /// Names are those of the width being assembled
    /// ([`excalibur::csr::rv32_only`]): the high halves of 64-bit registers
    /// (`cycleh`, `mstatush`, ...) and the odd `pmpcfg`s exist only on RV32,
    /// where a CSR holds 32 bits, so on RV64 they are no CSR name. A number is
    /// any 12-bit address on either width, as LLVM takes it.
    pub fn csr(&self, e: &Expr) -> Option<u16> {
        if let Expr::Sym(name) = e {
            if let Some(addr) = excalibur::csr::parse(name, self.asm.xlen) {
                return Some(addr);
            }
        }
        let v = self.constant(e)?;
        (0..4096).contains(&v).then_some(v as u16)
    }
}

impl Assembler {
    /// An assembler for `xlen` (RV32 or RV64) with an empty `.text` section.
    pub fn new(xlen: Xlen) -> Self {
        let mut a = Assembler {
            xlen,
            sections: Vec::new(),
            current: 0,
            symbols: Vec::new(),
            by_name: HashMap::new(),
            local_counts: HashMap::new(),
            hidden_labels: 0,
            dot_labels: 0,
            pcrel_lo: Vec::new(),
            locals: HashSet::new(),
            file0: None,
            files: Vec::new(),
            loc: None,
            is_stmt: true,
            rows: Vec::new(),
            first_loc_line: 0,
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
        self.check_pcrel_lo();
        for s in &self.symbols {
            if !s.defined && s.name.starts_with(".L") {
                let shown = shown_name(&s.name);
                self.errors
                    .push(Error::new(0, format!("undefined local label '{shown}'")));
            }
        }
        self.resolve_local_fixups();
        self.line_table();
        self.debug_relocs_to_sections();
        if !self.errors.is_empty() {
            self.errors.sort_by_key(|e| e.line);
            return Err(self.errors);
        }
        // `.L` labels are assembler-internal: keep only those a relocation needs.
        let referenced: HashSet<&str> = self
            .sections
            .iter()
            .flat_map(|s| &s.relocs)
            .map(|r| r.symbol.as_str())
            .collect();
        let symbols = self
            .symbols
            .iter()
            .filter(|s| !s.name.starts_with(".L") || referenced.contains(s.name.as_str()))
            .cloned()
            .collect();
        Ok(Object {
            sections: self.sections,
            symbols,
            xlen: self.xlen,
        })
    }

    /// The pc-relative distance from `offset` in `section` to a fixup's
    /// target, if the target is a local label defined in that same section
    /// (the case clang resolves at assembly time).
    fn local_distance(&self, section: usize, offset: u32, symbol: &str, addend: i64) -> Option<i64> {
        let s = &self.symbols[*self.by_name.get(symbol)?];
        let local = s.defined && !s.global && s.constant.is_none() && s.section == Some(section);
        local.then(|| (s.value as i64).wrapping_add(addend).wrapping_sub(offset as i64))
    }

    /// Resolves the fixups clang resolves with `-mno-relax` (see the module
    /// docs) and removes their relocations.
    ///
    /// Two passes, because the low half of a pc-relative pair does not name
    /// the symbol: its relocation points at the label on its `auipc`. So the
    /// first pass resolves branches, jumps, calls and `auipc`s, remembering the
    /// distance each resolved `auipc` got; the second gives each `%pcrel_lo`
    /// the low 12 bits of its own `auipc`'s distance.
    fn resolve_local_fixups(&mut self) {
        use RelocKind::*;
        // (section, offset of a resolved auipc) -> distance from it to its target.
        let mut auipc_distance: HashMap<(usize, u32), i64> = HashMap::new();
        for sec in 0..self.sections.len() {
            let relocs = std::mem::take(&mut self.sections[sec].relocs);
            let mut kept = Vec::new();
            for r in relocs {
                let distance = match r.kind {
                    Branch | Jal | CallPlt | PcrelHi20 => {
                        self.local_distance(sec, r.offset, &r.symbol, r.addend)
                    }
                    _ => None,
                };
                let Some(distance) = distance else {
                    kept.push(r);
                    continue;
                };
                if r.kind == PcrelHi20 {
                    auipc_distance.insert((sec, r.offset), distance);
                }
                if let Err(msg) = self.patch(sec, r.offset, r.kind, distance) {
                    let shown = shown_name(&r.symbol);
                    self.errors.push(Error::new(r.line, format!("'{shown}': {msg}")));
                }
            }
            self.sections[sec].relocs = kept;
        }
        for sec in 0..self.sections.len() {
            let relocs = std::mem::take(&mut self.sections[sec].relocs);
            let mut kept = Vec::new();
            for r in relocs {
                // The label of a %pcrel_lo is on its auipc, in this section
                // (check_pcrel_lo made sure of that).
                let auipc = match r.kind {
                    PcrelLo12I | PcrelLo12S => self
                        .by_name
                        .get(&r.symbol)
                        .map(|&i| &self.symbols[i])
                        .filter(|s| s.defined && s.section == Some(sec))
                        .map(|s| s.value),
                    _ => None,
                };
                match auipc.and_then(|a| auipc_distance.get(&(sec, a))) {
                    Some(&distance) => {
                        // Can't fail: the distance fit in its auipc.
                        let _ = self.patch(sec, r.offset, r.kind, distance);
                    }
                    None => kept.push(r),
                }
            }
            self.sections[sec].relocs = kept;
        }
    }

    /// Writes a resolved pc-relative `distance` into the instruction(s) at
    /// `offset`, as the linker would for a relocation of this kind. The
    /// encoder left those immediate bits zero, so they are simply OR-ed in.
    fn patch(&mut self, section: usize, offset: u32, kind: RelocKind, distance: i64) -> Result<(), String> {
        use RelocKind::*;
        let (bits, next) = match kind {
            Branch => {
                check_jump(distance, 13, "branch")?;
                (encode::b_imm(distance as i32), None)
            }
            Jal => {
                check_jump(distance, 21, "jump")?;
                (encode::j_imm(distance as i32), None)
            }
            // A distance within a section always fits auipc's ±2 GiB reach.
            PcrelHi20 => (encode::u_imm(encode::hi_lo(distance as i32).0), None),
            PcrelLo12I => (encode::i_imm(encode::hi_lo(distance as i32).1), None),
            PcrelLo12S => (encode::s_imm(encode::hi_lo(distance as i32).1), None),
            // auipc + jalr: the jalr's 12 bits complete the auipc's 20.
            CallPlt => {
                let (hi, lo) = encode::hi_lo(distance as i32);
                (encode::u_imm(hi), Some(encode::i_imm(lo)))
            }
            Abs32 | Abs64 | Hi20 | Lo12I | Lo12S => {
                unreachable!("absolute relocations are never resolved")
            }
        };
        let data = &mut self.sections[section].data;
        let at = offset as usize;
        for (at, bits) in [(at, Some(bits)), (at + 4, next)] {
            if let Some(bits) = bits {
                let w = u32::from_le_bytes(data[at..at + 4].try_into().unwrap()) | bits;
                data[at..at + 4].copy_from_slice(&w.to_le_bytes());
            }
        }
        Ok(())
    }

    /// The low half of an explicit pc-relative pair (`addi a0, a0, %pcrel_lo(1b)`)
    /// names the label on its `auipc`, not the final symbol: the linker reads the
    /// `R_RISCV_PCREL_HI20` at that label to find the address. A label in another
    /// section, or one with no `%pcrel_hi` auipc on it, would link to garbage, so
    /// it is an error here (llvm-mc: "could not find corresponding %pcrel_hi").
    /// The encoder can't check this itself: the label may be defined later.
    fn check_pcrel_lo(&mut self) {
        for (section, name, line) in std::mem::take(&mut self.pcrel_lo) {
            let sym = self.by_name.get(&name).map(|&i| &self.symbols[i]);
            if sym.is_some_and(|s| !s.defined && s.name.starts_with(".L")) {
                continue; // reported below as an undefined local label
            }
            let paired = sym.is_some_and(|s| {
                s.defined
                    && s.section == Some(section)
                    && self.sections[section]
                        .relocs
                        .iter()
                        .any(|r| r.offset == s.value && r.kind == RelocKind::PcrelHi20)
            });
            if !paired {
                let shown = shown_name(&name);
                self.errors.push(Error::new(
                    line,
                    format!("%pcrel_lo({shown}): no auipc with %pcrel_hi at that label in this section"),
                ));
            }
        }
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

    /// Defines a hidden label at `offset` in the current section for a
    /// reference to `.`, the location counter, and returns its name. Like
    /// clang's `.L0` temporaries, it then resolves like any local label: a
    /// `j .` is patched on the spot, a `.word .` keeps an `R_RISCV_32`
    /// against it.
    fn dot_label(&mut self, offset: u32) -> String {
        let name = format!("{DOT_LABEL}{}", self.dot_labels);
        self.dot_labels += 1;
        self.define(&name, self.current, offset);
        name
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
                    // Logical, not arithmetic, as in LLVM (MCAsmInfo's
                    // `UseLogicalShr`, on for ELF targets): a negative value
                    // shifts zeros in (`-16 >> 2` is 0x3ffffffffffffffc).
                    BinOp::Shr => (a as u64).wrapping_shr(b as u32) as i64,
                    BinOp::And => a & b,
                    BinOp::Or => a | b,
                    BinOp::Xor => a ^ b,
                }
            }
            Expr::Mod(..) => return None,
        })
    }

    /// The value of an expression that may hold labels (`.size f, . - f`,
    /// `.word .Lfunc_end0 - .Lfunc_begin0`). Besides constants, `.` and the
    /// labels defined so far count as their offsets in their sections, so
    /// the difference of two in one section is the distance between them:
    /// merlin-as never moves code once it is placed (no relaxation), so the
    /// distance is final. Only `+`, `-` and negation combine such terms;
    /// anything else must be constant.
    ///
    /// An offset on its own is not a number: `.size f, f` or `.size f, . + f`
    /// would depend on where the section ends up. Like LLVM ("Size
    /// expression must be absolute"), the result is accepted only when the
    /// offsets cancel out, each added one matched by a subtracted one in the
    /// same section.
    fn eval_relative(&self, e: &Expr) -> Option<i64> {
        let mut terms: HashMap<usize, i64> = HashMap::new();
        let v = self.relative_terms(e, 1, &mut terms)?;
        terms.values().all(|&n| n == 0).then_some(v)
    }

    /// `eval_relative`'s worker: the value of `e` times `sign`, counting in
    /// `terms` how many times each section's offsets were added (+1) or
    /// subtracted (-1).
    fn relative_terms(&self, e: &Expr, sign: i64, terms: &mut HashMap<usize, i64>) -> Option<i64> {
        match e {
            Expr::Sym(name) if name == DOT => {
                *terms.entry(self.current).or_default() += sign;
                Some(sign.wrapping_mul(self.sections[self.current].size as i64))
            }
            Expr::Sym(name) => {
                let s = &self.symbols[*self.by_name.get(&self.resolve_name(name))?];
                match (s.constant, s.section) {
                    (Some(c), _) => Some(sign.wrapping_mul(c)),
                    (None, Some(sec)) if s.defined => {
                        *terms.entry(sec).or_default() += sign;
                        Some(sign.wrapping_mul(s.value as i64))
                    }
                    _ => None,
                }
            }
            Expr::Bin(op @ (BinOp::Add | BinOp::Sub), a, b) => {
                let a = self.relative_terms(a, sign, terms)?;
                let b_sign = if *op == BinOp::Add { sign } else { -sign };
                Some(a.wrapping_add(self.relative_terms(b, b_sign, terms)?))
            }
            Expr::Neg(a) => self.relative_terms(a, -sign, terms),
            _ => Some(sign.wrapping_mul(self.eval(e)?)),
        }
    }

    // ----- sections and emission -----

    fn switch_section(&mut self, name: &str) {
        if let Some(i) = self.sections.iter().position(|s| s.name == name) {
            self.current = i;
            return;
        }
        let kind = if name.starts_with(".debug_") {
            SectionKind::Debug
        } else if name == ".text" || name.starts_with(".text.") || name.starts_with(".init") {
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
            line: self.line,
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
        match encode::encode(mnemonic, ops, &Ctx { asm: self }) {
            Ok(emits) => self.place(mnemonic, emits),
            Err(msg) => self.error(msg),
        }
    }

    /// Appends the encoder's words to the current section and turns their
    /// fixups into relocations.
    fn place(&mut self, mnemonic: &str, emits: Vec<Emit>) {
        match self.sections[self.current].kind {
            SectionKind::Bss => return self.error("instructions are not allowed in .bss"),
            SectionKind::Debug => {
                let name = self.sections[self.current].name.clone();
                return self.error(format!("instructions are not allowed in {name}"));
            }
            _ => {}
        }
        if !self.sections[self.current].size.is_multiple_of(4) {
            return self.error("instruction is not 4-byte aligned (add .align 2)");
        }
        self.take_loc();
        let base = self.sections[self.current].size;
        // `.` in any operand is the address of the instruction's first word.
        let mut dot = None;
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
                    Target::Symbol { mut name, addend } => {
                        if name == DOT {
                            name = dot.get_or_insert_with(|| self.dot_label(base)).clone();
                        }
                        if matches!(f.kind, RelocKind::PcrelLo12I | RelocKind::PcrelLo12S) {
                            self.pcrel_lo.push((self.current, name.clone(), self.line));
                        }
                        (name, addend)
                    }
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

    /// `.byte`, `.half`, `.word` and `.dword` (and their other names): each
    /// argument is `size` bytes, little-endian.
    ///
    /// A number must fit in those bytes as a signed or an unsigned value,
    /// as LLVM requires (`isIntN || isUIntN`): `.byte 255` and `.byte -1`
    /// are the same byte, `.byte 256` is an error, not a silent 0. A
    /// `.dword` takes any 64-bit value.
    ///
    /// A label's address is only known at link time, so `.word sym` and
    /// `.dword sym` leave zeros and a relocation (`R_RISCV_32`, `R_RISCV_64`)
    /// for the linker to fill. The width of the data, not of the machine,
    /// picks it: LLVM writes `R_RISCV_64` for a `.dword` on RV32 too, and
    /// `R_RISCV_32` for a `.word` on RV64. The psABI has no 1- or 2-byte
    /// absolute relocation.
    fn data(&mut self, args: &[DirArg], size: u32) {
        if !args.is_empty() {
            self.take_loc();
        }
        for a in args {
            let DirArg::Expr(e) = a else {
                self.error("expected a number, not a string");
                continue;
            };
            if let Some(v) = self.eval(e).or_else(|| self.eval_relative(e)) {
                if !fits_in_bytes(v, size) {
                    let unit = if size == 1 { "byte" } else { "bytes" };
                    self.error(format!("data value {v} does not fit in {size} {unit}"));
                    continue;
                }
                let bytes = (v as u64).to_le_bytes();
                self.emit_bytes(&bytes[..size as usize]);
                continue;
            }
            let kind = match size {
                4 => RelocKind::Abs32,
                8 => RelocKind::Abs64,
                _ => {
                    self.error("only .word and .dword can hold a symbol address");
                    continue;
                }
            };
            let ctx = Ctx { asm: self };
            let Some((mut name, addend)) = ctx.symbol(e) else {
                self.error("expected a constant or symbol(+offset)");
                continue;
            };
            let off = self.sections[self.current].size;
            if name == DOT {
                name = self.dot_label(off);
            }
            self.add_reloc(off, kind, name, addend);
            self.emit_bytes(&[0; 8][..size as usize]);
        }
    }

    /// `.set name, label + offset` (or `name = label + offset`): `name`
    /// becomes another label at that address, in the label's section. clang
    /// writes these when it merges a file's `static` variables into one
    /// block (`.L_MergedGlobals`) and names each part of it. The label must
    /// be defined already, so the address is known.
    fn alias(&mut self, directive: &str, name: &str, e: &Expr) {
        let ctx = Ctx { asm: self };
        let Some((target, addend)) = ctx.symbol(e) else {
            return self.error(format!("{directive}: expected a constant or label(+offset)"));
        };
        let i = self.symbol_index(&target);
        let t = &self.symbols[i];
        let (Some(section), true, None) = (t.section, t.defined, t.constant) else {
            let shown = shown_name(&target).to_string();
            return self.error(format!(
                "{directive}: '{shown}' must be a label defined before this"
            ));
        };
        let value = t.value as i64 + addend;
        if !(0..=u32::MAX as i64).contains(&value) {
            return self.error(format!("{directive}: '{name}' would be outside its section"));
        }
        self.define(name, section, value as u32);
    }

    /// `.comm sym, size[, align]` for a symbol made `.local` first, or
    /// `.lcomm sym, size[, align]`: reserves `size` zero bytes for `sym` in
    /// `.bss`, aligned to `align` bytes, as clang's assembler does. This is
    /// how clang writes a `static` variable without an initializer in its
    /// assembly output (`.local buf` then `.comm buf,100,1`). The current
    /// section stays what it was. A global `.comm` would be an
    /// `SHN_COMMON` symbol for the linker to merge, which merlin-ld doesn't
    /// do: it is an error (compile with `-fno-common`).
    fn common(&mut self, name: &str, args: &[DirArg]) {
        let Some(sym) = self.arg_name(args, 0, name) else {
            return;
        };
        if name == ".comm" && !self.locals.contains(&sym) {
            return self.error(format!(
                ".comm: '{sym}' is not .local; common symbols are not supported (use -fno-common)"
            ));
        }
        let Some(size) = self.arg_const(args, 1, name) else {
            return;
        };
        let align = if args.len() > 2 {
            match self.arg_const(args, 2, name) {
                Some(a) => a,
                None => return,
            }
        } else {
            1
        };
        if !(0..=u32::MAX as i64).contains(&size) {
            return self.error(format!("{name}: bad size {size}"));
        }
        if align <= 0 || align & (align - 1) != 0 || align > 1 << 16 {
            return self.error(format!("{name}: alignment must be a power of two up to 65536"));
        }
        let back = self.current;
        self.switch_section(".bss");
        self.align_to(align as u32);
        let off = self.sections[self.current].size;
        self.define(&sym, self.current, off);
        let i = self.symbol_index(&sym);
        self.symbols[i].size = size as u32;
        if self.symbols[i].ty == SymType::NoType {
            self.symbols[i].ty = SymType::Object;
        }
        self.emit_zeros(size as u32);
        self.current = back;
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
            ".local" => {
                for i in 0..args.len() {
                    if let Some(n) = self.arg_name(args, i, name) {
                        self.locals.insert(n);
                    }
                }
            }
            ".comm" | ".lcomm" => self.common(name, args),
            ".equ" | ".set" => {
                let Some(n) = self.arg_name(args, 0, name) else {
                    return;
                };
                if let Some(DirArg::Expr(e)) = args.get(1) {
                    if self.eval(e).is_none() {
                        return self.alias(name, &n, e);
                    }
                }
                let Some(v) = self.arg_const(args, 1, name) else {
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
            ".dword" | ".quad" | ".8byte" => self.data(args, 8),
            ".ascii" | ".asciz" | ".string" => {
                if !args.is_empty() {
                    self.take_loc();
                }
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
                // `.size f, . - f` (merlin's), `.size f, .Lfunc_end0 - f`
                // (clang's) or a constant.
                if let Some(n) = self.arg_name(args, 0, ".size") {
                    let v = match args.get(1) {
                        Some(DirArg::Expr(e)) => self.eval_relative(e),
                        _ => None,
                    };
                    match v {
                        Some(v) if (0..=u32::MAX as i64).contains(&v) => {
                            let s = self.symbol_index(&n);
                            self.symbols[s].size = v as u32;
                        }
                        _ => self.error(
                            ".size: expected a constant, or a difference of '.' and labels \
                             defined earlier in this section",
                        ),
                    }
                }
            }
            ".file" => self.file_directive(args),
            ".loc" => self.loc_directive(args),
            // Accepted and ignored: metadata that does not change the output.
            ".ident" | ".option" | ".attribute" | ".addrsig" | ".addrsig_sym" | ".cfi_startproc"
            | ".cfi_endproc" => {}
            _ => self.error(format!("unknown directive '{name}'")),
        }
    }
}

impl Assembler {
    // ----- line information -----

    /// Gives the pending `.loc`, if any, a row at the current offset: the
    /// instruction or data about to be emitted is where it points.
    fn take_loc(&mut self) {
        if let Some(loc) = self.loc.take() {
            let at = self.sections[self.current].size;
            self.rows.push((self.current, at, loc));
        }
    }

    /// `.file "name"` (the file's name, which merlin-as has no use for) or
    /// `.file N ["dir"] "name" [md5 0x...]`: file number N of the line table.
    /// `.file 0` gives the compilation directory and the main file.
    fn file_directive(&mut self, args: &[DirArg]) {
        let n = match args.first() {
            Some(DirArg::Str(_)) if args.len() == 1 => return,
            Some(DirArg::Expr(e)) => match self.eval(e) {
                Some(n) if (0..=u32::MAX as i64).contains(&n) => n as usize,
                _ => return self.error(".file: expected a file number"),
            },
            _ => return self.error(".file: expected a file number or a name"),
        };
        let mut strings = Vec::new();
        let mut i = 1;
        while let Some(DirArg::Str(s)) = args.get(i) {
            strings.push(String::from_utf8_lossy(s).into_owned());
            i += 1;
        }
        let (dir, name) = match strings.as_slice() {
            [name] => (String::new(), name.clone()),
            [dir, name] => (dir.clone(), name.clone()),
            _ => return self.error(".file: expected [\"directory\"] \"name\" after the number"),
        };
        let md5 = match &args[i..] {
            [] => None,
            [DirArg::Expr(Expr::Sym(m)), sum] if m == "md5" => match sum {
                DirArg::Wide(v) => Some(v.to_be_bytes()),
                DirArg::Expr(Expr::Num(v)) => Some((*v as u64 as u128).to_be_bytes()),
                _ => return self.error(".file: md5 needs a 128-bit number"),
            },
            [DirArg::Expr(Expr::Sym(m)), ..] if m == "source" => {
                return self.error(".file: embedded source is not supported")
            }
            _ => return self.error(".file: unexpected text after the name"),
        };
        let file = dwarf::File { dir, name, md5 };
        if n == 0 {
            self.file0 = Some(file);
            return;
        }
        if self.files.len() <= n {
            self.files.resize(n + 1, None);
        }
        match &self.files[n] {
            Some(old) if *old != file => self.error(format!("file number {n} is already defined")),
            _ => self.files[n] = Some(file),
        }
    }

    /// `.loc FILE [LINE [COLUMN]] [basic_block] [prologue_end]
    /// [epilogue_begin] [is_stmt 0|1] [isa N] [discriminator N]`: the next
    /// instruction comes from this place in the source.
    fn loc_directive(&mut self, args: &[DirArg]) {
        // Two `.loc`s in a row: the first still gets a row, here (LLVM's rule).
        self.take_loc();
        let mut nums = Vec::new();
        let mut i = 0;
        while nums.len() < 3 {
            match args.get(i) {
                Some(DirArg::Expr(e)) if !matches!(e, Expr::Sym(_)) => match self.eval(e) {
                    Some(v) if (0..=u32::MAX as i64).contains(&v) => nums.push(v as u32),
                    _ => return self.error(".loc: expected a number"),
                },
                _ => break,
            }
            i += 1;
        }
        let Some(&file) = nums.first() else {
            return self.error(".loc: expected a file number");
        };
        let defined = match file {
            0 => self.file0.is_some(),
            n => self.files.get(n as usize).is_some_and(Option::is_some),
        };
        if !defined {
            return self.error(format!(".loc: file number {file} has no .file"));
        }
        let mut loc = dwarf::Loc {
            file,
            line: nums.get(1).copied().unwrap_or(0),
            column: nums.get(2).copied().unwrap_or(0),
            ..dwarf::Loc::default()
        };
        let mut is_stmt = self.is_stmt;
        while i < args.len() {
            let Some(DirArg::Expr(Expr::Sym(opt))) = args.get(i) else {
                return self.error(".loc: expected an option such as prologue_end");
            };
            i += 1;
            let flag = match opt.as_str() {
                "basic_block" => dwarf::BASIC_BLOCK,
                "prologue_end" => dwarf::PROLOGUE_END,
                "epilogue_begin" => dwarf::EPILOGUE_BEGIN,
                "is_stmt" | "isa" | "discriminator" => 0,
                _ => return self.error(format!(".loc: unknown option '{opt}'")),
            };
            if flag != 0 {
                loc.flags |= flag;
                continue;
            }
            // An option with a number after it.
            let value = match args.get(i) {
                Some(DirArg::Expr(e)) => self.eval(e).filter(|v| (0..=u32::MAX as i64).contains(v)),
                _ => None,
            };
            let Some(value) = value else {
                return self.error(format!(".loc: {opt} needs a number"));
            };
            i += 1;
            match opt.as_str() {
                "is_stmt" if value > 1 => return self.error(".loc: is_stmt must be 0 or 1"),
                "is_stmt" => is_stmt = value == 1,
                "isa" => loc.isa = value as u32,
                _ => loc.discriminator = value as u32,
            }
        }
        self.is_stmt = is_stmt;
        if is_stmt {
            loc.flags |= dwarf::IS_STMT;
        }
        if self.first_loc_line == 0 {
            self.first_loc_line = self.line;
        }
        self.loc = Some(loc);
    }

    /// The symbol of section `sec`, made the first time it is needed.
    fn section_symbol(&mut self, sec: usize) -> String {
        let name = section_symbol_name(sec);
        let i = self.symbol_index(&name);
        let s = &mut self.symbols[i];
        s.section = Some(sec);
        s.defined = true;
        s.ty = SymType::Section;
        name
    }

    /// Appends `bytes` to the section named `name` (made if there is none,
    /// as debug sections are) and returns where they start.
    fn append(&mut self, name: &str, bytes: &[u8]) -> (usize, u32) {
        let back = self.current;
        self.switch_section(name);
        let sec = self.current;
        let at = self.sections[sec].size;
        self.sections[sec].data.extend_from_slice(bytes);
        self.sections[sec].size += bytes.len() as u32;
        self.current = back;
        (sec, at)
    }

    /// Writes the rows as `.debug_line` (and its strings in
    /// `.debug_line_str`), one sequence per section in the order their first
    /// rows came, with relocations for the strings and the sections' addresses.
    fn line_table(&mut self) {
        if self.rows.is_empty() {
            return;
        }
        let line = self.first_loc_line;
        let Some(root) = self.file0.clone() else {
            let msg = "line information (.loc) needs .file 0, the compilation directory and the main file";
            return self.errors.push(Error::new(line, msg));
        };
        let mut files = Vec::new();
        for (n, f) in self.files.iter().enumerate().skip(1) {
            match f {
                Some(f) => files.push(f.clone()),
                None => {
                    let msg = format!("the line table's files must be numbered 1, 2, ...: no .file {n}");
                    return self.errors.push(Error::new(line, msg));
                }
            }
        }
        let mut sequences: Vec<dwarf::Sequence> = Vec::new();
        for &(section, offset, loc) in &self.rows {
            match sequences.iter_mut().find(|s| s.section == section) {
                Some(seq) => seq.rows.push((offset, loc)),
                None => sequences.push(dwarf::Sequence {
                    section,
                    rows: vec![(offset, loc)],
                    end: self.sections[section].size,
                }),
            }
        }
        let base = self.section_size(".debug_line");
        let str_base = self.section_size(".debug_line_str");
        let table = dwarf::encode(&root.dir, &root, &files, &sequences, base, str_base);
        let (line_sec, _) = self.append(".debug_line", &table.line);
        let (str_sec, _) = self.append(".debug_line_str", &table.line_str);
        for (at, target) in table.fixups {
            let (sec, addend) = match target {
                dwarf::Target::LineStr(off) => (str_sec, off),
                dwarf::Target::Section(sec, off) => (sec, off),
            };
            let symbol = self.section_symbol(sec);
            self.sections[line_sec].relocs.push(Reloc {
                offset: at,
                kind: RelocKind::Abs32,
                symbol,
                addend: addend.into(),
                line,
            });
        }
    }

    fn section_size(&self, name: &str) -> u32 {
        self.sections
            .iter()
            .find(|s| s.name == name)
            .map_or(0, |s| s.size)
    }

    /// A relocation in debug information against a `.L` label becomes one
    /// against the label's section, plus the label's offset (as GNU as and
    /// LLVM do for local symbols they can drop), so that compiler-made labels
    /// need no symbols.
    fn debug_relocs_to_sections(&mut self) {
        for sec in 0..self.sections.len() {
            if self.sections[sec].kind != SectionKind::Debug {
                continue;
            }
            for r in 0..self.sections[sec].relocs.len() {
                let name = &self.sections[sec].relocs[r].symbol;
                if !name.starts_with(".L") {
                    continue;
                }
                let s = &self.symbols[self.by_name[name]];
                let (Some(target), true, None) = (s.section, s.defined, s.constant) else {
                    continue;
                };
                let value = s.value;
                let symbol = self.section_symbol(target);
                let r = &mut self.sections[sec].relocs[r];
                r.symbol = symbol;
                r.addend = r.addend.wrapping_add(i64::from(value));
            }
        }
    }
}

/// True if `v` can be written in `size` bytes: as a signed value
/// (-2^(8·size-1) and up) or an unsigned one (up to 2^(8·size) - 1). This is
/// LLVM's check for data directives (`isIntN(n, v) || isUIntN(n, v)`). Every
/// i64 fits in 8 bytes.
fn fits_in_bytes(v: i64, size: u32) -> bool {
    if size >= 8 {
        return true;
    }
    let bits = 8 * size;
    (-(1i64 << (bits - 1))..1i64 << bits).contains(&v)
}

impl Default for Assembler {
    /// RV32, until the migration to RV64 ends (#159).
    fn default() -> Self {
        Self::new(Xlen::Rv32)
    }
}

/// A branch (`bits` = 13) or `jal` (21) distance: even and in range.
fn check_jump(distance: i64, bits: u32, what: &str) -> Result<(), String> {
    let half = 1i64 << (bits - 1);
    if !(-half..half).contains(&distance) {
        return Err(format!(
            "{what} target out of range: {distance:+} bytes (limit {}..=+{})",
            -half,
            half - 2
        ));
    }
    if distance % 2 != 0 {
        return Err(format!("{what} target at an odd distance ({distance:+} bytes)"));
    }
    Ok(())
}

/// The name of section `sec`'s symbol ([`SymType::Section`]): no name a
/// source can write.
pub fn section_symbol_name(sec: usize) -> String {
    format!("\0section{sec}")
}

/// The location counter as the encoder sees it: a symbol named `.`.
const DOT: &str = ".";
/// Prefix of the hidden labels that stand for `.` (see [`Assembler::dot_label`]).
const DOT_LABEL: &str = ".Ldot$";

/// How a label is named in errors: `.L1$0` (the internal name of the numeric
/// label `1:`) → `1`, `.Ldot$0` → `.`, `.Lfoo` → `foo`, other names unchanged.
fn shown_name(name: &str) -> &str {
    if name.starts_with(DOT_LABEL) {
        return DOT;
    }
    match name.strip_prefix(".L") {
        Some(rest) => rest.split('$').next().unwrap_or(rest),
        None => name,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AUIPC_A0: u32 = 0x0000_0517;
    const ADDI_A0: u32 = 0x0005_0513;

    fn hi(sym: &str) -> Emit {
        let target = Target::Symbol {
            name: sym.into(),
            addend: 0,
        };
        Emit::with_fixup(AUIPC_A0, RelocKind::PcrelHi20, target)
    }

    fn lo(label: &str) -> Emit {
        let target = Target::Symbol {
            name: label.into(),
            addend: 0,
        };
        Emit::with_fixup(ADDI_A0, RelocKind::PcrelLo12I, target)
    }

    /// Feeds `place` the words an encoder would return, one instruction per
    /// line, with `labels` defined just before the instruction on that line.
    fn assemble(lines: &[(&[&str], Emit)]) -> Result<Object, Vec<Error>> {
        let mut a = Assembler::new(Xlen::Rv32);
        for (i, (labels, emit)) in lines.iter().enumerate() {
            a.line = i + 1;
            for l in *labels {
                a.label(l);
            }
            a.place("test", vec![emit.clone()]);
        }
        a.finish()
    }

    #[test]
    fn pcrel_lo_pairs_with_the_auipc_label() {
        let obj = assemble(&[(&["1"], hi("target")), (&[], lo(".L1$0"))]).unwrap();
        let r = &obj.sections[0].relocs;
        assert_eq!((r[1].offset, r[1].kind), (4, RelocKind::PcrelLo12I));
        assert_eq!(r[1].symbol, ".L1$0");

        // The label may come after the %pcrel_lo.
        assemble(&[(&[], lo("later")), (&["later"], hi("target"))]).unwrap();
    }

    #[test]
    fn pcrel_lo_without_pcrel_hi_is_an_error() {
        // The label is on the addi itself, not on an auipc with %pcrel_hi.
        let errs = assemble(&[(&["start"], lo("start"))]).unwrap_err();
        assert_eq!(errs.len(), 1);
        assert_eq!(errs[0].line, 1);
        assert!(errs[0].msg.contains("%pcrel_lo(start)"), "{}", errs[0].msg);

        // An external symbol has no auipc in this object.
        let errs = assemble(&[(&[], lo("elsewhere"))]).unwrap_err();
        assert_eq!((errs.len(), errs[0].line), (1, 1));

        // An undefined numeric label is reported once, as undefined.
        let errs = assemble(&[(&[], lo(".L1$0"))]).unwrap_err();
        assert_eq!(errs.len(), 1);
        assert_eq!(errs[0].msg, "undefined local label '1'");
    }

    #[test]
    fn pcrel_lo_label_in_another_section_is_an_error() {
        let mut a = Assembler::new(Xlen::Rv32);
        a.line = 1;
        a.label("1");
        a.place("test", vec![hi("target")]);
        a.switch_section(".text.other");
        a.line = 2;
        a.place("test", vec![lo(".L1$0")]);
        let errs = a.finish().unwrap_err();
        assert_eq!(errs.len(), 1);
        assert_eq!(errs[0].line, 2);
        assert!(errs[0].msg.contains("%pcrel_lo(1)"), "{}", errs[0].msg);
    }

    // ----- same-section label resolution (compare clang -mno-relax) -----

    /// `.text` of `src` as (word, relocation "KIND symbol+addend" or "").
    fn text(src: &str) -> Vec<(u32, String)> {
        let obj = crate::assemble(src).unwrap();
        let sec = &obj.sections[0];
        sec.data
            .chunks(4)
            .enumerate()
            .map(|(i, c)| {
                let off = 4 * i as u32;
                let r = sec.relocs.iter().find(|r| r.offset == off);
                let r = r.map_or(String::new(), |r| {
                    format!("{} {}+{}", r.kind.name(), r.symbol, r.addend)
                });
                (u32::from_le_bytes(c.try_into().unwrap()), r)
            })
            .collect()
    }

    fn words(src: &str) -> Vec<u32> {
        text(src).into_iter().map(|(w, _)| w).collect()
    }

    #[test]
    fn local_labels_in_the_same_section_are_resolved() {
        // Words from clang -mno-relax for the same source (llvm-objdump -d).
        let src = "
back:   nop
        beq  a0, a1, back
        beq  a0, a1, fwd
        bnez a0, 1f
        jal  back
        j    fwd
        call back
        tail fwd
1:      nop
fwd:    nop
";
        assert_eq!(
            text(src),
            [
                (0x0000_0013, ""),
                (0xfeb5_0ee3, ""), // beq a0, a1, back (-4)
                (0x02b5_0263, ""), // beq a0, a1, fwd (+36)
                (0x0005_1e63, ""), // bnez a0, 1f (+28)
                (0xff1f_f0ef, ""), // jal back (-16)
                (0x0180_006f, ""), // j fwd (+24)
                (0x0000_0097, ""), // auipc ra, 0
                (0xfe80_80e7, ""), // jalr -24(ra)
                (0x0000_0317, ""), // auipc t1, 0
                (0x00c3_0067, ""), // jr 12(t1)
                (0x0000_0013, ""),
                (0x0000_0013, ""),
            ]
            .map(|(w, r)| (w, r.to_string()))
        );
    }

    #[test]
    fn pcrel_pairs_to_local_labels_are_resolved() {
        // la, a load and a store to `data`, then an explicit pair with an
        // addend, all resolved; the hidden .Lpcrel_hi labels disappear.
        let src = "
        la   a0, data
        lw   a1, data
        sw   a1, data, t0
2:      auipc a2, %pcrel_hi(data + 4)
        lw   a3, %pcrel_lo(2b)(a2)
data:   .word 0
";
        assert_eq!(
            words(src),
            [
                0x0000_0517, // auipc a0, 0
                0x0205_0513, // addi a0, a0, 32
                0x0000_0597, // auipc a1, 0
                0x0185_a583, // lw a1, 24(a1)
                0x0000_0297, // auipc t0, 0
                0x00b2_a823, // sw a1, 16(t0)
                0x0000_0617, // auipc a2, 0
                0x00c6_2683, // lw a3, 12(a2)
                0,
            ]
        );
        let obj = crate::assemble(src).unwrap();
        assert!(obj.sections[0].relocs.is_empty());
        assert!(obj.symbols.iter().all(|s| !s.name.starts_with(".L")));
    }

    #[test]
    fn far_pcrel_pairs_round_the_upper_part() {
        // `la` 0x1800 bytes ahead: the low 12 bits are 0x800, so lo12 is
        // -2048 and hi20 rounds up to 2. The call is 0x17f8 ahead: no rounding.
        let src = "    la a0, data\n    call far\n    .space 0x17f0\ndata:\nfar:\n";
        assert_eq!(
            words(src)[..4],
            [
                0x0000_2517, // auipc a0, 2
                0x8005_0513, // addi a0, a0, -2048
                0x0000_1097, // auipc ra, 1
                0x7f80_80e7, // jalr 2040(ra)
            ]
        );
    }

    #[test]
    fn dot_is_the_current_address() {
        // Words from clang -mno-relax for the same source (llvm-objdump -dr):
        // every reference to `.` in code is resolved, no relocation.
        let src = "
        nop
        j    .
        beq  x0, x0, .
        la   a0, .
        j    .+8
        bnez a0, .-8
        nop
";
        assert_eq!(
            text(src),
            [
                (0x0000_0013, ""),
                (0x0000_006f, ""), // j 0x4
                (0x0000_0063, ""), // beqz zero, 0x8
                (0x0000_0517, ""), // auipc a0, 0
                (0x0005_0513, ""), // addi a0, a0, 0
                (0x0080_006f, ""), // j 0x1c
                (0xfe05_1ce3, ""), // bnez a0, 0x10
                (0x0000_0013, ""),
            ]
            .map(|(w, r)| (w, r.to_string()))
        );
        let obj = crate::assemble(src).unwrap();
        assert!(obj
            .symbols
            .iter()
            .all(|s| s.name != "." && !s.name.starts_with(".L")));

        // In data, as in clang (`R_RISCV_32 .L0`, `.L0+0x4`): a relocation
        // against a hidden label at that word.
        let obj = crate::assemble(".data\n.word 0\n.word .\n.word . + 4\n").unwrap();
        let relocs: Vec<_> = obj.sections[1]
            .relocs
            .iter()
            .map(|r| {
                let s = obj.symbols.iter().find(|s| s.name == r.symbol).unwrap();
                (r.offset, r.kind, s.section, s.value, r.addend)
            })
            .collect();
        assert_eq!(
            relocs,
            [
                (4, RelocKind::Abs32, Some(1), 4, 0),
                (8, RelocKind::Abs32, Some(1), 8, 4),
            ]
        );
        assert!(obj.symbols.iter().all(|s| s.name != "."));

        // Out of range: the error names `.`.
        let errs = crate::assemble("nop\nbeqz a0, . + 8192\n").unwrap_err();
        assert_eq!(errs[0].line, 2);
        assert!(
            errs[0].msg.starts_with("'.': branch target out of range"),
            "{}",
            errs[0].msg
        );
    }

    #[test]
    fn other_references_stay_relocations() {
        let src = "
        .globl gfn
gfn:    nop
        beq  a0, a1, gfn
        jal  later_global
        call ext
        la   a0, other
        lui  a0, %hi(local)
        .globl later_global
later_global:
local:  nop
        .section .text.other
other:  j local
";
        let got: Vec<String> = text(src).into_iter().map(|(_, r)| r).collect();
        assert_eq!(
            got,
            [
                "",
                "R_RISCV_BRANCH gfn+0",
                "R_RISCV_JAL later_global+0",
                "R_RISCV_CALL_PLT ext+0",
                "",
                "R_RISCV_PCREL_HI20 other+0",
                "R_RISCV_PCREL_LO12_I .Lpcrel_hi0+0",
                "R_RISCV_HI20 local+0",
                "",
            ]
        );
        // Referenced by a relocation, so the hidden label stays in the symbol table.
        let obj = crate::assemble(src).unwrap();
        assert!(obj.symbols.iter().any(|s| s.name == ".Lpcrel_hi0"));
        // A label in the other section: a relocation there too.
        assert_eq!(obj.sections[1].relocs[0].kind, RelocKind::Jal);
        assert_eq!(obj.sections[1].relocs[0].symbol, "local");
    }

    // ----- what clang -S writes for C files -----

    fn sym<'a>(obj: &'a Object, name: &str) -> &'a Symbol {
        obj.symbols
            .iter()
            .find(|s| s.name == name)
            .unwrap_or_else(|| panic!("no symbol {name}"))
    }

    fn section<'a>(obj: &'a Object, name: &str) -> (usize, &'a Section) {
        obj.sections
            .iter()
            .enumerate()
            .find(|(_, s)| s.name == name)
            .unwrap_or_else(|| panic!("no section {name}"))
    }

    #[test]
    fn size_of_a_function_from_labels_or_dot() {
        // merlin writes `.size f, . - f`; clang `.size g, .Lfunc_end1 - g`.
        let src = "
        .text
        nop
        .type f, @function
f:      nop
        nop
        .size f, . - f
        .type g, @function
g:      addi a0, a0, 1
        ret
.Lfunc_end1:
        nop
        .size g, .Lfunc_end1 - g
        .data
        .type v, @object
v:      .word 1, 2, 3
        .size v, 12
        .equ N, 3
w:      .word 0, 0, 0
        .size w, N * 4
";
        let obj = crate::assemble(src).unwrap();
        for (name, size, ty) in [
            ("f", 8, SymType::Func),
            ("g", 8, SymType::Func),
            ("v", 12, SymType::Object),
            ("w", 12, SymType::NoType),
        ] {
            let s = sym(&obj, name);
            assert_eq!((s.size, s.ty), (size, ty), "{name}");
        }
        // A label in another section, or one not defined yet, has no
        // distance from `.`: an error, not a silently wrong size.
        for bad in [
            "    .text\nf: nop\n    .data\n    .size f, . - f\n",
            "    .text\n    .size f, . - f\nf: nop\n",
            "    .text\nf: nop\n    .size f, . * f\n",
            "    .text\nf: nop\n    .size f\n",
            "    .text\nf: nop\n    .size f, -4\n",
            // An offset that does not cancel out is not a size.
            "    .text\n    nop\nf: nop\n    .size f, f\n",
            "    .text\n    nop\nf: nop\n    .size f, . + f\n",
            "    .text\n    nop\nf: nop\ng: nop\n    .size f, . - f + g\n",
        ] {
            let errs = crate::assemble(bad).unwrap_err();
            assert!(errs[0].msg.contains(".size: expected"), "{bad:?}: {errs:?}");
        }
    }

    #[test]
    fn local_common_symbols_go_in_bss() {
        // clang's output for `static char buf[100]; static int n;` in a
        // function's file: .local, then .comm with size and alignment.
        let src = "
        .text
f:      nop
        .local  buf
        .comm   buf,100,1
        .local  n
        .comm   n,4,4
        .lcomm  big, 16, 8
g:      nop
";
        let obj = crate::assemble(src).unwrap();
        let (bss, s) = section(&obj, ".bss");
        // buf at 0, n aligned up to 100, big aligned up to 104.
        assert_eq!(s.size, 120);
        assert_eq!(s.align, 8);
        for (name, value, size) in [("buf", 0, 100), ("n", 100, 4), ("big", 104, 16)] {
            let s = sym(&obj, name);
            assert_eq!((s.section, s.value, s.size), (Some(bss), value, size), "{name}");
            assert!(s.defined && !s.global, "{name}");
            assert_eq!(s.ty, SymType::Object, "{name}");
        }
        // The section in use is still .text: g follows f.
        let (text, _) = section(&obj, ".text");
        assert_eq!((sym(&obj, "g").section, sym(&obj, "g").value), (Some(text), 4));
    }

    #[test]
    fn global_common_symbols_are_refused() {
        let errs = crate::assemble("        .comm shared, 4, 4\n").unwrap_err();
        assert_eq!(errs.len(), 1);
        assert!(errs[0].msg.contains("-fno-common"), "{}", errs[0].msg);
        for bad in [
            ".local x\n.comm x, 4, 3\n",
            ".local x\n.comm x, -1\n",
            ".lcomm x\n",
        ] {
            assert!(crate::assemble(bad).is_err(), "{bad}");
        }
    }

    // ----- debug information -----

    #[test]
    fn label_differences_are_data() {
        // What merlin -g writes for a function's size in .debug_info, and
        // clang's `.Ldebug_info_end0 - .Ldebug_info_start0`.
        let src = "
        .text
f:      nop
        nop
.Lend:
        .section .debug_info,\"\",@progbits
.Ls:    .word .Lend - f
        .half .Lend - f - 4
        .byte -(f - .Lend)
        .word . - .Ls
        .word .Lend
";
        let obj = crate::assemble(src).unwrap();
        let (i, info) = section(&obj, ".debug_info");
        assert_eq!(info.data, [8, 0, 0, 0, 4, 0, 8, 7, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(obj.sections[i].kind, SectionKind::Debug);
        // The label's word: a relocation against .text's section symbol, the
        // label's offset its addend; the label itself is no symbol.
        let r = &info.relocs[0];
        assert_eq!((r.offset, r.kind, r.addend), (11, RelocKind::Abs32, 8));
        let s = sym(&obj, &r.symbol);
        assert_eq!((s.ty, s.section), (SymType::Section, Some(0)));
        assert!(obj.symbols.iter().all(|s| !s.name.starts_with(".L")));
        // Differences across sections are not numbers.
        let errs = crate::assemble(".text\nf: nop\n.data\ng: .word 0\n.word g - f\n").unwrap_err();
        assert!(errs[0].msg.contains("expected a constant or symbol"), "{errs:?}");
    }

    /// Assembles `src` for the width `xlen`.
    fn assemble_for(src: &str, xlen: Xlen) -> Result<Object, Vec<Error>> {
        crate::assemble_with(src, &crate::Options { xlen })
    }

    /// A relocation as (offset, kind, the target's section and value, addend).
    fn relocs_of(obj: &Object, sec: &Section) -> Vec<(u32, RelocKind, Option<usize>, u32, i64)> {
        sec.relocs
            .iter()
            .map(|r| {
                let s = sym(obj, &r.symbol);
                (r.offset, r.kind, s.section, s.value, r.addend)
            })
            .collect()
    }

    #[test]
    fn data_of_every_size() {
        // The bytes and relocations llvm-mc 23 writes for this source, the
        // same for riscv32 and riscv64 (llvm-objdump -s, llvm-readelf -r):
        // `.dword sym` is R_RISCV_64 on RV32 too, `.word sym` R_RISCV_32 on
        // RV64 too, and `>>` is a logical shift.
        let src = "
        .data
        .byte 255, -128
        .half -32768, 0xffff
        .word 0xffffffff, -0x80000000
        .dword -16 >> 2
        .dword 1 << 63 >> 63
        .word 0x80000000 >> 31
x:      .dword x + 8, .
        .quad .
        .8byte -1
        .word x
";
        #[rustfmt::skip]
        let want: [u8; 0x46] = [
            0xff, 0x80,                                     // .byte
            0x00, 0x80, 0xff, 0xff,                         // .half
            0xff, 0xff, 0xff, 0xff, 0x00, 0x00, 0x00, 0x80, // .word
            0xfc, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x3f, // -16 >> 2
            0x01, 0, 0, 0, 0, 0, 0, 0,                      // 1 << 63 >> 63
            0x01, 0, 0, 0,                                  // 0x80000000 >> 31
            0, 0, 0, 0, 0, 0, 0, 0,                         // x + 8
            0, 0, 0, 0, 0, 0, 0, 0,                         // .
            0, 0, 0, 0, 0, 0, 0, 0,                         // .
            0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, // -1
            0, 0, 0, 0,                                     // x
        ];
        for xlen in [Xlen::Rv32, Xlen::Rv64] {
            let obj = assemble_for(src, xlen).unwrap();
            let (i, data) = section(&obj, ".data");
            assert_eq!(data.data, want, "{xlen:?}");
            // `.` is a hidden label at its own word, as clang's `.L0`.
            assert_eq!(
                relocs_of(&obj, data),
                [
                    (0x22, RelocKind::Abs64, Some(i), 0x22, 8),
                    (0x2a, RelocKind::Abs64, Some(i), 0x2a, 0),
                    (0x32, RelocKind::Abs64, Some(i), 0x32, 0),
                    (0x42, RelocKind::Abs32, Some(i), 0x22, 0),
                ],
                "{xlen:?}"
            );
        }
    }

    #[test]
    fn data_must_fit_its_size() {
        // llvm-mc says "out of range literal value" for each number here,
        // and "2-byte data relocations not supported" for `.half x`.
        for xlen in [Xlen::Rv32, Xlen::Rv64] {
            for (src, want) in [
                (".byte 256", "data value 256 does not fit in 1 byte"),
                (".byte -129", "data value -129 does not fit in 1 byte"),
                (".half 0x10000", "data value 65536 does not fit in 2 bytes"),
                (".short -32769", "data value -32769 does not fit in 2 bytes"),
                (
                    ".word 0x100000000",
                    "data value 4294967296 does not fit in 4 bytes",
                ),
                (
                    ".4byte -0x80000001",
                    "data value -2147483649 does not fit in 4 bytes",
                ),
                // Logical: a negative value shifted right is a big one.
                (
                    ".word -16 >> 2",
                    "data value 4611686018427387900 does not fit in 4 bytes",
                ),
                // An `.equ` holds any 64-bit value; it is checked where used.
                (
                    ".equ BIG, 0x123456789\n.word BIG",
                    "data value 4886718345 does not fit in 4 bytes",
                ),
                (
                    ".byte 1, 2\n.byte 3, 0x100, 4",
                    "data value 256 does not fit in 1 byte",
                ),
                ("x: .half x", "only .word and .dword can hold a symbol address"),
                (".byte .", "only .word and .dword can hold a symbol address"),
            ] {
                let errs = assemble_for(&format!(".data\n{src}\n"), xlen).unwrap_err();
                let line = 1 + src.lines().count();
                assert_eq!(errs.len(), 1, "{src}: {errs:?}");
                assert_eq!(
                    (errs[0].line, errs[0].msg.as_str()),
                    (line, want),
                    "{xlen:?} {src}"
                );
            }
            // The edges of each range, and any `.dword`.
            let src = ".data\n.byte -128, 255\n.half -32768, 65535\n.word -0x80000000, 0xffffffff\n\
                       .dword -0x8000000000000000, 0xffffffffffffffff\n";
            assert!(assemble_for(src, xlen).is_ok(), "{xlen:?}");
        }
    }

    #[test]
    fn right_shifts_are_logical() {
        // llvm-mc 23 -show-encoding: `li a0, -1 >> 32` is `li a0, -1` on
        // riscv32 (0xffffffff is -1 in 32 bits) and `li a0, -1; srli a0, a0,
        // 32` on riscv64; `-16 >> 60` is 15 on both.
        let words = |src: &str, xlen| {
            let obj = assemble_for(src, xlen).unwrap();
            let data = &obj.sections[0].data;
            data.chunks(4)
                .map(|w| u32::from_le_bytes(w.try_into().unwrap()))
                .collect::<Vec<_>>()
        };
        let src = "li a0, -1 >> 32\nli a1, -16 >> 60\n";
        assert_eq!(words(src, Xlen::Rv32), [0xfff0_0513, 0x00f0_0593]);
        assert_eq!(words(src, Xlen::Rv64), [0xfff0_0513, 0x0205_5513, 0x00f0_0593]);
    }

    #[test]
    fn symbol_addends_wrap_around() {
        // A 64-bit constant added to a label: the addend wraps in 64 bits as
        // in LLVM (`x - 0x8000000000000000` from llvm-readelf on riscv64),
        // where an unchecked sum would overflow (`sym + c` and `c + sym`).
        let src =
            ".data\nx: .word 0\n.equ BIG, 0x7fffffffffffffff\n.dword x + BIG + 1\n.word 1 + (BIG + x)\n";
        for xlen in [Xlen::Rv32, Xlen::Rv64] {
            let obj = assemble_for(src, xlen).unwrap();
            let (i, data) = section(&obj, ".data");
            assert_eq!(
                relocs_of(&obj, data),
                [
                    (4, RelocKind::Abs64, Some(i), 0, i64::MIN),
                    (12, RelocKind::Abs32, Some(i), 0, i64::MIN),
                ],
                "{xlen:?}"
            );
        }
    }

    #[test]
    fn dwords_in_debug_sections_name_the_section() {
        // RV64's DWARF holds 8-byte addresses (`.quad .Lfunc_begin0`): as
        // for a `.word`, a `.L` label becomes its section's symbol plus the
        // label's offset; any other symbol stays.
        let src = "
        .text
f:      nop
.Lend:  nop
        .section .debug_info,\"\",@progbits
        .quad .Lend
        .dword f + 2
        .8byte .Lend - f
";
        for xlen in [Xlen::Rv32, Xlen::Rv64] {
            let obj = assemble_for(src, xlen).unwrap();
            let (_, info) = section(&obj, ".debug_info");
            let mut want = [0; 24];
            want[16] = 4;
            assert_eq!(info.data, want, "{xlen:?}");
            let r = &info.relocs;
            assert_eq!(r.len(), 2, "{xlen:?}");
            assert_eq!((r[0].offset, r[0].kind, r[0].addend), (0, RelocKind::Abs64, 4));
            let s = sym(&obj, &r[0].symbol);
            assert_eq!((s.ty, s.section), (SymType::Section, Some(0)));
            assert_eq!(
                (r[1].offset, r[1].kind, r[1].symbol.as_str(), r[1].addend),
                (8, RelocKind::Abs64, "f", 2)
            );
            assert!(obj.symbols.iter().all(|s| !s.name.starts_with(".L")));
        }
    }

    #[test]
    fn line_information_errors() {
        for (src, want) in [
            (".loc 0 1 0\nnop\n", "file number 0 has no .file"),
            (".file 1 \"a.c\"\n.loc 1 1 0\nnop\n", "needs .file 0"),
            (
                ".file 0 \"d\" \"a.c\"\n.file 2 \"b.h\"\n.loc 2 1 0\nnop\n",
                "no .file 1",
            ),
            (
                ".file 0 \"d\" \"a.c\"\n.loc 0 1 0 frobnicate\nnop\n",
                "unknown option 'frobnicate'",
            ),
            (
                ".file 0 \"d\" \"a.c\"\n.loc 0 1 0 is_stmt 2\nnop\n",
                "is_stmt must be 0 or 1",
            ),
            (
                ".file 0 \"d\" \"a.c\"\n.loc 0 1 0 isa\nnop\n",
                "isa needs a number",
            ),
            (
                ".file 1 \"a.c\"\n.file 1 \"b.c\"\n",
                "file number 1 is already defined",
            ),
            (
                ".file 0 \"d\" \"a.c\" source \"int x;\"\n",
                "embedded source is not supported",
            ),
            (".file 0 \"d\" \"a.c\" md5\n", "unexpected text after the name"),
            (
                ".section .debug_info\nnop\n",
                "instructions are not allowed in .debug_info",
            ),
            (
                ".word 0x0102030405060708090a0b0c0d0e0f10\n",
                "number too large for 64 bits",
            ),
        ] {
            let errs = crate::assemble(src).unwrap_err();
            assert!(errs.iter().any(|e| e.msg.contains(want)), "{src:?}: {errs:?}");
        }
        // The same .file twice is fine; a .file and .loc with no code make no table.
        let obj =
            crate::assemble(".file 1 \"a.c\"\n.file 1 \"a.c\"\n.file 0 \"d\" \"a.c\"\n.loc 0 3\n").unwrap();
        assert!(obj.sections.iter().all(|s| s.kind != SectionKind::Debug));
    }

    #[test]
    fn set_to_a_label_defines_an_alias() {
        // clang merges a file's static variables into one block and names
        // its parts: `name = .L_MergedGlobals+offset`.
        let src = "
        .data
.L_MergedGlobals:
        .word 1, 2, 3
        .set second, .L_MergedGlobals+4
third = .L_MergedGlobals+8
        .equ  four, 4
        .addrsig
        .addrsig_sym second
        .text
        la   a0, third
";
        let obj = crate::assemble(src).unwrap();
        let (data, _) = section(&obj, ".data");
        assert_eq!(
            (sym(&obj, "second").section, sym(&obj, "second").value),
            (Some(data), 4)
        );
        assert_eq!(
            (sym(&obj, "third").section, sym(&obj, "third").value),
            (Some(data), 8)
        );
        assert_eq!(sym(&obj, "four").constant, Some(4));

        // The label must be defined first, and be a label.
        for bad in [".set early, later\nlater: nop\n", ".set c, 5\n.set e, nowhere\n"] {
            let errs = crate::assemble(bad).unwrap_err();
            assert!(
                errs[0].msg.contains("must be a label defined before this"),
                "{bad}: {errs:?}"
            );
        }
    }
}
