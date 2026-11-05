//! merlin-as: assembles Excalibur (RV64) assembly into ELF64 relocatable
//! objects.
//!
//! Pipeline: [`lexer`] → [`parser`] → [`asm`] (calls [`encode`] per instruction,
//! [`dwarf`] for the line table of `.file` and `.loc`, and [`section`] for
//! the ELF attributes of the sections) → [`elf`].

pub mod asm;
pub mod ast;
pub mod dwarf;
pub mod elf;
pub mod encode;
pub mod isa;
pub mod lexer;
pub mod parser;
pub mod reloc;
pub mod section;

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

/// The `-march=` string merlin-as takes: the ISA it implements (spec section
/// 1), as clang spells it.
pub const MARCH: &str = "rv64ima_zicsr_zifencei";

/// Reads the value of `-march=`. Only the exact ISA string is accepted:
/// merlin-as has no other extension to turn on or off, so anything else
/// (`rv64gc`, `rv64i`) would promise instructions it does not assemble, and an
/// RV32 one (`rv32ima_zicsr_zifencei`, which merlin-as took until #159) a
/// width it does not assemble.
pub fn parse_march(s: &str) -> Result<(), String> {
    if s == MARCH {
        Ok(())
    } else {
        Err(unsupported_march(s))
    }
}

/// The error for a `-march=` value that is not [`MARCH`].
fn unsupported_march(s: &str) -> String {
    format!("unsupported -march '{s}' (merlin-as takes {MARCH})")
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn march_takes_the_isa_string() {
        assert_eq!(parse_march("rv64ima_zicsr_zifencei"), Ok(()));
    }

    #[test]
    fn march_rejects_any_other_isa() {
        for s in [
            "",
            "rv64gc",
            "rv32i",
            "rv32ima_zicsr_zifencei",
            "rv64ima",
            "RV64IMA_ZICSR_ZIFENCEI",
            "rv64ima_zicsr_zifencei_zba",
        ] {
            assert_eq!(parse_march(s), Err(unsupported_march(s)));
        }
        // What the message looks like, spelled out once.
        assert_eq!(
            unsupported_march("rv64gc"),
            "unsupported -march 'rv64gc' (merlin-as takes rv64ima_zicsr_zifencei)"
        );
    }
}
