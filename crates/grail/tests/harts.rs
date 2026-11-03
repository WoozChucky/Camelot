//! Machines of several harts (spec section 7, "Harts" and "Boot"): reset state and
//! `mhartid`, the boot ROM's park loop and IPIs, the CLINT's and PLIC's per-hart
//! registers, the order harts take turns in, `wfi` on some harts and on all of them,
//! LR/SC reservations broken by other harts and by DMA, and pacing.
//!
//! Each test puts a few hand-encoded instructions in RAM for each hart (hart `i`'s at
//! `code(i)`) and steps the machine. `examples/smp` (run by the xtask tests on 1 to 8
//! harts) is the end-to-end program.

use excalibur::csr::{IRQ_M_EXT, IRQ_M_SOFT, IRQ_M_TIMER, IRQ_S_EXT, MHARTID};
use grail::block::{self, CMD_FLUSH};
use grail::bus::{
    BLK_BASE, BOOT_ROM_BASE, CLINT_BASE, PARK_PC, PLIC_BASE, RAM_BASE, SYSINFO_BASE, UART_BASE,
};
use grail::cpu::Mode;
use grail::devices::{PLIC_CONTEXT, PLIC_CONTEXT_STRIDE, PLIC_ENABLE, PLIC_ENABLE_STRIDE};
use grail::{DiskConfig, Machine, MachineConfig, RunResult, MAX_HARTS};

const T0: u32 = 5;
const T1: u32 = 6;
const A0: u32 = 10;
const A1: u32 = 11;
const A2: u32 = 12;
const A3: u32 = 13;
const A4: u32 = 14;
const A5: u32 = 15;
const A6: u32 = 16;
const A7: u32 = 17;
const ZERO: u32 = 0;

/// The word the reservation tests fight over.
const DATA: u32 = RAM_BASE + 0x8000;

fn addi(rd: u32, rs1: u32, imm: i32) -> u32 {
    ((imm as u32) & 0xfff) << 20 | rs1 << 15 | rd << 7 | 0x13
}
const NOP: u32 = 0x0000_0013;
/// `j .`: a hart that has finished stays here.
const STOP: u32 = 0x0000_006f;
/// `j .-4`.
const BACK: u32 = 0xffdf_f06f;
const WFI: u32 = 0x1050_0073;
/// `xcal.halt zero`.
const HALT: u32 = 0x0000_100b;
fn csrr(rd: u32, csr: u16) -> u32 {
    (csr as u32) << 20 | 2 << 12 | rd << 7 | 0x73
}
/// `sw rs2, imm(rs1)` / `sb rs2, imm(rs1)`.
fn store(funct3: u32, rs2: u32, rs1: u32, imm: u32) -> u32 {
    (imm >> 5) << 25 | rs2 << 20 | rs1 << 15 | funct3 << 12 | (imm & 31) << 7 | 0x23
}
fn sw(rs2: u32, rs1: u32, imm: u32) -> u32 {
    store(2, rs2, rs1, imm)
}
fn sb(rs2: u32, rs1: u32, imm: u32) -> u32 {
    store(0, rs2, rs1, imm)
}
/// `lw rd, 0(rs1)`.
fn lw(rd: u32, rs1: u32) -> u32 {
    rs1 << 15 | 2 << 12 | rd << 7 | 0x03
}
fn a_type(funct5: u32, rd: u32, rs1: u32, rs2: u32) -> u32 {
    funct5 << 27 | rs2 << 20 | rs1 << 15 | 2 << 12 | rd << 7 | 0b0101111
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

/// Where hart `i`'s code goes.
fn code(hart: usize) -> u32 {
    RAM_BASE + 0x100 * hart as u32
}

fn config(harts: usize) -> MachineConfig {
    MachineConfig {
        ram_size: 1 << 20,
        harts,
        ..MachineConfig::default()
    }
}

fn machine(harts: usize) -> Machine {
    Machine::new(config(harts)).unwrap()
}

/// Puts `program` at `code(hart)` and starts the hart there (out of the park loop).
fn start(m: &mut Machine, hart: usize, program: &[u32]) {
    let bytes: Vec<u8> = program.iter().flat_map(|w| w.to_le_bytes()).collect();
    m.cpu.bus.load(code(hart), &bytes).unwrap();
    let h = m.cpu.hart_mut(hart);
    h.pc = code(hart).into();
    h.waiting = false;
}

fn steps(m: &mut Machine, n: usize) {
    for _ in 0..n {
        m.cpu.step();
    }
}

#[test]
fn harts_are_validated_and_counted() {
    for bad in [0, MAX_HARTS + 1] {
        let e = Machine::new(config(bad)).err().unwrap();
        assert_eq!(e, format!("{bad} harts, must be 1 to 8"));
    }
    for n in 1..=MAX_HARTS {
        let mut m = machine(n);
        assert_eq!(m.cpu.harts().len(), n);
        assert_eq!(m.cpu.bus.harts(), n);
        assert_eq!(m.config().harts, n);
        assert_eq!(m.cpu.bus.read(SYSINFO_BASE + 0x20, 4), Ok(n as u32), "SYS_HARTS");
        assert_eq!(m.cpu.bus.clint.msip.len(), n);
        assert_eq!(m.cpu.bus.plic.contexts(), 2 * n);
    }
}

#[test]
fn reset_state_and_mhartid() {
    let mut m = machine(4);
    for (i, h) in m.cpu.harts().iter().enumerate() {
        assert_eq!(h.id, i as u32);
        assert_eq!(h.mode, Mode::M);
        assert_eq!(h.pc, u64::from(if i == 0 { BOOT_ROM_BASE } else { PARK_PC }));
        assert!(!h.waiting);
    }
    for i in 0..4 {
        start(&mut m, i, &[csrr(A0, MHARTID), STOP]);
    }
    m.cpu.step();
    let ids: Vec<u64> = m.cpu.harts().iter().map(|h| h.x[A0 as usize]).collect();
    assert_eq!(ids, [0, 1, 2, 3]);
    // Between steps the Cpu reads as hart 0.
    assert_eq!(m.cpu.id, 0);
    assert_eq!(m.cpu.pc, u64::from(code(0) + 4));
}

#[test]
fn a_one_hart_rom_is_the_old_stub() {
    let mut m = machine(1);
    assert!(m.cpu.bus.rom.is_empty(), "nothing before a program is loaded");
    m.cpu.bus.set_boot_entry(0x8000_0000);
    let words: Vec<u32> = m
        .cpu
        .bus
        .rom
        .chunks(4)
        .map(|w| u32::from_le_bytes(w.try_into().unwrap()))
        .collect();
    assert_eq!(
        words,
        [0x0000_0297, 0x0102_a283, 0x0000_0513, 0x0002_8067, 0x8000_0000]
    );
}

/// The park loop as Grail's disassembler shows it (spec section 7, "Boot").
#[test]
fn the_park_loop() {
    let mut m = machine(2);
    m.cpu.bus.set_boot_entry(0x8000_0000);
    assert_eq!(m.cpu.bus.peek(BOOT_ROM_BASE + 16), Some(0x8000_0000));
    let text: Vec<String> = (PARK_PC..)
        .step_by(4)
        .map_while(|addr| m.cpu.bus.peek(addr))
        .map(|w| excalibur::decode(w, excalibur::Xlen::Rv32).to_string())
        .collect();
    assert_eq!(
        text,
        [
            "csrrs a0, mhartid, zero",
            "slli t1, a0, 2",
            "lui t0, 0x2000",
            "add t1, t0, t1",
            "addi t2, zero, 8",
            "csrrw zero, mie, t2",
            "wfi",
            "lw t2, 0(t1)",
            "beq t2, zero, -8",
            "sw zero, 0(t1)",
            "csrrw zero, mie, zero",
            "lui t0, 0x1",
            "lw t0, 16(t0)",
            "jalr zero, 0(t0)",
        ]
    );
}

#[test]
fn the_boot_rom_parks_the_other_harts_until_an_ipi() {
    let mut m = machine(3);
    m.cpu.bus.set_boot_entry(code(0));
    // Every hart that starts records its a0 (its hart id) in a1, plus 0x10.
    m.cpu
        .bus
        .load(code(0), &addi(A1, A0, 0x10).to_le_bytes())
        .unwrap();
    m.cpu.bus.load(code(0) + 4, &STOP.to_le_bytes()).unwrap();
    steps(&mut m, 50);
    let a1 = |m: &Machine| -> Vec<u64> { m.cpu.harts().iter().map(|h| h.x[A1 as usize]).collect() };
    assert_eq!(a1(&m), [0x10, 0, 0]);
    assert_eq!(m.cpu.hart(0).pc, u64::from(code(0) + 4));
    for h in &m.cpu.harts()[1..] {
        assert!(h.waiting, "hart {} waits for its IPI", h.id);
        assert!((PARK_PC..PARK_PC + 0x40).contains(&(h.pc as u32)));
        assert_eq!(h.csr.mie, 1 << IRQ_M_SOFT);
        assert_eq!(h.mip(&m.cpu.bus), 0);
    }

    // An IPI to hart 2: its msip, as another hart's `sw` would set it.
    m.cpu.bus.write(CLINT_BASE + 8, 4, 1).unwrap();
    steps(&mut m, 50);
    assert_eq!(a1(&m), [0x10, 0, 0x12]);
    let h2 = m.cpu.hart(2);
    assert_eq!(h2.pc, u64::from(code(0) + 4));
    assert_eq!(h2.mode, Mode::M);
    assert_eq!(h2.csr.mie, 0, "the ROM leaves mie as it found it");
    assert!(!m.cpu.bus.clint.msip[2], "and acknowledged the IPI");
    assert_eq!(h2.csr.mcause, 0, "no trap was taken");
    assert!(m.cpu.hart(1).waiting, "hart 1 still waits");

    // A write of 0 or of another hart's msip does not wake hart 1.
    m.cpu.bus.write(CLINT_BASE + 4, 4, 0).unwrap();
    steps(&mut m, 50);
    assert!(m.cpu.hart(1).waiting);
    m.cpu.bus.write(CLINT_BASE + 4, 4, 1).unwrap();
    steps(&mut m, 50);
    assert_eq!(a1(&m), [0x10, 0x11, 0x12]);
}

#[test]
fn clint_registers_per_hart() {
    let mut m = machine(2);
    let bus = &mut m.cpu.bus;
    let r = |bus: &mut grail::bus::Bus, off: u32| bus.read(CLINT_BASE + off, 4).unwrap();
    bus.write(CLINT_BASE + 4, 4, 1).unwrap(); // msip[1]
    assert_eq!([r(bus, 0), r(bus, 4)], [0, 1]);
    assert_eq!(bus.clint.msip, [false, true]);
    // There is no hart 2: its msip and mtimecmp are reserved.
    bus.write(CLINT_BASE + 8, 4, 1).unwrap();
    assert_eq!(r(bus, 8), 0);
    bus.write(CLINT_BASE + 0x4010, 4, 1).unwrap();
    assert_eq!(r(bus, 0x4010), 0);

    bus.write(CLINT_BASE + 0x4008, 4, 0x1234).unwrap(); // mtimecmp[1], low then high
    bus.write(CLINT_BASE + 0x400c, 4, 5).unwrap();
    assert_eq!(bus.clint.mtimecmp, [u64::MAX, 0x5_0000_1234]);
    assert_eq!([r(bus, 0x4008), r(bus, 0x400c)], [0x1234, 5]);
    assert_eq!([r(bus, 0x4000), r(bus, 0x4004)], [u32::MAX, u32::MAX]);

    let mip = |m: &Machine, hart: usize| m.cpu.hart(hart).mip(&m.cpu.bus);
    assert_eq!(mip(&m, 0), 0);
    assert_eq!(mip(&m, 1), 1 << IRQ_M_SOFT);
    // mtime is shared: at hart 1's mtimecmp only hart 1's timer fires.
    m.cpu.bus.write(CLINT_BASE + 0xbffc, 4, 5).unwrap();
    m.cpu.bus.write(CLINT_BASE + 0xbff8, 4, 0x1234).unwrap();
    assert_eq!(m.cpu.bus.clint.mtime, 0x5_0000_1234);
    assert_eq!(mip(&m, 0), 0);
    assert_eq!(mip(&m, 1), 1 << IRQ_M_SOFT | 1 << IRQ_M_TIMER);
    m.cpu.bus.write(CLINT_BASE + 0x4004, 4, 0).unwrap();
    m.cpu.bus.write(CLINT_BASE + 0x4000, 4, 0).unwrap();
    assert_eq!(mip(&m, 0), 1 << IRQ_M_TIMER);
}

#[test]
fn plic_contexts_per_hart() {
    let mut m = machine(2);
    let enable = |ctx: u32| PLIC_BASE + PLIC_ENABLE + PLIC_ENABLE_STRIDE * ctx;
    let claim = |ctx: u32| PLIC_BASE + PLIC_CONTEXT + PLIC_CONTEXT_STRIDE * ctx + 4;
    let mip = |m: &Machine, hart: usize| m.cpu.hart(hart).mip(&m.cpu.bus);
    let bus = &mut m.cpu.bus;
    bus.write(PLIC_BASE + 4, 4, 1).unwrap(); // source 1, priority 1
    bus.write(enable(2), 4, 0b10).unwrap(); // context 2: hart 1, M-mode
    bus.plic.set_level(1, true);
    assert_eq!([mip(&m, 0), mip(&m, 1)], [0, 1 << IRQ_M_EXT]);
    let bus = &mut m.cpu.bus;
    bus.write(enable(3), 4, 0b10).unwrap(); // context 3: hart 1, S-mode
    bus.write(enable(0), 4, 0b10).unwrap(); // context 0: hart 0, M-mode
    assert_eq!(
        [mip(&m, 0), mip(&m, 1)],
        [1 << IRQ_M_EXT, 1 << IRQ_M_EXT | 1 << IRQ_S_EXT]
    );
    // The first context to claim it gets it; the others find nothing.
    let bus = &mut m.cpu.bus;
    assert_eq!(bus.read(claim(2), 4), Ok(1));
    assert_eq!(bus.read(claim(0), 4), Ok(0));
    assert_eq!([mip(&m, 0), mip(&m, 1)], [0, 0]);
    // No context 4 on a two-hart machine.
    let bus = &mut m.cpu.bus;
    bus.write(enable(4), 4, 0b10).unwrap();
    assert_eq!(bus.read(enable(4), 4), Ok(0));
    assert_eq!(bus.read(claim(4), 4), Ok(0));
}

#[test]
fn harts_take_turns_one_instruction_each() {
    let mut m = machine(3);
    for i in 0..3 {
        start(&mut m, i, &[addi(A0, A0, 1), BACK]);
    }
    let mut order = Vec::new();
    for _ in 0..4 {
        m.cpu.step_traced(&mut |cpu| order.push((cpu.id, cpu.pc as u32)));
    }
    let want: Vec<(u32, u32)> = (0..4)
        .flat_map(|step| (0..3).map(move |i| (i as u32, code(i) + 4 * (step % 2))))
        .collect();
    assert_eq!(order, want, "hart 0, 1, 2 in every step");
    assert_eq!(m.cpu.bus.clint.mtime, 4, "one tick per step");
    for h in m.cpu.harts() {
        assert_eq!((h.csr.minstret, h.csr.mcycle, h.x[A0 as usize]), (4, 4, 2));
    }
    // run_for counts steps too.
    assert_eq!(m.run_for(10, &mut |_| {}), 10);
    assert_eq!(m.cpu.bus.clint.mtime, 14);
    assert!(m.cpu.harts().iter().all(|h| h.csr.minstret == 14));
}

#[test]
fn wfi_on_one_hart_does_not_stall_the_others() {
    let mut m = machine(2);
    start(&mut m, 0, &[addi(A0, A0, 1), BACK]);
    start(&mut m, 1, &[WFI, STOP]);
    m.cpu.hart_mut(1).csr.mie = 1 << IRQ_M_TIMER;
    m.cpu.bus.clint.mtimecmp[1] = 1000;
    steps(&mut m, 100);
    assert!(m.cpu.hart(1).waiting);
    assert_eq!(
        m.cpu.bus.clint.mtime, 100,
        "no jump to the timer while hart 0 runs"
    );
    assert_eq!(m.cpu.hart(0).csr.minstret, 100);
    assert_eq!(m.cpu.hart(1).csr.minstret, 1, "only the wfi");
    steps(&mut m, 900);
    assert!(!m.cpu.hart(1).waiting, "the timer woke it");
    assert_eq!(m.cpu.hart(1).pc, u64::from(code(1) + 4));
}

#[test]
fn when_every_hart_waits_mtime_jumps_to_the_earliest_timer() {
    let mut m = machine(3);
    for (i, cmp) in [(0, 5000), (1, 3000), (2, u64::MAX)] {
        start(&mut m, i, &[WFI, STOP]);
        m.cpu.hart_mut(i).csr.mie = 1 << IRQ_M_TIMER;
        m.cpu.bus.clint.mtimecmp[i] = cmp;
    }
    m.cpu.step();
    assert!(m.cpu.all_idle());
    assert_eq!(m.cpu.bus.clint.mtime, 1);
    m.cpu.step();
    assert_eq!(m.cpu.bus.clint.mtime, 3000, "hart 1's timer, the earliest");
    m.cpu.step();
    assert!(!m.cpu.hart(1).waiting);
    assert!(m.cpu.hart(0).waiting && m.cpu.hart(2).waiting);
    assert!(!m.cpu.all_idle());
    steps(&mut m, 10);
    assert_eq!(m.cpu.bus.clint.mtime, 3011, "ticking again: hart 1 runs");
}

/// At full speed the jump goes to the earliest armed `mtimecmp` whatever the hart's
/// `mie.MTIE`, as on a one-hart machine: a masked timer only ends the jump early, it
/// wakes nobody, and the next step jumps on.
#[test]
fn a_masked_timer_ends_the_full_speed_jump_early() {
    let mut m = machine(2);
    for (i, mie, cmp) in [(0, 1 << IRQ_M_TIMER, 5000), (1, 0, 3000)] {
        start(&mut m, i, &[WFI, STOP]);
        m.cpu.hart_mut(i).csr.mie = mie;
        m.cpu.bus.clint.mtimecmp[i] = cmp;
    }
    steps(&mut m, 2);
    assert_eq!(m.cpu.bus.clint.mtime, 3000, "hart 1's masked timer");
    assert!(m.cpu.all_idle(), "which wakes nobody");
    m.cpu.step();
    assert_eq!(m.cpu.bus.clint.mtime, 5000, "hart 0's timer");
    m.cpu.step();
    assert!(!m.cpu.hart(0).waiting);
    assert!(m.cpu.hart(1).waiting);
}

#[test]
fn idle_skips_only_while_every_hart_waits() {
    let mut m = machine(2);
    m.cpu.wfi_skips_to_timer = false;
    start(&mut m, 0, &[WFI, STOP]);
    start(&mut m, 1, &[WFI, STOP]);
    for (i, cmp) in [(0, 5000), (1, 3000)] {
        m.cpu.hart_mut(i).csr.mie = 1 << IRQ_M_TIMER;
        m.cpu.bus.clint.mtimecmp[i] = cmp;
    }
    assert_eq!(m.cpu.idle(100), 0, "not waiting yet");
    m.cpu.step();
    assert_eq!(m.cpu.idle(100), 100);
    assert_eq!(m.cpu.bus.clint.mtime, 101);
    assert_eq!(m.cpu.idle(10_000), 2898, "one tick short of hart 1's timer");
    m.cpu.step();
    assert_eq!(m.cpu.bus.clint.mtime, 3000);
    assert!(!m.cpu.hart(1).waiting);
    assert_eq!(m.cpu.idle(100), 0, "hart 1 is running");

    // A masked timer does not limit the skip.
    let mut m = machine(2);
    m.cpu.wfi_skips_to_timer = false;
    start(&mut m, 0, &[WFI, STOP]);
    start(&mut m, 1, &[WFI, STOP]);
    m.cpu.bus.clint.mtimecmp[1] = 10;
    m.cpu.step();
    assert_eq!(m.cpu.idle(10_000), 10_000);
}

/// Hart 0 runs `lr.w a2, (a0)`, `nop`, `sc.w a3, a4, (a0)` on `DATA` while hart 1 runs
/// `other` (its first instruction in the same step as the `lr.w`), with a0 = `DATA`,
/// a5 = 0x55, a6 = `UART_BASE` and a1 = `DATA + 8`. Returns what the `sc.w` wrote to
/// a3 (0 if the reservation survived) and the machine.
fn race(other: &[u32]) -> (u64, Machine) {
    let mut m = machine(2);
    start(&mut m, 0, &[lr(A2, A0), NOP, sc(A3, A4, A0), STOP]);
    start(&mut m, 1, other);
    for h in 0..2 {
        let hart = m.cpu.hart_mut(h);
        hart.set_reg(A0 as u8, DATA.into());
        hart.set_reg(A1 as u8, (DATA + 8).into());
        hart.x[A4 as usize] = 0x44;
        hart.x[A5 as usize] = 0x55;
        hart.x[A6 as usize] = UART_BASE.into();
    }
    m.cpu.bus.uart.capture = true;
    steps(&mut m, 3);
    assert_eq!(m.cpu.hart(0).pc, u64::from(code(0) + 12), "no trap");
    (m.cpu.hart(0).x[A3 as usize], m)
}

#[test]
fn stores_from_another_hart_break_a_reservation() {
    for (what, other, sc) in [
        ("sw to the word", vec![sw(A5, A0, 0), STOP], 1),
        ("sb to its last byte", vec![sb(A5, A0, 3), STOP], 1),
        ("amoadd on it", vec![amoadd(ZERO, A5, A0), STOP], 1),
        // The reservation set is the 64-byte line holding the word (DATA starts one).
        ("sw to the next word, in the line", vec![sw(A5, A0, 4), STOP], 1),
        ("sw to the next line", vec![sw(A5, A0, 64), STOP], 0),
        ("lw of the word", vec![lw(A7, A0), STOP], 0),
        ("nothing", vec![NOP, STOP], 0),
    ] {
        let (got, m) = race(&other);
        assert_eq!(got, sc, "{what}");
        assert_eq!(m.cpu.hart(1).csr.mcause, 0, "{what}: hart 1 took no trap");
    }
    // Both harts in an LR/SC sequence on the word: the first sc.w wins, and its store
    // breaks the other hart's reservation.
    let (got, mut m) = race(&[lr(A7, A0), sc(T1, A5, A0), STOP]);
    assert_eq!(got, 1, "hart 0's sc.w fails");
    assert_eq!(m.cpu.hart(1).x[T1 as usize], 0, "hart 1's succeeded");
    assert_eq!(m.cpu.bus.read(DATA, 4), Ok(0x55));
}

#[test]
fn a_device_store_breaks_only_the_storing_harts_reservation() {
    // Hart 1 reserves DATA + 8, writes to the UART, then tries its sc.w.
    let (got, m) = race(&[lr(T0, A1), sb(A5, A6, 0), sc(T1, A5, A1), STOP]);
    assert_eq!(got, 0, "hart 0's reservation survives");
    assert_eq!(m.cpu.hart(1).x[T1 as usize], 1, "hart 1's does not");
    assert_eq!(m.cpu.bus.uart.captured, [0x55]);
}

/// A trap breaks the reservation of the hart that takes it (spec section 4.1), and only
/// that one: here hart 1 takes an IPI between its `lr.w` and `sc.w` while hart 0 is in
/// an LR/SC sequence of its own.
#[test]
fn an_interrupt_breaks_only_its_own_harts_reservation() {
    let mut m = machine(2);
    // Hart 0: lr.w on DATA, a few nops, sc.w. Hart 1: lr.w on DATA + 8 and a loop; its
    // trap handler (at code(1) + 0x40) tries the sc.w.
    start(&mut m, 0, &[lr(A2, A0), NOP, NOP, NOP, sc(A3, A4, A0), STOP]);
    start(&mut m, 1, &[lr(T0, A1), STOP]);
    let handler: Vec<u8> = [sc(T1, A5, A1), STOP]
        .iter()
        .flat_map(|w| w.to_le_bytes())
        .collect();
    m.cpu.bus.load(code(1) + 0x40, &handler).unwrap();
    for h in 0..2 {
        let hart = m.cpu.hart_mut(h);
        hart.set_reg(A0 as u8, DATA.into());
        hart.set_reg(A1 as u8, (DATA + 8).into());
        hart.x[A4 as usize] = 0x44;
        hart.x[A5 as usize] = 0x55;
    }
    let h1 = m.cpu.hart_mut(1);
    h1.csr.mtvec = (code(1) + 0x40).into();
    h1.csr.mie = 1 << IRQ_M_SOFT;
    h1.csr.mstatus |= excalibur::csr::MSTATUS_MIE;
    m.cpu.step(); // both lr.w
                  // An IPI to hart 1 (msip[1]), as a third party would send it: no hart's store.
    m.cpu.bus.write(CLINT_BASE + 4, 4, 1).unwrap();
    steps(&mut m, 5);
    let h1 = m.cpu.hart(1);
    assert_eq!(
        h1.csr.mcause,
        1 << 31 | u64::from(IRQ_M_SOFT),
        "hart 1 took the IPI"
    );
    assert_eq!(h1.pc, u64::from(code(1) + 0x44), "and ran its handler's sc.w");
    assert_eq!(h1.x[T1 as usize], 1, "hart 1's sc.w fails");
    assert_eq!(m.cpu.bus.read(DATA + 8, 4), Ok(0));
    assert_eq!(m.cpu.hart(0).pc, u64::from(code(0) + 20));
    assert_eq!(m.cpu.hart(0).x[A3 as usize], 0, "hart 0's sc.w succeeds");
    assert_eq!(m.cpu.bus.read(DATA, 4), Ok(0x44));
}

#[test]
fn disk_commands_break_every_reservation() {
    for (reg, val, want) in [(block::REG_CMD, CMD_FLUSH, 1), (block::REG_SECTOR, 0, 0)] {
        let mut m = Machine::new(MachineConfig {
            disks: vec![DiskConfig::memory(vec![0; 512])],
            ..config(2)
        })
        .unwrap();
        start(&mut m, 0, &[lr(A2, A0), NOP, sc(A3, A4, A0), STOP]);
        start(&mut m, 1, &[sw(A5, A6, 0), STOP]);
        m.cpu.hart_mut(0).set_reg(A0 as u8, DATA.into());
        m.cpu.hart_mut(1).x[A5 as usize] = val.into();
        m.cpu.hart_mut(1).x[A6 as usize] = (BLK_BASE + reg).into();
        steps(&mut m, 3);
        assert_eq!(m.cpu.hart(0).x[A3 as usize], want, "register {reg:#x}");
    }
}

#[test]
fn a_halt_ends_the_step() {
    let mut m = machine(2);
    start(&mut m, 0, &[NOP, HALT]);
    start(&mut m, 1, &[addi(A0, A0, 1), BACK]);
    assert_eq!(m.run(Some(100), |_| {}), RunResult::Exited(0));
    assert_eq!(m.cpu.bus.clint.mtime, 2);
    assert_eq!(
        m.cpu.hart(1).csr.minstret,
        1,
        "hart 1 ran in the first step only: hart 0 stopped the machine in the second"
    );
}

/// With `--clock`, the clock is each hart's: a step runs every hart, and the pacer
/// counts steps. Two harts at 1 MHz run 2 million instructions a second in all.
#[test]
fn paced_harts_each_run_at_the_clock() {
    let mut m = Machine::new(MachineConfig {
        clock_hz: Some(1_000_000),
        ..config(2)
    })
    .unwrap();
    assert!(!m.cpu.wfi_skips_to_timer);
    start(&mut m, 0, &[addi(A0, A0, 1), BACK]);
    start(&mut m, 1, &[addi(A0, A0, 1), BACK]);
    // The first batch: 1 ms of guest time, 1000 steps.
    assert_eq!(m.run_for(u64::MAX, &mut |_| {}), 1000);
    assert_eq!(m.cpu.bus.clint.mtime, 1000);
    for h in m.cpu.harts() {
        assert_eq!(h.csr.minstret, 1000);
    }
    assert!(m.pace_delay().is_some(), "ahead of the wall clock");
}

/// A paced machine whose harts all wait skips their idle ticks at once, and wakes on
/// time for the earliest timer.
#[test]
fn paced_harts_idle_until_the_earliest_timer() {
    let mut m = Machine::new(MachineConfig {
        clock_hz: Some(1_000_000),
        ..config(2)
    })
    .unwrap();
    start(&mut m, 0, &[WFI, STOP]);
    start(&mut m, 1, &[WFI, STOP]);
    for (i, cmp) in [(0, 10_000_000), (1, 1500)] {
        m.cpu.hart_mut(i).csr.mie = 1 << IRQ_M_TIMER;
        m.cpu.bus.clint.mtimecmp[i] = cmp;
    }
    // The first batch: both wfi, then the rest of the 1000 ticks skipped at once.
    assert_eq!(m.run_for(u64::MAX, &mut |_| {}), 1000);
    assert!(m.cpu.all_idle());
    assert_eq!(m.cpu.hart(0).csr.minstret, 1);
    for _ in 0..100 {
        if let Some(d) = m.pace_delay() {
            std::thread::sleep(d);
        }
        m.run_for(u64::MAX, &mut |_| {});
        if !m.cpu.hart(1).waiting {
            break;
        }
    }
    assert!(!m.cpu.hart(1).waiting);
    assert_eq!(m.cpu.hart(1).pc, u64::from(code(1) + 4));
    assert!(m.cpu.hart(0).waiting, "hart 0's timer is later");
    assert!(m.cpu.bus.clint.mtime >= 1500);
}
