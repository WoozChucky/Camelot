//! Tokens to AST, by recursive descent. Stops at the first error.
//!
//! ```text
//! program     := item* EOF
//! item        := specifiers declarator block                            a function definition
//!              | specifiers (init-decl (',' init-decl)*)? ';'           declarations
//! specifiers  := (storage | qualifier | 'inline' | type-specifier)+    in any order, as in C
//! storage     := 'static' | 'extern' | 'typedef'
//! qualifier   := 'const' | 'volatile' | 'restrict'
//! type-specifier := 'void' | 'char' | 'short' | 'int' | 'long' | 'signed' | 'unsigned' | '_Bool'
//!              | struct-spec | enum-spec | TYPEDEF-NAME
//! struct-spec := ('struct' | 'union') IDENT? '{' member-decl+ '}' | ('struct' | 'union') IDENT
//! member-decl := specifiers declarator (',' declarator)* ';'
//! enum-spec   := 'enum' IDENT? '{' enumerator (',' enumerator)* ','? '}' | 'enum' IDENT
//! enumerator  := IDENT ('=' constant)?
//! declarator  := ('*' qualifier*)* direct suffix*
//! direct      := IDENT | '(' declarator ')' | ε                         ε only in parameters and type names
//! suffix      := '[' constant? ']' | '(' params ')'
//! init-decl   := declarator ('=' initializer)?
//! initializer := assign | '{' item (',' item)* ','? '}'
//! item        := (designator+ '=')? initializer
//! designator  := '.' IDENT | '[' constant ']'
//! params      := ε | 'void' | param (',' param)* (',' '...')?          at most 127
//! param       := specifiers declarator                                  the name is optional
//!                                                                       in a prototype
//! type-name   := specifiers declarator                                  with no name: casts, sizeof
//! block       := '{' (decl | stmt)* '}'
//! decl        := specifiers (init-decl (',' init-decl)*)? ';'
//! stmt        := block | ';' | expr ';' | IDENT ':' stmt
//!              | 'if' '(' expr ')' stmt ('else' stmt)?
//!              | 'switch' '(' expr ')' stmt | 'case' constant ':' stmt | 'default' ':' stmt
//!              | 'while' '(' expr ')' stmt
//!              | 'do' stmt 'while' '(' expr ')' ';'
//!              | 'for' '(' (decl | expr? ';') expr? ';' expr? ')' stmt
//!              | 'return' expr? ';' | 'break' ';' | 'continue' ';' | 'goto' IDENT ';'
//! expr        := assign (',' assign)*
//! assign      := cond (('=' | '+=' | '-=' | '*=' | '/=' | '%=' | '<<=' | '>>='
//!                      | '&=' | '|=' | '^=') assign)?                   right-associative
//! cond        := binary(1) ('?' expr ':' cond)?
//! binary(p)   := unary (OP binary(prec(OP) + 1))*                       for OP with prec >= p
//! unary       := ('++' | '--') unary | ('-' | '+' | '!' | '~' | '&' | '*') unary
//!              | '(' type-name ')' unary | 'sizeof' '(' type-name ')' | 'sizeof' unary
//!              | postfix
//! postfix     := (primary | '(' type-name ')' '{' ... '}') ops           a compound literal
//! ops         := ('[' expr ']' | '(' (assign (',' assign)*)? ')'
//!                 | '.' IDENT | '->' IDENT | '++' | '--')*
//! primary     := NUM | STR+ | IDENT | '(' expr ')' | va-builtin
//! va-builtin  := '__builtin_va_start' '(' assign ',' assign ')'
//!              | '__builtin_va_arg' '(' assign ',' type-name ')'
//!              | '__builtin_va_end' '(' assign ')'
//!              | '__builtin_va_copy' '(' assign ',' assign ')'
//! ```
//!
//! The parser sees tokens after preprocessing ([`crate::preprocess`]), so
//! macros and `#include` are gone by then. `__builtin_va_list` is a typedef
//! name from the start (for `void *`).
//!
//! Binary precedence (precedence climbing), loosest first: `||` 1, `&&` 2,
//! `|` 3, `^` 4, `&` 5, `== !=` 6, `< <= > >=` 7, `<< >>` 8, `+ -` 9,
//! `* / %` 10. All left-associative.
//!
//! The parser already knows some types: integer literals carry theirs from the
//! lexer, a string literal is `char[len + 1]`, a cast has its target type, and
//! array sizes are evaluated here ([`crate::consteval`]), so declared types are
//! complete. `a[i]` becomes `*(a + i)`, which is what C defines it to mean.
//!
//! ## Names the parser must know
//!
//! C cannot be parsed without knowing which identifiers are typedef names:
//! `T * x;` declares a pointer when `T` is a type and multiplies otherwise. So
//! the parser keeps block scopes of its own ([`Scope`]) for what it needs to
//! resolve: typedef names (replaced by their types), enumeration constants
//! (replaced by their values, so `int a[N]` works) and struct, union and enum
//! tags (each `struct tag { ... }` creates one [`Record`], which every later
//! `struct tag` in scope shares). Variables and functions are recorded only
//! as names that hide an outer typedef; sema resolves them.
//!
//! ## Declarators
//!
//! A declarator is read inside out, as C defines it. `int *(*fp[3])(char)`
//! applies to `int`, in order: `*` (pointer to int), then the outer suffix
//! `(char)` (function taking char, returning that), then the parenthesized
//! declarator's `*` and `[3]`, innermost last: `fp` is an array of 3 pointers
//! to functions. [`Parser::declarator_ops`] collects these steps
//! ([`Derive`]) and [`Parser::declarator`] applies them to the base type.

use crate::ast::*;
use crate::consteval;
use crate::lexer::{Kw, Tok, Token};
use crate::types::{
    align_up, common, ptr_size, EnumType, IntKind, Quals, Record, Spelled, Spelling, Type, TypedefName, Xlen,
    MAX_OBJECT_SIZE,
};
use crate::{Error, Pos};
use std::collections::HashMap;
use std::sync::Arc;

/// At most this many parameters in a function, and arguments in a call:
/// C99's translation limit (5.2.4.1), which keeps every argument within
/// reach of a 12-bit offset from `sp`.
pub const MAX_PARAMS: usize = 127;

/// How deeply statements and expressions may nest. Each nested statement, `=`,
/// unary operator, cast, parenthesized expression, `?:` or call argument is a
/// level, and so is each operator in a chain like `a + b + c` or `a[1][2]`
/// (the tree leans left). The parser, sema and the AST printer recurse once per
/// level, so the limit turns runaway input into an error instead of a stack
/// overflow (given a few MiB of stack in unoptimized builds: the merlin binary
/// runs on a 64 MiB thread). clang's bracket limit is also 256.
pub const MAX_DEPTH: usize = 256;

/// Parses a translation unit for the target `xlen`, whose sizes the parser
/// needs for array bounds, `sizeof` and struct layouts.
pub fn parse(toks: &[Token], xlen: Xlen) -> Result<Program, Error> {
    // The type clang's `<stdarg.h>` builds `va_list` from: on RISC-V a plain
    // pointer into the argument words (see `crate::codegen`). A typedef of
    // no position, as clang describes it under `-g`.
    let mut file = Scope::default();
    let va_list = Spelling::Typedef(Arc::new(TypedefName {
        name: "__builtin_va_list".to_string(),
        ty: Type::ptr(Type::Void),
        spelled: Spelled::default(),
        pos: Pos::default(),
    }));
    file.names.insert(
        "__builtin_va_list".to_string(),
        (
            Name::Typedef(Type::ptr(Type::Void), Spelled(Some(va_list))),
            Pos::default(),
        ),
    );
    let mut p = Parser {
        toks,
        pos: 0,
        depth: 0,
        scopes: vec![file],
        xlen,
    };
    let mut items = Vec::new();
    while *p.peek() != Tok::Eof {
        if let Some(item) = p.item()? {
            items.push(item);
        }
    }
    Ok(Program { items, xlen })
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

/// The length an initializer gives an array of unknown size with elements
/// of type `elem`, when it is a plain one: a string literal (or one in
/// braces) for a character array, or a list of scalars without designators.
/// `None` for the rest (brace elision, designators), which sema works out.
fn init_len(elem: &Type, init: &Init) -> Option<u32> {
    // A character type: 1 byte, on every target.
    let is_char = elem.int_kind().is_some_and(|k| k.size(Xlen::Rv32) == 1);
    let string = |i: &Init| match i {
        Init::Expr(Expr {
            kind: ExprKind::Str(b),
            ..
        }) if is_char => u32::try_from(b.len() + 1).ok(),
        _ => None,
    };
    match init {
        Init::List(items, _) if items.len() == 1 && string(&items[0]).is_some() => string(&items[0]),
        Init::List(items, _) if elem.is_scalar() && items.iter().all(|i| matches!(i, Init::Expr(_))) => {
            u32::try_from(items.len()).ok()
        }
        _ => string(init),
    }
}

/// Where a declaration appears: what storage classes and declarators it may have.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Ctx {
    File,
    Block,
    ForInit,
    Param,
    /// A struct or union member.
    Member,
    TypeName,
}

/// The storage class a declaration's specifiers give.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Class {
    None,
    Static,
    Extern,
    Typedef,
}

/// What a list of declaration specifiers says.
struct Specs {
    ty: Type,
    class: Class,
    inline: bool,
    /// The specifiers declare something by themselves (a struct, union or
    /// enum tag, or enumeration constants), so they may end the declaration
    /// with no declarator: `struct point { int x, y; };`.
    declares: bool,
    /// How the type was spelled: a typedef name or an enum (for `-g`).
    spelled: Spelled,
}

/// What an ordinary identifier (C's namespace of variables, functions,
/// typedef names and enumeration constants) means to the parser.
#[derive(Clone, Debug, PartialEq)]
enum Name {
    /// A typedef name, the type it stands for, and the name as a spelling.
    Typedef(Type, Spelled),
    /// An enumeration constant, its value (an `int`) and its enum, once the
    /// enum's definition is complete (for `-g`: clang describes the enum of
    /// a constant the code uses).
    EnumConst(i128, Spelled),
    /// A variable or function, and its declared type: resolved by sema, but
    /// the type lets `sizeof x` size an array ([`Parser::operand_type`]).
    Object(Type),
}

/// What a struct, union or enum tag names.
#[derive(Clone)]
enum Tag {
    Record(Arc<Record>),
    /// An enum: the integer type it is, and the enum as a spelling.
    Enum(Type, Spelled),
}

/// One block scope: the file, a function's parameters and body, a block, a
/// `for` statement, or a parameter list.
#[derive(Default)]
struct Scope {
    names: HashMap<String, (Name, Pos)>,
    tags: HashMap<String, Tag>,
}

/// One step of a declarator, from the base type towards the declared type.
enum Derive {
    /// Pointer to the type so far, with these qualifiers.
    Ptr(Quals),
    /// Array of the type so far (`None`: of unknown size). The last field is
    /// set when the brackets held qualifiers or `static` (`int a[const 3]`):
    /// C99 6.7.5.3p7 allows them only on a parameter's outermost array, and
    /// the qualifiers then apply to the pointer the array is adjusted to.
    Array(Option<i128>, Pos, Option<Quals>),
    /// Function returning the type so far, with these parameters (and the
    /// scope holding their names), variadic if `...` ends them, and whether
    /// the list is a prototype's (not `()`).
    Func(Vec<Param>, bool, bool, Scope),
}

/// A parsed declarator: the name, the full type, and where the name is.
struct Decl {
    /// Empty when the declarator has no name (allowed in parameters and type names).
    name: String,
    ty: Type,
    pos: Pos,
    /// When the declarator declares a function (its last step is a parameter
    /// list): the parameters with their names, and their scope, which a
    /// function body continues.
    func: Option<(Vec<Param>, Scope)>,
    /// When it declares a function: whether with a prototype, a parameter list
    /// that is not `()` (C99 6.7.5.3p14: `int f()` says nothing of the
    /// parameters, even though merlin takes it as `int f(void)`). `-g` says
    /// which (`DW_AT_prototyped`).
    prototyped: bool,
}

struct Parser<'a> {
    toks: &'a [Token],
    pos: usize,
    /// Current nesting level; see [`MAX_DEPTH`].
    depth: usize,
    /// Block scopes, innermost last; the first is the file scope.
    scopes: Vec<Scope>,
    /// The target, for sizes.
    xlen: Xlen,
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

    /// Runs `f` in a new scope.
    fn scoped<T>(&mut self, f: impl FnOnce(&mut Self) -> Result<T, Error>) -> Result<T, Error> {
        self.scopes.push(Scope::default());
        let r = f(self);
        self.scopes.pop();
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

    // ---- scopes ----

    fn lookup_name(&self, name: &str) -> Option<&Name> {
        self.scopes
            .iter()
            .rev()
            .find_map(|s| s.names.get(name))
            .map(|(n, _)| n)
    }

    /// The type a typedef name in scope stands for.
    fn typedef(&self, name: &str) -> Option<&Type> {
        match self.lookup_name(name) {
            Some(Name::Typedef(t, _)) => Some(t),
            _ => None,
        }
    }

    /// A typedef name in scope as a spelling.
    fn typedef_spelling(&self, name: &str) -> Spelled {
        match self.lookup_name(name) {
            Some(Name::Typedef(_, s)) => s.clone(),
            _ => Spelled::default(),
        }
    }

    fn is_typedef(&self, t: &Tok) -> bool {
        matches!(t, Tok::Ident(n) if self.typedef(n).is_some())
    }

    /// Declares an ordinary identifier in the innermost scope. Declaring a
    /// variable or function again is sema's to judge; any other clash is an
    /// error here.
    fn bind_name(&mut self, name: &str, what: Name, pos: Pos) -> Result<(), Error> {
        let scope = self.scopes.last_mut().expect("the file scope");
        match scope.names.get_mut(name) {
            None => {
                scope.names.insert(name.to_string(), (what, pos));
                Ok(())
            }
            // A variable declared again (`extern int a[]; int a[3];`): keep
            // the type that says the most. Whether the types agree is sema's
            // to judge.
            Some((Name::Object(prev), prev_pos)) => {
                if let Name::Object(new) = what {
                    if new.size(self.xlen).is_some() || prev.size(self.xlen).is_none() {
                        *prev = new;
                    }
                    return Ok(());
                }
                Err(Error::new(
                    pos,
                    format!(
                        "redefinition of '{name}' as a different kind of symbol (first declared at {prev_pos})"
                    ),
                ))
            }
            Some((prev, _)) if *prev == what && !matches!(what, Name::EnumConst(..)) => Ok(()),
            Some((Name::Typedef(prev, _), _)) if matches!(what, Name::Typedef(..)) => {
                let Name::Typedef(new, _) = what else {
                    unreachable!()
                };
                Err(Error::new(
                    pos,
                    format!("typedef redefinition with different types ('{new}' vs '{prev}')"),
                ))
            }
            Some((Name::EnumConst(..), prev_pos)) if matches!(what, Name::EnumConst(..)) => Err(Error::new(
                pos,
                format!("redefinition of enumerator '{name}' (first declared at {prev_pos})"),
            )),
            Some((_, prev_pos)) => Err(Error::new(
                pos,
                format!(
                    "redefinition of '{name}' as a different kind of symbol (first declared at {prev_pos})"
                ),
            )),
        }
    }

    // ---- constant expressions ----

    /// The value of an integer constant expression the parser needs now: an
    /// array size, an enumerator's value or a `_Static_assert`. Each
    /// `sizeof expr` in it whose operand's type the parser can work out
    /// becomes `sizeof (type)` first, so `char buf[sizeof x]` and
    /// `int b[sizeof a / sizeof a[0]]` work before sema has typed anything.
    fn const_int(&self, e: &mut Expr) -> Option<i128> {
        self.resolve_sizeof(e);
        consteval::parsed_int(e, self.xlen)
    }

    /// Rewrites `sizeof expr` into `sizeof (type)` throughout the operators
    /// [`consteval`] evaluates, where [`Self::operand_type`] knows the type.
    fn resolve_sizeof(&self, e: &mut Expr) {
        match &mut e.kind {
            ExprKind::SizeofExpr(x) => {
                if let Some(t) = self.operand_type(x) {
                    e.kind = ExprKind::SizeofType(t);
                }
            }
            ExprKind::Unary(_, x) | ExprKind::Cast { x, .. } | ExprKind::Addr(x) | ExprKind::Deref(x) => {
                self.resolve_sizeof(x)
            }
            // A member's offset is sema's to fill in, which comes too late for
            // a constant the parser evaluates: `offsetof`'s
            // `(unsigned long)&((struct s *)0)->m` in a `_Static_assert`.
            ExprKind::Member {
                base,
                name,
                arrow,
                offset,
            } => {
                self.resolve_sizeof(base);
                let record = if *arrow {
                    self.value_type(base).and_then(|t| t.pointee().cloned())
                } else {
                    self.operand_type(base)
                };
                if let Some(m) = record
                    .as_ref()
                    .and_then(|r| r.record())
                    .and_then(|r| r.body()?.member(name))
                {
                    *offset = m.offset;
                }
            }
            ExprKind::Binary(_, l, r) => {
                self.resolve_sizeof(l);
                self.resolve_sizeof(r);
            }
            ExprKind::Cond(c, a, b) => {
                self.resolve_sizeof(c);
                self.resolve_sizeof(a);
                self.resolve_sizeof(b);
            }
            _ => {}
        }
    }

    /// The type of `sizeof`'s operand, as far as the parser can tell before
    /// sema: variables by their declared types, literals, casts and compound
    /// literals by theirs, and `*`, `&`, `[]`, `.`, `->`, `+` and `-` from
    /// their operands'. `None` for anything else (calls, assignments, ...),
    /// which leaves the `sizeof` for consteval to refuse.
    fn operand_type(&self, e: &Expr) -> Option<Type> {
        match &e.kind {
            ExprKind::Num(_) | ExprKind::Enumerator { .. } | ExprKind::Str(_) | ExprKind::Cast { .. } => {
                Some(e.ty.clone())
            }
            // `(int[]){1, 2, 3}`: the list completes the array's size.
            ExprKind::CompoundLit { init, .. } => match &e.ty {
                Type::Array(elem, None) => Some(Type::array((**elem).clone(), Some(init_len(elem, init)?))),
                t => Some(t.clone()),
            },
            ExprKind::SizeofType(_) | ExprKind::SizeofExpr(_) => Some(Type::Int(IntKind::size_t(self.xlen))),
            ExprKind::Var { name, .. } => match self.lookup_name(name)? {
                Name::Object(t) => Some(t.clone()),
                _ => None,
            },
            // `a[i]` is `*(a + i)`.
            ExprKind::Deref(p) => Some(self.value_type(p)?.pointee()?.clone()),
            ExprKind::Addr(x) => Some(Type::ptr(self.operand_type(x)?)),
            ExprKind::Member {
                base, name, arrow, ..
            } => {
                let record = if *arrow {
                    self.value_type(base)?.pointee()?.clone()
                } else {
                    self.operand_type(base)?
                };
                Some(record.record()?.body()?.member(name)?.ty.clone())
            }
            ExprKind::Binary(op @ (BinOp::Add | BinOp::Sub), l, r) => {
                let (lt, rt) = (self.value_type(l)?, self.value_type(r)?);
                match (lt.is_ptr(), rt.is_ptr()) {
                    // The difference of two pointers is a `ptrdiff_t`.
                    (true, true) if *op == BinOp::Sub => Some(Type::Int(IntKind::ptrdiff_t(self.xlen))),
                    (true, false) => Some(lt),
                    (false, true) if *op == BinOp::Add => Some(rt),
                    (false, false) => Some(Type::Int(common(lt.int_kind()?, rt.int_kind()?, self.xlen))),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// The type of `e`'s value: [`Self::operand_type`] after decay.
    fn value_type(&self, e: &Expr) -> Option<Type> {
        Some(self.operand_type(e)?.decay())
    }

    fn lookup_tag(&self, name: &str) -> Option<&Tag> {
        self.scopes.iter().rev().find_map(|s| s.tags.get(name))
    }

    fn bind_tag(&mut self, name: &str, tag: Tag) {
        let scope = self.scopes.last_mut().expect("the file scope");
        scope.tags.insert(name.to_string(), tag);
    }

    /// Whether the current token starts a declaration: a type keyword,
    /// storage class or qualifier, or a typedef name (that is not a label).
    fn at_decl(&self) -> bool {
        match self.peek() {
            Tok::Kw(k) => k.starts_decl(),
            t => self.is_typedef(t) && !matches!(self.peek_at(1), Tok::Punct(":")),
        }
    }

    /// Whether the token after the current one starts a type name: `(` then a
    /// type means a cast or `sizeof (type)`.
    fn type_name_follows(&self) -> bool {
        match self.peek_at(1) {
            Tok::Kw(k) => k.starts_decl(),
            t => self.is_typedef(t),
        }
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

    fn expect_ident(&mut self, what: &str) -> Result<(String, Pos), Error> {
        match self.peek().clone() {
            Tok::Ident(name) => Ok((name, self.advance().pos)),
            _ => Err(self.unexpected(what)),
        }
    }

    // ---- declaration specifiers ----

    /// Type specifiers, qualifiers, storage class and `inline`, in any order
    /// (`unsigned static int` is as good as `static unsigned int`, as in C).
    fn specifiers(&mut self, ctx: Ctx) -> Result<Specs, Error> {
        let start = self.here();
        let mut class = Class::None;
        let mut inline = false;
        let mut quals = Quals::NONE;
        // A struct, union or enum, or a typedef name: a type on its own.
        let mut named: Option<Type> = None;
        let mut spelled = Spelled::default();
        let mut declares = false;
        // How many times each type keyword appears.
        let (mut void, mut char, mut short, mut int, mut long, mut signed, mut unsigned, mut bool_) =
            (0, 0, 0, 0, 0, 0, 0, 0);
        loop {
            let pos = self.here();
            let k = match self.peek().clone() {
                Tok::Kw(k) if k.starts_decl() => k,
                // A typedef name is a type specifier only where no other is yet:
                // in `int T;` the `T` is the declarator, even if it names a type.
                Tok::Ident(name)
                    if named.is_none()
                        && void + char + short + int + long + signed + unsigned + bool_ == 0
                        && self.typedef(&name).is_some() =>
                {
                    named = self.typedef(&name).cloned();
                    spelled = self.typedef_spelling(&name);
                    self.advance();
                    continue;
                }
                _ => break,
            };
            self.advance();
            let count = match k {
                Kw::Static | Kw::Extern | Kw::Typedef => {
                    let c = match k {
                        Kw::Static => Class::Static,
                        Kw::Extern => Class::Extern,
                        _ => Class::Typedef,
                    };
                    let allowed = match ctx {
                        Ctx::File => true,
                        Ctx::Block => k != Kw::Extern,
                        Ctx::ForInit | Ctx::Param | Ctx::Member | Ctx::TypeName => false,
                    };
                    if !allowed {
                        return Err(Error::new(pos, format!("'{}' is not allowed here", k.as_str())));
                    }
                    if class != Class::None {
                        return Err(Error::new(
                            pos,
                            "at most one storage class ('static', 'extern', 'typedef') is allowed",
                        ));
                    }
                    class = c;
                    continue;
                }
                Kw::Inline => {
                    if ctx != Ctx::File {
                        return Err(Error::new(pos, "'inline' can only appear on functions"));
                    }
                    inline = true;
                    continue;
                }
                Kw::Const => {
                    quals.konst = true;
                    continue;
                }
                Kw::Volatile => {
                    quals.volatile = true;
                    continue;
                }
                // `restrict` promises no aliasing, which merlin never relies on.
                Kw::Restrict => continue,
                Kw::Struct | Kw::Union | Kw::Enum => {
                    if named.is_some() || void + char + short + int + long + signed + unsigned + bool_ > 0 {
                        return Err(Error::new(start, "invalid combination of type specifiers"));
                    }
                    let (ty, d, s) = self.nested(|p| match k {
                        Kw::Enum => p.enum_spec(pos),
                        _ => p
                            .record_spec(k == Kw::Union, pos)
                            .map(|(t, d)| (t, d, Spelled::default())),
                    })?;
                    named = Some(ty);
                    declares = d;
                    spelled = s;
                    continue;
                }
                Kw::Void => &mut void,
                Kw::Char => &mut char,
                Kw::Short => &mut short,
                Kw::Int => &mut int,
                Kw::Long => &mut long,
                Kw::Signed => &mut signed,
                Kw::Unsigned => &mut unsigned,
                Kw::Bool => &mut bool_,
                _ => unreachable!("starts_decl"),
            };
            *count += 1;
            // `long` may come twice: `long long`.
            if k == Kw::Long && *count == 3 {
                return Err(Error::new(pos, "'long long long' is invalid"));
            }
            if *count > 1 && !(k == Kw::Long && *count == 2) {
                return Err(Error::new(pos, format!("duplicate '{}'", k.as_str())));
            }
        }
        let keywords = void + char + short + int + long + signed + unsigned + bool_;
        let ty = match named {
            Some(_) if keywords > 0 => {
                return Err(Error::new(start, "invalid combination of type specifiers"));
            }
            Some(ty) => ty,
            None if keywords == 0 => {
                return Err(Error::new(
                    self.here(),
                    format!("expected a type, found {}", self.tok().describe()),
                ));
            }
            None => {
                // At most one of void, char, short, long (or long long) and
                // _Bool; `int` may join short and long; `signed` or
                // `unsigned` may join anything but void and _Bool.
                let invalid = signed + unsigned > 1
                    || void + char + short + long.min(1) + bool_ > 1
                    || int + void + char + bool_ > 1
                    || void + bool_ + signed + unsigned > 1;
                if invalid {
                    return Err(Error::new(start, "invalid combination of type specifiers"));
                }
                if void == 1 {
                    Type::Void
                } else if bool_ == 1 {
                    Type::BOOL
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
                } else if long == 2 {
                    Type::Int(if unsigned == 1 {
                        IntKind::ULongLong
                    } else {
                        IntKind::LongLong
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
                }
            }
        };
        Ok(Specs {
            ty: ty.qualified(quals),
            class,
            inline,
            declares,
            spelled,
        })
    }

    /// After `struct` or `union`: a definition (`struct tag { ... }`, the tag
    /// optional) or a reference to a tag. Returns the type and whether the
    /// specifier declares a tag by itself.
    fn record_spec(&mut self, union: bool, kw_pos: Pos) -> Result<(Type, bool), Error> {
        let kw = if union { "union" } else { "struct" };
        let tag = match self.peek().clone() {
            Tok::Ident(name) => Some((name, self.advance().pos)),
            _ => None,
        };
        let mismatch = |name: &str, pos: Pos| {
            Error::new(
                pos,
                format!("use of '{name}' with tag type that does not match previous declaration"),
            )
        };
        if self.is_punct("{") {
            let record = match &tag {
                Some((name, pos)) => match self.scopes.last().unwrap().tags.get(name) {
                    // `struct s;` earlier in this scope: this completes it.
                    Some(Tag::Record(r)) if r.union == union && r.body().is_none() => r.clone(),
                    Some(Tag::Record(r)) if r.union == union => {
                        return Err(Error::new(*pos, format!("redefinition of '{kw} {name}'")));
                    }
                    Some(_) => return Err(mismatch(name, *pos)),
                    None => {
                        // Declared before the body, so members can point to it.
                        let r = Record::new(union, Some(name.clone()), *pos);
                        self.bind_tag(name, Tag::Record(r.clone()));
                        r
                    }
                },
                None => Record::new(union, None, kw_pos),
            };
            self.record_body(&record)?;
            // `struct { int x; };` declares nothing anyone can name.
            return Ok((Type::Record(record), tag.is_some()));
        }
        let Some((name, pos)) = tag else {
            return Err(self.unexpected(&format!("a name or '{{' after '{kw}'")));
        };
        // `struct s;` on its own declares a new type in this scope, even when
        // an outer scope has a `struct s`.
        let alone = self.is_punct(";");
        let found = if alone {
            self.scopes.last().unwrap().tags.get(&name)
        } else {
            self.lookup_tag(&name)
        };
        match found {
            Some(Tag::Record(r)) if r.union == union => Ok((Type::Record(r.clone()), alone)),
            Some(_) => Err(mismatch(&name, pos)),
            None => {
                // A tag not seen before: an incomplete type, completed by a
                // definition later in the same scope.
                let r = Record::new(union, Some(name.clone()), pos);
                self.bind_tag(&name, Tag::Record(r.clone()));
                Ok((Type::Record(r), true))
            }
        }
    }

    /// `{ member-decl+ }`: the members, laid out into `record`.
    fn record_body(&mut self, record: &Record) -> Result<(), Error> {
        let open = self.expect_punct("{")?;
        let mut members: Vec<(String, Type, Spelled, Pos)> = Vec::new();
        let mut seen: HashMap<String, Pos> = HashMap::new();
        // The layout so far, to reject a struct too big before laying it out.
        let mut end = 0u64;
        while !self.eat_punct("}") {
            if !self.at_decl() {
                return Err(self.unexpected(&format!(
                    "a member declaration or '}}' in the {}",
                    record.keyword()
                )));
            }
            let specs = self.specifiers(Ctx::Member)?;
            if self.is_punct(";") {
                return Err(Error::new(
                    self.here(),
                    "declaration does not declare anything (anonymous members are not supported)",
                ));
            }
            loop {
                let d = self.declarator(&specs.ty, Ctx::Member)?;
                if self.is_punct(":") {
                    return Err(Error::new(self.here(), "bit-fields are not supported yet"));
                }
                if d.ty.is_func() {
                    return Err(Error::new(
                        d.pos,
                        format!("field '{}' declared as a function", d.name),
                    ));
                }
                let Some(size) = d.ty.size(self.xlen) else {
                    let msg = match &d.ty {
                        Type::Array(_, None) => "flexible array members are not supported yet".to_string(),
                        t => format!("field '{}' has incomplete type '{t}'", d.name),
                    };
                    return Err(Error::new(d.pos, msg));
                };
                if let Some(prev) = seen.insert(d.name.clone(), d.pos) {
                    return Err(Error::new(
                        d.pos,
                        format!("duplicate member '{}' (first declared at {prev})", d.name),
                    ));
                }
                end = if record.union {
                    end.max(u64::from(size))
                } else {
                    align_up(end, d.ty.align(self.xlen)) + u64::from(size)
                };
                if end > MAX_OBJECT_SIZE {
                    return Err(Error::new(d.pos, format!("{} is too large", record.keyword())));
                }
                members.push((d.name, d.ty, specs.spelled.clone(), d.pos));
                if !self.eat_punct(",") {
                    break;
                }
            }
            self.expect_semi("at end of declaration list")?;
        }
        if members.is_empty() {
            return Err(Error::new(open, format!("{} has no members", record.keyword())));
        }
        if align_up(
            end,
            members
                .iter()
                .map(|(_, t, _, _)| t.align(self.xlen))
                .max()
                .unwrap_or(1),
        ) > MAX_OBJECT_SIZE
        {
            return Err(Error::new(open, format!("{} is too large", record.keyword())));
        }
        record.complete_spelled(members, self.xlen);
        Ok(())
    }

    /// After `enum`: a definition, whose constants go in scope one by one
    /// (so `B = A + 1` works), or a reference to a defined tag. An enum type
    /// is `unsigned int` when no constant is negative and `int` otherwise
    /// (clang's and GCC's choice for C); the constants themselves are `int`s.
    fn enum_spec(&mut self, kw_pos: Pos) -> Result<(Type, bool, Spelled), Error> {
        let tag = match self.peek().clone() {
            Tok::Ident(name) => Some((name, self.advance().pos)),
            _ => None,
        };
        if !self.eat_punct("{") {
            let Some((name, pos)) = tag else {
                return Err(self.unexpected("a name or '{' after 'enum'"));
            };
            return match self.lookup_tag(&name) {
                Some(Tag::Enum(ty, s)) => Ok((ty.clone(), false, s.clone())),
                Some(Tag::Record(_)) => Err(Error::new(
                    pos,
                    format!("use of '{name}' with tag type that does not match previous declaration"),
                )),
                None => Err(Error::new(
                    pos,
                    format!(
                        "use of undefined 'enum {name}' (merlin needs an enum's constants before its name)"
                    ),
                )),
            };
        }
        if let Some((name, pos)) = &tag {
            if self.scopes.last().unwrap().tags.contains_key(name) {
                return Err(Error::new(*pos, format!("redefinition of 'enum {name}'")));
            }
        }
        let (int_min, int_max) = IntKind::Int.range(self.xlen);
        let mut next = 0i128;
        let mut negative = false;
        let mut constants = Vec::new();
        loop {
            if self.eat_punct("}") {
                if constants.is_empty() {
                    return Err(Error::new(kw_pos, "use of empty enum"));
                }
                break;
            }
            let (name, pos) = self.expect_ident("an enumerator name")?;
            let value = if self.eat_punct("=") {
                let at = self.here();
                let mut e = self.cond()?;
                self.const_int(&mut e)
                    .ok_or_else(|| Error::new(at, "enumerator value is not an integer constant expression"))?
            } else {
                next
            };
            if !(int_min..=int_max).contains(&value) {
                return Err(Error::new(
                    pos,
                    format!("enumerator value {value} is not representable as an 'int'"),
                ));
            }
            self.bind_name(&name, Name::EnumConst(value, Spelled::default()), pos)?;
            negative |= value < 0;
            next = value + 1;
            constants.push((name, value));
            if !self.eat_punct(",") && !self.is_punct("}") {
                return Err(self.unexpected("',' or '}' in the enum"));
            }
        }
        let ty = if negative { Type::INT } else { Type::UINT };
        let names: Vec<String> = constants.iter().map(|(n, _)| n.clone()).collect();
        let spelled = Spelled(Some(Spelling::Enum(Arc::new(EnumType {
            tag: tag.as_ref().map(|(n, _)| n.clone()),
            ty: ty.clone(),
            constants,
            pos: kw_pos,
        }))));
        // The constants now name their enum, for the code after it.
        let scope = self.scopes.last_mut().expect("a scope");
        for name in &names {
            if let Some((Name::EnumConst(_, of), _)) = scope.names.get_mut(name) {
                *of = spelled.clone();
            }
        }
        if let Some((name, _)) = tag {
            self.bind_tag(&name, Tag::Enum(ty.clone(), spelled.clone()));
        }
        Ok((ty, true, spelled))
    }

    /// `const`, `volatile` and `restrict` after a `*`.
    fn qualifiers(&mut self) -> Quals {
        let mut q = Quals::NONE;
        loop {
            match self.peek() {
                Tok::Kw(Kw::Const) => q.konst = true,
                Tok::Kw(Kw::Volatile) => q.volatile = true,
                Tok::Kw(Kw::Restrict) => {}
                _ => return q,
            }
            self.advance();
        }
    }

    // ---- declarators ----

    /// Types nest like expressions (and are as recursive to check), so a
    /// declarator may have at most [`MAX_DEPTH`] `*`s and as many `[]`s.
    fn too_deep(&self, n: usize, what: &str) -> Result<(), Error> {
        if n == MAX_DEPTH {
            let msg = format!("type nesting too deep: merlin allows at most {MAX_DEPTH} {what}");
            return Err(Error::new(self.here(), msg));
        }
        Ok(())
    }

    /// After the `*`s, whether a `(` opens a parenthesized declarator
    /// (`(*fp)`, `(name)`) rather than a parameter list (`(int)`, `()`).
    fn nested_declarator_follows(&self) -> bool {
        match self.peek_at(1) {
            Tok::Punct("*" | "(") => true,
            t @ Tok::Ident(_) => !self.is_typedef(t),
            _ => false,
        }
    }

    /// Reads a declarator: its name (if any), where the name is (or would
    /// be), and its steps in the order they apply to the base type.
    fn declarator_ops(&mut self, ctx: Ctx) -> Result<(String, Pos, Vec<Derive>), Error> {
        let mut ptrs = Vec::new();
        while self.is_punct("*") {
            self.too_deep(ptrs.len(), "'*'s")?;
            self.advance();
            ptrs.push(Derive::Ptr(self.qualifiers()));
        }
        let pos = self.here();
        let (name, pos, inner) = if self.is_punct("(") && self.nested_declarator_follows() {
            self.advance();
            let (name, pos, inner) = self.nested(|p| p.declarator_ops(ctx))?;
            self.expect_punct(")")?;
            (name, pos, inner)
        } else if let Tok::Ident(name) = self.peek().clone() {
            if ctx == Ctx::TypeName {
                return Err(self.unexpected("')' after the type name"));
            }
            self.advance();
            (name, pos, Vec::new())
        } else if matches!(ctx, Ctx::Param | Ctx::TypeName) {
            (String::new(), pos, Vec::new())
        } else {
            return Err(self.unexpected("a name"));
        };
        let suffixes = self.suffixes()?;
        let mut ops = ptrs;
        ops.extend(suffixes.into_iter().rev());
        ops.extend(inner);
        Ok((name, pos, ops))
    }

    /// `('[' constant? ']' | '(' params ')')*` after a declarator's name, in
    /// source order. In a run of dimensions only the first may be left out.
    fn suffixes(&mut self) -> Result<Vec<Derive>, Error> {
        let mut out = Vec::new();
        let mut dims = 0;
        loop {
            if self.is_punct("[") {
                self.too_deep(dims, "array dimensions")?;
                let open = self.advance().pos;
                // `[const 3]`, `[static 3]`, `[restrict static 3]`: qualifiers
                // and `static` before the size (`static` promises the caller
                // passes at least that many elements; merlin does not use it).
                // `declarator` checks they are on a parameter's outermost array.
                let mut quals = None;
                let mut is_static = false;
                loop {
                    if self.eat_kw(Kw::Static) {
                        is_static = true;
                        quals.get_or_insert(Quals::NONE);
                    } else if matches!(self.peek(), Tok::Kw(Kw::Const | Kw::Volatile | Kw::Restrict)) {
                        let q = self.qualifiers();
                        quals = Some(quals.unwrap_or(Quals::NONE).union(q));
                    } else {
                        break;
                    }
                }
                if is_static && self.is_punct("]") {
                    return Err(self.unexpected("the array size after 'static'"));
                }
                if self.eat_punct("]") {
                    if dims > 0 {
                        return Err(Error::new(open, "only the first array dimension may be left out"));
                    }
                    out.push(Derive::Array(None, open, quals));
                    dims += 1;
                    continue;
                }
                let pos = self.here();
                let mut e = self.cond()?;
                self.expect_punct("]")?;
                let n = self.const_int(&mut e).ok_or_else(|| {
                    Error::new(
                        pos,
                        "array size must be an integer constant expression \
                         (literals, enum constants, operators, casts and sizeof)",
                    )
                })?;
                if n <= 0 {
                    return Err(Error::new(pos, format!("array size must be positive, not {n}")));
                }
                out.push(Derive::Array(Some(n), open, quals));
                dims += 1;
            } else if self.is_punct("(") {
                self.advance();
                let prototyped = !self.is_punct(")");
                let (params, variadic, scope) = self.params()?;
                out.push(Derive::Func(params, variadic, prototyped, scope));
                dims = 0;
            } else {
                return Ok(out);
            }
        }
    }

    /// A declarator, applied to `base`. In a parameter list or a type name
    /// the name may be missing.
    fn declarator(&mut self, base: &Type, ctx: Ctx) -> Result<Decl, Error> {
        let (name, pos, ops) = self.declarator_ops(ctx)?;
        let end = self.here();
        let mut ty = base.clone();
        let mut func = None;
        let mut prototyped = false;
        // The size of `ty`, kept as it grows: `Type::size` recurses, so asking
        // it at every dimension would be quadratic in the number of dimensions.
        let mut size = ty.size(self.xlen).map(u64::from);
        let steps = ops.len();
        for (i, op) in ops.into_iter().enumerate() {
            match op {
                Derive::Ptr(q) => {
                    ty = Type::ptr(ty).qualified(q);
                    size = Some(u64::from(ptr_size(self.xlen)));
                }
                Derive::Array(n, open, quals) => {
                    // Only a parameter's outermost array becomes a pointer,
                    // the one thing the qualifiers in the brackets can apply to.
                    if quals.is_some() && !(ctx == Ctx::Param && i + 1 == steps) {
                        return Err(Error::new(
                            open,
                            "qualifiers and 'static' inside '[]' are only allowed in the \
                             outermost array of a function parameter",
                        ));
                    }
                    if ty.is_func() {
                        let what = if name.is_empty() { "type name" } else { &name };
                        return Err(Error::new(
                            open,
                            format!("'{what}' declared as array of functions of type '{ty}'"),
                        ));
                    }
                    let Some(elem_size) = size else {
                        return Err(Error::new(
                            end,
                            format!("array has incomplete element type '{ty}'"),
                        ));
                    };
                    let total = u128::from(elem_size) * n.unwrap_or(1) as u128;
                    if total > u128::from(MAX_OBJECT_SIZE) {
                        return Err(Error::new(self.after_prev(), "array is too large"));
                    }
                    size = n.map(|_| total as u64);
                    ty = Type::array(ty, n.map(|n| n as u32));
                    if let Some(q) = quals {
                        // The parameter adjustment, done here so the
                        // qualifiers land on the pointer: `int a[const 3]`
                        // declares `int *const a`.
                        ty = ty.decay().qualified(q);
                        size = Some(u64::from(ptr_size(self.xlen)));
                    }
                }
                Derive::Func(params, variadic, with_prototype, scope) => {
                    if ty.is_array() || ty.is_func() {
                        let what = if ty.is_array() { "array" } else { "function" };
                        return Err(Error::new(
                            pos,
                            format!("function cannot return {what} type '{ty}'"),
                        ));
                    }
                    // A qualified result means nothing: the value is a copy.
                    let ret = ty.unqual().clone();
                    let types = params.iter().map(|p| p.ty.unqual().clone()).collect();
                    ty = if variadic {
                        Type::variadic_func(ret, types)
                    } else {
                        Type::func(ret, types)
                    };
                    size = None;
                    if i + 1 == steps {
                        func = Some((params, scope));
                        prototyped = with_prototype;
                    }
                }
            }
        }
        Ok(Decl {
            name,
            ty,
            pos,
            func,
            prototyped,
        })
    }

    /// After `(`, up to and including `)`: the parameters, in a scope of
    /// their own, and whether `...` ends them.
    fn params(&mut self) -> Result<(Vec<Param>, bool, Scope), Error> {
        self.scopes.push(Scope::default());
        let params = self.params_inner();
        let scope = self.scopes.pop().expect("pushed above");
        let (params, variadic) = params?;
        Ok((params, variadic, scope))
    }

    fn params_inner(&mut self) -> Result<(Vec<Param>, bool), Error> {
        let mut params = Vec::new();
        if self.eat_punct(")") {
            return Ok((params, false));
        }
        if *self.peek() == Tok::Kw(Kw::Void) && matches!(self.peek_at(1), Tok::Punct(")")) {
            self.advance();
            self.advance();
            return Ok((params, false));
        }
        loop {
            if self.is_punct("...") {
                // C99 wants a named parameter before `...` (C23 does not).
                if params.is_empty() {
                    return Err(Error::new(
                        self.here(),
                        "a named parameter must come before '...'",
                    ));
                }
                self.advance();
                if !self.eat_punct(")") {
                    return Err(self.unexpected("')' after '...'"));
                }
                return Ok((params, true));
            }
            if !self.at_decl() {
                return Err(self.unexpected("a parameter type"));
            }
            let start = self.here();
            let specs = self.specifiers(Ctx::Param)?;
            let d = self.declarator(&specs.ty, Ctx::Param)?;
            // `(V)` with `typedef void V;` is `(void)` too: what C99 6.7.5.3p10
            // asks for is one unnamed parameter of (unqualified) type void,
            // however it is spelled.
            if params.is_empty() && d.name.is_empty() && d.ty == Type::Void && self.eat_punct(")") {
                return Ok((params, false));
            }
            let pos = if d.name.is_empty() { start } else { d.pos };
            if params.len() == MAX_PARAMS {
                return Err(Error::new(
                    pos,
                    format!("too many parameters: merlin supports at most {MAX_PARAMS}"),
                ));
            }
            // C adjusts a parameter declared as an array or a function to a
            // pointer (keeping qualifiers on what it points to).
            let ty = if d.ty.is_array() || d.ty.is_func() {
                d.ty.decay()
            } else {
                d.ty
            };
            if !d.name.is_empty() {
                self.bind_name(&d.name, Name::Object(ty.clone()), pos)?;
            }
            params.push(Param {
                name: d.name,
                ty,
                pos,
                spelled: specs.spelled.clone(),
            });
            if self.eat_punct(")") {
                return Ok((params, false));
            }
            if !self.eat_punct(",") {
                return Err(self.unexpected("',' or ')' in the parameter list"));
            }
        }
    }

    /// A type name, for casts and `sizeof`: specifiers and a declarator
    /// without a name (`int`, `char *[4]`, `int (*)(void)`).
    fn type_name(&mut self) -> Result<Type, Error> {
        Ok(self.type_name_spelled()?.0)
    }

    /// A type name, and how it spelled its base (for `-g`).
    fn type_name_spelled(&mut self) -> Result<(Type, Spelled), Error> {
        let specs = self.specifiers(Ctx::TypeName)?;
        let ty = self.declarator(&specs.ty, Ctx::TypeName)?.ty;
        Ok((ty, specs.spelled))
    }

    // ---- declarations ----

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
                if p.is_punct(".") || p.is_punct("[") {
                    let designators = p.designation()?;
                    let init = p.initializer()?;
                    items.push(Init::Designated(designators, Box::new(init)));
                } else {
                    items.push(p.initializer()?);
                }
                if !p.eat_punct(",") && !p.is_punct("}") {
                    return Err(p.unexpected("',' or '}' in the initializer list"));
                }
            }
        })
    }

    /// `.member` and `[index]` designators, through the `=`.
    fn designation(&mut self) -> Result<Vec<Designator>, Error> {
        let mut out = Vec::new();
        loop {
            if self.is_punct(".") {
                let pos = self.advance().pos;
                let (name, _) = self.expect_ident("a member name")?;
                out.push(Designator::Member(name, pos));
            } else if self.is_punct("[") {
                let pos = self.advance().pos;
                let index = self.cond()?;
                self.expect_punct("]")?;
                out.push(Designator::Index(index, pos));
            } else {
                break;
            }
        }
        self.expect_punct("=")?;
        Ok(out)
    }

    /// The rest of a declaration after its specifiers (and its first
    /// declarator, if the caller has read it), through the `;`.
    fn init_declarators(
        &mut self,
        specs: &Specs,
        first: Option<Decl>,
        ctx: Ctx,
    ) -> Result<Vec<Declarator>, Error> {
        let storage = match specs.class {
            Class::Static => Storage::Static,
            Class::Extern => Storage::Extern,
            Class::None | Class::Typedef => Storage::None,
        };
        let mut first = first;
        let mut decls = Vec::new();
        loop {
            let d = match first.take() {
                Some(d) => d,
                None => self.declarator(&specs.ty, ctx)?,
            };
            if d.ty.is_func() {
                let msg = if ctx == Ctx::File {
                    "declare a function on its own, not in a list of variables"
                } else {
                    "function declarations inside a function are not supported"
                };
                return Err(Error::new(d.pos, msg));
            }
            self.bind_name(&d.name, Name::Object(d.ty.clone()), d.pos)?;
            let init = if self.eat_punct("=") {
                Some(self.initializer()?)
            } else {
                None
            };
            // `int a[] = {1, 2, 3};`: the size the initializer gives, for a
            // later `sizeof a` in an array size. (The declaration keeps its
            // incomplete type: sema completes it, the same way.) Unless an
            // earlier declaration gave the size: `int a[3]; int a[] = {1};`.
            let bound = self.scopes.last().and_then(|s| s.names.get(&d.name));
            let incomplete = matches!(bound, Some((Name::Object(t), _)) if t.size(self.xlen).is_none());
            if let (Type::Array(elem, None), Some(init), true) = (&d.ty, &init, incomplete) {
                if let Some(n) = init_len(elem, init) {
                    let full = Type::array((**elem).clone(), Some(n));
                    self.bind_name(&d.name, Name::Object(full), d.pos)?;
                }
            }
            decls.push(Declarator {
                name: d.name,
                ty: d.ty,
                storage,
                init,
                var: None,
                define: false,
                pos: d.pos,
                spelled: specs.spelled.clone(),
            });
            if !self.eat_punct(",") {
                break;
            }
        }
        self.expect_semi("at end of declaration")?;
        Ok(decls)
    }

    /// The declarators of a `typedef`, each binding a name to its type.
    fn typedefs(&mut self, specs: &Specs, first: Decl, ctx: Ctx) -> Result<(), Error> {
        let mut d = first;
        loop {
            if self.is_punct("=") {
                return Err(Error::new(
                    self.here(),
                    "illegal initializer (only variables can be initialized)",
                ));
            }
            let spelling = Spelled(Some(Spelling::Typedef(Arc::new(TypedefName {
                name: d.name.clone(),
                ty: d.ty.clone(),
                spelled: specs.spelled.clone(),
                pos: d.pos,
            }))));
            self.bind_name(&d.name, Name::Typedef(d.ty, spelling), d.pos)?;
            if !self.eat_punct(",") {
                break;
            }
            d = self.declarator(&specs.ty, ctx)?;
        }
        self.expect_semi("at end of declaration")
    }

    /// After specifiers that may stand alone: `;` is fine if they declared
    /// something (`struct s { ... };`).
    fn bare_specifiers(&mut self, specs: &Specs) -> Result<(), Error> {
        let pos = self.here();
        self.advance(); // ;
        if specs.declares {
            Ok(())
        } else {
            Err(Error::new(pos, "declaration does not declare anything"))
        }
    }

    /// A top-level function or declaration; `None` for a declaration that
    /// only declares types (a typedef, a struct, an enum).
    fn item(&mut self) -> Result<Option<Item>, Error> {
        if *self.peek() == Tok::Kw(Kw::StaticAssert) {
            self.static_assert()?;
            return Ok(None);
        }
        if !self.at_decl() {
            return Err(self.unexpected("a declaration"));
        }
        let specs = self.specifiers(Ctx::File)?;
        if self.is_punct(";") {
            self.bare_specifiers(&specs)?;
            return Ok(None);
        }
        let d = self.declarator(&specs.ty, Ctx::File)?;
        if specs.class == Class::Typedef {
            self.typedefs(&specs, d, Ctx::File)?;
            return Ok(None);
        }
        if !d.ty.is_func() {
            if specs.inline {
                return Err(Error::new(d.pos, "'inline' can only appear on functions"));
            }
            return Ok(Some(Item::Decl(self.init_declarators(
                &specs,
                Some(d),
                Ctx::File,
            )?)));
        }
        self.bind_name(&d.name, Name::Object(d.ty.clone()), d.pos)?;
        let mut d = d;
        let body = if self.eat_punct(";") {
            None
        } else if self.is_punct("{") {
            let Some((params, scope)) = d.func.take() else {
                return Err(Error::new(
                    d.pos,
                    "a function definition needs its parameter list",
                ));
            };
            if let Some(p) = params.iter().find(|p| p.name.is_empty()) {
                return Err(Error::new(
                    p.pos,
                    "parameter name omitted in a function definition",
                ));
            }
            // The body's outermost block shares the parameters' scope.
            self.scopes.push(scope);
            let body = self.block_body(false);
            self.scopes.pop();
            let body = body?;
            // The `}` just read.
            let end = self.toks[self.pos - 1].pos;
            return Ok(Some(Item::Func(function(d, params, &specs, Some(body), end))));
        } else if self.is_punct(",") {
            return Err(Error::new(
                d.pos,
                "declare a function on its own, not in a list of variables",
            ));
        } else {
            return Err(self.unexpected("'{' or ';' after the parameter list"));
        };
        let params = d.func.as_ref().map(|(p, _)| p.clone()).unwrap_or_else(|| {
            // Declared through a typedef of a function type: unnamed parameters.
            d.ty.func_type()
                .map(|f| {
                    f.params
                        .iter()
                        .map(|t| Param {
                            name: String::new(),
                            ty: t.clone(),
                            pos: d.pos,
                            spelled: Spelled::default(),
                        })
                        .collect()
                })
                .unwrap_or_default()
        });
        let end = d.pos;
        Ok(Some(Item::Func(function(d, params, &specs, body, end))))
    }

    /// `{ item* }`, returning the items; in a new scope unless `new_scope`
    /// is false (a function body continues its parameters' scope).
    fn block_body(&mut self, new_scope: bool) -> Result<Vec<Stmt>, Error> {
        if new_scope {
            self.scoped(|p| p.block_items())
        } else {
            self.block_items()
        }
    }

    fn block_items(&mut self) -> Result<Vec<Stmt>, Error> {
        self.expect_punct("{")?;
        let mut items = Vec::new();
        loop {
            if self.eat_punct("}") {
                return Ok(items);
            }
            if *self.peek() == Tok::Eof {
                return Err(self.unexpected("'}'"));
            }
            if *self.peek() == Tok::Kw(Kw::StaticAssert) {
                self.static_assert()?;
            } else if self.at_decl() {
                if let Some(s) = self.decl(Ctx::Block)? {
                    items.push(s);
                }
            } else {
                items.push(self.stmt()?);
            }
        }
    }

    /// `_Static_assert(constant, "message");` (C11, which clang accepts in
    /// C99 too), at file or block scope. The parser evaluates constants, so
    /// it checks the assertion here; nothing is left of it.
    fn static_assert(&mut self) -> Result<(), Error> {
        let pos = self.advance().pos;
        self.expect_punct("(")?;
        let at = self.here();
        let mut cond = self.cond()?;
        self.expect_punct(",")?;
        let mut msg = Vec::new();
        if !matches!(self.peek(), Tok::Str(_)) {
            return Err(self.unexpected("a string literal"));
        }
        while let Tok::Str(s) = self.peek() {
            msg.extend_from_slice(s);
            self.advance();
        }
        self.expect_punct(")")?;
        self.expect_punct(";")?;
        match self.const_int(&mut cond) {
            None => Err(Error::new(
                at,
                "static assertion expression is not an integer constant expression",
            )),
            Some(0) => Err(Error::new(
                pos,
                format!("static assertion failed: {}", String::from_utf8_lossy(&msg)),
            )),
            Some(_) => Ok(()),
        }
    }

    /// `int a = 1, *b;`, or `None` for one that only declares types.
    fn decl(&mut self, ctx: Ctx) -> Result<Option<Stmt>, Error> {
        let pos = self.here();
        let specs = self.specifiers(ctx)?;
        if self.is_punct(";") {
            self.bare_specifiers(&specs)?;
            return Ok(None);
        }
        let d = self.declarator(&specs.ty, ctx)?;
        if specs.class == Class::Typedef {
            self.typedefs(&specs, d, ctx)?;
            return Ok(None);
        }
        let decls = self.init_declarators(&specs, Some(d), ctx)?;
        Ok(Some(Stmt {
            kind: StmtKind::Decl(decls),
            pos,
        }))
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
        if self.at_decl() {
            return Err(Error::new(
                pos,
                "a declaration is not a statement; wrap it in '{ }'",
            ));
        }
        let kind = match self.peek().clone() {
            Tok::Punct("{") => StmtKind::Block(self.block_body(true)?),
            Tok::Punct(";") => {
                self.advance();
                StmtKind::Empty
            }
            Tok::Ident(name) if matches!(self.peek_at(1), Tok::Punct(":")) => {
                self.advance();
                self.advance();
                StmtKind::Label {
                    name,
                    body: Box::new(self.stmt()?),
                }
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
            Tok::Kw(Kw::Switch) => {
                self.advance();
                let cond = self.paren_expr()?;
                StmtKind::Switch {
                    cond,
                    body: Box::new(self.stmt()?),
                    cases: Vec::new(),
                    default: None,
                }
            }
            Tok::Kw(Kw::Case) => {
                self.advance();
                let expr = self.cond()?;
                self.expect_punct(":")?;
                StmtKind::Case {
                    expr,
                    value: 0,
                    id: 0,
                    body: Box::new(self.stmt()?),
                }
            }
            Tok::Kw(Kw::Default) => {
                self.advance();
                self.expect_punct(":")?;
                StmtKind::Default {
                    id: 0,
                    body: Box::new(self.stmt()?),
                }
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
                self.scoped(|p| p.for_rest())?
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
            Tok::Kw(Kw::Goto) => {
                self.advance();
                let (name, _) = self.expect_ident("a label name after 'goto'")?;
                self.expect_semi("after 'goto'")?;
                StmtKind::Goto(name)
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

    /// After `for`: `(init; cond; step) body`, in the `for`'s own scope.
    fn for_rest(&mut self) -> Result<StmtKind, Error> {
        self.expect_punct("(")?;
        let init = if self.at_decl() {
            self.decl(Ctx::ForInit)?.map(Box::new)
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
        Ok(StmtKind::For {
            init,
            cond,
            step,
            body,
        })
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
                let (ty, spelled) = self.type_name_spelled()?;
                self.expect_punct(")")?;
                if self.is_punct("{") {
                    let e = self.compound_literal(ty, pos)?;
                    return self.postfix_ops(e);
                }
                let x = self.nested(Self::unary)?;
                return Ok(Expr {
                    kind: ExprKind::Cast {
                        x: Box::new(x),
                        implicit: false,
                        spelled,
                    },
                    // A cast's result is a value: qualifiers mean nothing on it.
                    ty: ty.unqual().clone(),
                    pos,
                });
            }
            Tok::Kw(Kw::Sizeof) => {
                self.advance();
                if self.is_punct("(") && self.type_name_follows() {
                    let at = self.advance().pos;
                    let ty = self.type_name()?;
                    self.expect_punct(")")?;
                    if self.is_punct("{") {
                        let e = self.compound_literal(ty, at)?;
                        ExprKind::SizeofExpr(Box::new(self.postfix_ops(e)?))
                    } else {
                        ExprKind::SizeofType(ty)
                    }
                } else {
                    ExprKind::SizeofExpr(Box::new(self.nested(Self::unary)?))
                }
            }
            _ => return self.postfix(),
        };
        Ok(Expr::new(kind, pos))
    }

    /// `(type){ ... }`, after the `)`.
    fn compound_literal(&mut self, ty: Type, pos: Pos) -> Result<Expr, Error> {
        let init = self.initializer()?;
        Ok(Expr {
            kind: ExprKind::CompoundLit {
                init: Box::new(init),
                local: None,
            },
            ty,
            pos,
        })
    }

    fn postfix(&mut self) -> Result<Expr, Error> {
        let e = self.primary()?;
        self.postfix_ops(e)
    }

    /// The postfix operators after `e`: `[i]`, calls, `.`, `->`, `++`, `--`.
    fn postfix_ops(&mut self, mut e: Expr) -> Result<Expr, Error> {
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
                Tok::Punct("(") => {
                    self.enter()?;
                    self.advance();
                    let mut args = Vec::new();
                    if !self.eat_punct(")") {
                        loop {
                            if args.len() == MAX_PARAMS {
                                return Err(Error::new(
                                    self.here(),
                                    format!("too many arguments: merlin supports at most {MAX_PARAMS}"),
                                ));
                            }
                            args.push(self.assign()?);
                            if self.eat_punct(")") {
                                break;
                            }
                            if !self.eat_punct(",") {
                                return Err(self.unexpected("',' or ')' in the argument list"));
                            }
                        }
                    }
                    // A call is positioned at its callee, as clang reports it.
                    let pos = e.pos;
                    e = Expr::new(ExprKind::call(e, args), pos);
                    continue;
                }
                Tok::Punct(p @ ("." | "->")) => {
                    let arrow = *p == "->";
                    self.enter()?;
                    let pos = self.advance().pos;
                    let (name, _) = self.expect_ident("a member name")?;
                    e = Expr::new(
                        ExprKind::Member {
                            base: Box::new(e),
                            name,
                            arrow,
                            offset: 0,
                        },
                        pos,
                    );
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
            Tok::Ident(name)
                if name.starts_with("__builtin_va_") && matches!(self.peek_at(1), Tok::Punct("(")) =>
            {
                return self.va_builtin(&name);
            }
            Tok::Ident(name) => {
                match self.lookup_name(&name) {
                    // An enumeration constant is an `int`, which remembers its
                    // enum (none inside the enum's own definition).
                    Some(Name::EnumConst(v, of)) => {
                        let kind = match of.0 {
                            Some(_) => ExprKind::Enumerator {
                                name: name.clone(),
                                value: *v,
                                of: of.clone(),
                            },
                            None => ExprKind::Num(*v),
                        };
                        self.advance();
                        return Ok(Expr::new(kind, pos));
                    }
                    Some(Name::Typedef(..)) => {
                        return Err(Error::new(
                            pos,
                            format!("unexpected type name '{name}': expected expression"),
                        ));
                    }
                    _ => {}
                }
                self.advance();
                ExprKind::Var { name, var: None }
            }
            Tok::Punct("(") => {
                // Keep the inner node; parentheses only steer the tree's shape.
                return self.paren_expr();
            }
            _ => return Err(self.unexpected("expression")),
        };
        Ok(Expr::new(kind, pos))
    }

    /// `__builtin_va_start(ap, last)`, `__builtin_va_arg(ap, type)`,
    /// `__builtin_va_end(ap)` and `__builtin_va_copy(dst, src)`: clang's
    /// builtins behind `<stdarg.h>`'s macros. They look like calls, but
    /// `va_arg` takes a type, so the parser knows them by name.
    fn va_builtin(&mut self, name: &str) -> Result<Expr, Error> {
        let pos = self.advance().pos;
        self.expect_punct("(")?;
        let first = Box::new(self.assign()?);
        let kind = match name {
            "__builtin_va_start" | "__builtin_va_copy" => {
                self.expect_punct(",")?;
                let second = Box::new(self.assign()?);
                if name == "__builtin_va_start" {
                    ExprKind::VaStart {
                        ap: first,
                        last: second,
                    }
                } else {
                    ExprKind::VaCopy(first, second)
                }
            }
            "__builtin_va_arg" => {
                self.expect_punct(",")?;
                let ty = self.type_name()?;
                self.expect_punct(")")?;
                return Ok(Expr {
                    kind: ExprKind::VaArg(first),
                    // The value is a copy: qualifiers mean nothing on it.
                    ty: ty.unqual().clone(),
                    pos,
                });
            }
            "__builtin_va_end" => ExprKind::VaEnd(first),
            _ => return Err(Error::new(pos, format!("unknown builtin '{name}'"))),
        };
        self.expect_punct(")")?;
        Ok(Expr::new(kind, pos))
    }
}

/// A function declaration or definition from its declarator.
fn function(d: Decl, params: Vec<Param>, specs: &Specs, body: Option<Vec<Stmt>>, end: Pos) -> Function {
    let (ret, variadic) = match d.ty.func_type() {
        Some(f) => (f.ret.clone(), f.variadic),
        None => unreachable!("only function declarators get here"),
    };
    Function {
        name: d.name,
        ret,
        ret_spelled: specs.spelled.clone(),
        params,
        variadic,
        is_static: specs.class == Class::Static,
        is_inline: specs.inline,
        is_extern: specs.class == Class::Extern,
        inline_only: false,
        prototyped: d.prototyped,
        body,
        locals: Vec::new(),
        ret_ptr: None,
        pos: d.pos,
        end,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::lex;

    fn prog(src: &str) -> Program {
        parse(&lex(src, Xlen::Rv32).unwrap(), Xlen::Rv32).unwrap_or_else(|e| panic!("{src}: {e}"))
    }

    fn err(src: &str) -> String {
        parse(&lex(src, Xlen::Rv32).unwrap(), Xlen::Rv32)
            .unwrap_err()
            .to_string()
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
            ExprKind::Num(v) | ExprKind::Enumerator { value: v, .. } => v.to_string(),
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
            ExprKind::Member {
                base, name, arrow, ..
            } => {
                format!("({} {} {name})", if *arrow { "->" } else { "." }, sx(base))
            }
            ExprKind::Call { callee, args, .. } => {
                let mut s = format!("(call {}", sx(callee));
                for a in args {
                    s.push(' ');
                    s.push_str(&sx(a));
                }
                s + ")"
            }
            ExprKind::VaStart { ap, last } => format!("(va_start {} {})", sx(ap), sx(last)),
            ExprKind::VaArg(ap) => format!("(va_arg {} {})", sx(ap), e.ty),
            ExprKind::VaEnd(ap) => format!("(va_end {})", sx(ap)),
            ExprKind::VaCopy(d, s) => format!("(va_copy {} {})", sx(d), sx(s)),
            ExprKind::CompoundLit { init, .. } => format!("(literal {} {})", e.ty, si(init)),
        }
    }

    fn si(i: &Init) -> String {
        match i {
            Init::Expr(e) => sx(e),
            Init::List(items, _) => format!("{{{}}}", items.iter().map(si).collect::<Vec<_>>().join(" ")),
            Init::Designated(ds, init) => {
                let mut s = String::new();
                for d in ds {
                    match d {
                        Designator::Member(name, _) => s += &format!(".{name}"),
                        Designator::Index(e, _) => s += &format!("[{}]", sx(e)),
                    }
                }
                format!("{s}={}", si(init))
            }
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
            StmtKind::Switch { cond, body, .. } => format!("(switch {} {})", sx(cond), ss(body)),
            StmtKind::Case { expr, body, .. } => format!("(case {} {})", sx(expr), ss(body)),
            StmtKind::Default { body, .. } => format!("(default {})", ss(body)),
            StmtKind::Label { name, body } => format!("({name}: {})", ss(body)),
            StmtKind::Goto(name) => format!("(goto {name})"),
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
    fn void_parameter_list_through_a_typedef() {
        let p = prog("typedef void V; int f(V); int g(V) { return 0; }");
        assert!(funcs(&p).iter().all(|f| f.params.is_empty()));
        assert_eq!(globals("typedef void V; int (*h)(V);")[0], "int (*h)(void)");
        // Only a lone, unnamed, unqualified void is "no parameters".
        prog("typedef void V; int f(V x); int g(const V); int h(V, int);");
    }

    #[test]
    fn array_parameter_qualifiers() {
        let params = |src: &str| -> Vec<String> {
            let p = prog(src);
            let f = funcs(&p)[0];
            f.params.iter().map(|p| p.ty.declare(&p.name)).collect()
        };
        assert_eq!(params("int f(int a[const 3]);"), ["int *const a"]);
        assert_eq!(params("int f(int a[static 3]);"), ["int *a"]);
        assert_eq!(
            params("int f(char a[const volatile static 4][2], int b[restrict], int [const]);"),
            ["char (*const volatile a)[2]", "int *b", "int *const"]
        );
        assert_eq!(params("int f(int a[static const 3]);"), ["int *const a"]);
        // Anywhere else they have no pointer to qualify.
        let only = "qualifiers and 'static' inside '[]' are only allowed in the outermost array of a \
                    function parameter";
        assert_eq!(err("int a[const 3];"), format!("1:6: {only}"));
        assert_eq!(err("int f(int a[3][const 2]);"), format!("1:15: {only}"));
        assert_eq!(err("int f(int (*a)[const 2]);"), format!("1:15: {only}"));
        assert_eq!(
            err("int f() { return sizeof(int [const 2]); }"),
            format!("1:29: {only}")
        );
        assert_eq!(
            err("int f(int a[static]);"),
            "1:19: expected the array size after 'static', found ']'"
        );
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
        assert_eq!(err("long long long x;"), "1:11: 'long long long' is invalid");
        assert_eq!(
            err("short long long x;"),
            "1:1: invalid combination of type specifiers"
        );
        assert_eq!(err("static x;"), "1:8: expected a type, found identifier 'x'");
        assert_eq!(
            err("static extern int x;"),
            "1:8: at most one storage class ('static', 'extern', 'typedef') is allowed"
        );
        assert_eq!(ty("_Bool"), "_Bool x");
        assert_eq!(ty("const int"), "const int x");
        assert_eq!(ty("int const volatile"), "const volatile int x");
        assert_eq!(err("_Bool int x;"), "1:1: invalid combination of type specifiers");
        assert_eq!(
            err("unsigned _Bool x;"),
            "1:1: invalid combination of type specifiers"
        );
        assert_eq!(err("int;"), "1:4: declaration does not declare anything");
        assert_eq!(err("const x;"), "1:7: expected a type, found identifier 'x'");
    }

    /// The statements of the last function of `{globals} int f() { {stmts} }`.
    fn body_with(globals: &str, stmts: &str) -> Vec<String> {
        let p = prog(&format!("{globals}\nint f() {{ {stmts} }}"));
        funcs(&p)
            .last()
            .unwrap()
            .body
            .as_ref()
            .unwrap()
            .iter()
            .map(ss)
            .collect()
    }

    #[test]
    fn function_pointer_declarators() {
        assert_eq!(globals("int (*f)(int);"), ["int (*f)(int)"]);
        assert_eq!(
            globals("void (*table[3])(void); int *(*pick)(char, int *);"),
            ["void (*table[3])(void)", "int *(*pick)(char, int *)"]
        );
        assert_eq!(globals("int (*pa)[4];"), ["int (*pa)[4]"]);
        assert_eq!(globals("char *const *volatile p;"), ["char *const *volatile p"]);
        // A function returning a pointer to a function: the parameter list
        // next to the name is the function's own.
        let p = prog("int (*get(int which))(char);");
        assert_eq!(funcs(&p)[0].ret.to_string(), "int (*)(char)");
        assert_eq!(funcs(&p)[0].params[0].name, "which");
        // Parameters of function type are pointers.
        let p = prog("void apply(int f(int), int (*g)(int), const int k);");
        let ps: Vec<String> = funcs(&p)[0]
            .params
            .iter()
            .map(|p| p.ty.declare(&p.name))
            .collect();
        assert_eq!(ps, ["int (*f)(int)", "int (*g)(int)", "const int k"]);
        // ... and the function's type has no top-level qualifiers on them.
        assert_eq!(
            funcs(&p)[0].ty().to_string(),
            "void (int (*)(int), int (*)(int), int)"
        );
        assert_eq!(ex("(int (*)(void))0"), "(cast int (*)(void) 0)");
        assert_eq!(ex("sizeof(void (*[2])(int))"), "(sizeof void (*[2])(int))");
        assert_eq!(
            ex("(const int)x"),
            "(cast int x)",
            "a cast's type has no qualifiers"
        );
    }

    #[test]
    fn typedefs() {
        assert_eq!(
            globals(
                "typedef unsigned long size_t; typedef int *ip, arr3[3]; size_t n; ip p; arr3 a; \
                 const ip cp;"
            ),
            ["unsigned long n", "int *p", "int a[3]", "int *const cp"]
        );
        assert_eq!(
            globals("typedef void (*handler)(int); handler table[2]; handler (*pick)(int);"),
            ["void (*table[2])(int)", "void (*(*pick)(int))(int)"]
        );
        // A typedef's name may be reused for a variable in an inner scope,
        // and then `T * y` multiplies.
        assert_eq!(
            body_with(
                "typedef int T; int x, y;",
                "T * p; x * y; { int T = 2; T * y; } T q;"
            ),
            [
                "(decl int *p)",
                "(* x y);",
                "{(decl int T=2) (* T y);}",
                "(decl int q)"
            ]
        );
        // Typedefs in a block end with it.
        assert_eq!(
            body_with("", "{ typedef char C; C c; } return 0;"),
            ["{(decl char c)}", "(return 0)"]
        );
        assert_eq!(
            err("typedef int T; int f() { return T; }"),
            "1:33: unexpected type name 'T': expected expression"
        );
        assert_eq!(
            err("typedef int T; typedef char T;"),
            "1:29: typedef redefinition with different types ('char' vs 'int')"
        );
        prog("typedef int T; typedef int T;");
        assert_eq!(
            err("typedef int x = 1;"),
            "1:15: illegal initializer (only variables can be initialized)"
        );
        assert_eq!(
            err("int T; typedef int T;"),
            "1:20: redefinition of 'T' as a different kind of symbol (first declared at 1:5)"
        );
        // A label may have a typedef's name.
        assert_eq!(body_with("typedef int L;", "L: return 1;"), ["(L: (return 1))"]);
    }

    fn global_types(src: &str) -> Vec<Type> {
        prog(src)
            .items
            .iter()
            .filter_map(|i| match i {
                Item::Decl(ds) => Some(ds),
                Item::Func(_) => None,
            })
            .flatten()
            .map(|d| d.ty.clone())
            .collect()
    }

    #[test]
    fn structs_and_unions() {
        assert_eq!(
            globals("struct point { int x, y; } origin, *pp; union u { char c; int i; } v; struct point q;"),
            [
                "struct point origin, struct point *pp",
                "union u v",
                "struct point q"
            ]
        );
        let t = global_types("struct point { int x, y; } origin, *pp; struct point q;");
        // One tag, one type.
        assert_eq!(t[1].pointee(), Some(&t[0]));
        assert_eq!(t[2], t[0]);
        assert_eq!(t[0].size(Xlen::Rv32), Some(8));
        // A struct may point to itself.
        let t = global_types("struct node { int v; struct node *next; } n;");
        let body = t[0].record().unwrap().body().unwrap();
        assert_eq!(body.members[1].ty.pointee(), Some(&t[0]));
        // A forward declaration, completed later.
        let t = global_types("struct s; struct s *p; struct s { char c; } x;");
        assert_eq!(t[0].pointee(), Some(&t[1]));
        assert_eq!(t[0].pointee().unwrap().size(Xlen::Rv32), Some(1));
        // An inner scope's struct is a different type.
        let p = prog("struct s { int a; } g; int f() { struct s { char c[3]; } l; return sizeof l; }");
        let StmtKind::Decl(ds) = &funcs(&p)[0].body.as_ref().unwrap()[0].kind else {
            panic!()
        };
        assert_eq!(ds[0].ty.size(Xlen::Rv32), Some(3));
        // Anonymous structs and nested definitions (whose tags are in scope outside).
        let t = global_types("struct { struct in { short a; } i; char c; } a; struct in b;");
        assert_eq!((t[0].size(Xlen::Rv32), t[1].size(Xlen::Rv32)), (Some(4), Some(2)));
        assert_eq!(t[0].to_string(), "struct (anonymous at 1:1)");
        for (src, msg) in [
            (
                "struct s { int a; int a; };",
                "1:23: duplicate member 'a' (first declared at 1:16)",
            ),
            (
                "struct s { int a; }; struct s { int b; };",
                "1:29: redefinition of 'struct s'",
            ),
            (
                "struct s { int a; }; union s x;",
                "1:28: use of 's' with tag type that does not match previous declaration",
            ),
            ("struct s { };", "1:10: struct has no members"),
            (
                "struct s { int a : 3; };",
                "1:18: bit-fields are not supported yet",
            ),
            (
                "struct s { struct t m; };",
                "1:21: field 'm' has incomplete type 'struct t'",
            ),
            (
                "struct s { int f(void); };",
                "1:16: field 'f' declared as a function",
            ),
            (
                "struct s { int n; int x[]; };",
                "1:23: flexible array members are not supported yet",
            ),
            (
                "struct s { int; };",
                "1:15: declaration does not declare anything (anonymous members are not supported)",
            ),
            (
                "struct { int a; };",
                "1:18: declaration does not declare anything",
            ),
            (
                "struct s { static int a; };",
                "1:12: 'static' is not allowed here",
            ),
            ("struct;", "1:7: expected a name or '{' after 'struct', found ';'"),
            (
                "struct s { char c[0x7fffffff]; int i; };",
                "1:36: struct is too large",
            ),
        ] {
            assert_eq!(err(src), msg, "{src}");
        }
    }

    #[test]
    fn enums() {
        assert_eq!(
            globals("enum color { RED, GREEN = 5, BLUE } c; int a[BLUE]; enum color d;"),
            ["unsigned int c", "int a[6]", "unsigned int d"]
        );
        assert_eq!(globals("enum { NEG = -1, ZERO, } e;"), ["int e"]);
        assert_eq!(
            body_with("enum { K = 3, L = K * 2 };", "return K + L;"),
            ["(return (+ 3 6))"]
        );
        // An enumeration constant can be hidden by a variable.
        assert_eq!(
            body_with("enum { K = 3 };", "int K = 4; return K;"),
            ["(decl int K=4)", "(return K)"]
        );
        for (src, msg) in [
            (
                "enum e x;",
                "1:6: use of undefined 'enum e' (merlin needs an enum's constants before its name)",
            ),
            (
                "enum { A, A };",
                "1:11: redefinition of enumerator 'A' (first declared at 1:8)",
            ),
            (
                "enum { A = 2147483647, B };",
                "1:24: enumerator value 2147483648 is not representable as an 'int'",
            ),
            ("enum { };", "1:1: use of empty enum"),
            (
                "int x; enum { x };",
                "1:15: redefinition of 'x' as a different kind of symbol (first declared at 1:5)",
            ),
            (
                "int y; enum { A = y };",
                "1:19: enumerator value is not an integer constant expression",
            ),
            ("enum e { A }; enum e { B };", "1:20: redefinition of 'enum e'"),
            (
                "enum { A B };",
                "1:10: expected ',' or '}' in the enum, found identifier 'B'",
            ),
        ] {
            assert_eq!(err(src), msg, "{src}");
        }
    }

    #[test]
    fn switch_case_and_goto() {
        assert_eq!(
            ss(&stmt(
                "switch (x) { case 1: y; case 2: case 3: break; default: z; }"
            )),
            "(switch x {(case 1 y;) (case 2 (case 3 break)) (default z;)})"
        );
        assert_eq!(ss(&stmt("goto out;")), "(goto out)");
        assert_eq!(ss(&stmt("out: return 1;")), "(out: (return 1))");
        assert_eq!(ss(&stmt("a: b: ;")), "(a: (b: ;))");
        assert_eq!(
            err("int f() { case 1 x; }"),
            "1:18: expected ':', found identifier 'x'"
        );
        assert_eq!(
            err("int f() { goto 3; }"),
            "1:16: expected a label name after 'goto', found '3'"
        );
        assert_eq!(
            err("int f() { default x; }"),
            "1:19: expected ':', found identifier 'x'"
        );
    }

    #[test]
    fn members_and_calls() {
        assert_eq!(ex("s.a"), "(. s a)");
        assert_eq!(ex("p->a.b[2]"), "(* (+ (. (-> p a) b) 2))");
        assert_eq!(ex("&p->a"), "(& (-> p a))");
        assert_eq!(ex("p->a++"), "((-> p a) ++)");
        assert_eq!(ex("*p->q"), "(* (-> p q))");
        assert_eq!(ex("fp(1)(2)"), "(call (call fp 1) 2)");
        assert_eq!(ex("(*fp)(1)"), "(call (* fp) 1)");
        assert_eq!(ex("table[i](x)"), "(call (* (+ table i)) x)");
        assert_eq!(ex("s.f(1).g"), "(. (call (. s f) 1) g)");
        assert_eq!(
            err("int f() { s.(1); }"),
            "1:13: expected a member name, found '('"
        );
    }

    #[test]
    fn qualifiers_and_function_specifiers() {
        assert_eq!(
            globals(
                "const int a = 1; int const b = 2; volatile unsigned *reg; const char *const name = 0; \
                 int *restrict r;"
            ),
            [
                "const int a=1",
                "const int b=2",
                "volatile unsigned int *reg",
                "const char *const name=0",
                "int *r"
            ]
        );
        assert_eq!(globals("const int m[2] = {1, 2};"), ["const int m[2]={1 2}"]);
        let p = prog("static inline int twice(int x) { return 2 * x; } inline int g(void);");
        assert!(funcs(&p)[0].is_inline && funcs(&p)[0].is_static);
        assert!(funcs(&p)[1].is_inline && !funcs(&p)[1].is_static);
        assert_eq!(
            err("inline int x;"),
            "1:12: 'inline' can only appear on functions"
        );
        assert_eq!(
            err("int f(inline int x);"),
            "1:7: 'inline' can only appear on functions"
        );
    }

    #[test]
    fn sizeof_an_expression_sizes_an_array() {
        // The parser works out the operand's type from declarations.
        assert_eq!(
            globals(
                "int a[5]; char b[sizeof a]; char c[sizeof a / sizeof a[0]]; char d[sizeof a[1] + sizeof *a];
                 struct s { char x[3]; int y; } s, *p; char e[sizeof s.x]; char f[sizeof p->y]; char g[sizeof p];
                 char h[sizeof \"abc\"]; char i[sizeof(&a)]; char j[sizeof ((short)1) + sizeof (a + 1)];
                 char k[sizeof (&a[4] - &a[0])]; char l[sizeof sizeof a]; enum { N = sizeof s }; char m[N];"
            ),
            [
                "int a[5]",
                "char b[20]",
                "char c[5]",
                "char d[8]",
                "struct s s, struct s *p",
                "char e[3]",
                "char f[4]",
                "char g[4]",
                "char h[4]",
                "char i[4]",
                "char j[6]",
                "char k[4]",
                "char l[4]",
                "char m[8]",
            ]
        );
        // Innermost declaration wins; parameters are pointers; an array
        // sized by its initializer, or by an earlier declaration.
        let p = prog("int x; int f(char x, int v[10]) { char a[sizeof x]; char b[sizeof v]; return 0; }");
        let body = format!("{:?}", funcs(&p)[0].body);
        assert!(body.contains("Array(Int(Char), Some(1))"), "{body}");
        assert!(body.contains("Array(Int(Char), Some(4))"), "{body}");
        let g: Vec<String> = globals(
            "int t[] = {1, 2, 3}; char u[sizeof t]; char s[] = \"hello\"; char v[sizeof s];
             char w[] = {\"hi\"}; char y[sizeof w]; int z[4]; int z[] = {1}; char q[sizeof z];
             extern int e[]; int e[6]; char r[sizeof e];",
        );
        let sized: Vec<&String> = g
            .iter()
            .filter(|d| d.starts_with("char ") && !d.contains('='))
            .collect();
        assert_eq!(
            sized,
            ["char u[12]", "char v[6]", "char y[3]", "char q[16]", "char r[24]"]
        );
        // A compound literal of unknown size gets its size from its list,
        // as clang does.
        assert_eq!(
            globals(
                "char a[sizeof (int[]){1, 2, 3}]; char b[sizeof (char[]){\"hello\"}];
                 char c[sizeof (short[2]){1}];"
            ),
            ["char a[12]", "char b[6]", "char c[4]"]
        );

        // What the parser can't type stays an error.
        let msg = "array size must be an integer constant expression \
                   (literals, enum constants, operators, casts and sizeof)";
        assert_eq!(err("int f(void); char a[sizeof f()];"), format!("1:21: {msg}"));
        assert_eq!(err("char a[sizeof nothing];"), format!("1:8: {msg}"));
        assert_eq!(
            err("int t[] = {[3] = 1}; char a[sizeof t];"),
            format!("1:29: {msg}")
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
            "1:14: array size must be an integer constant expression \
             (literals, enum constants, operators, casts and sizeof)"
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
            err("int f()[2];"),
            "1:5: function cannot return array type 'int [2]'"
        );
        assert_eq!(
            err("int f()(int);"),
            "1:5: function cannot return function type 'int (int)'"
        );
        assert_eq!(
            err("int a[2](void);"),
            "1:6: 'a' declared as array of functions of type 'int (void)'"
        );
        assert_eq!(err("int a[1] = {[0] 1};"), "1:17: expected '=', found '1'");
        assert_eq!(err("int a[1] = {.};"), "1:14: expected a member name, found '}'");
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
        let ExprKind::Call { args, .. } = e.kind else {
            panic!()
        };
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
        assert!(parse(&lex(&deep(MAX_DEPTH), Xlen::Rv32).unwrap(), Xlen::Rv32).is_ok());
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
        let src = ret(&nest(MAX_DEPTH - 2, "(", "1", ")"));
        assert!(parse(&lex(&src, Xlen::Rv32).unwrap(), Xlen::Rv32).is_ok());
        limit(&ret(&nest(MAX_DEPTH - 1, "(", "1", ")")));
        limit(&ret(&nest(100_000, "(", "1", ")")));
        limit(&ret(&nest(100_000, "- ", "1", "")));
        // Declarators: as many `*`s, or `[]`s, as the limit allows, and no more.
        let stars = |n: usize| format!("int {}p;", "*".repeat(n));
        let dims = |n: usize| format!("char a{};", "[1]".repeat(n));
        assert!(parse(&lex(&stars(MAX_DEPTH), Xlen::Rv32).unwrap(), Xlen::Rv32).is_ok());
        assert!(parse(&lex(&dims(MAX_DEPTH), Xlen::Rv32).unwrap(), Xlen::Rv32).is_ok());
        assert_eq!(
            err(&stars(MAX_DEPTH + 1)),
            "1:261: type nesting too deep: merlin allows at most 256 '*'s"
        );
        assert_eq!(
            err(&dims(100_000)),
            "1:775: type nesting too deep: merlin allows at most 256 array dimensions"
        );
        let cast = format!("int f() {{ return sizeof(int {}); }}", "*".repeat(100_000));
        assert!(err(&cast).ends_with("type nesting too deep: merlin allows at most 256 '*'s"));
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
        let args = vec!["1"; MAX_PARAMS].join(", ");
        let src = ret(&format!("f({args})"));
        assert!(parse(&lex(&src, Xlen::Rv32).unwrap(), Xlen::Rv32).is_ok());
        let args = vec!["1"; MAX_PARAMS + 1].join(", ");
        let src = ret(&format!("f({args})"));
        let e = parse(&lex(&src, Xlen::Rv32).unwrap(), Xlen::Rv32).unwrap_err();
        assert!(
            e.msg.contains("too many arguments: merlin supports at most 127"),
            "{}",
            e.msg
        );
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
        let params: Vec<String> = (0..128).map(|i| format!("int p{i}")).collect();
        let e = err(&format!("int f({}) {{}}", params.join(", ")));
        assert!(
            e.ends_with("too many parameters: merlin supports at most 127"),
            "{e}"
        );
        assert_eq!(
            err("int f(...);"),
            "1:7: a named parameter must come before '...'"
        );
        assert_eq!(
            err("int f(int a, ..., int b);"),
            "1:17: expected ')' after '...', found ','"
        );
    }

    #[test]
    fn static_assertions() {
        assert!(
            prog(
                "_Static_assert(sizeof(int) == 4, \"ILP32\"); enum { N = 3 }; \
                      int f() { _Static_assert(N * 2 == 6, \"in a \" \"block\"); return 0; }"
            )
            .items
            .len()
                == 1
        );
        assert_eq!(
            err("_Static_assert(1 + 1 == 3, \"math\");"),
            "1:1: static assertion failed: math"
        );
        assert_eq!(
            err("int x; _Static_assert(x, \"x\");"),
            "1:23: static assertion expression is not an integer constant expression"
        );
        assert_eq!(err("_Static_assert(1);"), "1:17: expected ',', found ')'");
        // offsetof, as <stddef.h> spells it: the member's offset, which sema
        // has not filled in yet when the parser evaluates the assertion.
        let offsetof = "struct s { char c; int n; struct { short a, b; } in; }; \
                        _Static_assert((unsigned long)&((struct s *)0)->n == 4, \"n\"); \
                        _Static_assert((unsigned long)&((struct s *)0)->in.b == 10, \"in.b\"); \
                        _Static_assert((unsigned long)&(*(struct s *)0).in == 8, \"in\");";
        assert_eq!(prog(offsetof).items.len(), 0);
        assert_eq!(
            err("struct s { char c; int n; }; _Static_assert((unsigned long)&((struct s *)0)->n == 0, \"n\");"),
            "1:30: static assertion failed: n"
        );
    }

    #[test]
    fn designators_and_compound_literals() {
        let StmtKind::Decl(d) = stmt("struct s x = {.a = 1, [2].b.c = 3, 4};").kind else {
            panic!()
        };
        assert_eq!(sd(&d[0]), "struct s x={.a=1 [2].b.c=3 4}");
        let StmtKind::Expr(e) = stmt("p = (int[]){1, 2}[1] + sizeof (char[3]){0};").kind else {
            panic!()
        };
        assert_eq!(
            sx(&e),
            "(= p (+ (* (+ (literal int [] {1 2}) 1)) (sizeof (literal char [3] {0}))))"
        );
    }

    #[test]
    fn variadic_functions() {
        let p = prog("int f(int n, ...); int g(int n, ...) { return n; }");
        let fs = funcs(&p);
        assert!(fs[0].variadic && fs[1].variadic);
        assert_eq!(fs[0].ty().to_string(), "int (int, ...)");
        // The builtins behind <stdarg.h>; __builtin_va_list is a pointer.
        let b = body(
            "__builtin_va_list ap; __builtin_va_start(ap, n); x = __builtin_va_arg(ap, unsigned int); \
                      __builtin_va_copy(aq, ap); __builtin_va_end(ap);",
        );
        let s: Vec<String> = b.iter().map(ss).collect();
        assert_eq!(
            s,
            [
                "(decl void *ap)",
                "(va_start ap n);",
                "(= x (va_arg ap unsigned int));",
                "(va_copy aq ap);",
                "(va_end ap);"
            ]
        );
        assert_eq!(
            err("int f() { __builtin_va_arg(ap); }"),
            "1:30: expected ',', found ')'"
        );
    }
}
