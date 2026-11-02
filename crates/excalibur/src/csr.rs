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
        _ => return None,
    })
}
