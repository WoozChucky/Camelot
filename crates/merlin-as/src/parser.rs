//! Tokens to statements.
//!
//! Grammar, one statement per line (labels can share a line with what follows):
//!
//! ```text
//! line      := label* (directive | insn | instruction)?
//! label     := IDENT ':' | INT ':'
//! directive := '.'IDENT (dirarg (',' dirarg)*)?
//! insn      := '.insn' IDENT? operand (',' operand)*   (an instruction named `.insn IDENT`)
//! instr     := IDENT (operand (',' operand)*)?
//! operand   := REG | expr '(' REG ')' | '(' REG ')' | expr
//! expr      := standard C-like precedence: | ^ & << >> + - * / % unary(- ~) primary
//! primary   := INT | IDENT | LOCALREF | '(' expr ')' | '%' IDENT '(' expr ')'
//! ```

use crate::ast::*;
use crate::lexer::{Tok, Token};
use crate::Error;

struct Parser<'a> {
    toks: &'a [Token],
    pos: usize,
}

pub fn parse(toks: &[Token]) -> Result<Vec<Stmt>, Vec<Error>> {
    let mut p = Parser { toks, pos: 0 };
    let mut stmts = Vec::new();
    let mut errors = Vec::new();
    while p.pos < toks.len() {
        if let Err(e) = p.line(&mut stmts) {
            errors.push(e);
            // Skip to the next line and keep going so we report every error.
            while !matches!(p.peek(), Tok::Newline) {
                p.pos += 1;
            }
            p.pos += 1;
        }
    }
    if errors.is_empty() {
        Ok(stmts)
    } else {
        Err(errors)
    }
}

/// Local label references are kept as `Sym("1b")` / `Sym("1f")`; the assembler
/// resolves them because it knows which definitions came before.
pub fn local_ref_name(n: u32, forward: bool) -> String {
    format!("{n}{}", if forward { 'f' } else { 'b' })
}

impl Parser<'_> {
    fn peek(&self) -> &Tok {
        &self.toks[self.pos.min(self.toks.len() - 1)].tok
    }

    fn peek_at(&self, n: usize) -> &Tok {
        &self.toks[(self.pos + n).min(self.toks.len() - 1)].tok
    }

    fn line_no(&self) -> usize {
        self.toks[self.pos.min(self.toks.len() - 1)].line
    }

    fn next(&mut self) -> Tok {
        let t = self.peek().clone();
        self.pos += 1;
        t
    }

    fn err(&self, msg: impl Into<String>) -> Error {
        Error::new(self.line_no(), msg)
    }

    fn expect_punct(&mut self, c: char) -> Result<(), Error> {
        if *self.peek() == Tok::Punct(c) {
            self.pos += 1;
            Ok(())
        } else {
            Err(self.err(format!("expected '{c}'")))
        }
    }

    fn line(&mut self, out: &mut Vec<Stmt>) -> Result<(), Error> {
        let line = self.line_no();
        // Labels.
        loop {
            match (self.peek().clone(), self.peek_at(1)) {
                (Tok::Ident(name), Tok::Punct(':')) => {
                    self.pos += 2;
                    out.push(Stmt {
                        line,
                        kind: StmtKind::Label(name),
                    });
                }
                (Tok::Int(n), Tok::Punct(':')) if n >= 0 => {
                    self.pos += 2;
                    out.push(Stmt {
                        line,
                        kind: StmtKind::Label(n.to_string()),
                    });
                }
                _ => break,
            }
        }
        match self.next() {
            Tok::Newline => return Ok(()),
            // `.insn i 0x0b, 0, x0, a0, 0` spells an instruction out field by
            // field. Its operands are instruction operands (registers,
            // `offset(reg)`), so it becomes an instruction named `.insn i`
            // (`.insn` alone for the raw `.insn 0x0005000b` form).
            Tok::Ident(name) if name == ".insn" => {
                let mut mnemonic = name;
                if let Tok::Ident(format) = self.peek().clone() {
                    if !matches!(self.peek_at(1), Tok::Punct(',') | Tok::Newline) {
                        self.pos += 1;
                        mnemonic = format!(".insn {}", format.to_ascii_lowercase());
                    }
                }
                let ops = self.operands()?;
                out.push(Stmt {
                    line,
                    kind: StmtKind::Instruction(mnemonic, ops),
                });
            }
            Tok::Ident(name) if name.starts_with('.') => {
                let args = match name.as_str() {
                    ".file" | ".loc" => self.spaced_args()?,
                    ".section" => self.section_args()?,
                    _ => self.dir_args()?,
                };
                out.push(Stmt {
                    line,
                    kind: StmtKind::Directive(name, args),
                });
            }
            // `sym = expr` is shorthand for `.set sym, expr`.
            Tok::Ident(name) if *self.peek() == Tok::Punct('=') => {
                self.pos += 1;
                let e = self.expr()?;
                let args = vec![DirArg::Expr(Expr::Sym(name)), DirArg::Expr(e)];
                out.push(Stmt {
                    line,
                    kind: StmtKind::Directive(".set".into(), args),
                });
            }
            Tok::Ident(name) => {
                let ops = self.operands()?;
                out.push(Stmt {
                    line,
                    kind: StmtKind::Instruction(name.to_ascii_lowercase(), ops),
                });
            }
            _ => return Err(Error::new(line, "expected a label, directive or instruction")),
        }
        match self.next() {
            Tok::Newline => Ok(()),
            _ => Err(Error::new(line, "unexpected text at end of line")),
        }
    }

    fn dir_args(&mut self) -> Result<Vec<DirArg>, Error> {
        let mut args = Vec::new();
        if matches!(self.peek(), Tok::Newline) {
            return Ok(args);
        }
        loop {
            match self.peek().clone() {
                Tok::Str(s) => {
                    self.pos += 1;
                    args.push(DirArg::Str(s));
                }
                _ => args.push(DirArg::Expr(self.expr()?)),
            }
            if *self.peek() != Tok::Punct(',') {
                return Ok(args);
            }
            self.pos += 1;
        }
    }

    /// The arguments of `.section name, "flags", @type, entsize`, read as
    /// [`Parser::dir_args`] reads them, except the type, the third: LLVM
    /// takes it as `@progbits`, `%progbits` or `"progbits"` (a name or a
    /// number), and whichever it is, it becomes the string `progbits` for
    /// the assembler. `%progbits` alone would not parse as an expression.
    fn section_args(&mut self) -> Result<Vec<DirArg>, Error> {
        let mut args = Vec::new();
        if matches!(self.peek(), Tok::Newline) {
            return Ok(args);
        }
        loop {
            let ty = if args.len() == 2 {
                match (self.peek().clone(), self.peek_at(1).clone()) {
                    (Tok::Ident(t), _) if t.starts_with('@') => {
                        self.pos += 1;
                        Some(t[1..].to_string())
                    }
                    (Tok::Punct('%'), Tok::Ident(t)) => {
                        self.pos += 2;
                        Some(t)
                    }
                    (Tok::Punct('%'), Tok::Int(n)) => {
                        self.pos += 2;
                        Some(n.to_string())
                    }
                    _ => None,
                }
            } else {
                None
            };
            match (ty, self.peek().clone()) {
                (Some(t), _) => args.push(DirArg::Str(t.into_bytes())),
                (None, Tok::Str(s)) => {
                    self.pos += 1;
                    args.push(DirArg::Str(s));
                }
                (None, _) => args.push(DirArg::Expr(self.expr()?)),
            }
            if *self.peek() != Tok::Punct(',') {
                return Ok(args);
            }
            self.pos += 1;
        }
    }

    /// The arguments of `.file` and `.loc`, separated by spaces as GNU as and
    /// LLVM write them (`.loc 1 12 5 prologue_end`, `.file 1 "dir" "a.c" md5
    /// 0x...`); commas are allowed too.
    fn spaced_args(&mut self) -> Result<Vec<DirArg>, Error> {
        let mut args = Vec::new();
        loop {
            match self.peek().clone() {
                Tok::Newline => return Ok(args),
                Tok::Punct(',') => self.pos += 1,
                Tok::Str(s) => {
                    self.pos += 1;
                    args.push(DirArg::Str(s));
                }
                Tok::Wide(v) => {
                    self.pos += 1;
                    args.push(DirArg::Wide(v));
                }
                _ => args.push(DirArg::Expr(self.expr()?)),
            }
        }
    }

    fn operands(&mut self) -> Result<Vec<Operand>, Error> {
        let mut ops = Vec::new();
        if matches!(self.peek(), Tok::Newline) {
            return Ok(ops);
        }
        loop {
            ops.push(self.operand()?);
            if *self.peek() != Tok::Punct(',') {
                return Ok(ops);
            }
            self.pos += 1;
        }
    }

    fn reg_at(&self, n: usize) -> Option<u8> {
        match self.peek_at(n) {
            Tok::Ident(name) => excalibur::reg::parse(name),
            _ => None,
        }
    }

    fn operand(&mut self) -> Result<Operand, Error> {
        // Plain register.
        if let Some(r) = self.reg_at(0) {
            if !matches!(self.peek_at(1), Tok::Punct('(')) {
                self.pos += 1;
                return Ok(Operand::Reg(r));
            }
        }
        // `(reg)` with no offset.
        if *self.peek() == Tok::Punct('(') && self.peek_at(2) == &Tok::Punct(')') {
            if let Some(base) = self.reg_at(1) {
                self.pos += 3;
                return Ok(Operand::Mem {
                    offset: Expr::Num(0),
                    base,
                    int_token: true,
                });
            }
        }
        let start = self.pos;
        let e = self.expr()?;
        let int_token = self.pos == start + 1 && matches!(self.toks[start].tok, Tok::Int(_));
        // `expr(reg)`
        if *self.peek() == Tok::Punct('(') {
            if let (Some(base), Tok::Punct(')')) = (self.reg_at(1), self.peek_at(2)) {
                self.pos += 3;
                return Ok(Operand::Mem {
                    offset: e,
                    base,
                    int_token,
                });
            }
        }
        Ok(Operand::Expr(e))
    }

    pub fn expr(&mut self) -> Result<Expr, Error> {
        self.binary(0)
    }

    /// Precedence climbing. Higher level binds tighter.
    fn binary(&mut self, min_level: u8) -> Result<Expr, Error> {
        let mut lhs = self.unary()?;
        loop {
            let (op, level) = match self.peek() {
                Tok::Punct('|') => (BinOp::Or, 0),
                Tok::Punct('^') => (BinOp::Xor, 1),
                Tok::Punct('&') => (BinOp::And, 2),
                Tok::Shift(true) => (BinOp::Shl, 3),
                Tok::Shift(false) => (BinOp::Shr, 3),
                Tok::Punct('+') => (BinOp::Add, 4),
                Tok::Punct('-') => (BinOp::Sub, 4),
                Tok::Punct('*') => (BinOp::Mul, 5),
                Tok::Punct('/') => (BinOp::Div, 5),
                Tok::Punct('%') => (BinOp::Rem, 5),
                _ => return Ok(lhs),
            };
            if level < min_level {
                return Ok(lhs);
            }
            self.pos += 1;
            let rhs = self.binary(level + 1)?;
            lhs = Expr::Bin(op, Box::new(lhs), Box::new(rhs));
        }
    }

    fn unary(&mut self) -> Result<Expr, Error> {
        match self.peek() {
            Tok::Punct('-') => {
                self.pos += 1;
                Ok(Expr::Neg(Box::new(self.unary()?)))
            }
            Tok::Punct('+') => {
                self.pos += 1;
                self.unary()
            }
            Tok::Punct('~') => {
                self.pos += 1;
                Ok(Expr::Not(Box::new(self.unary()?)))
            }
            _ => self.primary(),
        }
    }

    fn primary(&mut self) -> Result<Expr, Error> {
        // Never consume a newline on error, so recovery resumes at the next line.
        if matches!(self.peek(), Tok::Newline) {
            return Err(self.err("expected an expression"));
        }
        match self.next() {
            Tok::Int(n) => Ok(Expr::Num(n)),
            Tok::Wide(_) => Err(self.err("number too large for 64 bits")),
            Tok::Ident(name) => Ok(Expr::Sym(name)),
            Tok::LocalRef(n, fwd) => Ok(Expr::Sym(local_ref_name(n, fwd))),
            Tok::Punct('(') => {
                let e = self.expr()?;
                self.expect_punct(')')?;
                Ok(e)
            }
            Tok::Punct('%') => {
                let Tok::Ident(name) = self.next() else {
                    return Err(self.err("expected a modifier name after '%'"));
                };
                let m = Modifier::from_name(&name)
                    .ok_or_else(|| self.err(format!("unknown modifier '%{name}'")))?;
                self.expect_punct('(')?;
                let e = self.expr()?;
                self.expect_punct(')')?;
                Ok(Expr::Mod(m, Box::new(e)))
            }
            _ => Err(self.err("expected an expression")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::lex;

    fn one(src: &str) -> StmtKind {
        let mut v = parse(&lex(src).unwrap()).unwrap();
        v.pop().unwrap().kind
    }

    #[test]
    fn instruction_operands() {
        assert_eq!(
            one("lw a0, %lo(msg+4)(sp)"),
            StmtKind::Instruction(
                "lw".into(),
                vec![
                    Operand::Reg(10),
                    Operand::Mem {
                        offset: Expr::Mod(
                            Modifier::Lo,
                            Box::new(Expr::Bin(
                                BinOp::Add,
                                Box::new(Expr::Sym("msg".into())),
                                Box::new(Expr::Num(4))
                            ))
                        ),
                        base: 2,
                        int_token: false
                    }
                ]
            )
        );
        assert_eq!(
            one("sw ra, (sp)"),
            StmtKind::Instruction(
                "sw".into(),
                vec![
                    Operand::Reg(1),
                    Operand::Mem {
                        offset: Expr::Num(0),
                        base: 2,
                        int_token: true
                    }
                ]
            )
        );
        // The offset folds to Num(0) either way; int_token keeps whether it
        // was one integer token (what an A instruction accepts, as in LLVM).
        let offset_token = |src: &str| match one(src) {
            StmtKind::Instruction(_, ops) => match &ops[1] {
                Operand::Mem {
                    offset: Expr::Num(0),
                    int_token,
                    ..
                } => *int_token,
                op => panic!("{src}: {op:?}"),
            },
            kind => panic!("{src}: {kind:?}"),
        };
        for (src, want) in [
            ("lr.w a0, (a1)", true),
            ("lr.w a0, 0(a1)", true),
            ("lr.w a0, 0x0 (a1)", true),
            ("lr.w a0, +0(a1)", false),
            ("lr.w a0, (0)(a1)", false),
            ("lr.w a0, ((0))(a1)", false),
        ] {
            assert_eq!(offset_token(src), want, "{src}");
        }
        assert_eq!(
            one("beqz t0, 1f"),
            StmtKind::Instruction(
                "beqz".into(),
                vec![Operand::Reg(5), Operand::Expr(Expr::Sym("1f".into()))]
            )
        );
    }

    #[test]
    fn insn_directive() {
        assert_eq!(
            one(".insn i 0x0b, 0, x0, a0, 0"),
            StmtKind::Instruction(
                ".insn i".into(),
                vec![
                    Operand::Expr(Expr::Num(0x0b)),
                    Operand::Expr(Expr::Num(0)),
                    Operand::Reg(0),
                    Operand::Reg(10),
                    Operand::Expr(Expr::Num(0)),
                ]
            )
        );
        assert_eq!(
            one(".insn S 0x23, 2, a0, 8(sp)"),
            StmtKind::Instruction(
                ".insn s".into(),
                vec![
                    Operand::Expr(Expr::Num(0x23)),
                    Operand::Expr(Expr::Num(2)),
                    Operand::Reg(10),
                    Operand::Mem {
                        offset: Expr::Num(8),
                        base: 2,
                        int_token: true
                    },
                ]
            )
        );
        assert_eq!(
            one(".insn 0x0005000b"),
            StmtKind::Instruction(".insn".into(), vec![Operand::Expr(Expr::Num(0x0005000b))])
        );
        // A constant named like a format is still a raw value when alone.
        assert_eq!(
            one(".insn WORD"),
            StmtKind::Instruction(".insn".into(), vec![Operand::Expr(Expr::Sym("WORD".into()))])
        );
    }

    #[test]
    fn precedence() {
        let StmtKind::Directive(_, args) = one(".word 1 + 2 * 3 << 1") else {
            panic!()
        };
        // (1 + (2*3)) << 1
        let DirArg::Expr(Expr::Bin(BinOp::Shl, lhs, _)) = &args[0] else {
            panic!("{args:?}")
        };
        assert!(matches!(**lhs, Expr::Bin(BinOp::Add, _, _)));
    }

    #[test]
    fn labels_and_errors() {
        let v = parse(&lex("a: 1: b: nop").unwrap()).unwrap();
        assert_eq!(v.len(), 4);
        let errs = parse(&lex("addi a0,\n nop )\n").unwrap()).unwrap_err();
        assert_eq!(errs.iter().map(|e| e.line).collect::<Vec<_>>(), vec![1, 2]);
    }
}
