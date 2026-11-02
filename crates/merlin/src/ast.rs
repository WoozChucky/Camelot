//! The syntax tree, and `--dump-ast`'s printer.
//!
//! The parser builds it with every `var` field `None` and the types of
//! literals and casts filled in; [`crate::sema`] does the rest. After a
//! successful [`crate::parse`]:
//!
//! * every variable reference and declarator says where the variable lives
//!   ([`VarRef`]): a local of its function ([`Function::locals`]) or a global
//!   symbol;
//! * every expression has its C type in [`Expr::ty`], and conversions C does
//!   implicitly (assigning an `int` to a `char`, passing arguments, returning)
//!   are explicit [`ExprKind::Cast`] nodes with `implicit: true`;
//! * `sizeof` has been replaced by its value.
//!
//! So code generation never has to track scopes or C's conversion rules.

use crate::types::Type;
use crate::Pos;
use std::fmt::Write;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Program {
    /// Top-level declarations and function definitions, in source order.
    pub items: Vec<Item>,
}

impl Program {
    /// Every function declaration and definition, in source order.
    pub fn funcs(&self) -> impl Iterator<Item = &Function> {
        self.items.iter().filter_map(|i| match i {
            Item::Func(f) => Some(f),
            Item::Decl(_) => None,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Item {
    Func(Function),
    /// A declaration of global variables: `int a = 1, *p;`.
    Decl(Vec<Declarator>),
}

/// Index into [`Function::locals`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct LocalId(pub usize);

/// Where a variable lives.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum VarRef {
    /// In its function's stack frame.
    Local(LocalId),
    /// In memory at a symbol: a global variable (its own name) or a `static`
    /// local (`function.name`).
    Global(String),
}

/// `static` or `extern`, if the declaration says so.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Storage {
    #[default]
    None,
    Static,
    Extern,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Function {
    pub name: String,
    pub ret: Type,
    pub params: Vec<Param>,
    /// `static`: not visible to other files (no `.globl`).
    pub is_static: bool,
    /// `None` for a prototype (`int f(int a);`): declared here, defined elsewhere.
    pub body: Option<Vec<Stmt>>,
    /// Filled in by sema: parameters first (in order), then every local declared in
    /// the body, in source order. Shadowed names get their own entry; `static`
    /// locals are globals and are not listed.
    pub locals: Vec<Local>,
    pub pos: Pos,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Param {
    /// Empty in a prototype that leaves the parameter unnamed (`int f(char *)`).
    pub name: String,
    /// Arrays are already adjusted to pointers, as C does for parameters.
    pub ty: Type,
    pub pos: Pos,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Local {
    pub name: String,
    pub ty: Type,
    pub pos: Pos,
}

/// One name in a declaration: `x`, `*p = &x`, `a[3] = {1, 2, 3}`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Declarator {
    pub name: String,
    /// The declared type. Sema completes arrays of unknown size from their
    /// initializer (`int a[] = {1, 2}` becomes `int a[2]`).
    pub ty: Type,
    pub storage: Storage,
    pub init: Option<Init>,
    pub var: Option<VarRef>,
    /// Set by sema on global and `static` declarators: this one defines the
    /// variable, so code generation emits its storage. Of several declarations
    /// of one global, exactly one defines it (the one with an initializer, or
    /// else the first that is not `extern`).
    pub define: bool,
    pub pos: Pos,
}

/// An initializer: `= expr` or `= { ... }`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Init {
    Expr(Expr),
    /// Elements in order; elements left out are zero. After sema, a list only
    /// initializes arrays: a scalar's `{ x }` has become `x`.
    List(Vec<Init>, Pos),
}

impl Init {
    /// Where the initializer starts.
    pub fn pos(&self) -> Pos {
        match self {
            Init::Expr(e) => e.pos,
            Init::List(_, pos) => *pos,
        }
    }
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
    /// `int a = 1, *b;`
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
    /// `do body while (cond);`
    DoWhile {
        body: Box<Stmt>,
        cond: Expr,
    },
    /// `for (init; cond; step) body`. `init` is a `Decl` or `Expr` statement whose
    /// scope is the loop; a missing `cond` means "forever".
    For {
        init: Option<Box<Stmt>>,
        cond: Option<Expr>,
        step: Option<Expr>,
        body: Box<Stmt>,
    },
    /// `return expr;`, or `return;` in a `void` function.
    Return(Option<Expr>),
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
    Shl,
    Shr,
    BitAnd,
    BitOr,
    BitXor,
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
            BinOp::Shl => "<<",
            BinOp::Shr => ">>",
            BinOp::BitAnd => "&",
            BinOp::BitOr => "|",
            BinOp::BitXor => "^",
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

    pub fn is_comparison(self) -> bool {
        matches!(
            self,
            BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnOp {
    /// `-x`
    Neg,
    /// `+x`: only promotes.
    Plus,
    /// `!x`: 1 if x is 0, else 0.
    Not,
    /// `~x`
    BitNot,
}

impl UnOp {
    pub fn symbol(self) -> &'static str {
        match self {
            UnOp::Neg => "-",
            UnOp::Plus => "+",
            UnOp::Not => "!",
            UnOp::BitNot => "~",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Expr {
    pub kind: ExprKind,
    /// The expression's C type. The parser sets it for literals and casts,
    /// sema for everything else (before sema it is `int` elsewhere).
    pub ty: Type,
    pub pos: Pos,
}

impl Expr {
    pub fn new(kind: ExprKind, pos: Pos) -> Expr {
        Expr {
            kind,
            ty: Type::INT,
            pos,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExprKind {
    /// An integer or character constant, in range for its type (`ty`).
    Num(i64),
    /// A string literal's bytes, without the terminating NUL; its type is
    /// `char[len + 1]`.
    Str(Vec<u8>),
    Var {
        name: String,
        var: Option<VarRef>,
    },
    /// `target = value`, or with `op`, `target op= value` (the target is
    /// evaluated once). The value of the whole expression is what was stored.
    Assign {
        op: Option<BinOp>,
        target: Box<Expr>,
        value: Box<Expr>,
    },
    /// `++x`, `--x` (prefix: the new value) or `x++`, `x--` (the old value).
    IncDec {
        inc: bool,
        prefix: bool,
        target: Box<Expr>,
    },
    Binary(BinOp, Box<Expr>, Box<Expr>),
    Unary(UnOp, Box<Expr>),
    /// `&x`
    Addr(Box<Expr>),
    /// `*p` (and `a[i]`, which the parser writes as `*(a + i)`).
    Deref(Box<Expr>),
    /// `(type) x`, converting to `ty`. Sema adds implicit ones.
    Cast {
        x: Box<Expr>,
        implicit: bool,
    },
    /// `sizeof (type)`; sema replaces it with its value.
    SizeofType(Type),
    /// `sizeof expr` (not evaluated); sema replaces it with its value.
    SizeofExpr(Box<Expr>),
    /// `c ? a : b`
    Cond(Box<Expr>, Box<Expr>, Box<Expr>),
    /// `a, b`
    Comma(Box<Expr>, Box<Expr>),
    Call(String, Vec<Expr>),
}

/// Renders the tree as indented text, one node per line, with source positions.
/// Expressions show their type after a colon; resolved variables show where they
/// live (`#n` for local `n`, `@name` for a global symbol).
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
        for item in &prog.items {
            match item {
                Item::Func(f) => self.function(f),
                Item::Decl(decls) => {
                    for d in decls {
                        self.declarator(0, d);
                    }
                }
            }
        }
    }

    fn function(&mut self, f: &Function) {
        let params: Vec<String> = f.params.iter().map(|p| p.ty.declare(&p.name)).collect();
        let params = if params.is_empty() {
            "void".to_string()
        } else {
            params.join(", ")
        };
        let what = if f.body.is_some() { "Function" } else { "Prototype" };
        let stat = if f.is_static { "static " } else { "" };
        self.line(
            0,
            f.pos,
            &format!("{what} {stat}{}", f.ret.declare(&format!("{}({params})", f.name))),
        );
        if let Some(body) = &f.body {
            for (i, l) in f.locals.iter().enumerate() {
                let what = if i < f.params.len() { "Param" } else { "Local" };
                self.line(1, l.pos, &format!("{what} #{i} {}", l.ty.declare(&l.name)));
            }
            for s in body {
                self.stmt(1, s);
            }
        }
    }

    fn declarator(&mut self, d: usize, decl: &Declarator) {
        let storage = match decl.storage {
            Storage::None => "",
            Storage::Static => "static ",
            Storage::Extern => "extern ",
        };
        let define = if decl.define { " (definition)" } else { "" };
        self.line(
            d,
            decl.pos,
            &format!(
                "Var {storage}{}{}{define}",
                decl.ty.declare(&decl.name),
                where_(&decl.var)
            ),
        );
        if let Some(init) = &decl.init {
            self.init(d + 1, init);
        }
    }

    fn init(&mut self, d: usize, init: &Init) {
        match init {
            Init::Expr(e) => self.expr(d, e),
            Init::List(items, pos) => {
                self.line(d, *pos, "InitList");
                for i in items {
                    self.init(d + 1, i);
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
                    self.declarator(d + 1, decl);
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
            StmtKind::DoWhile { body, cond } => {
                self.line(d, s.pos, "DoWhile");
                self.stmt(d + 1, body);
                self.expr(d + 1, cond);
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
                if let Some(e) = e {
                    self.expr(d + 1, e);
                }
            }
            StmtKind::Break => self.line(d, s.pos, "Break"),
            StmtKind::Continue => self.line(d, s.pos, "Continue"),
        }
    }

    fn expr(&mut self, d: usize, e: &Expr) {
        let ty = &e.ty;
        let node = |this: &mut Self, text: String| this.line(d, e.pos, &format!("{text} : {ty}"));
        match &e.kind {
            ExprKind::Num(v) => node(self, format!("Num {v}")),
            ExprKind::Str(bytes) => node(self, format!("Str {}", quote(bytes))),
            ExprKind::Var { name, var } => node(self, format!("Var {name}{}", where_(var))),
            ExprKind::Assign { op, target, value } => {
                let op = op.map_or(String::new(), |op| format!(" {}=", op.symbol()));
                node(self, format!("Assign{op}"));
                self.expr(d + 1, target);
                self.expr(d + 1, value);
            }
            ExprKind::IncDec { inc, prefix, target } => {
                let op = if *inc { "++" } else { "--" };
                let what = if *prefix { "Pre" } else { "Post" };
                node(self, format!("{what}{op}"));
                self.expr(d + 1, target);
            }
            ExprKind::Binary(op, l, r) => {
                node(self, format!("Binary {}", op.symbol()));
                self.expr(d + 1, l);
                self.expr(d + 1, r);
            }
            ExprKind::Unary(op, x) => {
                node(self, format!("Unary {}", op.symbol()));
                self.expr(d + 1, x);
            }
            ExprKind::Addr(x) => {
                node(self, "Addr".to_string());
                self.expr(d + 1, x);
            }
            ExprKind::Deref(x) => {
                node(self, "Deref".to_string());
                self.expr(d + 1, x);
            }
            ExprKind::Cast { x, implicit } => {
                node(self, if *implicit { "ImplicitCast" } else { "Cast" }.to_string());
                self.expr(d + 1, x);
            }
            ExprKind::SizeofType(t) => node(self, format!("Sizeof ({t})")),
            ExprKind::SizeofExpr(x) => {
                node(self, "Sizeof".to_string());
                self.expr(d + 1, x);
            }
            ExprKind::Cond(c, a, b) => {
                node(self, "Cond".to_string());
                self.expr(d + 1, c);
                self.expr(d + 1, a);
                self.expr(d + 1, b);
            }
            ExprKind::Comma(a, b) => {
                node(self, "Comma".to_string());
                self.expr(d + 1, a);
                self.expr(d + 1, b);
            }
            ExprKind::Call(name, args) => {
                node(self, format!("Call {name}"));
                for a in args {
                    self.expr(d + 1, a);
                }
            }
        }
    }
}

fn where_(var: &Option<VarRef>) -> String {
    match var {
        None => String::new(),
        Some(VarRef::Local(LocalId(i))) => format!(" #{i}"),
        Some(VarRef::Global(sym)) => format!(" @{sym}"),
    }
}

/// A string literal as C would write it.
pub fn quote(bytes: &[u8]) -> String {
    let mut s = String::from("\"");
    for &b in bytes {
        match b {
            b'"' => s.push_str("\\\""),
            b'\\' => s.push_str("\\\\"),
            b'\n' => s.push_str("\\n"),
            b'\t' => s.push_str("\\t"),
            b'\r' => s.push_str("\\r"),
            0x20..=0x7e => s.push(b as char),
            _ => {
                let _ = write!(s, "\\{b:03o}");
            }
        }
    }
    s.push('"');
    s
}
