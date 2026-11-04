//! The target description the stub sends a client (`qXfer:features:read:target.xml`,
//! GDB manual, "Target Descriptions"): which registers there are, their names, sizes
//! and numbers in `p`/`P` packets.
//!
//! Every register is as wide as the machine's harts: 32 bits on RV32, 64 on RV64, as
//! GDB's own RISC-V descriptions have them (`riscv:rv32` and `riscv:rv64`).
//!
//! The numbers are GDB's for RISC-V, so a client without the description agrees with
//! them: `x0`-`x31` are 0-31, `pc` 32 (in a `g` packet, these 33 come in this order),
//! the floating-point registers 33-64 would come next (Excalibur has none), and CSR
//! `n` is 65 + `n`. `priv`, the hart's privilege mode (0 U, 1 S, 3 M), is GDB's
//! "virtual" register, numbered after the last CSR.

use crate::{Machine, Xlen};
use std::fmt::Write;

/// The `x` registers and `pc`, the ones a `g` packet carries.
pub const GENERAL_REGS: usize = 33;
pub const PC: usize = 32;
/// The register number of CSR 0.
pub const CSR_BASE: usize = 65;
/// The `priv` register: after the 4096 CSR numbers.
pub const PRIV: usize = CSR_BASE + 4096;

/// What a register number names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reg {
    X(usize),
    Pc,
    Csr(u16),
    Priv,
}

impl Reg {
    pub fn from_number(n: usize) -> Option<Reg> {
        match n {
            0..=31 => Some(Reg::X(n)),
            PC => Some(Reg::Pc),
            _ if (CSR_BASE..PRIV).contains(&n) => Some(Reg::Csr((n - CSR_BASE) as u16)),
            PRIV => Some(Reg::Priv),
            _ => None,
        }
    }
}

/// The bytes of one register of `m`'s harts: 4, or 8 on RV64.
pub fn reg_bytes(m: &Machine) -> usize {
    m.config().xlen.bits() as usize / 8
}

/// The CSRs a client is told about: those Grail implements that have a name for the
/// machine's width (RV64 has no `cycleh`).
pub fn csrs(m: &Machine) -> Vec<(u16, &'static str)> {
    let xlen = m.config().xlen;
    (0..4096u16)
        .filter(|&a| m.csr(a).is_some())
        .filter_map(|a| excalibur::csr::name(a, xlen).map(|n| (a, n)))
        .collect()
}

/// The target description of `m`'s harts (all alike).
pub fn xml(m: &Machine) -> String {
    let bits = m.config().xlen.bits();
    let arch = match m.config().xlen {
        Xlen::Rv32 => "riscv:rv32",
        Xlen::Rv64 => "riscv:rv64",
    };
    let mut x = format!(
        "<?xml version=\"1.0\"?>\n<!DOCTYPE target SYSTEM \"gdb-target.dtd\">\n<target version=\"1.0\">\n\
         <architecture>{arch}</architecture>\n<feature name=\"org.gnu.gdb.riscv.cpu\">\n",
    );
    for r in 0..32u8 {
        let ty = match r {
            1 => "code_ptr",
            2 | 8 => "data_ptr",
            _ => "int",
        };
        let name = excalibur::reg::abi_name(r);
        // The ABI name of x8 is s0; GDB calls it fp.
        let name = if r == 8 { "fp" } else { name };
        let _ = writeln!(
            x,
            "<reg name=\"{name}\" bitsize=\"{bits}\" type=\"{ty}\" regnum=\"{r}\"/>"
        );
    }
    let _ = writeln!(
        x,
        "<reg name=\"pc\" bitsize=\"{bits}\" type=\"code_ptr\" regnum=\"{PC}\"/>"
    );
    x.push_str("</feature>\n<feature name=\"org.gnu.gdb.riscv.csr\">\n");
    for (a, name) in csrs(m) {
        let n = CSR_BASE + a as usize;
        let _ = writeln!(
            x,
            "<reg name=\"{name}\" bitsize=\"{bits}\" type=\"int\" regnum=\"{n}\" group=\"csr\"/>"
        );
    }
    x.push_str("</feature>\n<feature name=\"org.gnu.gdb.riscv.virtual\">\n");
    let _ = writeln!(
        x,
        "<reg name=\"priv\" bitsize=\"{bits}\" type=\"int\" regnum=\"{PRIV}\" group=\"general\"/>"
    );
    x.push_str("</feature>\n</target>\n");
    x
}

/// LLDB's `qRegisterInfo<n>` reply for register `n`, for registers of `bytes` bytes:
/// the same numbers as `g` and `p` for `x0`-`x31` and `pc`, the only registers described
/// this way (LLDB stops asking at the first number without one), at their offsets in a
/// `g` packet. The CSRs are in target.xml only.
pub fn lldb_register_info(n: usize, bytes: usize) -> Option<String> {
    let bits = 8 * bytes;
    let common = |name: &str, offset: usize| {
        format!(
            "name:{name};bitsize:{bits};offset:{offset};encoding:uint;format:hex;set:General Purpose Registers;"
        )
    };
    if n == PC {
        return Some(common("pc", bytes * PC) + "generic:pc;");
    }
    if n >= 32 {
        return None;
    }
    let abi = excalibur::reg::abi_name(n as u8);
    let mut info = common(abi, bytes * n).replacen(';', &format!(";alt-name:x{n};"), 1);
    info.push_str(&format!("dwarf:{n};"));
    let generic = match n {
        1 => Some("ra".to_string()),
        2 => Some("sp".to_string()),
        8 => Some("fp".to_string()),
        10..=17 => Some(format!("arg{}", n - 9)),
        _ => None,
    };
    if let Some(g) = generic {
        info.push_str(&format!("generic:{g};"));
    }
    Some(info)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_name_registers() {
        assert_eq!(Reg::from_number(0), Some(Reg::X(0)));
        assert_eq!(Reg::from_number(31), Some(Reg::X(31)));
        assert_eq!(Reg::from_number(32), Some(Reg::Pc));
        assert_eq!(Reg::from_number(33), None, "no floating point");
        assert_eq!(Reg::from_number(64), None);
        assert_eq!(Reg::from_number(65 + 0x300), Some(Reg::Csr(0x300)));
        assert_eq!(Reg::from_number(65 + 4095), Some(Reg::Csr(4095)));
        assert_eq!(Reg::from_number(4161), Some(Reg::Priv));
        assert_eq!(Reg::from_number(4162), None);
    }

    #[test]
    fn describes_the_registers_and_csrs() {
        let m = Machine::with_ram(64 * 1024);
        let x = xml(&m);
        assert!(x.contains("<architecture>riscv:rv32</architecture>"));
        assert!(x.contains("<reg name=\"zero\" bitsize=\"32\" type=\"int\" regnum=\"0\"/>"));
        assert!(x.contains("<reg name=\"sp\" bitsize=\"32\" type=\"data_ptr\" regnum=\"2\"/>"));
        assert!(x.contains("<reg name=\"fp\" bitsize=\"32\" type=\"data_ptr\" regnum=\"8\"/>"));
        assert!(x.contains("<reg name=\"pc\" bitsize=\"32\" type=\"code_ptr\" regnum=\"32\"/>"));
        // mstatus is CSR 0x300: register 65 + 768.
        assert!(
            x.contains("<reg name=\"mstatus\" bitsize=\"32\" type=\"int\" regnum=\"833\" group=\"csr\"/>")
        );
        assert!(x.contains("name=\"satp\""));
        assert!(x.contains("name=\"mhartid\""));
        assert!(x.contains("name=\"pmpaddr15\""));
        // Named but not implemented: not described.
        assert!(!x.contains("mnstatus"));
        assert!(x.contains("<reg name=\"priv\" bitsize=\"32\" type=\"int\" regnum=\"4161\""));
        assert_eq!(x.matches("<reg ").count(), 33 + csrs(&m).len() + 1);
        assert!(x.contains("name=\"cycleh\""));
    }

    #[test]
    fn an_rv64_machine_is_described_with_64_bit_registers() {
        let m = Machine::new(crate::MachineConfig {
            xlen: Xlen::Rv64,
            ..crate::MachineConfig::with_ram(64 * 1024)
        })
        .unwrap();
        let x = xml(&m);
        assert!(x.contains("<architecture>riscv:rv64</architecture>"));
        assert!(x.contains("<reg name=\"sp\" bitsize=\"64\" type=\"data_ptr\" regnum=\"2\"/>"));
        assert!(x.contains("<reg name=\"pc\" bitsize=\"64\" type=\"code_ptr\" regnum=\"32\"/>"));
        assert!(
            x.contains("<reg name=\"mstatus\" bitsize=\"64\" type=\"int\" regnum=\"833\" group=\"csr\"/>")
        );
        assert!(x.contains("<reg name=\"priv\" bitsize=\"64\""));
        // RV32's high halves are not RV64's registers.
        assert!(!x.contains("cycleh") && !x.contains("mstatush"));
        assert!(!x.contains("bitsize=\"32\""));
        assert_eq!(reg_bytes(&m), 8);
        assert_eq!(
            lldb_register_info(2, 8).unwrap(),
            "name:sp;alt-name:x2;bitsize:64;offset:16;encoding:uint;format:hex;set:General Purpose Registers;dwarf:2;generic:sp;"
        );
        assert!(lldb_register_info(PC, 8).unwrap().contains("offset:256;"));
    }
}
