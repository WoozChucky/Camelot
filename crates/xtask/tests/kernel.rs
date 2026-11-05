//! Kernel tests. The committed `kernel/kernel.elf` always runs on Grail, so
//! these pass without LLVM. When clang and ld.lld are installed, the kernel
//! is also rebuilt from source with them and checked against merlin's build.
//!
//! The kernel is a higher-half kernel in Linux's riscv64 layout (#154): the
//! image is linked at 0xffff_ffff_8000_0000 and loaded at 0x8020_0000, after
//! the firmware's 2 MiB, and runs in S-mode on Camelot's firmware (`--bios
//! default`), through its boot self-tests and its kernel processes, on 1 to
//! 8 harts, then runs the shell, a 64-bit program, as the first process
//! (#155): the boot ends when it exits (`exit`, typed on the UART), and the
//! exit code is the number of failed self-tests, else the shell's status.
//! Its symbols are link addresses, which `common::image_pa` turns into the
//! physical addresses the bus takes; a pointer the kernel computes at run
//! time goes through the hart's page table (`common::kpa`).
//!
//! The tests of processes, their address spaces and the system calls are in
//! `processes.rs`.

mod common;

use common::{
    function, image_pa, kpa, load_with, machine_config, peek, printed, sym, CPU_HARTID, CPU_NOFF,
    CPU_PAGETABLE, CPU_SIZE, CPU_THREAD, KERNEL_LINK_ADDR, LOAD_ADDR, LOCK_CPU, PROMPT,
};
use excalibur::csr::{
    IRQ_M_EXT, IRQ_M_SOFT, IRQ_M_TIMER, IRQ_S_EXT, IRQ_S_SOFT, IRQ_S_TIMER, MSTATUS_SIE, MSTATUS_SPIE,
    MSTATUS_SPP,
};
use excalibur::{reg, Xlen};
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
use grail::{DiskConfig, Machine, MachineConfig, RunResult};
use std::cell::RefCell;
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use xtask::kernel::{self, Assembler, Lang, BOOT_MAX_STEPS};
use xtask::userprogs::CCompiler;
use xtask::{repo_root, tools};

/// The interrupt bit of mcause and scause: bit 63.
const INTERRUPT_BIT: u64 = 1 << 63;

/// The RAM of the tests' machines (`kernel::RAM_SIZE`): the firmware's 2
/// MiB, the image's 2.1 MiB from 0x8020_0000, and the device tree at the
/// last 2 MiB boundary below the end.
const RAM: usize = kernel::RAM_SIZE;

/// What depends on the paging mode the kernel chose (vm.h, Linux's
/// vm-layout.rst): Sv48 on a hart that has it, Sv39 on one without.
#[derive(Clone, Copy, Debug)]
struct PagingMode {
    paging: grail::Paging,
    name: &'static str,
    levels: u32,
    satp_mode: u64,
    /// The direct map's base (Linux's PAGE_OFFSET).
    page_offset: u64,
    /// The vmalloc area's start, where the kernel stacks begin, and the
    /// devices' mappings after them.
    vmalloc_start: u64,
    /// The kernel's root entries in use at the end of the boot: the fixmap,
    /// the vmalloc area (the kernel stacks and the devices), the direct map
    /// of 8 MiB of RAM, the image.
    root_entries: &'static [usize],
}

const SV48: PagingMode = PagingMode {
    paging: grail::Paging::Sv48,
    name: "Sv48",
    levels: 4,
    satp_mode: 9 << 60,
    page_offset: 0xffff_af80_0000_0000,
    vmalloc_start: 0xffff_8f80_0000_0000,
    root_entries: &[282, 287, 351, 511],
};

const SV39: PagingMode = PagingMode {
    paging: grail::Paging::Sv39,
    name: "Sv39",
    levels: 3,
    satp_mode: 8 << 60,
    page_offset: 0xffff_ffd6_0000_0000,
    vmalloc_start: 0xffff_ffc6_0000_0000,
    root_entries: &[275, 280, 344, 510],
};

thread_local! {
    /// The paging mode the harts of this thread's machines have: Sv48 (and
    /// Sv39), or Sv39 only (`the_kernel_falls_back_to_sv39`).
    static MODE: std::cell::Cell<PagingMode> = const { std::cell::Cell::new(SV48) };
    /// The kernel the tests on this thread run: the committed one, or a
    /// reference build (`the_reference_builds_pass_the_same_tests`).
    static UNDER_TEST: RefCell<Option<Vec<u8>>> = const { RefCell::new(None) };
    /// A reference build's clang assembly of the user programs, which its
    /// bootable disk's /bin holds built (`bootable_image`).
    static UNDER_TEST_USER_ASM: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
}

fn mode() -> PagingMode {
    MODE.get()
}

/// The kernel stacks' size and slot, and how many slots there are, at the
/// start of the vmalloc area (vm.h).
const KSTACK_SIZE: u64 = 16 * 1024;
const KSTACK_SLOT: u64 = 2 * KSTACK_SIZE;
const KSTACK_SLOTS: u64 = 64;

/// What the kernel prints before its kstack self-test: the boot up to the
/// tenth tick, with `xcal.dbg`'s pc masked (`kernel::mask_dbg_pc`), for the
/// mode the harts have.
const BOOT_OUTPUT_TEMPLATE: &str = concat!(
    xtask::banner!(),
    "\
Camelot v0.1.0 booting...
sbi: v2.0, implementation 0x434d4c54 version 0x1
sbi: extensions TIME IPI HSM SRST DBCN
kprintf: d=-42 u=4294967295 x=cafef00d p=0x0000000080000000 s=excalibur c=K %=%
kprintf self-test: 33/33 passed
[dbg] pc=................ a0 = 0x0000000000000000 (0)
ram: 8 MiB at 0x80000000
paging: on (MODE)
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
"
);

/// The boot's output to the ticks for the mode the harts have.
fn boot_output() -> String {
    BOOT_OUTPUT_TEMPLATE.replace("(MODE)", &format!("({})", mode().name))
}

/// What the kernel prints after its boot and before the shell: the kernel
/// stacks' self-test, and the deepest use of a kernel stack, which `mask`
/// writes as N.
const KSTACK_OUTPUT: &str = "kstack self-test: 3/3 passed\nkstack: N of 16384 bytes used at most\n";

/// What the user-space shell, the first process, prints before its first
/// prompt (kernel/user/sh.c).
const SHELL_BANNER: &str = "Type help for the list of commands.\n";

/// The input most runs get: the shell's command to leave, after which the
/// kernel stops the machine.
const EXIT: &[u8] = b"exit\n";

/// The whole output of a run whose shell is given `EXIT`, masked (`mask`).
fn halted_output() -> String {
    format!("{}{KSTACK_OUTPUT}{SHELL_BANNER}{PROMPT}exit\n", boot_output())
}

/// `out` with what changes from run to run masked: `xcal.dbg`'s pc, and the
/// bytes of kernel stack the deepest use took.
fn mask(out: &str) -> String {
    common::mask(out)
}

/// mtime ticks between timer interrupts: the nominal 10 MHz timebase / 10 Hz.
const TICK_INTERVAL: u64 = 1_000_000;

/// More than the steps from reset to `kmain`: the firmware's coldboot,
/// `_start` zeroing `.bss` a doubleword at a time (the buffer cache's 54 KiB
/// live there), and setup_vm() building the early page table.
const TO_KMAIN_STEPS: u64 = 400_000;

// Kernel constants, from the kernel sources named in each comment.
// crates/grail/tests/kernel.rs has copies of some: keep them in sync.

/// Exit code of a kernel panic (`PANIC_EXIT_CODE` in kernel/include/trap.h).
const PANIC_EXIT: u32 = 0xfe;

/// Exit code after a trap before the kernel's trap vector is set
/// (`EXIT_UNEXPECTED_TRAP` in kernel/include/trap.h, early_s_trap).
const UNEXPECTED_TRAP_EXIT: u32 = 0xff;

/// Each hart's next tick deadline, a u64 (`next_tick` in kernel/timer.c),
/// and the timebase the kernel read from sysinfo (`timebase`, a u32).
const NEXT_TICK: &str = "next_tick";
const TIMEBASE: &str = "timebase";

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
const BOOT_FAULT_DOUBLE_ACQUIRE: u32 = 12;
const BOOT_FAULT_RELEASE_UNHELD: u32 = 13;
const BOOT_FAULT_YIELD_HOLDING: u32 = 14;
const BOOT_FAULT_POP_UNMATCHED: u32 = 15;
const BOOT_FAULT_POP_INTR_ON: u32 = 16;
const BOOT_FAULT_KSTACK_OVERFLOW: u32 = 17;
const BOOT_FAULT_PA_VMALLOC: u32 = 18;
const BOOT_FAULT_BOOT_STACK_OVERFLOW: u32 = 19;
/// Where `BOOT_FAULT_LOAD` loads from: below the boot ROM, nothing is mapped.
const UNMAPPED_ADDR: u64 = 0x100;

/// mcause of an ecall from S-mode.
const ECALL_FROM_S: u64 = grail::trap::Exception::EcallS as u64;
/// scause of a page fault on a load and on a store.
const LOAD_PAGE_FAULT: u64 = grail::trap::Exception::LoadPageFault as u64;
const STORE_PAGE_FAULT: u64 = grail::trap::Exception::StorePageFault as u64;
/// scause of an illegal instruction.
const ILLEGAL_INSTRUCTION: u64 = grail::trap::Exception::IllegalInstr as u64;

const PAGE: u64 = 4096;

fn kernel_dir() -> PathBuf {
    repo_root().join("kernel")
}

fn committed_elf() -> Vec<u8> {
    common::committed_elf()
}

/// The kernel under test.
fn under_test() -> Vec<u8> {
    UNDER_TEST
        .with(|u| u.borrow().clone())
        .unwrap_or_else(committed_elf)
}

/// The kernel a bootable disk holds: the committed disk kernel, without the
/// programs (they are in the image's /bin); a reference build's own, which
/// has them, when one is under test (from a mounted disk it runs /bin's).
fn disk_kernel_under_test() -> Vec<u8> {
    UNDER_TEST.with(|u| u.borrow().clone()).unwrap_or_else(|| {
        let path = kernel_dir().join(kernel::DISK_KERNEL);
        std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
    })
}

/// The machine the tests boot the kernel on: [`RAM`], Camelot's firmware,
/// the paging mode under test, one hart.
fn config() -> MachineConfig {
    MachineConfig {
        paging: mode().paging,
        ..machine_config(1)
    }
}

fn assert_boots(elf: &[u8]) {
    let mut m = Machine::new(config()).unwrap();
    m.cpu.bus.uart.capture = true;
    m.load_elf(elf).unwrap();
    m.set_uart_input(kernel::typed(EXIT));
    let r = m.run(Some(BOOT_MAX_STEPS), |_| {});
    let out = printed(&m);
    assert_eq!(mask(&out), halted_output(), "raw output:\n{out}");
    assert_eq!(r, RunResult::Exited(0));
}

/// Loads the kernel under test without running it, with `EXIT` typed on the
/// UART so that a full run ends.
fn load_committed() -> (Machine, grail::elf::Elf) {
    load_committed_on(config(), Some(kernel::typed(EXIT)))
}

/// Loads the kernel under test into a machine built from `config`, with the
/// UART reading `input` (nothing at all if `None`).
fn load_committed_on(config: MachineConfig, input: Option<Receiver<u8>>) -> (Machine, grail::elf::Elf) {
    let file = under_test();
    let elf = grail::elf::parse(&file).unwrap();
    let mut m = Machine::new(config).unwrap();
    m.cpu.bus.uart.capture = true;
    m.load_elf(&file).unwrap();
    if let Some(input) = input {
        m.set_uart_input(input);
    }
    (m, elf)
}

/// Writes the u32 `v` at `va`, an address in the image, before the kernel
/// runs (or while paging maps the image where it is linked).
fn poke_image(m: &mut Machine, va: u64, bytes: &[u8]) {
    m.cpu.bus.load(image_pa(va), bytes).unwrap();
}

/// The u32 at `va` in the image.
fn image_u32(m: &Machine, va: u64) -> u32 {
    m.cpu.bus.peek(image_pa(va)).unwrap()
}

/// The u64 at `va` in the image.
fn image_u64(m: &Machine, va: u64) -> u64 {
    let pa = image_pa(va);
    u64::from(m.cpu.bus.peek(pa).unwrap()) | u64::from(m.cpu.bus.peek(pa + 4).unwrap()) << 32
}

/// The u64 at physical address `pa`.
fn read64_pa(m: &Machine, pa: u64) -> u64 {
    u64::from(m.cpu.bus.peek(pa).unwrap()) | u64::from(m.cpu.bus.peek(pa + 4).unwrap()) << 32
}

/// Steps the machine until it is about to execute (or take a trap at) `pc`.
fn step_to(m: &mut Machine, pc: u64) {
    for _ in 0..TO_KMAIN_STEPS {
        if m.cpu.pc == pc {
            return;
        }
        m.cpu.step();
    }
    panic!("never reached {pc:#x}");
}

/// Steps the machine until it is about to execute `pc`, which may be past
/// the whole early boot (step_to stops at TO_KMAIN_STEPS).
fn step_to_far(m: &mut Machine, pc: u64) {
    step_until(m, BOOT_MAX_STEPS, |_| {}, |m| m.cpu.pc == pc);
    assert_eq!(m.cpu.pc, pc, "never reached {pc:#x}");
}

/// Register number of an ABI name.
fn x(name: &str) -> u32 {
    u32::from(reg::parse(name).unwrap_or_else(|| panic!("no register {name}")))
}

/// What `xcal.dbg` printed, without the `[dbg] pc=... a0 = ` prefixes.
fn dbg_values(out: &str) -> Vec<&str> {
    out.lines().map(|l| l.split(" = ").nth(1).unwrap_or(l)).collect()
}

/// The `[dbg]` values early_s_trap prints: scause, sepc, stval, as xcal.dbg
/// shows a register, in 16 hex digits and as a signed number.
fn trap_report(scause: u64, sepc: u64, stval: u64) -> Vec<String> {
    [scause, sepc, stval]
        .iter()
        .map(|v| format!("{v:#018x} ({})", *v as i64))
        .collect()
}

/// Encodes `addi rd, rs1, imm`.
fn addi(rd: u32, rs1: u32, imm: i32) -> u32 {
    ((imm as u32) << 20) | (rs1 << 15) | (rd << 7) | 0x13
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

// ---- The build and the image ----

#[test]
fn committed_kernel_is_what_our_toolchain_builds() {
    // The whole kernel, its C included, is built by merlin, merlin-as and
    // merlin-ld (no LLVM), so this always runs: kernel/kernel.elf must be
    // byte for byte what the current sources and tools give.
    let built = kernel::build_merlin(&kernel_dir(), None).unwrap();
    assert!(
        built.elf == committed_elf(),
        "kernel/kernel.elf is out of date: run `cargo xtask kernel` and commit it"
    );
    // And kernel/kernel-disk.elf is the same objects linked without the user
    // programs: the kernel a bootable root image holds.
    let disk = kernel::disk_kernel(&kernel_dir(), &built).unwrap();
    let path = kernel_dir().join(kernel::DISK_KERNEL);
    assert!(
        disk == std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display())),
        "kernel/kernel-disk.elf is out of date: run `cargo xtask kernel` and commit it"
    );
    // `user_program_count` as each kernel has it, loaded on the tests' machine.
    let programs = |elf: &[u8]| {
        let mut m = Machine::new(config()).unwrap();
        m.load_elf(elf).unwrap();
        let at = grail::elf::parse(elf)
            .unwrap()
            .symbol("user_program_count")
            .unwrap();
        image_u32(&m, at)
    };
    let names = program_names();
    assert_eq!(programs(&built.elf) as usize, names.len());
    assert_eq!(programs(&disk), 0);
    // Under a third of the loaded bytes: the programs were most of them. (The
    // files also hold the kernel's debug information, which is not loaded.)
    let loaded = |file: &[u8]| -> u64 {
        let parsed = elf::Elf::parse(file).unwrap();
        parsed
            .segments
            .iter()
            .filter(|s| s.ty == elf::PT_LOAD)
            .map(|s| s.filesz)
            .sum()
    };
    let (disk_loaded, full_loaded) = (loaded(&disk), loaded(&built.elf));
    assert!(disk_loaded * 3 < full_loaded, "{disk_loaded} of {full_loaded}");
    let c = built.objects.iter().filter(|o| o.source.lang == Lang::C).count();
    assert!(c >= 20, "{c} C files");
    // merlin's assembly of each C file, and the preprocessed text of each
    // .S file, are kept: what merlin-as assembled.
    assert!(built
        .objects
        .iter()
        .all(|o| o.asm.is_some() == (o.source.lang == Lang::C || o.source.is_preprocessed())));
}

#[test]
fn userprogs_s_is_generated_by_merlin() {
    // The kernel's userprogs.s is generated from kernel/user by merlin,
    // merlin-as and merlin-ld (no LLVM) whenever the kernel is built, and is
    // not a file in kernel/: a stale committed copy would be an error
    // (`kernel::sources`). Its contents are checked through kernel.elf
    // (committed_kernel_is_what_our_toolchain_builds).
    assert!(!kernel_dir().join(kernel::USERPROGS).exists());
    let fresh = kernel::userprogs(&kernel_dir()).unwrap();
    // Most programs are C, compiled by merlin.
    let dir = xtask::userprogs::user_dir(&repo_root());
    let c = xtask::userprogs::sources(&dir)
        .unwrap()
        .into_iter()
        .filter(|s| s.lang == xtask::userprogs::Lang::C)
        .count();
    assert!(c >= 8, "{c} C files");
    assert!(fresh.contains("compiled by merlin,"));
    for name in program_names() {
        assert!(fresh.contains(&format!("\nuser_elf_{name}:\n")), "{name}");
    }
}

/// The embedded programs, by name: all but the disk-only ones.
fn program_names() -> Vec<String> {
    xtask::userprogs::embedded_names(&xtask::userprogs::user_dir(&repo_root())).unwrap()
}

#[test]
fn the_kernel_is_linked_high_and_loaded_low() {
    let file = under_test();
    let e = elf::Elf::parse(&file).unwrap();
    // The firmware starts the image at its load address; _start is its first
    // byte, linked at the top 2 GiB.
    assert_eq!(e.header.entry, KERNEL_LINK_ADDR);
    let parsed = grail::elf::parse(&file).unwrap();
    assert_eq!(parsed.physical_entry().unwrap(), LOAD_ADDR);
    assert_eq!(sym(&parsed, "_start"), KERNEL_LINK_ADDR);
    // Every segment runs high and loads at the same distance below.
    assert!(!parsed.segments.is_empty());
    for s in &parsed.segments {
        assert_eq!(
            s.vaddr - s.paddr,
            KERNEL_LINK_ADDR - LOAD_ADDR,
            "{:#x} {:#x}",
            s.vaddr,
            s.paddr
        );
    }
    for s in [
        "kmain",
        "supervisor_entry",
        "secondary_entry",
        "early_s_trap",
        "machine_power_off",
        "sbi_ecall",
        "kernel_vec",
        "kernel_trap",
        "illegal_instruction",
        "kprintf",
        "ksnprintf",
        "kvformat",
        "uart_putc",
        "xcal_dbg",
        "setup_vm",
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
        assert!(sym(&parsed, s) > KERNEL_LINK_ADDR, "{s}");
    }
    // Text, rodata and the writable rest start on page boundaries, so the
    // kernel page table can give each its own permissions.
    let (text, rodata, data) = (
        sym(&parsed, "__text_start"),
        sym(&parsed, "__rodata_start"),
        sym(&parsed, "__data_start"),
    );
    assert_eq!(text, KERNEL_LINK_ADDR);
    assert!(text < rodata && rodata < data && data <= sym(&parsed, "__bss_start"));
    assert!(rodata.is_multiple_of(PAGE) && data.is_multiple_of(PAGE));
    for s in ["kmain", "kernel_vec", "probe_store_insn"] {
        assert!((text..rodata).contains(&sym(&parsed, s)), "{s} not in text");
    }
    assert!((data..sym(&parsed, "__bss_start")).contains(&sym(&parsed, "boot_fault")));
    // Direct-mode trap vectors: the low two bits of stvec are the mode.
    for s in ["early_s_trap", "kernel_vec", "user_vec"] {
        assert!(sym(&parsed, s).is_multiple_of(4), "{s}");
    }
    let (bss, bss_end) = (sym(&parsed, "__bss_start"), sym(&parsed, "__bss_end"));
    let (stack_bottom, stack_top) = (sym(&parsed, "__stack_bottom"), sym(&parsed, "__stack_top"));
    assert!(bss <= bss_end && bss.is_multiple_of(8) && bss_end.is_multiple_of(8));
    // The boot stack is 16 KiB, aligned to twice that (kernel_vec's check),
    // past a page of its own after .bss (its guard page).
    assert_eq!(stack_top - stack_bottom, KSTACK_SIZE);
    assert_eq!(stack_bottom % KSTACK_SLOT, 0);
    assert!(stack_bottom - PAGE >= bss_end);
    let end = sym(&parsed, "__kernel_end");
    assert!(end >= stack_top && end.is_multiple_of(PAGE));
    // boot_fault ships as none, in .data, so the image carries it and _start
    // doesn't clear it.
    let (m, _) = load_committed();
    assert_eq!(image_u32(&m, sym(&parsed, "boot_fault")), BOOT_FAULT_NONE);
}

#[test]
fn the_kernels_have_no_m_mode_code() {
    // The M-mode layer is the firmware's (#174): no instruction of either
    // kernel's text is M-mode's, no mret, no access to a CSR that only
    // M-mode has (bits 9:8 of its number are its lowest privilege, 3), and
    // none of the SBI's symbols is in it.
    for file in [under_test(), disk_kernel_under_test()] {
        let e = grail::elf::parse(&file).unwrap();
        let (text, end) = (sym(&e, "__text_start"), sym(&e, "__rodata_start"));
        let seg = e
            .segments
            .iter()
            .find(|s| (s.vaddr..s.vaddr + s.mem_size).contains(&text))
            .unwrap();
        // The text, up to the rodata pages (its last page's padding may not be
        // in the file).
        let stop = ((end - seg.vaddr) as usize).min(seg.data.len());
        let code = &seg.data[(text - seg.vaddr) as usize..stop];
        let mut csrs = 0;
        for (i, w) in code.chunks(4).enumerate() {
            let word = u32::from_le_bytes(w.try_into().unwrap());
            let at = text + 4 * i as u64;
            match excalibur::decode(word, Xlen::Rv64) {
                excalibur::Inst::Mret => panic!("mret at {at:#x}"),
                excalibur::Inst::Csr { csr, .. } => {
                    assert_ne!(csr >> 8 & 3, 3, "an M-mode CSR ({csr:#x}) at {at:#x}");
                    csrs += 1;
                }
                _ => {}
            }
        }
        assert!(csrs > 20, "the S-mode CSRs it uses: {csrs}");
        for name in [
            "machine_vec",
            "machine_scratch",
            "sbi_coldboot",
            "sbi_hart_init",
            "sbi_hart_wait",
            "early_trap",
        ] {
            assert!(e.symbol(name).is_none(), "{name} is in the kernel");
        }
        // It loads after the firmware's 2 MiB, where the firmware starts it.
        assert_eq!(e.physical_entry().unwrap(), LOAD_ADDR);
        assert_eq!(LOAD_ADDR, grail::firmware::FW_JUMP_ADDR);
    }
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

/// `text` with its comments and the contents of its string and character
/// literals blanked out, lines kept where they were.
fn c_code_only(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                let mut prev = ' ';
                for c in chars.by_ref() {
                    if c == '\n' {
                        out.push('\n');
                    }
                    if prev == '*' && c == '/' {
                        break;
                    }
                    prev = c;
                }
                out.push(' ');
            }
            '/' if chars.peek() == Some(&'/') => {
                for c in chars.by_ref() {
                    if c == '\n' {
                        out.push('\n');
                        break;
                    }
                }
            }
            '"' | '\'' => {
                out.push(c);
                while let Some(d) = chars.next() {
                    if d == '\\' {
                        chars.next();
                    } else if d == c || d == '\n' {
                        out.push(d);
                        break;
                    }
                }
            }
            c => out.push(c),
        }
    }
    out
}

#[test]
fn kernel_c_stays_freestanding() {
    // No libc headers (everything is #include "..."), no floating point and
    // no inline assembly, so Merlin's C subset compiles it: the kernel and
    // the user programs alike.
    for (name, text) in kernel_c_sources() {
        for (i, code) in c_code_only(&text).lines().enumerate() {
            assert!(!code.contains("#include <"), "{name}:{}: system header", i + 1);
            for word in code.split(|c: char| !c.is_ascii_alphanumeric() && c != '_') {
                assert!(
                    word != "float" && word != "double",
                    "{name}:{}: floating point",
                    i + 1
                );
                assert!(
                    !matches!(word, "asm" | "__asm" | "__asm__"),
                    "{name}:{}: inline assembly",
                    i + 1
                );
            }
        }
    }
    // What the scan sees.
    let code = c_code_only("int a; /* asm\n float */ char *s = \"double \\\" asm\"; // __asm__\nasm(\"x\");");
    assert_eq!(code, "int a; \n  char *s = \"\"; \nasm(\"\");");
}

#[test]
fn kernel_asm_is_merlin_dialect() {
    // The assembly files must use only what merlin-as understands, once
    // merlin has preprocessed them (userprogs.s is generated, and assembled
    // by merlin-as whenever the kernel is built).
    let sources = kernel::sources(&kernel_dir()).unwrap();
    let asm: Vec<_> = sources
        .iter()
        .filter(|s| s.lang == Lang::Asm && !s.is_generated())
        .collect();
    assert!(!asm.is_empty());
    for s in &asm {
        let text = kernel::assembly_of(&kernel_dir(), s).unwrap();
        if let Err(errs) = merlin_as::assemble(&text) {
            let errs: Vec<String> = errs.iter().map(|e| format!("{}: {e}", s.name)).collect();
            panic!("merlin-as rejects kernel assembly:\n{}", errs.join("\n"));
        }
    }
}

#[test]
fn the_kernel_embeds_every_user_program() {
    // The table in the image: names, sizes and bytes match a fresh build of
    // every program but the disk-only ones (the assembler: no files without
    // a disk), which are not there at all. An entry is 24 bytes: two
    // pointers and the size, padded (proc.h's struct user_program).
    let (m, elf) = load_committed();
    let dir = xtask::userprogs::user_dir(&repo_root());
    let all = xtask::userprogs::build_all(&dir).unwrap();
    let programs = xtask::userprogs::embedded(&all);
    for name in xtask::userprogs::DISK_ONLY {
        assert!(
            elf.symbol(&format!("user_elf_{name}")).is_none(),
            "{name} is built in"
        );
    }
    let table = sym(&elf, "user_programs");
    assert_eq!(table % 8, 0);
    assert_eq!(
        image_u32(&m, sym(&elf, "user_program_count")),
        programs.len() as u32
    );
    for (i, p) in programs.iter().enumerate() {
        let entry = table + 24 * i as u64;
        let (name, image, size) = (
            image_u64(&m, entry),
            image_u64(&m, entry + 8),
            image_u32(&m, entry + 16),
        );
        assert_eq!(image, sym(&elf, &format!("user_elf_{}", p.name)));
        assert_eq!(size as usize, p.elf.len());
        let mut s = Vec::new();
        for a in name.. {
            match m.cpu.bus.read_mem(image_pa(a), 1).unwrap() {
                0 => break,
                c => s.push(c as u8),
            }
        }
        assert_eq!(String::from_utf8(s).unwrap(), p.name);
        let pa = image_pa(image);
        let at = (pa - RAM_BASE) as usize;
        assert!(m.cpu.bus.ram[at..at + p.elf.len()] == p.elf[..], "{}", p.name);
        // Read-only data: the kernel maps it without W.
        assert!((sym(&elf, "__rodata_start")..sym(&elf, "__data_start")).contains(&image));
    }
}

// ---- Booting ----

#[test]
fn committed_kernel_boots() {
    assert_boots(&under_test());
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
    // What `cargo xtask kernel --run` uses (with stdout and stderr): the UART
    // text and the xcal.dbg line must each reach their own sink.
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
    // The one xcal.dbg line goes to the debug sink; everything around it to
    // the UART.
    let all = halted_output();
    let (before, rest) = all.split_at(all.find("[dbg]").unwrap());
    let (dbg, after) = rest.split_at(rest.find('\n').unwrap() + 1);
    assert_eq!(mask(&uart.text()), format!("{before}{after}"));
    assert_eq!(kernel::mask_dbg_pc(&debug.text()), dbg);
}

#[test]
fn entry_clears_bss() {
    // Fill .bss with garbage after loading: the self-test counters live
    // there, so the output is only right if _start zeroes it.
    let (mut m, elf) = load_committed();
    let (start, end) = (sym(&elf, "__bss_start"), sym(&elf, "__bss_end"));
    assert!(end > start, "kernel has no .bss to clear");
    poke_image(&mut m, start, &vec![0xa5; (end - start) as usize]);
    let r = m.run(Some(BOOT_MAX_STEPS), |_| {});
    let out = printed(&m);
    assert_eq!(mask(&out), halted_output());
    assert_eq!(r, RunResult::Exited(0));
}

#[test]
fn the_kernel_boots_through_its_self_tests() {
    let mut m = load_committed().0;
    let elf = grail::elf::parse(&under_test()).unwrap();
    let tree = device_tree(&m);
    let r = m.run(Some(BOOT_MAX_STEPS), |_| {});
    let out = printed(&m);
    assert_eq!(r, RunResult::Exited(0), "output:\n{out}");
    // Kernel stacks freed past each hart's cache are unmapped and their
    // pages freed (vm.h, kstack_free()); then the shell, a 64-bit program,
    // as the first process: it reads `exit` and the machine stops with its
    // status.
    assert_eq!(mask(&out), halted_output(), "output:\n{out}");

    // The layout's variables: Sv48 on a hart that has it, else Sv39, with
    // Linux's values for the mode.
    let mode = mode();
    assert_eq!(image_u32(&m, sym(&elf, "pt_levels")), mode.levels);
    assert_eq!(image_u64(&m, sym(&elf, "satp_mode")), mode.satp_mode);
    assert_eq!(image_u64(&m, sym(&elf, "page_offset")), mode.page_offset);
    assert_eq!(image_u64(&m, sym(&elf, "vmalloc_start")), mode.vmalloc_start);
    assert_eq!(
        image_u64(&m, sym(&elf, "va_pa_offset")),
        mode.page_offset - RAM_BASE
    );
    assert_eq!(
        image_u64(&m, sym(&elf, "va_kernel_pa_offset")),
        KERNEL_LINK_ADDR - LOAD_ADDR
    );
    // The kernel's page table: a direct-map pointer, in satp's form in
    // kernel_satp, and hart 0's struct cpu has it.
    let kpt = image_u64(&m, sym(&elf, "kernel_pagetable"));
    assert!(
        kpt >= mode.page_offset && kpt < mode.page_offset + RAM as u64,
        "{kpt:#x}"
    );
    let kpt_pa = kpt - mode.page_offset + RAM_BASE;
    assert_eq!(
        image_u64(&m, sym(&elf, "kernel_satp")),
        mode.satp_mode | kpt_pa >> 12
    );
    let cpus = sym(&elf, "cpus");
    assert_eq!(image_u64(&m, cpus + CPU_PAGETABLE), kpt);
    // The firmware's a1, the device tree Grail made, kept for the kernel
    // (Linux's dtb_early_pa).
    let fdt = m.cpu.bus.read_mem(grail::boot::FDT_ADDR_ADDR, 8).unwrap();
    assert_eq!(fdt, 0x8060_0000);
    assert_eq!(image_u64(&m, sym(&elf, "dtb_early_pa")), fdt);
    // Its pages kept out of the allocator, as Linux memblock_reserve()s the
    // tree: after a boot whose self-tests take every free page, it is
    // still there byte for byte.
    assert!(
        ram(&m, fdt, tree.len()) == tree,
        "the device tree was overwritten"
    );
    assert_eq!(image_u64(&m, sym(&elf, "dtb_base")), fdt);
    assert_eq!(
        image_u64(&m, sym(&elf, "dtb_size")),
        (tree.len() as u64).div_ceil(PAGE) * PAGE
    );
    // Every lock let go, hart 0 out of push_off.
    assert_eq!(image_u32(&m, cpus + CPU_NOFF), 0);
    for name in ["kmem_lock", "proc_lock", "uart_tx_lock", "tickslock"] {
        let lock = sym(&elf, name);
        assert_eq!(image_u32(&m, lock), 0, "{name} held");
        assert_eq!(image_u64(&m, lock + LOCK_CPU), 0, "{name}'s cpu");
    }
}

/// The `len` bytes of RAM at physical address `at`.
fn ram(m: &Machine, at: u64, len: usize) -> Vec<u8> {
    (0..len as u64)
        .map(|i| m.cpu.bus.read_mem(at + i, 1).unwrap() as u8)
        .collect()
}

/// The device tree the firmware will hand the kernel in a1: its header's
/// totalsize of bytes.
fn device_tree(m: &Machine) -> Vec<u8> {
    let at = m.cpu.bus.read_mem(grail::boot::FDT_ADDR_ADDR, 8).unwrap();
    let header = ram(m, at, 8);
    let size = u32::from_be_bytes(header[4..8].try_into().unwrap());
    ram(m, at, size as usize)
}

#[test]
fn an_early_trap_reports_and_halts() {
    // A trap before kmain, while stvec still points at early_s_trap (paging
    // off, or going on): setup_vm()'s first instruction made illegal. The
    // handler prints scause, sepc and stval and stops the machine with exit
    // code 0xff through the SBI. The word is not 0, so the report shows
    // that stval is really read (it holds the instruction's bits). setup_vm
    // runs at its physical address, before paging is on.
    let illegal = 0xffff_ffff;
    assert_eq!(
        excalibur::decode(illegal, Xlen::Rv64),
        excalibur::Inst::Illegal(illegal)
    );
    let (mut m, elf) = load_committed();
    let setup_vm = sym(&elf, "setup_vm");
    poke_image(&mut m, setup_vm, &illegal.to_le_bytes());
    let r = m.run(Some(BOOT_MAX_STEPS), |_| {});
    let out = printed(&m);
    let report = &out[out.find("[dbg]").unwrap_or_else(|| panic!("no report:\n{out}"))..];
    assert_eq!(
        dbg_values(report),
        trap_report(ILLEGAL_INSTRUCTION, image_pa(setup_vm), u64::from(illegal)),
        "output:\n{out}"
    );
    assert_eq!(r, RunResult::Exited(UNEXPECTED_TRAP_EXIT));
}

#[test]
fn halt_uses_kmain_return_value() {
    // Make kmain return 7 straight away (li a0, 7; ret).
    let (mut m, elf) = load_committed();
    let kmain = sym(&elf, "kmain");
    let li_a0_7: u32 = addi(x("a0"), 0, 7);
    let ret: u32 = 0x0000_8067;
    let decoded = (
        excalibur::decode(li_a0_7, Xlen::Rv64),
        excalibur::decode(ret, Xlen::Rv64),
    );
    let ra = x("ra") as u8;
    assert_eq!(
        decoded,
        (
            excalibur::Inst::OpImm {
                op: excalibur::AluOp::Add,
                rd: 10,
                rs1: 0,
                imm: 7
            },
            excalibur::Inst::Jalr {
                rd: 0,
                rs1: ra,
                imm: 0
            }
        )
    );
    let mut code = li_a0_7.to_le_bytes().to_vec();
    code.extend_from_slice(&ret.to_le_bytes());
    poke_image(&mut m, kmain, &code);
    // kmain runs in S-mode, so the code reaches xcal.halt through the SBI's
    // system_reset (machine_power_off()).
    assert_eq!(m.run(Some(BOOT_MAX_STEPS), |_| {}), RunResult::Exited(7));
    assert!(m.cpu.bus.uart.captured.is_empty());
}

#[test]
fn unknown_sbi_extension_returns_not_supported() {
    // kmain: li a7, 99; li a1, 40; li a2, 50; ecall; add a0, a0, a1;
    // add a0, a0, a2; ret. The SBI returns SBI_ERR_NOT_SUPPORTED (-2) in a0
    // and 0 in a1 and keeps a2, so kmain returns 48. (tests/sbi.rs drives
    // every call.)
    let (mut m, elf) = load_committed();
    let (a0, a1, a2, a7) = (x("a0"), x("a1"), x("a2"), x("a7"));
    let code = [
        addi(a7, 0, 99),
        addi(a1, 0, 40),
        addi(a2, 0, 50),
        0x0000_0073,                                // ecall
        (a1 << 20) | (a0 << 15) | (a0 << 7) | 0x33, // add a0, a0, a1
        (a2 << 20) | (a0 << 15) | (a0 << 7) | 0x33, // add a0, a0, a2
        0x0000_8067,                                // ret
    ];
    let bytes: Vec<u8> = code.iter().flat_map(|w| w.to_le_bytes()).collect();
    poke_image(&mut m, sym(&elf, "kmain"), &bytes);
    assert_eq!(m.run(Some(BOOT_MAX_STEPS), |_| {}), RunResult::Exited(48));
}

/// The firmware's symbols (crates/grail/rom/firmware.elf): where its M-mode
/// trap vector is, which every SBI call and machine timer interrupt enters.
fn firmware() -> grail::elf::Elf {
    grail::elf::parse(grail::firmware::FIRMWARE_ELF).unwrap()
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
    // At its link address, with paging on through the early page table.
    assert_eq!(m.cpu.mode, Mode::S);
    let c = &m.cpu.csr;
    assert_eq!(c.satp >> 60, mode().satp_mode >> 60);
    // What the firmware delegates: every exception but ecall from S (the
    // SBI calls) and from M.
    assert_eq!(c.medeleg, 0xb1ff);
    // All S-level interrupts; the machine timer stays in M-mode.
    assert_eq!(
        c.mideleg,
        (1 << IRQ_S_SOFT) | (1 << IRQ_S_TIMER) | (1 << IRQ_S_EXT)
    );
    // Only the IPI (on one hart too): the SBI's set_timer enables the
    // machine timer, and trap_init the S-level interrupts.
    assert_eq!(c.mie, 1 << IRQ_M_SOFT);
    assert_eq!(c.mtvec, sym(&firmware(), "machine_vec"));
    assert_eq!(c.stvec, sym(&elf, "kernel_vec"));
    // S-mode interrupts stay off until trap_init.
    assert_eq!(c.mstatus & MSTATUS_SIE, 0);
    // No tick armed yet: trap_init arms the first.
    assert_eq!(m.cpu.bus.clint.mtimecmp[0], u64::MAX);
}

#[test]
fn the_kernel_runs_on_up_to_eight_harts() {
    for harts in [2, 3, 8] {
        let (mut m, elf) = load_committed_on(MachineConfig { harts, ..config() }, Some(kernel::typed(EXIT)));
        let r = m.run(Some(BOOT_MAX_STEPS), |_| {});
        let out = printed(&m);
        assert_eq!(r, RunResult::Exited(0), "{harts} harts:\n{out}");
        assert!(out.contains(&format!("\nsmp: {harts} harts running\n")), "{out}");
        assert!(!out.contains("TLB"), "{out}");
        // Every hart has its struct cpu, and runs on the kernel page table.
        let cpus = sym(&elf, "cpus");
        let kpt = image_u64(&m, sym(&elf, "kernel_pagetable"));
        for h in 0..harts as u64 {
            let cpu = cpus + h * CPU_SIZE;
            assert_eq!(image_u32(&m, cpu + CPU_HARTID), h as u32);
            assert_eq!(image_u64(&m, cpu + CPU_PAGETABLE), kpt, "hart {h}");
        }
        // The other harts run on the kernel stack smp_boot() gave each with
        // hart_start: in the vmalloc area, in the lower half of a
        // KSTACK_SLOT aligned slot, as every kernel stack is (vm.h).
        for h in 1..harts as u64 {
            let sp = m.cpu.hart(h as usize).x[2];
            let slot = sp & !(KSTACK_SLOT - 1);
            assert!(sp > slot && sp <= slot + KSTACK_SIZE, "{sp:#x}");
            let area = mode().vmalloc_start..=mode().vmalloc_start + KSTACK_SLOTS * KSTACK_SLOT;
            assert!(area.contains(&sp), "{sp:#x}");
            // The scheduler isn't running a thread of a process at the end.
            assert_eq!(image_u64(&m, cpus + h * CPU_SIZE + CPU_THREAD), 0);
        }
    }
}

#[test]
fn the_kernel_configures_its_network_with_dhcp() {
    // Version 2 of the network card (64-bit ring bases, 16-byte descriptors
    // with their buffers' physical addresses) and Grail's user-mode gateway.
    let (mut m, _) = load_committed_on(
        MachineConfig {
            nets: vec![grail::NetConfig::user(Vec::new())],
            ..config()
        },
        Some(kernel::typed(EXIT)),
    );
    let r = m.run(Some(BOOT_MAX_STEPS), |_| {});
    let out = printed(&m);
    assert_eq!(r, RunResult::Exited(0), "{out}");
    assert!(out.contains("\nnet 0: mac 52:54:00:12:34:56, irq "), "{out}");
    assert!(
        out.contains("\nnet: 10.0.2.15/24, gateway 10.0.2.2, dns 10.0.2.3 (dhcp)\n"),
        "{out}"
    );
}

#[test]
fn kernel_stacks_keep_a_margin() {
    let (mut m, _) = load_committed_on(MachineConfig { harts: 2, ..config() }, Some(kernel::typed(EXIT)));
    assert_eq!(m.run(Some(BOOT_MAX_STEPS), |_| {}), RunResult::Exited(0));
    let out = printed(&m);
    let line = out.lines().find(|l| l.starts_with("kstack: ")).unwrap();
    let used: u64 = line["kstack: ".len()..]
        .split(' ')
        .next()
        .unwrap()
        .parse()
        .unwrap();
    assert!(
        line.ends_with(&format!(" of {KSTACK_SIZE} bytes used at most")),
        "{line}"
    );
    // Measured, and with three quarters of the stack to spare.
    assert!(used > 0 && used < KSTACK_SIZE / 4, "{line}");
}

// ---- Library functions, called one at a time ----

/// Scratch memory for `call_kernel`: buffers, then the stack's top, then the
/// return address, all in the last MiB of the machine's 8 MiB of RAM, above
/// the kernel image and the device tree.
const CALL_DATA: u64 = RAM_BASE + 0x70_0000;
const CALL_STACK: u64 = RAM_BASE + 0x7f_0000;
const CALL_RETURN: u64 = RAM_BASE + 0x7f_f000;

/// A machine with the kernel loaded and nothing run: M-mode, no paging, so
/// `CALL_DATA` is at its physical address and the kernel's functions run
/// at theirs (merlin's code reaches its data pc-relatively).
fn call_machine() -> (Machine, grail::elf::Elf) {
    let (m, elf) = load_committed_on(config(), None);
    assert!(
        image_pa(sym(&elf, "__kernel_end")) < CALL_DATA,
        "the kernel reaches the call scratch area"
    );
    (m, elf)
}

/// Calls the kernel's function `name` with `args` in a0, a1, ... (the
/// calling convention's registers, variadic arguments included) and returns
/// its a0. It returns to `CALL_RETURN`, where the run stops.
fn call_kernel(m: &mut Machine, elf: &grail::elf::Elf, name: &str, args: &[u64]) -> u64 {
    for (i, &a) in args.iter().enumerate() {
        m.cpu.set_reg(x("a0") as u8 + i as u8, a);
    }
    m.cpu.set_reg(x("sp") as u8, CALL_STACK);
    m.cpu.set_reg(x("ra") as u8, CALL_RETURN);
    m.cpu.pc = image_pa(sym(elf, name));
    for _ in 0..1_000_000 {
        if m.cpu.pc == CALL_RETURN {
            return m.cpu.x[x("a0") as usize];
        }
        m.cpu.step();
    }
    panic!("{name} never returned (pc {:#x})", m.cpu.pc);
}

fn put_bytes(m: &mut Machine, addr: u64, bytes: &[u8]) {
    m.cpu.bus.load(addr, bytes).unwrap();
}

fn get_bytes(m: &Machine, addr: u64, len: u64) -> Vec<u8> {
    ram(m, addr, len as usize)
}

#[test]
fn kernel_string_functions() {
    let (mut m, elf) = call_machine();
    let (a, b) = (CALL_DATA, CALL_DATA + 0x100);
    let pattern: Vec<u8> = (1..=16).collect();

    // memset: the byte of c, n times, and nothing past them; returns dst.
    put_bytes(&mut m, a, &[0xee; 8]);
    assert_eq!(call_kernel(&mut m, &elf, "memset", &[a + 1, 0x1ab, 5]), a + 1);
    assert_eq!(
        get_bytes(&m, a, 8),
        [0xee, 0xab, 0xab, 0xab, 0xab, 0xab, 0xee, 0xee]
    );
    // A whole page from an odd start: the bytes to a doubleword boundary,
    // then whole doublewords, then the bytes left.
    put_bytes(&mut m, a, &[0x11; 4100]);
    call_kernel(&mut m, &elf, "memset", &[a + 3, 0, 4096]);
    let page = get_bytes(&m, a, 4100);
    assert_eq!(&page[..3], &[0x11; 3]);
    assert!(page[3..4099].iter().all(|&b| b == 0));
    assert_eq!(page[4099], 0x11);

    // memcpy.
    put_bytes(&mut m, a, &pattern);
    put_bytes(&mut m, b, &[0; 16]);
    assert_eq!(call_kernel(&mut m, &elf, "memcpy", &[b, a, 15]), b);
    assert_eq!(get_bytes(&m, b, 16), [&pattern[..15], &[0]].concat());

    // memmove with overlap, both ways: the bytes as they were before.
    put_bytes(&mut m, a, &pattern);
    assert_eq!(call_kernel(&mut m, &elf, "memmove", &[a + 3, a, 10]), a + 3);
    assert_eq!(
        get_bytes(&m, a, 16),
        [&pattern[..3], &pattern[..10], &pattern[13..]].concat()
    );
    put_bytes(&mut m, a, &pattern);
    assert_eq!(call_kernel(&mut m, &elf, "memmove", &[a, a + 3, 10]), a);
    assert_eq!(get_bytes(&m, a, 16), [&pattern[3..13], &pattern[10..]].concat());

    // memcmp: the sign of the first difference, as unsigned bytes, an int
    // sign-extended in a0.
    put_bytes(&mut m, a, b"abcd\x80");
    put_bytes(&mut m, b, b"abce\x01");
    let cmp = |m: &mut Machine, x, y, n| call_kernel(m, &elf, "memcmp", &[x, y, n]) as i64;
    assert_eq!(cmp(&mut m, a, b, 3), 0);
    assert_eq!(cmp(&mut m, a, b, 4), -1);
    assert_eq!(cmp(&mut m, b, a, 4), 1);
    assert_eq!(cmp(&mut m, a + 4, b + 4, 1), 1, "0x80 > 0x01");
    assert_eq!(cmp(&mut m, a, b, 0), 0);
}

#[test]
fn ksnprintf_corner_cases() {
    // What the boot-time kprintf self-test leaves out.
    let (mut m, elf) = call_machine();
    let (buf, fmt) = (CALL_DATA, CALL_DATA + 0x100);
    // Formats `format` with `args` into a 32-byte buffer of `size`; returns
    // ksnprintf's value and the buffer up to its NUL.
    let mut format = |format: &str, size: u64, args: &[u64]| {
        put_bytes(&mut m, fmt, &[format.as_bytes(), &[0]].concat());
        put_bytes(&mut m, buf - 1, &[0x5a; 34]);
        let n = call_kernel(&mut m, &elf, "ksnprintf", &[&[buf, size, fmt][..], args].concat());
        let out = get_bytes(&m, buf, 32);
        let text = out.split(|&b| b == 0).next().unwrap();
        (
            n as i32,
            String::from_utf8_lossy(text).into_owned(),
            get_bytes(&m, buf - 1, 34),
        )
    };
    // A width for %c pads on the left.
    assert_eq!(format("[%3c]", 32, &[u64::from(b'x')]).1, "[  x]");
    // A long is a register's 64 bits; %p prints all 16 digits.
    assert_eq!(format("%lx", 32, &[0x1234_5678_9abc_def0]).1, "123456789abcdef0");
    assert_eq!(format("%p", 32, &[0xffff_ffff_8000_0000]).1, "0xffffffff80000000");
    // An incomplete conversion at the end is copied as written.
    for f in ["end %", "end %5", "end %05", "end %l", "end %12l"] {
        let (n, text, _) = format(f, 32, &[]);
        assert_eq!((n as usize, text.as_str()), (f.len(), f));
    }
    // Size 0: nothing is written, not even the NUL, and the full length
    // comes back.
    let (n, _, bytes) = format("abc%d", 0, &[42]);
    assert_eq!(n, 5);
    assert!(bytes.iter().all(|&b| b == 0x5a), "{bytes:x?}");
    // Size 1: only the NUL.
    let (n, text, bytes) = format("abc", 1, &[]);
    assert_eq!((n, text.as_str(), bytes[0], bytes[2]), (3, "", 0x5a, 0x5a));
}

#[test]
fn timer_interval_is_never_zero() {
    // A timebase below the tick rate would give an interval of 0, which
    // would set every deadline to now; the kernel uses 1 instead. (A machine
    // that slow can't run the kernel anyway: the timer fires faster than
    // the kernel handles it. So timer_interval() is called on its own.)
    for (hz, interval) in [(5, 1), (10_000_000, TICK_INTERVAL)] {
        let (mut m, elf) = load_committed_on(
            MachineConfig {
                nominal_hz: hz,
                ..config()
            },
            None,
        );
        assert_eq!(
            call_kernel(&mut m, &elf, "timer_interval", &[]),
            interval,
            "{hz} Hz"
        );
    }
}

// ---- Ticks, and interrupts in the middle of kernel code ----

#[test]
fn ticks_are_one_interval_apart() {
    let (mut m, elf) = load_committed();
    let machine_vec = sym(&firmware(), "machine_vec");
    let kernel_vec = sym(&elf, "kernel_vec");
    let trap_init = sym(&elf, "trap_init");
    // The boot's ticks are the ten it waits for: after them come the
    // kernel stacks' self-test and starting the shell, which may take more
    // than an interval (merlin's code is slow), and those later ticks land
    // wherever the kernel is.
    let ticks = sym(&elf, "ticks");
    let mut boot_done = false;
    let mut ticks_at_boot = None;
    // Machine timer interrupts before trap_init arms the first tick, and
    // after.
    let mut early_ticks = 0;
    let mut machine_ticks = Vec::new();
    let mut deadlines = Vec::new();
    let mut kernel_ticks = Vec::new();
    // The SBI calls before the shell: set_timer, and the others' extensions.
    let mut set_timers = 0;
    let mut other_calls = Vec::new();
    let mut external = 0;
    let mut steps_after_trap_init = None;
    let mut resets = 0;
    let probes = [sym(&elf, "probe_load_insn"), sym(&elf, "probe_store_insn")];
    let mut probe_faults = 0;
    let r = m.run(Some(BOOT_MAX_STEPS), |cpu| {
        if !boot_done && cpu.bus.uart.captured.ends_with(b"tick 10\n") {
            boot_done = true;
            ticks_at_boot = Some(peek(cpu, ticks));
        }
        if cpu.pc == trap_init {
            steps_after_trap_init = Some(0u64);
        }
        if let (Some(steps), false) = (&mut steps_after_trap_init, boot_done) {
            *steps += 1;
        }
        let ecall = cpu.pc == machine_vec && cpu.csr.mcause == ECALL_FROM_S;
        if ecall && cpu.x[x("a7") as usize] == common::sbi::EXT_SRST {
            resets += 1;
        }
        if boot_done || cpu.mode == Mode::M && cpu.pc != machine_vec {
            return;
        }
        if cpu.pc == machine_vec {
            if cpu.csr.mcause == INTERRUPT_BIT | u64::from(IRQ_M_TIMER) {
                if steps_after_trap_init.is_none() {
                    early_ticks += 1;
                } else {
                    machine_ticks.push(cpu.bus.clint.mtime);
                }
                deadlines.push(cpu.bus.clint.mtimecmp[0]);
            } else {
                // The only other trap to M-mode once the kernel runs: SBI
                // calls.
                assert!(ecall, "mcause {:#x}", cpu.csr.mcause);
                match cpu.x[x("a7") as usize] {
                    common::sbi::EXT_TIME => set_timers += 1,
                    eid => other_calls.push(eid),
                }
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
        } else if cpu.pc == kernel_vec && cpu.csr.scause == INTERRUPT_BIT | u64::from(IRQ_S_EXT) {
            // The UART receiving the shell's `exit`.
            external += 1;
        } else if cpu.pc == kernel_vec {
            // A tick: the supervisor timer interrupt, never an IPI on one
            // hart.
            assert_eq!(cpu.csr.scause, INTERRUPT_BIT | u64::from(IRQ_S_TIMER));
            // Ticks arrive with paging on.
            assert_ne!(cpu.csr.satp >> 60, 0, "tick before paging");
            kernel_ticks.push(cpu.bus.clint.mtime);
        }
    });
    assert_eq!(r, RunResult::Exited(0));
    assert_eq!(resets, 1);
    // All of `exit\n` was waiting before interrupts were on: one interrupt
    // drains it.
    assert_eq!(external, 1);
    // Unmapped load and store, stores to text and rodata.
    assert_eq!(probe_faults, 4);
    // No tick before trap_init arms the first; then the kernel waits ten
    // whole intervals, one machine tick each, which the SBI forwards.
    assert_eq!(early_ticks, 0);
    assert_eq!(machine_ticks.len(), 10, "{machine_ticks:?}");
    assert_eq!(kernel_ticks.len(), 10);
    // One set_timer for the first tick, then one in each tick for the next.
    assert_eq!(set_timers, 11);
    // sbi_init's: the version, the implementation and its version, and a
    // probe for each extension it looks for.
    assert_eq!(other_calls, [common::sbi::EXT_BASE; 8]);
    // The deadlines are exactly one interval apart (the kernel adds the
    // interval to the last deadline, so ticks don't drift), and every tick
    // lands in wfi, right on its deadline.
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
    assert_eq!(ticks_at_boot, Some(10));
    // wfi skips to the next tick: ten intervals of mtime pass in far fewer
    // instructions than one.
    assert!(m.cpu.bus.clint.mtime >= 10 * TICK_INTERVAL);
    let steps = steps_after_trap_init.unwrap();
    assert!(steps < TICK_INTERVAL, "{steps} steps");
}

// In a normal boot every tick lands in wfi(), where hardly any register is
// live, so the boot tests can't tell whether the trap handlers give the
// interrupted code its registers back. These tests interrupt code that has
// live values everywhere.

/// `j .`: an endless loop, standing in for busy kernel code.
const J_SELF: u32 = 0x0000_006f;

/// Byte offsets in `struct trap_frame` (kernel/include/trap.h): 8-byte slots.
const FRAME_SLOT: u64 = 8;
const FRAME_SEPC: u64 = 32 * FRAME_SLOT;
const FRAME_SCAUSE: u64 = 33 * FRAME_SLOT;
const FRAME_SSTATUS: u64 = 35 * FRAME_SLOT;

/// Sets the registers of `m`'s current hart to `regs`.
fn set_regs(m: &mut Machine, regs: &[u64; 32]) {
    for (r, &v) in regs.iter().enumerate() {
        m.cpu.set_reg(r as u8, v);
    }
}

/// A distinct value for every register but x0 and sp: `tag` in the top
/// byte, the register number in each of the others, so every one of the 64
/// bits counts.
fn reg_pattern(tag: u64, sp: u64) -> [u64; 32] {
    let mut r: [u64; 32] = std::array::from_fn(|i| (tag << 56) | (i as u64 * 0x0001_0101_0101_0101));
    r[0] = 0;
    r[2] = sp;
    r
}

/// Boots the kernel up to kmain, then turns kmain into `j .; j .` and
/// fills every register but sp and tp with `reg_pattern(0xa5, ..)`: busy
/// code with every register live. S-mode interrupts are on and the timer's
/// enabled, as after trap_init. Returns the machine, the ELF and the
/// registers.
fn busy_kmain() -> (Machine, grail::elf::Elf, [u64; 32]) {
    let (mut m, elf) = load_committed();
    let kmain = sym(&elf, "kmain");
    step_to(&mut m, kmain);
    let code: Vec<u8> = [J_SELF, J_SELF].iter().flat_map(|w| w.to_le_bytes()).collect();
    poke_image(&mut m, kmain, &code);
    let mut regs = reg_pattern(0xa5, m.cpu.x[2]);
    // tp keeps the hart's id: kernel code finds its `struct cpu` through it
    // (spec section 10, "Kernel locking"), and the trap handlers take locks.
    regs[x("tp") as usize] = m.cpu.x[x("tp") as usize];
    set_regs(&mut m, &regs);
    m.cpu.csr.mie |= (1 << IRQ_S_SOFT) | (1 << IRQ_S_TIMER);
    m.cpu.csr.mstatus |= MSTATUS_SIE;
    (m, elf, regs)
}

#[test]
fn tick_preserves_every_register() {
    // A tick goes through both handlers: the firmware's machine_vec (raise
    // STIP, mask the machine timer) and kernel_vec (set the next deadline
    // with an SBI call, which goes through machine_vec again, and count the
    // tick). None may change a register.
    let (mut m, elf, regs) = busy_kmain();
    let kmain = sym(&elf, "kmain");
    // A deadline 20 ticks away, as set_timer would leave it: in mtimecmp,
    // with the machine timer enabled, and the kernel's record of it.
    let clint = &mut m.cpu.bus.clint;
    clint.mtimecmp[0] = clint.mtime + 20;
    let cmp = clint.mtimecmp[0];
    m.cpu.csr.mie |= 1 << IRQ_M_TIMER;
    poke_image(&mut m, sym(&elf, NEXT_TICK), &cmp.to_le_bytes());

    step_to(&mut m, sym(&firmware(), "machine_vec"));
    assert_eq!(m.cpu.csr.mcause, INTERRUPT_BIT | u64::from(IRQ_M_TIMER));
    assert_eq!(m.cpu.csr.mepc, kmain);
    // machine_vec returns to the loop, which takes the forwarded tick at once.
    step_to(&mut m, sym(&elf, "kernel_vec"));
    assert_eq!(m.cpu.csr.scause, INTERRUPT_BIT | u64::from(IRQ_S_TIMER));
    assert_eq!(m.cpu.csr.sepc, kmain);
    assert_eq!(m.cpu.x, regs, "machine_vec changed registers");
    assert_eq!(
        m.cpu.csr.mie & (1 << IRQ_M_TIMER),
        0,
        "the machine timer is masked"
    );

    step_to(&mut m, kmain);
    assert_eq!(m.cpu.mode, Mode::S);
    assert_eq!(m.cpu.x, regs, "kernel_vec changed registers");
    assert_ne!(m.cpu.csr.mstatus & MSTATUS_SIE, 0);
    // The next deadline one interval after this one, the tick no longer
    // pending, the machine timer on again for the next.
    assert_eq!(m.cpu.bus.clint.mtimecmp[0], cmp + TICK_INTERVAL);
    assert_eq!(m.cpu.csr.mip & (1 << IRQ_S_TIMER), 0);
    assert_ne!(m.cpu.csr.mie & (1 << IRQ_M_TIMER), 0);
    assert_eq!(image_u32(&m, sym(&elf, "ticks")), 1);
}

#[test]
fn kernel_vec_resumes_from_the_frame() {
    // kernel_trap may change the trap frame, and kernel_vec must resume from
    // it: every register, sepc and sstatus are loaded back from the frame.
    let (mut m, elf, regs) = busy_kmain();
    let kmain = sym(&elf, "kmain");
    // A tick, as machine_vec raises it: STIP.
    m.cpu.csr.mip |= 1 << IRQ_S_TIMER;
    step_to(&mut m, sym(&elf, "kernel_trap"));
    // On the boot stack, in the image: its physical address is the image's.
    let frame = m.cpu.x[x("a0") as usize];
    let frame_pa = kpa(&m.cpu, frame);
    let get = |m: &Machine, off: u64| read64_pa(m, frame_pa + off);
    for (i, &v) in regs.iter().enumerate() {
        assert_eq!(get(&m, 8 * i as u64), v, "{}", reg::abi_name(i as u8));
    }
    assert_eq!(get(&m, FRAME_SEPC), kmain);
    assert_eq!(get(&m, FRAME_SCAUSE), INTERRUPT_BIT | u64::from(IRQ_S_TIMER));
    let sstatus = get(&m, FRAME_SSTATUS);
    assert_ne!(sstatus & MSTATUS_SPIE, 0);
    assert_ne!(sstatus & MSTATUS_SPP, 0);

    // Other values for every register but sp, which kernel_vec recomputes by
    // popping the frame; resume at kmain + 4 with interrupts off.
    let new = reg_pattern(0x5a, regs[2]);
    let mut put = |off: u64, v: u64| m.cpu.bus.load(frame_pa + off, &v.to_le_bytes()).unwrap();
    for (i, &v) in new.iter().enumerate() {
        put(8 * i as u64, v);
    }
    put(FRAME_SEPC, kmain + 4);
    put(FRAME_SSTATUS, sstatus & !MSTATUS_SPIE);

    step_to(&mut m, kmain + 4);
    assert_eq!(m.cpu.mode, Mode::S);
    assert_eq!(m.cpu.x, new);
    assert_eq!(m.cpu.csr.mstatus & MSTATUS_SIE, 0);
    assert_eq!(image_u32(&m, sym(&elf, "ticks")), 1);
}

#[test]
fn ticks_preempting_kernel_code_change_nothing() {
    // Shorten the tick interval to about fifteen hundred mtime ticks (a
    // prime, so the ticks don't fall into step with a loop; much shorter and
    // merlin's tick handler, the next deadline and its SBI call included,
    // would leave the kernel no time between ticks) once trap_init has armed
    // the first tick and kmain has printed the timebase: the kernel's
    // timebase, which the interval comes from, ten times the interval, and
    // the deadline set to match. Ticks then land all over the disk,
    // filesystem and network setup and the self-tests instead of only in
    // wait_ticks' idle loop. A clobbered register would fail a self-test,
    // garble the output or fault; and every tick still gets its own line.
    const SHORT_INTERVAL: u32 = 1499;
    let (mut m, elf) = load_committed();
    step_until(
        &mut m,
        BOOT_MAX_STEPS,
        |_| {},
        |m| printed(m).ends_with("Hz timebase\n"),
    );
    let timebase = sym(&elf, TIMEBASE);
    let set_timebase = |m: &mut Machine, hz: u32| poke_image(m, timebase, &hz.to_le_bytes());
    set_timebase(&mut m, SHORT_INTERVAL * 10);
    let cmp = m.cpu.bus.clint.mtime + u64::from(SHORT_INTERVAL);
    m.cpu.bus.clint.mtimecmp[0] = cmp;
    poke_image(&mut m, sym(&elf, NEXT_TICK), &cmp.to_le_bytes());

    // Where a tick interrupts wait_ticks' idle loop: after `wfi` in M-mode,
    // and after intr_on's `csrsi` in S-mode.
    let idle = [sym(&elf, "wfi") + 4, sym(&elf, "intr_on") + 4];
    let (machine_vec, kernel_vec) = (sym(&firmware(), "machine_vec"), sym(&elf, "kernel_vec"));
    let (mut machine_busy, mut kernel_busy) = (0, 0);
    // Up to the first process. A tick that short leaves a process no time to
    // run (taking one and switching back costs more than that), so the
    // normal interval is back for the shell.
    step_until(
        &mut m,
        BOOT_MAX_STEPS,
        |cpu| {
            if cpu.pc == machine_vec
                && cpu.csr.mcause == INTERRUPT_BIT | u64::from(IRQ_M_TIMER)
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
    set_timebase(&mut m, (TICK_INTERVAL * 10) as u32);
    let r = m.run(Some(BOOT_MAX_STEPS), |_| {});
    let out = printed(&m);
    assert_eq!(mask(&out), halted_output(), "raw output:\n{out}");
    assert_eq!(r, RunResult::Exited(0));
    // Each machine tick is one kernel tick: the SBI forwards it as STIP,
    // which only the kernel's next set_timer clears.
    assert!(
        machine_busy >= 100 && kernel_busy >= 30,
        "only {machine_busy} machine and {kernel_busy} kernel ticks outside the idle loop"
    );
}

#[test]
fn the_next_deadline_is_one_register() {
    // set_timer takes the next deadline whole, in a0 (RV64's 64-bit
    // registers): a tick just below 2^32 goes to the SBI past it, in one
    // register, and mtimecmp gets all of it.
    const CMP: u64 = 0xffff_ff00;
    let (mut m, elf, _) = busy_kmain();
    let clint = &mut m.cpu.bus.clint;
    clint.mtime = CMP - 0x100;
    clint.mtimecmp[0] = CMP;
    m.cpu.csr.mie |= 1 << IRQ_M_TIMER;
    poke_image(&mut m, sym(&elf, NEXT_TICK), &CMP.to_le_bytes());
    let fw = firmware();
    step_to(&mut m, sym(&fw, "machine_vec"));
    assert_eq!(m.cpu.csr.mcause, INTERRUPT_BIT | u64::from(IRQ_M_TIMER));
    step_to(&mut m, sym(&fw, "sbi_time"));
    assert_eq!(m.cpu.x[x("a0") as usize], CMP + TICK_INTERVAL);
    step_to(&mut m, sym(&elf, "kmain"));
    assert_eq!(m.cpu.bus.clint.mtimecmp[0], CMP + TICK_INTERVAL);
    assert!(m.cpu.bus.clint.mtimecmp[0] > 1 << 32);
}

/// A memory access by the instruction about to run, at its physical
/// address (through the hart's page table).
struct Access {
    addr: u64,
    /// The register a load writes; `None` for a store.
    load_rd: Option<u8>,
    /// The value a store writes.
    value: u64,
}

/// The load or store the instruction about to run makes, if it is one, at
/// the physical address its virtual one translates to.
fn mem_access(cpu: &grail::cpu::Cpu) -> Option<Access> {
    let word = cpu.bus.peek(cpu.debug_translate(&cpu.bus, cpu.pc)?)?;
    let (base, imm, load_rd, value) = match excalibur::decode(word, Xlen::Rv64) {
        excalibur::Inst::Load { rd, rs1, imm, .. } => (rs1, imm, Some(rd), 0),
        excalibur::Inst::Store { rs1, rs2, imm, .. } => (rs1, imm, None, cpu.x[rs2 as usize]),
        _ => return None,
    };
    let va = cpu.x[base as usize].wrapping_add(imm as i64 as u64);
    let addr = cpu.debug_translate(&cpu.bus, va)?;
    Some(Access { addr, load_rd, value })
}

/// The offset of physical address `addr` in the `size` bytes of device
/// registers at `base`, if it is there.
fn device_offset(addr: u64, base: u64, size: u64) -> Option<u64> {
    let off = addr.checked_sub(base)?;
    (off < size).then_some(off)
}

/// A UART register access by the instruction about to run: (offset, store?,
/// stored value).
fn uart_access(cpu: &grail::cpu::Cpu) -> Option<(u64, bool, u64)> {
    let a = mem_access(cpu)?;
    let off = device_offset(a.addr, UART_BASE, 8)?;
    Some((off, a.load_rd.is_none(), a.value))
}

/// Whether the instruction about to run is in `f`, or in a leaf helper `f`
/// called. merlin doesn't inline `static inline` functions such as
/// mmio_read32, so a register access is in the file's own copy of the
/// helper, with `ra` still pointing back into `f`.
fn in_function(cpu: &grail::cpu::Cpu, f: &std::ops::Range<u64>) -> bool {
    f.contains(&cpu.pc) || f.contains(&cpu.x[x("ra") as usize])
}

#[test]
fn kernel_reads_the_timebase_from_sysinfo() {
    // A machine reporting a 2 MHz clock: the 10 Hz tick is 200 000 mtime ticks.
    const HZ: u32 = 2_000_000;
    let (mut m, elf) = load_committed_on(
        MachineConfig {
            nominal_hz: HZ,
            ..config()
        },
        Some(kernel::typed(EXIT)),
    );
    let interval = HZ / 10;
    let kernel_vec = sym(&elf, "kernel_vec");
    let mut kernel_ticks = Vec::new();
    let mut boot_done = false;
    let r = m.run(Some(BOOT_MAX_STEPS), |cpu| {
        boot_done |= cpu.bus.uart.captured.ends_with(b"tick 10\n");
        if !boot_done && cpu.pc == kernel_vec && cpu.csr.scause == INTERRUPT_BIT | u64::from(IRQ_S_TIMER) {
            kernel_ticks.push(cpu.bus.clint.mtime);
        }
    });
    assert_eq!(r, RunResult::Exited(0));
    let want = halted_output().replace("10000000 Hz timebase", "2000000 Hz timebase");
    assert_eq!(mask(&printed(&m)), want);
    // The ten ticks the boot waits for, each a 200 000-tick interval.
    // (Setting up memory comes before them, so the total mtime says little.)
    // The first may come while
    // the kernel is still busy rather than idle in wfi, which it reaches a
    // few instructions sooner: how long the setup after trap_init took
    // decides that, so the first interval is only close.
    assert_eq!(kernel_ticks.len(), 10, "{kernel_ticks:?}");
    let first = kernel_ticks[1] - kernel_ticks[0];
    assert!(first.abs_diff(u64::from(interval)) < 100, "{kernel_ticks:?}");
    for w in kernel_ticks[1..].windows(2) {
        assert_eq!(w[1] - w[0], u64::from(interval), "{kernel_ticks:?}");
    }
}

#[test]
fn timebase_is_read_once() {
    // timebase_hz() reads sysinfo on its first call, from trap_init's
    // timer_start, and keeps the value: kmain's print of the timebase, every
    // tick's next deadline and the other harts' first never read it again.
    // The firmware, in M-mode, never reads it at all.
    let (mut m, _) = load_committed();
    let mut reads = Vec::new();
    let r = m.run(Some(BOOT_MAX_STEPS), |cpu| {
        if let Some(a) = mem_access(cpu) {
            if let Some(off) = device_offset(a.addr, SYSINFO_BASE, SYSINFO_SIZE) {
                assert!(a.load_rd.is_some(), "store to sysinfo at {:#x}", cpu.pc);
                reads.push((off as u32, cpu.mode));
            }
        }
    });
    assert_eq!(r, RunResult::Exited(0));
    let timebase: Vec<_> = reads.iter().filter(|r| r.0 == REG_TIMEBASE_HZ).collect();
    assert_eq!(timebase.len(), 1, "{reads:?}");
    assert_eq!(timebase[0].1, Mode::S);
    // M-mode reads only what the SBI's coldboot needs: the magic, the harts,
    // RAM and its own region.
    assert!(reads.iter().any(|r| r.1 == Mode::M), "{reads:?}");
    assert!(reads
        .iter()
        .filter(|r| r.1 == Mode::M)
        .all(|r| r.0 != REG_TIMEBASE_HZ));
}

/// Boots the kernel on a machine reporting a 2 MHz clock, but with sysinfo
/// register `reg` reading as 0 to the first timebase_hz(), the one that
/// reads sysinfo. Returns the sysinfo registers that call read and the
/// whole output.
fn boot_with_sysinfo_reg_zero(reg: u32) -> (Vec<u32>, String) {
    let (mut m, elf) = load_committed_on(
        MachineConfig {
            nominal_hz: 2_000_000,
            ..config()
        },
        Some(kernel::typed(EXIT)),
    );
    step_to_far(&mut m, sym(&elf, "timebase_hz"));
    let back = m.cpu.x[x("ra") as usize];
    let mut reads = Vec::new();
    while m.cpu.pc != back {
        let read = mem_access(&m.cpu)
            .and_then(|a| Some((device_offset(a.addr, SYSINFO_BASE, SYSINFO_SIZE)?, a.load_rd?)));
        m.cpu.step();
        if let Some((offset, rd)) = read {
            reads.push(offset as u32);
            if offset as u32 == reg {
                m.cpu.x[rd as usize] = 0;
            }
        }
    }
    assert_eq!(m.run(Some(BOOT_MAX_STEPS), |_| {}), RunResult::Exited(0));
    (reads, mask(&printed(&m)))
}

#[test]
fn timebase_falls_back_to_nominal_without_sysinfo() {
    // SYS_MAGIC is wrong: no sysinfo, so SYS_TIMEBASE_HZ is not read and the
    // timebase is the nominal 10 MHz, not the machine's 2 MHz.
    let (reads, out) = boot_with_sysinfo_reg_zero(REG_MAGIC);
    assert_eq!(reads, [REG_MAGIC]);
    assert_eq!(out, halted_output());
}

#[test]
fn timebase_falls_back_to_nominal_when_sysinfo_reports_zero() {
    let (reads, out) = boot_with_sysinfo_reg_zero(REG_TIMEBASE_HZ);
    assert_eq!(reads, [REG_MAGIC, REG_TIMEBASE_HZ]);
    assert_eq!(out, halted_output());
}

// ---- Faults and panics ----

/// Runs the kernel with `boot_fault` patched to `fault`. Returns the
/// machine afterwards, how the run ended and what the kernel printed.
fn run_with_fault(fault: u32) -> (Machine, grail::elf::Elf, RunResult, String) {
    let (mut m, elf) = load_committed();
    poke_image(&mut m, sym(&elf, "boot_fault"), &fault.to_le_bytes());
    let r = m.run(Some(BOOT_MAX_STEPS), |_| {});
    let out = printed(&m);
    (m, elf, r, out)
}

/// A panic report (trap.c): the cause line, then `name 0xvalue` pairs for
/// the trap CSRs, then `name value` pairs for x1-x31, every value 16 hex
/// digits. Returns the cause and the values. A page fault's report also has
/// a line on how the page table maps stval (`  va ...`), which this skips.
fn parse_panic(out: &str) -> (String, HashMap<String, u64>) {
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
            assert_eq!(hex.len(), 16, "{line:?}");
            let v = u64::from_str_radix(hex, 16).unwrap();
            assert!(
                values.insert(pair[0].to_string(), v).is_none(),
                "{} twice",
                pair[0]
            );
        }
    }
    assert!(n >= 1 + 8, "report:\n{report}");
    assert_eq!(values.len(), 4 + 31, "report:\n{report}");
    (cause, values)
}

/// The `name 0xvalue` pairs of 16 hex digits in a report, whatever else its
/// lines hold (a kernel stack overflow's has the stack's range too).
fn panic_values(report: &str) -> HashMap<String, u64> {
    let mut values = HashMap::new();
    for line in report.lines().filter(|l| !l.starts_with("  va ")) {
        let words: Vec<&str> = line.split_whitespace().collect();
        if !words.len().is_multiple_of(2) || words.is_empty() {
            continue;
        }
        for pair in words.chunks(2) {
            let hex = pair[1].strip_prefix("0x").unwrap_or(pair[1]);
            if hex.len() != 16 {
                continue;
            }
            if let Ok(v) = u64::from_str_radix(hex, 16) {
                values.insert(pair[0].to_string(), v);
            }
        }
    }
    values
}

#[test]
fn illegal_instruction_panics_with_a_64_bit_register_dump() {
    let (m, elf, r, out) = run_with_fault(BOOT_FAULT_ILLEGAL);
    assert_eq!(r, RunResult::Exited(PANIC_EXIT), "output:\n{out}");
    // The ticks ran first.
    let before = &out[..out.find("panic: ").unwrap()];
    assert_eq!(kernel::mask_dbg_pc(before), boot_output());
    let (cause, v) = parse_panic(&out);
    assert_eq!(cause, "illegal instruction");
    assert_eq!(v["scause"], 2);
    // stval holds the instruction bits: unimp (csrrw zero, cycle, zero).
    assert_eq!(v["stval"], 0xc000_1073);
    let sepc = v["sepc"];
    assert_eq!(image_u32(&m, sepc), 0xc000_1073);
    let func = sym(&elf, "illegal_instruction");
    // The unimp after the 64-bit li's, which take up to 8 instructions each.
    assert!((func..func + 0x400).contains(&sepc), "sepc {sepc:#x}");
    // From S-mode, with interrupts on.
    assert_ne!(v["sstatus"] & MSTATUS_SPP, 0);
    assert_ne!(v["sstatus"] & MSTATUS_SPIE, 0);
    // Every register but sp holds its number in each of its 8 bytes: the
    // trap frame's 8-byte slots, all 64 bits of each, so this checks the
    // whole frame's layout.
    for i in 1..32u8 {
        if i != 2 {
            let name = excalibur::reg::abi_name(i);
            assert_eq!(v[name], u64::from(i) * 0x0101_0101_0101_0101, "{name}");
        }
    }
    let sp = v["sp"];
    assert!(
        sp > sym(&elf, "__stack_bottom") && sp <= sym(&elf, "__stack_top"),
        "{sp:#x}"
    );
    assert_eq!(sp % 16, 0);
}

#[test]
fn unmapped_load_panics() {
    let (m, elf, r, out) = run_with_fault(BOOT_FAULT_LOAD);
    assert_eq!(r, RunResult::Exited(PANIC_EXIT), "output:\n{out}");
    let before = &out[..out.find("panic: ").unwrap()];
    assert_eq!(kernel::mask_dbg_pc(before), boot_output());
    // With paging on, an address the page table doesn't map is a page fault
    // (the lower half is not mapped at all).
    let (cause, v) = parse_panic(&out);
    assert_eq!(cause, "load page fault");
    assert_eq!((v["scause"], v["stval"]), (LOAD_PAGE_FAULT, UNMAPPED_ADDR));
    assert!(
        out.contains(&format!("\n  va {UNMAPPED_ADDR:#018x} is not mapped\n")),
        "{out}"
    );
    let sepc = v["sepc"];
    assert!(sepc > sym(&elf, "_start") && sepc < sym(&elf, "__rodata_start"));
    let word = image_u32(&m, sepc);
    assert!(
        matches!(excalibur::decode(word, Xlen::Rv64), excalibur::Inst::Load { .. }),
        "sepc {sepc:#x} holds {}",
        excalibur::decode(word, Xlen::Rv64)
    );
    assert_ne!(v["sstatus"] & MSTATUS_SPP, 0);
}

#[test]
fn store_to_kernel_text_panics() {
    // Outside the probes a store page fault is a kernel bug: the panic shows
    // the cause, stval and how the kernel page table maps stval.
    let (mut m, elf) = load_committed();
    let kmain = sym(&elf, "kmain");
    poke_image(
        &mut m,
        sym(&elf, "boot_fault"),
        &BOOT_FAULT_STORE_TEXT.to_le_bytes(),
    );
    let mut faulted = false;
    let r = m.run(Some(BOOT_MAX_STEPS), |cpu| {
        if mem_access(cpu).is_some_and(|a| a.load_rd.is_none() && a.addr == image_pa(kmain)) {
            faulted = true;
        }
    });
    let out = printed(&m);
    assert_eq!(r, RunResult::Exited(PANIC_EXIT), "output:\n{out}");
    assert!(faulted, "no store to kmain");
    let before = &out[..out.find("panic: ").unwrap()];
    assert_eq!(kernel::mask_dbg_pc(before), boot_output());

    let (cause, v) = parse_panic(&out);
    assert_eq!(cause, "store page fault");
    assert_eq!(v["scause"], STORE_PAGE_FAULT);
    assert_eq!(v["stval"], kmain);
    // kmain's page at its link address, read-only, onto its physical page:
    // R+X, not W, not U, and A, D and G, which every kernel leaf has (vm.h's
    // PTE_KERNEL): a hart that does not set A and D itself would fault.
    let pa = image_pa(kmain);
    let pte = (pa >> 12) << 10 | PTE_V | PTE_R | PTE_X | PTE_KERNEL;
    let line = format!("\n  va {kmain:#018x} maps to pa {pa:#018x}, pte {pte:#018x} DAG-X-RV\n");
    assert!(out.contains(&line), "want{line}in:\n{out}");
    let word = image_u32(&m, v["sepc"]);
    assert!(
        matches!(excalibur::decode(word, Xlen::Rv64), excalibur::Inst::Store { .. }),
        "sepc holds {}",
        excalibur::decode(word, Xlen::Rv64)
    );
    // The store didn't happen.
    let (fresh, _) = load_committed();
    assert_eq!(image_u32(&m, kmain), image_u32(&fresh, kmain));
}

/// Runs the kernel with `boot_fault` = `fault`, one of the memory API
/// misuses, and checks that it panics after the normal boot output,
/// printing `want`.
fn assert_misuse_panics(fault: u32, want: &str) {
    let (_, _, r, out) = run_with_fault(fault);
    assert_eq!(r, RunResult::Exited(PANIC_EXIT), "output:\n{out}");
    let masked = kernel::mask_dbg_pc(&out);
    let rest = masked
        .strip_prefix(&boot_output())
        .unwrap_or_else(|| panic!("output:\n{out}"));
    assert_eq!(rest, want);
}

#[test]
fn map_pages_panics_on_a_remap() {
    // The image's first page, mapped already.
    let va = KERNEL_LINK_ADDR;
    assert_misuse_panics(
        BOOT_FAULT_REMAP,
        &format!("map_pages: {va:#018x} is already mapped\npanic: map_pages: remap\n"),
    );
}

#[test]
fn unmap_pages_panics_on_an_unmapped_page() {
    assert_misuse_panics(
        BOOT_FAULT_UNMAP,
        "unmap_pages: 0x0000000000000000 is not mapped\npanic: unmap_pages: not mapped\n",
    );
}

#[test]
fn map_pages_panics_on_write_without_read() {
    assert_misuse_panics(BOOT_FAULT_W_ONLY, "panic: map_pages: bad permissions\n");
}

#[test]
fn walk_panics_on_a_superpage() {
    assert_misuse_panics(BOOT_FAULT_MEGAPAGE, "panic: walk: unexpected superpage\n");
}

#[test]
fn kfree_panics_on_a_bad_page() {
    // Pages are kernel pointers: the image's first page (an image address
    // converts to a page of the image, which is no free page), and the
    // direct map's page after the end of RAM.
    let (_, elf) = load_committed();
    let text = sym(&elf, "__text_start");
    let ram_end = mode().page_offset + RAM as u64;
    for (fault, page) in [
        (BOOT_FAULT_KFREE_KERNEL, text),
        (BOOT_FAULT_KFREE_PAST_RAM, ram_end),
    ] {
        assert_misuse_panics(fault, &format!("kfree: bad page {page:#018x}\npanic: kfree\n"));
    }
    // A free page plus 4: whichever page kalloc handed out.
    let (_, _, r, out) = run_with_fault(BOOT_FAULT_KFREE_UNALIGNED);
    assert_eq!(r, RunResult::Exited(PANIC_EXIT), "output:\n{out}");
    let rest = &out[out.find("kfree: ").unwrap()..];
    let addr = rest
        .strip_prefix("kfree: bad page 0x")
        .and_then(|r| r.strip_suffix("\npanic: kfree\n"))
        .unwrap_or_else(|| panic!("{rest}"));
    let addr = u64::from_str_radix(addr, 16).unwrap();
    assert_eq!(addr % PAGE, 4, "{addr:#x}");
    let kernel_end = mode().page_offset + (image_pa(sym(&elf, "__kernel_end")) - RAM_BASE);
    assert!(addr > kernel_end && addr < ram_end, "{addr:#x}");
}

#[test]
fn acquiring_a_held_lock_panics() {
    // Issue #61: a hart that acquires a lock it already holds would spin on
    // it forever; acquire panics instead, naming the lock and the hart.
    assert_misuse_panics(
        BOOT_FAULT_DOUBLE_ACQUIRE,
        "panic: acquire: lock \"boot_fault\" already held by hart 0\n",
    );
}

#[test]
fn releasing_a_lock_not_held_panics() {
    assert_misuse_panics(
        BOOT_FAULT_RELEASE_UNHELD,
        "panic: release: lock \"boot_fault\" not held by hart 0\n",
    );
}

#[test]
fn yielding_while_holding_a_lock_panics() {
    // A process that gave up the CPU holding a spinlock would leave
    // interrupts off and the lock taken for whoever runs next: sched()
    // allows proc_lock and nothing else.
    assert_misuse_panics(
        BOOT_FAULT_YIELD_HOLDING,
        "panic: sched: holding a lock other than proc_lock\n",
    );
}

#[test]
fn unbalanced_pop_off_panics() {
    // A pop_off() with no push_off() to match, and one that finds
    // interrupts back on under a push_off().
    assert_misuse_panics(BOOT_FAULT_POP_UNMATCHED, "panic: pop_off: no push_off to match\n");
    assert_misuse_panics(BOOT_FAULT_POP_INTR_ON, "panic: pop_off: interrupts are on\n");
}

/// The address after the call to `callee` in `caller`, an assembly
/// function: where `callee` returns to. A call is `auipc ra` then `jalr ra`.
fn return_address(m: &Machine, elf: &grail::elf::Elf, caller: &str, callee: &str) -> u64 {
    let (from, target) = (sym(elf, caller), sym(elf, callee));
    for at in (from..from + 0x100).step_by(4) {
        let decode = |a: u64| excalibur::decode(image_u32(m, a), Xlen::Rv64);
        // Auipc's immediate is the value it adds, already shifted.
        if let (
            excalibur::Inst::Auipc { rd: 1, imm: hi },
            excalibur::Inst::Jalr {
                rd: 1,
                rs1: 1,
                imm: lo,
            },
        ) = (decode(at), decode(at + 4))
        {
            if at.wrapping_add(hi as i64 as u64).wrapping_add(lo as i64 as u64) == target {
                return at + 8;
            }
        }
    }
    panic!("{caller} does not call {callee}");
}

#[test]
fn fault_with_interrupts_off_panics() {
    // An illegal instruction as kmain's first: S-mode traps work before
    // trap_init, since supervisor_entry sets stvec before it calls kmain.
    let (mut m, elf) = load_committed();
    let kmain = sym(&elf, "kmain");
    poke_image(&mut m, kmain, &[0, 0, 0, 0]);
    let r = m.run(Some(BOOT_MAX_STEPS), |_| {});
    let out = printed(&m);
    assert_eq!(r, RunResult::Exited(PANIC_EXIT), "output:\n{out}");
    assert!(out.starts_with("panic: illegal instruction\n"), "{out}");
    let (_, v) = parse_panic(&out);
    assert_eq!((v["scause"], v["sepc"], v["stval"]), (2, kmain, 0));
    assert_eq!(v["sstatus"] & MSTATUS_SPIE, 0);
    // ra is where kmain would have returned to: supervisor_entry's
    // instruction after the call.
    assert_eq!(v["ra"], return_address(&m, &elf, "supervisor_entry", "kmain"));
}

#[test]
fn the_boot_stack_has_a_guard_page() {
    // The page below hart 0's boot stack is past .bss, and unmapped: an
    // overflow faults there, and kernel_vec reports it like a kernel
    // stack's.
    let (m, elf, r, out) = run_with_fault(BOOT_FAULT_BOOT_STACK_OVERFLOW);
    assert_eq!(r, RunResult::Exited(PANIC_EXIT), "output:\n{out}");
    let bottom = sym(&elf, "__stack_bottom");
    assert_eq!(bottom % KSTACK_SLOT, 0);
    assert!(bottom - PAGE >= sym(&elf, "__bss_end"));
    let kpt = kernel_pagetable_pa(&m, &elf);
    assert!(translate(&m, kpt, bottom - PAGE).is_none());
    assert!(translate(&m, kpt, bottom).is_some());
    assert!(translate(&m, kpt, bottom - 2 * PAGE).is_some());
    let report = &out[out.find("panic: ").unwrap()..];
    let mut lines = report.lines();
    assert_eq!(
        lines.next(),
        Some("panic: kernel stack overflow on hart 0's boot stack"),
        "{report}"
    );
    assert_eq!(
        lines.next().unwrap().rsplit(' ').next(),
        Some(format!("{bottom:#018x}-{:#018x}", bottom + KSTACK_SIZE).as_str()),
        "{report}"
    );
    assert_eq!(lines.next(), Some("  store page fault"), "{report}");
}

#[test]
fn pa_of_a_vmalloc_address_panics() {
    // pa() of the UART's ioremap() address: in the vmalloc area, where a
    // virtual address has no physical one to compute (Linux's
    // CONFIG_DEBUG_VIRTUAL check), so it panics instead of giving a wrong one.
    let (m, elf, r, out) = run_with_fault(BOOT_FAULT_PA_VMALLOC);
    assert_eq!(r, RunResult::Exited(PANIC_EXIT), "output:\n{out}");
    let uart = image_u64(&m, sym(&elf, "uart_regs"));
    let want = format!(
        "pa: {uart:#018x} is neither in the direct map nor in the kernel image\npanic: pa: no physical address\n"
    );
    assert!(out.ends_with(&want), "want {want:?}, got:\n{out}");
}

#[test]
fn a_kernel_stack_overflow_hits_the_guard_page_and_is_reported() {
    let (_, elf, r, out) = run_with_fault(BOOT_FAULT_KSTACK_OVERFLOW);
    assert_eq!(r, RunResult::Exited(PANIC_EXIT), "output:\n{out}");
    let report = &out[out.find("panic: ").unwrap()..];
    let mut lines = report.lines();
    assert_eq!(
        lines.next(),
        Some("panic: kernel stack overflow in overflow (hart 0)"),
        "{report}"
    );
    // The stack the thread ran out of: a slot's lower half in the vmalloc
    // area, and sp just below it, in the guard page.
    let sp_line = lines.next().unwrap();
    let words: Vec<&str> = sp_line.split_whitespace().collect();
    assert_eq!(words[0], "sp", "{sp_line}");
    let sp = u64::from_str_radix(&words[1][2..18], 16).unwrap();
    let range: Vec<u64> = words[4]
        .split('-')
        .map(|w| u64::from_str_radix(&w[2..], 16).unwrap())
        .collect();
    assert_eq!(range[1] - range[0], KSTACK_SIZE);
    assert_eq!(range[0] % KSTACK_SLOT, 0);
    assert!(range[0] >= mode().vmalloc_start, "{report}");
    assert!(
        range[1] <= mode().vmalloc_start + KSTACK_SLOTS * KSTACK_SLOT,
        "{report}"
    );
    // The fault itself: a store to the guard page, the page below the
    // stack, from the recursion (not from kernel_vec: the frame went to the
    // overflow stack instead).
    assert_eq!(lines.next(), Some("  store page fault"), "{report}");
    let v = panic_values(report);
    assert_eq!(v["scause"], STORE_PAGE_FAULT);
    let stval = v["stval"];
    assert!(
        (range[0] - PAGE..range[0]).contains(&stval),
        "stval {stval:#x}\n{report}"
    );
    assert!(
        (range[0] - PAGE..range[0] + 512).contains(&sp),
        "sp {sp:#x}\n{report}"
    );
    let recurse = sym(&elf, "recurse");
    assert!(
        (recurse..recurse + 0x100).contains(&v["sepc"]),
        "sepc {:#x}",
        v["sepc"]
    );
    assert!(
        report.contains(&format!("\n  va {stval:#018x} is not mapped\n")),
        "{report}"
    );
}

// ---- Memory: the page allocator and the kernel page table ----

// PTE bits (Sv48 and Sv39).
const PTE_V: u64 = 1 << 0;
const PTE_R: u64 = 1 << 1;
const PTE_W: u64 = 1 << 2;
const PTE_X: u64 = 1 << 3;
const PTE_U: u64 = 1 << 4;
const PTE_G: u64 = 1 << 5;
const PTE_A: u64 = 1 << 6;
const PTE_D: u64 = 1 << 7;
/// What the kernel's own leaves have besides their permissions (vm.h's
/// PTE_KERNEL, Linux's _PAGE_KERNEL): A and D set up front, and G.
const PTE_KERNEL: u64 = PTE_G | PTE_A | PTE_D;
/// The bits of a PTE the kernel chooses; RSW (9:8) is for software, which
/// the kernel doesn't use, and bits 63:54 are reserved.
const PTE_KERNEL_BITS: u64 = PTE_V | PTE_R | PTE_W | PTE_X | PTE_U | PTE_G | PTE_A | PTE_D;
const PTE_RSW: u64 = 0x300;

/// The physical address of the kernel page table's root.
fn kernel_pagetable_pa(m: &Machine, elf: &grail::elf::Elf) -> u64 {
    let kpt = image_u64(m, sym(elf, "kernel_pagetable"));
    kpt - image_u64(m, sym(elf, "va_pa_offset"))
}

/// What the page table whose root is at physical address `root` maps `va`
/// to: the physical address and the leaf PTE, or None.
fn translate(m: &Machine, root: u64, va: u64) -> Option<(u64, u64)> {
    let mut table = root;
    for level in (0..mode().levels).rev() {
        let vpn = va >> (12 + 9 * level) & 0x1ff;
        let pte = read64_pa(m, table + 8 * vpn);
        if pte & PTE_V == 0 {
            return None;
        }
        let pa = (pte >> 10 & 0xfff_ffff_ffff) << 12;
        if pte & (PTE_R | PTE_W | PTE_X) != 0 {
            let offset = va & ((1u64 << (12 + 9 * level)) - 1);
            return Some((pa + offset, pte));
        }
        table = pa;
    }
    None
}

/// Every leaf of the page table whose root is at physical address `root`,
/// in the upper half, as (va, pa, pte, the bytes it maps), and the tables
/// it has (their physical addresses, the root's among them). Panics on a
/// reserved encoding, a pointer with D, A, U or G (reserved there) or a
/// misaligned superpage, which the kernel never makes.
fn kernel_leaves(m: &Machine, root: u64) -> (Vec<(u64, u64, u64, u64)>, Vec<u64>) {
    fn walk(
        m: &Machine,
        table: u64,
        level: u32,
        base: u64,
        leaves: &mut Vec<(u64, u64, u64, u64)>,
        tables: &mut Vec<u64>,
        top: bool,
    ) {
        tables.push(table);
        let range = if top { 256..512 } else { 0..512 };
        for i in range {
            let pte = read64_pa(m, table + 8 * i);
            if pte & PTE_V == 0 {
                continue;
            }
            assert_eq!(pte >> 54, 0, "reserved bits in {pte:#x}");
            let shift = 12 + 9 * level;
            let mut va = base | i << shift;
            if top {
                // Sign-extended: the upper half.
                va |= !0u64 << (12 + 9 * mode().levels);
            }
            let pa = (pte >> 10 & 0xfff_ffff_ffff) << 12;
            if pte & (PTE_R | PTE_W | PTE_X) != 0 {
                assert_eq!(pa % (1 << shift), 0, "misaligned superpage {va:#x}: {pte:#x}");
                leaves.push((va, pa, pte, 1 << shift));
            } else {
                assert_eq!(pte & 0x3ff, PTE_V, "pointer {va:#x}: {pte:#x}");
                assert!(level > 0, "pointer at level 0: {va:#x}");
                walk(m, pa, level - 1, va, leaves, tables, false);
            }
        }
    }
    let (mut leaves, mut tables) = (Vec::new(), Vec::new());
    walk(m, root, mode().levels - 1, 0, &mut leaves, &mut tables, true);
    (leaves, tables)
}

/// Checks the kernel page table after a boot with `ram_mib` MiB of RAM:
/// the image at its link address, page by page with its parts'
/// permissions, but for the boot stack's guard page; RAM in the direct map,
/// R+W, but for the firmware's region, in the largest leaves its alignment
/// allows; the fixmap's two pages; the devices the drivers asked for, and
/// the kernel stacks, in the vmalloc area; nothing in the lower half, and
/// nothing else. Every leaf with A, D and G, none with U.
fn check_kernel_memory(ram_mib: usize) {
    let (mut m, elf) = load_committed_on(
        MachineConfig {
            ram_size: ram_mib << 20,
            ..config()
        },
        Some(kernel::typed(EXIT)),
    );
    let r = m.run(Some(BOOT_MAX_STEPS * 4), |_| {});
    let out = printed(&m);
    assert_eq!(r, RunResult::Exited(0), "{ram_mib} MiB, output:\n{out}");
    assert!(
        out.contains(&format!(
            "ram: {ram_mib} MiB at 0x80000000\npaging: on ({})\nvm self-test: 26/26 passed\n",
            mode().name
        )),
        "{out}"
    );
    assert_eq!(image_u64(&m, sym(&elf, "ram_size")), (ram_mib as u64) << 20);
    let root = kernel_pagetable_pa(&m, &elf);
    let kernel_end = image_pa(sym(&elf, "__kernel_end"));
    let ram_end = RAM_BASE + ((ram_mib as u64) << 20);
    assert!(root >= kernel_end && root < ram_end, "root {root:#x}");
    // Nothing in the lower half: the identity mapping is gone.
    for i in 0..256 {
        assert_eq!(read64_pa(&m, root + 8 * i) & PTE_V, 0, "root[{i}]");
    }
    let (leaves, tables) = kernel_leaves(&m, root);
    for &(va, _, pte, _) in &leaves {
        assert_eq!(pte & PTE_KERNEL, PTE_KERNEL, "{va:#x}: {pte:#x}");
        assert_eq!(pte & (PTE_U | PTE_RSW), 0, "{va:#x}: {pte:#x}");
    }
    // Every table is a page of free RAM but the fixmap's, which are the early
    // page table's, in the image.
    let early = (
        image_pa(sym(&elf, "early_tables")),
        image_pa(sym(&elf, "__bss_end")),
    );
    for &t in &tables {
        assert!(
            (t >= kernel_end && t < ram_end) || (early.0..early.1).contains(&t),
            "table {t:#x}"
        );
    }

    let perm = |pte: u64| pte & PTE_KERNEL_BITS & !PTE_KERNEL;
    // The image.
    let (text, rodata, data) = (
        sym(&elf, "__text_start"),
        sym(&elf, "__rodata_start"),
        sym(&elf, "__data_start"),
    );
    let guard = sym(&elf, "__stack_bottom") - PAGE;
    let image: Vec<_> = leaves.iter().filter(|l| l.0 >= KERNEL_LINK_ADDR).collect();
    let mut want_image = Vec::new();
    for va in (text..sym(&elf, "__kernel_end")).step_by(PAGE as usize) {
        let p = if va < rodata {
            PTE_V | PTE_R | PTE_X
        } else if va < data {
            PTE_V | PTE_R
        } else {
            PTE_V | PTE_R | PTE_W
        };
        if va != guard {
            want_image.push((va, image_pa(va), p, PAGE));
        }
    }
    let got_image: Vec<_> = image.iter().map(|l| (l.0, l.1, perm(l.2), l.3)).collect();
    assert_eq!(got_image, want_image, "the image's mapping");
    // The direct map: RAM from the end of the firmware's region, R+W, each
    // stretch in the largest leaves whose alignment fits.
    let offset = mode().page_offset - RAM_BASE;
    let direct: Vec<_> = leaves
        .iter()
        .filter(|l| l.0 >= mode().page_offset && l.0 < KERNEL_LINK_ADDR)
        .collect();
    let mut at = RAM_BASE + 0x20_0000;
    for l in &direct {
        assert_eq!(l.0, l.1 + offset, "{:#x} not in the direct map's place", l.0);
        assert_eq!(l.1, at, "a hole in the direct map at {at:#x}");
        assert_eq!(perm(l.2), PTE_V | PTE_R | PTE_W, "{:#x}", l.0);
        // As large as the place allows, up to a 1 GiB leaf.
        let largest = [1u64 << 30, 1 << 21, PAGE]
            .into_iter()
            .find(|&s| at % s == 0 && at + s <= ram_end)
            .unwrap();
        assert_eq!(l.3, largest, "the leaf at {at:#x}");
        at += l.3;
    }
    assert_eq!(at, ram_end, "the direct map ends with RAM");
    // The rest: the fixmap, then the vmalloc area's kernel stacks and
    // devices, below the direct map.
    let rest: Vec<_> = leaves.iter().filter(|l| l.0 < mode().page_offset).collect();
    let ioremap = mode().vmalloc_start + KSTACK_SLOTS * KSTACK_SLOT;
    let mut devices = Vec::new();
    let mut fixmap = 0;
    for l in &rest {
        if l.0 < mode().vmalloc_start {
            fixmap += 1;
            assert!(
                [SYSINFO_BASE, UART_BASE].contains(&l.1),
                "fixmap {:#x} -> {:#x}",
                l.0,
                l.1
            );
        } else if l.0 < ioremap {
            // A kernel stack's page: in the lower half of its slot, in RAM.
            let slot_off = (l.0 - mode().vmalloc_start) % KSTACK_SLOT;
            assert!(slot_off < KSTACK_SIZE, "{:#x} past its stack", l.0);
            assert!(l.1 >= kernel_end && l.1 < ram_end, "stack page at {:#x}", l.1);
            assert_eq!(perm(l.2), PTE_V | PTE_R | PTE_W);
        } else {
            assert_eq!(perm(l.2), PTE_V | PTE_R | PTE_W);
            devices.push(l.1);
        }
    }
    assert_eq!(fixmap, 2, "the fixmap: sysinfo and the UART");
    // What the drivers asked for: sysinfo, the UART and the PLIC; not the
    // CLINT, which is the firmware's, nor the test device or the
    // framebuffer.
    for base in [SYSINFO_BASE, UART_BASE, PLIC_BASE] {
        assert!(devices.contains(&base), "{base:#x} not mapped: {devices:x?}");
    }
    for base in [
        grail::bus::CLINT_BASE,
        grail::bus::TEST_DEV_BASE,
        grail::bus::FB_CTRL_BASE,
    ] {
        assert!(
            !devices.iter().any(|&d| (base..base + PAGE).contains(&d)),
            "{base:#x}"
        );
    }
}

#[test]
fn the_kernel_page_table_maps_the_kernel_half_only() {
    check_kernel_memory(RAM >> 20);
}

#[test]
fn the_kernel_takes_ram_from_sysinfo() {
    // 8 MiB at least: the image is 2.1 MiB with the programs it embeds, from
    // 0x8020_0000, after the firmware's 2 MiB, and the device tree goes at
    // the last 2 MiB boundary below the end of RAM, above it. RAM that ends
    // inside a 2 MiB leaf's stretch, and RAM up to 4 GiB, whose direct map
    // ends in a 1 GiB leaf.
    for mib in [10, 64, 2048] {
        check_kernel_memory(mib);
    }
}

#[test]
fn the_page_table_shares_the_fixmap_with_the_early_one() {
    let (mut m, elf) = load_committed();
    assert_eq!(m.run(Some(BOOT_MAX_STEPS), |_| {}), RunResult::Exited(0));
    let kpt = kernel_pagetable_pa(&m, &elf);
    let root: Vec<u64> = (0..512).map(|i| read64_pa(&m, kpt + 8 * i)).collect();
    // The fixmap, the vmalloc area (the kernel stacks, the devices), the
    // direct map and the image, each through a table under the root.
    let used: Vec<usize> = (256..512).filter(|&i| root[i] & PTE_V != 0).collect();
    assert_eq!(used, mode().root_entries);
    // The fixmap's entry is the early page table's own, as Linux shares its
    // fixmap tables between early_pg_dir and swapper_pg_dir.
    let early = image_pa(sym(&elf, "early_pg_dir"));
    let fix = mode().root_entries[0];
    assert_eq!(root[fix], read64_pa(&m, early + 8 * fix as u64));
    // The devices the kernel uses, ioremap()ed after the kernel stacks,
    // each in its own place: sysinfo and the UART, which the boot reached
    // through the fixmap first. Every leaf has A, D and G.
    let ioremap = mode().vmalloc_start + KSTACK_SLOTS * KSTACK_SLOT;
    for (name, pa) in [("sysinfo_regs", SYSINFO_BASE), ("uart_regs", UART_BASE)] {
        let va = image_u64(&m, sym(&elf, name));
        assert!(
            va >= ioremap && va < mode().vmalloc_start + (1 << 30),
            "{name} {va:#x}"
        );
        let (got, pte) = translate(&m, kpt, va).unwrap_or_else(|| panic!("{name} {va:#x} unmapped"));
        assert_eq!(got, pa, "{name}");
        assert_eq!(pte & 0xff, 0xe7, "{name}: {pte:#x}");
    }
    // The early page table maps RAM exactly, and no devices but the
    // fixmap's: its direct map ends with RAM. Neither table maps the
    // firmware's 2 MiB, which the device tree marks no-map, as Linux's linear
    // map leaves a MEMBLOCK_NOMAP region out: the direct map starts after it,
    // still at RAM's base plus PAGE_OFFSET.
    let ram_end = mode().page_offset + RAM as u64;
    assert!(translate(&m, early, ram_end - PAGE).is_some());
    assert!(translate(&m, early, ram_end).is_none());
    for (table, name) in [(kpt, "the kernel's"), (early, "the early")] {
        for off in (0..2 << 20).step_by(PAGE as usize) {
            assert!(
                translate(&m, table, mode().page_offset + off).is_none(),
                "{name} page table maps the firmware at {:#x}",
                RAM_BASE + off
            );
        }
        let (pa, pte) = translate(&m, table, mode().page_offset + (2 << 20))
            .unwrap_or_else(|| panic!("{name} page table: 0x80200000 unmapped"));
        assert_eq!(pa, RAM_BASE + (2 << 20), "{name}");
        assert_eq!(pte & 0xf, 0x7, "{name}: R+W, {pte:#x}");
    }
    assert!(translate(&m, early, mode().vmalloc_start).is_none());
    assert!(translate(&m, early, mode().vmalloc_start + 0x1000_0000).is_none());
}

#[test]
fn the_kernel_falls_back_to_sv39() {
    // A hart without Sv48 (Grail's --paging sv39): satp refuses MODE 9, the
    // kernel's probe sees it and builds its tables for Sv39, with Linux's
    // Sv39 layout; the image's address is the same. The tests of the boot
    // and of the address space, on such harts.
    MODE.set(SV39);
    the_kernel_boots_through_its_self_tests();
    the_kernel_page_table_maps_the_kernel_half_only();
    the_page_table_shares_the_fixmap_with_the_early_one();
    the_kernel_runs_on_up_to_eight_harts();
    the_kernel_configures_its_network_with_dhcp();
    kernel_stacks_keep_a_margin();
    illegal_instruction_panics_with_a_64_bit_register_dump();
    unmapped_load_panics();
    store_to_kernel_text_panics();
    map_pages_panics_on_a_remap();
    walk_panics_on_a_superpage();
    kfree_panics_on_a_bad_page();
    a_kernel_stack_overflow_hits_the_guard_page_and_is_reported();
    the_kernel_boots_from_disk();
    MODE.set(SV48);
}

// ---- The console and the shell ----

/// The UART registers the tests look for (spec section 7).
const UART_RBR: u64 = 0;
const UART_IER: u64 = 1;
const UART_LSR: u64 = 5;
/// IER bit 0: interrupt while received data is available.
const UART_IER_ERBFI: u64 = 1;

/// PLIC registers (spec section 7.1) for source `src` / context `ctx`.
fn plic_priority(src: u32) -> u64 {
    PLIC_BASE + 4 * u64::from(src)
}
fn plic_enable(ctx: u32) -> u64 {
    PLIC_BASE + 0x2000 + 0x80 * u64::from(ctx)
}
fn plic_threshold(ctx: u32) -> u64 {
    PLIC_BASE + 0x20_0000 + 0x1000 * u64::from(ctx)
}

/// The size of the kernel's receive ring buffer (`UART_RX_SIZE` in
/// kernel/include/uart.h).
const UART_RX_SIZE: usize = 128;

/// What the shell's `help` prints (kernel/user/sh.c).
const HELP: &str = "\
help            this list
exit [N]        leave the shell, with status N or the last command's
wait            wait for every background program
cd [DIR]        change the working directory, to HOME (or /) without DIR
exec [PROG ...] replace the shell with PROG; without one, keep its redirections
export [NAME..] pass variables (NAME or NAME=VALUE) to programs; alone, list them
unset NAME...   remove variables
NAME=VALUE      set a variable; before a command, for that command alone
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

/// Runs the kernel with `input` typed on the UART. Returns how the run
/// ended and what was printed from the shell's banner on.
fn run_shell(input: &[u8]) -> (RunResult, String) {
    let (mut m, _) = load_committed_on(config(), Some(kernel::typed(input)));
    let r = m.run(Some(BOOT_MAX_STEPS), |_| {});
    let out = printed(&m);
    // Everything before the shell is the usual boot.
    let at = out
        .find(SHELL_BANNER)
        .unwrap_or_else(|| panic!("the shell never started:\n{out}"));
    assert_eq!(mask(&out[..at]), format!("{}{KSTACK_OUTPUT}", boot_output()));
    (r, out[at..].to_string())
}

/// The lines between the last echo of `command` and the next prompt.
fn command_output<'a>(out: &'a str, command: &str) -> Vec<&'a str> {
    common::command_output(out, command)
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
fn a_failed_builtin_gives_dashs_status() {
    // A built-in's status is the last one, which `exit` gives: 0, or 2 when
    // it fails, as dash's (kernel/user/sh.c, at the top). The console's
    // shell is interactive, so it carries on (fs.rs runs the same errors in
    // shells that are not, which end). export and unset stop at a word that
    // is no name; a successful built-in's status is 0 again.
    for (input, line, printed) in [
        (
            &b"hello\nexport 1A\nexit\n"[..],
            "export 1A",
            "sh: export: 1A: bad variable name",
        ),
        (
            b"hello\nunset 1A\nexit\n",
            "unset 1A",
            "sh: unset: 1A: bad variable name",
        ),
        (
            b"cd /nope\nexit\n",
            "cd /nope",
            "sh: cd: /nope: no such file or directory",
        ),
    ] {
        let (r, out) = run_shell(input);
        assert_eq!(command_output(&out, line), [printed], "{out}");
        assert_eq!(r, RunResult::Exited(2), "{out}");
    }
    let (r, out) = run_shell(b"export A=1 1B C=3\nenv\nexport 1A\nexport A\nexit\n");
    assert_eq!(
        command_output(&out, "export A=1 1B C=3"),
        ["sh: export: 1B: bad variable name"],
        "{out}"
    );
    assert_eq!(
        command_output(&out, "env"),
        ["HOME=/", "TERM=linux", "A=1"],
        "{out}"
    );
    assert_eq!(r, RunResult::Exited(0), "{out}");
    // An assignment that can't be made: the shell has room for 32
    // variables, two of them HOME and TERM, so the 15th of the second line
    // is one too many, and the line's status is 2.
    let names = |c: char| (0..16).map(|i| format!("{c}{i}=1")).collect::<Vec<_>>().join(" ");
    let (a, b) = (names('A'), names('B'));
    let (r, out) = run_shell(format!("{a}\n{b}\nexit\n").as_bytes());
    assert_eq!(
        command_output(&out, &b),
        ["sh: B14=1: too many variables"],
        "{out}"
    );
    assert_eq!(r, RunResult::Exited(2), "{out}");
}

#[test]
fn shell_takes_more_input_than_the_ring_buffer_holds() {
    // 40 commands, typed all at once: 245 bytes for a 128-byte buffer. When it
    // is full the driver turns the receive interrupt off rather than drop
    // bytes, and turns it back on once the console has taken half of it.
    let input = format!("{}exit\n", "ticks\n".repeat(40));
    assert!(input.len() > UART_RX_SIZE + UART_RX_SIZE / 2);
    assert!(input.len() < 2 * UART_RX_SIZE);
    let (mut m, elf) = load_committed_on(config(), Some(kernel::typed(input.as_bytes())));
    let trap_init = sym(&elf, "trap_init");
    let mut booted = false;
    let mut ier_writes = Vec::new();
    let r = m.run(Some(common::RUN_MAX_STEPS), |cpu| {
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
    // No input: the kernel waits at the prompt. It is looked at once the
    // hart first reaches wfi: before, a tick may be on its way (the timing
    // of boot and of the shell's start decides it), STIP pending with
    // M-mode's timer off until the handler's set_timer turns it on again.
    let (mut m, elf) = load_committed_on(config(), None);
    let wfi = sym(&elf, "wfi");
    step_until(&mut m, BOOT_MAX_STEPS, |_| {}, |m| printed(m).ends_with(PROMPT));
    step_until(&mut m, BOOT_MAX_STEPS, |_| {}, |m| m.cpu.pc == wfi);
    let mut reg = |addr| m.cpu.bus.read(addr, 4).unwrap();
    // Source 1 (the UART) at priority 1, enabled for context 1 (S-mode) only,
    // whose threshold of 0 lets it through.
    assert_eq!(reg(plic_priority(UART_IRQ)), 1);
    assert_eq!(reg(plic_enable(1)), 1 << UART_IRQ);
    assert_eq!(reg(plic_threshold(1)), 0);
    assert_eq!(reg(plic_enable(0)), 0);
    // sie.SEIE on (with the tick's STIE and the IPI's SSIE, and M-mode's
    // timer, which the next tick's set_timer has enabled, and IPI); M-mode
    // external interrupts stay off.
    let mie = m.cpu.csr.mie;
    assert_eq!(
        mie,
        (1 << IRQ_M_TIMER) | (1 << IRQ_M_SOFT) | (1 << IRQ_S_SOFT) | (1 << IRQ_S_TIMER) | (1 << IRQ_S_EXT),
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
    let (mut m, elf) = load_committed_on(config(), Some(rx));
    let ticks = sym(&elf, "ticks");
    let tick_count = |m: &Machine| image_u32(m, ticks);
    let uart_intr = function("uart_intr");
    let kernel_vec = sym(&elf, "kernel_vec");
    let wfi = sym(&elf, "wfi");

    step_until(&mut m, BOOT_MAX_STEPS, |_| {}, |m| printed(m).ends_with(PROMPT));
    // The ten boot ticks, and the few the kernel stacks' self-test and the
    // shell's start take.
    let start = tick_count(&m);
    assert!((10..=20).contains(&start), "{start} ticks at the prompt");
    // The shell still has its read to make after the prompt: the count starts once
    // the hart first waits, so that a tick that comes on the way (the timing of boot
    // and of the shell's start decides it) is not counted as one it waited for.
    step_until(&mut m, BOOT_MAX_STEPS, |_| {}, |m| m.cpu.pc == wfi);
    let start = tick_count(&m);
    let idle_end = start + 50;

    // Fifty idle ticks: the hart spends them in wfi (Grail skips the wait to
    // the next tick), waking only for the tick itself, a couple of thousand
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
        |m| tick_count(m) == idle_end,
    );
    assert_eq!(uart_reads, 0, "the kernel polls the UART while idle");
    assert!((50..=51).contains(&wfis), "{wfis} wfi for 50 ticks");
    // A few thousand each with merlin's unoptimized code: the scheduler's
    // look for work, the tick, its next deadline and the SBI call that sets
    // it.
    assert!(steps < 50 * 4000, "{steps} steps for 50 idle ticks");

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
                Some((UART_RBR, false, _)) => rbr_reads.push((cpu.pc, in_function(cpu, &uart_intr))),
                Some((UART_LSR, false, _)) => lsr_reads += 1,
                _ => {}
            }
            if cpu.pc == kernel_vec && cpu.csr.scause == INTERRUPT_BIT | u64::from(IRQ_S_EXT) {
                external += 1;
            }
        },
        |m| printed(m).ends_with(PROMPT) && printed(m).len() > before,
    );
    assert_eq!(
        &printed(&m)[before..],
        format!("ticks\nticks: {idle_end}\n{PROMPT}")
    );
    assert_eq!(tick_count(&m), idle_end);
    assert_eq!(rbr_reads.len(), 6, "one read per byte");
    assert!(
        rbr_reads.iter().all(|&(_, inside)| inside),
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
fn input_arriving_just_before_wfi_wakes_the_console() {
    // The scheduler turns interrupts off from its search for a runnable
    // process until wfi. A byte that arrives in between is then still pending
    // when wfi runs, so wfi returns at once, the console wakes the shell and
    // the answer comes within the same tick. With interrupts on, the byte's
    // interrupt would be taken before wfi, which would then sleep until the
    // next tick. A 100 MHz timebase makes the interval ten million steps,
    // so that starting and running `ticks` (merlin's code is unoptimized)
    // surely fits in what is left of the tick.
    let (tx, rx) = std::sync::mpsc::channel();
    let (mut m, elf) = load_committed_on(
        MachineConfig {
            nominal_hz: 100_000_000,
            ..config()
        },
        Some(rx),
    );
    let ticks = sym(&elf, "ticks");
    let tick_count = |m: &Machine| image_u32(m, ticks);
    let wfi = sym(&elf, "wfi");

    step_until(&mut m, BOOT_MAX_STEPS, |_| {}, |m| printed(m).ends_with(PROMPT));
    // The shell sleeps in read; the scheduler found nothing to run and is
    // about to idle.
    step_until(&mut m, 20_000, |_| {}, |m| m.cpu.pc == wfi);
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

// ---- Disks: the block driver and the buffer cache (issue #65) ----

/// Block size of the buffer cache (`BSIZE` in kernel/include/bio.h): two
/// sectors.
const BSIZE: usize = 1024;
const BLOCK_SECTORS: u64 = 2;
/// Buffers in the cache (`NBUF` in kernel/include/bio.h).
const NBUF: u64 = 54;
/// The disk most tests attach: 64 blocks, enough for the self-test's LRU
/// check, which needs NBUF + 1.
const DISK_SECTORS: usize = 128;
/// What the self-test counts on a read-write disk that big
/// (kernel/blktest.c): 4 for the three readers, 7 for the scratch block, 3
/// for the errors, 2 for the LRU.
const BLK_SELFTEST_CHECKS: u32 = 16;
/// The self-test's scratch block: the last whole one, here sectors 126-127.
const SCRATCH_SECTOR: u64 = (DISK_SECTORS as u64 / BLOCK_SECTORS - 1) * BLOCK_SECTORS;

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
/// has no filesystem, and the rest of a run that exits.
fn boot_output_with(disks: &str, selftest: &str) -> String {
    format!(
        "{}{selftest}{NO_FILESYSTEM}{KSTACK_OUTPUT}{SHELL_BANNER}{PROMPT}exit\n",
        boot_output().replace("disk: none\n", disks)
    )
}

/// Loads the kernel on the tests' machine with `disks` attached and `EXIT`
/// typed.
fn load_with_disks(disks: Vec<DiskConfig>) -> (Machine, grail::elf::Elf) {
    load_committed_on(MachineConfig { disks, ..config() }, Some(kernel::typed(EXIT)))
}

/// A command a disk ran, as the device saw it when `BLK_CMD` was written.
#[derive(Debug, Clone, PartialEq)]
struct DiskCommand {
    disk: usize,
    cmd: u32,
    sector: u64,
    count: u32,
    dma: u64,
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
    sleeps: Vec<u64>,
    /// Every load from or store to a disk register.
    accesses: usize,
}

/// A disk register access by the instruction about to run: (disk, offset,
/// store?), the address translated through the hart's page table (the
/// driver reaches the registers through their ioremap() mapping).
fn disk_access(cpu: &grail::cpu::Cpu) -> Option<(usize, u32, bool)> {
    let a = mem_access(cpu)?;
    let off = device_offset(a.addr, BLK_BASE, BLK_SIZE * MAX_DISKS as u64)?;
    Some((
        (off / BLK_SIZE) as usize,
        (off % BLK_SIZE) as u32,
        a.load_rd.is_none(),
    ))
}

/// Runs `m` (loaded with the kernel) for at most `max_steps`, watching every
/// disk access.
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
        let in_blk_intr = in_function(cpu, &blk_intr);
        match (off, store) {
            (BLK_REG_CMD, true) => {
                if cpu.bus.disks[disk].done() {
                    t.overlapping += 1;
                }
                pending_cmd = Some((disk, mem_access(cpu).unwrap().value as u32));
            }
            (BLK_REG_STATUS, false) => t.status_reads += 1,
            (BLK_REG_ERROR, false) if !in_blk_intr => t.stray_error_reads += 1,
            (BLK_REG_STATUS, true) if in_blk_intr => {
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
    let out = mask(&printed(&m));
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
        mask(&out),
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
        assert_eq!(
            (w.disk, w.sector, w.count),
            (0, SCRATCH_SECTOR, BLOCK_SECTORS as u32)
        );
    }
    assert_eq!(writes[0].data, blk_pattern());
    let scratch = SCRATCH_SECTOR as usize * 512;
    assert_eq!(writes[1].data, image[scratch..scratch + BSIZE]);
    assert_eq!(disk_contents(&mut m, 0), image);
    assert_eq!(t.commands.iter().filter(|c| c.cmd == BLK_CMD_FLUSH).count(), 1);

    // The errors reached the kernel: the block past the end (twice, since a
    // failed read isn't cached) and the buffer outside RAM. Everything else
    // worked.
    let failed: Vec<(u64, u64, u32)> = t
        .commands
        .iter()
        .zip(&t.results)
        .filter(|(_, e)| **e != 0)
        .map(|(c, &e)| (c.sector, c.dma, e))
        .collect();
    let past_end = DISK_SECTORS as u64;
    assert_eq!(failed.len(), 3, "{failed:?}");
    assert_eq!((failed[0].0, failed[0].2), (past_end, blk_error::OUT_OF_RANGE));
    assert_eq!((failed[1].0, failed[1].2), (past_end, blk_error::OUT_OF_RANGE));
    // The page after the end of RAM (the self-test's buffer outside RAM).
    let ram_end = RAM_BASE + RAM as u64;
    assert_eq!((failed[2].1, failed[2].2), (ram_end, blk_error::BAD_DMA));

    // The cache: every bread() that missed read one block into a buffer, and
    // the hits needed no request at all. The buffers are in the image: their
    // physical addresses are what the disk got.
    let bufs = function("bufs");
    let bufs_pa = image_pa(bufs.start)..image_pa(bufs.start) + (bufs.end - bufs.start);
    let buffer_reads = t
        .commands
        .iter()
        .filter(|c| c.cmd == BLK_CMD_READ && bufs_pa.contains(&c.dma))
        .count() as u32;
    let (hits, misses) = (
        image_u32(&m, sym(&elf, "bio_hits")),
        image_u32(&m, sym(&elf, "bio_misses")),
    );
    assert_eq!(misses, buffer_reads);
    // Reader c, the scratch block read back and read again to restore it;
    // in the LRU check blocks 0 and 1 (left there by the readers) at the
    // start of its loop, block 0 again after it, then blocks 0 and NBUF;
    // last, the filesystem's mount looking for a superblock in block 1,
    // which the LRU check read last.
    assert_eq!(hits, 9);
    assert!(bufs.end - bufs.start >= NBUF * BSIZE as u64);

    // Which buffer each miss took. A buffer whose read failed holds nothing,
    // so the next miss reuses it before any cached block: the second read
    // past the end, then block 2 in the LRU loop, take the buffer of the
    // first.
    let block_reads: Vec<(u64, u64)> = t
        .commands
        .iter()
        .filter(|c| c.cmd == BLK_CMD_READ && bufs_pa.contains(&c.dma))
        .map(|c| (c.sector / BLOCK_SECTORS, c.dma))
        .collect();
    let read_at = |block: u64| {
        block_reads
            .iter()
            .position(|&(b, _)| b == block)
            .unwrap_or_else(|| panic!("block {block} never read: {block_reads:?}"))
    };
    let end = DISK_SECTORS as u64 / BLOCK_SECTORS;
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
    // But for the disk's registers' mapping: their ioremap() takes a page
    // for the table it needs.
    let tables = |m: &Machine| kernel_leaves(m, kernel_pagetable_pa(m, &elf)).1.len() as u32;
    let disk_tables = tables(&with_disk) - tables(&plain);
    assert_eq!(disk_tables, 1);
    assert_eq!(
        common::free_pages(&with_disk, &elf) + disk_tables,
        common::free_pages(&plain, &elf)
    );
}

#[test]
fn a_slow_disk_leaves_the_kernel_asleep_in_wfi() {
    // Grail's disks finish each command at once. Here the first one's
    // interrupt is held back (BLK_CTRL cleared right after the command) for
    // a few ticks, as a slow disk would: every process is then asleep and
    // the scheduler idles in wfi, waking for each tick and nothing else.
    let (mut m, elf) = load_with_disks(vec![DiskConfig::memory(disk_bytes(DISK_SECTORS))]);
    let ticks = sym(&elf, "ticks");
    let tick_count = |m: &Machine| image_u32(m, ticks);
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
    // Polling would spend a whole tick interval (1 000 000 steps) per tick;
    // waking for one takes a few thousand with merlin's code.
    assert!(steps < 5 * 6000, "{steps} steps for 5 ticks");
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
        mask(&out),
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
    let (mut m, elf) = load_with(1, kernel::typed(EXIT), vec![Box::new(disk)]);
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
            mask(&printed(&m)),
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
        // The same block with the one entry changed: the firmware's reserved
        // RAM too.
        let reserved = m.cpu.bus.sysinfo.reserved();
        m.cpu.bus.sysinfo = Sysinfo::new(m.config(), devices);
        m.cpu.bus.sysinfo.set_reserved(reserved.0, reserved.1);
        let (r, t) = trace_disks(&mut m, &elf, BOOT_MAX_STEPS);
        assert_eq!(
            r,
            RunResult::Exited(0),
            "irq {irq}:
{}",
            printed(&m)
        );
        assert_eq!(mask(&printed(&m)), halted_output(), "irq {irq}");
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
        mask(&printed(&m)),
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

/// A bootable disk: the root image's files (kernel/fs, /dev/console, the
/// programs in /bin, `xtask::fs::build_with`) in the filesystem, the
/// bootloader in the boot block and the disk kernel after the filesystem,
/// as `camelot-fs mkfs --boot` makes one.
fn bootable_image(blocks: u32) -> (Vec<u8>, camelot_fs::boot::BootInfo) {
    let root = repo_root();
    let kernel = disk_kernel_under_test();
    let opts = xtask::fs::bootable_layout(blocks, &kernel).unwrap();
    // A reference kernel checks that /bin holds the programs it embeds: its
    // own, built from clang's assembly.
    let asm = UNDER_TEST_USER_ASM.with(|a| a.borrow().clone());
    let cc = match &asm {
        Some(dir) => CCompiler::Clang(dir),
        None => CCompiler::Merlin,
    };
    let mut image = xtask::fs::build_with(&root, &opts, cc).unwrap();
    let boot = xtask::boot::bootloader(&root).unwrap();
    let info = camelot_fs::boot::install(&mut image, &boot, &kernel).unwrap();
    (image, info)
}

#[test]
fn the_kernel_boots_from_disk() {
    let root = repo_root();
    let (image, info) = bootable_image(4096);
    for harts in [1, 4] {
        let mut m = Machine::new(MachineConfig {
            harts,
            paging: mode().paging,
            bios: grail::Bios::Default,
            // The firmware's 2 MiB, the kernel's own, and room for programs.
            ram_size: 6 << 20,
            disks: vec![DiskConfig::memory(image.clone())],
            ..MachineConfig::default()
        })
        .unwrap();
        m.boot_from_disk().unwrap();
        m.cpu.bus.uart.capture = true;
        m.set_uart_input(kernel::typed(b"hello\nls /bin\nexit\n"));
        let r = m.run(Some(2 * BOOT_MAX_STEPS), |_| {});
        let out = printed(&m);
        // The bootloader found the kernel, loaded its segments at their
        // physical addresses and started it at the entry's: the kernel
        // turned paging on and ran its self-tests, the disk's too.
        assert_eq!(r, RunResult::Exited(0), "{harts} harts:\n{out}");
        assert!(
            out.starts_with(&format!(
                "boot: kernel at sector 0x{:08x}\n{}Camelot v0.1.0 booting...\n",
                info.kernel_sector,
                xtask::kernel::BANNER
            )),
            "{out}"
        );
        assert!(
            out.contains(&format!("\npaging: on ({})\n", mode().name)),
            "{out}"
        );
        assert!(out.contains("\ndisk 0: 8192 sectors (4096 KiB), irq "), "{out}");
        // The disk driver's DMA (physical addresses of direct-map buffers),
        // the filesystem on it, and their self-tests.
        assert!(out.contains("\nblk self-test: 16/16 passed\n"), "{out}");
        assert!(out.contains("\nfs: disk 0 mounted: "), "{out}");
        // One more check than without /bin: the programs there ("N
        // programs, the kernel none" for the disk kernel, "N of the
        // kernel's N programs" for a reference build, which embeds them):
        // every program on the disk, as and ld too, which the kernel never
        // embeds.
        let user = xtask::userprogs::user_dir(&root);
        let holds = if UNDER_TEST_USER_ASM.with(|a| a.borrow().is_some()) {
            let n = xtask::userprogs::embedded_names(&user).unwrap().len();
            format!("\nfs: /bin holds {n} of the kernel's {n} programs\n")
        } else {
            let n = xtask::userprogs::program_names(&user).unwrap().len();
            format!("\nfs: /bin holds {n} programs, the kernel none\n")
        };
        assert!(out.contains(&holds), "{out}");
        assert!(out.contains("\nfs self-test: 12/12 passed\n"), "{out}");
        assert!(!out.contains("FAILED"), "{out}");
        // Then the shell from /bin/sh on the disk, and the programs it
        // runs from there: 64-bit ELF files, read through the filesystem.
        assert!(
            out.contains(&format!(
                "{SHELL_BANNER}{PROMPT}hello\nhello from user mode\n{PROMPT}ls /bin\n"
            )),
            "{out}"
        );
        assert!(out.contains(" sh\n") && out.contains(" systest\n"), "{out}");
        assert!(out.ends_with(&format!("{PROMPT}exit\n")), "{out}");
        if harts > 1 {
            assert!(out.contains(&format!("\nsmp: {harts} harts running\n")), "{out}");
        }
        // The bootloader ran in S-mode and wrote nothing to the ROM: its
        // entry doubleword is still the hand-off to the firmware, which
        // started the other harts for the kernel (HSM hart_start).
        let lo = u64::from(m.cpu.bus.peek(grail::boot::BOOT_ENTRY_ADDR).unwrap());
        let hi = u64::from(m.cpu.bus.peek(grail::boot::BOOT_ENTRY_ADDR + 4).unwrap());
        assert_eq!(lo | hi << 32, grail::boot::FW_HANDOFF_PC);
    }
}

// ---- The system call document ----

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
    assert_eq!(calls.len(), 47, "{calls:?}");
    let sys = |table: &[(u32, String)]| -> Vec<(u32, String)> {
        table
            .iter()
            .map(|(n, name)| (*n, format!("SYS_{name}")))
            .collect()
    };
    assert_eq!(defines(&header, "SYS_"), sys(&calls), "syscall.h");
    assert_eq!(defines(&usys, "SYS_"), sys(&calls), "usys.s");
    // A call's declaration in user.h: `type name(...);`, or `type *name(...);`
    // for one that returns a pointer (sbrk).
    let declares = |l: &str, name: &str| {
        (l.contains(&format!(" {name}(")) || l.contains(&format!(" *{name}("))) && l.ends_with(");")
    };
    for (_, name) in &calls {
        // A stub in usys.s that uses the number, and a declaration in user.h.
        assert!(
            usys.contains(&format!(
                "    .globl {name}\n{name}:\n    li    a7, SYS_{name}\n    ecall\n"
            )),
            "usys.s: no stub for {name}"
        );
        assert!(
            user_h.lines().any(|l| declares(l, name)),
            "user.h: no declaration of {name}"
        );
    }

    let errors = doc_table(&doc, "## Error numbers");
    assert_eq!(errors.len(), 45, "{errors:?}");
    assert_eq!(defines(&header, "E"), errors, "syscall.h");
    assert_eq!(defines(&user_h, "E"), errors, "user.h");

    // Each call's arguments are its user.h declaration's parameters, and
    // what it may fail with are error numbers.
    for (_, name, cells) in doc_rows(&doc, "## Calls") {
        let [args, returns] = cells[..] else {
            panic!("{name}: {cells:?}");
        };
        let decl = user_h.lines().find(|l| declares(l, &name)).unwrap();
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
        let infallible = [
            "exit",
            "getpid",
            "yield",
            "sleep",
            "uptime",
            "thread_exit",
            "gettid",
        ];
        assert_eq!(
            listed == 0,
            infallible.contains(&name.as_str()),
            "{name}: {returns}"
        );
    }
    // The structures' sizes (## Structures): what user.h and the kernel's
    // headers make of them, compiled by merlin (LP64), each size a
    // _Static_assert.
    let at = doc.find("\n## Structures\n").expect("no ## Structures");
    let section = &doc[at + 1..];
    let section = &section[..section[3..].find("\n## ").unwrap() + 3];
    let sizes: Vec<(&str, u32)> = section
        .lines()
        .filter_map(|line| {
            let cells: Vec<&str> = line.split('|').map(str::trim).collect();
            let name = cells.get(1)?.strip_prefix('`')?.strip_suffix('`')?;
            Some((name, cells.get(2)?.parse().ok()?))
        })
        .collect();
    assert_eq!(sizes.len(), 9, "{sizes:?}");
    for (dir, include, headers) in [
        ("kernel/user", None, "#include \"user.h\"\n"),
        (
            "kernel",
            Some("include"),
            "#include \"syscall.h\"\n#include \"fs.h\"\n",
        ),
    ] {
        let mut src = headers.to_string();
        for (name, size) in &sizes {
            src.push_str(&format!(
                "_Static_assert(sizeof({name}) == {size}, \"{name} is {size} bytes\");\n"
            ));
        }
        let opts = merlin::Options {
            include_dirs: include.into_iter().map(PathBuf::from).collect(),
            ..merlin::Options::default()
        };
        let base = repo_root().join(dir);
        let load = |p: &std::path::Path| std::fs::read_to_string(base.join(p)).ok();
        if let Err(errors) = merlin::compile_with("sizes.c", &src, &opts, &load) {
            let errors: Vec<String> = errors.iter().map(|e| e.render("sizes.c")).collect();
            panic!("{dir}: {}", errors.join("\n"));
        }
    }

    // The auxiliary vector's types, in the order the kernel writes them
    // (proc.c's push_start; tests/common's start::check checks the vector).
    let mut auxv = doc_table(&doc, "## Starting a program");
    assert_eq!(auxv.len(), 17, "{auxv:?}");
    assert_eq!(auxv.last().map(|a| a.0), Some(0), "AT_NULL last");
    auxv.sort();
    for (text, what) in [(&header, "syscall.h"), (&user_h, "user.h")] {
        let mut defined = defines(text, "AT_");
        defined.sort();
        assert_eq!(defined, auxv, "{what}");
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
    let socktest = read("kernel/user/socktest.c");
    for (header, name, copies) in [
        (
            "kernel/include/file.h",
            "NOFILE",
            vec![&filetest, &systest, &socktest],
        ),
        ("kernel/include/file.h", "CONSOLE", vec![&filetest, &systest]),
        ("kernel/include/pipe.h", "PIPESIZE", vec![&systest]),
        ("kernel/include/proc.h", "NPROC", vec![&systest, &socktest]),
        ("kernel/include/net.h", "TCP_SOCKETS", vec![&socktest]),
        ("kernel/include/net.h", "UDP_SOCKETS", vec![&socktest]),
        ("kernel/include/net.h", "UDP_QUEUE_MAX", vec![&socktest]),
        ("kernel/include/net.h", "TCP_EPHEMERAL_FIRST", vec![&socktest]),
        // The states procs reports.
        ("kernel/include/proc.h", "PROC_RUNNABLE", vec![&user_h]),
        ("kernel/include/proc.h", "PROC_RUNNING", vec![&user_h]),
        ("kernel/include/proc.h", "PROC_ZOMBIE", vec![&user_h]),
        ("kernel/include/proc.h", "PROC_SLEEPING", vec![&user_h]),
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

// ---- The clang reference build (ignored: needs LLVM) ----

fn llvm() -> Option<(PathBuf, PathBuf)> {
    let clang = tools::find("clang")?;
    let lld = tools::find_beside("ld.lld", &clang)?;
    Some((clang, lld))
}

/// The reference build (`cargo xtask kernel --cc clang`) into a scratch
/// directory, with `assembler` for the `.S` files. Returns the kernel and
/// the directory, which also holds the objects (`obj/`) and clang's
/// assembly of the user programs (`user/`).
fn reference_build(assembler: Assembler, name: &str) -> (Vec<u8>, PathBuf) {
    let (clang, lld) = llvm().expect("clang and ld.lld (the official LLVM install, or on PATH)");
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("kernel-tests")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let out = kernel::build_reference(
        &repo_root(),
        &clang,
        &lld,
        assembler,
        &dir,
        &dir.join("kernel.elf"),
        false,
    )
    .unwrap();
    (std::fs::read(out).unwrap(), dir)
}

#[test]
#[ignore = "needs clang and ld.lld"]
fn the_reference_builds_pass_the_same_tests() {
    // clang -O2 and ld.lld, with clang's assembler and then with merlin-as:
    // the tests of the boot, the address space, the panics and the disk
    // boot, on this thread's kernel under test.
    for (assembler, name) in [(Assembler::Clang, "clang"), (Assembler::Merlin, "merlin-as")] {
        let (elf, dir) = reference_build(assembler, name);
        assert_ne!(elf, committed_elf(), "{name}: the reference build is merlin's");
        UNDER_TEST_USER_ASM.with(|a| *a.borrow_mut() = Some(dir.join("user")));
        UNDER_TEST.with(|u| *u.borrow_mut() = Some(elf));
        MODE.set(SV48);
        the_kernel_falls_back_to_sv39();
        the_kernel_is_linked_high_and_loaded_low();
        the_kernels_have_no_m_mode_code();
        the_kernel_boots_through_its_self_tests();
        the_kernel_page_table_maps_the_kernel_half_only();
        the_page_table_shares_the_fixmap_with_the_early_one();
        the_kernel_runs_on_up_to_eight_harts();
        the_kernel_takes_ram_from_sysinfo();
        the_kernel_configures_its_network_with_dhcp();
        kernel_stacks_keep_a_margin();
        illegal_instruction_panics_with_a_64_bit_register_dump();
        unmapped_load_panics();
        misuses_panic_on_the_reference_build();
        a_kernel_stack_overflow_hits_the_guard_page_and_is_reported();
        the_kernel_boots_from_disk();
    }
}

/// The misuse panics' messages, on a build whose addresses differ.
fn misuses_panic_on_the_reference_build() {
    for (fault, want) in [
        (BOOT_FAULT_REMAP, "panic: map_pages: remap\n"),
        (BOOT_FAULT_W_ONLY, "panic: map_pages: bad permissions\n"),
        (BOOT_FAULT_MEGAPAGE, "panic: walk: unexpected superpage\n"),
        (BOOT_FAULT_KFREE_UNALIGNED, "panic: kfree\n"),
        (
            BOOT_FAULT_DOUBLE_ACQUIRE,
            "panic: acquire: lock \"boot_fault\" already held by hart 0\n",
        ),
        (BOOT_FAULT_POP_UNMATCHED, "panic: pop_off: no push_off to match\n"),
    ] {
        let (_, _, r, out) = run_with_fault(fault);
        assert_eq!(r, RunResult::Exited(PANIC_EXIT), "fault {fault}:\n{out}");
        assert!(out.ends_with(want), "fault {fault}: want {want:?}, got:\n{out}");
    }
}

/// `out` with what may differ between two builds of the same kernel masked:
/// `xcal.dbg`'s pc, the kernel stack use, the tick counts the `ticks`
/// program prints (slower code sees more ticks go by), every hexadecimal
/// number (code and stack addresses in a killed program's report) and the
/// free blocks the mounted disk reports.
fn mask_build(out: &str) -> String {
    let out = mask(out);
    let mut masked = String::new();
    let mut rest = out.as_str();
    while let Some(at) = rest.find("0x") {
        masked.push_str(&rest[..at + 2]);
        let after = &rest[at + 2..];
        let digits = after.bytes().take_while(u8::is_ascii_hexdigit).count();
        masked.push_str(if digits > 0 { "?" } else { "" });
        rest = &after[digits..];
    }
    masked.push_str(rest);
    let mut lines = String::new();
    for line in masked.split_inclusive('\n') {
        if line.starts_with("ticks: ") {
            lines.push_str("ticks: N\n");
        } else if let (true, Some(at)) = (line.starts_with("fs: disk "), line.find(" blocks (")) {
            // The free blocks: clang's programs in /bin are smaller.
            let rest = &line[at + " blocks (".len()..];
            let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
            lines.push_str(&line[..at]);
            lines.push_str(" blocks (N");
            lines.push_str(&rest[digits..]);
        } else {
            lines.push_str(line);
        }
    }
    lines
}

/// Boots `elf` on the tests' machine with `disks`, `input` typed on the
/// UART, for at most `max_steps`. Returns how it ended, what it printed and
/// the steps it ran.
fn run_build(elf: &[u8], disks: Vec<DiskConfig>, input: &[u8], max_steps: u64) -> (RunResult, String, u64) {
    let mut m = Machine::new(MachineConfig {
        disks,
        ram_size: 16 << 20,
        ..config()
    })
    .unwrap();
    m.cpu.bus.uart.capture = true;
    m.load_elf(elf).unwrap();
    m.set_uart_input(kernel::typed(input));
    let mut steps = 0u64;
    let r = m.run(Some(max_steps), |_| steps += 1);
    (r, printed(&m), steps)
}

/// The sessions the two builds must agree on: booting and leaving the
/// shell; the built-in programs, system calls and faults without a disk;
/// and the root filesystem, its programs and files. Nothing runs in the
/// background: how programs interleave depends on where the ticks land,
/// which differs between the builds. The disk's `/bin` holds the programs
/// compiled by `cc`, as the kernel embeds them, on a filesystem the size of
/// the root image's (`cargo xtask fs`): filetest writes a file of 300 KiB,
/// which merlin's bigger programs leave no room for on mkfs's default 2048
/// blocks.
fn sessions(cc: CCompiler) -> Vec<(&'static str, Vec<DiskConfig>, &'static [u8])> {
    let root = repo_root();
    let kernel = std::fs::read(root.join(xtask::fs::KERNEL)).unwrap();
    let opts = xtask::fs::bootable_layout(xtask::fs::IMAGE_SIZE, &kernel).unwrap();
    let image = xtask::fs::build_with(&root, &opts, cc).unwrap();
    vec![
        ("boot", Vec::new(), EXIT),
        (
            "programs",
            Vec::new(),
            b"help\nhello\necho a  b   c\nticks\nsystest\nlibtest\nbadptr\nbadload\nbadstore\nregs\nping\npong\nexit 3\n",
        ),
        (
            "filesystem",
            vec![DiskConfig::memory(image)],
            b"ls /\ncat /README | wc\necho hi > f\ncat f\nls /bin | wc\nmkdir d\ncd d\nfiletest\n\
              cat /README | grep Camelot | wc -l\ncp /README r\nmv r s\ngrep -n the s\ncp s /\nkill 99\n\
              sleep 0.2\nexit\n",
        ),
    ]
}

/// Bootstrapping step 3 for the whole kernel (spec section 10): the
/// committed kernel, built entirely by merlin, merlin-as and merlin-ld,
/// behaves like the reference build, where clang -O2 compiles every C file
/// (the user programs' too) and ld.lld links. Also prints how much bigger
/// and slower merlin's unoptimized code is (spec section 10, "Kernel build").
#[test]
#[ignore = "needs clang and ld.lld"]
fn clang_reference_build_behaves_like_merlins() {
    let (clang_elf, dir) = reference_build(Assembler::Clang, "clang");
    let clang_user = dir.join("user");
    let merlin_elf = committed_elf();
    for (name, elf) in [("merlin", &merlin_elf), ("clang", &clang_elf)] {
        let e = elf::Elf::parse(elf).unwrap();
        let size = |s: &str| e.sections.iter().find(|x| x.name == s).map_or(0, |x| x.size);
        eprintln!(
            "{name}: .text {} bytes, .rodata {} bytes, .data {} bytes, .bss {} bytes",
            size(".text"),
            size(".rodata"),
            size(".data"),
            size(".bss")
        );
    }
    const MAX_STEPS: u64 = 1_000_000_000;
    let clang_sessions = sessions(CCompiler::Clang(&clang_user));
    for ((name, disks, input), (_, clang_disks, _)) in
        sessions(CCompiler::Merlin).into_iter().zip(clang_sessions)
    {
        let (r_merlin, out_merlin, steps_merlin) = run_build(&merlin_elf, disks, input, MAX_STEPS);
        let (r_clang, out_clang, steps_clang) = run_build(&clang_elf, clang_disks, input, MAX_STEPS);
        eprintln!(
            "{name}: merlin {steps_merlin} steps, clang {steps_clang} steps ({:.2}x)",
            steps_merlin as f64 / steps_clang as f64
        );
        assert_eq!(r_merlin, r_clang, "{name}:\n{out_merlin}\n----\n{out_clang}");
        assert!(matches!(r_merlin, RunResult::Exited(_)), "{name}: {r_merlin:?}");
        assert_eq!(mask_build(&out_merlin), mask_build(&out_clang), "{name}");
    }
}

/// Bootstrapping steps 1 and 2 for the kernel: clang's C objects with the
/// `.S` files assembled by merlin-as and linked by ld.lld, and the
/// reference build's objects linked by merlin-ld (`kernel.lds.S` uses
/// `AT`, `ALIGN(n)` output section attributes, `NOLOAD` and `/DISCARD/`),
/// both boot like the reference build.
#[test]
#[ignore = "needs clang and ld.lld"]
fn merlin_as_and_merlin_ld_take_the_reference_builds_place() {
    let (elf, _) = reference_build(Assembler::Merlin, "merlin-as");
    assert_boots(&elf);

    let (lld_elf, dir) = reference_build(Assembler::Clang, "merlin-ld");
    let inputs: Vec<merlin_ld::Input> = kernel::sources(&kernel_dir())
        .unwrap()
        .iter()
        .map(|s| merlin_ld::Input {
            name: s.object_name(),
            data: std::fs::read(dir.join("obj").join(s.object_name())).unwrap(),
        })
        .collect();
    let opts = merlin_ld::Options {
        script: Some(merlin_ld::ScriptSource {
            name: kernel::SCRIPT.into(),
            text: kernel::linker_script(&kernel_dir()).unwrap(),
        }),
        entry: None,
    };
    // merlin-ld does not merge clang's string literals (spec section 10),
    // so it may warn and lay .rodata out bigger; it still runs the same.
    let linked = merlin_ld::link(&inputs, &opts).unwrap_or_else(|e| panic!("{e:?}"));
    assert_boots(&linked.elf);
    // The layout follows kernel.lds.S like ld.lld's: same sections, .text
    // where ld.lld put it, page-aligned .rodata and .data, NOBITS .bss and
    // .stack, nothing from /DISCARD/.
    let ours = elf::Elf::parse(&linked.elf).unwrap();
    let theirs = elf::Elf::parse(&lld_elf).unwrap();
    let alloc = |e: &elf::Elf| -> Vec<(String, u64, bool)> {
        e.sections
            .iter()
            .filter(|s| s.flags & 2 != 0)
            .map(|s| (s.name.clone(), s.addr, s.ty == 8))
            .collect()
    };
    let (a, b) = (alloc(&ours), alloc(&theirs));
    let names = |v: &[(String, u64, bool)]| v.iter().map(|x| (x.0.clone(), x.2)).collect::<Vec<_>>();
    assert_eq!(names(&a), names(&b));
    assert_eq!(a[0], b[0], ".text");
    for (name, addr, _) in &a {
        if name == ".rodata" || name == ".data" {
            assert_eq!(addr % 4096, 0, "{name}");
        }
    }
}
