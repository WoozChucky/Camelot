//! # Code generation: AST to Excalibur assembly
//!
//! Turns a checked [`Program`] into `.s` text for merlin-as. It is a *stack
//! machine* code generator, the approach of Rui Ueyama's chibicc: every
//! expression leaves its value in `a0`, and a value that must survive while
//! another expression is evaluated is pushed on the stack. Nothing is kept in
//! registers across statements and every variable lives in memory, so the
//! output is long but easy to follow, and easy to check by hand.
//!
//! ## Contract
//!
//! * Input: a [`Program`] that passed [`crate::sema`]. Every expression has its
//!   C type, conversions are explicit cast nodes, `sizeof` is folded, and every
//!   variable says where it lives: local `#i` of its function (parameters
//!   first) or a global symbol. Every call names a declared function with the
//!   right number of arguments (at most 8, in `a0-a7`).
//! * Output: assembly merlin-as accepts (and so does clang): functions in
//!   `.text`, initialized variables in `.data`, zero-initialized ones in
//!   `.bss`, string literals in `.rodata`. Every function and variable that is
//!   not `static` gets `.globl`; declarations without a definition emit nothing
//!   (the linker finds them).
//! * Semantics: 32-bit wrapping arithmetic, `/` and `%` truncate toward zero
//!   (`div`/`rem`, or `divu`/`remu` when unsigned), shifts use the low 5 bits
//!   of the amount, comparisons and `! && ||` produce 0 or 1, `&&`/`||`
//!   short-circuit, an assignment evaluates to the stored value. Falling off
//!   the end of a function returns 0.
//!
//! ## Values and types
//!
//! Every value in a register is 32 bits wide. A `char` or `short` is widened
//! when it is loaded (`lbu`/`lb`, `lhu`/`lh`, by its signedness; plain `char`
//! is unsigned) and narrowed when it is stored (`sb`, `sh`), so integer
//! promotion costs nothing. A conversion *to* a narrow type in a register (a
//! cast, or the value of `c = 300` for a `char c`) is done in place:
//!
//! ```text
//!   andi  a0, a0, 255         # to unsigned char (and plain char)
//!   slli  a0, a0, 24          # to signed char: keep the low byte...
//!   srai  a0, a0, 24          # ...and copy its top bit up
//!   slli  a0, a0, 16          # to unsigned short (srai for short)
//!   srli  a0, a0, 16
//! ```
//!
//! The usual arithmetic conversions decide the instruction: `divu`, `remu`,
//! `sltu` and `srl` when the operation is unsigned (an operand is `unsigned
//! int` or `unsigned long`, or a pointer), `div`, `rem`, `slt` and `sra`
//! otherwise.
//!
//! An array is never loaded: an expression of array type leaves its address in
//! `a0`, which is exactly C's array-to-pointer decay.
//!
//! ## Calling convention (RISC-V ILP32)
//!
//! * Arguments go in `a0-a7`, left to right; the result comes back in `a0`.
//!   Arguments and results narrower than 32 bits travel widened by their type
//!   (sema's casts do that).
//! * `ra` holds the return address (`call f` is `auipc ra` + `jalr ra`).
//! * `sp`, `s0-s11` must look untouched to the caller when we return; the
//!   `a` and `t` registers may be clobbered by any call. We only ever use
//!   `s0` (saved in the prologue), `a0`, `a1`, the argument registers, `t0`
//!   and `t1`.
//! * `sp` is 16-byte aligned at every call.
//!
//! ## Stack frame
//!
//! `s0` (the frame pointer) holds the value `sp` had on entry, so the caller's
//! frame starts at `s0` and ours lies below it:
//!
//! ```text
//!            caller's frame
//!   s0 ->  +------------------+  (16-byte aligned: sp before the call)
//!          | saved ra         |  -4(s0)
//!          | caller's s0      |  -8(s0)
//!          | local #0         |           parameters first: a0 is stored here,
//!          | local #1         |           a1 next, and so on
//!          | ...              |
//!          | (padding)        |           frame size rounded up to 16
//!   sp ->  +------------------+  (16-byte aligned) after the prologue
//!          | temporaries      |  pushed and popped while evaluating expressions
//! ```
//!
//! Locals are placed downwards from `-8(s0)` in order, each at the next
//! offset aligned for its type (arrays at least to 4 bytes): an `int` takes the
//! 4 bytes below the previous local, a `char` 1 byte, an `int a[3]` 12 bytes.
//! With only `int`s, local `#i` is at `-(12 + 4*i)(s0)`. Sema gives every
//! declaration in the function its own local (shadowed names included), so a
//! slot never has to be reused and scopes cost nothing here.
//!
//! The prologue saves `ra` and `s0` in a fixed 16 bytes, points `s0` at the top
//! of the frame and then makes room for the rest; the epilogue undoes it from
//! `s0`, so it never needs to know the frame size:
//!
//! ```text
//!   f:  addi  sp, sp, -16          .L.return.f:              # result in a0
//!       sw    ra, 12(sp)               addi  sp, s0, -16
//!       sw    s0, 8(sp)                lw    ra, 12(sp)
//!       addi  s0, sp, 16               lw    s0, 8(sp)
//!       addi  sp, sp, -(frame - 16)    addi  sp, sp, 16
//!       sw    a0, -12(s0)  # params    ret
//! ```
//!
//! ## Expressions
//!
//! `expr(e)` emits code that leaves `e`'s value in `a0`. For `l op r`:
//!
//! ```text
//!   <l>                    # a0 = l
//!   addi  sp, sp, -4       # push a0: <r> may need a0 (and any register)
//!   sw    a0, 0(sp)
//!   <r>                    # a0 = r
//!   lw    a1, 0(sp)        # pop: a1 = l
//!   addi  sp, sp, 4
//!   sub   a0, a1, a0       # a0 = l - r
//! ```
//!
//! Comparisons give 0 or 1 with `slt` (`l < r`), `slt` with the operands
//! swapped (`l > r`), those two followed by `xori 1` (`>=`, `<=`), and `xor`
//! followed by `seqz`/`snez` (`==`, `!=`). `&&` and `||` branch around their
//! right side and turn the result into 0 or 1 with `snez`; `?:` branches like
//! an `if`.
//!
//! Pointer arithmetic counts elements: `p + i` scales `i` by the size of what
//! `p` points to (a shift for powers of two), and `p - q` divides the byte
//! difference by it.
//!
//! ## Places: where assignments store
//!
//! The target of `=`, `op=`, `++` and `--` is a *place*. A scalar local is
//! reached directly at its offset from `s0`; for anything else (a global,
//! `*p`, `a[i]`) the address is computed first and pushed, so that the value
//! can be computed with any registers, then popped into `a1` for the store:
//!
//! ```text
//!   x = 5;   (local)          a[i] = 5;
//!     li    a0, 5               <address of a[i]>
//!     sw    a0, -12(s0)         addi sp, sp, -4    # push it
//!                               sw   a0, 0(sp)
//!                               li   a0, 5
//!                               lw   a1, 0(sp)     # pop it
//!                               addi sp, sp, 4
//!                               sw   a0, 0(a1)
//! ```
//!
//! `x op= v` loads the place's value (peeking at the pushed address, which
//! stays for the store), pushes it, evaluates `v`, applies the operator,
//! converts the result to `x`'s type and stores it, so `x` is evaluated once.
//! `++x` and `x++` add the step (1, or the pointed-to size) in place; `x++`
//! then subtracts it again to give back the old value.
//!
//! Globals are reached through their address: `la a0, g` (`auipc` + `addi`,
//! PC-relative), then a load or store at `0(a0)`.
//!
//! A call evaluates its arguments left to right, pushing all but the last;
//! the last goes from `a0` to `a(n-1)` with `mv`, then the others are popped
//! into `a(n-2)` down to `a0`. If temporaries of an enclosing expression
//! are still pushed (as in `x + f(y)`), `sp` is lowered to a multiple of 16
//! around the `call`, to keep the convention's alignment.
//!
//! ## Initializers
//!
//! A local scalar's initializer is an assignment. A local array with an
//! initializer is first cleared (word by word, or with a loop when it is
//! big), then each element given is stored. A local without an initializer is
//! left as it is (C: indeterminate).
//!
//! Globals and `static` locals are data: the initializer, a constant
//! expression, is written out with `.byte`, `.half`, `.word` (`.word sym+off`
//! for an address, which the linker fills in), `.ascii` and `.zero`, after a
//! `.balign` for the type. A `static` local's symbol is `function.name`, a
//! local symbol (no `.globl`). Each string literal gets a label `.L.str.<n>`
//! in `.rodata`.
//!
//! ## Control flow
//!
//! Conditions are evaluated into `a0` and tested with `beqz`/`bnez`. Jumps go
//! to local labels named `.L.<what>.<n>`, numbered across the whole file
//! (`.L.else.3`, `.L.break.7`); every function returns through
//! `.L.return.<name>`.
//!
//! A conditional branch reaches only ±4 KiB, and merlin-as reports a branch
//! that cannot reach its label as an error rather than rewriting it. So once a
//! function is generated, [`relax_branches`] estimates every instruction's
//! address (never underestimating a distance) and turns a branch that may be
//! too far into the opposite branch over a `j`, which reaches ±1 MiB:
//!
//! ```text
//!   beqz  a0, .L.end.4      becomes      bnez  a0, 1f
//!                                        j     .L.end.4
//!                                     1:
//! ```

use crate::ast::*;
use crate::consteval::{self, Base, Const};
use crate::sema::arith_type;
use crate::types::{IntKind, Type};
use crate::Error;
use std::collections::HashMap;
use std::fmt::Write;

/// Generates assembly for a whole translation unit.
pub fn generate(prog: &Program) -> Result<String, Error> {
    // merlin-as reads a register name as a register, never as a symbol, so a
    // function or global called `sp` or `a0` could not be referenced or even
    // defined.
    for item in &prog.items {
        let names: Vec<(&str, crate::Pos)> = match item {
            Item::Func(f) => vec![(&f.name, f.pos)],
            Item::Decl(ds) => ds.iter().map(|d| (d.name.as_str(), d.pos)).collect(),
        };
        if let Some((name, pos)) = names
            .into_iter()
            .find(|(n, _)| excalibur::reg::parse(n).is_some())
        {
            return Err(Error::new(
                pos,
                format!("'{name}' is a register name, which cannot be used as a symbol name"),
            ));
        }
    }

    let mut file = FileGen {
        out: String::new(),
        section: "",
        labels: 0,
        strings: Vec::new(),
    };
    for item in &prog.items {
        match item {
            Item::Func(f) => {
                let Some(body) = &f.body else {
                    continue; // a prototype: the definition is elsewhere
                };
                let (offsets, frame) = layout(&f.locals);
                let mut g = FuncGen {
                    func: f,
                    offsets,
                    frame,
                    lines: Vec::new(),
                    depth: 0,
                    loops: Vec::new(),
                    file: &mut file,
                    ret_label: format!(".L.return.{}", f.name),
                    statics: Vec::new(),
                };
                g.function(body);
                let (lines, statics) = (g.lines, g.statics);
                file.switch(".text");
                file.out.push('\n');
                render(&lines, &relax_branches(&lines), &mut file.out);
                for d in statics {
                    file.variable(d);
                }
            }
            Item::Decl(decls) => {
                for d in decls.iter().filter(|d| d.define) {
                    file.variable(d);
                }
            }
        }
    }
    file.strings();
    Ok(file.out)
}

/// `ra` and the caller's `s0` take the top 8 bytes of every frame.
const SAVED_REGS: i32 = 8;
/// The prologue first lowers `sp` by this much, enough for `ra` and `s0` and a
/// multiple of 16 so that `sp` stays aligned at every step.
const SAVE_AREA: i32 = 16;

/// Places every local in the frame: its offset from `s0`, and the frame size
/// (a multiple of 16, the convention's stack alignment).
fn layout(locals: &[Local]) -> (Vec<i32>, i32) {
    let mut off = -SAVED_REGS;
    let offsets = locals
        .iter()
        .map(|l| {
            let size = l.ty.size().expect("sema: locals have complete types") as i32;
            // Arrays are word-aligned so that clearing them can use `sw`.
            let align = if l.ty.is_array() {
                l.ty.align().max(4)
            } else {
                l.ty.align()
            } as i32;
            // Round down to the alignment (two's complement: works below zero).
            off = (off - size) & !(align - 1);
            off
        })
        .collect();
    (offsets, (-off + 15) & !15)
}

/// Does `v` fit a 12-bit signed immediate (`addi`, `lw`, `sw`)?
fn fits_imm12(v: i32) -> bool {
    (-2048..=2047).contains(&v)
}

/// The load that brings a value of type `ty` into a register, widened.
fn load_op(ty: &Type) -> &'static str {
    match ty {
        Type::Int(k) => match (k.size(), k.signed()) {
            (1, true) => "lb",
            (1, false) => "lbu",
            (2, true) => "lh",
            (2, false) => "lhu",
            _ => "lw",
        },
        Type::Ptr(_) => "lw",
        Type::Void | Type::Array(..) => unreachable!("no value of type '{ty}' is loaded"),
    }
}

/// The store that writes a value of type `ty` (its low bytes) to memory.
fn store_op(ty: &Type) -> &'static str {
    match ty.size() {
        Some(1) => "sb",
        Some(2) => "sh",
        Some(4) => "sw",
        _ => unreachable!("no value of type '{ty}' is stored"),
    }
}

/// `log2(n)` if `n` is a power of two.
fn log2(n: u32) -> Option<u32> {
    n.is_power_of_two().then(|| n.trailing_zeros())
}

/// What is shared by the whole file: the output, the label counter and the
/// string literals seen so far.
struct FileGen {
    out: String,
    /// The section the output is in.
    section: &'static str,
    /// Label counter shared by every function in the file, so labels are unique.
    labels: usize,
    /// String literals, emitted at the end; literal `i` is `.L.str.<i>`.
    strings: Vec<Vec<u8>>,
}

impl FileGen {
    fn switch(&mut self, section: &'static str) {
        if self.section != section {
            let directive = if section == ".rodata" {
                ".section .rodata"
            } else {
                section
            };
            // A blank line sets the new section apart from what came before.
            if !self.out.is_empty() {
                self.out.push('\n');
            }
            let _ = writeln!(self.out, "    {directive}");
            self.section = section;
        }
    }

    /// The label of a new string literal.
    fn string(&mut self, bytes: &[u8]) -> String {
        self.strings.push(bytes.to_vec());
        format!(".L.str.{}", self.strings.len() - 1)
    }

    /// Emits a global variable or `static` local: its symbol and its data.
    fn variable(&mut self, d: &Declarator) {
        let Some(VarRef::Global(sym)) = &d.var else {
            unreachable!("sema gives globals and statics a symbol")
        };
        let mut data = Vec::new();
        match &d.init {
            Some(init) => {
                self.data(&d.ty, init, &mut data);
                self.switch(".data");
            }
            None => self.switch(".bss"),
        }
        let size = d.ty.size().expect("sema: definitions have complete types");
        let storage = if d.storage == Storage::Static {
            "static "
        } else {
            ""
        };
        let bytes = if size == 1 { "byte" } else { "bytes" };
        let _ = writeln!(
            self.out,
            "\n    # {storage}{}: {size} {bytes}",
            d.ty.declare(&d.name)
        );
        if d.storage != Storage::Static && !sym.contains('.') {
            let _ = writeln!(self.out, "    .globl {sym}");
        }
        if d.ty.align() > 1 {
            let _ = writeln!(self.out, "    .balign {}", d.ty.align());
        }
        let _ = writeln!(self.out, "{sym}:");
        if d.init.is_none() {
            data.push(format!(".zero {size}"));
        }
        for line in data {
            let _ = writeln!(self.out, "    {line}");
        }
    }

    /// The directives for an object of type `ty` initialized by `init`.
    fn data(&mut self, ty: &Type, init: &Init, out: &mut Vec<String>) {
        match (ty, init) {
            (Type::Array(elem, Some(n)), Init::List(items, _)) => {
                for item in items {
                    self.data(elem, item, out);
                }
                let rest = (*n as usize - items.len()) as u32 * elem.size().unwrap();
                if rest > 0 {
                    out.push(format!(".zero {rest}"));
                }
            }
            (
                Type::Array(_, Some(n)),
                Init::Expr(Expr {
                    kind: ExprKind::Str(bytes),
                    ..
                }),
            ) => {
                // The text, cut to the array (C allows leaving out the NUL), then zeros.
                let take = bytes.len().min(*n as usize);
                out.extend(byte_directives(&bytes[..take]));
                if *n as usize > take {
                    out.push(format!(".zero {}", *n as usize - take));
                }
            }
            (_, Init::Expr(e)) => {
                let value = consteval::eval(e).expect("sema: static initializers are constant");
                let directive = match ty.size() {
                    Some(1) => ".byte",
                    Some(2) => ".half",
                    _ => ".word",
                };
                let operand = match value {
                    Const::Int(v, _) => v.to_string(),
                    Const::Addr { base: None, offset } => offset.to_string(),
                    Const::Addr {
                        base: Some(base),
                        offset,
                    } => {
                        let sym = match base {
                            Base::Sym(s) => s,
                            Base::Str(bytes) => self.string(&bytes),
                        };
                        match offset {
                            0 => sym,
                            o if o > 0 => format!("{sym}+{o}"),
                            o => format!("{sym}{o}"),
                        }
                    }
                };
                out.push(format!("{directive} {operand}"));
            }
            _ => unreachable!("sema checked initializer shapes"),
        }
    }

    /// Emits every string literal, NUL-terminated, in `.rodata`.
    fn strings(&mut self) {
        if self.strings.is_empty() {
            return;
        }
        self.switch(".rodata");
        for (i, bytes) in std::mem::take(&mut self.strings).iter().enumerate() {
            let _ = writeln!(self.out, "\n.L.str.{i}:");
            let mut with_nul = bytes.clone();
            with_nul.push(0);
            for line in byte_directives(&with_nul) {
                let _ = writeln!(self.out, "    {line}");
            }
        }
    }
}

/// Bytes as data directives: text as `.ascii "..."` (or `.asciz` when it ends
/// with the NUL), anything else as `.byte` lists. Only escapes that every
/// assembler reads the same are used (`\n \t \" \\`).
fn byte_directives(bytes: &[u8]) -> Vec<String> {
    let printable = |b: u8| (0x20..0x7f).contains(&b) || b == b'\n' || b == b'\t';
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let start = i;
        if printable(bytes[i]) {
            while i < bytes.len() && printable(bytes[i]) {
                i += 1;
            }
            let mut text = String::new();
            for &b in &bytes[start..i] {
                match b {
                    b'"' => text.push_str("\\\""),
                    b'\\' => text.push_str("\\\\"),
                    b'\n' => text.push_str("\\n"),
                    b'\t' => text.push_str("\\t"),
                    _ => text.push(b as char),
                }
            }
            // A NUL right after the text: one `.asciz` says both.
            if i + 1 == bytes.len() && bytes[i] == 0 {
                out.push(format!(".asciz \"{text}\""));
                i += 1;
            } else {
                out.push(format!(".ascii \"{text}\""));
            }
        } else {
            while i < bytes.len() && !printable(bytes[i]) && i - start < 16 {
                i += 1;
            }
            let list: Vec<String> = bytes[start..i].iter().map(u8::to_string).collect();
            out.push(format!(".byte {}", list.join(", ")));
        }
    }
    out
}

/// One line of a function's assembly.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Line {
    /// `name:`
    Label(String),
    /// An instruction, already formatted, and the most bytes it can assemble
    /// to (pseudo-instructions like `li`, `la` and `call` can take two words).
    Insn(String, u32),
    /// `beqz a0, target` (`if_zero`) or `bnez a0, target`: a conditional jump
    /// whose reach is limited, see [`relax_branches`].
    Branch { if_zero: bool, target: String },
    /// `# text`, or a directive such as `.globl f`: no bytes.
    Text(String),
}

/// Where the target of an assignment is, once its address is known.
enum Place {
    /// A scalar local, at this offset from `s0`.
    Frame(i32),
    /// Anywhere else: its address is the top temporary on the stack.
    Pushed,
}

/// Per-function code generator state. `'a` is the tree's lifetime, `'f` the
/// (shorter) borrow of the file's state.
struct FuncGen<'a, 'f> {
    func: &'a Function,
    /// Offset from `s0` of each local.
    offsets: Vec<i32>,
    frame: i32,
    lines: Vec<Line>,
    /// How many 4-byte temporaries expression evaluation has pushed right now.
    /// Always 0 between statements.
    depth: u32,
    /// `(continue target, break target)` for each enclosing loop, innermost last.
    loops: Vec<(String, String)>,
    file: &'f mut FileGen,
    /// Every `return` jumps here, to the epilogue.
    ret_label: String,
    /// The function's `static` locals, emitted as data after it.
    statics: Vec<&'a Declarator>,
}

impl<'a> FuncGen<'a, '_> {
    // ---- emitting lines ----

    /// Emits a one-word instruction: `mnemonic operands`.
    fn ins(&mut self, mnemonic: &str, operands: impl AsRef<str>) {
        self.ins_sized(4, mnemonic, operands);
    }

    fn ins_sized(&mut self, size: u32, mnemonic: &str, operands: impl AsRef<str>) {
        self.lines
            .push(Line::Insn(insn_text(mnemonic, operands.as_ref()), size));
    }

    /// `li reg, value`: one instruction for small values, two for others.
    fn li(&mut self, reg: &str, value: i32) {
        let size = if fits_imm12(value) { 4 } else { 8 };
        self.ins_sized(size, "li", format!("{reg}, {value}"));
    }

    /// `la reg, symbol`: `auipc` + `addi`.
    fn la(&mut self, reg: &str, symbol: &str) {
        self.ins_sized(8, "la", format!("{reg}, {symbol}"));
    }

    fn comment(&mut self, text: impl AsRef<str>) {
        self.lines.push(Line::Text(format!("# {}", text.as_ref())));
    }

    fn label(&mut self, name: &str) {
        self.lines.push(Line::Label(name.to_string()));
    }

    fn branch_if_zero(&mut self, target: &str) {
        self.lines.push(Line::Branch {
            if_zero: true,
            target: target.to_string(),
        });
    }

    fn branch_if_nonzero(&mut self, target: &str) {
        self.lines.push(Line::Branch {
            if_zero: false,
            target: target.to_string(),
        });
    }

    /// A fresh label name, `.L.<what>.<n>`.
    fn new_label(&mut self, what: &str) -> String {
        self.file.labels += 1;
        format!(".L.{what}.{}", self.file.labels)
    }

    /// `rd = rs + imm` for any `imm`; large ones go through `t0`.
    fn add_imm(&mut self, rd: &str, rs: &str, imm: i32) {
        if fits_imm12(imm) {
            self.ins("addi", format!("{rd}, {rs}, {imm}"));
        } else {
            self.li("t0", imm);
            self.ins("add", format!("{rd}, {rs}, t0"));
        }
    }

    /// A load or store (`op`) of `reg` at `offset(s0)`. Offsets beyond 12 bits
    /// (frames over 2 KiB) compute the address in `t0` first.
    fn access_local(&mut self, op: &str, reg: &str, offset: i32) {
        if fits_imm12(offset) {
            self.ins(op, format!("{reg}, {offset}(s0)"));
        } else {
            self.add_imm("t0", "s0", offset);
            self.ins(op, format!("{reg}, 0(t0)"));
        }
    }

    fn offset(&self, id: LocalId) -> i32 {
        self.offsets[id.0]
    }

    // ---- the stack of temporaries ----

    /// Pushes `a0`.
    fn push(&mut self) {
        self.ins("addi", "sp, sp, -4");
        self.ins("sw", "a0, 0(sp)");
        self.depth += 1;
    }

    /// Pops the top temporary into `reg`.
    fn pop(&mut self, reg: &str) {
        self.ins("lw", format!("{reg}, 0(sp)"));
        self.ins("addi", "sp, sp, 4");
        self.depth -= 1;
    }

    // ---- functions ----

    fn function(&mut self, body: &'a [Stmt]) {
        let f = self.func;

        // A header that maps every local to its slot, for whoever reads the output.
        let params: Vec<String> = f.params.iter().map(|p| p.ty.declare(&p.name)).collect();
        let params = if params.is_empty() {
            "void".to_string()
        } else {
            params.join(", ")
        };
        let frame = self.frame;
        self.comment(format!(
            "{}: frame {frame} bytes",
            f.ret.declare(&format!("{}({params})", f.name))
        ));
        for (i, l) in f.locals.iter().enumerate() {
            let what = if i < f.params.len() { "param" } else { "local" };
            let off = self.offsets[i];
            self.comment(format!("  {off:>5}(s0)  {what} {}", l.ty.declare(&l.name)));
        }
        if !f.is_static {
            self.lines.push(Line::Text(format!(".globl {}", f.name)));
        }
        self.label(&f.name);

        // Prologue: save ra and the caller's s0, set up s0, make room for locals.
        self.add_imm("sp", "sp", -SAVE_AREA);
        self.ins("sw", format!("ra, {}(sp)", SAVE_AREA - 4));
        self.ins("sw", format!("s0, {}(sp)", SAVE_AREA - 8));
        self.add_imm("s0", "sp", SAVE_AREA);
        if frame > SAVE_AREA {
            self.add_imm("sp", "sp", -(frame - SAVE_AREA));
        }
        // Parameters arrive in a0-a7; give them their slots like any local.
        for (i, p) in f.params.iter().enumerate() {
            let off = self.offsets[i];
            self.access_local(store_op(&p.ty), &format!("a{i}"), off);
        }

        for s in body {
            self.stmt(s);
        }

        // Falling off the end returns 0. A final `return` needs no jump to the
        // epilogue, which comes right after it.
        if self.lines.last() == Some(&Line::Insn(insn_text("j", &self.ret_label), 4)) {
            self.lines.pop();
        } else {
            self.li("a0", 0);
        }

        // Epilogue: the result is in a0. Free the frame from s0 (whatever its
        // size), restore ra and s0, return.
        let ret = self.ret_label.clone();
        self.label(&ret);
        self.ins("addi", format!("sp, s0, {}", -SAVE_AREA));
        self.ins("lw", format!("ra, {}(sp)", SAVE_AREA - 4));
        self.ins("lw", format!("s0, {}(sp)", SAVE_AREA - 8));
        self.ins("addi", format!("sp, sp, {SAVE_AREA}"));
        self.ins("ret", "");
    }

    // ---- statements ----

    fn stmt(&mut self, s: &'a Stmt) {
        debug_assert_eq!(self.depth, 0, "temporaries left on the stack between statements");
        match &s.kind {
            StmtKind::Block(body) => {
                for s in body {
                    self.stmt(s);
                }
            }
            StmtKind::Decl(decls) => {
                for d in decls {
                    self.local_decl(d);
                }
            }
            StmtKind::Expr(e) => self.expr(e),
            StmtKind::Empty => {}
            StmtKind::If { cond, then, els } => {
                //     <cond>                    <cond>
                //     beqz a0, .L.end.N         beqz a0, .L.else.N
                //     <then>                    <then>
                //   .L.end.N:                   j    .L.end.N
                //                             .L.else.N:
                //                               <els>
                //                             .L.end.N:
                self.expr(cond);
                match els {
                    None => {
                        let end = self.new_label("end");
                        self.branch_if_zero(&end);
                        self.stmt(then);
                        self.label(&end);
                    }
                    Some(els) => {
                        let else_ = self.new_label("else");
                        let end = self.new_label("end");
                        self.branch_if_zero(&else_);
                        self.stmt(then);
                        self.ins("j", &end);
                        self.label(&else_);
                        self.stmt(els);
                        self.label(&end);
                    }
                }
            }
            StmtKind::While { cond, body } => {
                //   .L.continue.N:      (`continue` re-tests the condition)
                //     <cond>
                //     beqz a0, .L.break.N
                //     <body>
                //     j    .L.continue.N
                //   .L.break.N:
                let cont = self.new_label("continue");
                let brk = self.new_label("break");
                self.label(&cont);
                self.expr(cond);
                self.branch_if_zero(&brk);
                self.loop_body(body, &cont, &brk);
                self.ins("j", &cont);
                self.label(&brk);
            }
            StmtKind::DoWhile { body, cond } => {
                //   .L.do.N:
                //     <body>
                //   .L.continue.N:      (`continue` tests the condition)
                //     <cond>
                //     bnez a0, .L.do.N
                //   .L.break.N:
                let top = self.new_label("do");
                let cont = self.new_label("continue");
                let brk = self.new_label("break");
                self.label(&top);
                self.loop_body(body, &cont, &brk);
                self.label(&cont);
                self.expr(cond);
                self.branch_if_nonzero(&top);
                self.label(&brk);
            }
            StmtKind::For {
                init,
                cond,
                step,
                body,
            } => {
                //     <init>
                //   .L.loop.N:
                //     <cond>            (no condition: loop forever)
                //     beqz a0, .L.break.N
                //     <body>
                //   .L.continue.N:      (`continue` still runs the step)
                //     <step>
                //     j    .L.loop.N
                //   .L.break.N:
                if let Some(init) = init {
                    self.stmt(init);
                }
                let top = self.new_label("loop");
                let cont = self.new_label("continue");
                let brk = self.new_label("break");
                self.label(&top);
                if let Some(cond) = cond {
                    self.expr(cond);
                    self.branch_if_zero(&brk);
                }
                self.loop_body(body, &cont, &brk);
                self.label(&cont);
                if let Some(step) = step {
                    self.expr(step);
                }
                self.ins("j", &top);
                self.label(&brk);
            }
            StmtKind::Return(e) => {
                if let Some(e) = e {
                    self.expr(e);
                }
                let ret = self.ret_label.clone();
                self.ins("j", ret);
            }
            StmtKind::Break => {
                let (_, brk) = self
                    .loops
                    .last()
                    .expect("sema rejects break outside a loop")
                    .clone();
                self.ins("j", brk);
            }
            StmtKind::Continue => {
                let (cont, _) = self
                    .loops
                    .last()
                    .expect("sema rejects continue outside a loop")
                    .clone();
                self.ins("j", cont);
            }
        }
    }

    fn loop_body(&mut self, body: &'a Stmt, cont: &str, brk: &str) {
        self.loops.push((cont.to_string(), brk.to_string()));
        self.stmt(body);
        self.loops.pop();
    }

    fn local_decl(&mut self, d: &'a Declarator) {
        let id = match &d.var {
            // A static local is data, emitted after the function.
            Some(VarRef::Global(_)) => return self.statics.push(d),
            Some(VarRef::Local(id)) => *id,
            None => unreachable!("sema resolves every declarator"),
        };
        // Without an initializer the local is left as it is (C: indeterminate).
        let Some(init) = &d.init else { return };
        let off = self.offset(id);
        if d.ty.is_array() {
            // Clear the whole array, then store what the initializer gives.
            self.comment(format!("{} = ...", d.ty.declare(&d.name)));
            self.clear_local(off, d.ty.size().unwrap() as i32);
        }
        self.init_local(&d.ty, init, off);
    }

    /// Stores an initializer into the local object at `off(s0)`; arrays have
    /// been cleared, so zeros are skipped.
    fn init_local(&mut self, ty: &Type, init: &'a Init, off: i32) {
        match (ty, init) {
            (Type::Array(elem, _), Init::List(items, _)) => {
                let size = elem.size().unwrap() as i32;
                for (i, item) in items.iter().enumerate() {
                    self.init_local(elem, item, off + i as i32 * size);
                }
            }
            (
                Type::Array(_, Some(n)),
                Init::Expr(Expr {
                    kind: ExprKind::Str(bytes),
                    ..
                }),
            ) => {
                for (i, &b) in bytes.iter().take(*n as usize).enumerate() {
                    if b != 0 {
                        self.li("a0", b.into());
                        self.access_local("sb", "a0", off + i as i32);
                    }
                }
            }
            (_, Init::Expr(e)) => {
                self.expr(e);
                self.access_local(store_op(ty), "a0", off);
            }
            _ => unreachable!("sema checked initializer shapes"),
        }
    }

    /// Clears `size` bytes at `off(s0)` (word-aligned, see [`layout`]): word
    /// stores, or a loop for more than 16 words, then the last bytes.
    fn clear_local(&mut self, off: i32, size: i32) {
        let words = size / 4;
        if words > 16 {
            self.add_imm("t1", "s0", off); //  t1 = start
            self.li("t0", words * 4); //       t0 = end
            self.ins("add", "t0, t1, t0");
            self.label("2");
            self.ins("sw", "zero, 0(t1)");
            self.ins("addi", "t1, t1, 4");
            self.ins("bltu", "t1, t0, 2b");
        } else {
            for w in 0..words {
                self.access_local("sw", "zero", off + 4 * w);
            }
        }
        for b in words * 4..size {
            self.access_local("sb", "zero", off + b);
        }
    }

    // ---- expressions: each leaves its value in a0 ----

    fn expr(&mut self, e: &'a Expr) {
        match &e.kind {
            ExprKind::Num(v) => {
                // The bit pattern: an `unsigned int` 0xffffffff is `li a0, -1`.
                self.li("a0", IntKind::Int.wrap(*v) as i32);
            }
            ExprKind::Str(bytes) => {
                let label = self.file.string(bytes);
                self.la("a0", &label);
            }
            ExprKind::Var { var, .. } => match var {
                Some(VarRef::Local(id)) => {
                    let off = self.offset(*id);
                    if e.ty.is_array() {
                        self.add_imm("a0", "s0", off); // an array's value is its address
                    } else {
                        self.access_local(load_op(&e.ty), "a0", off);
                    }
                }
                Some(VarRef::Global(sym)) => {
                    self.la("a0", sym);
                    self.load(&e.ty);
                }
                None => unreachable!("sema resolves every variable"),
            },
            ExprKind::Assign {
                op: None,
                target,
                value,
            } => {
                let place = self.place(target);
                self.expr(value); // already converted to the target's type by sema
                self.store_place(place, &target.ty);
            }
            ExprKind::Assign {
                op: Some(op),
                target,
                value,
            } => {
                let place = self.place(target);
                self.load_place(&place, &target.ty);
                self.push();
                self.expr(value);
                self.pop("a1"); // a1 = the target's value, a0 = value
                let (t, v) = (target.ty.decay(), value.ty.decay());
                self.binary(*op, &t, &v);
                let result = arith_type(*op, &t, &v).expect("sema checked the operands");
                self.convert(&result, &target.ty);
                self.store_place(place, &target.ty);
            }
            ExprKind::IncDec { inc, prefix, target } => {
                let ty = &target.ty;
                let step = match ty {
                    Type::Ptr(to) => to.size().expect("sema: pointer to a complete type") as i32,
                    _ => 1,
                };
                let delta = if *inc { step } else { -step };
                let place = self.place(target);
                self.load_place(&place, ty);
                self.add_imm("a0", "a0", delta);
                self.wrap_int(ty);
                self.store_place(place, ty);
                if !prefix {
                    // x++ gives the old value: the new one minus the step, wrapped again.
                    self.add_imm("a0", "a0", -delta);
                    self.wrap_int(ty);
                }
            }
            ExprKind::Binary(BinOp::LogAnd, l, r) => {
                //     <l>
                //     beqz a0, .L.and.N   # l is 0: so is the result, skip r
                //     <r>
                //   .L.and.N:
                //     snez a0, a0         # 0 or 1
                let end = self.new_label("and");
                self.expr(l);
                self.branch_if_zero(&end);
                self.expr(r);
                self.label(&end);
                self.ins("snez", "a0, a0");
            }
            ExprKind::Binary(BinOp::LogOr, l, r) => {
                // The mirror image: a nonzero l skips r and becomes 1.
                let end = self.new_label("or");
                self.expr(l);
                self.branch_if_nonzero(&end);
                self.expr(r);
                self.label(&end);
                self.ins("snez", "a0, a0");
            }
            ExprKind::Binary(op, l, r) => {
                self.expr(l);
                self.push();
                self.expr(r);
                self.pop("a1");
                // Now a1 = l, a0 = r.
                self.binary(*op, &l.ty.decay(), &r.ty.decay());
            }
            ExprKind::Unary(op, x) => {
                self.expr(x);
                match op {
                    UnOp::Neg => self.ins("neg", "a0, a0"),    // sub a0, zero, a0
                    UnOp::Plus => {}                           // promotion only: nothing to do
                    UnOp::Not => self.ins("seqz", "a0, a0"),   // a0 = (a0 == 0)
                    UnOp::BitNot => self.ins("not", "a0, a0"), // xori a0, a0, -1
                }
            }
            ExprKind::Addr(x) => self.addr(x),
            ExprKind::Deref(p) => {
                self.expr(p);
                self.load(&e.ty);
            }
            // A constant converted (`char c = 1;`): just the converted value.
            ExprKind::Cast { x, .. } if e.ty.is_integer() && matches!(x.kind, ExprKind::Num(_)) => {
                let Some(Const::Int(v, _)) = consteval::eval(e) else {
                    unreachable!("an integer literal cast to an integer type is constant")
                };
                self.li("a0", IntKind::Int.wrap(v) as i32);
            }
            ExprKind::Cast { x, .. } => {
                self.expr(x);
                self.convert(&x.ty, &e.ty);
            }
            ExprKind::SizeofType(_) | ExprKind::SizeofExpr(_) => unreachable!("sema folds sizeof"),
            ExprKind::Cond(c, a, b) => {
                //     <c>
                //     beqz a0, .L.else.N
                //     <a>
                //     j    .L.end.N
                //   .L.else.N:
                //     <b>
                //   .L.end.N:
                let else_ = self.new_label("else");
                let end = self.new_label("end");
                self.expr(c);
                self.branch_if_zero(&else_);
                self.expr(a);
                self.ins("j", &end);
                self.label(&else_);
                self.expr(b);
                self.label(&end);
            }
            ExprKind::Comma(a, b) => {
                self.expr(a);
                self.expr(b);
            }
            ExprKind::Call(name, args) => self.call(name, args),
        }
    }

    /// Loads the value of type `ty` at the address in `a0`. Arrays are not
    /// loaded: their value is that address.
    fn load(&mut self, ty: &Type) {
        if !ty.is_array() {
            self.ins(load_op(ty), "a0, 0(a0)");
        }
    }

    /// Leaves the address of the lvalue `e` in `a0`.
    fn addr(&mut self, e: &'a Expr) {
        match &e.kind {
            ExprKind::Var { var, .. } => match var {
                Some(VarRef::Local(id)) => {
                    let off = self.offset(*id);
                    self.add_imm("a0", "s0", off);
                }
                Some(VarRef::Global(sym)) => self.la("a0", sym),
                None => unreachable!("sema resolves every variable"),
            },
            // The address of *p is p.
            ExprKind::Deref(p) => self.expr(p),
            ExprKind::Str(_) => self.expr(e),
            _ => unreachable!("sema only takes the address of lvalues"),
        }
    }

    /// Prepares to store to `e`; see [`Place`].
    fn place(&mut self, e: &'a Expr) -> Place {
        if let ExprKind::Var {
            var: Some(VarRef::Local(id)),
            ..
        } = &e.kind
        {
            return Place::Frame(self.offset(*id));
        }
        self.addr(e);
        self.push();
        Place::Pushed
    }

    /// Loads the value at a place into `a0`; a pushed address stays pushed.
    fn load_place(&mut self, place: &Place, ty: &Type) {
        match place {
            Place::Frame(off) => self.access_local(load_op(ty), "a0", *off),
            Place::Pushed => {
                self.ins("lw", "a0, 0(sp)"); // the address, left on the stack
                self.load(ty);
            }
        }
    }

    /// Stores `a0` to a place, popping a pushed address.
    fn store_place(&mut self, place: Place, ty: &Type) {
        match place {
            Place::Frame(off) => self.access_local(store_op(ty), "a0", off),
            Place::Pushed => {
                self.pop("a1");
                self.ins(store_op(ty), "a0, 0(a1)");
            }
        }
    }

    /// Converts the value in `a0` from type `from` to type `to`. Only a
    /// narrowing to `char` or `short` changes bits: everything else is 32 bits
    /// already, and an array's value is its address, already a pointer.
    fn convert(&mut self, from: &Type, to: &Type) {
        if let (Type::Int(f), Type::Int(t)) = (from, to) {
            // A narrower or equal type whose values all fit is already right.
            let fits = (f.size() < t.size() && (t.signed() || !f.signed()))
                || (f.size() == t.size() && f.signed() == t.signed());
            if fits {
                return;
            }
        }
        self.wrap_int(to);
    }

    /// If `ty` is narrower than 32 bits, cuts `a0` down to it and widens it
    /// back by its signedness.
    fn wrap_int(&mut self, ty: &Type) {
        let Type::Int(k) = ty else { return };
        let shift = 32 - 8 * k.size();
        match (k.size(), k.signed()) {
            (4, _) => {}
            (1, false) => self.ins("andi", "a0, a0, 255"),
            (_, signed) => {
                self.ins("slli", format!("a0, a0, {shift}"));
                let right = if signed { "srai" } else { "srli" };
                self.ins(right, format!("a0, a0, {shift}"));
            }
        }
    }

    /// Multiplies `reg` by `size`: the scaling of pointer arithmetic.
    fn scale(&mut self, reg: &str, size: u32) {
        match log2(size) {
            Some(0) => {}
            Some(n) => self.ins("slli", format!("{reg}, {reg}, {n}")),
            None => {
                self.li("t0", size as i32);
                self.ins("mul", format!("{reg}, {reg}, t0"));
            }
        }
    }

    /// `a0 = a1 op a0` for the operators that always evaluate both sides, with
    /// `l` and `r` the (decayed) types of a1 and a0.
    fn binary(&mut self, op: BinOp, l: &Type, r: &Type) {
        // Comparisons, division and remainder follow the common type's
        // signedness; a pointer compares as an unsigned address.
        let unsigned = l.is_unsigned() || r.is_unsigned();
        let pick = |s: &'static str, u: &'static str| if unsigned { u } else { s };
        let elem_size = |t: &Type| t.pointee().and_then(Type::size).expect("sema: complete pointee");
        match op {
            BinOp::Add => {
                // A pointer plus an integer moves by whole elements.
                if l.is_ptr() {
                    self.scale("a0", elem_size(l));
                } else if r.is_ptr() {
                    self.scale("a1", elem_size(r));
                }
                self.ins("add", "a0, a1, a0");
            }
            BinOp::Sub if l.is_ptr() && r.is_ptr() => {
                // The difference of two pointers counts elements.
                self.ins("sub", "a0, a1, a0");
                match log2(elem_size(l)) {
                    Some(0) => {}
                    Some(n) => self.ins("srai", format!("a0, a0, {n}")),
                    None => {
                        self.li("t0", elem_size(l) as i32);
                        self.ins("div", "a0, a0, t0");
                    }
                }
            }
            BinOp::Sub => {
                if l.is_ptr() {
                    self.scale("a0", elem_size(l));
                }
                self.ins("sub", "a0, a1, a0");
            }
            BinOp::Mul => self.ins("mul", "a0, a1, a0"),
            BinOp::Div => self.ins(pick("div", "divu"), "a0, a1, a0"),
            BinOp::Rem => self.ins(pick("rem", "remu"), "a0, a1, a0"),
            BinOp::Shl => self.ins("sll", "a0, a1, a0"),
            // Only the left operand's type matters for a shift.
            BinOp::Shr => {
                let op = if l.is_unsigned() { "srl" } else { "sra" };
                self.ins(op, "a0, a1, a0");
            }
            BinOp::BitAnd => self.ins("and", "a0, a1, a0"),
            BinOp::BitOr => self.ins("or", "a0, a1, a0"),
            BinOp::BitXor => self.ins("xor", "a0, a1, a0"),
            BinOp::Lt => self.ins(pick("slt", "sltu"), "a0, a1, a0"), // l < r
            BinOp::Gt => self.ins(pick("slt", "sltu"), "a0, a0, a1"), // r < l
            BinOp::Le => {
                self.ins(pick("slt", "sltu"), "a0, a0, a1"); // l <= r is !(r < l)
                self.ins("xori", "a0, a0, 1");
            }
            BinOp::Ge => {
                self.ins(pick("slt", "sltu"), "a0, a1, a0"); // l >= r is !(l < r)
                self.ins("xori", "a0, a0, 1");
            }
            BinOp::Eq => {
                self.ins("xor", "a0, a1, a0"); // 0 exactly when l == r
                self.ins("seqz", "a0, a0");
            }
            BinOp::Ne => {
                self.ins("xor", "a0, a1, a0");
                self.ins("snez", "a0, a0");
            }
            BinOp::LogAnd | BinOp::LogOr => unreachable!("handled in expr: they short-circuit"),
        }
    }

    fn call(&mut self, name: &str, args: &'a [Expr]) {
        assert!(args.len() <= 8, "sema allows at most 8 arguments");
        // Evaluate every argument before loading any argument register: a later
        // argument may itself contain a call, which would clobber a0-a7. So all
        // but the last are pushed; the last one, already in a0, goes straight
        // to its register, and then the others are popped into theirs.
        if let Some((last, rest)) = args.split_last() {
            for a in rest {
                self.expr(a);
                self.push();
            }
            self.expr(last);
            if !rest.is_empty() {
                self.ins("mv", format!("a{}, a0", rest.len()));
            }
            for i in (0..rest.len()).rev() {
                self.pop(&format!("a{i}"));
            }
        }
        // Temporaries of an enclosing expression may still be pushed (in
        // `x + f(y)`, x is). The convention wants sp 16-byte aligned at the
        // call, so pad the stack to the next multiple of 16 around it.
        let pad = (16 - (self.depth as i32 * 4) % 16) % 16;
        if pad != 0 {
            self.ins("addi", format!("sp, sp, {}", -pad));
        }
        // `call` is `auipc ra` + `jalr ra`: two words.
        self.ins_sized(8, "call", name);
        if pad != 0 {
            self.ins("addi", format!("sp, sp, {pad}"));
        }
    }
}

/// `mnemonic operands`, with the operands lined up in a column.
fn insn_text(mnemonic: &str, operands: &str) -> String {
    if operands.is_empty() {
        mnemonic.to_string()
    } else {
        format!("{mnemonic:<5} {operands}")
    }
}

/// B-type branches reach -4096..=+4094 bytes from themselves.
const BRANCH_REACH: std::ops::RangeInclusive<i64> = -4096..=4094;

/// Decides which conditional branches need the long form (the opposite branch
/// over a `j`). Returns one flag per line, true for a branch that does.
///
/// Addresses are estimated with every pseudo-instruction at its largest
/// (`li` as two words, say), so a distance is never underestimated; a branch
/// that is safe here is safe in the assembled code. Making a branch long moves
/// code after it, which can push another branch out of reach, so this repeats
/// until nothing changes. It always ends: branches only ever become long.
fn relax_branches(lines: &[Line]) -> Vec<bool> {
    let mut long = vec![false; lines.len()];
    loop {
        // Where each line starts and where each label is, with the current choice.
        let mut addr = Vec::with_capacity(lines.len());
        let mut labels = HashMap::new();
        let mut pc: i64 = 0;
        for (i, line) in lines.iter().enumerate() {
            addr.push(pc);
            pc += match line {
                Line::Label(name) => {
                    labels.insert(name.as_str(), pc);
                    0
                }
                Line::Insn(_, size) => i64::from(*size),
                Line::Branch { .. } if long[i] => 8,
                Line::Branch { .. } => 4,
                Line::Text(_) => 0,
            };
        }
        let mut changed = false;
        for (i, line) in lines.iter().enumerate() {
            if let Line::Branch { target, .. } = line {
                let distance = labels[target.as_str()] - addr[i];
                if !long[i] && !BRANCH_REACH.contains(&distance) {
                    long[i] = true;
                    changed = true;
                }
            }
        }
        if !changed {
            return long;
        }
    }
}

/// Writes a function's lines as assembly text.
fn render(lines: &[Line], long: &[bool], out: &mut String) {
    for (line, &long) in lines.iter().zip(long) {
        match line {
            Line::Label(name) => {
                let _ = writeln!(out, "{name}:");
            }
            Line::Insn(text, _) => {
                let _ = writeln!(out, "    {text}");
            }
            Line::Text(text) => {
                let _ = writeln!(out, "    {text}");
            }
            Line::Branch { if_zero, target } => {
                let (near, inverse) = if *if_zero {
                    ("beqz", "bnez")
                } else {
                    ("bnez", "beqz")
                };
                if long {
                    // Too far for a branch: skip over a `j` when the condition fails.
                    let _ = writeln!(out, "    {}", insn_text(inverse, "a0, 1f"));
                    let _ = writeln!(out, "    {}", insn_text("j", target));
                    let _ = writeln!(out, "1:");
                } else {
                    let _ = writeln!(out, "    {}", insn_text(near, &format!("a0, {target}")));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn compile(src: &str) -> String {
        crate::compile(src).unwrap_or_else(|e| panic!("{e:?}"))
    }

    /// The instructions of `f`, without comments, labels or indentation.
    fn body_of(asm: &str, f: &str) -> Vec<String> {
        asm.lines()
            .skip_while(|l| *l != format!("{f}:"))
            .skip(1)
            .take_while(|l| *l != format!(".L.return.{f}:"))
            .map(str::trim)
            .filter(|l| !l.starts_with('#') && !l.ends_with(':'))
            .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
            .collect()
    }

    /// The instructions `main` runs for `body`, after the prologue.
    fn main_body(globals: &str, body: &str) -> String {
        let asm = compile(&format!("{globals}\nint main() {{ {body} }}"));
        body_of(&asm, "main")[4..].join("; ")
    }

    fn locals(decls: &str) -> (Vec<i32>, i32) {
        let p = crate::parse(&format!("int f() {{ {decls} }}")).unwrap();
        let f = p.funcs().next().unwrap();
        layout(&f.locals)
    }

    #[test]
    fn frame_layout() {
        // Only ints: local #i at -(12 + 4*i), as in v0.
        assert_eq!(locals("int a, b, c;"), (vec![-12, -16, -20], 32));
        assert_eq!(locals("int a, b;"), (vec![-12, -16], 16));
        assert_eq!(locals(""), (vec![], 16));
        // Each type at its own alignment, packed downwards.
        assert_eq!(
            locals("char c; short s; char d; int i;"),
            (vec![-9, -12, -13, -20], 32)
        );
        // Arrays are word-aligned.
        assert_eq!(locals("char c; char buf[5]; char d;"), (vec![-9, -16, -17], 32));
        assert_eq!(locals("int a[10];"), (vec![-48], 48));
    }

    #[test]
    fn smallest_function() {
        let asm = compile("int main() { return 42; }");
        assert_eq!(
            asm,
            "    .text

    # int main(void): frame 16 bytes
    .globl main
main:
    addi  sp, sp, -16
    sw    ra, 12(sp)
    sw    s0, 8(sp)
    addi  s0, sp, 16
    li    a0, 42
.L.return.main:
    addi  sp, s0, -16
    lw    ra, 12(sp)
    lw    s0, 8(sp)
    addi  sp, sp, 16
    ret
"
        );
    }

    #[test]
    fn prototypes_and_externs_emit_nothing() {
        let asm = compile("int f(int a); extern int g; int main() { return f(g); }");
        assert!(!asm.contains("f:"), "{asm}");
        assert!(!asm.contains("g:"), "{asm}");
        assert!(asm.contains("call  f\n"), "{asm}");
        assert!(asm.contains("la    a0, g\n"), "{asm}");
        assert_eq!(asm.matches(".globl").count(), 1);
    }

    #[test]
    fn static_functions_are_not_global() {
        let asm = compile("static int f() { return 1; } int main() { return f(); }");
        assert!(!asm.contains(".globl f"), "{asm}");
        assert!(asm.contains("\nf:\n"), "{asm}");
    }

    #[test]
    fn falling_off_the_end_returns_zero() {
        let body = body_of(&compile("int main() { int x = 3; }"), "main");
        assert_eq!(body.last().unwrap(), "li a0, 0");
        // A final return jumps nowhere: the epilogue is next.
        let body = body_of(&compile("int main() { return 3; }"), "main");
        assert_eq!(body.last().unwrap(), "li a0, 3");
        // A return elsewhere jumps to it.
        let asm = compile("int main(int a) { if (a) return 1; return 2; }");
        assert!(asm.contains("j     .L.return.main\n"), "{asm}");
    }

    #[test]
    fn parameters_are_stored_in_their_slots() {
        let asm = compile("int f(int a, char b, short c) { return c; } int main() { return f(1, 2, 3); }");
        let body = body_of(&asm, "f");
        assert_eq!(
            body,
            [
                "addi sp, sp, -16",
                "sw ra, 12(sp)",
                "sw s0, 8(sp)",
                "addi s0, sp, 16",
                // 8 + 4 + 1 (+1 to align) + 2 = 16 bytes: the save area is the frame.
                "sw a0, -12(s0)",
                "sb a1, -13(s0)",
                "sh a2, -16(s0)",
                "lh a0, -16(s0)",
            ]
        );
        assert!(
            asm.contains("# int f(int a, char b, short c): frame 16 bytes\n"),
            "{asm}"
        );
        assert!(asm.contains("#     -13(s0)  param char b\n"), "{asm}");
    }

    #[test]
    fn binary_operators() {
        let op = |ty: &str, src: &str| {
            let body = body_of(
                &compile(&format!("int main({ty} x, {ty} y) {{ return {src}; }}")),
                "main",
            );
            // Skip the prologue (4 lines), the parameter stores (2), `lw` x, the
            // push (2) and `lw` y: what is left is the pop and the operator.
            body[10..]
                .join("; ")
                .replace("lw a1, 0(sp); addi sp, sp, 4; ", "")
        };
        assert_eq!(op("int", "x + y"), "add a0, a1, a0");
        assert_eq!(op("int", "x - y"), "sub a0, a1, a0");
        assert_eq!(op("int", "x * y"), "mul a0, a1, a0");
        assert_eq!(op("int", "x / y"), "div a0, a1, a0");
        assert_eq!(op("int", "x % y"), "rem a0, a1, a0");
        assert_eq!(op("int", "x << y"), "sll a0, a1, a0");
        assert_eq!(op("int", "x >> y"), "sra a0, a1, a0");
        assert_eq!(op("int", "x & y"), "and a0, a1, a0");
        assert_eq!(op("int", "x | y"), "or a0, a1, a0");
        assert_eq!(op("int", "x ^ y"), "xor a0, a1, a0");
        assert_eq!(op("int", "x < y"), "slt a0, a1, a0");
        assert_eq!(op("int", "x > y"), "slt a0, a0, a1");
        assert_eq!(op("int", "x <= y"), "slt a0, a0, a1; xori a0, a0, 1");
        assert_eq!(op("int", "x >= y"), "slt a0, a1, a0; xori a0, a0, 1");
        assert_eq!(op("int", "x == y"), "xor a0, a1, a0; seqz a0, a0");
        assert_eq!(op("int", "x != y"), "xor a0, a1, a0; snez a0, a0");
        // Unsigned operations.
        assert_eq!(op("unsigned", "x / y"), "divu a0, a1, a0");
        assert_eq!(op("unsigned", "x % y"), "remu a0, a1, a0");
        assert_eq!(op("unsigned", "x >> y"), "srl a0, a1, a0");
        assert_eq!(op("unsigned", "x < y"), "sltu a0, a1, a0");
        assert_eq!(op("unsigned long", "x >= y"), "sltu a0, a1, a0; xori a0, a0, 1");
        // unsigned char promotes to int: signed.
        let body = body_of(
            &compile("int main(unsigned char x, unsigned char y) { return x / y; }"),
            "main",
        );
        assert_eq!(body.last().unwrap(), "div a0, a1, a0");
        assert!(body.contains(&"lbu a0, -9(s0)".to_string()), "{body:?}");
    }

    #[test]
    fn pointer_arithmetic_scales() {
        let p = |decls: &str, e: &str| main_body(decls, &format!("return {e};"));
        // int *: 4 bytes, a shift by 2.
        assert!(p("int *p; int i;", "*(p + i)").ends_with("slli a0, a0, 2; add a0, a1, a0; lw a0, 0(a0)"));
        assert!(p("int *p; int i;", "*(i + p)").ends_with("slli a1, a1, 2; add a0, a1, a0; lw a0, 0(a0)"));
        assert!(p("char *p; int i;", "*(p - i)").ends_with("sub a0, a1, a0; lbu a0, 0(a0)"));
        assert!(p("short *p, *q;", "p - q").ends_with("sub a0, a1, a0; srai a0, a0, 1"));
        // 12-byte elements: multiply and divide.
        assert!(p("int m[4][3]; int i;", "m[i][0]").contains("li t0, 12; mul a0, a0, t0"));
    }

    #[test]
    fn narrow_types_load_and_store_by_size() {
        assert_eq!(
            main_body("", "signed char c = 1; unsigned short s = 2; c = s; return c;"),
            "li a0, 1; sb a0, -9(s0); li a0, 2; sh a0, -12(s0); lhu a0, -12(s0); \
             slli a0, a0, 24; srai a0, a0, 24; sb a0, -9(s0); lb a0, -9(s0)"
        );
        // Widening is free.
        assert!(main_body("", "unsigned char c = 1; int i = c; return i;")
            .contains("lbu a0, -9(s0); sw a0, -16(s0)"));
    }

    #[test]
    fn conversions() {
        let cast = |from: &str, to: &str| {
            let body = body_of(
                &compile(&format!("int main({from} x) {{ return ({to})x; }}")),
                "main",
            );
            body[6..].join("; ")
        };
        assert_eq!(cast("int", "char"), "andi a0, a0, 255");
        assert_eq!(cast("int", "unsigned char"), "andi a0, a0, 255");
        assert_eq!(cast("int", "signed char"), "slli a0, a0, 24; srai a0, a0, 24");
        assert_eq!(cast("int", "short"), "slli a0, a0, 16; srai a0, a0, 16");
        assert_eq!(cast("int", "unsigned short"), "slli a0, a0, 16; srli a0, a0, 16");
        assert_eq!(cast("int", "unsigned"), "");
        assert_eq!(cast("char", "short"), "");
        assert_eq!(cast("char", "unsigned char"), "");
        assert_eq!(cast("char", "signed char"), "slli a0, a0, 24; srai a0, a0, 24");
        assert_eq!(cast("signed char", "char"), "andi a0, a0, 255");
        assert_eq!(
            cast("short", "unsigned short"),
            "slli a0, a0, 16; srli a0, a0, 16"
        );
    }

    #[test]
    fn places() {
        // A local: straight to its slot.
        assert_eq!(
            main_body("", "int x; x = 5;"),
            "li a0, 5; sw a0, -12(s0); li a0, 0"
        );
        // A global: through its address.
        assert_eq!(
            main_body("int g;", "g = 5;"),
            "la a0, g; addi sp, sp, -4; sw a0, 0(sp); li a0, 5; lw a1, 0(sp); addi sp, sp, 4; sw a0, 0(a1); \
             li a0, 0"
        );
        // Compound assignment evaluates the target once and keeps its address.
        assert_eq!(
            main_body("char *p;", "*p += 300;"),
            "la a0, p; lw a0, 0(a0); addi sp, sp, -4; sw a0, 0(sp); lw a0, 0(sp); lbu a0, 0(a0); \
             addi sp, sp, -4; sw a0, 0(sp); li a0, 300; lw a1, 0(sp); addi sp, sp, 4; add a0, a1, a0; \
             andi a0, a0, 255; lw a1, 0(sp); addi sp, sp, 4; sb a0, 0(a1); li a0, 0"
        );
        // Increments step by the pointed-to size; postfix gives the old value back.
        assert_eq!(
            main_body("", "int *p; return p++ == 0;").split("; xor").next().unwrap(),
            "lw a0, -12(s0); addi a0, a0, 4; sw a0, -12(s0); addi a0, a0, -4; addi sp, sp, -4; sw a0, 0(sp); \
             li a0, 0; lw a1, 0(sp); addi sp, sp, 4"
        );
        assert_eq!(
            main_body("", "unsigned char c; return --c;"),
            "lbu a0, -9(s0); addi a0, a0, -1; andi a0, a0, 255; sb a0, -9(s0)"
        );
    }

    #[test]
    fn arrays_are_addresses() {
        assert_eq!(
            main_body("", "int a[2]; return a[1];"),
            "addi a0, s0, -16; addi sp, sp, -4; sw a0, 0(sp); li a0, 1; lw a1, 0(sp); \
             addi sp, sp, 4; slli a0, a0, 2; add a0, a1, a0; lw a0, 0(a0)"
        );
        // &a and a are the same address; a global array is `la`.
        assert!(main_body("int g[3];", "return &g == 0;").starts_with("la a0, g; addi sp"));
    }

    #[test]
    fn local_array_initializers() {
        assert_eq!(
            main_body("", "int a[3] = {7}; return a[0];").split("; addi a0, s0").next().unwrap(),
            "addi sp, sp, -16; sw zero, -20(s0); sw zero, -16(s0); sw zero, -12(s0); li a0, 7; sw a0, -20(s0)"
        );
        assert_eq!(
            main_body("", "char s[] = \"a\\0b\"; return s[2];")
                .split("; addi a0, s0")
                .next()
                .unwrap(),
            "sw zero, -12(s0); li a0, 97; sb a0, -12(s0); li a0, 98; sb a0, -10(s0)"
        );
        // Big arrays are cleared with a loop.
        let body = main_body("", "int a[100] = {1}; return a[99];");
        assert!(
            body.contains("addi t1, s0, -408; li t0, 400; add t0, t1, t0; sw zero, 0(t1); addi t1, t1, 4; bltu t1, t0, 2b"),
            "{body}"
        );
    }

    #[test]
    fn global_data() {
        let asm = compile(
            "int g = 5; char c = 'x'; short h[3] = {1, -1}; int *p = &g + 1; char s[] = \"hi\\n\";
             static unsigned u; char *str = \"a\\\"b\\\\\"; char raw[3] = {1, 2, 255}; int zeros[2] = {0};
             int main() { static int n = 3; return n; }",
        );
        let data = &asm;
        for want in [
            // No empty `.text` first: sections are switched as needed.
            "    .data\n\n    # int g: 4 bytes\n    .globl g\n    .balign 4\ng:\n    .word 5\n",
            "\n    # char c: 1 byte\n    .globl c\nc:\n    .byte 120\n",
            "\n    .balign 2\nh:\n    .half 1\n    .half -1\n    .zero 2\n",
            "p:\n    .word g+4\n",
            "s:\n    .ascii \"hi\\n\"\n    .zero 1\n",
            "    .bss\n\n    # static unsigned int u: 4 bytes\n    .balign 4\nu:\n    .zero 4\n",
            "str:\n    .word .L.str.0\n",
            "raw:\n    .byte 1\n    .byte 2\n    .byte 255\n",
            "zeros:\n    .word 0\n    .zero 4\n",
            "\n    # static int n: 4 bytes\n    .balign 4\nmain.n:\n    .word 3\n",
            "    .section .rodata\n\n.L.str.0:\n    .asciz \"a\\\"b\\\\\"\n",
        ] {
            assert!(data.contains(want), "missing {want:?} in\n{data}");
        }
        assert!(data.starts_with("    .data\n"), "{data}");
        assert!(!data.contains(".globl u"), "static");
        assert!(!data.contains(".globl main.n"), "static local");
        assert!(asm.contains("la    a0, main.n\n"), "{asm}");
    }

    #[test]
    fn string_literals() {
        let asm = compile("char *f() { return \"one\"; } int main() { return f()[0] + \"\\x01\\x02z\"[1]; }");
        assert!(asm.contains("la    a0, .L.str.0\n"), "{asm}");
        assert!(asm.ends_with(
            "    .section .rodata\n\n.L.str.0:\n    .asciz \"one\"\n\n.L.str.1:\n    .byte 1, 2\n    .asciz \"z\"\n"
        ), "{asm}");
    }

    #[test]
    fn byte_directive_forms() {
        assert_eq!(byte_directives(b"abc"), [".ascii \"abc\""]);
        assert_eq!(byte_directives(b"abc\0"), [".asciz \"abc\""]);
        assert_eq!(byte_directives(b"\0"), [".byte 0"]);
        assert_eq!(
            byte_directives(b"a\0b\0"),
            [".ascii \"a\"", ".byte 0", ".asciz \"b\""]
        );
        assert_eq!(byte_directives(&[200, 201]), [".byte 200, 201"]);
        assert_eq!(byte_directives(&[1; 20]).len(), 2, "16 per line");
    }

    #[test]
    fn calls_pad_the_stack_to_16_bytes() {
        // No temporaries pushed: no padding.
        let asm = compile("int f(int a) { return a; } int main() { return f(1); }");
        let body = body_of(&asm, "main");
        assert_eq!(&body[4..], ["li a0, 1", "call f"]);
        // `1 + f(2)`: 1 is pushed (4 bytes), so 12 more bytes keep sp aligned.
        let asm = compile("int f(int a) { return a; } int main() { return 1 + f(2); }");
        let body = body_of(&asm, "main").join("; ");
        assert!(
            body.contains("li a0, 2; addi sp, sp, -12; call f; addi sp, sp, 12; lw a1, 0(sp)"),
            "{body}"
        );
    }

    #[test]
    fn arguments_are_evaluated_before_loading_registers() {
        let asm = compile("int f(int a, int b) { return a - b; } int main() { return f(f(5, 1), f(2, 3)); }");
        let body = body_of(&asm, "main").join("; ");
        // Both inner calls happen before a0/a1 are loaded for the outer one.
        let outer = body.rfind("call f").unwrap();
        assert!(
            body[..outer].ends_with("call f; addi sp, sp, 12; mv a1, a0; lw a0, 0(sp); addi sp, sp, 4; "),
            "{body}"
        );
    }

    #[test]
    fn arguments_and_results_are_converted() {
        let asm = compile("char f(char c) { return c + 1; } int main() { return f(300); }");
        // The argument is cut to a char (a constant: at compile time), and so is the result.
        assert_eq!(body_of(&asm, "main")[4..].join("; "), "li a0, 44; call f");
        assert!(body_of(&asm, "f")
            .join("; ")
            .ends_with("add a0, a1, a0; andi a0, a0, 255"));
    }

    #[test]
    fn labels_are_unique_across_functions() {
        let asm = compile(
            "int f(int a) { while (a) a = a - 1; do a++; while (a < 3); return a ? 1 : 2; }
             int main() { int i; for (i = 0; i < 3; i = i + 1) if (i) continue; else break; return f(i); }",
        );
        let mut defined: Vec<&str> = asm.lines().filter(|l| l.ends_with(':')).collect();
        let n = defined.len();
        defined.sort();
        defined.dedup();
        assert_eq!(defined.len(), n, "{asm}");
        assert!(asm.contains("j     .L.continue."), "{asm}");
        assert!(asm.contains("j     .L.break."), "{asm}");
        assert!(asm.contains("bnez  a0, .L.do."), "{asm}");
    }

    #[test]
    fn short_circuit_operators() {
        let asm = compile("int main(int a, int b) { return a && b; }");
        let body = body_of(&asm, "main").join("; ");
        assert!(
            body.ends_with("lw a0, -12(s0); beqz a0, .L.and.1; lw a0, -16(s0); snez a0, a0"),
            "{body}"
        );
        let asm = compile("int main(int a, int b) { return a || b; }");
        let body = body_of(&asm, "main").join("; ");
        assert!(
            body.ends_with("lw a0, -12(s0); bnez a0, .L.or.1; lw a0, -16(s0); snez a0, a0"),
            "{body}"
        );
    }

    #[test]
    fn big_frames_use_t0() {
        let decls: String = (0..600).map(|i| format!("int v{i} = {i};")).collect();
        let asm = compile(&format!("int main() {{ {decls} return v599; }}"));
        // 8 + 600 * 4 = 2408 bytes, rounded up to 2416; 16 come from the save area.
        let body = body_of(&asm, "main").join("; ");
        assert!(body.contains("li t0, -2400; add sp, sp, t0"), "{body}");
        // v509 is at -12 - 4 * 509 = -2048, the last slot a 12-bit offset reaches.
        assert!(body.contains("li a0, 509; sw a0, -2048(s0)"), "{body}");
        assert!(
            body.contains("li a0, 510; li t0, -2052; add t0, s0, t0; sw a0, 0(t0)"),
            "{body}"
        );
        assert!(
            body.ends_with("li t0, -2408; add t0, s0, t0; lw a0, 0(t0)"),
            "{body}"
        );
    }

    #[test]
    fn far_branches_are_rewritten() {
        let lines = |n| {
            let mut v = vec![Line::Branch {
                if_zero: true,
                target: "end".into(),
            }];
            v.extend((0..n).map(|_| Line::Insn("nop".into(), 4)));
            v.push(Line::Label("end".into()));
            v
        };
        // The branch and 1022 words before the label: +4092 bytes, in reach.
        assert!(!relax_branches(&lines(1022))[0]);
        // +4096 is not (forward, the limit is +4094).
        assert!(relax_branches(&lines(1023))[0]);

        // Backward: a label then a branch to it.
        let back = |n| {
            let mut v = vec![Line::Label("top".into())];
            v.extend((0..n).map(|_| Line::Insn("nop".into(), 4)));
            v.push(Line::Branch {
                if_zero: false,
                target: "top".into(),
            });
            v
        };
        assert!(!*relax_branches(&back(1024)).last().unwrap()); // -4096
        assert!(*relax_branches(&back(1025)).last().unwrap()); // -4100

        let l = lines(2000);
        let mut out = String::new();
        render(&l[..1], &relax_branches(&l)[..1], &mut out);
        assert_eq!(out, "    bnez  a0, 1f\n    j     end\n1:\n");
    }

    #[test]
    fn making_one_branch_long_can_push_another_out_of_reach() {
        // Branch A spans branch B and 1022 words: exactly in reach, until B
        // grows to two words. B spans 2000 words and is long from the start.
        let mut v = vec![Line::Branch {
            if_zero: true,
            target: "a".into(),
        }];
        v.push(Line::Branch {
            if_zero: true,
            target: "b".into(),
        });
        v.extend((0..1021).map(|_| Line::Insn("nop".into(), 4)));
        v.push(Line::Label("a".into()));
        v.extend((0..2000).map(|_| Line::Insn("nop".into(), 4)));
        v.push(Line::Label("b".into()));
        assert_eq!(&relax_branches(&v)[..2], [true, true]);
    }

    #[test]
    fn register_names_cannot_be_symbols() {
        for name in ["sp", "a0", "x5", "fp", "zero"] {
            for src in [
                format!("int {name}() {{ return 0; }} int main() {{ return 0; }}"),
                format!("int {name}; int main() {{ return 0; }}"),
            ] {
                let errs = crate::compile(&src).unwrap_err();
                assert_eq!(
                    errs[0].to_string(),
                    format!("1:5: '{name}' is a register name, which cannot be used as a symbol name")
                );
            }
        }
        // Locals may have any name: they have no symbol.
        compile("int main() { int a0 = 1; return a0; }");
    }
}
