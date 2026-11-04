//! What the parser produces: statements, operands and expressions.

use excalibur::Reg;

/// Relocation modifiers written in source, like `%hi(sym)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Modifier {
    Hi,
    Lo,
    PcrelHi,
    PcrelLo,
}

impl Modifier {
    pub fn from_name(name: &str) -> Option<Modifier> {
        Some(match name {
            "hi" => Modifier::Hi,
            "lo" => Modifier::Lo,
            "pcrel_hi" => Modifier::PcrelHi,
            "pcrel_lo" => Modifier::PcrelLo,
            _ => return None,
        })
    }

    /// As written in source: `%hi`, `%lo`, `%pcrel_hi`, `%pcrel_lo`.
    pub fn name(self) -> &'static str {
        match self {
            Modifier::Hi => "%hi",
            Modifier::Lo => "%lo",
            Modifier::PcrelHi => "%pcrel_hi",
            Modifier::PcrelLo => "%pcrel_lo",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Shl,
    /// `>>`: a logical shift, of the value's 64 bits, as in LLVM, whose
    /// MCAsmInfo has `UseLogicalShr` true by default and RISC-V leaves it on
    /// (`-16 >> 2` is 0x3ffffffffffffffc).
    Shr,
    And,
    Or,
    Xor,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Expr {
    Num(i64),
    /// A symbol name. Numeric local label references (`1b`, `1f`) are already
    /// rewritten to unique internal names by the time the encoder sees them.
    Sym(String),
    Neg(Box<Expr>),
    Not(Box<Expr>),
    Bin(BinOp, Box<Expr>, Box<Expr>),
    Mod(Modifier, Box<Expr>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Operand {
    Reg(Reg),
    Expr(Expr),
    /// `offset(base)`. A bare `(base)` has offset `Num(0)`.
    Mem {
        offset: Expr,
        base: Reg,
        /// The offset was left out or written as one integer token (`0`,
        /// `0x0`), not as an expression. The expression is folded while
        /// parsing (`+0` and `(0)` both become `Num(0)`), so this keeps what
        /// was written: an A instruction takes only a literal 0, as in LLVM.
        int_token: bool,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StmtKind {
    /// `name:` (numeric labels like `1:` keep their digits as the name).
    Label(String),
    /// `.name args...`, args kept as raw expressions/strings.
    Directive(String, Vec<DirArg>),
    Instruction(String, Vec<Operand>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DirArg {
    Expr(Expr),
    Str(Vec<u8>),
    /// A number wider than 64 bits (`.file`'s MD5 checksum).
    Wide(u128),
    /// An argument left out between two commas: the fill of
    /// `.p2align 3,,4`.
    Empty,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stmt {
    pub line: usize,
    pub kind: StmtKind,
}
