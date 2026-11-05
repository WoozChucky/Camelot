//! Tokens to AST, by recursive descent. Stops at the first error, which
//! is clang's, in its words and where it puts it (`expected ';' after
//! expression`, after the expression; `expected ')'`, at the token there
//! instead, with clang's note `to match this '('`). What clang only warns
//! of, the parser warns of too, and goes on.
//!
//! ```text
//! program     := item* EOF
//! item        := specifiers declarator block                            a function definition
//!              | specifiers (init-decl (',' init-decl)*)? ';'           declarations, of
//!                                                                       functions too
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
//! initializer := assign | '{' (item (',' item)* ','?)? '}'              `{}`: C23's, as clang takes it
//! item        := (designator+ '=')? initializer                       `[i] x`: GNU's, warned of
//! designator  := '.' IDENT | '[' constant ']'
//! params      := ε | 'void' | param (',' param)* (',' '...')?          at most 127
//! param       := specifiers declarator                                  the name is optional
//!                                                                       (in a definition C23's)
//! type-name   := specifiers declarator                                  with no name: casts, sizeof
//! block       := '{' (decl | stmt)* '}'
//! decl        := specifiers (init-decl (',' init-decl)*)? ';'
//! stmt        := block | ';' | expr ';' | IDENT ':' stmt                 no stmt before '}': C23's
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
    align_up, common, distinct_enums, EnumType, IntKind, Quals, Record, Spelled, Spelling, Spellings, Type,
    TypedefName, Unprototyped, MAX_OBJECT_SIZE, PTR_SIZE,
};
use crate::{Error, Pos};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

/// clang's warning for an array size it folds (`int a[1 ? 2 : f()];`, see
/// [`Parser::folded`]), and its flag.
const FOLDED: (&str, &str) = (
    "variable length array folded to constant array as an extension",
    "gnu-folding-constant",
);

/// The error of an array size that is no integer constant expression.
const NOT_CONSTANT_SIZE: &str = "array size must be an integer constant expression \
                                 (literals, enum constants, operators, casts and sizeof)";

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

/// Parses a translation unit, whose sizes the parser
/// needs for array bounds, `sizeof` and struct layouts.
pub fn parse(toks: &[Token]) -> Result<Program, Error> {
    parse_salvaging(toks).map_err(|b| b.0)
}

/// [`parse`], which with a syntax error also gives what came before it:
/// the items parsed, and as much of the function whose body has the error
/// as its blocks hold before it. Sema finds in them the warnings clang
/// gives before such an error (it checks as it parses).
pub fn parse_salvaging(toks: &[Token]) -> Result<Program, Box<(Error, Program)>> {
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
        unevaluated: RefCell::new(Vec::new()),
        prototype: RefCell::new(Vec::new()),
        folding: Cell::new(false),
        salvaged: RefCell::new(Vec::new()),
        type_open: Cell::new(Pos::default()),
        undeclared: RefCell::new(Vec::new()),
        partial: RefCell::new(None),
        parens: RefCell::new(None),
        parens_written: RefCell::new(Default::default()),
        brackets_written: RefCell::new(Default::default()),
        bit_patterns: RefCell::new(Default::default()),
        int_literals: RefCell::new(Default::default()),
        char_literals: RefCell::new(Default::default()),
        corrections: RefCell::new(Default::default()),
        trailing_else: Cell::new(None),
        warnings: RefCell::new(Vec::new()),
    };
    let mut items = Vec::new();
    let mut unevaluated = Vec::new();
    while *p.peek() != Tok::Eof {
        let mark = p.undeclared.borrow().len();
        let start = ExprId::next();
        let item = p.item();
        let error = match item {
            Ok(item) => {
                items.extend(item);
                None
            }
            Err(e) => {
                items.extend(p.partial.take());
                Some(e)
            }
        };
        let n = items.len();
        unevaluated.extend(p.unevaluated.take().into_iter().map(|u| (n, u)));
        // A declaration cut short: its undeclared names (a function's are
        // its blocks').
        if error.is_some() {
            if let Some(Stmt {
                kind: StmtKind::Unevaluated(us),
                ..
            }) = p.cut_names(mark, start)
            {
                unevaluated.extend(us.into_iter().map(|u| (n, u)));
            }
        }
        if let Some(e) = error {
            let partial = Program {
                items,
                unevaluated,
                warnings: p.warnings.take(),
                written: Written {
                    parens: p.parens_written.take(),
                    brackets: p.brackets_written.take(),
                    bit_patterns: p.bit_patterns.take(),
                    int_literals: p.int_literals.take(),
                    char_literals: p.char_literals.take(),
                    corrections: p.corrections.take(),
                    ..Written::default()
                },
            };
            return Err(Box::new((e, partial)));
        }
    }
    Ok(Program {
        items,
        unevaluated,
        warnings: p.warnings.take(),
        written: Written {
            parens: p.parens_written.take(),
            brackets: p.brackets_written.take(),
            bit_patterns: p.bit_patterns.take(),
            int_literals: p.int_literals.take(),
            char_literals: p.char_literals.take(),
            corrections: p.corrections.take(),
            ..Written::default()
        },
    })
}

/// clang's typo correction (`Sema::CorrectTypo`) of `typo` among the
/// `candidates` (names, and what goes with each): the one closest to it by
/// edit distance (Levenshtein's: insertions, deletions, replacements),
/// none if two are as close. A name is a candidate only if the lengths
/// differ by less than a third of the typo's (`typo.len() / diff >= 3`)
/// and the distance is at most a third of it, rounded up; and the best is
/// taken only if the typo is three times as long as the distance or more.
pub fn correction<'n, T>(
    typo: &str,
    candidates: impl IntoIterator<Item = (&'n str, T)>,
) -> Option<(&'n str, T)> {
    let len = typo.chars().count();
    let bound = len.div_ceil(3);
    let mut best: Option<(usize, &'n str, T)> = None;
    let mut tied = false;
    for (name, what) in candidates {
        let diff = len.abs_diff(name.chars().count());
        if diff > 0 && len / diff < 3 {
            continue;
        }
        let d = edit_distance(typo, name);
        if d > bound || d == 0 {
            continue;
        }
        match &best {
            Some((b, ..)) if d > *b => {}
            Some((b, ..)) if d == *b => tied = true,
            _ => {
                best = Some((d, name, what));
                tied = false;
            }
        }
    }
    let (d, name, what) = best?;
    (!tied && len / d >= 3).then_some((name, what))
}

/// Levenshtein's distance of `a` and `b`, in characters.
fn edit_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut prev = row[0];
        row[0] = i + 1;
        for j in 0..b.len() {
            let here = row[j + 1];
            row[j + 1] = (prev + usize::from(ca != b[j])).min(row[j] + 1).min(here + 1);
            prev = here;
        }
    }
    row[b.len()]
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
/// braces) for a character array, or a list of scalars, each designated by
/// a constant index or none (`{[3] = 1, 2}` has 5). `None` for the rest
/// (brace elision, other designators), which sema works out.
fn init_len(elem: &Type, init: &Init) -> Option<u32> {
    // A character type: 1 byte, on every target.
    let is_char = elem.int_kind().is_some_and(|k| k.size() == 1);
    let string = |i: &Init| match i {
        Init::Expr(Expr {
            kind: ExprKind::Str(b),
            ..
        }) if is_char => u32::try_from(b.len() + 1).ok(),
        _ => None,
    };
    match init {
        Init::List(items, _) if items.len() == 1 && string(&items[0]).is_some() => string(&items[0]),
        Init::List(items, _) if elem.is_scalar() => {
            // The index of the next element, and the length so far.
            let (mut next, mut len) = (0u128, 0u128);
            for item in items {
                match item {
                    Init::Expr(_) => {}
                    Init::Designated(designators, init) if matches!(**init, Init::Expr(_)) => {
                        match &designators[..] {
                            [Designator::Index(e, _)] => {
                                next = u128::try_from(consteval::parsed_int(e)?).ok()?
                            }
                            _ => return None,
                        }
                    }
                    _ => return None,
                }
                next += 1;
                len = len.max(next);
            }
            u32::try_from(len).ok()
        }
        _ => string(init),
    }
}

/// How a parameter declared with type `declared` and spelling `spelled`
/// spells its adjusted type, for `-g`. Declared as an array through an array
/// typedef, its own (`arr3 b`, `typedef u32 arr3[3];`), it is a pointer to
/// the elements, and the typedef is gone with the array it named: what is
/// left is spelled as the typedef's own declaration spelled it (`u32 *`),
/// as clang has it. Again while that is a typedef of the same array
/// (`typedef arr3 arr3b;`); not when it names only the elements
/// (`typedef row mat[2];`: `mat m` is a `row *`). Qualifiers go to the
/// elements, so `const row r` is a `const u32 *`. A parameter declared as
/// an array of the typedef's arrays (`arr3 b[2]`), or as a function, keeps
/// its spelling: the decayed type still has the typedef's type (`arr3 *`).
fn decayed_spelling(declared: &Type, mut spelled: Spelled) -> Spelled {
    if !declared.is_array() {
        return spelled;
    }
    while let Some(Spelling::Typedef(t)) = &spelled.0 {
        if !t.ty.is_array() || declared.quals_added_to(&t.ty).is_none() {
            break;
        }
        spelled = t.spelled.clone();
    }
    spelled
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
    /// `register`, and C99's `auto`, which says what no storage class does.
    Register,
    Auto,
}

/// What a list of declaration specifiers says.
struct Specs {
    ty: Type,
    class: Class,
    /// Where the storage class is (where the specifiers start, if none is).
    class_pos: Pos,
    /// Where `inline` is, if it is there.
    inline: Option<Pos>,
    /// The specifiers declare something by themselves (a struct, union or
    /// enum tag, or enumeration constants), so they may end the declaration
    /// with no declarator: `struct point { int x, y; };`.
    declares: bool,
    /// They define a struct or union without a tag: `struct { int x; }`.
    anonymous: bool,
    /// How the type was spelled: a typedef name or an enum (for `-g`).
    spelled: Spelled,
    /// Where they start.
    pos: Pos,
}

/// The width a type's specifiers give it, as clang's `DeclSpec` keeps it
/// apart from the rest: `short`, `long` or `long long`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Width {
    Short,
    Long,
    LongLong,
}

impl Width {
    fn name(self) -> &'static str {
        match self {
            Width::Short => "short",
            Width::Long => "long",
            Width::LongLong => "long long",
        }
    }
}

/// The one type specifier besides the width and the sign: a keyword, or a
/// struct, union, enum or typedef name (with the type it names).
enum Base {
    Void,
    Char,
    Int,
    Bool,
    Named(Type, &'static str),
}

impl Base {
    /// What clang calls it in its diagnostics.
    fn name(&self) -> &'static str {
        match self {
            Base::Void => "void",
            Base::Char => "char",
            Base::Int => "int",
            Base::Bool => "_Bool",
            Base::Named(_, name) => name,
        }
    }
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
    /// An enum: the integer type it is, the enum as a spelling, and where
    /// its tag is.
    Enum(Type, Spelled, Pos),
}

impl Tag {
    /// Where the tag was first declared.
    fn pos(&self) -> Pos {
        match self {
            Tag::Record(r) => r.pos,
            Tag::Enum(.., pos) => *pos,
        }
    }
}

/// One block scope: the file, a function's parameters and body, a block, a
/// `for` statement, or a parameter list.
#[derive(Default)]
struct Scope {
    names: HashMap<String, (Name, Pos)>,
    tags: HashMap<String, Tag>,
    /// The names a block declares with linkage (`extern int a[];`, a
    /// function): at file scope every object and function has it.
    linked: std::collections::HashSet<String>,
}

/// One step of a declarator, from the base type towards the declared type.
enum Derive {
    /// Pointer to the type so far, with these qualifiers.
    Ptr(Quals),
    /// Array of the type so far.
    Array(Array),
    /// Function returning the type so far, with these parameters (and the
    /// scope holding their names), variadic if `...` ends them, whether
    /// the list is a prototype's (not `()`), and where its `(` is.
    Func(Vec<Param>, bool, bool, Scope, Pos),
}

/// A declarator's array step: `[size]`.
struct Array {
    /// How many elements: `None` for `[]`, 1 for a size that is no constant.
    len: Option<i128>,
    /// Where the `[` is, and the size (or the `]`).
    open: Pos,
    size: Pos,
    /// The qualifiers in the brackets (`int a[const 3]`), and whether
    /// `static` was there too: C99 6.7.5.3p7 allows them only on a
    /// parameter's outermost array, and the qualifiers then apply to the
    /// pointer the array is adjusted to.
    quals: Option<(Quals, bool)>,
    /// Where the size is if the parser folded it ([`Parser::folded`]).
    folded: Option<Pos>,
    /// Which of the [`Unevaluated`]s waiting are its size's.
    entries: std::ops::Range<usize>,
    /// Whether the size is a constant (or there is none): if not, sema
    /// reports it ([`Parser::variable_sizes`]).
    constant: bool,
}

/// `n` as clang writes a count, its digits grouped by threes with `'`:
/// `1'073'741'824`.
fn grouped(n: u128) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push('\'');
        }
        out.push(c);
    }
    out
}

/// Where the array sizes a declarator folded are, seen from what it
/// declares: what clang folds depends on it ([`Parser::folded`]). The
/// order is from the best to the worst of them, the worst counting.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Reach {
    /// In its own array type, through arrays only (`a[2][F]`, `*a[F]`).
    Arrays,
    /// Through pointers first, then arrays only (`(*p)[F]`, `(**p)[2][F]`).
    Pointers,
    /// Through a pointer after an array, or an array of unknown size
    /// (`(*a[2])[F]`, `a[][F]`).
    Other,
    /// Through a function's type, in what it returns (`(*f(void))[F]`).
    Function,
}

/// The [`Reach`] of the arrays `pick` chooses among the steps of a
/// declarator (`ops`, the base's first, what is declared last): those
/// whose size is folded, or no constant.
fn reach(ops: &[Derive], pick: impl Fn(&Array) -> bool) -> Reach {
    let mut reach = Reach::Arrays;
    for (k, op) in ops.iter().enumerate() {
        if !matches!(op, Derive::Array(a) if pick(a)) {
            continue;
        }
        // From what is declared in to this array.
        let mut arrays = false;
        for op in ops[k + 1..].iter().rev() {
            match op {
                Derive::Func(..) => {
                    reach = Reach::Function;
                    break;
                }
                Derive::Ptr(_) if arrays => reach = reach.max(Reach::Other),
                Derive::Ptr(_) => reach = reach.max(Reach::Pointers),
                Derive::Array(a) if a.len.is_none() => reach = reach.max(Reach::Other),
                Derive::Array(..) => arrays = true,
            }
        }
    }
    reach
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
    /// When it declares a function: where its parameter list opens.
    open: Option<Pos>,
    /// Where the first array size of its own the parser folded is
    /// ([`Parser::folded`]), if one is, and where the folded ones are.
    folded: Option<Pos>,
    reach: Reach,
    /// Its folded sizes' [`Unevaluated`]s (from its first): its own
    /// array's, if it is one and folded, and the others' (see
    /// [`Unevaluated::vla`]).
    folded_sizes: (Option<std::ops::Range<usize>>, Vec<std::ops::Range<usize>>),
    /// Where its own [`Unevaluated`]s are among those waiting: they came
    /// as it was parsed.
    unevaluated: std::ops::Range<usize>,
}

struct Parser<'a> {
    toks: &'a [Token],
    pos: usize,
    /// Current nesting level; see [`MAX_DEPTH`].
    depth: usize,
    /// Block scopes, innermost last; the first is the file scope.
    scopes: Vec<Scope>,
    /// The expressions the constants evaluated so far have that sema must
    /// still check ([`Unevaluated`]), until the declaration or statement
    /// they are in is done.
    unevaluated: RefCell<Vec<Unevaluated>>,
    /// The parameters declared so far in the parameter lists being parsed,
    /// outermost first ([`Unevaluated::params`]).
    prototype: RefCell<Vec<(String, Type, Pos)>>,
    /// Whether the constant being evaluated folded an operand that is not
    /// constant ([`Parser::not_chosen`]).
    folding: Cell<bool>,
    /// The statements of the blocks a syntax error cut short, the innermost
    /// first, and the function they make up so far ([`parse_salvaging`]).
    salvaged: RefCell<Vec<Vec<Stmt>>>,
    /// Where the `(` around the type name being read is.
    type_open: Cell<Pos>,
    /// The names read as expressions that nothing declares (not called:
    /// that is an implicit declaration), in order, kept for a syntax error
    /// in the statement or declaration they are in: clang checks them as
    /// it reads them, so `return a[1;` is the error of the undeclared `a`
    /// before the syntax error. A block cut short checks those of its
    /// statement that was cut short ([`Parser::cut_names`]).
    undeclared: RefCell<Vec<Expr>>,
    partial: RefCell<Option<Item>>,
    /// While a `_Static_assert`'s condition is parsed, the parenthesized
    /// expressions in it, and where their `(` is ([`Printer`]).
    parens: RefCell<Option<Vec<(ParenKey, Pos)>>>,
    /// The warnings so far: what clang warns of as it parses, and goes on
    /// (`declaration does not declare anything`).
    warnings: RefCell<Vec<Error>>,
    /// Where the operators in parentheses are ([`Written::parens`]), and
    /// the subscripts' `]` ([`Written::brackets`]).
    parens_written: RefCell<std::collections::HashMap<Pos, Pos>>,
    brackets_written: RefCell<std::collections::HashMap<Pos, Pos>>,
    /// Where the literals written in hex or octal are ([`Written::bit_patterns`]).
    bit_patterns: RefCell<std::collections::HashSet<Pos>>,
    /// Where the integer literals are ([`Written::int_literals`]).
    int_literals: RefCell<std::collections::HashSet<Pos>>,
    /// Where the character constants are ([`Written::char_literals`]).
    char_literals: RefCell<std::collections::HashSet<Pos>>,
    /// Typo corrections ([`Written::corrections`]).
    corrections: RefCell<HashMap<Pos, Correction>>,
    /// The `else` of the `if` the statement just parsed ends with, through
    /// loops and labels, not braces: an `if` without one of its own around
    /// it is clang's dangling `else` (its `TrailingElseLoc`).
    trailing_else: Cell<Option<Pos>>,
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

    /// clang's `unknown type name` of the name here, with what its typo
    /// correction takes it for ([`Self::correct_type`]).
    fn unknown_type_name(&self, name: &str) -> Error {
        let at = self.here();
        match self.correct_type(name) {
            Some((to, decl)) => {
                let e = Error::new(at, format!("unknown type name '{name}'; did you mean '{to}'?"));
                match decl {
                    Some(decl) => e.note(decl, format!("'{to}' declared here")),
                    None => e,
                }
            }
            None => Error::new(at, format!("unknown type name '{name}'")),
        }
    }

    /// What clang's typo correction takes the undeclared `typo`, read as
    /// an expression, for: a variable, function or enumeration constant in
    /// scope ([`correction`]).
    fn correct_expr(&self, typo: &str) -> Option<Correction> {
        let mut seen = std::collections::HashSet::new();
        let candidates = self
            .scopes
            .iter()
            .rev()
            .flat_map(|s| &s.names)
            .filter_map(|(n, (what, at))| {
                if !seen.insert(n.as_str()) {
                    return None;
                }
                match what {
                    Name::Typedef(..) => None,
                    Name::EnumConst(v, of) => Some((n.as_str(), (*at, Some((*v, of.clone()))))),
                    Name::Object(_) => Some((n.as_str(), (*at, None))),
                }
            });
        let (name, (pos, enumerator)) = correction(typo, candidates)?;
        Some(Correction {
            name: name.to_string(),
            pos,
            enumerator,
        })
    }

    /// What clang's typo correction takes the unknown type name `typo` for:
    /// a typedef name in scope (where it is declared), or a keyword of a
    /// type specifier or qualifier ([`correction`]).
    fn correct_type(&self, typo: &str) -> Option<(String, Option<Pos>)> {
        const KEYWORDS: [&str; 18] = [
            "char",
            "const",
            "double",
            "enum",
            "float",
            "int",
            "long",
            "short",
            "signed",
            "struct",
            "union",
            "unsigned",
            "void",
            "volatile",
            "_Complex",
            "_Imaginary",
            "_Bool",
            "restrict",
        ];
        let mut seen = std::collections::HashSet::new();
        let typedefs = self
            .scopes
            .iter()
            .rev()
            .flat_map(|s| &s.names)
            .filter_map(|(n, (what, at))| {
                (seen.insert(n.as_str()) && matches!(what, Name::Typedef(..)))
                    .then_some((n.as_str(), Some(*at)))
            });
        let keywords = KEYWORDS.iter().map(|k| (*k, None));
        let (name, decl) = correction(typo, typedefs.chain(keywords))?;
        Some((name.to_string(), decl))
    }

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
    /// error here, with clang's note where the name was declared before.
    /// The composite of `ty` and the type of the earlier declaration of
    /// `name` with linkage this block can see: the innermost around it,
    /// unless it has no linkage (a local hides the outer ones); `ty` if
    /// there is none, or it is not compatible (sema's error).
    fn visible_composite(&self, name: &str, ty: &Type) -> Type {
        for (depth, scope) in self.scopes.iter().enumerate().rev() {
            let Some((what, _)) = scope.names.get(name) else {
                continue;
            };
            return match what {
                Name::Object(t) if (depth == 0 || scope.linked.contains(name)) && t.compatible(ty) => {
                    t.composite(ty)
                }
                _ => ty.clone(),
            };
        }
        ty.clone()
    }

    fn bind_name(&mut self, name: &str, what: Name, pos: Pos) -> Result<(), Error> {
        let scope = self.scopes.last_mut().expect("the file scope");
        let redefinition = |how: String, prev: Pos| {
            Error::new(pos, format!("redefinition of {how}")).note(prev, "previous definition is here")
        };
        match scope.names.get_mut(name) {
            None => {
                scope.names.insert(name.to_string(), (what, pos));
                Ok(())
            }
            // A variable declared again (`extern int a[]; int a[3];`): keep
            // the type that says the most. Whether the types agree is sema's
            // to judge.
            Some((Name::Object(prev), prev_pos)) => match what {
                Name::Object(new) => {
                    if new.size().is_some() || prev.size().is_none() {
                        *prev = new;
                    }
                    Ok(())
                }
                Name::EnumConst(..) => Err(redefinition(format!("'{name}'"), *prev_pos)),
                Name::Typedef(..) => Err(redefinition(
                    format!("'{name}' as different kind of symbol"),
                    *prev_pos,
                )),
            },
            // A typedef name defined again: of the same type, what C11
            // allows and clang warns of under C99 (the name is then the
            // later's); of another, an error. Two enums are two types,
            // though merlin's are their integer type.
            Some((Name::Typedef(prev, prev_name), prev_pos)) if matches!(what, Name::Typedef(..)) => {
                let Name::Typedef(new, new_name) = &what else {
                    unreachable!()
                };
                // How each typedef's own declaration spelled its type.
                let base = |name: &Spelled| match &name.0 {
                    Some(Spelling::Typedef(t)) => t.spelled.clone(),
                    _ => Spelled::default(),
                };
                let (prev_base, new_base) = (base(prev_name), base(new_name));
                if prev == new && !distinct_enums(prev, &prev_base, new, &new_base) {
                    let msg = format!("redefinition of typedef '{name}' is a C11 feature");
                    let w = Error::warning(pos, msg, "typedef-redefinition")
                        .note(*prev_pos, "previous definition is here");
                    self.warnings.borrow_mut().push(w);
                    scope.names.insert(name.to_string(), (what, pos));
                    return Ok(());
                }
                let (new, prev) = (new.named(&new_base), prev.named(&prev_base));
                Err(Error::new(
                    pos,
                    format!("typedef redefinition with different types ({new} vs {prev})"),
                )
                .note(*prev_pos, "previous definition is here"))
            }
            Some((prev, _)) if *prev == what && !matches!(what, Name::EnumConst(..)) => Ok(()),
            Some((Name::EnumConst(..), prev_pos)) if matches!(what, Name::EnumConst(..)) => {
                Err(redefinition(format!("enumerator '{name}'"), *prev_pos))
            }
            // An enumerator after anything else is plainly a redefinition
            // to clang.
            Some((_, prev_pos)) if matches!(what, Name::EnumConst(..)) => {
                Err(redefinition(format!("'{name}'"), *prev_pos))
            }
            Some((_, prev_pos)) => Err(redefinition(
                format!("'{name}' as different kind of symbol"),
                *prev_pos,
            )),
        }
    }

    // ---- constant expressions ----

    /// The value of an integer constant expression the parser needs now: an
    /// array size, an enumerator's value or a `_Static_assert`; or the error
    /// `msg` at `at` if it is none. Each `sizeof expr` in it whose operand's
    /// type the parser can work out becomes `sizeof (type)` first, so
    /// `char buf[sizeof x]` and `int b[sizeof a / sizeof a[0]]` work before
    /// sema has typed anything; and so does each operand a constant `?:`
    /// does not choose, whose type alone counts (see
    /// [`Self::resolve_sizeof`]). Those operands are not evaluated, but sema
    /// still checks them ([`Unevaluated`]). The flag says whether the value
    /// is sure: false if the parser could not type an operand `?:` does not
    /// choose, or found no constant, whose error sema will report (`msg` if
    /// it finds none); the value is then 1, which nothing uses.
    /// `non_integer` is clang's error for a constant of a type that is no
    /// integer, if it has one there: with it, an operand not chosen that
    /// is none makes sema check the whole expression, whose type clang
    /// reports (`1 ? 2 : p` is an `int *`).
    fn const_int(
        &self,
        e: &mut Expr,
        at: Pos,
        msg: &str,
        non_integer: Option<&'static str>,
    ) -> Result<(i128, bool), Error> {
        let mark = self.unevaluated.borrow().len();
        let original = e.clone();
        self.resolve_sizeof(e);
        let Some(v) = consteval::parsed_int(e) else {
            // No constant the parser can work out: sema checks the whole
            // expression, and reports its error if it has one, as clang
            // does (`sizeof n.y` of an `int n` is no member), else `msg`.
            // Nothing in it was folded.
            self.unevaluated.borrow_mut().truncate(mark);
            self.folding.set(false);
            self.unevaluated.borrow_mut().push(Unevaluated {
                expr: original,
                untyped: Some((at, msg.to_string())),
                non_integer: non_integer.map(|m| (at, m)),
                folded: false,
                diagnostic: None,
                with: 0,
                vla: None,
                params: self.prototype.borrow().clone(),
                constant: false,
            });
            return Ok((1, false));
        };
        let mut sure = true;
        for u in &mut self.unevaluated.borrow_mut()[mark..] {
            if let Some(error) = &mut u.untyped {
                *error = (at, msg.to_string());
                sure = false;
            }
        }
        if !sure && non_integer.is_some() {
            let mut us = self.unevaluated.borrow_mut();
            us.truncate(mark);
            self.folding.set(false);
            us.push(Unevaluated {
                expr: original,
                untyped: Some((at, msg.to_string())),
                non_integer: non_integer.map(|m| (at, m)),
                folded: false,
                diagnostic: None,
                with: 0,
                vla: None,
                params: self.prototype.borrow().clone(),
                constant: false,
            });
            return Ok((1, false));
        }
        // The constant itself, for sema to check as clang does: not a lone
        // literal, constant or `sizeof`, of which there is nothing to say.
        let leaf = matches!(
            e.kind,
            ExprKind::Num(_) | ExprKind::Enumerator { .. } | ExprKind::SizeofType(_)
        );
        if sure && !leaf {
            self.unevaluated.borrow_mut().push(Unevaluated {
                expr: e.clone(),
                untyped: None,
                non_integer: None,
                folded: false,
                diagnostic: None,
                with: 0,
                vla: None,
                params: self.prototype.borrow().clone(),
                constant: true,
            });
        }
        Ok((v, sure))
    }

    /// Rewrites `sizeof expr` into `sizeof (type)` throughout the operators
    /// [`consteval`] evaluates, where [`Self::operand_type`] knows the type,
    /// and the operand a constant `?:` does not choose as
    /// [`Self::not_chosen`] says.
    fn resolve_sizeof(&self, e: &mut Expr) {
        match &mut e.kind {
            ExprKind::SizeofExpr(x, _) => {
                if let Some(t) = self.operand_type(x) {
                    self.unevaluated.borrow_mut().push(Unevaluated {
                        expr: (**x).clone(),
                        untyped: None,
                        non_integer: None,
                        folded: false,
                        diagnostic: None,
                        with: 0,
                        vla: None,
                        params: self.prototype.borrow().clone(),
                        constant: false,
                    });
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
                ..
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
                match consteval::parsed_int(c) {
                    Some(v) => {
                        let (chosen, other) = if v != 0 { (a, b) } else { (b, a) };
                        self.resolve_sizeof(chosen);
                        self.not_chosen(other);
                    }
                    None => {
                        self.resolve_sizeof(a);
                        self.resolve_sizeof(b);
                    }
                }
            }
            _ => {}
        }
    }

    /// The operand a constant `?:` does not choose. It is not evaluated
    /// (C99 6.6p3): only its type counts, which the result's depends on. One
    /// that is no integer constant expression (`1 ? 2 : f()`) becomes a 0
    /// of its type, as sema types it, and sema checks what it was
    /// ([`Unevaluated`]); clang takes it, folding it (`folding`: an array
    /// sized so is folded, [`Self::folded`]). If the parser cannot work out
    /// its type, or it is no integer type (which makes the `?:` no integer
    /// constant expression), the 0 is an `int` and sema reports the error.
    fn not_chosen(&self, e: &mut Expr) {
        let mark = self.unevaluated.borrow().len();
        let mut resolved = e.clone();
        self.resolve_sizeof(&mut resolved);
        // A constant, as it is not evaluated: `1 / 0` is one there (C99
        // 6.6p3), and nothing is folded.
        if consteval::parsed_unevaluated_int(&resolved).is_some() {
            *e = resolved;
            return;
        }
        // The whole operand is checked, so not what is in it again.
        self.unevaluated.borrow_mut().truncate(mark);
        let (ty, untyped) = match self.value_type(e) {
            Some(t) if t.is_integer() => (t.unqual().clone(), None),
            _ => (Type::INT, Some((e.pos, String::new()))),
        };
        let pos = e.pos;
        let original = std::mem::replace(
            e,
            Expr {
                id: ExprId::new(),
                kind: ExprKind::Num(0),
                ty,
                pos,
            },
        );
        self.unevaluated.borrow_mut().push(Unevaluated {
            expr: original,
            untyped,
            non_integer: None,
            folded: true,
            diagnostic: None,
            with: 0,
            vla: None,
            params: self.prototype.borrow().clone(),
            constant: false,
        });
        self.folding.set(true);
    }

    /// An array size folded (`int a[1 ? 2 : f()]`, whose `f()` is no
    /// constant): clang takes the array as a constant one where an array
    /// cannot be variable-length, warning that it folds it (GNU's folding
    /// of a constant), where it can ([`Reach`]):
    ///
    /// * at file scope and in a struct or union member, through pointers
    ///   and then arrays of a known size (`(*p)[F]`, `a[2][F]`), as
    ///   `TryToFixInvalidVariablyModifiedType` folds; any other place is
    ///   clang's error (`(*a[2])[F]`, `a[][F]`);
    /// * in a `static` local and in a local array with an initializer, in
    ///   the object's own array type (`a[2][F]`), where clang needs a
    ///   constant array; through a pointer (`static int (*p)[F]`) it is
    ///   clang's variably modified type;
    /// * never in a function's return type (`int (*f(void))[F]`), which
    ///   clang calls an error.
    ///
    /// Elsewhere (a local array without an initializer, a local typedef, a
    /// pointer to such an array, a parameter, a type name) it is a
    /// variable-length array, or a type made of one, which merlin does not
    /// have: the size is an error, as `int a[n]` is. `d` is declared in
    /// `ctx` with storage class `class`, and an initializer if `init`.
    ///
    /// Whatever is said of it, sema says, unless an operand of the
    /// declarator's sizes has an error, which clang reports alone
    /// ([`Self::fold_diagnostic`]).
    fn folded(&self, d: &Decl, ctx: Ctx, class: Class, init: bool) {
        let Some(at) = d.folded else { return };
        if d.ty.is_func() && class != Class::Typedef && d.reach == Reach::Function {
            let msg = "function declaration cannot have variably modified type";
            return self.fold_diagnostic(d, Error::new(d.pos, msg));
        }
        // What clang can make a constant array of.
        let fixable = d.reach <= Reach::Pointers;
        let folds = match ctx {
            Ctx::File if !fixable => {
                let msg = "variably modified type declaration not allowed at file scope";
                return self.fold_diagnostic(d, Error::new(d.pos, msg));
            }
            Ctx::Member if !fixable => {
                let msg = "fields must have a constant size: 'variable length array in structure' \
                           extension will never be supported";
                return self.fold_diagnostic(d, Error::new(d.pos, msg));
            }
            Ctx::File | Ctx::Member => true,
            Ctx::Block | Ctx::ForInit => match class {
                Class::Static => d.reach == Reach::Arrays,
                Class::Extern => fixable,
                Class::Typedef => false,
                Class::None | Class::Register | Class::Auto => init && d.reach == Reach::Arrays,
            },
            Ctx::Param | Ctx::TypeName => false,
        };
        let (msg, flag) = FOLDED;
        let said = if folds {
            Error::warning(d.pos, msg, flag)
        } else {
            Error::new(at, NOT_CONSTANT_SIZE)
        };
        self.fold_diagnostic(d, said);
        if folds && ctx == Ctx::File {
            let mut us = self.unevaluated.borrow_mut();
            let u = us[d.unevaluated.clone()]
                .last_mut()
                .expect("a folded size has its operand");
            u.vla = Some(FileVla {
                at: d.pos,
                top: d.folded_sizes.0.clone(),
                inner: d.folded_sizes.1.clone(),
            });
        }
    }

    /// A type name's folded size. A compound literal's array (`(int[F]){1,
    /// 2}`, its `(` at `literal`) is folded as an initialized array is,
    /// with clang's warning at the `(`; one clang does not fold
    /// (`(int[][F]){1}`) is its error. At file scope, a type that only
    /// points to such an array (`(int (*)[F])0`) is clang's variably
    /// modified type, which it takes there without a word: merlin takes it
    /// as the array of the folded size, which the size is at run time too
    /// (the operand not chosen is never evaluated). Anything else is a
    /// variable-length array, an error ([`Self::folded`]).
    fn type_name_folded(&self, d: &Decl, literal: Option<Pos>) {
        let Some(at) = d.folded else { return };
        match literal {
            Some(paren) if d.reach == Reach::Arrays => {
                let (msg, flag) = FOLDED;
                self.fold_diagnostic(d, Error::warning(paren, msg, flag));
            }
            // A literal of an array clang does not fold (`(int[][F]){1}`)
            // is one of variable length, which nothing initializes.
            Some(_) if d.ty.is_array() => {
                let msg = "variable-sized object may not be initialized";
                self.fold_diagnostic(d, Error::new(at, msg));
            }
            _ if self.scopes.len() == 1 && d.reach > Reach::Arrays => {}
            _ => self.fold_diagnostic(d, Error::new(at, NOT_CONSTANT_SIZE)),
        }
    }

    /// What is said of a declarator's folded size: kept with the last of
    /// its own operands, for sema to give if none of them has an error, as
    /// clang does ([`Unevaluated::diagnostic`]).
    fn fold_diagnostic(&self, d: &Decl, said: Error) {
        let mut us = self.unevaluated.borrow_mut();
        let own = &mut us[d.unevaluated.clone()];
        let with = own.len() - 1;
        let u = own.last_mut().expect("a folded size has its operand");
        u.diagnostic = Some(said);
        u.with = with;
    }

    /// What the declaration or statement just parsed left for sema to
    /// check, as a statement after it, at `pos`.
    /// The undeclared names read since `mark` that are in the item a syntax
    /// error cut short (read after `item`, its [`ExprId::next`]), as a
    /// statement for sema to check, unevaluated; those of the items before
    /// it are in them. The names since `mark` are taken.
    fn cut_names(&self, mark: usize, item: u64) -> Option<Stmt> {
        let names = self.undeclared.borrow_mut().split_off(mark);
        let us: Vec<Unevaluated> = names
            .into_iter()
            .filter(|e| e.id.0 >= item)
            .map(|expr| Unevaluated {
                expr,
                untyped: None,
                non_integer: None,
                folded: false,
                diagnostic: None,
                with: 0,
                vla: None,
                params: Vec::new(),
                constant: false,
            })
            .collect();
        let pos = us.first()?.expr.pos;
        Some(Stmt {
            kind: StmtKind::Unevaluated(us),
            pos,
        })
    }

    fn unevaluated_stmt(&self, pos: Pos) -> Option<Stmt> {
        let us = self.unevaluated.take();
        (!us.is_empty()).then_some(Stmt {
            kind: StmtKind::Unevaluated(us),
            pos,
        })
    }

    /// The type of `sizeof`'s operand (or of an operand `?:` does not
    /// choose), as far as the parser can tell before sema, as sema types it:
    /// variables by their declared types, literals, casts and compound
    /// literals by theirs, a call by what its function returns, and the
    /// operators from their operands' (C's usual arithmetic conversions;
    /// `int` for a comparison, `!`, `&&` and `||`). `None` for what it cannot
    /// tell, which leaves the `sizeof` for consteval to refuse.
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
            ExprKind::SizeofType(_) | ExprKind::SizeofExpr(..) => Some(Type::Int(IntKind::size_t())),
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
                    (true, true) if *op == BinOp::Sub => Some(Type::Int(IntKind::ptrdiff_t())),
                    (true, false) => Some(lt),
                    (false, true) if *op == BinOp::Add => Some(rt),
                    (false, false) => Some(Type::Int(common(lt.int_kind()?, rt.int_kind()?))),
                    _ => None,
                }
            }
            ExprKind::Binary(
                BinOp::Mul | BinOp::Div | BinOp::Rem | BinOp::BitAnd | BinOp::BitOr | BinOp::BitXor,
                l,
                r,
            ) => {
                let (lk, rk) = (self.value_type(l)?.int_kind()?, self.value_type(r)?.int_kind()?);
                Some(Type::Int(common(lk, rk)))
            }
            // A shift is of its promoted left operand's type.
            ExprKind::Binary(BinOp::Shl | BinOp::Shr, l, _) => {
                Some(Type::Int(self.value_type(l)?.int_kind()?.promoted()))
            }
            ExprKind::Binary(_, _, _) | ExprKind::Unary(UnOp::Not, _) => Some(Type::INT),
            ExprKind::Unary(_, x) => Some(Type::Int(self.value_type(x)?.int_kind()?.promoted())),
            ExprKind::Cond(_, a, b) => {
                let (at, bt) = (self.value_type(a)?, self.value_type(b)?);
                match (at.int_kind(), bt.int_kind()) {
                    (Some(ak), Some(bk)) => Some(Type::Int(common(ak, bk))),
                    _ => (at == bt).then_some(at),
                }
            }
            ExprKind::Comma(_, r) => self.value_type(r),
            ExprKind::Assign { target, .. } | ExprKind::IncDec { target, .. } => {
                Some(self.value_type(target)?.unqual().clone())
            }
            ExprKind::Call { callee, .. } => match self.value_type(callee)?.pointee()? {
                Type::Func(f) => Some(f.ret.unqual().clone()),
                _ => None,
            },
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
        self.type_name_at(self.peek_at(1))
    }

    /// Whether the current token starts a type name.
    fn type_name_here(&self) -> bool {
        self.type_name_at(self.peek())
    }

    fn type_name_at(&self, t: &Tok) -> bool {
        match t {
            // Not a storage class or `inline`: `(static int)1` is no cast.
            Tok::Kw(Kw::Static | Kw::Extern | Kw::Typedef | Kw::Register | Kw::Auto | Kw::Inline) => false,
            Tok::Kw(k) => k.starts_decl(),
            t => self.is_typedef(t),
        }
    }

    /// clang's error for what the current token is not: `expected
    /// expression`, `expected ')'`, at the token.
    fn expected(&self, what: &str) -> Error {
        Error::new(self.here(), format!("expected {what}"))
    }

    /// A warning, given as the parser goes on.
    fn warn(&self, w: Error) {
        self.warnings.borrow_mut().push(w);
    }

    /// The punctuator `p`, or clang's error: at the token there instead for
    /// an opening bracket, else where it is missing, right after the
    /// previous token (clang's `ExpectAndConsume`).
    fn expect_punct(&mut self, p: &str) -> Result<Pos, Error> {
        if self.is_punct(p) {
            return Ok(self.advance().pos);
        }
        let at = match p {
            "(" | "[" | "{" => self.here(),
            _ => self.after_prev(),
        };
        Err(Error::new(at, format!("expected '{p}'")))
    }

    /// The bracket that closes the one at `open` (`)`, `]` or `}`), or
    /// clang's error at the token that is there instead, with its note.
    fn expect_close(&mut self, close: &str, open: Pos) -> Result<Pos, Error> {
        if self.is_punct(close) {
            return Ok(self.advance().pos);
        }
        let opener = match close {
            ")" => "(",
            "]" => "[",
            _ => "{",
        };
        Err(self
            .expected(&format!("'{close}'"))
            .note(open, format!("to match this '{opener}'")))
    }

    /// `;` is reported right after the previous token, like clang does, since that's
    /// where it's missing.
    /// The `;` that ends a statement; but where its expression read a name
    /// nothing declares (since `named` of [`Self::undeclared`]), which
    /// makes it no expression to clang, what is left of it up to its `;` is
    /// skipped, as clang skips it, with no error but sema's of the name
    /// (`retrun 1;`).
    fn semi_or_skip(&mut self, after: &str, named: usize) -> Result<(), Error> {
        if self.is_punct(";") || self.undeclared.borrow().len() == named {
            return self.expect_semi(after);
        }
        while !matches!(self.peek(), Tok::Punct(";" | "}") | Tok::Eof) {
            self.advance();
        }
        self.eat_punct(";");
        Ok(())
    }

    /// A block's statement that starts with two names, the first declared
    /// nowhere (`myit x = 1;`): clang's typo correction of it among the
    /// names in scope, typedefs and the keywords a statement can start with.
    /// A typedef is clang's `unknown type name ... did you mean`, with where
    /// it is declared; a keyword `use of undeclared identifier ... did you
    /// mean`; none, `unknown type name`.
    fn unknown_statement_name(&self, name: &str) -> Error {
        const KEYWORDS: [&str; 34] = [
            "char", "const", "double", "enum", "float", "int", "long", "short", "signed", "struct", "union",
            "unsigned", "void", "volatile", "_Bool", "restrict", "auto", "register", "static", "extern",
            "typedef", "inline", "if", "else", "for", "while", "do", "switch", "case", "default", "break",
            "continue", "goto", "return",
        ];
        let at = self.here();
        let mut seen = std::collections::HashSet::new();
        let names = self
            .scopes
            .iter()
            .rev()
            .flat_map(|s| &s.names)
            .filter_map(|(n, (what, decl))| {
                seen.insert(n.as_str())
                    .then_some((n.as_str(), Some((*decl, matches!(what, Name::Typedef(..))))))
            });
        let keywords = KEYWORDS.iter().chain(&["sizeof"]).map(|k| (*k, None));
        match correction(name, names.chain(keywords)) {
            Some((to, Some((decl, true)))) => {
                Error::new(at, format!("unknown type name '{name}'; did you mean '{to}'?"))
                    .note(decl, format!("'{to}' declared here"))
            }
            Some((to, None)) => Error::new(
                at,
                format!("use of undeclared identifier '{name}'; did you mean '{to}'?"),
            ),
            _ => Error::new(at, format!("unknown type name '{name}'")),
        }
    }

    fn expect_semi(&mut self, after: &str) -> Result<(), Error> {
        if self.eat_punct(";") {
            Ok(())
        } else {
            Err(Error::new(self.after_prev(), format!("expected ';' {after}")))
        }
    }

    fn expect_ident(&mut self) -> Result<(String, Pos), Error> {
        match self.peek().clone() {
            Tok::Ident(name) => Ok((name, self.advance().pos)),
            _ => Err(self.expected("identifier")),
        }
    }

    // ---- declaration specifiers ----

    /// Type specifiers, qualifiers, storage class and `inline`, in any order
    /// (`unsigned static int` is as good as `static unsigned int`, as in C).
    /// They are read as clang's `DeclSpec` reads them: a width (`short`,
    /// `long`, `long long`), a sign, and one other type specifier (a keyword,
    /// a struct, union or enum, or a typedef name), each checked as it comes
    /// against what came before (`cannot combine with previous 'short'
    /// declaration specifier`; the same one twice, `short short`, is only
    /// warned of, as a qualifier, storage class or `inline` twice is), then
    /// the three against each other (`'short char' is invalid`, `'void'
    /// cannot be signed or unsigned`). With none of them, the type is C89's
    /// implicit `int`, an error in C99.
    fn specifiers(&mut self, ctx: Ctx) -> Result<Specs, Error> {
        let start = self.here();
        let mut class: Option<(Class, Kw, Pos)> = None;
        let mut inline = None;
        let mut quals = Quals::NONE;
        let mut restrict = false;
        let mut width: Option<(Width, Pos)> = None;
        // `unsigned` (true) or `signed`, and where.
        let mut sign: Option<(bool, Pos)> = None;
        let mut base: Option<Base> = None;
        let mut spelled = Spelled::default();
        let mut declares = false;
        let mut anonymous = false;
        let combine = |pos: Pos, prev: &str| {
            Error::new(
                pos,
                format!("cannot combine with previous '{prev}' declaration specifier"),
            )
        };
        loop {
            let pos = self.here();
            let k = match self.peek().clone() {
                Tok::Kw(k) if k.starts_decl() => k,
                // A typedef name is a type specifier only where no other is
                // yet: in `int T;` and `unsigned T;` the `T` is the
                // declarator, even if it names a type.
                Tok::Ident(name)
                    if base.is_none()
                        && width.is_none()
                        && sign.is_none()
                        && self.typedef(&name).is_some() =>
                {
                    let ty = self.typedef(&name).cloned().expect("a typedef name");
                    base = Some(Base::Named(ty, "type-name"));
                    spelled = self.typedef_spelling(&name);
                    self.advance();
                    continue;
                }
                _ => break,
            };
            self.advance();
            // What is there twice, which clang only warns of.
            let duplicate = |p: &Self| {
                p.warn(Error::warning(
                    pos,
                    format!("duplicate '{}' declaration specifier", k.as_str()),
                    "duplicate-decl-specifier",
                ))
            };
            match k {
                Kw::Static | Kw::Extern | Kw::Typedef | Kw::Register | Kw::Auto => {
                    match ctx {
                        Ctx::Member | Ctx::TypeName => {
                            let msg = "type name does not allow storage class to be specified";
                            return Err(Error::new(pos, msg));
                        }
                        // A parameter may be `register`, and nothing else.
                        Ctx::Param if k != Kw::Register => {
                            let msg = "invalid storage class specifier in function declarator";
                            return Err(Error::new(pos, msg));
                        }
                        Ctx::Param | Ctx::File | Ctx::Block | Ctx::ForInit => {}
                    }
                    let c = match k {
                        Kw::Static => Class::Static,
                        Kw::Extern => Class::Extern,
                        Kw::Register => Class::Register,
                        Kw::Auto => Class::Auto,
                        _ => Class::Typedef,
                    };
                    match class {
                        None => class = Some((c, k, pos)),
                        Some((_, prev, _)) if prev == k => duplicate(self),
                        Some((_, prev, _)) => return Err(combine(pos, prev.as_str())),
                    }
                }
                Kw::Inline => {
                    match ctx {
                        Ctx::Member | Ctx::TypeName => {
                            let msg = "type name does not allow function specifier to be specified";
                            return Err(Error::new(pos, msg));
                        }
                        Ctx::Param => return Err(Error::new(pos, "'inline' can only appear on functions")),
                        Ctx::File | Ctx::Block | Ctx::ForInit => {}
                    }
                    match inline {
                        None => inline = Some(pos),
                        Some(_) => duplicate(self),
                    }
                }
                Kw::Const | Kw::Volatile | Kw::Restrict => {
                    let had = match k {
                        Kw::Const => std::mem::replace(&mut quals.konst, true),
                        Kw::Volatile => std::mem::replace(&mut quals.volatile, true),
                        // `restrict` promises no aliasing, which merlin never relies on.
                        _ => std::mem::replace(&mut restrict, true),
                    };
                    if had {
                        duplicate(self);
                    }
                }
                Kw::Short | Kw::Long => {
                    width = match (width, k) {
                        (None, Kw::Short) => Some((Width::Short, pos)),
                        (None, _) => Some((Width::Long, pos)),
                        (Some((Width::Short, at)), Kw::Short) => {
                            duplicate(self);
                            Some((Width::Short, at))
                        }
                        // `long` may come twice: `long long`, which is where
                        // the first one is.
                        (Some((Width::Long, at)), Kw::Long) => Some((Width::LongLong, at)),
                        (Some((w, _)), _) => return Err(combine(pos, w.name())),
                    };
                }
                Kw::Signed | Kw::Unsigned => {
                    let unsigned = k == Kw::Unsigned;
                    match sign {
                        None => sign = Some((unsigned, pos)),
                        Some((u, _)) if u == unsigned => duplicate(self),
                        Some((u, _)) => return Err(combine(pos, if u { "unsigned" } else { "signed" })),
                    }
                }
                _ => {
                    if let Some(b) = &base {
                        return Err(combine(pos, b.name()));
                    }
                    base = Some(match k {
                        Kw::Void => Base::Void,
                        Kw::Char => Base::Char,
                        Kw::Int => Base::Int,
                        Kw::Bool => Base::Bool,
                        Kw::Struct | Kw::Union | Kw::Enum => {
                            let (ty, d, s, anon) = self.nested(|p| match k {
                                Kw::Enum => p.enum_spec(pos).map(|(t, d, s)| (t, d, s, false)),
                                _ => p
                                    .record_spec(k == Kw::Union, pos)
                                    .map(|(t, d, anon)| (t, d, Spelled::default(), anon)),
                            })?;
                            declares = d;
                            spelled = s;
                            anonymous = anon;
                            Base::Named(ty, k.as_str())
                        }
                        _ => unreachable!("starts_decl"),
                    });
                }
            }
        }
        // (Specifiers that end the declaration, `static;`, have none to
        // give a type: clang only warns that they declare nothing.)
        if base.is_none() && width.is_none() && sign.is_none() && !self.is_punct(";") {
            let msg =
                "type specifier missing, defaults to 'int'; ISO C99 and later do not support implicit int";
            return Err(Error::new(self.here(), msg).flagged("implicit-int"));
        }
        // Only `char` and `int` have a sign, and only `int` a width (when
        // there is no other, the type is `int`: `unsigned`, `long long`).
        if let (Some((_, at)), Some(b)) = (sign, &base) {
            if !matches!(b, Base::Char | Base::Int) {
                return Err(Error::new(
                    at,
                    format!("'{}' cannot be signed or unsigned", b.name()),
                ));
            }
        }
        if let (Some((w, at)), Some(b)) = (width, &base) {
            if !matches!(b, Base::Int) {
                return Err(Error::new(at, format!("'{} {}' is invalid", w.name(), b.name())));
            }
        }
        let unsigned = matches!(sign, Some((true, _)));
        let ty = match base {
            Some(Base::Void) => Type::Void,
            Some(Base::Bool) => Type::BOOL,
            Some(Base::Named(ty, _)) => ty,
            Some(Base::Char) => Type::Int(match sign {
                Some((true, _)) => IntKind::UChar,
                Some((false, _)) => IntKind::SChar,
                None => IntKind::Char,
            }),
            Some(Base::Int) | None => Type::Int(match (width.map(|(w, _)| w), unsigned) {
                (Some(Width::Short), false) => IntKind::Short,
                (Some(Width::Short), true) => IntKind::UShort,
                (Some(Width::Long), false) => IntKind::Long,
                (Some(Width::Long), true) => IntKind::ULong,
                (Some(Width::LongLong), false) => IntKind::LongLong,
                (Some(Width::LongLong), true) => IntKind::ULongLong,
                (None, false) => IntKind::Int,
                (None, true) => IntKind::UInt,
            }),
        };
        Ok(Specs {
            ty: ty.qualified(quals),
            class: class.map_or(Class::None, |(c, ..)| c),
            class_pos: class.map_or(start, |(.., at)| at),
            inline,
            declares,
            anonymous,
            spelled,
            pos: start,
        })
    }

    /// After `struct` or `union`: a definition (`struct tag { ... }`, the tag
    /// optional) or a reference to a tag. Returns the type, whether the
    /// specifier declares a tag by itself, and whether it defines a struct
    /// or union without one.
    fn record_spec(&mut self, union: bool, kw_pos: Pos) -> Result<(Type, bool, bool), Error> {
        let kw = if union { "union" } else { "struct" };
        let tag = match self.peek().clone() {
            Tok::Ident(name) => Some((name, self.advance().pos)),
            _ => None,
        };
        // A tag of the other kind, first used at `prev`: clang's error is at
        // the keyword.
        let mismatch = |name: &str, prev: Pos| {
            Error::new(
                kw_pos,
                format!("use of '{name}' with tag type that does not match previous declaration"),
            )
            .note(prev, "previous use is here")
        };
        if self.is_punct("{") {
            let record = match &tag {
                Some((name, pos)) => match self.scopes.last().unwrap().tags.get(name) {
                    // `struct s;` earlier in this scope: this completes it.
                    Some(Tag::Record(r)) if r.union == union && r.body().is_none() => r.clone(),
                    Some(Tag::Record(r)) if r.union == union => {
                        let prev = r.body().map_or(r.pos, |b| b.pos);
                        return Err(Error::new(*pos, format!("redefinition of '{name}'"))
                            .note(prev, "previous definition is here"));
                    }
                    Some(t) => return Err(mismatch(name, t.pos())),
                    None => {
                        // Declared before the body, so members can point to it.
                        let r = Record::new(union, Some(name.clone()), *pos);
                        self.bind_tag(name, Tag::Record(r.clone()));
                        r
                    }
                },
                None => Record::new(union, None, kw_pos),
            };
            let at = tag.as_ref().map_or(kw_pos, |(_, pos)| *pos);
            self.record_body(&record, at)?;
            self.after_definition(kw)?;
            // `struct { int x; };` declares nothing anyone can name.
            return Ok((Type::Record(record), tag.is_some(), tag.is_none()));
        }
        let Some((name, pos)) = tag else {
            return Err(Error::new(
                kw_pos,
                format!("declaration of anonymous {kw} must be a definition"),
            ));
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
            Some(Tag::Record(r)) if r.union == union => Ok((Type::Record(r.clone()), alone, false)),
            Some(t) => Err(mismatch(&name, t.pos())),
            None => {
                // A tag not seen before: an incomplete type, completed by a
                // definition later in the same scope.
                let r = Record::new(union, Some(name.clone()), pos);
                self.bind_tag(&name, Tag::Record(r.clone()));
                Ok((Type::Record(r), true, false))
            }
        }
    }

    /// `{ member-decl+ }`: the members, laid out into `record`, whose
    /// definition is at `at`.
    fn record_body(&mut self, record: &Record, at: Pos) -> Result<(), Error> {
        let open = self.expect_punct("{")?;
        let mut members: Vec<(String, Type, Spelled, Pos, Pos)> = Vec::new();
        let mut seen: HashMap<String, Pos> = HashMap::new();
        // The layout so far, to reject a struct too big before laying it out.
        let mut end = 0u64;
        while !self.eat_punct("}") {
            if !self.at_decl() {
                if *self.peek() == Tok::Eof {
                    return Err(self.expect_close("}", open).unwrap_err());
                }
                return Err(self.expected_specifier());
            }
            let specs = self.specifiers(Ctx::Member)?;
            if self.is_punct(";") {
                // C11's anonymous members, which clang takes in C99 too:
                // merlin's language has none.
                if specs.anonymous {
                    let kw = specs.ty.record().map_or("struct", |r| r.keyword());
                    let msg = format!("anonymous {kw}s are not supported");
                    return Err(Error::new(specs.pos, msg));
                }
                self.advance();
                self.nothing_declared(&specs);
                continue;
            }
            loop {
                let d = self.declarator(&specs.ty, Ctx::Member)?;
                self.folded(&d, Ctx::Member, Class::None, false);
                if self.is_punct(":") {
                    return Err(Error::new(self.here(), "bit-fields are not supported yet"));
                }
                if d.ty.is_func() {
                    return Err(Error::new(
                        d.pos,
                        format!("field '{}' declared as a function", d.name),
                    ));
                }
                let Some(size) = d.ty.size() else {
                    if let Type::Array(_, None) = &d.ty {
                        return Err(Error::new(d.pos, "flexible array members are not supported yet"));
                    }
                    let e = Error::new(d.pos, format!("field has incomplete type '{}'", d.ty));
                    return Err(match d.ty.unqual() {
                        Type::Record(r) => {
                            e.note(r.pos, format!("forward declaration of '{}'", d.ty.unqual()))
                        }
                        _ => e,
                    });
                };
                if let Some(prev) = seen.insert(d.name.clone(), d.pos) {
                    return Err(Error::new(d.pos, format!("duplicate member '{}'", d.name))
                        .note(prev, "previous declaration is here"));
                }
                end = if record.union {
                    end.max(u64::from(size))
                } else {
                    align_up(end, d.ty.align()) + u64::from(size)
                };
                if end > MAX_OBJECT_SIZE {
                    return Err(Error::new(d.pos, format!("{} is too large", record.keyword())));
                }
                members.push((d.name, d.ty, specs.spelled.clone(), d.pos, specs.pos));
                if !self.eat_punct(",") {
                    break;
                }
            }
            // The last member's `;` left out: clang only warns.
            if self.is_punct("}") {
                let msg = "expected ';' at end of declaration list";
                self.warn(Error::warning_unflagged(self.after_prev(), msg));
            } else {
                self.expect_semi("at end of declaration list")?;
            }
        }
        if members.is_empty() {
            return Err(Error::new(open, format!("{} has no members", record.keyword())));
        }
        if align_up(
            end,
            members.iter().map(|(_, t, _, _, _)| t.align()).max().unwrap_or(1),
        ) > MAX_OBJECT_SIZE
        {
            return Err(Error::new(open, format!("{} is too large", record.keyword())));
        }
        record.complete_spelled(members, at);
        Ok(())
    }

    /// clang's error where a declaration's specifiers should be and are
    /// not: in a struct (`struct s { 3; }`).
    fn expected_specifier(&self) -> Error {
        Error::new(self.here(), "type name requires a specifier or qualifier")
    }

    /// clang's warning for specifiers that end their declaration and declare
    /// nothing (`int;`), at their start.
    fn nothing_declared(&self, specs: &Specs) {
        self.warn(Error::warning(
            specs.pos,
            "declaration does not declare anything",
            "missing-declarations",
        ));
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
        let mismatch = |name: &str, prev: Pos| {
            Error::new(
                kw_pos,
                format!("use of '{name}' with tag type that does not match previous declaration"),
            )
            .note(prev, "previous use is here")
        };
        if !self.eat_punct("{") {
            let Some((name, pos)) = tag else {
                return Err(self.expected("identifier or '{'"));
            };
            return match self.lookup_tag(&name) {
                Some(Tag::Enum(ty, s, _)) => Ok((ty.clone(), false, s.clone())),
                Some(t) => Err(mismatch(&name, t.pos())),
                None => Err(Error::new(
                    pos,
                    format!(
                        "use of undefined 'enum {name}' (merlin needs an enum's constants before its name)"
                    ),
                )),
            };
        }
        if let Some((name, pos)) = &tag {
            match self.scopes.last().unwrap().tags.get(name) {
                Some(Tag::Enum(.., prev)) => {
                    return Err(Error::new(*pos, format!("redefinition of '{name}'"))
                        .note(*prev, "previous definition is here"));
                }
                Some(t) => return Err(mismatch(name, t.pos())),
                None => {}
            }
        }
        let (int_min, int_max) = IntKind::Int.range();
        let mut next = 0i128;
        let mut negative = false;
        let mut constants = Vec::new();
        loop {
            if self.is_punct("}") {
                if constants.is_empty() {
                    return Err(Error::new(self.here(), "use of empty enum"));
                }
                self.advance();
                break;
            }
            let (name, pos) = self.expect_ident()?;
            let valued = self.is_punct("=");
            let value = if self.eat_punct("=") {
                let at = self.here();
                let mut e = self.cond()?;
                let msg = "expression is not an integer constant expression";
                let typed = Some("integer constant expression must have integer type, not");
                match self.const_int(&mut e, at, msg, typed)? {
                    (v, true) => v,
                    // Sema reports the error.
                    (_, false) => 0,
                }
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
            if self.eat_punct(",") || self.is_punct("}") {
                continue;
            }
            // Another enumerator: clang takes the comma to be missing.
            if matches!(self.peek(), Tok::Ident(_)) {
                return Err(Error::new(self.after_prev(), "missing ',' between enumerators"));
            }
            return Err(if valued {
                self.expected("'}' or ','")
            } else {
                self.expected("'= constant-expression' or end of enumerator definition")
            });
        }
        let ty = if negative { Type::INT } else { Type::UINT };
        let names: Vec<String> = constants.iter().map(|(n, _)| n.clone()).collect();
        let spelled = Spelled(Some(Spelling::Enum(Arc::new(EnumType {
            tag: tag.as_ref().map(|(n, _)| n.clone()),
            ty: ty.clone(),
            constants,
            pos: kw_pos,
            typedef_name: OnceLock::new(),
        }))));
        // The constants now name their enum, for the code after it.
        let scope = self.scopes.last_mut().expect("a scope");
        for name in &names {
            if let Some((Name::EnumConst(_, of), _)) = scope.names.get_mut(name) {
                *of = spelled.clone();
            }
        }
        if let Some((name, pos)) = tag {
            self.bind_tag(&name, Tag::Enum(ty.clone(), spelled.clone(), pos));
        }
        self.after_definition("enum")?;
        Ok((ty, true, spelled))
    }

    /// After a struct, union or enum's `}` (the `kw` it is), what can
    /// follow a type specifier, as clang's `isValidAfterTypeSpecifier` has
    /// it, else clang's error that the `;` is missing after it: a type
    /// specifier (`struct s { ... } struct t;`) or a keyword that begins
    /// none of a declarator. A qualifier or storage class followed by a
    /// type specifier is taken for the next declaration's, too.
    fn after_definition(&self, kw: &str) -> Result<(), Error> {
        let type_specifier = |t: &Tok| match t {
            Tok::Kw(k) => matches!(
                k,
                Kw::Void
                    | Kw::Char
                    | Kw::Short
                    | Kw::Int
                    | Kw::Long
                    | Kw::Signed
                    | Kw::Unsigned
                    | Kw::Bool
                    | Kw::Struct
                    | Kw::Union
                    | Kw::Enum
            ),
            Tok::Ident(name) => matches!(self.lookup_name(name), Some(Name::Typedef(..))),
            _ => false,
        };
        let valid = match self.peek() {
            Tok::Punct(";" | "*" | ")" | "(" | "," | "[" | "..." | ":" | "}") | Tok::Ident(_) => true,
            Tok::Kw(
                Kw::Const
                | Kw::Volatile
                | Kw::Restrict
                | Kw::Inline
                | Kw::Static
                | Kw::Extern
                | Kw::Typedef
                | Kw::Register
                | Kw::Auto,
            ) => !type_specifier(self.peek_at(1)),
            _ => false,
        };
        if valid {
            Ok(())
        } else {
            Err(Error::new(self.after_prev(), format!("expected ';' after {kw}")))
        }
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
            let open = self.advance().pos;
            let (name, pos, inner) = self.nested(|p| p.declarator_ops(ctx))?;
            self.expect_close(")", open)?;
            (name, pos, inner)
        } else if let Tok::Ident(name) = self.peek().clone() {
            if ctx == Ctx::TypeName {
                let open = self.type_open.get();
                return Err(self.expected("')'").note(open, "to match this '('"));
            }
            self.advance();
            (name, pos, Vec::new())
        } else if matches!(ctx, Ctx::Param | Ctx::TypeName) {
            (String::new(), pos, Vec::new())
        } else if ctx == Ctx::Member {
            return Err(self.expected("member name or ';' after declaration specifiers"));
        } else {
            return Err(self.expected("identifier or '('"));
        };
        let suffixes = self.suffixes()?;
        let mut ops = ptrs;
        ops.extend(suffixes.into_iter().rev());
        ops.extend(inner);
        Ok((name, pos, ops))
    }

    /// `('[' constant? ']' | '(' params ')')*` after a declarator's name, in
    /// source order.
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
                let mut is_static = None;
                loop {
                    if *self.peek() == Tok::Kw(Kw::Static) {
                        is_static = Some(self.advance().pos);
                        quals.get_or_insert(Quals::NONE);
                    } else if matches!(self.peek(), Tok::Kw(Kw::Const | Kw::Volatile | Kw::Restrict)) {
                        let q = self.qualifiers();
                        quals = Some(quals.unwrap_or(Quals::NONE).union(q));
                    } else {
                        break;
                    }
                }
                if let (Some(at), true) = (is_static, self.is_punct("]")) {
                    return Err(Error::new(at, "'static' may not be used without an array size"));
                }
                let quals = quals.map(|q| (q, is_static.is_some()));
                let size = self.here();
                if self.eat_punct("]") {
                    out.push(Derive::Array(Array {
                        len: None,
                        open,
                        size,
                        quals,
                        folded: None,
                        entries: 0..0,
                        constant: true,
                    }));
                    dims += 1;
                    continue;
                }
                let first = self.unevaluated.borrow().len();
                let mut e = self.cond()?;
                self.expect_close("]", open)?;
                let outer = self.folding.replace(false);
                let (n, sure) = self.const_int(
                    &mut e,
                    size,
                    NOT_CONSTANT_SIZE,
                    Some("size of array has non-integer type"),
                )?;
                let mut folded = self.folding.replace(outer).then_some(size);
                let entries = first..self.unevaluated.borrow().len();
                // A size clang's evaluator notes as no constant expression
                // (`1 << 40 ? 1 : 2`, `INT_MAX + 1 > 0 ? 1 : 2`) clang folds
                // wherever it is, with its warning where the size begins,
                // given unless the size has an error.
                if let Some((at, note)) = consteval::parsed_note(&e).filter(|_| sure) {
                    // Not a leaf (one never is noted): the constant is kept.
                    folded = None;
                    let mut us = self.unevaluated.borrow_mut();
                    let with = entries.len() - 1;
                    let u = us.last_mut().expect("the constant itself");
                    let (msg, flag) = FOLDED;
                    u.diagnostic = Some(Error::warning(size, msg, flag).note(at, note));
                    u.with = with;
                }
                // Not sure: sema reports the error, and the size is not used.
                out.push(Derive::Array(Array {
                    len: Some(if sure { n } else { 1 }),
                    open,
                    size,
                    quals,
                    folded,
                    entries,
                    constant: sure,
                }));
                dims += 1;
            } else if self.is_punct("(") {
                let open = self.advance().pos;
                let prototyped = !self.is_punct(")");
                let (params, variadic, scope) = self.params(open)?;
                out.push(Derive::Func(params, variadic, prototyped, scope, open));
                dims = 0;
            } else {
                return Ok(out);
            }
        }
    }

    /// A declarator, applied to `base`. In a parameter list or a type name
    /// the name may be missing.
    fn declarator(&mut self, base: &Type, ctx: Ctx) -> Result<Decl, Error> {
        let start = self.unevaluated.borrow().len();
        let (name, pos, ops) = self.declarator_ops(ctx)?;
        let folded_reach = reach(&ops, |a| a.folded.is_some());
        let variable = reach(&ops, |a| !a.constant);
        let mut ty = base.clone();
        let mut func = None;
        let mut prototyped = false;
        let mut func_open = None;
        let mut folded = None;
        let mut folded_sizes = (None, Vec::new());
        // The sizes that are not constant, which sema reports.
        let mut sizes = Vec::new();
        // An array larger than merlin lays out, though clang takes it.
        let mut too_large = None;
        // The size of `ty`, kept as it grows: `Type::size` recurses, so asking
        // it at every dimension would be quadratic in the number of dimensions.
        let mut size = ty.size().map(u64::from);
        let steps = ops.len();
        for (i, op) in ops.into_iter().enumerate() {
            match op {
                Derive::Ptr(q) => {
                    ty = Type::ptr(ty).qualified(q);
                    size = Some(u64::from(PTR_SIZE));
                }
                Derive::Array(a) => {
                    folded = folded.or(a.folded);
                    // The last step is what is declared: its own array.
                    if a.folded.is_some() {
                        let entries = a.entries.start - start..a.entries.end - start;
                        if i + 1 == steps {
                            folded_sizes.0 = Some(entries);
                        } else {
                            folded_sizes.1.push(entries);
                        }
                    }
                    if !a.constant {
                        sizes.push(a.size);
                    }
                    // Only a parameter's outermost array becomes a pointer,
                    // the one thing the qualifiers in the brackets can apply to.
                    if let Some((_, is_static)) = a.quals.filter(|_| !(ctx == Ctx::Param && i + 1 == steps)) {
                        let what = if is_static { "'static'" } else { "type qualifier" };
                        let place = if ctx == Ctx::Param {
                            "non-outermost array type derivation"
                        } else {
                            "array declarator outside of function prototype"
                        };
                        return Err(Error::new(a.open, format!("{what} used in {place}")));
                    }
                    if ty.is_func() {
                        let what = if name.is_empty() { "type name" } else { &name };
                        return Err(Error::new(
                            a.open,
                            format!("'{what}' declared as array of functions of type '{ty}'"),
                        ));
                    }
                    let Some(elem_size) = size else {
                        let e = Error::new(a.open, format!("array has incomplete element type '{ty}'"));
                        return Err(match ty.unqual() {
                            Type::Record(r) => {
                                e.note(r.pos, format!("forward declaration of '{}'", ty.unqual()))
                            }
                            _ => e,
                        });
                    };
                    match a.len {
                        Some(n) if n < 0 => {
                            let msg = if name.is_empty() {
                                "array size is negative".to_string()
                            } else {
                                format!("'{name}' declared as an array with a negative size")
                            };
                            return Err(Error::new(a.size, msg));
                        }
                        // A GNU extension clang takes: merlin's objects all
                        // have a size.
                        Some(0) => return Err(Error::new(a.size, "zero-length arrays are not supported")),
                        _ => {}
                    }
                    let n = a.len.map(|n| n as u128);
                    let total = u128::from(elem_size) * n.unwrap_or(1);
                    // clang's limit: the size in bytes has to fit in a
                    // `size_t` (of 61 bits at most).
                    let bits = (8 * PTR_SIZE).min(61);
                    if total >> bits != 0 {
                        let msg = format!("array is too large ({} elements)", grouped(n.unwrap_or(1)));
                        return Err(Error::new(a.size, msg));
                    }
                    if total > u128::from(MAX_OBJECT_SIZE) && too_large.is_none() {
                        too_large = Some(a.size);
                    }
                    size = n.map(|_| total as u64);
                    ty = Type::array(ty, n.map(|n| u32::try_from(n).unwrap_or(u32::MAX)));
                    if let Some((q, _)) = a.quals {
                        // The parameter adjustment, done here so the
                        // qualifiers land on the pointer: `int a[const 3]`
                        // declares `int *const a`.
                        ty = ty.decay().qualified(q);
                        size = Some(u64::from(PTR_SIZE));
                    }
                }
                Derive::Func(params, variadic, with_prototype, scope, open) => {
                    if ty.is_array() || ty.is_func() {
                        let what = if ty.is_array() { "array" } else { "function" };
                        return Err(Error::new(
                            open,
                            format!("function cannot return {what} type '{ty}'"),
                        ));
                    }
                    // A qualified result is part of the type (`const int
                    // f(void)` and `int f(void)` conflict), though a call's
                    // value is unqualified (sema).
                    let ret = ty.clone();
                    let types = params.iter().map(|p| p.ty.unqual().clone()).collect();
                    ty = if variadic {
                        Type::variadic_func(ret, types)
                    } else {
                        Type::func(ret, types)
                    };
                    if let Type::Func(f) = &mut ty {
                        f.spelled =
                            Spellings(params.iter().map(|p| (p.ty.clone(), p.spelled.clone())).collect());
                        f.unprototyped = Unprototyped(!with_prototype);
                    }
                    size = None;
                    if i + 1 == steps {
                        func = Some((params, scope));
                        prototyped = with_prototype;
                        func_open = Some(open);
                    }
                }
            }
        }
        if let Some(at) = too_large {
            let msg = format!("array is too large: merlin's objects are at most {MAX_OBJECT_SIZE} bytes");
            return Err(Error::new(at, msg));
        }
        let d = Decl {
            name,
            ty,
            pos,
            func,
            prototyped,
            open: func_open,
            folded,
            reach: folded_reach,
            folded_sizes,
            unevaluated: start..self.unevaluated.borrow().len(),
        };
        self.variable_sizes(&d, ctx, &sizes, variable);
        // A parameter is never folded (see `folded`); a type name is as its
        // use says (`type_name_folded`).
        if ctx == Ctx::Param {
            self.folded(&d, ctx, Class::None, false);
        }
        Ok(d)
    }

    /// The sizes of a declarator's own arrays that are no constants (at
    /// `sizes`, through `reach`), which sema reports as merlin's error of a
    /// variable-length array, or as clang's where it can have none: at file
    /// scope and in a member, at the declarator's name.
    fn variable_sizes(&self, d: &Decl, ctx: Ctx, sizes: &[Pos], reach: Reach) {
        let msg = match (ctx, reach) {
            _ if sizes.is_empty() => return,
            (Ctx::File, Reach::Arrays) => "variable length array declaration not allowed at file scope",
            (Ctx::File, Reach::Function) => "function declaration cannot have variably modified type",
            (Ctx::File, _) => "variably modified type declaration not allowed at file scope",
            (Ctx::Member, _) => {
                "fields must have a constant size: 'variable length array in structure' extension will \
                 never be supported"
            }
            _ => return,
        };
        let mut us = self.unevaluated.borrow_mut();
        // A size's own (not an operand `?:` did not choose, which sema
        // could not type: `1 ? 2 : p` of a pointer has no integer type).
        for u in us[d.unevaluated.clone()].iter_mut().filter(|u| !u.folded) {
            if let Some(untyped) = &mut u.untyped {
                if sizes.contains(&untyped.0) && untyped.1 == NOT_CONSTANT_SIZE {
                    *untyped = (d.pos, msg.to_string());
                }
            }
        }
    }
    /// After `(` (at `open`), up to and including `)`: the parameters, in a
    /// scope of their own, and whether `...` ends them.
    fn params(&mut self, open: Pos) -> Result<(Vec<Param>, bool, Scope), Error> {
        self.scopes.push(Scope::default());
        let outer = self.prototype.borrow().len();
        let params = self.params_inner(open);
        self.prototype.borrow_mut().truncate(outer);
        let scope = self.scopes.pop().expect("pushed above");
        let (params, variadic) = params?;
        Ok((params, variadic, scope))
    }

    fn params_inner(&mut self, open: Pos) -> Result<(Vec<Param>, bool), Error> {
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
                        "ISO C requires a named parameter before '...'",
                    ));
                }
                self.advance();
                self.expect_close(")", open)?;
                return Ok((params, true));
            }
            if !self.at_decl() {
                // `int f(unsinged x)`: a type's name, misspelt or none.
                if let (Tok::Ident(name), Tok::Ident(_)) = (self.peek().clone(), self.peek_at(1)) {
                    return Err(self.unknown_type_name(&name));
                }
                if params.is_empty() && matches!(self.peek(), Tok::Ident(_)) {
                    return Err(self.identifier_list(open));
                }
                return Err(self.expected("parameter declarator"));
            }
            let specs = self.specifiers(Ctx::Param)?;
            let d = self.declarator(&specs.ty, Ctx::Param)?;
            // `(V)` with `typedef void V;` is `(void)` too: what C99 6.7.5.3p10
            // asks for is one unnamed parameter of (unqualified) type void,
            // however it is spelled.
            if params.is_empty() && d.name.is_empty() && d.ty == Type::Void && self.eat_punct(")") {
                return Ok((params, false));
            }
            // Where the name is, or would be: clang's place for a
            // parameter's diagnostics.
            let pos = d.pos;
            // Any other `void` parameter is clang's error: a named one, a
            // qualified one alone (reported at the `(`), or one among
            // others.
            if d.ty.is_void() {
                if !d.name.is_empty() {
                    return Err(Error::new(pos, "argument may not have 'void' type"));
                }
                if params.is_empty() && self.is_punct(")") {
                    return Err(Error::new(
                        open,
                        "'void' as parameter must not have type qualifiers",
                    ));
                }
                return Err(Error::new(
                    pos,
                    "'void' must be the first and only parameter if specified",
                ));
            }
            if params.len() == MAX_PARAMS {
                return Err(Error::new(
                    pos,
                    format!("too many parameters: merlin supports at most {MAX_PARAMS}"),
                ));
            }
            // C adjusts a parameter declared as an array or a function to a
            // pointer (keeping qualifiers on what it points to).
            let spelled = decayed_spelling(&d.ty, specs.spelled);
            let array = d.ty.is_array().then(|| d.ty.clone());
            let ty = if d.ty.is_array() || d.ty.is_func() {
                d.ty.decay()
            } else {
                d.ty
            };
            if !d.name.is_empty() {
                if let Some((_, prev)) = self.scopes.last().and_then(|s| s.names.get(&d.name)) {
                    return Err(Error::new(pos, format!("redefinition of parameter '{}'", d.name))
                        .note(*prev, "previous declaration is here"));
                }
                self.bind_name(&d.name, Name::Object(ty.clone()), pos)?;
                self.prototype
                    .borrow_mut()
                    .push((d.name.clone(), ty.clone(), pos));
            }
            params.push(Param {
                name: d.name,
                ty,
                pos,
                spelled,
                register: specs.class == Class::Register,
                array,
            });
            if self.eat_punct(")") {
                return Ok((params, false));
            }
            if !self.eat_punct(",") {
                return Err(self.expect_close(")", open).unwrap_err());
            }
        }
    }

    /// `int f(x, y)`: a list of parameter names, C89's, whose types are
    /// declared after it, before the body. merlin's language has no such
    /// definitions; clang's error on a name the list leaves undeclared, as
    /// it does when the body follows (on the last name first), or where a
    /// declaration has the list.
    fn identifier_list(&mut self, open: Pos) -> Error {
        let mut names = Vec::new();
        loop {
            match self.expect_ident() {
                Ok(name) => names.push(name),
                Err(e) => return e,
            }
            if self.eat_punct(")") {
                break;
            }
            if !self.eat_punct(",") {
                return self.expect_close(")", open).unwrap_err();
            }
        }
        let (name, pos) = names.last().expect("one name at least");
        if self.is_punct("{") {
            let msg = format!(
                "parameter '{name}' was not declared, defaults to 'int'; ISO C99 and later do not support \
                 implicit int"
            );
            return Error::new(*pos, msg).flagged("implicit-int");
        }
        let first = names[0].1;
        if self.at_decl() {
            return Error::new(first, "old-style parameter declarations are not supported");
        }
        Error::new(
            first,
            "a parameter list without types is only allowed in a function definition",
        )
    }

    /// A type name, for casts and `sizeof`: specifiers and a declarator
    /// without a name (`int`, `char *[4]`, `int (*)(void)`), no compound
    /// literal's.
    fn type_name(&mut self, open: Pos) -> Result<Type, Error> {
        let (d, _) = self.type_name_decl(open)?;
        self.type_name_folded(&d, None);
        Ok(d.ty)
    }

    /// A type name's declarator, and how it spelled its base (for `-g`),
    /// in parentheses opened at `open`. What it folded is for its use to
    /// say ([`Self::type_name_folded`]).
    fn type_name_decl(&mut self, open: Pos) -> Result<(Decl, Spelled), Error> {
        self.type_open.set(open);
        let specs = self.specifiers(Ctx::TypeName)?;
        let d = self.declarator(&specs.ty, Ctx::TypeName)?;
        Ok((d, specs.spelled))
    }

    // ---- declarations ----

    /// `'{' ... '}'` (empty too, as clang takes it) or an expression.
    fn initializer(&mut self) -> Result<Init, Error> {
        if !self.is_punct("{") {
            return Ok(Init::Expr(self.assign()?));
        }
        self.nested(|p| {
            let pos = p.advance().pos;
            let mut items = Vec::new();
            loop {
                if p.eat_punct("}") {
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
                    return Err(p.expect_close("}", pos).unwrap_err());
                }
            }
        })
    }

    /// `.member` and `[index]` designators, through the `=`. A lone
    /// `[index]` may go without it (`[0] 1`): GNU's older syntax, which
    /// clang takes with a warning.
    fn designation(&mut self) -> Result<Vec<Designator>, Error> {
        let mut out = Vec::new();
        loop {
            if self.is_punct(".") {
                let pos = self.advance().pos;
                let Tok::Ident(name) = self.peek().clone() else {
                    return Err(self.expected("a field designator, such as '.field = 4'"));
                };
                let at = self.advance().pos;
                out.push(Designator::Member(name, pos, at));
            } else if self.is_punct("[") {
                let pos = self.advance().pos;
                let index = self.cond()?;
                self.expect_close("]", pos)?;
                out.push(Designator::Index(index, pos));
            } else {
                break;
            }
        }
        if self.eat_punct("=") {
            return Ok(out);
        }
        if let [Designator::Index(..)] = out[..] {
            let msg = "use of GNU 'missing =' extension in designator";
            self.warn(Error::warning(self.here(), msg, "gnu-designator"));
            return Ok(out);
        }
        Err(self.expected("'=' or another designator"))
    }

    /// clang's error for a declaration's missing `;`, which says where it is.
    fn end_of_declaration(&mut self, ctx: Ctx) -> Result<(), Error> {
        match ctx {
            Ctx::File => self.expect_semi("after top level declarator"),
            Ctx::ForInit => self.for_semi(),
            _ => self.expect_semi("at end of declaration"),
        }
    }

    /// The declarators of a declaration of objects and functions, from
    /// `first` on, through the `;`; what the declaration left for sema to
    /// check from `start` on (in the waiting [`Unevaluated`]s) goes with
    /// them. At file scope a function is an item of its own, between the
    /// declarations of the objects before and after it (`int x, f(int);`);
    /// in a block it is a declarator, which sema binds to the function
    /// outside.
    fn init_declarators(
        &mut self,
        specs: &Specs,
        first: Option<Decl>,
        ctx: Ctx,
        start: usize,
    ) -> Result<Vec<Item>, Error> {
        let storage = match specs.class {
            Class::Static => Storage::Static,
            Class::Extern => Storage::Extern,
            Class::Register => Storage::Register,
            Class::None | Class::Typedef | Class::Auto => Storage::None,
        };
        let mut first = first;
        let mut items = Vec::new();
        let mut decls = Vec::new();
        loop {
            let d = match first.take() {
                Some(d) => d,
                None => self.declarator(&specs.ty, ctx)?,
            };
            if d.ty.is_func() && self.is_punct("=") {
                return Err(Error::new(
                    d.pos,
                    "illegal initializer (only variables can be initialized)",
                ));
            }
            // `register` and `auto` are for objects in a block (C99 6.9p2).
            if matches!(specs.class, Class::Register | Class::Auto) {
                if d.ty.is_func() {
                    return Err(Error::new(specs.class_pos, "illegal storage class on function"));
                }
                if ctx == Ctx::File {
                    return Err(Error::new(d.pos, "illegal storage class on file-scoped variable"));
                }
            }
            if d.ty.is_func() && ctx == Ctx::File {
                self.folded(&d, ctx, specs.class, false);
                // What the function's declarator left, checked at file scope.
                let unevaluated = self.unevaluated.borrow_mut().split_off(start);
                self.bind_name(&d.name, Name::Object(d.ty.clone()), d.pos)?;
                if !decls.is_empty() {
                    items.push(Item::Decl(std::mem::take(&mut decls)));
                }
                items.push(Item::Func(declared_function(d, specs, unevaluated)));
                if !self.eat_punct(",") {
                    break;
                }
                continue;
            }
            if d.ty.is_func() && specs.class == Class::Static {
                let msg = "function declared in block scope cannot have 'static' storage class";
                return Err(Error::new(specs.class_pos, msg));
            }
            if let (Some(at), false) = (specs.inline, d.ty.is_func()) {
                return Err(Error::new(at, "'inline' can only appear on functions"));
            }
            // A block's declaration with linkage has the composite type of
            // it and the earlier one it can see, as sema gives it (C99
            // 6.2.7p4), for the sizes the parser works out.
            let linked = ctx != Ctx::File && (specs.class == Class::Extern || d.ty.is_func());
            let ty = if linked {
                self.visible_composite(&d.name, &d.ty)
            } else {
                d.ty.clone()
            };
            self.bind_name(&d.name, Name::Object(ty), d.pos)?;
            if linked {
                let scope = self.scopes.last_mut().expect("a scope");
                scope.linked.insert(d.name.clone());
            }
            let init = if self.eat_punct("=") {
                Some(self.initializer()?)
            } else {
                None
            };
            self.folded(&d, ctx, specs.class, init.is_some());
            // What its declarator left, and what its initializer did.
            let (sized, unevaluated) = {
                let mut us = self.unevaluated.borrow_mut();
                let unevaluated = us.split_off(d.unevaluated.end);
                (us.split_off(start), unevaluated)
            };
            // `int a[] = {1, 2, 3};`: the size the initializer gives, for a
            // later `sizeof a` in an array size. (The declaration keeps its
            // incomplete type: sema completes it, the same way.) Unless an
            // earlier declaration gave the size: `int a[3]; int a[] = {1};`.
            let bound = self.scopes.last().and_then(|s| s.names.get(&d.name));
            let incomplete = matches!(bound, Some((Name::Object(t), _)) if t.size().is_none());
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
                begin: specs.pos,
                spelled: specs.spelled.clone(),
                sized,
                unevaluated,
            });
            if !self.eat_punct(",") {
                break;
            }
        }
        self.end_of_declaration(ctx)?;
        if !decls.is_empty() {
            items.push(Item::Decl(decls));
        }
        Ok(items)
    }

    /// The declarators of a `typedef`, each binding a name to its type.
    fn typedefs(&mut self, specs: &Specs, first: Decl, ctx: Ctx) -> Result<(), Error> {
        if let Some(at) = specs.inline {
            return Err(Error::new(at, "'inline' can only appear on functions"));
        }
        let mut d = first;
        loop {
            if self.is_punct("=") {
                return Err(Error::new(
                    d.pos,
                    "illegal initializer (only variables can be initialized)",
                ));
            }
            self.folded(&d, ctx, Class::Typedef, false);
            let spelling = Spelled(Some(Spelling::Typedef(Arc::new(TypedefName {
                name: d.name.clone(),
                ty: d.ty.clone(),
                spelled: specs.spelled.clone(),
                pos: d.pos,
            }))));
            // A typedef of a struct, union or enum without a tag names it
            // (the first typedef that is the type itself, unqualified).
            match (&d.ty, &specs.spelled.0) {
                (Type::Record(r), _) if r.tag.is_none() => {
                    let _ = r.typedef_name.set(d.name.clone());
                }
                (t, Some(Spelling::Enum(e))) if e.tag.is_none() && *t == e.ty => {
                    let _ = e.typedef_name.set(d.name.clone());
                }
                _ => {}
            }
            self.bind_name(&d.name, Name::Typedef(d.ty, spelling), d.pos)?;
            if !self.eat_punct(",") {
                break;
            }
            d = self.declarator(&specs.ty, ctx)?;
        }
        self.end_of_declaration(ctx)
    }

    /// After specifiers that stand alone, at the `;`: they should declare
    /// something (`struct s { ... };`), or clang warns.
    fn bare_specifiers(&mut self, specs: &Specs) {
        self.advance(); // ;
        if !specs.declares {
            self.nothing_declared(specs);
        }
    }

    /// clang's error where a declaration should start at file scope and
    /// none does.
    fn not_a_declaration(&mut self) -> Error {
        match self.peek() {
            Tok::Punct("}") => Error::new(self.here(), "extraneous closing brace ('}')"),
            // What would start an Objective-C method, which clang tells
            // apart at file scope.
            Tok::Punct("+" | "-") => Error::new(self.here(), "expected external declaration"),
            // C89's implicit `int` (`f() {}`, `x;`), where what follows the
            // name can follow a declarator's name, as clang's
            // `isValidAfterIdentifierInDeclarator` has it; otherwise the
            // name was meant as a type (`x y;`, `x` at the end).
            Tok::Ident(_)
                if matches!(
                    self.peek_at(1),
                    Tok::Punct("[" | "(" | ")" | ";" | "," | "=" | "{" | ":")
                ) =>
            {
                // clang reports the missing type once the declaration is
                // read: a function's declarator that nothing a declaration
                // can go on with follows (`typeof(int) x;`, `typeof` being
                // no keyword in C99) is its syntax error instead.
                let (start, warned) = (self.pos, self.warnings.borrow().len());
                let (kept, named) = (self.unevaluated.borrow().len(), self.undeclared.borrow().len());
                let at = self.here();
                let func = self
                    .declarator(&Type::INT, Ctx::File)
                    .is_ok_and(|d| d.ty.is_func());
                let goes_on = matches!(self.peek(), Tok::Punct(";" | "," | "=" | "{"));
                let here = self.here();
                self.pos = start;
                self.warnings.borrow_mut().truncate(warned);
                self.unevaluated.borrow_mut().truncate(kept);
                self.undeclared.borrow_mut().truncate(named);
                if func && !goes_on {
                    return Error::new(here, "expected function body after function declarator");
                }
                let msg = "type specifier missing, defaults to 'int'; ISO C99 and later do not support implicit int";
                Error::new(at, msg).flagged("implicit-int")
            }
            Tok::Ident(name) => {
                let name = name.clone();
                self.unknown_type_name(&name)
            }
            _ => self.expected("identifier or '('"),
        }
    }

    /// A top-level function, or a declaration: the items it makes, none for
    /// one that only declares types (a typedef, a struct, an enum).
    fn item(&mut self) -> Result<Vec<Item>, Error> {
        if *self.peek() == Tok::Kw(Kw::StaticAssert) {
            self.static_assert()?;
            return Ok(Vec::new());
        }
        // An empty declaration, which clang takes.
        if self.eat_punct(";") {
            return Ok(Vec::new());
        }
        if !self.at_decl() {
            return Err(self.not_a_declaration());
        }
        let start = self.unevaluated.borrow().len();
        let specs = self.specifiers(Ctx::File)?;
        if self.is_punct(";") {
            self.bare_specifiers(&specs);
            return Ok(Vec::new());
        }
        let d = self.declarator(&specs.ty, Ctx::File)?;
        if specs.class == Class::Typedef {
            self.typedefs(&specs, d, Ctx::File)?;
            return Ok(Vec::new());
        }
        if !d.ty.is_func() || matches!(self.peek(), Tok::Punct(";" | "," | "=")) {
            return self.init_declarators(&specs, Some(d), Ctx::File, start);
        }
        if !self.is_punct("{") {
            return Err(self.expected("function body after function declarator"));
        }
        // `register` and `auto` are for objects in a block (C99 6.9p2):
        // clang's error, after which it goes on with the function.
        if matches!(specs.class, Class::Register | Class::Auto) {
            self.warn(Error::new(specs.class_pos, "illegal storage class on function"));
        }
        self.folded(&d, Ctx::File, specs.class, false);
        // What the function's declarator left, checked at file scope.
        let unevaluated = self.unevaluated.borrow_mut().split_off(start);
        self.bind_name(&d.name, Name::Object(d.ty.clone()), d.pos)?;
        let mut d = d;
        let Some((params, scope)) = d.func.take() else {
            return Err(Error::new(
                d.pos,
                "a function definition needs its parameter list",
            ));
        };
        for p in params.iter().filter(|p| p.name.is_empty()) {
            let msg = "omitting the parameter name in a function definition is a C23 extension";
            self.warn(Error::warning(p.pos, msg, "c23-extensions"));
        }
        // A qualified `void` result, which no value has: clang's warning
        // of a definition, at its parameter list.
        if let (Some(f), Some(open)) = (d.ty.func_type(), d.open) {
            if f.ret.is_void() && !f.ret.quals().is_empty() {
                let msg = format!(
                    "function cannot return qualified void type {}",
                    f.ret.named(&specs.spelled)
                );
                self.warn(Error::warning(open, msg, "qualified-void-return-type"));
            }
        }
        // The body's outermost block shares the parameters' scope.
        self.scopes.push(scope);
        let body = self.block_body(false);
        self.scopes.pop();
        let body = match body {
            Ok(body) => body,
            Err(e) => {
                // The function as far as it got: each block cut short
                // ends with the one inside it.
                let mut blocks = self.salvaged.take().into_iter();
                let mut body = blocks.next().unwrap_or_default();
                for mut outer in blocks {
                    outer.push(Stmt {
                        kind: StmtKind::Block(body),
                        pos: e.pos,
                    });
                    body = outer;
                }
                let f = function(d, params, &specs, Some(body), e.pos, unevaluated);
                *self.partial.borrow_mut() = Some(Item::Func(f));
                return Err(e);
            }
        };
        // The `}` just read.
        let end = self.toks[self.pos - 1].pos;
        Ok(vec![Item::Func(function(
            d,
            params,
            &specs,
            Some(body),
            end,
            unevaluated,
        ))])
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
        let open = self.expect_punct("{")?;
        // What the statement around the block left so far (`if (c)`) is
        // checked with that statement, out of the block's scope.
        let outer = self.unevaluated.take();
        let mut items = Vec::new();
        let mark = self.undeclared.borrow().len();
        let mut item = ExprId::next();
        if let Err(e) = self.block_items_into(&mut items, open, &mut item) {
            // What the error leaves of the block ([`parse_salvaging`]),
            // and the undeclared names of the item it cut short.
            items.extend(self.unevaluated_stmt(self.here()));
            items.extend(self.cut_names(mark, item));
            self.salvaged.borrow_mut().push(items);
            *self.unevaluated.borrow_mut() = outer;
            return Err(e);
        }
        *self.unevaluated.borrow_mut() = outer;
        Ok(items)
    }

    /// The items of a block after its `{` (at `open`), up to and including
    /// its `}`; `item` is the [`ExprId::next`] of where the item being read
    /// began.
    fn block_items_into(&mut self, items: &mut Vec<Stmt>, open: Pos, item: &mut u64) -> Result<(), Error> {
        loop {
            *item = ExprId::next();
            if self.eat_punct("}") {
                return Ok(());
            }
            if *self.peek() == Tok::Eof {
                return Err(self.expect_close("}", open).unwrap_err());
            }
            let pos = self.here();
            if *self.peek() == Tok::Kw(Kw::StaticAssert) {
                self.static_assert()?;
            } else if self.at_decl() {
                if let Some(s) = self.decl(Ctx::Block)? {
                    items.push(s);
                }
            } else {
                items.push(self.stmt()?);
            }
            items.extend(self.unevaluated_stmt(pos));
        }
    }

    /// `_Static_assert(constant, "message");` (C11, which clang accepts in
    /// C99 too; without the message C23's, which clang takes with a
    /// warning), at file or block scope. The parser evaluates constants, so
    /// it checks the assertion here; what is left of it is the condition
    /// for sema to check ([`Unevaluated::constant`]). A failed one is
    /// clang's error, which says what failed as clang's
    /// `findFailedBooleanCondition` does ([`Self::failed_requirement`]):
    /// sema gives it after the condition's diagnostics, unless it has an
    /// error, and clang goes on after it, as merlin does.
    fn static_assert(&mut self) -> Result<(), Error> {
        self.advance();
        let open = self.expect_punct("(")?;
        let at = self.here();
        let first = self.pos;
        *self.parens.borrow_mut() = Some(Vec::new());
        let cond = self.cond();
        let parens = self.parens.take().unwrap_or_default();
        let mut cond = cond?;
        let tokens = first..self.pos;
        let mut msg = None;
        if self.eat_punct(",") {
            if !matches!(self.peek(), Tok::Str(_)) {
                return Err(self.expected("string literal for diagnostic message in static_assert"));
            }
            let mut text = Vec::new();
            while let Tok::Str(s) = self.peek() {
                text.extend_from_slice(s);
                self.advance();
            }
            msg = Some(String::from_utf8_lossy(&text).into_owned());
        } else if self.is_punct(")") {
            let warning = "'_Static_assert' with no message is a C23 extension";
            self.warn(Error::warning(self.here(), warning, "c23-extensions"));
        } else {
            return Err(self.expect_punct(",").unwrap_err());
        }
        self.expect_close(")", open)?;
        self.expect_semi("after '_Static_assert'")?;
        let original = cond.clone();
        let not_constant = "static assertion expression is not an integral constant expression";
        let mark = self.unevaluated.borrow().len();
        match self.const_int(&mut cond, at, not_constant, None)? {
            // Not sure of the value: sema reports the error.
            (v, sure) if !sure || v != 0 => Ok(()),
            _ => {
                let printer = Printer {
                    toks: &self.toks[tokens],
                    parens: &parens,
                };
                let msg = msg.map(|m| format!(": {m}")).unwrap_or_default();
                let failed = match self.failed_requirement(&original, &cond, &printer) {
                    Some((term, value)) => {
                        let what = printer.expr(term, true);
                        let e = Error::new(
                            printer.begin(term, true),
                            format!("static assertion failed due to requirement '{what}'{msg}"),
                        );
                        match self.evaluates_to(term, value, &printer) {
                            Some((at, note)) => e.note(at, note),
                            None => e,
                        }
                    }
                    None => Error::new(at, format!("static assertion failed{msg}")),
                };
                let mut us = self.unevaluated.borrow_mut();
                if us.len() == mark {
                    // `_Static_assert(0, "")`: of a lone literal, nothing
                    // was kept to check.
                    drop(us);
                    self.warn(failed);
                    return Ok(());
                }
                let with = us.len() - mark - 1;
                let u = us.last_mut().expect("the condition itself");
                u.diagnostic = Some(failed);
                u.with = with;
                Ok(())
            }
        }
    }

    /// What a `_Static_assert` whose condition is 0 failed on, as clang
    /// finds it: the first term of the condition's `&&`s (through
    /// parentheses) that is 0 and no integer literal, else the whole
    /// condition, or `None` if that is an integer literal itself. Each
    /// term of the condition as written (`original`) is evaluated as the
    /// parser has resolved it (`resolved`, of the same shape).
    fn failed_requirement<'e, 'r>(
        &self,
        original: &'e Expr,
        resolved: &'r Expr,
        p: &Printer,
    ) -> Option<(&'e Expr, &'r Expr)> {
        fn terms<'x>(e: &'x Expr, out: &mut Vec<&'x Expr>) {
            match &e.kind {
                ExprKind::Binary(BinOp::LogAnd, l, r) => {
                    terms(l, out);
                    terms(r, out);
                }
                _ => out.push(e),
            }
        }
        let (mut written, mut values) = (Vec::new(), Vec::new());
        terms(original, &mut written);
        terms(resolved, &mut values);
        let failed = written
            .iter()
            .zip(&values)
            .find(|(w, v)| !p.is_integer_literal(w) && consteval::parsed_int(v) == Some(0));
        match failed {
            Some((w, v)) => Some((w, v)),
            None if p.is_integer_literal(original) => None,
            None => Some((original, resolved)),
        }
    }

    /// clang's note on a failed `_Static_assert` of a binary operator (not
    /// `&&` or `||`): what its operands evaluate to (`expression evaluates to '2 ==
    /// 3'`), at the operator, unless both are plain to see, literals or
    /// what unary operators make of them. `written` is the term as written,
    /// `value` as the parser resolved it.
    fn evaluates_to(&self, written: &Expr, value: &Expr, p: &Printer) -> Option<(Pos, String)> {
        let (ExprKind::Binary(op, wl, wr), ExprKind::Binary(_, l, r)) = (&written.kind, &value.kind) else {
            return None;
        };
        if matches!(op, BinOp::LogOr | BinOp::LogAnd) {
            return None;
        }
        fn plain(e: &Expr, p: &Printer) -> bool {
            match &e.kind {
                ExprKind::Unary(_, x) => plain(x, p),
                ExprKind::Num(_) => p.token(e.pos).is_some_and(|t| matches!(t.tok, Tok::Num(..))),
                _ => false,
            }
        }
        if plain(wl, p) && plain(wr, p) {
            return None;
        }
        let (a, b) = (consteval::parsed_int(l)?, consteval::parsed_int(r)?);
        Some((
            value.pos,
            format!("expression evaluates to '{a} {} {b}'", op.symbol()),
        ))
    }

    /// `int a = 1, *b;`, or `None` for one that only declares types.
    fn decl(&mut self, ctx: Ctx) -> Result<Option<Stmt>, Error> {
        let pos = self.here();
        let start = self.unevaluated.borrow().len();
        let specs = self.specifiers(ctx)?;
        if self.is_punct(";") {
            self.bare_specifiers(&specs);
            return Ok(None);
        }
        let d = self.declarator(&specs.ty, ctx)?;
        if specs.class == Class::Typedef {
            self.typedefs(&specs, d, ctx)?;
            return Ok(None);
        }
        // In a block, functions are declarators too: one declaration.
        let decls = self
            .init_declarators(&specs, Some(d), ctx, start)?
            .into_iter()
            .flat_map(|item| match item {
                Item::Decl(decls) => decls,
                Item::Func(_) => unreachable!("only at file scope"),
            })
            .collect();
        Ok(Some(Stmt {
            kind: StmtKind::Decl(decls),
            pos,
        }))
    }

    // ---- statements ----

    /// `(expr)` after the keyword `kw` (`if`, `while`, `switch`, and
    /// `do/while`, as clang calls it).
    fn paren_expr(&mut self, kw: &str) -> Result<Expr, Error> {
        if !self.is_punct("(") {
            return Err(self.expected(&format!("'(' after '{kw}'")));
        }
        let open = self.advance().pos;
        let e = self.expr()?;
        self.expect_close(")", open)?;
        Ok(e)
    }

    fn stmt(&mut self) -> Result<Stmt, Error> {
        self.nested(Self::stmt_inner)
    }

    /// What a label (or `case` or `default`) labels: a statement, or none
    /// at the end of a block, which clang takes with a warning (C23 allows
    /// it).
    fn labeled(&mut self) -> Result<Box<Stmt>, Error> {
        if self.is_punct("}") {
            let msg = "label at end of compound statement is a C23 extension";
            self.warn(Error::warning(self.here(), msg, "c23-extensions"));
            return Ok(Box::new(Stmt {
                kind: StmtKind::Empty { glued: false },
                pos: self.here(),
            }));
        }
        Ok(Box::new(self.stmt()?))
    }

    fn stmt_inner(&mut self) -> Result<Stmt, Error> {
        let pos = self.here();
        // A declaration is no statement (`if (c) int x;`): what clang
        // wanted there is an expression.
        if self.at_decl() {
            return Err(self.expected("expression"));
        }
        if self.is_punct("}") {
            return Err(self.expected("statement"));
        }
        let kind = match self.peek().clone() {
            Tok::Punct("{") => StmtKind::Block(self.block_body(true)?),
            Tok::Punct(";") => {
                let glued = self.after_prev().line == self.here().line;
                self.advance();
                StmtKind::Empty { glued }
            }
            Tok::Ident(name) if matches!(self.peek_at(1), Tok::Punct(":")) => {
                self.advance();
                self.advance();
                StmtKind::Label {
                    name,
                    body: self.labeled()?,
                }
            }
            Tok::Kw(Kw::If) => {
                self.advance();
                let cond = self.paren_expr("if")?;
                let then = Box::new(self.stmt()?);
                let inner = self.trailing_else.take();
                let els = if *self.peek() == Tok::Kw(Kw::Else) {
                    let at = self.advance().pos;
                    let els = Box::new(self.stmt()?);
                    self.trailing_else.set(Some(at));
                    Some(els)
                } else {
                    // `if (a) if (b) x; else y;`: the `else` is the inner
                    // `if`'s, which clang warns may not be what was meant.
                    if let Some(at) = inner {
                        self.warn(Error::warning(
                            at,
                            "add explicit braces to avoid dangling else",
                            "dangling-else",
                        ));
                    }
                    None
                };
                return Ok(Stmt {
                    kind: StmtKind::If { cond, then, els },
                    pos,
                });
            }
            Tok::Kw(Kw::Switch) => {
                self.advance();
                let cond = self.paren_expr("switch")?;
                StmtKind::Switch {
                    cond,
                    body: Box::new(self.stmt()?),
                    cases: Vec::new(),
                    default: None,
                    covered: false,
                }
            }
            Tok::Kw(Kw::Case) => {
                self.advance();
                let start = self.pos;
                let expr = self.cond()?;
                let bare = self.pos == start + 1;
                // clang's error is where the `:` should be, after the value.
                if !self.eat_punct(":") {
                    return Err(Error::new(self.after_prev(), "expected ':' after 'case'"));
                }
                StmtKind::Case {
                    expr,
                    value: 0,
                    id: 0,
                    body: self.labeled()?,
                    bare,
                }
            }
            Tok::Kw(Kw::Default) => {
                self.advance();
                if !self.eat_punct(":") {
                    return Err(Error::new(self.after_prev(), "expected ':' after 'default'"));
                }
                StmtKind::Default {
                    id: 0,
                    body: self.labeled()?,
                }
            }
            Tok::Kw(Kw::While) => {
                self.advance();
                let cond = self.paren_expr("while")?;
                let body = Box::new(self.stmt()?);
                StmtKind::While { cond, body }
            }
            Tok::Kw(Kw::Do) => {
                self.advance();
                let body = Box::new(self.stmt()?);
                if !self.eat_kw(Kw::While) {
                    return Err(self
                        .expected("'while' in do/while loop")
                        .note(pos, "to match this 'do'"));
                }
                let cond = self.paren_expr("do/while")?;
                self.expect_semi("after do/while statement")?;
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
                    let named = self.undeclared.borrow().len();
                    let e = self.expr()?;
                    self.semi_or_skip("after return statement", named)?;
                    StmtKind::Return(Some(e))
                }
            }
            Tok::Kw(Kw::Break) => {
                self.advance();
                self.expect_semi("after break statement")?;
                StmtKind::Break
            }
            Tok::Kw(Kw::Continue) => {
                self.advance();
                self.expect_semi("after continue statement")?;
                StmtKind::Continue
            }
            Tok::Kw(Kw::Goto) => {
                self.advance();
                let (name, at) = self.expect_ident()?;
                self.expect_semi("after goto statement")?;
                StmtKind::Goto(name, at)
            }
            // `x y` of a name nothing declares: what clang's typo
            // correction of a statement's first name takes it for.
            Tok::Ident(name)
                if self.lookup_name(&name).is_none() && matches!(self.peek_at(1), Tok::Ident(_)) =>
            {
                return Err(self.unknown_statement_name(&name));
            }
            _ => {
                let named = self.undeclared.borrow().len();
                let e = self.expr()?;
                self.semi_or_skip("after expression", named)?;
                StmtKind::Expr(e)
            }
        };
        // Only an `if`, and a statement that ends with one (a loop, a
        // `switch`, a label), ends with its `else`.
        if !matches!(
            kind,
            StmtKind::While { .. }
                | StmtKind::For { .. }
                | StmtKind::Switch { .. }
                | StmtKind::Label { .. }
                | StmtKind::Case { .. }
                | StmtKind::Default { .. }
        ) {
            self.trailing_else.set(None);
        }
        Ok(Stmt { kind, pos })
    }

    /// The `;` after a `for`'s first and second clauses: clang's error is
    /// at the token that is there instead.
    fn for_semi(&mut self) -> Result<(), Error> {
        if self.eat_punct(";") {
            return Ok(());
        }
        Err(self.expected("';' in 'for' statement specifier"))
    }

    /// After `for`: `(init; cond; step) body`, in the `for`'s own scope.
    fn for_rest(&mut self) -> Result<StmtKind, Error> {
        let start = self.unevaluated.borrow().len();
        if !self.is_punct("(") {
            return Err(self.expected("'(' after 'for'"));
        }
        let open = self.advance().pos;
        let init = if self.at_decl() {
            self.decl(Ctx::ForInit)?.map(Box::new)
        } else if self.eat_punct(";") {
            None
        } else {
            let ipos = self.here();
            let e = self.expr()?;
            self.for_semi()?;
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
        self.for_semi()?;
        let step = if self.is_punct(")") {
            None
        } else {
            Some(self.expr()?)
        };
        self.expect_close(")", open)?;
        let body = Box::new(self.stmt()?);
        Ok(StmtKind::For {
            init,
            cond,
            step,
            body,
            unevaluated: self.unevaluated.borrow_mut().split_off(start),
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
            if !p.eat_punct(":") {
                return Err(p.expected("':'").note(pos, "to match this '?'"));
            }
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
                let (d, spelled) = self.type_name_decl(pos)?;
                self.expect_close(")", pos)?;
                let literal = self.is_punct("{");
                self.type_name_folded(&d, literal.then_some(pos));
                let ty = d.ty;
                if literal {
                    let e = self.compound_literal(ty, pos)?;
                    return self.postfix_ops(e);
                }
                let x = self.nested(Self::unary)?;
                return Ok(Expr {
                    id: ExprId::new(),
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
                    let (d, _) = self.type_name_decl(at)?;
                    self.expect_close(")", at)?;
                    let literal = self.is_punct("{");
                    self.type_name_folded(&d, literal.then_some(at));
                    let ty = d.ty;
                    if literal {
                        let e = self.compound_literal(ty, at)?;
                        ExprKind::SizeofExpr(Box::new(self.postfix_ops(e)?), at)
                    } else {
                        ExprKind::SizeofType(ty)
                    }
                } else if self.type_name_here() {
                    // Where the `(` belongs, after `sizeof`.
                    let msg = "expected parentheses around type name in sizeof expression";
                    return Err(Error::new(self.after_prev(), msg));
                } else {
                    let at = self.here();
                    ExprKind::SizeofExpr(Box::new(self.nested(Self::unary)?), at)
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
            id: ExprId::new(),
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
                    let close = self.here();
                    self.expect_close("]", pos)?;
                    self.brackets_written.borrow_mut().insert(pos, close);
                    // a[i] is *(a + i).
                    let sum = Expr::new(ExprKind::Binary(BinOp::Add, Box::new(e), Box::new(index)), pos);
                    e = Expr::new(ExprKind::Deref(Box::new(sum)), pos);
                    continue;
                }
                Tok::Punct("(") => {
                    self.enter()?;
                    let open = self.advance().pos;
                    let mut args = Vec::new();
                    if !self.is_punct(")") {
                        loop {
                            if args.len() == MAX_PARAMS {
                                return Err(Error::new(
                                    self.here(),
                                    format!("too many arguments: merlin supports at most {MAX_PARAMS}"),
                                ));
                            }
                            args.push(self.assign()?);
                            if self.is_punct(")") {
                                break;
                            }
                            if !self.eat_punct(",") {
                                return Err(self.expect_close(")", open).unwrap_err());
                            }
                        }
                    }
                    let close = self.advance().pos;
                    // A call is positioned at its callee, as clang reports it.
                    let pos = e.pos;
                    e = Expr::new(ExprKind::call(e, args, (open, close)), pos);
                    continue;
                }
                Tok::Punct(p @ ("." | "->")) => {
                    let arrow = *p == "->";
                    self.enter()?;
                    let pos = self.advance().pos;
                    let (name, at) = self.expect_ident()?;
                    e = Expr::new(
                        ExprKind::Member {
                            base: Box::new(e),
                            name,
                            arrow,
                            offset: 0,
                            at,
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
                let t = self.advance();
                if t.text.len() > 1 && t.text.starts_with('0') && t.text.as_bytes()[1].is_ascii_alphanumeric()
                {
                    self.bit_patterns.borrow_mut().insert(pos);
                }
                if t.text.ends_with('\'') {
                    self.char_literals.borrow_mut().insert(pos);
                } else {
                    self.int_literals.borrow_mut().insert(pos);
                }
                return Ok(Expr {
                    id: ExprId::new(),
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
                    id: ExprId::new(),
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
                    Some(_) => {}
                    // `(unsinged)1`, `sizeof(myit)`: in parentheses alone,
                    // where a type's name could be, clang takes it for one
                    // if that is closest.
                    None if self.pos > 0
                        && matches!(self.toks[self.pos - 1].tok, Tok::Punct("("))
                        && matches!(self.peek_at(1), Tok::Punct(")"))
                        && self
                            .correct_type(&name)
                            .is_some_and(|(to, _)| self.correct_expr(&name).is_none_or(|c| c.name != to)) =>
                    {
                        let (to, decl) = self.correct_type(&name).expect("just found");
                        let e = Error::new(
                            pos,
                            format!("use of undeclared identifier '{name}'; did you mean '{to}'?"),
                        );
                        return Err(match decl {
                            Some(decl) => e.note(decl, format!("'{to}' declared here")),
                            None => e,
                        });
                    }
                    None => {
                        if let Some(c) = self.correct_expr(&name) {
                            self.corrections.borrow_mut().insert(pos, c);
                        }
                        let var = Expr::new(
                            ExprKind::Var {
                                name: name.clone(),
                                var: None,
                            },
                            pos,
                        );
                        // Called, it is declared implicitly: clang's error
                        // of that as it reads the name, which a call of no
                        // arguments gives too.
                        let e = match self.peek_at(1) {
                            Tok::Punct("(") => {
                                let open = self.toks[self.pos + 1].pos;
                                Expr::new(ExprKind::call(var, Vec::new(), (open, open)), pos)
                            }
                            _ => var,
                        };
                        self.undeclared.borrow_mut().push(e);
                    }
                }
                self.advance();
                ExprKind::Var { name, var: None }
            }
            Tok::Punct("(") => {
                // Keep the inner node; parentheses only steer the tree's
                // shape (and what a failed `_Static_assert` prints).
                self.advance();
                let e = self.expr()?;
                self.expect_close(")", pos)?;
                if let Some(parens) = self.parens.borrow_mut().as_mut() {
                    parens.push((paren_key(&e), pos));
                }
                self.parens_written.borrow_mut().insert(e.pos, pos);
                return Ok(e);
            }
            _ => return Err(self.expected("expression")),
        };
        Ok(Expr::new(kind, pos))
    }

    /// `__builtin_va_start(ap, last)`, `__builtin_va_arg(ap, type)`,
    /// `__builtin_va_end(ap)` and `__builtin_va_copy(dst, src)`: clang's
    /// builtins behind `<stdarg.h>`'s macros. They look like calls, but
    /// `va_arg` takes a type, so the parser knows them by name.
    fn va_builtin(&mut self, name: &str) -> Result<Expr, Error> {
        let pos = self.advance().pos;
        let open = self.expect_punct("(")?;
        let first = Box::new(self.assign()?);
        let kind = match name {
            // Calls to clang, which reads their arguments as any call's.
            "__builtin_va_start" | "__builtin_va_copy" => {
                if self.is_punct(")") {
                    let msg = "too few arguments to function call, expected 2, have 1";
                    return Err(Error::new(self.here(), msg));
                }
                if !self.eat_punct(",") {
                    return Err(self.expect_close(")", open).unwrap_err());
                }
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
                let at = self.here();
                let ty = self.type_name(open)?;
                self.expect_close(")", open)?;
                return Ok(Expr {
                    id: ExprId::new(),
                    kind: ExprKind::VaArg(first, at),
                    // The value is a copy: qualifiers mean nothing on it.
                    ty: ty.unqual().clone(),
                    pos,
                });
            }
            "__builtin_va_end" => ExprKind::VaEnd(first),
            _ => return Err(Error::new(pos, format!("unknown builtin '{name}'"))),
        };
        self.expect_close(")", open)?;
        Ok(Expr::new(kind, pos))
    }
}

/// What tells a parenthesized expression from the others, the tree
/// keeping no parentheses: its position and its kind (a call is where its
/// callee is, but no callee is a call).
type ParenKey = (Pos, std::mem::Discriminant<ExprKind>);

fn paren_key(e: &Expr) -> ParenKey {
    (e.pos, std::mem::discriminant(&e.kind))
}

/// Prints an expression as clang's `StmtPrinter` does, for what a failed
/// `_Static_assert` says: literals by their values (`0x10` is `16`, an
/// `unsigned` one `16U`), operators between spaces, the parentheses as
/// written (`parens`, for the tree has none), types as clang spells them
/// with their typedefs resolved, and names as written. `toks` are the
/// tokens of the expression, which say how a literal was written.
struct Printer<'a> {
    toks: &'a [Token],
    parens: &'a [(ParenKey, Pos)],
}

impl Printer<'_> {
    fn token(&self, pos: Pos) -> Option<&Token> {
        self.toks.iter().find(|t| t.pos == pos)
    }

    /// How many parentheses are around `e`, and where the outermost opens.
    fn parens_of(&self, e: &Expr) -> (usize, Option<Pos>) {
        let key = paren_key(e);
        let opens = self.parens.iter().filter(|(k, _)| *k == key).map(|&(_, at)| at);
        (opens.clone().count(), opens.min())
    }

    /// Whether `e` is an integer literal (not a character one).
    fn is_integer_literal(&self, e: &Expr) -> bool {
        matches!(e.kind, ExprKind::Num(_))
            && self
                .token(e.pos)
                .is_some_and(|t| matches!(t.tok, Tok::Num(..)) && !t.text.ends_with('\''))
    }

    /// The `[]` of `a[i]`, which the parser made `*(a + i)` at the `[`.
    fn subscript<'e>(&self, e: &'e Expr) -> Option<(&'e Expr, &'e Expr)> {
        match &e.kind {
            ExprKind::Deref(x) if x.pos == e.pos => match &x.kind {
                ExprKind::Binary(BinOp::Add, a, i) => Some((a, i)),
                _ => None,
            },
            _ => None,
        }
    }

    /// Where `e` starts: clang's place for what is said of it. Its own
    /// parentheses count unless it is the `top` of what is printed.
    fn begin(&self, e: &Expr, top: bool) -> Pos {
        if let (_, Some(open)) = self.parens_of(e) {
            if !top {
                return open;
            }
        }
        if let Some((a, _)) = self.subscript(e) {
            return self.begin(a, false);
        }
        match &e.kind {
            ExprKind::Binary(_, l, _) | ExprKind::Comma(l, _) | ExprKind::Cond(l, ..) => self.begin(l, false),
            ExprKind::Assign { target, .. }
            | ExprKind::IncDec {
                target,
                prefix: false,
                ..
            } => self.begin(target, false),
            ExprKind::Member { base, .. } => self.begin(base, false),
            ExprKind::Call { callee, .. } => self.begin(callee, false),
            _ => e.pos,
        }
    }

    /// `e` as clang prints it, without its own parentheses if `top`.
    fn expr(&self, e: &Expr, top: bool) -> String {
        let p = |x: &Expr| self.expr(x, false);
        let text = if let Some((a, i)) = self.subscript(e) {
            format!("{}[{}]", p(a), p(i))
        } else {
            match &e.kind {
                ExprKind::Num(v) => match self.token(e.pos) {
                    Some(t) if t.text.ends_with('\'') => char_literal(*v, &t.text),
                    // An enumeration constant inside its enum.
                    Some(t) if matches!(t.tok, Tok::Ident(_)) => t.text.clone(),
                    _ => {
                        let suffix = match e.ty.int_kind() {
                            Some(IntKind::UInt) => "U",
                            Some(IntKind::Long) => "L",
                            Some(IntKind::ULong) => "UL",
                            Some(IntKind::LongLong) => "LL",
                            Some(IntKind::ULongLong) => "ULL",
                            _ => "",
                        };
                        format!("{v}{suffix}")
                    }
                },
                ExprKind::Enumerator { name, .. } | ExprKind::Var { name, .. } => name.clone(),
                ExprKind::Str(bytes) => {
                    let chars: String = bytes.iter().map(|&b| escaped(u32::from(b), '"')).collect();
                    format!("\"{chars}\"")
                }
                ExprKind::Binary(op, l, r) => format!("{} {} {}", p(l), op.symbol(), p(r)),
                ExprKind::Comma(l, r) => format!("{} , {}", p(l), p(r)),
                ExprKind::Unary(op, x) => format!("{}{}", op.symbol(), p(x)),
                ExprKind::Addr(x) => format!("&{}", p(x)),
                ExprKind::Deref(x) => format!("*{}", p(x)),
                ExprKind::Cast { x, .. } => format!("({}){}", e.ty, p(x)),
                ExprKind::SizeofType(t) => format!("sizeof({t})"),
                ExprKind::SizeofExpr(x, _) => format!("sizeof {}", p(x)),
                ExprKind::Cond(c, a, b) => format!("{} ? {} : {}", p(c), p(a), p(b)),
                ExprKind::Assign { op, target, value } => {
                    let op = op.map_or("", |o| o.symbol());
                    format!("{} {op}= {}", p(target), p(value))
                }
                ExprKind::IncDec { inc, prefix, target } => {
                    let op = if *inc { "++" } else { "--" };
                    if *prefix {
                        format!("{op}{}", p(target))
                    } else {
                        format!("{}{op}", p(target))
                    }
                }
                ExprKind::Member {
                    base, name, arrow, ..
                } => format!("{}{}{name}", p(base), if *arrow { "->" } else { "." }),
                ExprKind::Call { callee, args, .. } => {
                    let args: Vec<String> = args.iter().map(p).collect();
                    format!("{}({})", p(callee), args.join(", "))
                }
                ExprKind::CompoundLit { init, .. } => format!("({}){}", e.ty, self.init(init)),
                ExprKind::VaStart { ap, last } => format!("__builtin_va_start({}, {})", p(ap), p(last)),
                ExprKind::VaArg(ap, _) => format!("__builtin_va_arg({}, {})", p(ap), e.ty),
                ExprKind::VaEnd(ap) => format!("__builtin_va_end({})", p(ap)),
                ExprKind::VaCopy(d, s) => format!("__builtin_va_copy({}, {})", p(d), p(s)),
            }
        };
        let (n, _) = if top { (0, None) } else { self.parens_of(e) };
        format!("{}{text}{}", "(".repeat(n), ")".repeat(n))
    }

    fn init(&self, init: &Init) -> String {
        match init {
            Init::Expr(e) => self.expr(e, false),
            Init::List(items, _) => {
                let items: Vec<String> = items.iter().map(|i| self.init(i)).collect();
                format!("{{{}}}", items.join(", "))
            }
            Init::Designated(designators, init) => {
                let mut s = String::new();
                for d in designators {
                    match d {
                        Designator::Member(name, ..) => s += &format!(".{name}"),
                        Designator::Index(e, _) => s += &format!("[{}]", self.expr(e, false)),
                    }
                }
                format!("{s} = {}", self.init(init))
            }
        }
    }
}

/// A character `c` of a literal quoted by `quote`, as clang prints it:
/// C's escapes, the printable ones as they are, the rest in hexadecimal.
fn escaped(c: u32, quote: char) -> String {
    match char::from_u32(c) {
        Some('\\') => "\\\\".to_string(),
        Some(q) if q == quote => format!("\\{q}"),
        Some('\x07') => "\\a".to_string(),
        Some('\x08') => "\\b".to_string(),
        Some('\x0c') => "\\f".to_string(),
        Some('\n') => "\\n".to_string(),
        Some('\r') => "\\r".to_string(),
        Some('\t') => "\\t".to_string(),
        Some('\x0b') => "\\v".to_string(),
        Some(ch) if (' '..='~').contains(&ch) => ch.to_string(),
        _ if c < 0x100 => format!("\\x{c:02x}"),
        _ if c <= 0xffff => format!("\\u{c:04x}"),
        _ => format!("\\U{c:08x}"),
    }
}

/// A character literal of value `v`, written `text` (`'a'`, `L'a'`), as
/// clang prints it (`CharacterLiteral::print`).
fn char_literal(v: i128, text: &str) -> String {
    let wide = if text.starts_with('L') { "L" } else { "" };
    let mut c = v as u32;
    // A plain one may be sign-extended.
    if wide.is_empty() && c & !0xff == !0xff {
        c &= 0xff;
    }
    format!("{wide}'{}'", escaped(c, '\''))
}

/// A function's declaration (no body) from its declarator, with what the
/// declarator left for sema to check.
fn declared_function(d: Decl, specs: &Specs, unevaluated: Vec<Unevaluated>) -> Function {
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
                        register: false,
                        array: None,
                    })
                    .collect()
            })
            .unwrap_or_default()
    });
    let end = d.pos;
    function(d, params, specs, None, end, unevaluated)
}
/// A function declaration or definition from its declarator, with what
/// the declarator left for sema to check.
fn function(
    d: Decl,
    params: Vec<Param>,
    specs: &Specs,
    body: Option<Vec<Stmt>>,
    end: Pos,
    unevaluated: Vec<Unevaluated>,
) -> Function {
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
        is_inline: specs.inline.is_some(),
        is_extern: specs.class == Class::Extern,
        inline_only: false,
        prototyped: d.prototyped,
        body,
        locals: Vec::new(),
        ret_ptr: None,
        pos: d.pos,
        end,
        unevaluated,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::lex;

    fn prog(src: &str) -> Program {
        parse(&lex(src).unwrap()).unwrap_or_else(|e| panic!("{src}: {e}"))
    }

    /// The first error of `src`: the parser's, or sema's, which checks what
    /// the parser left it (an array size it could not work out is sema's
    /// to report).
    fn err(src: &str) -> String {
        let errors = crate::parse(src).unwrap_err();
        errors
            .iter()
            .find(|e| !e.is_warning())
            .unwrap_or_else(|| panic!("{src}: no error in {errors:?}"))
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
            ExprKind::SizeofExpr(x, _) => format!("(sizeof {})", sx(x)),
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
            ExprKind::VaArg(ap, _) => format!("(va_arg {} {})", sx(ap), e.ty),
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
                        Designator::Member(name, ..) => s += &format!(".{name}"),
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
            Storage::Register => "register ",
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
            StmtKind::Empty { .. } => ";".into(),
            StmtKind::Unevaluated(us) => format!(
                "(unevaluated {})",
                us.iter().map(|u| sx(&u.expr)).collect::<Vec<_>>().join(" ")
            ),
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
                ..
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
            StmtKind::Goto(name, _) => format!("(goto {name})"),
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
        // Only a lone, unnamed, unqualified void is "no parameters"
        // (tests/diagnostics has the errors of the others).
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
        // The same specifier twice is only warned of; `long` twice is
        // `long long`.
        assert_eq!(ty("unsigned unsigned"), "unsigned int x");
        assert_eq!(ty("short short int"), "short x");
        assert_eq!(ty("long unsigned long"), "unsigned long long x");
        assert_eq!(ty("static static int"), "static int x");
        assert_eq!(ty("_Bool"), "_Bool x");
        assert_eq!(ty("const int"), "const int x");
        assert_eq!(ty("int const volatile"), "const volatile int x");
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
        prog("typedef int T; typedef int T;");
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
        assert_eq!(t[0].size(), Some(8));
        // A struct may point to itself.
        let t = global_types("struct node { int v; struct node *next; } n;");
        let body = t[0].record().unwrap().body().unwrap();
        assert_eq!(body.members[1].ty.pointee(), Some(&t[0]));
        // A forward declaration, completed later.
        let t = global_types("struct s; struct s *p; struct s { char c; } x;");
        assert_eq!(t[0].pointee(), Some(&t[1]));
        assert_eq!(t[0].pointee().unwrap().size(), Some(1));
        // An inner scope's struct is a different type.
        let p = prog("struct s { int a; } g; int f() { struct s { char c[3]; } l; return sizeof l; }");
        let StmtKind::Decl(ds) = &funcs(&p)[0].body.as_ref().unwrap()[0].kind else {
            panic!()
        };
        assert_eq!(ds[0].ty.size(), Some(3));
        // Anonymous structs and nested definitions (whose tags are in scope outside).
        let t = global_types("struct { struct in { short a; } i; char c; } a; struct in b;");
        assert_eq!((t[0].size(), t[1].size()), (Some(4), Some(2)));
        // The file is the preprocessor's to name (crate::place).
        assert_eq!(t[0].to_string(), "struct (unnamed at \u{1}0\u{1}1:1)");
        // Specifiers that declare nothing are warned of, and left out.
        let p = prog("int; struct s { int; char c; } x; struct { int a; };");
        assert_eq!(p.warnings.len(), 3);
        assert_eq!(global_types("struct s { int; char c; } x;")[0].size(), Some(1));
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
        // A label may end a block (C23), which clang takes with a warning.
        assert_eq!(ss(&stmt("{ x: }")), "{(x: ;)}");
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
    }

    /// The operand a constant `?:` does not choose is not evaluated: in an
    /// array size, an enumerator or a `_Static_assert` it need not be
    /// constant, and only its type counts, as sema gives it (clang 23 takes
    /// them all, the array size as a GNU folding).
    #[test]
    fn the_operand_not_chosen_counts_by_its_type() {
        assert_eq!(
            globals(
                "unsigned u(void); long long l(void); int g;
                 int a[1 ? 2 : u()]; char b[sizeof(1 ? 1 : l())];
                 enum { A = (1 ? -1 : u()) < 0, B = 0 ? g : 3 } e; char c[B]; char d[A + 1];
                 _Static_assert(1 ? 1 : u(), \"chosen\");"
            ),
            [
                "int g",
                "int a[2]",
                "char b[8]",
                "unsigned int e",
                "char c[3]",
                "char d[1]",
            ]
        );
        // clang folds such an array size only where an array cannot be
        // variable-length: at file scope, in a member, in a `static` local,
        // in a local array with an initializer. (Elsewhere the array is a
        // variable-length one, which merlin does not have: an error, as
        // `int a[n]` is.)
        let p = prog(
            "unsigned u(void); typedef int T[1 ? 2 : u()]; int (*pa)[1 ? 3 : u()];\n\
             int f(void) {\n\
                 static int s[1 ? 2 : u()];\n\
                 int i[1 ? 2 : u()] = {1, 2};\n\
                 struct m { char c[1 ? 4 : u()]; } v;\n\
                 return s[0] + i[0] + v.c[0];\n\
             }",
        );
        let body = format!("{:?}", funcs(&p)[1].body);
        assert!(body.contains("Array(Int(Int), Some(2))"), "{body}");
        let p = prog(
            "unsigned u(void); int *q = (int[1 ? 2 : u()]){1, 2};
             int f(void) { return ((int[1 ? 3 : u()][1]){{1}, {2}, {3}})[2][0]; }",
        );
        let Item::Decl(q) = &p.items[1] else { panic!() };
        let Some(Init::Expr(e)) = &q[0].init else { panic!() };
        assert_eq!(format!("{}", e.ty), "int[2]");
        // At file scope, a type name that only points to such an array is
        // clang's variably modified type, which it takes there.
        assert_eq!(
            globals(
                "unsigned u(void); void *p = (int (*)[1 ? 2 : u()])0;
                 int z = sizeof(int (*(*)(void))[1 ? 2 : u()]);"
            ),
            [
                "void *p=(cast int (*)[2] 0)",
                "int z=(sizeof int (*(*)(void))[2])"
            ]
        );
    }

    /// The operand a constant `?:` does not choose is not evaluated, so
    /// `1 / 0` there is a constant: nothing is folded (clang 23 takes the
    /// array in a block, and gives no warning).
    #[test]
    fn an_operand_not_evaluated_divides_by_zero() {
        assert_eq!(
            globals("int a[1 ? 2 : 1 / 0]; int b[0 ? 1 % 0 : 3];"),
            ["int a[2]", "int b[3]"]
        );
        let p = prog("int main(void) { int a[1 ? 2 : 1 / 0]; a[0] = 1; return sizeof a; }");
        let body = format!("{:?}", funcs(&p)[0].body);
        assert!(body.contains("Array(Int(Int), Some(2))"), "{body}");
        assert!(p.unevaluated.is_empty());
    }

    /// The operands the parser evaluates away in a constant it needs (an
    /// array size, an enumerator, a `_Static_assert`) are left for sema to
    /// check where they were: those of `sizeof` and those a constant `?:`
    /// does not choose. A declarator keeps its own and its initializer's,
    /// a function its declarator's, a `for` statement its own; the rest
    /// are statements after the declaration or statement in a block, and
    /// in the program at file scope after the items before them. One in a
    /// parameter list has the parameters before it.
    #[test]
    fn what_is_evaluated_away_is_left_for_sema() {
        let p = prog(
            "int f(void); int g;\nint a[1 ? 2 : f()];\nint b[sizeof g];\nenum { E = 0 ? g : 1 };\n\
             int h(int n, char p[sizeof n]) {\n    int x;\n    char c[sizeof x] = {0}, d = sizeof(char[sizeof c]);\n    \
                 _Static_assert(1 ? 1 : f(), \"\");\n    \
                 for (int i = 0; i < sizeof(char[sizeof i]); i++) x++;\n    \
                 if (sizeof(char[sizeof g])) { int g = 0; x = g; }\n    \
                 return c[0];\n}\n",
        );
        // What is evaluated away; the constants kept with it are below.
        let list = |us: &[Unevaluated]| -> Vec<String> {
            us.iter().filter(|u| !u.constant).map(|u| sx(&u.expr)).collect()
        };
        let file: Vec<(usize, String)> = p.unevaluated.iter().map(|(n, u)| (*n, sx(&u.expr))).collect();
        // The enumerator's operand not chosen, then the constant as the
        // parser resolved it, which sema checks as a constant.
        assert_eq!(file, [(4, "g".to_string()), (4, "(? 0 0 1)".to_string())]);
        assert!(p.unevaluated[1].1.constant && !p.unevaluated[0].1.constant);
        let Item::Decl(a) = &p.items[2] else { panic!() };
        assert_eq!(list(&a[0].sized), ["(call f)"]);
        let Item::Decl(b) = &p.items[3] else { panic!() };
        assert_eq!(list(&b[0].sized), ["g"]);
        let h = funcs(&p)[1];
        assert_eq!(list(&h.unevaluated), ["n"]);
        let params: Vec<&str> = h.unevaluated[0].params.iter().map(|(n, ..)| n.as_str()).collect();
        assert_eq!(params, ["n"]);
        let body = h.body.as_ref().unwrap();
        let StmtKind::Decl(cd) = &body[1].kind else {
            panic!()
        };
        assert_eq!(
            (list(&cd[0].sized), list(&cd[0].unevaluated)),
            (vec!["x".to_string()], vec![])
        );
        assert_eq!(list(&cd[1].unevaluated), ["c"]);
        let StmtKind::For { unevaluated, .. } = &body[3].kind else {
            panic!()
        };
        assert_eq!(list(unevaluated), ["i"]);
        let body: Vec<String> = body.iter().map(ss).collect();
        assert_eq!(
            body,
            [
                "(decl int x)",
                "(decl char c[4]={0}, char d=(sizeof char[4]))",
                // The assertion's operand not chosen, and its condition.
                "(unevaluated (call f) (? 1 1 0))",
                "(for (decl int i=0) (< i (sizeof char[4])) (i ++) (x ++);)",
                "(if (sizeof char[4]) {(decl int g=0) (= x g);})",
                "(unevaluated g)",
                "(return (* (+ c 0)))"
            ]
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
                "char g[8]",
                "char h[4]",
                "char i[8]",
                "char j[10]",
                "char k[8]",
                "char l[8]",
                "char m[8]",
            ]
        );
        // Innermost declaration wins; parameters are pointers; an array
        // sized by its initializer, or by an earlier declaration.
        let p = prog("int x; int f(char x, int v[10]) { char a[sizeof x]; char b[sizeof v]; return 0; }");
        let body = format!("{:?}", funcs(&p)[0].body);
        assert!(body.contains("Array(Int(Char), Some(1))"), "{body}");
        assert!(body.contains("Array(Int(Char), Some(8))"), "{body}");
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

        // A call is of what its function returns, an operator's result of
        // the type sema gives it.
        assert_eq!(
            globals(
                "int f(void); long long g(int); char a[sizeof f()];
                 char b[sizeof g(1) + sizeof(1 ? 'c' : 2u) + sizeof(f() < 2) + sizeof(-(char)1)];"
            ),
            ["char a[4]", "char b[20]"]
        );

        // An array sized by designators (`[3] = 1` makes 4 elements).
        assert_eq!(
            globals(
                "int t[] = {[3] = 1}; char a[sizeof t]; int u[] = {[1] = 1, 2, [0] = 3}; char b[sizeof u];"
            )
            .iter()
            .filter(|d| d.starts_with("char"))
            .collect::<Vec<_>>(),
            ["char a[16]", "char b[12]"]
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
        // GNU's `[0] 1`, clang's warning, and an empty list (C23).
        assert_eq!(globals("int a[2] = {[1] 3};"), ["int a[2]={[1]=3}"]);
        assert_eq!(globals("int b[2] = {};"), ["int b[2]={}"]);
        // A function declared among variables is an item of its own.
        let p = prog("int x, f(int), y;");
        assert_eq!(globals("int x, f(int), y;"), ["int x", "int y"]);
        assert_eq!(funcs(&p)[0].name, "f");
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
        // `extern` and functions in a block declare what is outside it.
        assert_eq!(
            body("extern int x; int g(int), y;")
                .iter()
                .map(ss)
                .collect::<Vec<_>>(),
            ["(decl extern int x)", "(decl int g(int), int y)"]
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
            ss(&stmt("for (static int i = 0;;) ;")),
            "(for (decl static int i=0) _ _ ;)"
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
            ["int", "unsigned int", "unsigned int", "int", "long", "char[4]"]
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
        let src = ret(&nest(MAX_DEPTH - 2, "(", "1", ")"));
        assert!(parse(&lex(&src).unwrap()).is_ok());
        limit(&ret(&nest(MAX_DEPTH - 1, "(", "1", ")")));
        limit(&ret(&nest(100_000, "(", "1", ")")));
        limit(&ret(&nest(100_000, "- ", "1", "")));
        // Declarators: as many `*`s, or `[]`s, as the limit allows, and no more.
        let stars = |n: usize| format!("int {}p;", "*".repeat(n));
        let dims = |n: usize| format!("char a{};", "[1]".repeat(n));
        assert!(parse(&lex(&stars(MAX_DEPTH)).unwrap()).is_ok());
        assert!(parse(&lex(&dims(MAX_DEPTH)).unwrap()).is_ok());
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
        assert!(parse(&lex(&src).unwrap()).is_ok());
        let args = vec!["1"; MAX_PARAMS + 1].join(", ");
        let src = ret(&format!("f({args})"));
        let e = parse(&lex(&src).unwrap()).unwrap_err();
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
    fn too_many_parameters() {
        let params: Vec<String> = (0..128).map(|i| format!("int p{i}")).collect();
        let e = err(&format!("int f({}) {{}}", params.join(", ")));
        assert!(
            e.ends_with("too many parameters: merlin supports at most 127"),
            "{e}"
        );
    }

    #[test]
    fn static_assertions() {
        assert!(
            prog(
                "_Static_assert(sizeof(int) == 4, \"LP64\"); enum { N = 3 }; \
                      int f() { _Static_assert(N * 2 == 6, \"in a \" \"block\"); return 0; }"
            )
            .items
            .len()
                == 1
        );
        // C23's assertion without a message, which clang takes with a warning.
        let p = prog("_Static_assert(1);");
        assert_eq!(p.warnings[0].flag, Some("c23-extensions"));
        // offsetof, as <stddef.h> spells it: the member's offset, which sema
        // has not filled in yet when the parser evaluates the assertion.
        let offsetof = "struct s { char c; int n; struct { short a, b; } in; }; \
                        _Static_assert((unsigned long)&((struct s *)0)->n == 4, \"n\"); \
                        _Static_assert((unsigned long)&((struct s *)0)->in.b == 10, \"in.b\"); \
                        _Static_assert((unsigned long)&(*(struct s *)0).in == 8, \"in\");";
        assert_eq!(prog(offsetof).items.len(), 0);
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
            "(= p (+ (* (+ (literal int[] {1 2}) 1)) (sizeof (literal char[3] {0}))))"
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
    }
}
