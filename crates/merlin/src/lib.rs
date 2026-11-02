//! merlin: compiles a subset of C into Excalibur assembly for merlin-as.
//!
//! Pipeline: [`lexer`] → [`parser`] → [`sema`] (name resolution, arity checks)
//! → [`codegen`]. [`parse`] runs the front end and returns a checked AST;
//! [`compile`] goes all the way to assembly text.
//!
//! The v0 language (spec section 10) is `int` only: functions with up to 8 `int`
//! parameters, blocks, local declarations, `if`/`else`, `while`, `for`, `return`,
//! `break`, `continue`, and the expressions `= + - * / % == != < <= > >= && || !`,
//! unary `-`, parentheses and calls.

pub mod ast;
pub mod codegen;
pub mod lexer;
pub mod parser;
pub mod sema;

use std::fmt;

/// A source position: 1-based line and column (columns count characters).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Pos {
    pub line: u32,
    pub col: u32,
}

impl Pos {
    pub const fn new(line: u32, col: u32) -> Self {
        Pos { line, col }
    }
}

impl fmt::Display for Pos {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.line, self.col)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    /// Where the problem is, or line 0 when it isn't tied to a place in the source.
    pub pos: Pos,
    pub msg: String,
}

impl Error {
    pub fn new(pos: Pos, msg: impl Into<String>) -> Self {
        Error { pos, msg: msg.into() }
    }

    /// Formats the error the way compilers and editors expect:
    /// `file.c:line:col: error: message`.
    pub fn render(&self, file: &str) -> String {
        if self.pos.line > 0 {
            format!("{file}:{}: error: {}", self.pos, self.msg)
        } else {
            format!("{file}: error: {}", self.msg)
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.pos.line > 0 {
            write!(f, "{}: {}", self.pos, self.msg)
        } else {
            write!(f, "{}", self.msg)
        }
    }
}

/// Lexes, parses and checks one translation unit. Lexing and parsing stop at the
/// first error; the checker reports every error it finds.
pub fn parse(src: &str) -> Result<ast::Program, Vec<Error>> {
    let toks = lexer::lex(src).map_err(|e| vec![e])?;
    let mut prog = parser::parse(&toks).map_err(|e| vec![e])?;
    sema::check(&mut prog)?;
    Ok(prog)
}

/// Compiles one translation unit to assembly text for merlin-as.
pub fn compile(src: &str) -> Result<String, Vec<Error>> {
    let prog = parse(src)?;
    codegen::generate(&prog).map_err(|e| vec![e])
}
