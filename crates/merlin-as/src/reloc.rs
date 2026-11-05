//! What the encoder hands back to the assembler: instruction words, each
//! optionally carrying a fixup that becomes an ELF relocation.

/// The subset of RISC-V ELF relocations we emit (psABI numbering).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RelocKind {
    /// 32-bit absolute address: `.word sym`, and the 4-byte DWARF32 string
    /// offsets (`.debug_line_str`).
    Abs32,
    /// 64-bit absolute address: `.dword sym`, and DWARF addresses.
    Abs64,
    /// B-type branch offset (beq, bne, ...).
    Branch,
    /// J-type jump offset (jal).
    Jal,
    /// An `auipc` + `jalr` pair (call, tail). Attached to the auipc word.
    CallPlt,
    /// `auipc` upper 20 bits of a pc-relative address.
    PcrelHi20,
    /// I-type low 12 bits matching a PcrelHi20. Its target is the auipc, not the symbol.
    PcrelLo12I,
    /// S-type low 12 bits matching a PcrelHi20.
    PcrelLo12S,
    /// `lui` upper 20 bits of an absolute address (`%hi`).
    Hi20,
    /// I-type low 12 bits of an absolute address (`%lo`).
    Lo12I,
    /// S-type low 12 bits of an absolute address (`%lo` in a store).
    Lo12S,
}

impl RelocKind {
    /// Every kind, for tables generated from this one (the C assembler's),
    /// in [`RelocKind::index`] order: a new kind needs an arm there, which
    /// the compiler asks for, and a place here, which the test checks.
    /// The order is the psABI's numbering, so the C header lists them that way.
    pub const ALL: [RelocKind; 11] = [
        RelocKind::Abs32,
        RelocKind::Abs64,
        RelocKind::Branch,
        RelocKind::Jal,
        RelocKind::CallPlt,
        RelocKind::PcrelHi20,
        RelocKind::PcrelLo12I,
        RelocKind::PcrelLo12S,
        RelocKind::Hi20,
        RelocKind::Lo12I,
        RelocKind::Lo12S,
    ];

    /// The kind's place in [`RelocKind::ALL`].
    pub fn index(self) -> usize {
        match self {
            RelocKind::Abs32 => 0,
            RelocKind::Abs64 => 1,
            RelocKind::Branch => 2,
            RelocKind::Jal => 3,
            RelocKind::CallPlt => 4,
            RelocKind::PcrelHi20 => 5,
            RelocKind::PcrelLo12I => 6,
            RelocKind::PcrelLo12S => 7,
            RelocKind::Hi20 => 8,
            RelocKind::Lo12I => 9,
            RelocKind::Lo12S => 10,
        }
    }

    pub fn elf_type(self) -> u32 {
        match self {
            RelocKind::Abs32 => 1,
            RelocKind::Abs64 => 2,
            RelocKind::Branch => 16,
            RelocKind::Jal => 17,
            RelocKind::CallPlt => 19,
            RelocKind::PcrelHi20 => 23,
            RelocKind::PcrelLo12I => 24,
            RelocKind::PcrelLo12S => 25,
            RelocKind::Hi20 => 26,
            RelocKind::Lo12I => 27,
            RelocKind::Lo12S => 28,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            RelocKind::Abs32 => "R_RISCV_32",
            RelocKind::Abs64 => "R_RISCV_64",
            RelocKind::Branch => "R_RISCV_BRANCH",
            RelocKind::Jal => "R_RISCV_JAL",
            RelocKind::CallPlt => "R_RISCV_CALL_PLT",
            RelocKind::PcrelHi20 => "R_RISCV_PCREL_HI20",
            RelocKind::PcrelLo12I => "R_RISCV_PCREL_LO12_I",
            RelocKind::PcrelLo12S => "R_RISCV_PCREL_LO12_S",
            RelocKind::Hi20 => "R_RISCV_HI20",
            RelocKind::Lo12I => "R_RISCV_LO12_I",
            RelocKind::Lo12S => "R_RISCV_LO12_S",
        }
    }
}

/// What a fixup points at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// A symbol plus a constant addend. Get these from [`crate::Ctx::symbol`].
    Symbol { name: String, addend: i64 },
    /// The `auipc` at this index in the same `Vec<Emit>`. Used by the low half of a
    /// pc-relative pair (`la`, `lw a0, sym`, ...): the assembler puts a hidden label
    /// on that auipc and points the relocation at it, as the psABI requires.
    Auipc(usize),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fixup {
    pub kind: RelocKind,
    pub target: Target,
}

/// One encoded 32-bit instruction. Fields that a fixup will fill in are left as zero.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Emit {
    pub word: u32,
    pub fixup: Option<Fixup>,
}

impl Emit {
    pub fn word(word: u32) -> Emit {
        Emit { word, fixup: None }
    }

    pub fn with_fixup(word: u32, kind: RelocKind, target: Target) -> Emit {
        Emit {
            word,
            fixup: Some(Fixup { kind, target }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_lists_every_kind_once() {
        for (i, k) in RelocKind::ALL.iter().enumerate() {
            assert_eq!(k.index(), i, "{}", k.name());
        }
        // Every index has its kind: none is missing from ALL.
        let max = RelocKind::ALL.iter().map(|k| k.index()).max().unwrap();
        assert_eq!(max + 1, RelocKind::ALL.len());
        // In psABI number order, as the comment on ALL says.
        assert!(RelocKind::ALL
            .windows(2)
            .all(|w| w[0].elf_type() < w[1].elf_type()));
    }
}
