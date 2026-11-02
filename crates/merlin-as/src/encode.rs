//! # The encoder
//!
//! Turns one instruction (mnemonic + parsed operands) into machine words.
//! Everything around it (lexing, parsing, labels, sections, directives,
//! relocations, ELF output) is already done; this function is the whole ISA.
//!
//! ## Contract
//!
//! * Return one [`Emit`] per 32-bit word, in order. Pseudo-instructions return
//!   several (`li` big constants, `call`, `la`, ...).
//! * The number of words must not depend on a label's address (the assembler is
//!   single pass). It may depend on constants: `li a0, 5` is 1 word,
//!   `li a0, 0x12345678` is 2.
//! * Operands referring to a label become a fixup instead of an immediate: leave
//!   the immediate bits zero and attach `Emit::with_fixup(word, kind, target)`.
//!   Use [`Ctx::constant`] first; if that's `None`, try [`Ctx::symbol`].
//! * For the low half of a pc-relative pair (`la`, `lw a0, sym`), target the
//!   auipc with `Target::Auipc(index_of_the_auipc_word)`.
//! * Return `Err(message)` for anything wrong (bad operand count or type,
//!   immediate out of range). The assembler adds the line number.
//!
//! ## Helpers you'll want to write
//!
//! `r_type`, `i_type`, `s_type`, `b_type`, `u_type`, `j_type` that pack fields
//! into a `u32` (the inverse of `excalibur::decode`'s `imm_*` functions), plus
//! range checks (`fits_signed(v, bits)`), `hi20`/`lo12` splitting for `li`
//! (remember lo12 is sign-extended, so hi20 needs `+ 0x800`).
//!
//! ## Checking your work
//!
//! `cargo test -p merlin-as --test encode_golden` compares against 118 lines
//! assembled by llvm-mc (`tests/golden/encode.golden`). It prints a progress
//! report; mnemonics you haven't done yet are counted, not failed. Run it with
//! `-- --ignored` to require everything. See `ENCODER.md` for the suggested order.

use crate::ast::Operand;
#[allow(unused_imports)]
use crate::reloc::{Emit, RelocKind, Target};
use crate::Ctx;

/// Prefix for "not written yet" errors. The golden test counts these separately.
pub const UNIMPLEMENTED: &str = "not implemented yet: ";

pub fn encode(mnemonic: &str, ops: &[Operand], _ctx: &Ctx) -> Result<Vec<Emit>, String> {
    match mnemonic {
        // Worked example: an R-type instruction.
        //   funct7 | rs2 | rs1 | funct3 | rd | opcode
        "add" => {
            let [Operand::Reg(rd), Operand::Reg(rs1), Operand::Reg(rs2)] = ops else {
                return Err("add expects: add rd, rs1, rs2".into());
            };
            let word = (*rs2 as u32) << 20 | (*rs1 as u32) << 15 | (*rd as u32) << 7 | 0b0110011;
            Ok(vec![Emit::word(word)])
        }
        _ => Err(format!("{UNIMPLEMENTED}{mnemonic}")),
    }
}
