//! Kernel tests. The committed `kernel/kernel.elf` always runs on Grail, so
//! these pass without LLVM. When clang and ld.lld are installed, the kernel is
//! also rebuilt from source and checked against the committed image.

use excalibur::csr::{
    IRQ_M_TIMER, IRQ_S_EXT, IRQ_S_SOFT, IRQ_S_TIMER, MSTATUS_SIE, MSTATUS_SPIE, MSTATUS_SPP,
};
use grail::cpu::Mode;
use grail::trap::INTERRUPT_BIT;
use grail::{Machine, RunResult};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use xtask::kernel::{self, Assembler, Build, Lang, BOOT_MAX_STEPS};
use xtask::{repo_root, tools};

/// What the kernel prints, with `xcal.dbg`'s pc masked (see `mask_dbg_pc`).
const BOOT_OUTPUT: &str = "\
Camelot v0.1.0 booting...
kprintf: d=-42 u=4294967295 x=cafef00d p=0x80000000 s=excalibur c=K %=%
kprintf self-test: 26/26 passed
[dbg] pc=........ a0 = 0x00000000 (0)
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

/// Exit code of a kernel panic (`PANIC_EXIT_CODE` in kernel/include/trap.h).
const PANIC_EXIT: u32 = 0xfe;

/// Values of the kernel's `boot_fault` (kernel/main.c).
const BOOT_FAULT_NONE: u32 = 1;
const BOOT_FAULT_ILLEGAL: u32 = 2;
const BOOT_FAULT_LOAD: u32 = 3;
/// Where `BOOT_FAULT_LOAD` loads from: below the boot ROM, nothing is mapped.
const UNMAPPED_ADDR: u32 = 0x100;

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
    let mut m = Machine::new(grail::DEFAULT_RAM_SIZE);
    m.cpu.bus.uart.capture = true;
    m.load_elf(&file).unwrap();
    (m, elf)
}

fn sym(elf: &grail::elf::Elf, name: &str) -> u32 {
    elf.symbol(name).unwrap_or_else(|| panic!("no symbol {name}"))
}

#[test]
fn committed_kernel_boots() {
    assert_boots(&committed_elf());
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
    ] {
        assert!(sym(&elf, s) > 0x8000_0000, "{s}");
    }
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
    let values: Vec<&str> = out.lines().map(|l| l.split(" = ").nth(1).unwrap_or(l)).collect();
    assert_eq!(
        values,
        [
            "0x00000002 (2)".to_string(),                // mcause: illegal instruction
            format!("{start:#010x} ({})", start as i32), // mepc
            "0x00000000 (0)".to_string(),                // mtval: the instruction bits
        ],
        "output:\n{out}"
    );
    assert_eq!(r, RunResult::Exited(0xff));
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
    let (a0, a1, a7) = (10, 11, 17);
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
    assert_eq!(m.cpu.bus.peek(scratch + 12), Some(TICK_INTERVAL as u32));
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
    let mut kernel_ticks = Vec::new();
    let mut steps = 0u64;
    let mut halts = 0;
    let r = m.run(Some(BOOT_MAX_STEPS), |cpu| {
        steps += 1;
        if cpu.pc == machine_vec {
            if cpu.csr.mcause == INTERRUPT_BIT | IRQ_M_TIMER {
                machine_ticks.push(cpu.bus.clint.mtime);
            } else {
                // The only other trap to M-mode: the final MCALL_HALT.
                assert_eq!(cpu.csr.mcause, ECALL_FROM_S);
                halts += 1;
            }
        }
        if cpu.pc == kernel_vec {
            assert_eq!(cpu.csr.scause, INTERRUPT_BIT | IRQ_S_SOFT);
            kernel_ticks.push(cpu.bus.clint.mtime);
        }
    });
    assert_eq!(r, RunResult::Exited(0));
    assert_eq!(halts, 1);
    assert_eq!(machine_ticks.len(), 10);
    assert_eq!(kernel_ticks.len(), 10);
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
    assert!(steps < TICK_INTERVAL, "{steps} steps");
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
fn parse_panic(out: &str) -> (String, HashMap<String, u32>) {
    let report = &out[out
        .find("panic: ")
        .unwrap_or_else(|| panic!("no panic in:\n{out}"))..];
    let mut lines = report.lines();
    let cause = lines.next().unwrap()["panic: ".len()..].to_string();
    let mut values = HashMap::new();
    let mut n = 0;
    for line in lines {
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

    let (cause, v) = parse_panic(&out);
    assert_eq!(cause, "load access fault");
    assert_eq!(v["scause"], 5);
    assert_eq!(v["stval"], UNMAPPED_ADDR);
    let sepc = v["sepc"];
    assert!(sepc > sym(&elf, "_start") && sepc < sym(&elf, "__kernel_end"));
    let word = m.cpu.bus.peek(sepc).unwrap();
    assert!(
        matches!(excalibur::decode(word), excalibur::Inst::Load { .. }),
        "sepc {sepc:#x} holds {}",
        excalibur::decode(word)
    );
    assert_ne!(v["sstatus"] & MSTATUS_SPP, 0);
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
