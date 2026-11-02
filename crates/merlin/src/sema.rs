//! Name resolution and type checking.
//!
//! * Every name must be declared before use (C99: no implicit declarations).
//!   Block scoping follows C: inner blocks may shadow outer names and globals.
//!   [`Function::locals`] gets every parameter and local, and every variable
//!   reference and declarator its [`VarRef`].
//! * Every expression gets its C type, by C99's rules with ILP32 sizes (see
//!   [`crate::types`]). Operands must make sense for their operator (no
//!   arithmetic on `void *`, no pointer plus pointer, `*` only on pointers),
//!   assignments, arguments and returns must convert as C allows, and the
//!   conversions C applies there become explicit implicit-cast nodes.
//! * `break`/`continue` must be inside a loop, `return` must match the
//!   function's type, global and `static` initializers must be constant, and
//!   of several declarations of one global, one is marked as its definition.
//! * `sizeof` is replaced by its value.
//!
//! Reports all errors, in source order. An expression with an error doesn't
//! produce more errors about the expressions around it.

use crate::ast::*;
use crate::consteval;
use crate::types::{common, IntKind, Type};
use crate::{Error, Pos};
use std::collections::{HashMap, HashSet};

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
    match (op, l, r) {
        (_, Type::Int(a), Type::Int(b)) => Some(Type::Int(match op {
            BinOp::Shl | BinOp::Shr => a.promoted(),
            _ => common(*a, *b),
        })),
        (BinOp::Add | BinOp::Sub, Type::Ptr(_), Type::Int(_)) if sized(l) => Some(l.clone()),
        (BinOp::Add, Type::Int(_), Type::Ptr(_)) if sized(r) => Some(r.clone()),
        // The difference of two pointers counts elements: ptrdiff_t, `int` on ILP32.
        (BinOp::Sub, Type::Ptr(a), Type::Ptr(b)) if a == b && sized(l) => Some(Type::INT),
        _ => None,
    }
}

/// Whether `e` designates an object (C's lvalue): a variable, `*p`, or a
/// string literal.
fn is_lvalue(e: &Expr) -> bool {
    matches!(
        e.kind,
        ExprKind::Var { .. } | ExprKind::Deref(_) | ExprKind::Str(_)
    )
}

/// Wraps `e` in an implicit conversion to `to`, unless it already has that type.
fn coerce(e: &mut Expr, to: &Type) {
    if e.ty == *to {
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

static VOID: Type = Type::Void;

/// An error has been reported; the caller should not add more about the same
/// expression.
struct Reported;

type Checked = Result<(), Reported>;

struct FuncSig {
    ret: Type,
    params: Vec<Type>,
    is_static: bool,
    defined: bool,
    pos: Pos,
}

impl FuncSig {
    /// `int (char *, int)`, for messages.
    fn describe(&self) -> String {
        let params: Vec<String> = self.params.iter().map(Type::to_string).collect();
        let params = if params.is_empty() {
            "void".to_string()
        } else {
            params.join(", ")
        };
        format!("{} ({params})", self.ret)
    }
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
            if p.ty == Type::Void {
                self.err(p.pos, "parameter has incomplete type 'void'");
            }
        }
        let sig = FuncSig {
            ret: f.ret.clone(),
            params: f.params.iter().map(|p| p.ty.clone()).collect(),
            is_static: f.is_static,
            defined: f.body.is_some(),
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
                let (prev_pos, prev_arity) = (prev.pos, prev.params.len());
                if prev_arity != sig.params.len() {
                    let msg = format!(
                        "conflicting types for '{}': declared at {prev_pos} with {prev_arity} parameter{}, \
                         here with {}",
                        f.name,
                        plural(prev_arity),
                        sig.params.len()
                    );
                    self.err(f.pos, msg);
                } else if prev.ret != sig.ret || prev.params != sig.params {
                    let msg = format!(
                        "conflicting types for '{}': declared at {prev_pos} as '{}', here as '{}'",
                        f.name,
                        prev.describe(),
                        sig.describe()
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
        let Some(body) = &mut f.body else { return };
        let mut c = FnChecker {
            globals: &self.globals,
            func: &f.name,
            ret: &f.ret,
            scopes: vec![HashMap::new()],
            locals: Vec::new(),
            loop_depth: 0,
            static_labels: HashSet::new(),
            errors: &mut self.errors,
        };
        for p in &f.params {
            c.declare(&p.name, &p.ty, p.pos, "parameter ");
        }
        // The body's outermost block shares the parameters' scope, as in C.
        for s in body {
            c.stmt(s);
        }
        f.locals = c.locals;
    }

    fn global_var(&mut self, d: &mut Declarator, at: (usize, usize)) {
        if d.ty == Type::Void {
            return self.err(d.pos, format!("variable '{}' has incomplete type 'void'", d.name));
        }
        if d.init.is_some() && d.storage == Storage::Extern {
            return self.err(
                d.pos,
                format!("'extern' declaration of '{}' cannot have an initializer", d.name),
            );
        }
        d.var = Some(VarRef::Global(d.name.clone()));
        if let Some(init) = &mut d.init {
            let mut fc = FnChecker::file_scope(&self.globals, &mut self.errors);
            if fc.init(&mut d.ty, init, true).is_err() {
                return;
            }
        }
        let is_static = d.storage == Storage::Static;
        let tentative = (d.storage != Storage::Extern).then_some(at);
        let init = d.init.is_some().then_some(at);
        match self.globals.get_mut(&d.name) {
            Some(Global::Func(_)) => {
                self.err(
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
                if d.ty.size().is_some() {
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
        for ((i, j), ty) in defs {
            let Item::Decl(decls) = &mut prog.items[i] else {
                unreachable!("recorded from a declaration")
            };
            let d = &mut decls[j];
            if ty.size().is_none() {
                let msg = format!("definition of '{}' needs the array size ('{}')", d.name, ty);
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

struct FnChecker<'a> {
    globals: &'a HashMap<String, Global>,
    /// The function being checked (empty at file scope).
    func: &'a str,
    ret: &'a Type,
    /// Block scopes, innermost last: names to where they live, their type and
    /// where they were declared.
    scopes: Vec<HashMap<String, (VarRef, Type, Pos)>>,
    locals: Vec<Local>,
    loop_depth: usize,
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
        });
        self.bind(name, VarRef::Local(id), ty, pos, what);
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
        f(self);
        self.loop_depth -= 1;
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
            StmtKind::Break | StmtKind::Continue if self.loop_depth == 0 => {
                let what = if matches!(s.kind, StmtKind::Break) {
                    "break"
                } else {
                    "continue"
                };
                self.err(s.pos, format!("'{what}' statement not in a loop"));
            }
            StmtKind::Break | StmtKind::Continue => {}
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
        if d.ty == Type::Void {
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
            if let Some(init) = &mut d.init {
                let _ = self.init(&mut d.ty, init, true);
            }
            self.bind(&d.name, VarRef::Global(label.clone()), &d.ty, d.pos, "");
            d.var = Some(VarRef::Global(label));
            d.define = true;
        } else {
            // As in C, the name is in scope inside its own initializer.
            let id = self.declare(&d.name, &d.ty, d.pos, "");
            d.var = Some(VarRef::Local(id));
            if let Some(init) = &mut d.init {
                let _ = self.init(&mut d.ty, init, false);
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
    /// array of unknown size. `constant`: for globals and statics, every
    /// element must be a constant expression.
    fn init(&mut self, ty: &mut Type, init: &mut Init, constant: bool) -> Checked {
        // `{ x }` for a scalar, or `{ "text" }` for a char array, means `x`.
        if let Init::List(items, _) = init {
            let unwrap = match &items[..] {
                [Init::Expr(_)] if !ty.is_array() => true,
                [Init::Expr(Expr {
                    kind: ExprKind::Str(_),
                    ..
                })] => is_char_array(ty),
                _ => false,
            };
            if unwrap {
                let Init::List(mut items, _) =
                    std::mem::replace(init, Init::List(Vec::new(), Pos::default()))
                else {
                    unreachable!()
                };
                *init = items.remove(0);
            }
        }
        match (&mut *ty, init) {
            (Type::Array(elem, n), Init::Expr(e)) if is_char_array_elem(elem) => {
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
            (Type::Array(..), Init::Expr(e)) => {
                Err(self.err(e.pos, "array initializer must be an initializer list"))
            }
            (Type::Array(elem, n), Init::List(items, _)) => {
                if let Some(n) = *n {
                    if items.len() > n as usize {
                        return Err(self.err(items[n as usize].pos(), "excess elements in array initializer"));
                    }
                }
                let mut result = Ok(());
                for item in items.iter_mut() {
                    // Inner dimensions are always known: the parser requires them.
                    let mut elem_ty = (**elem).clone();
                    if self.init(&mut elem_ty, item, constant).is_err() {
                        result = Err(Reported);
                    }
                }
                if n.is_none() {
                    *n = Some(items.len() as u32);
                }
                result
            }
            (_, Init::List(items, pos)) => {
                if items.is_empty() {
                    return Ok(());
                }
                let what = if matches!(items[0], Init::List(..)) {
                    "too many braces around scalar initializer"
                } else {
                    "excess elements in scalar initializer"
                };
                Err(self.err(*pos, what))
            }
            (ty, Init::Expr(e)) => {
                self.expr(e)?;
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
    /// (C99 6.5.16.1): arithmetic to arithmetic, a pointer to a pointer of the
    /// same type or to or from `void *`, and the null pointer constant to any
    /// pointer.
    fn convert(&mut self, e: &Expr, to: &Type, how: Conv) -> Checked {
        let from = e.ty.decay();
        let problem = match (to, &from) {
            (Type::Int(_), Type::Int(_)) => return Ok(()),
            (Type::Ptr(a), Type::Ptr(b)) if a == b || **a == Type::Void || **b == Type::Void => return Ok(()),
            (Type::Ptr(_), Type::Int(_)) if consteval::is_null_pointer(e) => return Ok(()),
            (Type::Ptr(_), Type::Ptr(_)) => "incompatible pointer types",
            (Type::Ptr(_), Type::Int(_)) => "incompatible integer to pointer conversion",
            (Type::Int(_), Type::Ptr(_)) => "incompatible pointer to integer conversion",
            _ => "incompatible types",
        };
        let msg = format!("{problem} {}", how.phrase(to, &from));
        Err(self.err(e.pos, msg))
    }

    /// The target of `=`, `op=`, `++` and `--`: an lvalue that is not an array.
    fn assignable(&mut self, target: &Expr) -> Checked {
        if !is_lvalue(target) || matches!(target.kind, ExprKind::Str(_)) {
            return Err(self.err(target.pos, "expression is not assignable"));
        }
        if target.ty.is_array() {
            let msg = format!("array type '{}' is not assignable", target.ty);
            return Err(self.err(target.pos, msg));
        }
        Ok(())
    }

    fn invalid_operands(&mut self, pos: Pos, l: &Type, r: &Type) -> Reported {
        let msg = if l.pointee() == Some(&Type::Void) || r.pointee() == Some(&Type::Void) {
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
                    Some(Global::Func(_)) => {
                        let msg = format!("'{name}' is a function; function pointers are not supported yet");
                        return Err(self.err(pos, msg));
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
                target.ty.clone()
            }
            ExprKind::IncDec { inc, target, .. } => {
                self.expr(target)?;
                self.assignable(target)?;
                let ok = match &target.ty {
                    Type::Int(_) => true,
                    Type::Ptr(t) => t.size().is_some(),
                    _ => false,
                };
                if !ok {
                    let what = if *inc { "increment" } else { "decrement" };
                    let msg = format!("cannot {what} value of type '{}'", target.ty);
                    return Err(self.err(pos, msg));
                }
                target.ty.clone()
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
                    Type::Ptr(t) if *t == Type::Void => {
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
                let ok = match to {
                    Type::Void => true,
                    Type::Int(_) | Type::Ptr(_) => from.is_scalar(),
                    Type::Array(..) => false,
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
            ExprKind::Call(name, args) => {
                let mut ok = true;
                for a in args.iter_mut() {
                    ok &= self.expr(a).is_ok();
                }
                if self.lookup(name).is_some() || matches!(globals.get(name.as_str()), Some(Global::Var(_))) {
                    let msg = format!("called object '{name}' is a variable, not a function");
                    return Err(self.err(pos, msg));
                }
                let Some(Global::Func(sig)) = globals.get(name.as_str()) else {
                    return Err(self.err(pos, format!("call to undeclared function '{name}'")));
                };
                if sig.params.len() != args.len() {
                    let (arity, sig_pos) = (sig.params.len(), sig.pos);
                    let which = if args.len() > arity { "many" } else { "few" };
                    let msg = format!(
                        "too {which} arguments to '{name}': expected {arity}, have {} (declared at {sig_pos})",
                        args.len()
                    );
                    return Err(self.err(pos, msg));
                }
                if !ok {
                    return Err(Reported);
                }
                let (params, ret) = (sig.params.clone(), sig.ret.clone());
                for (a, p) in args.iter_mut().zip(&params) {
                    // Each argument converts to its parameter's type as if assigned.
                    if self.convert(a, p, Conv::Arg).is_ok() {
                        coerce(a, p);
                    } else {
                        ok = false;
                    }
                }
                if !ok {
                    return Err(Reported);
                }
                ret
            }
        };
        e.ty = ty;
        Ok(())
    }

    /// Replaces a `sizeof` with its value, an `unsigned int` (C's `size_t`).
    fn sizeof(&mut self, e: &mut Expr, t: &Type) -> Checked {
        let Some(size) = t.size() else {
            let msg = format!("invalid application of 'sizeof' to an incomplete type '{t}'");
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
                let ok = match (&lt, &rt) {
                    (Type::Int(_), Type::Int(_)) => true,
                    (Type::Ptr(a), Type::Ptr(b)) => {
                        a == b
                            || (matches!(op, BinOp::Eq | BinOp::Ne)
                                && (**a == Type::Void || **b == Type::Void))
                    }
                    // A pointer may be compared for equality with the null pointer constant.
                    (Type::Ptr(_), Type::Int(_)) => {
                        matches!(op, BinOp::Eq | BinOp::Ne) && consteval::is_null_pointer(r)
                    }
                    (Type::Int(_), Type::Ptr(_)) => {
                        matches!(op, BinOp::Eq | BinOp::Ne) && consteval::is_null_pointer(l)
                    }
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
            (Type::Ptr(x), Type::Ptr(y)) if x == y => at.clone(),
            (Type::Ptr(x), Type::Ptr(_)) if **x == Type::Void => at.clone(),
            (Type::Ptr(_), Type::Ptr(y)) if **y == Type::Void => bt.clone(),
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

/// `char`, `signed char` or `unsigned char`: the element types a string
/// literal may initialize.
fn is_char_array_elem(t: &Type) -> bool {
    matches!(t, Type::Int(IntKind::Char | IntKind::SChar | IntKind::UChar))
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
                ExprKind::Call(_, args) => args.iter().for_each(|a| expr(a, out)),
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
                ExprKind::Call(n, args) => {
                    format!("({n} {})", args.iter().map(sx).collect::<Vec<_>>().join(" "))
                }
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
        assert_eq!(
            err("char a[2][2] = {\"ab\", 1};"),
            "1:23: array initializer must be an initializer list or a string literal"
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
            "1:40: 'f' is a function; function pointers are not supported yet"
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
            "1:23: called object 'x' is a variable, not a function"
        );
        assert_eq!(
            err("int x; int f() { return x(1); }"),
            "1:25: called object 'x' is a variable, not a function"
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
        assert_eq!(err("int f() { break; }"), "1:11: 'break' statement not in a loop");
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
            "1:24: 'break' statement not in a loop"
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
    }

    #[test]
    fn reports_every_error_in_order() {
        assert_eq!(
            errs("int f() {\n  x = 1;\n  break;\n  return g();\n}\nint h() { return y; }"),
            [
                "2:3: use of undeclared identifier 'x'",
                "3:3: 'break' statement not in a loop",
                "4:10: call to undeclared function 'g'",
                "6:18: use of undeclared identifier 'y'",
            ]
        );
    }
}
