//! RV64 harts (spec sections 4 to 6): what riscv-tests rv64ui, rv64um, rv64ua and
//! rv64mi do not pin down on their own. The sign-extension rules (the W
//! instructions, `lw` and `lwu`, the `.w` atomics), the RV32-only CSRs missing on
//! RV64, `misa`, `mstatus`'s XLEN fields, `satp`'s WARL rule and the interrupt bit;
//! and an RV32 hart's registers, which hold their values sign-extended.
//!
//! Each test puts hand-encoded instructions at `RAM_BASE` and steps them one at a
//! time. Every encoding is checked against excalibur's disassembly first, so the
//! comments cannot drift from the bits.

use excalibur::csr::{
    CYCLE, CYCLEH, MCYCLE, MCYCLEH, MENVCFGH, MINSTRETH, MISA, MSTATUS, MSTATUSH, MSTATUS_MIE, MSTATUS_SXL,
    MSTATUS_SXL_64, MSTATUS_UXL, MSTATUS_UXL_64, PMPCFG0, SATP, SSTATUS,
};
use excalibur::Xlen;
use grail::bus::RAM_BASE;
use grail::{Machine, MachineConfig};

const A0: u32 = 10;
const A1: u32 = 11;
const A2: u32 = 12;

/// Where the tests keep their data.
const DATA: u64 = RAM_BASE + 0x8000;
/// Trap vector: the tests only look at the cause, nothing runs there.
const TRAP_VECTOR: u64 = RAM_BASE + 0x4000;

const LOAD: u32 = 0x03;
const OP_IMM: u32 = 0x13;
const OP_IMM_32: u32 = 0x1b;
const STORE: u32 = 0x23;
const AMO: u32 = 0x2f;
const OP: u32 = 0x33;
const LUI: u32 = 0x37;
const OP_32: u32 = 0x3b;
const SYSTEM: u32 = 0x73;

fn r_type(funct7: u32, rs2: u32, rs1: u32, funct3: u32, rd: u32, opcode: u32) -> u32 {
    funct7 << 25 | rs2 << 20 | rs1 << 15 | funct3 << 12 | rd << 7 | opcode
}
fn i_type(imm: i32, rs1: u32, funct3: u32, rd: u32, opcode: u32) -> u32 {
    ((imm as u32) & 0xfff) << 20 | rs1 << 15 | funct3 << 12 | rd << 7 | opcode
}
fn s_type(imm: i32, rs2: u32, rs1: u32, funct3: u32) -> u32 {
    let imm = imm as u32 & 0xfff;
    (imm >> 5) << 25 | rs2 << 20 | rs1 << 15 | funct3 << 12 | (imm & 0x1f) << 7 | STORE
}
/// An A instruction: funct5, aq/rl clear, funct3 2 (`.w`) or 3 (`.d`).
fn a_type(funct5: u32, funct3: u32, rd: u32, rs1: u32, rs2: u32) -> u32 {
    funct5 << 27 | rs2 << 20 | rs1 << 15 | funct3 << 12 | rd << 7 | AMO
}
fn csrr(rd: u32, csr: u16) -> u32 {
    (csr as u32) << 20 | 2 << 12 | rd << 7 | SYSTEM
}
fn csrw(csr: u16, rs1: u32) -> u32 {
    (csr as u32) << 20 | rs1 << 15 | 1 << 12 | SYSTEM
}

fn machine(xlen: Xlen) -> Machine {
    let mut m = Machine::new(MachineConfig {
        xlen,
        ..MachineConfig::with_ram(1 << 20)
    })
    .unwrap();
    m.cpu.csr.mtvec = TRAP_VECTOR;
    m
}

/// Runs `word`, which must disassemble as `text`, at `RAM_BASE`, and checks it did not
/// trap.
fn run(m: &mut Machine, text: &str, word: u32) {
    assert_eq!(
        excalibur::decode(word, m.cpu.xlen()).to_string(),
        text,
        "{word:#010x}"
    );
    m.cpu.bus.load(RAM_BASE, &word.to_le_bytes()).unwrap();
    m.cpu.pc = RAM_BASE;
    m.cpu.step();
    assert_eq!(
        m.cpu.pc,
        RAM_BASE + 4,
        "{text} trapped: mcause {:#x}",
        m.cpu.csr.mcause
    );
}

/// Runs `word` (disassembling as `text`) and checks it trapped with `mcause` and
/// `mtval`.
fn run_traps(m: &mut Machine, text: &str, word: u32, mcause: u64, mtval: u64) {
    assert_eq!(
        excalibur::decode(word, m.cpu.xlen()).to_string(),
        text,
        "{word:#010x}"
    );
    m.cpu.bus.load(RAM_BASE, &word.to_le_bytes()).unwrap();
    m.cpu.pc = RAM_BASE;
    m.cpu.step();
    assert_eq!(m.cpu.pc, TRAP_VECTOR, "{text} did not trap");
    assert_eq!((m.cpu.csr.mcause, m.cpu.csr.mtval), (mcause, mtval), "{text}");
    assert_eq!(m.cpu.csr.mepc, RAM_BASE, "{text}");
}

/// Runs `word` and checks it raised illegal-instruction, with the word in `mtval`.
fn run_illegal(m: &mut Machine, text: &str, word: u32) {
    run_traps(m, text, word, 2, word.into());
}

fn x(m: &Machine, r: u32) -> u64 {
    m.cpu.x[r as usize]
}
fn set(m: &mut Machine, r: u32, v: u64) {
    m.cpu.set_reg(r as u8, v);
}
fn poke64(m: &mut Machine, addr: u64, v: u64) {
    m.cpu.bus.write(addr, 4, v).unwrap();
    m.cpu.bus.write(addr + 4, 4, v >> 32).unwrap();
}
fn peek64(m: &mut Machine, addr: u64) -> u64 {
    let lo = m.cpu.bus.read(addr, 4).unwrap();
    lo | m.cpu.bus.read(addr + 4, 4).unwrap() << 32
}

#[test]
fn lui_sign_extends_from_bit_31() {
    let mut m = machine(Xlen::Rv64);
    run(&mut m, "lui a0, 0x80000", 0x8000_0000 | A0 << 7 | LUI);
    assert_eq!(x(&m, A0), 0xffff_ffff_8000_0000);
    run(&mut m, "lui a0, 0x7ffff", 0x7fff_f000 | A0 << 7 | LUI);
    assert_eq!(x(&m, A0), 0x7fff_f000);
}

#[test]
fn w_instructions_compute_on_32_bits_and_sign_extend() {
    let mut m = machine(Xlen::Rv64);
    // addiw: the upper 32 bits of rs1 are ignored, the 32-bit sum sign-extended.
    set(&mut m, A1, 0x1234_5678_7fff_ffff);
    run(&mut m, "addiw a0, a1, 1", i_type(1, A1, 0, A0, OP_IMM_32));
    assert_eq!(x(&m, A0), 0xffff_ffff_8000_0000);
    run(&mut m, "addiw a0, a1, 0", i_type(0, A1, 0, A0, OP_IMM_32));
    assert_eq!(x(&m, A0), 0x7fff_ffff);
    // addi on all 64 bits, for contrast.
    run(&mut m, "addi a0, a1, 1", i_type(1, A1, 0, A0, OP_IMM));
    assert_eq!(x(&m, A0), 0x1234_5678_8000_0000);

    // addw/subw.
    set(&mut m, A1, 0xffff_ffff_0000_0001);
    set(&mut m, A2, 0x0000_0001_ffff_ffff);
    run(&mut m, "addw a0, a1, a2", r_type(0, A2, A1, 0, A0, OP_32));
    assert_eq!(x(&m, A0), 0);
    run(&mut m, "subw a0, a1, a2", r_type(0x20, A2, A1, 0, A0, OP_32));
    assert_eq!(x(&m, A0), 2);

    // The W shifts take 5 bits of rs2 and shift the low word; RV64's take 6.
    set(&mut m, A1, 0x0000_0000_8000_0001);
    set(&mut m, A2, 32 + 1);
    run(&mut m, "sllw a0, a1, a2", r_type(0, A2, A1, 1, A0, OP_32));
    assert_eq!(x(&m, A0), 2, "shift by 33 & 31 = 1");
    run(&mut m, "sll a0, a1, a2", r_type(0, A2, A1, 1, A0, OP));
    assert_eq!(x(&m, A0), 0x8000_0001 << 33, "shift by 33");
    run(&mut m, "srlw a0, a1, a2", r_type(0, A2, A1, 5, A0, OP_32));
    assert_eq!(x(&m, A0), 0x4000_0000);
    run(&mut m, "sraw a0, a1, a2", r_type(0x20, A2, A1, 5, A0, OP_32));
    assert_eq!(x(&m, A0), 0xffff_ffff_c000_0000);
    run(&mut m, "srliw a0, a1, 0", i_type(0, A1, 5, A0, OP_IMM_32));
    assert_eq!(x(&m, A0), 0xffff_ffff_8000_0001, "even a shift by 0 sign-extends");
    run(&mut m, "srai a0, a1, 63", i_type(0x400 | 63, A1, 5, A0, OP_IMM));
    assert_eq!(x(&m, A0), 0);
    set(&mut m, A1, 1 << 63);
    run(&mut m, "srai a0, a1, 63", i_type(0x400 | 63, A1, 5, A0, OP_IMM));
    assert_eq!(x(&m, A0), u64::MAX);
    run(&mut m, "srli a0, a1, 63", i_type(63, A1, 5, A0, OP_IMM));
    assert_eq!(x(&m, A0), 1);
}

#[test]
fn rv64m_on_64_and_32_bits() {
    let mut m = machine(Xlen::Rv64);
    let mul = |f3| r_type(1, A2, A1, f3, A0, OP);
    let mulw = |f3| r_type(1, A2, A1, f3, A0, OP_32);

    // The upper halves of 128-bit products.
    set(&mut m, A1, u64::MAX); // -1
    set(&mut m, A2, u64::MAX);
    run(&mut m, "mulh a0, a1, a2", mul(1));
    assert_eq!(x(&m, A0), 0, "-1 * -1 = 1");
    run(&mut m, "mulhsu a0, a1, a2", mul(2));
    assert_eq!(x(&m, A0), u64::MAX, "-1 * (2^64 - 1)");
    run(&mut m, "mulhu a0, a1, a2", mul(3));
    assert_eq!(x(&m, A0), u64::MAX - 1);

    // i64::MIN / -1 and division by zero.
    set(&mut m, A1, 1 << 63);
    run(&mut m, "div a0, a1, a2", mul(4));
    assert_eq!(x(&m, A0), 1 << 63);
    run(&mut m, "rem a0, a1, a2", mul(6));
    assert_eq!(x(&m, A0), 0);
    set(&mut m, A2, 0);
    run(&mut m, "div a0, a1, a2", mul(4));
    assert_eq!(x(&m, A0), u64::MAX);
    run(&mut m, "divu a0, a1, a2", mul(5));
    assert_eq!(x(&m, A0), u64::MAX);
    run(&mut m, "rem a0, a1, a2", mul(6));
    assert_eq!(x(&m, A0), 1 << 63);
    run(&mut m, "remu a0, a1, a2", mul(7));
    assert_eq!(x(&m, A0), 1 << 63);

    // The W forms: 32-bit operands, results sign-extended.
    set(&mut m, A1, 0x1234_5678_8000_0000); // the low word is i32::MIN
    set(&mut m, A2, u64::MAX);
    run(&mut m, "divw a0, a1, a2", mulw(4));
    assert_eq!(x(&m, A0), 0xffff_ffff_8000_0000, "i32::MIN / -1");
    run(&mut m, "remw a0, a1, a2", mulw(6));
    assert_eq!(x(&m, A0), 0);
    run(&mut m, "mulw a0, a1, a2", mulw(0));
    assert_eq!(x(&m, A0), 0xffff_ffff_8000_0000, "-i32::MIN wraps to itself");
    set(&mut m, A2, 0xffff_ffff_0000_0000); // the low word is 0
    run(&mut m, "divw a0, a1, a2", mulw(4));
    assert_eq!(x(&m, A0), u64::MAX);
    run(&mut m, "divuw a0, a1, a2", mulw(5));
    assert_eq!(x(&m, A0), u64::MAX, "0xffff_ffff sign-extended");
    run(&mut m, "remw a0, a1, a2", mulw(6));
    assert_eq!(x(&m, A0), 0xffff_ffff_8000_0000, "the dividend, sign-extended");
    run(&mut m, "remuw a0, a1, a2", mulw(7));
    assert_eq!(x(&m, A0), 0xffff_ffff_8000_0000);
    set(&mut m, A2, 2);
    run(&mut m, "divuw a0, a1, a2", mulw(5));
    assert_eq!(x(&m, A0), 0x4000_0000, "unsigned: 0x8000_0000 / 2");
}

#[test]
fn lw_sign_extends_and_lwu_zero_extends() {
    let mut m = machine(Xlen::Rv64);
    poke64(&mut m, DATA, 0x1122_3344_8899_aabb);
    set(&mut m, A1, DATA);
    run(&mut m, "lw a0, 0(a1)", i_type(0, A1, 2, A0, LOAD));
    assert_eq!(x(&m, A0), 0xffff_ffff_8899_aabb);
    run(&mut m, "lwu a0, 0(a1)", i_type(0, A1, 6, A0, LOAD));
    assert_eq!(x(&m, A0), 0x8899_aabb);
    run(&mut m, "lw a0, 4(a1)", i_type(4, A1, 2, A0, LOAD));
    assert_eq!(x(&m, A0), 0x1122_3344);
    run(&mut m, "ld a0, 0(a1)", i_type(0, A1, 3, A0, LOAD));
    assert_eq!(x(&m, A0), 0x1122_3344_8899_aabb);
    run(&mut m, "lh a0, 0(a1)", i_type(0, A1, 1, A0, LOAD));
    assert_eq!(x(&m, A0), 0xffff_ffff_ffff_aabb);
    run(&mut m, "lbu a0, 1(a1)", i_type(1, A1, 4, A0, LOAD));
    assert_eq!(x(&m, A0), 0xaa);

    // sd writes 8 bytes, sw the low 4 of the register.
    set(&mut m, A2, 0x0102_0304_0506_0708);
    run(&mut m, "sd a2, 8(a1)", s_type(8, A2, A1, 3));
    assert_eq!(peek64(&mut m, DATA + 8), 0x0102_0304_0506_0708);
    run(&mut m, "sw a2, 8(a1)", s_type(8, A2, A1, 2));
    run(&mut m, "sw a2, 12(a1)", s_type(12, A2, A1, 2));
    assert_eq!(peek64(&mut m, DATA + 8), 0x0506_0708_0506_0708);

    // ld and sd must be 8-byte aligned (spec section 4).
    let at = DATA + 4;
    run_traps(&mut m, "ld a0, 4(a1)", i_type(4, A1, 3, A0, LOAD), 4, at);
    run_traps(&mut m, "sd a2, 4(a1)", s_type(4, A2, A1, 3), 6, at);
    // An 8-byte access to 32-bit registers (the UART's) is an access fault.
    let uart = grail::bus::UART_BASE;
    set(&mut m, A1, uart);
    run_traps(&mut m, "ld a0, 0(a1)", i_type(0, A1, 3, A0, LOAD), 5, uart);
    // The CLINT's mtime is a 64-bit register: one ld reads it whole (after the step's
    // own tick).
    m.cpu.bus.clint.mtime = 0x1234_5678_ffff_fffe;
    set(&mut m, A1, grail::bus::CLINT_BASE + 0xbff8);
    run(&mut m, "ld a0, 0(a1)", i_type(0, A1, 3, A0, LOAD));
    assert_eq!(x(&m, A0), 0x1234_5678_ffff_ffff);
}

#[test]
fn word_atomics_sign_extend_and_touch_32_bits() {
    let mut m = machine(Xlen::Rv64);
    set(&mut m, A1, DATA);

    // lr.w sign-extends; sc.w stores the low 32 bits of rs2 and leaves the next word.
    poke64(&mut m, DATA, 0x5555_5555_8000_0000);
    run(&mut m, "lr.w a0, (a1)", a_type(0b00010, 2, A0, A1, 0));
    assert_eq!(x(&m, A0), 0xffff_ffff_8000_0000);
    set(&mut m, A2, 0xaaaa_aaaa_1234_5678);
    run(&mut m, "sc.w a0, a2, (a1)", a_type(0b00011, 2, A0, A1, A2));
    assert_eq!(x(&m, A0), 0, "stored");
    assert_eq!(peek64(&mut m, DATA), 0x5555_5555_1234_5678);

    // amoadd.w: a 32-bit sum (the carry does not reach the next word), rd the old word
    // sign-extended.
    poke64(&mut m, DATA, 0x5555_5555_ffff_ffff);
    set(&mut m, A2, 0x7fff_ffff_0000_0001);
    run(&mut m, "amoadd.w a0, a2, (a1)", a_type(0b00000, 2, A0, A1, A2));
    assert_eq!(x(&m, A0), u64::MAX, "the old word, -1");
    assert_eq!(peek64(&mut m, DATA), 0x5555_5555_0000_0000);

    // amomin.w compares 32-bit signed values: 0x8000_0000 is the smaller, whatever the
    // upper bits of rs2 say.
    poke64(&mut m, DATA, 1);
    set(&mut m, A2, 0x0000_0000_8000_0000);
    run(&mut m, "amomin.w a0, a2, (a1)", a_type(0b10000, 2, A0, A1, A2));
    assert_eq!(peek64(&mut m, DATA), 0x8000_0000);
    run(&mut m, "amomaxu.w a0, a2, (a1)", a_type(0b11100, 2, A0, A1, A2));
    assert_eq!(x(&m, A0), 0xffff_ffff_8000_0000);

    // The .d forms work on all 64 bits, and must be 8-byte aligned.
    poke64(&mut m, DATA, 0x7fff_ffff_ffff_ffff);
    set(&mut m, A2, 1);
    run(&mut m, "amoadd.d a0, a2, (a1)", a_type(0b00000, 3, A0, A1, A2));
    assert_eq!(x(&m, A0), 0x7fff_ffff_ffff_ffff);
    assert_eq!(peek64(&mut m, DATA), 1 << 63);
    run(&mut m, "lr.d a0, (a1)", a_type(0b00010, 3, A0, A1, 0));
    assert_eq!(x(&m, A0), 1 << 63);
    run(&mut m, "sc.w a0, a2, (a1)", a_type(0b00011, 2, A0, A1, A2));
    assert_eq!(x(&m, A0), 1, "sc.w after lr.d fails");
    let at = DATA + 4;
    set(&mut m, A1, at);
    run_traps(&mut m, "lr.d a0, (a1)", a_type(0b00010, 3, A0, A1, 0), 4, at);
    run(&mut m, "lr.w a0, (a1)", a_type(0b00010, 2, A0, A1, 0));
}

#[test]
fn rv32_only_csrs_are_illegal_on_rv64() {
    let csrs = [
        CYCLEH,
        CYCLEH + 1,
        CYCLEH + 2,
        MCYCLEH,
        MINSTRETH,
        MSTATUSH,
        MENVCFGH,
        PMPCFG0 + 1,
        PMPCFG0 + 3,
    ];
    for csr in csrs {
        let mut m = machine(Xlen::Rv64);
        let text = excalibur::decode(csrr(A0, csr), Xlen::Rv64).to_string();
        run_illegal(&mut m, &text, csrr(A0, csr));
        let text = excalibur::decode(csrw(csr, A0), Xlen::Rv64).to_string();
        run_illegal(&mut m, &text, csrw(csr, A0));
        // They are there on RV32.
        let mut m = machine(Xlen::Rv32);
        let text = excalibur::decode(csrr(A0, csr), Xlen::Rv32).to_string();
        run(&mut m, &text, csrr(A0, csr));
    }
}

#[test]
fn counters_read_64_bits_on_rv64() {
    let mut m = machine(Xlen::Rv64);
    m.cpu.csr.mcycle = 0x1_2345_6789;
    run(&mut m, "csrrs a0, mcycle, zero", csrr(A0, MCYCLE));
    // The read happens during the instruction's cycle, before it is counted.
    assert_eq!(x(&m, A0), 0x1_2345_6789);
    run(&mut m, "csrrs a0, cycle, zero", csrr(A0, CYCLE));
    assert_eq!(x(&m, A0), 0x1_2345_678a);
    set(&mut m, A1, 0x5_0000_0000);
    run(&mut m, "csrrw zero, mcycle, a1", csrw(MCYCLE, A1));
    // The write sets all 64 bits; the instruction's own cycle is still counted.
    assert_eq!(m.cpu.csr.mcycle, 0x5_0000_0001);

    // RV32: the low half, and the high one through cycleh.
    let mut m = machine(Xlen::Rv32);
    m.cpu.csr.mcycle = 0x1_8000_0000;
    run(&mut m, "csrrs a0, cycle, zero", csrr(A0, CYCLE));
    assert_eq!(
        x(&m, A0),
        0xffff_ffff_8000_0000,
        "an RV32 register holds it sign-extended"
    );
    run(&mut m, "csrrs a0, cycleh, zero", csrr(A0, CYCLEH));
    assert_eq!(x(&m, A0), 1);
}

#[test]
fn misa_mstatus_and_sstatus_show_the_width() {
    let mut m = machine(Xlen::Rv64);
    run(&mut m, "csrrs a0, misa, zero", csrr(A0, MISA));
    assert_eq!(x(&m, A0), 0x8000_0000_0094_1101);
    run(&mut m, "csrrs a0, mstatus, zero", csrr(A0, MSTATUS));
    assert_eq!(
        x(&m, A0) & (MSTATUS_UXL | MSTATUS_SXL),
        MSTATUS_UXL_64 | MSTATUS_SXL_64
    );
    assert_eq!(x(&m, A0) >> 63, 0, "SD reads 0");
    run(&mut m, "csrrs a0, sstatus, zero", csrr(A0, SSTATUS));
    assert_eq!(x(&m, A0) & (MSTATUS_UXL | MSTATUS_SXL), MSTATUS_UXL_64);
    // UXL, SXL and SD are read-only: writing other values changes nothing.
    set(&mut m, A1, MSTATUS_MIE);
    run(&mut m, "csrrw zero, mstatus, a1", csrw(MSTATUS, A1));
    run(&mut m, "csrrs a0, mstatus, zero", csrr(A0, MSTATUS));
    assert_eq!(x(&m, A0), MSTATUS_UXL_64 | MSTATUS_SXL_64 | MSTATUS_MIE);
    set(&mut m, A1, u64::MAX);
    run(&mut m, "csrrw zero, misa, a1", csrw(MISA, A1));
    run(&mut m, "csrrs a0, misa, zero", csrr(A0, MISA));
    assert_eq!(x(&m, A0), 0x8000_0000_0094_1101);

    let mut m = machine(Xlen::Rv32);
    run(&mut m, "csrrs a0, misa, zero", csrr(A0, MISA));
    assert_eq!(x(&m, A0), 0x4094_1101);
    run(&mut m, "csrrs a0, mstatus, zero", csrr(A0, MSTATUS));
    assert_eq!(x(&m, A0), 0, "no UXL or SXL on RV32");
}

#[test]
fn satp_keeps_only_what_the_hart_implements() {
    let mut m = machine(Xlen::Rv64);
    // Bare, with ASID bits (there are no ASIDs) and a PPN wider than a 48-bit physical
    // address.
    set(&mut m, A1, 0x0fff_ffff_ffff_ffff);
    run(&mut m, "csrrw zero, satp, a1", csrw(SATP, A1));
    assert_eq!(m.cpu.csr.satp, 0x0000_000f_ffff_ffff, "ASID 0, PPN bits 35:0");
    // A MODE the hart does not implement changes nothing: the reserved ones, Sv57 (10)
    // and Sv64 (11).
    for mode in [1u64, 7, 10, 11, 15] {
        set(&mut m, A1, mode << 60 | 0x1234);
        run(&mut m, "csrrw zero, satp, a1", csrw(SATP, A1));
        assert_eq!(m.cpu.csr.satp, 0x0000_000f_ffff_ffff, "MODE {mode}");
    }
    // Sv39 (8) and Sv48 (9) are kept, with the same masks.
    for mode in [8u64, 9] {
        set(&mut m, A1, mode << 60 | 0xffff_ffff_ffff_ffff >> 4);
        run(&mut m, "csrrw zero, satp, a1", csrw(SATP, A1));
        assert_eq!(m.cpu.csr.satp, mode << 60 | 0x0000_000f_ffff_ffff, "MODE {mode}");
    }

    // RV32 keeps any value, MODE in bit 31.
    let mut m = machine(Xlen::Rv32);
    set(&mut m, A1, 0x8000_1234);
    run(&mut m, "csrrw zero, satp, a1", csrw(SATP, A1));
    assert_eq!(
        m.cpu.csr.satp, 0x8000_1234,
        "zero-extended, though a1 holds it sign-extended"
    );
}

#[test]
fn interrupts_set_the_top_bit_of_the_width() {
    for (xlen, bit) in [(Xlen::Rv64, 1u64 << 63), (Xlen::Rv32, 1 << 31)] {
        let mut m = machine(xlen);
        // The timer fires at once: mtimecmp 0, mie.MTIE, mstatus.MIE.
        m.cpu.bus.clint.mtimecmp[0] = 0;
        m.cpu.csr.mie = 1 << 7;
        m.cpu.csr.mstatus = MSTATUS_MIE;
        m.cpu.pc = RAM_BASE;
        m.cpu.step();
        assert_eq!(m.cpu.pc, TRAP_VECTOR, "{xlen:?}");
        assert_eq!(m.cpu.csr.mcause, bit | 7, "{xlen:?}");
    }
}

#[test]
fn rv32_registers_hold_values_sign_extended() {
    let mut m = machine(Xlen::Rv32);
    run(&mut m, "lui a0, 0x80000", 0x8000_0000 | A0 << 7 | LUI);
    assert_eq!(x(&m, A0), 0xffff_ffff_8000_0000);
    // 32-bit arithmetic: 0x7fff_ffff + 1 wraps to 0x8000_0000, held sign-extended.
    set(&mut m, A1, 0x7fff_ffff);
    run(&mut m, "addi a0, a1, 1", i_type(1, A1, 0, A0, OP_IMM));
    assert_eq!(x(&m, A0), 0xffff_ffff_8000_0000);
    // Comparisons on the sign-extended values give the 32-bit answers.
    set(&mut m, A2, 1);
    run(&mut m, "sltu a0, a2, a0", r_type(0, A0, A2, 3, A0, OP));
    assert_eq!(x(&m, A0), 1, "1 < 0x8000_0000 unsigned");
    // A shift amount is 5 bits, and srl shifts the 32-bit value.
    set(&mut m, A1, 0x8000_0000);
    set(&mut m, A2, 33);
    run(&mut m, "srl a0, a1, a2", r_type(0, A2, A1, 5, A0, OP));
    assert_eq!(x(&m, A0), 0x4000_0000);
    // mulhu: the upper 32 bits of the 64-bit product.
    set(&mut m, A2, 0x8000_0000);
    run(&mut m, "mulhu a0, a1, a2", r_type(1, A2, A1, 3, A0, OP));
    assert_eq!(x(&m, A0), 0x4000_0000);
    // lw gives the loaded word sign-extended, and set_reg cuts what it is given.
    m.cpu.bus.write(DATA, 4, 0x8000_0001).unwrap();
    set(&mut m, A1, DATA);
    run(&mut m, "lw a0, 0(a1)", i_type(0, A1, 2, A0, LOAD));
    assert_eq!(x(&m, A0), 0xffff_ffff_8000_0001);
    set(&mut m, A0, 0x1_0000_0002);
    assert_eq!(x(&m, A0), 2);
    // RV64's instructions are illegal.
    let ld = i_type(0, A1, 3, A0, LOAD);
    run_illegal(&mut m, &format!(".word {ld:#010x}"), ld);
    let addiw = i_type(1, A1, 0, A0, OP_IMM_32);
    run_illegal(&mut m, &format!(".word {addiw:#010x}"), addiw);
}

#[test]
fn rv32_addresses_wrap_at_4_gib() {
    let mut m = machine(Xlen::Rv32);
    // a1 holds 0x8000_7ff0 sign-extended; the sum is cut to its low 32 bits, DATA.
    m.cpu.bus.write(DATA, 4, 0x1234_5678).unwrap();
    set(&mut m, A1, DATA.wrapping_sub(16));
    run(&mut m, "lw a0, 16(a1)", i_type(16, A1, 2, A0, LOAD));
    assert_eq!(x(&m, A0), 0x1234_5678);
    // A jump target is cut to 32 bits: 0xffff_ffff_8000_0102 & !1 is 0x8000_0102,
    // misaligned, and mtval holds it cut.
    set(&mut m, A1, 0xffff_ffff_8000_0102);
    run_traps(
        &mut m,
        "jalr a0, 0(a1)",
        i_type(0, A1, 0, A0, 0x67),
        0,
        0x8000_0102,
    );
}

/// riscv-tests' `simple`, built for each width.
fn simple(xlen: Xlen) -> Vec<u8> {
    let path = match xlen {
        Xlen::Rv32 => "tests/riscv-tests/rv32ui-p-simple",
        Xlen::Rv64 => "tests/riscv-tests/rv64/rv64ui-p-simple",
    };
    std::fs::read(format!("{}/../../{path}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

#[test]
fn an_elf64_program_makes_the_machine_rv64() {
    let mut m = Machine::with_ram(1 << 20);
    assert_eq!(m.cpu.xlen(), Xlen::Rv32);
    m.load_elf(&simple(Xlen::Rv64)).unwrap();
    assert_eq!((m.cpu.xlen(), m.config().xlen), (Xlen::Rv64, Xlen::Rv64));
    // The boot ROM's stub reads the entry word with lwu, so the program starts at
    // 0x8000_0000, not at its sign-extension.
    for _ in 0..4 {
        m.cpu.step();
    }
    assert_eq!(m.cpu.pc, RAM_BASE);

    // An RV32 machine stays RV32 for an ELF32 program; an RV64 one refuses it.
    let mut m = Machine::with_ram(1 << 20);
    m.load_elf(&simple(Xlen::Rv32)).unwrap();
    assert_eq!(m.cpu.xlen(), Xlen::Rv32);
    let mut m = machine(Xlen::Rv64);
    assert_eq!(
        m.load_elf(&simple(Xlen::Rv32)),
        Err("an RV32 program (ELF32) cannot run on an RV64 machine".into())
    );
}

/// `sb` and `sh` hand the bus only the bytes they store, never the rest of the
/// register: a device (the test device here) or the riscv-tests `tohost` word sees
/// exactly what was written.
#[test]
fn narrow_stores_write_only_their_bytes() {
    for xlen in [Xlen::Rv32, Xlen::Rv64] {
        let mut m = machine(xlen);
        // sb of 0x5555 writes 0x55 to the test device: not its exit code 0x5555.
        set(&mut m, A1, grail::bus::TEST_DEV_BASE);
        set(&mut m, A2, 0x5555);
        run(&mut m, "sb a2, 0(a1)", s_type(0, A2, A1, 0));
        assert_eq!(m.exit_code(), None, "{xlen:?}");
        run(&mut m, "sh a2, 0(a1)", s_type(0, A2, A1, 1));
        assert_eq!(m.exit_code(), Some(0), "{xlen:?}: the halfword is 0x5555");

        // sb of 0x100 writes a zero byte to tohost, which does not stop the machine.
        let mut m = machine(xlen);
        m.cpu.bus.tohost = Some(DATA);
        set(&mut m, A1, DATA);
        set(&mut m, A2, 0x100);
        run(&mut m, "sb a2, 0(a1)", s_type(0, A2, A1, 0));
        assert_eq!(m.exit_code(), None, "{xlen:?}");
    }
}

/// The disk loader and the boot chain it starts are RV32 code until #151: an RV64
/// machine refuses to boot from disk rather than running them.
#[test]
fn an_rv64_machine_does_not_boot_from_disk_yet() {
    let disk = || grail::DiskConfig::memory(vec![0; 1 << 20]);
    let mut m = Machine::new(MachineConfig {
        xlen: Xlen::Rv64,
        disks: vec![disk()],
        ..MachineConfig::default()
    })
    .unwrap();
    assert_eq!(
        m.boot_from_disk(),
        Err("booting from disk takes an RV32 machine: its boot code is RV32".into())
    );
    let mut m = Machine::new(MachineConfig {
        disks: vec![disk()],
        ..MachineConfig::default()
    })
    .unwrap();
    assert_eq!(m.boot_from_disk(), Ok(()));
}

/// On a machine of several harts, loading an ELF64 program rebuilds the boot ROM for
/// RV64: the stub and the park loop read the entry word with `lwu`, so hart 0 and a
/// hart woken by an IPI both start at 0x8000_0000, not at its sign-extension.
#[test]
fn the_park_loop_reads_the_entry_with_lwu_on_rv64() {
    const LW_T0: u32 = 0x0102_a283; // lw t0, 16(t0)
    const LWU_T0: u32 = 0x0102_e283; // lwu t0, 16(t0)
    let mut m = Machine::new(MachineConfig {
        harts: 2,
        ..MachineConfig::with_ram(1 << 20)
    })
    .unwrap();
    let rom_words = |m: &Machine| -> Vec<u32> {
        let rom = &m.cpu.bus.rom;
        rom.chunks(4)
            .map(|w| u32::from_le_bytes(w.try_into().unwrap()))
            .collect()
    };
    assert_eq!(
        rom_words(&m).iter().filter(|&&w| w == LW_T0).count(),
        2,
        "RV32 at first"
    );
    m.load_elf(&simple(Xlen::Rv64)).unwrap();
    let words = rom_words(&m);
    assert_eq!(
        words.iter().filter(|&&w| w == LWU_T0).count(),
        2,
        "the stub and the park loop"
    );
    assert!(!words.contains(&LW_T0));

    // Hart 1 waits in the park loop until its msip is set, then jumps to the entry.
    for _ in 0..20 {
        m.cpu.step();
    }
    assert!(m.cpu.hart(1).waiting, "parked in wfi");
    m.cpu.bus.clint.msip[1] = true;
    for _ in 0..20 {
        m.cpu.step();
        if m.cpu.hart(1).pc == RAM_BASE {
            return;
        }
    }
    panic!("hart 1 did not reach the entry: pc {:#x}", m.cpu.hart(1).pc);
}
