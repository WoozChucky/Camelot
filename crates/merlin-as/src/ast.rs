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
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stmt {
    pub line: usize,
    pub kind: StmtKind,
}
