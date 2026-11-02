//! Tokens to AST, by recursive descent. Stops at the first error.
//!
//! ```text
//! program     := item* EOF
//! item        := specifiers declarator '(' params ')' (block | ';')     a function
//!              | specifiers init-decl (',' init-decl)* ';'              global variables
//! specifiers  := ('static' | 'extern' | 'void' | 'char' | 'short' | 'int'
//!                | 'long' | 'signed' | 'unsigned')+                     in any order, as in C
//! declarator  := '*'* IDENT ('[' constant? ']')*
//! init-decl   := declarator ('=' initializer)?
//! initializer := assign | '{' initializer (',' initializer)* ','? '}'
//! params      := ε | 'void' | param (',' param)*                        at most 8
//! param       := specifiers declarator                                  the name is optional
//!                                                                       in a prototype
//! type-name   := specifiers '*'* ('[' constant ']')*                    for casts and sizeof
//! block       := '{' (decl | stmt)* '}'
//! decl        := specifiers init-decl (',' init-decl)* ';'
//! stmt        := block | ';' | expr ';'
//!              | 'if' '(' expr ')' stmt ('else' stmt)?
//!              | 'while' '(' expr ')' stmt
//!              | 'do' stmt 'while' '(' expr ')' ';'
//!              | 'for' '(' (decl | expr? ';') expr? ';' expr? ')' stmt
//!              | 'return' expr? ';' | 'break' ';' | 'continue' ';'
//! expr        := assign (',' assign)*
//! assign      := cond (('=' | '+=' | '-=' | '*=' | '/=' | '%=' | '<<=' | '>>='
//!                      | '&=' | '|=' | '^=') assign)?                   right-associative
//! cond        := binary(1) ('?' expr ':' cond)?
//! binary(p)   := unary (OP binary(prec(OP) + 1))*                       for OP with prec >= p
//! unary       := ('++' | '--') unary | ('-' | '+' | '!' | '~' | '&' | '*') unary
//!              | '(' type-name ')' unary | 'sizeof' '(' type-name ')' | 'sizeof' unary
//!              | postfix
//! postfix     := primary ('[' expr ']' | '++' | '--')*
//! primary     := NUM | STR+ | IDENT | IDENT '(' (assign (',' assign)*)? ')' | '(' expr ')'
//! ```
//!
//! Binary precedence (precedence climbing), loosest first: `||` 1, `&&` 2,
//! `|` 3, `^` 4, `&` 5, `== !=` 6, `< <= > >=` 7, `<< >>` 8, `+ -` 9,
//! `* / %` 10. All left-associative.
//!
//! The parser already knows some types: integer literals carry theirs from the
//! lexer, a string literal is `char[len + 1]`, a cast has its target type, and
//! array sizes are evaluated here ([`crate::consteval`]), so declared types are
//! complete. `a[i]` becomes `*(a + i)`, which is what C defines it to mean.

use crate::ast::*;
use crate::consteval;
use crate::lexer::{Kw, Tok, Token};
use crate::types::{IntKind, Type, MAX_OBJECT_SIZE};
use crate::{Error, Pos};

/// Arguments travel in `a0-a7`; no stack-passed arguments yet.
pub const MAX_PARAMS: usize = 8;

/// How deeply statements and expressions may nest. Each nested statement, `=`,
/// unary operator, cast, parenthesized expression, `?:` or call argument is a
/// level, and so is each operator in a chain like `a + b + c` or `a[1][2]`
/// (the tree leans left). The parser, sema and the AST printer recurse once per
/// level, so the limit turns runaway input into an error instead of a stack
/// overflow (given a few MiB of stack in unoptimized builds: the merlin binary
/// runs on a 64 MiB thread). clang's bracket limit is also 256.
pub const MAX_DEPTH: usize = 256;

pub fn parse(toks: &[Token]) -> Result<Program, Error> {
    let mut p = Parser {
        toks,
        pos: 0,
        depth: 0,
    };
    let mut items = Vec::new();
    while *p.peek() != Tok::Eof {
        items.push(p.item()?);
    }
    Ok(Program { items })
}

fn binop(t: &Tok) -> Option<(BinOp, u8)> {
    let Tok::Punct(p) = t else { return None };
    Some(match *p {
        "||" => (BinOp::LogOr, 1),
        "&&" => (BinOp::LogAnd, 2),
        "|" => (BinOp::BitOr, 3),
        "^" => (BinOp::BitXor, 4),
        "&" => (BinOp::BitAnd, 5),
        "==" => (BinOp::Eq, 6),
        "!=" => (BinOp::Ne, 6),
        "<" => (BinOp::Lt, 7),
        "<=" => (BinOp::Le, 7),
        ">" => (BinOp::Gt, 7),
        ">=" => (BinOp::Ge, 7),
        "<<" => (BinOp::Shl, 8),
        ">>" => (BinOp::Shr, 8),
        "+" => (BinOp::Add, 9),
        "-" => (BinOp::Sub, 9),
        "*" => (BinOp::Mul, 10),
        "/" => (BinOp::Div, 10),
        "%" => (BinOp::Rem, 10),
        _ => return None,
    })
}

/// `=` (no operator) or a compound assignment and its operator.
fn assign_op(t: &Tok) -> Option<Option<BinOp>> {
    let Tok::Punct(p) = t else { return None };
    Some(match *p {
        "=" => None,
        "+=" => Some(BinOp::Add),
        "-=" => Some(BinOp::Sub),
        "*=" => Some(BinOp::Mul),
        "/=" => Some(BinOp::Div),
        "%=" => Some(BinOp::Rem),
        "<<=" => Some(BinOp::Shl),
        ">>=" => Some(BinOp::Shr),
        "&=" => Some(BinOp::BitAnd),
        "|=" => Some(BinOp::BitOr),
        "^=" => Some(BinOp::BitXor),
        _ => return None,
    })
}

/// Where a declaration appears: what storage classes and declarators it may have.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Ctx {
    File,
    Block,
    ForInit,
    Param,
    TypeName,
}

/// A parsed declarator: the name, the full type, and where the name is.
struct Decl {
    name: String,
    ty: Type,
    pos: Pos,
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

    /// Whether the current token starts a declaration (a type or storage class).
    fn at_decl(&self) -> bool {
        matches!(self.peek(), Tok::Kw(k) if k.starts_decl())
    }

    /// Whether the token after the current one starts a type name: `(` then a
    /// type keyword means a cast or `sizeof (type)`.
    fn type_name_follows(&self) -> bool {
        matches!(self.peek_at(1), Tok::Kw(k) if k.starts_decl())
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

    // ---- declarations ----

    /// Type specifiers and storage class, in any order (`unsigned static int`
    /// is as good as `static unsigned int`, as in C).
    fn specifiers(&mut self, ctx: Ctx) -> Result<(Type, Storage), Error> {
        let start = self.here();
        let mut storage = Storage::None;
        // How many times each type keyword appears.
        let (mut void, mut char, mut short, mut int, mut long, mut signed, mut unsigned) =
            (0, 0, 0, 0, 0, 0, 0);
        while let Tok::Kw(k) = *self.peek() {
            if !k.starts_decl() {
                break;
            }
            let pos = self.advance().pos;
            let count = match k {
                Kw::Static | Kw::Extern => {
                    let s = if k == Kw::Static {
                        Storage::Static
                    } else {
                        Storage::Extern
                    };
                    let allowed = match ctx {
                        Ctx::File => true,
                        Ctx::Block => k == Kw::Static,
                        Ctx::ForInit | Ctx::Param | Ctx::TypeName => false,
                    };
                    if !allowed {
                        return Err(Error::new(pos, format!("'{}' is not allowed here", k.as_str())));
                    }
                    if storage != Storage::None {
                        return Err(Error::new(pos, "at most one of 'static' and 'extern' is allowed"));
                    }
                    storage = s;
                    continue;
                }
                Kw::Void => &mut void,
                Kw::Char => &mut char,
                Kw::Short => &mut short,
                Kw::Int => &mut int,
                Kw::Long => &mut long,
                Kw::Signed => &mut signed,
                Kw::Unsigned => &mut unsigned,
                _ => unreachable!("starts_decl"),
            };
            *count += 1;
            if k == Kw::Long && *count == 2 {
                return Err(Error::new(pos, "'long long' is not supported yet"));
            }
            if *count > 1 {
                return Err(Error::new(pos, format!("duplicate '{}'", k.as_str())));
            }
        }
        if void + char + short + int + long + signed + unsigned == 0 {
            return Err(Error::new(
                self.here(),
                format!("expected a type, found {}", self.tok().describe()),
            ));
        }
        // At most one of void, char, short and long; `int` may join short and
        // long; `signed` or `unsigned` may join anything but void.
        let invalid = signed + unsigned > 1
            || void + char + short + long > 1
            || int + void + char > 1
            || void + signed + unsigned > 1;
        if invalid {
            return Err(Error::new(start, "invalid combination of type specifiers"));
        }
        let ty = if void == 1 {
            Type::Void
        } else if char == 1 {
            Type::Int(match (signed, unsigned) {
                (1, _) => IntKind::SChar,
                (_, 1) => IntKind::UChar,
                _ => IntKind::Char,
            })
        } else if short == 1 {
            Type::Int(if unsigned == 1 {
                IntKind::UShort
            } else {
                IntKind::Short
            })
        } else if long == 1 {
            Type::Int(if unsigned == 1 {
                IntKind::ULong
            } else {
                IntKind::Long
            })
        } else {
            Type::Int(if unsigned == 1 {
                IntKind::UInt
            } else {
                IntKind::Int
            })
        };
        Ok((ty, storage))
    }

    /// `'*'*`: pointers to `base`.
    fn pointers(&mut self, mut ty: Type) -> Type {
        while self.eat_punct("*") {
            ty = Type::ptr(ty);
        }
        ty
    }

    /// `('[' constant? ']')*` after a name: the array dimensions, applied to
    /// `elem` innermost last, so `int a[2][3]` is an array of 2 `int[3]`.
    /// Only the first dimension may be left out.
    fn array_suffix(&mut self, elem: Type) -> Result<Type, Error> {
        let mut dims = Vec::new();
        while self.is_punct("[") {
            let open = self.advance().pos;
            if self.eat_punct("]") {
                if !dims.is_empty() {
                    return Err(Error::new(open, "only the first array dimension may be left out"));
                }
                dims.push(None);
                continue;
            }
            let pos = self.here();
            let e = self.cond()?;
            self.expect_punct("]")?;
            let n = consteval::int(&e).ok_or_else(|| {
                Error::new(
                    pos,
                    "array size must be an integer constant expression \
                     (literals, operators, casts and sizeof(type))",
                )
            })?;
            if n <= 0 {
                return Err(Error::new(pos, format!("array size must be positive, not {n}")));
            }
            dims.push(Some(n));
        }
        let mut ty = elem;
        for n in dims.into_iter().rev() {
            let Some(elem_size) = ty.size() else {
                return Err(Error::new(
                    self.here(),
                    format!("array has incomplete element type '{ty}'"),
                ));
            };
            if elem_size as u64 * n.unwrap_or(1) as u64 > MAX_OBJECT_SIZE {
                return Err(Error::new(self.after_prev(), "array is too large"));
            }
            ty = Type::array(ty, n.map(|n| n as u32));
        }
        Ok(ty)
    }

    /// A declarator: pointers, the name and array dimensions. In a parameter
    /// list the name is optional.
    fn declarator(&mut self, base: &Type, ctx: Ctx) -> Result<Decl, Error> {
        let ty = self.pointers(base.clone());
        let pos = self.here();
        let name =
            match self.peek().clone() {
                Tok::Ident(name) => {
                    self.advance();
                    name
                }
                _ if ctx == Ctx::Param => String::new(),
                Tok::Punct("(") => return Err(Error::new(
                    pos,
                    "parenthesized declarators (function pointers, pointers to arrays) are not supported yet",
                )),
                _ => return Err(self.unexpected("a name")),
            };
        let ty = self.array_suffix(ty)?;
        Ok(Decl { name, ty, pos })
    }

    /// `'{' ... '}'` or an expression.
    fn initializer(&mut self) -> Result<Init, Error> {
        if !self.is_punct("{") {
            return Ok(Init::Expr(self.assign()?));
        }
        self.nested(|p| {
            let pos = p.advance().pos;
            let mut items = Vec::new();
            loop {
                if p.eat_punct("}") {
                    if items.is_empty() {
                        return Err(Error::new(
                            pos,
                            "empty initializer lists are not C99; write '{ 0 }'",
                        ));
                    }
                    return Ok(Init::List(items, pos));
                }
                items.push(p.initializer()?);
                if !p.eat_punct(",") && !p.is_punct("}") {
                    return Err(p.unexpected("',' or '}' in the initializer list"));
                }
            }
        })
    }

    /// The declarators after the specifiers, through the `;`.
    fn init_declarators(
        &mut self,
        base: &Type,
        storage: Storage,
        ctx: Ctx,
    ) -> Result<Vec<Declarator>, Error> {
        let mut decls = Vec::new();
        loop {
            let d = self.declarator(base, ctx)?;
            if self.is_punct("(") {
                let msg = if ctx == Ctx::File {
                    "declare a function on its own, not in a list of variables"
                } else {
                    "function declarations inside a function are not supported"
                };
                return Err(Error::new(d.pos, msg));
            }
            let init = if self.eat_punct("=") {
                Some(self.initializer()?)
            } else {
                None
            };
            decls.push(Declarator {
                name: d.name,
                ty: d.ty,
                storage,
                init,
                var: None,
                define: false,
                pos: d.pos,
            });
            if !self.eat_punct(",") {
                break;
            }
        }
        self.expect_semi("at end of declaration")?;
        Ok(decls)
    }

    /// A top-level function or declaration.
    fn item(&mut self) -> Result<Item, Error> {
        if !self.at_decl() {
            return Err(self.unexpected("a declaration"));
        }
        let (base, storage) = self.specifiers(Ctx::File)?;
        // Look ahead past the pointers and the name: `(` means a function.
        let save = self.pos;
        let ret = self.pointers(base.clone());
        if let (Tok::Ident(name), Tok::Punct("(")) = (self.peek().clone(), self.peek_at(1)) {
            let pos = self.advance().pos;
            self.advance(); // (
            return Ok(Item::Func(self.function(name, ret, storage, pos)?));
        }
        self.pos = save;
        Ok(Item::Decl(self.init_declarators(&base, storage, Ctx::File)?))
    }

    /// After `name(`: the parameters and the body or `;`.
    fn function(&mut self, name: String, ret: Type, storage: Storage, pos: Pos) -> Result<Function, Error> {
        let params = self.params()?;
        let body = if self.eat_punct(";") {
            None
        } else if self.is_punct("{") {
            if let Some(p) = params.iter().find(|p| p.name.is_empty()) {
                return Err(Error::new(
                    p.pos,
                    "parameter name omitted in a function definition",
                ));
            }
            Some(self.block_body()?)
        } else {
            return Err(self.unexpected("'{' or ';' after the parameter list"));
        };
        Ok(Function {
            name,
            ret,
            params,
            is_static: storage == Storage::Static,
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
            if !self.at_decl() {
                return Err(self.unexpected("a parameter type"));
            }
            let start = self.here();
            let (base, _) = self.specifiers(Ctx::Param)?;
            let d = self.declarator(&base, Ctx::Param)?;
            let pos = if d.name.is_empty() { start } else { d.pos };
            if params.len() == MAX_PARAMS {
                return Err(Error::new(
                    pos,
                    format!("too many parameters: merlin supports at most {MAX_PARAMS} (a0-a7)"),
                ));
            }
            // C adjusts a parameter declared as an array to a pointer.
            let ty = d.ty.decay();
            params.push(Param {
                name: d.name,
                ty,
                pos,
            });
            if self.eat_punct(")") {
                return Ok(params);
            }
            if !self.eat_punct(",") {
                return Err(self.unexpected("',' or ')' in the parameter list"));
            }
        }
    }

    /// A type name, for casts and `sizeof`: specifiers, pointers, dimensions.
    fn type_name(&mut self) -> Result<Type, Error> {
        let (base, _) = self.specifiers(Ctx::TypeName)?;
        let ty = self.pointers(base);
        if matches!(self.peek(), Tok::Ident(_)) {
            return Err(self.unexpected("')' after the type name"));
        }
        self.array_suffix(ty)
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
            items.push(if self.at_decl() {
                self.decl(Ctx::Block)?
            } else {
                self.stmt()?
            });
        }
    }

    /// `int a = 1, *b;`
    fn decl(&mut self, ctx: Ctx) -> Result<Stmt, Error> {
        let pos = self.here();
        let (base, storage) = self.specifiers(ctx)?;
        let decls = self.init_declarators(&base, storage, ctx)?;
        Ok(Stmt {
            kind: StmtKind::Decl(decls),
            pos,
        })
    }

    // ---- statements ----

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
            Tok::Kw(k) if k.starts_decl() => {
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
            Tok::Kw(Kw::Do) => {
                self.advance();
                let body = Box::new(self.stmt()?);
                if !self.eat_kw(Kw::While) {
                    return Err(self.unexpected("'while' after the body of 'do'"));
                }
                let cond = self.paren_expr()?;
                self.expect_semi("after 'do'/'while' statement")?;
                StmtKind::DoWhile { body, cond }
            }
            Tok::Kw(Kw::For) => {
                self.advance();
                self.expect_punct("(")?;
                let init = if self.at_decl() {
                    Some(Box::new(self.decl(Ctx::ForInit)?))
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
                if self.eat_punct(";") {
                    StmtKind::Return(None)
                } else {
                    let e = self.expr()?;
                    self.expect_semi("after return statement")?;
                    StmtKind::Return(Some(e))
                }
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

    // ---- expressions ----

    /// `a, b, c`: the comma operator.
    fn expr(&mut self) -> Result<Expr, Error> {
        let mut e = self.assign()?;
        let outer = self.depth;
        while self.is_punct(",") {
            self.enter()?;
            let pos = self.advance().pos;
            let rhs = self.assign()?;
            e = Expr::new(ExprKind::Comma(Box::new(e), Box::new(rhs)), pos);
        }
        self.depth = outer;
        Ok(e)
    }

    fn assign(&mut self) -> Result<Expr, Error> {
        self.nested(Self::assign_inner)
    }

    fn assign_inner(&mut self) -> Result<Expr, Error> {
        let lhs = self.cond()?;
        if let Some(op) = assign_op(self.peek()) {
            let pos = self.advance().pos;
            let rhs = self.assign()?;
            return Ok(Expr::new(
                ExprKind::Assign {
                    op,
                    target: Box::new(lhs),
                    value: Box::new(rhs),
                },
                pos,
            ));
        }
        Ok(lhs)
    }

    /// `c ? a : b`; the middle operand is a full expression, as in C.
    fn cond(&mut self) -> Result<Expr, Error> {
        let c = self.binary(1)?;
        if !self.is_punct("?") {
            return Ok(c);
        }
        self.nested(|p| {
            let pos = p.advance().pos;
            let a = p.expr()?;
            p.expect_punct(":")?;
            let b = p.cond()?;
            Ok(Expr::new(
                ExprKind::Cond(Box::new(c), Box::new(a), Box::new(b)),
                pos,
            ))
        })
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
            lhs = Expr::new(ExprKind::Binary(op, Box::new(lhs), Box::new(rhs)), pos);
        }
        self.depth = outer;
        Ok(lhs)
    }

    fn unary(&mut self) -> Result<Expr, Error> {
        let pos = self.here();
        let kind = match self.peek() {
            Tok::Punct(p @ ("++" | "--")) => {
                let inc = *p == "++";
                self.advance();
                let target = Box::new(self.nested(Self::unary)?);
                ExprKind::IncDec {
                    inc,
                    prefix: true,
                    target,
                }
            }
            Tok::Punct(p @ ("-" | "+" | "!" | "~")) => {
                let op = match *p {
                    "-" => UnOp::Neg,
                    "+" => UnOp::Plus,
                    "!" => UnOp::Not,
                    _ => UnOp::BitNot,
                };
                self.advance();
                ExprKind::Unary(op, Box::new(self.nested(Self::unary)?))
            }
            Tok::Punct("&") => {
                self.advance();
                ExprKind::Addr(Box::new(self.nested(Self::unary)?))
            }
            Tok::Punct("*") => {
                self.advance();
                ExprKind::Deref(Box::new(self.nested(Self::unary)?))
            }
            Tok::Punct("(") if self.type_name_follows() => {
                self.advance();
                let ty = self.type_name()?;
                self.expect_punct(")")?;
                let x = self.nested(Self::unary)?;
                return Ok(Expr {
                    kind: ExprKind::Cast {
                        x: Box::new(x),
                        implicit: false,
                    },
                    ty,
                    pos,
                });
            }
            Tok::Kw(Kw::Sizeof) => {
                self.advance();
                if self.is_punct("(") && self.type_name_follows() {
                    self.advance();
                    let ty = self.type_name()?;
                    self.expect_punct(")")?;
                    ExprKind::SizeofType(ty)
                } else {
                    ExprKind::SizeofExpr(Box::new(self.nested(Self::unary)?))
                }
            }
            _ => return self.postfix(),
        };
        Ok(Expr::new(kind, pos))
    }

    fn postfix(&mut self) -> Result<Expr, Error> {
        let mut e = self.primary()?;
        let outer = self.depth;
        loop {
            let kind = match self.peek() {
                Tok::Punct("[") => {
                    self.enter()?;
                    let pos = self.advance().pos;
                    let index = self.expr()?;
                    self.expect_punct("]")?;
                    // a[i] is *(a + i).
                    let sum = Expr::new(ExprKind::Binary(BinOp::Add, Box::new(e), Box::new(index)), pos);
                    e = Expr::new(ExprKind::Deref(Box::new(sum)), pos);
                    continue;
                }
                Tok::Punct(p @ ("++" | "--")) => ExprKind::IncDec {
                    inc: *p == "++",
                    prefix: false,
                    target: Box::new(e),
                },
                _ => break,
            };
            self.enter()?;
            let pos = self.advance().pos;
            e = Expr::new(kind, pos);
        }
        self.depth = outer;
        Ok(e)
    }

    fn primary(&mut self) -> Result<Expr, Error> {
        let pos = self.here();
        let kind = match self.peek().clone() {
            Tok::Num(v, k) => {
                self.advance();
                return Ok(Expr {
                    kind: ExprKind::Num(v),
                    ty: Type::Int(k),
                    pos,
                });
            }
            Tok::Str(_) => {
                // Adjacent string literals are one: "ab" "c" is "abc".
                let mut bytes = Vec::new();
                while let Tok::Str(s) = self.peek() {
                    bytes.extend_from_slice(s);
                    self.advance();
                }
                let len = bytes.len() as u32 + 1;
                return Ok(Expr {
                    kind: ExprKind::Str(bytes),
                    ty: Type::array(Type::CHAR, Some(len)),
                    pos,
                });
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
                    ExprKind::Var { name, var: None }
                }
            }
            Tok::Punct("(") => {
                // Keep the inner node; parentheses only steer the tree's shape.
                return self.paren_expr();
            }
            _ => return Err(self.unexpected("expression")),
        };
        Ok(Expr::new(kind, pos))
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

    fn funcs(p: &Program) -> Vec<&Function> {
        p.funcs().collect()
    }

    /// Body of `int f() { <body> }`.
    fn body(stmts: &str) -> Vec<Stmt> {
        let p = prog(&format!("int f() {{ {stmts} }}"));
        funcs(&p)[0].body.clone().unwrap()
    }

    fn stmt(s: &str) -> Stmt {
        let mut b = body(s);
        assert_eq!(b.len(), 1, "{s}");
        b.remove(0)
    }

    /// The expression as an s-expression, e.g. `(+ 1 (* 2 3))`.
    pub fn sx(e: &Expr) -> String {
        match &e.kind {
            ExprKind::Num(v) => v.to_string(),
            ExprKind::Str(b) => quote(b),
            ExprKind::Var { name, .. } => name.clone(),
            ExprKind::Assign { op, target, value } => {
                let op = op.map_or("", |o| o.symbol());
                format!("({op}= {} {})", sx(target), sx(value))
            }
            ExprKind::IncDec { inc, prefix, target } => {
                let op = if *inc { "++" } else { "--" };
                if *prefix {
                    format!("({op} {})", sx(target))
                } else {
                    format!("({} {op})", sx(target))
                }
            }
            ExprKind::Binary(op, l, r) => format!("({} {} {})", op.symbol(), sx(l), sx(r)),
            ExprKind::Unary(op, x) => format!("({} {})", op.symbol(), sx(x)),
            ExprKind::Addr(x) => format!("(& {})", sx(x)),
            ExprKind::Deref(x) => format!("(* {})", sx(x)),
            ExprKind::Cast { x, .. } => format!("(cast {} {})", e.ty, sx(x)),
            ExprKind::SizeofType(t) => format!("(sizeof {t})"),
            ExprKind::SizeofExpr(x) => format!("(sizeof {})", sx(x)),
            ExprKind::Cond(c, a, b) => format!("(? {} {} {})", sx(c), sx(a), sx(b)),
            ExprKind::Comma(a, b) => format!("(, {} {})", sx(a), sx(b)),
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

    fn si(i: &Init) -> String {
        match i {
            Init::Expr(e) => sx(e),
            Init::List(items, _) => format!("{{{}}}", items.iter().map(si).collect::<Vec<_>>().join(" ")),
        }
    }

    fn sd(d: &Declarator) -> String {
        let storage = match d.storage {
            Storage::None => "",
            Storage::Static => "static ",
            Storage::Extern => "extern ",
        };
        match &d.init {
            Some(i) => format!("{storage}{}={}", d.ty.declare(&d.name), si(i)),
            None => format!("{storage}{}", d.ty.declare(&d.name)),
        }
    }

    /// A statement as a compact string, so tests read like the source.
    fn ss(s: &Stmt) -> String {
        let opt = |o: Option<String>| o.unwrap_or_else(|| "_".into());
        match &s.kind {
            StmtKind::Block(b) => format!("{{{}}}", b.iter().map(ss).collect::<Vec<_>>().join(" ")),
            StmtKind::Decl(ds) => format!("(decl {})", ds.iter().map(sd).collect::<Vec<_>>().join(", ")),
            StmtKind::Expr(e) => format!("{};", sx(e)),
            StmtKind::Empty => ";".into(),
            StmtKind::If { cond, then, els } => match els {
                Some(e) => format!("(if {} {} {})", sx(cond), ss(then), ss(e)),
                None => format!("(if {} {})", sx(cond), ss(then)),
            },
            StmtKind::While { cond, body } => format!("(while {} {})", sx(cond), ss(body)),
            StmtKind::DoWhile { body, cond } => format!("(do {} {})", ss(body), sx(cond)),
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
            StmtKind::Return(Some(e)) => format!("(return {})", sx(e)),
            StmtKind::Return(None) => "(return)".into(),
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

    /// The global declarations of a program, compactly.
    fn globals(src: &str) -> Vec<String> {
        prog(src)
            .items
            .iter()
            .filter_map(|i| match i {
                Item::Decl(ds) => Some(ds.iter().map(sd).collect::<Vec<_>>().join(", ")),
                Item::Func(_) => None,
            })
            .collect()
    }

    #[test]
    fn functions_and_params() {
        let p = prog(
            "int a() { return 0; }
             int b(void) { return 1; }
             int c(int x, int y);
             int d(int p1, int p2, int p3, int p4, int p5, int p6, int p7, int p8) { return p8; }
             char *e(unsigned char *s, int v[], long n);
             static void f(int, char *);",
        );
        let shape: Vec<(String, String, bool)> = funcs(&p)
            .iter()
            .map(|f| {
                let params: Vec<String> = f.params.iter().map(|p| p.ty.declare(&p.name)).collect();
                let decl = f.ret.declare(&format!("{}({})", f.name, params.join(", ")));
                (decl, format!("static={}", f.is_static), f.body.is_some())
            })
            .collect();
        let row = |d: &str, s: bool, b: bool| (d.to_string(), format!("static={s}"), b);
        assert_eq!(
            shape,
            vec![
                row("int a()", false, true),
                row("int b()", false, true),
                row("int c(int x, int y)", false, false),
                row(
                    "int d(int p1, int p2, int p3, int p4, int p5, int p6, int p7, int p8)",
                    false,
                    true
                ),
                // Array parameters are pointers.
                row("char *e(unsigned char *s, int *v, long n)", false, false),
                row("void f(int, char *)", true, false),
            ]
        );
        assert_eq!(funcs(&p)[0].pos, Pos::new(1, 5));
        assert_eq!(funcs(&p)[2].params[1].pos, Pos::new(3, 31));
        assert!(prog("").items.is_empty());
    }

    #[test]
    fn type_specifiers() {
        let ty = |s: &str| globals(&format!("{s} x;"))[0].clone();
        assert_eq!(ty("int"), "int x");
        assert_eq!(ty("signed"), "int x");
        assert_eq!(ty("signed int"), "int x");
        assert_eq!(ty("unsigned"), "unsigned int x");
        assert_eq!(ty("int unsigned"), "unsigned int x");
        assert_eq!(ty("char"), "char x");
        assert_eq!(ty("signed char"), "signed char x");
        assert_eq!(ty("unsigned char"), "unsigned char x");
        assert_eq!(ty("short"), "short x");
        assert_eq!(ty("short int"), "short x");
        assert_eq!(ty("unsigned short int"), "unsigned short x");
        assert_eq!(ty("long"), "long x");
        assert_eq!(ty("long int"), "long x");
        assert_eq!(ty("unsigned long"), "unsigned long x");
        assert_eq!(ty("static int"), "static int x");
        assert_eq!(ty("int static"), "static int x");
        assert_eq!(ty("extern char"), "extern char x");
        for bad in [
            "signed unsigned",
            "char int",
            "short char",
            "short long",
            "void int",
            "unsigned void",
        ] {
            assert_eq!(
                err(&format!("{bad} x;")),
                "1:1: invalid combination of type specifiers",
                "{bad}"
            );
        }
        assert_eq!(err("int int x;"), "1:5: duplicate 'int'");
        assert_eq!(err("long long x;"), "1:6: 'long long' is not supported yet");
        assert_eq!(err("static x;"), "1:8: expected a type, found identifier 'x'");
        assert_eq!(
            err("static extern int x;"),
            "1:8: at most one of 'static' and 'extern' is allowed"
        );
    }

    #[test]
    fn declarators() {
        assert_eq!(
            globals("int *p, **pp, a[3], m[2][4], *ap[5];"),
            ["int *p, int **pp, int a[3], int m[2][4], int *ap[5]"]
        );
        assert_eq!(
            globals("int a[] = {1, 2, 3,}; char s[] = \"hi\"; int m[2][2] = {{1}, {2, 3}};"),
            ["int a[]={1 2 3}", "char s[]=\"hi\"", "int m[2][2]={{1} {2 3}}"]
        );
        assert_eq!(
            globals("char buf[4 * sizeof(int) + 1]; int a[(char)258];"),
            ["char buf[17]", "int a[2]"]
        );
        assert_eq!(err("int a[0];"), "1:7: array size must be positive, not 0");
        assert_eq!(err("int a[-1];"), "1:7: array size must be positive, not -1");
        assert_eq!(
            err("int n; int a[n];"),
            "1:14: array size must be an integer constant expression (literals, operators, casts and sizeof(type))"
        );
        assert_eq!(
            err("int a[2][];"),
            "1:9: only the first array dimension may be left out"
        );
        assert_eq!(err("int a[0x40000000][2];"), "1:21: array is too large");
        assert_eq!(
            err("void a[2];"),
            "1:10: array has incomplete element type 'void'"
        );
        assert_eq!(
            err("int (*f)(int);"),
            "1:5: parenthesized declarators (function pointers, pointers to arrays) are not supported yet"
        );
        assert_eq!(
            err("int x, f(int);"),
            "1:8: declare a function on its own, not in a list of variables"
        );
        assert_eq!(
            err("int a[2] = {};"),
            "1:12: empty initializer lists are not C99; write '{ 0 }'"
        );
    }

    #[test]
    fn blocks_and_empty_statements() {
        assert_eq!(ss(&stmt("{ ; { 1; } {} }")), "{; {1;} {}}");
        assert!(body("").is_empty());
    }

    #[test]
    fn declarations() {
        assert_eq!(ss(&stmt("int x;")), "(decl int x)");
        assert_eq!(ss(&stmt("int x = 1 + 2;")), "(decl int x=(+ 1 2))");
        assert_eq!(
            ss(&stmt("int a = 1, b, c = a;")),
            "(decl int a=1, int b, int c=a)"
        );
        assert_eq!(
            ss(&stmt("unsigned char *p = 0, buf[8] = {1, 2};")),
            "(decl unsigned char *p=0, unsigned char buf[8]={1 2})"
        );
        assert_eq!(ss(&stmt("static int n = 3;")), "(decl static int n=3)");
        let s = stmt("int a, b = 2;");
        let StmtKind::Decl(ds) = &s.kind else { panic!() };
        assert_eq!(s.pos, Pos::new(1, 11));
        assert_eq!(ds[1].pos, Pos::new(1, 18));
        assert!(
            ds.iter().all(|d| d.var.is_none()),
            "sema resolves variables, not the parser"
        );
        assert_eq!(
            err("int f() { extern int x; }"),
            "1:11: 'extern' is not allowed here"
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
    fn loops() {
        assert_eq!(
            ss(&stmt("while (i < 10) i = i + 1;")),
            "(while (< i 10) (= i (+ i 1));)"
        );
        assert_eq!(
            ss(&stmt("while (1) { break; continue; }")),
            "(while 1 {break continue})"
        );
        assert_eq!(ss(&stmt("do x++; while (x < 3);")), "(do (x ++); (< x 3))");
        assert_eq!(ss(&stmt("do { } while (0);")), "(do {} 0)");
        assert_eq!(
            err("int f() { do x; until (1); }"),
            "1:17: expected 'while' after the body of 'do', found identifier 'until'"
        );
        assert_eq!(
            err("int f() { do ; while (1) }"),
            "1:25: expected ';' after 'do'/'while' statement"
        );
        assert_eq!(
            ss(&stmt("for (int i = 0; i < 10; i = i + 1) s = s + i;")),
            "(for (decl int i=0) (< i 10) (= i (+ i 1)) (= s (+ s i));)"
        );
        assert_eq!(ss(&stmt("for (i = 0; i; i--) {}")), "(for (= i 0); i (i --) {})");
        assert_eq!(ss(&stmt("for (;;) break;")), "(for _ _ _ break)");
        assert_eq!(
            ss(&stmt("for (char *p = s, c; ; ) ;")),
            "(for (decl char *p=s, char c) _ _ ;)"
        );
        assert_eq!(
            err("int f() { for (static int i = 0;;) ; }"),
            "1:16: 'static' is not allowed here"
        );
    }

    #[test]
    fn jumps() {
        assert_eq!(ss(&stmt("return 1 + 2;")), "(return (+ 1 2))");
        assert_eq!(ss(&stmt("return;")), "(return)");
        assert_eq!(ss(&stmt("break;")), "break");
        assert_eq!(ss(&stmt("continue;")), "continue");
    }

    #[test]
    fn primaries() {
        assert_eq!(ex("42"), "42");
        assert_eq!(ex("0x10"), "16");
        assert_eq!(ex("'A'"), "65");
        assert_eq!(ex("x"), "x");
        assert_eq!(ex("(((x)))"), "x");
        assert_eq!(ex("f()"), "(call f)");
        assert_eq!(ex("f(a)"), "(call f a)");
        assert_eq!(
            ex("f(a, b + 1, g(c), d = 2)"),
            "(call f a (+ b 1) (call g c) (= d 2))"
        );
        assert_eq!(ex(r#""ab" "c""#), r#""abc""#);
    }

    #[test]
    fn literal_types() {
        let StmtKind::Expr(e) = stmt("f(1, 2u, 0xffffffff, 'a', 3L, \"hey\");").kind else {
            panic!()
        };
        let ExprKind::Call(_, args) = e.kind else { panic!() };
        let tys: Vec<String> = args.iter().map(|a| a.ty.to_string()).collect();
        assert_eq!(
            tys,
            ["int", "unsigned int", "unsigned int", "int", "long", "char [4]"]
        );
    }

    #[test]
    fn every_binary_operator() {
        for op in [
            "+", "-", "*", "/", "%", "<<", ">>", "&", "|", "^", "==", "!=", "<", "<=", ">", ">=", "&&", "||",
        ] {
            assert_eq!(ex(&format!("a {op} b")), format!("({op} a b)"));
        }
    }

    #[test]
    fn assignment_operators() {
        for op in ["=", "+=", "-=", "*=", "/=", "%=", "<<=", ">>=", "&=", "|=", "^="] {
            assert_eq!(ex(&format!("a {op} b")), format!("({op} a b)"));
        }
        assert_eq!(ex("a += b -= c"), "(+= a (-= b c))");
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
        // The bitwise operators bind looser than comparisons, a classic C trap.
        assert_eq!(ex("a & b == c"), "(& a (== b c))");
        assert_eq!(ex("a | b ^ c & d"), "(| a (^ b (& c d)))");
        assert_eq!(ex("a << 1 + b"), "(<< a (+ 1 b))");
        assert_eq!(ex("a < b << c"), "(< a (<< b c))");
        assert_eq!(ex("a && b | c"), "(&& a (| b c))");
        // ?: is right-associative and binds looser than ||, tighter than = and ,.
        assert_eq!(ex("a ? b : c ? d : e"), "(? a b (? c d e))");
        assert_eq!(ex("a || b ? c : d"), "(? (|| a b) c d)");
        assert_eq!(ex("x = a ? b : c"), "(= x (? a b c))");
        assert_eq!(ex("a ? b, c : d"), "(? a (, b c) d)");
        assert_eq!(ex("a = 1, b = 2"), "(, (= a 1) (= b 2))");
        assert_eq!(ex("f((a, b), c)"), "(call f (, a b) c)");
    }

    #[test]
    fn associativity() {
        assert_eq!(ex("a - b - c"), "(- (- a b) c)");
        assert_eq!(ex("a / b / c"), "(/ (/ a b) c)");
        assert_eq!(ex("a < b < c"), "(< (< a b) c)");
        assert_eq!(ex("a || b || c"), "(|| (|| a b) c)");
        assert_eq!(ex("a = b = c"), "(= a (= b c))");
        assert_eq!(ex("a << b >> c"), "(>> (<< a b) c)");
        assert_eq!(ex("a, b, c"), "(, (, a b) c)");
    }

    #[test]
    fn unary() {
        assert_eq!(ex("-1"), "(- 1)");
        assert_eq!(ex("!x"), "(! x)");
        assert_eq!(ex("~x"), "(~ x)");
        assert_eq!(ex("+x"), "(+ x)");
        assert_eq!(ex("- -x"), "(- (- x))");
        assert_eq!(ex("!!x"), "(! (! x))");
        assert_eq!(ex("-!x"), "(- (! x))");
        assert_eq!(ex("a - -b"), "(- a (- b))");
        assert_eq!(ex("&x"), "(& x)");
        assert_eq!(ex("*p"), "(* p)");
        assert_eq!(ex("**pp"), "(* (* pp))");
        assert_eq!(ex("*p + 1"), "(+ (* p) 1)");
        assert_eq!(ex("a & &b"), "(& a (& b))");
        assert_eq!(ex("a * *b"), "(* a (* b))");
    }

    #[test]
    fn increment_and_decrement() {
        assert_eq!(ex("++x"), "(++ x)");
        assert_eq!(ex("--x"), "(-- x)");
        assert_eq!(ex("x++"), "(x ++)");
        assert_eq!(ex("x--"), "(x --)");
        // C's longest match: `a+++b` is `(a++) + b`, `a---b` is `(a--) - b`.
        assert_eq!(ex("a+++b"), "(+ (a ++) b)");
        assert_eq!(ex("a---b"), "(- (a --) b)");
        assert_eq!(ex("- -x"), "(- (- x))");
        assert_eq!(ex("*p++"), "(* (p ++))");
        assert_eq!(ex("(*p)++"), "((* p) ++)");
        assert_eq!(ex("++*p"), "(++ (* p))");
        assert_eq!(ex("-x++"), "(- (x ++))");
    }

    #[test]
    fn subscripts_are_pointer_arithmetic() {
        assert_eq!(ex("a[i]"), "(* (+ a i))");
        assert_eq!(ex("m[i][j]"), "(* (+ (* (+ m i)) j))");
        assert_eq!(
            ex("a[i + 1] = b[f(2)]"),
            "(= (* (+ a (+ i 1))) (* (+ b (call f 2))))"
        );
        assert_eq!(ex("&a[1]"), "(& (* (+ a 1)))");
        assert_eq!(ex("a[i]++"), "((* (+ a i)) ++)");
        assert_eq!(ex("a[1, 2]"), "(* (+ a (, 1 2)))");
    }

    #[test]
    fn casts_and_sizeof() {
        assert_eq!(ex("(char)x"), "(cast char x)");
        assert_eq!(ex("(unsigned long *)p + 1"), "(+ (cast unsigned long * p) 1)");
        assert_eq!(ex("(int)(char)x"), "(cast int (cast char x))");
        assert_eq!(ex("(void)f()"), "(cast void (call f))");
        assert_eq!(ex("(x)"), "x");
        assert_eq!(ex("(x) - 1"), "(- x 1)", "(x) is not a cast: x is no type");
        assert_eq!(ex("sizeof(int)"), "(sizeof int)");
        assert_eq!(ex("sizeof(char *[4])"), "(sizeof char *[4])");
        assert_eq!(ex("sizeof x"), "(sizeof x)");
        assert_eq!(ex("sizeof(x)"), "(sizeof x)");
        assert_eq!(ex("sizeof x + 1"), "(+ (sizeof x) 1)");
        assert_eq!(ex("sizeof -x"), "(sizeof (- x))");
        assert_eq!(ex("sizeof a[0]"), "(sizeof (* (+ a 0)))");
        assert_eq!(ex("sizeof(int) * 2"), "(* (sizeof int) 2)");
        assert_eq!(
            err("int f() { return (int x)1; }"),
            "1:23: expected ')' after the type name, found identifier 'x'"
        );
        assert_eq!(
            err("int f() { return (static int)1; }"),
            "1:19: 'static' is not allowed here"
        );
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
        limit(&ret(&nest(100_000, "~", "1", "")));
        limit(&ret(&nest(100_000, "*", "1", "")));
        limit(&ret(&nest(100_000, "& ", "1", "")));
        limit(&ret(&nest(100_000, "++", "x", "")));
        limit(&ret(&nest(100_000, "(int)", "1", "")));
        limit(&ret(&nest(100_000, "sizeof ", "1", "")));
        limit(&ret(&nest(100_000, "x = ", "1", "")));
        limit(&ret(&nest(100_000, "x += ", "1", "")));
        limit(&ret(&nest(100_000, "f(", "1", ")")));
        limit(&ret(&nest(100_000, "1 + ", "1", "")));
        limit(&ret(&nest(100_000, "", "1", " * 1")));
        limit(&ret(&nest(100_000, "1 ? 1 : ", "1", "")));
        limit(&ret(&nest(100_000, "", "x", "[0]")));
        limit(&ret(&nest(100_000, "", "x", "++")));
        limit(&ret(&format!("({})", nest(100_000, "", "1", ", 1"))));
        limit(&format!("int f() {{ {} }}", nest(100_000, "if (1) ", ";", "")));
        limit(&format!("int f() {{ {} }}", nest(100_000, "while (1) ", ";", "")));
        limit(&format!(
            "int f() {{ {} }}",
            nest(100_000, "do ", ";", " while (1);")
        ));
        limit(&format!(
            "int f() {{ {} return 0; }}",
            nest(100_000, "if (1) ; else ", ";", "")
        ));
        limit(&format!("int a[1] = {};", nest(100_000, "{", "1", "}")));
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
        let ExprKind::Assign { target, value, .. } = &e.kind else {
            panic!()
        };
        assert_eq!(target.pos, Pos::new(1, 11));
        assert_eq!(value.pos, Pos::new(1, 17));
        let ExprKind::Binary(_, _, neg) = &value.kind else {
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
            err("int x = 1 int y;"),
            "1:10: expected ';' at end of declaration"
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
            "1:7: expected a parameter type, found identifier 'x'"
        );
        assert_eq!(
            err("int f(int) { return 0; }"),
            "1:7: parameter name omitted in a function definition"
        );
        assert_eq!(
            err("int f() return 1;"),
            "1:9: expected '{' or ';' after the parameter list, found 'return'"
        );
        assert_eq!(err("f() {}"), "1:1: expected a declaration, found identifier 'f'");
        assert_eq!(err("int 3() {}"), "1:5: expected a name, found '3'");
        assert_eq!(err("int f() { int 1; }"), "1:15: expected a name, found '1'");
        assert_eq!(err("int f() { else x; }"), "1:11: 'else' without a matching 'if'");
        assert_eq!(err("int f() { } }"), "1:13: expected a declaration, found '}'");
        assert_eq!(err("int f() { a ? b; }"), "1:16: expected ':', found ';'");
        assert_eq!(err("int f() { a[1; }"), "1:14: expected ']', found ';'");
        assert_eq!(
            err("int a[2] = {1 2};"),
            "1:15: expected ',' or '}' in the initializer list, found '2'"
        );
    }

    #[test]
    fn unsupported_constructs() {
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
