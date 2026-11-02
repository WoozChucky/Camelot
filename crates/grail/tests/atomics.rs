//! The A extension in Grail (spec section 4.1): what riscv-tests rv32ua does not
//! cover. AMO results with every register aliasing, the LR/SC reservation (what
//! breaks it), misaligned and device addresses, and Sv32 permissions and A/D bits.
//!
//! Each test puts a few hand-encoded instructions at `RAM_BASE`, sets registers
//! and steps the hart one instruction at a time.

use grail::bus::{CLINT_BASE, RAM_BASE, UART_BASE};
use grail::cpu::Mode;
use grail::Machine;

const A0: u32 = 10;
const A1: u32 = 11;
const A2: u32 = 12;
const ZERO: u32 = 0;

/// Where the tests keep their data words.
const DATA: u32 = RAM_BASE + 0x8000;
/// Trap vector: the tests only look at the cause, nothing runs there.
const TRAP_VECTOR: u32 = RAM_BASE + 0x4000;

const OP_AMO: u32 = 0b0101111;

/// An A instruction: funct5, aq/rl clear, funct3 = 010 (word).
fn a_type(funct5: u32, rd: u32, rs1: u32, rs2: u32) -> u32 {
    funct5 << 27 | rs2 << 20 | rs1 << 15 | 2 << 12 | rd << 7 | OP_AMO
}
fn lr(rd: u32, rs1: u32) -> u32 {
    a_type(0b00010, rd, rs1, 0)
}
fn sc(rd: u32, rs2: u32, rs1: u32) -> u32 {
    a_type(0b00011, rd, rs1, rs2)
}
fn amoadd(rd: u32, rs2: u32, rs1: u32) -> u32 {
    a_type(0b00000, rd, rs1, rs2)
}
/// `sw rs2, 0(rs1)` / `sb rs2, 0(rs1)`.
fn sw(rs2: u32, rs1: u32) -> u32 {
    rs2 << 20 | rs1 << 15 | 2 << 12 | 0x23
}
fn sb(rs2: u32, rs1: u32) -> u32 {
    rs2 << 20 | rs1 << 15 | 0x23
}
/// `lw rd, 0(rs1)`.
fn lw(rd: u32, rs1: u32) -> u32 {
    rs1 << 15 | 2 << 12 | rd << 7 | 0x03
}
const ECALL: u32 = 0x0000_0073;

fn machine(code: &[u32]) -> Machine {
    let mut m = Machine::with_ram(1 << 20);
    let bytes: Vec<u8> = code.iter().flat_map(|w| w.to_le_bytes()).collect();
    m.cpu.bus.load(RAM_BASE, &bytes).unwrap();
    m.cpu.pc = RAM_BASE;
    m.cpu.csr.mtvec = TRAP_VECTOR;
    m
}

fn reg(m: &Machine, r: u32) -> u32 {
    m.cpu.x[r as usize]
}
fn set(m: &mut Machine, r: u32, v: u32) {
    m.cpu.x[r as usize] = v;
}
fn word(m: &mut Machine, addr: u32) -> u32 {
    m.cpu.bus.read(addr, 4).unwrap()
}
fn poke(m: &mut Machine, addr: u32, v: u32) {
    m.cpu.bus.write(addr, 4, v).unwrap();
}

/// Steps one instruction and checks it did not trap.
fn step(m: &mut Machine) {
    let pc = m.cpu.pc;
    m.cpu.step();
    assert_eq!(
        m.cpu.pc,
        pc + 4,
        "instruction at {pc:#x} trapped: mcause {}",
        m.cpu.csr.mcause
    );
}

/// Steps one instruction and checks it trapped with `cause` and `mtval`.
fn step_traps(m: &mut Machine, cause: u32, tval: u32) {
    let pc = m.cpu.pc;
    m.cpu.step();
    assert_eq!(m.cpu.pc, TRAP_VECTOR, "instruction at {pc:#x} did not trap");
    assert_eq!((m.cpu.csr.mcause, m.cpu.csr.mtval), (cause, tval));
    assert_eq!(m.cpu.csr.mepc, pc);
}

#[test]
fn misa_reports_a() {
    // csrr a0, misa: RV32 with A, I, M, S, U and X.
    let mut m = machine(&[0x3010_2573]);
    step(&mut m);
    assert_eq!(reg(&m, A0), 0x4094_1101);
    assert_ne!(reg(&m, A0) & 1, 0, "A is bit 0");
}

#[test]
fn every_amo_returns_the_old_word_and_writes_the_result() {
    let (neg, pos) = (0x8000_0000u32, 1u32);
    // (funct5, memory before, rs2, memory after)
    let cases = [
        (0b00001, 5, 7, 7),                               // amoswap
        (0b00000, u32::MAX, 2, 1),                        // amoadd wraps
        (0b00100, 0b1100, 0b1010, 0b0110),                // amoxor
        (0b01100, 0b1100, 0b1010, 0b1000),                // amoand
        (0b01000, 0b1100, 0b1010, 0b1110),                // amoor
        (0b10000, neg, pos, neg),                         // amomin: signed
        (0b10100, neg, pos, pos),                         // amomax
        (0b11000, neg, pos, pos),                         // amominu: unsigned
        (0b11100, neg, pos, neg),                         // amomaxu
        (0b10000, 0xffff_fffe, 0xffff_ffff, 0xffff_fffe), // amomin, both negative
    ];
    for (funct5, before, src, after) in cases {
        let mut m = machine(&[a_type(funct5, A2, A0, A1)]);
        poke(&mut m, DATA, before);
        set(&mut m, A0, DATA);
        set(&mut m, A1, src);
        step(&mut m);
        assert_eq!(reg(&m, A2), before, "funct5 {funct5:05b}: rd gets the old word");
        assert_eq!(word(&mut m, DATA), after, "funct5 {funct5:05b}: memory");
        assert_eq!(reg(&m, A1), src);
    }
}

#[test]
fn amo_reads_its_sources_before_writing_rd() {
    // amoadd a1, a1, (a0): rd is also rs2.
    let mut m = machine(&[amoadd(A1, A1, A0)]);
    poke(&mut m, DATA, 40);
    set(&mut m, A0, DATA);
    set(&mut m, A1, 2);
    step(&mut m);
    assert_eq!((reg(&m, A1), word(&mut m, DATA)), (40, 42));

    // amoadd a0, a1, (a0): rd is also rs1.
    let mut m = machine(&[amoadd(A0, A1, A0)]);
    poke(&mut m, DATA, 40);
    set(&mut m, A0, DATA);
    set(&mut m, A1, 2);
    step(&mut m);
    assert_eq!((reg(&m, A0), word(&mut m, DATA)), (40, 42));

    // amoadd zero, a1, (a0): memory is still written.
    let mut m = machine(&[amoadd(ZERO, A1, A0)]);
    set(&mut m, A0, DATA);
    set(&mut m, A1, 9);
    step(&mut m);
    assert_eq!((reg(&m, ZERO), word(&mut m, DATA)), (0, 9));
}

#[test]
fn lr_sc_pair_succeeds_once() {
    let mut m = machine(&[lr(A2, A0), sc(A2, A1, A0), sc(A2, A1, A0)]);
    poke(&mut m, DATA, 7);
    set(&mut m, A0, DATA);
    set(&mut m, A1, 99);
    step(&mut m);
    assert_eq!(reg(&m, A2), 7);
    assert_eq!(m.cpu.reservation.held(), Some(DATA));
    step(&mut m);
    assert_eq!((reg(&m, A2), word(&mut m, DATA)), (0, 99), "sc succeeds");
    assert_eq!(m.cpu.reservation.held(), None);
    poke(&mut m, DATA, 1);
    step(&mut m);
    assert_eq!((reg(&m, A2), word(&mut m, DATA)), (1, 1), "a second sc fails");
}

#[test]
fn sc_without_a_reservation_fails_and_writes_nothing() {
    let mut m = machine(&[sc(A2, A1, A0)]);
    poke(&mut m, DATA, 5);
    set(&mut m, A0, DATA);
    set(&mut m, A1, 99);
    step(&mut m);
    assert_eq!((reg(&m, A2), word(&mut m, DATA)), (1, 5));
}

#[test]
fn sc_to_another_word_fails_and_drops_the_reservation() {
    let mut m = machine(&[lr(A2, A0), sc(A2, A1, A2), sc(A2, A1, A0)]);
    set(&mut m, A0, DATA);
    poke(&mut m, DATA, DATA + 4); // lr loads the other word's address into a2
    poke(&mut m, DATA + 4, 5);
    set(&mut m, A1, 99);
    step(&mut m);
    step(&mut m);
    assert_eq!((reg(&m, A2), word(&mut m, DATA + 4)), (1, 5));
    step(&mut m);
    assert_eq!(
        (reg(&m, A2), word(&mut m, DATA)),
        (1, DATA + 4),
        "the failed sc dropped it"
    );
}

/// Runs `lr a2, (a0)`, then `between` (with a1 = `a1`), then `sc a2, zero, (a0)`,
/// and returns what the sc wrote to a2: 0 if the reservation survived `between`.
fn sc_after(between: &[u32], a1: u32) -> u32 {
    let mut code = vec![lr(A2, A0)];
    code.extend(between);
    code.push(sc(A2, ZERO, A0));
    let mut m = machine(&code);
    set(&mut m, A0, DATA);
    set(&mut m, A1, a1);
    for _ in 0..code.len() {
        step(&mut m);
    }
    reg(&m, A2)
}

#[test]
fn stores_to_the_reserved_word_break_the_reservation() {
    assert_eq!(sc_after(&[], 0), 0);
    assert_eq!(sc_after(&[lw(A2, A0)], 0), 0, "a load keeps it");
    assert_eq!(sc_after(&[sw(A2, A0)], 0), 1, "sw to the word");
    assert_eq!(sc_after(&[sb(A2, A1)], DATA + 3), 1, "sb to its last byte");
    assert_eq!(sc_after(&[amoadd(ZERO, A2, A0)], 0), 1, "an AMO");
    assert_eq!(
        sc_after(&[sw(A2, A1)], DATA + 4),
        0,
        "the next word is not reserved"
    );
    assert_eq!(sc_after(&[sw(A2, A1)], DATA - 4), 0, "nor the one before");
    // A store to a device register may start a DMA into RAM.
    assert_eq!(
        sc_after(&[sb(ZERO, A1)], UART_BASE + 7),
        1,
        "UART scratch register"
    );
}

#[test]
fn traps_break_the_reservation() {
    let mut m = machine(&[lr(A2, A0), ECALL]);
    set(&mut m, A0, DATA);
    step(&mut m);
    assert_eq!(m.cpu.reservation.held(), Some(DATA));
    step_traps(&mut m, 11, 0);
    assert_eq!(m.cpu.reservation.held(), None);

    // An interrupt too: the machine timer, with mtimecmp already passed.
    let mut m = machine(&[lr(A2, A0)]);
    set(&mut m, A0, DATA);
    step(&mut m);
    m.cpu.bus.clint.mtimecmp = 0;
    m.cpu.csr.mie = 1 << 7;
    m.cpu.csr.mstatus |= 1 << 3;
    m.cpu.step();
    assert_eq!(m.cpu.csr.mcause, 0x8000_0007);
    assert_eq!(m.cpu.reservation.held(), None);
}

#[test]
fn misaligned_addresses_trap() {
    for (inst, cause) in [
        (lr(A2, A0), 4),         // load address misaligned
        (sc(A2, A1, A0), 6),     // store/AMO address misaligned, even with no reservation
        (amoadd(A2, A1, A0), 6), // store/AMO address misaligned
    ] {
        for off in [1, 2, 3] {
            let mut m = machine(&[inst]);
            set(&mut m, A0, DATA + off);
            step_traps(&mut m, cause, DATA + off);
            assert_eq!(word(&mut m, DATA), 0);
        }
    }
}

#[test]
fn atomics_on_devices_and_unmapped_addresses_are_access_faults() {
    let mtimecmp = CLINT_BASE + 0x4000;
    for addr in [mtimecmp, 0x2000_0000] {
        let mut m = machine(&[lr(A2, A0)]);
        set(&mut m, A0, addr);
        step_traps(&mut m, 5, addr);

        let mut m = machine(&[amoadd(A2, A1, A0)]);
        set(&mut m, A0, addr);
        step_traps(&mut m, 7, addr);

        // sc only looks at the address when it holds a reservation.
        let mut m = machine(&[lr(A2, A1), sc(A2, A1, A0)]);
        set(&mut m, A0, addr);
        set(&mut m, A1, DATA);
        step(&mut m);
        step_traps(&mut m, 7, addr);
    }
}

#[test]
fn atomics_work_on_vram() {
    let vram = grail::bus::VRAM_BASE;
    let mut m = machine(&[amoadd(A2, A1, A0), lr(A2, A0), sc(A2, A1, A0)]);
    set(&mut m, A0, vram);
    set(&mut m, A1, 3);
    step(&mut m);
    step(&mut m);
    assert_eq!(reg(&m, A2), 3);
    step(&mut m);
    assert_eq!((reg(&m, A2), word(&mut m, vram)), (0, 3));
}

// --- Sv32: M-mode with MPRV set and MPP = S, so data accesses are translated
// (as for S-mode) while instructions are still fetched from physical RAM.

const ROOT: u32 = RAM_BASE + 0x1000;
const LEAF_TABLE: u32 = RAM_BASE + 0x2000;
/// Virtual pages, all in the 4 MiB region at 0x4000_0000 (one leaf table).
const VA: u32 = 0x4000_0000;
/// Physical page behind them.
const PAGE: u32 = RAM_BASE + 0x8000;

const PTE_V: u32 = 1;
const PTE_R: u32 = 2;
const PTE_W: u32 = 4;
const PTE_A: u32 = 1 << 6;
const PTE_D: u32 = 1 << 7;

/// Maps virtual page `VA + 4 KiB * i` to `PAGE` with `flags` (`PTE_V` added), and
/// returns the PTE's address.
fn map(m: &mut Machine, i: u32, flags: u32) -> u32 {
    poke(m, ROOT + (VA >> 22) * 4, (LEAF_TABLE >> 12) << 10 | PTE_V);
    let pte = LEAF_TABLE + i * 4;
    poke(m, pte, (PAGE >> 12) << 10 | flags | PTE_V);
    pte
}

fn paged_machine(code: &[u32]) -> Machine {
    let mut m = machine(code);
    m.cpu.csr.satp = 1 << 31 | ROOT >> 12;
    m.cpu.csr.mstatus |= 1 << 17 | (Mode::S as u32) << 11; // MPRV, MPP = S
    m
}

#[test]
fn amo_needs_a_writable_page_and_sets_a_and_d() {
    let mut m = paged_machine(&[amoadd(A2, A1, A0), amoadd(A2, A1, A0)]);
    let read_only = map(&mut m, 0, PTE_R);
    set(&mut m, A0, VA);
    set(&mut m, A1, 1);
    step_traps(&mut m, 15, VA); // store/AMO page fault
    assert_eq!(word(&mut m, read_only) & (PTE_A | PTE_D), 0);

    let mut m = paged_machine(&[amoadd(A2, A1, A0)]);
    let pte = map(&mut m, 1, PTE_R | PTE_W);
    poke(&mut m, PAGE, 41);
    set(&mut m, A0, VA + 0x1000);
    set(&mut m, A1, 1);
    step(&mut m);
    assert_eq!((reg(&m, A2), word(&mut m, PAGE)), (41, 42));
    assert_eq!(word(&mut m, pte) & (PTE_A | PTE_D), PTE_A | PTE_D);

    // Unmapped: a store/AMO page fault, not a load one.
    let mut m = paged_machine(&[amoadd(A2, A1, A0)]);
    map(&mut m, 0, PTE_R | PTE_W);
    set(&mut m, A0, VA + 0x5000);
    step_traps(&mut m, 15, VA + 0x5000);
}

#[test]
fn lr_faults_like_a_load_and_sc_like_a_store() {
    // lr on a read-only page works and sets only A.
    let mut m = paged_machine(&[lr(A2, A0), sc(A2, A1, A0)]);
    let pte = map(&mut m, 0, PTE_R);
    poke(&mut m, PAGE, 5);
    set(&mut m, A0, VA);
    step(&mut m);
    assert_eq!(reg(&m, A2), 5);
    assert_eq!(word(&mut m, pte) & (PTE_A | PTE_D), PTE_A);
    // sc there, holding a reservation, is a store page fault.
    step_traps(&mut m, 15, VA);
    assert_eq!(m.cpu.reservation.held(), None);

    // lr on an unmapped page is a load page fault.
    let mut m = paged_machine(&[lr(A2, A0)]);
    map(&mut m, 0, PTE_R);
    set(&mut m, A0, VA + 0x3000);
    step_traps(&mut m, 13, VA + 0x3000);

    // sc with no reservation fails without translating: no fault even unmapped.
    let mut m = paged_machine(&[sc(A2, A1, A0)]);
    map(&mut m, 0, PTE_R);
    set(&mut m, A0, VA + 0x3000);
    step(&mut m);
    assert_eq!(reg(&m, A2), 1);
}

#[test]
fn reservations_are_on_physical_addresses() {
    // Two virtual pages on one physical page: lr through one, sc through the other.
    let mut m = paged_machine(&[lr(A2, A0), sc(A2, A1, A2)]);
    map(&mut m, 0, PTE_R | PTE_W | PTE_A | PTE_D);
    map(&mut m, 1, PTE_R | PTE_W | PTE_A | PTE_D);
    poke(&mut m, PAGE + 8, VA + 0x1008); // lr reads the alias's address into a2
    set(&mut m, A0, VA + 8);
    set(&mut m, A1, 77);
    step(&mut m);
    assert_eq!(m.cpu.reservation.held(), Some(PAGE + 8));
    step(&mut m);
    assert_eq!((reg(&m, A2), word(&mut m, PAGE + 8)), (0, 77));
}

#[test]
fn hardware_a_d_updates_break_a_reservation_on_the_pte() {
    // Software holds a reservation on a PTE (lr with MPRV clear, so physical);
    // then an access through that PTE makes the walker set its A bit. The sc must
    // fail, or it would overwrite the A bit with a stale PTE.
    let mut m = paged_machine(&[lr(A2, A0), lw(A1, A1), sc(A2, A2, A0)]);
    let pte = map(&mut m, 0, PTE_R | PTE_W);
    m.cpu.csr.mstatus &= !(1 << 17);
    set(&mut m, A0, pte);
    set(&mut m, A1, VA);
    step(&mut m);
    m.cpu.csr.mstatus |= 1 << 17;
    step(&mut m);
    m.cpu.csr.mstatus &= !(1 << 17);
    step(&mut m);
    assert_eq!(reg(&m, A2), 1, "sc failed");
    assert_ne!(word(&mut m, pte) & PTE_A, 0, "and the A bit stayed");
}
