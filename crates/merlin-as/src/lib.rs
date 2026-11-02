//! merlin-as: assembles Excalibur assembly into ELF32 relocatable objects.
//!
//! Pipeline: [`lexer`] → [`parser`] → [`asm`] (calls [`encode`] per instruction,
//! and [`dwarf`] for the line table of `.file` and `.loc`) → [`elf`].

pub mod asm;
pub mod ast;
pub mod dwarf;
pub mod elf;
pub mod encode;
pub mod lexer;
pub mod parser;
pub mod reloc;

pub use asm::{Ctx, Object};

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    /// 1-based source line, or 0 when the error isn't tied to one line.
    pub line: usize,
    pub msg: String,
}

impl Error {
    pub fn new(line: usize, msg: impl Into<String>) -> Self {
        Error {
            line,
            msg: msg.into(),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.line > 0 {
            write!(f, "line {}: {}", self.line, self.msg)
        } else {
            write!(f, "{}", self.msg)
        }
    }
}

/// Assembles source text into an object. Returns every error found.
pub fn assemble(src: &str) -> Result<Object, Vec<Error>> {
    let toks = lexer::lex(src).map_err(|e| vec![e])?;
    let stmts = parser::parse(&toks)?;
    asm::Assembler::new().run(&stmts)
}

/// Assembles straight to ELF bytes.
pub fn assemble_to_elf(src: &str) -> Result<Vec<u8>, Vec<Error>> {
    assemble(src).map(|o| elf::write(&o))
}
