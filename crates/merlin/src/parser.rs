//! Tokens to AST, by recursive descent. Stops at the first error.
//!
//! ```text
//! program    := function* EOF
//! function   := 'int' IDENT '(' params ')' (block | ';')
//! params     := ε | 'void' | 'int' IDENT (',' 'int' IDENT)*        at most 8
//! block      := '{' item* '}'
//! item       := decl | stmt
//! decl       := 'int' declarator (',' declarator)* ';'
//! declarator := IDENT ('=' assign)?
//! stmt       := block | ';' | expr ';'
//!             | 'if' '(' expr ')' stmt ('else' stmt)?
//!             | 'while' '(' expr ')' stmt
//!             | 'for' '(' (decl | expr? ';') expr? ';' expr? ')' stmt
//!             | 'return' expr ';' | 'break' ';' | 'continue' ';'
//! expr       := assign
//! assign     := binary(1) ('=' assign)?                  right-associative
//! binary(p)  := unary (OP binary(prec(OP) + 1))*         for OP with prec >= p
//! unary      := ('-' | '!') unary | primary
//! primary    := INT | IDENT | IDENT '(' (assign (',' assign)*)? ')' | '(' expr ')'
//! ```
//!
//! Binary precedence (precedence climbing), loosest first: `||` 1, `&&` 2,
//! `== !=` 3, `< <= > >=` 4, `+ -` 5, `* / %` 6. All left-associative.

use crate::ast::*;
use crate::lexer::{Kw, Tok, Token};
use crate::{Error, Pos};

/// Arguments travel in `a0-a7`; no stack-passed arguments in v0.
pub const MAX_PARAMS: usize = 8;

/// How deeply statements and expressions may nest. Each nested statement, `=`,
/// unary operator, parenthesized expression or call argument is a level, and so
/// is each operator in a chain like `a + b + c` (the tree leans left). The parser,
/// sema and the AST printer recurse once per level, so the limit turns runaway
/// input into an error instead of a stack overflow (given a few MiB of stack in
/// unoptimized builds: the merlin binary runs on a 64 MiB thread). clang's
/// bracket limit is also 256.
pub const MAX_DEPTH: usize = 256;

pub fn parse(toks: &[Token]) -> Result<Program, Error> {
    let mut p = Parser {
        toks,
        pos: 0,
        depth: 0,
    };
    let mut funcs = Vec::new();
    while *p.peek() != Tok::Eof {
        funcs.push(p.function()?);
    }
    Ok(Program { funcs })
}

fn binop(t: &Tok) -> Option<(BinOp, u8)> {
    let Tok::Punct(p) = t else { return None };
    Some(match *p {
        "||" => (BinOp::LogOr, 1),
        "&&" => (BinOp::LogAnd, 2),
        "==" => (BinOp::Eq, 3),
        "!=" => (BinOp::Ne, 3),
        "<" => (BinOp::Lt, 4),
        "<=" => (BinOp::Le, 4),
        ">" => (BinOp::Gt, 4),
        ">=" => (BinOp::Ge, 4),
        "+" => (BinOp::Add, 5),
        "-" => (BinOp::Sub, 5),
        "*" => (BinOp::Mul, 6),
        "/" => (BinOp::Div, 6),
        "%" => (BinOp::Rem, 6),
        _ => return None,
    })
}

struct Parser<'a> {
    toks: &'a [Token],
    pos: usize,
    /// Current nesting level; see [`MAX_DEPTH`].
    depth: usize,
}

impl Parser<'_> {
    /// Goes one nesting level deeper, or fails at the current token.
    fn enter(&mut self) -> Result<(), Error> {
        if self.depth == MAX_DEPTH {
            return Err(Error::new(
                self.here(),
                format!("nesting too deep: merlin allows at most {MAX_DEPTH} levels"),
            ));
        }
        self.depth += 1;
        Ok(())
    }

    /// Runs `f` one nesting level deeper.
    fn nested<T>(&mut self, f: impl FnOnce(&mut Self) -> Result<T, Error>) -> Result<T, Error> {
        self.enter()?;
        let r = f(self);
        self.depth -= 1;
        r
    }

    fn tok(&self) -> &Token {
        &self.toks[self.pos.min(self.toks.len() - 1)]
    }

    fn peek(&self) -> &Tok {
        &self.tok().tok
    }

    fn peek_at(&self, n: usize) -> &Tok {
        &self.toks[(self.pos + n).min(self.toks.len() - 1)].tok
    }

    fn here(&self) -> Pos {
        self.tok().pos
    }

    /// Just past the previous token: where a missing `;` or `)` belongs.
    fn after_prev(&self) -> Pos {
        if self.pos == 0 {
            self.here()
        } else {
            self.toks[self.pos - 1].end
        }
    }

    fn advance(&mut self) -> Token {
        let t = self.tok().clone();
        if self.pos < self.toks.len() - 1 {
            self.pos += 1;
        }
        t
    }

    fn is_punct(&self, p: &str) -> bool {
        matches!(self.peek(), Tok::Punct(q) if *q == p)
    }

    fn eat_punct(&mut self, p: &str) -> bool {
        let yes = self.is_punct(p);
        if yes {
            self.advance();
        }
        yes
    }

    fn eat_kw(&mut self, k: Kw) -> bool {
        let yes = *self.peek() == Tok::Kw(k);
        if yes {
            self.advance();
        }
        yes
    }

    fn unexpected(&self, what: &str) -> Error {
        Error::new(
            self.here(),
            format!("expected {what}, found {}", self.tok().describe()),
        )
    }

    fn expect_punct(&mut self, p: &str) -> Result<Pos, Error> {
        if self.is_punct(p) {
            Ok(self.advance().pos)
        } else {
            Err(self.unexpected(&format!("'{p}'")))
        }
    }

    /// `;` is reported right after the previous token, like clang does, since that's
    /// where it's missing.
    fn expect_semi(&mut self, after: &str) -> Result<(), Error> {
        if self.eat_punct(";") {
            Ok(())
        } else {
            Err(Error::new(self.after_prev(), format!("expected ';' {after}")))
        }
    }

    fn ident(&mut self, what: &str) -> Result<(String, Pos), Error> {
        match self.peek().clone() {
            Tok::Ident(name) => Ok((name, self.advance().pos)),
            _ => Err(self.unexpected(what)),
        }
    }

    fn function(&mut self) -> Result<Function, Error> {
        if !self.eat_kw(Kw::Int) {
            return Err(self.unexpected("'int' to start a function"));
        }
        let (name, pos) = self.ident("function name")?;
        if matches!(self.peek(), Tok::Punct(";" | "=" | ",")) {
            return Err(Error::new(pos, "global variables are not supported yet"));
        }
        self.expect_punct("(")?;
        let params = self.params()?;
        let body = if self.eat_punct(";") {
            None
        } else if self.is_punct("{") {
            Some(self.block_body()?)
        } else {
            return Err(self.unexpected("'{' or ';' after the parameter list"));
        };
        Ok(Function {
            name,
            params,
            body,
            locals: Vec::new(),
            pos,
        })
    }

    /// After `(`, up to and including `)`.
    fn params(&mut self) -> Result<Vec<Param>, Error> {
        let mut params = Vec::new();
        if self.eat_punct(")") {
            return Ok(params);
        }
        if *self.peek() == Tok::Kw(Kw::Void) && matches!(self.peek_at(1), Tok::Punct(")")) {
            self.advance();
            self.advance();
            return Ok(params);
        }
        loop {
            if !self.eat_kw(Kw::Int) {
                return Err(self.unexpected("parameter type 'int'"));
            }
            let (name, pos) = self.ident("parameter name")?;
            if params.len() == MAX_PARAMS {
                return Err(Error::new(
                    pos,
                    format!("too many parameters: merlin supports at most {MAX_PARAMS} (a0-a7)"),
                ));
            }
            params.push(Param { name, pos });
            if self.eat_punct(")") {
                return Ok(params);
            }
            if !self.eat_punct(",") {
                return Err(self.unexpected("',' or ')' in the parameter list"));
            }
        }
    }

    /// `{ item* }`, returning the items.
    fn block_body(&mut self) -> Result<Vec<Stmt>, Error> {
        self.expect_punct("{")?;
        let mut items = Vec::new();
        loop {
            if self.eat_punct("}") {
                return Ok(items);
            }
            if *self.peek() == Tok::Eof {
                return Err(self.unexpected("'}'"));
            }
            items.push(if *self.peek() == Tok::Kw(Kw::Int) {
                self.decl()?
            } else {
                self.stmt()?
            });
        }
    }

    /// `int a = 1, b;`
    fn decl(&mut self) -> Result<Stmt, Error> {
        let pos = self.advance().pos; // `int`
        let mut decls = Vec::new();
        loop {
            let (name, dpos) = self.ident("variable name")?;
            if self.is_punct("(") {
                return Err(Error::new(
                    dpos,
                    "function declarations inside a function are not supported",
                ));
            }
            let init = if self.eat_punct("=") {
                Some(self.assign()?)
            } else {
                None
            };
            decls.push(Declarator {
                name,
                init,
                local: None,
                pos: dpos,
            });
            if !self.eat_punct(",") {
                break;
            }
        }
        self.expect_semi("at end of declaration")?;
        Ok(Stmt {
            kind: StmtKind::Decl(decls),
            pos,
        })
    }

    fn paren_expr(&mut self) -> Result<Expr, Error> {
        self.expect_punct("(")?;
        let e = self.expr()?;
        if !self.is_punct(")") {
            return Err(self.unexpected("')'"));
        }
        self.advance();
        Ok(e)
    }

    fn stmt(&mut self) -> Result<Stmt, Error> {
        self.nested(Self::stmt_inner)
    }

    fn stmt_inner(&mut self) -> Result<Stmt, Error> {
        let pos = self.here();
        let kind = match self.peek().clone() {
            Tok::Punct("{") => StmtKind::Block(self.block_body()?),
            Tok::Punct(";") => {
                self.advance();
                StmtKind::Empty
            }
            Tok::Kw(Kw::Int) => {
                return Err(Error::new(
                    pos,
                    "a declaration is not a statement; wrap it in '{ }'",
                ));
            }
            Tok::Kw(Kw::If) => {
                self.advance();
                let cond = self.paren_expr()?;
                let then = Box::new(self.stmt()?);
                let els = if self.eat_kw(Kw::Else) {
                    Some(Box::new(self.stmt()?))
                } else {
                    None
                };
                StmtKind::If { cond, then, els }
            }
            Tok::Kw(Kw::While) => {
                self.advance();
                let cond = self.paren_expr()?;
                let body = Box::new(self.stmt()?);
                StmtKind::While { cond, body }
            }
            Tok::Kw(Kw::For) => {
                self.advance();
                self.expect_punct("(")?;
                let init = if *self.peek() == Tok::Kw(Kw::Int) {
                    Some(Box::new(self.decl()?))
                } else if self.eat_punct(";") {
                    None
                } else {
                    let ipos = self.here();
                    let e = self.expr()?;
                    self.expect_semi("in 'for' statement")?;
                    Some(Box::new(Stmt {
                        kind: StmtKind::Expr(e),
                        pos: ipos,
                    }))
                };
                let cond = if self.is_punct(";") {
                    None
                } else {
                    Some(self.expr()?)
                };
                self.expect_semi("in 'for' statement")?;
                let step = if self.is_punct(")") {
                    None
                } else {
                    Some(self.expr()?)
                };
                if !self.eat_punct(")") {
                    return Err(self.unexpected("')'"));
                }
                let body = Box::new(self.stmt()?);
                StmtKind::For {
                    init,
                    cond,
                    step,
                    body,
                }
            }
            Tok::Kw(Kw::Return) => {
                self.advance();
                if self.is_punct(";") {
                    return Err(Error::new(
                        pos,
                        "return needs a value: every function returns int",
                    ));
                }
                let e = self.expr()?;
                self.expect_semi("after return statement")?;
                StmtKind::Return(e)
            }
            Tok::Kw(Kw::Break) => {
                self.advance();
                self.expect_semi("after 'break'")?;
                StmtKind::Break
            }
            Tok::Kw(Kw::Continue) => {
                self.advance();
                self.expect_semi("after 'continue'")?;
                StmtKind::Continue
            }
            Tok::Kw(Kw::Else) => return Err(Error::new(pos, "'else' without a matching 'if'")),
            _ => {
                let e = self.expr()?;
                self.expect_semi("after expression")?;
                StmtKind::Expr(e)
            }
        };
        Ok(Stmt { kind, pos })
    }

    fn expr(&mut self) -> Result<Expr, Error> {
        self.assign()
    }

    fn assign(&mut self) -> Result<Expr, Error> {
        self.nested(Self::assign_inner)
    }

    fn assign_inner(&mut self) -> Result<Expr, Error> {
        let lhs = self.binary(1)?;
        if self.is_punct("=") {
            let pos = self.advance().pos;
            let rhs = self.assign()?;
            return Ok(Expr {
                kind: ExprKind::Assign(Box::new(lhs), Box::new(rhs)),
                pos,
            });
        }
        Ok(lhs)
    }

    /// Precedence climbing: parses operators binding at least as tightly as `min`.
    fn binary(&mut self, min: u8) -> Result<Expr, Error> {
        let mut lhs = self.unary()?;
        let outer = self.depth;
        while let Some((op, prec)) = binop(self.peek()) {
            if prec < min {
                break;
            }
            // Each operator puts everything parsed so far one level deeper.
            self.enter()?;
            let pos = self.advance().pos;
            let rhs = self.binary(prec + 1)?;
            lhs = Expr {
                kind: ExprKind::Binary(op, Box::new(lhs), Box::new(rhs)),
                pos,
            };
        }
        self.depth = outer;
        Ok(lhs)
    }

    fn unary(&mut self) -> Result<Expr, Error> {
        let op = match self.peek() {
            Tok::Punct("-") => UnOp::Neg,
            Tok::Punct("!") => UnOp::Not,
            _ => return self.primary(),
        };
        let pos = self.advance().pos;
        let x = self.nested(Self::unary)?;
        Ok(Expr {
            kind: ExprKind::Unary(op, Box::new(x)),
            pos,
        })
    }

    fn primary(&mut self) -> Result<Expr, Error> {
        let pos = self.here();
        let kind = match self.peek().clone() {
            Tok::Int(v) => {
                self.advance();
                ExprKind::Num(v)
            }
            Tok::Ident(name) => {
                self.advance();
                if self.eat_punct("(") {
                    let mut args = Vec::new();
                    if !self.eat_punct(")") {
                        loop {
                            args.push(self.assign()?);
                            if self.eat_punct(")") {
                                break;
                            }
                            if !self.eat_punct(",") {
                                return Err(self.unexpected("',' or ')' in the argument list"));
                            }
                        }
                    }
                    ExprKind::Call(name, args)
                } else {
                    ExprKind::Var { name, local: None }
                }
            }
            Tok::Punct("(") => {
                // Keep the inner node; parentheses only steer the tree's shape.
                return self.paren_expr();
            }
            _ => return Err(self.unexpected("expression")),
        };
        Ok(Expr { kind, pos })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::lex;

    fn prog(src: &str) -> Program {
        parse(&lex(src).unwrap()).unwrap_or_else(|e| panic!("{src}: {e}"))
    }

    fn err(src: &str) -> String {
        parse(&lex(src).unwrap()).unwrap_err().to_string()
    }

    /// Body of `int f() { <body> }`.
    fn body(stmts: &str) -> Vec<Stmt> {
        let p = prog(&format!("int f() {{ {stmts} }}"));
        p.funcs.into_iter().next().unwrap().body.unwrap()
    }

    fn stmt(s: &str) -> Stmt {
        let mut b = body(s);
        assert_eq!(b.len(), 1, "{s}");
        b.remove(0)
    }

    /// The expression as an s-expression, e.g. `(+ 1 (* 2 3))`.
    fn sx(e: &Expr) -> String {
        match &e.kind {
            ExprKind::Num(v) => v.to_string(),
            ExprKind::Var { name, .. } => name.clone(),
            ExprKind::Assign(t, v) => format!("(= {} {})", sx(t), sx(v)),
            ExprKind::Binary(op, l, r) => format!("({} {} {})", op.symbol(), sx(l), sx(r)),
            ExprKind::Unary(op, x) => format!("({} {})", op.symbol(), sx(x)),
            ExprKind::Call(n, args) => {
                let mut s = format!("(call {n}");
                for a in args {
                    s.push(' ');
                    s.push_str(&sx(a));
                }
                s + ")"
            }
        }
    }

    /// A statement as a compact string, so tests read like the source.
    fn ss(s: &Stmt) -> String {
        let opt = |o: Option<String>| o.unwrap_or_else(|| "_".into());
        match &s.kind {
            StmtKind::Block(b) => format!("{{{}}}", b.iter().map(ss).collect::<Vec<_>>().join(" ")),
            StmtKind::Decl(ds) => {
                let parts: Vec<String> = ds
                    .iter()
                    .map(|d| match &d.init {
                        Some(e) => format!("{}={}", d.name, sx(e)),
                        None => d.name.clone(),
                    })
                    .collect();
                format!("(int {})", parts.join(" "))
            }
            StmtKind::Expr(e) => format!("{};", sx(e)),
            StmtKind::Empty => ";".into(),
            StmtKind::If { cond, then, els } => match els {
                Some(e) => format!("(if {} {} {})", sx(cond), ss(then), ss(e)),
                None => format!("(if {} {})", sx(cond), ss(then)),
            },
            StmtKind::While { cond, body } => format!("(while {} {})", sx(cond), ss(body)),
            StmtKind::For {
                init,
                cond,
                step,
                body,
            } => format!(
                "(for {} {} {} {})",
                opt(init.as_deref().map(ss)),
                opt(cond.as_ref().map(sx)),
                opt(step.as_ref().map(sx)),
                ss(body)
            ),
            StmtKind::Return(e) => format!("(return {})", sx(e)),
            StmtKind::Break => "break".into(),
            StmtKind::Continue => "continue".into(),
        }
    }

    fn ex(src: &str) -> String {
        match stmt(&format!("{src};")).kind {
            StmtKind::Expr(e) => sx(&e),
            k => panic!("not an expression statement: {k:?}"),
        }
    }

    #[test]
    fn functions_and_params() {
        let p = prog(
            "int a() { return 0; }
             int b(void) { return 1; }
             int c(int x, int y);
             int d(int p1, int p2, int p3, int p4, int p5, int p6, int p7, int p8) { return p8; }",
        );
        let shape: Vec<(String, Vec<String>, bool)> = p
            .funcs
            .iter()
            .map(|f| {
                let params = f.params.iter().map(|p| p.name.clone()).collect();
                (f.name.clone(), params, f.body.is_some())
            })
            .collect();
        let names = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            shape,
            vec![
                ("a".into(), vec![], true),
                ("b".into(), vec![], true),
                ("c".into(), names(&["x", "y"]), false),
                (
                    "d".into(),
                    names(&["p1", "p2", "p3", "p4", "p5", "p6", "p7", "p8"]),
                    true
                ),
            ]
        );
        assert_eq!(p.funcs[0].pos, Pos::new(1, 5));
        assert_eq!(p.funcs[2].params[1].pos, Pos::new(3, 31));
        assert!(prog("").funcs.is_empty());
    }

    #[test]
    fn blocks_and_empty_statements() {
        assert_eq!(ss(&stmt("{ ; { 1; } {} }")), "{; {1;} {}}");
        assert!(body("").is_empty());
    }

    #[test]
    fn declarations() {
        assert_eq!(ss(&stmt("int x;")), "(int x)");
        assert_eq!(ss(&stmt("int x = 1 + 2;")), "(int x=(+ 1 2))");
        assert_eq!(ss(&stmt("int a = 1, b, c = a;")), "(int a=1 b c=a)");
        let s = stmt("int a, b = 2;");
        let StmtKind::Decl(ds) = &s.kind else { panic!() };
        assert_eq!(s.pos, Pos::new(1, 11));
        assert_eq!(ds[1].pos, Pos::new(1, 18));
        assert!(
            ds.iter().all(|d| d.local.is_none()),
            "sema fills locals, not the parser"
        );
    }

    #[test]
    fn expression_statements() {
        assert_eq!(ss(&stmt("x = 1;")), "(= x 1);");
        assert_eq!(ss(&stmt("f(1);")), "(call f 1);");
        assert_eq!(ss(&stmt("1;")), "1;");
    }

    #[test]
    fn if_else() {
        assert_eq!(ss(&stmt("if (x) return 1;")), "(if x (return 1))");
        assert_eq!(
            ss(&stmt("if (x < 1) { y = 1; } else y = 2;")),
            "(if (< x 1) {(= y 1);} (= y 2);)"
        );
        // The else belongs to the nearest if.
        assert_eq!(ss(&stmt("if (a) if (b) x; else y;")), "(if a (if b x; y;))");
        assert_eq!(
            ss(&stmt("if (a) x; else if (b) y; else z;")),
            "(if a x; (if b y; z;))"
        );
    }

    #[test]
    fn while_loops() {
        assert_eq!(
            ss(&stmt("while (i < 10) i = i + 1;")),
            "(while (< i 10) (= i (+ i 1));)"
        );
        assert_eq!(
            ss(&stmt("while (1) { break; continue; }")),
            "(while 1 {break continue})"
        );
    }

    #[test]
    fn for_loops() {
        assert_eq!(
            ss(&stmt("for (int i = 0; i < 10; i = i + 1) s = s + i;")),
            "(for (int i=0) (< i 10) (= i (+ i 1)) (= s (+ s i));)"
        );
        assert_eq!(
            ss(&stmt("for (i = 0; i; i = i - 1) {}")),
            "(for (= i 0); i (= i (- i 1)) {})"
        );
        assert_eq!(ss(&stmt("for (;;) break;")), "(for _ _ _ break)");
        assert_eq!(
            ss(&stmt("for (int a = 1, b = 2; ; ) ;")),
            "(for (int a=1 b=2) _ _ ;)"
        );
    }

    #[test]
    fn jumps() {
        assert_eq!(ss(&stmt("return 1 + 2;")), "(return (+ 1 2))");
        assert_eq!(ss(&stmt("break;")), "break");
        assert_eq!(ss(&stmt("continue;")), "continue");
    }

    #[test]
    fn primaries() {
        assert_eq!(ex("42"), "42");
        assert_eq!(ex("0x10"), "16");
        assert_eq!(ex("x"), "x");
        assert_eq!(ex("(((x)))"), "x");
        assert_eq!(ex("f()"), "(call f)");
        assert_eq!(ex("f(a)"), "(call f a)");
        assert_eq!(
            ex("f(a, b + 1, g(c), d = 2)"),
            "(call f a (+ b 1) (call g c) (= d 2))"
        );
    }

    #[test]
    fn every_binary_operator() {
        for op in [
            "+", "-", "*", "/", "%", "==", "!=", "<", "<=", ">", ">=", "&&", "||",
        ] {
            assert_eq!(ex(&format!("a {op} b")), format!("({op} a b)"));
        }
    }

    #[test]
    fn precedence() {
        assert_eq!(ex("1 + 2 * 3"), "(+ 1 (* 2 3))");
        assert_eq!(ex("1 * 2 + 3"), "(+ (* 1 2) 3)");
        assert_eq!(ex("a % b - c / d"), "(- (% a b) (/ c d))");
        assert_eq!(ex("a + b < c - d"), "(< (+ a b) (- c d))");
        assert_eq!(ex("a < b == c >= d"), "(== (< a b) (>= c d))");
        assert_eq!(ex("a == b && c != d"), "(&& (== a b) (!= c d))");
        assert_eq!(ex("a || b && c"), "(|| a (&& b c))");
        assert_eq!(ex("a && b || c && d"), "(|| (&& a b) (&& c d))");
        assert_eq!(ex("x = a || b"), "(= x (|| a b))");
        assert_eq!(ex("(1 + 2) * 3"), "(* (+ 1 2) 3)");
        assert_eq!(ex("-a * b"), "(* (- a) b)");
        assert_eq!(ex("!a == b"), "(== (! a) b)");
        assert_eq!(ex("-(a + b)"), "(- (+ a b))");
        assert_eq!(ex("f(1) + 2 * g(3)"), "(+ (call f 1) (* 2 (call g 3)))");
    }

    #[test]
    fn associativity() {
        assert_eq!(ex("a - b - c"), "(- (- a b) c)");
        assert_eq!(ex("a / b / c"), "(/ (/ a b) c)");
        assert_eq!(ex("a < b < c"), "(< (< a b) c)");
        assert_eq!(ex("a || b || c"), "(|| (|| a b) c)");
        assert_eq!(ex("a = b = c"), "(= a (= b c))");
    }

    #[test]
    fn unary() {
        assert_eq!(ex("-1"), "(- 1)");
        assert_eq!(ex("!x"), "(! x)");
        assert_eq!(ex("- -x"), "(- (- x))");
        assert_eq!(ex("!!x"), "(! (! x))");
        assert_eq!(ex("-!x"), "(- (! x))");
        assert_eq!(ex("a - -b"), "(- a (- b))");
    }

    #[test]
    fn decrement_is_not_double_negation() {
        // In C `--x` decrements and `a--b` doesn't parse; neither may become `- -`.
        let front = |s: &str| {
            crate::parse(&format!("int f(int a, int b, int x) {{ {s} }}")).unwrap_err()[0].to_string()
        };
        assert_eq!(front("--x; return x;"), "1:30: '--' is not supported yet");
        assert_eq!(front("x--; return x;"), "1:31: '--' is not supported yet");
        assert_eq!(front("return a--b;"), "1:38: '--' is not supported yet");
        assert_eq!(front("return ++x;"), "1:37: '++' is not supported yet");
        assert_eq!(front("x -= 1; return x;"), "1:32: '-=' is not supported yet");
    }

    /// `n` copies of `open`, then `inner`, then `n` copies of `close`.
    fn nest(n: usize, open: &str, inner: &str, close: &str) -> String {
        format!("{}{inner}{}", open.repeat(n), close.repeat(n))
    }

    #[test]
    fn nesting_limit() {
        // Unoptimized, 256 levels need more than a test thread's 2 MiB of stack (the
        // merlin binary runs on a big stack for the same reason).
        std::thread::Builder::new()
            .stack_size(64 << 20)
            .spawn(nesting_limit_checks)
            .unwrap()
            .join()
            .unwrap();
    }

    fn nesting_limit_checks() {
        let deep = |n: usize| format!("int f() {{ {} }}", nest(n, "{", "", "}"));
        // `int f() { ` is 10 characters, so the 257th `{` is at column 267.
        assert!(parse(&lex(&deep(MAX_DEPTH)).unwrap()).is_ok());
        assert_eq!(
            err(&deep(MAX_DEPTH + 1)),
            "1:267: nesting too deep: merlin allows at most 256 levels"
        );
        let limit = |src: &str| {
            let e = err(src);
            assert!(
                e.ends_with("nesting too deep: merlin allows at most 256 levels"),
                "{e}"
            );
        };
        // A return statement and its expression take two levels.
        let ret = |e: &str| format!("int f(int x) {{ return {e}; }}");
        assert!(parse(&lex(&ret(&nest(MAX_DEPTH - 2, "(", "1", ")"))).unwrap()).is_ok());
        limit(&ret(&nest(MAX_DEPTH - 1, "(", "1", ")")));
        limit(&ret(&nest(100_000, "(", "1", ")")));
        limit(&ret(&nest(100_000, "- ", "1", "")));
        limit(&ret(&nest(100_000, "!", "1", "")));
        limit(&ret(&nest(100_000, "x = ", "1", "")));
        limit(&ret(&nest(100_000, "f(", "1", ")")));
        limit(&ret(&nest(100_000, "1 + ", "1", "")));
        limit(&ret(&nest(100_000, "", "1", " * 1")));
        limit(&format!("int f() {{ {} }}", nest(100_000, "if (1) ", ";", "")));
        limit(&format!("int f() {{ {} }}", nest(100_000, "while (1) ", ";", "")));
        limit(&format!(
            "int f() {{ {} return 0; }}",
            nest(100_000, "if (1) ; else ", ";", "")
        ));
        // Long but shallow is fine: a block's statements and a call's arguments are lists.
        assert_eq!(body(&"x = 1; ".repeat(100_000)).len(), 100_000);
        let args = vec!["1"; 100_000].join(", ");
        assert!(parse(&lex(&ret(&format!("f({args})"))).unwrap()).is_ok());
    }

    #[test]
    fn expression_positions() {
        let StmtKind::Expr(e) = stmt("x = a + -b;").kind else {
            panic!()
        };
        // Operators are positioned at the operator, leaves at themselves.
        assert_eq!(e.pos, Pos::new(1, 13));
        let ExprKind::Assign(t, v) = &e.kind else { panic!() };
        assert_eq!(t.pos, Pos::new(1, 11));
        assert_eq!(v.pos, Pos::new(1, 17));
        let ExprKind::Binary(_, _, neg) = &v.kind else {
            panic!()
        };
        assert_eq!(neg.pos, Pos::new(1, 19));
    }

    #[test]
    fn missing_semicolon() {
        // Reported right after the previous token, where the `;` belongs.
        assert_eq!(err("int f() { x = 1 }"), "1:16: expected ';' after expression");
        assert_eq!(
            err("int f() {\n  return 1\n}"),
            "2:11: expected ';' after return statement"
        );
        assert_eq!(
            err("int f() { int x = 1 int y; }"),
            "1:20: expected ';' at end of declaration"
        );
        assert_eq!(
            err("int f() { while (1) break }"),
            "1:26: expected ';' after 'break'"
        );
        assert_eq!(
            err("int f() { while (1) continue }"),
            "1:29: expected ';' after 'continue'"
        );
        assert_eq!(
            err("int f() { for (i = 0 i < 3; ) ; }"),
            "1:21: expected ';' in 'for' statement"
        );
        assert_eq!(
            err("int f() { for (;1) ; }"),
            "1:18: expected ';' in 'for' statement"
        );
    }

    #[test]
    fn syntax_errors() {
        assert_eq!(err("int f() { return (1 + 2; }"), "1:24: expected ')', found ';'");
        assert_eq!(
            err("int f() { return 1 + ; }"),
            "1:22: expected expression, found ';'"
        );
        assert_eq!(
            err("int f() { return 1;"),
            "1:20: expected '}', found end of file"
        );
        assert_eq!(
            err("int f() { if x) return 1; }"),
            "1:14: expected '(', found identifier 'x'"
        );
        assert_eq!(
            err("int f() { f(1 2); }"),
            "1:15: expected ',' or ')' in the argument list, found '2'"
        );
        // Literals are quoted as written.
        assert_eq!(
            err("int f() { f(1 0x7f); }"),
            "1:15: expected ',' or ')' in the argument list, found '0x7f'"
        );
        assert_eq!(
            err("int f(int a b) {}"),
            "1:13: expected ',' or ')' in the parameter list, found identifier 'b'"
        );
        assert_eq!(
            err("int f(x) {}"),
            "1:7: expected parameter type 'int', found identifier 'x'"
        );
        assert_eq!(
            err("int f() return 1;"),
            "1:9: expected '{' or ';' after the parameter list, found 'return'"
        );
        assert_eq!(
            err("f() {}"),
            "1:1: expected 'int' to start a function, found identifier 'f'"
        );
        assert_eq!(err("int 3() {}"), "1:5: expected function name, found '3'");
        assert_eq!(
            err("int f() { int 1; }"),
            "1:15: expected variable name, found '1'"
        );
        assert_eq!(err("int f() { else x; }"), "1:11: 'else' without a matching 'if'");
        assert_eq!(
            err("int f() { return; }"),
            "1:11: return needs a value: every function returns int"
        );
        assert_eq!(
            err("int f() { } }"),
            "1:13: expected 'int' to start a function, found '}'"
        );
    }

    #[test]
    fn unsupported_constructs() {
        assert_eq!(err("int x;"), "1:5: global variables are not supported yet");
        assert_eq!(err("int x = 1;"), "1:5: global variables are not supported yet");
        assert_eq!(
            err("int f() { if (1) int x = 1; }"),
            "1:18: a declaration is not a statement; wrap it in '{ }'"
        );
        assert_eq!(
            err("int f() { int g(int a); }"),
            "1:15: function declarations inside a function are not supported"
        );
        assert_eq!(
            err("int f(int a, int b, int c, int d, int e, int f, int g, int h, int i) {}"),
            "1:67: too many parameters: merlin supports at most 8 (a0-a7)"
        );
    }
}
