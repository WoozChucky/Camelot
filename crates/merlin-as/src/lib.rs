//! merlin-as: assembles Excalibur assembly into ELF relocatable objects, RV32
//! or RV64 ([`Options::xlen`], `-march=` on the command line).
//!
//! Pipeline: [`lexer`] → [`parser`] → [`asm`] (calls [`encode`] per instruction,
//! [`dwarf`] for the line table of `.file` and `.loc`, and [`section`] for
//! the ELF attributes of the sections) → [`elf`].
//!
//! The width is chosen once, for the whole file, as clang's `-march` does: it
//! reaches the encoder through [`Ctx::xlen`], and the object carries it
//! ([`Object::xlen`]) to the ELF writer.

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
pub use excalibur::Xlen;

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

/// How to assemble: what clang's `-march` decides for merlin-as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Options {
    /// The register width: RV32 or RV64 instructions, ELF32 or ELF64 objects.
    pub xlen: Xlen,
}

impl Default for Options {
    /// RV32, until the migration to RV64 ends (#159).
    fn default() -> Self {
        Options { xlen: Xlen::Rv32 }
    }
}

/// The `-march=` strings merlin-as takes: the ISA it implements (spec
/// section 1), at either width. clang spells them the same way.
const MARCH: [(&str, Xlen); 2] = [
    ("rv32ima_zicsr_zifencei", Xlen::Rv32),
    ("rv64ima_zicsr_zifencei", Xlen::Rv64),
];

/// Reads the value of `-march=`. Only the two exact ISA strings are accepted:
/// merlin-as has no other extension to turn on or off, so anything else
/// (`rv64gc`, `rv32i`) would promise instructions it does not assemble.
pub fn parse_march(s: &str) -> Result<Xlen, String> {
    MARCH
        .iter()
        .find(|(name, _)| *name == s)
        .map(|&(_, xlen)| xlen)
        .ok_or_else(|| unsupported_march(s))
}

/// The error for a `-march=` value that is not in [`MARCH`]. It lists the
/// strings from the table, so a new one is named without a second edit.
fn unsupported_march(s: &str) -> String {
    let takes: Vec<&str> = MARCH.iter().map(|&(name, _)| name).collect();
    format!(
        "unsupported -march '{s}' (merlin-as takes {})",
        takes.join(" or ")
    )
}

/// Assembles source text into an RV32 object. Returns every error found.
pub fn assemble(src: &str) -> Result<Object, Vec<Error>> {
    assemble_with(src, &Options::default())
}

/// Assembles source text into an object for `opts`. Returns every error found.
pub fn assemble_with(src: &str, opts: &Options) -> Result<Object, Vec<Error>> {
    let toks = lexer::lex(src).map_err(|e| vec![e])?;
    let stmts = parser::parse(&toks)?;
    asm::Assembler::new(opts.xlen).run(&stmts)
}

/// Assembles straight to RV32 ELF bytes.
pub fn assemble_to_elf(src: &str) -> Result<Vec<u8>, Vec<Error>> {
    assemble_to_elf_with(src, &Options::default())
}

/// Assembles straight to ELF bytes for `opts`.
pub fn assemble_to_elf_with(src: &str, opts: &Options) -> Result<Vec<u8>, Vec<Error>> {
    assemble_with(src, opts).map(|o| elf::write(&o))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn march_takes_the_two_isa_strings() {
        assert_eq!(parse_march("rv32ima_zicsr_zifencei"), Ok(Xlen::Rv32));
        assert_eq!(parse_march("rv64ima_zicsr_zifencei"), Ok(Xlen::Rv64));
        assert_eq!(Options::default().xlen, Xlen::Rv32);
    }

    #[test]
    fn march_rejects_any_other_isa() {
        for s in [
            "",
            "rv64gc",
            "rv32i",
            "rv64ima",
            "RV64IMA_ZICSR_ZIFENCEI",
            "rv64ima_zicsr_zifencei_zba",
        ] {
            assert_eq!(parse_march(s), Err(unsupported_march(s)));
        }
        // What the message looks like, spelled out once.
        assert_eq!(
            unsupported_march("rv64gc"),
            "unsupported -march 'rv64gc' (merlin-as takes rv32ima_zicsr_zifencei or rv64ima_zicsr_zifencei)"
        );
    }
}
