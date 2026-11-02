//! A reference interpreter for the checked AST, used only by the tests.
//!
//! It gives the harness's `// expect:` lines a second opinion while there is no
//! code generator, and later a quick way to tell a codegen bug from a parser bug.
//! Semantics match what the generated code must do on Excalibur (spec section
//! 10, "Merlin C subset"): 32-bit wrapping arithmetic, `div`/`rem` rounding
//! toward zero with RISC-V's results for `i32::MIN / -1` and division by zero,
//! comparisons and `! && ||` give 0 or 1, and falling off the end of a function
//! returns 0.

use merlin::ast::*;
use std::collections::HashMap;

/// Runs `entry()` and returns its result. Uses a thread with a big stack, since
/// every interpreted call nests several Rust calls (debug builds use big frames).
pub fn run(prog: &Program, entry: &str) -> Result<i32, String> {
    std::thread::scope(|s| {
        std::thread::Builder::new()
            .stack_size(256 << 20)
            .spawn_scoped(s, || run_here(prog, entry))
            .unwrap()
            .join()
            .unwrap()
    })
}

fn run_here(prog: &Program, entry: &str) -> Result<i32, String> {
    let mut it = Interp {
        funcs: prog
            .funcs
            .iter()
            .filter(|f| f.body.is_some())
            .map(|f| (f.name.as_str(), f))
            .collect(),
        steps: 0,
        depth: 0,
    };
    it.call(entry, Vec::new())
}

/// Enough for every harness program; catches accidental infinite loops.
const MAX_STEPS: u64 = 50_000_000;
/// Keeps runaway recursion from overflowing the interpreter's stack.
const MAX_DEPTH: usize = 1_000;

struct Interp<'a> {
    funcs: HashMap<&'a str, &'a Function>,
    steps: u64,
    depth: usize,
}

enum Flow {
    Next,
    Break,
    Continue,
    Return(i32),
}

impl Interp<'_> {
    fn call(&mut self, name: &str, args: Vec<i32>) -> Result<i32, String> {
        let f = *self
            .funcs
            .get(name)
            .ok_or_else(|| format!("call to '{name}', which has no definition"))?;
        if self.depth == MAX_DEPTH {
            return Err(format!("recursion deeper than {MAX_DEPTH} calls"));
        }
        let mut frame = vec![0i32; f.locals.len()];
        frame[..args.len()].copy_from_slice(&args);
        self.depth += 1;
        let mut result = 0;
        for s in f.body.as_ref().unwrap() {
            if let Flow::Return(v) = self.stmt(s, &mut frame)? {
                result = v;
                break;
            }
        }
        self.depth -= 1;
        Ok(result)
    }

    fn tick(&mut self) -> Result<(), String> {
        self.steps += 1;
        if self.steps > MAX_STEPS {
            return Err(format!("more than {MAX_STEPS} steps"));
        }
        Ok(())
    }

    fn block(&mut self, body: &[Stmt], frame: &mut [i32]) -> Result<Flow, String> {
        for s in body {
            match self.stmt(s, frame)? {
                Flow::Next => {}
                other => return Ok(other),
            }
        }
        Ok(Flow::Next)
    }

    fn stmt(&mut self, s: &Stmt, frame: &mut [i32]) -> Result<Flow, String> {
        self.tick()?;
        Ok(match &s.kind {
            StmtKind::Block(body) => self.block(body, frame)?,
            StmtKind::Decl(decls) => {
                for d in decls {
                    if let Some(e) = &d.init {
                        let v = self.expr(e, frame)?;
                        frame[slot(d.local)] = v;
                    }
                }
                Flow::Next
            }
            StmtKind::Expr(e) => {
                self.expr(e, frame)?;
                Flow::Next
            }
            StmtKind::Empty => Flow::Next,
            StmtKind::If { cond, then, els } => {
                if self.expr(cond, frame)? != 0 {
                    self.stmt(then, frame)?
                } else if let Some(e) = els {
                    self.stmt(e, frame)?
                } else {
                    Flow::Next
                }
            }
            StmtKind::While { cond, body } => {
                while self.expr(cond, frame)? != 0 {
                    match self.stmt(body, frame)? {
                        Flow::Break => break,
                        Flow::Return(v) => return Ok(Flow::Return(v)),
                        Flow::Next | Flow::Continue => {}
                    }
                }
                Flow::Next
            }
            StmtKind::For {
                init,
                cond,
                step,
                body,
            } => {
                if let Some(i) = init {
                    self.stmt(i, frame)?;
                }
                loop {
                    if let Some(c) = cond {
                        if self.expr(c, frame)? == 0 {
                            break;
                        }
                    }
                    match self.stmt(body, frame)? {
                        Flow::Break => break,
                        Flow::Return(v) => return Ok(Flow::Return(v)),
                        Flow::Next | Flow::Continue => {}
                    }
                    if let Some(e) = step {
                        self.expr(e, frame)?;
                    }
                }
                Flow::Next
            }
            StmtKind::Return(e) => Flow::Return(self.expr(e, frame)?),
            StmtKind::Break => Flow::Break,
            StmtKind::Continue => Flow::Continue,
        })
    }

    fn expr(&mut self, e: &Expr, frame: &mut [i32]) -> Result<i32, String> {
        self.tick()?;
        Ok(match &e.kind {
            ExprKind::Num(v) => *v as i32,
            ExprKind::Var { local, .. } => frame[slot(*local)],
            ExprKind::Assign(target, value) => {
                let v = self.expr(value, frame)?;
                let ExprKind::Var { local, .. } = target.kind else {
                    return Err(format!("{}: assignment to a non-variable", e.pos));
                };
                frame[slot(local)] = v;
                v
            }
            ExprKind::Unary(UnOp::Neg, x) => self.expr(x, frame)?.wrapping_neg(),
            ExprKind::Unary(UnOp::Not, x) => (self.expr(x, frame)? == 0) as i32,
            ExprKind::Binary(BinOp::LogAnd, l, r) => {
                (self.expr(l, frame)? != 0 && self.expr(r, frame)? != 0) as i32
            }
            ExprKind::Binary(BinOp::LogOr, l, r) => {
                (self.expr(l, frame)? != 0 || self.expr(r, frame)? != 0) as i32
            }
            ExprKind::Binary(op, l, r) => {
                let a = self.expr(l, frame)?;
                let b = self.expr(r, frame)?;
                match op {
                    BinOp::Add => a.wrapping_add(b),
                    BinOp::Sub => a.wrapping_sub(b),
                    BinOp::Mul => a.wrapping_mul(b),
                    // RISC-V `div`/`rem` by zero: quotient all ones, remainder the dividend.
                    BinOp::Div if b == 0 => -1,
                    BinOp::Rem if b == 0 => a,
                    BinOp::Div => a.wrapping_div(b),
                    BinOp::Rem => a.wrapping_rem(b),
                    BinOp::Eq => (a == b) as i32,
                    BinOp::Ne => (a != b) as i32,
                    BinOp::Lt => (a < b) as i32,
                    BinOp::Le => (a <= b) as i32,
                    BinOp::Gt => (a > b) as i32,
                    BinOp::Ge => (a >= b) as i32,
                    BinOp::LogAnd | BinOp::LogOr => unreachable!(),
                }
            }
            ExprKind::Call(name, args) => {
                // Left to right; C leaves the order unspecified, so harness programs
                // must not depend on it.
                let mut vals = Vec::with_capacity(args.len());
                for a in args {
                    vals.push(self.expr(a, frame)?);
                }
                self.call(name, vals)?
            }
        })
    }
}

fn slot(local: Option<LocalId>) -> usize {
    local.expect("sema resolves every variable").0
}
