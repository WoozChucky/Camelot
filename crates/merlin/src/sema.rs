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
use crate::types::{common, FuncType, IntKind, Spelled, Type, Xlen};
use crate::{Error, Pos};
use std::collections::{HashMap, HashSet, VecDeque};

pub fn check(prog: &mut Program) -> Result<(), Vec<Error>> {
    let mut c = Checker {
        globals: HashMap::new(),
        errors: Vec::new(),
        literals: Vec::new(),
        xlen: prog.xlen,
    };
    // What the parser evaluated away at file scope is checked where it was,
    // after the items before it.
    let mut unevaluated = std::mem::take(&mut prog.unevaluated);
    let mut next = 0;
    for (i, item) in prog.items.iter_mut().enumerate() {
        next = c.unevaluated(&mut unevaluated, next, i);
        match item {
            Item::Func(f) => {
                c.file_unevaluated(&mut f.unevaluated);
                c.function(f);
            }
            Item::Decl(decls) => {
                for (j, d) in decls.iter_mut().enumerate() {
                    // A declarator's own before it is declared, its
                    // initializer's after.
                    c.file_unevaluated(&mut d.sized);
                    c.global_var(d, (i, j));
                    c.file_unevaluated(&mut d.unevaluated);
                }
            }
        }
    }
    c.unevaluated(&mut unevaluated, next, usize::MAX);
    prog.unevaluated = unevaluated;
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
    // The diagnostics are in the order clang gives them, which is the order
    // they were found in: the warnings go with the program, unless there is
    // an error, which stops it, and then they come with the errors.
    if c.errors.iter().all(Error::is_warning) {
        prog.warnings.extend(c.errors);
        Ok(())
    } else {
        Err(c.errors)
    }
}

/// The type of `l op r` for the arithmetic, bitwise and shift operators, with
/// both operands already decayed; `None` if C doesn't allow the operands.
/// Comparisons and `&&`/`||` always give `int`. Code generation uses this too,
/// to know the type a compound assignment computes in.
pub fn arith_type(op: BinOp, l: &Type, r: &Type, x: Xlen) -> Option<Type> {
    // Pointer arithmetic needs to know the size of what is pointed to.
    let sized = |t: &Type| t.step(x).is_some();
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

/// The type of `e` if it is a compound literal, or a member of one (`.`,
/// at any depth), before sema has checked it: the literal's, and its
/// members' as its record has them.
fn literal_type_of(e: &Expr) -> Option<Type> {
    match &e.kind {
        ExprKind::CompoundLit { .. } => Some(e.ty.clone()),
        ExprKind::Member {
            base,
            name,
            arrow: false,
            ..
        } => {
            let outer = literal_type_of(base)?;
            let m = outer.record()?.body()?.member(name)?.ty.clone();
            Some(m.qualified(outer.quals()))
        }
        _ => None,
    }
}

/// Where an expression begins, as clang points at it: the left operand of
/// an operator after it, the base of a member, the callee of a call.
fn begin(e: &Expr) -> Pos {
    match &e.kind {
        ExprKind::Binary(_, l, _) | ExprKind::Comma(l, _) | ExprKind::Cond(l, ..) => begin(l),
        ExprKind::Assign { target, .. } => begin(target),
        ExprKind::IncDec {
            prefix: false,
            target,
            ..
        } => begin(target),
        ExprKind::Member { base, .. } => begin(base),
        ExprKind::Call { callee, .. } => begin(callee),
        // `a[i]` is `*(a + i)`, at the `[`.
        ExprKind::Deref(x) if x.pos == e.pos => begin(x),
        ExprKind::Cast {
            x, implicit: true, ..
        } => begin(x),
        _ => e.pos,
    }
}

/// Where clang reports an initializer that is not a constant, `e` (its
/// `isConstantInitializer` blames the expression as a whole, but for what
/// it looks through): at its operator, a member at its name, and anything
/// else, a conversion too, where it begins.
fn culprit(e: &Expr) -> Pos {
    match &e.kind {
        ExprKind::Cast {
            implicit: true, x, ..
        } => begin(x),
        // `a[i]` is `*(a + i)`, at the `[`.
        ExprKind::Deref(x) if x.pos == e.pos => begin(e),
        ExprKind::Binary(..)
        | ExprKind::Comma(..)
        | ExprKind::Assign { .. }
        | ExprKind::IncDec { .. }
        | ExprKind::Unary(..)
        | ExprKind::Addr(_)
        | ExprKind::Deref(_)
        | ExprKind::Cast { .. } => e.pos,
        ExprKind::Member { at, .. } => *at,
        _ => begin(e),
    }
}

/// clang's warning of the `;` at `pos` that is the empty body of `what`
/// (`while loop`), with its note.
fn empty_body(pos: Pos, what: &str) -> Error {
    Error::warning(pos, format!("{what} has empty body"), "empty-body").note(
        pos,
        "put the semicolon on a separate line to silence this warning",
    )
}

/// The initializer of a member a braced list leaves out: zero.
fn zero(ty: &Type, pos: Pos) -> Init {
    if ty.is_scalar() {
        Init::Expr(Expr {
            kind: ExprKind::Num(0),
            ty: ty.unqual().clone(),
            pos,
        })
    } else {
        Init::List(Vec::new(), pos)
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
            spelled: Spelled::default(),
        },
        ty: to.clone(),
        pos,
    };
}

/// clang's note on an earlier declaration a new one clashes with, at
/// `pos`: `previous definition is here` if it was a definition (of a
/// variable, a tentative one too), otherwise `previous declaration is here`.
fn previous(pos: Pos, definition: bool) -> (Pos, &'static str) {
    let what = if definition {
        "previous definition is here"
    } else {
        "previous declaration is here"
    };
    (pos, what)
}

/// clang's error for a call with too many or too few arguments (`args`)
/// for the function type `ft`, if it has: too many at the first one too
/// many, too few at the call's `)` (`close`). A function called by name
/// (`sig`) gets clang's note on its declaration, and when it has one
/// parameter to give, a message naming it.
fn arity_error(ft: &FuncType, args: &[Expr], sig: Option<(&str, &FuncSig)>, close: Pos) -> Option<Error> {
    let (have, arity) = (args.len(), ft.params.len());
    // The one parameter of a function called by name, if it is named.
    let single = || {
        let (name, _) = sig?.1.params.first()?;
        (!name.is_empty()).then_some(name)
    };
    let e = if have > arity && !ft.variadic {
        let msg = match single().filter(|_| arity == 1) {
            Some(name) => format!(
                "too many arguments to function call, expected single argument '{name}', have {have} arguments"
            ),
            None => format!("too many arguments to function call, expected {arity}, have {have}"),
        };
        Error::new(begin(&args[arity]), msg)
    } else if have < arity {
        let msg = match (single().filter(|_| arity == 1), ft.variadic) {
            (Some(name), false) => {
                format!("too few arguments to function call, single argument '{name}' was not specified")
            }
            (Some(name), true) => {
                format!("too few arguments to function call, at least argument '{name}' must be specified")
            }
            (None, false) => format!("too few arguments to function call, expected {arity}, have {have}"),
            (None, true) => {
                format!("too few arguments to function call, expected at least {arity}, have {have}")
            }
        };
        Error::new(close, msg)
    } else {
        return None;
    };
    Some(match sig {
        Some((name, sig)) => e.note(sig.pos, format!("'{name}' declared here")),
        None => e,
    })
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
    ty: FuncType,
    is_static: bool,
    defined: bool,
    /// Every declaration so far says `inline` (without `extern`).
    inline_only: bool,
    /// Where the latest declaration is, the one a call names (clang's
    /// `'f' declared here`), and where the definition is, if any.
    pos: Pos,
    def: Option<Pos>,
    /// The parameters of the latest declaration, their names (empty for one
    /// it leaves unnamed) and where they are, which clang's messages and
    /// notes on a call name.
    params: Vec<(String, Pos)>,
}

struct GlobalVar {
    ty: Type,
    is_static: bool,
    /// Where the latest declaration is, and whether it is a definition
    /// (not `extern`: a tentative definition is one), for clang's notes.
    pos: Pos,
    defines: bool,
    /// The declarator with an initializer, as (item, declarator) indices,
    /// and where it is.
    init: Option<(usize, usize)>,
    init_pos: Option<Pos>,
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
        let Type::Func(mut ty) = f.ty() else {
            unreachable!("a function's type")
        };
        ty.unprototyped = crate::types::Unprototyped(!f.prototyped);
        let sig = FuncSig {
            ty: *ty,
            is_static: f.is_static,
            defined: f.body.is_some(),
            inline_only: f.is_inline && !f.is_extern,
            pos: f.pos,
            def: f.body.is_some().then_some(f.pos),
            params: f.params.iter().map(|p| (p.name.clone(), p.pos)).collect(),
        };
        // Functions are visible from their declaration onwards, so a function can
        // call itself and anything declared above it (use a prototype for mutual
        // recursion).
        match self.globals.get_mut(&f.name) {
            Some(Global::Var(prev)) => {
                // clang's note on a declaration of another kind is always this.
                let note = previous(prev.pos, true);
                let msg = format!("redefinition of '{}' as different kind of symbol", f.name);
                self.errors.push(Error::new(f.pos, msg).note(note.0, note.1));
            }
            Some(Global::Func(prev)) => {
                // clang's note names the latest declaration before this one,
                // or the definition when this is one more.
                let note = previous(prev.pos, prev.def == Some(prev.pos));
                let problem = if prev.ty != sig.ty {
                    Some((format!("conflicting types for '{}'", f.name), note))
                } else if let (Some(def), true) = (prev.def, sig.defined) {
                    Some((format!("redefinition of '{}'", f.name), previous(def, true)))
                } else if sig.is_static && !prev.is_static {
                    let msg = format!(
                        "static declaration of '{}' follows non-static declaration",
                        f.name
                    );
                    Some((msg, note))
                } else {
                    None
                };
                match problem {
                    Some((msg, (at, what))) => self.errors.push(Error::new(f.pos, msg).note(at, what)),
                    None => {
                        prev.defined |= sig.defined;
                        prev.def = prev.def.or(sig.def);
                        prev.inline_only &= sig.inline_only;
                        prev.pos = sig.pos;
                        prev.params = sig.params;
                    }
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
            // A declaration names each parameter once too.
            let mut seen: HashMap<&str, Pos> = HashMap::new();
            for p in f.params.iter().filter(|p| !p.name.is_empty()) {
                if let Some(prev) = seen.insert(&p.name, p.pos) {
                    let msg = format!("redefinition of parameter '{}'", p.name);
                    self.errors
                        .push(Error::new(p.pos, msg).note(prev, "previous declaration is here"));
                }
            }
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
            constant: false,
            address_taken: false,
            errors: &mut self.errors,
            pending: Vec::new(),
            unevaluated: 0,
            xlen: self.xlen,
        };
        for p in &f.params {
            c.declare(&p.name, &p.ty, p.pos, "parameter ");
        }
        // A big struct result goes where the caller says: keep that address.
        f.ret_ptr = (ret_complete && abi::returns_in_memory(&f.ret, self.xlen))
            .then(|| c.temp(&Type::ptr(f.ret.clone()), "result address", f.pos));
        // The body's outermost block shares the parameters' scope, as in C.
        c.block(body);
        for (name, pos) in std::mem::take(&mut c.gotos) {
            if !c.labels.contains_key(&name) {
                c.err(pos, format!("use of undeclared label '{name}'"));
            }
        }
        let pending = std::mem::take(&mut c.pending);
        c.errors.extend(pending);
        f.locals = c.locals;
    }

    fn global_var(&mut self, d: &mut Declarator, at: (usize, usize)) {
        if d.ty.is_void() {
            return self.err(d.pos, "variable has incomplete type 'void'");
        }
        if d.init.is_some() && d.storage == Storage::Extern {
            // clang warns, and the declaration is the variable's definition,
            // as C99 6.9.2p1 has it.
            self.errors.push(Error::warning(
                d.pos,
                "'extern' variable has an initializer",
                "extern-initializer",
            ));
            d.storage = Storage::None;
        }
        d.var = Some(VarRef::Global(d.name.clone()));
        // As in C, the name is in scope from the end of its declarator, so it
        // is declared before its initializer is checked: `void *self = &self;`.
        let is_static = d.storage == Storage::Static;
        let tentative = (d.storage != Storage::Extern).then_some(at);
        let init = d.init.is_some().then_some(at);
        match self.globals.get_mut(&d.name) {
            Some(Global::Func(prev)) => {
                let (at, what) = previous(prev.pos, true);
                let msg = format!("redefinition of '{}' as different kind of symbol", d.name);
                return self.errors.push(Error::new(d.pos, msg).note(at, what));
            }
            Some(Global::Var(prev)) => {
                let note = previous(prev.pos, prev.defines);
                let problem = if !compatible(&prev.ty, &d.ty) {
                    let msg = format!(
                        "redefinition of '{}' with a different type: '{}' vs '{}'",
                        d.name, d.ty, prev.ty
                    );
                    Some((msg, note))
                } else if let (Some(first), true) = (prev.init_pos, init.is_some()) {
                    Some((format!("redefinition of '{}'", d.name), previous(first, true)))
                } else if is_static && !prev.is_static {
                    let msg = format!(
                        "static declaration of '{}' follows non-static declaration",
                        d.name
                    );
                    Some((msg, note))
                } else if d.storage == Storage::None && prev.is_static {
                    let msg = format!(
                        "non-static declaration of '{}' follows static declaration",
                        d.name
                    );
                    Some((msg, note))
                } else {
                    None
                };
                if let Some((msg, (at, what))) = problem {
                    return self.errors.push(Error::new(d.pos, msg).note(at, what));
                }
                // `int a[3]; int a[] = {1};` declares an `int [3]`.
                if d.ty.is_array() && d.ty.size(self.xlen).is_none() {
                    d.ty = prev.ty.clone();
                }
                if !d.ty.is_array() || d.ty.size(self.xlen).is_some() {
                    prev.ty = d.ty.clone();
                }
                prev.init = prev.init.or(init);
                prev.init_pos = prev.init_pos.or(init.map(|_| d.pos));
                prev.tentative = prev.tentative.or(tentative);
                prev.pos = d.pos;
                prev.defines = d.storage != Storage::Extern;
            }
            None => {
                self.globals.insert(
                    d.name.clone(),
                    Global::Var(GlobalVar {
                        ty: d.ty.clone(),
                        is_static,
                        pos: d.pos,
                        defines: d.storage != Storage::Extern,
                        init,
                        init_pos: init.map(|_| d.pos),
                        tentative,
                    }),
                );
            }
        }
        let Some(d_init) = &mut d.init else { return };
        let mut fc = FnChecker::file_scope(&self.globals, &mut self.errors, self.literals.len(), self.xlen);
        let checked = fc.init_object(&mut d.ty, d_init, true, d.pos);
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

    /// Checks the expressions of the file's scope the parser evaluated away
    /// that come before item `before`, from `from` on; returns where the
    /// rest start.
    fn unevaluated(&mut self, us: &mut [(usize, Unevaluated)], from: usize, before: usize) -> usize {
        let to = from + us[from..].iter().take_while(|(n, _)| *n <= before).count();
        if to > from {
            let mut fc =
                FnChecker::file_scope(&self.globals, &mut self.errors, self.literals.len(), self.xlen);
            fc.unevaluated(us[from..to].iter_mut().map(|(_, u)| u));
        }
        to
    }

    /// Checks what the parser evaluated away of a declarator or function
    /// at file scope ([`Unevaluated`]).
    fn file_unevaluated(&mut self, us: &mut [Unevaluated]) {
        if !us.is_empty() {
            FnChecker::file_scope(&self.globals, &mut self.errors, self.literals.len(), self.xlen)
                .unevaluated(us);
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
            let mut ty = ty;
            match &mut ty {
                // `int a[];` and nothing more: C99 6.9.2p5's example, an
                // array of one element, with clang's warning.
                Type::Array(elem, n @ None) if elem.size(self.xlen).is_some() => {
                    self.errors.push(Error::warning(
                        d.pos,
                        "tentative array definition assumed to have one element",
                        "tentative-definition-array",
                    ));
                    *n = Some(1);
                }
                t if t.size(self.xlen).is_none() => {
                    let msg = format!("tentative definition has type '{t}' that is never completed");
                    let mut e = Error::new(d.pos, msg);
                    if let Some(r) = t.record() {
                        e = e.note(r.pos, format!("forward declaration of '{}'", r.name()));
                    }
                    self.errors.push(e);
                }
                _ => {}
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
    /// What converts what, as clang words it: `from` is the source type
    /// as messages name it, quotes included.
    fn phrase(self, to: &Type, from: &str) -> String {
        match self {
            Conv::Assign => format!("assigning to '{to}' from {from}"),
            Conv::Init => format!("initializing '{to}' with an expression of type {from}"),
            Conv::Arg => format!("passing {from} to parameter of type '{to}'"),
            Conv::Return => format!("returning {from} from a function with result type '{to}'"),
        }
    }

    /// clang's message for a conversion between types C does not convert
    /// between at all (a struct and anything else, `void`).
    fn incompatible(self, to: &Type, from: &str) -> String {
        match self {
            Conv::Assign => format!("assigning to '{to}' from incompatible type {from}"),
            Conv::Init => format!("initializing '{to}' with an expression of incompatible type {from}"),
            Conv::Arg => format!("passing {from} to parameter of incompatible type '{to}'"),
            Conv::Return => format!("returning {from} from a function with incompatible result type '{to}'"),
        }
    }
}

/// The fix clang suggests after a conversion it rejects, of the value of
/// `e` with type `from` to `to` (its `ConversionFixItGenerator`): `;
/// dereference with *` when `from` is a pointer to what `to` is, `; take
/// the address with &` when `e` is an lvalue and `to` points to what
/// `from` is; in either case, the target at least as qualified.
fn conversion_hint(e: &Expr, from: &Type, to: &Type) -> &'static str {
    // `to` is `from` but for qualifiers it has more of.
    let fits = |to: &Type, from: &Type| to.unqual() == from.unqual() && to.quals().contains(from.quals());
    match (from.unqual(), to.unqual()) {
        (Type::Ptr(p), _) if fits(to, p) => "; dereference with *",
        (_, Type::Ptr(p)) if is_lvalue(e) && fits(p, from) => "; take the address with &",
        _ => "",
    }
}

/// clang's error for assigning to `target` (at `at`) whose struct or
/// union type has a `const` member, at any depth: the first such member,
/// breadth first, with a note on each (`nested` below the top), as clang's
/// `DiagnoseRecursiveConstFields` has it; `None` if there is none.
fn const_fields(target: &Expr, at: Pos) -> Option<Error> {
    let whose = match &target.kind {
        ExprKind::Var { name, .. } => format!("variable '{name}'"),
        ExprKind::Member { name, .. } => format!("non-static data member '{name}'"),
        _ => "lvalue".to_string(),
    };
    let mut records = vec![target.ty.record()?.clone()];
    let mut error: Option<Error> = None;
    let mut next = 0;
    while next < records.len() {
        let nested = if next > 0 { "nested " } else { "" };
        let body = records[next].body().cloned();
        for m in body.iter().flat_map(|b| &b.members) {
            // A member's type, an array's element type for an array.
            let mut elem = &m.ty;
            while let Type::Array(t, _) = elem {
                elem = t;
            }
            if elem.quals().konst {
                let err = error.take().unwrap_or_else(|| {
                    let msg = format!(
                        "cannot assign to {whose} with {nested}const-qualified data member '{}'",
                        m.name
                    );
                    Error::new(at, msg)
                });
                error = Some(err.note(
                    m.pos,
                    format!("{nested}data member '{}' declared const here", m.name),
                ));
            }
            if let Some(r) = m.ty.record() {
                if !records.iter().any(|seen| std::sync::Arc::ptr_eq(seen, r)) {
                    records.push(r.clone());
                }
            }
        }
        next += 1;
    }
    error
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
    /// Checking an initializer that must be constant (a global's, a static
    /// local's): a compound literal there whose value, not its object, is
    /// what counts is folded into that value (clang's constant evaluator
    /// reads it), as [`FnChecker::folded_literal`] says.
    constant: bool,
    /// The expression being checked is the operand of `&` (or the base of a
    /// member that is): it is an object, not a value.
    address_taken: bool,
    errors: &'a mut Vec<Error>,
    /// The warnings of what the code does when it runs, which clang gives
    /// at the end of the function ([`FnChecker::runtime`]).
    pending: Vec<Error>,
    /// How many operands deep in what is never evaluated (`sizeof`'s,
    /// what the parser evaluated away) the checker is.
    unevaluated: usize,
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
            constant: false,
            address_taken: false,
            errors,
            pending: Vec::new(),
            unevaluated: 0,
            xlen,
        }
    }

    fn err(&mut self, pos: Pos, msg: impl Into<String>) -> Reported {
        self.errors.push(Error::new(pos, msg));
        Reported
    }

    /// A warning of what the code does when it runs (dividing by zero,
    /// say), which clang's `DiagRuntimeBehavior` gives: never for what is
    /// not evaluated (`sizeof`'s operand), and in a function at its end,
    /// after its other diagnostics.
    fn runtime(&mut self, e: Error) {
        if self.unevaluated > 0 {
        } else if self.func.is_empty() {
            self.errors.push(e);
        } else {
            self.pending.push(e);
        }
    }

    /// The expressions the parser evaluated away at one place
    /// ([`Unevaluated`]), in order: each checked as the operand of `sizeof`
    /// is, never evaluated, so a compound literal in it is no object, with
    /// the parameters before it in scope if it was in a parameter list. One
    /// the parser could not type made its constant an error, reported here
    /// if the expression has none of its own. A declarator's warning is
    /// given if none of its operands has an error.
    fn unevaluated<'u>(&mut self, us: impl IntoIterator<Item = &'u mut Unevaluated>) {
        let mut clean = Vec::new();
        for u in us {
            let (literals, locals) = (self.literals.len(), self.locals.len());
            let params = !u.params.is_empty();
            if params {
                // C's prototype scope: what the parameter list declared.
                self.scopes.push(HashMap::new());
                for (name, ty, pos) in &u.params {
                    self.declare(name, ty, *pos, "parameter ");
                }
            }
            self.unevaluated += 1;
            let checked = self.expr(&mut u.expr);
            self.unevaluated -= 1;
            if params {
                self.scopes.pop();
            }
            self.literals.truncate(literals);
            self.locals.truncate(locals);
            let ok = checked.is_ok() && u.untyped.is_none();
            if let (Ok(()), Some((pos, msg))) = (checked, &u.untyped) {
                self.err(*pos, msg.clone());
            }
            clean.push(ok);
            if let Some(said) = &u.diagnostic {
                if clean[clean.len() - 1 - u.with..].iter().all(|&ok| ok) {
                    self.errors.push(said.clone());
                }
            }
        }
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
        // An unnamed parameter (`int f(int) { ... }`) is no name.
        if name.is_empty() {
            return;
        }
        let scope = self.scopes.last_mut().expect("at least one scope");
        if let Some((_, _, prev)) = scope.insert(name.to_string(), (var, ty.clone(), pos)) {
            // clang's note on a parameter is of its declaration, but a local
            // of the body's outermost block clashes with its definition.
            let note = previous(prev, what.is_empty());
            let e = Error::new(pos, format!("redefinition of {what}'{name}'")).note(note.0, note.1);
            self.errors.push(e);
            if !what.is_empty() && !self.func.is_empty() {
                self.errors.push(Error::warning(
                    pos,
                    "omitting the parameter name in a function definition is a C23 extension",
                    "c23-extensions",
                ));
            }
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

    /// The statements of a block, then clang's warnings of its loops'
    /// empty bodies, which it gives at the block's end.
    fn block(&mut self, body: &mut [Stmt]) {
        for s in body.iter_mut() {
            self.stmt(s);
        }
        for pair in body.windows(2) {
            self.empty_loop_body(&pair[0], &pair[1]);
        }
    }

    /// clang's `-Wempty-body` for a `while` or `for` loop `s` whose body
    /// is a `;` on the line of its `)`, followed by `next`, which looks
    /// like the body meant: a block, or a statement further right than
    /// the loop (`for (...); a[i] = 0;`), as its `DiagnoseEmptyLoopBody`
    /// has it. `for (;;);` alone is an idiom.
    fn empty_loop_body(&mut self, s: &Stmt, next: &Stmt) {
        let (body, what) = match &s.kind {
            StmtKind::While { body, .. } => (body, "while loop"),
            StmtKind::For { body, .. } => (body, "for loop"),
            _ => return,
        };
        let typo = matches!(next.kind, StmtKind::Block(_)) || next.pos.col > s.pos.col;
        if typo && matches!(body.kind, StmtKind::Empty { glued: true }) {
            self.errors.push(empty_body(body.pos, what));
        }
    }

    fn stmt(&mut self, s: &mut Stmt) {
        match &mut s.kind {
            StmtKind::Block(body) => self.scoped(|c| c.block(body)),
            StmtKind::Decl(decls) => {
                for d in decls {
                    self.local_decl(d);
                }
            }
            StmtKind::Expr(e) => {
                let _ = self.expr(e);
            }
            StmtKind::Return(e) => self.ret(e.as_mut(), s.pos),
            StmtKind::Empty { .. } => {}
            StmtKind::Unevaluated(us) => self.unevaluated(us),
            // clang reports a condition that is no scalar at the statement's
            // keyword.
            StmtKind::If { cond, then, els } => {
                let _ = self.condition(cond, s.pos);
                self.scoped(|c| c.stmt(then));
                if let Some(e) = els {
                    self.scoped(|c| c.stmt(e));
                } else if matches!(then.kind, StmtKind::Empty { glued: true }) {
                    self.errors.push(empty_body(then.pos, "if statement"));
                }
            }
            StmtKind::While { cond, body } => {
                let _ = self.condition(cond, s.pos);
                self.in_loop(|c| c.scoped(|c| c.stmt(body)));
            }
            StmtKind::DoWhile { body, cond } => {
                self.in_loop(|c| c.scoped(|c| c.stmt(body)));
                let _ = self.condition(cond, s.pos);
            }
            StmtKind::For {
                init,
                cond,
                step,
                body,
                unevaluated,
            } => self.scoped(|c| {
                if let Some(i) = init {
                    c.stmt(i);
                }
                c.unevaluated(unevaluated);
                if let Some(e) = cond {
                    let _ = c.condition(e, s.pos);
                }
                if let Some(e) = step {
                    let _ = c.expr(e);
                }
                c.in_loop(|c| c.scoped(|c| c.stmt(body)));
            }),
            StmtKind::Break if self.break_depth == 0 => {
                self.err(s.pos, "'break' statement not in loop or switch statement");
            }
            StmtKind::Continue if self.loop_depth == 0 => {
                self.err(s.pos, "'continue' statement not in loop statement");
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
                            self.err(s.pos, msg);
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
                if matches!(body.kind, StmtKind::Empty { glued: true }) {
                    self.errors.push(empty_body(body.pos, "switch statement"));
                }
                let ctx = self.switches.pop().expect("pushed above");
                *cases = ctx.cases;
                *default = ctx.default.map(|(id, _)| id);
            }
            StmtKind::Case {
                expr,
                value,
                id,
                body,
                ..
            } => {
                let pos = s.pos;
                let checked = self.expr(expr);
                if self.switches.is_empty() {
                    self.err(pos, "'case' statement not in switch statement");
                } else if checked.is_ok() {
                    match consteval::int(expr, self.xlen).filter(|_| expr.ty.is_integer()) {
                        None => {
                            self.err(expr.pos, "expression is not an integer constant expression");
                        }
                        Some(v) => {
                            *id = self.next_case;
                            self.next_case += 1;
                            let sw = self.switches.last_mut().expect("checked above");
                            *value = sw.kind.wrap(v, self.xlen);
                            if let Some(prev) = sw.seen.insert(*value, expr.pos) {
                                let msg = format!("duplicate case value '{}'", *value);
                                let e = Error::new(expr.pos, msg).note(prev, "previous case defined here");
                                self.errors.push(e);
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
                        let e = Error::new(pos, "multiple default labels in one switch")
                            .note(*prev, "previous case defined here");
                        self.errors.push(e);
                    }
                    Some(sw) => sw.default = Some((*id, pos)),
                }
                self.stmt(body);
            }
            // clang defines a label after the statement it labels, so of
            // `a: a: ;` the first is the one defined again.
            StmtKind::Label { name, body } => {
                self.stmt(body);
                if let Some(prev) = self.labels.insert(name.clone(), s.pos) {
                    let msg = format!("redefinition of label '{name}'");
                    let e = Error::new(s.pos, msg).note(prev, "previous definition is here");
                    self.errors.push(e);
                }
            }
            StmtKind::Goto(name, at) => self.gotos.push((name.clone(), *at)),
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
                    // `return v();` of a `void` function is C99's too.
                    if !e.ty.is_void() {
                        let msg = format!("void function '{}' should not return a value", self.func);
                        self.errors.push(Error::new(pos, msg).flagged("return-mismatch"));
                    }
                } else if self.convert(e, ret, Conv::Return, begin(e)).is_ok() {
                    coerce(e, ret);
                }
            }
            None if *ret != Type::Void => {
                let msg = format!("non-void function '{}' should return a value", self.func);
                self.errors.push(Error::new(pos, msg).flagged("return-mismatch"));
            }
            None => {}
        }
    }

    /// The condition of a statement (`if`, a loop) at `at`, which must be
    /// a scalar.
    fn condition(&mut self, e: &mut Expr, at: Pos) -> Checked {
        self.expr(e)?;
        if !e.ty.decay().is_scalar() {
            let msg = format!(
                "statement requires expression of scalar type ('{}' invalid)",
                e.ty
            );
            return Err(self.err(at, msg));
        }
        Ok(())
    }

    /// The first operand of `?:`, which must be a scalar.
    fn cond_operand(&mut self, e: &mut Expr) -> Checked {
        self.expr(e)?;
        let t = e.ty.decay();
        if !t.is_scalar() {
            let msg = format!("used type '{t}' where arithmetic or pointer type is required");
            return Err(self.err(begin(e), msg));
        }
        Ok(())
    }

    /// A declarator in a block: what the parser evaluated away of it before
    /// its name is declared, of its initializer after.
    fn local_decl(&mut self, d: &mut Declarator) {
        self.unevaluated(&mut d.sized);
        self.local_var(d);
        self.unevaluated(&mut d.unevaluated);
    }

    fn local_var(&mut self, d: &mut Declarator) {
        // A block's `extern` declaration, or a function's: in the block the
        // name is the object or function outside it, which it defines
        // nothing of.
        if d.storage == Storage::Extern || d.ty.is_func() {
            let var = VarRef::Global(d.name.clone());
            self.bind(&d.name, var.clone(), &d.ty, d.pos, "");
            d.var = Some(var);
            if d.init.is_some() {
                let msg = "declaration of block scope identifier with linkage cannot have an initializer";
                self.err(d.pos, msg);
            }
            return;
        }
        if d.ty.is_void() {
            self.err(d.pos, "variable has incomplete type 'void'");
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
                if self.init_object(&mut d.ty, init, true, d.pos).is_err() {
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
                if self.init_object(&mut d.ty, init, false, d.pos).is_err() {
                    return;
                }
                // The initializer may have completed an array's size.
                self.locals[id.0].ty = d.ty.clone();
                let binding = (VarRef::Local(id), d.ty.clone(), d.pos);
                self.scopes.last_mut().unwrap().insert(d.name.clone(), binding);
            }
        }
        if d.ty.size(self.xlen).is_none() {
            self.incomplete_variable(d.pos, &d.ty);
        }
    }

    /// [`Self::init`] of a declared object (declared at `at`), which a
    /// compound literal of its type may initialize as its list would (a GNU
    /// extension clang takes, [`Self::init_by_literal`]).
    fn init_object(&mut self, ty: &mut Type, init: &mut Init, constant: bool, at: Pos) -> Checked {
        match self.init_by_literal(ty, init, constant, at) {
            Some(checked) => checked,
            None => match init {
                Init::Expr(e) => self.init_expr(ty, e, constant, false, Some(at)),
                _ => self.init(ty, init, constant, Some(at)),
            },
        }
    }

    /// An array or struct initialized by a compound literal of its type,
    /// `int a[2] = (int[2]){1, 2};`, or by a member of one: it is
    /// initialized by the literal's list, as clang has it, which emits no
    /// object for the literal. An array only by an array of its own type
    /// (`int a[] = (int[]){1, 2}` takes the literal's size), anywhere; a
    /// struct where its initializer must be constant (elsewhere it is
    /// copied as any struct value is). `None` if `init` is no such
    /// literal; the list replaces it, checked, otherwise.
    fn init_by_literal(
        &mut self,
        ty: &mut Type,
        init: &mut Init,
        constant: bool,
        at: Pos,
    ) -> Option<Checked> {
        let Init::Expr(e) = init else { return None };
        let applies = match ty {
            Type::Array(..) => matches!(e.kind, ExprKind::CompoundLit { .. }) && e.ty.is_array(),
            Type::Record(_) => constant && literal_type_of(e).is_some_and(|t| t.is_record()),
            _ => false,
        };
        if !applies {
            return None;
        }
        let (lt, list) = match self.literal_value(e, constant) {
            Ok(Some(found)) => found,
            Ok(None) => return None,
            Err(r) => return Some(Err(r)),
        };
        let fits = match (&*ty, &lt) {
            (Type::Array(elem, n), Type::Array(lelem, ln)) => elem == lelem && (n.is_none() || n == ln),
            (t, lt) => t.unqual() == lt.unqual(),
        };
        if !fits {
            let msg = if ty.is_array() {
                format!("cannot initialize array of type '{ty}' with array of type '{lt}'")
            } else {
                format!("initializing '{ty}' with an expression of incompatible type '{lt}'")
            };
            return Some(Err(self.err(at, msg)));
        }
        if ty.is_array() {
            *ty = lt;
        }
        *init = list;
        Some(Ok(()))
    }

    /// The type and the checked, fully braced list of a compound literal
    /// `e`, or of a member of one (`(struct u){{1, 2}, 3}.s`, where a
    /// constant is needed): what initializes it; or `None` for what is no
    /// such thing, a member of a union other than the one its list gives,
    /// which clang does not read either. `e` is left as it is, for what
    /// takes it as an object then.
    fn literal_value(&mut self, e: &mut Expr, constant: bool) -> Result<Option<(Type, Init)>, Reported> {
        match &mut e.kind {
            ExprKind::CompoundLit { init, .. } => {
                let mut ty = e.ty.clone();
                self.literal_type(&ty, e.pos)?;
                let mut list = (**init).clone();
                self.init(&mut ty, &mut list, constant, Some(e.pos))?;
                Ok(Some((ty, list)))
            }
            ExprKind::Member {
                base,
                name,
                arrow: false,
                ..
            } if constant => {
                let Some((bt, list)) = self.literal_value(base, constant)? else {
                    return Ok(None);
                };
                let pos = e.pos;
                let Some(record) = bt.record().cloned() else {
                    return Ok(None);
                };
                let Some(body) = record.body() else {
                    let msg = format!("incomplete definition of type '{}'", record.name());
                    return Err(self.err(pos, msg));
                };
                let Some(i) = body.members.iter().position(|m| m.name == *name) else {
                    let msg = format!("no member named '{name}' in '{}'", record.name());
                    return Err(self.err(pos, msg));
                };
                let mty = body.members[i].ty.clone().qualified(bt.quals());
                // The fully braced list has an item per member up to the
                // last one given: the rest are zero.
                let Init::List(items, at) = list else {
                    unreachable!("a struct's initializer is a list")
                };
                // A union's list ends with the one member it holds.
                if record.union && i + 1 != items.len().max(1) {
                    return Ok(None);
                }
                let item = items.into_iter().nth(i).unwrap_or_else(|| zero(&mty, at));
                Ok(Some((mty, item)))
            }
            _ => Ok(None),
        }
    }

    /// A compound literal's value where a constant is needed and its
    /// object is not (`(int *){&g}`, `(struct s){1, 2}.b`): the value its
    /// list gives, which `e` becomes, as clang's constant evaluator reads
    /// it; `false` if `e` is no such literal of a scalar type.
    fn folded_literal(&mut self, e: &mut Expr) -> Result<bool, Reported> {
        if !self.constant || !literal_type_of(e).is_some_and(|t| t.is_scalar()) {
            return Ok(false);
        }
        match self.literal_value(e, true)? {
            Some((_, Init::Expr(x))) => {
                *e = x;
                Ok(true)
            }
            Some(_) => unreachable!("a scalar's initializer is an expression"),
            None => Ok(false),
        }
    }

    /// Checks an initializer against the type it initializes, completing an
    /// array of unknown size, and rewrites a list into its fully braced form
    /// (see [`Init::List`]). `constant`: for globals and statics, every
    /// element must be a constant expression. `decl` is where the object
    /// initialized is declared (a compound literal, where its type is), for
    /// clang's errors of its whole initializer; a list in a list has none.
    fn init(&mut self, ty: &mut Type, init: &mut Init, constant: bool, decl: Option<Pos>) -> Checked {
        let Init::List(items, pos) = init else {
            let Init::Expr(e) = init else { unreachable!() };
            return self.init_expr(ty, e, constant, false, None);
        };
        let pos = *pos;
        // `{}` for a scalar (C23's, which clang takes in C99 too) means 0.
        if items.is_empty() && !ty.is_array() && !ty.is_record() {
            *init = Init::Expr(Expr {
                kind: ExprKind::Num(0),
                ty: Type::INT,
                pos,
            });
            let Init::Expr(e) = init else { unreachable!() };
            return self.init_expr(ty, e, constant, false, None);
        }
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
            return self.init_expr(ty, e, constant, false, None);
        }
        if !ty.is_array() && !ty.is_record() {
            if items.iter().any(|i| matches!(i, Init::Designated(..))) {
                let msg =
                    format!("initialization of non-aggregate type '{ty}' with a designated initializer list");
                return Err(self.err(decl.unwrap_or(pos), msg));
            }
            // `{{1}}` for a scalar is `1`, `{1, 2}` is `1`: clang warns.
            if let Init::List(_, at) = &items[0] {
                let at = *at;
                self.errors.push(Error::warning(
                    at,
                    "too many braces around scalar initializer",
                    "many-braces-around-scalar-init",
                ));
                let inner = items.remove(0);
                *init = inner;
                return self.init(ty, init, constant, None);
            }
            self.excess(&mut items[1..], "scalar");
            let first = items.remove(0);
            *init = first;
            return self.init(ty, init, constant, None);
        }
        let mut queue: VecDeque<Pending> = std::mem::take(items)
            .into_iter()
            .map(|init| Pending { init, checked: false })
            .collect();
        let list = self.fill(ty, &mut queue, constant, pos, Fill::Braced)?;
        if !queue.is_empty() {
            let what = match ty.record() {
                Some(r) => r.keyword(),
                None => "array",
            };
            let mut rest: Vec<Init> = queue.into_iter().map(|p| p.init).collect();
            self.excess(&mut rest, what);
        }
        *init = list;
        Ok(())
    }

    /// Items of a braced list past what it initializes (`what`: `array`,
    /// `struct`, `union` or `scalar`): clang's warning at the first, and
    /// they are left out, after their expressions are checked.
    fn excess(&mut self, rest: &mut [Init], what: &str) {
        let Some(first) = rest.first() else { return };
        let at = first.pos();
        for i in rest.iter_mut() {
            if let Init::Expr(e) = i {
                let _ = self.value(e, false);
            }
        }
        self.errors.push(Error::warning(
            at,
            format!("excess elements in {what} initializer"),
            "excess-initializers",
        ));
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
            Designator::Member(name, pos, at) => {
                let (pos, at) = (*pos, *at);
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
                        at,
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
        // A struct initialized by a struct literal's list, where a constant
        // is needed (`{ (struct s){1, 2}, 3 }`).
        if !front.checked && ty.is_record() {
            let at = front.init.pos();
            if let Some(checked) = self.init_by_literal(ty, &mut front.init, constant, at) {
                checked?;
                return Ok(queue.pop_front().expect("not empty").init);
            }
        }
        let elide = match &mut front.init {
            Init::List(..) | Init::Designated(..) => false,
            Init::Expr(e) if ty.is_array() => !(is_char_array(ty) && matches!(e.kind, ExprKind::Str(_))),
            Init::Expr(e) if ty.is_record() => {
                // A struct is initialized by a struct of its type, or member by member.
                if !front.checked {
                    front.checked = true;
                    self.value(e, constant)?;
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
            Init::Expr(e) => self.init_expr(ty, e, constant, checked, None)?,
            Init::List(..) => self.init(ty, &mut init, constant, None)?,
            Init::Designated(..) => unreachable!("fill takes designators"),
        }
        Ok(init)
    }

    /// An object of type `ty` initialized by the expression `e` (already
    /// checked, if `checked`): a string literal for a char array, otherwise
    /// as if by assignment. `decl` is where the declarator is when `e` is
    /// its initializer, not an item of a list: clang reports a conversion
    /// there, and an item where it begins.
    fn init_expr(
        &mut self,
        ty: &mut Type,
        e: &mut Expr,
        constant: bool,
        checked: bool,
        decl: Option<Pos>,
    ) -> Checked {
        let at = decl.unwrap_or_else(|| begin(e));
        match ty {
            Type::Array(elem, n) if is_char_array_elem(elem) => {
                let ExprKind::Str(bytes) = &mut e.kind else {
                    return Err(self.err(
                        at,
                        "array initializer must be an initializer list or string literal",
                    ));
                };
                // The terminating NUL may be left out when the array is exactly
                // as long as the text, as C allows; a longer text is cut to
                // the array's size, with clang's warning.
                match n {
                    None => *n = Some(bytes.len() as u32 + 1),
                    Some(n) if bytes.len() as u32 > *n => {
                        self.errors.push(Error::warning(
                            e.pos,
                            "initializer-string for char array is too long",
                            "excess-initializers",
                        ));
                        bytes.truncate(*n as usize);
                    }
                    Some(_) => {}
                }
                e.ty = Type::array(Type::CHAR, Some(bytes.len() as u32 + 1));
                Ok(())
            }
            // clang's words for an array that is no char array: `int`'s is
            // C99's `wchar_t` on RISC-V, a wide string's element type.
            Type::Array(elem, _) => {
                let wide = **elem == Type::INT;
                let msg = match (&e.kind, wide) {
                    (ExprKind::Str(_), true) => "initializing wide char array with non-wide string literal",
                    (_, true) => "array initializer must be an initializer list or wide string literal",
                    _ => "array initializer must be an initializer list",
                };
                Err(self.err(at, msg))
            }
            _ => {
                if !checked {
                    self.value(e, constant)?;
                }
                self.convert(e, ty, Conv::Init, at)?;
                coerce(e, ty);
                if constant && consteval::eval(e, self.xlen).is_none() {
                    return Err(self.err(culprit(e), "initializer element is not a compile-time constant"));
                }
                Ok(())
            }
        }
    }

    // ---- expressions ----

    /// Checks whether `e`'s value may be converted to `to` as if by
    /// assignment ([`Self::convert_value`]), with clang's diagnostics at
    /// `at`.
    fn convert(&mut self, e: &Expr, to: &Type, how: Conv, at: Pos) -> Checked {
        let null = consteval::is_null_pointer(e, self.xlen);
        let from = e.ty.clone();
        self.convert_value(e, &from, &format!("'{from}'"), null, to, how, at)
    }

    /// An argument converted to its parameter's type `to`: at the argument,
    /// with clang's note on the parameter `param` (its name and where it
    /// is) of a function called by name.
    fn convert_arg(&mut self, e: &Expr, to: &Type, param: Option<&(String, Pos)>) -> Checked {
        let mark = self.errors.len();
        let checked = self.convert(e, to, Conv::Arg, begin(e));
        if let (Some((name, pos)), true) = (param, self.errors.len() > mark) {
            let what = if name.is_empty() {
                "passing argument to parameter here".to_string()
            } else {
                format!("passing argument to parameter '{name}' here")
            };
            let e = self.errors.pop().expect("just reported");
            self.errors.push(e.note(*pos, what));
        }
        checked
    }

    /// Checks whether a value of type `from` (named `named` in messages,
    /// quotes included), the value of `e` (a null pointer constant if
    /// `null`), may be converted to `to` as if by assignment (C99
    /// 6.5.16.1), reporting what clang reports at `at`: arithmetic converts
    /// to arithmetic, a struct to the same struct, a pointer to a pointer
    /// to the same type or to or from `void *` (to or from a function's
    /// too, which clang takes as GNU C does), the null pointer constant to
    /// any pointer, and a pointer to `_Bool`. A pointer that loses
    /// qualifiers of what it points to converts, with clang's warning.
    /// Converting between pointers to different types, or a pointer and an
    /// integer, are the errors clang makes of those warnings, with their
    /// flags.
    #[allow(clippy::too_many_arguments)]
    fn convert_value(
        &mut self,
        e: &Expr,
        from: &Type,
        named: &str,
        null: bool,
        to: &Type,
        how: Conv,
        at: Pos,
    ) -> Checked {
        let to = to.unqual();
        let value = from.decay();
        let (problem, flag) = match (to, &value) {
            (Type::Int(_), Type::Int(_)) => return Ok(()),
            (Type::Int(IntKind::Bool), Type::Ptr(_)) => return Ok(()),
            (Type::Record(a), Type::Record(b)) if a == b => {
                if to.size(self.xlen).is_none() {
                    let msg = format!("incomplete type '{to}' cannot be copied");
                    return Err(self.err(at, msg));
                }
                return Ok(());
            }
            (Type::Ptr(_), Type::Ptr(_) | Type::Int(_)) if null => return Ok(()),
            (Type::Ptr(a), Type::Ptr(b)) => {
                let (au, bu) = (a.unqual(), b.unqual());
                if au.is_func() && bu.is_func() && au != bu {
                    (
                        "incompatible function pointer types",
                        Some("incompatible-function-pointer-types"),
                    )
                } else if au == bu || au.is_void() || bu.is_void() {
                    if !a.quals().contains(b.quals()) {
                        let msg = format!("{} discards qualifiers", how.phrase(to, named));
                        let flag = "incompatible-pointer-types-discards-qualifiers";
                        self.errors.push(Error::warning(at, msg, flag));
                    }
                    return Ok(());
                } else {
                    ("incompatible pointer types", Some("incompatible-pointer-types"))
                }
            }
            (Type::Ptr(_), Type::Int(_)) => (
                "incompatible integer to pointer conversion",
                Some("int-conversion"),
            ),
            (Type::Int(_), Type::Ptr(_)) => (
                "incompatible pointer to integer conversion",
                Some("int-conversion"),
            ),
            _ => ("", None),
        };
        let hint = conversion_hint(e, from, to);
        let msg = if problem.is_empty() {
            format!("{}{hint}", how.incompatible(to, named))
        } else {
            format!("{problem} {}{hint}", how.phrase(to, named))
        };
        let e = Error::new(at, msg);
        match flag {
            // One of the warnings clang makes errors: it converts the value
            // all the same, and goes on with it.
            Some(flag) => {
                self.errors.push(e.flagged(flag));
                Ok(())
            }
            None => {
                self.errors.push(e);
                Err(Reported)
            }
        }
    }

    /// The target of `=`, `op=`, `++` and `--` (whose operator is at `at`,
    /// where clang reports it): a modifiable lvalue (not an array, a
    /// function, or anything `const`).
    fn assignable(&mut self, target: &Expr, at: Pos) -> Checked {
        let ty = &target.ty;
        let e = if !is_lvalue(target) {
            Error::new(at, "expression is not assignable")
        } else if ty.is_func() {
            Error::new(at, format!("non-object type '{ty}' is not assignable"))
        } else if ty.is_array() {
            Error::new(at, format!("array type '{ty}' is not assignable"))
        } else if ty.quals().konst {
            self.const_assignment(target, at)
        } else if let Some(e) = const_fields(target, at) {
            e
        } else {
            return Ok(());
        };
        self.errors.push(e);
        Err(Reported)
    }

    /// clang's error for assigning to a `const` lvalue `target`, as its
    /// `DiagnoseConstAssignment` finds it: it walks from the lvalue to the
    /// variable it is part of, through members (`s.m`, `p->m`), and names
    /// the first thing whose type is `const` (once through a `->`, what
    /// the pointer points to), with a note on each such member or variable;
    /// anything else (`*p`, `a[i]`) is a `read-only variable`.
    fn const_assignment(&self, target: &Expr, at: Pos) -> Error {
        let mut e = target;
        let mut deref = false;
        let mut found: Option<Error> = None;
        let mut notes = Vec::new();
        // Whether a type, once through a `->` if `deref`, is not `const`.
        let modifiable = |t: &Type, deref: bool| match (deref, t.unqual()) {
            (true, Type::Ptr(to)) => !to.quals().konst,
            _ => !t.quals().konst,
        };
        loop {
            match &e.kind {
                ExprKind::Member {
                    base, name, arrow, ..
                } => {
                    let outer = if *arrow {
                        base.ty.decay().pointee().cloned()
                    } else {
                        Some(base.ty.clone())
                    };
                    let member = outer
                        .as_ref()
                        .and_then(Type::record)
                        .and_then(|r| r.body()?.member(name).cloned());
                    if let Some(m) = member.filter(|m| !modifiable(&m.ty, deref)) {
                        let ty = &m.ty;
                        found.get_or_insert_with(|| {
                            let msg =
                                format!("cannot assign to non-static data member '{name}' with const-qualified type '{ty}'");
                            Error::new(at, msg)
                        });
                        notes.push((
                            m.pos,
                            format!("non-static data member '{name}' declared const here"),
                        ));
                    }
                    deref = *arrow;
                    e = base;
                    continue;
                }
                ExprKind::Var { name, .. } => {
                    if let Some((ty, pos)) = self.declared(name) {
                        if !modifiable(&ty, deref) {
                            found.get_or_insert_with(|| {
                                let msg = format!(
                                    "cannot assign to variable '{name}' with const-qualified type '{ty}'"
                                );
                                Error::new(at, msg)
                            });
                            notes.push((pos, format!("variable '{name}' declared const here")));
                        }
                    }
                }
                _ => {}
            }
            break;
        }
        match found {
            Some(err) => notes.into_iter().fold(err, |err, (pos, msg)| err.note(pos, msg)),
            None => Error::new(at, "read-only variable is not assignable"),
        }
    }

    /// The type and the position of the declaration a name in scope refers
    /// to: a local's, or a global's latest.
    fn declared(&self, name: &str) -> Option<(Type, Pos)> {
        match self.lookup(name) {
            Some((_, ty, pos)) => Some((ty.clone(), *pos)),
            None => match self.globals.get(name)? {
                Global::Var(g) => Some((g.ty.clone(), g.pos)),
                Global::Func(f) => Some((Type::Func(Box::new(f.ty.clone())), f.pos)),
            },
        }
    }

    /// clang's error for operands `l` and `r` (named as written) that the
    /// binary operator `op` at `pos` does not take.
    fn invalid_operands(&mut self, op: BinOp, pos: Pos, l: &Type, r: &Type) -> Reported {
        let (lv, rv) = (l.decay(), r.decay());
        match (op, lv.unqual(), rv.unqual()) {
            // The difference of pointers to different types.
            (BinOp::Sub, Type::Ptr(_), Type::Ptr(_))
                if lv.step(self.xlen).is_some() && rv.step(self.xlen).is_some() =>
            {
                self.err(
                    pos,
                    format!("'{lv}' and '{rv}' are not pointers to compatible types"),
                )
            }
            // Arithmetic on a pointer to an incomplete type.
            (BinOp::Add | BinOp::Sub, Type::Ptr(to), Type::Int(_))
            | (BinOp::Add, Type::Int(_), Type::Ptr(to))
            | (BinOp::Sub, Type::Ptr(to), Type::Ptr(_))
                if lv.step(self.xlen).is_none() || rv.step(self.xlen).is_none() =>
            {
                let to = to.clone();
                self.incomplete_arithmetic(pos, &to)
            }
            _ => self.err(
                pos,
                format!("invalid operands to binary expression ('{l}' and '{r}')"),
            ),
        }
    }

    /// clang's error for a variable (or a compound literal) of the
    /// incomplete type `ty` declared at `pos`: an array of unknown size
    /// needs one or an initializer; anything else is incomplete, a struct
    /// with clang's note on its declaration.
    fn incomplete_variable(&mut self, pos: Pos, ty: &Type) -> Reported {
        if matches!(ty, Type::Array(_, None)) {
            return self.err(
                pos,
                "definition of variable with array type needs an explicit size or an initializer",
            );
        }
        let mut e = Error::new(pos, format!("variable has incomplete type '{ty}'"));
        if let Some(r) = ty.record() {
            e = e.note(r.pos, format!("forward declaration of '{}'", r.name()));
        }
        self.errors.push(e);
        Reported
    }

    /// clang's error for arithmetic on a pointer to the incomplete type
    /// `to`, at `pos`, with its note on the type's declaration.
    fn incomplete_arithmetic(&mut self, pos: Pos, to: &Type) -> Reported {
        let mut e = Error::new(
            pos,
            format!("arithmetic on a pointer to an incomplete type '{to}'"),
        );
        if let Some(r) = to.record() {
            e = e.note(r.pos, format!("forward declaration of '{}'", r.name()));
        }
        self.errors.push(e);
        Reported
    }

    /// [`Self::expr`] of an initializer's expression, constant if `constant`
    /// ([`Self::constant`]).
    fn value(&mut self, e: &mut Expr, constant: bool) -> Checked {
        let outer = std::mem::replace(&mut self.constant, constant);
        let checked = self.expr(e);
        self.constant = outer;
        checked
    }

    /// Types `e` and everything in it.
    fn expr(&mut self, e: &mut Expr) -> Checked {
        let pos = e.pos;
        // Only `e` itself is the operand of `&`, not what is inside it.
        let addressed = std::mem::take(&mut self.address_taken);
        if !addressed && self.folded_literal(e)? {
            return Ok(());
        }
        if matches!(e.kind, ExprKind::VaEnd(_) | ExprKind::VaCopy(..)) {
            return self.va_end_or_copy(e);
        }
        let globals = self.globals;
        let ty = match &mut e.kind {
            ExprKind::Num(_) | ExprKind::Enumerator { .. } | ExprKind::Str(_) => return Ok(()),
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
            // clang reports what is wrong with an assignment at its operator.
            ExprKind::Assign { op, target, value } => {
                let (a, b) = (self.expr(target), self.expr(value));
                a?;
                b?;
                self.assignable(target, pos)?;
                match op {
                    None => {
                        self.convert(value, &target.ty, Conv::Assign, pos)?;
                        coerce(value, &target.ty);
                    }
                    Some(op) => {
                        // `t op= v` computes `t op v`, then stores it as `t = ...`
                        // would: clang checks that conversion, of the value
                        // `v` with the type of `t op v`.
                        let (t, v) = (target.ty.decay(), value.ty.decay());
                        let Some(r) = arith_type(*op, &t, &v, self.xlen) else {
                            return Err(self.invalid_operands(*op, pos, &target.ty, &value.ty));
                        };
                        // A pointer difference is clang's `__ptrdiff_t`, as
                        // its messages name it.
                        let named = if t.is_ptr() && v.is_ptr() {
                            format!("'__ptrdiff_t' (aka '{r}')")
                        } else {
                            format!("'{r}'")
                        };
                        self.convert_value(value, &r, &named, false, &target.ty, Conv::Assign, pos)?;
                    }
                }
                // The value of an assignment is what was stored: no qualifiers.
                target.ty.unqual().clone()
            }
            ExprKind::IncDec { inc, target, .. } => {
                self.expr(target)?;
                match target.ty.unqual() {
                    Type::Int(_) => {}
                    Type::Ptr(to) if target.ty.step(self.xlen).is_none() => {
                        return Err(self.incomplete_arithmetic(pos, to));
                    }
                    Type::Ptr(_) => {}
                    _ => {
                        let what = if *inc { "increment" } else { "decrement" };
                        let msg = format!("cannot {what} value of type '{}'", target.ty);
                        return Err(self.err(pos, msg));
                    }
                }
                self.assignable(target, pos)?;
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
                        let msg = format!("invalid argument type '{t}' to unary expression");
                        return Err(self.err(pos, msg));
                    }
                }
            }
            ExprKind::Addr(x) => {
                self.address_taken = true;
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
                    // `*p` of a `void *` is an expression of type `void`, which
                    // clang takes with a warning (it then can only be
                    // discarded).
                    Type::Ptr(t) if t.is_void() && !subscript => {
                        let msg = format!("ISO C does not allow indirection on operand of type '{}'", x.ty);
                        self.errors.push(Error::warning(pos, msg, "void-ptr-dereference"));
                        *t
                    }
                    Type::Ptr(t) => *t,
                    t => {
                        let msg = if subscript {
                            "subscripted value is not an array, pointer, or vector".to_string()
                        } else {
                            format!("indirection requires pointer operand ('{t}' invalid)")
                        };
                        return Err(self.err(pos, msg));
                    }
                }
            }
            // An explicit conversion (C99 6.5.4): to `void`, or between
            // scalars, where clang reports a type that is no scalar, the
            // target at the cast's `(`, the operand where it begins.
            ExprKind::Cast { x, .. } => {
                self.expr(x)?;
                let to = &e.ty;
                let from = x.ty.decay();
                if to.is_void() {
                    return Ok(());
                }
                if !to.is_scalar() {
                    let msg = format!("used type '{to}' where arithmetic or pointer type is required");
                    return Err(self.err(pos, msg));
                }
                if !from.is_scalar() {
                    let msg =
                        format!("operand of type '{from}' where arithmetic or pointer type is required");
                    return Err(self.err(begin(x), msg));
                }
                // A pointer to an integer narrower than it loses the address.
                if let (Type::Int(k), Type::Ptr(_)) = (to.unqual(), &from) {
                    if *k != IntKind::Bool && k.size(self.xlen) < from.size(self.xlen).unwrap_or(0) {
                        let msg = format!("cast to smaller integer type '{to}' from '{from}'");
                        self.errors.push(Error::warning(pos, msg, "pointer-to-int-cast"));
                    }
                }
                return Ok(());
            }
            ExprKind::SizeofType(t) => {
                let t = t.clone();
                return self.sizeof(e, &t, pos);
            }
            ExprKind::SizeofExpr(x, at) => {
                // The operand is typed (and checked) but never evaluated,
                // so a file-scope compound literal in it is no object:
                // drop the static variable checking it made.
                let (mark, at) = (self.literals.len(), *at);
                self.unevaluated += 1;
                let r = self.expr(x);
                self.unevaluated -= 1;
                self.literals.truncate(mark);
                r?;
                let t = x.ty.clone();
                return self.sizeof(e, &t, at);
            }
            ExprKind::Cond(c, a, b) => {
                let (r1, r2, r3) = (self.cond_operand(c), self.expr(a), self.expr(b));
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
                at,
            } => {
                // `&s.m` takes the address of a part of `s`.
                self.address_taken = addressed && !*arrow;
                self.expr(base)?;
                // `p->m` reaches through a pointer; `s.m` names a part of `s`.
                // clang reports what is wrong with the base at the `.` or
                // `->`, a member it cannot find at the member's name.
                let (outer, quals) = match (*arrow, base.ty.decay()) {
                    (true, Type::Ptr(t)) => {
                        let q = t.quals();
                        (*t, q)
                    }
                    (true, t) => {
                        let hint = if t.is_record() {
                            "; did you mean to use '.'?"
                        } else {
                            ""
                        };
                        let msg = format!("member reference type '{t}' is not a pointer{hint}");
                        return Err(self.err(pos, msg));
                    }
                    (false, t @ Type::Ptr(_)) if t.pointee().is_some_and(Type::is_record) => {
                        let msg =
                            format!("member reference type '{t}' is a pointer; did you mean to use '->'?");
                        return Err(self.err(pos, msg));
                    }
                    (false, _) => (base.ty.clone(), base.ty.quals()),
                };
                let Some(record) = outer.record() else {
                    let msg = format!("member reference base type '{outer}' is not a structure or union");
                    return Err(self.err(pos, msg));
                };
                let Some(body) = record.body() else {
                    let msg = format!("incomplete definition of type '{}'", record.name());
                    let note = format!("forward declaration of '{}'", record.name());
                    self.errors.push(Error::new(pos, msg).note(record.pos, note));
                    return Err(Reported);
                };
                let Some(m) = body.member(name) else {
                    let msg = format!("no member named '{name}' in '{}'", record.name());
                    return Err(self.err(*at, msg));
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
                parens: (open, close),
            } => {
                if let ExprKind::Var { name, .. } = &callee.kind {
                    if self.lookup(name).is_none() && !globals.contains_key(name.as_str()) {
                        let msg = format!(
                            "call to undeclared function '{name}'; ISO C99 and later do not support \
                             implicit function declarations"
                        );
                        self.errors
                            .push(Error::new(pos, msg).flagged("implicit-function-declaration"));
                        return Err(Reported);
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
                        return Err(self.err(*open, msg));
                    }
                };
                // The function called by name, whose declaration clang's
                // notes point at and its messages name the parameters of.
                let sig = match callee.direct_callee().map(|n| (n, globals.get(n))) {
                    Some((name, Some(Global::Func(sig)))) => Some((name, sig)),
                    _ => None,
                };
                if let Some(e) = arity_error(&ft, args, sig, *close) {
                    return Err(Reported).inspect_err(|_| self.errors.push(e));
                }
                if !ok {
                    return Err(Reported);
                }
                *arg_copies = vec![None; args.len()];
                for (i, (a, p)) in args.iter_mut().zip(&ft.params).enumerate() {
                    // Each argument converts to its parameter's type as if
                    // assigned, with clang's note on the parameter.
                    let param = sig.and_then(|(_, s)| s.params.get(i));
                    if self.convert_arg(a, p, param).is_err() {
                        ok = false;
                        continue;
                    }
                    coerce(a, p);
                    // A struct too big for registers is passed as the address of a copy.
                    if p.is_record() && abi::classify(p, self.xlen) == abi::Pass::Ref {
                        arg_copies[i] = Some(self.temp(p, "argument copy", a.pos));
                    }
                }
                let arity = ft.params.len();
                ok &= self.variadic_args(&mut args[arity..], &mut arg_copies[arity..]);
                if !ok {
                    return Err(Reported);
                }
                if ft.ret.is_record() {
                    if ft.ret.size(self.xlen).is_none() {
                        let mut e = match sig {
                            Some((name, sig)) => {
                                let msg =
                                    format!("calling '{name}' with incomplete return type '{}'", ft.ret);
                                Error::new(pos, msg).note(sig.pos, format!("'{name}' declared here"))
                            }
                            _ => Error::new(
                                pos,
                                format!("calling function with incomplete return type '{}'", ft.ret),
                            ),
                        };
                        if let Some(r) = ft.ret.record() {
                            e = e.note(r.pos, format!("forward declaration of '{}'", r.name()));
                        }
                        self.errors.push(e);
                        return Err(Reported);
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
            ExprKind::VaArg(ap, at) => {
                let t = e.ty.clone();
                self.va_arg(ap, &t, *at)?;
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
            return Err(self.incomplete_variable(pos, ty));
        }
        Ok(())
    }

    /// `(type){ ... }` in a function: checks the list against the type
    /// (completing an array's size) and gives the object a hidden local.
    fn compound_literal(&mut self, ty: &mut Type, init: &mut Init, pos: Pos) -> Result<LocalId, Reported> {
        self.literal_type(ty, pos)?;
        self.init(ty, init, false, Some(pos))?;
        Ok(self.temp(ty, "compound literal", pos))
    }

    /// `(type){ ... }` at file scope: an object with static storage, so its
    /// list must be constant, like a global's. It becomes a `static`
    /// variable of its own, whose name this returns.
    fn static_literal(&mut self, ty: &mut Type, mut init: Init, pos: Pos) -> Result<String, Reported> {
        self.literal_type(ty, pos)?;
        self.init(ty, &mut init, true, Some(pos))?;
        // Numbered after any literal nested in this one's list.
        let name = format!(".compoundliteral.{}", self.literal_base + self.literals.len());
        self.literals.push(Declarator {
            name: name.clone(),
            ty: ty.clone(),
            storage: Storage::Static,
            init: Some(init),
            var: Some(VarRef::Global(name.clone())),
            define: true,
            spelled: Default::default(),
            pos,
            sized: Vec::new(),
            unevaluated: Vec::new(),
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
            return Err(self.err(pos, "'va_start' used in function with fixed args"));
        };
        self.va_list(ap, true)?;
        self.expr(last)?;
        // merlin does not need it, and clang only warns.
        if !matches!(&last.kind, ExprKind::Var { name, .. } if *name == param) {
            self.errors.push(Error::warning(
                begin(last),
                "second argument to 'va_start' is not the last non-variadic parameter",
                "varargs",
            ));
        }
        Ok(())
    }

    /// `va_arg(ap, t)`, with `t` written at `at`: `t` must be a complete
    /// object type. One the default argument promotions change, or an
    /// array, makes it undefined, which clang warns of (when it runs:
    /// [`Self::runtime`]), and then reads as the type says.
    fn va_arg(&mut self, ap: &mut Expr, t: &Type, at: Pos) -> Checked {
        self.expr(ap)?;
        if ap.ty.decay() != Type::ptr(Type::Void) {
            let msg = format!(
                "first argument to 'va_arg' is of type '{}' and not 'va_list'",
                ap.ty
            );
            return Err(self.err(begin(ap), msg));
        }
        if t.is_void() || t.is_func() || t.size(self.xlen).is_none() {
            let mut e = Error::new(
                at,
                format!("second argument to 'va_arg' is of incomplete type '{t}'"),
            );
            if let Some(r) = t.record() {
                e = e.note(r.pos, format!("forward declaration of '{}'", r.name()));
            }
            self.errors.push(e);
            return Err(Reported);
        }
        let undefined = if t.is_array() {
            Some(format!(
                "second argument to 'va_arg' is of array type '{t}'; this va_arg has undefined behavior \
                 because arguments will never be compatible with array type"
            ))
        } else if t.int_kind().is_some_and(|k| k.promoted() != k) {
            Some(format!(
                "second argument to 'va_arg' is of promotable type '{t}'; this va_arg has undefined \
                 behavior because arguments will be promoted to 'int'"
            ))
        } else {
            None
        };
        if let Some(msg) = undefined {
            self.runtime(Error::warning(at, msg, "varargs"));
        }
        self.assignable(ap, ap.pos)
    }

    /// Checks that `ap`, the first argument of a builtin, is a `va_list`
    /// (an object, which the builtin changes, if `target`). clang's
    /// builtins take it by reference, as its message says.
    fn va_list(&mut self, ap: &mut Expr, target: bool) -> Checked {
        self.expr(ap)?;
        if ap.ty.decay() != Type::ptr(Type::Void) {
            let msg = format!(
                "non-const lvalue reference to type '__builtin_va_list' cannot bind to a value of unrelated \
                 type '{}'",
                ap.ty
            );
            return Err(self.err(begin(ap), msg));
        }
        if target {
            self.assignable(ap, ap.pos)?;
        }
        Ok(())
    }

    /// `va_end(ap)` does nothing: it becomes `(void)ap`. `va_copy(dst,
    /// src)` copies a pointer: `(void)(dst = src)`.
    fn va_end_or_copy(&mut self, e: &mut Expr) -> Checked {
        let pos = e.pos;
        let x = match std::mem::replace(&mut e.kind, ExprKind::Num(0)) {
            ExprKind::VaEnd(mut ap) => {
                self.va_list(&mut ap, false)?;
                ap
            }
            ExprKind::VaCopy(mut dst, mut src) => {
                let (a, b) = (self.va_list(&mut dst, true), self.va_list(&mut src, false));
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
        e.kind = ExprKind::Cast {
            x,
            implicit: false,
            spelled: Spelled::default(),
        };
        e.ty = Type::Void;
        Ok(())
    }

    /// Replaces a `sizeof` with its value, a `size_t`: `unsigned int` on ILP32,
    /// `unsigned long` on LP64.
    /// `void` and a function are 1 byte, a GNU extension clang takes
    /// without a word (as `-Wpointer-arith`, which is not on by default,
    /// would say).
    /// An incomplete type is reported at `at`: the `sizeof` of a type, the
    /// operand of `sizeof expr`.
    fn sizeof(&mut self, e: &mut Expr, t: &Type, at: Pos) -> Checked {
        let size = if t.is_void() || t.is_func() {
            Some(1)
        } else {
            t.size(self.xlen)
        };
        let Some(size) = size else {
            let mut d = Error::new(
                at,
                format!("invalid application of 'sizeof' to an incomplete type '{t}'"),
            );
            if let Some(r) = t.record() {
                d = d.note(r.pos, format!("forward declaration of '{}'", r.name()));
            }
            self.errors.push(d);
            return Err(Reported);
        };
        e.kind = ExprKind::Num(size.into());
        e.ty = Type::Int(IntKind::size_t(self.xlen));
        Ok(())
    }

    fn binary(&mut self, op: BinOp, l: &mut Expr, r: &mut Expr, pos: Pos) -> Result<Type, Reported> {
        let (lt, rt) = (l.ty.decay(), r.ty.decay());
        match op {
            BinOp::LogAnd | BinOp::LogOr => {
                if !lt.is_scalar() || !rt.is_scalar() {
                    return Err(self.invalid_operands(op, pos, &l.ty, &r.ty));
                }
                Ok(Type::INT)
            }
            _ if op.is_comparison() => {
                // clang's warnings come first, of what the operands are,
                // then whether they can be compared at all.
                if same_operand(l, r) {
                    let always = matches!(op, BinOp::Eq | BinOp::Le | BinOp::Ge);
                    let msg = format!("self-comparison always evaluates to {always}");
                    self.errors.push(Error::warning(pos, msg, "tautological-compare"));
                }
                let (lnull, rnull) = (
                    consteval::is_null_pointer(l, self.xlen),
                    consteval::is_null_pointer(r, self.xlen),
                );
                let equality = matches!(op, BinOp::Eq | BinOp::Ne);
                if equality && (lnull || rnull) {
                    let other = if rnull { &*l } else { &*r };
                    if let Some(e) = never_null(other, Some(op == BinOp::Ne)) {
                        self.errors.push(e);
                    }
                }
                let pair = format!("('{lt}' and '{rt}')");
                match (&lt, &rt) {
                    (Type::Int(_), Type::Int(_)) => {}
                    // Pointers to different types compare as addresses,
                    // with clang's warning; `void *` and an object
                    // pointer, or any pointer and a null pointer constant,
                    // without; `void *` and a function pointer, for
                    // equality only.
                    (Type::Ptr(a), Type::Ptr(b)) => {
                        let (au, bu) = (a.unqual(), b.unqual());
                        let void = equality && (au.is_void() || bu.is_void());
                        if au != bu && !void {
                            let msg = format!("comparison of distinct pointer types {pair}");
                            self.errors
                                .push(Error::warning(pos, msg, "compare-distinct-pointer-types"));
                        }
                    }
                    // A pointer and an integer that is no null pointer
                    // constant compare as addresses too, with clang's
                    // warning (one of its own for `<`, `<=`, `>`, `>=`).
                    (Type::Ptr(_), Type::Int(_)) | (Type::Int(_), Type::Ptr(_)) => {
                        if !lnull && !rnull {
                            let e = if equality {
                                let msg = format!("comparison between pointer and integer {pair}");
                                Error::warning(pos, msg, "pointer-integer-compare")
                            } else {
                                Error::warning_unflagged(
                                    pos,
                                    format!("ordered comparison between pointer and integer {pair}"),
                                )
                            };
                            self.errors.push(e);
                            // The integer becomes a pointer, as clang
                            // converts it (a null pointer constant needs no
                            // conversion to compare).
                            let to = if lt.is_ptr() { lt.clone() } else { rt.clone() };
                            coerce(l, &to);
                            coerce(r, &to);
                        }
                    }
                    _ => return Err(self.invalid_operands(op, pos, &l.ty, &r.ty)),
                }
                Ok(Type::INT)
            }
            // Named as written, before arrays decay, as clang names them.
            _ => arith_type(op, &lt, &rt, self.xlen)
                .ok_or_else(|| self.invalid_operands(op, pos, &l.ty, &r.ty)),
        }
    }

    /// The type of `c ? a : b` (C99 6.5.15) at `pos` (its `?`), converting
    /// both branches to it. What C99 has no type for, clang gives one, with
    /// a warning: pointers to different types make a `void *`, a pointer
    /// and an integer the pointer; `void` and anything, `void`.
    fn cond_type(&mut self, a: &mut Expr, b: &mut Expr, pos: Pos) -> Result<Type, Reported> {
        let (at, bt) = (a.ty.decay(), b.ty.decay());
        let ty = match (&at, &bt) {
            (Type::Int(x), Type::Int(y)) => Type::Int(common(*x, *y, self.xlen)),
            (Type::Void, _) | (_, Type::Void) => Type::Void,
            (Type::Record(x), Type::Record(y)) if x == y => at.clone(),
            // A null pointer constant takes the other pointer's type, even
            // when it is `(void *)0` (C99 6.5.15p6).
            (Type::Ptr(_), Type::Ptr(_)) if consteval::is_null_pointer(a, self.xlen) => bt.clone(),
            (Type::Ptr(_), Type::Ptr(_)) if consteval::is_null_pointer(b, self.xlen) => at.clone(),
            // Otherwise both point to the same type (or one to void), and
            // the result points to it with every qualifier of either side.
            (Type::Ptr(x), Type::Ptr(y)) => {
                let q = x.quals().union(y.quals());
                let to = if x.unqual() == y.unqual() {
                    x.unqual().clone()
                } else {
                    if !x.is_void() && !y.is_void() {
                        let msg = format!("pointer type mismatch ('{at}' and '{bt}')");
                        self.errors
                            .push(Error::warning(pos, msg, "pointer-type-mismatch"));
                    }
                    Type::Void
                };
                Type::ptr(to.qualified(q))
            }
            (Type::Ptr(_), Type::Int(_)) | (Type::Int(_), Type::Ptr(_)) => {
                let null =
                    consteval::is_null_pointer(a, self.xlen) || consteval::is_null_pointer(b, self.xlen);
                if !null {
                    let msg = format!(
                        "pointer/integer type mismatch in conditional expression ('{at}' and '{bt}')"
                    );
                    self.errors
                        .push(Error::warning(pos, msg, "conditional-type-mismatch"));
                }
                if at.is_ptr() {
                    at.clone()
                } else {
                    bt.clone()
                }
            }
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

/// Whether two operands of a comparison are the same thing, so that it is
/// always true or false, as clang's `isSameComparisonOperand` sees it: the
/// same variable, the same member of the same variable (through `.` and
/// `->`), the same element (by the same index) of the same array.
fn same_operand(a: &Expr, b: &Expr) -> bool {
    match (&a.kind, &b.kind) {
        (ExprKind::Var { var: Some(x), .. }, ExprKind::Var { var: Some(y), .. }) => x == y,
        (
            ExprKind::Member {
                base: x,
                name: m,
                arrow: p,
                ..
            },
            ExprKind::Member {
                base: y,
                name: n,
                arrow: q,
                ..
            },
        ) => m == n && p == q && member_base(x, y),
        // `a[i]` is `*(a + i)`, at the `[`.
        (ExprKind::Deref(x), ExprKind::Deref(y)) if x.pos == a.pos && y.pos == b.pos => {
            match (&x.kind, &y.kind) {
                (ExprKind::Binary(BinOp::Add, xa, xi), ExprKind::Binary(BinOp::Add, ya, yi)) => {
                    same_operand(xa, ya) && same_operand(xi, yi)
                }
                _ => false,
            }
        }
        (ExprKind::Num(x), ExprKind::Num(y)) => x == y,
        _ => false,
    }
}

/// The bases of two members [`same_operand`] compares: the same variable,
/// or the same member of one.
fn member_base(a: &Expr, b: &Expr) -> bool {
    match (&a.kind, &b.kind) {
        (ExprKind::Var { var: Some(x), .. }, ExprKind::Var { var: Some(y), .. }) => x == y,
        (
            ExprKind::Member {
                base: x,
                name: m,
                arrow: p,
                ..
            },
            ExprKind::Member {
                base: y,
                name: n,
                arrow: q,
                ..
            },
        ) => m == n && p == q && member_base(x, y),
        _ => false,
    }
}

/// clang's warning for a pointer `e` that can never be null, used as a
/// truth value (`if (&g)`: `never` is `None`) or compared with a null
/// pointer constant (`never` says whether the comparison is `!=`): the
/// address of an object (`&g`), an array or a function named, as clang's
/// `DiagnoseAlwaysNonNullPointer` finds them; at the name.
fn never_null(e: &Expr, ne: Option<bool>) -> Option<Error> {
    // A function's name, or an array's, decays to a pointer to it.
    let (name, pos, what, function) = match &e.kind {
        ExprKind::Addr(x) => match &x.kind {
            ExprKind::Var { name, .. } => (name, x.pos, "address of", false),
            _ => return None,
        },
        ExprKind::Var { name, .. } if e.ty.is_func() => (name, e.pos, "function", true),
        ExprKind::Var { name, .. } if e.ty.is_array() => (name, e.pos, "array", false),
        _ => return None,
    };
    let named = match (what, ne) {
        ("address of", _) => format!("address of '{name}'"),
        (_, None) => format!("address of {what} '{name}'"),
        (_, Some(_)) => format!("{what} '{name}'"),
    };
    let mut d = match ne {
        None => Error::warning(
            pos,
            format!("{named} will always evaluate to 'true'"),
            "pointer-bool-conversion",
        ),
        Some(ne) => {
            let (how, always) = if ne {
                ("not equal", "true")
            } else {
                ("equal", "false")
            };
            Error::warning(
                pos,
                format!("comparison of {named} {how} to a null pointer is always {always}"),
                "tautological-pointer-compare",
            )
        }
    };
    if function {
        d = d.note(pos, "prefix with the address-of operator to silence this warning");
        if ne.is_some() {
            d = d.note(pos, "suffix with parentheses to turn this into a function call");
        }
    }
    Some(d)
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

    fn func<'a>(p: &'a Program, name: &str) -> &'a Function {
        p.funcs().find(|f| f.name == name && f.body.is_some()).unwrap()
    }

    fn var(v: &Option<VarRef>) -> String {
        match v.as_ref().unwrap() {
            VarRef::Local(LocalId(i)) => format!("#{i}"),
            VarRef::Global(s) => format!("@{s}"),
        }
    }

    /// What the parser evaluated away in an array size, an enumerator or a
    /// `_Static_assert` (the operand a constant `?:` does not choose, that
    /// of `sizeof`) is checked as clang 23 checks it, without evaluating it:
    /// its errors are reported, at file scope and in a block, where it was
    /// (a local's name is in scope). One the parser could not type is an
    /// error even without one of its own. The warning of an array size
    /// folded goes with the program, unless its operand has an error.
    #[test]
    fn what_the_parser_evaluated_away_is_checked() {
        // A pointer is valid, but makes the size no integer constant.
        // Checked where it was: a local is in scope.
        let p = ok("int g(void) { int n = 3; enum { B = 1 ? 2 : n }; return B; }");
        assert!(p.warnings.is_empty());
        let p = ok("unsigned u(void); int a[1 ? 2 : u()];");
        let w: Vec<String> = p.warnings.iter().map(|w| w.to_string()).collect();
        assert_eq!(
            w,
            ["1:23: variable length array folded to constant array as an extension"]
        );
        assert_eq!(
            (p.warnings[0].severity, p.warnings[0].flag),
            (crate::Severity::Warning, Some("gnu-folding-constant"))
        );
    }

    /// What the parser evaluated away is checked with the names in scope
    /// where it was (clang 23 takes each of these): a parameter list's with
    /// the parameters before it, a `for` statement's in its scope, a
    /// declarator's before its name is declared, and before the next
    /// declarator's, a parameter's at file scope, not in the body.
    #[test]
    fn what_was_evaluated_away_is_checked_in_its_scope() {
        for src in [
            "int f(int n, char a[sizeof n]);",
            "void f(int n, char a[sizeof n]) { }",
            "int g(void) { return sizeof(int (*)(int n, char a[sizeof n])); }",
            "int f(int n, int (*cb)(int m, char b[sizeof m + sizeof n]));",
            "int f(void) { int r = 0; for (int i = 0; i < (int)sizeof(char[sizeof i]); i++) r++; return r; }",
            "struct S { int y; } g;\nint f(void) { char a[sizeof g.y] = {0}, g = 1; return a[0] + g; }",
            "struct S { int y; } g;\nint f(void) { char g[sizeof g.y] = {0}; return g[0]; }",
            "struct S { int y; } g;\nint f(char a[sizeof g.y]) { int g = 0; return a[0] + g; }",
            "struct S { int y; } g;\nint f(int x) { if (sizeof(char[sizeof g.y])) { int g = x; return g; } return 0; }",
        ] {
            let p = ok(src);
            assert!(p.warnings.is_empty(), "{src}");
        }
    }

    /// A compound literal's folding warning is at its `(` (the table of
    /// tests/diagnostics has the rest).
    #[test]
    fn folding_warnings_are_clangs() {
        let p = ok("unsigned u(void);\nint *q = (int[1 ? 2 : u()]){1, 2};");
        assert_eq!(
            p.warnings[0].to_string(),
            "2:10: variable length array folded to constant array as an extension"
        );
    }

    /// A compound literal's value where a constant is needed is the value
    /// its list gives, and an array or a constant struct is initialized by
    /// a literal of its type as by its list (GNU extensions clang 23 takes):
    /// the data are clang's, and no object is emitted but for a literal
    /// whose address is taken.
    #[test]
    fn compound_literals_are_read_as_constants() {
        let src = "int g = 7;\nstruct S { int a, b; };\nstruct U { struct S s; int z; };\n\
                   union W { int i; char c; };\n\
                   void *v = (int *){0};\nint m = (int){5};\nint *p = (int *){&g};\n\
                   long l = (long){(char)3};\nint m2 = (int){5} + 1;\n\
                   int *ps[2] = { (int *){0}, (int *){&g} };\nint *pp = (int *){&g} + 1;\n\
                   int z = !(int *){0};\nint *pz = (int *){0} ? &g : 0;\n\
                   struct S st = (struct S){8, 9};\nint ia[2] = (int[2]){1, 2};\n\
                   int ia3[] = (int[]){4, 5, 6};\nchar cs[4] = (char[4]){\"xy\"};\n\
                   struct U u = { (struct S){1, 2}, 3 };\nint nm = (struct U){{1, 2}, 3}.s.b;\n\
                   struct S sv = (struct U){{4, 5}, 6}.s;\nint ib[2] = (int[2]){[1] = 4};\n\
                   int wi = (union W){5}.i;\nunion W w = (union W){6};\n\
                   int *pm = &(struct S){1, 2}.b;\n\
                   int f(void) { static int *sp = (int *){&g}; static struct S ss = (struct S){3, 4};\n\
                       static int s6 = (struct S){1, 2}.b + (int){3}; int la[2] = (int[2]){g, 2};\n\
                       return *sp + ss.b + s6 + la[0]; }\n";
        let asm = crate::compile(src).unwrap();
        // A symbol's directives, up to the blank line after them.
        let data = |sym: &str| -> Vec<String> {
            let from = asm
                .find(&format!("\n{sym}:\n"))
                .unwrap_or_else(|| panic!("no {sym} in\n{asm}"));
            asm[from + sym.len() + 3..]
                .lines()
                .take_while(|l| !l.is_empty())
                .map(|l| l.trim().to_string())
                .collect()
        };
        for (sym, want) in [
            ("v", vec![".word 0"]),
            ("m", vec![".word 5"]),
            ("p", vec![".word g"]),
            ("l", vec![".word 3"]),
            ("m2", vec![".word 6"]),
            ("ps", vec![".word 0", ".word g"]),
            ("pp", vec![".word g+4"]),
            ("z", vec![".word 1"]),
            ("pz", vec![".word 0"]),
            ("st", vec![".word 8", ".word 9"]),
            ("ia", vec![".word 1", ".word 2"]),
            ("ia3", vec![".word 4", ".word 5", ".word 6"]),
            ("cs", vec![".ascii \"xy\"", ".zero 2"]),
            ("u", vec![".word 1", ".word 2", ".word 3"]),
            ("nm", vec![".word 2"]),
            ("sv", vec![".word 4", ".word 5"]),
            ("ib", vec![".zero 4", ".word 4"]),
            ("wi", vec![".word 5"]),
            ("w", vec![".word 6"]),
            ("pm", vec![".word .compoundliteral.0+4"]),
            ("f.sp", vec![".word g"]),
            ("f.ss", vec![".word 3", ".word 4"]),
            ("f.s6", vec![".word 5"]),
        ] {
            assert_eq!(data(sym), want, "{sym}");
        }
        // The one literal whose address is taken is the one object.
        assert_eq!(asm.matches("\n.compoundliteral.").count(), 1, "{asm}");
        // Not a constant, or of another type, as clang has it.
    }

    /// `name#slot` (or `name@symbol`) for every variable use in the function, in
    /// source order.
    fn uses(f: &Function) -> Vec<String> {
        fn expr(e: &Expr, out: &mut Vec<String>) {
            match &e.kind {
                ExprKind::Num(_)
                | ExprKind::Enumerator { .. }
                | ExprKind::Str(_)
                | ExprKind::SizeofType(_) => {}
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
                | ExprKind::SizeofExpr(x, _)
                | ExprKind::VaArg(x, _)
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
                StmtKind::Unevaluated(us) => us.iter().for_each(|u| expr(&u.expr, out)),
                StmtKind::Decl(ds) => {
                    for d in ds {
                        out.push(format!("decl {}{}", d.name, var(&d.var)));
                        if let Some(Init::Expr(e)) = &d.init {
                            expr(e, out);
                        }
                    }
                }
                StmtKind::Expr(e) | StmtKind::Return(Some(e)) => expr(e, out),
                StmtKind::Empty { .. } | StmtKind::Break | StmtKind::Continue | StmtKind::Return(None) => {}
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
                    ..
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
                StmtKind::Goto(..) => {}
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
                ExprKind::Cast {
                    x, implicit: true, ..
                } => format!("({} {})", e.ty, sx(x)),
                ExprKind::Cast {
                    x, implicit: false, ..
                } => format!("(cast {} {})", e.ty, sx(x)),
                ExprKind::Num(v) => v.to_string(),
                ExprKind::Enumerator { name, .. } => name.clone(),
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
            defined("char s[] = \"abc\", t[2] = \"ab\";"),
            ["char s[4] (def)", "char t[2] (def)"]
        );
        assert_eq!(
            defined("static int s; extern int s;"),
            ["static int s (def)", "extern int s"]
        );
    }

    #[test]
    fn constant_initializers() {
        ok(
            "int a = 1 + 2 * 3; int b = sizeof(int) << 2; int *p = &a; int arr[4]; int *q = arr + 2;\n\
            int *r = &arr[3]; char *s = \"hi\"; char *n = 0; void *v = (void *)0; int *w = (int *)0x1000;\n\
            unsigned char u = -1; int m[2][2]; int *mp = m[1];",
        );
        // An address converted to an integer as wide as a pointer is one,
        // moved by adding or subtracting an integer; any other width or
        // arithmetic is not constant, as clang has it.
        let err_on = |x: Xlen, src: &str| -> Option<String> {
            let opts = crate::Options {
                xlen: x,
                ..crate::Options::default()
            };
            crate::parse_with("t.c", src, &opts, &crate::no_files)
                .err()
                .map(|e| e.iter().map(|e| e.to_string()).collect::<Vec<_>>().join("; "))
        };
        const NOT_CONSTANT: &str = "initializer element is not a compile-time constant";
        for (src, rv32, rv64) in [
            ("int a; long b = (long)&a;", true, true),
            ("int a; unsigned long b = (unsigned long)&a + 4;", true, true),
            ("int a; long b = 4 + (long)&a - 8;", true, true),
            ("int a; int *b = (int *)((long)&a + 4);", true, true),
            (
                "int a[2]; long b = (long)a, c = (long)\"s\", d = (long)&a[1];",
                true,
                true,
            ),
            ("void f(void); long b = (long)f;", true, true),
            ("int a; int b = (int)&a;", true, false),
            ("int a; unsigned b = (unsigned)&a;", true, false),
            ("int a; int b = (int)(long)&a;", true, false),
            ("int a; long long b = (long long)&a;", false, true),
            (
                "int a; unsigned long long b = (unsigned long long)&a;",
                false,
                true,
            ),
            ("int a; short b = (short)&a;", false, false),
            ("int a; char b = (char)&a;", false, false),
            ("int a; long b = (long)&a * 2;", false, false),
            ("int a; long b = (long)&a | 1;", false, false),
            ("int a; long b = -(long)&a;", false, false),
            ("int a; long b = 4 - (long)&a;", false, false),
            ("int a; long b = (long)&a - (long)&a;", false, false),
        ] {
            for (x, ok) in [(Xlen::Rv32, rv32), (Xlen::Rv64, rv64)] {
                match err_on(x, src) {
                    None => assert!(ok, "{src} ({x:?}) compiles"),
                    Some(e) => assert!(!ok && e.ends_with(NOT_CONSTANT), "{src} ({x:?}): {e}"),
                }
            }
        }
        // Locals may be initialized with anything.
        ok("int f(int x) { int y = x * 2; int a[2] = {x, f(y)}; return a[1]; }");
    }

    #[test]
    fn array_initializers() {
        ok(
            "int a[3] = {1, 2}; int m[2][3] = {{1, 2, 3}, {4}}; char s[3] = {'a', 98, 0}; \
            char t[] = {\"x\"}; int one = {1}; unsigned char u[2] = \"ab\"; signed char v[] = \"q\";",
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
        let p = ok("int f() { int a[] = {1, 2, 3}; return sizeof a; }");
        assert_eq!(func(&p, "f").locals[0].ty.to_string(), "int[3]");
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
        assert_eq!(ty("int x", "buf"), "char[8]");
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
        assert_eq!(ty("int x", "\"abc\""), "char[4]");
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

    #[test]
    fn conversion_errors() {
        // `(void *)0` in a `?:` is a null pointer constant, not a `void *`.
        // Conversions C allows.
        ok("void *malloc(unsigned long n); int f(int *p, void *v, char c) { \
            p = v; v = p; p = 0; p = (int *)0; v = (void *)0; c = 1000; p = malloc(4); p += 1; p -= 2; \
            return p == 0 || 0 != p || p == v || v != p; }");
        // A function pointer against NULL, `(void *)0`: allowed for == and
        // != only; against any other `void *` it is not.
        ok("void g(void); int f(void (*fp)(void)) {             return fp != (void *)0 || (void *)0 == fp || fp == 0 || g != (void *)0; }");
    }

    #[test]
    fn returns() {
        ok("void f() { return; } void g(int x) { if (x) return; f(); }");
        // A typedef for void spells `(void)` too, but only unqualified.
        ok("typedef void V; int f(V); int main(V) { return f(); }");
        // `int a[const 3]` makes the pointer const, not the elements.
        ok("int f(int a[const 3]) { a[0] = 1; return a[1]; }");
    }

    #[test]
    fn conditions_must_be_scalar() {
        // Pointers and arrays are fine.
        ok("char b[2]; int f(int *p) { if (p) return 1; while (b) break; do ; while (p); for (; p; ) break; \
            return !p; }");
    }

    #[test]
    fn calls() {
        ok("int f(int n) { return f(n); }");
        ok("int g(int a, int b); int f() { return g(1, 2); }");
        ok(
            "int odd(int n); int even(int n) { return n == 0 || odd(n - 1); } \
            int odd(int n) { return n != 0 && even(n - 1); }",
        );
        // Declared after the use: still undeclared at the call, as in C99.
    }

    #[test]
    fn redefinitions() {
        // The body's top level shares the parameters' scope.
        // A prototype first: the first definition must still count.
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
        ok(
            "int f() { while (1) { if (1) break; else continue; } for (;;) { { break; } } \
            do { continue; } while (0); return 0; }",
        );
        // A loop's scope ends with its body.
    }

    #[test]
    fn assignment_target_must_be_assignable() {
        ok("int f(int a, int *p) { (a) = 2; *p = 3; p[1] = 4; *(p + 2) += 1; ++*p; p[0]--; return a; }");
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
        // A member of a const struct is const.
    }

    #[test]
    fn struct_values() {
        ok(&format!(
            "{P} struct p g(struct p a) {{ struct p b; b = a; return b; }} \
             int f(struct p a, int c) {{ return (c ? a : g(a)).x + g(a).c; }}"
        ));
        // Complete types are needed where the size matters.
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
        assert_eq!(temps, ["int[3]", "struct p", "char[4]"]);
        // An lvalue.
        ok("int f() { int *p = &(int){5}; (int){1} = 2; return *p; }");
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
                ".compoundliteral.0: int[3]",
                ".compoundliteral.1: struct p",
                ".compoundliteral.2: const char *const[2]",
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
        // In the unevaluated operand of `sizeof` a literal is no object:
        // nothing is emitted for it (as with clang), and the numbering
        // carries on as if it were not there.
        let p = ok("int n = sizeof (int[]){1, 2, 3}; int *a = (int[]){4}; int k = sizeof (int){5} + 1;");
        let Some(Item::Decl(literals)) = p.items.last() else {
            panic!("no literals")
        };
        let got: Vec<String> = literals.iter().map(|d| format!("{}: {}", d.name, d.ty)).collect();
        assert_eq!(got, [".compoundliteral.0: int[1]"]);
        // Its value, where it is read (a GNU extension clang takes), is
        // folded: no object is made for it.
        let p = ok("int m = (int){5};");
        assert!(!format!("{:?}", p.items).contains(".compoundliteral"));
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
    }

    #[test]
    fn va_builtins() {
        ok(
            "int f(int n, ...) { __builtin_va_list ap, aq; __builtin_va_start(ap, n); \
            __builtin_va_copy(aq, ap); int k = __builtin_va_arg(aq, int); __builtin_va_end(aq); \
            __builtin_va_end(ap); return k; }",
        );
    }

    #[test]
    fn qualifiers() {
        ok("int f(char *s) { const char *t = s; const char **u = &t; volatile int v = 1; return *t + v; }");
        ok("const int k = 3; int f(const int *p) { return *p + k; }");
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
        // `NULL`, `((void *)0)`, is a function pointer's too, initialized,
        // assigned or passed; another `void *` is not.
        ok("struct S { int (*f)(void); } g = { ((void *)0) };
int (*h)(void) = ((void *)0);
            void take(void (*cb)(int));
            int f(void) { int (*l)(void) = ((void *)0); l = ((void *)0); take(((void *)0)); return 0; }");
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
        // `break` leaves a switch; `continue` still needs a loop.
        ok("int f(int x) { switch (x) { case 1: break; } return 0; }");
    }

    #[test]
    fn labels_and_goto() {
        ok("int f() { goto end; { inner: ; } end: goto inner; }");
        ok("int f() { a: ; } int g() { a: ; }");
    }

    #[test]
    fn struct_and_union_initializers() {
        ok("struct s { int a; char b[3]; struct { short x, y; } in; }; \
            struct s g = {1, \"ab\", {2, 3}}; struct s h = {1, {'a'}, 4}; union { char c; int i; } u = {5};");
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
