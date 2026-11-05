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
//! * `sizeof` has been replaced by its value;
//! * every member access knows its member's offset, every `switch` its
//!   `case` values, and every call that passes or returns a struct by value
//!   the hidden locals it needs ([`Local::temp`]).
//!
//! So code generation never has to track scopes or C's conversion rules.
//! Typedefs, struct tags and enum constants are gone even earlier: the parser
//! resolves them, so the tree only has types and numbers.

use crate::types::{Spelled, Type, Xlen};
use crate::Pos;
use std::fmt::Write;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Program {
    /// Top-level declarations and function definitions, in source order.
    pub items: Vec<Item>,
    /// The target the program is for: its types' sizes, and the code
    /// generated for it ([`crate::types`]).
    pub xlen: Xlen,
    /// The expressions of the file's scope the parser evaluated away that
    /// no declarator or function keeps ([`Declarator::sized`],
    /// [`Function::unevaluated`]), each after how many items it comes, for
    /// sema to check ([`Unevaluated`]); a block's are statements of their
    /// own.
    pub unevaluated: Vec<(usize, Unevaluated)>,
    /// The warnings so far (`Error`s of [`crate::Severity::Warning`]):
    /// what is not an error but clang warns of.
    pub warnings: Vec<crate::Error>,
}

/// An expression the parser needed no value of, for sema to check without
/// evaluating it, as clang's checks it: in an array size, an enumerator or
/// a `_Static_assert`, whose values the parser works out before sema runs,
/// the operand of a `sizeof` and the operand a constant `?:` does not
/// choose. Its type is all that counts there, which the parser works out,
/// but it must still be valid C (`f(1, 2)` of an `int f(void)` is not).
///
/// Sema checks it where it was, with the names declared there: a
/// declarator's own before the name it declares ([`Declarator::sized`]:
/// `char g[sizeof g.y]` sizes by an earlier `g`), one in an initializer
/// after it ([`Declarator::unevaluated`]), a function declarator's at file
/// scope ([`Function::unevaluated`]), a `for` statement's in its scope, a
/// parameter list's with the parameters before it ([`Self::params`]), and
/// the rest after the declaration or statement they are in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unevaluated {
    pub expr: Expr,
    /// The parser could not work out its type (or it is no integer type),
    /// and took the constant to be this error (where, what) unless sema
    /// finds one in the expression.
    pub untyped: Option<(Pos, String)>,
    /// The operand of a `?:` the parser folded, not one of `sizeof`.
    pub folded: bool,
    /// What is said of the array size a declarator folds, kept with the
    /// last of the declarator's own operands: clang's warning that it folds
    /// it, or the error that it cannot (clang's, or merlin's for a
    /// variable-length array). Given unless this one or one of the `with`
    /// before it in its list (the declarator's others) has an error, as
    /// clang says nothing of a declarator whose size is in error.
    pub diagnostic: Option<crate::Error>,
    pub with: usize,
    /// The parameters in scope where it was (C's prototype scope): those
    /// declared before it in the parameter lists it is in, outermost first,
    /// so `char a[sizeof n]` after `int n` sizes by that `n`.
    pub params: Vec<(String, Type, Pos)>,
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
    /// How the return type was spelled (for `-g`).
    pub ret_spelled: Spelled,
    pub params: Vec<Param>,
    /// `...` ends the parameter list.
    pub variadic: bool,
    /// `static`: not visible to other files (no `.globl`).
    pub is_static: bool,
    /// Declared `inline`.
    pub is_inline: bool,
    /// Declared `extern` (which only matters with `inline`).
    pub is_extern: bool,
    /// Set by sema on a definition that C99 calls an *inline definition*
    /// (every file-scope declaration says `inline` and none `extern`): it
    /// provides no external definition, so it is emitted without `.globl`.
    pub inline_only: bool,
    /// Declared with a prototype: a parameter list that is not `()`, which
    /// merlin takes as `(void)` but C as saying nothing of the parameters
    /// (`DW_AT_prototyped` under `-g`, as clang has it).
    pub prototyped: bool,
    /// `None` for a prototype (`int f(int a);`): declared here, defined elsewhere.
    pub body: Option<Vec<Stmt>>,
    /// Filled in by sema: parameters first (in order), then every local declared in
    /// the body, in source order, and the hidden temporaries calls need.
    /// Shadowed names get their own entry; `static` locals are globals and are
    /// not listed.
    pub locals: Vec<Local>,
    /// Set by sema when the function returns a struct too big for registers:
    /// the hidden local that keeps the address the caller passed in `a0`, of
    /// the memory the result goes to.
    pub ret_ptr: Option<LocalId>,
    pub pos: Pos,
    /// The closing brace of a definition's body (the declaration's position
    /// for a prototype): where `-g` says the epilogue comes from.
    pub end: Pos,
    /// What the parser evaluated away in its declarator (its return type,
    /// its parameters), checked at file scope before it is declared
    /// ([`Unevaluated`]).
    pub unevaluated: Vec<Unevaluated>,
}

impl Function {
    /// The function's type.
    pub fn ty(&self) -> Type {
        let params = self.params.iter().map(|p| p.ty.unqual().clone()).collect();
        if self.variadic {
            Type::variadic_func(self.ret.clone(), params)
        } else {
            Type::func(self.ret.clone(), params)
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Param {
    /// Empty in a prototype that leaves the parameter unnamed (`int f(char *)`).
    pub name: String,
    /// Arrays and functions are already adjusted to pointers, as C does for
    /// parameters.
    pub ty: Type,
    pub pos: Pos,
    /// How its type was spelled (for `-g`).
    pub spelled: Spelled,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Local {
    /// The variable's name; for a temporary, what it holds.
    pub name: String,
    pub ty: Type,
    pub pos: Pos,
    /// A temporary sema added, with no name in the source: the result of a
    /// call returning a struct, a copy of a struct argument passed by
    /// reference, or the address a big struct result goes to.
    pub temp: bool,
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
    /// How its type was spelled (for `-g`).
    pub spelled: Spelled,
    /// What the parser evaluated away in its declarator (and, for a
    /// declaration's first, in the specifiers before it), checked before
    /// its name is declared; and in its initializer, checked after, before
    /// the next declarator's ([`Unevaluated`]).
    pub sized: Vec<Unevaluated>,
    pub unevaluated: Vec<Unevaluated>,
}

/// An initializer: `= expr` or `= { ... }`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Init {
    Expr(Expr),
    /// Elements in order; elements left out are zero. After sema the list is
    /// fully braced, with exactly the shape of the object: one item per
    /// array element or struct member, in order, up to the last one given,
    /// each a list itself if its subobject is an array or a struct
    /// initialized element by element. A subobject that a designator skips
    /// has an empty list (zero, whatever its type). A union's list ends with
    /// the member it initializes, the items before it empty. A scalar's
    /// `{ x }` has become `x`, and C's brace elision (`int m[2][2] = {1, 2,
    /// 3, 4}`) and designators (`{ .b = 1, [4] = 2 }`) have been made
    /// explicit.
    List(Vec<Init>, Pos),
    /// In a list, as parsed: designators, then the initializer they place
    /// (`.next = 0`, `[2].x = 1`). Sema resolves them: none are left after it.
    Designated(Vec<Designator>, Box<Init>),
}

/// One step of a designation: a member (`.name`) or an array element
/// (`[index]`, a constant expression), each with the position of its `.` or
/// `[` (and a member's, of its name too).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Designator {
    Member(String, Pos, Pos),
    Index(Expr, Pos),
}

impl Designator {
    pub fn pos(&self) -> Pos {
        match self {
            Designator::Member(_, pos, _) | Designator::Index(_, pos) => *pos,
        }
    }
}

impl Init {
    /// Where the initializer starts.
    pub fn pos(&self) -> Pos {
        match self {
            Init::Expr(e) => e.pos,
            Init::List(_, pos) => *pos,
            Init::Designated(d, _) => d[0].pos(),
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
    /// No statement: what the declaration or statement before it in its
    /// block had that the parser evaluated away, for sema to check
    /// ([`Unevaluated`]). It generates nothing.
    Unevaluated(Vec<Unevaluated>),
    /// `int a = 1, *b;`
    Decl(Vec<Declarator>),
    /// `expr;`
    Expr(Expr),
    /// `;`. `glued`: on the line of the token before it (the `)` of the
    /// `if` or loop whose body it is), where clang's `-Wempty-body` warns
    /// of it.
    Empty {
        glued: bool,
    },
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
    /// scope is the loop; a missing `cond` means "forever". `unevaluated`:
    /// what the parser evaluated away in it, but in a declaration of `init`
    /// or in a block of `body`, checked in its scope after `init`
    /// ([`Unevaluated`]).
    For {
        init: Option<Box<Stmt>>,
        cond: Option<Expr>,
        step: Option<Expr>,
        body: Box<Stmt>,
        unevaluated: Vec<Unevaluated>,
    },
    /// `return expr;`, or `return;` in a `void` function.
    Return(Option<Expr>),
    Break,
    Continue,
    /// `switch (cond) body`. Sema fills in every `case` of this switch (its
    /// value, converted to the promoted type of `cond`, and its id) and the
    /// id of `default`, if there is one.
    Switch {
        cond: Expr,
        body: Box<Stmt>,
        cases: Vec<(i128, usize)>,
        default: Option<usize>,
    },
    /// `case expr: body`. Sema sets the value and an id, unique in the
    /// function, that its switch refers to. `bare`: the expression is one
    /// token (an enumeration constant alone, not in parentheses), whose
    /// enum `-g` describes, as clang's `EmitCaseStmt` does.
    Case {
        expr: Expr,
        value: i128,
        id: usize,
        body: Box<Stmt>,
        bare: bool,
    },
    /// `default: body`, with an id like a case's.
    Default {
        id: usize,
        body: Box<Stmt>,
    },
    /// `name: body`
    Label {
        name: String,
        body: Box<Stmt>,
    },
    /// `goto name;`, with where the name is.
    Goto(String, Pos),
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
    Num(i128),
    /// An enumeration constant `name`, an `int` of value `value`, as a
    /// number is. `of` is its enum, which `-g` describes when the code
    /// uses the constant, as clang does.
    Enumerator {
        name: String,
        value: i128,
        of: Spelled,
    },
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
    /// `(type) x`, converting to `ty`. Sema adds implicit ones. `spelled`:
    /// how an explicit one spelled its type (for `-g`).
    Cast {
        x: Box<Expr>,
        implicit: bool,
        spelled: Spelled,
    },
    /// `sizeof (type)`; sema replaces it with its value.
    SizeofType(Type),
    /// `sizeof expr` (not evaluated), with where the operand begins (its
    /// `(`, if it has one: clang reports an incomplete type there); sema
    /// replaces it with its value.
    SizeofExpr(Box<Expr>, Pos),
    /// `c ? a : b`
    Cond(Box<Expr>, Box<Expr>, Box<Expr>),
    /// `a, b`
    Comma(Box<Expr>, Box<Expr>),
    /// `s.name` or, with `arrow`, `p->name`. Sema sets the member's offset.
    Member {
        base: Box<Expr>,
        name: String,
        arrow: bool,
        offset: u32,
        /// Where the member's name is (the expression is at the `.` or
        /// `->`): clang reports a member it cannot find there.
        at: Pos,
    },
    /// `callee(args)`. The callee is a function designator (a function's
    /// name, or `*fp`) or a pointer to a function. Set by sema when the call
    /// returns a struct or union: `result`, the hidden local the result is
    /// kept in; and for every argument passed by reference (a struct bigger
    /// than two registers), the hidden local the caller copies it to.
    Call {
        callee: Box<Expr>,
        args: Vec<Expr>,
        result: Option<LocalId>,
        arg_copies: Vec<Option<LocalId>>,
        /// Where its `(` and its `)` are, where clang reports a callee that
        /// is no function and missing arguments.
        parens: (Pos, Pos),
    },
    /// `__builtin_va_start(ap, last)` (`va_start`, in a variadic function
    /// whose last named parameter is `last`): points the `va_list` `ap` at
    /// the first variadic argument. Its type is `void`.
    VaStart {
        ap: Box<Expr>,
        last: Box<Expr>,
    },
    /// `__builtin_va_arg(ap, T)` (`va_arg`): the variadic argument `ap`
    /// points at, of type `T` (the expression's type), and moves `ap` to
    /// the next one; with where `T` is written.
    VaArg(Box<Expr>, Pos),
    /// `__builtin_va_end(ap)`: nothing to do. Sema checks it and turns it
    /// into `(void)ap`.
    VaEnd(Box<Expr>),
    /// `__builtin_va_copy(dst, src)`: sema checks it and turns it into
    /// `(void)(dst = src)`, since a `va_list` is a pointer.
    VaCopy(Box<Expr>, Box<Expr>),
    /// `(type){ ... }`: an unnamed object of the expression's type,
    /// initialized by the list each time the expression is evaluated; an
    /// lvalue. Sema gives it a hidden local (`local`).
    CompoundLit {
        init: Box<Init>,
        local: Option<LocalId>,
    },
}

impl ExprKind {
    /// A call, before sema: no temporaries yet.
    pub fn call(callee: Expr, args: Vec<Expr>, parens: (Pos, Pos)) -> ExprKind {
        ExprKind::Call {
            callee: Box::new(callee),
            args,
            result: None,
            arg_copies: Vec::new(),
            parens,
        }
    }
}

impl Expr {
    /// In a checked tree, the symbol of the function a call goes straight to
    /// (`f(x)`), as opposed to through a pointer (`fp(x)`, `(*fp)(x)`).
    pub fn direct_callee(&self) -> Option<&str> {
        match &self.kind {
            ExprKind::Var {
                var: Some(VarRef::Global(sym)),
                ..
            } if self.ty.is_func() => Some(sym),
            _ => None,
        }
    }
}

/// Renders the tree as indented text, one node per line, with source positions.
/// Expressions show their type after a colon; resolved variables show where they
/// live (`#n` for local `n`, `@name` for a global symbol).
pub fn dump(prog: &Program) -> String {
    let mut d = Dumper {
        out: String::new(),
        xlen: prog.xlen,
    };
    d.program(prog);
    d.out
}

struct Dumper {
    out: String,
    xlen: Xlen,
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
        let mut params: Vec<String> = f.params.iter().map(|p| p.ty.declare(&p.name)).collect();
        if f.variadic {
            params.push("...".to_string());
        }
        let params = if params.is_empty() {
            "void".to_string()
        } else {
            params.join(", ")
        };
        let what = if f.body.is_some() { "Function" } else { "Prototype" };
        let stat = if f.is_static { "static " } else { "" };
        let inline = if f.is_inline { "inline " } else { "" };
        self.line(
            0,
            f.pos,
            &format!(
                "{what} {stat}{inline}{}",
                f.ret.declare(&format!("{}({params})", f.name))
            ),
        );
        if let Some(body) = &f.body {
            for (i, l) in f.locals.iter().enumerate() {
                let text = if l.temp {
                    format!("Temp #{i} {} ({})", l.ty, l.name)
                } else {
                    let what = if i < f.params.len() { "Param" } else { "Local" };
                    format!("{what} #{i} {}", l.ty.declare(&l.name))
                };
                self.line(1, l.pos, &text);
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
            Init::Designated(designators, init) => {
                let mut text = String::new();
                for ds in designators {
                    match ds {
                        Designator::Member(name, ..) => text += &format!(".{name}"),
                        Designator::Index(e, _) => {
                            text += &format!("[{}]", crate::consteval::int(e, self.xlen).unwrap_or(0))
                        }
                    }
                }
                self.line(d, init.pos(), &format!("Designated {text}"));
                self.init(d + 1, init);
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
            StmtKind::Empty { .. } => self.line(d, s.pos, "Empty"),
            StmtKind::Unevaluated(us) => {
                self.line(d, s.pos, "Unevaluated");
                for u in us {
                    self.expr(d + 1, &u.expr);
                }
            }
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
                ..
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
            StmtKind::Switch { cond, body, .. } => {
                self.line(d, s.pos, "Switch");
                self.expr(d + 1, cond);
                self.stmt(d + 1, body);
            }
            StmtKind::Case { value, body, .. } => {
                self.line(d, s.pos, &format!("Case {value}"));
                self.stmt(d + 1, body);
            }
            StmtKind::Default { body, .. } => {
                self.line(d, s.pos, "Default");
                self.stmt(d + 1, body);
            }
            StmtKind::Label { name, body } => {
                self.line(d, s.pos, &format!("Label {name}"));
                self.stmt(d + 1, body);
            }
            StmtKind::Goto(name, _) => self.line(d, s.pos, &format!("Goto {name}")),
        }
    }

    fn expr(&mut self, d: usize, e: &Expr) {
        let ty = &e.ty;
        let node = |this: &mut Self, text: String| this.line(d, e.pos, &format!("{text} : {ty}"));
        match &e.kind {
            ExprKind::Num(v) => node(self, format!("Num {v}")),
            ExprKind::Enumerator { name, value, .. } => node(self, format!("Enumerator {name} = {value}")),
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
            ExprKind::Cast { x, implicit, .. } => {
                node(self, if *implicit { "ImplicitCast" } else { "Cast" }.to_string());
                self.expr(d + 1, x);
            }
            ExprKind::SizeofType(t) => node(self, format!("Sizeof ({t})")),
            ExprKind::SizeofExpr(x, _) => {
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
            ExprKind::Member {
                base,
                name,
                arrow,
                offset,
                ..
            } => {
                let op = if *arrow { "->" } else { "." };
                node(self, format!("Member {op}{name} (offset {offset})"));
                self.expr(d + 1, base);
            }
            ExprKind::Call {
                callee,
                args,
                result,
                arg_copies,
                ..
            } => {
                let mut text = match callee.direct_callee() {
                    Some(name) => format!("Call {name}"),
                    None => "Call".to_string(),
                };
                if let Some(LocalId(r)) = result {
                    let _ = write!(text, " (result in #{r})");
                }
                node(self, text);
                if callee.direct_callee().is_none() {
                    self.expr(d + 1, callee);
                }
                for (i, a) in args.iter().enumerate() {
                    match arg_copies.get(i) {
                        Some(Some(LocalId(c))) => {
                            self.line(d + 1, a.pos, &format!("ByReference (copied to #{c})"));
                            self.expr(d + 2, a);
                        }
                        _ => self.expr(d + 1, a),
                    }
                }
            }
            ExprKind::VaStart { ap, last } => {
                node(self, "VaStart".to_string());
                self.expr(d + 1, ap);
                self.expr(d + 1, last);
            }
            ExprKind::VaArg(ap, _) => {
                node(self, "VaArg".to_string());
                self.expr(d + 1, ap);
            }
            ExprKind::VaEnd(ap) => {
                node(self, "VaEnd".to_string());
                self.expr(d + 1, ap);
            }
            ExprKind::VaCopy(dst, src) => {
                node(self, "VaCopy".to_string());
                self.expr(d + 1, dst);
                self.expr(d + 1, src);
            }
            ExprKind::CompoundLit { init, local } => {
                let at = match local {
                    Some(LocalId(id)) => format!(" (#{id})"),
                    None => String::new(),
                };
                node(self, format!("CompoundLiteral{at}"));
                self.init(d + 1, init);
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
