//! Name resolution and checks: every variable must be declared (C scoping, inner
//! blocks may shadow), every call must name a function declared earlier with the
//! right number of arguments, `break`/`continue` must be inside a loop, and only
//! variables can be assigned to.
//!
//! Fills in [`Function::locals`] and every `local` field. Reports all errors, in
//! source order.

use crate::ast::*;
use crate::{Error, Pos};
use std::collections::HashMap;

pub fn check(prog: &mut Program) -> Result<(), Vec<Error>> {
    let mut errors = Vec::new();
    // Functions are visible from their declaration onwards, so a function can call
    // itself and anything declared above it (use a prototype for mutual recursion).
    let mut funcs: HashMap<String, FuncSig> = HashMap::new();
    for f in &mut prog.funcs {
        match funcs.get_mut(&f.name) {
            Some(sig) if sig.arity != f.params.len() => errors.push(Error::new(
                f.pos,
                format!(
                    "conflicting types for '{}': declared at {} with {} parameter{}, here with {}",
                    f.name,
                    sig.pos,
                    sig.arity,
                    plural(sig.arity),
                    f.params.len()
                ),
            )),
            Some(sig) if sig.defined && f.body.is_some() => {
                errors.push(Error::new(f.pos, format!("redefinition of '{}'", f.name)))
            }
            Some(sig) => sig.defined |= f.body.is_some(),
            None => {
                funcs.insert(
                    f.name.clone(),
                    FuncSig {
                        arity: f.params.len(),
                        defined: f.body.is_some(),
                        pos: f.pos,
                    },
                );
            }
        }
        let mut c = FnChecker {
            funcs: &funcs,
            scopes: vec![HashMap::new()],
            locals: Vec::new(),
            loop_depth: 0,
            errors: &mut errors,
        };
        for p in &f.params {
            c.declare(&p.name, p.pos, "parameter ");
        }
        if let Some(body) = &mut f.body {
            // The body's outermost block shares the parameters' scope, as in C.
            for s in body {
                c.stmt(s);
            }
        }
        f.locals = c.locals;
    }
    if errors.is_empty() {
        Ok(())
    } else {
        errors.sort_by_key(|e| e.pos);
        Err(errors)
    }
}

fn plural(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

struct FuncSig {
    arity: usize,
    defined: bool,
    pos: Pos,
}

struct FnChecker<'a> {
    funcs: &'a HashMap<String, FuncSig>,
    scopes: Vec<HashMap<String, LocalId>>,
    locals: Vec<Local>,
    loop_depth: usize,
    errors: &'a mut Vec<Error>,
}

impl FnChecker<'_> {
    fn err(&mut self, pos: Pos, msg: String) {
        self.errors.push(Error::new(pos, msg));
    }

    fn declare(&mut self, name: &str, pos: Pos, what: &str) -> LocalId {
        let id = LocalId(self.locals.len());
        self.locals.push(Local {
            name: name.to_string(),
            pos,
        });
        let scope = self.scopes.last_mut().expect("at least one scope");
        if let Some(prev) = scope.insert(name.to_string(), id) {
            let prev_pos = self.locals[prev.0].pos;
            self.err(
                pos,
                format!("redefinition of {what}'{name}' (first declared at {prev_pos})"),
            );
        }
        id
    }

    fn lookup(&self, name: &str) -> Option<LocalId> {
        self.scopes.iter().rev().find_map(|s| s.get(name).copied())
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

    fn stmt(&mut self, s: &mut Stmt) {
        match &mut s.kind {
            StmtKind::Block(body) => self.scoped(|c| body.iter_mut().for_each(|s| c.stmt(s))),
            StmtKind::Decl(decls) => {
                for d in decls {
                    // As in C, the name is in scope inside its own initializer.
                    d.local = Some(self.declare(&d.name, d.pos, ""));
                    if let Some(e) = &mut d.init {
                        self.expr(e);
                    }
                }
            }
            StmtKind::Expr(e) | StmtKind::Return(e) => self.expr(e),
            StmtKind::Empty => {}
            StmtKind::If { cond, then, els } => {
                self.expr(cond);
                self.scoped(|c| c.stmt(then));
                if let Some(e) = els {
                    self.scoped(|c| c.stmt(e));
                }
            }
            StmtKind::While { cond, body } => {
                self.expr(cond);
                self.in_loop(|c| c.scoped(|c| c.stmt(body)));
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
                    c.expr(e);
                }
                if let Some(e) = step {
                    c.expr(e);
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

    fn expr(&mut self, e: &mut Expr) {
        match &mut e.kind {
            ExprKind::Num(_) => {}
            ExprKind::Var { name, local } => {
                *local = self.lookup(name);
                if local.is_none() {
                    let msg = if self.funcs.contains_key(name.as_str()) {
                        format!("'{name}' is a function; function pointers are not supported yet")
                    } else {
                        format!("use of undeclared identifier '{name}'")
                    };
                    self.err(e.pos, msg);
                }
            }
            ExprKind::Assign(target, value) => {
                if !matches!(target.kind, ExprKind::Var { .. }) {
                    self.err(target.pos, "expression is not assignable".to_string());
                }
                self.expr(target);
                self.expr(value);
            }
            ExprKind::Binary(_, l, r) => {
                self.expr(l);
                self.expr(r);
            }
            ExprKind::Unary(_, x) => self.expr(x),
            ExprKind::Call(name, args) => {
                if self.lookup(name).is_some() {
                    self.err(
                        e.pos,
                        format!("called object '{name}' is a variable, not a function"),
                    );
                } else {
                    match self.funcs.get(name.as_str()) {
                        None => self.err(e.pos, format!("call to undeclared function '{name}'")),
                        Some(sig) if sig.arity != args.len() => {
                            let (arity, sig_pos) = (sig.arity, sig.pos);
                            let which = if args.len() > arity { "many" } else { "few" };
                            self.err(
                                e.pos,
                                format!(
                                    "too {which} arguments to '{name}': expected {arity}, have {} \
                                     (declared at {sig_pos})",
                                    args.len()
                                ),
                            );
                        }
                        Some(_) => {}
                    }
                }
                for a in args {
                    self.expr(a);
                }
            }
        }
    }
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

    /// `name#slot` for every variable use in the function, in source order.
    fn uses(f: &Function) -> Vec<String> {
        fn expr(e: &Expr, out: &mut Vec<String>) {
            match &e.kind {
                ExprKind::Num(_) => {}
                ExprKind::Var { name, local } => out.push(format!("{name}#{}", local.unwrap().0)),
                ExprKind::Assign(a, b) | ExprKind::Binary(_, a, b) => {
                    expr(a, out);
                    expr(b, out);
                }
                ExprKind::Unary(_, x) => expr(x, out),
                ExprKind::Call(_, args) => args.iter().for_each(|a| expr(a, out)),
            }
        }
        fn stmt(s: &Stmt, out: &mut Vec<String>) {
            match &s.kind {
                StmtKind::Block(b) => b.iter().for_each(|s| stmt(s, out)),
                StmtKind::Decl(ds) => {
                    for d in ds {
                        out.push(format!("decl {}#{}", d.name, d.local.unwrap().0));
                        if let Some(e) = &d.init {
                            expr(e, out);
                        }
                    }
                }
                StmtKind::Expr(e) | StmtKind::Return(e) => expr(e, out),
                StmtKind::Empty | StmtKind::Break | StmtKind::Continue => {}
                StmtKind::If { cond, then, els } => {
                    expr(cond, out);
                    stmt(then, out);
                    if let Some(e) = els {
                        stmt(e, out);
                    }
                }
                StmtKind::While { cond, body } => {
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

    #[test]
    fn params_then_locals_in_source_order() {
        let p = ok("int f(int a, int b) { int c = a; { int d = b; } return c; }");
        let f = &p.funcs[0];
        let names: Vec<&str> = f.locals.iter().map(|l| l.name.as_str()).collect();
        assert_eq!(names, ["a", "b", "c", "d"]);
        assert_eq!(uses(f), ["decl c#2", "a#0", "decl d#3", "b#1", "c#2"]);
    }

    #[test]
    fn shadowing_gets_its_own_slot() {
        let p = ok("int f(int x) { int y = x; { int x = 2; y = x; } if (1) { int y = 3; } return x + y; }");
        assert_eq!(
            uses(&p.funcs[0]),
            ["decl y#1", "x#0", "decl x#2", "y#1", "x#2", "decl y#3", "x#0", "y#1"]
        );
    }

    #[test]
    fn for_scope() {
        let p = ok("int f() { int i = 5; for (int i = 0; i < 3; i = i + 1) { int j = i; } return i; }");
        assert_eq!(
            uses(&p.funcs[0]),
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
        assert_eq!(uses(&p.funcs[0]), ["decl x#1", "x#1", "x#1"]);
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
        assert_eq!(
            err("int f(int a);\nint f(int a, int b) { return a; }"),
            "2:5: conflicting types for 'f': declared at 1:5 with 1 parameter, here with 2"
        );
        // Repeating a matching prototype is fine, before or after the definition.
        ok("int f(int a); int f(int b); int f(int c) { return c; } int f(int d);");
    }

    #[test]
    fn break_and_continue_need_a_loop() {
        assert_eq!(err("int f() { break; }"), "1:11: 'break' statement not in a loop");
        assert_eq!(
            err("int f() { if (1) { continue; } return 0; }"),
            "1:20: 'continue' statement not in a loop"
        );
        ok("int f() { while (1) { if (1) break; else continue; } for (;;) { { break; } } return 0; }");
        // A loop's scope ends with its body.
        assert_eq!(
            err("int g() { while (1) {} break; }"),
            "1:24: 'break' statement not in a loop"
        );
    }

    #[test]
    fn assignment_target_must_be_a_variable() {
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
        ok("int f(int a) { (a) = 2; return a; }");
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
