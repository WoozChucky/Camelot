//! Exception and interrupt causes (spec section 6).

/// The bit of `xcause` that marks an interrupt: its top bit, bit 63.
pub const INTERRUPT_BIT: u64 = 1 << 63;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum Exception {
    InstrMisaligned = 0,
    InstrAccessFault = 1,
    IllegalInstr = 2,
    Breakpoint = 3,
    LoadMisaligned = 4,
    LoadAccessFault = 5,
    StoreMisaligned = 6,
    StoreAccessFault = 7,
    EcallU = 8,
    EcallS = 9,
    EcallM = 11,
    InstrPageFault = 12,
    LoadPageFault = 13,
    StorePageFault = 15,
}

/// A synchronous exception plus the value that goes into `xtval`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Trap {
    pub cause: Exception,
    pub tval: u64,
}

impl Trap {
    pub fn new(cause: Exception, tval: u64) -> Self {
        Trap { cause, tval }
    }
}

/// Kind of memory access, used to pick the right fault cause.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    Fetch,
    Load,
    Store,
}

impl Access {
    pub fn misaligned(self) -> Exception {
        match self {
            Access::Fetch => Exception::InstrMisaligned,
            Access::Load => Exception::LoadMisaligned,
            Access::Store => Exception::StoreMisaligned,
        }
    }
    pub fn access_fault(self) -> Exception {
        match self {
            Access::Fetch => Exception::InstrAccessFault,
            Access::Load => Exception::LoadAccessFault,
            Access::Store => Exception::StoreAccessFault,
        }
    }
    pub fn page_fault(self) -> Exception {
        match self {
            Access::Fetch => Exception::InstrPageFault,
            Access::Load => Exception::LoadPageFault,
            Access::Store => Exception::StorePageFault,
        }
    }
}
