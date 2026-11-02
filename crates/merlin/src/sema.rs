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
//!
//! Reports all errors, in source order. An expression with an error doesn't
//! produce more errors about the expressions around it.

use crate::abi;
use crate::ast::*;
use crate::consteval;
use crate::types::{common, FuncType, IntKind, Type};
use crate::{Error, Pos};
use std::collections::{HashMap, HashSet, VecDeque};

pub fn check(prog: &mut Program) -> Result<(), Vec<Error>> {
    let mut c = Checker {
        globals: HashMap::new(),
        errors: Vec::new(),
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
pub fn arith_type(op: BinOp, l: &Type, r: &Type) -> Option<Type> {
    // Pointer arithmetic needs to know the size of what is pointed to.
    let sized = |t: &Type| t.pointee().and_then(Type::size).is_some();
    match (op, l.unqual(), r.unqual()) {
        (_, Type::Int(a), Type::Int(b)) => Some(Type::Int(match op {
            BinOp::Shl | BinOp::Shr => a.promoted(),
            _ => common(*a, *b),
        })),
        (BinOp::Add | BinOp::Sub, Type::Ptr(_), Type::Int(_)) if sized(l) => Some(l.clone()),
        (BinOp::Add, Type::Int(_), Type::Ptr(_)) if sized(r) => Some(r.clone()),
        // The difference of two pointers counts elements: ptrdiff_t, `int` on
        // ILP32. What they point to may differ in qualifiers only.
        (BinOp::Sub, Type::Ptr(a), Type::Ptr(b)) if a.unqual() == b.unqual() && sized(l) => Some(Type::INT),
        _ => None,
    }
}

/// Whether `e` designates an object (C's lvalue): a variable, `*p`, a
/// string literal, `p->m`, or `s.m` of an lvalue `s` (a function's name, which
/// designates the function, counts too, so that `&f` works).
fn is_lvalue(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::Var { .. } | ExprKind::Deref(_) | ExprKind::Str(_) => true,
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
        // A definition needs complete parameter and result types, and its
        // arguments must fit the argument registers.
        for p in &f.params {
            if p.ty.is_record() && p.ty.size().is_none() {
                self.err(p.pos, format!("variable has incomplete type '{}'", p.ty));
            }
        }
        let ret_complete = !f.ret.is_record() || f.ret.size().is_some();
        if !ret_complete {
            self.err(
                f.pos,
                format!("incomplete result type '{}' in function definition", f.ret),
            );
        }
        let Type::Func(ft) = f.ty() else { unreachable!() };
        if ret_complete && ft.params.iter().all(|p| p.size().is_some()) {
            self.check_registers(&ft, f.pos);
        }
        let body = f.body.as_mut().expect("checked above");
        let mut c = FnChecker {
            globals: &self.globals,
            func: &f.name,
            ret: &f.ret,
            scopes: vec![HashMap::new()],
            locals: Vec::new(),
            loop_depth: 0,
            break_depth: 0,
            switches: Vec::new(),
            next_case: 0,
            labels: HashMap::new(),
            gotos: Vec::new(),
            static_labels: HashSet::new(),
            errors: &mut self.errors,
        };
        for p in &f.params {
            c.declare(&p.name, &p.ty, p.pos, "parameter ");
        }
        // A big struct result goes where the caller says: keep that address.
        f.ret_ptr = (ret_complete && abi::returns_in_memory(&f.ret))
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

    /// Arguments travel only in `a0-a7`: a function whose arguments (a struct
    /// may take two registers, a big struct result one more) need more is an
    /// error.
    fn check_registers(&mut self, f: &FuncType, pos: Pos) {
        let (_, used) = abi::arguments(f);
        if used > abi::ARG_REGS {
            self.err(
                pos,
                format!("the arguments need {used} registers; merlin passes arguments only in a0-a7"),
            );
        }
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
                if d.ty.is_array() && d.ty.size().is_none() {
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
                if !d.ty.is_array() || d.ty.size().is_some() {
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
        let mut fc = FnChecker::file_scope(&self.globals, &mut self.errors);
        let checked = fc.init(&mut d.ty, d_init, true);
        let Some(Global::Var(g)) = self.globals.get_mut(&d.name) else {
            unreachable!("declared above")
        };
        if checked.is_err() {
            // Still declared, so later uses don't report it as undeclared, but
            // not defined here: there is nothing to emit.
            g.init = g.init.filter(|&i| i != at);
            g.tentative = g.tentative.filter(|&i| i != at);
        } else if g.ty.size().is_none() {
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
            if ty.size().is_none() {
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
    cases: Vec<(i64, usize)>,
    seen: HashMap<i64, Pos>,
    default: Option<(usize, Pos)>,
}

/// An element of an initializer list not yet matched to a subobject, and
/// whether its expression has been checked already (to see if it
/// initializes a struct as a whole).
struct Pending {
    init: Init,
    checked: bool,
}

struct FnChecker<'a> {
    globals: &'a HashMap<String, Global>,
    /// The function being checked (empty at file scope).
    func: &'a str,
    ret: &'a Type,
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
    errors: &'a mut Vec<Error>,
}

impl<'a> FnChecker<'a> {
    /// A checker for global initializers, outside any function.
    fn file_scope(globals: &'a HashMap<String, Global>, errors: &'a mut Vec<Error>) -> Self {
        FnChecker {
            globals,
            func: "",
            ret: &VOID,
            scopes: Vec::new(),
            locals: Vec::new(),
            loop_depth: 0,
            break_depth: 0,
            switches: Vec::new(),
            next_case: 0,
            labels: HashMap::new(),
            gotos: Vec::new(),
            static_labels: HashSet::new(),
            errors,
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
                    match consteval::int(expr).filter(|_| expr.ty.is_integer()) {
                        None => {
                            self.err(expr.pos, "case value is not an integer constant expression");
                        }
                        Some(v) => {
                            *id = self.next_case;
                            self.next_case += 1;
                            let sw = self.switches.last_mut().expect("checked above");
                            *value = sw.kind.wrap(v);
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
        if d.ty.size().is_none() {
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
        let list = self.fill(ty, &mut queue, constant, pos)?;
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
    /// after subobject (elements, or members; for a union only the first),
    /// as long as there are items. Returns its fully braced list.
    fn fill(
        &mut self,
        ty: &mut Type,
        queue: &mut VecDeque<Pending>,
        constant: bool,
        pos: Pos,
    ) -> Result<Init, Reported> {
        let mut out = Vec::new();
        if let Type::Array(elem, n) = ty {
            while !queue.is_empty() && n.is_none_or(|n| out.len() < n as usize) {
                // Inner dimensions are always known: the parser requires them.
                let mut elem_ty = (**elem).clone();
                out.push(self.subobject(&mut elem_ty, queue, constant)?);
            }
            if n.is_none() {
                *n = Some(out.len() as u32);
            }
        } else {
            let record = ty.record().expect("an aggregate").clone();
            let Some(body) = record.body() else {
                let msg = format!("initializer for incomplete type '{ty}'");
                return Err(self.err(pos, msg));
            };
            let count = if record.union { 1 } else { body.members.len() };
            for m in &body.members[..count] {
                if queue.is_empty() {
                    break;
                }
                let mut member_ty = m.ty.clone();
                out.push(self.subobject(&mut member_ty, queue, constant)?);
            }
        }
        Ok(Init::List(out, pos))
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
            Init::List(..) => false,
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
            return self.fill(ty, queue, constant, pos);
        }
        let Pending { mut init, checked } = queue.pop_front().expect("not empty");
        match &mut init {
            Init::Expr(e) => self.init_expr(ty, e, constant, checked)?,
            Init::List(..) => self.init(ty, &mut init, constant)?,
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
                if constant && consteval::eval(e).is_none() {
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
                if to.size().is_none() {
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
            (Type::Ptr(_), Type::Int(_)) if consteval::is_null_pointer(e) => return Ok(()),
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
                        let ok = match arith_type(*op, &t, &v) {
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
                    Type::Ptr(t) => t.size().is_some(),
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
                // The operand is typed (and checked) but never evaluated.
                self.expr(x)?;
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
                if ft.params.len() != args.len() {
                    let arity = ft.params.len();
                    let which = if args.len() > arity { "many" } else { "few" };
                    let msg = match callee.direct_callee().map(|n| (n, globals.get(n))) {
                        Some((name, Some(Global::Func(sig)))) => format!(
                            "too {which} arguments to '{name}': expected {arity}, have {} (declared at {})",
                            args.len(),
                            sig.pos
                        ),
                        _ => format!(
                            "too {which} arguments to function call: expected {arity}, have {}",
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
                    if p.is_record() && abi::classify(p) == abi::Pass::Ref {
                        arg_copies[i] = Some(self.temp(p, "argument copy", a.pos));
                    }
                }
                if !ok {
                    return Err(Reported);
                }
                if ft.ret.is_record() {
                    if ft.ret.size().is_none() {
                        let msg = format!("calling a function with incomplete result type '{}'", ft.ret);
                        return Err(self.err(pos, msg));
                    }
                    // A struct result is kept in a hidden local of the caller.
                    *result = Some(self.temp(&ft.ret, "call result", pos));
                }
                let (_, used) = abi::arguments(&ft);
                if used > abi::ARG_REGS {
                    let msg =
                        format!("the arguments need {used} registers; merlin passes arguments only in a0-a7");
                    return Err(self.err(pos, msg));
                }
                ft.ret
            }
        };
        e.ty = ty;
        Ok(())
    }

    /// Replaces a `sizeof` with its value, an `unsigned int` (C's `size_t`).
    fn sizeof(&mut self, e: &mut Expr, t: &Type) -> Checked {
        let Some(size) = t.size() else {
            let what = if t.is_func() {
                "a function type"
            } else {
                "an incomplete type"
            };
            let msg = format!("invalid application of 'sizeof' to {what} '{t}'");
            return Err(self.err(e.pos, msg));
        };
        e.kind = ExprKind::Num(size.into());
        e.ty = Type::UINT;
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
                    (Type::Ptr(a), Type::Ptr(b)) => {
                        a.unqual() == b.unqual()
                            || (equality && ((a.is_void() && !b.is_func()) || (b.is_void() && !a.is_func())))
                    }
                    // A pointer may be compared for equality with the null pointer constant.
                    (Type::Ptr(_), Type::Int(_)) => equality && consteval::is_null_pointer(r),
                    (Type::Int(_), Type::Ptr(_)) => equality && consteval::is_null_pointer(l),
                    _ => false,
                };
                if !ok {
                    let msg = format!("invalid comparison between '{lt}' and '{rt}'");
                    return Err(self.err(pos, msg));
                }
                Ok(Type::INT)
            }
            _ => arith_type(op, &lt, &rt).ok_or_else(|| self.invalid_operands(pos, &lt, &rt)),
        }
    }

    /// The type of `c ? a : b` (C99 6.5.15), converting both branches to it.
    fn cond_type(&mut self, a: &mut Expr, b: &mut Expr, pos: Pos) -> Result<Type, Reported> {
        let (at, bt) = (a.ty.decay(), b.ty.decay());
        let ty = match (&at, &bt) {
            (Type::Int(x), Type::Int(y)) => Type::Int(common(*x, *y)),
            (Type::Void, Type::Void) => Type::Void,
            (Type::Record(x), Type::Record(y)) if x == y => at.clone(),
            // A null pointer constant takes the other pointer's type, even
            // when it is `(void *)0` (C99 6.5.15p6).
            (Type::Ptr(_), Type::Ptr(_)) if consteval::is_null_pointer(a) => bt.clone(),
            (Type::Ptr(_), Type::Ptr(_)) if consteval::is_null_pointer(b) => at.clone(),
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
            (Type::Ptr(_), Type::Int(_)) if consteval::is_null_pointer(b) => at.clone(),
            (Type::Int(_), Type::Ptr(_)) if consteval::is_null_pointer(a) => bt.clone(),
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
                | ExprKind::IncDec { target: x, .. } => expr(x, out),
                ExprKind::Member { base, .. } => expr(base, out),
                ExprKind::Call { callee, args, .. } => {
                    if callee.direct_callee().is_none() {
                        expr(callee, out);
                    }
                    args.iter().for_each(|a| expr(a, out));
                }
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
        assert_eq!(crate::consteval::int(e), Some(4));
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
        assert_eq!(crate::consteval::int(e), Some(12));
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
                    Init::Expr(e) => match crate::consteval::int(e) {
                        Some(v) => v.to_string(),
                        None => "s".into(),
                    },
                }
            }
            show(d[0].init.as_ref().unwrap())
        };
        assert_eq!(braced("char a[2][2] = {\"ab\", 1};"), "{s, {1}}");
        assert_eq!(braced("int m[2][2] = {1, 2, 3, 4};"), "{{1, 2}, {3, 4}}");
        assert_eq!(braced("int m[][2] = {1, 2, 3};"), "{{1, 2}, {3}}");
        assert_eq!(braced("int m[2][3] = {{1}, 2};"), "{{1}, {2}}");
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
        assert_eq!(crate::consteval::int(e), Some(34));
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
        // Every argument must fit a0-a7: a pair takes two.
        assert_eq!(
            err("struct s { int a, b; }; int g(struct s a, struct s b, struct s c, struct s d, int k) { return 0; }"),
            "1:29: the arguments need 9 registers; merlin passes arguments only in a0-a7"
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
    fn switch_cases(src: &str) -> (Vec<(i64, usize)>, Option<usize>) {
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
