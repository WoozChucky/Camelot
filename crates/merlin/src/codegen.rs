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
//! * Input: a [`Program`] that passed [`crate::sema`]. Every variable and
//!   declarator has `local: Some(LocalId(i))`, an index into its function's
//!   `locals` (parameters first, in order), so each local can live at a fixed
//!   offset from `s0`. Every call names a declared function with the right
//!   number of arguments (at most 8, in `a0-a7`).
//! * Output: assembly merlin-as accepts (and so does clang): `.text`,
//!   `.globl name` and `name:` for every function with a body; prototypes emit
//!   nothing (the linker finds them).
//! * Semantics: 32-bit wrapping arithmetic, `/` and `%` truncate toward zero
//!   (`div`/`rem`), comparisons and `! && ||` produce 0 or 1, `&&`/`||`
//!   short-circuit, `a = b` evaluates to the new value. Falling off the end of a
//!   function returns 0.
//!
//! ## Calling convention (RISC-V ILP32)
//!
//! * Arguments go in `a0-a7`, left to right; the result comes back in `a0`.
//! * `ra` holds the return address (`call f` is `auipc ra` + `jalr ra`).
//! * `sp`, `s0-s11` must look untouched to the caller when we return; the
//!   `a` and `t` registers may be clobbered by any call. We only ever use
//!   `s0` (saved in the prologue), `a0`, `a1`, the argument registers and `t0`.
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
//!          | local #0         | -12(s0)   parameters first: a0 is stored here,
//!          | local #1         | -16(s0)   a1 here, and so on
//!          | ...              |           local #i at -(12 + 4*i)(s0)
//!          | (padding)        |           frame size: 8 + 4*locals, rounded up to 16
//!   sp ->  +------------------+  (16-byte aligned) after the prologue
//!          | temporaries      |  pushed and popped while evaluating expressions
//! ```
//!
//! Sema gives every declaration in the function its own local (shadowed names
//! included), so a slot never has to be reused and scopes cost nothing here.
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
//! right side and turn the result into 0 or 1 with `snez`.
//!
//! A call evaluates its arguments left to right, pushing all but the last;
//! the last goes from `a0` to `a(n-1)` with `mv`, then the others are popped
//! into `a(n-2)` down to `a0`. If temporaries of an enclosing expression
//! are still pushed (as in `x + f(y)`), `sp` is lowered to a multiple of 16
//! around the `call`, to keep the convention's alignment.
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
use crate::Error;
use std::collections::HashMap;
use std::fmt::Write;

/// Generates assembly for a whole translation unit.
pub fn generate(prog: &Program) -> Result<String, Error> {
    // merlin-as reads a register name as a register, never as a symbol, so a
    // function called `sp` or `a0` could not be called or even defined.
    if let Some(f) = prog
        .funcs
        .iter()
        .find(|f| excalibur::reg::parse(&f.name).is_some())
    {
        return Err(Error::new(
            f.pos,
            format!(
                "'{}' is a register name, which cannot be used as a function name",
                f.name
            ),
        ));
    }

    let mut out = String::from("    .text\n");
    let mut labels = 0;
    for f in &prog.funcs {
        let Some(body) = &f.body else {
            continue; // a prototype: the definition is elsewhere
        };
        let mut g = FuncGen {
            func: f,
            lines: Vec::new(),
            depth: 0,
            loops: Vec::new(),
            labels: &mut labels,
            ret_label: format!(".L.return.{}", f.name),
        };
        g.function(body);
        let lines = g.lines;
        out.push('\n');
        render(&lines, &relax_branches(&lines), &mut out);
    }
    Ok(out)
}

/// `ra` and the caller's `s0` take the top 8 bytes of every frame.
const SAVED_REGS: i32 = 8;
/// The prologue first lowers `sp` by this much, enough for `ra` and `s0` and a
/// multiple of 16 so that `sp` stays aligned at every step.
const SAVE_AREA: i32 = 16;

/// Where local `#i` lives: its offset from `s0`.
fn slot(LocalId(i): LocalId) -> i32 {
    -(SAVED_REGS + 4) - 4 * i as i32
}

/// Size of a frame with `n` locals: saved registers plus locals, rounded up to
/// a multiple of 16 (the convention's stack alignment).
fn frame_size(n: usize) -> i32 {
    (SAVED_REGS + 4 * n as i32 + 15) & !15
}

/// Does `v` fit a 12-bit signed immediate (`addi`, `lw`, `sw`)?
fn fits_imm12(v: i32) -> bool {
    (-2048..=2047).contains(&v)
}

/// One line of a function's assembly.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Line {
    /// `name:`
    Label(String),
    /// An instruction, already formatted, and the most bytes it can assemble
    /// to (pseudo-instructions like `li` and `call` can take two words).
    Insn(String, u32),
    /// `beqz a0, target` (`if_zero`) or `bnez a0, target`: a conditional jump
    /// whose reach is limited, see [`relax_branches`].
    Branch { if_zero: bool, target: String },
    /// `# text`, or a directive such as `.globl f`: no bytes.
    Text(String),
}

/// Per-function code generator state.
struct FuncGen<'a> {
    func: &'a Function,
    lines: Vec<Line>,
    /// How many 4-byte temporaries expression evaluation has pushed right now.
    /// Always 0 between statements.
    depth: u32,
    /// `(continue target, break target)` for each enclosing loop, innermost last.
    loops: Vec<(String, String)>,
    /// Label counter shared by every function in the file, so labels are unique.
    labels: &'a mut usize,
    /// Every `return` jumps here, to the epilogue.
    ret_label: String,
}

impl FuncGen<'_> {
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
        *self.labels += 1;
        format!(".L.{what}.{}", self.labels)
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

    /// `lw`/`sw` of `reg` at `offset(s0)`. Offsets beyond 12 bits (a function
    /// with more than 509 locals) compute the address in `t0` first.
    fn access_local(&mut self, op: &str, reg: &str, offset: i32) {
        if fits_imm12(offset) {
            self.ins(op, format!("{reg}, {offset}(s0)"));
        } else {
            self.add_imm("t0", "s0", offset);
            self.ins(op, format!("{reg}, 0(t0)"));
        }
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

    fn function(&mut self, body: &[Stmt]) {
        let f = self.func;
        let frame = frame_size(f.locals.len());

        // A header that maps every local to its slot, for whoever reads the output.
        let params: Vec<&str> = f.params.iter().map(|p| p.name.as_str()).collect();
        self.comment(format!(
            "int {}({}): frame {frame} bytes",
            f.name,
            params.join(", ")
        ));
        for (i, l) in f.locals.iter().enumerate() {
            let what = if i < f.params.len() { "param" } else { "local" };
            self.comment(format!("  {:>5}(s0)  {what} {}", slot(LocalId(i)), l.name));
        }
        self.lines.push(Line::Text(format!(".globl {}", f.name)));
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
        for i in 0..f.params.len() {
            self.access_local("sw", &format!("a{i}"), slot(LocalId(i)));
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

    fn stmt(&mut self, s: &Stmt) {
        debug_assert_eq!(self.depth, 0, "temporaries left on the stack between statements");
        match &s.kind {
            StmtKind::Block(body) => {
                for s in body {
                    self.stmt(s);
                }
            }
            StmtKind::Decl(decls) => {
                // A declaration without an initializer leaves the slot as it is
                // (C: indeterminate). With one, it is an assignment.
                for d in decls {
                    if let Some(init) = &d.init {
                        self.expr(init);
                        self.access_local("sw", "a0", slot(local(d.local)));
                    }
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
                self.expr(e);
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

    fn loop_body(&mut self, body: &Stmt, cont: &str, brk: &str) {
        self.loops.push((cont.to_string(), brk.to_string()));
        self.stmt(body);
        self.loops.pop();
    }

    // ---- expressions: each leaves its value in a0 ----

    fn expr(&mut self, e: &Expr) {
        match &e.kind {
            ExprKind::Num(v) => {
                let v = i32::try_from(*v).expect("the lexer keeps literals within i32");
                self.li("a0", v);
            }
            ExprKind::Var { local: id, .. } => self.access_local("lw", "a0", slot(local(*id))),
            ExprKind::Assign(target, value) => {
                let ExprKind::Var { local: id, .. } = &target.kind else {
                    unreachable!("sema only allows variables on the left of =")
                };
                self.expr(value);
                // a0 keeps the value: it is also the value of the whole `=`.
                self.access_local("sw", "a0", slot(local(*id)));
            }
            ExprKind::Unary(op, x) => {
                self.expr(x);
                match op {
                    UnOp::Neg => self.ins("neg", "a0, a0"),  // sub a0, zero, a0
                    UnOp::Not => self.ins("seqz", "a0, a0"), // a0 = (a0 == 0)
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
                self.binary(*op);
            }
            ExprKind::Call(name, args) => self.call(name, args),
        }
    }

    /// `a0 = a1 op a0` for the operators that always evaluate both sides.
    fn binary(&mut self, op: BinOp) {
        match op {
            BinOp::Add => self.ins("add", "a0, a1, a0"),
            BinOp::Sub => self.ins("sub", "a0, a1, a0"),
            BinOp::Mul => self.ins("mul", "a0, a1, a0"),
            BinOp::Div => self.ins("div", "a0, a1, a0"),
            BinOp::Rem => self.ins("rem", "a0, a1, a0"),
            BinOp::Lt => self.ins("slt", "a0, a1, a0"), // l < r
            BinOp::Gt => self.ins("slt", "a0, a0, a1"), // r < l
            BinOp::Le => {
                self.ins("slt", "a0, a0, a1"); // l <= r is !(r < l)
                self.ins("xori", "a0, a0, 1");
            }
            BinOp::Ge => {
                self.ins("slt", "a0, a1, a0"); // l >= r is !(l < r)
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

    fn call(&mut self, name: &str, args: &[Expr]) {
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

/// The slot sema assigned.
fn local(id: Option<LocalId>) -> LocalId {
    id.expect("sema resolves every variable")
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

    #[test]
    fn frame_layout() {
        assert_eq!(slot(LocalId(0)), -12);
        assert_eq!(slot(LocalId(1)), -16);
        assert_eq!(slot(LocalId(9)), -48);
        assert_eq!(frame_size(0), 16);
        assert_eq!(frame_size(2), 16);
        assert_eq!(frame_size(3), 32);
        assert_eq!(frame_size(6), 32);
        assert_eq!(frame_size(7), 48);
    }

    #[test]
    fn smallest_function() {
        let asm = compile("int main() { return 42; }");
        assert_eq!(
            asm,
            "    .text

    # int main(): frame 16 bytes
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
    fn prototypes_emit_nothing() {
        let asm = compile("int f(int a); int main() { return f(1); }");
        assert!(!asm.contains("f:"), "{asm}");
        assert!(asm.contains("call  f\n"), "{asm}");
        assert_eq!(asm.matches(".globl").count(), 1);
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
        let asm = compile("int f(int a, int b, int c) { return c; } int main() { return f(1, 2, 3); }");
        let body = body_of(&asm, "f");
        assert_eq!(
            body,
            [
                "addi sp, sp, -16",
                "sw ra, 12(sp)",
                "sw s0, 8(sp)",
                "addi s0, sp, 16",
                "addi sp, sp, -16", // 8 + 3 * 4 = 20 bytes, rounded up to 32
                "sw a0, -12(s0)",
                "sw a1, -16(s0)",
                "sw a2, -20(s0)",
                "lw a0, -20(s0)",
            ]
        );
        assert!(asm.contains("# int f(a, b, c): frame 32 bytes\n"), "{asm}");
        assert!(asm.contains("#     -20(s0)  param c\n"), "{asm}");
    }

    #[test]
    fn binary_operators() {
        let op = |src: &str| {
            let body = body_of(
                &compile(&format!("int main(int x, int y) {{ return {src}; }}")),
                "main",
            );
            // Skip the prologue (4 lines), the parameter stores (2), `lw` x, the
            // push (2) and `lw` y: what is left is the pop and the operator.
            body[10..].join("; ")
        };
        assert_eq!(op("x + y"), "lw a1, 0(sp); addi sp, sp, 4; add a0, a1, a0");
        assert_eq!(op("x - y"), "lw a1, 0(sp); addi sp, sp, 4; sub a0, a1, a0");
        assert_eq!(op("x * y"), "lw a1, 0(sp); addi sp, sp, 4; mul a0, a1, a0");
        assert_eq!(op("x / y"), "lw a1, 0(sp); addi sp, sp, 4; div a0, a1, a0");
        assert_eq!(op("x % y"), "lw a1, 0(sp); addi sp, sp, 4; rem a0, a1, a0");
        assert_eq!(op("x < y"), "lw a1, 0(sp); addi sp, sp, 4; slt a0, a1, a0");
        assert_eq!(op("x > y"), "lw a1, 0(sp); addi sp, sp, 4; slt a0, a0, a1");
        assert_eq!(
            op("x <= y"),
            "lw a1, 0(sp); addi sp, sp, 4; slt a0, a0, a1; xori a0, a0, 1"
        );
        assert_eq!(
            op("x >= y"),
            "lw a1, 0(sp); addi sp, sp, 4; slt a0, a1, a0; xori a0, a0, 1"
        );
        assert_eq!(
            op("x == y"),
            "lw a1, 0(sp); addi sp, sp, 4; xor a0, a1, a0; seqz a0, a0"
        );
        assert_eq!(
            op("x != y"),
            "lw a1, 0(sp); addi sp, sp, 4; xor a0, a1, a0; snez a0, a0"
        );
    }

    #[test]
    fn unary_operators_and_constants() {
        let body = body_of(&compile("int main() { return -(!2147483647); }"), "main");
        assert_eq!(&body[4..], ["li a0, 2147483647", "seqz a0, a0", "neg a0, a0"]);
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
    fn labels_are_unique_across_functions() {
        let asm = compile(
            "int f(int a) { while (a) a = a - 1; return a; }
             int main() { int i; for (i = 0; i < 3; i = i + 1) if (i) continue; else break; return f(i); }",
        );
        let mut defined: Vec<&str> = asm.lines().filter(|l| l.ends_with(':')).collect();
        let n = defined.len();
        defined.sort();
        defined.dedup();
        assert_eq!(defined.len(), n, "{asm}");
        assert!(asm.contains("j     .L.continue."), "{asm}");
        assert!(asm.contains("j     .L.break."), "{asm}");
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
    fn register_names_cannot_be_functions() {
        for name in ["sp", "a0", "x5", "fp", "zero"] {
            let errs = crate::compile(&format!(
                "int {name}() {{ return 0; }} int main() {{ return 0; }}"
            ))
            .unwrap_err();
            assert_eq!(
                errs[0].to_string(),
                format!("1:5: '{name}' is a register name, which cannot be used as a function name")
            );
        }
    }
}
