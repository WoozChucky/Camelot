//! The syntax tree, and `--dump-ast`'s printer.
//!
//! The parser builds it with every `local` field `None`; [`crate::sema`] fills
//! them in. After a successful [`crate::parse`], every variable reference and
//! declaration points into its function's [`Function::locals`], so code
//! generation can give each local a fixed stack slot without tracking scopes.

use crate::Pos;
use std::fmt::Write;

/// Every value in v0 is an `int` (32-bit, two's complement).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Program {
    pub funcs: Vec<Function>,
}

/// Index into [`Function::locals`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct LocalId(pub usize);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Function {
    pub name: String,
    pub params: Vec<Param>,
    /// `None` for a prototype (`int f(int a);`): declared here, defined elsewhere.
    pub body: Option<Vec<Stmt>>,
    /// Filled in by sema: parameters first (in order), then every local declared in
    /// the body, in source order. Shadowed names get their own entry.
    pub locals: Vec<Local>,
    pub pos: Pos,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Param {
    pub name: String,
    pub pos: Pos,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Local {
    pub name: String,
    pub pos: Pos,
}

/// One name in a declaration: `x` or `x = init`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Declarator {
    pub name: String,
    pub init: Option<Expr>,
    pub local: Option<LocalId>,
    pub pos: Pos,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stmt {
    pub kind: StmtKind,
    pub pos: Pos,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StmtKind {
    /// `{ ... }`, a new scope.
    Block(Vec<Stmt>),
    /// `int a = 1, b;`
    Decl(Vec<Declarator>),
    /// `expr;`
    Expr(Expr),
    /// `;`
    Empty,
    If {
        cond: Expr,
        then: Box<Stmt>,
        els: Option<Box<Stmt>>,
    },
    While {
        cond: Expr,
        body: Box<Stmt>,
    },
    /// `for (init; cond; step) body`. `init` is a `Decl` or `Expr` statement whose
    /// scope is the loop; a missing `cond` means "forever".
    For {
        init: Option<Box<Stmt>>,
        cond: Option<Expr>,
        step: Option<Expr>,
        body: Box<Stmt>,
    },
    Return(Expr),
    Break,
    Continue,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    /// `&&`: short-circuit, result 0 or 1.
    LogAnd,
    /// `||`: short-circuit, result 0 or 1.
    LogOr,
}

impl BinOp {
    pub fn symbol(self) -> &'static str {
        match self {
            BinOp::Add => "+",
            BinOp::Sub => "-",
            BinOp::Mul => "*",
            BinOp::Div => "/",
            BinOp::Rem => "%",
            BinOp::Eq => "==",
            BinOp::Ne => "!=",
            BinOp::Lt => "<",
            BinOp::Le => "<=",
            BinOp::Gt => ">",
            BinOp::Ge => ">=",
            BinOp::LogAnd => "&&",
            BinOp::LogOr => "||",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnOp {
    /// `-x`
    Neg,
    /// `!x`: 1 if x is 0, else 0.
    Not,
}

impl UnOp {
    pub fn symbol(self) -> &'static str {
        match self {
            UnOp::Neg => "-",
            UnOp::Not => "!",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Expr {
    pub kind: ExprKind,
    pub pos: Pos,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExprKind {
    /// Integer literal; the `int` value is the low 32 bits.
    Num(i64),
    Var {
        name: String,
        local: Option<LocalId>,
    },
    /// `target = value`; the value of the whole expression is `value`. Sema checks
    /// that `target` is a variable.
    Assign(Box<Expr>, Box<Expr>),
    Binary(BinOp, Box<Expr>, Box<Expr>),
    Unary(UnOp, Box<Expr>),
    Call(String, Vec<Expr>),
}

/// Renders the tree as indented text, one node per line, with source positions.
/// Resolved variables show their local slot as `#n`.
pub fn dump(prog: &Program) -> String {
    let mut d = Dumper { out: String::new() };
    d.program(prog);
    d.out
}

struct Dumper {
    out: String,
}

impl Dumper {
    fn line(&mut self, depth: usize, pos: Pos, text: &str) {
        let _ = writeln!(self.out, "{:indent$}{text} <{pos}>", "", indent = depth * 2);
    }

    fn program(&mut self, prog: &Program) {
        for f in &prog.funcs {
            let params: Vec<&str> = f.params.iter().map(|p| p.name.as_str()).collect();
            let what = if f.body.is_some() { "Function" } else { "Prototype" };
            self.line(0, f.pos, &format!("{what} {}({})", f.name, params.join(", ")));
            if let Some(body) = &f.body {
                for (i, l) in f.locals.iter().enumerate() {
                    let what = if i < f.params.len() { "Param" } else { "Local" };
                    self.line(1, l.pos, &format!("{what} #{i} {}", l.name));
                }
                for s in body {
                    self.stmt(1, s);
                }
            }
        }
    }

    fn stmt(&mut self, d: usize, s: &Stmt) {
        match &s.kind {
            StmtKind::Block(body) => {
                self.line(d, s.pos, "Block");
                for s in body {
                    self.stmt(d + 1, s);
                }
            }
            StmtKind::Decl(decls) => {
                self.line(d, s.pos, "Decl");
                for decl in decls {
                    self.line(d + 1, decl.pos, &format!("Var {}{}", decl.name, slot(decl.local)));
                    if let Some(e) = &decl.init {
                        self.expr(d + 2, e);
                    }
                }
            }
            StmtKind::Expr(e) => {
                self.line(d, s.pos, "ExprStmt");
                self.expr(d + 1, e);
            }
            StmtKind::Empty => self.line(d, s.pos, "Empty"),
            StmtKind::If { cond, then, els } => {
                self.line(d, s.pos, "If");
                self.expr(d + 1, cond);
                self.stmt(d + 1, then);
                if let Some(e) = els {
                    self.line(d, e.pos, "Else");
                    self.stmt(d + 1, e);
                }
            }
            StmtKind::While { cond, body } => {
                self.line(d, s.pos, "While");
                self.expr(d + 1, cond);
                self.stmt(d + 1, body);
            }
            StmtKind::For {
                init,
                cond,
                step,
                body,
            } => {
                self.line(d, s.pos, "For");
                match init {
                    Some(i) => self.stmt(d + 1, i),
                    None => self.line(d + 1, s.pos, "NoInit"),
                }
                match cond {
                    Some(c) => self.expr(d + 1, c),
                    None => self.line(d + 1, s.pos, "NoCond"),
                }
                match step {
                    Some(e) => self.expr(d + 1, e),
                    None => self.line(d + 1, s.pos, "NoStep"),
                }
                self.stmt(d + 1, body);
            }
            StmtKind::Return(e) => {
                self.line(d, s.pos, "Return");
                self.expr(d + 1, e);
            }
            StmtKind::Break => self.line(d, s.pos, "Break"),
            StmtKind::Continue => self.line(d, s.pos, "Continue"),
        }
    }

    fn expr(&mut self, d: usize, e: &Expr) {
        match &e.kind {
            ExprKind::Num(v) => self.line(d, e.pos, &format!("Num {v}")),
            ExprKind::Var { name, local } => self.line(d, e.pos, &format!("Var {name}{}", slot(*local))),
            ExprKind::Assign(target, value) => {
                self.line(d, e.pos, "Assign");
                self.expr(d + 1, target);
                self.expr(d + 1, value);
            }
            ExprKind::Binary(op, l, r) => {
                self.line(d, e.pos, &format!("Binary {}", op.symbol()));
                self.expr(d + 1, l);
                self.expr(d + 1, r);
            }
            ExprKind::Unary(op, x) => {
                self.line(d, e.pos, &format!("Unary {}", op.symbol()));
                self.expr(d + 1, x);
            }
            ExprKind::Call(name, args) => {
                self.line(d, e.pos, &format!("Call {name}"));
                for a in args {
                    self.expr(d + 1, a);
                }
            }
        }
    }
}

fn slot(local: Option<LocalId>) -> String {
    local.map_or(String::new(), |LocalId(i)| format!(" #{i}"))
}
