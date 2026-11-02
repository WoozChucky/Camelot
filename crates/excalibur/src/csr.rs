//! CSR addresses and status-register bits.

pub const SSTATUS: u16 = 0x100;
pub const SIE: u16 = 0x104;
pub const STVEC: u16 = 0x105;
pub const SCOUNTEREN: u16 = 0x106;
pub const SENVCFG: u16 = 0x10a;
pub const SSCRATCH: u16 = 0x140;
pub const SEPC: u16 = 0x141;
pub const SCAUSE: u16 = 0x142;
pub const STVAL: u16 = 0x143;
pub const SIP: u16 = 0x144;
pub const SATP: u16 = 0x180;

pub const MSTATUS: u16 = 0x300;
pub const MISA: u16 = 0x301;
pub const MEDELEG: u16 = 0x302;
pub const MIDELEG: u16 = 0x303;
pub const MIE: u16 = 0x304;
pub const MTVEC: u16 = 0x305;
pub const MCOUNTEREN: u16 = 0x306;
pub const MENVCFG: u16 = 0x30a;
pub const MSTATUSH: u16 = 0x310;
pub const MENVCFGH: u16 = 0x31a;
pub const MCOUNTINHIBIT: u16 = 0x320;
pub const MSCRATCH: u16 = 0x340;
pub const MEPC: u16 = 0x341;
pub const MCAUSE: u16 = 0x342;
pub const MTVAL: u16 = 0x343;
pub const MIP: u16 = 0x344;
// Smrnmi (resumable NMI). Grail does not implement these; riscv-tests
// touches mnstatus, so the disassembler knows the names.
pub const MNSCRATCH: u16 = 0x740;
pub const MNEPC: u16 = 0x741;
pub const MNCAUSE: u16 = 0x742;
pub const MNSTATUS: u16 = 0x744;
pub const PMPCFG0: u16 = 0x3a0;
pub const PMPADDR0: u16 = 0x3b0;

pub const MCYCLE: u16 = 0xb00;
pub const MINSTRET: u16 = 0xb02;
pub const MCYCLEH: u16 = 0xb80;
pub const MINSTRETH: u16 = 0xb82;
pub const CYCLE: u16 = 0xc00;
pub const TIME: u16 = 0xc01;
pub const INSTRET: u16 = 0xc02;
pub const CYCLEH: u16 = 0xc80;
pub const TIMEH: u16 = 0xc81;
pub const INSTRETH: u16 = 0xc82;

pub const MVENDORID: u16 = 0xf11;
pub const MARCHID: u16 = 0xf12;
pub const MIMPID: u16 = 0xf13;
pub const MHARTID: u16 = 0xf14;
pub const MCONFIGPTR: u16 = 0xf15;

// mstatus bits
pub const MSTATUS_SIE: u32 = 1 << 1;
pub const MSTATUS_MIE: u32 = 1 << 3;
pub const MSTATUS_SPIE: u32 = 1 << 5;
pub const MSTATUS_MPIE: u32 = 1 << 7;
pub const MSTATUS_SPP: u32 = 1 << 8;
pub const MSTATUS_MPP_SHIFT: u32 = 11;
pub const MSTATUS_MPP: u32 = 3 << MSTATUS_MPP_SHIFT;
pub const MSTATUS_MPRV: u32 = 1 << 17;
pub const MSTATUS_SUM: u32 = 1 << 18;
pub const MSTATUS_MXR: u32 = 1 << 19;
pub const MSTATUS_TVM: u32 = 1 << 20;
pub const MSTATUS_TW: u32 = 1 << 21;
pub const MSTATUS_TSR: u32 = 1 << 22;

// Interrupt bits (mip/mie and cause numbers).
pub const IRQ_S_SOFT: u32 = 1;
pub const IRQ_M_SOFT: u32 = 3;
pub const IRQ_S_TIMER: u32 = 5;
pub const IRQ_M_TIMER: u32 = 7;
pub const IRQ_S_EXT: u32 = 9;
pub const IRQ_M_EXT: u32 = 11;

/// CSR address for a name (the inverse of [`name`]), for the assembler.
pub fn parse(name: &str) -> Option<u16> {
    (0..4096u16).find(|&a| self::name(a) == Some(name))
}

/// Human-readable name for disassembly.
pub fn name(addr: u16) -> Option<&'static str> {
    Some(match addr {
        SSTATUS => "sstatus",
        SIE => "sie",
        STVEC => "stvec",
        SCOUNTEREN => "scounteren",
        SENVCFG => "senvcfg",
        SSCRATCH => "sscratch",
        SEPC => "sepc",
        SCAUSE => "scause",
        STVAL => "stval",
        SIP => "sip",
        SATP => "satp",
        MSTATUS => "mstatus",
        MISA => "misa",
        MEDELEG => "medeleg",
        MIDELEG => "mideleg",
        MIE => "mie",
        MTVEC => "mtvec",
        MCOUNTEREN => "mcounteren",
        MENVCFG => "menvcfg",
        MSTATUSH => "mstatush",
        MENVCFGH => "menvcfgh",
        MCOUNTINHIBIT => "mcountinhibit",
        MSCRATCH => "mscratch",
        MEPC => "mepc",
        MCAUSE => "mcause",
        MTVAL => "mtval",
        MIP => "mip",
        MNSCRATCH => "mnscratch",
        MNEPC => "mnepc",
        MNCAUSE => "mncause",
        MNSTATUS => "mnstatus",
        MCYCLE => "mcycle",
        MINSTRET => "minstret",
        MCYCLEH => "mcycleh",
        MINSTRETH => "minstreth",
        CYCLE => "cycle",
        TIME => "time",
        INSTRET => "instret",
        CYCLEH => "cycleh",
        TIMEH => "timeh",
        INSTRETH => "instreth",
        MVENDORID => "mvendorid",
        MARCHID => "marchid",
        MIMPID => "mimpid",
        MHARTID => "mhartid",
        MCONFIGPTR => "mconfigptr",
        a if (PMPCFG0..PMPCFG0 + 4).contains(&a) => PMPCFG_NAMES[(a - PMPCFG0) as usize],
        a if (PMPADDR0..PMPADDR0 + 16).contains(&a) => PMPADDR_NAMES[(a - PMPADDR0) as usize],
        _ => return None,
    })
}

const PMPCFG_NAMES: [&str; 4] = ["pmpcfg0", "pmpcfg1", "pmpcfg2", "pmpcfg3"];
const PMPADDR_NAMES: [&str; 16] = [
    "pmpaddr0",
    "pmpaddr1",
    "pmpaddr2",
    "pmpaddr3",
    "pmpaddr4",
    "pmpaddr5",
    "pmpaddr6",
    "pmpaddr7",
    "pmpaddr8",
    "pmpaddr9",
    "pmpaddr10",
    "pmpaddr11",
    "pmpaddr12",
    "pmpaddr13",
    "pmpaddr14",
    "pmpaddr15",
];

#[cfg(test)]
mod tests {
    #[test]
    fn names_round_trip() {
        for a in 0..4096u16 {
            if let Some(n) = super::name(a) {
                assert_eq!(super::parse(n), Some(a), "{n}");
            }
        }
        assert_eq!(super::name(0x3a1), Some("pmpcfg1"));
        assert_eq!(super::name(0x3bf), Some("pmpaddr15"));
        assert_eq!(super::parse("pmpaddr3"), Some(0x3b3));
        assert_eq!(super::name(0x7c0), None);
    }
}
