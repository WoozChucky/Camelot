//! RV64 harts (spec sections 4 to 6): what riscv-tests rv64ui, rv64um, rv64ua and
//! rv64mi do not pin down on their own. The sign-extension rules (the W
//! instructions, `lw` and `lwu`, the `.w` atomics), the RV32-only CSRs missing on
//! RV64, `misa`, `mstatus`'s XLEN fields, `satp`'s WARL rule and the interrupt bit.
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

fn machine() -> Machine {
    let mut m = Machine::new(MachineConfig::with_ram(1 << 20)).unwrap();
    m.cpu.csr.mtvec = TRAP_VECTOR;
    m
}

/// Runs `word`, which must disassemble as `text`, at `RAM_BASE`, and checks it did not
/// trap.
fn run(m: &mut Machine, text: &str, word: u32) {
    assert_eq!(
        excalibur::decode(word, Xlen::Rv64).to_string(),
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
        excalibur::decode(word, Xlen::Rv64).to_string(),
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
    let mut m = machine();
    run(&mut m, "lui a0, 0x80000", 0x8000_0000 | A0 << 7 | LUI);
    assert_eq!(x(&m, A0), 0xffff_ffff_8000_0000);
    run(&mut m, "lui a0, 0x7ffff", 0x7fff_f000 | A0 << 7 | LUI);
    assert_eq!(x(&m, A0), 0x7fff_f000);
}

#[test]
fn w_instructions_compute_on_32_bits_and_sign_extend() {
    let mut m = machine();
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
    let mut m = machine();
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
    let mut m = machine();
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

/// An RV64 machine's sysinfo block is version 2 (spec section 9.5): its RAM size is a
/// 64-bit register that one ld reads whole, and its 32-bit registers take no ld.
#[test]
fn sysinfo_is_version_2() {
    use grail::sysinfo::{REG_RAM_SIZE, REG_VERSION};
    let sysinfo = grail::bus::SYSINFO_BASE;
    let mut m = machine();
    set(&mut m, A1, sysinfo);
    run(
        &mut m,
        "lw a0, 4(a1)",
        i_type(REG_VERSION as i32, A1, 2, A0, LOAD),
    );
    assert_eq!(x(&m, A0), 2);
    let ram_size = REG_RAM_SIZE as i32;
    run(&mut m, "ld a0, 16(a1)", i_type(ram_size, A1, 3, A0, LOAD));
    assert_eq!(x(&m, A0), m.config().ram_size as u64);
    run(&mut m, "lwu a0, 20(a1)", i_type(ram_size + 4, A1, 6, A0, LOAD));
    assert_eq!(x(&m, A0), m.config().ram_size as u64 >> 32);
    run_traps(&mut m, "ld a0, 0(a1)", i_type(0, A1, 3, A0, LOAD), 5, sysinfo);
}

/// The disks have version 2's registers (spec section 9.6): sd writes a 64-bit BLK_DMA
/// whole, and a command reads into the buffer it names.
#[test]
fn disks_have_64_bit_registers() {
    use grail::block as v2;
    use grail::block::{CMD_READ, SECTOR_SIZE};
    let image: Vec<u8> = (1..=2u8).flat_map(|n| [n; SECTOR_SIZE]).collect();
    let mut m = Machine::new(MachineConfig {
        disks: vec![grail::DiskConfig::memory(image)],
        ..MachineConfig::with_ram(1 << 20)
    })
    .unwrap();
    m.cpu.csr.mtvec = TRAP_VECTOR;
    let disk = grail::bus::BLK_BASE;
    set(&mut m, A1, disk);
    run(
        &mut m,
        "ld a0, 0(a1)",
        i_type(v2::REG_CAPACITY as i32, A1, 3, A0, LOAD),
    );
    assert_eq!(x(&m, A0), 2);
    set(&mut m, A2, 1);
    run(&mut m, "sd a2, 8(a1)", s_type(v2::REG_SECTOR as i32, A2, A1, 3));
    run(&mut m, "sw a2, 24(a1)", s_type(v2::REG_COUNT as i32, A2, A1, 2));
    set(&mut m, A2, DATA);
    run(&mut m, "sd a2, 16(a1)", s_type(v2::REG_DMA as i32, A2, A1, 3));
    set(&mut m, A2, CMD_READ.into());
    run(&mut m, "sw a2, 32(a1)", s_type(v2::REG_CMD as i32, A2, A1, 2));
    assert_eq!(peek64(&mut m, DATA), 0x0202_0202_0202_0202, "sector 1");
    run(
        &mut m,
        "lw a0, 40(a1)",
        i_type(v2::REG_ERROR as i32, A1, 2, A0, LOAD),
    );
    assert_eq!(x(&m, A0), 0);
    // BLK_COUNT is 32-bit: no sd.
    let count = disk + u64::from(v2::REG_COUNT);
    run_traps(
        &mut m,
        "sd a2, 24(a1)",
        s_type(v2::REG_COUNT as i32, A2, A1, 3),
        7,
        count,
    );
}

#[test]
fn word_atomics_sign_extend_and_touch_32_bits() {
    let mut m = machine();
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
        let mut m = machine();
        let text = excalibur::decode(csrr(A0, csr), Xlen::Rv64).to_string();
        run_illegal(&mut m, &text, csrr(A0, csr));
        let text = excalibur::decode(csrw(csr, A0), Xlen::Rv64).to_string();
        run_illegal(&mut m, &text, csrw(csr, A0));
    }
}

#[test]
fn counters_read_64_bits_on_rv64() {
    let mut m = machine();
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
}

#[test]
fn misa_mstatus_and_sstatus_show_the_width() {
    let mut m = machine();
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
}

#[test]
fn satp_keeps_only_what_the_hart_implements() {
    let mut m = machine();
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

    // A hart without Sv48 (--paging sv39, #172) refuses MODE 9 like any other it
    // lacks, which is how a kernel finds out, and keeps Sv39.
    let mut m = Machine::new(MachineConfig {
        paging: grail::Paging::Sv39,
        ..MachineConfig::with_ram(1 << 20)
    })
    .unwrap();
    m.cpu.csr.mtvec = TRAP_VECTOR;
    set(&mut m, A1, 9 << 60 | 0x1234);
    run(&mut m, "csrrw zero, satp, a1", csrw(SATP, A1));
    assert_eq!(m.cpu.csr.satp, 0, "no Sv48");
    set(&mut m, A1, 8 << 60 | 0x1234);
    run(&mut m, "csrrw zero, satp, a1", csrw(SATP, A1));
    assert_eq!(m.cpu.csr.satp, 8 << 60 | 0x1234, "Sv39");
    assert_eq!(grail::parse_paging("sv39"), Ok(grail::Paging::Sv39));
    assert_eq!(grail::parse_paging("sv48"), Ok(grail::Paging::Sv48));
    assert!(grail::parse_paging("sv57").is_err());
}

#[test]
fn interrupts_set_bit_63() {
    let mut m = machine();
    // The timer fires at once: mtimecmp 0, mie.MTIE, mstatus.MIE.
    m.cpu.bus.clint.mtimecmp[0] = 0;
    m.cpu.csr.mie = 1 << 7;
    m.cpu.csr.mstatus = MSTATUS_MIE;
    m.cpu.pc = RAM_BASE;
    m.cpu.step();
    assert_eq!(m.cpu.pc, TRAP_VECTOR);
    assert_eq!(m.cpu.csr.mcause, 1 << 63 | 7);
}

/// riscv-tests' `simple`.
fn simple() -> Vec<u8> {
    let path = "tests/riscv-tests/rv64ui-p-simple";
    std::fs::read(format!("{}/../../{path}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

#[test]
fn an_elf64_program_runs_and_an_elf32_one_is_refused() {
    let mut m = Machine::with_ram(1 << 20);
    m.load_elf(&simple()).unwrap();
    // The boot ROM's stub reads the entry point with ld, so the program starts at
    // 0x8000_0000, not at its sign-extension.
    for _ in 0..4 {
        m.cpu.step();
    }
    assert_eq!(m.cpu.pc, RAM_BASE);

    // An RV32 program: there is no RV32 mode (mstatus.UXL is hard-wired to 64).
    let rv32 = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../elf/tests/llvm/rv32/prog.elf"
    ))
    .unwrap();
    let mut m = machine();
    let e = m.load_elf(&rv32).unwrap_err();
    assert!(e.ends_with(grail::elf::NO_ELF32), "{e}");
}

/// `sb` and `sh` hand the bus only the bytes they store, never the rest of the
/// register: a device (the test device here) or the riscv-tests `tohost` word sees
/// exactly what was written.
#[test]
fn narrow_stores_write_only_their_bytes() {
    let mut m = machine();
    // sb of 0x5555 writes 0x55 to the test device: not its exit code 0x5555.
    set(&mut m, A1, grail::bus::TEST_DEV_BASE);
    set(&mut m, A2, 0x5555);
    run(&mut m, "sb a2, 0(a1)", s_type(0, A2, A1, 0));
    assert_eq!(m.exit_code(), None);
    run(&mut m, "sh a2, 0(a1)", s_type(0, A2, A1, 1));
    assert_eq!(m.exit_code(), Some(0), "the halfword is 0x5555");

    // sb of 0x100 writes a zero byte to tohost, which does not stop the machine.
    let mut m = machine();
    m.cpu.bus.tohost = Some(DATA);
    set(&mut m, A1, DATA);
    set(&mut m, A2, 0x100);
    run(&mut m, "sb a2, 0(a1)", s_type(0, A2, A1, 0));
    assert_eq!(m.exit_code(), None);
}

/// A machine boots from disk through the ROM's disk loader (#151), which the stub
/// reaches through the entry doubleword (`ld`). crates/grail/tests/disk_boot.rs boots
/// blocks with it.
#[test]
fn a_machine_boots_from_disk_with_the_roms_loader() {
    let disk = || grail::DiskConfig::memory(vec![0; 1 << 20]);
    let at = (grail::boot::DISK_BOOT_PC - grail::bus::BOOT_ROM_BASE) as usize;
    let mut m = Machine::new(MachineConfig {
        disks: vec![disk()],
        ..MachineConfig::default()
    })
    .unwrap();
    assert_eq!(m.boot_from_disk(), Ok(()));
    assert_eq!(&m.cpu.bus.rom[at..], &grail::boot::disk_loader()[..]);
    assert!(
        grail::elf::parse(grail::boot::DISKBOOT_ELF).is_ok(),
        "an ELF64 file"
    );
}

/// On a machine of several harts, the stub and the park loop read the doubleword entry
/// point with `ld`, so hart 0 and a hart woken by an IPI both start at 0x8000_0000, not
/// at its sign-extension.
#[test]
fn the_park_loop_reads_the_entry_with_ld() {
    const LW_T0: u32 = 0x0102_a283; // lw t0, 16(t0)
    const LD_T0: u32 = 0x0102_b283; // ld t0, 16(t0)
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
    m.load_elf(&simple()).unwrap();
    let words = rom_words(&m);
    assert_eq!(
        words.iter().filter(|&&w| w == LD_T0).count(),
        2,
        "the stub and the park loop"
    );
    assert!(!words.contains(&LW_T0));
    assert_eq!(m.cpu.bus.read_mem(grail::boot::BOOT_ENTRY_ADDR, 8), Ok(RAM_BASE));

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

/// The entry point at 0x1010 is the boot ROM's one writable location: a doubleword,
/// written whole by an 8-byte store (no 4-byte halves).
#[test]
fn the_boot_entry_is_written_whole() {
    let entry = grail::boot::BOOT_ENTRY_ADDR;
    let mut m = machine();
    m.cpu.bus.set_boot_entry(0x1_2345_6780);
    assert_eq!(m.cpu.bus.read_mem(entry, 8), Ok(0x1_2345_6780), "above 4 GiB");
    set(&mut m, A1, entry);
    set(&mut m, A2, 0x0000_00ab_8000_1000);
    run(&mut m, "sd a2, 0(a1)", s_type(0, A2, A1, 3));
    assert_eq!(m.cpu.bus.read_mem(entry, 8), Ok(0x0000_00ab_8000_1000));
    // Half of it, or anything else in the ROM, is an access fault.
    run_traps(&mut m, "sw a2, 0(a1)", s_type(0, A2, A1, 2), 7, entry);
    run_traps(&mut m, "sw a2, 4(a1)", s_type(4, A2, A1, 2), 7, entry + 4);
    run_traps(&mut m, "sd a2, 8(a1)", s_type(8, A2, A1, 3), 7, entry + 8);
    assert_eq!(m.cpu.bus.read_mem(entry, 8), Ok(0x0000_00ab_8000_1000));
}

#[test]
fn ld_and_sd_reach_ram_past_4_gib() {
    // 6 GiB of RAM, backed only where it is touched. In M-mode addresses are physical,
    // so `ld` and `sd` at RAM_BASE + 5 GiB use the whole 64-bit address.
    let mut m = Machine::new(MachineConfig::with_ram(6 << 30)).unwrap();
    m.cpu.csr.mtvec = TRAP_VECTOR;
    let high = RAM_BASE + (5 << 30);
    set(&mut m, A1, high);
    set(&mut m, A2, 0x8877_6655_4433_2211);
    run(&mut m, "sd a2, 0(a1)", s_type(0, A2, A1, 3));
    assert_eq!(peek64(&mut m, high), 0x8877_6655_4433_2211);
    assert_eq!(peek64(&mut m, high - (4 << 30)), 0, "not 4 GiB lower");
    poke64(&mut m, high + 8, 0x0102_0304_0506_0708);
    run(&mut m, "ld a0, 8(a1)", i_type(8, A1, 3, A0, LOAD));
    assert_eq!(x(&m, A0), 0x0102_0304_0506_0708);
    // Just past the end of RAM: an access fault (load 5, store 7), the address in mtval.
    let end = RAM_BASE + (6 << 30);
    set(&mut m, A1, end);
    run_traps(&mut m, "ld a0, 0(a1)", i_type(0, A1, 3, A0, LOAD), 5, end);
    run_traps(&mut m, "sd a2, 0(a1)", s_type(0, A2, A1, 3), 7, end);
}
