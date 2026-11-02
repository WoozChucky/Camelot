//! # Relocation application
//!
//! An object file is assembled before anybody knows where its sections will
//! end up, so every instruction or data word that depends on an address is
//! left with a hole (usually zero bits) plus a *relocation*: "at offset
//! `r_offset`, put the value of formula F(S, A, P) into the field of kind
//! `r_type`". Once [`crate::layout`] has fixed every address, the linker
//! calls [`apply`] once per relocation to fill the hole.
//!
//! The three inputs of every formula, with the RISC-V psABI's names:
//!
//! * **S**: the final address of the relocation's symbol. For
//!   `R_RISCV_PCREL_LO12_*` that symbol is the label on the matching `auipc`,
//!   so S is that auipc's address; its `R_RISCV_PCREL_HI20` is found through
//!   [`Context::pcrel_hi20`].
//! * **A**: the addend, `r_addend` (`la a0, msg+8` has A = 8).
//! * **P**: the *place*, the run-time address of the bytes being patched.
//!
//! `bytes` is the input section's contents starting at P: `bytes[0..4]` is the
//! instruction or word to patch, little-endian (`bytes[4..8]` is the `jalr`
//! of a `CALL` pair). On error nothing is written, and the [`RelocError`]
//! comes back to [`crate::relocate`], which names the file, section, offset
//! and symbol: `a.o: relocation R_RISCV_JAL out of range for 'foo' at
//! .text+0x10: ...`.
//!
//! ## What gets patched
//!
//! | type                     | value              | field                                    |
//! |--------------------------|--------------------|------------------------------------------|
//! | `R_RISCV_32`             | S + A              | the whole 32-bit word                    |
//! | `R_RISCV_BRANCH`         | S + A - P          | B-type imm, ±4 KiB, even                 |
//! | `R_RISCV_JAL`            | S + A - P          | J-type imm, ±1 MiB, even                 |
//! | `R_RISCV_CALL(_PLT)`     | S + A - P          | `auipc` U-imm (hi) + `jalr` I-imm (lo)   |
//! | `R_RISCV_PCREL_HI20`     | S + A - P          | U-type imm, the high part                |
//! | `R_RISCV_PCREL_LO12_I/S` | its HI20's S + A - P | I- or S-type imm, the low part         |
//! | `R_RISCV_HI20`           | S + A              | U-type imm, the high part                |
//! | `R_RISCV_LO12_I/S`       | S + A              | I- or S-type imm, the low part           |
//! | `R_RISCV_RELAX`          | (none)             | nothing: a hint, merlin-ld never relaxes |
//! | `R_RISCV_NONE`           | (none)             | nothing                                  |
//!
//! `R_RISCV_ALIGN` (padding that relaxation may shrink) is refused: merlin-ld
//! does no relaxation, so objects must be built with `-mno-relax`, which
//! makes clang pad with plain `nop`s and emit no `R_RISCV_ALIGN`. Everything
//! else (GOT, TLS, compressed, `ADD`/`SUB` pairs) is [`RelocError::Unsupported`].
//!
//! Two decisions are made by the caller, [`crate::relocate`], because they
//! need the object file: a `PCREL_LO12`'s auipc label must be in its own
//! section, and a jump or call to an undefined weak symbol gets S = P (it
//! targets itself, as in ld.lld, since address 0 is out of a branch's reach).
//!
//! ## Arithmetic
//!
//! Values are computed exactly in `i64`, then checked against what the field
//! can hold. The `hi`/`lo` pairs (`lui`/`auipc` + `addi`/`lw`/`sw`/`jalr`)
//! build a full 32-bit value, and on RV32 `lui`, `auipc` and the addition that
//! follows all wrap modulo 2^32, so a pair reaches every address: its value
//! is taken modulo 2^32 and is never out of range (ld.lld does the same on
//! RV32). Only `R_RISCV_32`, `BRANCH` and `JAL` can overflow.

use std::collections::HashMap;

pub const R_RISCV_NONE: u32 = 0;
pub const R_RISCV_32: u32 = 1;
pub const R_RISCV_BRANCH: u32 = 16;
pub const R_RISCV_JAL: u32 = 17;
pub const R_RISCV_CALL: u32 = 18;
pub const R_RISCV_CALL_PLT: u32 = 19;
pub const R_RISCV_GOT_HI20: u32 = 20;
pub const R_RISCV_PCREL_HI20: u32 = 23;
pub const R_RISCV_PCREL_LO12_I: u32 = 24;
pub const R_RISCV_PCREL_LO12_S: u32 = 25;
pub const R_RISCV_HI20: u32 = 26;
pub const R_RISCV_LO12_I: u32 = 27;
pub const R_RISCV_LO12_S: u32 = 28;
pub const R_RISCV_ALIGN: u32 = 43;
pub const R_RISCV_RVC_BRANCH: u32 = 44;
pub const R_RISCV_RVC_JUMP: u32 = 45;
pub const R_RISCV_RELAX: u32 = 51;

/// The psABI name of a relocation type, for diagnostics (from the table
/// shared with merlin-objdump in [`elf32::reloc_name`]).
pub fn name(ty: u32) -> String {
    elf32::reloc_name(ty).map_or_else(|| format!("relocation type {ty}"), str::to_string)
}

/// One `R_RISCV_PCREL_HI20` site, resolved: what a `PCREL_LO12` needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HiSite {
    /// S: the HI20's symbol address.
    pub sym: u32,
    /// A: the HI20's addend.
    pub addend: i32,
}

/// Link-wide facts a single relocation may need beyond its own fields.
#[derive(Debug, Default)]
pub struct Context {
    /// Every `R_RISCV_PCREL_HI20` in the link, keyed by its place (the `auipc`'s
    /// address). The HI20's P is the key itself.
    pub pcrel_hi20: HashMap<u32, HiSite>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RelocError {
    /// A type merlin-ld does not handle (GOT, TLS, compressed, ...).
    Unsupported,
    /// `R_RISCV_ALIGN`: only meaningful with linker relaxation.
    NeedsRelaxation,
    /// The value doesn't fit the field: it must lie in `min..=max`.
    OutOfRange { value: i64, min: i64, max: i64 },
    /// A branch or jump offset that is not a multiple of `align` bytes.
    Misaligned { value: i64, align: i64 },
    /// The patched field runs past the end of its section: it needs `need`
    /// bytes, only `have` are left.
    Truncated { need: usize, have: usize },
    /// A `PCREL_LO12_*` whose symbol is not the place of any `PCREL_HI20`.
    MissingPcrelHi20,
    /// A `PCREL_LO12_*` with a non-zero addend: the offset belongs on the
    /// `PCREL_HI20` (the addend here would be silently dropped otherwise).
    PcrelLo12Addend,
}

// ---------------------------------------------------------------------------
// Instruction fields (spec §3). Each `set_*` replaces one immediate field of
// an instruction word and keeps every other bit (opcode, registers, funct3).

/// U-type (`lui`, `auipc`): imm[31:12] goes to bits 31:12 unchanged.
fn set_u(inst: u32, hi20: u32) -> u32 {
    (inst & 0x0000_0fff) | (hi20 << 12)
}

/// I-type (`addi`, loads, `jalr`): imm[11:0] goes to bits 31:20.
fn set_i(inst: u32, imm: u32) -> u32 {
    (inst & 0x000f_ffff) | ((imm & 0xfff) << 20)
}

/// S-type (stores): imm[11:5] to bits 31:25, imm[4:0] to bits 11:7. Split so
/// that rs1/rs2 (bits 24:15) sit at the same place as in every other format.
fn set_s(inst: u32, imm: u32) -> u32 {
    let imm = imm & 0xfff;
    (inst & 0x01ff_f07f) | ((imm >> 5) << 25) | ((imm & 0x1f) << 7)
}

/// B-type (branches): a 13-bit even offset whose bit 0 is not stored.
/// imm[12] → bit 31, imm[10:5] → bits 30:25, imm[4:1] → bits 11:8,
/// imm[11] → bit 7. Like S-type, but with bit 11 moved down to bit 7 so the
/// sign bit stays at bit 31.
fn set_b(inst: u32, imm: u32) -> u32 {
    let bit = |n: u32| (imm >> n) & 1;
    (inst & 0x01ff_f07f)
        | (bit(12) << 31)
        | (((imm >> 5) & 0x3f) << 25)
        | (((imm >> 1) & 0xf) << 8)
        | (bit(11) << 7)
}

/// J-type (`jal`): a 21-bit even offset whose bit 0 is not stored.
/// imm[20] → bit 31, imm[10:1] → bits 30:21, imm[11] → bit 20,
/// imm[19:12] → bits 19:12 (where U-type keeps them).
fn set_j(inst: u32, imm: u32) -> u32 {
    let bit = |n: u32| (imm >> n) & 1;
    (inst & 0x0000_0fff)
        | (bit(20) << 31)
        | (((imm >> 1) & 0x3ff) << 21)
        | (bit(11) << 20)
        | (((imm >> 12) & 0xff) << 12)
}

// ---------------------------------------------------------------------------
// The %hi / %lo split.

/// The upper 20 bits for `lui`/`auipc` such that adding the *sign-extended*
/// low 12 bits ([`lo12`]) gives `value` back.
///
/// The low half is used by `addi`, `lw`, `sw` and `jalr`, which sign-extend
/// their 12-bit immediate: a low part of 0x800..=0xfff counts as -0x800..=-1.
/// To compensate, the high part is rounded up by one whenever bit 11 is set:
///
/// ```text
/// value = 0x1234_5800  →  hi = 0x12346, lo = 0x800 (= -0x800)
///                         0x1234_6000 + (-0x800) = 0x1234_5800
/// ```
///
/// which is `(value + 0x800) >> 12`. On RV32 the result wraps to 20 bits:
/// for `value` = 0xffff_f800 it is 0x100000 → 0, and `0 + (-0x800)` wraps
/// back to 0xffff_f800, as the hardware computes it.
pub fn hi20(value: u32) -> u32 {
    (value.wrapping_add(0x800) >> 12) & 0xf_ffff
}

/// The low 12 bits for the `addi`/load/store/`jalr` half of a pair. Only the
/// low 12 bits of `value` matter: `lo12` + `hi20 << 12` == `value` once `lo12`
/// is sign-extended, as [`hi20`] explains.
pub fn lo12(value: u32) -> u32 {
    value & 0xfff
}

// ---------------------------------------------------------------------------

fn read32(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

fn write32(bytes: &mut [u8], at: usize, v: u32) {
    bytes[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

/// Checks that `value` is a signed `bits`-bit even number: the range of a
/// B-type (13 bits) or J-type (21 bits) offset.
fn check_offset(value: i64, bits: u32) -> Result<u32, RelocError> {
    let (min, max) = (-(1i64 << (bits - 1)), (1i64 << (bits - 1)) - 1);
    if value < min || value > max {
        return Err(RelocError::OutOfRange {
            value,
            min,
            // The largest *even* offset: an odd one could not be encoded.
            max: max - 1,
        });
    }
    if value % 2 != 0 {
        return Err(RelocError::Misaligned { value, align: 2 });
    }
    Ok(value as u32)
}

/// Patches `bytes` for one relocation. See the module documentation.
pub fn apply(
    ty: u32,
    place: u32,
    sym: u32,
    addend: i32,
    bytes: &mut [u8],
    ctx: &Context,
) -> Result<(), RelocError> {
    let (s, a, p) = (sym as i64, addend as i64, place as i64);

    // How many bytes each type patches; check them before touching anything.
    let need = match ty {
        R_RISCV_NONE | R_RISCV_RELAX => return Ok(()),
        R_RISCV_ALIGN => return Err(RelocError::NeedsRelaxation),
        R_RISCV_CALL | R_RISCV_CALL_PLT => 8,
        R_RISCV_32 | R_RISCV_BRANCH | R_RISCV_JAL | R_RISCV_PCREL_HI20 | R_RISCV_PCREL_LO12_I
        | R_RISCV_PCREL_LO12_S | R_RISCV_HI20 | R_RISCV_LO12_I | R_RISCV_LO12_S => 4,
        _ => return Err(RelocError::Unsupported),
    };
    if bytes.len() < need {
        return Err(RelocError::Truncated {
            need,
            have: bytes.len(),
        });
    }
    let inst = read32(bytes, 0);

    let patched = match ty {
        // A data word holding an address: `.word sym + A`. It must fit 32
        // bits, read either as unsigned (an address) or signed (a negative
        // `.word sym - 0x1000` below a small address wraps the same way).
        R_RISCV_32 => {
            let value = s + a;
            let (min, max) = (i32::MIN as i64, u32::MAX as i64);
            if value < min || value > max {
                return Err(RelocError::OutOfRange { value, min, max });
            }
            value as u32
        }

        // `beq a0, a1, target`: a pc-relative offset in 13 signed bits,
        // so the target lies within -4096..=4094 bytes of the branch.
        R_RISCV_BRANCH => set_b(inst, check_offset(s + a - p, 13)?),

        // `jal ra, target`: a pc-relative offset in 21 signed bits,
        // -1 MiB..=1 MiB - 2 around the jump.
        R_RISCV_JAL => set_j(inst, check_offset(s + a - p, 21)?),

        // `call target` is `auipc ra, %hi(off)` + `jalr ra, %lo(off)(ra)`
        // with off = S + A - P, P being the auipc's address. The jalr adds
        // its offset to ra (= P + hi), not to its own pc, so both halves use
        // the same off. The relocation sits on the auipc and patches both.
        // R_RISCV_CALL_PLT is what clang emits for `call`; without shared
        // libraries there is no PLT and it means the same thing.
        R_RISCV_CALL | R_RISCV_CALL_PLT => {
            let off = (s + a - p) as u32;
            let jalr = read32(bytes, 4);
            write32(bytes, 4, set_i(jalr, lo12(off)));
            set_u(inst, hi20(off))
        }

        // `auipc rd, %pcrel_hi(sym)`: the high part of the distance from this
        // auipc to the symbol. A `PCREL_LO12` pointing at this auipc's label
        // supplies the low part (below).
        R_RISCV_PCREL_HI20 => set_u(inst, hi20((s + a - p) as u32)),

        // `addi rd, rd, %pcrel_lo(label)` / `sw rs, %pcrel_lo(label)(rd)`.
        //
        // The low part must come from the *same* value as the high part, and
        // that value is relative to the auipc, not to this instruction (which
        // may be anywhere after it, even after a branch). So the psABI points
        // this relocation's symbol at the auipc's label, S = the auipc's
        // address, and the linker looks up the R_RISCV_PCREL_HI20 at that
        // address to recompute its S + A - P. `la a0, msg` assembles to:
        //
        //   .Lpcrel_hi0: auipc a0, 0       # PCREL_HI20   msg
        //                addi  a0, a0, 0   # PCREL_LO12_I .Lpcrel_hi0
        R_RISCV_PCREL_LO12_I | R_RISCV_PCREL_LO12_S => {
            if addend != 0 {
                return Err(RelocError::PcrelLo12Addend);
            }
            let hi = ctx.pcrel_hi20.get(&sym).ok_or(RelocError::MissingPcrelHi20)?;
            let value = (hi.sym as i64 + hi.addend as i64 - s) as u32;
            if ty == R_RISCV_PCREL_LO12_I {
                set_i(inst, lo12(value))
            } else {
                set_s(inst, lo12(value))
            }
        }

        // `lui rd, %hi(sym)` + `addi rd, rd, %lo(sym)`: an absolute address
        // in two halves, split as for the pc-relative pair.
        R_RISCV_HI20 => set_u(inst, hi20((s + a) as u32)),
        R_RISCV_LO12_I => set_i(inst, lo12((s + a) as u32)),
        R_RISCV_LO12_S => set_s(inst, lo12((s + a) as u32)),

        _ => unreachable!("filtered above"),
    };
    write32(bytes, 0, patched);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Instruction templates with registers set and immediates zero, as an
    // assembler leaves them.
    const AUIPC_A0: u32 = 0x0000_0517; // auipc a0, 0
    const LUI_A0: u32 = 0x0000_0537; // lui   a0, 0
    const ADDI_A0: u32 = 0x0005_0513; // addi  a0, a0, 0
    const SW_A1_A0: u32 = 0x00b5_2023; // sw    a1, 0(a0)
    const BEQ_A0_A1: u32 = 0x00b5_0063; // beq   a0, a1, .
    const JAL_RA: u32 = 0x0000_00ef; // jal   ra, .
    const AUIPC_RA: u32 = 0x0000_0097; // auipc ra, 0
    const JALR_RA: u32 = 0x0000_80e7; // jalr  ra, 0(ra)

    fn run(ty: u32, place: u32, sym: u32, addend: i32, words: &[u32]) -> Result<Vec<u32>, RelocError> {
        run_ctx(ty, place, sym, addend, words, &Context::default())
    }

    fn run_ctx(
        ty: u32,
        place: u32,
        sym: u32,
        addend: i32,
        words: &[u32],
        ctx: &Context,
    ) -> Result<Vec<u32>, RelocError> {
        let mut bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        let before = bytes.clone();
        let r = apply(ty, place, sym, addend, &mut bytes, ctx);
        if r.is_err() {
            assert_eq!(bytes, before, "a failed relocation must not write");
        }
        r.map(|()| bytes.chunks(4).map(|c| read32(c, 0)).collect())
    }

    /// Immediate fields as the decoder (an independent implementation in
    /// `excalibur`) reads them back.
    fn imm(word: u32) -> i64 {
        use excalibur::Inst;
        match excalibur::decode(word) {
            Inst::Lui { imm, .. } | Inst::Auipc { imm, .. } => imm as i32 as i64,
            Inst::Jal { imm, .. } | Inst::Jalr { imm, .. } | Inst::Branch { imm, .. } => imm as i64,
            Inst::Load { imm, .. } | Inst::Store { imm, .. } | Inst::OpImm { imm, .. } => imm as i64,
            other => panic!("unexpected {other:?}"),
        }
    }

    /// What a hi/lo pair computes at run time: `hi + sext(lo)`, mod 2^32.
    fn pair(hi: u32, lo: u32) -> u32 {
        (imm(hi) as u32).wrapping_add(imm(lo) as u32)
    }

    #[test]
    fn split_round_trips() {
        for v in [
            0,
            1,
            0x7ff,
            0x800,
            0xfff,
            0x1000,
            0x8000_0000,
            0x8000_07ff,
            0x8000_0800,
            0x1234_5fff,
            0x7fff_f800,
            0xffff_f800,
            0xffff_ffff,
        ] {
            let lo = (lo12(v) << 20) as i32 >> 20;
            assert_eq!((hi20(v) << 12).wrapping_add(lo as u32), v, "{v:#x}");
        }
        // The rounding edge: low 12 bits >= 0x800 bump the high part.
        assert_eq!(hi20(0x1234_57ff), 0x12345);
        assert_eq!(hi20(0x1234_5800), 0x12346);
        assert_eq!(hi20(0xffff_f800), 0, "wraps to 20 bits");
    }

    #[test]
    fn abs32() {
        let w = run(R_RISCV_32, 0x8000_0100, 0x8000_1000, 0x10, &[0]).unwrap();
        assert_eq!(w, [0x8000_1010]);
        // The old contents are replaced, not added to (RELA, not REL).
        let w = run(R_RISCV_32, 0, 0x1234, 0, &[0xdead_beef]).unwrap();
        assert_eq!(w, [0x1234]);
        // Negative addend, and negative values that fit as signed.
        let w = run(R_RISCV_32, 0, 0x8000_1000, -0x1000, &[0]).unwrap();
        assert_eq!(w, [0x8000_0000]);
        let w = run(R_RISCV_32, 0, 0x10, -0x20, &[0]).unwrap();
        assert_eq!(w, [0xffff_fff0]);
        assert_eq!(
            run(R_RISCV_32, 0, 0xffff_fff0, 0x20, &[0]),
            Err(RelocError::OutOfRange {
                value: 0x1_0000_0010,
                min: -0x8000_0000,
                max: 0xffff_ffff
            })
        );
    }

    #[test]
    fn branch() {
        let p = 0x8000_1000;
        for off in [8i64, -8, 4094, -4096, 0x7fe, -0x800, 2, 0] {
            let w = run(R_RISCV_BRANCH, p, (p as i64 + off) as u32, 0, &[BEQ_A0_A1]).unwrap();
            assert_eq!(imm(w[0]), off, "{off}");
            assert_eq!(w[0] & 0x01ff_f07f, BEQ_A0_A1, "registers and funct3 kept");
        }
        // Addend counts: S + A - P.
        let w = run(R_RISCV_BRANCH, p, p, -12, &[BEQ_A0_A1]).unwrap();
        assert_eq!(imm(w[0]), -12);
        // ±4 KiB.
        assert_eq!(
            run(R_RISCV_BRANCH, p, p + 4096, 0, &[BEQ_A0_A1]),
            Err(RelocError::OutOfRange {
                value: 4096,
                min: -4096,
                max: 4094
            })
        );
        assert!(matches!(
            run(R_RISCV_BRANCH, p, p - 4098, 0, &[BEQ_A0_A1]),
            Err(RelocError::OutOfRange { value: -4098, .. })
        ));
        assert_eq!(
            run(R_RISCV_BRANCH, p, p + 7, 0, &[BEQ_A0_A1]),
            Err(RelocError::Misaligned { value: 7, align: 2 })
        );
    }

    #[test]
    fn jal() {
        let p = 0x8010_0000;
        for off in [
            4i64, -4, 0x800, -0x800, 0x1000, 0xf_fffe, -0x10_0000, 0x7_ff00, -0x8_1234,
        ] {
            let w = run(R_RISCV_JAL, p, (p as i64 + off) as u32, 0, &[JAL_RA]).unwrap();
            assert_eq!(imm(w[0]), off, "{off}");
            assert_eq!(w[0] & 0xfff, JAL_RA, "rd and opcode kept");
        }
        assert_eq!(
            run(R_RISCV_JAL, p, p + 0x10_0000, 0, &[JAL_RA]),
            Err(RelocError::OutOfRange {
                value: 0x10_0000,
                min: -0x10_0000,
                max: 0xf_fffe
            })
        );
        assert!(matches!(
            run(R_RISCV_JAL, p, p - 0x10_0002, 0, &[JAL_RA]),
            Err(RelocError::OutOfRange { .. })
        ));
        assert_eq!(
            run(R_RISCV_JAL, p, p + 1, 0, &[JAL_RA]),
            Err(RelocError::Misaligned { value: 1, align: 2 })
        );
    }

    #[test]
    fn call_patches_auipc_and_jalr() {
        for ty in [R_RISCV_CALL, R_RISCV_CALL_PLT] {
            for (p, s) in [
                (0x8000_0000u32, 0x8000_0010u32),
                (0x8000_1000, 0x8000_0010), // backward
                (0x8000_0000, 0x8000_0800), // low part 0x800: hi rounds up
                (0x8000_0000, 0x8123_4fff), // far forward
                (0x8000_0000, 0x0000_1000), // far backward
                (0x0000_0000, 0xffff_f000), // wraps around 4 GiB
            ] {
                let w = run(ty, p, s, 0, &[AUIPC_RA, JALR_RA, 0x1234_5678]).unwrap();
                assert_eq!(p.wrapping_add(pair(w[0], w[1])), s, "{p:#x} -> {s:#x}");
                assert_eq!(w[0] & 0xfff, AUIPC_RA);
                assert_eq!(w[1] & 0xf_ffff, JALR_RA);
                assert_eq!(w[2], 0x1234_5678, "only two words");
            }
            // The call goes to S + A: addends that push the offset across
            // the %hi rounding edge (low part 0x800) in either direction.
            let (p, s) = (0x8000_0000u32, 0x8000_1004u32);
            for a in [4, -4, 0x7fc, -0x804] {
                let w = run(ty, p, s, a, &[AUIPC_RA, JALR_RA]).unwrap();
                let to = p.wrapping_add(pair(w[0], w[1]));
                assert_eq!(to, s.wrapping_add(a as u32), "{p:#x} -> {s:#x}{a:+}");
            }
        }
        let w = run(
            R_RISCV_CALL_PLT,
            0x8000_0000,
            0x8000_0800,
            0,
            &[AUIPC_RA, JALR_RA],
        )
        .unwrap();
        assert_eq!((imm(w[0]), imm(w[1])), (0x1000, -0x800));
        assert_eq!(
            run(R_RISCV_CALL, 0, 0, 0, &[AUIPC_RA]),
            Err(RelocError::Truncated { need: 8, have: 4 })
        );
    }

    #[test]
    fn pcrel_pair() {
        // `la a0, sym+A` at auipc address `auipc`, the addi 4 bytes later.
        for (auipc, s, a) in [
            (0x8000_0000u32, 0x8000_2000u32, 0),
            (0x8000_0000, 0x8000_27ff, 0),
            (0x8000_0000, 0x8000_2800, 0), // rounding edge
            (0x8000_0000, 0x8000_2ffc, 4),
            (0x8000_4000, 0x8000_0000, 0), // negative distance
            (0x8000_4000, 0x8000_0000, -0x801),
            (0x0000_1000, 0xffff_f800, 0),
        ] {
            let w = run(R_RISCV_PCREL_HI20, auipc, s, a, &[AUIPC_A0]).unwrap();
            let mut ctx = Context::default();
            ctx.pcrel_hi20.insert(auipc, HiSite { sym: s, addend: a });
            let lo_i = run_ctx(R_RISCV_PCREL_LO12_I, auipc + 4, auipc, 0, &[ADDI_A0], &ctx).unwrap();
            let lo_s = run_ctx(R_RISCV_PCREL_LO12_S, auipc + 8, auipc, 0, &[SW_A1_A0], &ctx).unwrap();
            let target = s.wrapping_add(a as u32);
            assert_eq!(
                auipc.wrapping_add(pair(w[0], lo_i[0])),
                target,
                "{auipc:#x} {s:#x}{a:+}"
            );
            assert_eq!(auipc.wrapping_add(pair(w[0], lo_s[0])), target);
            assert_eq!(lo_i[0] & 0xf_ffff, ADDI_A0);
            assert_eq!(lo_s[0] & 0x01ff_f07f, SW_A1_A0);
        }
    }

    #[test]
    fn pcrel_lo12_uses_the_hi20_not_its_own_place() {
        // The addi is far from the auipc (after a loop, say): its own P must
        // not enter the computation.
        let mut ctx = Context::default();
        ctx.pcrel_hi20.insert(
            0x8000_0000,
            HiSite {
                sym: 0x8000_0abc,
                addend: 0,
            },
        );
        let w = run_ctx(
            R_RISCV_PCREL_LO12_I,
            0x8000_0400,
            0x8000_0000,
            0,
            &[ADDI_A0],
            &ctx,
        )
        .unwrap();
        assert_eq!(imm(w[0]), 0xabc - 0x1000);
        assert_eq!(
            run_ctx(
                R_RISCV_PCREL_LO12_I,
                0x8000_0400,
                0x8000_0004,
                0,
                &[ADDI_A0],
                &ctx
            ),
            Err(RelocError::MissingPcrelHi20)
        );
        assert_eq!(
            run_ctx(
                R_RISCV_PCREL_LO12_S,
                0x8000_0400,
                0x8000_0000,
                4,
                &[SW_A1_A0],
                &ctx
            ),
            Err(RelocError::PcrelLo12Addend)
        );
    }

    #[test]
    fn absolute_pair() {
        for (s, a) in [
            (0x8000_1234u32, 0),
            (0x8000_1800, 0), // rounding edge
            (0x8000_1fff, 0),
            (0x8000_0000, -1),
            (0x1000_0000, 5),
            (0xffff_f800, 0),
            (0, 0), // an undefined weak symbol
        ] {
            let hi = run(R_RISCV_HI20, 0x8000_0000, s, a, &[LUI_A0]).unwrap();
            let lo_i = run(R_RISCV_LO12_I, 0x8000_0004, s, a, &[ADDI_A0]).unwrap();
            let lo_s = run(R_RISCV_LO12_S, 0x8000_0008, s, a, &[SW_A1_A0]).unwrap();
            let target = s.wrapping_add(a as u32);
            assert_eq!(pair(hi[0], lo_i[0]), target, "{s:#x}{a:+}");
            assert_eq!(pair(hi[0], lo_s[0]), target);
            assert_eq!(hi[0] & 0xfff, LUI_A0);
        }
        let hi = run(R_RISCV_HI20, 0, 0x1234_5800, 0, &[LUI_A0]).unwrap();
        assert_eq!(imm(hi[0]), 0x1234_6000);
        let lo = run(R_RISCV_LO12_I, 0, 0x1234_5800, 0, &[ADDI_A0]).unwrap();
        assert_eq!(imm(lo[0]), -0x800);
    }

    #[test]
    fn hints_and_unsupported() {
        // R_RISCV_RELAX and NONE change nothing, even with no bytes left.
        assert_eq!(
            run(R_RISCV_RELAX, 0, 0x1234, 0, &[0xffff_ffff]).unwrap(),
            [0xffff_ffff]
        );
        assert_eq!(
            apply(R_RISCV_RELAX, 0, 0, 0, &mut [], &Context::default()),
            Ok(())
        );
        assert_eq!(run(R_RISCV_NONE, 0, 0, 0, &[7]).unwrap(), [7]);
        assert_eq!(
            run(R_RISCV_ALIGN, 0, 0, 4, &[0x13]),
            Err(RelocError::NeedsRelaxation)
        );
        for ty in [
            R_RISCV_GOT_HI20,
            R_RISCV_RVC_BRANCH,
            R_RISCV_RVC_JUMP,
            35,
            39,
            200,
        ] {
            assert_eq!(run(ty, 0, 0, 0, &[0]), Err(RelocError::Unsupported), "{ty}");
        }
        assert_eq!(
            run(R_RISCV_JAL, 0, 0, 0, &[]),
            Err(RelocError::Truncated { need: 4, have: 0 })
        );
        let mut two = [0u8; 2];
        assert_eq!(
            apply(R_RISCV_32, 0, 0, 0, &mut two, &Context::default()),
            Err(RelocError::Truncated { need: 4, have: 2 })
        );
    }
}
