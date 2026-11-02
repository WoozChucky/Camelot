//! PLIC and UART receive interrupts, seen from the hart: `mip` wiring, trap delivery,
//! and the interrupt-driven echo example fed from a test harness and from real stdin.

use grail::bus::{PLIC_BASE, RAM_BASE, UART_BASE};
use grail::cpu::Mode;
use grail::{Machine, RunResult};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{channel, Sender};

use excalibur::csr::{IRQ_M_EXT, IRQ_S_EXT, IRQ_S_SOFT, MIP, MSTATUS_MPP_SHIFT, MSTATUS_MPRV, MSTATUS_SIE};
use grail::trap::INTERRUPT_BIT;

const MEIP: u32 = 1 << IRQ_M_EXT;
const SEIP: u32 = 1 << IRQ_S_EXT;
const SSIP: u32 = 1 << IRQ_S_SOFT;
/// Register numbers of t0 and t1.
const T0: u32 = 5;
const T1: u32 = 6;

const PENDING: u32 = PLIC_BASE + 0x1000;
const ENABLE0: u32 = PLIC_BASE + 0x2000;
const ENABLE1: u32 = PLIC_BASE + 0x2080;
const CLAIM0: u32 = PLIC_BASE + 0x20_0004;
const CLAIM1: u32 = PLIC_BASE + 0x20_1004;

/// mcause codes of the exceptions these tests expect.
const INSTR_ACCESS_FAULT: u32 = 1;
const LOAD_ACCESS_FAULT: u32 = 5;
const STORE_ACCESS_FAULT: u32 = 7;

/// Where `machine()` points mtvec: a run of nops past the program.
const HANDLER: u32 = RAM_BASE + 0x40;

const WFI: u32 = 0x1050_0073;

fn echo_elf() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/uart-echo/uart-echo.elf")
}

/// Encodes `csrr{w,s,c} x0, csr, rs1` (funct3 1, 2, 3).
fn csr_insn(funct3: u32, csr: u16, rs1: u32) -> u32 {
    ((csr as u32) << 20) | (rs1 << 15) | (funct3 << 12) | 0x73
}

/// Encodes a load `rd, imm(rs1)` (funct3 0 = lb, 1 = lh, 2 = lw).
fn load_insn(funct3: u32, rd: u32, rs1: u32, imm: u32) -> u32 {
    (imm << 20) | (rs1 << 15) | (funct3 << 12) | (rd << 7) | 0x03
}

/// Encodes a store `rs2, imm(rs1)` (funct3 0 = sb, 1 = sh, 2 = sw), for `imm` < 32.
fn store_insn(funct3: u32, rs2: u32, rs1: u32, imm: u32) -> u32 {
    (rs2 << 20) | (rs1 << 15) | (funct3 << 12) | (imm << 7) | 0x23
}

/// A machine whose RAM starts with `program` (then nops), pc at RAM, mtvec at [`HANDLER`],
/// a host input channel on the UART, and the UART's PLIC source at priority 1.
fn machine(program: &[u32]) -> (Machine, Sender<u8>) {
    let mut m = Machine::with_ram(64 * 1024);
    let mut code: Vec<u8> = program.iter().flat_map(|w| w.to_le_bytes()).collect();
    code.extend(std::iter::repeat_n([0x13, 0, 0, 0], 64).flatten()); // nops
    m.cpu.bus.load(RAM_BASE, &code).unwrap();
    m.cpu.pc = RAM_BASE;
    m.cpu.csr.mtvec = HANDLER;
    let (tx, rx) = channel();
    let bus = &mut m.cpu.bus;
    bus.uart.input = Some(rx);
    bus.uart.capture = true;
    bus.write(PLIC_BASE + 4, 4, 1).unwrap();
    bus.write(UART_BASE + 1, 1, 1).unwrap(); // IER.ERBFI
    (m, tx)
}

#[test]
fn uart_byte_raises_meip_through_context_0() {
    let (mut m, tx) = machine(&[]);
    m.cpu.bus.write(ENABLE0, 4, 1 << 1).unwrap();
    m.cpu.step();
    assert_eq!(m.cpu.mip() & (MEIP | SEIP), 0, "no input yet");

    tx.send(b'a').unwrap();
    m.cpu.step();
    assert_eq!(m.cpu.mip() & (MEIP | SEIP), MEIP);

    // Claiming drops MEIP; the line stays high, but the source is in flight.
    assert_eq!(m.cpu.bus.read(CLAIM0, 4).unwrap(), 1);
    m.cpu.step();
    assert_eq!(m.cpu.mip() & MEIP, 0);

    // Read RBR (line drops), complete: nothing pending any more.
    assert_eq!(m.cpu.bus.read(UART_BASE, 1).unwrap(), b'a' as u32);
    m.cpu.bus.write(CLAIM0, 4, 1).unwrap();
    m.cpu.step();
    assert_eq!(m.cpu.mip() & MEIP, 0);
}

#[test]
fn context_1_raises_seip_and_traps_into_s_mode_when_delegated() {
    let (mut m, tx) = machine(&[]);
    m.cpu.bus.write(ENABLE1, 4, 1 << 1).unwrap();
    m.cpu.mode = Mode::S;
    m.cpu.csr.mideleg = SEIP;
    m.cpu.csr.mie = SEIP;
    m.cpu.csr.mstatus = MSTATUS_SIE;
    m.cpu.csr.stvec = RAM_BASE + 0x100;

    m.cpu.step(); // one nop, nothing pending
    assert_eq!(m.cpu.pc, RAM_BASE + 4);

    tx.send(b'z').unwrap();
    m.cpu.step();
    assert_eq!(m.cpu.mip() & (MEIP | SEIP), SEIP);
    assert_eq!(m.cpu.mode, Mode::S);
    assert_eq!(m.cpu.pc, RAM_BASE + 0x100);
    assert_eq!(m.cpu.csr.scause, INTERRUPT_BIT | IRQ_S_EXT);
    assert_eq!(m.cpu.csr.sepc, RAM_BASE + 4);

    assert_eq!(m.cpu.bus.read(CLAIM0, 4).unwrap(), 0, "not enabled for context 0");
    assert_eq!(m.cpu.bus.read(CLAIM1, 4).unwrap(), 1);
}

#[test]
fn undelegated_sei_traps_into_m_mode() {
    let (mut m, tx) = machine(&[]);
    m.cpu.bus.write(ENABLE1, 4, 1 << 1).unwrap();
    m.cpu.mode = Mode::U;
    m.cpu.csr.mie = SEIP;
    m.cpu.csr.mtvec = RAM_BASE + 0x200;
    tx.send(b'z').unwrap();
    m.cpu.step();
    assert_eq!(m.cpu.mode, Mode::M);
    assert_eq!(m.cpu.pc, RAM_BASE + 0x200);
    assert_eq!(m.cpu.csr.mcause, INTERRUPT_BIT | IRQ_S_EXT);
}

#[test]
fn meip_is_read_only_and_csrs_does_not_latch_plic_seip() {
    let program = [
        csr_insn(1, MIP, 0),  // csrw mip, zero
        csr_insn(2, MIP, T0), // csrs mip, t0 (t0 = SSIP)
        csr_insn(3, MIP, T0), // csrc mip, t0
    ];
    let (mut m, tx) = machine(&program);
    m.cpu.bus.write(ENABLE0, 4, 1 << 1).unwrap();
    m.cpu.bus.write(ENABLE1, 4, 1 << 1).unwrap();
    m.cpu.x[T0 as usize] = SSIP;
    tx.send(b'q').unwrap();

    m.cpu.step(); // csrw mip, zero
    assert_eq!(
        m.cpu.mip() & (MEIP | SEIP),
        MEIP | SEIP,
        "device bits survive a write of 0"
    );

    m.cpu.step(); // csrs mip, t0: reads SEIP=1 from the PLIC, writes back SSIP only
    assert_eq!(m.cpu.csr.mip, SSIP);
    m.cpu.step(); // csrc mip, t0
    assert_eq!(m.cpu.csr.mip, 0);

    // Once the PLIC drops SEIP, mip shows nothing left over from the read-modify-writes.
    assert_eq!(m.cpu.bus.read(UART_BASE, 1).unwrap(), b'q' as u32);
    assert_eq!(m.cpu.bus.read(CLAIM1, 4).unwrap(), 1);
    assert_eq!(
        m.cpu.bus.read(CLAIM0, 4).unwrap(),
        0,
        "already claimed by context 1"
    );
    m.cpu.step();
    assert_eq!(m.cpu.mip() & (MEIP | SEIP), 0);
}

#[test]
fn software_seip_is_ored_with_the_plic() {
    let (mut m, _tx) = machine(&[csr_insn(1, MIP, T0)]);
    m.cpu.x[T0 as usize] = SEIP;
    m.cpu.step();
    assert_eq!(m.cpu.mip() & SEIP, SEIP, "software-set SEIP with no PLIC request");
}

#[test]
fn wfi_wakes_on_meip_with_interrupts_globally_disabled() {
    // mie.MEIE set, mstatus.MIE clear: wfi must resume without taking the trap
    // (privileged spec, the polling-style wakeup).
    let (mut m, tx) = machine(&[WFI]);
    m.cpu.bus.write(ENABLE0, 4, 1 << 1).unwrap();
    m.cpu.csr.mie = MEIP;
    m.cpu.csr.mstatus = 0;

    m.cpu.step();
    assert!(m.cpu.waiting, "parked in wfi");
    m.cpu.step();
    assert!(m.cpu.waiting, "still nothing pending");
    let parked_pc = m.cpu.pc;

    tx.send(b'w').unwrap();
    m.cpu.step();
    assert!(!m.cpu.waiting, "MEIP woke the hart");
    assert_eq!(m.cpu.pc, parked_pc + 4, "and it ran the instruction after wfi");
    assert_eq!(m.cpu.mode, Mode::M);
    assert_eq!(m.cpu.csr.mcause, 0, "no trap taken");
    assert_eq!(m.cpu.mip() & MEIP, MEIP, "the request is still there to poll");
}

#[test]
fn narrow_plic_accesses_from_the_hart_raise_access_faults() {
    let program = [
        load_insn(1, T0, T1, 4),  // lh t0, 4(t1)
        store_insn(0, T0, T1, 4), // sb t0, 4(t1)
    ];
    let (mut m, _tx) = machine(&program);
    m.cpu.x[T1 as usize] = PLIC_BASE;
    m.cpu.x[T0 as usize] = 7;

    m.cpu.step();
    assert_eq!(m.cpu.csr.mcause, LOAD_ACCESS_FAULT);
    assert_eq!(m.cpu.csr.mtval, PLIC_BASE + 4);
    assert_eq!(m.cpu.csr.mepc, RAM_BASE);
    assert_eq!(m.cpu.x[T0 as usize], 7, "rd not written");

    m.cpu.pc = RAM_BASE + 4;
    m.cpu.step();
    assert_eq!(m.cpu.csr.mcause, STORE_ACCESS_FAULT);
    assert_eq!(m.cpu.csr.mtval, PLIC_BASE + 4);
    assert_eq!(m.cpu.bus.read(PLIC_BASE + 4, 4).unwrap(), 1, "priority unchanged");
}

/// A UART byte pending on PLIC source 1, enabled for context 0 but with MEIE off.
fn machine_with_claimable_uart(program: &[u32]) -> Machine {
    let (mut m, tx) = machine(program);
    m.cpu.bus.write(ENABLE0, 4, 1 << 1).unwrap();
    tx.send(b'c').unwrap();
    // Move the byte into the UART now: the sender is dropped on return.
    m.cpu.bus.update_interrupts();
    assert_eq!(m.cpu.bus.read(PENDING, 4).unwrap(), 1 << 1);
    m
}

/// After a fault that must not have claimed anything: source 1 is still pending, and once
/// the handler gets going a real claim still works.
fn assert_source_1_unclaimed(m: &mut Machine) {
    assert_eq!(m.cpu.bus.read(PENDING, 4).unwrap(), 1 << 1, "still pending");
    for _ in 0..5 {
        m.cpu.step();
    }
    assert_eq!(m.cpu.bus.read(PENDING, 4).unwrap(), 1 << 1);
    assert_eq!(m.cpu.bus.read(CLAIM0, 4).unwrap(), 1);
}

#[test]
fn fetching_from_the_plic_is_an_access_fault_and_claims_nothing() {
    let mut m = machine_with_claimable_uart(&[]);
    m.cpu.pc = CLAIM0;
    m.cpu.step();
    assert_eq!(m.cpu.csr.mcause, INSTR_ACCESS_FAULT);
    assert_eq!(m.cpu.csr.mtval, CLAIM0);
    assert_eq!(m.cpu.csr.mepc, CLAIM0);
    assert_eq!(m.cpu.pc, HANDLER);
    assert_source_1_unclaimed(&mut m);
}

#[test]
fn page_table_walks_into_the_plic_are_access_faults_and_claim_nothing() {
    // lw t0, 0(t1) from M-mode with MPRV and MPP = S, so the load is translated while the
    // fetch is not. satp's root table is the page holding CLAIM0, and t1's VPN[1] = 1 picks
    // the PTE at CLAIM0 itself.
    let mut m = machine_with_claimable_uart(&[load_insn(2, T0, T1, 0)]);
    let vaddr = 1 << 22;
    m.cpu.x[T1 as usize] = vaddr;
    m.cpu.csr.mstatus = MSTATUS_MPRV | (1 << MSTATUS_MPP_SHIFT);
    m.cpu.csr.satp = (1 << 31) | ((CLAIM0 & !0xfff) >> 12);
    assert_eq!(((m.cpu.csr.satp & 0x3f_ffff) << 12) + (vaddr >> 22) * 4, CLAIM0);

    m.cpu.step();
    assert_eq!(m.cpu.csr.mcause, LOAD_ACCESS_FAULT);
    assert_eq!(m.cpu.csr.mtval, vaddr);
    assert_eq!(m.cpu.csr.mepc, RAM_BASE);
    m.cpu.csr.mstatus = 0;
    m.cpu.csr.satp = 0;
    assert_source_1_unclaimed(&mut m);
}

/// Runs the echo example with `input` already queued on the UART.
fn run_echo(input: &[u8]) -> (RunResult, String) {
    let mut m = Machine::with_ram(grail::DEFAULT_RAM_SIZE);
    m.load_elf(&std::fs::read(echo_elf()).unwrap()).unwrap();
    let (tx, rx) = channel();
    m.cpu.bus.uart.input = Some(rx);
    m.cpu.bus.uart.capture = true;
    for &b in input {
        tx.send(b).unwrap();
    }
    let r = m.run(Some(1_000_000), |_| {});
    (
        r,
        String::from_utf8(std::mem::take(&mut m.cpu.bus.uart.captured)).unwrap(),
    )
}

#[test]
fn echo_example_takes_one_interrupt_per_byte() {
    let (r, out) = run_echo(b"Hello, PLIC!\n\x04");
    assert_eq!(r, RunResult::Exited(0));
    assert_eq!(out, "Hello, PLIC!\n[dbg] pc=800000ac s0 = 0x0000000e (14)\n");
}

#[test]
fn echo_example_sleeps_until_input_arrives() {
    let mut m = Machine::with_ram(grail::DEFAULT_RAM_SIZE);
    m.load_elf(&std::fs::read(echo_elf()).unwrap()).unwrap();
    let (tx, rx) = channel();
    m.cpu.bus.uart.input = Some(rx);
    m.cpu.bus.uart.capture = true;

    assert_eq!(m.run(Some(10_000), |_| {}), RunResult::StepLimit);
    assert!(m.cpu.waiting, "parked in wfi");
    assert!(m.cpu.bus.uart.captured.is_empty());

    for (i, &b) in b"abc".iter().enumerate() {
        tx.send(b).unwrap();
        assert_eq!(m.run(Some(10_000), |_| {}), RunResult::StepLimit);
        assert_eq!(m.cpu.bus.uart.captured, &b"abc"[..=i]);
    }

    tx.send(4).unwrap();
    assert_eq!(m.run(Some(10_000), |_| {}), RunResult::Exited(0));
    assert!(String::from_utf8_lossy(&m.cpu.bus.uart.captured).ends_with("s0 = 0x00000004 (4)\n"));
}

#[test]
fn echo_example_from_stdin_of_the_grail_binary() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_grail"))
        .args(["--max-steps", "4000000000"])
        .arg(echo_elf())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(b"echo\x04").unwrap();
    let mut out = String::new();
    child.stdout.take().unwrap().read_to_string(&mut out).unwrap();
    let mut err = String::new();
    child.stderr.take().unwrap().read_to_string(&mut err).unwrap();
    let status = child.wait().unwrap();
    assert!(status.success(), "grail exited with {status}: {err}");
    assert_eq!(out, "echo");
    assert_eq!(err, "[dbg] pc=800000ac s0 = 0x00000005 (5)\n");
}
