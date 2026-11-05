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
//! | `R_RISCV_64`             | S + A              | the whole 64-bit doubleword              |
//! | `R_RISCV_BRANCH`         | S + A - P          | B-type imm, ±4 KiB, even                 |
//! | `R_RISCV_JAL`            | S + A - P          | J-type imm, ±1 MiB, even                 |
//! | `R_RISCV_CALL(_PLT)`     | S + A - P          | `auipc` U-imm (hi) + `jalr` I-imm (lo)   |
//! | `R_RISCV_PCREL_HI20`     | S + A - P          | U-type imm, the high part                |
//! | `R_RISCV_PCREL_LO12_I/S` | its HI20's S + A - P | I- or S-type imm, the low part         |
//! | `R_RISCV_HI20`           | S + A              | U-type imm, the high part                |
//! | `R_RISCV_LO12_I/S`       | S + A              | I- or S-type imm, the low part           |
//! | `R_RISCV_ADD8/16/32/64`  | field + (S + A)    | a byte, half, word or doubleword         |
//! | `R_RISCV_SUB8/16/32/64`  | field - (S + A)    | a byte, half, word or doubleword         |
//! | `R_RISCV_SUB6`           | field - (S + A)    | the low 6 bits of a byte                 |
//! | `R_RISCV_SET6`           | S + A              | the low 6 bits of a byte                 |
//! | `R_RISCV_SET8/16/32`     | S + A              | a byte, half or word                     |
//! | `R_RISCV_32_PCREL`       | S + A - P          | the whole 32-bit word                    |
//! | `R_RISCV_RELAX`          | (none)             | nothing: a hint, merlin-ld never relaxes |
//! | `R_RISCV_NONE`           | (none)             | nothing                                  |
//!
//! `R_RISCV_64` is `.dword sym` (`.quad`, `.8byte`): a pointer, and the
//! addresses in debug information.
//!
//! The `ADD`/`SUB` pairs are label differences the assembler could not
//! work out (`.dword g - .`, `.word end - start` across sections), the
//! `SET`s and `SUB6` what DWARF call frame information uses, and
//! `R_RISCV_32_PCREL` a pc-relative pointer (clang's `.eh_frame` with
//! `-funwind-tables`); LLVM emits them with `-mno-relax` too.
//!
//! `R_RISCV_ALIGN` (padding that relaxation may shrink) is refused. ld.lld
//! deletes the padding beyond what the alignment needs, even with
//! `--no-relax`, moving what follows; merlin-ld does not, so objects must be
//! built with `-mno-relax`, which makes clang pad with plain `nop`s and emit
//! no `R_RISCV_ALIGN`. (With relaxation, LLVM 23 emits one only when a
//! linker-relaxable instruction, a `call` or an `la`, comes before the
//! padding in its section: before that, nothing can shrink, so the
//! assembler knows the padding's size.) Everything else (GOT, TLS,
//! compressed, `SET_ULEB128`/`SUB_ULEB128`) is [`RelocError::Unsupported`].
//!
//! Two decisions are made by the caller, [`crate::relocate`], because they
//! need the object file: a `PCREL_LO12`'s auipc label must be in its own
//! section, and a jump or call to an undefined weak symbol gets S = P (it
//! targets itself, as in ld.lld, since address 0 is out of a branch's reach).
//!
//! ## Arithmetic
//!
//! S and P are `u64` addresses and A an `i64` addend, and every value is
//! computed the way the hardware adds: S + A (and S + A - P) modulo 2^64, as
//! `auipc` and `jal` do and as ld.lld computes, then read as a signed `i64`
//! for the range checks. Each field then has its own rule:
//!
//! * **The `hi`/`lo` pairs** (`lui`/`auipc` + `addi`/`lw`/`sw`/`jalr`) build
//!   a 32-bit value: `hi << 12` plus the sign-extended 12-bit `lo`. `lui` and
//!   `auipc` sign-extend `hi << 12` to 64 bits, so a pair only reaches the
//!   values from -2^31 - 0x800 to 2^31 - 0x801. The window is ±2 GiB moved
//!   down by 0x800 because [`hi20`] rounds up when the low part is negative:
//!   0x7fff_f800 would need `hi` = 0x80000, which `lui` sign-extends to
//!   -2^31. `R_RISCV_HI20` (on S + A) and `PCREL_HI20`,
//!   `CALL` and `CALL_PLT` (on S + A - P) are checked against it, as
//!   ld.lld's `checkInt((v + 0x800) >> 12, 20)`, and the error gives that
//!   20-bit value, as ld.lld does (`524288 is not in [-524288, 524287]`).
//!   The `LO12` halves are never checked: their `HI20` is. So `lui %hi(x)`
//!   only reaches about the lowest and the highest 2 GiB, and code linked at
//!   RAM addresses (0x8000_0000) must be pc-relative (`-mcmodel=medany`),
//!   which reaches ±2 GiB around itself.
//! * **`R_RISCV_BRANCH` and `R_RISCV_JAL`**: ±4 KiB and ±1 MiB. Offsets wrap
//!   at 2^64 (a jump from 0 to the top of memory is -16). The error gives
//!   ld.lld's range, a 13- or 21-bit number (`4096 is not in [-4096,
//!   4095]`, though only even offsets can be encoded), and an offset that is
//!   also odd gets the misalignment error too, after it, as in ld.lld.
//! * **`SET32` and `32_PCREL`** must fit 32 bits read as signed, as ld.lld
//!   checks them; the other `ADD`, `SUB` and `SET` types keep the low bits of
//!   their value, unchecked.
//! * **`R_RISCV_32`** must fit 32 bits read as unsigned (an address) or as
//!   signed (`.word sym - 0x1000` below a small address). S + A is read as
//!   signed, so a higher-half address 0xffff_ffff_8xxx_xxxx is
//!   -0x7xxx_xxxx and fits: what clang's `-mcmodel=medlow` jump tables hold
//!   in a kernel linked there, and what `lw` sign-extends back. ld.lld never
//!   checks it (it keeps the low 32 bits); merlin-ld refuses a value that
//!   would change.
//! * **`R_RISCV_64`** holds any value and is never checked.
//!
//! (Until #159 merlin-ld linked RV32 too, whose pairs wrap at 2^32 and whose
//! debug sections ld.lld sign-extends from 32 bits.)

use std::collections::HashMap;

pub const R_RISCV_NONE: u32 = 0;
pub const R_RISCV_32: u32 = 1;
pub const R_RISCV_64: u32 = 2;
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
pub const R_RISCV_ADD8: u32 = 33;
pub const R_RISCV_ADD16: u32 = 34;
pub const R_RISCV_ADD32: u32 = 35;
pub const R_RISCV_ADD64: u32 = 36;
pub const R_RISCV_SUB8: u32 = 37;
pub const R_RISCV_SUB16: u32 = 38;
pub const R_RISCV_SUB32: u32 = 39;
pub const R_RISCV_SUB64: u32 = 40;
pub const R_RISCV_ALIGN: u32 = 43;
pub const R_RISCV_RVC_BRANCH: u32 = 44;
pub const R_RISCV_RVC_JUMP: u32 = 45;
pub const R_RISCV_RELAX: u32 = 51;
pub const R_RISCV_SUB6: u32 = 52;
pub const R_RISCV_SET6: u32 = 53;
pub const R_RISCV_SET8: u32 = 54;
pub const R_RISCV_SET16: u32 = 55;
pub const R_RISCV_SET32: u32 = 56;
pub const R_RISCV_32_PCREL: u32 = 57;

/// The psABI name of a relocation type, for diagnostics (from the table
/// shared with merlin-objdump in [`elf::reloc_name`]).
pub fn name(ty: u32) -> String {
    elf::reloc_name(ty).map_or_else(|| format!("relocation type {ty}"), str::to_string)
}

/// One `R_RISCV_PCREL_HI20` site, resolved: what a `PCREL_LO12` needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HiSite {
    /// S: the HI20's symbol address.
    pub sym: u64,
    /// A: the HI20's addend.
    pub addend: i64,
}

/// Link-wide facts a single relocation may need beyond its own fields.
#[derive(Debug)]
pub struct Context {
    /// Every `R_RISCV_PCREL_HI20` in the link, keyed by its place (the `auipc`'s
    /// address). The HI20's P is the key itself.
    pub pcrel_hi20: HashMap<u64, HiSite>,
}

impl Context {
    /// A context with no PCREL_HI20 sites yet.
    pub fn new() -> Context {
        Context {
            pcrel_hi20: HashMap::new(),
        }
    }
}

impl Default for Context {
    fn default() -> Self {
        Context::new()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RelocError {
    /// A type merlin-ld does not handle (GOT, TLS, compressed, ...).
    Unsupported,
    /// `R_RISCV_ALIGN`: padding ld.lld deletes down to what the alignment
    /// needs (also with `--no-relax`), which merlin-ld does not do.
    AlignPadding,
    /// The value doesn't fit the field: it must lie in `min..=max`.
    OutOfRange { value: i64, min: i64, max: i64 },
    /// A branch or jump offset that is not a multiple of `align` bytes.
    Misaligned { value: i64, align: i64 },
    /// Both: a branch or jump offset out of range and odd, which ld.lld
    /// reports as two errors, the range first.
    OutOfRangeMisaligned {
        value: i64,
        min: i64,
        max: i64,
        align: i64,
    },
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
/// which is `(value + 0x800) >> 12`, kept to the 20 bits of the field (only
/// the low 32 bits of `value` reach them). Which values a pair can build is
/// [`apply`]'s check.
pub fn hi20(value: u64) -> u32 {
    ((value.wrapping_add(0x800) >> 12) & 0xf_ffff) as u32
}

/// The low 12 bits for the `addi`/load/store/`jalr` half of a pair. Only the
/// low 12 bits of `value` matter: `lo12` + `hi20 << 12` == `value` once `lo12`
/// is sign-extended, as [`hi20`] explains.
pub fn lo12(value: u64) -> u32 {
    (value & 0xfff) as u32
}

/// The range check of a `hi`/`lo` pair's value `v` (S + A for `HI20`,
/// S + A - P for `PCREL_HI20` and `CALL(_PLT)`), as the module documentation
/// explains: the high part `(v + 0x800) >> 12` must be a signed 20-bit
/// number, or `lui`/`auipc` would sign-extend it into a different value. That
/// is ld.lld's `checkInt(hi, 20)`, and the error gives `hi`, as ld.lld's does.
fn check_hi_window(v: i64) -> Result<(), RelocError> {
    // Arithmetic shift: the sign of v + 0x800 stays. The addition wraps as
    // ld.lld's does on its uint64_t (only for v within 0x800 of i64::MAX, far
    // outside the window anyway).
    let hi = v.wrapping_add(0x800) >> 12;
    let (min, max) = (-(1i64 << 19), (1i64 << 19) - 1);
    if hi < min || hi > max {
        return Err(RelocError::OutOfRange { value: hi, min, max });
    }
    Ok(())
}

// ---------------------------------------------------------------------------

fn read32(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

fn write32(bytes: &mut [u8], at: usize, v: u32) {
    bytes[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

/// Checks that `value` is a signed `bits`-bit even number: the range of a
/// B-type (13 bits) or J-type (21 bits) offset. As ld.lld's `checkInt` and
/// `checkAlignment`, the range is that of a `bits`-bit number (its largest
/// value odd, so out of reach), and an offset both out of range and odd is
/// two errors.
fn check_offset(value: i64, bits: u32) -> Result<u32, RelocError> {
    let (min, max) = (-(1i64 << (bits - 1)), (1i64 << (bits - 1)) - 1);
    let odd = value % 2 != 0;
    if value < min || value > max {
        return Err(if odd {
            RelocError::OutOfRangeMisaligned {
                value,
                min,
                max,
                align: 2,
            }
        } else {
            RelocError::OutOfRange { value, min, max }
        });
    }
    if odd {
        return Err(RelocError::Misaligned { value, align: 2 });
    }
    Ok(value as u32)
}

/// Reads a little-endian field of 1, 2, 4 or 8 bytes.
fn read_le(field: &[u8]) -> u64 {
    field.iter().rev().fold(0, |v, &b| v << 8 | u64::from(b))
}

/// Writes the low `field.len()` bytes of `v`, little-endian.
fn write_le(field: &mut [u8], v: u64) {
    let n = field.len();
    field.copy_from_slice(&v.to_le_bytes()[..n]);
}

/// The data relocations that compute a field from its old contents and a
/// value, or set it: label differences (`ADD`/`SUB` pairs) and `SET`s, and
/// `R_RISCV_32_PCREL`. Their field's size in bytes.
fn data_field(ty: u32) -> Option<usize> {
    Some(match ty {
        R_RISCV_ADD8 | R_RISCV_SUB8 | R_RISCV_SUB6 | R_RISCV_SET6 | R_RISCV_SET8 => 1,
        R_RISCV_ADD16 | R_RISCV_SUB16 | R_RISCV_SET16 => 2,
        R_RISCV_ADD32 | R_RISCV_SUB32 | R_RISCV_SET32 | R_RISCV_32_PCREL => 4,
        R_RISCV_ADD64 | R_RISCV_SUB64 => 8,
        _ => return None,
    })
}

/// Patches `bytes` for one relocation. See the module documentation.
///
/// `place`, `sym` and `addend` are P, S and A. (A debug relocation
/// against discarded code has S = -A, as a `u64`, so that S + A is 0: see
/// [`crate::relocate`].)
pub fn apply(
    ty: u32,
    place: u64,
    sym: u64,
    addend: i64,
    bytes: &mut [u8],
    ctx: &Context,
) -> Result<(), RelocError> {
    // The two values of every formula, S + A and S + A - P, modulo 2^64 as
    // the hardware adds, read as signed for the range checks.
    let abs = sym.wrapping_add(addend as u64);
    let pcrel = abs.wrapping_sub(place);
    let (abs_i, pcrel_i) = (abs as i64, pcrel as i64);

    // How many bytes each type patches; check them before touching anything.
    let need = match ty {
        R_RISCV_NONE | R_RISCV_RELAX => return Ok(()),
        R_RISCV_ALIGN => return Err(RelocError::AlignPadding),
        R_RISCV_64 | R_RISCV_CALL | R_RISCV_CALL_PLT => 8,
        R_RISCV_32 | R_RISCV_BRANCH | R_RISCV_JAL | R_RISCV_PCREL_HI20 | R_RISCV_PCREL_LO12_I
        | R_RISCV_PCREL_LO12_S | R_RISCV_HI20 | R_RISCV_LO12_I | R_RISCV_LO12_S => 4,
        _ => data_field(ty).ok_or(RelocError::Unsupported)?,
    };
    if bytes.len() < need {
        return Err(RelocError::Truncated {
            need,
            have: bytes.len(),
        });
    }

    // Label differences and set values, ld.lld's read-modify-write of the
    // field, cut to its width: `.dword a - b` is an R_RISCV_ADD64 of a and
    // an R_RISCV_SUB64 of b at the same place (LLVM emits the pair when the
    // difference is not known when assembling, with -mno-relax too: b in
    // another section, or `.`), which add S + A to the field and subtract
    // the other S + A. SUB6 and SET6 change only the low 6 bits of their
    // byte (DWARF call frame instructions). The SETs write S + A, and
    // R_RISCV_32_PCREL S + A - P (a pc-relative `.eh_frame` pointer). Only
    // SET32 and 32_PCREL are checked, as signed 32-bit numbers, as ld.lld's
    // `checkInt(val, 32)`; the others keep the low bits.
    if let Some(n) = data_field(ty) {
        let v = abs;
        let field = &mut bytes[..n];
        let old = read_le(field);
        let new = match ty {
            R_RISCV_ADD8 | R_RISCV_ADD16 | R_RISCV_ADD32 | R_RISCV_ADD64 => old.wrapping_add(v),
            R_RISCV_SUB8 | R_RISCV_SUB16 | R_RISCV_SUB32 | R_RISCV_SUB64 => old.wrapping_sub(v),
            R_RISCV_SUB6 => (old & 0xc0) | (old.wrapping_sub(v) & 0x3f),
            R_RISCV_SET6 => (old & 0xc0) | (v & 0x3f),
            R_RISCV_SET8 | R_RISCV_SET16 => v,
            _ => {
                let v = if ty == R_RISCV_32_PCREL { pcrel } else { v };
                let (min, max) = (i32::MIN as i64, i32::MAX as i64);
                if (v as i64) < min || (v as i64) > max {
                    return Err(RelocError::OutOfRange {
                        value: v as i64,
                        min,
                        max,
                    });
                }
                v
            }
        };
        write_le(field, new);
        return Ok(());
    }

    // `.dword sym + A`: a doubleword holding an address, never out of range.
    if ty == R_RISCV_64 {
        bytes[..8].copy_from_slice(&abs.to_le_bytes());
        return Ok(());
    }

    let inst = read32(bytes, 0);
    let patched = match ty {
        // A data word holding an address: `.word sym + A`. It must fit 32
        // bits, read either as unsigned (an address) or signed (a negative
        // `.word sym - 0x1000` below a small address wraps the same way, and
        // on RV64 so does a higher-half address, 0xffff_ffff_8xxx_xxxx).
        R_RISCV_32 => {
            let (min, max) = (i32::MIN as i64, u32::MAX as i64);
            if abs_i < min || abs_i > max {
                return Err(RelocError::OutOfRange {
                    value: abs_i,
                    min,
                    max,
                });
            }
            abs as u32
        }

        // `beq a0, a1, target`: a pc-relative offset in 13 signed bits,
        // so the target lies within -4096..=4094 bytes of the branch.
        R_RISCV_BRANCH => set_b(inst, check_offset(pcrel_i, 13)?),

        // `jal ra, target`: a pc-relative offset in 21 signed bits,
        // -1 MiB..=1 MiB - 2 around the jump.
        R_RISCV_JAL => set_j(inst, check_offset(pcrel_i, 21)?),

        // `call target` is `auipc ra, %hi(off)` + `jalr ra, %lo(off)(ra)`
        // with off = S + A - P, P being the auipc's address. The jalr adds
        // its offset to ra (= P + hi), not to its own pc, so both halves use
        // the same off. The relocation sits on the auipc and patches both.
        // R_RISCV_CALL_PLT is what clang emits for `call`; without shared
        // libraries there is no PLT and it means the same thing. The range
        // is checked before the jalr is written, so that on error neither
        // word changes.
        R_RISCV_CALL | R_RISCV_CALL_PLT => {
            check_hi_window(pcrel_i)?;
            let jalr = read32(bytes, 4);
            write32(bytes, 4, set_i(jalr, lo12(pcrel)));
            set_u(inst, hi20(pcrel))
        }

        // `auipc rd, %pcrel_hi(sym)`: the high part of the distance from this
        // auipc to the symbol. A `PCREL_LO12` pointing at this auipc's label
        // supplies the low part (below).
        R_RISCV_PCREL_HI20 => {
            check_hi_window(pcrel_i)?;
            set_u(inst, hi20(pcrel))
        }

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
        //
        // Not range-checked: the PCREL_HI20 is.
        R_RISCV_PCREL_LO12_I | R_RISCV_PCREL_LO12_S => {
            if addend != 0 {
                return Err(RelocError::PcrelLo12Addend);
            }
            let hi = ctx.pcrel_hi20.get(&sym).ok_or(RelocError::MissingPcrelHi20)?;
            let value = hi.sym.wrapping_add(hi.addend as u64).wrapping_sub(sym);
            if ty == R_RISCV_PCREL_LO12_I {
                set_i(inst, lo12(value))
            } else {
                set_s(inst, lo12(value))
            }
        }

        // `lui rd, %hi(sym)` + `addi rd, rd, %lo(sym)`: an absolute address
        // in two halves, split as for the pc-relative pair. The HI20 carries
        // the range check of the pair.
        R_RISCV_HI20 => {
            check_hi_window(abs_i)?;
            set_u(inst, hi20(abs))
        }
        R_RISCV_LO12_I => set_i(inst, lo12(abs)),
        R_RISCV_LO12_S => set_s(inst, lo12(abs)),

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

    /// [`run64`] for places and symbols below 4 GiB, written as 32-bit
    /// numbers (most of these tests' addresses are in RAM at 0x8000_0000).
    fn run(ty: u32, place: u32, sym: u32, addend: i32, words: &[u32]) -> Result<Vec<u32>, RelocError> {
        run_ctx(ty, place, sym, addend, words, &Context::new())
    }

    fn run_ctx(
        ty: u32,
        place: u32,
        sym: u32,
        addend: i32,
        words: &[u32],
        ctx: &Context,
    ) -> Result<Vec<u32>, RelocError> {
        apply_words(ty, place.into(), sym.into(), addend.into(), words, ctx)
    }

    /// [`apply`] on `words`, little-endian, returning them patched; a
    /// relocation that fails must leave every word as it was.
    fn apply_words(
        ty: u32,
        place: u64,
        sym: u64,
        addend: i64,
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
    /// `excalibur`) reads them back: `lui`/`auipc`'s value sign-extended to
    /// 64 bits, as an RV64 hart has it, which is what limits a pair's reach.
    fn imm(word: u32) -> i64 {
        use excalibur::Inst;
        match excalibur::decode(word, excalibur::Xlen::Rv64) {
            Inst::Lui { imm, .. } | Inst::Auipc { imm, .. } => imm as i64,
            Inst::Jal { imm, .. } | Inst::Jalr { imm, .. } | Inst::Branch { imm, .. } => imm as i64,
            Inst::Load { imm, .. } | Inst::Store { imm, .. } | Inst::OpImm { imm, .. } => imm as i64,
            other => panic!("unexpected {other:?}"),
        }
    }

    /// What a hi/lo pair computes at run time, `hi + sext(lo)`, mod 2^32:
    /// enough for the addresses of the tests that use [`run`] (`pair64` has
    /// the 64 bits).
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
            let v: u32 = v;
            let lo = (lo12(v.into()) << 20) as i32 >> 20;
            assert_eq!((hi20(v.into()) << 12).wrapping_add(lo as u32), v, "{v:#x}");
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
        // ±4 KiB, ld.lld's range (a 13-bit number).
        assert_eq!(
            run(R_RISCV_BRANCH, p, p + 4096, 0, &[BEQ_A0_A1]),
            Err(RelocError::OutOfRange {
                value: 4096,
                min: -4096,
                max: 4095
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
        // Out of range and odd: both, as ld.lld reports them.
        assert_eq!(
            run(R_RISCV_BRANCH, p, p - 4097, 0, &[BEQ_A0_A1]),
            Err(RelocError::OutOfRangeMisaligned {
                value: -4097,
                min: -4096,
                max: 4095,
                align: 2
            })
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
                max: 0xf_ffff
            })
        );
        assert_eq!(
            run(R_RISCV_JAL, p, p + 0x10_0001, 0, &[JAL_RA]),
            Err(RelocError::OutOfRangeMisaligned {
                value: 0x10_0001,
                min: -0x10_0000,
                max: 0xf_ffff,
                align: 2
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
        ] {
            let w = run(R_RISCV_PCREL_HI20, auipc, s, a, &[AUIPC_A0]).unwrap();
            let mut ctx = Context::new();
            ctx.pcrel_hi20.insert(
                auipc.into(),
                HiSite {
                    sym: s.into(),
                    addend: a.into(),
                },
            );
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
        let mut ctx = Context::new();
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
        // Below 2 GiB, in the window of `lui` (absolute_pair_rv64_window has
        // its edges).
        for (s, a) in [
            (0x0123_1234u32, 0),
            (0x0123_1800, 0), // rounding edge
            (0x0123_1fff, 0),
            (0x7000_0000, -1),
            (0x1000_0000, 5),
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
        assert_eq!(apply(R_RISCV_RELAX, 0, 0, 0, &mut [], &Context::new()), Ok(()));
        assert_eq!(run(R_RISCV_NONE, 0, 0, 0, &[7]).unwrap(), [7]);
        assert_eq!(
            run(R_RISCV_ALIGN, 0, 0, 4, &[0x13]),
            Err(RelocError::AlignPadding)
        );
        for ty in [
            R_RISCV_GOT_HI20,
            R_RISCV_RVC_BRANCH,
            R_RISCV_RVC_JUMP,
            60, // R_RISCV_SET_ULEB128
            61, // R_RISCV_SUB_ULEB128
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
            apply(R_RISCV_32, 0, 0, 0, &mut two, &Context::new()),
            Err(RelocError::Truncated { need: 4, have: 2 })
        );
    }

    // -----------------------------------------------------------------------
    // 64-bit places and symbols, the hi/lo window, R_RISCV_64.

    fn run64(ty: u32, place: u64, sym: u64, addend: i64, words: &[u32]) -> Result<Vec<u32>, RelocError> {
        apply_words(ty, place, sym, addend, words, &Context::new())
    }

    /// What a hi/lo pair computes on RV64: `sext(hi) + sext(lo)`, mod 2^64.
    fn pair64(hi: u32, lo: u32) -> u64 {
        (imm(hi) as u64).wrapping_add(imm(lo) as u64)
    }

    /// The error of a pair out of RV64's window: ld.lld's numbers, the high
    /// part `(v + 0x800) >> 12` and the signed 20-bit range.
    fn window_error(hi: i64) -> Result<Vec<u32>, RelocError> {
        Err(RelocError::OutOfRange {
            value: hi,
            min: -524288,
            max: 524287,
        })
    }

    /// The doubleword at the start of `words`.
    fn dword(words: &[u32]) -> u64 {
        u64::from(words[0]) | u64::from(words[1]) << 32
    }

    /// `lui %hi(x)` + `addi`/`sw %lo(x)`: the window is -2^31 - 0x800 ..
    /// 2^31 - 0x801, ld.lld's edges (not "a sign-extended 32-bit value":
    /// 0x7fff_f800 is one and is refused, 0xffff_ffff_7fff_f800 is not and
    /// is accepted).
    #[test]
    fn absolute_pair_rv64_window() {
        for s in [
            0u64,
            0x1000_0000,
            0x7fff_f7ff,
            0xffff_ffff_7fff_f800,
            0xffff_ffff_8000_0000,
            0xffff_ffff_ffff_f800,
            0xffff_ffff_ffff_ffff,
        ] {
            let hi = run64(R_RISCV_HI20, 0x8000_0000, s, 0, &[LUI_A0]).unwrap();
            let lo_i = run64(R_RISCV_LO12_I, 0x8000_0004, s, 0, &[ADDI_A0]).unwrap();
            let lo_s = run64(R_RISCV_LO12_S, 0x8000_0008, s, 0, &[SW_A1_A0]).unwrap();
            assert_eq!(pair64(hi[0], lo_i[0]), s, "{s:#x}");
            assert_eq!(pair64(hi[0], lo_s[0]), s, "{s:#x}");
        }
        // The addend counts: S + A.
        let hi = run64(R_RISCV_HI20, 0, 0x7fff_f000, 0x7ff, &[LUI_A0]).unwrap();
        let lo = run64(R_RISCV_LO12_I, 0, 0x7fff_f000, 0x7ff, &[ADDI_A0]).unwrap();
        assert_eq!(pair64(hi[0], lo[0]), 0x7fff_f7ff);
        for (s, a, hi) in [
            (0x7fff_f800u64, 0, 524288),
            (0x7fff_f000, 0x800, 524288),
            (0x8000_0000, 0, 524288),
            (0xffff_ffff_7fff_f7ff, 0, -524289),
            (0x1_0000_0000, 0, 1048576),
        ] {
            assert_eq!(
                run64(R_RISCV_HI20, 0, s, a, &[LUI_A0]),
                window_error(hi),
                "{s:#x}{a:+}"
            );
        }
        // The LO12 halves are never checked: their HI20 is.
        let lo = run64(R_RISCV_LO12_I, 0, 0x1234_5678_9abc, 0, &[ADDI_A0]).unwrap();
        assert_eq!(lo[0] >> 20, 0xabc);
        assert!(run64(R_RISCV_LO12_S, 0, 0x8000_0000, 0, &[SW_A1_A0]).is_ok());
    }

    /// `la`: PCREL_HI20 + PCREL_LO12 reach -2^31 - 0x800 ..= 2^31 - 0x801
    /// from the auipc, modulo 2^64.
    #[test]
    fn pcrel_pair_rv64_window() {
        let p = 0x8000_0000u64;
        for (auipc, s) in [
            (p, p + 0x7fff_f7ff),
            (p, p.wrapping_sub(0x8000_0800)),
            (p, p + 0x2800), // rounding edge
            (p, 0x1000_0000),
            (0xffff_ffff_8000_0000, 0xffff_ffff_8010_0000), // in the higher half
            (0, 0xffff_ffff_ffff_f000),                     // wraps below 0
            (0xffff_ffff_ffff_f000, 0x10),                  // wraps past 2^64
        ] {
            let hi = run64(R_RISCV_PCREL_HI20, auipc, s, 0, &[AUIPC_A0]).unwrap();
            let mut ctx = Context::new();
            ctx.pcrel_hi20.insert(auipc, HiSite { sym: s, addend: 0 });
            let lo_i = apply_words(R_RISCV_PCREL_LO12_I, auipc + 4, auipc, 0, &[ADDI_A0], &ctx).unwrap();
            let lo_s = apply_words(R_RISCV_PCREL_LO12_S, auipc + 8, auipc, 0, &[SW_A1_A0], &ctx).unwrap();
            assert_eq!(
                auipc.wrapping_add(pair64(hi[0], lo_i[0])),
                s,
                "{auipc:#x} -> {s:#x}"
            );
            assert_eq!(
                auipc.wrapping_add(pair64(hi[0], lo_s[0])),
                s,
                "{auipc:#x} -> {s:#x}"
            );
        }
        for (auipc, s, hi) in [
            (p, p + 0x7fff_f800, 524288),
            (p, p.wrapping_sub(0x8000_0801), -524289),
            (p, 0x1_0000_0000, 524288),
            // From the higher half, physical RAM and 0 are out of reach.
            (0xffff_ffff_8000_0000, 0x8000_0000, 1048576),
            (0xffff_ffff_8000_0000, 0, 524288),
        ] {
            assert_eq!(
                run64(R_RISCV_PCREL_HI20, auipc, s, 0, &[AUIPC_A0]),
                window_error(hi),
                "{auipc:#x} -> {s:#x}"
            );
        }
        // The addend is part of the value checked.
        assert_eq!(
            run64(R_RISCV_PCREL_HI20, p, p + 0x7fff_f000, 0x800, &[AUIPC_A0]),
            window_error(524288)
        );
    }

    /// `call`/`tail`: the same window as `la`, checked before either word is
    /// written.
    #[test]
    fn call_rv64_window() {
        let p = 0x8000_0000u64;
        for ty in [R_RISCV_CALL, R_RISCV_CALL_PLT] {
            for (p, s) in [
                (p, p + 0x7fff_f7ff),
                (p, p.wrapping_sub(0x8000_0800)),
                (p, p + 0x800),
                (0, 0xffff_ffff_ffff_f000), // wraps below 0
            ] {
                let w = run64(ty, p, s, 0, &[AUIPC_RA, JALR_RA, 0x1234_5678]).unwrap();
                assert_eq!(p.wrapping_add(pair64(w[0], w[1])), s, "{p:#x} -> {s:#x}");
                assert_eq!(w[0] & 0xfff, AUIPC_RA);
                assert_eq!(w[1] & 0xf_ffff, JALR_RA);
                assert_eq!(w[2], 0x1234_5678, "only two words");
            }
            for (s, hi) in [(p + 0x7fff_f800, 524288), (p.wrapping_sub(0x8000_0801), -524289)] {
                // apply_words checks that a failure writes nothing: neither
                // the auipc nor the jalr (which the patch writes first).
                assert_eq!(
                    run64(ty, p, s, 0, &[AUIPC_RA, JALR_RA]),
                    window_error(hi),
                    "{s:#x}"
                );
            }
        }
        assert_eq!(
            run64(R_RISCV_CALL_PLT, 0, 0, 0, &[AUIPC_RA]),
            Err(RelocError::Truncated { need: 8, have: 4 })
        );
    }

    /// Branches and jumps keep their ranges on RV64, and wrap at 2^64 as the
    /// hardware's pc does: from 0 to 0xffff_ffff_ffff_fff0 is -16.
    #[test]
    fn branch_and_jal_rv64() {
        for (p, s, off) in [
            (0u64, 0xffff_ffff_ffff_fff0u64, -16i64),
            (0xffff_ffff_ffff_fff0, 0x10, 0x20),
            (0xffff_ffff_8000_0000, 0xffff_ffff_8000_0ffe, 0xffe),
        ] {
            let b = run64(R_RISCV_BRANCH, p, s, 0, &[BEQ_A0_A1]).unwrap();
            assert_eq!(imm(b[0]), off, "{p:#x} -> {s:#x}");
            let j = run64(R_RISCV_JAL, p, s, 0, &[JAL_RA]).unwrap();
            assert_eq!(imm(j[0]), off, "{p:#x} -> {s:#x}");
        }
        let p = 0xffff_ffff_8000_0000u64;
        assert_eq!(
            run64(R_RISCV_BRANCH, p, p + 0x1000, 0, &[BEQ_A0_A1]),
            Err(RelocError::OutOfRange {
                value: 4096,
                min: -4096,
                max: 4095
            })
        );
        assert_eq!(
            run64(R_RISCV_JAL, p, p - 0x10_0002, 0, &[JAL_RA]),
            Err(RelocError::OutOfRange {
                value: -0x10_0002,
                min: -0x10_0000,
                max: 0xf_ffff
            })
        );
        // Not wrapped at 2^32: 0 to 0xffff_fff0 is 4 GiB away on RV64.
        assert!(matches!(
            run64(R_RISCV_JAL, 0, 0xffff_fff0, 0, &[JAL_RA]),
            Err(RelocError::OutOfRange {
                value: 0xffff_fff0,
                ..
            })
        ));
    }

    /// `.word sym` on RV64: S + A read as signed must fit 32 bits, so a
    /// higher-half address fits (as -0x8000_0000, which `lw` sign-extends
    /// back) and 4 GiB does not.
    #[test]
    fn abs32_rv64() {
        let w = run64(R_RISCV_32, 0, 0xffff_ffff_8000_0000, 0, &[0]).unwrap();
        assert_eq!(w, [0x8000_0000]);
        let w = run64(R_RISCV_32, 0, 0xffff_ffff_8000_0010, -0x10, &[0]).unwrap();
        assert_eq!(w, [0x8000_0000]);
        let w = run64(R_RISCV_32, 0, 0xffff_ffff, 0, &[0]).unwrap();
        assert_eq!(w, [0xffff_ffff]);
        let w = run64(R_RISCV_32, 0, 0, -1, &[0]).unwrap();
        assert_eq!(w, [0xffff_ffff]);
        let (min, max) = (-0x8000_0000, 0xffff_ffff);
        assert_eq!(
            run64(R_RISCV_32, 0, 0xffff_ffff_7fff_ffff, 0, &[0]),
            Err(RelocError::OutOfRange {
                value: -0x8000_0001,
                min,
                max
            })
        );
        assert_eq!(
            run64(R_RISCV_32, 0, 0x1_0000_0000, 0, &[0]),
            Err(RelocError::OutOfRange {
                value: 0x1_0000_0000,
                min,
                max
            })
        );
    }

    /// `.dword sym + A` on RV64: S + A modulo 2^64, all 8 bytes, never out of
    /// range, the same in loaded and in debug sections.
    #[test]
    fn abs64_rv64() {
        let debug = Context::new();
        for (s, a, want) in [
            (0x1234_5678_9abc_def0u64, 1i64, 0x1234_5678_9abc_def1u64),
            (0xffff_ffff_ffff_ffff, 2, 1),
            (0xffff_ffff_8000_0000, 0, 0xffff_ffff_8000_0000),
            (0x8000_0000, -0x8000_0001, 0xffff_ffff_ffff_ffff),
            (0, 0, 0),
        ] {
            let w = run64(R_RISCV_64, 0x8000_0000, s, a, &[0xdead_beef, 0xdead_beef, 7]).unwrap();
            assert_eq!(dword(&w), want, "{s:#x}{a:+}");
            assert_eq!(w[2], 7, "only 8 bytes");
            let w = apply_words(R_RISCV_64, 0, s, a, &[0, 0], &debug).unwrap();
            assert_eq!(dword(&w), want, "{s:#x}{a:+}, debug");
        }
        assert_eq!(
            run64(R_RISCV_64, 0, 0, 0, &[0]),
            Err(RelocError::Truncated { need: 8, have: 4 })
        );
        let mut seven = [0u8; 7];
        assert_eq!(
            apply(R_RISCV_64, 0, 0, 0, &mut seven, &debug),
            Err(RelocError::Truncated { need: 8, have: 7 })
        );
    }

    /// A debug relocation against discarded code has S = -A modulo 2^64
    /// (relocate.rs): R_RISCV_32 and R_RISCV_64 both write 0, on RV64 too.
    #[test]
    fn discarded_code_writes_zero_rv64() {
        let debug = Context::new();
        for a in [0i64, 4, 0x7fff_ffff] {
            let s = 0u64.wrapping_sub(a as u64);
            let w = apply_words(R_RISCV_32, 0, s, a, &[1], &debug).unwrap();
            assert_eq!(w, [0]);
            let w = apply_words(R_RISCV_64, 0, s, a, &[1, 1], &debug).unwrap();
            assert_eq!(dword(&w), 0);
        }
    }

    /// `apply` on a byte field: the bytes after it (0xee) stay.
    fn bytes(ty: u32, p: u64, s: u64, a: i64, old: &[u8], ctx: &Context) -> Result<Vec<u8>, RelocError> {
        let mut b = old.to_vec();
        b.push(0xee);
        apply(ty, p, s, a, &mut b, ctx)?;
        assert_eq!(b.pop(), Some(0xee));
        Ok(b)
    }

    /// The ADD/SUB/SET types and R_RISCV_32_PCREL, as ld.lld 23 applies them
    /// to clang's `.dword g - .`, `.word g - .`, `.half g - .`, `.byte g - .`
    /// and `.reloc`s of the others, with g = 0x100 and the data from
    /// 0x8000_0004.
    #[test]
    fn label_differences_and_sets() {
        let ctx = Context::new();
        let diff = |add, sub, p: u64, n: usize| {
            let b = bytes(add, p, 0x100, 0, &vec![0; n], &ctx).unwrap();
            bytes(sub, p, p, 0, &b, &ctx).unwrap()
        };
        let p = 0x8000_0004;
        assert_eq!(
            diff(R_RISCV_ADD64, R_RISCV_SUB64, p, 8),
            0xffff_ffff_8000_00fcu64.to_le_bytes()
        );
        assert_eq!(diff(R_RISCV_ADD32, R_RISCV_SUB32, p + 8, 4), [0xf4, 0, 0, 0x80]);
        assert_eq!(diff(R_RISCV_ADD16, R_RISCV_SUB16, p + 12, 2), [0xf0, 0]);
        assert_eq!(diff(R_RISCV_ADD8, R_RISCV_SUB8, p + 14, 1), [0xee]);
        let one = |ty, p, old: &[u8]| bytes(ty, p, 0x100, 4, old, &ctx);
        assert_eq!(
            one(R_RISCV_32_PCREL, p + 15, &[0; 4]).unwrap(),
            [0xf1, 0, 0, 0x80]
        );
        assert_eq!(one(R_RISCV_SET32, p + 19, &[0x11; 4]).unwrap(), [4, 1, 0, 0]);
        assert_eq!(one(R_RISCV_SET16, p + 23, &[0x11; 2]).unwrap(), [4, 1]);
        assert_eq!(one(R_RISCV_SET8, p + 25, &[0x11]).unwrap(), [4]);
        // The low 6 bits only: 0xff keeps 0xc0 and gets 0x104 & 0x3f;
        // 0xc3 gets (3 - 0x104) & 0x3f.
        assert_eq!(one(R_RISCV_SET6, p + 26, &[0xff]).unwrap(), [0xc4]);
        assert_eq!(one(R_RISCV_SUB6, p + 27, &[0xc3]).unwrap(), [0xff]);
        assert_eq!(bytes(R_RISCV_ADD8, 0, 0x100, 0, &[0xff], &ctx).unwrap(), [0xff]);
        // SET32 and 32_PCREL are signed 32-bit numbers (ld.lld: "2147488312 is not in [-2147483648, 2147483647]");
        // nothing is written.
        let (min, max) = (-0x8000_0000, 0x7fff_ffff);
        assert_eq!(
            bytes(R_RISCV_SET32, p, 0x8000_1234, 4, &[0; 4], &ctx),
            Err(RelocError::OutOfRange {
                value: 0x8000_1238,
                min,
                max
            })
        );
        assert_eq!(
            bytes(R_RISCV_32_PCREL, 0x100, 0x8000_0100, 0, &[0; 4], &ctx),
            Err(RelocError::OutOfRange {
                value: 0x8000_0000,
                min,
                max
            })
        );
        // The other fields keep the low bits, unchecked.
        assert_eq!(
            bytes(R_RISCV_SET16, 0, 0x7fff_1234, 0, &[0; 2], &ctx).unwrap(),
            [0x34, 0x12]
        );
        assert_eq!(
            apply(R_RISCV_ADD16, 0, 2, 0, &mut [0x41], &ctx),
            Err(RelocError::Truncated { need: 2, have: 1 })
        );
    }

    /// In debug information, a label difference is computed in 64 bits:
    /// `.dword g - _start`, g = 0x100, _start = 0x8000_0000, is
    /// 0xffff_ffff_8000_0100.
    #[test]
    fn label_differences_in_debug_information() {
        let pair = |a: u64, b: u64| {
            let ctx = Context::new();
            let f = bytes(R_RISCV_ADD64, 0, a, 0, &[0; 8], &ctx).unwrap();
            u64::from_le_bytes(
                bytes(R_RISCV_SUB64, 0, b, 0, &f, &ctx)
                    .unwrap()
                    .try_into()
                    .unwrap(),
            )
        };
        assert_eq!(pair(0x100, 0x8000_0000), 0xffff_ffff_8000_0100);
        assert_eq!(pair(0x8000_0000, 0), 0x8000_0000);
    }
}
