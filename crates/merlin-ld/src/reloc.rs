//! # Relocation application: the hook issue #7 fills in
//!
//! The linker calls [`apply`] once for every entry of every `.rela.*` section
//! that patches an allocated section, after layout, with every address final:
//!
//! * `ty`: the `R_RISCV_*` number (constants below, psABI numbering).
//! * `place` (P): the run-time address of the bytes being patched.
//! * `sym` (S): the final address of the relocation's symbol. For
//!   `R_RISCV_PCREL_LO12_*` that symbol is the label on the matching `auipc`,
//!   so S is that auipc's address; find its `R_RISCV_PCREL_HI20` with
//!   [`Context::pcrel_hi20`].
//! * `addend` (A): `r_addend`.
//! * `bytes`: the output section's bytes starting at `place`. `bytes[0..4]` is
//!   the instruction or word to patch (`bytes[4..8]` is the `jalr` of a `CALL`
//!   pair). Instructions are little-endian.
//!
//! Return `Ok(())` once the bytes are patched, or a [`RelocError`]; the linker
//! turns it into `file.o: relocation R_RISCV_JAL out of range for 'foo'` and
//! friends. Spec §10 lists the types merlin-ld must handle; anything else is
//! [`RelocError::Unsupported`].
//!
//! Until #7 lands, every relocation is [`RelocError::NotImplemented`], so a link
//! that needs one fails loudly instead of producing a broken executable.

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

/// The psABI name of a relocation type, for diagnostics.
pub fn name(ty: u32) -> String {
    let n = match ty {
        R_RISCV_NONE => "R_RISCV_NONE",
        R_RISCV_32 => "R_RISCV_32",
        R_RISCV_BRANCH => "R_RISCV_BRANCH",
        R_RISCV_JAL => "R_RISCV_JAL",
        R_RISCV_CALL => "R_RISCV_CALL",
        R_RISCV_CALL_PLT => "R_RISCV_CALL_PLT",
        R_RISCV_GOT_HI20 => "R_RISCV_GOT_HI20",
        R_RISCV_PCREL_HI20 => "R_RISCV_PCREL_HI20",
        R_RISCV_PCREL_LO12_I => "R_RISCV_PCREL_LO12_I",
        R_RISCV_PCREL_LO12_S => "R_RISCV_PCREL_LO12_S",
        R_RISCV_HI20 => "R_RISCV_HI20",
        R_RISCV_LO12_I => "R_RISCV_LO12_I",
        R_RISCV_LO12_S => "R_RISCV_LO12_S",
        R_RISCV_ALIGN => "R_RISCV_ALIGN",
        R_RISCV_RVC_BRANCH => "R_RISCV_RVC_BRANCH",
        R_RISCV_RVC_JUMP => "R_RISCV_RVC_JUMP",
        R_RISCV_RELAX => "R_RISCV_RELAX",
        _ => return format!("relocation type {ty}"),
    };
    n.to_string()
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
    /// Relocation processing is not written yet (issue #7).
    NotImplemented,
    /// A type merlin-ld does not handle (e.g. GOT or compressed relocations).
    Unsupported,
    /// The value doesn't fit the field (`BRANCH` ±4 KiB, `JAL` ±1 MiB, ...).
    OutOfRange,
    /// A `PCREL_LO12_*` whose symbol is not the place of any `PCREL_HI20`.
    MissingPcrelHi20,
    /// Anything else, with a message.
    Other(String),
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
    // TODO(#7): patch `bytes` for each type in spec §10. Until then no
    // relocation is applied, and saying so is better than a silent no-op.
    let _ = (ty, place, sym, addend, bytes, ctx);
    Err(RelocError::NotImplemented)
}
