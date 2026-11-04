//! Name resolution and type checking.
//!
//! * Every name must be declared before use (C99: no implicit declarations).
//!   Block scoping follows C: inner blocks may shadow outer names and globals.
//!   [`Function::locals`] gets every parameter and local, and every variable
//!   reference and declarator its [`VarRef`]. A function's name is a global
//!   of function type, which decays to a pointer to the function.
//! * Every expression gets its C type, by C99's rules with ILP32 sizes (see
//!   [`crate::types`]). Operands must make sense for their operator (no
//!   arithmetic on `void *`, no pointer plus pointer, `*` only on pointers,
//!   `.` only on structs), assignments, arguments and returns must convert as
//!   C allows (and keep the qualifiers of what pointers point to), and the
//!   conversions C applies there become explicit implicit-cast nodes.
//! * `const` objects are not assignable; `volatile` needs nothing here (code
//!   generation never removes or merges an access).
//! * `break` must be inside a loop or `switch`, `continue` inside a loop,
//!   `case` and `default` inside a `switch` (each `case` value an integer
//!   constant, none twice), every `goto` must have its label in the function;
//!   `return` must match the function's type, global and `static`
//!   initializers must be constant, and of several declarations of one
//!   global, one is marked as its definition.
//! * Initializers are rewritten fully braced (brace elision made explicit),
//!   and calls that pass or return structs by value get the hidden locals the
//!   calling convention needs ([`crate::abi`]).
//! * `sizeof` is replaced by its value.
//! * A compound literal at file scope (in a global's initializer) is an
//!   object with static storage, as C99 says: it becomes a hidden `static`
//!   variable, `.compoundliteral.<n>`, which the expression then names. Those
//!   variables are added to the program as a last declaration.
//!
//! Reports all errors, in source order. An expression with an error doesn't
//! produce more errors about the expressions around it.

use crate::abi;
use crate::ast::*;
use crate::consteval;
use crate::types::{common, FuncType, IntKind, Type, Xlen};
use crate::{Error, Pos};
use std::collections::{HashMap, HashSet, VecDeque};

pub fn check(prog: &mut Program) -> Result<(), Vec<Error>> {
    let mut c = Checker {
        globals: HashMap::new(),
        errors: Vec::new(),
        literals: Vec::new(),
        xlen: prog.xlen,
    };
    for (i, item) in prog.items.iter_mut().enumerate() {
        match item {
            Item::Func(f) => c.function(f),
            Item::Decl(decls) => {
                for (j, d) in decls.iter_mut().enumerate() {
                    c.global_var(d, (i, j));
                }
            }
        }
    }
    c.mark_definitions(prog);
    // File-scope compound literals: the objects their expressions name.
    let literals = std::mem::take(&mut c.literals);
    if !literals.is_empty() {
        prog.items.push(Item::Decl(literals));
    }
    // Whether a definition is an inline definition depends on every
    // declaration of the function, so it is decided at the end.
    for item in &mut prog.items {
        if let Item::Func(f) = item {
            if let Some(Global::Func(sig)) = c.globals.get(&f.name) {
                f.inline_only = f.body.is_some() && !f.is_static && sig.inline_only;
            }
        }
    }
    let mut errors = c.errors;
    if errors.is_empty() {
        Ok(())
    } else {
        errors.sort_by_key(|e| e.pos);
        Err(errors)
    }
}

/// The type of `l op r` for the arithmetic, bitwise and shift operators, with
/// both operands already decayed; `None` if C doesn't allow the operands.
/// Comparisons and `&&`/`||` always give `int`. Code generation uses this too,
/// to know the type a compound assignment computes in.
pub fn arith_type(op: BinOp, l: &Type, r: &Type, x: Xlen) -> Option<Type> {
    // Pointer arithmetic needs to know the size of what is pointed to.
    let sized = |t: &Type| t.pointee().and_then(|t| t.size(x)).is_some();
    match (op, l.unqual(), r.unqual()) {
        (_, Type::Int(a), Type::Int(b)) => Some(Type::Int(match op {
            BinOp::Shl | BinOp::Shr => a.promoted(),
            _ => common(*a, *b, x),
        })),
        (BinOp::Add | BinOp::Sub, Type::Ptr(_), Type::Int(_)) if sized(l) => Some(l.clone()),
        (BinOp::Add, Type::Int(_), Type::Ptr(_)) if sized(r) => Some(r.clone()),
        // The difference of two pointers counts elements: `ptrdiff_t`, `int`
        // on ILP32 and `long` on LP64. What they point to may differ in
        // qualifiers only.
        (BinOp::Sub, Type::Ptr(a), Type::Ptr(b)) if a.unqual() == b.unqual() && sized(l) => {
            Some(Type::Int(IntKind::ptrdiff_t(x)))
        }
        _ => None,
    }
}

/// Whether `e` designates an object (C's lvalue): a variable, `*p`, a
/// string literal, `p->m`, or `s.m` of an lvalue `s` (a function's name, which
/// designates the function, counts too, so that `&f` works).
fn is_lvalue(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::Var { .. } | ExprKind::Deref(_) | ExprKind::Str(_) | ExprKind::CompoundLit { .. } => true,
        ExprKind::Member { base, arrow, .. } => *arrow || is_lvalue(base),
        _ => false,
    }
}

/// Wraps `e` in an implicit conversion to `to`, unless it already has that
/// type (qualifiers aside: a value has none).
fn coerce(e: &mut Expr, to: &Type) {
    let to = to.unqual();
    if e.ty.unqual() == to {
        return;
    }
    let pos = e.pos;
    let inner = std::mem::replace(e, Expr::new(ExprKind::Num(0), pos));
    *e = Expr {
        kind: ExprKind::Cast {
            x: Box::new(inner),
            implicit: true,
        },
        ty: to.clone(),
        pos,
    };
}

fn plural(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

/// Whether two declarations of a variable agree: the same type, except that an
/// array of unknown size matches one of known size.
fn compatible(a: &Type, b: &Type) -> bool {
    match (a, b) {
        (Type::Array(x, n), Type::Array(y, m)) => x == y && (n == m || n.is_none() || m.is_none()),
        _ => a == b,
    }
}

/// Whether a struct or union has a `const` member somewhere inside, which
/// makes the whole unassignable.
fn has_const_member(t: &Type) -> bool {
    let Some(body) = t.record().and_then(|r| r.body()) else {
        return false;
    };
    body.members.iter().any(|m| {
        let mut elem = &m.ty;
        while let Type::Array(e, _) = elem {
            elem = e;
        }
        elem.quals().konst || has_const_member(elem)
    })
}

static VOID: Type = Type::Void;

/// An error has been reported; the caller should not add more about the same
/// expression.
struct Reported;

type Checked = Result<(), Reported>;

struct FuncSig {
    ty: FuncType,
    is_static: bool,
    defined: bool,
    /// Every declaration so far says `inline` (without `extern`).
    inline_only: bool,
    pos: Pos,
}

struct GlobalVar {
    ty: Type,
    is_static: bool,
    pos: Pos,
    /// The declarator with an initializer, as (item, declarator) indices.
    init: Option<(usize, usize)>,
    /// The first declarator that is not `extern` (a tentative definition).
    tentative: Option<(usize, usize)>,
}

enum Global {
    Func(FuncSig),
    Var(GlobalVar),
}

struct Checker {
    globals: HashMap<String, Global>,
    errors: Vec<Error>,
    /// The objects of file-scope compound literals so far, defined.
    literals: Vec<Declarator>,
    /// The target, for sizes and conversions.
    xlen: Xlen,
}

impl Checker {
    fn err(&mut self, pos: Pos, msg: impl Into<String>) {
        self.errors.push(Error::new(pos, msg));
    }

    fn function(&mut self, f: &mut Function) {
        for p in &f.params {
            if p.ty.is_void() {
                self.err(p.pos, "parameter has incomplete type 'void'");
            }
        }
        let Type::Func(ty) = f.ty() else {
            unreachable!("a function's type")
        };
        let sig = FuncSig {
            ty: *ty,
            is_static: f.is_static,
            defined: f.body.is_some(),
            inline_only: f.is_inline && !f.is_extern,
            pos: f.pos,
        };
        // Functions are visible from their declaration onwards, so a function can
        // call itself and anything declared above it (use a prototype for mutual
        // recursion).
        match self.globals.get_mut(&f.name) {
            Some(Global::Var(_)) => {
                self.err(
                    f.pos,
                    format!("redefinition of '{}' as a different kind of symbol", f.name),
                );
            }
            Some(Global::Func(prev)) => {
                let (prev_pos, prev_arity) = (prev.pos, prev.ty.params.len());
                if prev_arity != sig.ty.params.len() {
                    let msg = format!(
                        "conflicting types for '{}': declared at {prev_pos} with {prev_arity} parameter{}, \
                         here with {}",
                        f.name,
                        plural(prev_arity),
                        sig.ty.params.len()
                    );
                    self.err(f.pos, msg);
                } else if prev.ty != sig.ty {
                    let msg = format!(
                        "conflicting types for '{}': declared at {prev_pos} as '{}', here as '{}'",
                        f.name,
                        Type::Func(Box::new(prev.ty.clone())),
                        Type::Func(Box::new(sig.ty.clone()))
                    );
                    self.err(f.pos, msg);
                } else if prev.defined && sig.defined {
                    self.err(f.pos, format!("redefinition of '{}'", f.name));
                } else if sig.is_static && !prev.is_static {
                    self.err(
                        f.pos,
                        format!(
                            "static declaration of '{}' follows non-static declaration",
                            f.name
                        ),
                    );
                } else {
                    prev.defined |= sig.defined;
                    prev.inline_only &= sig.inline_only;
                }
            }
            None => {
                self.globals.insert(f.name.clone(), Global::Func(sig));
            }
        }
        // A static function keeps that from its first declaration on.
        if let Some(Global::Func(s)) = self.globals.get(&f.name) {
            f.is_static = s.is_static;
        }
        if f.body.is_none() {
            return;
        }
        // A definition needs complete parameter and result types.
        for p in &f.params {
            if p.ty.is_record() && p.ty.size(self.xlen).is_none() {
                self.err(p.pos, format!("variable has incomplete type '{}'", p.ty));
            }
        }
        let ret_complete = !f.ret.is_record() || f.ret.size(self.xlen).is_some();
        if !ret_complete {
            self.err(
                f.pos,
                format!("incomplete result type '{}' in function definition", f.ret),
            );
        }
        let body = f.body.as_mut().expect("checked above");
        let mut c = FnChecker {
            globals: &self.globals,
            func: &f.name,
            ret: &f.ret,
            last_param: f
                .variadic
                .then(|| f.params.last().map(|p| p.name.clone()))
                .flatten(),
            scopes: vec![HashMap::new()],
            locals: Vec::new(),
            loop_depth: 0,
            break_depth: 0,
            switches: Vec::new(),
            next_case: 0,
            labels: HashMap::new(),
            gotos: Vec::new(),
            static_labels: HashSet::new(),
            literals: Vec::new(),
            literal_base: 0,
            errors: &mut self.errors,
            xlen: self.xlen,
        };
        for p in &f.params {
            c.declare(&p.name, &p.ty, p.pos, "parameter ");
        }
        // A big struct result goes where the caller says: keep that address.
        f.ret_ptr = (ret_complete && abi::returns_in_memory(&f.ret, self.xlen))
            .then(|| c.temp(&Type::ptr(f.ret.clone()), "result address", f.pos));
        // The body's outermost block shares the parameters' scope, as in C.
        for s in body.iter_mut() {
            c.stmt(s);
        }
        for (name, pos) in std::mem::take(&mut c.gotos) {
            if !c.labels.contains_key(&name) {
                c.err(pos, format!("use of undeclared label '{name}'"));
            }
        }
        f.locals = c.locals;
    }

    fn global_var(&mut self, d: &mut Declarator, at: (usize, usize)) {
        if d.ty.is_void() {
            return self.err(d.pos, format!("variable '{}' has incomplete type 'void'", d.name));
        }
        if d.init.is_some() && d.storage == Storage::Extern {
            return self.err(
                d.pos,
                format!("'extern' declaration of '{}' cannot have an initializer", d.name),
            );
        }
        d.var = Some(VarRef::Global(d.name.clone()));
        // As in C, the name is in scope from the end of its declarator, so it
        // is declared before its initializer is checked: `void *self = &self;`.
        let is_static = d.storage == Storage::Static;
        let tentative = (d.storage != Storage::Extern).then_some(at);
        let init = d.init.is_some().then_some(at);
        match self.globals.get_mut(&d.name) {
            Some(Global::Func(_)) => {
                return self.err(
                    d.pos,
                    format!("redefinition of '{}' as a different kind of symbol", d.name),
                );
            }
            Some(Global::Var(prev)) => {
                if !compatible(&prev.ty, &d.ty) {
                    let msg = format!(
                        "redefinition of '{}' with a different type: '{}' vs '{}' (declared at {})",
                        d.name, d.ty, prev.ty, prev.pos
                    );
                    return self.err(d.pos, msg);
                }
                // `int a[3]; int a[] = {1};` declares an `int [3]`.
                if d.ty.is_array() && d.ty.size(self.xlen).is_none() {
                    d.ty = prev.ty.clone();
                }
                if prev.init.is_some() && init.is_some() {
                    return self.err(d.pos, format!("redefinition of '{}'", d.name));
                }
                if is_static && !prev.is_static {
                    return self.err(
                        d.pos,
                        format!(
                            "static declaration of '{}' follows non-static declaration",
                            d.name
                        ),
                    );
                }
                if d.storage == Storage::None && prev.is_static {
                    return self.err(
                        d.pos,
                        format!(
                            "non-static declaration of '{}' follows static declaration",
                            d.name
                        ),
                    );
                }
                if !d.ty.is_array() || d.ty.size(self.xlen).is_some() {
                    prev.ty = d.ty.clone();
                }
                prev.init = prev.init.or(init);
                prev.tentative = prev.tentative.or(tentative);
            }
            None => {
                self.globals.insert(
                    d.name.clone(),
                    Global::Var(GlobalVar {
                        ty: d.ty.clone(),
                        is_static,
                        pos: d.pos,
                        init,
                        tentative,
                    }),
                );
            }
        }
        let Some(d_init) = &mut d.init else { return };
        let mut fc = FnChecker::file_scope(&self.globals, &mut self.errors, self.literals.len(), self.xlen);
        let checked = fc.init(&mut d.ty, d_init, true);
        self.literals.extend(fc.literals);
        let Some(Global::Var(g)) = self.globals.get_mut(&d.name) else {
            unreachable!("declared above")
        };
        if checked.is_err() {
            // Still declared, so later uses don't report it as undeclared, but
            // not defined here: there is nothing to emit.
            g.init = g.init.filter(|&i| i != at);
            g.tentative = g.tentative.filter(|&i| i != at);
        } else if g.ty.size(self.xlen).is_none() {
            // The initializer completed an array's size.
            g.ty = d.ty.clone();
        }
    }

    /// Marks the declarator that defines each global variable: the one with an
    /// initializer, or the first that is not `extern` (C's tentative
    /// definition, which then means "zero"). Only `extern` declarations: the
    /// variable is defined in another file.
    fn mark_definitions(&mut self, prog: &mut Program) {
        let mut defs: Vec<((usize, usize), Type)> = Vec::new();
        for g in self.globals.values() {
            if let Global::Var(v) = g {
                if let Some(at) = v.init.or(v.tentative) {
                    defs.push((at, v.ty.clone()));
                }
            }
        }
        defs.sort_by_key(|(at, _)| *at);
        for ((i, j), ty) in defs {
            let Item::Decl(decls) = &mut prog.items[i] else {
                unreachable!("recorded from a declaration")
            };
            let d = &mut decls[j];
            if ty.size(self.xlen).is_none() {
                let msg = if ty.is_array() {
                    format!("definition of '{}' needs the array size ('{}')", d.name, ty)
                } else {
                    format!("variable '{}' has incomplete type '{}'", d.name, ty)
                };
                self.err(d.pos, msg);
            }
            d.define = true;
            d.ty = ty;
        }
    }
}

/// How a value is being converted to a type, for messages.
#[derive(Clone, Copy)]
enum Conv {
    Assign,
    Init,
    Arg,
    Return,
}

impl Conv {
    fn phrase(self, to: &Type, from: &Type) -> String {
        match self {
            Conv::Assign => format!("assigning to '{to}' from '{from}'"),
            Conv::Init => format!("initializing '{to}' with an expression of type '{from}'"),
            Conv::Arg => format!("passing '{from}' to parameter of type '{to}'"),
            Conv::Return => format!("returning '{from}' from a function with result type '{to}'"),
        }
    }
}

/// The `case` labels of a `switch` being checked.
struct SwitchCtx {
    /// The promoted type of the controlling expression: case values convert
    /// to it.
    kind: IntKind,
    cases: Vec<(i128, usize)>,
    seen: HashMap<i128, Pos>,
    default: Option<(usize, Pos)>,
}

/// An element of an initializer list not yet matched to a subobject, and
/// whether its expression has been checked already (to see if it
/// initializes a struct as a whole).
/// How [`FnChecker::fill`] fills an aggregate.
enum Fill {
    /// From its own braced list: designators in it are for it.
    Braced,
    /// By brace elision, from the enclosing list's items: a designator ends
    /// it.
    Elided,
    /// From the rest of a designation that ran into it (`.a.b = 1` fills
    /// `a` from `.b = 1`), changing what it had been initialized with, if
    /// anything; then by brace elision.
    Designated(Vec<Designator>, Option<Init>),
}

/// Array designators go up to this (exclusive): each element up to the
/// designated one gets an item in the initializer.
const MAX_DESIGNATED: i128 = 1 << 20;

struct Pending {
    init: Init,
    checked: bool,
}

struct FnChecker<'a> {
    globals: &'a HashMap<String, Global>,
    /// The function being checked (empty at file scope).
    func: &'a str,
    ret: &'a Type,
    /// In a variadic function, its last named parameter (what `va_start`
    /// names).
    last_param: Option<String>,
    /// Block scopes, innermost last: names to where they live, their type and
    /// where they were declared.
    scopes: Vec<HashMap<String, (VarRef, Type, Pos)>>,
    locals: Vec<Local>,
    /// Enclosing loops (for `continue`), and loops and switches (for `break`).
    loop_depth: usize,
    break_depth: usize,
    /// Enclosing switches, innermost last.
    switches: Vec<SwitchCtx>,
    /// The next id for a `case` or `default`.
    next_case: usize,
    /// The function's labels, and every `goto` (checked at the end, since a
    /// `goto` may jump forward).
    labels: HashMap<String, Pos>,
    gotos: Vec<(String, Pos)>,
    /// Symbols given to `static` locals so far.
    static_labels: HashSet<String>,
    /// At file scope: the objects of the compound literals checked, numbered
    /// from `literal_base` (those of earlier initializers come first).
    literals: Vec<Declarator>,
    literal_base: usize,
    errors: &'a mut Vec<Error>,
    /// The target, for sizes and conversions.
    xlen: Xlen,
}

impl<'a> FnChecker<'a> {
    /// A checker for global initializers, outside any function.
    fn file_scope(
        globals: &'a HashMap<String, Global>,
        errors: &'a mut Vec<Error>,
        literal_base: usize,
        xlen: Xlen,
    ) -> Self {
        FnChecker {
            globals,
            func: "",
            ret: &VOID,
            last_param: None,
            scopes: Vec::new(),
            locals: Vec::new(),
            loop_depth: 0,
            break_depth: 0,
            switches: Vec::new(),
            next_case: 0,
            labels: HashMap::new(),
            gotos: Vec::new(),
            static_labels: HashSet::new(),
            literals: Vec::new(),
            literal_base,
            errors,
            xlen,
        }
    }

    fn err(&mut self, pos: Pos, msg: impl Into<String>) -> Reported {
        self.errors.push(Error::new(pos, msg));
        Reported
    }

    fn declare(&mut self, name: &str, ty: &Type, pos: Pos, what: &str) -> LocalId {
        let id = LocalId(self.locals.len());
        self.locals.push(Local {
            name: name.to_string(),
            ty: ty.clone(),
            pos,
            temp: false,
        });
        self.bind(name, VarRef::Local(id), ty, pos, what);
        id
    }

    /// A hidden local for the calling convention: `what` says what it holds.
    fn temp(&mut self, ty: &Type, what: &str, pos: Pos) -> LocalId {
        let id = LocalId(self.locals.len());
        self.locals.push(Local {
            name: what.to_string(),
            ty: ty.clone(),
            pos,
            temp: true,
        });
        id
    }

    fn bind(&mut self, name: &str, var: VarRef, ty: &Type, pos: Pos, what: &str) {
        let scope = self.scopes.last_mut().expect("at least one scope");
        if let Some((_, _, prev_pos)) = scope.insert(name.to_string(), (var, ty.clone(), pos)) {
            self.err(
                pos,
                format!("redefinition of {what}'{name}' (first declared at {prev_pos})"),
            );
        }
    }

    fn lookup(&self, name: &str) -> Option<&(VarRef, Type, Pos)> {
        self.scopes.iter().rev().find_map(|s| s.get(name))
    }

    fn scoped(&mut self, f: impl FnOnce(&mut Self)) {
        self.scopes.push(HashMap::new());
        f(self);
        self.scopes.pop();
    }

    fn in_loop(&mut self, f: impl FnOnce(&mut Self)) {
        self.loop_depth += 1;
        self.break_depth += 1;
        f(self);
        self.loop_depth -= 1;
        self.break_depth -= 1;
    }

    // ---- statements ----

    fn stmt(&mut self, s: &mut Stmt) {
        match &mut s.kind {
            StmtKind::Block(body) => self.scoped(|c| body.iter_mut().for_each(|s| c.stmt(s))),
            StmtKind::Decl(decls) => {
                for d in decls {
                    self.local_decl(d);
                }
            }
            StmtKind::Expr(e) => {
                let _ = self.expr(e);
            }
            StmtKind::Return(e) => self.ret(e.as_mut(), s.pos),
            StmtKind::Empty => {}
            StmtKind::If { cond, then, els } => {
                let _ = self.condition(cond);
                self.scoped(|c| c.stmt(then));
                if let Some(e) = els {
                    self.scoped(|c| c.stmt(e));
                }
            }
            StmtKind::While { cond, body } => {
                let _ = self.condition(cond);
                self.in_loop(|c| c.scoped(|c| c.stmt(body)));
            }
            StmtKind::DoWhile { body, cond } => {
                self.in_loop(|c| c.scoped(|c| c.stmt(body)));
                let _ = self.condition(cond);
            }
            StmtKind::For {
                init,
                cond,
                step,
                body,
            } => self.scoped(|c| {
                if let Some(i) = init {
                    c.stmt(i);
                }
                if let Some(e) = cond {
                    let _ = c.condition(e);
                }
                if let Some(e) = step {
                    let _ = c.expr(e);
                }
                c.in_loop(|c| c.scoped(|c| c.stmt(body)));
            }),
            StmtKind::Break if self.break_depth == 0 => {
                self.err(s.pos, "'break' statement not in a loop or switch");
            }
            StmtKind::Continue if self.loop_depth == 0 => {
                self.err(s.pos, "'continue' statement not in a loop");
            }
            StmtKind::Break | StmtKind::Continue => {}
            StmtKind::Switch {
                cond,
                body,
                cases,
                default,
            } => {
                // The controlling expression is promoted; case values convert to it.
                let kind = match self.expr(cond) {
                    Err(_) => IntKind::Int,
                    Ok(()) => match cond.ty.decay() {
                        Type::Int(k) => {
                            coerce(cond, &Type::Int(k.promoted()));
                            k.promoted()
                        }
                        t => {
                            let msg =
                                format!("statement requires expression of integer type ('{t}' invalid)");
                            self.err(cond.pos, msg);
                            IntKind::Int
                        }
                    },
                };
                self.switches.push(SwitchCtx {
                    kind,
                    cases: Vec::new(),
                    seen: HashMap::new(),
                    default: None,
                });
                self.break_depth += 1;
                self.scoped(|c| c.stmt(body));
                self.break_depth -= 1;
                let ctx = self.switches.pop().expect("pushed above");
                *cases = ctx.cases;
                *default = ctx.default.map(|(id, _)| id);
            }
            StmtKind::Case {
                expr,
                value,
                id,
                body,
            } => {
                let pos = s.pos;
                let checked = self.expr(expr);
                if self.switches.is_empty() {
                    self.err(pos, "'case' statement not in switch statement");
                } else if checked.is_ok() {
                    match consteval::int(expr, self.xlen).filter(|_| expr.ty.is_integer()) {
                        None => {
                            self.err(expr.pos, "case value is not an integer constant expression");
                        }
                        Some(v) => {
                            *id = self.next_case;
                            self.next_case += 1;
                            let sw = self.switches.last_mut().expect("checked above");
                            *value = sw.kind.wrap(v, self.xlen);
                            if let Some(prev) = sw.seen.insert(*value, expr.pos) {
                                let msg =
                                    format!("duplicate case value '{}' (previous case at {prev})", *value);
                                self.err(expr.pos, msg);
                            } else {
                                sw.cases.push((*value, *id));
                            }
                        }
                    }
                }
                self.stmt(body);
            }
            StmtKind::Default { id, body } => {
                let pos = s.pos;
                *id = self.next_case;
                self.next_case += 1;
                match self.switches.last_mut() {
                    None => {
                        self.err(pos, "'default' statement not in switch statement");
                    }
                    Some(SwitchCtx {
                        default: Some((_, prev)),
                        ..
                    }) => {
                        let msg = format!("multiple default labels in one switch (first at {prev})");
                        self.err(pos, msg);
                    }
                    Some(sw) => sw.default = Some((*id, pos)),
                }
                self.stmt(body);
            }
            StmtKind::Label { name, body } => {
                if let Some(prev) = self.labels.insert(name.clone(), s.pos) {
                    let msg = format!("redefinition of label '{name}' (first defined at {prev})");
                    self.err(s.pos, msg);
                }
                self.stmt(body);
            }
            StmtKind::Goto(name) => self.gotos.push((name.clone(), s.pos)),
        }
    }

    fn ret(&mut self, e: Option<&mut Expr>, pos: Pos) {
        let ret = self.ret;
        match e {
            Some(e) => {
                if self.expr(e).is_err() {
                    return;
                }
                if *ret == Type::Void {
                    let msg = format!("void function '{}' should not return a value", self.func);
                    self.err(e.pos, msg);
                } else if self.convert(e, ret, Conv::Return).is_ok() {
                    coerce(e, ret);
                }
            }
            None if *ret != Type::Void => {
                let msg = format!("non-void function '{}' should return a value", self.func);
                self.err(pos, msg);
            }
            None => {}
        }
    }

    /// A condition (`if`, loops, `&&`, `||`, `!`, `?:`) must be a scalar.
    fn condition(&mut self, e: &mut Expr) -> Checked {
        self.expr(e)?;
        if !e.ty.decay().is_scalar() {
            let msg = format!(
                "statement requires expression of scalar type ('{}' invalid)",
                e.ty
            );
            return Err(self.err(e.pos, msg));
        }
        Ok(())
    }

    fn local_decl(&mut self, d: &mut Declarator) {
        if d.ty.is_void() {
            self.err(d.pos, format!("variable '{}' has incomplete type 'void'", d.name));
            return;
        }
        if d.storage == Storage::Static {
            // A static local is a global under a name of its own, visible only
            // in this block: `function.name`, numbered if the function has
            // several of the same name.
            let mut label = format!("{}.{}", self.func, d.name);
            let mut n = 0;
            while self.static_labels.contains(&label) {
                n += 1;
                label = format!("{}.{}.{n}", self.func, d.name);
            }
            self.static_labels.insert(label.clone());
            // As in C, the name is in scope inside its own initializer, so
            // `static struct node head = { &head }` means this `head`.
            let var = VarRef::Global(label);
            self.bind(&d.name, var.clone(), &d.ty, d.pos, "");
            if let Some(init) = &mut d.init {
                if self.init(&mut d.ty, init, true).is_err() {
                    return;
                }
                // The initializer may have completed an array's size.
                let binding = (var.clone(), d.ty.clone(), d.pos);
                self.scopes.last_mut().unwrap().insert(d.name.clone(), binding);
            }
            d.var = Some(var);
            d.define = true;
        } else {
            // As in C, the name is in scope inside its own initializer.
            let id = self.declare(&d.name, &d.ty, d.pos, "");
            d.var = Some(VarRef::Local(id));
            if let Some(init) = &mut d.init {
                if self.init(&mut d.ty, init, false).is_err() {
                    return;
                }
                // The initializer may have completed an array's size.
                self.locals[id.0].ty = d.ty.clone();
                let binding = (VarRef::Local(id), d.ty.clone(), d.pos);
                self.scopes.last_mut().unwrap().insert(d.name.clone(), binding);
            }
        }
        if d.ty.size(self.xlen).is_none() {
            let msg = format!("variable '{}' has incomplete type '{}'", d.name, d.ty);
            self.err(d.pos, msg);
        }
    }

    /// Checks an initializer against the type it initializes, completing an
    /// array of unknown size, and rewrites a list into its fully braced form
    /// (see [`Init::List`]). `constant`: for globals and statics, every
    /// element must be a constant expression.
    fn init(&mut self, ty: &mut Type, init: &mut Init, constant: bool) -> Checked {
        let Init::List(items, pos) = init else {
            let Init::Expr(e) = init else { unreachable!() };
            return self.init_expr(ty, e, constant, false);
        };
        let pos = *pos;
        // `{ x }` for a scalar, or `{ "text" }` for a char array, means `x`.
        let unwrap = match &items[..] {
            [Init::Expr(_)] if !ty.is_array() && !ty.is_record() => true,
            [Init::Expr(Expr {
                kind: ExprKind::Str(_),
                ..
            })] => is_char_array(ty),
            _ => false,
        };
        if unwrap {
            let Init::List(mut items, _) = std::mem::replace(init, Init::List(Vec::new(), pos)) else {
                unreachable!()
            };
            *init = items.remove(0);
            let Init::Expr(e) = init else { unreachable!() };
            return self.init_expr(ty, e, constant, false);
        }
        if !ty.is_array() && !ty.is_record() {
            if let Init::Designated(d, _) = &mut items[0] {
                let first = &mut d[0];
                self.designator(ty, first)?;
            }
            let what = if matches!(items[0], Init::List(..)) {
                "too many braces around scalar initializer"
            } else {
                "excess elements in scalar initializer"
            };
            return Err(self.err(pos, what));
        }
        let mut queue: VecDeque<Pending> = std::mem::take(items)
            .into_iter()
            .map(|init| Pending { init, checked: false })
            .collect();
        let list = self.fill(ty, &mut queue, constant, pos, Fill::Braced)?;
        if let Some(extra) = queue.front() {
            let what = match ty.record() {
                Some(r) => r.keyword(),
                None => "array",
            };
            return Err(self.err(extra.init.pos(), format!("excess elements in {what} initializer")));
        }
        *init = list;
        Ok(())
    }

    /// Initializes the aggregate `ty` from the front of `queue`: subobject
    /// after subobject (elements, or members; for a union only one), as long
    /// as there are items and subobjects. A designator moves to the
    /// subobject it names, and initialization goes on from the one after
    /// it. Returns the fully braced list (see [`Init::List`]).
    fn fill(
        &mut self,
        ty: &mut Type,
        queue: &mut VecDeque<Pending>,
        constant: bool,
        pos: Pos,
        how: Fill,
    ) -> Result<Init, Reported> {
        let record = ty.record().cloned();
        if let Some(r) = &record {
            if r.body().is_none() {
                let msg = format!("initializer for incomplete type '{ty}'");
                return Err(self.err(pos, msg));
            }
        }
        let (braced, mut designators, old) = match how {
            Fill::Braced => (true, Vec::new(), None),
            Fill::Elided => (false, Vec::new(), None),
            Fill::Designated(rest, old) => (false, rest, old),
        };
        // The initializer of each subobject so far (`None`: zero). A
        // designation into a subobject changes what it had.
        let mut slots: Vec<Option<Init>> = match old {
            None => Vec::new(),
            Some(Init::List(items, _)) => items.into_iter().map(Some).collect(),
            // `{ "ab", [0][3] = 'z' }`: the string becomes its characters,
            // one item each, so the designation changes only the one it
            // names (C99 6.7.8p19). Left-out ones, the NUL among them, are
            // zero anyway.
            Some(Init::Expr(Expr {
                kind: ExprKind::Str(bytes),
                pos: at,
                ..
            })) => {
                let Type::Array(elem, _) = &*ty else {
                    unreachable!("a string initializes a char array")
                };
                bytes
                    .into_iter()
                    .map(|b| {
                        let mut c = Expr::new(ExprKind::Num(b.into()), at);
                        coerce(&mut c, elem);
                        Some(Init::Expr(c))
                    })
                    .collect()
            }
            // `{ pv, .p.y = 9 }` keeps `pv`'s other members in C. That
            // needs the value copied first and then changed, which merlin
            // does not do: an error rather than a different meaning.
            Some(Init::Expr(e)) => {
                let msg = format!(
                    "designator partially overrides an initializer of type '{}' given by an \
                     expression; merlin does not support this",
                    e.ty
                );
                let at = designators.first().map_or(pos, Designator::pos);
                return Err(self.err(at, msg));
            }
            Some(Init::Designated(..)) => unreachable!("fill leaves no designations"),
        };
        let mut next = 0;
        loop {
            if designators.is_empty() {
                match queue.front() {
                    None => break,
                    // A designator belongs to the innermost braced list: an
                    // aggregate filled by brace elision ends there.
                    Some(Pending {
                        init: Init::Designated(..),
                        ..
                    }) if !braced => break,
                    Some(Pending {
                        init: Init::Designated(..),
                        ..
                    }) => {
                        let Some(Pending {
                            init: Init::Designated(d, value),
                            ..
                        }) = queue.pop_front()
                        else {
                            unreachable!("just matched")
                        };
                        designators = d;
                        queue.push_front(Pending {
                            init: *value,
                            checked: false,
                        });
                    }
                    Some(_) => {}
                }
            }
            let designated = !designators.is_empty();
            if designated {
                let mut first = designators.remove(0);
                next = self.designator(ty, &mut first)?;
            } else if match (&*ty, &record) {
                (Type::Array(_, n), _) => n.is_some_and(|n| next >= n as usize),
                (_, Some(r)) if r.union => next >= 1,
                (_, Some(r)) => next >= r.body().expect("checked above").members.len(),
                _ => unreachable!("an aggregate"),
            } {
                break;
            }
            let mut sub = match (&*ty, &record) {
                // Inner dimensions are always known: the parser requires them.
                (Type::Array(elem, _), _) => (**elem).clone(),
                (_, Some(r)) => r.body().expect("checked above").members[next].ty.clone(),
                _ => unreachable!("an aggregate"),
            };
            if slots.len() <= next {
                slots.resize(next + 1, None);
            }
            let init = if designators.is_empty() {
                self.subobject(&mut sub, queue, constant)?
            } else {
                // `.a.b = 1`: `a` from `.b = 1`, then what follows.
                let rest = std::mem::take(&mut designators);
                let old = slots[next].take();
                let at = queue.front().expect("the designated value").init.pos();
                self.fill(&mut sub, queue, constant, at, Fill::Designated(rest, old))?
            };
            // A union holds one member: the last one initialized.
            if record.as_ref().is_some_and(|r| r.union) {
                slots.iter_mut().for_each(|s| *s = None);
            }
            slots[next] = Some(init);
            next += 1;
        }
        if let Type::Array(_, n @ None) = ty {
            *n = Some(slots.len() as u32);
        }
        // A union's earlier member, dropped for a later one.
        while slots.last().is_some_and(Option::is_none) {
            slots.pop();
        }
        let items = slots
            .into_iter()
            .map(|s| s.unwrap_or_else(|| Init::List(Vec::new(), pos)))
            .collect();
        Ok(Init::List(items, pos))
    }

    /// Which subobject of `ty` a designator names: an element index, or a
    /// member's.
    fn designator(&mut self, ty: &Type, d: &mut Designator) -> Result<usize, Reported> {
        match d {
            Designator::Index(e, pos) => {
                let pos = *pos;
                let Type::Array(elem, n) = ty.unqual() else {
                    return Err(self.err(
                        pos,
                        format!("array designator cannot initialize non-array type '{ty}'"),
                    ));
                };
                self.expr(e)?;
                let Some(i) = consteval::int(e, self.xlen).filter(|_| e.ty.is_integer()) else {
                    return Err(self.err(e.pos, "expression is not an integer constant expression"));
                };
                if i < 0 {
                    return Err(self.err(e.pos, format!("array designator value '{i}' is negative")));
                }
                if let Some(n) = n {
                    if i >= i128::from(*n) {
                        let msg = format!("array designator index ({i}) exceeds array bounds ({n})");
                        return Err(self.err(e.pos, msg));
                    }
                }
                // Each element up to the designated one gets an item, so the
                // index must be reasonable even when the array is big.
                let size = i128::from(elem.size(self.xlen).unwrap_or(1).max(1));
                if i >= MAX_DESIGNATED || (i + 1) * size > i128::from(i32::MAX) {
                    let msg = format!(
                        "array designator index ({i}) is too large: merlin supports up to {}",
                        MAX_DESIGNATED - 1
                    );
                    return Err(self.err(e.pos, msg));
                }
                Ok(i as usize)
            }
            Designator::Member(name, pos) => {
                let pos = *pos;
                let body = match ty.record() {
                    Some(r) => r.body(),
                    None => {
                        let msg =
                            format!("field designator cannot initialize a non-struct, non-union type '{ty}'");
                        return Err(self.err(pos, msg));
                    }
                };
                let body = body.expect("fill checks completeness first");
                match body.members.iter().position(|m| m.name == *name) {
                    Some(i) => Ok(i),
                    None => Err(self.err(
                        pos,
                        format!("field designator '{name}' does not refer to any field in type '{ty}'"),
                    )),
                }
            }
        }
    }

    /// The initializer of one subobject of type `ty`, from the front of
    /// `queue`: a braced list or an expression for the whole subobject, or,
    /// when it is an aggregate the next expression does not initialize as a
    /// whole, as many items as it takes (C's brace elision: in
    /// `int m[2][2] = {1, 2, 3, 4}`, `1, 2` initialize `m[0]`).
    fn subobject(
        &mut self,
        ty: &mut Type,
        queue: &mut VecDeque<Pending>,
        constant: bool,
    ) -> Result<Init, Reported> {
        let front = queue.front_mut().expect("not empty");
        let elide = match &mut front.init {
            Init::List(..) | Init::Designated(..) => false,
            Init::Expr(e) if ty.is_array() => !(is_char_array(ty) && matches!(e.kind, ExprKind::Str(_))),
            Init::Expr(e) if ty.is_record() => {
                // A struct is initialized by a struct of its type, or member by member.
                if !front.checked {
                    front.checked = true;
                    self.expr(e)?;
                }
                e.ty.unqual() != ty.unqual()
            }
            Init::Expr(_) => false,
        };
        if elide {
            let pos = queue.front().expect("not empty").init.pos();
            return self.fill(ty, queue, constant, pos, Fill::Elided);
        }
        let Pending { mut init, checked } = queue.pop_front().expect("not empty");
        match &mut init {
            Init::Expr(e) => self.init_expr(ty, e, constant, checked)?,
            Init::List(..) => self.init(ty, &mut init, constant)?,
            Init::Designated(..) => unreachable!("fill takes designators"),
        }
        Ok(init)
    }

    /// An object of type `ty` initialized by the expression `e` (already
    /// checked, if `checked`): a string literal for a char array, otherwise
    /// as if by assignment.
    fn init_expr(&mut self, ty: &mut Type, e: &mut Expr, constant: bool, checked: bool) -> Checked {
        match ty {
            Type::Array(elem, n) if is_char_array_elem(elem) => {
                let ExprKind::Str(bytes) = &e.kind else {
                    return Err(self.err(
                        e.pos,
                        "array initializer must be an initializer list or a string literal",
                    ));
                };
                // The terminating NUL may be left out when the array is exactly
                // as long as the text, as C allows.
                let len = bytes.len() as u32;
                match n {
                    None => *n = Some(len + 1),
                    Some(n) if len > *n => {
                        return Err(self.err(e.pos, "initializer-string for char array is too long"));
                    }
                    Some(_) => {}
                }
                e.ty = Type::array(Type::CHAR, Some(len + 1));
                Ok(())
            }
            Type::Array(..) => Err(self.err(e.pos, "array initializer must be an initializer list")),
            _ => {
                if !checked {
                    self.expr(e)?;
                }
                self.convert(e, ty, Conv::Init)?;
                coerce(e, ty);
                if constant && consteval::eval(e, self.xlen).is_none() {
                    return Err(self.err(e.pos, "initializer element is not a compile-time constant"));
                }
                Ok(())
            }
        }
    }

    // ---- expressions ----

    /// Checks whether `e`'s value may be converted to `to` as if by assignment
    /// (C99 6.5.16.1): arithmetic to arithmetic, a struct to the same struct,
    /// a pointer to a pointer of the same type or to or from `void *` (for
    /// objects, not functions), keeping every qualifier of what it points to,
    /// the null pointer constant to any pointer, and a pointer to `_Bool`.
    fn convert(&mut self, e: &Expr, to: &Type, how: Conv) -> Checked {
        let to = to.unqual();
        let from = e.ty.decay();
        let problem = match (to, &from) {
            (Type::Int(_), Type::Int(_)) => return Ok(()),
            (Type::Int(IntKind::Bool), Type::Ptr(_)) => return Ok(()),
            (Type::Record(a), Type::Record(b)) if a == b => {
                if to.size(self.xlen).is_none() {
                    let msg = format!("incomplete type '{to}' cannot be copied");
                    return Err(self.err(e.pos, msg));
                }
                return Ok(());
            }
            (Type::Ptr(a), Type::Ptr(b)) => {
                let (au, bu) = (a.unqual(), b.unqual());
                let compatible =
                    au == bu || (au.is_void() && !bu.is_func()) || (bu.is_void() && !au.is_func());
                if !compatible {
                    "incompatible pointer types"
                } else if !a.quals().contains(b.quals()) {
                    let msg = format!("{} discards qualifiers", how.phrase(to, &from));
                    return Err(self.err(e.pos, msg));
                } else {
                    return Ok(());
                }
            }
            (Type::Ptr(_), Type::Int(_)) if consteval::is_null_pointer(e, self.xlen) => return Ok(()),
            (Type::Ptr(_), Type::Int(_)) => "incompatible integer to pointer conversion",
            (Type::Int(_), Type::Ptr(_)) => "incompatible pointer to integer conversion",
            _ => "incompatible types",
        };
        let msg = format!("{problem} {}", how.phrase(to, &from));
        Err(self.err(e.pos, msg))
    }

    /// The target of `=`, `op=`, `++` and `--`: a modifiable lvalue (not an
    /// array, a function, or anything `const`).
    fn assignable(&mut self, target: &Expr) -> Checked {
        if !is_lvalue(target) || matches!(target.kind, ExprKind::Str(_)) {
            return Err(self.err(target.pos, "expression is not assignable"));
        }
        let ty = &target.ty;
        let problem = if ty.is_array() {
            format!("array type '{ty}' is not assignable")
        } else if ty.is_func() {
            format!("non-object type '{ty}' is not assignable")
        } else if ty.quals().konst {
            match &target.kind {
                ExprKind::Var { name, .. } => {
                    format!("cannot assign to variable '{name}' with const-qualified type '{ty}'")
                }
                ExprKind::Member { name, .. } => {
                    format!("cannot assign to member '{name}' with const-qualified type '{ty}'")
                }
                _ => "read-only variable is not assignable".to_string(),
            }
        } else if has_const_member(ty) {
            format!("cannot assign to an object of type '{ty}': it has a const-qualified member")
        } else {
            return Ok(());
        };
        Err(self.err(target.pos, problem))
    }

    fn invalid_operands(&mut self, pos: Pos, l: &Type, r: &Type) -> Reported {
        let msg = if l.pointee().is_some_and(Type::is_void) || r.pointee().is_some_and(Type::is_void) {
            "arithmetic on a pointer to void".to_string()
        } else {
            format!("invalid operands to binary expression ('{l}' and '{r}')")
        };
        self.err(pos, msg)
    }

    /// Types `e` and everything in it.
    fn expr(&mut self, e: &mut Expr) -> Checked {
        let pos = e.pos;
        if matches!(e.kind, ExprKind::VaEnd(_) | ExprKind::VaCopy(..)) {
            return self.va_end_or_copy(e);
        }
        let globals = self.globals;
        let ty = match &mut e.kind {
            ExprKind::Num(_) | ExprKind::Str(_) => return Ok(()),
            ExprKind::Var { name, var } => match self.lookup(name) {
                Some((v, ty, _)) => {
                    *var = Some(v.clone());
                    ty.clone()
                }
                None => match globals.get(name.as_str()) {
                    Some(Global::Var(g)) => {
                        *var = Some(VarRef::Global(name.clone()));
                        g.ty.clone()
                    }
                    // A function's name designates the function.
                    Some(Global::Func(sig)) => {
                        *var = Some(VarRef::Global(name.clone()));
                        Type::Func(Box::new(sig.ty.clone()))
                    }
                    None => return Err(self.err(pos, format!("use of undeclared identifier '{name}'"))),
                },
            },
            ExprKind::Assign { op, target, value } => {
                let (a, b) = (self.expr(target), self.expr(value));
                a?;
                b?;
                self.assignable(target)?;
                match op {
                    None => {
                        self.convert(value, &target.ty, Conv::Assign)?;
                        coerce(value, &target.ty);
                    }
                    Some(op) => {
                        // `t op= v` computes `t op v`, then stores it as `t = ...` would.
                        let (t, v) = (target.ty.decay(), value.ty.decay());
                        let ok = match arith_type(*op, &t, &v, self.xlen) {
                            Some(r) => (t.is_integer() && r.is_integer()) || r == t,
                            None => false,
                        };
                        if !ok {
                            return Err(self.invalid_operands(pos, &t, &v));
                        }
                    }
                }
                // The value of an assignment is what was stored: no qualifiers.
                target.ty.unqual().clone()
            }
            ExprKind::IncDec { inc, target, .. } => {
                self.expr(target)?;
                self.assignable(target)?;
                let ok = match target.ty.unqual() {
                    Type::Int(_) => true,
                    Type::Ptr(t) => t.size(self.xlen).is_some(),
                    _ => false,
                };
                if !ok {
                    let what = if *inc { "increment" } else { "decrement" };
                    let msg = format!("cannot {what} value of type '{}'", target.ty);
                    return Err(self.err(pos, msg));
                }
                target.ty.unqual().clone()
            }
            ExprKind::Binary(op, l, r) => {
                let (a, b) = (self.expr(l), self.expr(r));
                a?;
                b?;
                self.binary(*op, l, r, pos)?
            }
            ExprKind::Unary(op, x) => {
                self.expr(x)?;
                let t = x.ty.decay();
                match (*op, &t) {
                    (UnOp::Not, t) if t.is_scalar() => Type::INT,
                    (UnOp::Neg | UnOp::Plus | UnOp::BitNot, Type::Int(k)) => Type::Int(k.promoted()),
                    _ => {
                        let msg = format!(
                            "invalid argument type '{t}' to unary expression '{}'",
                            op.symbol()
                        );
                        return Err(self.err(pos, msg));
                    }
                }
            }
            ExprKind::Addr(x) => {
                self.expr(x)?;
                if !is_lvalue(x) {
                    let msg = format!("cannot take the address of an rvalue of type '{}'", x.ty);
                    return Err(self.err(pos, msg));
                }
                Type::ptr(x.ty.clone())
            }
            ExprKind::Deref(x) => {
                // The parser writes `a[i]` as `*(a + i)`, both at the `[`.
                let subscript = matches!(&x.kind, ExprKind::Binary(BinOp::Add, ..)) && x.pos == pos;
                self.expr(x)?;
                match x.ty.decay() {
                    Type::Ptr(t) if t.is_void() => {
                        return Err(self.err(pos, "dereferencing a 'void *' pointer"));
                    }
                    Type::Ptr(t) => *t,
                    t => {
                        let msg = if subscript {
                            "subscripted value is not an array or pointer".to_string()
                        } else {
                            format!("indirection requires pointer operand ('{t}' invalid)")
                        };
                        return Err(self.err(pos, msg));
                    }
                }
            }
            ExprKind::Cast { x, .. } => {
                self.expr(x)?;
                let to = &e.ty;
                let from = x.ty.decay();
                let ok = match to.unqual() {
                    Type::Void => true,
                    Type::Int(_) | Type::Ptr(_) => from.is_scalar(),
                    _ => false,
                };
                if !ok {
                    let msg = format!("cannot cast from '{from}' to '{to}'");
                    return Err(self.err(pos, msg));
                }
                return Ok(());
            }
            ExprKind::SizeofType(t) => {
                let t = t.clone();
                return self.sizeof(e, &t);
            }
            ExprKind::SizeofExpr(x) => {
                // The operand is typed (and checked) but never evaluated,
                // so a file-scope compound literal in it is no object:
                // drop the static variable checking it made.
                let mark = self.literals.len();
                let r = self.expr(x);
                self.literals.truncate(mark);
                r?;
                let t = x.ty.clone();
                return self.sizeof(e, &t);
            }
            ExprKind::Cond(c, a, b) => {
                let (r1, r2, r3) = (self.condition(c), self.expr(a), self.expr(b));
                r1?;
                r2?;
                r3?;
                self.cond_type(a, b, pos)?
            }
            ExprKind::Comma(a, b) => {
                let (r1, r2) = (self.expr(a), self.expr(b));
                r1?;
                r2?;
                let t = b.ty.decay();
                coerce(b, &t);
                t
            }
            ExprKind::Member {
                base,
                name,
                arrow,
                offset,
            } => {
                self.expr(base)?;
                // `p->m` reaches through a pointer; `s.m` names a part of `s`.
                let (outer, quals) = if *arrow {
                    match base.ty.decay() {
                        Type::Ptr(t) => {
                            let q = t.quals();
                            (*t, q)
                        }
                        t => {
                            let msg = format!("member reference type '{t}' is not a pointer");
                            return Err(self.err(pos, msg));
                        }
                    }
                } else {
                    (base.ty.clone(), base.ty.quals())
                };
                let Some(record) = outer.record() else {
                    let msg = format!("member reference base type '{outer}' is not a structure or union");
                    return Err(self.err(pos, msg));
                };
                let Some(body) = record.body() else {
                    let msg = format!("incomplete definition of type '{}'", record.name());
                    return Err(self.err(pos, msg));
                };
                let Some(m) = body.member(name) else {
                    let msg = format!("no member named '{name}' in '{}'", record.name());
                    return Err(self.err(pos, msg));
                };
                *offset = m.offset;
                // A member of a const struct is const too.
                m.ty.clone().qualified(quals)
            }
            ExprKind::Call {
                callee,
                args,
                result,
                arg_copies,
            } => {
                if let ExprKind::Var { name, .. } = &callee.kind {
                    if self.lookup(name).is_none() && !globals.contains_key(name.as_str()) {
                        return Err(self.err(pos, format!("call to undeclared function '{name}'")));
                    }
                }
                let callee_ok = self.expr(callee);
                let mut ok = true;
                for a in args.iter_mut() {
                    ok &= self.expr(a).is_ok();
                }
                callee_ok?;
                // A function name decays to a pointer to the function, so
                // `f(x)`, `fp(x)` and `(*fp)(x)` all call through a pointer.
                let ft = match callee.ty.decay() {
                    Type::Ptr(t) if t.is_func() => t.func_type().expect("checked").clone(),
                    t => {
                        let msg = format!("called object type '{t}' is not a function or function pointer");
                        return Err(self.err(pos, msg));
                    }
                };
                let arity = ft.params.len();
                if args.len() < arity || (args.len() > arity && !ft.variadic) {
                    let which = if args.len() > arity { "many" } else { "few" };
                    let at_least = if ft.variadic { "at least " } else { "" };
                    let msg = match callee.direct_callee().map(|n| (n, globals.get(n))) {
                        Some((name, Some(Global::Func(sig)))) => format!(
                            "too {which} arguments to '{name}': expected {at_least}{arity}, have {} \
                             (declared at {})",
                            args.len(),
                            sig.pos
                        ),
                        _ => format!(
                            "too {which} arguments to function call: expected {at_least}{arity}, have {}",
                            args.len()
                        ),
                    };
                    return Err(self.err(pos, msg));
                }
                if !ok {
                    return Err(Reported);
                }
                *arg_copies = vec![None; args.len()];
                for (i, (a, p)) in args.iter_mut().zip(&ft.params).enumerate() {
                    // Each argument converts to its parameter's type as if assigned.
                    if self.convert(a, p, Conv::Arg).is_err() {
                        ok = false;
                        continue;
                    }
                    coerce(a, p);
                    // A struct too big for registers is passed as the address of a copy.
                    if p.is_record() && abi::classify(p, self.xlen) == abi::Pass::Ref {
                        arg_copies[i] = Some(self.temp(p, "argument copy", a.pos));
                    }
                }
                ok &= self.variadic_args(&mut args[arity..], &mut arg_copies[arity..]);
                if !ok {
                    return Err(Reported);
                }
                if ft.ret.is_record() {
                    if ft.ret.size(self.xlen).is_none() {
                        let msg = format!("calling a function with incomplete result type '{}'", ft.ret);
                        return Err(self.err(pos, msg));
                    }
                    // A struct result is kept in a hidden local of the caller.
                    *result = Some(self.temp(&ft.ret, "call result", pos));
                }
                ft.ret
            }
            ExprKind::VaStart { ap, last } => {
                self.va_start(ap, last, pos)?;
                Type::Void
            }
            ExprKind::VaArg(ap) => {
                let t = e.ty.clone();
                self.va_arg(ap, &t, pos)?;
                t
            }
            ExprKind::VaEnd(_) | ExprKind::VaCopy(..) => unreachable!("rewritten above"),
            ExprKind::CompoundLit { init, local } => {
                let mut ty = e.ty.clone();
                if self.func.is_empty() {
                    let init = std::mem::replace(init.as_mut(), Init::List(Vec::new(), pos));
                    let name = self.static_literal(&mut ty, init, pos)?;
                    let var = Some(VarRef::Global(name.clone()));
                    e.kind = ExprKind::Var { name, var };
                } else {
                    *local = Some(self.compound_literal(&mut ty, init, pos)?);
                }
                ty
            }
        };
        e.ty = ty;
        Ok(())
    }

    /// Checks a compound literal's type: any complete object type, or an
    /// array of unknown size (which the list completes).
    fn literal_type(&mut self, ty: &Type, pos: Pos) -> Checked {
        if ty.is_func()
            || ty.is_void()
            || (ty.size(self.xlen).is_none() && !matches!(ty, Type::Array(_, None)))
        {
            return Err(self.err(pos, format!("compound literal has incomplete type '{ty}'")));
        }
        Ok(())
    }

    /// `(type){ ... }` in a function: checks the list against the type
    /// (completing an array's size) and gives the object a hidden local.
    fn compound_literal(&mut self, ty: &mut Type, init: &mut Init, pos: Pos) -> Result<LocalId, Reported> {
        self.literal_type(ty, pos)?;
        self.init(ty, init, false)?;
        Ok(self.temp(ty, "compound literal", pos))
    }

    /// `(type){ ... }` at file scope: an object with static storage, so its
    /// list must be constant, like a global's. It becomes a `static`
    /// variable of its own, whose name this returns.
    fn static_literal(&mut self, ty: &mut Type, mut init: Init, pos: Pos) -> Result<String, Reported> {
        self.literal_type(ty, pos)?;
        self.init(ty, &mut init, true)?;
        // Numbered after any literal nested in this one's list.
        let name = format!(".compoundliteral.{}", self.literal_base + self.literals.len());
        self.literals.push(Declarator {
            name: name.clone(),
            ty: ty.clone(),
            storage: Storage::Static,
            init: Some(init),
            var: Some(VarRef::Global(name.clone())),
            define: true,
            pos,
        });
        Ok(name)
    }

    // The varargs checks live in functions of their own: `expr` recurses
    // once per nesting level, so its stack frame is kept small.

    /// The arguments a call's `...` takes get the default argument
    /// promotions: integers narrower than `int` become `int`, arrays and
    /// functions pointers; structs go as they are (a big one as the address
    /// of a copy, recorded in `copies`). False if one is incomplete.
    fn variadic_args(&mut self, args: &mut [Expr], copies: &mut [Option<LocalId>]) -> bool {
        let mut ok = true;
        for (a, copy) in args.iter_mut().zip(copies) {
            let t = a.ty.decay();
            if t.is_void() || (t.is_record() && t.size(self.xlen).is_none()) {
                self.err(a.pos, format!("argument type '{t}' is incomplete"));
                ok = false;
                continue;
            }
            let promoted = match t {
                Type::Int(k) => Type::Int(k.promoted()),
                t => t,
            };
            coerce(a, &promoted);
            if promoted.is_record() && abi::classify(&promoted, self.xlen) == abi::Pass::Ref {
                *copy = Some(self.temp(&promoted, "argument copy", a.pos));
            }
        }
        ok
    }

    /// `va_start(ap, last)`: only in a variadic function, naming its last
    /// named parameter (as C99 asks; merlin does not need it).
    fn va_start(&mut self, ap: &mut Expr, last: &mut Expr, pos: Pos) -> Checked {
        let Some(param) = self.last_param.clone() else {
            return Err(self.err(pos, "'va_start' used in a function with fixed parameters"));
        };
        self.va_list(ap, "__builtin_va_start", true)?;
        self.expr(last)?;
        if !matches!(&last.kind, ExprKind::Var { name, .. } if *name == param) {
            return Err(self.err(
                last.pos,
                format!("second argument to 'va_start' must be the last named parameter, '{param}'"),
            ));
        }
        Ok(())
    }

    /// `va_arg(ap, t)`: `t` must be a complete object type that the default
    /// argument promotions leave alone.
    fn va_arg(&mut self, ap: &mut Expr, t: &Type, pos: Pos) -> Checked {
        self.va_list(ap, "__builtin_va_arg", true)?;
        let problem = if t.is_void() || t.is_func() || t.size(self.xlen).is_none() {
            Some(format!("second argument to 'va_arg' is of incomplete type '{t}'"))
        } else if t.is_array() {
            Some(format!("second argument to 'va_arg' is of array type '{t}'"))
        } else if t.int_kind().is_some_and(|k| k.promoted() != k) {
            Some(format!(
                "second argument to 'va_arg' is of promotable type '{t}': the argument was \
                 promoted to 'int', so this would be undefined (write '({t})va_arg(ap, int)')"
            ))
        } else {
            None
        };
        match problem {
            Some(msg) => Err(self.err(pos, msg)),
            None => Ok(()),
        }
    }

    /// Checks that `ap`, the first argument of the builtin `what`, is a
    /// `va_list` (an object, which the builtin changes, if `target`).
    fn va_list(&mut self, ap: &mut Expr, what: &str, target: bool) -> Checked {
        self.expr(ap)?;
        if ap.ty.decay() != Type::ptr(Type::Void) {
            let msg = format!("first argument to '{what}' must be a 'va_list', not '{}'", ap.ty);
            return Err(self.err(ap.pos, msg));
        }
        if target {
            self.assignable(ap)?;
        }
        Ok(())
    }

    /// `va_end(ap)` does nothing: it becomes `(void)ap`. `va_copy(dst,
    /// src)` copies a pointer: `(void)(dst = src)`.
    fn va_end_or_copy(&mut self, e: &mut Expr) -> Checked {
        let pos = e.pos;
        let x = match std::mem::replace(&mut e.kind, ExprKind::Num(0)) {
            ExprKind::VaEnd(mut ap) => {
                self.va_list(&mut ap, "__builtin_va_end", false)?;
                ap
            }
            ExprKind::VaCopy(mut dst, mut src) => {
                let (a, b) = (
                    self.va_list(&mut dst, "__builtin_va_copy", true),
                    self.va_list(&mut src, "__builtin_va_copy", false),
                );
                a?;
                b?;
                coerce(&mut src, &dst.ty);
                let ty = dst.ty.unqual().clone();
                Box::new(Expr {
                    kind: ExprKind::Assign {
                        op: None,
                        target: dst,
                        value: src,
                    },
                    ty,
                    pos,
                })
            }
            _ => unreachable!("only va_end and va_copy"),
        };
        e.kind = ExprKind::Cast { x, implicit: false };
        e.ty = Type::Void;
        Ok(())
    }

    /// Replaces a `sizeof` with its value, a `size_t`: `unsigned int` on ILP32,
    /// `unsigned long` on LP64.
    fn sizeof(&mut self, e: &mut Expr, t: &Type) -> Checked {
        let Some(size) = t.size(self.xlen) else {
            let what = if t.is_func() {
                "a function type"
            } else {
                "an incomplete type"
            };
            let msg = format!("invalid application of 'sizeof' to {what} '{t}'");
            return Err(self.err(e.pos, msg));
        };
        e.kind = ExprKind::Num(size.into());
        e.ty = Type::Int(IntKind::size_t(self.xlen));
        Ok(())
    }

    fn binary(&mut self, op: BinOp, l: &mut Expr, r: &mut Expr, pos: Pos) -> Result<Type, Reported> {
        let (lt, rt) = (l.ty.decay(), r.ty.decay());
        match op {
            BinOp::LogAnd | BinOp::LogOr => {
                for (x, t) in [(&*l, &lt), (&*r, &rt)] {
                    if !t.is_scalar() {
                        let msg = format!("invalid operand of type '{t}' to '{}'", op.symbol());
                        return Err(self.err(x.pos, msg));
                    }
                }
                Ok(Type::INT)
            }
            _ if op.is_comparison() => {
                let equality = matches!(op, BinOp::Eq | BinOp::Ne);
                let ok = match (&lt, &rt) {
                    (Type::Int(_), Type::Int(_)) => true,
                    // Qualifiers of what the pointers point to don't matter.
                    // Any pointer, to a function too, may be compared for
                    // equality with a null pointer constant, `(void *)0`
                    // (`NULL`) included (C99 6.5.9p2).
                    (Type::Ptr(a), Type::Ptr(b)) => {
                        a.unqual() == b.unqual()
                            || (equality && ((a.is_void() && !b.is_func()) || (b.is_void() && !a.is_func())))
                            || (equality
                                && (consteval::is_null_pointer(l, self.xlen)
                                    || consteval::is_null_pointer(r, self.xlen)))
                    }
                    // A pointer may be compared for equality with the null pointer constant.
                    (Type::Ptr(_), Type::Int(_)) => equality && consteval::is_null_pointer(r, self.xlen),
                    (Type::Int(_), Type::Ptr(_)) => equality && consteval::is_null_pointer(l, self.xlen),
                    _ => false,
                };
                if !ok {
                    let msg = format!("invalid comparison between '{lt}' and '{rt}'");
                    return Err(self.err(pos, msg));
                }
                Ok(Type::INT)
            }
            _ => arith_type(op, &lt, &rt, self.xlen).ok_or_else(|| self.invalid_operands(pos, &lt, &rt)),
        }
    }

    /// The type of `c ? a : b` (C99 6.5.15), converting both branches to it.
    fn cond_type(&mut self, a: &mut Expr, b: &mut Expr, pos: Pos) -> Result<Type, Reported> {
        let (at, bt) = (a.ty.decay(), b.ty.decay());
        let ty = match (&at, &bt) {
            (Type::Int(x), Type::Int(y)) => Type::Int(common(*x, *y, self.xlen)),
            (Type::Void, Type::Void) => Type::Void,
            (Type::Record(x), Type::Record(y)) if x == y => at.clone(),
            // A null pointer constant takes the other pointer's type, even
            // when it is `(void *)0` (C99 6.5.15p6).
            (Type::Ptr(_), Type::Ptr(_)) if consteval::is_null_pointer(a, self.xlen) => bt.clone(),
            (Type::Ptr(_), Type::Ptr(_)) if consteval::is_null_pointer(b, self.xlen) => at.clone(),
            // Otherwise both point to the same type (or one to void), and
            // the result points to it with every qualifier of either side.
            (Type::Ptr(x), Type::Ptr(y))
                if x.unqual() == y.unqual()
                    || (x.is_void() && !y.is_func())
                    || (y.is_void() && !x.is_func()) =>
            {
                let q = x.quals().union(y.quals());
                let to = if x.unqual() == y.unqual() {
                    x.unqual().clone()
                } else {
                    Type::Void
                };
                Type::ptr(to.qualified(q))
            }
            (Type::Ptr(_), Type::Int(_)) if consteval::is_null_pointer(b, self.xlen) => at.clone(),
            (Type::Int(_), Type::Ptr(_)) if consteval::is_null_pointer(a, self.xlen) => bt.clone(),
            _ => {
                let msg = format!("incompatible operand types ('{at}' and '{bt}')");
                return Err(self.err(pos, msg));
            }
        };
        coerce(a, &ty);
        coerce(b, &ty);
        Ok(ty)
    }
}

/// `char`, `signed char` or `unsigned char` (qualified or not): the element
/// types a string literal may initialize.
fn is_char_array_elem(t: &Type) -> bool {
    matches!(
        t.unqual(),
        Type::Int(IntKind::Char | IntKind::SChar | IntKind::UChar)
    )
}

fn is_char_array(t: &Type) -> bool {
    matches!(t, Type::Array(elem, _) if is_char_array_elem(elem))
}

#[cfg(test)]
mod tests {
    use crate::ast::*;
    use crate::types::Xlen;

    fn ok(src: &str) -> Program {
        crate::parse(src).unwrap_or_else(|e| panic!("{src}: {e:?}"))
    }

    fn errs(src: &str) -> Vec<String> {
        crate::parse(src)
            .unwrap_err()
            .iter()
            .map(|e| e.to_string())
            .collect()
    }

    fn err(src: &str) -> String {
        let e = errs(src);
        assert_eq!(e.len(), 1, "{e:?}");
        e[0].clone()
    }

    fn func<'a>(p: &'a Program, name: &str) -> &'a Function {
        p.funcs().find(|f| f.name == name && f.body.is_some()).unwrap()
    }

    fn var(v: &Option<VarRef>) -> String {
        match v.as_ref().unwrap() {
            VarRef::Local(LocalId(i)) => format!("#{i}"),
            VarRef::Global(s) => format!("@{s}"),
        }
    }

    /// `name#slot` (or `name@symbol`) for every variable use in the function, in
    /// source order.
    fn uses(f: &Function) -> Vec<String> {
        fn expr(e: &Expr, out: &mut Vec<String>) {
            match &e.kind {
                ExprKind::Num(_) | ExprKind::Str(_) | ExprKind::SizeofType(_) => {}
                ExprKind::Var { name, var: v } => out.push(format!("{name}{}", var(v))),
                ExprKind::Assign { target, value, .. } => {
                    expr(target, out);
                    expr(value, out);
                }
                ExprKind::Binary(_, a, b) | ExprKind::Comma(a, b) => {
                    expr(a, out);
                    expr(b, out);
                }
                ExprKind::Cond(c, a, b) => {
                    expr(c, out);
                    expr(a, out);
                    expr(b, out);
                }
                ExprKind::Unary(_, x)
                | ExprKind::Addr(x)
                | ExprKind::Deref(x)
                | ExprKind::Cast { x, .. }
                | ExprKind::SizeofExpr(x)
                | ExprKind::VaArg(x)
                | ExprKind::VaEnd(x)
                | ExprKind::IncDec { target: x, .. } => expr(x, out),
                ExprKind::Member { base, .. } => expr(base, out),
                ExprKind::Call { callee, args, .. } => {
                    if callee.direct_callee().is_none() {
                        expr(callee, out);
                    }
                    args.iter().for_each(|a| expr(a, out));
                }
                ExprKind::VaStart { ap: a, last: b } | ExprKind::VaCopy(a, b) => {
                    expr(a, out);
                    expr(b, out);
                }
                ExprKind::CompoundLit { .. } => {}
            }
        }
        fn stmt(s: &Stmt, out: &mut Vec<String>) {
            match &s.kind {
                StmtKind::Block(b) => b.iter().for_each(|s| stmt(s, out)),
                StmtKind::Decl(ds) => {
                    for d in ds {
                        out.push(format!("decl {}{}", d.name, var(&d.var)));
                        if let Some(Init::Expr(e)) = &d.init {
                            expr(e, out);
                        }
                    }
                }
                StmtKind::Expr(e) | StmtKind::Return(Some(e)) => expr(e, out),
                StmtKind::Empty | StmtKind::Break | StmtKind::Continue | StmtKind::Return(None) => {}
                StmtKind::If { cond, then, els } => {
                    expr(cond, out);
                    stmt(then, out);
                    if let Some(e) = els {
                        stmt(e, out);
                    }
                }
                StmtKind::While { cond, body } | StmtKind::DoWhile { body, cond } => {
                    expr(cond, out);
                    stmt(body, out);
                }
                StmtKind::For {
                    init,
                    cond,
                    step,
                    body,
                } => {
                    if let Some(i) = init {
                        stmt(i, out);
                    }
                    cond.iter().chain(step).for_each(|e| expr(e, out));
                    stmt(body, out);
                }
                StmtKind::Switch { cond, body, .. } => {
                    expr(cond, out);
                    stmt(body, out);
                }
                StmtKind::Case { body, .. }
                | StmtKind::Default { body, .. }
                | StmtKind::Label { body, .. } => stmt(body, out),
                StmtKind::Goto(_) => {}
            }
        }
        let mut out = Vec::new();
        f.body.as_ref().unwrap().iter().for_each(|s| stmt(s, &mut out));
        out
    }

    /// The type sema gives the expression `e` in a function with the given
    /// parameters, and the tree with implicit casts as an s-expression.
    fn typed(params: &str, e: &str) -> (String, String) {
        fn sx(e: &Expr) -> String {
            match &e.kind {
                ExprKind::Cast { x, implicit: true } => format!("({} {})", e.ty, sx(x)),
                ExprKind::Cast { x, implicit: false } => format!("(cast {} {})", e.ty, sx(x)),
                ExprKind::Num(v) => v.to_string(),
                ExprKind::Var { name, .. } => name.clone(),
                ExprKind::Binary(op, l, r) => format!("({} {} {})", op.symbol(), sx(l), sx(r)),
                ExprKind::Assign { target, value, op } => {
                    format!(
                        "({}= {} {})",
                        op.map_or("", |o| o.symbol()),
                        sx(target),
                        sx(value)
                    )
                }
                ExprKind::Cond(c, a, b) => format!("(? {} {} {})", sx(c), sx(a), sx(b)),
                ExprKind::Call { callee, args, .. } => {
                    let n = callee.direct_callee().map_or_else(|| sx(callee), str::to_string);
                    format!("({n} {})", args.iter().map(sx).collect::<Vec<_>>().join(" "))
                }
                ExprKind::Member {
                    base, name, arrow, ..
                } => format!("({} {} {name})", if *arrow { "->" } else { "." }, sx(base)),
                ExprKind::Deref(x) => format!("(* {})", sx(x)),
                ExprKind::Addr(x) => format!("(& {})", sx(x)),
                _ => format!("{:?}", e.kind),
            }
        }
        let p = ok(&format!(
            "int g; char buf[8]; int f(char c, int *q) {{ return 0; }} void v(void) {{}}\n\
             int t({params}) {{ {e}; return 0; }}"
        ));
        let f = func(&p, "t");
        let StmtKind::Expr(e) = &f.body.as_ref().unwrap()[0].kind else {
            panic!()
        };
        (e.ty.to_string(), sx(e))
    }

    fn ty(params: &str, e: &str) -> String {
        typed(params, e).0
    }

    #[test]
    fn params_then_locals_in_source_order() {
        let p = ok("int f(int a, char *b) { int c = a; { long d[2]; } return c; }");
        let f = func(&p, "f");
        let names: Vec<String> = f.locals.iter().map(|l| l.ty.declare(&l.name)).collect();
        assert_eq!(names, ["int a", "char *b", "int c", "long d[2]"]);
        assert_eq!(uses(f), ["decl c#2", "a#0", "decl d#3", "c#2"]);
    }

    #[test]
    fn shadowing_gets_its_own_slot() {
        let p = ok("int f(int x) { int y = x; { int x = 2; y = x; } if (1) { int y = 3; } return x + y; }");
        assert_eq!(
            uses(func(&p, "f")),
            ["decl y#1", "x#0", "decl x#2", "y#1", "x#2", "decl y#3", "x#0", "y#1"]
        );
    }

    #[test]
    fn for_scope() {
        let p = ok("int f() { int i = 5; for (int i = 0; i < 3; i = i + 1) { int j = i; } return i; }");
        assert_eq!(
            uses(func(&p, "f")),
            ["decl i#0", "decl i#1", "i#1", "i#1", "i#1", "decl j#2", "i#1", "i#0"]
        );
        assert_eq!(
            err("int f() { for (int i = 0; i < 3; i = i + 1) {} return i; }"),
            "1:55: use of undeclared identifier 'i'"
        );
        // The loop body is its own scope, so it may redeclare the loop variable.
        ok("int f() { for (int i = 0; i < 3; i = i + 1) { int i = 7; } return 0; }");
    }

    #[test]
    fn initializer_sees_its_own_name() {
        // As in C: the scope of `x` starts at its declarator.
        let p = ok("int f(int x) { { int x = x + 1; return x; } }");
        assert_eq!(uses(func(&p, "f")), ["decl x#1", "x#1", "x#1"]);
        // So does a static local's, not an outer `q`.
        let p = ok("void *q; int f() { static void *q = &q; return q == &q; }");
        assert_eq!(uses(func(&p, "f")), ["decl q@f.q", "q@f.q", "q@f.q", "q@f.q"]);
        let p = ok("int f() { static void *head[2] = {head, head}; return head[0] == head; }");
        assert_eq!(uses(func(&p, "f"))[..2], ["decl head@f.head", "head@f.head"]);
        // `sizeof x` is the static `int`'s size, not the outer `char`'s.
        let p = ok("char x; int f(void) { static int x = sizeof x; return x; }");
        let StmtKind::Decl(ds) = &func(&p, "f").body.as_ref().unwrap()[0].kind else {
            panic!()
        };
        let Some(Init::Expr(e)) = &ds[0].init else {
            panic!()
        };
        assert_eq!(crate::consteval::int(e, Xlen::Rv32), Some(4));
        // An array the initializer completes is incomplete inside it, as in C,
        // and complete after it.
        assert_eq!(
            err("int f() { static int a[] = {1, 2, sizeof a}; return 0; }"),
            "1:35: invalid application of 'sizeof' to an incomplete type 'int []'"
        );
        let p = ok("int f() { static int a[] = {1, 2, 3}; return sizeof a; }");
        let StmtKind::Return(Some(e)) = &func(&p, "f").body.as_ref().unwrap()[1].kind else {
            panic!()
        };
        assert_eq!(crate::consteval::int(e, Xlen::Rv32), Some(12));
        // And a global's.
        ok("void *self = &self; int f() { return self == &self; }");
        ok("int *ring[2] = {(int *)&ring[1], (int *)&ring[0]};");
    }

    #[test]
    fn globals_and_statics() {
        let p = ok(
            "int g = 1; int f(int g2) { static int n; static int m = 2; { static int n = 3; n++; } \
                    return g + n + m + g2; }",
        );
        assert_eq!(
            uses(func(&p, "f")),
            [
                "decl n@f.n",
                "decl m@f.m",
                "decl n@f.n.1",
                "n@f.n.1",
                "g@g",
                "n@f.n",
                "m@f.m",
                "g2#0"
            ]
        );
        // Statics are not locals.
        assert_eq!(func(&p, "f").locals.len(), 1);
        // A local shadows a global.
        let p = ok("int g; int f() { int g = 2; return g; }");
        assert_eq!(uses(func(&p, "f")), ["decl g#0", "g#0"]);
        // Globals are visible from their declaration on.
        assert_eq!(
            err("int f() { return g; } int g;"),
            "1:18: use of undeclared identifier 'g'"
        );
    }

    #[test]
    fn global_definitions() {
        let defined = |src: &str| -> Vec<String> {
            ok(src)
                .items
                .iter()
                .filter_map(|i| match i {
                    Item::Decl(ds) => Some(ds),
                    _ => None,
                })
                .flatten()
                .map(|d| {
                    let storage = match d.storage {
                        Storage::None => "",
                        Storage::Static => "static ",
                        Storage::Extern => "extern ",
                    };
                    let def = if d.define { " (def)" } else { "" };
                    format!("{storage}{}{def}", d.ty.declare(&d.name))
                })
                .collect()
        };
        assert_eq!(
            defined("int a; int a; int a = 1; int a;"),
            ["int a", "int a", "int a (def)", "int a"]
        );
        assert_eq!(defined("int a; int a;"), ["int a (def)", "int a"]);
        assert_eq!(defined("extern int a; int a;"), ["extern int a", "int a (def)"]);
        assert_eq!(defined("extern int a;"), ["extern int a"]);
        // A later declaration completes the array; the definition gets the size.
        assert_eq!(defined("int a[]; int a[3];"), ["int a[3] (def)", "int a[3]"]);
        assert_eq!(defined("int a[] = {1, 2};"), ["int a[2] (def)"]);
        // An earlier size completes a later declaration (C's composite type).
        assert_eq!(
            defined("int a[3]; int a[] = {1};"),
            ["int a[3]", "int a[3] (def)"]
        );
        assert_eq!(
            defined("int a[] = {1, 2}; extern int a[];"),
            ["int a[2] (def)", "extern int a[2]"]
        );
        assert_eq!(
            err("int a[1]; int a[] = {1, 2};"),
            "1:25: excess elements in array initializer"
        );
        assert_eq!(
            defined("char s[] = \"abc\", t[2] = \"ab\";"),
            ["char s[4] (def)", "char t[2] (def)"]
        );
        assert_eq!(
            defined("static int s; extern int s;"),
            ["static int s (def)", "extern int s"]
        );
        assert_eq!(err("int a = 1; int a = 2;"), "1:16: redefinition of 'a'");
        assert_eq!(
            err("int a; char a;"),
            "1:13: redefinition of 'a' with a different type: 'char' vs 'int' (declared at 1:5)"
        );
        assert_eq!(
            err("int a[];"),
            "1:5: definition of 'a' needs the array size ('int []')"
        );
        assert_eq!(
            err("int a; static int a;"),
            "1:19: static declaration of 'a' follows non-static declaration"
        );
        assert_eq!(
            err("static int a; int a;"),
            "1:19: non-static declaration of 'a' follows static declaration"
        );
        assert_eq!(
            err("int f(); int f;"),
            "1:14: redefinition of 'f' as a different kind of symbol"
        );
        assert_eq!(
            err("int f; int f();"),
            "1:12: redefinition of 'f' as a different kind of symbol"
        );
        assert_eq!(err("void v;"), "1:6: variable 'v' has incomplete type 'void'");
        assert_eq!(
            err("extern int e = 1;"),
            "1:12: 'extern' declaration of 'e' cannot have an initializer"
        );
    }

    #[test]
    fn constant_initializers() {
        ok(
            "int a = 1 + 2 * 3; int b = sizeof(int) << 2; int *p = &a; int arr[4]; int *q = arr + 2;\n\
            int *r = &arr[3]; char *s = \"hi\"; char *n = 0; void *v = (void *)0; int *w = (int *)0x1000;\n\
            unsigned char u = -1; int m[2][2]; int *mp = m[1];",
        );
        assert_eq!(
            err("int a; int b = a;"),
            "1:16: initializer element is not a compile-time constant"
        );
        assert_eq!(
            err("int f(); int b = f();"),
            "1:18: initializer element is not a compile-time constant"
        );
        assert_eq!(
            err("int f() { static int s = f(); return s; }"),
            "1:26: initializer element is not a compile-time constant"
        );
        assert_eq!(
            err("int a; int b = (int)&a;"),
            "1:16: initializer element is not a compile-time constant"
        );
        // Locals may be initialized with anything.
        ok("int f(int x) { int y = x * 2; int a[2] = {x, f(y)}; return a[1]; }");
    }

    #[test]
    fn array_initializers() {
        ok(
            "int a[3] = {1, 2}; int m[2][3] = {{1, 2, 3}, {4}}; char s[3] = {'a', 98, 0}; \
            char t[] = {\"x\"}; int one = {1}; unsigned char u[2] = \"ab\"; signed char v[] = \"q\";",
        );
        assert_eq!(
            err("int a[2] = {1, 2, 3};"),
            "1:19: excess elements in array initializer"
        );
        assert_eq!(
            err("char s[2] = \"abc\";"),
            "1:13: initializer-string for char array is too long"
        );
        assert_eq!(
            err("int a[2] = 5;"),
            "1:12: array initializer must be an initializer list"
        );
        assert_eq!(
            err("int a[2] = \"ab\";"),
            "1:12: array initializer must be an initializer list"
        );
        // Brace elision: the items spread over the inner arrays, and sema
        // writes the braces out.
        let braced = |src: &str| {
            let p = ok(src);
            let Item::Decl(d) = &p.items[0] else { panic!() };
            fn show(i: &Init) -> String {
                match i {
                    Init::List(items, _) => {
                        format!("{{{}}}", items.iter().map(show).collect::<Vec<_>>().join(", "))
                    }
                    Init::Expr(e) => match crate::consteval::int(e, Xlen::Rv32) {
                        Some(v) => v.to_string(),
                        None => "s".into(),
                    },
                    Init::Designated(..) => panic!("a designator survived sema"),
                }
            }
            show(d[0].init.as_ref().unwrap())
        };
        assert_eq!(braced("char a[2][2] = {\"ab\", 1};"), "{s, {1}}");
        assert_eq!(braced("int m[2][2] = {1, 2, 3, 4};"), "{{1, 2}, {3, 4}}");
        assert_eq!(braced("int m[][2] = {1, 2, 3};"), "{{1, 2}, {3}}");
        assert_eq!(braced("int m[2][3] = {{1}, 2};"), "{{1}, {2}}");
        // Designators: gaps are empty lists, and filling goes on after the
        // designated subobject, at its level.
        assert_eq!(braced("int a[] = {[3] = 1, 2, [1] = 5};"), "{{}, 5, {}, 1, 2}");
        assert_eq!(
            braced("int m[2][2] = {[1] = 1, 2, [0][1] = 3};"),
            "{{{}, 3}, {1, 2}}"
        );
        assert_eq!(braced("int m[2][2] = {[0][1] = 3, 4, 5};"), "{{{}, 3}, {4, 5}}");
        assert_eq!(
            braced("struct p { int x, y; } a[2] = {[1].y = 2, [0] = {1, 9}, [0].y = 3};"),
            "{{1, 3}, {{}, 2}}"
        );
        assert_eq!(
            braced("struct p { int x, y, z; } v = {.y = 2, 3, .x = 1};"),
            "{1, 2, 3}"
        );
        // A union keeps its last member.
        assert_eq!(braced("union u { char c; int i; } v = {.i = 7};"), "{{}, 7}");
        assert_eq!(braced("union u { char c; int i; } v = {.i = 7, .c = 1};"), "{1}");
        // A designation into a string keeps its other characters.
        assert_eq!(
            braced("char s[2][4] = {\"ab\", [0][3] = 'z', [1] = \"c\", [1][0] = 'd'};"),
            "{{97, 98, {}, 122}, {100}}"
        );
        // Into a struct given by an expression: C keeps the other members,
        // which merlin cannot do, so it is an error.
        assert_eq!(
            err("struct p { int x, y; }; struct r { struct p p; int z; }; \
                 int f(struct p v) { struct r r = {v, .p.y = 9}; return r.p.x; }"),
            "1:97: designator partially overrides an initializer of type 'struct p' given by an \
             expression; merlin does not support this"
        );
        assert_eq!(
            err("int a[2] = {[2] = 1};"),
            "1:14: array designator index (2) exceeds array bounds (2)"
        );
        assert_eq!(
            err("int a[2] = {[2] = 1};"),
            "1:14: array designator index (2) exceeds array bounds (2)"
        );
        assert_eq!(
            err("int a[2] = {[-1] = 1};"),
            "1:14: array designator value '-1' is negative"
        );
        assert_eq!(
            err("int n; int a[2] = {[n] = 1};"),
            "1:21: expression is not an integer constant expression"
        );
        assert_eq!(
            err("int a[] = {[1 << 20] = 1};"),
            "1:15: array designator index (1048576) is too large: merlin supports up to 1048575"
        );
        assert_eq!(
            err("struct p { int x; } v = {.y = 1};"),
            "1:26: field designator 'y' does not refer to any field in type 'struct p'"
        );
        assert_eq!(
            err("struct p { int x; } v = {[0] = 1};"),
            "1:26: array designator cannot initialize non-array type 'struct p'"
        );
        assert_eq!(
            err("int a[2] = {.x = 1};"),
            "1:13: field designator cannot initialize a non-struct, non-union type 'int [2]'"
        );
        assert_eq!(
            err("int x = {.x = 1};"),
            "1:10: field designator cannot initialize a non-struct, non-union type 'int'"
        );
        assert_eq!(
            err("struct p { int x; } v = {.x.y = 1};"),
            "1:28: field designator cannot initialize a non-struct, non-union type 'int'"
        );
        assert_eq!(
            err("union u { char c; int i; } v = {.i = 7, 8};"),
            "1:41: excess elements in union initializer"
        );
        assert_eq!(
            err("int m[2][2] = {1, 2, 3, 4, 5};"),
            "1:28: excess elements in array initializer"
        );
        assert_eq!(
            err("int x = {1, 2};"),
            "1:9: excess elements in scalar initializer"
        );
        assert_eq!(
            err("int x = {{1}};"),
            "1:9: too many braces around scalar initializer"
        );
        assert_eq!(
            err("int f() { int a[]; return 0; }"),
            "1:15: variable 'a' has incomplete type 'int []'"
        );
        let p = ok("int f() { int a[] = {1, 2, 3}; return sizeof a; }");
        assert_eq!(func(&p, "f").locals[0].ty.to_string(), "int [3]");
    }

    #[test]
    fn expression_types() {
        assert_eq!(ty("char c", "c"), "char");
        assert_eq!(ty("char c", "c + c"), "int");
        assert_eq!(ty("char c", "-c"), "int");
        assert_eq!(ty("char c", "~c"), "int");
        assert_eq!(ty("unsigned u, int i", "u + i"), "unsigned int");
        assert_eq!(ty("unsigned u, long l", "u * l"), "unsigned long");
        assert_eq!(ty("unsigned short u, int i", "u + i"), "int");
        assert_eq!(ty("unsigned u, int i", "u < i"), "int");
        assert_eq!(
            ty("char c, unsigned u", "c << u"),
            "int",
            "shifts take the left type"
        );
        assert_eq!(ty("unsigned u", "u >> 1"), "unsigned int");
        assert_eq!(ty("int *p", "p + 1"), "int *");
        assert_eq!(ty("int *p", "1 + p"), "int *");
        assert_eq!(ty("int *p, int *q", "p - q"), "int");
        assert_eq!(ty("int *p", "*p"), "int");
        assert_eq!(ty("int *p", "&p"), "int **");
        assert_eq!(ty("int *p", "p[2]"), "int");
        assert_eq!(ty("int x", "buf"), "char [8]");
        assert_eq!(ty("int x", "buf + 1"), "char *");
        assert_eq!(ty("int x", "&buf"), "char (*)[8]");
        assert_eq!(ty("int x", "*buf"), "char");
        assert_eq!(ty("int x", "sizeof buf"), "unsigned int");
        // A null pointer constant takes the other side's type, even as `(void *)0`.
        assert_eq!(ty("int *p, int c", "c ? (void *)0 : p"), "int *");
        assert_eq!(ty("int *p, int c", "c ? p : (void *)0"), "int *");
        assert_eq!(ty("int *p, int c", "c ? 0 : p"), "int *");
        assert_eq!(ty("int *p, int c", "(c ? (void *)0 : p) + 1"), "int *");
        assert_eq!(ty("int *p, int c", "*(c ? p : (void *)0)"), "int");
        assert_eq!(ty("int *p, void *v, int c", "c ? v : p"), "void *");
        assert_eq!(ty("int c", "c ? (void *)0 : (void *)0"), "void *");
        assert_eq!(ty("int x", "\"abc\""), "char [4]");
        assert_eq!(ty("int x", "\"abc\"[1]"), "char");
        assert_eq!(ty("char c", "c = 300"), "char");
        assert_eq!(ty("char c", "c += 300"), "char");
        assert_eq!(ty("int *p", "p++"), "int *");
        assert_eq!(ty("int *p", "!p"), "int");
        assert_eq!(ty("int *p", "p && 1"), "int");
        assert_eq!(ty("int x", "x ? 1 : 2u"), "unsigned int");
        assert_eq!(ty("int *p, int x", "x ? p : 0"), "int *");
        assert_eq!(ty("void *v, int *p", "1 ? v : p"), "void *");
        assert_eq!(ty("int x", "1 ? v() : v()"), "void");
        assert_eq!(ty("int x", "(x, buf)"), "char *");
        assert_eq!(ty("int x", "v()"), "void");
        assert_eq!(ty("int x", "(char)x"), "char");
        assert_eq!(ty("int x", "(void)x"), "void");
    }

    #[test]
    fn sizeof_is_a_constant() {
        let p = ok(
            "int f(int *p) { char a[3][5]; return sizeof(a) + sizeof a[0] + sizeof(p) + sizeof(short) \
                    + sizeof *p + sizeof(p++); }",
        );
        let StmtKind::Return(Some(e)) = &func(&p, "f").body.as_ref().unwrap()[1].kind else {
            panic!()
        };
        // 15 + 5 + 4 + 2 + 4 + 4; `p++` is not evaluated.
        assert_eq!(crate::consteval::int(e, Xlen::Rv32), Some(34));
        assert_eq!(
            err("int f() { return sizeof(void); }"),
            "1:18: invalid application of 'sizeof' to an incomplete type 'void'"
        );
    }

    #[test]
    fn implicit_conversions_are_explicit() {
        assert_eq!(typed("char c", "c = 300").1, "(= c (char 300))");
        assert_eq!(typed("char c, int i", "i = c").1, "(= i (int c))");
        assert_eq!(typed("int *p", "p = 0").1, "(= p (int * 0))");
        assert_eq!(typed("char *s", "s = buf").1, "(= s (char * buf))");
        assert_eq!(typed("int x", "f(x, 0)").1, "(f (char x) (int * 0))");
        assert_eq!(typed("int x", "x ? 1 : 2u").1, "(? x (unsigned int 1) 2)");
        // No cast when nothing changes; compound assignment keeps its operands.
        assert_eq!(typed("int x", "x = 1").1, "(= x 1)");
        assert_eq!(typed("char c", "c += 1").1, "(+= c 1)");
    }

    /// The single error for `src`, where `@` marks the column it must point at
    /// (the `@` itself is removed before compiling).
    fn err_at(src: &str, msg: &str) {
        let col = src.find('@').expect("mark the error column with @") + 1;
        assert_eq!(err(&src.replace('@', "")), format!("1:{col}: {msg}"), "{src}");
    }

    #[test]
    fn type_errors() {
        let e = |params: &str, x: &str, msg: &str| {
            err_at(&format!("void v(void); int t({params}) {{ return {x}; }}"), msg)
        };
        e(
            "int *p",
            "p @+ p",
            "invalid operands to binary expression ('int *' and 'int *')",
        );
        e(
            "int *p",
            "p @* 2",
            "invalid operands to binary expression ('int *' and 'int')",
        );
        e(
            "int *p",
            "2 @- p",
            "invalid operands to binary expression ('int' and 'int *')",
        );
        e(
            "int *p",
            "p @% 2",
            "invalid operands to binary expression ('int *' and 'int')",
        );
        e("void *p", "p @+ 1", "arithmetic on a pointer to void");
        e("void *p", "@*p", "dereferencing a 'void *' pointer");
        e(
            "int *p, char *q",
            "p @- q",
            "invalid operands to binary expression ('int *' and 'char *')",
        );
        e(
            "int *p, char *q",
            "p @== q",
            "invalid comparison between 'int *' and 'char *'",
        );
        e("int *p", "p @< 0", "invalid comparison between 'int *' and 'int'");
        e(
            "int *p",
            "p @== 1",
            "invalid comparison between 'int *' and 'int'",
        );
        e(
            "int x",
            "@*x",
            "indirection requires pointer operand ('int' invalid)",
        );
        e("int x", "x@[1]", "subscripted value is not an array or pointer");
        e(
            "int x",
            "@&1",
            "cannot take the address of an rvalue of type 'int'",
        );
        e(
            "int x",
            "@&(x + 1)",
            "cannot take the address of an rvalue of type 'int'",
        );
        e(
            "int *p",
            "@-p",
            "invalid argument type 'int *' to unary expression '-'",
        );
        e(
            "int *p",
            "@~p",
            "invalid argument type 'int *' to unary expression '~'",
        );
        e(
            "int x",
            "v() @+ 1",
            "invalid operands to binary expression ('void' and 'int')",
        );
        e(
            "int x",
            "@!v()",
            "invalid argument type 'void' to unary expression '!'",
        );
        e("int x", "@v() && 1", "invalid operand of type 'void' to '&&'");
        e(
            "int x",
            "x @? v() : 1",
            "incompatible operand types ('void' and 'int')",
        );
        e(
            "int *p, char *q",
            "1 @? p : q",
            "incompatible operand types ('int *' and 'char *')",
        );
        e(
            "int *p",
            "(char)p + @(int[2])p",
            "cannot cast from 'int *' to 'int [2]'",
        );
        e("int x", "@(int)v()", "cannot cast from 'void' to 'int'");
        e("void *p", "p@++, 0", "cannot increment value of type 'void *'");
    }

    #[test]
    fn conversion_errors() {
        let e = |body: &str, msg: &str| err_at(&format!("int g; char *s; int f(int *p) {{ {body} }}"), msg);
        e(
            "p = @s; return 0;",
            "incompatible pointer types assigning to 'int *' from 'char *'",
        );
        e(
            "p = @1; return 0;",
            "incompatible integer to pointer conversion assigning to 'int *' from 'int'",
        );
        e(
            "g = @p; return 0;",
            "incompatible pointer to integer conversion assigning to 'int' from 'int *'",
        );
        e(
            "int *q = @s; return 0;",
            "incompatible pointer types initializing 'int *' with an expression of type 'char *'",
        );
        // `(void *)0` in a `?:` is a null pointer constant, not a `void *`.
        e(
            "s = 1 @? (void *)0 : p; return 0;",
            "incompatible pointer types assigning to 'char *' from 'int *'",
        );
        e(
            "return f(@s);",
            "incompatible pointer types passing 'char *' to parameter of type 'int *'",
        );
        e(
            "return @p;",
            "incompatible pointer to integer conversion returning 'int *' from a function with result type 'int'",
        );
        e(
            "int a[2]; int b[2]; @a = b; return 0;",
            "array type 'int [2]' is not assignable",
        );
        e("g @+ 1 = 2; return 0;", "expression is not assignable");
        e(
            "g @+= p; return 0;",
            "invalid operands to binary expression ('int' and 'int *')",
        );
        e(
            "p @-= p; return 0;",
            "invalid operands to binary expression ('int *' and 'int *')",
        );
        e(
            "p @*= 2; return 0;",
            "invalid operands to binary expression ('int *' and 'int')",
        );
        e("(g @+ 1)++; return 0;", "expression is not assignable");
        e("@\"ab\" = 0; return 0;", "expression is not assignable");
        // Conversions C allows.
        ok("void *malloc(unsigned long n); int f(int *p, void *v, char c) { \
            p = v; v = p; p = 0; p = (int *)0; v = (void *)0; c = 1000; p = malloc(4); p += 1; p -= 2; \
            return p == 0 || 0 != p || p == v || v != p; }");
        // A function pointer against NULL, `(void *)0`: allowed for == and
        // != only; against any other `void *` it is not.
        ok("void g(void); int f(void (*fp)(void)) {             return fp != (void *)0 || (void *)0 == fp || fp == 0 || g != (void *)0; }");
        e(
            "void (*fp)(void) = 0; void *v = 0; return fp @== v;",
            "invalid comparison between 'void (*)(void)' and 'void *'",
        );
        e(
            "void (*fp)(void) = 0; return fp @< (void *)0;",
            "invalid comparison between 'void (*)(void)' and 'void *'",
        );
    }

    #[test]
    fn returns() {
        assert_eq!(
            err("void f() { return 1; }"),
            "1:19: void function 'f' should not return a value"
        );
        assert_eq!(
            err("int f() { return; }"),
            "1:11: non-void function 'f' should return a value"
        );
        ok("void f() { return; } void g(int x) { if (x) return; f(); }");
        assert_eq!(
            err("char *f() { return 1; }"),
            "1:20: incompatible integer to pointer conversion returning 'int' from a function with result type 'char *'"
        );
        assert_eq!(
            err("void f(void x);"),
            "1:13: parameter has incomplete type 'void'"
        );
        // A typedef for void spells `(void)` too, but only unqualified.
        ok("typedef void V; int f(V); int main(V) { return f(); }");
        assert_eq!(
            err("typedef void V; void f(const V);"),
            "1:24: parameter has incomplete type 'void'"
        );
        // `int a[const 3]` makes the pointer const, not the elements.
        ok("int f(int a[const 3]) { a[0] = 1; return a[1]; }");
        assert_eq!(
            err("int f(int a[const 3]) { a = 0; return 0; }"),
            "1:25: cannot assign to variable 'a' with const-qualified type 'int *const'"
        );
    }

    #[test]
    fn conditions_must_be_scalar() {
        assert_eq!(
            err("void v(); int f() { if (v()) return 1; return 0; }"),
            "1:25: statement requires expression of scalar type ('void' invalid)"
        );
        assert_eq!(
            err("void v(); int f() { while (v()) ; return 0; }"),
            "1:28: statement requires expression of scalar type ('void' invalid)"
        );
        // Pointers and arrays are fine.
        ok("char b[2]; int f(int *p) { if (p) return 1; while (b) break; do ; while (p); for (; p; ) break; \
            return !p; }");
    }

    #[test]
    fn undeclared_identifiers() {
        assert_eq!(
            err("int f() { return y; }"),
            "1:18: use of undeclared identifier 'y'"
        );
        assert_eq!(
            err("int f() { y = 1; return 0; }"),
            "1:11: use of undeclared identifier 'y'"
        );
        assert_eq!(
            err("int f() { { int t = 1; } return t; }"),
            "1:33: use of undeclared identifier 't'"
        );
        assert_eq!(
            err("int f() { return 0; } int g() { return f; }"),
            "1:40: incompatible pointer to integer conversion returning 'int (*)(void)' from a function with result type 'int'"
        );
    }

    #[test]
    fn calls() {
        ok("int f(int n) { return f(n); }");
        ok("int g(int a, int b); int f() { return g(1, 2); }");
        ok(
            "int odd(int n); int even(int n) { return n == 0 || odd(n - 1); } \
            int odd(int n) { return n != 0 && even(n - 1); }",
        );
        assert_eq!(
            err("int f() { return g(); }"),
            "1:18: call to undeclared function 'g'"
        );
        // Declared after the use: still undeclared at the call, as in C99.
        assert_eq!(
            err("int f() { return g(); } int g() { return 1; }"),
            "1:18: call to undeclared function 'g'"
        );
        assert_eq!(
            err("int f(int x) { return x(1); }"),
            "1:23: called object type 'int' is not a function or function pointer"
        );
        assert_eq!(
            err("int x; int f() { return x(1); }"),
            "1:25: called object type 'int' is not a function or function pointer"
        );
    }

    #[test]
    fn wrong_argument_count() {
        assert_eq!(
            err("int add(int a, int b) { return a + b; }\nint f() { return add(1, 2, 3); }"),
            "2:18: too many arguments to 'add': expected 2, have 3 (declared at 1:5)"
        );
        assert_eq!(
            err("int add(int a, int b) { return a + b; }\nint f() { return add(1); }"),
            "2:18: too few arguments to 'add': expected 2, have 1 (declared at 1:5)"
        );
        assert_eq!(
            err("int z(void);\nint f() { return z(0); }"),
            "2:18: too many arguments to 'z': expected 0, have 1 (declared at 1:5)"
        );
    }

    #[test]
    fn redefinitions() {
        assert_eq!(
            err("int f() { int a; int a; return 0; }"),
            "1:22: redefinition of 'a' (first declared at 1:15)"
        );
        assert_eq!(
            err("int f(int a, int a) { return a; }"),
            "1:18: redefinition of parameter 'a' (first declared at 1:11)"
        );
        // The body's top level shares the parameters' scope.
        assert_eq!(
            err("int f(int a) { int a = 1; return a; }"),
            "1:20: redefinition of 'a' (first declared at 1:11)"
        );
        assert_eq!(
            err("int f() { return 0; }\nint f() { return 1; }"),
            "2:5: redefinition of 'f'"
        );
        // A prototype first: the first definition must still count.
        assert_eq!(
            err("int f();\nint f() { return 0; }\nint f() { return 1; }"),
            "3:5: redefinition of 'f'"
        );
        assert_eq!(
            err("int f(int a);\nint f(int a, int b) { return a; }"),
            "2:5: conflicting types for 'f': declared at 1:5 with 1 parameter, here with 2"
        );
        assert_eq!(
            err("int f(int a);\nchar f(int a);"),
            "2:6: conflicting types for 'f': declared at 1:5 as 'int (int)', here as 'char (int)'"
        );
        assert_eq!(
            err("int f(int *a);\nint f(char *a);"),
            "2:5: conflicting types for 'f': declared at 1:5 as 'int (int *)', here as 'int (char *)'"
        );
        assert_eq!(
            err("int f();\nstatic int f();"),
            "2:12: static declaration of 'f' follows non-static declaration"
        );
        // Repeating a matching prototype is fine, before or after the definition,
        // and arrays in parameters are pointers.
        ok("int f(int a); int f(int b); int f(int c) { return c; } int f(int d);");
        ok("int f(int a[]); int f(int *a) { return 0; } int f(int a[5]);");
        // A function keeps `static` from its first declaration.
        let p = ok("static int f(); int f() { return 0; }");
        assert!(func(&p, "f").is_static);
    }

    #[test]
    fn break_and_continue_need_a_loop() {
        assert_eq!(
            err("int f() { break; }"),
            "1:11: 'break' statement not in a loop or switch"
        );
        assert_eq!(
            err("int f() { if (1) { continue; } return 0; }"),
            "1:20: 'continue' statement not in a loop"
        );
        ok(
            "int f() { while (1) { if (1) break; else continue; } for (;;) { { break; } } \
            do { continue; } while (0); return 0; }",
        );
        // A loop's scope ends with its body.
        assert_eq!(
            err("int g() { while (1) {} break; }"),
            "1:24: 'break' statement not in a loop or switch"
        );
    }

    #[test]
    fn assignment_target_must_be_assignable() {
        assert_eq!(
            err("int f(int a) { 1 = a; return 0; }"),
            "1:16: expression is not assignable"
        );
        assert_eq!(
            err("int f(int a) { (a + 1) = 2; return 0; }"),
            "1:19: expression is not assignable"
        );
        assert_eq!(
            err("int f(int a) { f(a) = 2; return 0; }"),
            "1:16: expression is not assignable"
        );
        assert_eq!(
            err("int f(int a) { ++1; return 0; }"),
            "1:18: expression is not assignable"
        );
        ok("int f(int a, int *p) { (a) = 2; *p = 3; p[1] = 4; *(p + 2) += 1; ++*p; p[0]--; return a; }");
    }

    #[test]
    fn errors_do_not_cascade() {
        // One error per broken expression, however far it spreads.
        assert_eq!(
            errs("int f() { return *(x + 1) + y[2] * -z; }"),
            [
                "1:20: use of undeclared identifier 'x'",
                "1:29: use of undeclared identifier 'y'",
                "1:37: use of undeclared identifier 'z'",
            ]
        );
        // A global whose initializer is wrong is still declared.
        assert_eq!(
            errs("int m[2] = {1, 2, 3}; char s[] = 5; int f() { return m[0] + s[0]; }"),
            [
                "1:19: excess elements in array initializer",
                "1:34: array initializer must be an initializer list or a string literal",
            ]
        );
    }

    #[test]
    fn reports_every_error_in_order() {
        assert_eq!(
            errs("int f() {\n  x = 1;\n  break;\n  return g();\n}\nint h() { return y; }"),
            [
                "2:3: use of undeclared identifier 'x'",
                "3:3: 'break' statement not in a loop or switch",
                "4:10: call to undeclared function 'g'",
                "6:18: use of undeclared identifier 'y'",
            ]
        );
    }

    const P: &str = "struct p { int x; char c; };";

    #[test]
    fn members() {
        let p = ok(&format!(
            "{P} int f(struct p *q) {{ struct p a = *q; a.x = q->c; return a.x + (*q).x; }}"
        ));
        let StmtKind::Expr(e) = &func(&p, "f").body.as_ref().unwrap()[1].kind else {
            panic!()
        };
        let ExprKind::Assign { target, value, .. } = &e.kind else {
            panic!()
        };
        assert!(matches!(target.kind, ExprKind::Member { offset: 0, .. }));
        let ExprKind::Cast { x, .. } = &value.kind else {
            panic!()
        };
        assert!(matches!(
            x.kind,
            ExprKind::Member {
                offset: 4,
                arrow: true,
                ..
            }
        ));
        assert_eq!(
            err(&format!("{P} int f(struct p a) {{ return a.z; }}")),
            "1:58: no member named 'z' in 'struct p'"
        );
        assert_eq!(
            err("int f(int a) { return a.x; }"),
            "1:24: member reference base type 'int' is not a structure or union"
        );
        assert_eq!(
            err(&format!("{P} int f(struct p a) {{ return a->x; }}")),
            "1:58: member reference type 'struct p' is not a pointer"
        );
        assert_eq!(
            err("struct p; int f(struct p *a) { return a->x; }"),
            "1:40: incomplete definition of type 'struct p'"
        );
        // A member of a const struct is const.
        assert_eq!(
            err(&format!("{P} int f(const struct p *a) {{ a->x = 1; return 0; }}")),
            "1:58: cannot assign to member 'x' with const-qualified type 'const int'"
        );
    }

    #[test]
    fn struct_values() {
        ok(&format!(
            "{P} struct p g(struct p a) {{ struct p b; b = a; return b; }} \
             int f(struct p a, int c) {{ return (c ? a : g(a)).x + g(a).c; }}"
        ));
        assert_eq!(
            err("struct p { int x; }; struct q { int x; }; int f(struct p a, struct q b) { a = b; return 0; }"),
            "1:79: incompatible types assigning to 'struct p' from 'struct q'"
        );
        assert_eq!(
            err("struct p { int x; }; int f(struct p a) { return a; }"),
            "1:49: incompatible types returning 'struct p' from a function with result type 'int'"
        );
        assert_eq!(
            err("struct p { int x; }; int f(struct p a) { if (a) return 1; return 0; }"),
            "1:46: statement requires expression of scalar type ('struct p' invalid)"
        );
        assert_eq!(
            err("struct p { int x; }; int f(struct p a) { return a + 1; }"),
            "1:51: invalid operands to binary expression ('struct p' and 'int')"
        );
        assert_eq!(
            err("struct p { int x; }; int f(void) { struct p a; return a == a; }"),
            "1:57: invalid comparison between 'struct p' and 'struct p'"
        );
        assert_eq!(
            err("struct p { const int x; int y; }; int f(struct p a, struct p b) { a.y = 1; a = b; return 0; }"),
            "1:76: cannot assign to an object of type 'struct p': it has a const-qualified member"
        );
        // Complete types are needed where the size matters.
        assert_eq!(
            err("struct p; struct p g; int f() { return 0; }"),
            "1:20: variable 'g' has incomplete type 'struct p'"
        );
        assert_eq!(
            err("struct p; int f(struct p a) { return 0; }"),
            "1:26: variable has incomplete type 'struct p'"
        );
        assert_eq!(
            err("struct p; struct p f(void) { }"),
            "1:20: incomplete result type 'struct p' in function definition"
        );
        assert_eq!(
            err("struct p; struct p f(void); int g() { f(); return 0; }"),
            "1:39: calling a function with incomplete result type 'struct p'"
        );
    }

    #[test]
    fn struct_calls_get_hidden_locals() {
        let p = ok(
            "struct big { int v[3]; }; struct big g(struct big a, struct big b);
                    int f(struct big x) { return g(x, x).v[0]; }",
        );
        let f = func(&p, "f");
        let temps: Vec<String> = f
            .locals
            .iter()
            .filter(|l| l.temp)
            .map(|l| format!("{} {}", l.ty, l.name))
            .collect();
        assert_eq!(
            temps,
            [
                "struct big argument copy",
                "struct big argument copy",
                "struct big call result"
            ]
        );
        // A definition returning a big struct keeps the result's address.
        let p = ok("struct big { int v[3]; }; struct big g(int a) { struct big b; b.v[0] = a; return b; }");
        let g = func(&p, "g");
        assert_eq!(g.locals[g.ret_ptr.unwrap().0].ty.to_string(), "struct big *");
        // Small structs travel in registers: no copies.
        let p = ok("struct s { int a, b; }; struct s g(struct s a); int f(struct s x) { return g(x).a; }");
        assert_eq!(func(&p, "f").locals.iter().filter(|l| l.temp).count(), 1);
        // Past a0-a7 arguments go on the stack.
        ok("struct s { int a, b; }; int g(struct s a, struct s b, struct s c, struct s d, int k) { return k; }");
    }

    #[test]
    fn compound_literals() {
        let p = ok("struct p { int x, y; }; int f() { int *a = (int[]){1, 2, 3}; \
                    return a[2] + (struct p){.y = 4}.y + sizeof (char[]){\"abc\"}; }");
        let f = func(&p, "f");
        let temps: Vec<String> = f
            .locals
            .iter()
            .filter(|l| l.temp)
            .map(|l| l.ty.to_string())
            .collect();
        assert_eq!(temps, ["int [3]", "struct p", "char [4]"]);
        // An lvalue.
        ok("int f() { int *p = &(int){5}; (int){1} = 2; return *p; }");
        assert_eq!(
            err("int f() { return (void){0}; }"),
            "1:18: compound literal has incomplete type 'void'"
        );
    }

    #[test]
    fn file_scope_compound_literals_are_static_objects() {
        let p = ok(
            "struct p { int x, y; }; int *a = (int[]){1, 2, 3}; struct p *q = &(struct p){.y = 4};
                    const char *const *names = (const char *const[]){\"a\", \"b\"};
                    struct p **pp = (struct p *[]){&(struct p){5, 6}, 0};",
        );
        // Each became a static variable the initializer names, numbered in
        // order (a nested literal before the list it is in), declared last.
        let Some(Item::Decl(literals)) = p.items.last() else {
            panic!("no literals")
        };
        let got: Vec<String> = literals.iter().map(|d| format!("{}: {}", d.name, d.ty)).collect();
        assert_eq!(
            got,
            [
                ".compoundliteral.0: int [3]",
                ".compoundliteral.1: struct p",
                ".compoundliteral.2: const char *const [2]",
                ".compoundliteral.3: struct p",
                ".compoundliteral.4: struct p *[2]",
            ]
        );
        assert!(literals.iter().all(|d| d.define && d.storage == Storage::Static));
        // `a` and `q` hold their addresses: constants.
        for (i, name) in ["a", "q"].iter().enumerate() {
            let d = p
                .items
                .iter()
                .find_map(|item| match item {
                    Item::Decl(ds) => ds.iter().find(|d| d.name == *name),
                    Item::Func(_) => None,
                })
                .unwrap();
            let Some(Init::Expr(e)) = &d.init else { panic!() };
            assert_eq!(
                crate::consteval::eval(e, Xlen::Rv32),
                Some(crate::consteval::Const::Addr {
                    base: Some(crate::consteval::Base::Sym(format!(".compoundliteral.{i}"))),
                    offset: 0
                })
            );
        }
        // The list must be constant, as a global's.
        assert_eq!(
            err("int n; int *p = (int[]){n};"),
            "1:25: initializer element is not a compile-time constant"
        );
        assert_eq!(
            err("int *p = (void){0};"),
            "1:10: compound literal has incomplete type 'void'"
        );
        // In the unevaluated operand of `sizeof` a literal is no object:
        // nothing is emitted for it (as with clang), and the numbering
        // carries on as if it were not there.
        let p = ok("int n = sizeof (int[]){1, 2, 3}; int *a = (int[]){4}; int k = sizeof (int){5} + 1;");
        let Some(Item::Decl(literals)) = p.items.last() else {
            panic!("no literals")
        };
        let got: Vec<String> = literals.iter().map(|d| format!("{}: {}", d.name, d.ty)).collect();
        assert_eq!(got, [".compoundliteral.0: int [1]"]);
        // Only its address is constant: using its value to initialize a
        // global (a GNU extension clang accepts) is not supported.
        assert_eq!(
            err("int m = (int){5};"),
            "1:9: initializer element is not a compile-time constant"
        );
    }

    #[test]
    fn variadic_calls() {
        let p = ok("int f(int n, ...); struct s { int v[4]; } x; \
                    int g(char c) { return f(1, c, \"s\", x, (short)2); }");
        let Some(StmtKind::Return(Some(e))) = func(&p, "g").body.as_ref().unwrap().first().map(|s| &s.kind)
        else {
            panic!()
        };
        let ExprKind::Call { args, arg_copies, .. } = &e.kind else {
            panic!()
        };
        // The default argument promotions: char and short become int, an
        // array a pointer; a big struct goes as a copy's address.
        let types: Vec<String> = args.iter().map(|a| a.ty.to_string()).collect();
        assert_eq!(types, ["int", "int", "char *", "struct s", "int"]);
        assert!(arg_copies[3].is_some());
        assert_eq!(
            err("int f(int n, ...); int g() { return f(); }"),
            "1:37: too few arguments to 'f': expected at least 1, have 0 (declared at 1:5)"
        );
        assert_eq!(
            err("void v(void); int f(int n, ...); int g() { return f(1, v()); }"),
            "1:56: argument type 'void' is incomplete"
        );
    }

    #[test]
    fn va_builtins() {
        ok(
            "int f(int n, ...) { __builtin_va_list ap, aq; __builtin_va_start(ap, n); \
            __builtin_va_copy(aq, ap); int k = __builtin_va_arg(aq, int); __builtin_va_end(aq); \
            __builtin_va_end(ap); return k; }",
        );
        assert_eq!(
            err("int f(int n) { __builtin_va_list ap; __builtin_va_start(ap, n); return 0; }"),
            "1:38: 'va_start' used in a function with fixed parameters"
        );
        assert_eq!(
            err("int f(int n, int m, ...) { __builtin_va_list ap; __builtin_va_start(ap, n); return 0; }"),
            "1:73: second argument to 'va_start' must be the last named parameter, 'm'"
        );
        assert_eq!(
            err("int f(int n, ...) { int ap; __builtin_va_start(ap, n); return 0; }"),
            "1:48: first argument to '__builtin_va_start' must be a 'va_list', not 'int'"
        );
        assert_eq!(
            err("int f(int n, ...) { __builtin_va_list ap; return __builtin_va_arg(ap, char); }"),
            "1:50: second argument to 'va_arg' is of promotable type 'char': the argument was promoted \
             to 'int', so this would be undefined (write '(char)va_arg(ap, int)')"
        );
        assert_eq!(
            err("int f(int n, ...) { __builtin_va_list ap; return __builtin_va_arg(ap, int[2])[0]; }"),
            "1:50: second argument to 'va_arg' is of array type 'int [2]'"
        );
    }

    #[test]
    fn qualifiers() {
        ok("int f(char *s) { const char *t = s; const char **u = &t; volatile int v = 1; return *t + v; }");
        ok("const int k = 3; int f(const int *p) { return *p + k; }");
        assert_eq!(
            err("const int k = 1; int f() { k = 2; return 0; }"),
            "1:28: cannot assign to variable 'k' with const-qualified type 'const int'"
        );
        assert_eq!(
            err("int f(const int *p) { *p = 1; return 0; }"),
            "1:23: read-only variable is not assignable"
        );
        assert_eq!(
            err("int f(const int a[2]) { a[0]++; return 0; }"),
            "1:26: read-only variable is not assignable"
        );
        assert_eq!(
            err("int f(const char *s) { char *t = s; return 0; }"),
            "1:34: initializing 'char *' with an expression of type 'const char *' discards qualifiers"
        );
        assert_eq!(
            err("int g(int *p); int f(volatile int *v) { return g(v); }"),
            "1:50: passing 'volatile int *' to parameter of type 'int *' discards qualifiers"
        );
        // Comparisons and differences ignore what the pointers' targets add.
        assert_eq!(ty("const int *p, int *q", "p - q"), "int");
        assert_eq!(ty("const int *p, int *q", "p == q"), "int");
        assert_eq!(
            ty("const int *p, volatile int *q, int c", "c ? p : q"),
            "const volatile int *"
        );
        // A value has no qualifiers.
        assert_eq!(ty("const int x", "x + 0"), "int");
        assert_eq!(ty("const int x", "x"), "const int");
    }

    #[test]
    fn function_pointers() {
        ok("int g(int); int f() { int (*p)(int) = g; int (*q)(int) = &g; return p(1) + (*q)(2) + (**p)(3); }");
        assert_eq!(ty("int (*p)(int)", "p(1)"), "int");
        assert_eq!(ty("int (*p)(int)", "*p"), "int (int)");
        assert_eq!(ty("int (*p)(int)", "&*p"), "int (*)(int)");
        assert_eq!(
            err("void g(void); int f() { void *p = g; return 0; }"),
            "1:35: incompatible pointer types initializing 'void *' with an expression of type 'void (*)(void)'"
        );
        assert_eq!(
            err("int g(int); int f() { int (*q)(char) = g; return 0; }"),
            "1:40: incompatible pointer types initializing 'int (*)(char)' with an expression of type 'int (*)(int)'"
        );
        assert_eq!(
            err("int f(int x) { int (*p)(int) = 0; return p(1, 2); }"),
            "1:42: too many arguments to function call: expected 1, have 2"
        );
        assert_eq!(
            err("int f(void) { return 0; } int g() { return sizeof f; }"),
            "1:44: invalid application of 'sizeof' to a function type 'int (void)'"
        );
        assert_eq!(
            err("int f(void) { return 0; } int g() { f = 0; return 0; }"),
            "1:37: non-object type 'int (void)' is not assignable"
        );
    }

    /// The `switch` of the function's first statement: its cases as
    /// (value, id), and its default's id.
    fn switch_cases(src: &str) -> (Vec<(i128, usize)>, Option<usize>) {
        let p = ok(src);
        let f = p.funcs().find(|f| f.body.is_some()).unwrap();
        let StmtKind::Switch { cases, default, .. } = &f.body.as_ref().unwrap()[0].kind else {
            panic!()
        };
        (cases.clone(), *default)
    }

    #[test]
    fn switch_and_case() {
        assert_eq!(
            switch_cases(
                "int f(char c) { switch (c) { case 'a': case 2 + 3: { default: ; } case -1: break; } return 0; }"
            ),
            (vec![(97, 0), (5, 1), (-1, 3)], Some(2))
        );
        // Case values convert to the promoted type of the condition.
        assert_eq!(
            switch_cases("int f(unsigned u) { switch (u) { case -1: return 1; } return 0; }"),
            (vec![(0xffff_ffff, 0)], None)
        );
        // A nested switch has its own cases; ids are per function.
        assert_eq!(
            switch_cases("int f(int x) { switch (x) { case 1: switch (x) { case 1: break; } } return 0; }"),
            (vec![(1, 0)], None)
        );
        assert_eq!(
            err("int f(int x) { switch (x) { case 1: case 1: break; } return 0; }"),
            "1:42: duplicate case value '1' (previous case at 1:34)"
        );
        assert_eq!(
            err("int f(unsigned x) { switch (x) { case -1: case 0xffffffff: break; } return 0; }"),
            "1:48: duplicate case value '4294967295' (previous case at 1:39)"
        );
        assert_eq!(
            err("int f(int x) { switch (x) { default: default: break; } return 0; }"),
            "1:38: multiple default labels in one switch (first at 1:29)"
        );
        assert_eq!(
            err("int f(int x) { case 1: return 0; }"),
            "1:16: 'case' statement not in switch statement"
        );
        assert_eq!(
            err("int f(int x) { default: return 0; }"),
            "1:16: 'default' statement not in switch statement"
        );
        assert_eq!(
            err("int f(int x) { switch (x) { case x: break; } return 0; }"),
            "1:34: case value is not an integer constant expression"
        );
        assert_eq!(
            err("int f(int *x) { switch (x) { case 1: break; } return 0; }"),
            "1:25: statement requires expression of integer type ('int *' invalid)"
        );
        // `break` leaves a switch; `continue` still needs a loop.
        ok("int f(int x) { switch (x) { case 1: break; } return 0; }");
        assert_eq!(
            err("int f(int x) { switch (x) { case 1: continue; } return 0; }"),
            "1:37: 'continue' statement not in a loop"
        );
    }

    #[test]
    fn labels_and_goto() {
        ok("int f() { goto end; { inner: ; } end: goto inner; }");
        ok("int f() { a: ; } int g() { a: ; }");
        assert_eq!(
            err("int f() { goto nowhere; return 0; }"),
            "1:11: use of undeclared label 'nowhere'"
        );
        assert_eq!(
            err("int f() { a: a: return 0; }"),
            "1:14: redefinition of label 'a' (first defined at 1:11)"
        );
    }

    #[test]
    fn struct_and_union_initializers() {
        ok("struct s { int a; char b[3]; struct { short x, y; } in; }; \
            struct s g = {1, \"ab\", {2, 3}}; struct s h = {1, {'a'}, 4}; union { char c; int i; } u = {5};");
        assert_eq!(
            err("struct s { int a; char b; }; struct s g = {1, 2, 3};"),
            "1:50: excess elements in struct initializer"
        );
        assert_eq!(
            err("union u { int a; char b; }; union u g = {1, 2};"),
            "1:45: excess elements in union initializer"
        );
        assert_eq!(
            err("struct s { int a; }; struct s g = {{1, 2}};"),
            "1:36: excess elements in scalar initializer"
        );
        assert_eq!(
            err("struct p { int x; }; static struct p g = {1}; struct p h = g;"),
            "1:60: initializer element is not a compile-time constant"
        );
        // A local may be initialized by a struct value.
        ok("struct p { int x; }; struct p g(void); int f() { struct p a = g(), b = {a.x}, c[2] = {a, b}; return c[1].x; }");
    }

    #[test]
    fn inline_definitions() {
        // `inline` without `extern` anywhere: an inline definition.
        let p = ok("inline int f(void) { return 1; }");
        assert!(func(&p, "f").inline_only);
        let p = ok("int f(void); inline int f(void) { return 1; }");
        assert!(!func(&p, "f").inline_only);
        let p = ok("extern inline int f(void) { return 1; }");
        assert!(!func(&p, "f").inline_only);
        let p = ok("static inline int f(void) { return 1; }");
        assert!(!func(&p, "f").inline_only);
    }
}
