//! Kernel tests. The committed `kernel/kernel.elf` always runs on Grail, so
//! these pass without LLVM. When clang and ld.lld are installed, the kernel is
//! also rebuilt from source and checked against the committed image.

use excalibur::csr::{
    IRQ_M_TIMER, IRQ_S_EXT, IRQ_S_SOFT, IRQ_S_TIMER, MSTATUS_SIE, MSTATUS_SPIE, MSTATUS_SPP,
};
use excalibur::reg;
use grail::cpu::Mode;
use grail::trap::INTERRUPT_BIT;
use grail::{Machine, RunResult};
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use xtask::kernel::{self, Assembler, Build, Lang, BOOT_MAX_STEPS};
use xtask::{repo_root, tools};

/// What the kernel prints, with `xcal.dbg`'s pc masked (see `mask_dbg_pc`).
const BOOT_OUTPUT: &str = "\
Camelot v0.1.0 booting...
kprintf: d=-42 u=4294967295 x=cafef00d p=0x80000000 s=excalibur c=K %=%
kprintf self-test: 26/26 passed
[dbg] pc=........ a0 = 0x00000000 (0)
ram: 128 MiB at 0x80000000
paging: on (Sv32)
vm self-test: 24/24 passed
timer: 10 Hz tick, 10000000 Hz timebase
tick 1
tick 2
tick 3
tick 4
tick 5
tick 6
tick 7
tick 8
tick 9
tick 10
";

/// mtime ticks between timer interrupts: the nominal 10 MHz timebase / 10 Hz.
const TICK_INTERVAL: u64 = 1_000_000;

// Kernel constants, from the kernel sources named in each comment.
// crates/grail/tests/kernel.rs has copies of some: keep them in sync.

/// Exit code of a kernel panic (`PANIC_EXIT_CODE` in kernel/include/trap.h).
const PANIC_EXIT: u32 = 0xfe;

/// Exit code after an unexpected M-mode trap (`EXIT_UNEXPECTED_TRAP` in
/// kernel/entry.s).
const UNEXPECTED_TRAP_EXIT: u32 = 0xff;

/// Byte offset of the timer interval in `machine_scratch`
/// (`MSCRATCH_INTERVAL` words, kernel/include/machine.h).
const SCRATCH_INTERVAL: u32 = 12;

/// Values of the kernel's `boot_fault` (kernel/main.c).
const BOOT_FAULT_NONE: u32 = 1;
const BOOT_FAULT_ILLEGAL: u32 = 2;
const BOOT_FAULT_LOAD: u32 = 3;
const BOOT_FAULT_STORE_TEXT: u32 = 4;
/// Where `BOOT_FAULT_LOAD` loads from: below the boot ROM, nothing is mapped.
const UNMAPPED_ADDR: u32 = 0x100;

/// Where the vm self-test maps its alias page (`ALIAS_VA` in kernel/vmtest.c).
/// The test unmaps it again but leaves the second-level table in place.
const ALIAS_VA: u32 = 0x4000_0000;

/// mcause of a machine call.
const ECALL_FROM_S: u32 = grail::trap::Exception::EcallS as u32;

fn kernel_dir() -> PathBuf {
    repo_root().join("kernel")
}

fn committed_elf() -> Vec<u8> {
    let path = kernel_dir().join("kernel.elf");
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn assert_boots(elf: &[u8]) {
    let (r, out) = kernel::run_on_grail(elf, BOOT_MAX_STEPS).unwrap();
    assert_eq!(kernel::mask_dbg_pc(&out), BOOT_OUTPUT, "raw output:\n{out}");
    assert_eq!(r, RunResult::Exited(0));
}

/// Loads the committed kernel without running it.
fn load_committed() -> (Machine, grail::elf::Elf) {
    let file = committed_elf();
    let elf = grail::elf::parse(&file).unwrap();
    let mut m = Machine::with_ram(grail::DEFAULT_RAM_SIZE);
    m.cpu.bus.uart.capture = true;
    m.load_elf(&file).unwrap();
    (m, elf)
}

fn sym(elf: &grail::elf::Elf, name: &str) -> u32 {
    elf.symbol(name).unwrap_or_else(|| panic!("no symbol {name}"))
}

/// Steps the machine until it is about to execute (or take a trap at) `pc`.
fn step_to(m: &mut Machine, pc: u32) {
    for _ in 0..100_000 {
        if m.cpu.pc == pc {
            return;
        }
        m.cpu.step();
    }
    panic!("never reached {pc:#x}");
}

/// Register number of an ABI name.
fn x(name: &str) -> u32 {
    u32::from(reg::parse(name).unwrap_or_else(|| panic!("no register {name}")))
}

/// What `xcal.dbg` printed, without the `[dbg] pc=... a0 = ` prefixes.
fn dbg_values(out: &str) -> Vec<&str> {
    out.lines().map(|l| l.split(" = ").nth(1).unwrap_or(l)).collect()
}

/// The `[dbg]` values an unexpected M-mode trap prints: mcause, mepc, mtval.
fn m_trap_report(mcause: u32, mepc: u32, mtval: u32) -> Vec<String> {
    [mcause, mepc, mtval]
        .iter()
        .map(|v| format!("{v:#010x} ({})", *v as i32))
        .collect()
}

#[test]
fn committed_kernel_boots() {
    assert_boots(&committed_elf());
}

/// A `Write` the test can read back after giving it to a machine.
#[derive(Clone, Default)]
struct Shared(Arc<Mutex<Vec<u8>>>);

impl Shared {
    fn text(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
    }
}

impl Write for Shared {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn run_sends_uart_and_debug_output_to_the_given_sinks() {
    // What `cargo xtask kernel --run` uses (with stdout and stderr): the UART text and
    // the xcal.dbg line must each reach their own sink.
    let (uart, debug) = (Shared::default(), Shared::default());
    let (r, _) =
        kernel::run_with_output(&committed_elf(), BOOT_MAX_STEPS, uart.clone(), debug.clone()).unwrap();
    assert_eq!(r, RunResult::Exited(0));
    // The one xcal.dbg line goes to the debug sink; everything around it to the UART.
    let (before, rest) = BOOT_OUTPUT.split_at(BOOT_OUTPUT.find("[dbg]").unwrap());
    let (dbg, after) = rest.split_at(rest.find('\n').unwrap() + 1);
    assert_eq!(uart.text(), format!("{before}{after}"));
    assert_eq!(kernel::mask_dbg_pc(&debug.text()), dbg);
}

#[test]
fn committed_kernel_layout() {
    let (_, elf) = load_committed();
    // The boot ROM jumps to 0x8000_0000 (spec section 7); _start must be there.
    assert_eq!(elf.entry, 0x8000_0000);
    assert_eq!(sym(&elf, "_start"), 0x8000_0000);
    for s in [
        "kmain",
        "start",
        "supervisor_entry",
        "machine_vec",
        "machine_halt",
        "kernel_vec",
        "kernel_trap",
        "illegal_instruction",
        "kprintf",
        "ksnprintf",
        "kvformat",
        "uart_putc",
        "xcal_dbg",
        "xcal_halt",
        "kinit",
        "kalloc",
        "kfree",
        "kvminit",
        "walk",
        "map_pages",
        "unmap_pages",
        "pagetable_free",
        "probe_load",
        "probe_store",
        "probe_fault",
    ] {
        assert!(sym(&elf, s) > 0x8000_0000, "{s}");
    }
    // Text, rodata and the writable rest start on page boundaries, so the
    // kernel page table can give each its own permissions.
    let (text, rodata, data) = (
        sym(&elf, "__text_start"),
        sym(&elf, "__rodata_start"),
        sym(&elf, "__data_start"),
    );
    assert_eq!(text, 0x8000_0000);
    assert!(text < rodata && rodata < data && data <= sym(&elf, "__bss_start"));
    assert!(rodata.is_multiple_of(4096) && data.is_multiple_of(4096));
    for s in ["kmain", "kernel_vec", "probe_store_insn"] {
        assert!((text..rodata).contains(&sym(&elf, s)), "{s} not in text");
    }
    assert!((data..sym(&elf, "__bss_start")).contains(&sym(&elf, "boot_fault")));
    // Direct-mode trap vectors: the low two bits of xtvec are the mode.
    for s in ["early_trap", "machine_vec", "kernel_vec"] {
        assert!(sym(&elf, s).is_multiple_of(4), "{s}");
    }
    let (bss, bss_end) = (sym(&elf, "__bss_start"), sym(&elf, "__bss_end"));
    let (stack_bottom, stack_top) = (sym(&elf, "__stack_bottom"), sym(&elf, "__stack_top"));
    assert!(bss <= bss_end && bss.is_multiple_of(4) && bss_end.is_multiple_of(4));
    assert!(bss_end <= stack_bottom && stack_top - stack_bottom == 16 * 1024);
    assert!(stack_top.is_multiple_of(16), "ABI requires a 16-byte aligned sp");
    let end = sym(&elf, "__kernel_end");
    assert!(end >= stack_top && end.is_multiple_of(4096));
}

#[test]
fn entry_clears_bss() {
    // Fill .bss with garbage after loading: the self-test counters live there,
    // so the output is only right if entry.s zeroes it.
    let (mut m, elf) = load_committed();
    let (start, end) = (sym(&elf, "__bss_start"), sym(&elf, "__bss_end"));
    assert!(end > start, "kernel has no .bss to clear");
    m.cpu
        .bus
        .load(start, &vec![0xa5; (end - start) as usize])
        .unwrap();
    let r = m.run(Some(BOOT_MAX_STEPS), |_| {});
    let out = String::from_utf8_lossy(&m.cpu.bus.uart.captured).into_owned();
    assert_eq!(kernel::mask_dbg_pc(&out), BOOT_OUTPUT);
    assert_eq!(r, RunResult::Exited(0));
}

#[test]
fn early_trap_reports_and_halts() {
    // Replace start()'s first instruction (M-mode, before machine_vec is
    // installed) with an illegal one (all zeros): the early trap handler
    // prints mcause, mepc, mtval and exits with 0xff.
    let (mut m, elf) = load_committed();
    let start = sym(&elf, "start");
    m.cpu.bus.load(start, &[0, 0, 0, 0]).unwrap();
    let r = m.run(Some(BOOT_MAX_STEPS), |_| {});
    let out = String::from_utf8_lossy(&m.cpu.bus.uart.captured).into_owned();
    // mcause: illegal instruction; mtval: the instruction bits.
    assert_eq!(dbg_values(&out), m_trap_report(2, start, 0), "output:\n{out}");
    assert_eq!(r, RunResult::Exited(UNEXPECTED_TRAP_EXIT));
}

#[test]
fn unexpected_machine_trap_reports_and_halts() {
    // A trap in M-mode once start() has installed machine_vec, which handles
    // only the timer and machine calls, falls through to early_trap. timer_init
    // runs after w_mtvec(machine_vec): make it trap in three ways.
    let (_, elf) = load_committed();
    let (timer_init, machine_vec) = (sym(&elf, "timer_init"), sym(&elf, "machine_vec"));
    for (word, mcause, mtval) in [
        (0x0000_0000, 2, 0),          // illegal instruction: mtval = its bits
        (0x0000_0073, 11, 0),         // ecall from M-mode
        (0x0010_0073, 3, timer_init), // ebreak: mtval = its pc
    ] {
        let (mut m, _) = load_committed();
        m.cpu.bus.load(timer_init, &u32::to_le_bytes(word)).unwrap();
        let mut via_machine_vec = false;
        let r = m.run(Some(BOOT_MAX_STEPS), |cpu| {
            via_machine_vec |= cpu.pc == machine_vec;
        });
        let out = String::from_utf8_lossy(&m.cpu.bus.uart.captured).into_owned();
        assert!(via_machine_vec, "{word:#x}");
        assert_eq!(
            dbg_values(&out),
            m_trap_report(mcause, timer_init, mtval),
            "{word:#x}, output:\n{out}"
        );
        assert_eq!(r, RunResult::Exited(UNEXPECTED_TRAP_EXIT), "{word:#x}");
    }
}

#[test]
fn halt_uses_kmain_return_value() {
    // Make kmain return 7 straight away (li a0, 7; ret).
    let (mut m, elf) = load_committed();
    let kmain = sym(&elf, "kmain");
    let li_a0_7: u32 = (7 << 20) | (10 << 7) | 0x13;
    let ret: u32 = 0x0000_8067;
    let mut code = li_a0_7.to_le_bytes().to_vec();
    code.extend_from_slice(&ret.to_le_bytes());
    m.cpu.bus.load(kmain, &code).unwrap();
    // kmain runs in S-mode, so the code reaches xcal.halt via MCALL_HALT.
    assert_eq!(m.run(Some(BOOT_MAX_STEPS), |_| {}), RunResult::Exited(7));
    assert!(m.cpu.bus.uart.captured.is_empty());
}

/// Encodes `addi rd, rs1, imm`.
fn addi(rd: u32, rs1: u32, imm: i32) -> u32 {
    ((imm as u32) << 20) | (rs1 << 15) | (rd << 7) | 0x13
}

#[test]
fn unknown_machine_call_returns_minus_one() {
    // kmain: li a7, 99; li a1, 40; ecall; add a0, a0, a1; ret. machine_vec
    // returns -1 in a0 and leaves a1 alone, so kmain returns 39.
    let (mut m, elf) = load_committed();
    let (a0, a1, a7) = (x("a0"), x("a1"), x("a7"));
    let code = [
        addi(a7, 0, 99),
        addi(a1, 0, 40),
        0x0000_0073,                                // ecall
        (a1 << 20) | (a0 << 15) | (a0 << 7) | 0x33, // add a0, a0, a1
        0x0000_8067,                                // ret
    ];
    let bytes: Vec<u8> = code.iter().flat_map(|w| w.to_le_bytes()).collect();
    m.cpu.bus.load(sym(&elf, "kmain"), &bytes).unwrap();
    assert_eq!(m.run(Some(BOOT_MAX_STEPS), |_| {}), RunResult::Exited(39));
}

#[test]
fn kmain_runs_in_s_mode_with_traps_delegated() {
    let (mut m, elf) = load_committed();
    let kmain = sym(&elf, "kmain");
    let mut steps = 0;
    while m.cpu.pc != kmain {
        m.cpu.step();
        steps += 1;
        assert!(steps < 10_000, "never reached kmain");
    }
    assert_eq!(m.cpu.mode, Mode::S);
    let c = &m.cpu.csr;
    // Every exception but ecall from S (the machine calls) and from M.
    assert_eq!(c.medeleg, 0xb1ff);
    // All S-level interrupts; the machine timer stays in M-mode.
    assert_eq!(
        c.mideleg,
        (1 << IRQ_S_SOFT) | (1 << IRQ_S_TIMER) | (1 << IRQ_S_EXT)
    );
    assert_eq!(c.mie, 1 << IRQ_M_TIMER);
    assert_eq!(c.mtvec, sym(&elf, "machine_vec"));
    assert_eq!(c.stvec, sym(&elf, "kernel_vec"));
    let scratch = sym(&elf, "machine_scratch");
    assert_eq!(c.mscratch, scratch);
    assert_eq!(
        m.cpu.bus.peek(scratch + SCRATCH_INTERVAL),
        Some(TICK_INTERVAL as u32)
    );
    // S-mode interrupts stay off until trap_init.
    assert_eq!(c.mstatus & MSTATUS_SIE, 0);
    // The first tick is armed one interval after start() ran.
    let clint = &m.cpu.bus.clint;
    assert!(clint.mtimecmp > clint.mtime && clint.mtimecmp - clint.mtime < TICK_INTERVAL);
    assert!(clint.mtimecmp <= TICK_INTERVAL + steps);
}

#[test]
fn ticks_are_one_interval_apart() {
    let (mut m, elf) = load_committed();
    let (machine_vec, kernel_vec) = (sym(&elf, "machine_vec"), sym(&elf, "kernel_vec"));
    let mut machine_ticks = Vec::new();
    let mut deadlines = Vec::new();
    let mut kernel_ticks = Vec::new();
    let mut steps_after_first_tick = 0u64;
    let mut halts = 0;
    let probes = [sym(&elf, "probe_load_insn"), sym(&elf, "probe_store_insn")];
    let mut probe_faults = 0;
    let r = m.run(Some(BOOT_MAX_STEPS), |cpu| {
        if !kernel_ticks.is_empty() {
            steps_after_first_tick += 1;
        }
        if cpu.pc == machine_vec {
            if cpu.csr.mcause == INTERRUPT_BIT | IRQ_M_TIMER {
                machine_ticks.push(cpu.bus.clint.mtime);
                deadlines.push(cpu.bus.clint.mtimecmp);
            } else {
                // The only other trap to M-mode: the final MCALL_HALT.
                assert_eq!(cpu.csr.mcause, ECALL_FROM_S);
                halts += 1;
            }
        }
        if cpu.pc == kernel_vec && cpu.csr.scause & INTERRUPT_BIT == 0 {
            // The vm self-test's deliberate page faults, caught by the probes.
            assert!(probes.contains(&cpu.csr.sepc), "{:#x}", cpu.csr.sepc);
            assert!(matches!(cpu.csr.scause, 13 | 15), "{:#x}", cpu.csr.scause);
            probe_faults += 1;
        } else if cpu.pc == kernel_vec {
            assert_eq!(cpu.csr.scause, INTERRUPT_BIT | IRQ_S_SOFT);
            // Ticks arrive with paging on.
            assert_ne!(cpu.csr.satp >> 31, 0, "tick before paging");
            kernel_ticks.push(cpu.bus.clint.mtime);
        }
    });
    assert_eq!(r, RunResult::Exited(0));
    assert_eq!(halts, 1);
    assert_eq!(kernel_ticks.len(), 10);
    // Unmapped load and store, stores to text and rodata.
    assert_eq!(probe_faults, 4);
    // Setting up memory takes a few intervals before trap_init enables the
    // tick: those machine ticks pile up in the one SSIP bit, which the kernel
    // takes as its first tick as soon as trap_init runs.
    // The deadlines are exactly one interval apart (machine_vec adds the
    // interval to mtimecmp, so ticks don't drift); a tick that lands on an
    // exception being taken is entered a step late.
    assert!(machine_ticks.len() >= 10, "{machine_ticks:?}");
    for w in deadlines.windows(2) {
        assert_eq!(w[1] - w[0], TICK_INTERVAL, "{deadlines:?}");
    }
    for (t, d) in machine_ticks.iter().zip(&deadlines) {
        assert!((0..=1).contains(&(t - d)), "{machine_ticks:?}");
    }
    // After that, each machine tick reaches the kernel right away, the same
    // few instructions later every time.
    let delays: Vec<u64> = kernel_ticks[1..]
        .iter()
        .zip(&machine_ticks[machine_ticks.len() - 9..])
        .map(|(s, m)| s - m)
        .collect();
    assert!(delays.iter().all(|&d| d == delays[0] && d < 100), "{delays:?}");
    assert_eq!(m.cpu.bus.peek(sym(&elf, "ticks")), Some(10));
    // wfi skips to the next tick: ten intervals of mtime pass in far fewer
    // instructions than one.
    assert!(m.cpu.bus.clint.mtime >= 10 * TICK_INTERVAL);
    assert!(
        steps_after_first_tick < TICK_INTERVAL,
        "{steps_after_first_tick} steps"
    );
}

// ---- Interrupts in the middle of kernel code ----
//
// In a normal boot every tick lands in wfi(), where hardly any register is
// live, so the boot tests can't tell whether the trap handlers give the
// interrupted code its registers back. These tests interrupt code that has
// live values everywhere.

/// `j .`: an endless loop, standing in for busy kernel code.
const J_SELF: u32 = 0x0000_006f;

/// Byte offsets in `struct trap_frame` (kernel/include/trap.h).
const FRAME_SEPC: u32 = 128;
const FRAME_SCAUSE: u32 = 132;
const FRAME_SSTATUS: u32 = 140;

/// A distinct value for every register but x0 and sp: `tag` in the top byte,
/// the register number in the others.
fn reg_pattern(tag: u32, sp: u32) -> [u32; 32] {
    let mut r: [u32; 32] = std::array::from_fn(|i| (tag << 24) | (i as u32 * 0x0001_0101));
    r[0] = 0;
    r[2] = sp;
    r
}

/// Boots the committed kernel up to kmain, then turns kmain into `j .; j .`
/// and fills every register but sp with `reg_pattern(0xa5, ..)`: busy code
/// with every register live. S-mode interrupts are on, as after trap_init.
/// Returns the machine, the ELF and the registers.
fn busy_kmain() -> (Machine, grail::elf::Elf, [u32; 32]) {
    let (mut m, elf) = load_committed();
    let kmain = sym(&elf, "kmain");
    step_to(&mut m, kmain);
    let code: Vec<u8> = [J_SELF, J_SELF].iter().flat_map(|w| w.to_le_bytes()).collect();
    m.cpu.bus.load(kmain, &code).unwrap();
    let regs = reg_pattern(0xa5, m.cpu.x[2]);
    m.cpu.x = regs;
    m.cpu.csr.mie |= 1 << IRQ_S_SOFT;
    m.cpu.csr.mstatus |= MSTATUS_SIE;
    (m, elf, regs)
}

#[test]
fn tick_preserves_every_register() {
    // A tick goes through both handlers: machine_vec (rearm the timer, raise
    // SSIP) and kernel_vec (count it). Neither may change a register.
    let (mut m, elf, regs) = busy_kmain();
    let kmain = sym(&elf, "kmain");
    let clint = &mut m.cpu.bus.clint;
    clint.mtimecmp = clint.mtime + 20;
    let cmp = clint.mtimecmp;

    step_to(&mut m, sym(&elf, "machine_vec"));
    assert_eq!(m.cpu.csr.mcause, INTERRUPT_BIT | IRQ_M_TIMER);
    assert_eq!(m.cpu.csr.mepc, kmain);
    // machine_vec returns to the loop, which takes the forwarded tick at once.
    step_to(&mut m, sym(&elf, "kernel_vec"));
    assert_eq!(m.cpu.csr.scause, INTERRUPT_BIT | IRQ_S_SOFT);
    assert_eq!(m.cpu.csr.sepc, kmain);
    assert_eq!(m.cpu.x, regs, "machine_vec changed registers");
    assert_eq!(m.cpu.bus.clint.mtimecmp, cmp + TICK_INTERVAL);

    step_to(&mut m, kmain);
    assert_eq!(m.cpu.mode, Mode::S);
    assert_eq!(m.cpu.x, regs, "kernel_vec changed registers");
    assert_ne!(m.cpu.csr.mstatus & MSTATUS_SIE, 0);
    assert_eq!(m.cpu.csr.mip & (1 << IRQ_S_SOFT), 0);
    assert_eq!(m.cpu.bus.peek(sym(&elf, "ticks")), Some(1));
}

#[test]
fn kernel_vec_resumes_from_the_frame() {
    // kernel_trap may change the trap frame, and kernel_vec must resume from
    // it: every register, sepc and sstatus are loaded back from the frame.
    let (mut m, elf, regs) = busy_kmain();
    let kmain = sym(&elf, "kmain");
    m.cpu.csr.mip |= 1 << IRQ_S_SOFT; // a tick, as machine_vec raises it
    step_to(&mut m, sym(&elf, "kernel_trap"));
    let frame = m.cpu.x[x("a0") as usize];
    let peek = |m: &Machine, off: u32| m.cpu.bus.peek(frame + off).unwrap();
    for (i, &v) in regs.iter().enumerate() {
        assert_eq!(peek(&m, 4 * i as u32), v, "{}", reg::abi_name(i as u8));
    }
    assert_eq!(peek(&m, FRAME_SEPC), kmain);
    assert_eq!(peek(&m, FRAME_SCAUSE), INTERRUPT_BIT | IRQ_S_SOFT);
    let sstatus = peek(&m, FRAME_SSTATUS);
    assert_ne!(sstatus & MSTATUS_SPIE, 0);
    assert_ne!(sstatus & MSTATUS_SPP, 0);

    // Other values for every register but sp, which kernel_vec recomputes by
    // popping the frame; resume at kmain + 4 with interrupts off.
    let new = reg_pattern(0x5a, regs[2]);
    let mut put = |off: u32, v: u32| m.cpu.bus.load(frame + off, &v.to_le_bytes()).unwrap();
    for (i, &v) in new.iter().enumerate() {
        put(4 * i as u32, v);
    }
    put(FRAME_SEPC, kmain + 4);
    put(FRAME_SSTATUS, sstatus & !MSTATUS_SPIE);

    step_to(&mut m, kmain + 4);
    assert_eq!(m.cpu.mode, Mode::S);
    assert_eq!(m.cpu.x, new);
    assert_eq!(m.cpu.csr.mstatus & MSTATUS_SIE, 0);
    assert_eq!(m.cpu.bus.peek(sym(&elf, "ticks")), Some(1));
}

#[test]
fn ticks_preempting_kernel_code_change_nothing() {
    // Shorten the tick interval to a few hundred mtime ticks (a prime, so the
    // ticks don't fall into step with a loop) right after start() armed the
    // timer. Ticks then land all over kprintf and its self-test instead of
    // only in wait_ticks' idle loop. A clobbered register would fail the
    // self-test, garble the output or fault; and every tick still gets its
    // own line.
    const SHORT_INTERVAL: u32 = 211;
    let (mut m, elf) = load_committed();
    step_to(&mut m, sym(&elf, "supervisor_entry"));
    let scratch = sym(&elf, "machine_scratch");
    m.cpu
        .bus
        .load(scratch + SCRATCH_INTERVAL, &SHORT_INTERVAL.to_le_bytes())
        .unwrap();
    let clint = &mut m.cpu.bus.clint;
    clint.mtimecmp = clint.mtime + u64::from(SHORT_INTERVAL);

    // Where a tick interrupts wait_ticks' idle loop: after `wfi` in M-mode,
    // and after intr_on's `csrsi` in S-mode.
    let idle = [sym(&elf, "wfi") + 4, sym(&elf, "intr_on") + 4];
    let (machine_vec, kernel_vec) = (sym(&elf, "machine_vec"), sym(&elf, "kernel_vec"));
    let (mut machine_busy, mut kernel_busy) = (0, 0);
    let r = m.run(Some(BOOT_MAX_STEPS), |cpu| {
        if cpu.pc == machine_vec
            && cpu.csr.mcause == INTERRUPT_BIT | IRQ_M_TIMER
            && !idle.contains(&cpu.csr.mepc)
        {
            machine_busy += 1;
        }
        if cpu.pc == kernel_vec && !idle.contains(&cpu.csr.sepc) {
            kernel_busy += 1;
        }
    });
    let out = String::from_utf8_lossy(&m.cpu.bus.uart.captured).into_owned();
    assert_eq!(kernel::mask_dbg_pc(&out), BOOT_OUTPUT, "raw output:\n{out}");
    assert_eq!(r, RunResult::Exited(0));
    // Fewer kernel ticks: until trap_init, the machine ticks only pile up in
    // the one SSIP bit.
    assert!(
        machine_busy >= 100 && kernel_busy >= 30,
        "only {machine_busy} machine and {kernel_busy} kernel ticks outside the idle loop"
    );
}

#[test]
fn timer_rearm_carries_into_the_high_word() {
    // machine_vec adds the interval to mtimecmp as two 32-bit halves: a tick
    // just below 2^32 must carry into the high word.
    const CMP: u64 = 0xffff_ff00;
    let (mut m, elf) = load_committed();
    step_to(&mut m, sym(&elf, "kmain"));
    let clint = &mut m.cpu.bus.clint;
    clint.mtime = CMP - 0x100;
    clint.mtimecmp = CMP;
    step_to(&mut m, sym(&elf, "machine_vec"));
    assert_eq!(m.cpu.csr.mcause, INTERRUPT_BIT | IRQ_M_TIMER);
    for _ in 0..100 {
        if m.cpu.mode != Mode::M {
            break;
        }
        m.cpu.step();
    }
    assert_eq!(m.cpu.mode, Mode::S, "machine_vec never returned");
    assert_eq!(m.cpu.bus.clint.mtimecmp, CMP + TICK_INTERVAL);
    assert!(m.cpu.bus.clint.mtimecmp > 1 << 32);
}

/// Runs the committed kernel with `boot_fault` patched to `fault`. Returns the
/// machine afterwards, how the run ended and what the kernel printed.
fn run_with_fault(fault: u32) -> (Machine, grail::elf::Elf, RunResult, String) {
    let (mut m, elf) = load_committed();
    m.cpu
        .bus
        .load(sym(&elf, "boot_fault"), &fault.to_le_bytes())
        .unwrap();
    let r = m.run(Some(BOOT_MAX_STEPS), |_| {});
    let out = String::from_utf8_lossy(&m.cpu.bus.uart.captured).into_owned();
    (m, elf, r, out)
}

/// A panic report: the cause line, then `name 0xvalue` pairs for the trap
/// CSRs, then `name value` pairs for x1-x31. Returns the cause and the values.
/// A page fault's report also has a line on how the page table maps stval
/// (`  va ...`), which this skips.
fn parse_panic(out: &str) -> (String, HashMap<String, u32>) {
    let report = &out[out
        .find("panic: ")
        .unwrap_or_else(|| panic!("no panic in:\n{out}"))..];
    let mut lines = report.lines();
    let cause = lines.next().unwrap()["panic: ".len()..].to_string();
    let mut values = HashMap::new();
    let mut n = 0;
    for line in lines.filter(|l| !l.starts_with("  va ")) {
        n += 1;
        let words: Vec<&str> = line.split_whitespace().collect();
        assert!(
            words.len().is_multiple_of(2) && !words.is_empty(),
            "bad line {line:?}"
        );
        for pair in words.chunks(2) {
            let hex = pair[1].strip_prefix("0x").unwrap_or(pair[1]);
            assert_eq!(hex.len(), 8, "{line:?}");
            let v = u32::from_str_radix(hex, 16).unwrap();
            assert!(
                values.insert(pair[0].to_string(), v).is_none(),
                "{} twice",
                pair[0]
            );
        }
    }
    assert_eq!(n, 1 + 8, "report:\n{report}");
    assert_eq!(values.len(), 4 + 31, "report:\n{report}");
    (cause, values)
}

#[test]
fn boot_fault_ships_as_none() {
    // In .data, so the image carries it and entry.s doesn't clear it.
    let (m, elf) = load_committed();
    let addr = sym(&elf, "boot_fault");
    assert_eq!(m.cpu.bus.peek(addr), Some(BOOT_FAULT_NONE));
    assert!(addr < sym(&elf, "__bss_start"));
}

#[test]
fn illegal_instruction_panics_with_register_dump() {
    let (m, elf, r, out) = run_with_fault(BOOT_FAULT_ILLEGAL);
    assert_eq!(r, RunResult::Exited(PANIC_EXIT), "output:\n{out}");
    // The ticks ran first.
    let before = &out[..out.find("panic: ").unwrap()];
    assert_eq!(kernel::mask_dbg_pc(before), BOOT_OUTPUT);

    let (cause, v) = parse_panic(&out);
    assert_eq!(cause, "illegal instruction");
    assert_eq!(v["scause"], 2);
    // stval holds the instruction bits: unimp (csrrw zero, cycle, zero).
    assert_eq!(v["stval"], 0xc000_1073);
    let sepc = v["sepc"];
    assert_eq!(m.cpu.bus.peek(sepc), Some(0xc000_1073));
    let func = sym(&elf, "illegal_instruction");
    assert!((func..func + 0x100).contains(&sepc), "sepc {sepc:#x}");
    // From S-mode, with interrupts on.
    assert_ne!(v["sstatus"] & MSTATUS_SPP, 0);
    assert_ne!(v["sstatus"] & MSTATUS_SPIE, 0);
    // illegal_instruction set every register but sp to its number in each
    // byte, so this checks the whole trap frame layout.
    for i in 1..32u8 {
        let name = excalibur::reg::abi_name(i);
        if i != 2 {
            assert_eq!(v[name], u32::from(i) * 0x0101_0101, "{name}");
        }
    }
    let sp = v["sp"];
    assert!(sp > sym(&elf, "__stack_bottom") && sp <= sym(&elf, "__stack_top"));
    assert!(sp.is_multiple_of(16), "sp {sp:#x}");
}

#[test]
fn unmapped_load_panics() {
    let (m, elf, r, out) = run_with_fault(BOOT_FAULT_LOAD);
    assert_eq!(r, RunResult::Exited(PANIC_EXIT), "output:\n{out}");
    let before = &out[..out.find("panic: ").unwrap()];
    assert_eq!(kernel::mask_dbg_pc(before), BOOT_OUTPUT);

    // With paging on, an address the page table doesn't map is a page fault
    // (before paging, the bus would have refused it: a load access fault).
    let (cause, v) = parse_panic(&out);
    assert_eq!(cause, "load page fault");
    assert_eq!(v["scause"], 13);
    assert_eq!(v["stval"], UNMAPPED_ADDR);
    assert!(
        out.contains(&format!("\n  va {UNMAPPED_ADDR:#010x} is not mapped\n")),
        "{out}"
    );
    let sepc = v["sepc"];
    assert!(sepc > sym(&elf, "_start") && sepc < sym(&elf, "__rodata_start"));
    let word = m.cpu.bus.peek(sepc).unwrap();
    assert!(
        matches!(excalibur::decode(word), excalibur::Inst::Load { .. }),
        "sepc {sepc:#x} holds {}",
        excalibur::decode(word)
    );
    assert_ne!(v["sstatus"] & MSTATUS_SPP, 0);
}

#[test]
fn store_to_kernel_text_panics() {
    // Outside the probes a store page fault is a kernel bug: the panic shows
    // the cause, stval and how the kernel page table maps stval.
    let (m, elf, r, out) = run_with_fault(BOOT_FAULT_STORE_TEXT);
    assert_eq!(r, RunResult::Exited(PANIC_EXIT), "output:\n{out}");
    let before = &out[..out.find("panic: ").unwrap()];
    assert_eq!(kernel::mask_dbg_pc(before), BOOT_OUTPUT);

    let kmain = sym(&elf, "kmain");
    let (cause, v) = parse_panic(&out);
    assert_eq!(cause, "store page fault");
    assert_eq!(v["scause"], 15);
    assert_eq!(v["stval"], kmain);
    // Identity mapped, R+X, accessed (it ran) but never dirty.
    let pte = (kmain >> 12) << 10 | PTE_V | PTE_R | PTE_X | PTE_A;
    assert!(
        out.contains(&format!(
            "\n  va {kmain:#010x} maps to pa {kmain:#010x}, pte {pte:#010x} -A--X-RV\n"
        )),
        "{out}"
    );
    let word = m.cpu.bus.peek(v["sepc"]).unwrap();
    assert!(
        matches!(excalibur::decode(word), excalibur::Inst::Store { .. }),
        "sepc holds {}",
        excalibur::decode(word)
    );
    // The store didn't happen.
    let (fresh, _) = load_committed();
    assert_eq!(m.cpu.bus.peek(kmain), fresh.cpu.bus.peek(kmain));
}

// ---- Memory: the page allocator and the kernel page table ----

const PAGE: u32 = 4096;

// Sv32 PTE bits.
const PTE_V: u32 = 1 << 0;
const PTE_R: u32 = 1 << 1;
const PTE_W: u32 = 1 << 2;
const PTE_X: u32 = 1 << 3;
const PTE_U: u32 = 1 << 4;
const PTE_A: u32 = 1 << 6;

/// Boots the committed kernel on a machine with `ram_mib` MiB of RAM, to the
/// end. Returns the machine, the ELF and what the kernel printed.
fn boot_with_ram(ram_mib: usize) -> (Machine, grail::elf::Elf, String) {
    let file = committed_elf();
    let elf = grail::elf::parse(&file).unwrap();
    let mut m = Machine::with_ram(ram_mib << 20);
    m.cpu.bus.uart.capture = true;
    m.load_elf(&file).unwrap();
    let r = m.run(Some(BOOT_MAX_STEPS * 4), |_| {});
    let out = String::from_utf8_lossy(&m.cpu.bus.uart.captured).into_owned();
    assert_eq!(r, RunResult::Exited(0), "{ram_mib} MiB, output:\n{out}");
    (m, elf, out)
}

/// Every leaf of the Sv32 page table `satp` points at, as (va, pte), and the
/// number of pages the table takes (root and second-level tables). Panics on
/// a megapage or a reserved encoding, which the kernel never makes.
fn page_table(m: &Machine, satp: u32) -> (Vec<(u32, u32)>, u32) {
    let pte_at = |addr: u32| {
        m.cpu
            .bus
            .peek(addr)
            .unwrap_or_else(|| panic!("PTE at {addr:#x} not in RAM"))
    };
    let root = (satp & 0x3f_ffff) * PAGE;
    let mut leaves = Vec::new();
    let mut tables = 1;
    for i in 0..1024u32 {
        let pte = pte_at(root + 4 * i);
        if pte & PTE_V == 0 {
            continue;
        }
        assert_eq!(pte & (PTE_R | PTE_W | PTE_X), 0, "megapage at root[{i}]");
        tables += 1;
        let table = (pte >> 10) * PAGE;
        for j in 0..1024u32 {
            let pte = pte_at(table + 4 * j);
            if pte & PTE_V != 0 {
                assert_ne!(pte & (PTE_R | PTE_X), 0, "bad leaf {pte:#x}");
                leaves.push((i << 22 | j << 12, pte));
            }
        }
    }
    (leaves, tables)
}

/// Checks the kernel page table after a boot with `ram_mib` MiB of RAM, and
/// the free page count.
fn check_kernel_memory(ram_mib: usize) {
    let (m, elf, out) = boot_with_ram(ram_mib);
    assert!(
        out.contains(&format!(
            "ram: {ram_mib} MiB at 0x80000000\npaging: on (Sv32)\nvm self-test: 24/24 passed\n"
        )),
        "{out}"
    );
    let satp = m.cpu.csr.satp;
    assert_ne!(satp >> 31, 0, "paging is off");
    let ram_end = 0x8000_0000u64 + (ram_mib as u64) * (1 << 20);
    let kernel_end = sym(&elf, "__kernel_end");
    let root = (satp & 0x3f_ffff) * PAGE;
    assert!(root >= kernel_end && u64::from(root) < ram_end, "root {root:#x}");

    // What the kernel should map, identity, page by page.
    let mut want = std::collections::BTreeMap::new();
    let mut map = |start: u64, end: u64, perm: u32| {
        for page in (start..end).step_by(PAGE as usize) {
            assert!(want.insert(page as u32, perm).is_none(), "{page:#x} twice");
        }
    };
    let (text, rodata, data) = (
        sym(&elf, "__text_start"),
        sym(&elf, "__rodata_start"),
        sym(&elf, "__data_start"),
    );
    map(text.into(), rodata.into(), PTE_R | PTE_X);
    map(rodata.into(), data.into(), PTE_R);
    map(data.into(), ram_end, PTE_R | PTE_W);
    // Every device in the sysinfo table, whole pages.
    for dev in grail::bus::device_table(m.config()) {
        let start = u64::from(dev.base) & !u64::from(PAGE - 1);
        let end = (u64::from(dev.base) + u64::from(dev.size)).next_multiple_of(PAGE.into());
        map(start, end, PTE_R | PTE_W);
    }

    let (leaves, tables) = page_table(&m, satp);
    let mut got = std::collections::BTreeMap::new();
    for &(va, pte) in &leaves {
        assert_eq!((pte >> 10) * PAGE, va, "{va:#x} not identity mapped");
        assert_eq!(pte & PTE_U, 0, "{va:#x} is a user page");
        got.insert(va, pte & (PTE_R | PTE_W | PTE_X));
    }
    // Compared as a whole only after spotting the first difference, since the
    // maps have tens of thousands of entries.
    if let Some((va, perm)) = want.iter().find(|(va, perm)| got.get(va) != Some(perm)) {
        panic!("{va:#x}: want {perm:#x}, got {:?}", got.get(va));
    }
    if let Some(va) = got.keys().find(|va| !want.contains_key(va)) {
        panic!("{va:#x} is mapped but shouldn't be");
    }
    assert!(!got.contains_key(&ALIAS_VA));

    // Every page after the kernel is free but the page tables.
    let free_ram_pages = ((ram_end - u64::from(kernel_end)) / u64::from(PAGE)) as u32;
    assert_eq!(m.cpu.bus.peek(sym(&elf, "nfree")), Some(free_ram_pages - tables));
    // The table: one root, one second-level table per 4 MiB that has a
    // mapping, and the one the vm self-test left at ALIAS_VA.
    let mut regions: Vec<u32> = want.keys().map(|va| va >> 22).collect();
    regions.dedup();
    assert_eq!(tables as usize, 1 + regions.len() + 1);
}

#[test]
fn kernel_page_table_maps_kernel_ram_and_devices() {
    check_kernel_memory(128);
}

#[test]
fn allocator_follows_the_ram_size() {
    // RAM comes from sysinfo: the smallest machine, and one whose RAM ends
    // inside a 4 MiB region.
    for mib in [1, 6] {
        check_kernel_memory(mib);
    }
}

#[test]
fn allocator_handles_ram_up_to_the_top_of_the_address_space() {
    // The largest machine: RAM ends at 2^32, which wraps to 0 in a u32.
    check_kernel_memory(2048);
}

#[test]
fn fault_with_interrupts_off_panics() {
    // An illegal instruction as kmain's first: S-mode traps work before
    // trap_init, since start() sets stvec before dropping to S-mode.
    let (mut m, elf) = load_committed();
    let kmain = sym(&elf, "kmain");
    m.cpu.bus.load(kmain, &[0, 0, 0, 0]).unwrap();
    let r = m.run(Some(BOOT_MAX_STEPS), |_| {});
    let out = String::from_utf8_lossy(&m.cpu.bus.uart.captured).into_owned();
    assert_eq!(r, RunResult::Exited(PANIC_EXIT), "output:\n{out}");
    assert!(out.starts_with("panic: illegal instruction\n"), "{out}");
    let (_, v) = parse_panic(&out);
    assert_eq!((v["scause"], v["sepc"], v["stval"]), (2, kmain, 0));
    assert_eq!(v["sstatus"] & MSTATUS_SPIE, 0);
    // ra is where kmain would have returned to: supervisor_entry's next
    // instruction.
    assert_eq!(v["ra"], sym(&elf, "supervisor_entry") + 8);
}

/// Every C file and header in the kernel, with its name.
fn kernel_c_sources() -> Vec<(String, String)> {
    let mut files = Vec::new();
    for dir in [kernel_dir(), kernel_dir().join("include")] {
        for e in std::fs::read_dir(&dir).unwrap() {
            let path = e.unwrap().path();
            if matches!(path.extension().and_then(|x| x.to_str()), Some("c" | "h")) {
                let text = std::fs::read_to_string(&path).unwrap();
                files.push((path.file_name().unwrap().to_string_lossy().into_owned(), text));
            }
        }
    }
    assert!(files.len() >= 8);
    files
}

#[test]
fn kernel_c_stays_freestanding() {
    // No libc headers (everything is #include "...") and no floating point,
    // so Merlin's C subset (#15) can compile it later.
    for (name, text) in kernel_c_sources() {
        for (i, line) in text.lines().enumerate() {
            let code = line.split("/*").next().unwrap();
            assert!(!code.contains("#include <"), "{name}:{}: system header", i + 1);
            for word in code.split(|c: char| !c.is_ascii_alphanumeric() && c != '_') {
                assert!(
                    word != "float" && word != "double",
                    "{name}:{}: floating point",
                    i + 1
                );
            }
        }
    }
}

#[test]
fn kernel_asm_is_merlin_dialect() {
    // The .s files must use only what merlin-as understands. Until its encoder
    // is complete, "not implemented yet" is the only acceptable error.
    let sources = kernel::sources(&kernel_dir()).unwrap();
    let asm: Vec<_> = sources.iter().filter(|s| s.lang == Lang::Asm).collect();
    assert!(!asm.is_empty());
    for s in asm {
        let text = std::fs::read_to_string(kernel_dir().join(&s.name)).unwrap();
        if let Err(errs) = merlin_as::assemble(&text) {
            let other: Vec<String> = errs
                .iter()
                .filter(|e| !e.msg.starts_with(merlin_as::encode::UNIMPLEMENTED))
                .map(|e| format!("{}: {e}", s.name))
                .collect();
            assert!(
                other.is_empty(),
                "merlin-as rejects kernel assembly:\n{}",
                other.join("\n")
            );
        }
    }
}

// ---- Rebuilding from source (skipped without LLVM) ----

fn llvm() -> Option<(PathBuf, PathBuf)> {
    let clang = tools::find("clang")?;
    let lld = tools::find_beside("ld.lld", &clang)?;
    Some((clang, lld))
}

fn scratch(name: &str) -> PathBuf {
    Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("kernel-tests")
        .join(name)
}

fn fresh_build(clang: PathBuf, lld: PathBuf, assembler: Assembler, name: &str) -> Result<Vec<u8>, String> {
    let dir = scratch(name);
    let _ = std::fs::remove_dir_all(&dir);
    let build = Build {
        kernel_dir: kernel_dir(),
        obj_dir: dir.join("obj"),
        out: dir.join("kernel.elf"),
        clang,
        lld,
        assembler,
        verbose: true,
    };
    let out = build.run()?;
    Ok(std::fs::read(out).unwrap())
}

#[test]
fn clang_build_boots_and_matches_committed() {
    let Some((clang, lld)) = llvm() else {
        eprintln!("skipped: clang/ld.lld not found");
        return;
    };
    let elf = fresh_build(clang.clone(), lld, Assembler::Clang, "clang").unwrap();
    assert_boots(&elf);

    // With the clang that built the committed image the bytes must match, or
    // kernel.elf is stale: rebuild it with `cargo xtask kernel`.
    let probe = Build {
        kernel_dir: kernel_dir(),
        obj_dir: PathBuf::new(),
        out: PathBuf::new(),
        clang,
        lld: PathBuf::new(),
        assembler: Assembler::Clang,
        verbose: false,
    };
    let recorded = std::fs::read_to_string(kernel_dir().join(kernel::VERSION_FILE)).unwrap();
    match probe.clang_version() {
        Ok(v) if v == recorded.trim() => assert!(
            elf == committed_elf(),
            "kernel/kernel.elf is out of date: run `cargo xtask kernel` and commit it"
        ),
        Ok(v) => eprintln!(
            "byte comparison skipped: built with `{}`, have `{v}`",
            recorded.trim()
        ),
        Err(e) => panic!("{e}"),
    }
}

#[test]
fn merlin_as_build_boots() {
    // Bootstrapping step 1 (spec section 10): kernel .s files assembled by
    // merlin-as, C by clang, linked by ld.lld. Runs once the encoder handles
    // everything entry.s uses.
    let Some((clang, lld)) = llvm() else {
        eprintln!("skipped: clang/ld.lld not found");
        return;
    };
    match fresh_build(clang, lld, Assembler::Merlin, "merlin") {
        Ok(elf) => assert_boots(&elf),
        Err(e) if e.contains(merlin_as::encode::UNIMPLEMENTED) => {
            eprintln!("skipped: merlin-as encoder incomplete");
        }
        Err(e) => panic!("{e}"),
    }
}
