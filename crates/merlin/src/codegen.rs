//! # Code generation: AST to Excalibur assembly (issue #14)
//!
//! Everything before this (lexing, parsing, name resolution, arity checks) is
//! done; this module turns a checked [`Program`] into `.s` text for merlin-as.
//!
//! ## Contract
//!
//! * Input: a [`Program`] that passed [`crate::sema`]. Every variable and
//!   declarator has `local: Some(LocalId(i))`, an index into its function's
//!   `locals` (parameters first, in order), so each local can live at a fixed
//!   offset from `s0`. Every call names a declared function with the right
//!   number of arguments (at most 8, in `a0-a7`).
//! * Output: assembly merlin-as accepts: `.text`, `.globl name` and `name:` for
//!   every function with a body; prototypes emit nothing (the linker finds them).
//! * Calling convention: RISC-V ILP32. Arguments in `a0-a7`, result in `a0`,
//!   `ra`, `sp`, `s0-s11` preserved across calls, `sp` 16-byte aligned.
//! * Semantics: 32-bit wrapping arithmetic, `/` and `%` truncate toward zero
//!   (`div`/`rem`), comparisons and `! && ||` produce 0 or 1, `&&`/`||`
//!   short-circuit, `a = b` evaluates to the new value. Falling off the end of a
//!   function returns 0.
//!
//! ## Checking your work
//!
//! `cargo test -p merlin --test harness -- --ignored` compiles every program in
//! `tests/cases/`, assembles it with crt0, links, runs it in Grail and compares
//! the exit code with the `// expect:` line. See the harness's module docs.

use crate::ast::Program;
use crate::{Error, Pos};

/// Prefix for "not written yet" errors.
pub const UNIMPLEMENTED: &str = "not implemented yet: ";

pub fn generate(_prog: &Program) -> Result<String, Error> {
    Err(Error::new(
        Pos::default(),
        format!("{UNIMPLEMENTED}code generation (#14)"),
    ))
}
