//! Variables and types, from the program's DWARF debugging information entries
//! (`.debug_info`, through [`elf::dies`]): what `print`, `info locals`, `info
//! args`, `ptype`, `whatis` and the arguments of a backtrace show.
//!
//! A function (`DW_TAG_subprogram`) holds its parameters
//! (`DW_TAG_formal_parameter`), its variables (`DW_TAG_variable`) and its blocks
//! (`DW_TAG_lexical_block`), each block with an address range and variables of its
//! own. The variables in scope at a pc are those of the blocks whose ranges hold
//! it, the innermost first, then the function's, then the file's and every other
//! file's globals: C's scopes, as GDB looks names up.
//!
//! Where a variable lives is a DWARF expression (`DW_AT_location`). merlin's are
//! `DW_OP_fbreg N`, N bytes from the function's frame base (`DW_AT_frame_base`,
//! `DW_OP_reg8`: `s0`, the frame pointer), or `DW_OP_addr A` for a global or a
//! `static` local; clang's at -O0 are `DW_OP_breg2 N` (from `sp`) and `DW_OP_addrx
//! I` (an address in `.debug_addr`). [`Frame`] gives the registers a frame has: all
//! of them in the innermost one; in a caller its `s0` (the frame pointer the
//! unwinder found) and its `sp`, which is the CFA of the frame it called (the `sp`
//! of the call).
//!
//! Values are shown as GDB shows them for C: integers in decimal, a `char` as
//! `65 'A'`, an enum by its constant's name, a pointer in hex with the symbol it
//! points into (`0x80001018 <nodes+16>`), a `char *` with its string, structs and
//! unions as `{x = 1, y = 2}`, arrays as `{1, 2, 3}` (an array of chars as a
//! string), runs of ten or more equal elements as `<repeats N times>`, and at most
//! 200 elements. `print` also gives a pointer's type, `(Node *) 0x80001018 <nodes+16>`.
//! Types are written as C declares them (`Node *`, `int [3]`, `int (*)(int)`).

use crate::symbols::Symbols;
use elf::dies::{dw, Die, InfoUnit};
use excalibur::Xlen;

type Result<T> = std::result::Result<T, String>;

/// Reads the machine's memory.
pub trait Target {
    /// `len` bytes at `addr`, or an error if they cannot all be read.
    fn read(&mut self, addr: u64, len: usize) -> Result<Vec<u8>>;
}

impl Target for crate::remote::Remote {
    fn read(&mut self, addr: u64, len: usize) -> Result<Vec<u8>> {
        let b = self.read_memory(addr, len)?;
        if b.len() < len {
            return Err(format!(
                "cannot access memory at address {:#x}",
                addr + b.len() as u64
            ));
        }
        Ok(b)
    }
}

/// An entry of a loaded program's debugging information: the program (in the order
/// they were added), its unit and the entry's index there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DieRef {
    file: usize,
    unit: usize,
    die: usize,
}

/// The debugging information of every program loaded.
#[derive(Default)]
pub struct Debug {
    files: Vec<Vec<InfoUnit>>,
}

/// A frame's registers, as far as they are known, and the pc its scopes are looked
/// up at: a caller's is the call's (its return address less one, as GDB takes it),
/// which is in the block of the call.
#[derive(Debug, Clone)]
pub struct Frame {
    pub pc: u64,
    pub regs: [Option<u64>; 32],
}

const S0: usize = 8;

/// A type: one of the program's, or one an expression makes (`&x` of a sub-array,
/// a number, a difference of pointers).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ty {
    /// A type's entry (none: `void`).
    Die(Option<DieRef>),
    /// An array of the element type with these dimensions, outermost first: what
    /// one subrange less of a multidimensional array is.
    Array(Option<DieRef>, Vec<Option<u64>>),
    /// A pointer to the type.
    Ptr(Box<Ty>),
    /// An integer of `size` bytes: `int`, or `long` (4 or 8 bytes) for a result
    /// as wide as an address.
    Int { signed: bool, size: u64 },
}

/// What a type is, once typedefs, `const` and `volatile` are seen through.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Shape {
    Void,
    /// An integer: its size, whether it is signed, and whether it is a character
    /// type or `_Bool`.
    Int {
        size: u64,
        signed: bool,
        chr: bool,
        boolean: bool,
    },
    Enum(DieRef, u64),
    Ptr(Ty),
    /// An array of the element type, with its count (none: unknown).
    Array(Ty, Option<u64>),
    Record(DieRef),
    Func(DieRef),
}

/// Where a value is.
#[derive(Debug, Clone)]
enum Loc {
    Mem(u64),
    Bytes(Vec<u8>),
}

/// A value of a type.
#[derive(Debug, Clone)]
pub struct Value {
    ty: Ty,
    loc: Loc,
}

impl Debug {
    /// Adds a program's debugging information (none if it has no `.debug_info`).
    pub fn add_elf(&mut self, file: &[u8]) -> Result<()> {
        let elf = elf::Elf::parse(file).map_err(|e| e.to_string())?;
        let units = elf.debug_entries().map_err(|e| e.to_string())?;
        self.files.push(units);
        Ok(())
    }

    fn die(&self, r: DieRef) -> &Die {
        &self.files[r.file][r.unit].dies[r.die]
    }

    /// The entry at `offset` of a program's `.debug_info`.
    fn at_offset(&self, file: usize, offset: u64) -> Option<DieRef> {
        let units = &self.files[file];
        let unit = units.partition_point(|u| u.offset <= offset).checked_sub(1)?;
        let die = units[unit].index_of(offset)?;
        Some(DieRef { file, unit, die })
    }

    /// The entry attribute `at` of `r` refers to.
    fn reference(&self, r: DieRef, at: u64) -> Option<DieRef> {
        self.at_offset(r.file, self.die(r).reference(at)?)
    }

    /// The type of a variable, member or type entry (none: `void`).
    fn type_of(&self, r: DieRef) -> Option<DieRef> {
        self.reference(r, dw::AT_TYPE)
    }

    fn children(&self, r: DieRef) -> impl Iterator<Item = DieRef> + '_ {
        self.die(r).children.iter().map(move |&die| DieRef { die, ..r })
    }

    /// Every entry of every program, as a reference.
    fn all(&self) -> impl Iterator<Item = DieRef> + '_ {
        self.files.iter().enumerate().flat_map(|(file, units)| {
            units
                .iter()
                .enumerate()
                .flat_map(move |(unit, u)| (0..u.dies.len()).map(move |die| DieRef { file, unit, die }))
        })
    }

    /// The function whose code holds `pc`.
    pub fn function_at(&self, pc: u64) -> Option<DieRef> {
        self.all().find(|&r| {
            let d = self.die(r);
            d.tag == dw::TAG_SUBPROGRAM && d.ranges.iter().any(|&(lo, hi)| lo <= pc && pc < hi)
        })
    }

    /// A function's name.
    pub fn name(&self, r: DieRef) -> Option<&str> {
        self.die(r).name()
    }

    /// A function's parameters.
    pub fn params(&self, func: DieRef) -> Vec<DieRef> {
        self.children(func)
            .filter(|&c| self.die(c).tag == dw::TAG_FORMAL_PARAMETER)
            .collect()
    }

    /// The variables in scope at `pc` in `func`, block by block, the innermost
    /// first and the function's own last (its parameters left out).
    pub fn locals(&self, func: DieRef, pc: u64) -> Vec<Vec<DieRef>> {
        let mut blocks = Vec::new();
        let mut scope = Some(func);
        while let Some(s) = scope {
            let vars = self
                .children(s)
                .filter(|&c| self.die(c).tag == dw::TAG_VARIABLE)
                .collect();
            blocks.push(vars);
            scope = self.children(s).find(|&c| {
                let d = self.die(c);
                d.tag == dw::TAG_LEXICAL_BLOCK && d.ranges.iter().any(|&(lo, hi)| lo <= pc && pc < hi)
            });
        }
        blocks.reverse();
        blocks
    }

    /// A variable of the file's scope named `name`: of the unit holding `pc`
    /// first, then of any unit. A definition (with a location) wins over a
    /// declaration.
    fn global(&self, name: &str, pc: Option<u64>) -> Option<DieRef> {
        let candidates: Vec<DieRef> = self
            .all()
            .filter(|&r| {
                let d = self.die(r);
                d.tag == dw::TAG_VARIABLE
                    && d.parent == Some(0)
                    && d.name() == Some(name)
                    && d.attr(dw::AT_LOCATION).is_some()
            })
            .collect();
        let home = pc.and_then(|pc| self.function_at(pc));
        candidates
            .iter()
            .find(|r| home.is_some_and(|h| h.file == r.file && h.unit == r.unit))
            .or(candidates.first())
            .copied()
    }

    /// A function of any unit named `name`.
    fn function_named(&self, name: &str) -> Option<DieRef> {
        self.all().find(|&r| {
            let d = self.die(r);
            d.tag == dw::TAG_SUBPROGRAM && d.name() == Some(name) && !d.ranges.is_empty()
        })
    }

    /// A type named as C names it: `struct node`, `union word`, `enum color`, a
    /// typedef name or a base type.
    pub fn type_named(&self, name: &str) -> Option<Ty> {
        let name = name.trim();
        let (tags, bare): (&[u64], &str) = if let Some(n) = name.strip_prefix("struct ") {
            (&[dw::TAG_STRUCTURE_TYPE], n.trim())
        } else if let Some(n) = name.strip_prefix("union ") {
            (&[dw::TAG_UNION_TYPE], n.trim())
        } else if let Some(n) = name.strip_prefix("enum ") {
            (&[dw::TAG_ENUMERATION_TYPE], n.trim())
        } else {
            (&[dw::TAG_TYPEDEF, dw::TAG_BASE_TYPE], name)
        };
        // A definition wins over a declaration (an incomplete struct).
        let found: Vec<DieRef> = self
            .all()
            .filter(|&r| {
                let d = self.die(r);
                tags.contains(&d.tag) && d.name() == Some(bare)
            })
            .collect();
        found
            .iter()
            .find(|&&r| !self.die(r).flag(dw::AT_DECLARATION))
            .or(found.first())
            .map(|&r| Ty::Die(Some(r)))
    }

    // ---- types ----

    /// Through typedefs, `const` and `volatile`.
    /// (A chain longer than any real one is a loop in broken information: it
    /// ends as `void`.)
    fn strip(&self, mut t: Option<DieRef>) -> Option<DieRef> {
        for _ in 0..64 {
            let r = t?;
            match self.die(r).tag {
                dw::TAG_TYPEDEF | dw::TAG_CONST_TYPE | dw::TAG_VOLATILE_TYPE | dw::TAG_RESTRICT_TYPE => {
                    t = self.type_of(r)
                }
                _ => return Some(r),
            }
        }
        None
    }

    /// An array type's element type and dimensions.
    fn dims(&self, r: DieRef) -> (Option<DieRef>, Vec<Option<u64>>) {
        let dims = self
            .children(r)
            .filter(|&c| self.die(c).tag == dw::TAG_SUBRANGE_TYPE)
            .map(|c| {
                let d = self.die(c);
                d.unsigned(dw::AT_COUNT).or_else(|| {
                    d.attr(dw::AT_UPPER_BOUND)
                        .and_then(|v| v.signed())
                        .map(|u| (u + 1) as u64)
                })
            })
            .collect::<Vec<_>>();
        let dims = if dims.is_empty() { vec![None] } else { dims };
        (self.type_of(r), dims)
    }

    fn shape(&self, t: &Ty) -> Shape {
        match t {
            Ty::Int { signed, size } => Shape::Int {
                size: *size,
                signed: *signed,
                chr: false,
                boolean: false,
            },
            Ty::Ptr(to) => Shape::Ptr((**to).clone()),
            Ty::Array(elem, dims) => self.array_shape(*elem, dims),
            Ty::Die(t) => {
                let Some(r) = self.strip(*t) else {
                    return Shape::Void;
                };
                let d = self.die(r);
                match d.tag {
                    dw::TAG_BASE_TYPE => {
                        let enc = d.unsigned(dw::AT_ENCODING).unwrap_or(dw::ATE_SIGNED);
                        Shape::Int {
                            size: d.unsigned(dw::AT_BYTE_SIZE).unwrap_or(4),
                            signed: matches!(enc, dw::ATE_SIGNED | dw::ATE_SIGNED_CHAR),
                            chr: matches!(enc, dw::ATE_SIGNED_CHAR | dw::ATE_UNSIGNED_CHAR),
                            boolean: enc == dw::ATE_BOOLEAN,
                        }
                    }
                    dw::TAG_ENUMERATION_TYPE => Shape::Enum(r, d.unsigned(dw::AT_BYTE_SIZE).unwrap_or(4)),
                    dw::TAG_POINTER_TYPE => Shape::Ptr(Ty::Die(self.type_of(r))),
                    dw::TAG_ARRAY_TYPE => {
                        let (elem, dims) = self.dims(r);
                        self.array_shape(elem, &dims)
                    }
                    dw::TAG_STRUCTURE_TYPE | dw::TAG_UNION_TYPE => Shape::Record(r),
                    dw::TAG_SUBROUTINE_TYPE | dw::TAG_SUBPROGRAM => Shape::Func(r),
                    _ => Shape::Void,
                }
            }
        }
    }

    fn array_shape(&self, elem: Option<DieRef>, dims: &[Option<u64>]) -> Shape {
        let inner = if dims.len() > 1 {
            Ty::Array(elem, dims[1..].to_vec())
        } else {
            Ty::Die(elem)
        };
        Shape::Array(inner, dims.first().copied().flatten())
    }

    /// The size of a type in bytes (none: unknown, as of `void` or an incomplete
    /// struct).
    fn size(&self, t: &Ty, xlen: Xlen) -> Option<u64> {
        match self.shape(t) {
            Shape::Void | Shape::Func(_) => None,
            Shape::Int { size, .. } | Shape::Enum(_, size) => Some(size),
            Shape::Ptr(_) => Some(u64::from(xlen.bits() / 8)),
            Shape::Array(elem, n) => Some(self.size(&elem, xlen)? * n?),
            Shape::Record(r) => self.die(r).unsigned(dw::AT_BYTE_SIZE),
        }
    }

    /// The type as C declares something of it named `inner` (empty: the type's
    /// own name, as GDB writes it: `Node *`, `int [3]`, `int (*)(int)`).
    pub fn declare(&self, t: &Ty, inner: &str) -> String {
        let join = |base: &str, inner: &str| {
            if inner.is_empty() {
                base.to_string()
            } else {
                format!("{base} {inner}")
            }
        };
        let paren = |inner: &str| {
            if inner.starts_with('*') {
                format!("({inner})")
            } else {
                inner.to_string()
            }
        };
        let dims = |dims: &[Option<u64>]| -> String {
            dims.iter()
                .map(|n| n.map_or("[]".to_string(), |n| format!("[{n}]")))
                .collect()
        };
        match t {
            Ty::Int { signed, size } => {
                let name = match (signed, size) {
                    (true, 4) => "int",
                    (false, 4) => "unsigned int",
                    (true, _) => "long",
                    (false, _) => "unsigned long",
                };
                join(name, inner)
            }
            Ty::Ptr(to) => self.declare(to, &format!("*{inner}")),
            Ty::Array(elem, d) => self.declare(&Ty::Die(*elem), &format!("{}{}", paren(inner), dims(d))),
            Ty::Die(None) => join("void", inner),
            Ty::Die(Some(r)) => {
                let d = self.die(r.to_owned());
                let named = |kw: &str| match d.name() {
                    Some(n) if kw.is_empty() => n.to_string(),
                    Some(n) => format!("{kw} {n}"),
                    None if kw.is_empty() => "?".to_string(),
                    None => format!("{kw} {{...}}"),
                };
                match d.tag {
                    dw::TAG_BASE_TYPE | dw::TAG_TYPEDEF => join(&named(""), inner),
                    dw::TAG_STRUCTURE_TYPE => join(&named("struct"), inner),
                    dw::TAG_UNION_TYPE => join(&named("union"), inner),
                    dw::TAG_ENUMERATION_TYPE => join(&named("enum"), inner),
                    dw::TAG_POINTER_TYPE => self.declare(&Ty::Die(self.type_of(*r)), &format!("*{inner}")),
                    dw::TAG_CONST_TYPE | dw::TAG_VOLATILE_TYPE => {
                        let q = if d.tag == dw::TAG_CONST_TYPE {
                            "const"
                        } else {
                            "volatile"
                        };
                        let of = self.type_of(*r);
                        // `char *const p`: the qualifier follows the `*` it is on.
                        match of.map(|o| self.die(o).tag) {
                            Some(dw::TAG_POINTER_TYPE) => {
                                let target = self.type_of(of.unwrap());
                                self.declare(&Ty::Die(target), &format!("* {}", join(q, inner)))
                            }
                            _ => format!("{q} {}", self.declare(&Ty::Die(of), inner)),
                        }
                    }
                    dw::TAG_ARRAY_TYPE => {
                        let (elem, d) = self.dims(*r);
                        self.declare(&Ty::Die(elem), &format!("{}{}", paren(inner), dims(&d)))
                    }
                    dw::TAG_SUBROUTINE_TYPE | dw::TAG_SUBPROGRAM => {
                        let mut params: Vec<String> = self
                            .children(*r)
                            .filter_map(|c| match self.die(c).tag {
                                dw::TAG_FORMAL_PARAMETER => Some(self.declare(&Ty::Die(self.type_of(c)), "")),
                                dw::TAG_UNSPECIFIED_PARAMETERS => Some("...".to_string()),
                                _ => None,
                            })
                            .collect();
                        if params.is_empty() {
                            params.push("void".into());
                        }
                        let ret = Ty::Die(self.type_of(*r));
                        self.declare(&ret, &format!("{}({})", paren(inner), params.join(", ")))
                    }
                    _ => join("?", inner),
                }
            }
        }
    }

    /// `ptype`: the type, with the members of a struct or union (or the
    /// constants of an enum) it is, points to or is an array of, as GDB's
    /// `ptype` shows them; typedefs seen through.
    pub fn ptype(&self, t: &Ty) -> String {
        // A typedef name stands for its type (`ptype Node` is the struct).
        let mut base = t.clone();
        while let Ty::Die(Some(r)) = base {
            if self.die(r).tag != dw::TAG_TYPEDEF {
                break;
            }
            base = Ty::Die(self.type_of(r));
        }
        let t = &base;
        // The struct, union or enum at the bottom of the declarator.
        let mut bottom = t.clone();
        loop {
            bottom = match &bottom {
                Ty::Ptr(to) => (**to).clone(),
                Ty::Array(e, _) => Ty::Die(*e),
                Ty::Die(Some(r)) => match self.die(*r).tag {
                    dw::TAG_POINTER_TYPE
                    | dw::TAG_ARRAY_TYPE
                    | dw::TAG_CONST_TYPE
                    | dw::TAG_VOLATILE_TYPE => Ty::Die(self.type_of(*r)),
                    _ => break,
                },
                _ => break,
            };
        }
        // As the declarator names it (a typedef, say), and what that stands for.
        let named = bottom.clone();
        let Ty::Die(Some(r)) = bottom else {
            return self.declare(t, "");
        };
        let Some(r) = self.strip(Some(r)) else {
            return self.declare(t, "");
        };
        let d = self.die(r);
        let body = match d.tag {
            dw::TAG_STRUCTURE_TYPE | dw::TAG_UNION_TYPE if !d.flag(dw::AT_DECLARATION) => {
                let mut s = String::from("{\n");
                for m in self.children(r).filter(|&c| self.die(c).tag == dw::TAG_MEMBER) {
                    let name = self.die(m).name().unwrap_or("");
                    s += &format!("    {};\n", self.declare(&Ty::Die(self.type_of(m)), name));
                }
                s + "}"
            }
            dw::TAG_ENUMERATION_TYPE => {
                let mut next = 0i64;
                let constants: Vec<String> = self
                    .children(r)
                    .filter(|&c| self.die(c).tag == dw::TAG_ENUMERATOR)
                    .map(|c| {
                        let e = self.die(c);
                        let v = e.attr(dw::AT_CONST_VALUE).and_then(|v| v.signed()).unwrap_or(0);
                        let name = e.name().unwrap_or("?");
                        let s = if v == next {
                            name.to_string()
                        } else {
                            format!("{name} = {v}")
                        };
                        next = v + 1;
                        s
                    })
                    .collect();
                format!("{{{}}}", constants.join(", "))
            }
            _ => return self.declare(t, ""),
        };
        // `struct node {...} *`: the body where the name was (`Node *` names it
        // `Node`), the rest of the declarator around it.
        let name = self.declare(&Ty::Die(Some(r)), "");
        let whole = self.declare(t, "");
        let spelled = self.declare(&named, "");
        whole.replacen(&spelled, &format!("{name} {body}"), 1)
    }

    /// `whatis` of a type name: what it stands for, one typedef level down, as GDB
    /// has it.
    pub fn whatis_type(&self, t: &Ty) -> String {
        match t {
            Ty::Die(Some(r)) if self.die(*r).tag == dw::TAG_TYPEDEF => {
                self.declare(&Ty::Die(self.type_of(*r)), "")
            }
            _ => self.declare(t, ""),
        }
    }
}

// ---- expressions ----

/// A token of an expression.
#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Num(u64),
    Ident(String),
    Reg(String),
    Char(u8),
    Punct(&'static str),
}

fn tokens(s: &str) -> Result<Vec<Tok>> {
    const PUNCTS: [&str; 15] = [
        "->", "(", ")", "[", "]", ".", "*", "&", "+", "-", "/", "%", "~", "!", ",",
    ];
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c.is_ascii_whitespace() {
            i += 1;
        } else if c.is_ascii_digit() {
            let start = i;
            while i < b.len() && (b[i].is_ascii_alphanumeric()) {
                i += 1;
            }
            let text = &s[start..i];
            let digits = text.trim_end_matches(['u', 'U', 'l', 'L']);
            let n = if let Some(h) = digits.strip_prefix("0x").or(digits.strip_prefix("0X")) {
                u64::from_str_radix(h, 16)
            } else if digits.len() > 1 && digits.starts_with('0') {
                u64::from_str_radix(&digits[1..], 8)
            } else {
                digits.parse()
            };
            out.push(Tok::Num(n.map_err(|_| format!("invalid number \"{text}\""))?));
        } else if c.is_ascii_alphabetic() || c == b'_' || c == b'$' {
            let start = i;
            i += 1;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                i += 1;
            }
            let word = &s[start..i];
            out.push(match word.strip_prefix('$') {
                Some(r) => Tok::Reg(r.to_string()),
                None => Tok::Ident(word.to_string()),
            });
        } else if c == b'\'' {
            let (ch, len) = match (b.get(i + 1), b.get(i + 2)) {
                (Some(b'\\'), Some(&e)) => (
                    match e {
                        b'n' => b'\n',
                        b't' => b'\t',
                        b'r' => b'\r',
                        b'0' => 0,
                        e => e,
                    },
                    4,
                ),
                (Some(&ch), _) => (ch, 3),
                _ => return Err("unterminated character constant".into()),
            };
            if b.get(i + len - 1) != Some(&b'\'') {
                return Err("unterminated character constant".into());
            }
            out.push(Tok::Char(ch));
            i += len;
        } else if let Some(p) = PUNCTS.iter().find(|p| s[i..].starts_with(**p)) {
            out.push(Tok::Punct(p));
            i += p.len();
        } else {
            return Err(format!("invalid character '{}' in expression", c as char));
        }
    }
    Ok(out)
}

/// An expression as parsed: C's, the part a debugger needs.
#[derive(Debug, Clone)]
enum Expr {
    Num(u64),
    Char(u8),
    Var(String),
    Reg(String),
    Member(Box<Expr>, String),
    Arrow(Box<Expr>, String),
    Index(Box<Expr>, Box<Expr>),
    Deref(Box<Expr>),
    Addr(Box<Expr>),
    Neg(Box<Expr>),
    Not(Box<Expr>),
    Compl(Box<Expr>),
    Binary(&'static str, Box<Expr>, Box<Expr>),
    /// `sizeof x`, `sizeof (type)`.
    Sizeof(Box<Expr>),
    SizeofType(String),
    Cast(String, Box<Expr>),
}

struct Parser<'a> {
    toks: &'a [Tok],
    at: usize,
    debug: &'a Debug,
}

impl Parser<'_> {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.at)
    }
    fn eat(&mut self, p: &str) -> bool {
        if matches!(self.peek(), Some(Tok::Punct(q)) if *q == p) {
            self.at += 1;
            true
        } else {
            false
        }
    }
    fn expect(&mut self, p: &str) -> Result<()> {
        if self.eat(p) {
            Ok(())
        } else {
            Err(format!("a syntax error in the expression, near '{p}'"))
        }
    }

    /// `( type-name )` from here, if the parenthesis holds a type: a struct,
    /// union or enum, or a name that is a type and not a variable, with `*`s.
    fn type_name(&mut self) -> Option<String> {
        let save = self.at;
        let mut words = Vec::new();
        while let Some(Tok::Ident(w)) = self.peek() {
            words.push(w.clone());
            self.at += 1;
        }
        let mut stars = 0;
        while self.eat("*") {
            stars += 1;
        }
        let base = words.join(" ");
        let is_type = matches!(
            words.first().map(String::as_str),
            Some("struct" | "union" | "enum")
        ) || self.debug.type_named(&base).is_some() && self.debug.global(&base, None).is_none();
        if words.is_empty() || !is_type || !matches!(self.peek(), Some(Tok::Punct(")"))) {
            self.at = save;
            return None;
        }
        Some(format!("{base}{}", "*".repeat(stars)))
    }

    fn expr(&mut self) -> Result<Expr> {
        self.additive()
    }

    fn additive(&mut self) -> Result<Expr> {
        let mut l = self.multiplicative()?;
        loop {
            let op = if self.eat("+") {
                "+"
            } else if self.eat("-") {
                "-"
            } else {
                return Ok(l);
            };
            let r = self.multiplicative()?;
            l = Expr::Binary(op, Box::new(l), Box::new(r));
        }
    }

    fn multiplicative(&mut self) -> Result<Expr> {
        let mut l = self.unary()?;
        loop {
            let op = if self.eat("*") {
                "*"
            } else if self.eat("/") {
                "/"
            } else if self.eat("%") {
                "%"
            } else {
                return Ok(l);
            };
            let r = self.unary()?;
            l = Expr::Binary(op, Box::new(l), Box::new(r));
        }
    }

    fn unary(&mut self) -> Result<Expr> {
        if self.eat("*") {
            return Ok(Expr::Deref(Box::new(self.unary()?)));
        }
        if self.eat("&") {
            return Ok(Expr::Addr(Box::new(self.unary()?)));
        }
        if self.eat("-") {
            return Ok(Expr::Neg(Box::new(self.unary()?)));
        }
        if self.eat("!") {
            return Ok(Expr::Not(Box::new(self.unary()?)));
        }
        if self.eat("~") {
            return Ok(Expr::Compl(Box::new(self.unary()?)));
        }
        if matches!(self.peek(), Some(Tok::Ident(w)) if w == "sizeof") {
            self.at += 1;
            if self.eat("(") {
                if let Some(t) = self.type_name() {
                    self.expect(")")?;
                    return Ok(Expr::SizeofType(t));
                }
                let e = self.expr()?;
                self.expect(")")?;
                return Ok(Expr::Sizeof(Box::new(e)));
            }
            return Ok(Expr::Sizeof(Box::new(self.unary()?)));
        }
        if matches!(self.peek(), Some(Tok::Punct("("))) {
            let save = self.at;
            self.at += 1;
            if let Some(t) = self.type_name() {
                self.expect(")")?;
                return Ok(Expr::Cast(t, Box::new(self.unary()?)));
            }
            self.at = save;
        }
        self.postfix()
    }

    fn postfix(&mut self) -> Result<Expr> {
        let mut e = self.primary()?;
        loop {
            if self.eat(".") {
                e = Expr::Member(Box::new(e), self.ident()?);
            } else if self.eat("->") {
                e = Expr::Arrow(Box::new(e), self.ident()?);
            } else if self.eat("[") {
                let i = self.expr()?;
                self.expect("]")?;
                e = Expr::Index(Box::new(e), Box::new(i));
            } else {
                return Ok(e);
            }
        }
    }

    fn ident(&mut self) -> Result<String> {
        match self.peek().cloned() {
            Some(Tok::Ident(n)) => {
                self.at += 1;
                Ok(n)
            }
            _ => Err("a member name must follow '.' or '->'".into()),
        }
    }

    fn primary(&mut self) -> Result<Expr> {
        let t = self.peek().cloned();
        self.at += 1;
        match t {
            Some(Tok::Num(n)) => Ok(Expr::Num(n)),
            Some(Tok::Char(c)) => Ok(Expr::Char(c)),
            Some(Tok::Ident(n)) => Ok(Expr::Var(n)),
            Some(Tok::Reg(r)) => Ok(Expr::Reg(r)),
            Some(Tok::Punct("(")) => {
                let e = self.expr()?;
                self.expect(")")?;
                Ok(e)
            }
            _ => Err("a syntax error in the expression".into()),
        }
    }
}

/// Evaluates expressions in a frame and shows values.
pub struct Eval<'a, T: Target> {
    pub debug: &'a Debug,
    pub symbols: &'a Symbols,
    pub target: &'a mut T,
    /// The selected frame, if the machine has one with debugging information.
    pub frame: Option<Frame>,
    pub xlen: Xlen,
}

/// The `ATE`-less integer an address-sized result is: `long` (4 bytes on RV32, 8 on
/// RV64), signed or not.
fn long(xlen: Xlen, signed: bool) -> Ty {
    Ty::Int {
        signed,
        size: u64::from(xlen.bits() / 8),
    }
}

const INT: Ty = Ty::Int {
    signed: true,
    size: 4,
};

/// How many elements of an array are shown, as GDB's `print elements`.
const ELEMENTS: usize = 200;
/// A run of this many equal elements is shown once, as GDB's `print repeats`.
const REPEATS: usize = 10;

impl<T: Target> Eval<'_, T> {
    /// Parses `text` as an expression.
    fn parse(&self, text: &str) -> Result<Expr> {
        let toks = tokens(text)?;
        if toks.is_empty() {
            return Err("an expression is needed".into());
        }
        let mut p = Parser {
            toks: &toks,
            at: 0,
            debug: self.debug,
        };
        let e = p.expr()?;
        if p.at != toks.len() {
            return Err("a syntax error in the expression".into());
        }
        Ok(e)
    }

    /// The value of the expression `text`.
    pub fn value(&mut self, text: &str) -> Result<Value> {
        let e = self.parse(text)?;
        self.eval(&e)
    }

    /// `text` as a type name (`struct node`, `Node *`), if it is one.
    pub fn type_name(&self, text: &str) -> Option<Ty> {
        let text = text.trim();
        let stars = text.len() - text.trim_end_matches(['*', ' ']).len();
        let base = text.trim_end_matches(['*', ' ']);
        let stars = text[base.len()..].matches('*').count().min(stars);
        if self.debug.global(base, None).is_some() && !base.contains(' ') {
            return None;
        }
        let mut t = self.debug.type_named(base)?;
        for _ in 0..stars {
            t = Ty::Ptr(Box::new(t));
        }
        Some(t)
    }

    fn word(&self) -> u64 {
        u64::from(self.xlen.bits() / 8)
    }

    fn bytes(&mut self, v: &Value) -> Result<Vec<u8>> {
        match &v.loc {
            Loc::Bytes(b) => Ok(b.clone()),
            Loc::Mem(a) => {
                let n = self
                    .debug
                    .size(&v.ty, self.xlen)
                    .ok_or("a value of unknown size")?;
                if n > 1 << 20 {
                    return Err(format!("a value of {n} bytes is too big to show"));
                }
                self.target.read(*a, n as usize)
            }
        }
    }

    /// An integer or pointer value as a number (sign-extended if signed).
    fn number(&mut self, v: &Value) -> Result<i128> {
        let shape = self.debug.shape(&v.ty);
        let (size, signed) = match shape {
            Shape::Int { size, signed, .. } => (size, signed),
            Shape::Enum(_, size) => (size, false),
            Shape::Ptr(_) => (self.word(), false),
            _ => return Err("not a number".into()),
        };
        let b = self.bytes(v)?;
        Ok(from_le(&b[..size as usize], signed))
    }

    fn int_value(&self, ty: Ty, n: i128) -> Value {
        let size = self.debug.size(&ty, self.xlen).unwrap_or(4) as usize;
        Value {
            ty,
            loc: Loc::Bytes(n.to_le_bytes()[..size].to_vec()),
        }
    }

    /// The location of a variable's entry in the selected frame.
    fn var_location(&mut self, var: DieRef) -> Result<u64> {
        let d = self.debug.die(var);
        let name = d.name().unwrap_or("?").to_string();
        let Some(expr) = d.block(dw::AT_LOCATION) else {
            return Err(format!("'{name}' has no location (optimized out)"));
        };
        let expr = expr.to_vec();
        let pool = &self.debug.files[var.file][var.unit].addr_pool;
        let frame = self.frame.clone();
        let reg = |n: usize| -> Result<u64> {
            frame
                .as_ref()
                .and_then(|f| f.regs[n])
                .ok_or_else(|| format!("'{name}': register x{n} of this frame is not known"))
        };
        let mut stack: Vec<u64> = Vec::new();
        let mut i = 0;
        let mut r = Reader { b: &expr, at: &mut i };
        while let Some(op) = r.u8() {
            match op {
                dw::OP_ADDR => stack.push(r.uint(self.word() as usize).ok_or("a truncated expression")?),
                0xa1 => {
                    // DW_OP_addrx
                    let n = r.uleb().ok_or("a truncated expression")?;
                    stack.push(*pool.get(n as usize).ok_or("DW_OP_addrx past the address pool")?);
                }
                dw::OP_FBREG => {
                    let off = r.sleb().ok_or("a truncated expression")?;
                    let base = self.frame_base(var)?;
                    stack.push(base.wrapping_add(off as u64));
                }
                dw::OP_BREG0..=dw::OP_BREG31 => {
                    let off = r.sleb().ok_or("a truncated expression")?;
                    stack.push(reg(usize::from(op - dw::OP_BREG0))?.wrapping_add(off as u64));
                }
                dw::OP_PLUS_UCONST => {
                    let n = r.uleb().ok_or("a truncated expression")?;
                    let top = stack.pop().ok_or("an empty expression stack")?;
                    stack.push(top.wrapping_add(n));
                }
                _ => return Err(format!("'{name}': DWARF operation {op:#x} is not supported")),
            }
        }
        stack
            .pop()
            .map(|a| crate::remote::wrap(self.xlen, a))
            .ok_or_else(|| format!("'{name}' has an empty location"))
    }

    /// The frame base of the function holding `var` (`DW_AT_frame_base`).
    fn frame_base(&self, var: DieRef) -> Result<u64> {
        let mut f = Some(var);
        while let Some(r) = f {
            if self.debug.die(r).tag == dw::TAG_SUBPROGRAM {
                break;
            }
            f = self.debug.die(r).parent.map(|die| DieRef { die, ..r });
        }
        let func = f.ok_or("a variable outside every function")?;
        let base = self
            .debug
            .die(func)
            .block(dw::AT_FRAME_BASE)
            .ok_or("no frame base")?;
        let frame = self.frame.as_ref().ok_or("no frame selected")?;
        let reg = |n: usize| frame.regs[n].ok_or_else(|| format!("register x{n} of this frame is not known"));
        match base.first().copied() {
            Some(op @ dw::OP_REG0..=dw::OP_REG31) => reg(usize::from(op - dw::OP_REG0)),
            Some(op @ dw::OP_BREG0..=dw::OP_BREG31) => {
                let mut i = 1;
                let off = Reader { b: base, at: &mut i }.sleb().unwrap_or(0);
                Ok(reg(usize::from(op - dw::OP_BREG0))?.wrapping_add(off as u64))
            }
            // The CFA: in merlin's and clang's frames, where s0 points.
            Some(dw::OP_CALL_FRAME_CFA) => reg(S0),
            _ => Err("a frame base that is not supported".into()),
        }
    }

    /// The variable `name` in the selected frame's scope, or else of the file's.
    fn lookup(&mut self, name: &str) -> Result<Value> {
        if let Some(f) = self.frame.clone() {
            if let Some(func) = self.debug.function_at(f.pc) {
                let blocks = self.debug.locals(func, f.pc);
                let mut scopes = blocks;
                scopes.insert(scopes.len(), self.debug.params(func));
                for vars in scopes {
                    if let Some(&v) = vars.iter().find(|&&v| self.debug.die(v).name() == Some(name)) {
                        return self.var_value(v);
                    }
                }
            }
        }
        if let Some(v) = self.debug.global(name, self.frame.as_ref().map(|f| f.pc)) {
            return self.var_value(v);
        }
        if let Some(f) = self.debug.function_named(name) {
            let lo = self.debug.die(f).ranges[0].0;
            return Ok(Value {
                ty: Ty::Die(Some(f)),
                loc: Loc::Mem(lo),
            });
        }
        if self.symbols.lookup(name).is_some() {
            return Err(format!("'{name}' has unknown type; cast it to its declared type"));
        }
        Err(format!("no symbol \"{name}\" in current context"))
    }

    /// A variable's value: where it is, of its type.
    pub fn var_value(&mut self, var: DieRef) -> Result<Value> {
        let at = self.var_location(var)?;
        Ok(Value {
            ty: Ty::Die(self.debug.type_of(var)),
            loc: Loc::Mem(at),
        })
    }

    fn eval(&mut self, e: &Expr) -> Result<Value> {
        match e {
            Expr::Num(n) => {
                let ty = if *n <= i32::MAX as u64 {
                    INT
                } else {
                    long(self.xlen, *n > i64::MAX as u64)
                };
                Ok(self.int_value(ty, i128::from(*n)))
            }
            Expr::Char(c) => Ok(self.int_value(INT, i128::from(*c))),
            Expr::Var(n) => self.lookup(n),
            Expr::Reg(r) => {
                let n = crate::session::register_number(r, self.xlen)
                    .ok_or_else(|| format!("no register ${r}"))?;
                let v = match n {
                    0..=31 => self.frame.as_ref().and_then(|f| f.regs[n]),
                    _ => None,
                };
                let v = v.ok_or_else(|| format!("${r} is not available here"))?;
                Ok(self.int_value(long(self.xlen, true), i128::from(v as i64)))
            }
            Expr::Member(base, m) => {
                let v = self.eval(base)?;
                self.member(v, m)
            }
            Expr::Arrow(base, m) => {
                let v = self.eval(base)?;
                let v = self.deref(v)?;
                self.member(v, m)
            }
            Expr::Index(base, i) => {
                let b = self.eval(base)?;
                let i = self.eval(i)?;
                let i = self.number(&i)?;
                let p = self.decay(b)?;
                let p = self.offset(p, i)?;
                self.deref(p)
            }
            Expr::Deref(e) => {
                let v = self.eval(e)?;
                let v = self.decay(v)?;
                self.deref(v)
            }
            Expr::Addr(e) => {
                let v = self.eval(e)?;
                match v.loc {
                    Loc::Mem(a) => {
                        let ty = Ty::Ptr(Box::new(v.ty));
                        Ok(self.int_value(ty, i128::from(a)))
                    }
                    Loc::Bytes(_) => {
                        Err("attempt to take the address of a value not located in memory".into())
                    }
                }
            }
            Expr::Neg(e) => {
                let v = self.eval(e)?;
                let ty = self.promoted(&v.ty)?;
                let n = self.number(&v)?;
                Ok(self.int_value(ty, -n))
            }
            Expr::Compl(e) => {
                let v = self.eval(e)?;
                let ty = self.promoted(&v.ty)?;
                let n = self.number(&v)?;
                Ok(self.int_value(ty, !n))
            }
            Expr::Not(e) => {
                let v = self.eval(e)?;
                let v = self.decay(v)?;
                let n = self.number(&v)?;
                Ok(self.int_value(INT, i128::from(n == 0)))
            }
            Expr::Binary(op, l, r) => {
                let l = self.eval(l)?;
                let r = self.eval(r)?;
                self.binary(op, l, r)
            }
            Expr::Sizeof(e) => {
                let v = self.eval(e)?;
                let n = self
                    .debug
                    .size(&v.ty, self.xlen)
                    .ok_or("sizeof of a type of unknown size")?;
                Ok(self.int_value(long(self.xlen, false), i128::from(n)))
            }
            Expr::SizeofType(t) => {
                let t = self.type_name(t).ok_or_else(|| format!("no type named {t}"))?;
                let n = self
                    .debug
                    .size(&t, self.xlen)
                    .ok_or("sizeof of a type of unknown size")?;
                Ok(self.int_value(long(self.xlen, false), i128::from(n)))
            }
            Expr::Cast(t, e) => {
                let ty = self.type_name(t).ok_or_else(|| format!("no type named {t}"))?;
                let v = self.eval(e)?;
                let v = self.decay(v)?;
                let n = self.number(&v)?;
                Ok(self.int_value(ty, n))
            }
        }
    }
}

impl<T: Target> Eval<'_, T> {
    /// C's integer promotion of an operand's type: narrower than `int` becomes
    /// `int` (an enum `unsigned int`).
    fn promoted(&self, ty: &Ty) -> Result<Ty> {
        match self.debug.shape(ty) {
            Shape::Int { size, signed, .. } if size >= 4 => Ok(Ty::Int { signed, size }),
            Shape::Int { .. } => Ok(INT),
            Shape::Enum(..) => Ok(Ty::Int {
                signed: false,
                size: 4,
            }),
            _ => Err("an integer is needed".into()),
        }
    }

    /// An array as a pointer to its first element; anything else as it is.
    fn decay(&mut self, v: Value) -> Result<Value> {
        match self.debug.shape(&v.ty) {
            Shape::Array(elem, _) => match v.loc {
                Loc::Mem(a) => Ok(self.int_value(Ty::Ptr(Box::new(elem)), i128::from(a))),
                Loc::Bytes(_) => Err("an array not in memory".into()),
            },
            _ => Ok(v),
        }
    }

    /// `*v`.
    fn deref(&mut self, v: Value) -> Result<Value> {
        let v = self.decay(v)?;
        let Shape::Ptr(to) = self.debug.shape(&v.ty) else {
            return Err("attempt to take contents of a non-pointer value".into());
        };
        if matches!(self.debug.shape(&to), Shape::Void) {
            return Err("attempt to take contents of a non-pointer value".into());
        }
        let a = self.number(&v)? as u64;
        Ok(Value {
            ty: to,
            loc: Loc::Mem(a),
        })
    }

    /// The pointer `p` moved by `n` elements.
    fn offset(&mut self, p: Value, n: i128) -> Result<Value> {
        let Shape::Ptr(to) = self.debug.shape(&p.ty) else {
            return Err("cannot subscript something of a type that is not an array or a pointer".into());
        };
        let size = self
            .debug
            .size(&to, self.xlen)
            .ok_or("pointer arithmetic on a type of unknown size")?;
        let a = self.number(&p)?;
        let a = crate::remote::wrap(self.xlen, (a + n * i128::from(size)) as u64);
        Ok(self.int_value(p.ty, i128::from(a)))
    }

    /// Member `name` of the struct or union `v`.
    fn member(&mut self, v: Value, name: &str) -> Result<Value> {
        let Shape::Record(r) = self.debug.shape(&v.ty) else {
            return Err(format!(
                "attempt to extract a component of a value that is not a structure (it is {})",
                self.debug.declare(&v.ty, "")
            ));
        };
        let m = self
            .debug
            .children(r)
            .find(|&m| self.debug.die(m).tag == dw::TAG_MEMBER && self.debug.die(m).name() == Some(name))
            .ok_or_else(|| format!("there is no member named {name}"))?;
        let off = self
            .debug
            .die(m)
            .unsigned(dw::AT_DATA_MEMBER_LOCATION)
            .unwrap_or(0);
        let ty = Ty::Die(self.debug.type_of(m));
        let loc = match v.loc {
            Loc::Mem(a) => Loc::Mem(a.wrapping_add(off)),
            Loc::Bytes(b) => {
                let n = self.debug.size(&ty, self.xlen).unwrap_or(0) as usize;
                let off = off as usize;
                Loc::Bytes(b.get(off..off + n).ok_or("a member past its struct")?.to_vec())
            }
        };
        Ok(Value { ty, loc })
    }

    fn binary(&mut self, op: &str, l: Value, r: Value) -> Result<Value> {
        let l = self.decay(l)?;
        let r = self.decay(r)?;
        let (ls, rs) = (self.debug.shape(&l.ty), self.debug.shape(&r.ty));
        match (op, &ls, &rs) {
            ("+", Shape::Ptr(_), _) => {
                let n = self.number(&r)?;
                self.offset(l, n)
            }
            ("+", _, Shape::Ptr(_)) => {
                let n = self.number(&l)?;
                self.offset(r, n)
            }
            ("-", Shape::Ptr(a), Shape::Ptr(_)) => {
                let size = self.debug.size(a, self.xlen).unwrap_or(1).max(1);
                let d = (self.number(&l)? - self.number(&r)?) / i128::from(size);
                Ok(self.int_value(long(self.xlen, true), d))
            }
            ("-", Shape::Ptr(_), _) => {
                let n = self.number(&r)?;
                self.offset(l, -n)
            }
            _ => {
                // The usual arithmetic conversions: the wider type, unsigned if
                // either operand of that width is.
                let (lt, rt) = (self.promoted(&l.ty)?, self.promoted(&r.ty)?);
                let (Ty::Int { signed: a, size: x }, Ty::Int { signed: b, size: y }) = (&lt, &rt) else {
                    unreachable!("promoted types are integers")
                };
                let ty = match x.cmp(y) {
                    std::cmp::Ordering::Greater => lt.clone(),
                    std::cmp::Ordering::Less => rt.clone(),
                    std::cmp::Ordering::Equal => Ty::Int {
                        signed: *a && *b,
                        size: *x,
                    },
                };
                let (x, y) = (self.number(&l)?, self.number(&r)?);
                let v = match op {
                    "+" => x + y,
                    "-" => x - y,
                    "*" => x * y,
                    _ if y == 0 => return Err("division by zero".into()),
                    "/" => x / y,
                    _ => x % y,
                };
                Ok(self.int_value(ty, v))
            }
        }
    }

    // ---- showing values ----

    /// `v` as `print` shows it (`top`: with a pointer's type), integers in hex
    /// with `hex` (`print/x`).
    pub fn show(&mut self, v: &Value, top: bool, hex: bool) -> Result<String> {
        if let Shape::Func(f) = self.debug.shape(&v.ty) {
            let Loc::Mem(a) = v.loc else {
                return Err("a function not in memory".into());
            };
            let ty = self.debug.declare(&Ty::Die(Some(f)), "");
            return Ok(format!("{{{ty}}} {}", self.symbols.show_plain(a)));
        }
        let b = self.bytes(v)?;
        let addr = match v.loc {
            Loc::Mem(a) => Some(a),
            Loc::Bytes(_) => None,
        };
        Ok(self.show_bytes(&v.ty, &b, addr, top, hex))
    }

    fn show_bytes(&mut self, ty: &Ty, b: &[u8], addr: Option<u64>, top: bool, hex: bool) -> String {
        // A member past the end of its struct, in broken information.
        if self
            .debug
            .size(ty, self.xlen)
            .is_some_and(|n| (b.len() as u64) < n)
        {
            return "<unavailable>".into();
        }
        match self.debug.shape(ty) {
            Shape::Void => "void".into(),
            Shape::Int {
                size,
                signed,
                chr,
                boolean,
            } => {
                let n = from_le(&b[..size as usize], signed);
                if hex {
                    let bits = 8 * size as u32;
                    let mask = if bits >= 128 {
                        u128::MAX
                    } else {
                        (1u128 << bits) - 1
                    };
                    format!("{:#x}", (n as u128) & mask)
                } else if boolean {
                    match n {
                        0 => "false".into(),
                        1 => "true".into(),
                        n => n.to_string(),
                    }
                } else if chr {
                    format!("{n} {}", char_literal(n as u8, '\''))
                } else {
                    n.to_string()
                }
            }
            Shape::Enum(r, size) => {
                let n = from_le(&b[..size as usize], false);
                let signed_n = from_le(&b[..size as usize], true);
                if hex {
                    return format!("{n:#x}");
                }
                let name = self.debug.children(r).find_map(|c| {
                    let e = self.debug.die(c);
                    let v = e.attr(dw::AT_CONST_VALUE)?.signed()?;
                    (i128::from(v) == signed_n || i128::from(v) == n)
                        .then(|| e.name().unwrap_or("?").to_string())
                });
                name.unwrap_or_else(|| n.to_string())
            }
            Shape::Ptr(to) => {
                let a = from_le(&b[..self.word() as usize], false) as u64;
                let mut s = format!("{a:#x}");
                if hex {
                    return s;
                }
                let to_shape = self.debug.shape(&to);
                let is_char = matches!(
                    to_shape,
                    Shape::Int {
                        chr: true,
                        size: 1,
                        ..
                    }
                );
                if let Some(d) = self.symbols.describe(a) {
                    s += &format!(" <{d}>");
                }
                if is_char && a != 0 {
                    s += " ";
                    s += &self.string_at(a);
                } else if top {
                    s = format!("({}) {s}", self.debug.declare(ty, ""));
                }
                s
            }
            Shape::Array(elem, n) => {
                let Some(n) = n else {
                    return "0x0".into();
                };
                let esize = self.debug.size(&elem, self.xlen).unwrap_or(0) as usize;
                if let Shape::Int {
                    chr: true, size: 1, ..
                } = self.debug.shape(&elem)
                {
                    return quote_chars(&b[..n as usize]);
                }
                let shown = (n as usize).min(ELEMENTS);
                let items: Vec<String> = (0..shown)
                    .map(|i| {
                        let part = &b[i * esize..(i + 1) * esize];
                        let at = addr.map(|a| a + (i * esize) as u64);
                        self.show_bytes(&elem, part, at, false, hex)
                    })
                    .collect();
                let mut out = repeats(items);
                if shown < n as usize {
                    out.push("...".into());
                }
                format!("{{{}}}", out.join(", "))
            }
            Shape::Record(r) => {
                let members: Vec<(String, Ty, u64)> = self
                    .debug
                    .children(r)
                    .filter(|&m| self.debug.die(m).tag == dw::TAG_MEMBER)
                    .map(|m| {
                        let d = self.debug.die(m);
                        (
                            d.name().unwrap_or("").to_string(),
                            Ty::Die(self.debug.type_of(m)),
                            d.unsigned(dw::AT_DATA_MEMBER_LOCATION).unwrap_or(0),
                        )
                    })
                    .collect();
                if members.is_empty() {
                    return "<incomplete type>".into();
                }
                let parts: Vec<String> = members
                    .into_iter()
                    .map(|(name, t, off)| {
                        let size = self.debug.size(&t, self.xlen).unwrap_or(0) as usize;
                        let off = off as usize;
                        let part = b.get(off..off + size).unwrap_or_default();
                        let at = addr.map(|a| a + off as u64);
                        format!("{name} = {}", self.show_bytes(&t, part, at, false, hex))
                    })
                    .collect();
                format!("{{{}}}", parts.join(", "))
            }
            Shape::Func(f) => self.debug.declare(&Ty::Die(Some(f)), ""),
        }
    }

    /// The string at `a`, as GDB shows a `char *`'s: up to 200 characters, `...`
    /// past them.
    fn string_at(&mut self, a: u64) -> String {
        let mut bytes = Vec::new();
        let mut at = a;
        while bytes.len() < ELEMENTS {
            let chunk = match self.target.read(at, 32) {
                Ok(c) => c,
                Err(_) if bytes.is_empty() => match self.target.read(at, 1) {
                    Ok(c) => c,
                    Err(_) => return format!("<error: Cannot access memory at address {a:#x}>"),
                },
                Err(_) => match self.target.read(at, 1) {
                    Ok(c) => c,
                    Err(_) => break,
                },
            };
            if let Some(end) = chunk.iter().position(|&c| c == 0) {
                bytes.extend(&chunk[..end]);
                return quote_chars(&bytes);
            }
            at += chunk.len() as u64;
            bytes.extend(chunk);
        }
        bytes.truncate(ELEMENTS);
        format!("{}...", quote_chars(&bytes))
    }

    /// `name = value` for each variable, as `info locals` and `info args` show them.
    pub fn show_vars(&mut self, vars: &[DieRef]) -> Vec<String> {
        vars.iter()
            .map(|&v| {
                let name = self.debug.die(v).name().unwrap_or("?").to_string();
                let shown = self
                    .var_value(v)
                    .and_then(|val| self.show(&val, false, false))
                    .unwrap_or_else(|e| format!("<error: {e}>"));
                format!("{name} = {shown}")
            })
            .collect()
    }

    /// A function's arguments as a backtrace shows them, `n=5, p=0x80001000`,
    /// as GDB's `print frame-arguments scalars`: a struct, union or array is `...`.
    pub fn show_args(&mut self, func: DieRef) -> String {
        let params = self.debug.params(func);
        params
            .iter()
            .map(|&p| {
                let name = self.debug.die(p).name().unwrap_or("?").to_string();
                let ty = Ty::Die(self.debug.type_of(p));
                let shown = match self.debug.shape(&ty) {
                    Shape::Record(_) | Shape::Array(..) => "...".to_string(),
                    _ => self
                        .var_value(p)
                        .and_then(|val| self.show(&val, false, false))
                        .unwrap_or_else(|e| format!("<error: {e}>")),
                };
                format!("{name}={shown}")
            })
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// The type of a value, as `whatis` and `ptype` start from.
    pub fn type_of(&self, v: &Value) -> Ty {
        v.ty.clone()
    }
}

/// Elements as GDB lists them: a run of [`REPEATS`] or more equal ones once, with
/// `<repeats N times>`.
fn repeats(items: Vec<String>) -> Vec<String> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < items.len() {
        let mut j = i + 1;
        while j < items.len() && items[j] == items[i] {
            j += 1;
        }
        if j - i >= REPEATS {
            out.push(format!("{} <repeats {} times>", items[i], j - i));
        } else {
            out.extend(items[i..j].iter().cloned());
        }
        i = j;
    }
    out
}

/// A character as C writes it between `quote`s, as GDB does: printable ASCII as it
/// is, the usual escapes, and octal for the rest.
fn char_literal(c: u8, quote: char) -> String {
    let body = match c {
        b'\n' => "\\n".to_string(),
        b'\t' => "\\t".to_string(),
        b'\r' => "\\r".to_string(),
        7 => "\\a".to_string(),
        8 => "\\b".to_string(),
        12 => "\\f".to_string(),
        11 => "\\v".to_string(),
        27 => "\\033".to_string(),
        b'\\' => "\\\\".to_string(),
        c if c as char == quote => format!("\\{quote}"),
        0x20..=0x7e => (c as char).to_string(),
        c => format!("\\{c:03o}"),
    };
    if quote == '\'' {
        format!("'{body}'")
    } else {
        body
    }
}

/// An array of characters as GDB shows it: a string, without its last character
/// if that is a NUL, and runs of [`REPEATS`] or more of one character as
/// `'c' <repeats N times>` between the quoted parts.
fn quote_chars(b: &[u8]) -> String {
    let b = match b.last() {
        Some(0) => &b[..b.len() - 1],
        _ => b,
    };
    if b.is_empty() {
        return "\"\"".into();
    }
    let mut parts = Vec::new();
    let mut text = String::new();
    let mut i = 0;
    while i < b.len() {
        let mut j = i + 1;
        while j < b.len() && b[j] == b[i] {
            j += 1;
        }
        if j - i >= REPEATS {
            if !text.is_empty() {
                parts.push(format!("\"{text}\""));
                text.clear();
            }
            parts.push(format!("{} <repeats {} times>", char_literal(b[i], '\''), j - i));
        } else {
            for &c in &b[i..j] {
                text += &char_literal(c, '"');
            }
        }
        i = j;
    }
    if !text.is_empty() {
        parts.push(format!("\"{text}\""));
    }
    parts.join(", ")
}

/// A little-endian number, sign-extended if `signed`.
fn from_le(b: &[u8], signed: bool) -> i128 {
    let mut v = 0u128;
    for (i, &x) in b.iter().enumerate().take(16) {
        v |= u128::from(x) << (8 * i);
    }
    let bits = 8 * b.len().min(16) as u32;
    if signed && bits > 0 && bits < 128 && v >> (bits - 1) & 1 == 1 {
        (v | (u128::MAX << bits)) as i128
    } else {
        v as i128
    }
}

/// Reads the operands of a DWARF expression.
struct Reader<'a, 'b> {
    b: &'a [u8],
    at: &'b mut usize,
}

impl Reader<'_, '_> {
    fn u8(&mut self) -> Option<u8> {
        let v = *self.b.get(*self.at)?;
        *self.at += 1;
        Some(v)
    }
    fn uint(&mut self, n: usize) -> Option<u64> {
        let b = self.b.get(*self.at..*self.at + n)?;
        *self.at += n;
        Some(from_le(b, false) as u64)
    }
    fn uleb(&mut self) -> Option<u64> {
        let mut v = 0u64;
        let mut shift = 0;
        loop {
            let b = self.u8()?;
            if shift < 64 {
                v |= u64::from(b & 0x7f) << shift;
            }
            shift += 7;
            if b & 0x80 == 0 {
                return Some(v);
            }
        }
    }
    fn sleb(&mut self) -> Option<i64> {
        let mut v = 0i64;
        let mut shift = 0;
        loop {
            let b = self.u8()?;
            if shift < 64 {
                v |= i64::from(b & 0x7f) << shift;
            }
            shift += 7;
            if b & 0x80 == 0 {
                if shift < 64 && b & 0x40 != 0 {
                    v |= -1i64 << shift;
                }
                return Some(v);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn characters_and_strings_as_gdb_shows_them() {
        assert_eq!(char_literal(b'A', '\''), "'A'");
        assert_eq!(char_literal(0, '\''), "'\\000'");
        assert_eq!(char_literal(b'\n', '\''), "'\\n'");
        assert_eq!(char_literal(0xff, '\''), "'\\377'");
        assert_eq!(char_literal(b'\'', '\''), "'\\''");
        assert_eq!(quote_chars(b"box\0"), "\"box\"");
        assert_eq!(quote_chars(b"ab\0\0"), "\"ab\\000\"");
        assert_eq!(quote_chars(b"D3\"\x11"), "\"D3\\\"\\021\"");
        assert_eq!(quote_chars(&[b'x'; 12]), "'x' <repeats 12 times>");
        let mut s = b"ab".to_vec();
        s.extend([0; 11]);
        s.extend(b"c\0");
        assert_eq!(quote_chars(&s), "\"ab\", '\\000' <repeats 11 times>, \"c\"");
    }

    #[test]
    fn runs_of_elements() {
        let items = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(repeats(items(&["1", "2", "2"])), items(&["1", "2", "2"]));
        let mut v = vec!["0"; 10];
        v.push("1");
        assert_eq!(repeats(items(&v)), items(&["0 <repeats 10 times>", "1"]));
    }

    #[test]
    fn numbers_and_expressions() {
        assert_eq!(from_le(&[0xff], true), -1);
        assert_eq!(from_le(&[0xff], false), 255);
        assert_eq!(from_le(&[0, 0, 0, 0x80], true), i128::from(i32::MIN));
        let t = tokens("p->next[2].x + 0x10 * 'a'").unwrap();
        assert_eq!(t.len(), 12);
        assert_eq!(t[0], Tok::Ident("p".into()));
        assert_eq!(t[1], Tok::Punct("->"));
        assert_eq!(t[9], Tok::Num(16));
        assert_eq!(t[11], Tok::Char(b'a'));
        assert!(tokens("a # b").is_err());
    }
}
