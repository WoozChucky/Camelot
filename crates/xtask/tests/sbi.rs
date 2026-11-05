//! Camelot's SBI (kernel/machine.S), driven directly: every call the SBI
//! v2.0 defines for BASE, TIME, IPI, HSM, SRST and DBCN, made with an
//! `ecall` from S-mode as the kernel makes them, on both committed kernels
//! (kernel/kernel.elf, RV32, and kernel/kernel-rv64.elf, RV64), with each
//! call's results and errors, and every register but a0 and a1 kept. Then
//! the kernel's own side: the exit codes machine_power_off() gives, a hart
//! that hart_start can't start and one that never comes online, both
//! reported and left out, and, over whole boots, that every IPI store comes
//! after a fence that orders the stores before it (#170), that every msip
//! clear comes before a fence that orders it before the loads after it, and
//! that S-mode never touches the CLINT or the SBI's scratch blocks.

mod common;

use common::sbi::*;
use excalibur::csr::{IRQ_M_SOFT, IRQ_M_TIMER, IRQ_S_SOFT, IRQ_S_TIMER, MSTATUS_SIE};
use excalibur::{Inst, Xlen};
use grail::bus::RAM_BASE;
use grail::cpu::{Cpu, Mode};
use grail::{Machine, MachineConfig, RunResult};
use std::path::PathBuf;
use xtask::kernel::{self, BOOT_MAX_STEPS};
use xtask::repo_root;

/// Where the RV64 kernel is linked, and so the distance between its
/// symbols' addresses and their physical ones (kernel/kernel.lds.S).
const RV64_LINK: u64 = 0xffff_ffff_8000_0000;

/// The CLINT's msip registers, a u32 per hart (spec section 7).
const CLINT_MSIP: u64 = 0x0200_0000;

/// Scratch RAM, at physical addresses near the end of the 4 MiB machine,
/// far above either kernel image: buffers for the debug console, and code a
/// started hart runs. Free while the kernel is stopped at kmain, before
/// kinit() hands the pages out.
const BUFFER: u64 = RAM_BASE + 0x3e_0000;
const HART_CODE: u64 = RAM_BASE + 0x3f_0000;
const RAM_END: u64 = RAM_BASE + 0x40_0000;

/// What every test runs on: both kernels.
const WIDTHS: [Xlen; 2] = [Xlen::Rv32, Xlen::Rv64];

fn kernel_file(xlen: Xlen) -> PathBuf {
    let name = match xlen {
        Xlen::Rv32 => "kernel.elf",
        Xlen::Rv64 => "kernel-rv64.elf",
    };
    repo_root().join("kernel").join(name)
}

/// A committed kernel stopped at kmain, in S-mode with the SBI set up and
/// every other hart STOPPED, kmain turned into `ecall; j .`, so that each
/// call() is an SBI call from S-mode.
struct Sbi {
    m: Machine,
    elf: grail::elf::Elf,
    xlen: Xlen,
    /// kmain's address, where the ecall is: the pc runs from here to here + 4.
    kmain: u64,
}

impl Sbi {
    fn new(xlen: Xlen, harts: usize) -> Sbi {
        let mut sbi = Sbi::at_kmain(xlen, harts);
        let code: Vec<u8> = [0x0000_0073u32, 0x0000_006f] // ecall; j .
            .iter()
            .flat_map(|w| w.to_le_bytes())
            .collect();
        sbi.m.cpu.bus.load(sbi.pa(sbi.kmain), &code).unwrap();
        sbi
    }

    /// The kernel as it is, stopped at kmain.
    fn at_kmain(xlen: Xlen, harts: usize) -> Sbi {
        let file = std::fs::read(kernel_file(xlen)).unwrap();
        let elf = grail::elf::parse(&file).unwrap();
        let mut m = Machine::new(MachineConfig {
            xlen,
            harts,
            ..MachineConfig::default()
        })
        .unwrap();
        m.cpu.bus.uart.capture = true;
        m.load_elf(&file).unwrap();
        let kmain = elf.symbol("kmain").unwrap();
        let mut sbi = Sbi { m, elf, xlen, kmain };
        sbi.step_until(|m| m.cpu.hart(0).pc == kmain);
        assert_eq!(sbi.m.cpu.hart(0).mode, Mode::S);
        sbi
    }

    fn sym(&self, name: &str) -> u64 {
        self.elf
            .symbol(name)
            .unwrap_or_else(|| panic!("no symbol {name}"))
    }

    /// The physical address of kernel address `va`.
    fn pa(&self, va: u64) -> u64 {
        match self.xlen {
            Xlen::Rv32 => va,
            Xlen::Rv64 => va - RV64_LINK + RAM_BASE,
        }
    }

    fn step_until(&mut self, done: impl Fn(&Machine) -> bool) {
        for _ in 0..BOOT_MAX_STEPS {
            if done(&self.m) {
                return;
            }
            assert!(self.m.cpu.bus.exit.is_none(), "the machine stopped");
            self.m.cpu.step();
        }
        let h = self.m.cpu.hart(0);
        panic!(
            "gave up: hart 0 at {:#x} in {:?}, mip {:#x}; output:\n{}",
            h.pc,
            h.mode,
            h.csr.mip,
            self.printed()
        );
    }

    fn printed(&self) -> String {
        String::from_utf8_lossy(&self.m.cpu.bus.uart.captured).into_owned()
    }

    /// Hart 0 at the ecall with a7 = `eid`, a6 = `fid`, a0... = `args` and
    /// every other register a pattern of its own.
    fn set_up(&mut self, eid: u64, fid: u64, args: &[u64]) -> [u64; 32] {
        let kmain = self.kmain;
        let h = self.m.cpu.hart_mut(0);
        assert_eq!(h.mode, Mode::S, "hart 0 is still in the SBI");
        for r in 1..32u8 {
            h.set_reg(r, 0x5a00_0000_0000_0000 | (u64::from(r) * 0x0101_0101));
        }
        for (i, &a) in args.iter().enumerate() {
            h.set_reg(10 + i as u8, a);
        }
        h.set_reg(16, fid);
        h.set_reg(17, eid);
        h.pc = kmain;
        h.x
    }

    /// An SBI call: (error, value), checking that it kept every other
    /// register.
    fn call(&mut self, eid: u64, fid: u64, args: &[u64]) -> (i64, i64) {
        let before = self.set_up(eid, fid, args);
        let after = self.kmain + 4;
        self.m.cpu.step();
        self.step_until(|m| m.cpu.hart(0).pc == after);
        let h = self.m.cpu.hart(0);
        assert_eq!(h.mode, Mode::S);
        for r in (1..32).filter(|&r| r != 10 && r != 11) {
            assert_eq!(
                h.x[r],
                before[r],
                "{eid:#x}/{fid}: {} changed",
                excalibur::reg::abi_name(r as u8)
            );
        }
        (h.x[10] as i64, h.x[11] as i64)
    }

    /// An SBI call that should stop the machine: its exit code.
    fn call_exit(&mut self, eid: u64, fid: u64, args: &[u64]) -> RunResult {
        self.set_up(eid, fid, args);
        self.m.run(Some(10_000), |_| {})
    }

    /// Hart `h`'s HSM state, as the SBI reports it.
    fn status(&mut self, h: u64) -> i64 {
        let (err, state) = self.call(EXT_HSM, HSM_HART_GET_STATUS, &[h]);
        assert_eq!(err, SUCCESS);
        state
    }
}

/// `lui rd, imm20`, `addi rd, rs1, imm` and `ecall`, for the code a started
/// hart runs.
fn lui(rd: u32, imm20: u32) -> u32 {
    (imm20 << 12) | (rd << 7) | 0x37
}
fn addi(rd: u32, rs1: u32, imm: i32) -> u32 {
    ((imm as u32) << 20) | (rs1 << 15) | (rd << 7) | 0x13
}
const ECALL: u32 = 0x0000_0073;
const J_SELF: u32 = 0x0000_006f;

#[test]
fn the_base_extension_describes_the_sbi() {
    for xlen in WIDTHS {
        let mut s = Sbi::new(xlen, 1);
        assert_eq!(s.call(EXT_BASE, BASE_GET_SPEC_VERSION, &[]), (0, SPEC_VERSION));
        assert_eq!(s.call(EXT_BASE, BASE_GET_IMPL_ID, &[]), (0, IMPL_ID));
        assert_eq!(s.call(EXT_BASE, BASE_GET_IMPL_VERSION, &[]), (0, IMPL_VERSION));
        for ext in [EXT_BASE, EXT_TIME, EXT_IPI, EXT_HSM, EXT_SRST, EXT_DBCN] {
            assert_eq!(
                s.call(EXT_BASE, BASE_PROBE_EXT, &[ext]),
                (0, 1),
                "{xlen:?} {ext:#x}"
            );
        }
        // The legacy extensions (0x00-0x0f), RFENCE, PMU and others: absent.
        for ext in [0, 1, 8, 0x5246_4e43, 0x0050_4d55, 0x1234_5678] {
            assert_eq!(
                s.call(EXT_BASE, BASE_PROBE_EXT, &[ext]),
                (0, 0),
                "{xlen:?} {ext:#x}"
            );
        }
        // Grail's mvendorid, marchid and mimpid: 0, "not implemented".
        for fid in [BASE_GET_MVENDORID, BASE_GET_MARCHID, BASE_GET_MIMPID] {
            assert_eq!(s.call(EXT_BASE, fid, &[]), (0, 0));
        }
    }
}

#[test]
fn unknown_extensions_and_functions_are_not_supported() {
    for xlen in WIDTHS {
        let mut s = Sbi::new(xlen, 1);
        // The legacy calls, RFENCE, anything else.
        for ext in [0, 1, 8, 0x5246_4e43, 0x1234_5678, u64::MAX] {
            assert_eq!(
                s.call(ext, 0, &[1, 2, 3]),
                (ERR_NOT_SUPPORTED, 0),
                "{xlen:?} {ext:#x}"
            );
        }
        // A function each extension doesn't have.
        for (ext, fid) in [
            (EXT_BASE, 7),
            (EXT_TIME, 1),
            (EXT_IPI, 1),
            (EXT_HSM, 4),
            (EXT_SRST, 1),
            (EXT_DBCN, 3),
        ] {
            assert_eq!(
                s.call(ext, fid, &[]),
                (ERR_NOT_SUPPORTED, 0),
                "{xlen:?} {ext:#x}/{fid}"
            );
        }
    }
}

#[test]
fn set_timer_sets_the_deadline_the_supervisor_timer_interrupt_comes_at() {
    for xlen in WIDTHS {
        let mut s = Sbi::new(xlen, 1);
        // A deadline past 2^32, which RV32 passes as two halves.
        let deadline = s.m.cpu.bus.clint.mtime + 0x1_0000_0000;
        let args = match xlen {
            Xlen::Rv32 => vec![deadline & 0xffff_ffff, deadline >> 32],
            Xlen::Rv64 => vec![deadline],
        };
        // A supervisor timer interrupt pending from before, which the call
        // clears.
        s.m.cpu.hart_mut(0).csr.mip |= 1 << IRQ_S_TIMER;
        assert_eq!(s.call(EXT_TIME, TIME_SET_TIMER, &args), (0, 0));
        let h = s.m.cpu.hart(0);
        assert_eq!(s.m.cpu.bus.clint.mtimecmp[0], deadline, "{xlen:?}");
        assert_eq!(h.csr.mip & (1 << IRQ_S_TIMER), 0);
        assert_ne!(h.csr.mie & (1 << IRQ_M_TIMER), 0);
        // At the deadline the machine timer fires, and the SBI raises STIP
        // in its place (S-mode interrupts are off at kmain: it stays
        // pending) and masks the machine timer until the next set_timer.
        s.m.cpu.bus.clint.mtime = deadline;
        s.step_until(|m| m.cpu.hart(0).csr.mip & (1 << IRQ_S_TIMER) != 0);
        s.step_until(|m| m.cpu.hart(0).mode == Mode::S);
        assert_eq!(s.m.cpu.hart(0).csr.mie & (1 << IRQ_M_TIMER), 0);
        assert_eq!(s.m.cpu.hart(0).csr.mstatus & MSTATUS_SIE, 0);
    }
}

#[test]
fn send_ipi_interrupts_the_harts_in_its_mask() {
    for xlen in WIDTHS {
        let mut s = Sbi::new(xlen, 4);
        let ssip = |m: &Machine| m.cpu.hart(0).csr.mip & (1 << IRQ_S_SOFT) != 0;
        // To itself: the machine software interrupt, which the SBI acks
        // (msip = 0) and passes on as SSIP.
        assert_eq!(s.call(EXT_IPI, IPI_SEND_IPI, &[1, 0]), (0, 0));
        s.step_until(ssip);
        s.step_until(|m| m.cpu.hart(0).mode == Mode::S);
        assert!(!s.m.cpu.bus.clint.msip[0]);
        s.m.cpu.hart_mut(0).csr.mip &= !(1 << IRQ_S_SOFT);
        // Hart 3 as base 3, bit 0; every hart with base -1. The others are
        // STOPPED: the IPI wakes them for nothing, they stay STOPPED.
        assert_eq!(s.call(EXT_IPI, IPI_SEND_IPI, &[1, 3]), (0, 0));
        assert_eq!(s.call(EXT_IPI, IPI_SEND_IPI, &[0, u64::MAX]), (0, 0));
        s.step_until(ssip);
        s.step_until(|m| m.cpu.hart(0).mode == Mode::S);
        for h in 1..4 {
            assert_eq!(s.status(h), STATE_STOPPED, "{xlen:?} hart {h}");
        }
        // No hart: nothing to do, whatever the base.
        assert_eq!(s.call(EXT_IPI, IPI_SEND_IPI, &[0, 99]), (0, 0));
        // A hart the machine doesn't have.
        for (mask, base) in [(0b1_0000, 0), (2, 3), (1, 4), (1, u64::MAX - 1)] {
            assert_eq!(
                s.call(EXT_IPI, IPI_SEND_IPI, &[mask, base]),
                (ERR_INVALID_PARAM, 0),
                "{xlen:?} {mask:#b} from {base}"
            );
        }
    }
}

#[test]
fn hart_start_starts_a_stopped_hart_and_hart_stop_stops_it() {
    for xlen in WIDTHS {
        let mut s = Sbi::new(xlen, 2);
        // At kmain, before smp_boot(): hart 0 runs, hart 1 waits.
        assert_eq!(s.status(0), STATE_STARTED);
        assert_eq!(s.status(1), STATE_STOPPED);
        assert_eq!(s.call(EXT_HSM, HSM_HART_GET_STATUS, &[2]), (ERR_INVALID_PARAM, 0));
        // Errors: a hart the machine doesn't have, one that runs, an
        // address that isn't 4-byte aligned RAM.
        let start = |s: &mut Sbi, h: u64, addr: u64| s.call(EXT_HSM, HSM_HART_START, &[h, addr, 0]).0;
        assert_eq!(start(&mut s, 2, HART_CODE), ERR_INVALID_PARAM);
        assert_eq!(start(&mut s, 0, HART_CODE), ERR_ALREADY_AVAILABLE);
        for addr in [HART_CODE + 2, 0x1000, RAM_BASE - 4, RAM_END] {
            assert_eq!(start(&mut s, 1, addr), ERR_INVALID_ADDRESS, "{xlen:?} {addr:#x}");
        }
        assert_eq!(s.status(1), STATE_STOPPED);

        // hart 1 at HART_CODE: j . until the test makes it a nop, then li a7,
        // HSM; li a6, HART_STOP; ecall.
        let (a6, a7) = (16, 17);
        let code: Vec<u8> = [
            J_SELF,
            lui(a7, 0x485),
            addi(a7, a7, 0x34d),
            addi(a6, 0, HSM_HART_STOP as i32),
            ECALL,
            J_SELF,
        ]
        .iter()
        .flat_map(|w| w.to_le_bytes())
        .collect();
        s.m.cpu.bus.load(HART_CODE, &code).unwrap();
        assert_eq!(s.call(EXT_HSM, HSM_HART_START, &[1, HART_CODE, 0x5a5a]), (0, 0));
        // It starts as the SBI spec says: S-mode, paging off, sstatus.SIE 0,
        // a0 = its id, a1 = the opaque value.
        s.step_until(|m| m.cpu.hart(1).pc == HART_CODE);
        let h = s.m.cpu.hart(1);
        assert_eq!(h.mode, Mode::S, "{xlen:?}");
        assert_eq!((h.x[10], h.x[11], h.csr.satp), (1, 0x5a5a, 0));
        assert_eq!(h.csr.mstatus & MSTATUS_SIE, 0);
        assert_eq!(h.csr.mie, 1 << IRQ_M_SOFT, "only the IPI enabled");
        assert_eq!(s.status(1), STATE_STARTED);
        assert_eq!(start(&mut s, 1, HART_CODE), ERR_ALREADY_AVAILABLE);
        // Its hart_stop: STOPPED, waiting in the SBI again.
        let nop = addi(0, 0, 0);
        s.m.cpu.bus.load(HART_CODE, &nop.to_le_bytes()).unwrap();
        s.step_until(|m| m.cpu.hart(1).mode == Mode::M && m.cpu.hart(1).waiting);
        let pc = s.m.cpu.hart(1).pc;
        assert!(function_contains(&s, "sbi_hart_wait", pc), "{xlen:?} {pc:#x}");
        assert_eq!(s.status(1), STATE_STOPPED);
        // And it starts again, at another address.
        s.m.cpu.bus.load(HART_CODE + 64, &J_SELF.to_le_bytes()).unwrap();
        assert_eq!(s.call(EXT_HSM, HSM_HART_START, &[1, HART_CODE + 64, 7]), (0, 0));
        s.step_until(|m| m.cpu.hart(1).pc == HART_CODE + 64);
        assert_eq!((s.m.cpu.hart(1).x[10], s.m.cpu.hart(1).x[11]), (1, 7));
        assert_eq!(s.status(1), STATE_STARTED);
    }
}

/// Whether `pc`, an M-mode (physical) address, is in the code from label
/// `name` up to the next label after it (machine.S's routines have no
/// sizes).
fn function_contains(s: &Sbi, name: &str, pc: u64) -> bool {
    let pc = match s.xlen {
        Xlen::Rv32 => pc,
        Xlen::Rv64 => pc - RAM_BASE + RV64_LINK,
    };
    let start = s.sym(name);
    let next = s
        .elf
        .symbols
        .iter()
        .map(|&(_, v)| v)
        .filter(|&v| v > start)
        .min()
        .unwrap();
    (start..next).contains(&pc)
}

#[test]
fn hart_suspend_waits_for_an_interrupt() {
    for xlen in WIDTHS {
        let mut s = Sbi::new(xlen, 1);
        // The default non-retentive suspend: not on Grail. Reserved types
        // and platform ones: invalid.
        let suspend = |s: &mut Sbi, t: u64| s.call(EXT_HSM, HSM_HART_SUSPEND, &[t, HART_CODE, 0]);
        assert_eq!(suspend(&mut s, 0x8000_0000), (ERR_NOT_SUPPORTED, 0));
        for t in [1, 0x0fff_ffff, 0x1000_0000, 0x8000_0001, 0x9000_0000, 0xffff_ffff] {
            assert_eq!(suspend(&mut s, t), (ERR_INVALID_PARAM, 0), "{xlen:?} {t:#x}");
        }
        // The default retentive one: SUSPENDED until the timer, set 50 ticks
        // away, fires; then the call returns and the hart is STARTED.
        let deadline = s.m.cpu.bus.clint.mtime + 50;
        let args = match xlen {
            Xlen::Rv32 => vec![deadline, 0],
            Xlen::Rv64 => vec![deadline],
        };
        assert_eq!(s.call(EXT_TIME, TIME_SET_TIMER, &args), (0, 0));
        let state = s.sym("machine_scratch") + 6 * u64::from(xlen.bits() / 8);
        let state_pa = s.pa(state);
        s.set_up(EXT_HSM, HSM_HART_SUSPEND, &[0, 0, 0]);
        s.step_until(|m| m.cpu.hart(0).waiting);
        assert_eq!(s.m.cpu.bus.peek(state_pa), Some(STATE_SUSPENDED as u32));
        let after = s.kmain + 4;
        s.step_until(|m| m.cpu.hart(0).pc == after && m.cpu.hart(0).mode == Mode::S);
        assert_eq!(s.m.cpu.hart(0).x[10], 0);
        assert!(s.m.cpu.bus.clint.mtime >= deadline);
        assert_eq!(s.status(0), STATE_STARTED);
    }
}

#[test]
fn system_reset_shuts_down_with_an_exit_code() {
    for xlen in WIDTHS {
        // No reason, a system failure (the kernel's panic code), and
        // Camelot's own reasons, 0xE000_0000 | code.
        for (reason, code) in [(0, 0), (1, 0xfe), (0xe000_0007, 7), (0xe000_00ff, 0xff)] {
            let mut s = Sbi::new(xlen, 1);
            assert_eq!(
                s.call_exit(EXT_SRST, SRST_RESET, &[0, reason]),
                RunResult::Exited(code),
                "{xlen:?} {reason:#x}"
            );
        }
        let mut s = Sbi::new(xlen, 1);
        // Reboots: Grail can't. Reserved and vendor types and reasons:
        // invalid.
        for (ty, reason, err) in [
            (1, 0, ERR_NOT_SUPPORTED),
            (2, 1, ERR_NOT_SUPPORTED),
            (3, 0, ERR_INVALID_PARAM),
            (0xf000_0000, 0, ERR_INVALID_PARAM),
            (0, 2, ERR_INVALID_PARAM),
            (0, 0xdfff_ffff, ERR_INVALID_PARAM),
            (0, 0xf000_0000, ERR_INVALID_PARAM),
            (1, 0xf000_0000, ERR_INVALID_PARAM),
        ] {
            assert_eq!(
                s.call(EXT_SRST, SRST_RESET, &[ty, reason]),
                (err, 0),
                "{xlen:?} {ty:#x} {reason:#x}"
            );
        }
    }
}

#[test]
fn the_debug_console_writes_and_reads_the_uart() {
    for xlen in WIDTHS {
        let mut s = Sbi::new(xlen, 1);
        let printed = s.printed().len();
        assert_eq!(s.call(EXT_DBCN, DBCN_WRITE_BYTE, &[u64::from(b'Z')]), (0, 0));
        s.m.cpu.bus.load(BUFFER, b"hello").unwrap();
        assert_eq!(s.call(EXT_DBCN, DBCN_WRITE, &[5, BUFFER, 0]), (0, 5));
        assert_eq!(s.call(EXT_DBCN, DBCN_WRITE, &[0, BUFFER, 0]), (0, 0));
        assert_eq!(&s.printed()[printed..], "Zhello", "{xlen:?}");
        // Memory that isn't all RAM, or above XLEN bits.
        for (n, lo, hi) in [
            (1, BUFFER, 1),
            (4, 0x1000, 0),
            (16, RAM_END - 8, 0),
            (1, RAM_END, 0),
        ] {
            assert_eq!(s.call(EXT_DBCN, DBCN_WRITE, &[n, lo, hi]), (ERR_INVALID_PARAM, 0));
            assert_eq!(s.call(EXT_DBCN, DBCN_READ, &[n, lo, hi]), (ERR_INVALID_PARAM, 0));
        }
        // Reading: what the UART has, up to the count, without waiting.
        s.m.set_uart_input(kernel::typed(b"ab"));
        assert_eq!(s.call(EXT_DBCN, DBCN_READ, &[8, BUFFER + 16, 0]), (0, 2));
        let got: Vec<u8> = (0..2)
            .map(|i| s.m.cpu.bus.peek(BUFFER + 16 + i).unwrap() as u8)
            .collect();
        assert_eq!(got, b"ab");
        assert_eq!(s.call(EXT_DBCN, DBCN_READ, &[8, BUFFER + 16, 0]), (0, 0));
    }
}

#[test]
fn machine_power_off_turns_exit_codes_into_reset_reasons() {
    // The kernel's side (kernel/sbi.c): the reason it gives system_reset
    // for each exit code, and the exit code that comes out.
    for xlen in WIDTHS {
        for (code, reason, exit) in [
            (0u64, 0u64, 0),
            (7, 0xe000_0007, 7),
            (0xfe, 1, 0xfe),
            (0x1234, 0xe000_1234, 0x34),
            // -1, a killed shell's status: Grail's exit code is its low 8 bits.
            (0xffff_ffff, 0xefff_ffff, 0xff),
        ] {
            let mut s = Sbi::new(xlen, 1);
            let power_off = s.sym("machine_power_off");
            let h = s.m.cpu.hart_mut(0);
            // A u32 argument, sign-extended in its register as the calling
            // convention keeps it on RV64.
            h.set_reg(10, code as u32 as i32 as i64 as u64);
            h.pc = power_off;
            let mut reasons = Vec::new();
            // M-mode runs at physical addresses.
            let machine_vec = s.pa(s.sym("machine_vec"));
            let r = s.m.run(Some(100_000), |cpu| {
                if cpu.pc == machine_vec && cpu.x[17] == EXT_SRST {
                    reasons.push((cpu.x[10], cpu.x[11] & 0xffff_ffff));
                }
            });
            assert_eq!(r, RunResult::Exited(exit), "{xlen:?} {code:#x}");
            assert_eq!(reasons, [(0, reason)], "{xlen:?} {code:#x}");
        }
    }
}

/// A committed kernel on a 4-hart machine with `exit` typed for its shell,
/// stepped until hart 0 reaches kmain.
fn four_harts_at_kmain(xlen: Xlen) -> Sbi {
    let mut s = Sbi::at_kmain(xlen, 4);
    s.m.set_uart_input(kernel::typed(b"exit\n"));
    s
}

#[test]
fn a_hart_the_sbi_cannot_start_is_reported_and_left_out() {
    // Hart 2 marked STARTED in the SBI before smp_boot(): its hart_start
    // fails with SBI_ERR_ALREADY_AVAILABLE, and the kernel goes on with the
    // other three.
    for xlen in WIDTHS {
        let mut s = four_harts_at_kmain(xlen);
        let block = s.pa(s.sym("machine_scratch")) + 2 * 16 * u64::from(xlen.bits() / 8);
        let state = block + 6 * u64::from(xlen.bits() / 8);
        s.m.cpu.bus.load(state, &0u32.to_le_bytes()).unwrap();
        let r = s.m.run(Some(BOOT_MAX_STEPS), |_| {});
        let out = s.printed();
        assert_eq!(r, RunResult::Exited(0), "{xlen:?}:\n{out}");
        assert!(
            out.contains("\nsmp: hart 2 failed to start (SBI_ERR_ALREADY_AVAILABLE)\nsmp: 3 harts running\n"),
            "{xlen:?}:\n{out}"
        );
    }
}

#[test]
fn a_hart_that_never_comes_online_is_reported_after_a_second() {
    // Hart 2 starts, but gets stuck at its first instruction (the test puts
    // it back there after every step): hart 0 waits a second of `time` for
    // it, says so, and goes on with the other three rather than wait for
    // good.
    for xlen in WIDTHS {
        let mut s = four_harts_at_kmain(xlen);
        let entry = s.pa(s.sym("secondary_entry"));
        s.step_until(|m| m.cpu.hart(2).pc == entry);
        let started = s.m.cpu.bus.clint.mtime;
        for _ in 0..BOOT_MAX_STEPS {
            if s.m.cpu.bus.exit.is_some() {
                break;
            }
            s.m.cpu.step();
            s.m.cpu.hart_mut(2).pc = entry;
        }
        let out = s.printed();
        assert_eq!(
            s.m.cpu.bus.exit,
            Some(grail::bus::Exit::Code(0)),
            "{xlen:?}:\n{out}"
        );
        assert!(
            out.contains("\nsmp: hart 2 failed to come online\nsmp: 3 harts running\n"),
            "{xlen:?}:\n{out}"
        );
        // A second of the nominal 10 MHz timebase, at least.
        assert!(s.m.cpu.bus.clint.mtime - started >= 10_000_000);
    }
}

/// What the boots below watch on every hart's turn: each store to an msip
/// (an IPI) must come after a `fence` whose predecessor set has W and whose
/// successor set has O, with no store to memory in between (#170); each
/// store clearing an msip must be followed by a `fence` whose predecessor
/// set has O before the hart's next load from memory, or a second IPI could
/// be erased after the load missed what it asked; and S-mode must never
/// touch the CLINT or the SBI's scratch blocks.
struct Ordering {
    xlen: Xlen,
    /// Per hart: a fence ordering the stores so far before device stores
    /// has run since the hart's last store to memory.
    fenced: Vec<bool>,
    /// Per hart: it has cleared its msip, and no fence ordering device
    /// stores before loads has run since.
    cleared: Vec<bool>,
    scratch: std::ops::Range<u64>,
    ipis: usize,
    clears: usize,
}

impl Ordering {
    fn step(&mut self, cpu: &Cpu) {
        let h = cpu.id as usize;
        let pc = match (cpu.mode, self.xlen) {
            (Mode::U, _) => return,
            (_, Xlen::Rv64) if cpu.pc >= RV64_LINK => cpu.pc - RV64_LINK + RAM_BASE,
            _ => cpu.pc,
        };
        let Some(word) = cpu.bus.peek(pc) else { return };
        let reg = |r: u8| cpu.x[r as usize];
        let addr = |base: u8, imm: i32| {
            let a = reg(base).wrapping_add(imm as i64 as u64);
            match self.xlen {
                Xlen::Rv32 => a & 0xffff_ffff,
                Xlen::Rv64 => a,
            }
        };
        // The address, the value stored, and whether the access reads (an
        // AMO does both).
        let (target, value, reads) = match excalibur::decode(word, self.xlen) {
            Inst::Fence { pred, succ } => {
                if pred & 1 != 0 && succ & 4 != 0 {
                    self.fenced[h] = true;
                }
                if pred & 4 != 0 && succ & 2 != 0 {
                    self.cleared[h] = false;
                }
                return;
            }
            Inst::Load { rs1, imm, .. } => (addr(rs1, imm), None, true),
            Inst::Store { rs1, rs2, imm, .. } => (addr(rs1, imm), Some(reg(rs2)), false),
            Inst::Amo { rs1, rs2, .. } => (addr(rs1, 0), Some(reg(rs2)), true),
            Inst::Sc { rs1, rs2, .. } => (addr(rs1, 0), Some(reg(rs2)), false),
            Inst::Lr { rs1, .. } => (addr(rs1, 0), None, true),
            _ => return,
        };
        let clint = (0x0200_0000..0x0201_0000).contains(&target);
        if cpu.mode == Mode::S {
            assert!(!clint, "S-mode touches the CLINT at {:#x}", cpu.pc);
            assert!(
                !self.scratch.contains(&target),
                "S-mode touches the SBI's scratch blocks at {:#x}",
                cpu.pc
            );
        }
        let memory = !clint && target >= RAM_BASE;
        assert!(
            !(reads && memory && self.cleared[h]),
            "a load at {:#x} after an msip clear without a fence between them",
            cpu.pc
        );
        let msip = (CLINT_MSIP..CLINT_MSIP + 32).contains(&target);
        match value {
            Some(v) if msip && v & 0xffff_ffff != 0 => {
                assert!(
                    self.fenced[h],
                    "an IPI at {:#x} without a fence before it",
                    cpu.pc
                );
                self.ipis += 1;
            }
            Some(_) if msip => {
                self.cleared[h] = true;
                self.clears += 1;
            }
            Some(_) if memory => self.fenced[h] = false,
            _ => {}
        }
    }
}

#[test]
fn every_ipi_comes_after_a_fence_and_s_mode_never_touches_the_sbi() {
    // Boots with every kind of IPI: the coldboot waking the harts, each
    // hart_start, the TLB shootdowns and the idle kicks.
    for xlen in WIDTHS {
        let file = std::fs::read(kernel_file(xlen)).unwrap();
        let elf = grail::elf::parse(&file).unwrap();
        let mut m = Machine::new(MachineConfig {
            xlen,
            harts: 4,
            ..MachineConfig::default()
        })
        .unwrap();
        m.load_elf(&file).unwrap();
        m.set_uart_input(kernel::typed(b"ding & dong\nwait\nexit\n"));
        let scratch = elf.symbol("machine_scratch").unwrap();
        let size = 4 * 16 * u64::from(xlen.bits() / 8);
        let mut watch = Ordering {
            xlen,
            fenced: vec![false; 4],
            cleared: vec![false; 4],
            scratch: scratch..scratch + size,
            ipis: 0,
            clears: 0,
        };
        let r = m.run(Some(common::RUN_MAX_STEPS), |cpu| watch.step(cpu));
        assert_eq!(r, RunResult::Exited(0), "{xlen:?}");
        // At least the three harts woken, the three started, and the boot's
        // TLB self-test.
        assert!(watch.ipis >= 9, "{xlen:?}: {} IPIs", watch.ipis);
        // As many cleared: the ROM's and sbi_hart_wait's for the wake-up,
        // sbi_hart_wait's for each start, machine_soft's for the rest.
        assert!(watch.clears >= 9, "{xlen:?}: {} msip clears", watch.clears);
    }
}
