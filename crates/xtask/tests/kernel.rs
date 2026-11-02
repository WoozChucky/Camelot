//! Kernel tests. The committed `kernel/kernel.elf` always runs on Grail, so
//! these pass without LLVM. When clang and ld.lld are installed, the kernel is
//! also rebuilt from source and checked against the committed image.

use excalibur::csr::{
    IRQ_M_EXT, IRQ_M_TIMER, IRQ_S_EXT, IRQ_S_SOFT, IRQ_S_TIMER, MSTATUS_SIE, MSTATUS_SPIE, MSTATUS_SPP,
};
use excalibur::reg;
use grail::block::{error as blk_error, BlockBackend, MemDisk};
use grail::block::{
    CMD_FLUSH as BLK_CMD_FLUSH, CMD_READ as BLK_CMD_READ, CMD_WRITE as BLK_CMD_WRITE, REG_CMD as BLK_REG_CMD,
    REG_ERROR as BLK_REG_ERROR, REG_STATUS as BLK_REG_STATUS,
};
use grail::bus::{BLK_BASE, BLK_SIZE, MAX_DISKS, PLIC_BASE, RAM_BASE, SYSINFO_BASE, SYSINFO_SIZE, UART_BASE};
use grail::cpu::Mode;
use grail::devices::{BLK_IRQ, UART_IRQ};
use grail::sysinfo::{DeviceEntry, DeviceType, Sysinfo};
use grail::sysinfo::{REG_MAGIC, REG_TIMEBASE_HZ};
use grail::trap::INTERRUPT_BIT;
use grail::{DiskConfig, Machine, MachineConfig, RunResult};
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use xtask::kernel::{self, Assembler, Build, Lang, BOOT_MAX_STEPS};
use xtask::{repo_root, tools};

/// What the kernel prints before its shell starts, with `xcal.dbg`'s pc masked
/// (see `mask_dbg_pc`).
const BOOT_OUTPUT: &str = "\
Camelot v0.1.0 booting...
kprintf: d=-42 u=4294967295 x=cafef00d p=0x80000000 s=excalibur c=K %=%
kprintf self-test: 26/26 passed
[dbg] pc=........ a0 = 0x00000000 (0)
ram: 128 MiB at 0x80000000
paging: on (Sv32)
vm self-test: 26/26 passed
timer: 10 Hz tick, 10000000 Hz timebase
disk: none
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

/// What the user-space shell, the first process, prints before its first
/// prompt, and the prompt (kernel/user/sh.c).
const SHELL_BANNER: &str = "Type help for the list of commands.\n";
const PROMPT: &str = "$ ";

/// The input most runs get: the shell's command to leave, after which the
/// kernel stops the machine.
const EXIT: &[u8] = b"exit\n";

/// The whole output of a run whose shell is given `EXIT`.
fn halted_output() -> String {
    format!("{BOOT_OUTPUT}{SHELL_BANNER}{PROMPT}exit\n")
}

/// mtime ticks between timer interrupts: the nominal 10 MHz timebase / 10 Hz.
const TICK_INTERVAL: u64 = 1_000_000;

/// More than the steps from reset to `kmain`, nearly all of them `_start`
/// zeroing `.bss` a word at a time (about 35 000 since the buffer cache's
/// 30 KiB live there).
const TO_KMAIN_STEPS: u64 = 100_000;

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
const BOOT_FAULT_REMAP: u32 = 5;
const BOOT_FAULT_UNMAP: u32 = 6;
const BOOT_FAULT_W_ONLY: u32 = 7;
const BOOT_FAULT_MEGAPAGE: u32 = 8;
const BOOT_FAULT_KFREE_UNALIGNED: u32 = 9;
const BOOT_FAULT_KFREE_KERNEL: u32 = 10;
const BOOT_FAULT_KFREE_PAST_RAM: u32 = 11;
/// Where `BOOT_FAULT_LOAD` loads from: below the boot ROM, nothing is mapped.
const UNMAPPED_ADDR: u32 = 0x100;

/// Where the vm self-test maps its alias page (`ALIAS_VA` in kernel/vmtest.c).
/// The test unmaps it again but leaves the second-level table in place.
const ALIAS_VA: u32 = 0x4000_0000;

/// mcause of a machine call.
const ECALL_FROM_S: u32 = grail::trap::Exception::EcallS as u32;
/// scause of a page fault on a load and on a store.
const LOAD_PAGE_FAULT: u32 = grail::trap::Exception::LoadPageFault as u32;
const STORE_PAGE_FAULT: u32 = grail::trap::Exception::StorePageFault as u32;
/// scause of an illegal instruction.
const ILLEGAL_INSTRUCTION: u32 = grail::trap::Exception::IllegalInstr as u32;

fn kernel_dir() -> PathBuf {
    repo_root().join("kernel")
}

fn committed_elf() -> Vec<u8> {
    let path = kernel_dir().join("kernel.elf");
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn assert_boots(elf: &[u8]) {
    let (r, out) = kernel::run_on_grail(elf, EXIT, BOOT_MAX_STEPS).unwrap();
    assert_eq!(kernel::mask_dbg_pc(&out), halted_output(), "raw output:\n{out}");
    assert_eq!(r, RunResult::Exited(0));
}

/// Loads the committed kernel without running it, with `EXIT` typed on the
/// UART so that a full run ends.
fn load_committed() -> (Machine, grail::elf::Elf) {
    load_committed_on(MachineConfig::default(), Some(kernel::typed(EXIT)))
}

/// Loads the committed kernel into a machine built from `config`, with the
/// UART reading `input` (nothing at all if `None`).
fn load_committed_on(config: MachineConfig, input: Option<Receiver<u8>>) -> (Machine, grail::elf::Elf) {
    let file = committed_elf();
    let elf = grail::elf::parse(&file).unwrap();
    let mut m = Machine::new(config).unwrap();
    m.cpu.bus.uart.capture = true;
    m.load_elf(&file).unwrap();
    if let Some(input) = input {
        m.set_uart_input(input);
    }
    (m, elf)
}

/// What the kernel has printed so far.
fn printed(m: &Machine) -> String {
    String::from_utf8_lossy(&m.cpu.bus.uart.captured).into_owned()
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
    let (r, _) = kernel::run_with_output(
        &committed_elf(),
        Some(BOOT_MAX_STEPS),
        kernel::typed(EXIT),
        uart.clone(),
        debug.clone(),
    )
    .unwrap();
    assert_eq!(r, RunResult::Exited(0));
    // The one xcal.dbg line goes to the debug sink; everything around it to the UART.
    let all = halted_output();
    let (before, rest) = all.split_at(all.find("[dbg]").unwrap());
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
        "user_vec",
        "user_return",
        "user_trap",
        "swtch",
        "scheduler",
        "proc_create",
        "elf_load",
        "copyin",
        "user_programs",
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
    for s in ["early_trap", "machine_vec", "kernel_vec", "user_vec"] {
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
    let out = printed(&m);
    assert_eq!(kernel::mask_dbg_pc(&out), halted_output());
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
        assert!(steps < TO_KMAIN_STEPS, "never reached kmain");
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
    let trap_init = sym(&elf, "trap_init");
    // Machine ticks before trap_init enables the kernel's tick, and after.
    let mut early_ticks = 0;
    let mut machine_ticks = Vec::new();
    let mut deadlines = Vec::new();
    let mut kernel_ticks = Vec::new();
    let mut external = 0;
    let mut steps_after_trap_init = None;
    let mut halts = 0;
    let probes = [sym(&elf, "probe_load_insn"), sym(&elf, "probe_store_insn")];
    let mut probe_faults = 0;
    let r = m.run(Some(BOOT_MAX_STEPS), |cpu| {
        if cpu.pc == trap_init {
            steps_after_trap_init = Some(0u64);
        }
        if let Some(steps) = &mut steps_after_trap_init {
            *steps += 1;
        }
        if cpu.pc == machine_vec {
            if cpu.csr.mcause == INTERRUPT_BIT | IRQ_M_TIMER {
                if steps_after_trap_init.is_none() {
                    early_ticks += 1;
                } else {
                    machine_ticks.push(cpu.bus.clint.mtime);
                }
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
            assert!(
                [LOAD_PAGE_FAULT, STORE_PAGE_FAULT].contains(&cpu.csr.scause),
                "{:#x}",
                cpu.csr.scause
            );
            probe_faults += 1;
        } else if cpu.pc == kernel_vec && cpu.csr.scause == INTERRUPT_BIT | IRQ_S_EXT {
            // The UART receiving the shell's `exit`.
            external += 1;
        } else if cpu.pc == kernel_vec {
            assert_eq!(cpu.csr.scause, INTERRUPT_BIT | IRQ_S_SOFT);
            // Ticks arrive with paging on.
            assert_ne!(cpu.csr.satp >> 31, 0, "tick before paging");
            kernel_ticks.push(cpu.bus.clint.mtime);
        }
    });
    assert_eq!(r, RunResult::Exited(0));
    assert_eq!(halts, 1);
    // All of `exit\n` was waiting before interrupts were on: one interrupt
    // drains it.
    assert_eq!(external, 1);
    // Unmapped load and store, stores to text and rodata.
    assert_eq!(probe_faults, 4);
    // Setting up memory takes a few intervals, so machine ticks fire before
    // trap_init. They leave SSIP pending, which trap_init drops: the kernel
    // then waits ten whole intervals, one machine tick each.
    assert!(early_ticks > 0, "memory setup took less than one interval");
    assert_eq!(machine_ticks.len(), 10, "{machine_ticks:?}");
    assert_eq!(kernel_ticks.len(), 10);
    // The deadlines are exactly one interval apart (machine_vec adds the
    // interval to mtimecmp, so ticks don't drift), and after trap_init every
    // tick lands in wfi, right on its deadline.
    for w in deadlines.windows(2) {
        assert_eq!(w[1] - w[0], TICK_INTERVAL, "{deadlines:?}");
    }
    for w in machine_ticks.windows(2) {
        assert_eq!(w[1] - w[0], TICK_INTERVAL, "{machine_ticks:?}");
    }
    // Each machine tick reaches the kernel right away, the same few
    // instructions later every time.
    let delays: Vec<u64> = kernel_ticks
        .iter()
        .zip(&machine_ticks)
        .map(|(s, m)| s - m)
        .collect();
    assert!(delays.iter().all(|&d| d == delays[0] && d < 100), "{delays:?}");
    assert_eq!(m.cpu.bus.peek(sym(&elf, "ticks")), Some(10));
    // wfi skips to the next tick: ten intervals of mtime pass in far fewer
    // instructions than one.
    assert!(m.cpu.bus.clint.mtime >= 10 * TICK_INTERVAL);
    let steps = steps_after_trap_init.unwrap();
    assert!(steps < TICK_INTERVAL, "{steps} steps");
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
    // Up to the first process. A tick that short leaves a process no time to
    // run (taking one and switching back costs more than 211 instructions),
    // so the normal interval is back for the shell.
    step_until(
        &mut m,
        BOOT_MAX_STEPS,
        |cpu| {
            if cpu.pc == machine_vec
                && cpu.csr.mcause == INTERRUPT_BIT | IRQ_M_TIMER
                && !idle.contains(&cpu.csr.mepc)
            {
                machine_busy += 1;
            }
            if cpu.pc == kernel_vec && !idle.contains(&cpu.csr.sepc) {
                kernel_busy += 1;
            }
        },
        |m| m.cpu.pc == sym(&elf, "proc_run_init"),
    );
    m.cpu
        .bus
        .load(scratch + SCRATCH_INTERVAL, &(TICK_INTERVAL as u32).to_le_bytes())
        .unwrap();
    let r = m.run(Some(BOOT_MAX_STEPS), |_| {});
    let out = printed(&m);
    assert_eq!(kernel::mask_dbg_pc(&out), halted_output(), "raw output:\n{out}");
    assert_eq!(r, RunResult::Exited(0));
    // Fewer kernel ticks: until trap_init, the machine ticks only pile up in
    // the one SSIP bit, which trap_init then drops.
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
    assert_eq!(v["scause"], LOAD_PAGE_FAULT);
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
    let (mut m, elf) = load_committed();
    let kmain = sym(&elf, "kmain");
    m.cpu
        .bus
        .load(sym(&elf, "boot_fault"), &BOOT_FAULT_STORE_TEXT.to_le_bytes())
        .unwrap();
    // Grail sets A when an access goes through the PTE: a fetch or a load on
    // kmain's page with paging on, before the store that faults (which sets
    // neither A nor D). Whether one happens depends on the layout (kmain may
    // start near the end of a page whose code all runs before paging), so
    // watch for one.
    let page = kmain >> 12;
    let (mut accessed, mut faulted) = (false, false);
    let r = m.run(Some(BOOT_MAX_STEPS), |cpu| {
        let access = mem_access(cpu);
        if access
            .as_ref()
            .is_some_and(|a| a.load_rd.is_none() && a.addr == kmain)
        {
            faulted = true;
        }
        if faulted || cpu.mode == Mode::M || cpu.csr.satp >> 31 == 0 {
            return;
        }
        let load = access.is_some_and(|a| a.load_rd.is_some() && a.addr >> 12 == page);
        accessed |= cpu.pc >> 12 == page || load;
    });
    let out = String::from_utf8_lossy(&m.cpu.bus.uart.captured).into_owned();
    assert_eq!(r, RunResult::Exited(PANIC_EXIT), "output:\n{out}");
    assert!(faulted, "no store to kmain");
    let before = &out[..out.find("panic: ").unwrap()];
    assert_eq!(kernel::mask_dbg_pc(before), BOOT_OUTPUT);

    let (cause, v) = parse_panic(&out);
    assert_eq!(cause, "store page fault");
    assert_eq!(v["scause"], STORE_PAGE_FAULT);
    assert_eq!(v["stval"], kmain);
    // Identity mapped, R+X, never dirty, accessed as watched above.
    let pte = (kmain >> 12) << 10 | PTE_V | PTE_R | PTE_X;
    let (pte, flags) = if accessed {
        (pte | PTE_A, "-A--X-RV")
    } else {
        (pte, "----X-RV")
    };
    let line = format!("\n  va {kmain:#010x} maps to pa {kmain:#010x}, pte {pte:#010x} {flags}\n");
    assert!(out.contains(&line), "want{line}in:\n{out}");
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

/// Runs the kernel with `boot_fault` = `fault`, one of the memory API misuses,
/// and checks that it panics after the normal boot output, printing `want`.
fn assert_misuse_panics(fault: u32, want: &str) {
    let (_, _, r, out) = run_with_fault(fault);
    assert_eq!(r, RunResult::Exited(PANIC_EXIT), "output:\n{out}");
    let masked = kernel::mask_dbg_pc(&out);
    let rest = masked
        .strip_prefix(BOOT_OUTPUT)
        .unwrap_or_else(|| panic!("output:\n{out}"));
    assert_eq!(rest, want);
}

#[test]
fn map_pages_panics_on_a_remap() {
    assert_misuse_panics(
        BOOT_FAULT_REMAP,
        "map_pages: 0x80000000 is already mapped\npanic: map_pages: remap\n",
    );
}

#[test]
fn unmap_pages_panics_on_an_unmapped_page() {
    // Page 0's second-level table exists (the test device is in the same
    // 4 MiB), so this is the invalid-PTE check, not the missing-table one.
    assert_misuse_panics(
        BOOT_FAULT_UNMAP,
        "unmap_pages: 0x00000000 is not mapped\npanic: unmap_pages: not mapped\n",
    );
}

#[test]
fn map_pages_panics_on_write_without_read() {
    assert_misuse_panics(BOOT_FAULT_W_ONLY, "panic: map_pages: bad permissions\n");
}

#[test]
fn walk_panics_on_a_megapage() {
    assert_misuse_panics(BOOT_FAULT_MEGAPAGE, "panic: walk: unexpected megapage\n");
}

#[test]
fn kfree_panics_on_a_bad_page() {
    let (_, elf) = load_committed();
    let text = sym(&elf, "__text_start");
    let ram_end = text + grail::DEFAULT_RAM_SIZE as u32;
    for (fault, page) in [
        (BOOT_FAULT_KFREE_KERNEL, text),
        (BOOT_FAULT_KFREE_PAST_RAM, ram_end),
    ] {
        assert_misuse_panics(fault, &format!("kfree: bad page {page:#010x}\npanic: kfree\n"));
    }
    // A free page plus 4: whichever page kalloc handed out.
    let (_, _, r, out) = run_with_fault(BOOT_FAULT_KFREE_UNALIGNED);
    assert_eq!(r, RunResult::Exited(PANIC_EXIT), "output:\n{out}");
    let rest = &out[out.find("kfree: ").unwrap()..];
    let addr = rest
        .strip_prefix("kfree: bad page 0x")
        .and_then(|r| r.strip_suffix("\npanic: kfree\n"))
        .unwrap_or_else(|| panic!("{rest}"));
    let addr = u32::from_str_radix(addr, 16).unwrap();
    assert_eq!(addr % PAGE, 4, "{addr:#x}");
    assert!(addr > sym(&elf, "__kernel_end") && addr < ram_end, "{addr:#x}");
}

// ---- Memory: the page allocator and the kernel page table ----

const PAGE: u32 = 4096;

// Sv32 PTE bits.
const PTE_V: u32 = 1 << 0;
const PTE_R: u32 = 1 << 1;
const PTE_W: u32 = 1 << 2;
const PTE_X: u32 = 1 << 3;
const PTE_U: u32 = 1 << 4;
const PTE_G: u32 = 1 << 5;
const PTE_A: u32 = 1 << 6;
/// The bits of a PTE the kernel chooses; A and D are Grail's, and RSW (9:8)
/// is for software, which the kernel doesn't use yet.
const PTE_KERNEL_BITS: u32 = PTE_V | PTE_R | PTE_W | PTE_X | PTE_U | PTE_G;
const PTE_RSW: u32 = 0x300;

/// Boots the committed kernel on a machine with `ram_mib` MiB of RAM and
/// `extra_devices` added to its sysinfo device table, to the end. Returns the
/// machine, the ELF and what the kernel printed.
fn boot_with_ram(ram_mib: usize, extra_devices: &[DeviceEntry]) -> (Machine, grail::elf::Elf, String) {
    let file = committed_elf();
    let elf = grail::elf::parse(&file).unwrap();
    let mut m = Machine::with_ram(ram_mib << 20);
    if !extra_devices.is_empty() {
        let mut devices = m.cpu.bus.sysinfo.devices().to_vec();
        devices.extend_from_slice(extra_devices);
        m.cpu.bus.sysinfo = Sysinfo::new(m.config(), devices);
    }
    m.cpu.bus.uart.capture = true;
    m.load_elf(&file).unwrap();
    m.set_uart_input(kernel::typed(EXIT));
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
        // A pointer to the next level: D, A, U and G are reserved there, and
        // the kernel sets none of them (nor RSW).
        assert_eq!(pte & 0x3ff, PTE_V, "root[{i}] = {pte:#x}");
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

/// Checks the kernel page table after a boot with `ram_mib` MiB of RAM and
/// `extra_devices` (see `boot_with_ram`), and the free page count.
fn check_kernel_memory(ram_mib: usize, extra_devices: &[DeviceEntry]) {
    let (m, elf, out) = boot_with_ram(ram_mib, extra_devices);
    assert!(
        out.contains(&format!(
            "ram: {ram_mib} MiB at 0x80000000\npaging: on (Sv32)\nvm self-test: 26/26 passed\n"
        )),
        "{out}"
    );
    let satp = m.cpu.csr.satp;
    assert_ne!(satp >> 31, 0, "paging is off");
    let ram_end = 0x8000_0000u64 + (ram_mib as u64) * (1 << 20);
    let kernel_end = sym(&elf, "__kernel_end");
    let root = (satp & 0x3f_ffff) * PAGE;
    assert!(root >= kernel_end && u64::from(root) < ram_end, "root {root:#x}");

    // What the kernel should map, identity, page by page: the PTE bits the
    // kernel sets (no U, no G).
    let mut want = std::collections::BTreeMap::new();
    let mut map = |start: u64, end: u64, perm: u32| {
        for page in (start..end).step_by(PAGE as usize) {
            assert!(
                want.insert(page as u32, perm | PTE_V).is_none(),
                "{page:#x} twice"
            );
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
    // Every device in the sysinfo table, whole pages. Devices may share a
    // page with each other, but not with RAM.
    let ram_pages = want.len();
    for dev in m.cpu.bus.sysinfo.devices() {
        let start = u64::from(dev.base) & !u64::from(PAGE - 1);
        let end = (u64::from(dev.base) + u64::from(dev.size)).next_multiple_of(PAGE.into());
        for page in (start..end).step_by(PAGE as usize) {
            assert!(page < u64::from(text), "device page {page:#x} in RAM");
            want.insert(page as u32, PTE_R | PTE_W | PTE_V);
        }
    }
    assert!(want.len() > ram_pages);

    let (leaves, tables) = page_table(&m, satp);
    let mut got = std::collections::BTreeMap::new();
    for &(va, pte) in &leaves {
        assert_eq!((pte >> 10) * PAGE, va, "{va:#x} not identity mapped");
        assert_eq!(pte & PTE_RSW, 0, "{va:#x}: {pte:#x}");
        got.insert(va, pte & PTE_KERNEL_BITS);
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
    check_kernel_memory(128, &[]);
}

#[test]
fn devices_may_share_a_page() {
    // The sysinfo table doesn't promise page-aligned devices (the UART is 256
    // bytes), so the kernel must map a page two devices share only once.
    // These extra devices only need mapping: the kernel never touches them.
    let dev = |kind, base, size| DeviceEntry {
        kind,
        base,
        size,
        irq: 0,
    };
    check_kernel_memory(
        128,
        &[
            // A second UART in the first one's page.
            dev(
                DeviceType::Uart,
                grail::bus::UART_BASE + 0x100,
                grail::bus::UART_SIZE,
            ),
            // Straddling the keyboard's page and the sysinfo block's.
            dev(DeviceType::Keyboard, grail::bus::SYSINFO_BASE - 0x80, 0x100),
            // Straddling two pages in a gap of the memory map, the second
            // of them shared with a later entry.
            dev(DeviceType::TestFinisher, 0x2000_0800, 0x1000),
            dev(DeviceType::TestFinisher, 0x2000_1400, 0x10),
        ],
    );
}

#[test]
fn allocator_follows_the_ram_size() {
    // RAM comes from sysinfo: the smallest machine, and one whose RAM ends
    // inside a 4 MiB region.
    for mib in [1, 6] {
        check_kernel_memory(mib, &[]);
    }
}

#[test]
fn allocator_handles_ram_up_to_the_top_of_the_address_space() {
    // The largest machine: RAM ends at 2^32, which wraps to 0 in a u32.
    check_kernel_memory(2048, &[]);
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

// ---- The console and the shell ----

/// The UART registers the tests look for (spec section 7).
const UART_RBR: u32 = 0;
const UART_IER: u32 = 1;
const UART_LSR: u32 = 5;
/// IER bit 0: interrupt while received data is available.
const UART_IER_ERBFI: u32 = 1;

/// PLIC registers (spec section 7.1) for source `src` / context `ctx`.
fn plic_priority(src: u32) -> u32 {
    PLIC_BASE + 4 * src
}
fn plic_enable(ctx: u32) -> u32 {
    PLIC_BASE + 0x2000 + 0x80 * ctx
}
fn plic_threshold(ctx: u32) -> u32 {
    PLIC_BASE + 0x20_0000 + 0x1000 * ctx
}

/// The size of the kernel's receive ring buffer (`UART_RX_SIZE` in
/// kernel/include/uart.h).
const UART_RX_SIZE: usize = 128;

/// What the shell's `help` prints (kernel/user/sh.c).
const HELP: &str = "\
help            this list
exit [N]        leave the shell, with status N or the last program's
wait            wait for every background program
cd [DIR]        change the working directory, to / without DIR
PROG [ARG...]   run /bin/PROG (e.g. ls, cat, echo, wc, sh), or the program at PROG if it has a /
CMD < FILE      read the input from FILE
CMD > FILE      write the output to FILE
CMD | CMD       send the first command's output to the second one
CMD & CMD       run the first command in the background
";

/// `out` with the number after each `ticks: ` replaced by `N`, after checking
/// the numbers are at least the ten boot ticks and never go down.
fn mask_ticks(out: &str) -> String {
    let mut masked = String::new();
    let mut last = 10;
    let mut rest = out;
    while let Some(at) = rest.find("ticks: ") {
        let (before, after) = rest.split_at(at + "ticks: ".len());
        masked.push_str(before);
        let digits = after.bytes().take_while(u8::is_ascii_digit).count();
        let n: u32 = after[..digits]
            .parse()
            .unwrap_or_else(|_| panic!("no number after `ticks: ` in:\n{out}"));
        assert!(n >= last, "ticks went from {last} to {n}:\n{out}");
        last = n;
        masked.push('N');
        rest = &after[digits..];
    }
    masked.push_str(rest);
    masked
}

/// A memory access by the instruction about to run.
struct Access {
    addr: u32,
    /// The register a load writes; `None` for a store.
    load_rd: Option<u8>,
    /// The value a store writes.
    value: u32,
}

/// The load or store the instruction about to run makes, if it is one. Paging
/// is off, so the address is physical.
fn mem_access(cpu: &grail::cpu::Cpu) -> Option<Access> {
    let word = cpu.bus.peek(cpu.pc)?;
    let (base, imm, load_rd, value) = match excalibur::decode(word) {
        excalibur::Inst::Load { rd, rs1, imm, .. } => (rs1, imm, Some(rd), 0),
        excalibur::Inst::Store { rs1, rs2, imm, .. } => (rs1, imm, None, cpu.x[rs2 as usize]),
        _ => return None,
    };
    let addr = cpu.x[base as usize].wrapping_add(imm as u32);
    Some(Access { addr, load_rd, value })
}

/// A UART register access by the instruction about to run: (offset, store?,
/// stored value).
fn uart_access(cpu: &grail::cpu::Cpu) -> Option<(u32, bool, u32)> {
    let a = mem_access(cpu)?;
    (UART_BASE..UART_BASE + 8)
        .contains(&a.addr)
        .then_some((a.addr - UART_BASE, a.load_rd.is_none(), a.value))
}

/// The addresses of function `name` in the committed kernel, from its symbol's
/// value and size.
fn function(name: &str) -> std::ops::Range<u32> {
    let file = committed_elf();
    let s = elf32::Elf::parse(&file)
        .unwrap()
        .symbols()
        .unwrap()
        .into_iter()
        .find(|s| s.name == name)
        .unwrap_or_else(|| panic!("no symbol {name}"));
    assert!(s.size > 0, "{name} has no size");
    s.value..s.value + s.size
}

/// Runs the committed kernel with `input` typed on the UART. Returns how the
/// run ended and what was printed from the shell's banner on.
fn run_shell(input: &[u8]) -> (RunResult, String) {
    let (mut m, _) = load_committed_on(MachineConfig::default(), Some(kernel::typed(input)));
    let r = m.run(Some(BOOT_MAX_STEPS), |_| {});
    let out = printed(&m);
    // Everything before the shell is the usual boot.
    let at = out
        .find(SHELL_BANNER)
        .unwrap_or_else(|| panic!("the shell never started:\n{out}"));
    assert_eq!(kernel::mask_dbg_pc(&out[..at]), BOOT_OUTPUT);
    (r, out[at..].to_string())
}

/// Steps `m` until `done` holds, at most `max` steps. Calls `each` before
/// every step. Returns the steps taken.
fn step_until(
    m: &mut Machine,
    max: u64,
    mut each: impl FnMut(&grail::cpu::Cpu),
    done: impl Fn(&Machine) -> bool,
) -> u64 {
    for n in 0..max {
        if done(m) {
            return n;
        }
        each(&m.cpu);
        m.cpu.step();
    }
    panic!("gave up after {max} steps; output:\n{}", printed(m));
}

#[test]
fn shell_runs_its_commands() {
    let (r, out) = run_shell(b"help\nticks\nfrobnicate\n\n   \n  ticks \t\necho  a\tb   c \nexit\n");
    assert_eq!(r, RunResult::Exited(0));
    // The console echoes each line as the shell reads it, then the shell or
    // the program answers. Tabs are control characters: the console drops
    // them.
    let want = format!(
        "{SHELL_BANNER}\
         {PROMPT}help\n{HELP}\
         {PROMPT}ticks\nticks: N\n\
         {PROMPT}frobnicate\nsh: frobnicate: no such program\n\
         {PROMPT}\n\
         {PROMPT}   \n\
         {PROMPT}  ticks \nticks: N\n\
         {PROMPT}echo  ab   c \nab c\n\
         {PROMPT}exit\n"
    );
    assert_eq!(mask_ticks(&out), want);
}

#[test]
fn console_edits_lines() {
    let long = "x".repeat(200);
    let input = format!(
        // Backspace and DEL on an empty line do nothing; then both erase.
        "\x08\x7fticky\x7fs\x08\x08ks\n\
         {long}\n\
         ticks\r\n\r\r\
         \x01ex\ti\x1bt\n"
    );
    let (r, out) = run_shell(input.as_bytes());
    assert_eq!(r, RunResult::Exited(0));
    // Lines hold at most 127 characters and the newline (CONSOLE_LINE_MAX in
    // kernel/include/console.h); the rest is neither echoed nor kept.
    let kept = "x".repeat(127);
    let want = format!(
        "{SHELL_BANNER}\
         {PROMPT}ticky\x08 \x08s\x08 \x08\x08 \x08ks\nticks: N\n\
         {PROMPT}{kept}\nsh: {kept}: no such program\n\
         {PROMPT}ticks\nticks: N\n\
         {PROMPT}\n\
         {PROMPT}\n\
         {PROMPT}exit\n"
    );
    assert_eq!(mask_ticks(&out), want);
}

#[test]
fn end_of_input_ends_the_shell() {
    // Ctrl-D on an empty line: the read returns 0, and the shell exits with
    // the last program's status. Mid-line it ends the line without a
    // newline, so the shell sees `echo hi` and the program's output follows
    // the echo directly.
    let (r, out) = run_shell(b"echo hi\x04badload\n\x04");
    assert_eq!(r, RunResult::Exited(0xff), "{out}");
    assert!(
        out.starts_with(&format!("{SHELL_BANNER}{PROMPT}echo hihi\n{PROMPT}badload\n")),
        "{out}"
    );
    assert!(out.ends_with(&format!("badload: exit -1\n{PROMPT}")), "{out}");
}

#[test]
fn exit_status_becomes_the_machine_exit_code() {
    for (input, code) in [
        (&b"exit 7\n"[..], 7),
        (b"exit -2\n", 0xfe),
        (b"exit 0\n", 0),
        // No number: the last foreground program's status, -1 for a kill.
        (b"badload\nexit\n", 0xff),
        (b"badload\nhello\nexit\n", 0),
        (b"echo a & badload\nexit\n", 0xff),
    ] {
        let (r, out) = run_shell(input);
        assert_eq!(
            r,
            RunResult::Exited(code),
            "{}:\n{out}",
            String::from_utf8_lossy(input)
        );
    }
}

#[test]
fn shell_takes_more_input_than_the_ring_buffer_holds() {
    // 40 commands, typed all at once: 245 bytes for a 128-byte buffer. When it
    // is full the driver turns the receive interrupt off rather than drop
    // bytes, and turns it back on once the console has taken half of it.
    let input = format!("{}exit\n", "ticks\n".repeat(40));
    assert!(input.len() > UART_RX_SIZE + UART_RX_SIZE / 2);
    assert!(input.len() < 2 * UART_RX_SIZE);
    let (mut m, elf) = load_committed_on(MachineConfig::default(), Some(kernel::typed(input.as_bytes())));
    let trap_init = sym(&elf, "trap_init");
    let mut booted = false;
    let mut ier_writes = Vec::new();
    let r = m.run(Some(RUN_MAX_STEPS), |cpu| {
        booted |= cpu.pc == trap_init;
        if let Some((UART_IER, true, v)) = uart_access(cpu) {
            if booted {
                ier_writes.push(v);
            }
        }
    });
    assert_eq!(r, RunResult::Exited(0));
    let out = printed(&m);
    let shell = &out[out.find(SHELL_BANNER).unwrap()..];
    let want = format!(
        "{SHELL_BANNER}{}{PROMPT}exit\n",
        format!("{PROMPT}ticks\nticks: N\n").repeat(40)
    );
    assert_eq!(mask_ticks(shell), want);
    // Off when the buffer filled (128 bytes in), on again at half (64 taken),
    // off when 64 more filled it (192 in), on again at half: the last 53
    // bytes fit.
    assert_eq!(
        ier_writes,
        [0, UART_IER_ERBFI, 0, UART_IER_ERBFI],
        "{ier_writes:?}"
    );
}

#[test]
fn plic_routes_the_uart_to_the_kernel() {
    // No input: the kernel waits at the prompt.
    let (mut m, _) = load_committed_on(MachineConfig::default(), None);
    step_until(&mut m, BOOT_MAX_STEPS, |_| {}, |m| printed(m).ends_with(PROMPT));
    let mut reg = |addr| m.cpu.bus.read(addr, 4).unwrap();
    // Source 1 (the UART) at priority 1, enabled for context 1 (S-mode) only,
    // whose threshold of 0 lets it through.
    assert_eq!(reg(plic_priority(UART_IRQ)), 1);
    assert_eq!(reg(plic_enable(1)), 1 << UART_IRQ);
    assert_eq!(reg(plic_threshold(1)), 0);
    assert_eq!(reg(plic_enable(0)), 0);
    // sie.SEIE on (with the tick's SSIE); M-mode external interrupts stay off.
    let mie = m.cpu.csr.mie;
    assert_eq!(
        mie,
        (1 << IRQ_M_TIMER) | (1 << IRQ_S_SOFT) | (1 << IRQ_S_EXT),
        "mie {mie:#x}"
    );
    assert_eq!(mie & (1 << IRQ_M_EXT), 0);
    // The UART's receive interrupt is on.
    assert_eq!(m.cpu.bus.read(UART_BASE + UART_IER, 1), Ok(UART_IER_ERBFI));
}

#[test]
fn console_sleeps_until_input_and_never_polls_rbr() {
    // The shell prints its prompt with nothing typed and sleeps in read, so
    // the scheduler idles; then a command arrives. Input only arrives through
    // the interrupt: no UART register is read while the kernel waits, and RBR
    // only ever by the interrupt handler.
    let (tx, rx) = std::sync::mpsc::channel();
    let (mut m, elf) = load_committed_on(MachineConfig::default(), Some(rx));
    let ticks = sym(&elf, "ticks");
    let tick_count = |m: &Machine| m.cpu.bus.peek(ticks).unwrap();
    let uart_intr = function("uart_intr");
    let kernel_vec = sym(&elf, "kernel_vec");
    let wfi = sym(&elf, "wfi");

    step_until(&mut m, BOOT_MAX_STEPS, |_| {}, |m| printed(m).ends_with(PROMPT));
    assert_eq!(tick_count(&m), 10);

    // Fifty idle ticks: the hart spends them in wfi (Grail skips the wait to
    // the next tick), waking only for the tick itself, a couple of hundred
    // instructions each. Polling would run all the 1 000 000 steps between.
    let mut uart_reads = 0;
    let mut wfis = 0;
    let steps = step_until(
        &mut m,
        BOOT_MAX_STEPS,
        |cpu| {
            if matches!(uart_access(cpu), Some((_, false, _))) {
                uart_reads += 1;
            }
            if cpu.pc == wfi {
                wfis += 1;
            }
        },
        |m| tick_count(m) == 60,
    );
    assert_eq!(uart_reads, 0, "the kernel polls the UART while idle");
    assert!((50..=51).contains(&wfis), "{wfis} wfi for 50 ticks");
    assert!(steps < 50 * 500, "{steps} steps for 50 idle ticks");

    // Type a command: the UART interrupt wakes the hart at once and the
    // console wakes the shell (no tick passes before the ticks program
    // answers), and only uart_intr reads RBR.
    let before = printed(&m).len();
    for &b in b"ticks\n" {
        tx.send(b).unwrap();
    }
    let mut rbr_reads = Vec::new();
    let mut lsr_reads = 0;
    let mut external = 0;
    step_until(
        &mut m,
        BOOT_MAX_STEPS,
        |cpu| {
            match uart_access(cpu) {
                Some((UART_RBR, false, _)) => rbr_reads.push(cpu.pc),
                Some((UART_LSR, false, _)) => lsr_reads += 1,
                _ => {}
            }
            if cpu.pc == kernel_vec && cpu.csr.scause == INTERRUPT_BIT | IRQ_S_EXT {
                external += 1;
            }
        },
        |m| printed(m).ends_with(PROMPT) && printed(m).len() > before,
    );
    assert_eq!(&printed(&m)[before..], format!("ticks\nticks: 60\n{PROMPT}"));
    assert_eq!(tick_count(&m), 60);
    assert_eq!(rbr_reads.len(), 6, "one read per byte");
    assert!(
        rbr_reads.iter().all(|pc| uart_intr.contains(pc)),
        "RBR read outside uart_intr ({uart_intr:x?}): {rbr_reads:x?}"
    );
    // The handler checks LSR.DR once per byte and once more to see it clear,
    // and uart_putc checks THRE once per character printed.
    assert!(lsr_reads > 6, "{lsr_reads}");
    assert!(external >= 1);

    for &b in EXIT {
        tx.send(b).unwrap();
    }
    assert_eq!(m.run(Some(BOOT_MAX_STEPS), |_| {}), RunResult::Exited(0));
    assert!(printed(&m).ends_with(&format!("{PROMPT}exit\n")));
}

#[test]
fn kernel_reads_the_timebase_from_sysinfo() {
    // A machine reporting a 2 MHz clock: the 10 Hz tick is 200 000 mtime ticks.
    const HZ: u32 = 2_000_000;
    let config = MachineConfig {
        nominal_hz: HZ,
        ..MachineConfig::default()
    };
    let (mut m, elf) = load_committed_on(config, Some(kernel::typed(EXIT)));
    step_to(&mut m, sym(&elf, "kmain"));
    let interval = HZ / 10;
    assert_eq!(
        m.cpu.bus.peek(sym(&elf, "machine_scratch") + SCRATCH_INTERVAL),
        Some(interval)
    );
    let kernel_vec = sym(&elf, "kernel_vec");
    let mut kernel_ticks = Vec::new();
    let r = m.run(Some(BOOT_MAX_STEPS), |cpu| {
        if cpu.pc == kernel_vec && cpu.csr.scause == INTERRUPT_BIT | IRQ_S_SOFT {
            kernel_ticks.push(cpu.bus.clint.mtime);
        }
    });
    assert_eq!(r, RunResult::Exited(0));
    let want = halted_output().replace("10000000 Hz timebase", "2000000 Hz timebase");
    assert_eq!(kernel::mask_dbg_pc(&printed(&m)), want);
    // Ten ticks, each a 200 000-tick interval. (Setting up memory takes a
    // few intervals before them, so the total mtime says little.)
    assert_eq!(kernel_ticks.len(), 10, "{kernel_ticks:?}");
    for w in kernel_ticks.windows(2) {
        assert_eq!(w[1] - w[0], u64::from(interval), "{kernel_ticks:?}");
    }
}

#[test]
fn timer_interval_is_never_zero() {
    // A timebase below the tick rate would give an interval of 0, which
    // would never move mtimecmp; the kernel uses 1 instead. (A machine that
    // slow can't run the kernel anyway: the timer fires faster than
    // machine_vec handles it. So only timer_init is checked.)
    let config = MachineConfig {
        nominal_hz: 5,
        ..MachineConfig::default()
    };
    let (mut m, elf) = load_committed_on(config, None);
    let interval = sym(&elf, "machine_scratch") + SCRATCH_INTERVAL;
    step_until(
        &mut m,
        TO_KMAIN_STEPS,
        |_| {},
        |m| m.cpu.bus.peek(interval) != Some(0),
    );
    assert_eq!(m.cpu.bus.peek(interval), Some(1));
}

#[test]
fn timebase_is_read_once_in_m_mode() {
    // timebase_hz() reads sysinfo on its first call, from timer_init in
    // M-mode, and keeps the value: S-mode code (kmain prints the timebase)
    // never reads the timebase again. The S-mode reads are kinit's (magic,
    // version, RAM base and size) and kvminit's (the device table).
    let (mut m, _) = load_committed();
    let mut reads = Vec::new();
    let r = m.run(Some(BOOT_MAX_STEPS), |cpu| {
        if let Some(a) = mem_access(cpu) {
            if (SYSINFO_BASE..SYSINFO_BASE + SYSINFO_SIZE).contains(&a.addr) {
                assert!(a.load_rd.is_some(), "store to sysinfo at {:#x}", cpu.pc);
                reads.push((a.addr - SYSINFO_BASE, cpu.mode));
            }
        }
    });
    assert_eq!(r, RunResult::Exited(0));
    let m_mode: Vec<_> = reads.iter().filter(|r| r.1 == Mode::M).copied().collect();
    assert_eq!(m_mode, [(REG_MAGIC, Mode::M), (REG_TIMEBASE_HZ, Mode::M)]);
    assert!(
        !reads.contains(&(REG_TIMEBASE_HZ, Mode::S)),
        "timebase read again in S-mode: {reads:?}"
    );
}

/// Boots the committed kernel on a machine reporting a 2 MHz clock, but with
/// sysinfo register `reg` reading as 0. Returns the sysinfo registers read
/// before kmain, the timer interval timer_init stored and the whole output.
fn boot_with_sysinfo_reg_zero(reg: u32) -> (Vec<u32>, u32, String) {
    let config = MachineConfig {
        nominal_hz: 2_000_000,
        ..MachineConfig::default()
    };
    let (mut m, elf) = load_committed_on(config, Some(kernel::typed(EXIT)));
    let kmain = sym(&elf, "kmain");
    let mut reads = Vec::new();
    for _ in 0..TO_KMAIN_STEPS {
        if m.cpu.pc == kmain {
            break;
        }
        let read = mem_access(&m.cpu)
            .filter(|a| (SYSINFO_BASE..SYSINFO_BASE + SYSINFO_SIZE).contains(&a.addr))
            .and_then(|a| Some((a.addr - SYSINFO_BASE, a.load_rd?)));
        m.cpu.step();
        if let Some((offset, rd)) = read {
            reads.push(offset);
            if offset == reg {
                m.cpu.x[rd as usize] = 0;
            }
        }
    }
    assert_eq!(m.cpu.pc, kmain, "never reached kmain");
    let interval = m
        .cpu
        .bus
        .peek(sym(&elf, "machine_scratch") + SCRATCH_INTERVAL)
        .unwrap();
    assert_eq!(m.run(Some(BOOT_MAX_STEPS), |_| {}), RunResult::Exited(0));
    (reads, interval, kernel::mask_dbg_pc(&printed(&m)))
}

#[test]
fn timebase_falls_back_to_nominal_without_sysinfo() {
    // SYS_MAGIC is wrong: no sysinfo, so SYS_TIMEBASE_HZ is not read and the
    // timebase is the nominal 10 MHz, not the machine's 2 MHz.
    let (reads, interval, out) = boot_with_sysinfo_reg_zero(REG_MAGIC);
    assert_eq!(reads, [REG_MAGIC]);
    assert_eq!(u64::from(interval), TICK_INTERVAL);
    assert_eq!(out, halted_output());
}

#[test]
fn timebase_falls_back_to_nominal_when_sysinfo_reports_zero() {
    let (reads, interval, out) = boot_with_sysinfo_reg_zero(REG_TIMEBASE_HZ);
    assert_eq!(reads, [REG_MAGIC, REG_TIMEBASE_HZ]);
    assert_eq!(u64::from(interval), TICK_INTERVAL);
    assert_eq!(out, halted_output());
}

#[test]
fn input_arriving_just_before_wfi_wakes_the_console() {
    // The scheduler turns interrupts off from its search for a runnable
    // process until wfi. A byte that arrives in between is then still pending
    // when wfi runs, so wfi returns at once, the console wakes the shell and
    // the answer comes within the same tick. With interrupts on, the byte's
    // interrupt would be taken before wfi, which would then sleep until the
    // next tick.
    let (tx, rx) = std::sync::mpsc::channel();
    let (mut m, elf) = load_committed_on(MachineConfig::default(), Some(rx));
    let ticks = sym(&elf, "ticks");
    let tick_count = |m: &Machine| m.cpu.bus.peek(ticks).unwrap();
    let wfi = sym(&elf, "wfi");

    step_until(&mut m, BOOT_MAX_STEPS, |_| {}, |m| printed(m).ends_with(PROMPT));
    // The shell sleeps in read; the scheduler found nothing to run and is
    // about to idle.
    step_until(&mut m, 10_000, |_| {}, |m| m.cpu.pc == wfi);
    let n = tick_count(&m);
    let before = printed(&m).len();
    for &b in b"ticks\n" {
        tx.send(b).unwrap();
    }
    step_until(
        &mut m,
        BOOT_MAX_STEPS,
        |_| {},
        |m| printed(m).ends_with(PROMPT) && printed(m).len() > before,
    );
    assert_eq!(&printed(&m)[before..], format!("ticks\nticks: {n}\n{PROMPT}"));
    assert_eq!(tick_count(&m), n);
}

/// Every C file and header in the kernel and its user programs, with its
/// name.
fn kernel_c_sources() -> Vec<(String, String)> {
    let mut files = Vec::new();
    for dir in [
        kernel_dir(),
        kernel_dir().join("include"),
        kernel_dir().join("user"),
    ] {
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
    // so Merlin's C subset (#15) can compile it later: the kernel and the
    // user programs alike.
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
    // The .s files must use only what merlin-as understands.
    let sources = kernel::sources(&kernel_dir()).unwrap();
    let asm: Vec<_> = sources.iter().filter(|s| s.lang == Lang::Asm).collect();
    assert!(!asm.is_empty());
    for s in asm {
        let text = std::fs::read_to_string(kernel_dir().join(&s.name)).unwrap();
        if let Err(errs) = merlin_as::assemble(&text) {
            let errs: Vec<String> = errs.iter().map(|e| format!("{}: {e}", s.name)).collect();
            panic!("merlin-as rejects kernel assembly:\n{}", errs.join("\n"));
        }
    }
}

// ---- Processes: U-mode, page tables, the scheduler, the system calls ----

/// The top of user space and the stack page under it (kernel/include/vm.h).
const USER_TOP: u32 = 0x40_0000;
const USER_STACK: u32 = USER_TOP - PAGE;
/// Where program segments must end: below the guard page under the stack.
const USER_LOAD_TOP: u32 = USER_STACK - PAGE;
/// Where user programs start (kernel/user/user.ld): crt0's `_start`.
const USER_TEXT: u32 = 0x1000;
/// `sizeof(struct trap_frame)` (kernel/include/trap.h).
const TRAP_FRAME_SIZE: u32 = 144;
/// Process slots (`NPROC`, kernel/include/proc.h).
const NPROC: usize = 8;
/// Offset of `name` in `struct proc` (kernel/include/proc.h).
const PROC_NAME: u32 = 8;
/// scause of an ecall from U-mode.
const ECALL_FROM_U: u32 = grail::trap::Exception::EcallU as u32;
/// The exit system call's number (`SYS_exit`, kernel/include/syscall.h).
const SYS_EXIT: u32 = 1;
/// A boot plus a few programs: ping and pong run about two million
/// instructions each, regs three.
const RUN_MAX_STEPS: u64 = 60_000_000;

/// The embedded programs, by name.
fn program_names() -> Vec<String> {
    xtask::userprogs::program_names(&xtask::userprogs::user_dir(&repo_root())).unwrap()
}

/// A fresh build of user program `name`, as the kernel embeds it.
fn user_program(name: &str) -> xtask::userprogs::Program {
    xtask::userprogs::build_named(&xtask::userprogs::user_dir(&repo_root()), name).unwrap()
}

/// The value of symbol `name` in user program `elf`.
fn user_sym(elf: &[u8], name: &str) -> u32 {
    elf32::Elf::parse(elf)
        .unwrap()
        .symbols()
        .unwrap()
        .into_iter()
        .find(|s| s.name == name)
        .unwrap_or_else(|| panic!("no symbol {name}"))
        .value
}

/// The user pages a program's ELF image asks for, as (va, PTE permissions),
/// then its stack page: what the process's page table must map below
/// USER_TOP.
fn expected_user_pages(elf: &[u8]) -> Vec<(u32, u32)> {
    let parsed = elf32::Elf::parse(elf).unwrap();
    let mut pages = Vec::new();
    for seg in parsed
        .segments
        .iter()
        .filter(|s| s.ty == elf32::PT_LOAD && s.memsz > 0)
    {
        let mut perm = PTE_V | PTE_U | PTE_R;
        if seg.flags & elf32::PF_W != 0 {
            perm |= PTE_W;
        }
        if seg.flags & elf32::PF_X != 0 {
            perm |= PTE_X;
        }
        let first = seg.vaddr & !(PAGE - 1);
        let mut va = first;
        while va < seg.vaddr + seg.memsz {
            pages.push((va, perm));
            va += PAGE;
        }
    }
    pages.push((USER_STACK, PTE_V | PTE_U | PTE_R | PTE_W));
    pages
}

/// The user pages `satp` maps, as (va, the PTE bits the kernel chooses).
fn user_pages(m: &Machine, satp: u32) -> Vec<(u32, u32)> {
    let (leaves, _) = page_table(m, satp);
    leaves
        .iter()
        .filter(|(va, _)| *va < USER_TOP)
        .map(|&(va, pte)| (va, pte & PTE_KERNEL_BITS))
        .collect()
}

/// The byte at physical address `addr`, read without side effects.
fn peek_byte(bus: &grail::bus::Bus, addr: u32) -> u8 {
    (bus.peek(addr & !3).unwrap() >> (8 * (addr & 3))) as u8
}

/// Reads `len` bytes of user memory at `va` through page table `satp`.
fn user_read(m: &Machine, satp: u32, va: u32, len: u32) -> Vec<u8> {
    let (leaves, _) = page_table(m, satp);
    (va..va + len)
        .map(|a| {
            let (_, pte) = leaves
                .iter()
                .find(|(page, _)| *page == a & !(PAGE - 1))
                .unwrap_or_else(|| panic!("{a:#x} is not mapped"));
            peek_byte(&m.cpu.bus, (pte >> 10) * PAGE + a % PAGE)
        })
        .collect()
}

/// The NUL-terminated string at user address `va`.
fn user_string(m: &Machine, satp: u32, va: u32) -> String {
    let mut s = Vec::new();
    for a in va.. {
        match user_read(m, satp, a, 1)[0] {
            0 => break,
            c => s.push(c),
        }
    }
    String::from_utf8(s).unwrap()
}

/// The name of the process on the CPU (`current` in kernel/proc.c), or
/// `None` in the scheduler.
fn current_name(cpu: &grail::cpu::Cpu, current: u32) -> Option<String> {
    let p = cpu.bus.peek(current).unwrap();
    if p == 0 {
        return None;
    }
    let bytes: Vec<u8> = (0..16)
        .map(|i| peek_byte(&cpu.bus, p + PROC_NAME + i))
        .take_while(|&b| b != 0)
        .collect();
    Some(String::from_utf8(bytes).unwrap())
}

/// Steps `m` until program `name` runs in U-mode.
fn step_to_user(m: &mut Machine, elf: &grail::elf::Elf, name: &str) {
    let current = sym(elf, "current");
    step_until(
        m,
        RUN_MAX_STEPS,
        |_| {},
        |m| m.cpu.mode == Mode::U && current_name(&m.cpu, current).as_deref() == Some(name),
    );
}

/// Runs the committed kernel with `input` typed on the UART (ending in
/// `exit`). Returns what was printed from the shell's banner on.
fn run_processes(input: &[u8]) -> String {
    let (mut m, _) = load_committed_on(MachineConfig::default(), Some(kernel::typed(input)));
    let r = m.run(Some(RUN_MAX_STEPS), |_| {});
    let out = printed(&m);
    assert_eq!(r, RunResult::Exited(0), "output:\n{out}");
    out[out.find(SHELL_BANNER).unwrap()..].to_string()
}

/// The lines between the last echo of `command` and the next prompt.
fn command_output<'a>(out: &'a str, command: &str) -> Vec<&'a str> {
    let start = format!("{PROMPT}{command}\n");
    let at = out
        .rfind(&start)
        .unwrap_or_else(|| panic!("no `{command}` in:\n{out}"))
        + start.len();
    let rest = &out[at..];
    rest[..rest
        .find(PROMPT)
        .unwrap_or_else(|| panic!("`{command}` never ended:\n{out}"))]
        .lines()
        .collect()
}

/// The satp value of the kernel page table (`kernel_pagetable`).
fn kernel_satp(m: &Machine, elf: &grail::elf::Elf) -> u32 {
    let root = m.cpu.bus.peek(sym(elf, "kernel_pagetable")).unwrap();
    assert_ne!(root, 0, "no kernel page table yet");
    (1 << 31) | (root >> 12)
}

/// Steps `m` until the kernel has printed its `n`th prompt since boot, at
/// most `RUN_MAX_STEPS` steps. Only the bytes printed since the last step are
/// looked at, so a kernel that never gets there fails in seconds rather than
/// rebuilding its whole output on every instruction.
fn step_to_prompt(m: &mut Machine, n: usize) {
    let prompt = PROMPT.as_bytes();
    let (mut seen, mut count) = (0, 0);
    for _ in 0..RUN_MAX_STEPS {
        let out = &m.cpu.bus.uart.captured;
        while seen < out.len() {
            seen += 1;
            if out[..seen].ends_with(prompt) {
                count += 1;
            }
        }
        if count >= n {
            return;
        }
        m.cpu.step();
    }
    panic!(
        "no prompt {n} after {RUN_MAX_STEPS} steps; output:\n{}",
        printed(m)
    );
}

/// Pages free in the allocator (`nfree` in kernel/kalloc.c).
fn free_pages(m: &Machine, elf: &grail::elf::Elf) -> u32 {
    m.cpu.bus.peek(sym(elf, "nfree")).unwrap()
}

#[test]
fn booting_drops_into_the_user_shell() {
    // Issue #21's first criterion: the kernel boots into the shell, a user
    // program, which runs hello and exits; the kernel then stops the
    // machine with the shell's exit status.
    let (r, out) = kernel::run_on_grail(&committed_elf(), b"hello\nexit\n", RUN_MAX_STEPS).unwrap();
    assert_eq!(
        kernel::mask_dbg_pc(&out),
        format!("{BOOT_OUTPUT}{SHELL_BANNER}{PROMPT}hello\nhello from user mode\n{PROMPT}exit\n")
    );
    assert_eq!(r, RunResult::Exited(0));
}

#[test]
fn the_shell_is_the_first_process() {
    // The first instruction in U-mode is sh's entry point, crt0's _start,
    // with every register zero but sp, a0 = argc and a1 = argv, which point
    // at { "sh", NULL } at the top of the stack.
    let (mut m, elf) = load_committed();
    step_until(&mut m, RUN_MAX_STEPS, |_| {}, |m| m.cpu.mode == Mode::U);
    let c = &m.cpu;
    assert_eq!(current_name(c, sym(&elf, "current")).as_deref(), Some("sh"));
    let pid = m.cpu.bus.peek(m.cpu.bus.peek(sym(&elf, "current")).unwrap() + 4);
    assert_eq!(pid, Some(1));
    assert_eq!(c.pc, USER_TEXT);
    let (sp, argv) = (c.x[2], c.x[x("a1") as usize]);
    let mut regs = [0u32; 32];
    regs[2] = sp;
    regs[x("a0") as usize] = 1;
    regs[x("a1") as usize] = argv;
    assert_eq!(c.x, regs);
    assert_eq!(sp % 16, 0);
    assert!(
        sp <= argv && argv < USER_TOP && sp > USER_STACK + PAGE / 2,
        "sp {sp:#x} argv {argv:#x}"
    );
    let satp = c.csr.satp;
    let words = user_read(&m, satp, argv, 8);
    let argv0 = u32::from_le_bytes(words[..4].try_into().unwrap());
    assert_eq!(words[4..], [0; 4], "argv[1] is NULL");
    assert!(argv0 > argv);
    assert_eq!(user_string(&m, satp, argv0), "sh");
    // The strings end right at the top: "sh\0" is the last three bytes.
    assert_eq!(argv0, USER_TOP - 3);
    // Interrupts on, SUM off.
    assert_ne!(c.csr.mstatus & MSTATUS_SIE, 0);
    assert_eq!(c.csr.mstatus & excalibur::csr::MSTATUS_SUM, 0);
}

#[test]
fn a_process_page_table_maps_its_program_and_the_kernel() {
    let (mut m, elf) = load_committed_on(MachineConfig::default(), Some(kernel::typed(b"ping\nexit\n")));
    step_to_user(&mut m, &elf, "ping");
    let c = &m.cpu;
    // Traps go to user_vec, which finds the process's trap frame in
    // sscratch: at the top of its kernel stack page, in free RAM.
    assert_eq!(c.csr.stvec, sym(&elf, "user_vec"));
    let frame = c.csr.sscratch;
    assert_eq!(frame % PAGE, PAGE - TRAP_FRAME_SIZE, "frame {frame:#x}");
    assert!(frame > sym(&elf, "__kernel_end"));

    let satp = c.csr.satp;
    let ksatp = kernel_satp(&m, &elf);
    assert_ne!(satp, ksatp);
    // Root entries 1-1023 are the kernel's: the same second-level tables.
    let (root, kroot) = ((satp & 0x3f_ffff) * PAGE, (ksatp & 0x3f_ffff) * PAGE);
    for i in 1..1024 {
        assert_eq!(
            m.cpu.bus.peek(root + 4 * i),
            m.cpu.bus.peek(kroot + 4 * i),
            "root[{i}]"
        );
    }
    // User space: ping's segments, each page with U and the segment's
    // permissions, and the stack (R+W); nothing at page 0, in the guard page
    // under the stack or, with an empty heap, anywhere else. Everything above
    // is the kernel's, none of it with U.
    let ping = user_program("ping");
    let want = expected_user_pages(&ping.elf);
    assert_eq!(want[0], (USER_TEXT, PTE_V | PTE_U | PTE_R | PTE_X));
    assert!(want
        .iter()
        .all(|&(va, _)| va >= USER_TEXT && va != USER_STACK - PAGE));
    assert_eq!(user_pages(&m, satp), want);
    let (leaves, tables) = page_table(&m, satp);
    let (kleaves, ktables) = page_table(&m, ksatp);
    let kernel_part: Vec<_> = leaves.iter().filter(|(va, _)| *va >= USER_TOP).copied().collect();
    let kernel_all: Vec<_> = kleaves
        .iter()
        .filter(|(va, _)| *va >= USER_TOP)
        .copied()
        .collect();
    assert_eq!(kernel_part, kernel_all);
    assert!(kernel_part.iter().all(|(_, pte)| pte & PTE_U == 0));
    // The kernel's own root[0] (the test device) is not in the process's.
    assert!(kleaves.iter().any(|(va, _)| *va < USER_TOP));
    // User pages are fresh pages in free RAM.
    for (va, pte) in leaves.iter().filter(|(va, _)| *va < USER_TOP) {
        let pa = (pte >> 10) * PAGE;
        assert!(pa >= sym(&elf, "__kernel_end"), "{va:#x} at {pa:#x}");
    }
    // Its own root, one table for user space where the kernel has its root[0]
    // table, and the kernel's other second-level tables, shared: as many as
    // the kernel's.
    assert_eq!(tables, ktables);
}

#[test]
fn ping_and_pong_take_turns_on_timer_ticks() {
    // Issue #20's first criterion, watched from the hart: two processes in
    // U-mode, each in its own page table, and the CPU only moves between
    // them when a timer tick (or an exit) traps from U-mode. The shell runs
    // too: it starts ping in the background and waits for pong.
    let (mut m, elf) = load_committed_on(
        MachineConfig::default(),
        Some(kernel::typed(b"ping & pong\nwait\nexit\n")),
    );
    let (user_vec, current) = (sym(&elf, "user_vec"), sym(&elf, "current"));
    // The process of each stretch in U-mode, and the last trap from it:
    // (scause, whether it was the exit call).
    let mut stretches: Vec<(String, u32)> = Vec::new();
    let mut last_trap: Option<(u32, bool)> = None;
    let mut timer_switches = 0;
    let mut user_steps = 0u64;
    let r = m.run(Some(RUN_MAX_STEPS), |cpu| {
        if cpu.mode == Mode::U {
            let name = current_name(cpu, current).unwrap();
            if name != "sh" {
                user_steps += 1;
            }
            assert!((USER_TEXT..USER_TOP).contains(&cpu.pc), "U-mode at {:#x}", cpu.pc);
            if stretches.last().map(|s| &s.0) != Some(&name) {
                if let Some((prev, _)) = stretches.last() {
                    if prev != "sh" {
                        // ping and pong never wait for anything: they leave
                        // the CPU when a tick preempts them, or for good.
                        match last_trap {
                            Some((cause, false)) => {
                                assert_eq!(cause, INTERRUPT_BIT | IRQ_S_SOFT, "{prev} left after {cause:#x}");
                                timer_switches += 1;
                            }
                            Some((_, true)) => {}
                            None => panic!("{prev} switched without a trap"),
                        }
                    }
                }
                stretches.push((name, cpu.csr.satp));
            }
            last_trap = None;
        }
        if cpu.pc == user_vec {
            // Every trap from U-mode arrives here, in S-mode, with SPP = U.
            assert_eq!(cpu.mode, Mode::S);
            assert_eq!(cpu.csr.mstatus & MSTATUS_SPP, 0);
            let exit = cpu.csr.scause == ECALL_FROM_U && cpu.x[x("a7") as usize] == SYS_EXIT;
            last_trap = Some((cpu.csr.scause, exit));
        }
    });
    assert_eq!(r, RunResult::Exited(0));
    let mut satps: Vec<(String, u32)> = stretches.clone();
    satps.sort();
    satps.dedup();
    let names: Vec<&str> = satps.iter().map(|s| s.0.as_str()).collect();
    assert_eq!(names, ["ping", "pong", "sh"], "{stretches:?}");
    let mut distinct: Vec<u32> = satps.iter().map(|s| s.1).collect();
    distinct.dedup();
    assert_eq!(distinct.len(), 3, "each its own page table: {satps:x?}");
    assert!(!distinct.contains(&kernel_satp(&m, &elf)));
    assert!(timer_switches >= 3, "{timer_switches} switches: {stretches:?}");
    // Each prints 4 lines with half a tick (500 000 instructions) between.
    assert!(user_steps > 3_000_000, "{user_steps}");
    // The shell: ping in the background, pong in the foreground.
    let out = printed(&m);
    assert!(
        out.contains(&format!("{PROMPT}ping & pong\n[2]\nping 1\n")),
        "{out}"
    );
    assert!(
        out.contains(&format!("{PROMPT}wait\n[2] ping: exit 0\n{PROMPT}exit\n")),
        "{out}"
    );
}

#[test]
fn programs_get_zeroed_memory_and_writable_data() {
    // data checks that its stack page below sp and its .bss (two pages, the
    // second with no file bytes) read zero, that its .data word holds its
    // initial value, and that .data and .bss take stores; then it dirties
    // them. The pages it gives back are reused by the second run (the
    // allocator doesn't clear freed pages), so that one passes only if the
    // loader and the stack get zeroed pages.
    let (mut m, elf) = load_committed_on(
        MachineConfig::default(),
        Some(kernel::typed(b"data\ndata\nexit\n")),
    );
    step_to_user(&mut m, &elf, "data");
    let data = user_program("data");
    let want = expected_user_pages(&data.elf);
    // .data and .bss in one PF_R|PF_W segment of two pages, the second with
    // no file bytes.
    let rw: Vec<_> = want
        .iter()
        .filter(|(va, p)| *va != USER_STACK && p & PTE_W != 0)
        .collect();
    assert_eq!(rw.len(), 2, "{want:x?}");
    assert_eq!(user_pages(&m, m.cpu.csr.satp), want);
    // `exit` alone exits with the last program's status: the second run's.
    assert_eq!(m.run(Some(RUN_MAX_STEPS), |_| {}), RunResult::Exited(0));
    let out = printed(&m);
    assert!(
        out.ends_with(&format!("{PROMPT}data\n{PROMPT}data\n{PROMPT}exit\n")),
        "{out}"
    );
}

#[test]
fn a_store_to_read_only_data_is_killed() {
    // .rodata is mapped without W: badstore's store to it faults.
    let out = run_processes(b"badstore\nexit 0\n");
    let badstore = user_program("badstore");
    let (main, msg) = (user_sym(&badstore.elf, "main"), user_sym(&badstore.elf, "msg"));
    let lines = command_output(&out, "badstore");
    assert_eq!(lines.len(), 4, "{out}");
    assert_eq!(lines[0], "pid 2 (badstore) killed: store page fault");
    // The store follows `la` (two instructions).
    assert_eq!(
        lines[1],
        format!(
            "  scause {STORE_PAGE_FAULT:#010x}  sepc {:#010x}  stval {msg:#010x}",
            main + 8
        )
    );
    assert!(lines[2].ends_with(" ---U--RV"), "{out}");
    assert_eq!(lines[3], "badstore: exit -1");
}

/// The registers `cpu` holds, and its pc.
fn user_state(cpu: &grail::cpu::Cpu) -> ([u32; 32], u32) {
    (cpu.x, cpu.pc)
}

#[test]
fn a_tick_in_user_mode_saves_every_register_and_switches_back_later() {
    // regs fills every register and spins; ping runs beside it. Catch a tick
    // that interrupts regs, check user_vec saved the registers in its trap
    // frame, then follow the switch to ping and back: regs must resume with
    // every register and its pc as they were.
    let (mut m, elf) = load_committed_on(
        MachineConfig::default(),
        Some(kernel::typed(b"regs & ping\nwait\nexit\n")),
    );
    let (user_vec, user_trap) = (sym(&elf, "user_vec"), sym(&elf, "user_trap"));
    // regs, in its spin loop: all its registers are set.
    step_to_user(&mut m, &elf, "regs");
    step_until(
        &mut m,
        RUN_MAX_STEPS,
        |_| {},
        |m| m.cpu.mode == Mode::U && m.cpu.x[x("t5") as usize] == 0x1e1e_1e1e,
    );
    let regs_satp = m.cpu.csr.satp;
    step_until(
        &mut m,
        RUN_MAX_STEPS,
        |_| {},
        |m| {
            m.cpu.pc == user_vec
                && m.cpu.csr.scause == INTERRUPT_BIT | IRQ_S_SOFT
                && m.cpu.csr.satp == regs_satp
        },
    );
    let (saved, _) = user_state(&m.cpu);
    let sepc = m.cpu.csr.sepc;
    assert!((USER_TEXT..USER_TOP).contains(&sepc));
    assert_eq!(saved[2], 0x0202_0202, "regs set sp to its pattern");
    // In U-mode, sscratch points at the process's trap frame.
    let frame = m.cpu.csr.sscratch;

    step_to(&mut m, user_trap);
    assert_eq!(m.cpu.x[x("a0") as usize], frame);
    assert_eq!(m.cpu.x[2], frame, "the kernel stack starts below the frame");
    // user_vec swapped the user's a0 in, to save it.
    assert_eq!(m.cpu.csr.sscratch, saved[x("a0") as usize]);
    let peek = |m: &Machine, off: u32| m.cpu.bus.peek(frame + off).unwrap();
    for (i, &v) in saved.iter().enumerate() {
        assert_eq!(peek(&m, 4 * i as u32), v, "{}", reg::abi_name(i as u8));
    }
    assert_eq!(peek(&m, FRAME_SEPC), sepc);
    assert_eq!(peek(&m, FRAME_SCAUSE), INTERRUPT_BIT | IRQ_S_SOFT);
    assert_eq!(peek(&m, FRAME_SSTATUS) & MSTATUS_SPP, 0);
    // Traps in the kernel go to kernel_vec again.
    step_until(
        &mut m,
        100,
        |_| {},
        |m| m.cpu.csr.stvec == sym(&elf, "kernel_vec"),
    );

    // Off to another process, then back to regs.
    let mut ran_other = false;
    step_until(
        &mut m,
        RUN_MAX_STEPS,
        |cpu| ran_other |= cpu.mode == Mode::U && cpu.csr.satp != regs_satp,
        |m| m.cpu.mode == Mode::U && m.cpu.csr.satp == regs_satp,
    );
    assert!(ran_other, "the tick didn't switch to another process");
    assert_eq!(user_state(&m.cpu), (saved, sepc));
    assert_eq!(m.run(Some(RUN_MAX_STEPS), |_| {}), RunResult::Exited(0));
    // And regs found every register intact at the end, after more switches.
    let out = printed(&m);
    assert!(out.contains("[2] regs: exit 0\n"), "{out}");
}

#[test]
fn registers_survive_many_switches() {
    // regs checks its own registers after three ticks' worth of spinning
    // beside two programs that use the same registers: exit status 0, or the
    // number of the register that changed.
    let out = run_processes(b"regs & ping & pong\nwait\nexit\n");
    let lines = command_output(&out, "wait");
    let mut collected = lines.clone();
    collected.sort();
    assert_eq!(collected, ["[2] regs: exit 0", "[3] ping: exit 0"], "{out}");
    // pong, in the foreground, exited 0: the shell said nothing about it.
    assert!(!out.contains("pong: exit"), "{out}");
}

#[test]
fn a_system_call_changes_a0_and_nothing_else() {
    // docs/camelot-syscalls.md: every register but a0 is preserved, a1-a7
    // and the temporaries included, which C programs can't check (they are
    // caller-saved there). sysregs fills every register, makes calls that
    // answer, switch away (yield, sleep), fail (a bad descriptor, an unknown
    // number) and write, and checks them all after each one; it exits with
    // 32 * case + register at the first that changed, which the shell would
    // print. Run it beside ping, so other processes use the registers too.
    let out = run_processes(b"ping & sysregs\nwait\nexit\n");
    let lines: Vec<&str> = command_output(&out, "ping & sysregs")
        .into_iter()
        .filter(|l| !l.starts_with("ping ") && !l.starts_with('['))
        .collect();
    assert_eq!(
        lines,
        ["sysregs: write", "sysregs: every register but a0 kept"],
        "{out}"
    );
}

#[test]
fn faulting_programs_are_killed_and_the_rest_carry_on() {
    // Issue #20's second criterion. badload loads from the kernel's first
    // page (mapped, without U), badhalt executes xcal.halt (M-mode only).
    let (mut m, elf) = load_committed_on(
        MachineConfig::default(),
        Some(kernel::typed(b"badload & ping & badhalt\nwait\nticks\nexit\n")),
    );
    let user_vec = sym(&elf, "user_vec");
    let mut faults = Vec::new();
    let r = m.run(Some(RUN_MAX_STEPS), |cpu| {
        if cpu.pc == user_vec && cpu.csr.scause & INTERRUPT_BIT == 0 && cpu.csr.scause != ECALL_FROM_U {
            faults.push((cpu.csr.scause, cpu.csr.sepc, cpu.csr.stval));
        }
    });
    let out = printed(&m);
    assert_eq!(r, RunResult::Exited(0), "{out}");
    // Each faults at its second instruction.
    let badload = user_sym(&user_program("badload").elf, "main") + 4;
    let badhalt = user_sym(&user_program("badhalt").elf, "main") + 4;
    let xcal_halt_a0 = 0x0005_100b;
    assert_eq!(
        faults,
        [
            (LOAD_PAGE_FAULT, badload, 0x8000_0000),
            (ILLEGAL_INSTRUCTION, badhalt, xcal_halt_a0)
        ]
    );
    let text = sym(&elf, "__text_start");
    let pte = (text >> 12) << 10 | PTE_V | PTE_R | PTE_X | PTE_A;
    let lines = command_output(&out, "badload & ping & badhalt");
    let want = [
        "[2]".to_string(),
        "[3]".to_string(),
        "pid 2 (badload) killed: load page fault".to_string(),
        format!("  scause 0x0000000d  sepc {badload:#010x}  stval 0x80000000"),
        format!("  va 0x80000000 maps to pa 0x80000000, pte {pte:#010x} -A--X-RV"),
        "pid 4 (badhalt) killed: illegal instruction".to_string(),
        format!("  scause 0x00000002  sepc {badhalt:#010x}  stval 0x0005100b"),
        "badhalt: exit -1".to_string(),
    ];
    let kills: Vec<&str> = lines
        .iter()
        .copied()
        .filter(|l| !l.starts_with("ping "))
        .collect();
    assert_eq!(kills, want, "{out}");
    // ping wasn't disturbed, and the shell still works.
    let pings: Vec<&str> = out.lines().filter(|l| l.starts_with("ping ")).collect();
    assert_eq!(pings, ["ping 1", "ping 2", "ping 3", "ping 4"]);
    let mut collected: Vec<&str> = command_output(&out, "wait")
        .into_iter()
        .filter(|l| !l.starts_with("ping "))
        .collect();
    collected.sort();
    assert_eq!(collected, ["[2] badload: exit -1", "[3] ping: exit 0"]);
    assert_eq!(command_output(&out, "ticks").len(), 1);
    // The kernel never panicked, and xcal.halt never reached M-mode.
    assert!(!out.contains("panic"), "{out}");
}

#[test]
fn bad_pointers_are_refused() {
    // Issue #21's second criterion. badptr hands write and read kernel
    // memory, page 0, a range past the end of user space, the stack's guard
    // page, lengths that wrap, read-only memory and memory past the heap,
    // and wait and spawn bad pointers too (program names that run off the
    // heap, among them one longer than any program's): each call must fail
    // with -EFAULT, print nothing and leave the kernel running. Otherwise
    // badptr prints `badptr: case N failed`. Its failed reads take no input
    // either: hello still gets its line.
    let out = run_processes(b"badptr\nhello\nexit\n");
    assert_eq!(
        command_output(&out, "badptr"),
        ["badptr: every bad pointer refused"]
    );
    assert_eq!(command_output(&out, "hello"), ["hello from user mode"]);
}

#[test]
fn the_system_calls_do_what_the_document_says() {
    // systest checks every call from user space: the error numbers, exit
    // statuses and arguments through spawn and wait, filling the process
    // table, sleep, sbrk growing and shrinking the heap.
    let out = run_processes(b"systest\nexit\n");
    let lines = command_output(&out, "systest");
    assert_eq!(lines.len(), 1, "{out}");
    assert!(
        lines[0].starts_with("systest: ") && lines[0].ends_with(" checks, 0 failed"),
        "{out}"
    );
    let checks: u32 = lines[0]["systest: ".len()..]
        .split(' ')
        .next()
        .unwrap()
        .parse()
        .unwrap();
    assert!(checks > 70, "{checks}");
}

#[test]
fn without_a_disk_the_shell_still_pipes_and_finds_the_programs_in_bin() {
    // Pipes need no disk, and the kernel's own programs stand in for /bin:
    // by name, by /bin/NAME and by bin/NAME from the root. Any other path,
    // and a file to redirect to, doesn't exist.
    let out = run_processes(
        b"echo one two three | wc\n\
          echo a b | cat | cat | wc\n\
          /bin/echo by path\n\
          bin/echo from the root\n\
          /echo\n\
          echo x > /file\n\
          wc < /file\n\
          ls\n\
          exit 0\n",
    );
    assert_eq!(command_output(&out, "echo one two three | wc"), ["1 3 14"]);
    assert_eq!(command_output(&out, "echo a b | cat | cat | wc"), ["1 2 4"]);
    assert_eq!(command_output(&out, "/bin/echo by path"), ["by path"]);
    assert_eq!(command_output(&out, "bin/echo from the root"), ["from the root"]);
    assert_eq!(
        command_output(&out, "/echo"),
        ["sh: /echo: no such file or directory"]
    );
    assert_eq!(
        command_output(&out, "echo x > /file"),
        ["sh: /file: no such file or directory"]
    );
    assert_eq!(
        command_output(&out, "wc < /file"),
        ["sh: /file: no such file or directory"]
    );
    assert_eq!(
        command_output(&out, "ls"),
        ["ls: .: no such file or directory", "ls: exit 1"]
    );
}

#[test]
fn programs_get_their_arguments() {
    let out = run_processes(b"echo\necho one\necho  a   b\tc  d \nexit\n");
    assert_eq!(command_output(&out, "echo"), [""]);
    assert_eq!(command_output(&out, "echo one"), ["one"]);
    assert_eq!(command_output(&out, "echo  a   bc  d "), ["a bc d"]);
}

#[test]
fn a_shell_runs_inside_the_shell() {
    // sh is a program like any other: the inner one reads the next lines,
    // and the outer one carries on when it exits.
    let out = run_processes(b"sh\nhello\nexit 4\necho back\nexit 0\n");
    assert!(
        out.ends_with(&format!(
            "{PROMPT}sh\n{SHELL_BANNER}{PROMPT}hello\nhello from user mode\n{PROMPT}exit 4\n\
             sh: exit 4\n{PROMPT}echo back\nback\n{PROMPT}exit 0\n"
        )),
        "{out}"
    );
}

#[test]
fn sleeping_processes_leave_the_cpu_idle() {
    // While everyone sleeps (sh in wait, systest's child in sleep), the
    // scheduler idles in wfi: each tick costs a few hundred instructions.
    let (mut m, elf) = load_committed_on(
        MachineConfig::default(),
        Some(kernel::typed(b"systest sleep 50\nexit\n")),
    );
    let ticks = sym(&elf, "ticks");
    step_to_user(&mut m, &elf, "systest");
    let start = m.cpu.bus.peek(ticks).unwrap();
    let mut user_steps = 0;
    let steps = step_until(
        &mut m,
        RUN_MAX_STEPS,
        |cpu| user_steps += u64::from(cpu.mode == Mode::U),
        |m| printed(m).ends_with(&format!("{PROMPT}systest sleep 50\n{PROMPT}")),
    );
    let slept = m.cpu.bus.peek(ticks).unwrap() - start;
    assert!((50..=51).contains(&slept), "{slept} ticks");
    assert!(steps < 50 * 1000, "{steps} steps for {slept} ticks");
    assert!(user_steps < 10_000, "{user_steps} steps in U-mode");
    assert_eq!(m.run(Some(RUN_MAX_STEPS), |_| {}), RunResult::Exited(0));
}

#[test]
fn input_typed_while_processes_run_reaches_the_shell() {
    // The UART interrupt arrives while a process is in U-mode: user_trap
    // services it and the process carries on; the shell reads the line once
    // ping is over.
    let (tx, rx) = std::sync::mpsc::channel();
    let (mut m, elf) = load_committed_on(MachineConfig::default(), Some(rx));
    for &b in b"ping\n" {
        tx.send(b).unwrap();
    }
    step_to_user(&mut m, &elf, "ping");
    for &b in b"ticks\nexit\n" {
        tx.send(b).unwrap();
    }
    let user_vec = sym(&elf, "user_vec");
    let mut external = 0;
    let r = m.run(Some(RUN_MAX_STEPS), |cpu| {
        if cpu.pc == user_vec && cpu.csr.scause == INTERRUPT_BIT | IRQ_S_EXT {
            external += 1;
        }
    });
    assert_eq!(r, RunResult::Exited(0));
    assert!(external >= 1);
    let out = printed(&m);
    assert!(out.contains(&format!("ping 4\n{PROMPT}ticks\nticks: ")), "{out}");
}

#[test]
fn processes_give_back_every_page() {
    let (tx, rx) = std::sync::mpsc::channel();
    let (mut m, elf) = load_committed_on(MachineConfig::default(), Some(rx));
    let mut prompts = 1;
    let mut command = |m: &mut Machine, line: &str| {
        for b in line.bytes() {
            tx.send(b).unwrap();
        }
        prompts += 1;
        step_to_prompt(m, prompts);
    };
    step_to_prompt(&mut m, 1);
    // The shell holds its own pages meanwhile.
    let before = free_pages(&m, &elf);

    // Every program on its own: the ones that exit, the ones that are
    // killed, and systest, which spawns children, makes pipes and grows its
    // heap. (sh, cat and wc would read the next lines themselves: see
    // a_shell_runs_inside_the_shell. Without a disk, the file programs only
    // fail.)
    let names: Vec<String> = program_names()
        .into_iter()
        .filter(|n| !["sh", "cat", "wc"].contains(&n.as_str()))
        .collect();
    for name in &names {
        command(&mut m, &format!("{name}\n"));
        assert_eq!(free_pages(&m, &elf), before, "{name}");
    }
    // Several side by side, then collected.
    let some = ["hello", "badload", "ping", "data", "badstore", "echo"];
    command(&mut m, &format!("{} &\n", some.join(" & ")));
    command(&mut m, "wait\n");
    assert_eq!(free_pages(&m, &elf), before);
    // More than there are slots: the shell has one, and the eighth hello
    // can't start. Exited children keep their slots until collected.
    let hellos = ["hello"; NPROC].join(" & ");
    command(&mut m, &format!("{hellos} &\n"));
    let out = printed(&m);
    let lines = command_output(&out, &format!("{hellos} &"));
    assert!(lines.contains(&"sh: hello: no free process slot"), "{out}");
    command(&mut m, "wait\n");
    let out = printed(&m);
    // The hellos run while the shell starts the others and waits for its
    // next line; wait collects them.
    let waited = command_output(&out, "wait");
    let collected = waited.iter().filter(|l| l.ends_with("] hello: exit 0")).count();
    let greetings = lines
        .iter()
        .chain(&waited)
        .filter(|l| **l == "hello from user mode")
        .count();
    assert_eq!((collected, greetings), (NPROC - 1, NPROC - 1), "{out}");
    assert_eq!(free_pages(&m, &elf), before);
    // While a process runs, it holds its pages: a kernel stack, a root and a
    // user table, its segments and its stack.
    for &b in b"ping\n" {
        tx.send(b).unwrap();
    }
    step_to_user(&mut m, &elf, "ping");
    let pages = expected_user_pages(&user_program("ping").elf).len() as u32;
    assert_eq!(free_pages(&m, &elf), before - 3 - pages);
}

#[test]
fn a_heap_that_runs_out_of_memory_gives_back_what_it_took() {
    // sbrk maps the new pages one at a time: when memory runs out partway,
    // it must unmap and free the ones it had mapped and leave the heap as it
    // was. 3 MB fits in user space, and a 128 MiB machine has the pages...
    const LINE: &str = "systest sbrk 3000000";
    let out = run_processes(format!("{LINE}\nexit\n").as_bytes());
    assert_eq!(command_output(&out, LINE), ["systest: sbrk 3000000: 0"]);

    // ...but a 1 MiB one runs out of them after mapping some.
    let config = MachineConfig {
        ram_size: 1 << 20,
        ..MachineConfig::default()
    };
    let (tx, rx) = std::sync::mpsc::channel();
    let (mut m, elf) = load_committed_on(config, Some(rx));
    step_to_prompt(&mut m, 1);
    let before = free_pages(&m, &elf);
    // systest's own pages (see processes_give_back_every_page), then a page
    // for the heap at least.
    let pages = 3 + expected_user_pages(&user_program("systest").elf).len() as u32;
    assert!(
        before > pages + 1 && before < pages + 3_000_000 / PAGE,
        "{before} pages"
    );
    for b in format!("{LINE}\n").bytes() {
        tx.send(b).unwrap();
    }
    // Right after the call, systest prints its result: it holds no page more
    // than when it started.
    let want = "systest: sbrk 3000000: -12\n";
    step_until(
        &mut m,
        RUN_MAX_STEPS,
        |_| {},
        |m| m.cpu.bus.uart.captured.ends_with(want.as_bytes()),
    );
    assert_eq!(free_pages(&m, &elf), before - pages);
    // And it found the heap as it was: no exit line from the shell.
    step_to_prompt(&mut m, 2);
    let out = printed(&m);
    assert_eq!(command_output(&out, LINE), [want.trim_end()]);
    assert_eq!(free_pages(&m, &elf), before);
    for b in b"exit\n" {
        tx.send(*b).unwrap();
    }
    assert_eq!(m.run(Some(RUN_MAX_STEPS), |_| {}), RunResult::Exited(0));
}

/// Byte offsets in an ELF32 header and program header.
const E_TYPE: u32 = 16;
const E_MACHINE: u32 = 18;
const E_ENTRY: u32 = 24;
const E_PHOFF: u32 = 28;
const E_PHENTSIZE: u32 = 42;
const E_PHNUM: u32 = 44;
const P_OFFSET: u32 = 4;
const P_VADDR: u32 = 8;
const P_FILESZ: u32 = 16;
const P_MEMSZ: u32 = 20;
const P_FLAGS: u32 = 24;

/// Bytes to write at an offset into an ELF image.
type Patch<'a> = (u32, &'a [u8]);

#[test]
fn the_elf_loader_refuses_bad_images() {
    // hello's image in the kernel, patched in memory one way at a time; each
    // `hello` typed at the shell must fail with the loader's reason (or run,
    // for the patches that keep the image good), leak nothing, and leave the
    // kernel running.
    let hello = user_program("hello");
    let parsed = elf32::Elf::parse(&hello.elf).unwrap();
    let phoff = parsed.header.phoff;
    let size = hello.elf.len() as u32;
    let loads: Vec<u32> = (0..u32::from(parsed.header.phnum))
        .filter(|&i| parsed.segments[i as usize].ty == elf32::PT_LOAD)
        .collect();
    assert_eq!(loads.len(), 2, "hello: text and rodata");
    let ph = |i: u32, field: u32| phoff + elf32::PHDR_SIZE as u32 * loads[i as usize] + field;
    let text = &parsed.segments[loads[0] as usize];
    let rodata = &parsed.segments[loads[1] as usize];
    // The cases rely on this layout: the text from the first page on, then
    // less than a page of rodata on a page of its own, less than a page
    // before the end of the file.
    assert_eq!(text.vaddr, USER_TEXT);
    assert!(rodata.vaddr >= text.vaddr + text.memsz && rodata.vaddr.is_multiple_of(PAGE));
    assert!(rodata.memsz < PAGE);
    assert_eq!(rodata.filesz, rodata.memsz);
    assert!(rodata.offset + PAGE > size);
    // hello reaches its rodata pc-relatively (-mcmodel=medany), so it still
    // runs with both segments moved by the same amount: here, so that its
    // rodata, given a whole page, ends right at USER_LOAD_TOP.
    let high_rodata = USER_LOAD_TOP - PAGE;
    let high_text = high_rodata - (rodata.vaddr - text.vaddr);
    let entry = parsed.header.entry;
    let moved_high = |rodata_memsz: u32| -> Vec<(u32, [u8; 4])> {
        vec![
            (E_ENTRY, (entry - text.vaddr + high_text).to_le_bytes()),
            (ph(0, P_VADDR), high_text.to_le_bytes()),
            (ph(1, P_VADDR), high_rodata.to_le_bytes()),
            (ph(1, P_MEMSZ), rodata_memsz.to_le_bytes()),
        ]
    };
    let (at_top, past_top) = (moved_high(PAGE), moved_high(PAGE + 1));
    let at_top: Vec<Patch<'_>> = at_top.iter().map(|(o, b)| (*o, &b[..])).collect();
    let past_top: Vec<Patch<'_>> = past_top.iter().map(|(o, b)| (*o, &b[..])).collect();

    let (tx, rx) = std::sync::mpsc::channel();
    let (mut m, elf) = load_committed_on(MachineConfig::default(), Some(rx));
    let image = sym(&elf, "user_elf_hello");
    assert_eq!(
        m.cpu.bus.peek(image),
        Some(u32::from_le_bytes(hello.elf[..4].try_into().unwrap())),
        "the kernel embeds what kernel/user builds"
    );
    step_to_prompt(&mut m, 1);
    let before = free_pages(&m, &elf);

    // What `hello` must print: the loader's reason, or `None` if it runs.
    let cases: &[(Option<&str>, &[Patch<'_>])] = &[
        (Some("not an ELF file"), &[(0, b"\x7fELG")]),
        (Some("not a 32-bit little-endian RISC-V file"), &[(4, &[2])]),
        (Some("not a 32-bit little-endian RISC-V file"), &[(5, &[2])]),
        (
            Some("not a 32-bit little-endian RISC-V file"),
            &[(E_MACHINE, &[62, 0])],
        ),
        (Some("not an executable"), &[(E_TYPE, &[1, 0])]),
        (Some("bad program headers"), &[(E_PHENTSIZE, &[40, 0])]),
        (Some("bad program headers"), &[(E_PHNUM, &[0xff, 0xff])]),
        // Program headers past the end of the file, and partly outside it.
        (
            Some("bad program headers"),
            &[(E_PHOFF, &(size + 4).to_le_bytes())],
        ),
        (
            Some("bad program headers"),
            &[(E_PHOFF, &(size - 10).to_le_bytes())],
        ),
        (
            Some("segment outside user space"),
            &[(ph(0, P_VADDR), &0x8000_0000u32.to_le_bytes())],
        ),
        (
            Some("segment outside user space"),
            &[(ph(0, P_VADDR), &0u32.to_le_bytes())],
        ),
        // The guard page under the stack.
        (
            Some("segment outside user space"),
            &[(ph(1, P_VADDR), &(USER_STACK - PAGE).to_le_bytes())],
        ),
        // Starting below USER_LOAD_TOP but ending above it.
        (
            Some("segment outside user space"),
            &[(ph(1, P_VADDR), &(USER_LOAD_TOP - 0x10).to_le_bytes())],
        ),
        // A size that wraps around the address space.
        (
            Some("segment outside user space"),
            &[(ph(1, P_MEMSZ), &0xffff_f000u32.to_le_bytes())],
        ),
        // Ending right at USER_LOAD_TOP is fine, one byte further is not.
        (None, &at_top),
        (Some("segment outside user space"), &past_top),
        (
            Some("segment outside the file"),
            &[(ph(0, P_OFFSET), &0xffff_f000u32.to_le_bytes())],
        ),
        (
            Some("segment outside the file"),
            &[(ph(0, P_FILESZ), &0x10_0000u32.to_le_bytes())],
        ),
        // p_filesz <= p_memsz, but the file bytes run past its end.
        (
            Some("segment outside the file"),
            &[
                (ph(1, P_FILESZ), &PAGE.to_le_bytes()),
                (ph(1, P_MEMSZ), &PAGE.to_le_bytes()),
            ],
        ),
        // p_filesz > p_memsz, with the file bytes inside it.
        (
            Some("segment outside the file"),
            &[(ph(1, P_FILESZ), &(rodata.memsz + 1).to_le_bytes())],
        ),
        (
            Some("segment not readable"),
            &[(ph(0, P_FLAGS), &1u32.to_le_bytes())],
        ),
        (
            Some("segments share a page"),
            &[(ph(1, P_VADDR), &text.vaddr.to_le_bytes())],
        ),
        (
            Some("entry point not in an executable segment"),
            &[(E_ENTRY, &rodata.vaddr.to_le_bytes())],
        ),
        (
            Some("entry point not in an executable segment"),
            &[(E_ENTRY, &0x8000_0000u32.to_le_bytes())],
        ),
        // An empty segment is skipped: here the text, so there's no code.
        (
            Some("entry point not in an executable segment"),
            &[(ph(0, P_MEMSZ), &0u32.to_le_bytes())],
        ),
        // In the text, but not on an instruction.
        (
            Some("misaligned entry point"),
            &[(E_ENTRY, &(text.vaddr + 2).to_le_bytes())],
        ),
    ];
    let echo = format!("{PROMPT}hello\n");
    for (n, (want, patches)) in cases.iter().enumerate() {
        for (off, bytes) in *patches {
            m.cpu.bus.load(image + off, bytes).unwrap();
        }
        for b in b"hello\n" {
            tx.send(*b).unwrap();
        }
        step_to_prompt(&mut m, n + 2);
        // What this `hello` printed, up to the new prompt: the kernel's
        // reason, then the shell's (spawn returned -ENOEXEC).
        let out = printed(&m);
        let out = &out[..out.len() - PROMPT.len()];
        let lines: Vec<&str> = out[out.rfind(&echo).unwrap() + echo.len()..].lines().collect();
        let expected = match want {
            Some(reason) => vec![
                format!("spawn: /bin/hello: {reason}"),
                "sh: hello: not an executable".to_string(),
            ],
            None => vec!["hello from user mode".to_string()],
        };
        assert_eq!(lines, expected, "case {n}");
        assert_eq!(free_pages(&m, &elf), before, "case {n} leaked");
        // Put the image back.
        for (off, bytes) in *patches {
            let o = *off as usize;
            m.cpu
                .bus
                .load(image + off, &hello.elf[o..o + bytes.len()])
                .unwrap();
        }
    }
    // Restored, it runs.
    for b in b"hello\nexit\n" {
        tx.send(*b).unwrap();
    }
    assert_eq!(m.run(Some(RUN_MAX_STEPS), |_| {}), RunResult::Exited(0));
    assert!(printed(&m).ends_with(&format!("{echo}hello from user mode\n{PROMPT}exit\n")));
}

#[test]
fn userprogs_s_is_up_to_date() {
    // kernel/userprogs.s is generated from kernel/user by merlin-as and
    // merlin-ld (no LLVM): it must be what the current sources and tools
    // give, or `cargo xtask kernel` wasn't run after a change.
    let committed = std::fs::read_to_string(repo_root().join(xtask::userprogs::GENERATED)).unwrap();
    let fresh = xtask::userprogs::generated(&repo_root()).unwrap();
    assert!(
        committed == fresh,
        "kernel/userprogs.s is out of date: run `cargo xtask kernel` and commit it"
    );
}

#[test]
fn every_c_program_has_its_clang_output_committed() {
    // C programs are assembled from clang's output in kernel/user/clang, so
    // building them needs no LLVM: every C file has its file there, and
    // nothing else is.
    let dir = xtask::userprogs::user_dir(&repo_root());
    let c: Vec<_> = xtask::userprogs::sources(&dir)
        .unwrap()
        .into_iter()
        .filter(|s| s.lang == xtask::userprogs::Lang::C)
        .collect();
    assert!(c.len() >= 8, "{c:?}");
    for src in &c {
        let path = dir.join(src.asm_file());
        let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        assert!(
            text.starts_with("# Generated by `cargo xtask kernel` with clang: do not edit.\n"),
            "{}",
            path.display()
        );
        assert!(text.contains(&format!("# Source: kernel/user/{}", src.file())));
    }
    assert_eq!(
        xtask::userprogs::stale_clang_files(&dir).unwrap(),
        Vec::<String>::new()
    );
}

/// The `| N | `name` | ...` rows of the table under `heading` in `doc`, as
/// (N, name, the cells after the name).
fn doc_rows<'a>(doc: &'a str, heading: &str) -> Vec<(u32, String, Vec<&'a str>)> {
    let at = doc
        .find(&format!("\n{heading}\n"))
        .unwrap_or_else(|| panic!("no {heading}"));
    let section = &doc[at + heading.len() + 2..];
    let section = &section[..section.find("\n## ").unwrap_or(section.len())];
    section
        .lines()
        .filter_map(|line| {
            let cells: Vec<&str> = line.split('|').map(str::trim).collect();
            let n = cells.get(1)?.parse().ok()?;
            let name = cells.get(2)?.strip_prefix('`')?.strip_suffix('`')?;
            // The line ends with `|`: no cell after it.
            let rest = cells[3..cells.len() - 1].to_vec();
            Some((n, name.to_string(), rest))
        })
        .collect()
}

/// The `| N | `name` | ...` rows of the table under `heading` in `doc`.
fn doc_table(doc: &str, heading: &str) -> Vec<(u32, String)> {
    doc_rows(doc, heading)
        .into_iter()
        .map(|(n, name, _)| (n, name))
        .collect()
}

/// Every `#define PREFIX<name> <number>` in `text` (C) or `.equ
/// PREFIX<name>, <number>` (assembly), as (number, name with the prefix).
fn defines(text: &str, prefix: &str) -> Vec<(u32, String)> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.split("/*").next().unwrap().trim();
        let rest = line
            .strip_prefix("#define ")
            .or_else(|| line.strip_prefix(".equ "))
            .unwrap_or("");
        let mut words = rest.split([' ', ',']).filter(|w| !w.is_empty());
        if let (Some(name), Some(value)) = (words.next(), words.next()) {
            if let (true, Ok(v)) = (name.starts_with(prefix), value.parse()) {
                out.push((v, name.to_string()));
            }
        }
    }
    out
}

#[test]
fn the_system_call_document_matches_the_code() {
    // Issue #21's third criterion: docs/camelot-syscalls.md's tables are the
    // kernel's numbers, and the user side has the same ones.
    // Git for Windows may check text out with CRLF line ends: compare with
    // LF ones whatever it did.
    let read = |path: &str| {
        std::fs::read_to_string(repo_root().join(path))
            .unwrap()
            .replace("\r\n", "\n")
    };
    let doc = read("docs/camelot-syscalls.md");
    let header = read("kernel/include/syscall.h");
    let usys = read("kernel/user/usys.s");
    let user_h = read("kernel/user/user.h");

    let calls = doc_table(&doc, "## Calls");
    assert_eq!(calls.len(), 21, "{calls:?}");
    let sys = |table: &[(u32, String)]| -> Vec<(u32, String)> {
        table
            .iter()
            .map(|(n, name)| (*n, format!("SYS_{name}")))
            .collect()
    };
    assert_eq!(defines(&header, "SYS_"), sys(&calls), "syscall.h");
    assert_eq!(defines(&usys, "SYS_"), sys(&calls), "usys.s");
    for (_, name) in &calls {
        // A stub in usys.s that uses the number, and a declaration in user.h.
        assert!(
            usys.contains(&format!(
                "    .globl {name}\n{name}:\n    li    a7, SYS_{name}\n    ecall\n"
            )),
            "usys.s: no stub for {name}"
        );
        assert!(
            user_h
                .lines()
                .any(|l| l.contains(&format!(" {name}(")) && l.ends_with(");")),
            "user.h: no declaration of {name}"
        );
    }

    let errors = doc_table(&doc, "## Error numbers");
    assert_eq!(errors.len(), 25, "{errors:?}");
    assert_eq!(defines(&header, "E"), errors, "syscall.h");
    assert_eq!(defines(&user_h, "E"), errors, "user.h");

    // Each call's arguments are its user.h declaration's parameters, and
    // what it may fail with are error numbers.
    for (_, name, cells) in doc_rows(&doc, "## Calls") {
        let [args, returns] = cells[..] else {
            panic!("{name}: {cells:?}");
        };
        let decl = user_h
            .lines()
            .find(|l| l.contains(&format!(" {name}(")) && l.ends_with(");"))
            .unwrap();
        let params = &decl[decl.find('(').unwrap() + 1..decl.len() - 2];
        let params = if params == "void" { "" } else { params };
        assert_eq!(
            args.trim_matches('`'),
            params,
            "{name}: the document, then user.h"
        );
        let mut listed = 0;
        for word in returns.split(['`', ',', ';', ' ']) {
            if let Some(e) = word.strip_prefix("-E") {
                assert!(errors.iter().any(|(_, n)| n[1..] == *e), "{name}: no error E{e}");
                listed += 1;
            }
        }
        // A call that takes a pointer can fail with -EFAULT, and only those;
        // only the calls that can't fail list no error.
        assert_eq!(
            args.contains('*'),
            returns.contains("`-EFAULT`"),
            "{name}: {returns}"
        );
        let infallible = ["exit", "getpid", "yield", "sleep", "uptime"];
        assert_eq!(
            listed == 0,
            infallible.contains(&name.as_str()),
            "{name}: {returns}"
        );
    }
    // And the limits.
    for limit in ["MAXARG", "ARG_MAX", "MAXPATH"] {
        let want = defines(&header, limit);
        assert_eq!(want.len(), 1);
        assert_eq!(defines(&user_h, limit), want);
        assert!(doc.contains(&format!("`{limit}` ({})", want[0].0)), "{limit}");
    }
    // Constants the test programs copy from the kernel's headers, and the
    // console's major number mkfs gives /dev/console.
    let exactly = |text: &str, name: &str| -> Vec<(u32, String)> {
        defines(text, name)
            .into_iter()
            .filter(|(_, n)| n == name)
            .collect()
    };
    let filetest = read("kernel/user/filetest.c");
    let systest = read("kernel/user/systest.c");
    for (header, name, copies) in [
        ("kernel/include/file.h", "NOFILE", vec![&filetest, &systest]),
        ("kernel/include/file.h", "CONSOLE", vec![&filetest, &systest]),
        ("kernel/include/pipe.h", "PIPESIZE", vec![&systest]),
        ("kernel/include/proc.h", "NPROC", vec![&systest]),
    ] {
        let want = exactly(&read(header), name);
        assert_eq!(want.len(), 1, "{header}: {name}");
        for copy in copies {
            assert_eq!(exactly(copy, name), want, "a user program's copy of {name}");
        }
    }
    let console = exactly(&read("kernel/include/file.h"), "CONSOLE");
    assert_eq!(console[0].0, u32::from(xtask::fs::CONSOLE_MAJOR));
}

#[test]
fn the_kernel_embeds_every_user_program() {
    // The table in the image: names, sizes and bytes match a fresh build.
    let (mut m, elf) = load_committed();
    let dir = xtask::userprogs::user_dir(&repo_root());
    let programs = xtask::userprogs::build_all(&dir).unwrap();
    let table = sym(&elf, "user_programs");
    assert_eq!(
        m.cpu.bus.peek(sym(&elf, "user_program_count")),
        Some(programs.len() as u32)
    );
    for (i, p) in programs.iter().enumerate() {
        let entry = table + 12 * i as u32;
        let (name, image, size) = (
            m.cpu.bus.peek(entry).unwrap(),
            m.cpu.bus.peek(entry + 4).unwrap(),
            m.cpu.bus.peek(entry + 8).unwrap(),
        );
        assert_eq!(image, sym(&elf, &format!("user_elf_{}", p.name)));
        assert_eq!(size as usize, p.elf.len());
        let mut s = Vec::new();
        for a in name.. {
            match m.cpu.bus.read(a, 1).unwrap() {
                0 => break,
                c => s.push(c as u8),
            }
        }
        assert_eq!(String::from_utf8(s).unwrap(), p.name);
        for (off, chunk) in p.elf.chunks(4).enumerate() {
            let word = m
                .cpu
                .bus
                .read(image + 4 * off as u32, chunk.len() as u32)
                .unwrap();
            let mut want = [0u8; 4];
            want[..chunk.len()].copy_from_slice(chunk);
            assert_eq!(word, u32::from_le_bytes(want), "{} at {off}", p.name);
        }
        // Read-only data: the kernel maps it without W.
        assert!((sym(&elf, "__rodata_start")..sym(&elf, "__data_start")).contains(&image));
    }
}

// ---- Disks: the block driver and the buffer cache (issue #65) ----

/// Block size of the buffer cache (`BSIZE` in kernel/include/bio.h): two
/// sectors.
const BSIZE: usize = 1024;
const BLOCK_SECTORS: u32 = 2;
/// Buffers in the cache (`NBUF` in kernel/include/bio.h).
const NBUF: u32 = 30;
/// The disk most tests attach: 64 blocks, enough for the self-test's LRU
/// check, which needs NBUF + 1.
const DISK_SECTORS: usize = 128;
/// What the self-test counts on a read-write disk that big
/// (kernel/blktest.c): 4 for the three readers, 7 for the scratch block, 3
/// for the errors, 2 for the LRU.
const BLK_SELFTEST_CHECKS: u32 = 16;
/// The self-test's scratch block: the last whole one, here sectors 126-127.
const SCRATCH_SECTOR: u32 = (DISK_SECTORS as u32 / BLOCK_SECTORS - 1) * BLOCK_SECTORS;

/// A disk image of `sectors` sectors in which no two neighbouring bytes, and
/// no two sectors, are alike.
fn disk_bytes(sectors: usize) -> Vec<u8> {
    (0..sectors * 512).map(|i| (i * 13 + i / 512) as u8).collect()
}

/// The self-test's pattern (`fill` in kernel/blktest.c): byte i is i * 7 + 3.
fn blk_pattern() -> Vec<u8> {
    (0..BSIZE).map(|i| (i * 7 + 3) as u8).collect()
}

/// What the kernel says after the disk self-test when disk 0 holds no
/// filesystem, as none of these disks does (kernel/fstest.c; the
/// filesystem's own tests are in tests/fs.rs).
const NO_FILESYSTEM: &str = "fs: no filesystem on disk 0\n";

/// The boot output with `disks` (the kernel's lines about them) in place of
/// `disk: none`, and `selftest` after the ticks, then the line saying disk 0
/// has no filesystem.
fn boot_output_with(disks: &str, selftest: &str) -> String {
    format!(
        "{}{selftest}{NO_FILESYSTEM}{SHELL_BANNER}{PROMPT}exit\n",
        BOOT_OUTPUT.replace("disk: none\n", disks)
    )
}

/// Loads the committed kernel on a default machine with `disks` attached and
/// `EXIT` typed.
fn load_with_disks(disks: Vec<DiskConfig>) -> (Machine, grail::elf::Elf) {
    let config = MachineConfig {
        disks,
        ..MachineConfig::default()
    };
    load_committed_on(config, Some(kernel::typed(EXIT)))
}

/// A command a disk ran, as the device saw it when `BLK_CMD` was written.
#[derive(Debug, Clone, PartialEq)]
struct DiskCommand {
    disk: usize,
    cmd: u32,
    sector: u32,
    count: u32,
    dma: u32,
    /// What a write sent: the `count` sectors at `dma`.
    data: Vec<u8>,
}

/// What a run did with the disks, instruction by instruction.
#[derive(Debug, Default)]
struct DiskTrace {
    commands: Vec<DiskCommand>,
    /// `BLK_ERROR` as each acknowledgement in `blk_intr` found it, in order.
    results: Vec<u32>,
    /// Commands written while the disk's previous result was still
    /// unacknowledged (`BLK_STATUS.DONE` set): there must be none.
    overlapping: usize,
    /// Reads of `BLK_STATUS`: polling, which the driver never does.
    status_reads: usize,
    /// Reads of `BLK_ERROR` outside `blk_intr`.
    stray_error_reads: usize,
    /// The channel (a0) of each call to `sleep`.
    sleeps: Vec<u32>,
    /// Every load from or store to a disk register.
    accesses: usize,
}

/// A disk register access by the instruction about to run: (disk, offset,
/// store?). The kernel maps the disks identity, so the address is physical.
fn disk_access(cpu: &grail::cpu::Cpu) -> Option<(usize, u32, bool)> {
    let a = mem_access(cpu)?;
    let end = BLK_BASE + BLK_SIZE * MAX_DISKS as u32;
    (BLK_BASE..end).contains(&a.addr).then(|| {
        let off = a.addr - BLK_BASE;
        ((off / BLK_SIZE) as usize, off % BLK_SIZE, a.load_rd.is_none())
    })
}

/// Runs `m` (loaded with the committed kernel) for at most `max_steps`,
/// watching every disk access.
fn trace_disks(m: &mut Machine, elf: &grail::elf::Elf, max_steps: u64) -> (RunResult, DiskTrace) {
    let blk_intr = function("blk_intr");
    let sleep = sym(elf, "sleep");
    let mut t = DiskTrace::default();
    let mut pending_cmd = None;
    let r = m.run(Some(max_steps), |cpu| {
        // The command stored by the previous instruction has run by now: a
        // write's data is still in RAM, so take it from there either way.
        if let Some((disk, cmd)) = pending_cmd.take() {
            let d: &grail::block::BlockDevice = &cpu.bus.disks[disk];
            let data = if cmd == BLK_CMD_WRITE {
                let start = (d.dma - RAM_BASE) as usize;
                cpu.bus.ram[start..start + d.count as usize * 512].to_vec()
            } else {
                Vec::new()
            };
            t.commands.push(DiskCommand {
                disk,
                cmd,
                sector: d.sector,
                count: d.count,
                dma: d.dma,
                data,
            });
        }
        if cpu.pc == sleep {
            t.sleeps.push(cpu.x[x("a0") as usize]);
        }
        let Some((disk, off, store)) = disk_access(cpu) else {
            return;
        };
        t.accesses += 1;
        match (off, store) {
            (BLK_REG_CMD, true) => {
                if cpu.bus.disks[disk].done() {
                    t.overlapping += 1;
                }
                pending_cmd = Some((disk, mem_access(cpu).unwrap().value));
            }
            (BLK_REG_STATUS, false) => t.status_reads += 1,
            (BLK_REG_ERROR, false) if !blk_intr.contains(&cpu.pc) => t.stray_error_reads += 1,
            (BLK_REG_STATUS, true) if blk_intr.contains(&cpu.pc) => {
                t.results.push(cpu.bus.disks[disk].error());
            }
            _ => {}
        }
    });
    (r, t)
}

/// Disk `i`'s whole contents, read through its backend.
fn disk_contents(m: &mut Machine, i: usize) -> Vec<u8> {
    let d = &mut m.cpu.bus.disks[i];
    let mut buf = vec![0; d.capacity() as usize * 512];
    d.backend().read(0, &mut buf).unwrap();
    buf
}

#[test]
fn kernel_boots_without_a_disk_and_says_so() {
    // No disk: one line says so, no self-test runs, the disks' PLIC sources
    // stay off and no disk register is touched.
    let (mut m, elf) = load_committed();
    let (r, t) = trace_disks(&mut m, &elf, BOOT_MAX_STEPS);
    assert_eq!(r, RunResult::Exited(0));
    let out = kernel::mask_dbg_pc(&printed(&m));
    assert_eq!(out, halted_output());
    assert!(out.contains("timebase\ndisk: none\ntick 1\n"), "{out}");
    assert!(t.commands.is_empty() && t.sleeps.is_empty(), "{t:?}");
    assert_eq!(t.accesses, 0, "{t:?}");
    assert_eq!(m.cpu.bus.read(plic_enable(1), 4), Ok(1 << UART_IRQ));
}

#[test]
fn disk_blocks_round_trip_through_the_cache_with_interrupts() {
    // Issue #65's acceptance test: the boot self-test reads, writes and reads
    // back a scratch block of an in-memory disk through the buffer cache,
    // each request completing by interrupt, and leaves the disk as it was.
    let image = disk_bytes(DISK_SECTORS);
    let (mut m, elf) = load_with_disks(vec![DiskConfig::memory(image.clone())]);
    let (r, t) = trace_disks(&mut m, &elf, BOOT_MAX_STEPS * 2);
    let out = printed(&m);
    assert_eq!(r, RunResult::Exited(0), "output:\n{out}");
    assert_eq!(
        kernel::mask_dbg_pc(&out),
        boot_output_with(
            "disk 0: 128 sectors (64 KiB), irq 3\n",
            &format!("blk self-test: {BLK_SELFTEST_CHECKS}/{BLK_SELFTEST_CHECKS} passed\n")
        )
    );

    // The driver set up the disk's completion interrupt.
    assert_eq!(m.cpu.bus.disks[0].ctrl, 1);
    assert_eq!(m.cpu.bus.read(plic_priority(BLK_IRQ), 4), Ok(1));
    assert_eq!(
        m.cpu.bus.read(plic_enable(1), 4),
        Ok(1 << UART_IRQ | 1 << BLK_IRQ)
    );

    // One request at a time, each finished by its interrupt: every command
    // was acknowledged in blk_intr before the next, with nothing polled.
    assert!(t.commands.len() > 30, "{} commands", t.commands.len());
    assert_eq!(t.overlapping, 0);
    assert_eq!(t.status_reads, 0);
    assert_eq!(t.stray_error_reads, 0);
    assert_eq!(t.results.len(), t.commands.len(), "{t:?}");

    // The writes: the pattern to the scratch block, then its old contents.
    let writes: Vec<&DiskCommand> = t.commands.iter().filter(|c| c.cmd == BLK_CMD_WRITE).collect();
    assert_eq!(writes.len(), 2, "{writes:?}");
    for w in &writes {
        assert_eq!((w.disk, w.sector, w.count), (0, SCRATCH_SECTOR, BLOCK_SECTORS));
    }
    assert_eq!(writes[0].data, blk_pattern());
    let scratch = SCRATCH_SECTOR as usize * 512;
    assert_eq!(writes[1].data, image[scratch..scratch + BSIZE]);
    assert_eq!(disk_contents(&mut m, 0), image);
    assert_eq!(t.commands.iter().filter(|c| c.cmd == BLK_CMD_FLUSH).count(), 1);

    // The errors reached the kernel: the block past the end (twice, since a
    // failed read isn't cached) and the buffer outside RAM. Everything else
    // worked.
    let failed: Vec<(u32, u32, u32)> = t
        .commands
        .iter()
        .zip(&t.results)
        .filter(|(_, e)| **e != 0)
        .map(|(c, &e)| (c.sector, c.dma, e))
        .collect();
    let past_end = DISK_SECTORS as u32;
    assert_eq!(failed.len(), 3, "{failed:?}");
    assert_eq!((failed[0].0, failed[0].2), (past_end, blk_error::OUT_OF_RANGE));
    assert_eq!((failed[1].0, failed[1].2), (past_end, blk_error::OUT_OF_RANGE));
    assert_eq!((failed[2].1, failed[2].2), (0x1000, blk_error::BAD_DMA));

    // The cache: every bread() that missed read one block into a buffer, and
    // the hits needed no request at all.
    let bufs = function("bufs");
    let buffer_reads = t
        .commands
        .iter()
        .filter(|c| c.cmd == BLK_CMD_READ && bufs.contains(&c.dma))
        .count() as u32;
    let (hits, misses) = (
        m.cpu.bus.peek(sym(&elf, "bio_hits")).unwrap(),
        m.cpu.bus.peek(sym(&elf, "bio_misses")).unwrap(),
    );
    assert_eq!(misses, buffer_reads);
    // Reader c, the scratch block read back and read again to restore it;
    // in the LRU check blocks 0 and 1 (left there by the readers) at the
    // start of its loop, block 0 again after it, then blocks 0 and NBUF;
    // last, the filesystem's mount looking for a superblock in block 1,
    // which the LRU check read last.
    assert_eq!(hits, 9);
    assert!(bufs.len() >= NBUF as usize * BSIZE);

    // Which buffer each miss took. A buffer whose read failed holds nothing,
    // so the next miss reuses it before any cached block: the second read
    // past the end, then block 2 in the LRU loop, take the buffer of the
    // first.
    let block_reads: Vec<(u32, u32)> = t
        .commands
        .iter()
        .filter(|c| c.cmd == BLK_CMD_READ && bufs.contains(&c.dma))
        .map(|c| (c.sector / BLOCK_SECTORS, c.dma))
        .collect();
    let read_at = |block: u32| {
        block_reads
            .iter()
            .position(|&(b, _)| b == block)
            .unwrap_or_else(|| panic!("block {block} never read: {block_reads:?}"))
    };
    let end = DISK_SECTORS as u32 / BLOCK_SECTORS;
    let first_fail = read_at(end);
    let failed_buf = block_reads[first_fail].1;
    assert_eq!(block_reads[first_fail + 1], (end, failed_buf));
    assert_eq!(block_reads[first_fail + 2], (2, failed_buf));
    // LRU, not MRU: after blocks 0 to NBUF - 1, then block 0 again, block
    // NBUF takes the buffer block 1 was read into (by reader b), and the
    // last miss reads block 1 back.
    let block1 = block_reads[read_at(1)].1;
    assert_eq!(block_reads[read_at(NBUF)], (NBUF, block1));
    assert_eq!(block_reads.last().unwrap().0, 1);

    // The requests slept: on the disk, once per request for its interrupt
    // and once more for reader b, which found reader a's request in flight;
    // and reader c on block 0's buffer, which reader a held.
    let disk0 = sym(&elf, "disks");
    let on_disk = t.sleeps.iter().filter(|&&c| c == disk0).count();
    assert_eq!(on_disk, t.commands.len() + 1, "{:x?}", t.sleeps);
    let on_buffers = t.sleeps.iter().filter(|&&c| bufs.contains(&c)).count();
    assert_eq!(on_buffers, 1, "{:x?}", t.sleeps);
    assert_eq!(on_disk + on_buffers, t.sleeps.len());
}

#[test]
fn kernel_processes_give_back_their_pages() {
    // The self-test's four kernel processes are gone after it, with their
    // kernel stacks: the shell starts with as many free pages as without a
    // disk.
    let (mut plain, elf) = load_committed();
    step_until(
        &mut plain,
        BOOT_MAX_STEPS,
        |_| {},
        |m| printed(m).ends_with(PROMPT),
    );
    let (mut with_disk, _) = load_with_disks(vec![DiskConfig::memory(disk_bytes(DISK_SECTORS))]);
    step_until(
        &mut with_disk,
        BOOT_MAX_STEPS * 2,
        |_| {},
        |m| printed(m).ends_with(PROMPT),
    );
    assert!(printed(&with_disk).contains(" passed\n"));
    assert_eq!(free_pages(&with_disk, &elf), free_pages(&plain, &elf));
}

#[test]
fn a_slow_disk_leaves_the_kernel_asleep_in_wfi() {
    // Grail's disks finish each command at once. Here the first one's
    // interrupt is held back (BLK_CTRL cleared right after the command) for
    // a few ticks, as a slow disk would: every process is then asleep and
    // the scheduler idles in wfi, waking for each tick and nothing else.
    let (mut m, elf) = load_with_disks(vec![DiskConfig::memory(disk_bytes(DISK_SECTORS))]);
    let ticks = sym(&elf, "ticks");
    let tick_count = |m: &Machine| m.cpu.bus.peek(ticks).unwrap();
    for _ in 0..BOOT_MAX_STEPS {
        let cmd = matches!(disk_access(&m.cpu), Some((0, BLK_REG_CMD, true)));
        m.cpu.step();
        if cmd {
            break;
        }
    }
    assert!(m.cpu.bus.disks[0].done(), "no disk command");
    m.cpu.bus.disks[0].ctrl = 0;

    let wfi = sym(&elf, "wfi");
    let mut wfis = 0;
    let start = tick_count(&m);
    let steps = step_until(
        &mut m,
        BOOT_MAX_STEPS,
        |cpu| {
            if cpu.pc == wfi {
                wfis += 1;
            }
        },
        |m| tick_count(m) == start + 5,
    );
    // Polling would spend a whole tick interval (1 000 000 steps) per tick.
    assert!(steps < 5 * 1000, "{steps} steps for 5 ticks");
    assert!(wfis >= 5, "{wfis} wfi");
    assert!(
        m.cpu.bus.disks[0].done(),
        "the result was taken without the interrupt"
    );

    // The interrupt comes at last, and the self-test carries on.
    m.cpu.bus.disks[0].ctrl = 1;
    assert_eq!(m.run(Some(BOOT_MAX_STEPS * 2), |_| {}), RunResult::Exited(0));
    let want = format!("blk self-test: {BLK_SELFTEST_CHECKS}/{BLK_SELFTEST_CHECKS} passed\n");
    assert!(printed(&m).contains(&want), "{}", printed(&m));
}

#[test]
fn a_read_only_disk_refuses_the_write_and_stays_as_it_was() {
    let image = disk_bytes(DISK_SECTORS);
    let (mut m, elf) = load_with_disks(vec![DiskConfig::memory(image.clone()).with_read_only(true)]);
    let (r, t) = trace_disks(&mut m, &elf, BOOT_MAX_STEPS * 2);
    let out = printed(&m);
    assert_eq!(r, RunResult::Exited(0), "output:\n{out}");
    // No restore and no flush: 4 + 5 + 3 + 2 checks.
    assert_eq!(
        kernel::mask_dbg_pc(&out),
        boot_output_with(
            "disk 0: 128 sectors (64 KiB), irq 3, read-only\n",
            "blk self-test: 14/14 passed\n"
        )
    );
    let writes: Vec<(&DiskCommand, u32)> = t
        .commands
        .iter()
        .zip(t.results.iter().copied())
        .filter(|(c, _)| c.cmd == BLK_CMD_WRITE)
        .collect();
    assert_eq!(writes.len(), 1);
    assert_eq!(writes[0].1, blk_error::READ_ONLY);
    assert_eq!(disk_contents(&mut m, 0), image);
}

/// A disk whose writes all fail on the host side.
struct WritesFail(MemDisk);

impl BlockBackend for WritesFail {
    fn capacity(&self) -> u64 {
        self.0.capacity()
    }
    fn read(&mut self, sector: u64, buf: &mut [u8]) -> std::io::Result<()> {
        self.0.read(sector, buf)
    }
    fn write(&mut self, _: u64, _: &[u8]) -> std::io::Result<()> {
        Err(std::io::Error::other("the host disk is broken"))
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn a_disk_error_reaches_the_self_test_and_the_exit_code() {
    // Writes fail with BLK_ERR_IO: the self-test sees the error (and that
    // the block wasn't written), says what failed, and kmain's exit code
    // counts the failures.
    let image = disk_bytes(DISK_SECTORS);
    let disk = WritesFail(MemDisk::from_image(image.clone()).unwrap());
    let mut m = Machine::with_backends(MachineConfig::default(), vec![Box::new(disk)]).unwrap();
    m.cpu.bus.uart.capture = true;
    let file = committed_elf();
    let elf = grail::elf::parse(&file).unwrap();
    m.load_elf(&file).unwrap();
    m.set_uart_input(kernel::typed(EXIT));
    let (r, t) = trace_disks(&mut m, &elf, BOOT_MAX_STEPS * 2);
    let out = printed(&m);
    let failures: Vec<&str> = out
        .lines()
        .filter_map(|l| l.strip_prefix("blk self-test FAILED: "))
        .collect();
    assert_eq!(
        failures,
        [
            "write the scratch block",
            "read it back from the cache",
            "the disk holds the written block",
            "restore the scratch block",
        ],
        "{out}"
    );
    assert!(out.contains("blk self-test: 12/16 passed\n"), "{out}");
    assert_eq!(r, RunResult::Exited(4));
    assert!(t.results.contains(&blk_error::IO), "{t:?}");
    assert_eq!(disk_contents(&mut m, 0), image);
}

#[test]
fn a_disk_smaller_than_two_blocks_is_not_tested() {
    // The self-test needs two whole blocks: reader b reads block 1, and the
    // scratch block (the last) must not be block 0. A disk of 1 sector has
    // no whole block, one of 2 or 3 sectors only one: it is listed, but not
    // tested, and left alone. 4 sectors are tested (see the next tests).
    for sectors in [1, 2, 3] {
        let image = disk_bytes(sectors);
        let (mut m, elf) = load_with_disks(vec![DiskConfig::memory(image.clone())]);
        let (r, t) = trace_disks(&mut m, &elf, BOOT_MAX_STEPS);
        assert_eq!(r, RunResult::Exited(0), "{sectors} sectors");
        let kib = sectors / 2;
        assert_eq!(
            kernel::mask_dbg_pc(&printed(&m)),
            boot_output_with(
                &format!("disk 0: {sectors} sectors ({kib} KiB), irq 3\n"),
                "blk self-test: disk 0 is too small, skipped\n"
            )
        );
        assert!(t.commands.is_empty(), "{sectors} sectors: {t:?}");
        assert_eq!(disk_contents(&mut m, 0), image);
    }
}

#[test]
fn a_disk_entry_without_a_usable_interrupt_is_skipped() {
    // The driver doesn't poll, so a disk whose sysinfo entry has no PLIC
    // source (0), or one past the PLIC's 31, isn't used: the kernel boots as
    // without a disk and never touches its registers.
    for irq in [0, 32] {
        let (mut m, elf) = load_with_disks(vec![DiskConfig::memory(disk_bytes(DISK_SECTORS))]);
        let devices: Vec<DeviceEntry> = m
            .cpu
            .bus
            .sysinfo
            .devices()
            .iter()
            .map(|d| match d.kind {
                DeviceType::Block => DeviceEntry { irq, ..*d },
                _ => *d,
            })
            .collect();
        assert!(devices.iter().any(|d| d.kind == DeviceType::Block));
        m.cpu.bus.sysinfo = Sysinfo::new(m.config(), devices);
        let (r, t) = trace_disks(&mut m, &elf, BOOT_MAX_STEPS);
        assert_eq!(r, RunResult::Exited(0), "irq {irq}");
        assert_eq!(kernel::mask_dbg_pc(&printed(&m)), halted_output(), "irq {irq}");
        assert_eq!(t.accesses, 0, "irq {irq}: {t:?}");
        assert_eq!(m.cpu.bus.disks[0].ctrl, 0);
        assert_eq!(m.cpu.bus.read(plic_enable(1), 4), Ok(1 << UART_IRQ));
    }
}

#[test]
fn every_disk_is_found_and_disk_0_is_tested() {
    // Two blocks are enough for the self-test without its LRU check; the
    // second disk is listed and its interrupt enabled, but left alone.
    let second = disk_bytes(16);
    let (mut m, elf) = load_with_disks(vec![
        DiskConfig::memory(disk_bytes(4)),
        DiskConfig::memory(second.clone()),
    ]);
    let (r, t) = trace_disks(&mut m, &elf, BOOT_MAX_STEPS * 2);
    assert_eq!(r, RunResult::Exited(0));
    assert_eq!(
        kernel::mask_dbg_pc(&printed(&m)),
        boot_output_with(
            "disk 0: 4 sectors (2 KiB), irq 3\ndisk 1: 16 sectors (8 KiB), irq 4\n",
            "blk self-test: 14/14 passed\n"
        )
    );
    assert!(t.commands.iter().all(|c| c.disk == 0));
    assert_eq!(m.cpu.bus.disks[1].ctrl, 1);
    assert_eq!(
        m.cpu.bus.read(plic_enable(1), 4),
        Ok(1 << UART_IRQ | 1 << BLK_IRQ | 1 << (BLK_IRQ + 1))
    );
    assert_eq!(disk_contents(&mut m, 1), second);
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
        Ok(v) if v == recorded.trim() => {
            assert!(
                elf == committed_elf(),
                "kernel/kernel.elf is out of date: run `cargo xtask kernel` and commit it"
            );
            // And the user programs' C, compiled by the same clang, is the
            // committed assembly.
            let dir = xtask::userprogs::user_dir(&repo_root());
            let compiled =
                xtask::userprogs::compile_c(&probe.clang, &dir, &scratch("userprogs"), true).unwrap();
            assert!(!compiled.is_empty());
            for (rel, text) in compiled {
                let committed = std::fs::read_to_string(dir.join(&rel)).unwrap_or_default();
                assert!(
                    committed == text,
                    "kernel/user/{rel} is out of date: run `cargo xtask kernel` and commit it"
                );
            }
        }
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
    // merlin-as, C by clang, linked by ld.lld.
    let Some((clang, lld)) = llvm() else {
        eprintln!("skipped: clang/ld.lld not found");
        return;
    };
    assert_boots(&fresh_build(clang, lld, Assembler::Merlin, "merlin").unwrap());
}
