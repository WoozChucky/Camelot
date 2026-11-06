//! Processes (#155): `kernel/kernel.elf` running 64-bit programs in the
//! lower half of Sv48 (or Sv39), loaded from ELF64 files, with the LP64
//! system call ABI: the shell as the first process, each process in its
//! own page table, preempted by timer ticks, its registers kept across
//! switches and system calls, its pages given back; the process, thread,
//! futex, file, pipe, socket and stack-growth checks of the user programs
//! (kernel/user); and what LP64 makes of the ABI:
//!
//! - a negative result arrives sign-extended in all 64 bits of a0
//!   (`negative_results_arrive_sign_extended`);
//! - a pointer into the kernel's half of the address space, or between the
//!   halves, is refused with -EFAULT (`pointers_into_the_kernel_half_are_refused`);
//! - the widths are LP64's: `lseek`'s 64-bit `off_t`, `stat`'s 24 bytes,
//!   `sbrk`'s pointers above 4 GiB (`the_calls_take_and_return_64_bit_values`);
//! - a 32-bit program is refused (`an_elf32_program_is_refused`);
//! - a program starts as Linux starts one, with 8-byte words, its
//!   environment's among them (`a_program_starts_as_linux_starts_one`,
//!   `the_shell_passes_its_environment_on`).

mod common;

use common::{
    command_output, image_pa, kpa, peek64, printed, proc_name, spare_pages, sym, LockWatch, CPU_PROC,
    KSTACK_PAGES, MAPTEST_CHECKS, PROC_PID, PROMPT,
};
use excalibur::csr::{IRQ_S_EXT, IRQ_S_TIMER, MSTATUS_SIE, MSTATUS_SPP};
use grail::cpu::Mode;
use grail::{DiskConfig, Machine, MachineConfig, NetConfig, RunResult};
use std::collections::HashMap;
use std::sync::mpsc::Receiver;
use xtask::kernel::{self, BOOT_MAX_STEPS};
use xtask::repo_root;

const SHELL_BANNER: &str = "Type help for the list of commands.\n";
const MIB: usize = 1 << 20;
/// Enough steps for a boot and the commands of a test.
const MAX_STEPS: u64 = 2 * BOOT_MAX_STEPS;
/// A boot plus a few programs: ding and dong run about two million
/// instructions each, regs three.
const RUN_MAX_STEPS: u64 = common::RUN_MAX_STEPS;

/// The interrupt bit of scause: bit 63.
const INTERRUPT_BIT: u64 = 1 << 63;
/// scause of an ecall from U-mode, and of the faults the tests make.
const ECALL_FROM_U: u64 = grail::trap::Exception::EcallU as u64;
const LOAD_PAGE_FAULT: u64 = grail::trap::Exception::LoadPageFault as u64;
const STORE_PAGE_FAULT: u64 = grail::trap::Exception::StorePageFault as u64;
const ILLEGAL_INSTRUCTION: u64 = grail::trap::Exception::IllegalInstr as u64;
/// The exit system call's number (`SYS_exit`, kernel/include/syscall.h).
const SYS_EXIT: u64 = 1;
/// Process slots (`NPROC`, kernel/include/proc.h).
const NPROC: usize = 8;
/// The pid of the shell's first child: the shell is pid 1, as Linux's init
/// (kernel/proc.c's INIT_PID), and the pids after it went to the kernel
/// processes of the boot's kernel-stack self-test, four rounds of NPROC
/// (kernel/vmtest.c).
const FIRST_PID: usize = 2 + 4 * NPROC;

const PAGE: u64 = 4096;
/// The top of user space, by paging mode (kernel/include/vm.h): the lower
/// half, 2^47 in Sv48, 2^38 in Sv39.
const USER_TOP: u64 = 1 << 47;
/// The stack's first page under it, and its lowest address: it grows down
/// to here at most, 8 MiB.
const USER_STACK: u64 = USER_TOP - PAGE;
const USER_STACK_LIMIT: u64 = USER_TOP - (8 << 20);
/// Where program segments and the heap must end: below the guard page
/// under the stack's limit.
const USER_LOAD_TOP: u64 = USER_STACK_LIMIT - PAGE;
/// Where user programs start (kernel/user/user.ld): crt0's `_start`.
const USER_TEXT: u64 = 0x1000;
/// `sizeof(struct trap_frame)` (kernel/include/trap.h), and offsets in it:
/// 8-byte slots.
const TRAP_FRAME_SIZE: u64 = 304;
const FRAME_SEPC: u64 = 32 * 8;
const FRAME_SCAUSE: u64 = 33 * 8;
const FRAME_SSTATUS: u64 = 35 * 8;
const FRAME_KERNEL_TP: u64 = 36 * 8;

// PTE bits.
const PTE_V: u64 = 1 << 0;
const PTE_R: u64 = 1 << 1;
const PTE_W: u64 = 1 << 2;
const PTE_X: u64 = 1 << 3;
const PTE_U: u64 = 1 << 4;
const PTE_G: u64 = 1 << 5;
const PTE_A: u64 = 1 << 6;
const PTE_D: u64 = 1 << 7;
/// The bits of a PTE the kernel chooses.
const PTE_KERNEL_BITS: u64 = PTE_V | PTE_R | PTE_W | PTE_X | PTE_U | PTE_G | PTE_A | PTE_D;

fn committed() -> Vec<u8> {
    common::committed_elf()
}

/// A machine for the kernel.
struct Setup {
    harts: usize,
    ram: usize,
    paging: grail::Paging,
    disks: Vec<DiskConfig>,
    nets: Vec<NetConfig>,
}

impl Default for Setup {
    fn default() -> Self {
        Setup {
            harts: 1,
            ram: 16 * MIB,
            paging: grail::Paging::Sv48,
            disks: Vec::new(),
            nets: Vec::new(),
        }
    }
}

impl Setup {
    fn config(self) -> MachineConfig {
        MachineConfig {
            harts: self.harts,
            ram_size: self.ram,
            bios: grail::Bios::Default,
            paging: self.paging,
            disks: self.disks,
            nets: self.nets,
            ..MachineConfig::default()
        }
    }
}

/// Boots the kernel on `setup`'s machine with `input` typed on the UART
/// (ending in `exit 0`), runs it until the shell exits, and returns what was
/// printed from the shell's banner on. The machine must stop with status 0:
/// no self-test failed, and the shell exited 0.
fn run(setup: Setup, input: &[u8]) -> String {
    let (mut m, _) = load(setup.config(), Some(kernel::typed(input)));
    let r = m.run(Some(MAX_STEPS), |_| {});
    let out = printed(&m);
    assert_eq!(r, RunResult::Exited(0), "output:\n{out}");
    let at = out
        .find(SHELL_BANNER)
        .unwrap_or_else(|| panic!("the shell never started:\n{out}"));
    out[at..].to_string()
}

/// [`run`] on the kernel tests' machine (8 MiB, `kernel::machine_config`).
fn run_processes(input: &[u8]) -> String {
    run(
        Setup {
            ram: kernel::RAM_SIZE,
            ..Setup::default()
        },
        input,
    )
}

/// Loads the kernel into a machine built from `config`, with the UART
/// reading `input` (nothing at all if `None`).
fn load(config: MachineConfig, input: Option<Receiver<u8>>) -> (Machine, grail::elf::Elf) {
    let file = committed();
    let elf = grail::elf::parse(&file).unwrap();
    let mut m = Machine::new(config).unwrap();
    m.cpu.bus.uart.capture = true;
    m.load_elf(&file).unwrap();
    if let Some(input) = input {
        m.set_uart_input(input);
    }
    (m, elf)
}

/// [`load`] on the kernel tests' machine.
fn load_committed(input: Option<Receiver<u8>>) -> (Machine, grail::elf::Elf) {
    load(common::machine_config(1), input)
}

/// The one line `program` prints at its end, "PROGRAM: N checks, 0 failed",
/// and nothing else; returns N.
fn all_checks_pass(out: &str, command: &str, program: &str) -> u32 {
    let lines = command_output(out, command);
    let last = lines.last().copied().unwrap_or("");
    let prefix = format!("{program}: ");
    assert!(
        last.starts_with(&prefix) && last.ends_with(" checks, 0 failed"),
        "{out}"
    );
    last[prefix.len()..last.find(" checks").unwrap()].parse().unwrap()
}

/// A disk with the root filesystem (`xtask::fs::build`: the programs in
/// /bin), `blocks` blocks, and `extra` files besides.
fn root_disk(blocks: u32, extra: &[(&str, &[u8])]) -> DiskConfig {
    let root = repo_root();
    let opts = camelot_fs::mkfs::Options {
        size: blocks,
        ..camelot_fs::mkfs::Options::default()
    };
    let image = if extra.is_empty() {
        xtask::fs::build(&root, &opts).unwrap()
    } else {
        // build's files, then the extra ones.
        let mut fs = camelot_fs::mkfs::Mkfs::new(&opts).unwrap();
        fs.add_host_dir(&root.join(xtask::fs::ROOT_DIR), "/").unwrap();
        fs.mkdir(xtask::fs::DEV).unwrap();
        fs.add_device(
            &format!("{}/console", xtask::fs::DEV),
            xtask::fs::CONSOLE_MAJOR,
            0,
        )
        .unwrap();
        fs.mkdir(xtask::fs::BIN).unwrap();
        let user = xtask::userprogs::user_dir(&root);
        for p in xtask::userprogs::build_all(&user).unwrap() {
            fs.add_file(&format!("{}/{}", xtask::fs::BIN, p.name), &p.elf)
                .unwrap();
        }
        for (path, bytes) in extra {
            fs.add_file(path, bytes).unwrap();
        }
        fs.finish()
    };
    DiskConfig::memory(image)
}

/// Register number of an ABI name.
fn x(name: &str) -> usize {
    usize::from(excalibur::reg::parse(name).unwrap_or_else(|| panic!("no register {name}")))
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
    panic!("no prompt {n}; output:\n{}", printed(m));
}

/// The name of the process on the current hart (`cpus[h].proc`, the first
/// field of `struct cpu`, kernel/include/spinlock.h), or `None` in the
/// scheduler.
fn current_name(cpu: &grail::cpu::Cpu, cpus: u64) -> Option<String> {
    let p = peek64(cpu, cpus + u64::from(cpu.id) * common::CPU_SIZE + CPU_PROC);
    (p != 0).then(|| proc_name(cpu, p))
}

/// Steps `m` until program `name` runs in U-mode.
fn step_to_user(m: &mut Machine, elf: &grail::elf::Elf, name: &str) {
    let cpus = sym(elf, "cpus");
    step_until(
        m,
        RUN_MAX_STEPS,
        |_| {},
        |m| m.cpu.mode == Mode::U && current_name(&m.cpu, cpus).as_deref() == Some(name),
    );
}

/// The embedded programs, by name: all but the disk-only ones.
fn program_names() -> Vec<String> {
    xtask::userprogs::embedded_names(&xtask::userprogs::user_dir(&repo_root())).unwrap()
}

/// A fresh build of user program `name`, as the kernel embeds it.
fn user_program(name: &str) -> xtask::userprogs::Program {
    xtask::userprogs::build_named(&xtask::userprogs::user_dir(&repo_root()), name).unwrap()
}

/// The value of symbol `name` in user program `elf`.
fn user_sym(elf: &[u8], name: &str) -> u64 {
    elf::Elf::parse(elf)
        .unwrap()
        .symbols()
        .unwrap()
        .into_iter()
        .find(|s| s.name == name)
        .unwrap_or_else(|| panic!("no symbol {name}"))
        .value
}

/// The user pages a program's ELF image asks for, as (va, PTE bits), then its
/// stack page: what the process's page table must map below USER_TOP. Each
/// has A, and D if it is writable, as map_pages() makes a user leaf (vm.h's
/// PTE_USER_AD, Linux's user protections).
fn expected_user_pages(elf: &[u8]) -> Vec<(u64, u64)> {
    let parsed = elf::Elf::parse(elf).unwrap();
    let mut pages = Vec::new();
    for seg in parsed
        .segments
        .iter()
        .filter(|s| s.ty == elf::PT_LOAD && s.memsz > 0)
    {
        let mut perm = PTE_V | PTE_U | PTE_R | PTE_A;
        if seg.flags & elf::PF_W != 0 {
            perm |= PTE_W | PTE_D;
        }
        if seg.flags & elf::PF_X != 0 {
            perm |= PTE_X;
        }
        let first = seg.vaddr & !(PAGE - 1);
        let mut va = first;
        while va < seg.vaddr + seg.memsz {
            pages.push((va, perm));
            va += PAGE;
        }
    }
    pages.push((USER_STACK, PTE_V | PTE_U | PTE_R | PTE_W | PTE_A | PTE_D));
    pages
}

/// The u64 at physical address `pa`.
fn read64_pa(m: &Machine, pa: u64) -> u64 {
    u64::from(m.cpu.bus.peek(pa).unwrap()) | u64::from(m.cpu.bus.peek(pa + 4).unwrap()) << 32
}

/// The Sv48 page table whose root is at physical address `root`: every
/// leaf of the half `upper` says, as (va, pa, pte), and the number of
/// tables under the root that half has.
fn leaves(m: &Machine, root: u64, upper: bool) -> (Vec<(u64, u64, u64)>, usize) {
    fn walk(
        m: &Machine,
        table: u64,
        level: u32,
        base: u64,
        out: &mut Vec<(u64, u64, u64)>,
        tables: &mut usize,
    ) {
        for i in 0..512u64 {
            let pte = read64_pa(m, table + 8 * i);
            if pte & PTE_V == 0 {
                continue;
            }
            let va = base | i << (12 + 9 * level);
            let pa = (pte >> 10 & 0xfff_ffff_ffff) << 12;
            if pte & (PTE_R | PTE_W | PTE_X) != 0 {
                out.push((va, pa, pte));
            } else {
                *tables += 1;
                walk(m, pa, level - 1, va, out, tables);
            }
        }
    }
    let (mut out, mut tables) = (Vec::new(), 0);
    let entries = if upper { 256..512 } else { 0..256 };
    for i in entries {
        let pte = read64_pa(m, root + 8 * i);
        if pte & PTE_V == 0 {
            continue;
        }
        let mut va = i << 39;
        if upper {
            va |= !0u64 << 48;
        }
        tables += 1;
        walk(
            m,
            (pte >> 10 & 0xfff_ffff_ffff) << 12,
            2,
            va,
            &mut out,
            &mut tables,
        );
    }
    (out, tables)
}

/// The user pages `satp` maps, as (va, the PTE bits the kernel chooses).
fn user_pages(m: &Machine, satp: u64) -> Vec<(u64, u64)> {
    let (leaves, _) = leaves(m, (satp & 0xfff_ffff_ffff) << 12, false);
    leaves
        .iter()
        .map(|&(va, _, pte)| (va, pte & PTE_KERNEL_BITS))
        .collect()
}

/// The satp value of the kernel page table (`kernel_satp`).
fn kernel_satp(m: &Machine, elf: &grail::elf::Elf) -> u64 {
    let pa = image_pa(sym(elf, "kernel_satp"));
    read64_pa(m, pa)
}

// ---- The first process ----

#[test]
fn booting_drops_into_the_user_shell() {
    // Issue #21's first criterion: the kernel boots into the shell, a user
    // program, which runs hello and exits; the kernel then stops the
    // machine with the shell's exit status.
    let (r, out) = kernel::run_on_grail(&committed(), b"hello\nexit\n", RUN_MAX_STEPS).unwrap();
    assert!(
        out.ends_with(&format!(
            "{SHELL_BANNER}{PROMPT}hello\nhello from user mode\n{PROMPT}exit\n"
        )),
        "{out}"
    );
    assert_eq!(r, RunResult::Exited(0));
}

#[test]
fn a_program_starts_as_linux_starts_one() {
    // The first instruction in U-mode is the shell's entry point, crt0's
    // _start, as pid 1, with Linux's start (docs/camelot-syscalls.md,
    // "Starting a program"): every register zero but sp, which points at
    // argc, a long, then argv { "sh", NULL }, envp (Linux's environment for
    // init, HOME=/ and TERM=linux) and the auxiliary vector, 8 bytes each,
    // under USER_TOP, which depends on the paging mode; the strings, argv's
    // then envp's, then the path, "/bin/sh", above. Interrupts on, SUM off.
    // The programs it runs get the same: systest checks its own start, its
    // environment's strings among them, and its children's after a spawn
    // and an exec (the_system_calls_do_what_the_document_says).
    let sh = user_program("sh");
    for (paging, top) in [
        (grail::Paging::Sv48, 1u64 << 47),
        (grail::Paging::Sv39, 1u64 << 38),
    ] {
        let (mut m, elf) = load(
            Setup {
                paging,
                ..Setup::default()
            }
            .config(),
            None,
        );
        step_until(&mut m, MAX_STEPS, |_| {}, |m| m.cpu.mode == Mode::U);
        assert_eq!(m.cpu.pc, USER_TEXT, "{paging:?}");
        let cpus = sym(&elf, "cpus");
        assert_eq!(current_name(&m.cpu, cpus).as_deref(), Some("sh"));
        let p = peek64(&m.cpu, cpus + CPU_PROC);
        assert_eq!(common::peek(&m.cpu, p + PROC_PID), 1);
        common::start::check(&m, top, &sh.elf, "/bin/sh", &["sh"], common::start::INIT_ENV);
        assert_ne!(m.cpu.csr.mstatus & MSTATUS_SIE, 0);
        assert_eq!(m.cpu.csr.mstatus & excalibur::csr::MSTATUS_SUM, 0);
    }
}

#[test]
fn the_shell_passes_its_environment_on() {
    // Issue #216: the shell starts with the kernel's environment for init,
    // keeps variables (assignments, export, unset) and gives every program
    // the exported ones, through spawn and exec (tests/common's environment
    // session): envp's pointers 8 bytes each.
    let out = run(Setup::default(), common::environment::INPUT);
    common::environment::check(&out, command_output);
}

#[test]
fn the_shell_runs_64_bit_programs() {
    // The shell is the first process: it runs programs by name, with their
    // arguments, in pipelines, and itself.
    let out = run(
        Setup::default(),
        b"hello\necho  a   b\tc\necho one two three | wc\nsh\necho inner\nexit 4\nps\nexit 0\n",
    );
    assert_eq!(command_output(&out, "hello"), ["hello from user mode"]);
    assert_eq!(command_output(&out, "echo  a   bc"), ["a bc"]);
    assert_eq!(command_output(&out, "echo one two three | wc"), ["1 3 14"]);
    assert_eq!(command_output(&out, "echo inner"), ["inner"]);
    assert_eq!(command_output(&out, "exit 4"), ["sh: exit 4"]);
    // ps: procs' 40-byte entries, with 64-bit sizes.
    let ps = command_output(&out, "ps");
    assert_eq!(ps[0], "  PID  PPID STATE      SIZE NAME", "{out}");
    assert!(ps[1].ends_with(" sh") && ps[2].ends_with(" ps"), "{out}");
}

#[test]
fn a_process_page_table_maps_its_program_and_the_kernel() {
    let (mut m, elf) = load_committed(Some(kernel::typed(b"ding\nexit\n")));
    step_to_user(&mut m, &elf, "ding");
    let c = &m.cpu;
    // Traps go to user_vec, which finds the process's trap frame in
    // sscratch: at the top of its kernel stack, in the vmalloc area.
    assert_eq!(c.csr.stvec, sym(&elf, "user_vec"));
    let frame = c.csr.sscratch;
    assert_eq!(
        frame % (16 * 1024),
        16 * 1024 - TRAP_FRAME_SIZE,
        "frame {frame:#x}"
    );
    assert!(
        (0xffff_8f80_0000_0000..0xffff_af80_0000_0000).contains(&frame),
        "frame {frame:#x}"
    );

    let satp = c.csr.satp;
    let ksatp = kernel_satp(&m, &elf);
    assert_ne!(satp, ksatp);
    assert_eq!(satp >> 60, ksatp >> 60, "the same mode");
    // Root entries 256-511 are the kernel's: the same tables below them.
    let (root, kroot) = ((satp & 0xfff_ffff_ffff) << 12, (ksatp & 0xfff_ffff_ffff) << 12);
    for i in 256..512 {
        assert_eq!(
            read64_pa(&m, root + 8 * i),
            read64_pa(&m, kroot + 8 * i),
            "root[{i}]"
        );
    }
    // The kernel maps nothing in the lower half.
    for i in 0..256 {
        assert_eq!(read64_pa(&m, kroot + 8 * i) & PTE_V, 0, "kernel root[{i}]");
    }
    // User space: ding's segments, each page with U and the segment's
    // permissions, and the stack's first page (R+W); nothing at page 0, in
    // the guard page under the stack's limit or, with an empty heap and a
    // stack that has not grown, anywhere else. The upper half is the
    // kernel's, none of it with U.
    let ding = user_program("ding");
    let want = expected_user_pages(&ding.elf);
    assert_eq!(want[0], (USER_TEXT, PTE_V | PTE_U | PTE_R | PTE_X | PTE_A));
    assert!(want.iter().all(|&(va, _)| va >= USER_TEXT && va != USER_LOAD_TOP));
    assert_eq!(user_pages(&m, satp), want);
    let (kleaves, _) = leaves(&m, kroot, true);
    assert!(kleaves.iter().all(|(_, _, pte)| pte & PTE_U == 0));
    // User pages are fresh pages in free RAM, after the kernel image.
    let kernel_end = image_pa(sym(&elf, "__kernel_end"));
    let (uleaves, utables) = leaves(&m, root, false);
    for (va, pa, _) in &uleaves {
        assert!(*pa >= kernel_end, "{va:#x} at {pa:#x}");
    }
    // Two branches of tables under its own root, each three levels down
    // in Sv48: its segments' at the bottom of user space, its stack's at
    // the top.
    assert_eq!(utables, 2 * 3);
}

#[test]
fn ding_and_dong_take_turns_on_timer_ticks() {
    // Issue #20's first criterion, watched from the hart: two processes in
    // U-mode, each in its own page table, and the CPU only moves between
    // them when a timer tick (or an exit) traps from U-mode. The shell runs
    // too: it starts ding in the background and waits for dong.
    let (mut m, elf) = load_committed(Some(kernel::typed(b"ding & dong\nwait\nexit\n")));
    let (user_vec, cpus) = (sym(&elf, "user_vec"), sym(&elf, "cpus"));
    // The process of each stretch in U-mode, and the last trap from it:
    // (scause, whether it was the exit call).
    let mut stretches: Vec<(String, u64)> = Vec::new();
    let mut last_trap: Option<(u64, bool)> = None;
    let mut timer_switches = 0;
    let mut user_steps = 0u64;
    let r = m.run(Some(RUN_MAX_STEPS), |cpu| {
        if cpu.mode == Mode::U {
            let name = current_name(cpu, cpus).unwrap();
            if name != "sh" {
                user_steps += 1;
            }
            assert!((USER_TEXT..USER_TOP).contains(&cpu.pc), "U-mode at {:#x}", cpu.pc);
            if stretches.last().map(|s| &s.0) != Some(&name) {
                if let Some((prev, _)) = stretches.last() {
                    if prev != "sh" {
                        // ding and dong never wait for anything: they leave
                        // the CPU when a tick preempts them, or for good.
                        match last_trap {
                            Some((cause, false)) => {
                                assert_eq!(
                                    cause,
                                    INTERRUPT_BIT | u64::from(IRQ_S_TIMER),
                                    "{prev} left after {cause:#x}"
                                );
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
            let exit = cpu.csr.scause == ECALL_FROM_U && cpu.x[x("a7")] == SYS_EXIT;
            last_trap = Some((cpu.csr.scause, exit));
        }
    });
    assert_eq!(r, RunResult::Exited(0));
    let mut satps: Vec<(String, u64)> = stretches.clone();
    satps.sort();
    satps.dedup();
    let names: Vec<&str> = satps.iter().map(|s| s.0.as_str()).collect();
    assert_eq!(names, ["ding", "dong", "sh"], "{stretches:?}");
    let mut distinct: Vec<u64> = satps.iter().map(|s| s.1).collect();
    distinct.dedup();
    assert_eq!(distinct.len(), 3, "each its own page table: {satps:x?}");
    assert!(!distinct.contains(&kernel_satp(&m, &elf)));
    assert!(timer_switches >= 3, "{timer_switches} switches: {stretches:?}");
    // Each prints 4 lines with half a tick (500 000 instructions) between.
    assert!(user_steps > 3_000_000, "{user_steps}");
    // The shell: ding in the background (its `[FIRST_PID]` before or after
    // ding's first line, depending on where a tick lands in the shell's
    // fork), dong in the foreground.
    let out = printed(&m);
    let started = command_output(&out, "ding & dong");
    let job = format!("[{FIRST_PID}]");
    assert!(
        started.starts_with(&[job.as_str(), "ding 1"]) || started.starts_with(&["ding 1", job.as_str()]),
        "{out}"
    );
    assert!(
        out.contains(&format!("{PROMPT}wait\n{job} ding: exit 0\n{PROMPT}exit\n")),
        "{out}"
    );
}

#[test]
fn every_return_to_user_mode_sets_the_frames_hart_id() {
    // user_vec loads tp, the hart's id in the kernel, from the frame's
    // kernel_tp, so user_trap_return must write it on every way back to
    // U-mode. On hart 0 a frame that was never written already reads 0, so
    // the test spoils it while the process is in the kernel and checks that
    // it reads 0 again once the process is back in U-mode.
    let (mut m, elf) = load_committed(Some(kernel::typed(b"ding\nexit\n")));
    step_to_user(&mut m, &elf, "ding");
    let user_trap = sym(&elf, "user_trap");
    step_until(&mut m, RUN_MAX_STEPS, |_| {}, |m| m.cpu.pc == user_trap);
    let frame = kpa(&m.cpu, m.cpu.x[x("a0")]);
    assert_eq!(read64_pa(&m, frame + FRAME_KERNEL_TP), 0);
    m.cpu
        .bus
        .load(frame + FRAME_KERNEL_TP, &0xdead_beef_dead_beefu64.to_le_bytes())
        .unwrap();
    step_to_user(&mut m, &elf, "ding");
    assert_eq!(read64_pa(&m, frame + FRAME_KERNEL_TP), 0);
    assert_eq!(m.run(Some(RUN_MAX_STEPS), |_| {}), RunResult::Exited(0));
}

#[test]
fn programs_get_zeroed_memory_and_writable_data() {
    // data checks that its stack page below sp and its .bss (two pages, the
    // second with no file bytes) read zero, that its .data word holds its
    // initial value, and that .data and .bss take stores; then it dirties
    // them. The pages it gives back are reused by the second run (the
    // allocator doesn't clear freed pages), so that one passes only if the
    // loader and the stack get zeroed pages.
    let (mut m, elf) = load_committed(Some(kernel::typed(b"data\ndata\nexit\n")));
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
    // .rodata is mapped without W: badstore's store to it faults; its PTE
    // has A set up front (user leaves have A, and D if writable: vm.h).
    let out = run_processes(b"badstore\nexit 0\n");
    let badstore = user_program("badstore");
    let (main, msg) = (user_sym(&badstore.elf, "main"), user_sym(&badstore.elf, "msg"));
    let lines = command_output(&out, "badstore");
    assert_eq!(lines.len(), 4, "{out}");
    assert_eq!(
        lines[0],
        format!("pid {FIRST_PID} (badstore) killed: store page fault")
    );
    // The store follows `la` (two instructions).
    assert_eq!(
        lines[1],
        format!(
            "  scause {STORE_PAGE_FAULT:#018x}  sepc {:#018x}  stval {msg:#018x}",
            main + 8
        )
    );
    assert!(lines[2].ends_with(" -A-U--RV"), "{out}");
    assert_eq!(lines[3], "badstore: exit -1");
}

#[test]
fn a_tick_in_user_mode_saves_every_register_and_switches_back_later() {
    // regs fills every register and spins; ding runs beside it. Catch a tick
    // that interrupts regs, check user_vec saved the registers in its trap
    // frame, then follow the switch to ding and back: regs must resume with
    // every register and its pc as they were, all 64 bits of each.
    let (mut m, elf) = load_committed(Some(kernel::typed(b"regs & ding\nwait\nexit\n")));
    let (user_vec, user_trap) = (sym(&elf, "user_vec"), sym(&elf, "user_trap"));
    // regs, in its spin loop: all its registers are set.
    step_to_user(&mut m, &elf, "regs");
    step_until(
        &mut m,
        RUN_MAX_STEPS,
        |_| {},
        |m| m.cpu.mode == Mode::U && m.cpu.x[x("t5")] == 0x9e9e_9e9e_1e1e_1e1e,
    );
    let regs_satp = m.cpu.csr.satp;
    step_until(
        &mut m,
        RUN_MAX_STEPS,
        |_| {},
        |m| {
            m.cpu.pc == user_vec
                && m.cpu.csr.scause == INTERRUPT_BIT | u64::from(IRQ_S_TIMER)
                && m.cpu.csr.satp == regs_satp
        },
    );
    let saved = m.cpu.x;
    let sepc = m.cpu.csr.sepc;
    assert!((USER_TEXT..USER_TOP).contains(&sepc));
    assert_eq!(saved[2], 0x8282_8282_0202_0202, "regs set sp to its pattern");
    // In U-mode, sscratch points at the process's trap frame.
    let frame = m.cpu.csr.sscratch;

    step_until(&mut m, 1000, |_| {}, |m| m.cpu.pc == user_trap);
    assert_eq!(m.cpu.x[x("a0")], frame);
    assert_eq!(m.cpu.x[2], frame, "the kernel stack starts below the frame");
    // user_vec swapped the user's a0 in, to save it.
    assert_eq!(m.cpu.csr.sscratch, saved[x("a0")]);
    let frame_pa = kpa(&m.cpu, frame);
    for (i, &v) in saved.iter().enumerate() {
        assert_eq!(
            read64_pa(&m, frame_pa + 8 * i as u64),
            v,
            "{}",
            excalibur::reg::abi_name(i as u8)
        );
    }
    assert_eq!(read64_pa(&m, frame_pa + FRAME_SEPC), sepc);
    assert_eq!(
        read64_pa(&m, frame_pa + FRAME_SCAUSE),
        INTERRUPT_BIT | u64::from(IRQ_S_TIMER)
    );
    assert_eq!(read64_pa(&m, frame_pa + FRAME_SSTATUS) & MSTATUS_SPP, 0);
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
    assert_eq!((m.cpu.x, m.cpu.pc), (saved, sepc));
    assert_eq!(m.run(Some(RUN_MAX_STEPS), |_| {}), RunResult::Exited(0));
    // And regs found every register intact at the end, after more switches.
    let out = printed(&m);
    assert!(out.contains(&format!("[{FIRST_PID}] regs: exit 0\n")), "{out}");
}

#[test]
fn registers_survive_switches_and_system_calls() {
    // regs checks its own registers after three ticks' worth of spinning
    // beside two programs that use the same registers: exit status 0, or the
    // number of the register that changed.
    let out = run_processes(b"regs & ding & dong\nwait\nexit 0\n");
    let mut collected = command_output(&out, "wait");
    collected.sort();
    assert_eq!(
        collected,
        [
            format!("[{FIRST_PID}] regs: exit 0"),
            format!("[{}] ding: exit 0", FIRST_PID + 1)
        ],
        "{out}"
    );
    // dong, in the foreground, exited 0: the shell said nothing about it.
    assert!(!out.contains("dong: exit"), "{out}");
}

#[test]
fn a_system_call_changes_a0_and_nothing_else() {
    // docs/camelot-syscalls.md: every register but a0 is preserved, a1-a7
    // and the temporaries included, which C programs can't check (they are
    // caller-saved there). sysregs fills every register, makes calls that
    // answer, switch away (yield, sleep), fail (a bad descriptor, an unknown
    // number) and write, and checks them all after each one; it exits with
    // 32 * case + register at the first that changed, which the shell would
    // print. Run it beside ding, so other processes use the registers too.
    let out = run_processes(b"ding & sysregs\nwait\nexit 0\n");
    let lines: Vec<&str> = command_output(&out, "ding & sysregs")
        .into_iter()
        .filter(|l| !l.starts_with("ding ") && !l.starts_with('['))
        .collect();
    assert_eq!(
        lines,
        ["sysregs: write", "sysregs: every register but a0 kept"],
        "{out}"
    );
}

#[test]
fn negative_results_arrive_sign_extended() {
    // sysregs makes calls with every register set to a pattern and checks
    // each one after: a0 against the case's result, a 32-bit word that lw
    // sign-extends. Its cases that fail (read from a closed descriptor:
    // -EBADF; an unknown call: -ENOSYS) pass only if the kernel left all 64
    // bits of a0 negative, 0xffff_ffff_ffff_fff7, not 0x0000_0000_ffff_fff7.
    let out = run(Setup::default(), b"sysregs\nexit 0\n");
    assert_eq!(
        command_output(&out, "sysregs"),
        ["sysregs: write", "sysregs: every register but a0 kept"]
    );
    // And the same from C: syscall()'s long and read()'s ssize_t (systest).
    let out = run(Setup::default(), b"systest\nexit 0\n");
    all_checks_pass(&out, "systest", "systest");
}

#[test]
fn faulting_programs_are_killed_and_the_rest_carry_on() {
    // Issue #20's second criterion. badload loads from the kernel's first
    // page (mapped, without U), badhalt executes xcal.halt (M-mode only).
    // The faults that grow a stack on demand are not the programs': they
    // carry on.
    let (mut m, elf) = load_committed(Some(kernel::typed(
        b"badload & ding & badhalt\nwait\nticks\nexit\n",
    )));
    let user_vec = sym(&elf, "user_vec");
    let mut faults = Vec::new();
    let r = m.run(Some(RUN_MAX_STEPS), |cpu| {
        let stack = (USER_STACK_LIMIT..USER_TOP).contains(&cpu.csr.stval)
            && [LOAD_PAGE_FAULT, STORE_PAGE_FAULT].contains(&cpu.csr.scause);
        if cpu.pc == user_vec
            && cpu.csr.scause & INTERRUPT_BIT == 0
            && cpu.csr.scause != ECALL_FROM_U
            && !stack
        {
            faults.push((cpu.csr.scause, cpu.csr.sepc, cpu.csr.stval));
        }
    });
    let out = printed(&m);
    assert_eq!(r, RunResult::Exited(0), "{out}");
    // badload faults at its load, its second instruction (after the li of
    // the kernel's address, one lui); badhalt at its second.
    let badload = user_sym(&user_program("badload").elf, "main") + 4;
    let badhalt = user_sym(&user_program("badhalt").elf, "main") + 4;
    let xcal_halt_a0 = 0x0005_100b;
    let kernel_first = 0xffff_ffff_8000_0000;
    assert_eq!(
        faults,
        [
            (LOAD_PAGE_FAULT, badload, kernel_first),
            (ILLEGAL_INSTRUCTION, badhalt, xcal_halt_a0)
        ]
    );
    // The kernel's first page, at its link address, onto its load address,
    // without U.
    let pa = image_pa(kernel_first);
    let pte = (pa >> 12) << 10 | PTE_V | PTE_R | PTE_X | PTE_G | PTE_A | PTE_D;
    let lines = command_output(&out, "badload & ding & badhalt");
    // The shell's `[FIRST_PID]` and the next for the background programs
    // come in order, but whether before or after badload's report depends
    // on where the ticks land in the shell's fork (with merlin's slow code,
    // often before it prints).
    let jobs: Vec<&str> = lines.iter().copied().filter(|l| l.starts_with('[')).collect();
    assert_eq!(
        jobs,
        [format!("[{FIRST_PID}]"), format!("[{}]", FIRST_PID + 1)],
        "{out}"
    );
    let want = [
        format!("pid {FIRST_PID} (badload) killed: load page fault"),
        format!("  scause 0x000000000000000d  sepc {badload:#018x}  stval {kernel_first:#018x}"),
        format!("  va {kernel_first:#018x} maps to pa {pa:#018x}, pte {pte:#018x} DAG-X-RV"),
        format!("pid {} (badhalt) killed: illegal instruction", FIRST_PID + 2),
        format!("  scause 0x0000000000000002  sepc {badhalt:#018x}  stval 0x000000000005100b"),
        "badhalt: exit -1".to_string(),
    ];
    let kills: Vec<&str> = lines
        .iter()
        .copied()
        .filter(|l| !l.starts_with("ding ") && !l.starts_with('['))
        .collect();
    assert_eq!(kills, want, "{out}");
    // ding wasn't disturbed, and the shell still works.
    let dings: Vec<&str> = out.lines().filter(|l| l.starts_with("ding ")).collect();
    assert_eq!(dings, ["ding 1", "ding 2", "ding 3", "ding 4"]);
    let mut collected: Vec<&str> = command_output(&out, "wait")
        .into_iter()
        .filter(|l| !l.starts_with("ding "))
        .collect();
    collected.sort();
    assert_eq!(
        collected,
        [
            format!("[{FIRST_PID}] badload: exit -1"),
            format!("[{}] ding: exit 0", FIRST_PID + 1)
        ]
    );
    assert_eq!(command_output(&out, "ticks").len(), 1);
    // The kernel never panicked, and xcal.halt never reached M-mode.
    assert!(!out.contains("panic"), "{out}");
}

#[test]
fn programs_get_their_memory_and_faults_kill_them() {
    // data: zeroed stack and .bss, initialized and writable .data. badstore
    // stores to its .rodata: killed. badhalt: an M-mode instruction. The
    // kernel carries on (stress).
    let out = run(Setup::default(), b"data\nbadstore\nbadhalt\nstress\nexit 0\n");
    assert!(command_output(&out, "data").is_empty(), "{out}");
    let badstore = command_output(&out, "badstore");
    assert!(
        badstore[0].ends_with("(badstore) killed: store page fault"),
        "{out}"
    );
    assert!(badstore[2].ends_with(" -A-U--RV"), "{out}");
    let badhalt = command_output(&out, "badhalt");
    assert!(
        badhalt[0].ends_with("(badhalt) killed: illegal instruction"),
        "{out}"
    );
    assert_eq!(
        command_output(&out, "stress").last().copied(),
        Some("stress: 4 workers, 4 rounds, 3200 bytes: ok")
    );
}

/// Runs `commands` (each a line) on the kernel booted on a machine with
/// `ram_mib` MiB of RAM, checking after each that every page the command's
/// process had is free again. Returns the output.
fn run_counting_pages(ram_mib: usize, commands: &[&str]) -> String {
    let (tx, rx) = std::sync::mpsc::channel();
    let (mut m, elf) = load(
        Setup {
            ram: ram_mib * MIB,
            ..Setup::default()
        }
        .config(),
        Some(rx),
    );
    step_to_prompt(&mut m, 1);
    let before = spare_pages(&m, &elf);
    for (i, line) in commands.iter().enumerate() {
        for b in line.bytes().chain(*b"\n") {
            tx.send(b).unwrap();
        }
        step_to_prompt(&mut m, i + 2);
        assert_eq!(
            spare_pages(&m, &elf),
            before,
            "{line} on {ram_mib} MiB:\n{}",
            printed(&m)
        );
    }
    printed(&m)
}

#[test]
fn the_user_stack_grows_on_demand_up_to_its_limit() {
    // spacetest: a stack of 200 KiB, a system call into a stack frame not
    // touched yet, and the pointer checks over the layout (vm.h). On 8 MiB,
    // a stack overflow runs out of RAM before it reaches the stack's limit:
    // said, then killed, and the kernel carries on, every page back.
    let out = run_counting_pages(8, &["spacetest", "spacetest overflow", "echo alive"]);
    assert_eq!(all_checks_pass(&out, "spacetest", "spacetest"), 14, "{out}");
    let overflow = command_output(&out, "spacetest overflow");
    assert!(
        overflow[0].starts_with("pid ")
            && overflow[0].contains(" (spacetest): no memory to grow its stack to 0x00007fff"),
        "{out}"
    );
    assert!(
        overflow[1].ends_with("(spacetest) killed: store page fault"),
        "{out}"
    );
    assert_eq!(command_output(&out, "echo alive"), ["alive"]);

    // With 16 MiB: a 6 MiB heap, every page written, then given back; and
    // an overflow that reaches the guard page under the stack's 8 MiB:
    // USER_STACK_LIMIT - 4 KiB to USER_STACK_LIMIT, below 2^47 in Sv48 and
    // 2^38 in Sv39.
    let out = run_counting_pages(16, &["spacetest heap 6", "spacetest overflow", "echo alive"]);
    assert_eq!(all_checks_pass(&out, "spacetest heap 6", "spacetest"), 7, "{out}");
    for (paging, top) in [
        (grail::Paging::Sv48, 1u64 << 47),
        (grail::Paging::Sv39, 1u64 << 38),
    ] {
        let out = if paging == grail::Paging::Sv48 {
            out.clone()
        } else {
            run(
                Setup {
                    paging,
                    ..Setup::default()
                },
                b"spacetest\nspacetest heap 6\nspacetest overflow\necho alive\nexit 0\n",
            )
        };
        let overflow = command_output(&out, "spacetest overflow");
        assert!(
            overflow[0].ends_with("(spacetest) killed: store page fault"),
            "{out}"
        );
        let stval = overflow[1].rsplit("stval 0x").next().unwrap();
        let stval = u64::from_str_radix(stval.trim(), 16).unwrap();
        let limit = top - (8 << 20);
        assert!((limit - PAGE..limit).contains(&stval), "{paging:?}: {out}");
        assert_eq!(command_output(&out, "echo alive"), ["alive"]);
    }
}

#[test]
fn memory_mappings_are_mapped_on_demand_and_given_back() {
    // Issue #179: maptest's checks of mmap, munmap and mprotect on
    // anonymous memory (placement, MAP_FIXED, splits, protections, the
    // errors, the kernel's copies into pages not touched yet, the heap
    // kept out of a mapping, 200 MiB mapped on an 8 MiB machine), and of
    // shared-memory objects (memfd_create, ftruncate, two mappings of one
    // in a process and in two); then a touch of a PROT_NONE page, of an
    // unmapped one, a store to a read-only one and a touch past an object's
    // end, each killing the program: every page is back after each command,
    // the mappings' and the objects' included.
    let commands = [
        "maptest",
        "maptest none",
        "maptest unmapped",
        "maptest readonly",
        "maptest past",
        "echo alive",
    ];
    let out = run_counting_pages(8, &commands);
    assert_eq!(
        all_checks_pass(&out, "maptest", "maptest"),
        MAPTEST_CHECKS,
        "{out}"
    );
    for (command, fault) in [
        ("maptest none", "load page fault"),
        ("maptest unmapped", "load page fault"),
        ("maptest readonly", "store page fault"),
        ("maptest past", "load page fault"),
    ] {
        let lines = command_output(&out, command);
        assert!(
            lines[0].starts_with("pid ") && lines[0].ends_with(&format!(" (maptest) killed: {fault}")),
            "{command}: {out}"
        );
    }
    assert_eq!(command_output(&out, "echo alive"), ["alive"]);

    // In Sv39's smaller user space too.
    let out = run(
        Setup {
            paging: grail::Paging::Sv39,
            ..Setup::default()
        },
        b"maptest\nexit 0\n",
    );
    assert_eq!(
        all_checks_pass(&out, "maptest", "maptest"),
        MAPTEST_CHECKS,
        "{out}"
    );
}

#[test]
fn a_program_draws_into_the_framebuffer_through_a_mapping_of_dev_fb0() {
    // Issue #179's device mappings: `maptest fb` on the root image maps
    // /dev/fb0, the framebuffer's VRAM, draws a pattern into it and checks
    // the errors. The display shows what it drew, and no page of RAM went
    // to it: a device's pages are the device's.
    let (tx, rx) = std::sync::mpsc::channel();
    let (mut m, elf) = load(
        Setup {
            disks: vec![root_disk(4096, &[])],
            ..Setup::default()
        }
        .config(),
        Some(rx),
    );
    step_to_prompt(&mut m, 1);
    let before = spare_pages(&m, &elf);
    for b in b"maptest fb\n" {
        tx.send(*b).unwrap();
    }
    step_to_prompt(&mut m, 2);
    let out = printed(&m);
    assert_eq!(spare_pages(&m, &elf), before, "{out}");
    assert_eq!(all_checks_pass(&out, "maptest fb", "maptest"), 15, "{out}");
    let fb = &m.cpu.bus.fb;
    for (x, y) in [(0, 0), (1, 0), (639, 0), (0, 479), (321, 123), (639, 479)] {
        assert_eq!(
            fb.pixel(x, y),
            (x & 0xff) << 16 | (y & 0xff) << 8 | 0x5a,
            "({x}, {y})"
        );
    }
}

/// A machine with the root image, booted to its first prompt, and the pages
/// spare then.
fn root_machine() -> (Machine, grail::elf::Elf, std::sync::mpsc::Sender<u8>, u32) {
    let (tx, rx) = std::sync::mpsc::channel();
    let (mut m, elf) = load(
        Setup {
            disks: vec![root_disk(4096, &[])],
            ..Setup::default()
        }
        .config(),
        Some(rx),
    );
    step_to_prompt(&mut m, 1);
    let spare = spare_pages(&m, &elf);
    (m, elf, tx, spare)
}

#[test]
fn the_framebuffer_device_answers_linuxs_fbdev_requests() {
    // Issue #178: fbtest's checks of /dev/fb0's ioctls: the mode and the
    // buffer (FBIOGET_VSCREENINFO, FBIOGET_FSCREENINFO), modes set and
    // tested (FBIOPUT_VSCREENINFO) in RAM and back in VRAM, -EBUSY while the
    // buffer is mapped, a double-height buffer flipped with FBIOPAN_DISPLAY,
    // FBIO_WAITFORVSYNC, FBIOBLANK and the errors. Every buffer in RAM is
    // given back once the device is let go, and the display is 640x480
    // from VRAM again.
    let (mut m, elf, tx, before) = root_machine();
    for b in b"fbtest\n" {
        tx.send(*b).unwrap();
    }
    step_to_prompt(&mut m, 2);
    let out = printed(&m);
    assert_eq!(all_checks_pass(&out, "fbtest", "fbtest"), 63, "{out}");
    assert_eq!(spare_pages(&m, &elf), before, "{out}");
    let fb = &m.cpu.bus.fb;
    assert_eq!((fb.mode().width, fb.mode().height, fb.scanout()), (640, 480, 0));
    assert!(fb.enabled());
}

#[test]
fn a_program_shows_800x600_from_ram_with_a_flip() {
    // `fbtest show` sets 800x600 with two frames in RAM, draws both and
    // flips to the second: the display scans it out, from row 608, the
    // first on a page after the first frame.
    // When the program lets go of the device, the display is 640x480 from
    // VRAM again and the buffer's pages are free.
    let (mut m, elf, tx, before) = root_machine();
    for b in b"fbtest show\n" {
        tx.send(*b).unwrap();
    }
    step_until(
        &mut m,
        RUN_MAX_STEPS,
        |_| {},
        |m| m.cpu.bus.uart.captured.ends_with(b"fbtest: shown\n"),
    );
    let fb = &m.cpu.bus.fb;
    assert_eq!((fb.mode().width, fb.mode().height), (800, 600));
    let scanout = fb.scanout();
    assert!(scanout != 0 && scanout % 4096 == 0, "{scanout:#x}");
    assert!(spare_pages(&m, &elf) <= before - 1208 * 800 * 4 / 4096);
    let frame = m.cpu.bus.frame();
    assert_eq!((frame.width, frame.height), (800, 600));
    for (x, y) in [(0, 0), (799, 0), (123, 321), (0, 599), (799, 599)] {
        assert_eq!(
            frame.pixel(x, y),
            (x & 0xff) << 16 | (y & 0xff) << 8 | 0x41,
            "({x}, {y})"
        );
    }
    tx.send(b'\n').unwrap();
    step_to_prompt(&mut m, 2);
    let fb = &m.cpu.bus.fb;
    assert_eq!((fb.mode().width, fb.mode().height, fb.scanout()), (640, 480, 0));
    assert_eq!(spare_pages(&m, &elf), before, "{}", printed(&m));
}

/// Types `line` on the UART.
fn type_line(tx: &std::sync::mpsc::Sender<u8>, line: &str) {
    for b in line.bytes() {
        tx.send(b).unwrap();
    }
}

/// Steps `m` until what it printed ends with `tail`.
fn step_to_output(m: &mut Machine, tail: &str) {
    step_until(m, RUN_MAX_STEPS, |_| {}, |m| m.cpu.bus.uart.captured.ends_with(tail.as_bytes()));
}

/// Runs `command`, an `inputtest` that prints `ready` when it is reading,
/// on a root machine; then gives it `input` (each a key or a pointer
/// change) and returns the lines it printed after `ready`, once it is back
/// at the prompt. `before` runs first, at the prompt.
fn input_events(
    command: &str,
    before: impl FnOnce(&mut Machine, &grail::elf::Elf),
    input: impl FnOnce(&mut Machine),
) -> (Machine, String) {
    let (mut m, elf, tx, _) = root_machine();
    before(&mut m, &elf);
    type_line(&tx, &format!("{command}\n"));
    step_to_output(&mut m, "inputtest: ready\n");
    let at = m.cpu.bus.uart.captured.len();
    input(&mut m);
    step_to_prompt(&mut m, 2);
    let out = String::from_utf8_lossy(&m.cpu.bus.uart.captured[at..]).into_owned();
    let out = out.strip_suffix(PROMPT).unwrap_or(&out).to_string();
    (m, out)
}

/// A key of both forms, as a host sends it: its ASCII-mode code and its
/// usage.
fn key(code: u8, usage: u16) -> grail::keyboard::KeyEvent {
    grail::keyboard::KeyEvent::press(code).with_usage(usage)
}

fn release(k: grail::keyboard::KeyEvent) -> grail::keyboard::KeyEvent {
    grail::keyboard::KeyEvent { release: true, ..k }
}

#[test]
fn the_input_devices_answer_evdevs_requests() {
    // Issue #178: inputtest's checks of /dev/input/event0 and event1:
    // EVIOCGVERSION, EVIOCGID, EVIOCGNAME, EVIOCGBIT, EVIOCGABS (the
    // display's size), EVIOCGRAB's rules, reads of whole events and the
    // errors. The keyboard is in raw mode while it has a reader, and back
    // in ASCII mode, the console's, after.
    let (mut m, _, tx, _) = root_machine();
    type_line(&tx, "inputtest\n");
    step_to_prompt(&mut m, 2);
    let out = printed(&m);
    assert_eq!(all_checks_pass(&out, "inputtest", "inputtest"), 46, "{out}");
    assert!(!m.cpu.bus.kbd.raw());
}

#[test]
fn keys_reach_a_reader_as_linux_key_codes() {
    // A reader of the keyboard puts it in raw mode: each key is its HID
    // usage, which the driver makes Linux's key code, a press and a release
    // each with its SYN_REPORT. A modifier is a key of its own; a press of
    // a key already down is no event.
    let (m, out) = input_events(
        "inputtest keys 8",
        |_, _| {},
        |m| {
            assert!(m.cpu.bus.kbd.raw());
            let a = key(b'a', 0x04);
            assert!(m.key_event(a));
            assert!(m.key_event(release(a)));
            assert!(m.key_event(key(0, 0xe1))); // Left Shift
            assert!(m.key_event(key(0, 0xe3))); // Left GUI
        },
    );
    assert_eq!(out, "1 30 1\n0 0 0\n1 30 0\n0 0 0\n1 42 1\n0 0 0\n1 125 1\n0 0 0\n");
    assert!(!m.cpu.bus.kbd.raw());
}

#[test]
fn a_keyboard_without_raw_mode_gives_key_codes_made_from_ascii() {
    // The driver found no raw mode (as on a keyboard before it): an ASCII
    // code becomes the key that types it on a US layout, with the Shift
    // and Ctrl its bits say pressed (and let go) around it.
    let (_, out) = input_events(
        "inputtest keys 9",
        |m, elf| m.cpu.bus.load(image_pa(sym(elf, "keyboard_has_raw")), &[0; 4]).unwrap(),
        |m| {
            assert!(!m.cpu.bus.kbd.raw());
            let shifted = key(b'A', 0x04).with_shift(true);
            assert!(m.key_event(shifted));
            assert!(m.key_event(release(shifted)));
            let ctrl_c = key(b'c', 0x06).with_ctrl(true);
            assert!(m.key_event(ctrl_c));
            assert!(m.key_event(release(ctrl_c)));
        },
    );
    assert_eq!(
        out,
        "1 42 1\n1 30 1\n0 0 0\n1 30 0\n0 0 0\n1 42 0\n1 29 1\n1 46 1\n0 0 0\n"
    );
}

#[test]
fn the_console_types_what_the_keyboard_types() {
    // With no reader, the keyboard is the console's, in ASCII mode: the
    // characters the host's layout typed, which the line editor echoes as
    // it does the UART's. Ctrl with a letter is its control character:
    // Ctrl-H erases.
    let (mut m, _) = load_committed(None);
    step_to_prompt(&mut m, 1);
    for c in "hellp".bytes() {
        assert!(m.key_event(key(c, 0)));
    }
    assert!(m.key_event(key(b'h', 0x0b).with_ctrl(true)));
    for c in "o\n".bytes() {
        assert!(m.key_event(key(c, 0)));
    }
    step_to_prompt(&mut m, 2);
    let out = printed(&m);
    assert!(out.ends_with(&format!("{PROMPT}hellp\x08 \x08o\nhello from user mode\n{PROMPT}")), "{out}");
}

#[test]
fn the_pointer_reports_where_it_is_its_buttons_and_the_wheel() {
    // The pointer in absolute mode: a move is ABS_X and ABS_Y, the axes
    // that changed; a button BTN_LEFT; a notch REL_WHEEL; each change a
    // packet.
    use grail::pointer::{PointerInput, BUTTON_LEFT};
    let (_, out) = input_events(
        "inputtest pointer 11",
        |_, _| {},
        |m| {
            assert!(m.pointer_event(PointerInput::move_to(100, 200)));
            assert!(m.pointer_event(PointerInput::move_to(100, 201)));
            assert!(m.pointer_event(PointerInput::buttons(BUTTON_LEFT)));
            assert!(m.pointer_event(PointerInput::buttons(BUTTON_LEFT).with_wheel(1)));
            assert!(m.pointer_event(PointerInput::buttons(0)));
        },
    );
    assert_eq!(
        out,
        "3 0 100\n3 1 200\n0 0 0\n3 1 201\n0 0 0\n1 272 1\n0 0 0\n2 8 1\n0 0 0\n1 272 0\n0 0 0\n"
    );
}

#[test]
fn a_grab_keeps_the_keys_from_the_other_readers() {
    // Two readers of the keyboard, the first grabbing it: a key goes to it
    // alone. Once it lets go, the next goes to every reader: the second
    // reads that one, and never saw the first.
    let (mut m, _, tx, _) = root_machine();
    type_line(&tx, "inputtest grab\n");
    step_to_output(&mut m, "inputtest: grabbed\n");
    assert!(m.key_event(key(b'a', 0x04)));
    step_to_output(&mut m, "inputtest: let go\n");
    assert!(m.key_event(key(b'b', 0x05)));
    step_to_prompt(&mut m, 2);
    let out = printed(&m);
    assert!(
        out.ends_with(&format!(
            "inputtest: grabbed\n1 30 1\n0 0 0\ninputtest: let go\n1 48 1\n0 0 0\n{PROMPT}"
        )),
        "{out}"
    );
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
    let out = run_processes(b"badptr\nhello\nexit 0\n");
    assert_eq!(
        command_output(&out, "badptr"),
        ["badptr: every bad pointer refused"]
    );
    assert_eq!(command_output(&out, "hello"), ["hello from user mode"]);
}

#[test]
fn pointers_into_the_kernel_half_are_refused() {
    // badptr: every call with a bad pointer fails with -EFAULT, among them
    // the kernel image's link address, the lowest address of the upper
    // half, the direct map's start in Sv48 and in Sv39, the vmalloc area,
    // the top page, USER_TOP itself (not canonical) and 2^32 + 0x1000 (not
    // truncated to 0x1000), in Sv48 and in Sv39.
    for paging in [grail::Paging::Sv48, grail::Paging::Sv39] {
        let out = run(
            Setup {
                paging,
                ..Setup::default()
            },
            b"badptr\nbadload\nexit 0\n",
        );
        assert_eq!(
            command_output(&out, "badptr"),
            ["badptr: every bad pointer refused"],
            "{paging:?}"
        );
        // A load from the kernel image in U-mode: mapped (its link address
        // to its load address, after the firmware's 2 MiB), without U.
        let badload = command_output(&out, "badload");
        assert!(
            badload[0].ends_with(" (badload) killed: load page fault"),
            "{out}"
        );
        assert!(badload[1].ends_with("stval 0xffffffff80000000"), "{out}");
        assert!(
            badload[2].starts_with("  va 0xffffffff80000000 maps to pa 0x0000000080200000")
                && badload[2].ends_with(" DAG-X-RV"),
            "{out}"
        );
        assert_eq!(badload[3], "badload: exit -1");
    }
}

#[test]
fn the_system_calls_do_what_the_document_says() {
    // systest checks every call from user space: the error numbers, exit
    // statuses and arguments through spawn and wait, filling the process
    // table, sleep, sbrk growing and shrinking the heap, and the RV64
    // checks (a negative result as a long, all of a7 the call number, sbrk
    // by LONG_MIN).
    let out = run_processes(b"systest\nexit 0\n");
    assert!(all_checks_pass(&out, "systest", "systest") > 200, "{out}");
}

#[test]
fn the_c_library_does_what_user_h_says() {
    // libtest checks the user C library (kernel/user/lib): strings, numbers
    // (strtol's 64-bit LONG_MAX), snprintf's formats (%ld of 64-bit values,
    // %p's 16 digits), malloc's 16-byte blocks and free (a big block in a
    // mapping of its own, given back by free), readers on a pipe
    // fed by a child of its own, and addresses (inet.c, and resolve.c on
    // addresses).
    let out = run_processes(b"libtest\nexit 0\n");
    assert!(all_checks_pass(&out, "libtest", "libtest") > 70, "{out}");
}

#[test]
fn programs_link_only_the_library_members_they_need() {
    // The library is linked like an archive: hello prints with printf, so
    // it has printf and strlen but no malloc; grep grows its line buffer
    // with realloc, so it has the allocator too. Neither has a function it
    // never calls, such as strtol.
    let has = |prog: &str, name: &str| {
        let p = user_program(prog);
        let syms = elf::Elf::parse(&p.elf).unwrap().symbols().unwrap();
        syms.iter().any(|s| s.name == name)
    };
    for (prog, name, want) in [
        ("hello", "printf", true),
        ("hello", "strlen", true),
        ("hello", "malloc", false),
        ("hello", "strtol", false),
        ("grep", "realloc", true),
        ("grep", "malloc", true),
        ("grep", "reader_getline", true),
        ("grep", "strtol", false),
        ("kill", "strtol", true),
        ("regs", "printf", false),
    ] {
        assert_eq!(has(prog, name), want, "{prog}: {name}");
    }
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
    let out = run_processes(b"echo\necho one\necho  a   b\tc  d \nexit 0\n");
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
    // scheduler idles in wfi: each tick costs about ten thousand
    // instructions (merlin's code; a few hundred with clang -O2), where
    // polling would run the whole interval, a million.
    let (mut m, elf) = load_committed(Some(kernel::typed(b"systest sleep 50\nexit\n")));
    let ticks = image_pa(sym(&elf, "ticks"));
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
    assert!(steps < 50 * 15_000, "{steps} steps for {slept} ticks");
    assert!(user_steps < 10_000, "{user_steps} steps in U-mode");
    assert_eq!(m.run(Some(RUN_MAX_STEPS), |_| {}), RunResult::Exited(0));
}

#[test]
fn input_typed_while_processes_run_reaches_the_shell() {
    // The UART interrupt arrives while a process is in U-mode: user_trap
    // services it and the process carries on; the shell reads the line once
    // ding is over.
    let (tx, rx) = std::sync::mpsc::channel();
    let (mut m, elf) = load_committed(Some(rx));
    for &b in b"ding\n" {
        tx.send(b).unwrap();
    }
    step_to_user(&mut m, &elf, "ding");
    for &b in b"ticks\nexit\n" {
        tx.send(b).unwrap();
    }
    let user_vec = sym(&elf, "user_vec");
    let mut external = 0;
    let r = m.run(Some(RUN_MAX_STEPS), |cpu| {
        if cpu.pc == user_vec && cpu.csr.scause == INTERRUPT_BIT | u64::from(IRQ_S_EXT) {
            external += 1;
        }
    });
    assert_eq!(r, RunResult::Exited(0));
    assert!(external >= 1);
    let out = printed(&m);
    assert!(out.contains(&format!("ding 4\n{PROMPT}ticks\nticks: ")), "{out}");
}

#[test]
fn processes_give_back_every_page() {
    let (tx, rx) = std::sync::mpsc::channel();
    let (mut m, elf) = load_committed(Some(rx));
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
    let before = spare_pages(&m, &elf);

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
        assert_eq!(spare_pages(&m, &elf), before, "{name}");
    }
    // Several side by side, then collected.
    let some = ["hello", "badload", "ding", "data", "badstore", "echo"];
    command(&mut m, &format!("{} &\n", some.join(" & ")));
    command(&mut m, "wait\n");
    assert_eq!(spare_pages(&m, &elf), before);
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
    assert_eq!(spare_pages(&m, &elf), before);
    // While a process runs, it holds its pages: a root and the six user
    // tables (three levels down to its segments at the bottom of user
    // space, three to its stack at the top), its segments and its stack.
    // Its kernel stack is one the hart keeps cached from the processes
    // before (vm.h, NR_CACHED_STACKS), so no longer spare, and its thread
    // and process are slots of the kernel's tables.
    for &b in b"ding\n" {
        tx.send(b).unwrap();
    }
    step_to_user(&mut m, &elf, "ding");
    let pages = expected_user_pages(&user_program("ding").elf).len() as u32;
    assert_eq!(spare_pages(&m, &elf), before - 1 - 6 - pages - KSTACK_PAGES);
}

/// Checks stress's console output for `workers` workers and `rounds` rounds
/// (kernel/user/stress.c): each `stress W: round R` line exactly once, each
/// worker's rounds in order, and the final verdict.
fn check_stress_output(lines: &[&str], workers: u32, rounds: u32) {
    let mut next = vec![1; workers as usize + 1];
    for line in &lines[..lines.len() - 1] {
        let (w, r) = line
            .strip_prefix("stress ")
            .and_then(|l| l.split_once(": round "))
            .unwrap_or_else(|| panic!("{line:?} in {lines:#?}"));
        let (w, r): (usize, u32) = (w.parse().unwrap(), r.parse().unwrap());
        assert_eq!(r, next[w], "worker {w} in {lines:#?}");
        next[w] += 1;
    }
    assert!(next[1..].iter().all(|&n| n == rounds + 1), "{lines:#?}");
    assert_eq!(
        lines.last(),
        Some(
            &format!(
                "stress: {workers} workers, {rounds} rounds, {} bytes: ok",
                workers * rounds * 200
            )
            .as_str()
        )
    );
}

#[test]
fn stress_runs_many_processes_through_the_locks() {
    // Issue #61's stress program: six workers (with the shell and stress,
    // every process slot) allocating and freeing pages, printing, and
    // writing records down one pipe, all under timer preemption. Each
    // worker's bytes arrive complete and in order (possibly interleaved
    // with the other workers'), every page comes back, the workers really
    // are preempted by timer ticks in the middle of their rounds, and no
    // lock is left held whenever a process runs in U-mode.
    const LINE: &str = "stress 6 3";
    const WORKERS: usize = 6;
    const ROUNDS: u32 = 3;
    let (tx, rx) = std::sync::mpsc::channel();
    let (mut m, elf) = load_committed(Some(rx));
    step_to_prompt(&mut m, 1);
    let before = spare_pages(&m, &elf);
    for b in format!("{LINE}\n").bytes() {
        tx.send(b).unwrap();
    }
    // A worker in spin() is inside heap_round, between the sbrk that grows
    // its heap and the one that shrinks it.
    let spin = elf::Elf::parse(&user_program("stress").elf)
        .unwrap()
        .symbols()
        .unwrap()
        .into_iter()
        .find(|s| s.name == "spin")
        .map(|s| s.value..s.value + s.size)
        .unwrap();
    assert!(!spin.is_empty());
    // Every switch from a stress process to another process in U-mode, and
    // what made it leave: preemptions are the switches whose last trap from
    // U-mode was a timer tick (as in ding_and_dong_take_turns_on_timer_ticks).
    let (cpus, user_vec) = (sym(&elf, "cpus"), sym(&elf, "user_vec"));
    let (mut last, mut last_trap) = (0, None);
    let (mut preemptions, mut in_heap_round) = (0, HashMap::new());
    let mut locks = LockWatch::new(&elf);
    let printed_before = m.cpu.bus.uart.captured.len();
    let mut steps = 0;
    while !m.cpu.bus.uart.captured[printed_before..].ends_with(PROMPT.as_bytes()) {
        assert!(steps < RUN_MAX_STEPS, "stress never ended:\n{}", printed(&m));
        locks.step(&m.cpu);
        if m.cpu.mode == Mode::U {
            let p = peek64(&m.cpu, cpus + CPU_PROC);
            if p != last {
                let prev = last;
                last = p;
                if let Some((cause, pc)) = last_trap.take() {
                    if prev != 0
                        && cause == INTERRUPT_BIT | u64::from(IRQ_S_TIMER)
                        && proc_name(&m.cpu, prev) == "stress"
                    {
                        preemptions += 1;
                        if spin.contains(&pc) {
                            *in_heap_round.entry(prev).or_insert(0) += 1;
                        }
                    }
                }
            }
        }
        if m.cpu.pc == user_vec {
            last_trap = Some((m.cpu.csr.scause, m.cpu.csr.sepc));
        }
        m.cpu.step();
        steps += 1;
    }
    let out = printed(&m);
    check_stress_output(&command_output(&out, LINE), WORKERS as u32, ROUNDS);
    // Each worker spins about half a tick per round, so ticks land in most
    // rounds' spins; voluntary switches, sleeping on the pipe or exiting,
    // don't count. With the tick's yield() taken out of user_trap there are
    // none, though the processes still switch dozens of times.
    assert!(
        in_heap_round.len() == WORKERS && in_heap_round.values().all(|&n| n >= 1),
        "workers preempted in heap_round: {in_heap_round:?}"
    );
    let total: u32 = in_heap_round.values().sum();
    assert!(
        total >= WORKERS as u32 * ROUNDS / 2,
        "{total} preemptions in heap_round"
    );
    assert!(
        preemptions >= WORKERS as u32 * ROUNDS,
        "{preemptions} preemptions"
    );
    assert_eq!(spare_pages(&m, &elf), before);
    common::exit_shell(&mut m, &tx);
}

#[test]
fn killing_stress_workers_mid_run_leaves_no_lock_held() {
    // Two of stress's workers killed while they go through the locked paths
    // (sbrk, the console, the pipe): they end with -1, stress says so, the
    // others finish, and no lock or push_off() is left behind.
    const LINE: &str = "stress 4 8 &";
    let (tx, rx) = std::sync::mpsc::channel();
    let (mut m, elf) = load_committed(Some(rx));
    step_to_prompt(&mut m, 1);
    let before = spare_pages(&m, &elf);
    // stress is pid FIRST_PID; by the end of the sleep each of its workers is a few
    // rounds in, so the kills land in the middle of their work.
    for b in format!("{LINE}\nsleep 3\nps\n").bytes() {
        tx.send(b).unwrap();
    }
    let mut locks = LockWatch::new(&elf);
    step_until(
        &mut m,
        RUN_MAX_STEPS,
        |cpu| locks.step(cpu),
        |m| {
            let out = printed(m);
            out.find(&format!("{PROMPT}ps\n"))
                .is_some_and(|at| out[at + 3..].contains(&format!("\n{PROMPT}")))
        },
    );
    // ps shows the workers, stress's children, spawned in order: worker 1 has
    // the lowest pid. Whether the shell's `sleep 3` or the first workers came
    // first depends on who ran first, so the pids are read, not assumed.
    let out = printed(&m);
    let mut workers: Vec<u32> = command_output(&out, "ps")
        .iter()
        .filter_map(|l| {
            let f: Vec<&str> = l.split_whitespace().collect();
            (f.len() == 5 && f[1] == FIRST_PID.to_string() && f[4] == "stress").then(|| f[0].parse().unwrap())
        })
        .collect();
    workers.sort();
    assert_eq!(workers.len(), 4, "{out}");
    for b in format!("kill {}\nkill {}\nwait\n", workers[0], workers[2]).bytes() {
        tx.send(b).unwrap();
    }
    step_until(
        &mut m,
        RUN_MAX_STEPS,
        |cpu| locks.step(cpu),
        |m| {
            m.cpu
                .bus
                .uart
                .captured
                .ends_with(format!("[{FIRST_PID}] stress: exit 1\n$ ").as_bytes())
        },
    );
    let out = printed(&m);
    let lines: Vec<&str> = out[out.find(&format!("{PROMPT}{LINE}\n")).unwrap()..]
        .lines()
        .collect();
    for w in 1..=4 {
        // The killed workers stop partway; the others finish every round.
        let rounds = lines
            .iter()
            .filter(|l| l.starts_with(&format!("stress {w}: round ")))
            .count();
        assert_eq!(rounds == 8, w % 2 == 0, "worker {w}: {rounds} rounds\n{out}");
    }
    assert!(
        command_output(&out, "wait").ends_with(&[
            "stress: worker 1: exit -1",
            "stress: worker 3: exit -1",
            &format!("[{FIRST_PID}] stress: exit 1")
        ]),
        "{out}"
    );
    assert_eq!(spare_pages(&m, &elf), before);
    for &b in b"exit\n" {
        tx.send(b).unwrap();
    }
    assert_eq!(
        m.run(Some(RUN_MAX_STEPS), |cpu| locks.step(cpu)),
        RunResult::Exited(0)
    );
}

#[test]
fn a_heap_that_runs_out_of_memory_gives_back_what_it_took() {
    // sbrk maps the new pages one at a time: when memory runs out partway,
    // it must unmap and free the ones it had mapped and leave the heap as it
    // was. 4 MB fits in user space, and a 16 MiB machine has the pages...
    const LINE: &str = "systest sbrk 4000000";
    let out = run(Setup::default(), format!("{LINE}\nexit 0\n").as_bytes());
    assert_eq!(command_output(&out, LINE), ["systest: sbrk 4000000: 0"]);

    // ...but an 8 MiB one runs out of them after mapping some (the
    // firmware's 2 MiB and the kernel with its embedded programs, a little
    // over 2 MiB, leave systest too few).
    let (tx, rx) = std::sync::mpsc::channel();
    let (mut m, elf) = load_committed(Some(rx));
    step_to_prompt(&mut m, 1);
    let before = spare_pages(&m, &elf);
    // systest's own pages (see processes_give_back_every_page), then a page
    // for the heap at least.
    let pages = 1 + 6 + expected_user_pages(&user_program("systest").elf).len() as u32;
    assert!(
        before > pages + 1 && before < pages + 4_000_000 / PAGE as u32,
        "{before} pages"
    );
    for b in format!("{LINE}\n").bytes() {
        tx.send(b).unwrap();
    }
    // Right after the call, systest prints its result: it holds no page more
    // than when it started.
    let want = "systest: sbrk 4000000: -12\n";
    let cpus = sym(&elf, "cpus");
    let mut satp = 0;
    step_until(
        &mut m,
        RUN_MAX_STEPS,
        |cpu| {
            if cpu.mode == Mode::U && current_name(cpu, cpus).as_deref() == Some("systest") {
                satp = cpu.csr.satp;
            }
        },
        |m| m.cpu.bus.uart.captured.ends_with(want.as_bytes()),
    );
    // Its pages: its segments' and its stack's (deeper than one page by
    // now: printf's frames), and none of a heap.
    let (held, tables) = leaves(&m, (satp & 0xfff_ffff_ffff) << 12, false);
    let segments = expected_user_pages(&user_program("systest").elf);
    for (va, _, _) in &held {
        assert!(
            segments.iter().any(|(s, _)| s == va) || (USER_STACK_LIMIT..USER_TOP).contains(va),
            "{va:#x} mapped"
        );
    }
    // Its six tables (processes_give_back_every_page), and the one the heap
    // needed as it grew past the first 2 MiB, which the process keeps until
    // it exits (uvm_free() frees the tables, as xv6's does).
    assert_eq!(tables, 6 + 1);
    // Its root, tables and pages, and its kernel stack, one the hart had
    // cached.
    let held = 1 + tables as u32 + held.len() as u32;
    assert!(held >= pages);
    assert_eq!(spare_pages(&m, &elf), before - held - KSTACK_PAGES);
    // And it found the heap as it was: no exit line from the shell.
    step_to_prompt(&mut m, 2);
    let out = printed(&m);
    assert_eq!(command_output(&out, LINE), [want.trim_end()]);
    assert_eq!(spare_pages(&m, &elf), before);
    common::exit_shell(&mut m, &tx);
}

#[test]
fn a_heap_growth_past_the_whole_of_ram_is_refused_at_once() {
    // sbrk refuses a growth that maps more pages than RAM holds before it
    // takes any, as Linux's overcommit check does, counting the pages from
    // the old end's page boundary to the new end's: RAM and a byte more is
    // a page more than RAM, refused at once. RAM itself is not: sbrk takes
    // and zeroes every free page before it runs out, which takes far longer.
    let ram: usize = 16 * MIB;
    let (tx, rx) = std::sync::mpsc::channel();
    let (mut m, elf) = load(
        Setup {
            ram,
            ..Setup::default()
        }
        .config(),
        Some(rx),
    );
    step_to_prompt(&mut m, 1);
    let before = spare_pages(&m, &elf);
    let steps = |m: &mut Machine, n: usize, prompts: usize| {
        let line = format!("systest sbrk {n}");
        for b in format!("{line}\n").bytes() {
            tx.send(b).unwrap();
        }
        let want = format!("systest: sbrk {n}: -12\n");
        let took = step_until(
            m,
            RUN_MAX_STEPS,
            |_| {},
            |m| m.cpu.bus.uart.captured.ends_with(want.as_bytes()),
        );
        step_to_prompt(m, prompts);
        assert_eq!(command_output(&printed(m), &line), [want.trim_end()]);
        assert_eq!(spare_pages(m, &elf), before, "{line}");
        took
    };
    let all_of_ram = steps(&mut m, ram, 2);
    let past_ram = steps(&mut m, ram + 1, 3);
    assert!(
        past_ram * 4 < all_of_ram,
        "RAM and a byte took {past_ram} steps to refuse, RAM {all_of_ram}"
    );
    common::exit_shell(&mut m, &tx);
}

/// Byte offsets in an ELF64 header and program header.
const E_TYPE: u64 = 16;
const E_MACHINE: u64 = 18;
const E_ENTRY: u64 = 24;
const E_PHOFF: u64 = 32;
const E_PHENTSIZE: u64 = 54;
const E_PHNUM: u64 = 56;
const P_FLAGS: u64 = 4;
const P_OFFSET: u64 = 8;
const P_VADDR: u64 = 16;
const P_FILESZ: u64 = 32;
const P_MEMSZ: u64 = 40;

/// Bytes to write at an offset into an ELF image.
type Patch<'a> = (u64, &'a [u8]);

#[test]
fn the_elf_loader_refuses_bad_images() {
    // hello's image in the kernel, patched in memory one way at a time; each
    // `hello` typed at the shell must fail with the loader's reason (or run,
    // for the patches that keep the image good), leak nothing, and leave the
    // kernel running.
    let hello = user_program("hello");
    let parsed = elf::Elf::parse(&hello.elf).unwrap();
    // hello is ELF64: the address, offset and size fields patched below are
    // 8 bytes.
    let phoff = parsed.header.phoff;
    let size = hello.elf.len() as u64;
    let loads: Vec<u64> = (0..u64::from(parsed.header.phnum))
        .filter(|&i| parsed.segments[i as usize].ty == elf::PT_LOAD)
        .collect();
    assert_eq!(loads.len(), 3, "hello: text, rodata and data (environ)");
    let ph = |i: u64, field: u64| phoff + elf::PHDR64_SIZE as u64 * loads[i as usize] + field;
    struct Load {
        offset: u64,
        vaddr: u64,
        filesz: u64,
        memsz: u64,
    }
    let load = |i: u64| {
        let p = &parsed.segments[loads[i as usize] as usize];
        Load {
            offset: p.offset,
            vaddr: p.vaddr,
            filesz: p.filesz,
            memsz: p.memsz,
        }
    };
    let (text, rodata, data) = (&load(0), &load(1), &load(2));
    // The cases rely on this layout: the text from the first page on, then
    // less than a page of rodata on a page of its own, then the data, the
    // C library's environ (crt0 sets it), all .bss, on a page of its own.
    assert_eq!(text.vaddr, USER_TEXT);
    assert!(rodata.vaddr >= text.vaddr + text.memsz && rodata.vaddr.is_multiple_of(PAGE));
    assert!(rodata.memsz < PAGE);
    assert_eq!(rodata.filesz, rodata.memsz);
    assert!(data.vaddr >= rodata.vaddr + rodata.memsz && data.vaddr.is_multiple_of(PAGE));
    assert!(data.memsz < PAGE);
    assert_eq!(data.filesz, 0);
    // hello reaches its rodata and its data pc-relatively
    // (-mcmodel=medany), so it still runs with every segment moved by the
    // same amount: here, so that its data, given a whole page, ends right
    // at USER_LOAD_TOP.
    let high_data = USER_LOAD_TOP - PAGE;
    let high_text = high_data - (data.vaddr - text.vaddr);
    let high_rodata = high_text + (rodata.vaddr - text.vaddr);
    let entry = parsed.header.entry;
    let moved_high = |data_memsz: u64| -> Vec<(u64, [u8; 8])> {
        vec![
            (E_ENTRY, (entry - text.vaddr + high_text).to_le_bytes()),
            (ph(0, P_VADDR), high_text.to_le_bytes()),
            (ph(1, P_VADDR), high_rodata.to_le_bytes()),
            (ph(2, P_VADDR), high_data.to_le_bytes()),
            (ph(2, P_MEMSZ), data_memsz.to_le_bytes()),
        ]
    };
    let (at_top, past_top) = (moved_high(PAGE), moved_high(PAGE + 1));
    let at_top: Vec<Patch<'_>> = at_top.iter().map(|(o, b)| (*o, &b[..])).collect();
    let past_top: Vec<Patch<'_>> = past_top.iter().map(|(o, b)| (*o, &b[..])).collect();

    let (tx, rx) = std::sync::mpsc::channel();
    let (mut m, elf) = load_committed(Some(rx));
    let image = image_pa(sym(&elf, "user_elf_hello"));
    assert_eq!(
        m.cpu.bus.peek(image),
        Some(u32::from_le_bytes(hello.elf[..4].try_into().unwrap())),
        "the kernel embeds what kernel/user builds"
    );
    step_to_prompt(&mut m, 1);
    let before = spare_pages(&m, &elf);

    let kernel_addr = 0xffff_ffff_8000_0000u64.to_le_bytes();
    let zero = 0u64.to_le_bytes();
    let (load_top, below_stack) = (USER_LOAD_TOP.to_le_bytes(), (USER_STACK - PAGE).to_le_bytes());
    let straddle = (USER_LOAD_TOP - 0x10).to_le_bytes();
    let wraps = 0xffff_ffff_ffff_f000u64.to_le_bytes();
    let (phoff_past, phoff_cut) = ((size + 4).to_le_bytes(), (size - 10).to_le_bytes());
    let big_file = 0x10_0000u64.to_le_bytes();
    let past_file = (size - rodata.offset + 1).to_le_bytes();
    let over_mem = (rodata.memsz + 1).to_le_bytes();
    let text_va = text.vaddr.to_le_bytes();
    let rodata_va = rodata.vaddr.to_le_bytes();
    let misaligned = (text.vaddr + 2).to_le_bytes();
    // What `hello` must print: the loader's reason, or `None` if it runs.
    let cases: &[(Option<&str>, &[Patch<'_>])] = &[
        (Some("not an ELF file"), &[(0, b"\x7fELG")]),
        // ELFCLASS32: an RV32 program, which no compat mode runs.
        (Some("not a 64-bit little-endian RISC-V file"), &[(4, &[1])]),
        (Some("not a 64-bit little-endian RISC-V file"), &[(5, &[2])]),
        (
            Some("not a 64-bit little-endian RISC-V file"),
            &[(E_MACHINE, &[62, 0])],
        ),
        (Some("not an executable"), &[(E_TYPE, &[1, 0])]),
        (Some("bad program headers"), &[(E_PHENTSIZE, &[32, 0])]),
        (Some("bad program headers"), &[(E_PHNUM, &[0xff, 0xff])]),
        // Program headers past the end of the file, and partly outside it.
        (Some("bad program headers"), &[(E_PHOFF, &phoff_past)]),
        (Some("bad program headers"), &[(E_PHOFF, &phoff_cut)]),
        (
            Some("segment outside user space"),
            &[(ph(0, P_VADDR), &kernel_addr)],
        ),
        (Some("segment outside user space"), &[(ph(0, P_VADDR), &zero)]),
        // The guard page under the stack's limit, and the stack itself.
        (Some("segment outside user space"), &[(ph(1, P_VADDR), &load_top)]),
        (
            Some("segment outside user space"),
            &[(ph(1, P_VADDR), &below_stack)],
        ),
        // Starting below USER_LOAD_TOP but ending above it.
        (Some("segment outside user space"), &[(ph(1, P_VADDR), &straddle)]),
        // A size that wraps around the address space.
        (Some("segment outside user space"), &[(ph(1, P_MEMSZ), &wraps)]),
        // Ending right at USER_LOAD_TOP is fine, one byte further is not.
        (None, &at_top),
        (Some("segment outside user space"), &past_top),
        (Some("segment outside the file"), &[(ph(0, P_OFFSET), &wraps)]),
        (Some("segment outside the file"), &[(ph(0, P_FILESZ), &big_file)]),
        // p_filesz <= p_memsz, but the file bytes run past its end.
        (
            Some("segment outside the file"),
            &[(ph(1, P_FILESZ), &past_file), (ph(1, P_MEMSZ), &past_file)],
        ),
        // p_filesz > p_memsz, with the file bytes inside it.
        (Some("segment outside the file"), &[(ph(1, P_FILESZ), &over_mem)]),
        (Some("segment not readable"), &[(ph(0, P_FLAGS), &[1, 0, 0, 0])]),
        (Some("segments share a page"), &[(ph(1, P_VADDR), &text_va)]),
        (
            Some("entry point not in an executable segment"),
            &[(E_ENTRY, &rodata_va)],
        ),
        (
            Some("entry point not in an executable segment"),
            &[(E_ENTRY, &kernel_addr)],
        ),
        // An empty segment is skipped: here the text, so there's no code.
        (
            Some("entry point not in an executable segment"),
            &[(ph(0, P_MEMSZ), &zero)],
        ),
        // In the text, but not on an instruction.
        (Some("misaligned entry point"), &[(E_ENTRY, &misaligned)]),
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
        assert_eq!(spare_pages(&m, &elf), before, "case {n} leaked");
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
fn an_elf32_program_is_refused() {
    // The kernel loads ELF64 executables only, with no compat mode for RV32
    // ones (spec, "Processes", "Loading"): clang's RV32 executable of
    // crates/elf/tests/llvm, an ELF32 file in /bin, does not start, and
    // hello runs.
    let prog32 = std::fs::read(repo_root().join("crates/elf/tests/llvm/rv32/prog.elf")).unwrap();
    assert_eq!(prog32[4], 1, "not ELFCLASS32");
    let out = run(
        Setup {
            disks: vec![root_disk(4096, &[("/bin/prog32", &prog32)])],
            ..Setup::default()
        },
        b"prog32\nhello\nexit 0\n",
    );
    assert_eq!(
        command_output(&out, "prog32"),
        [
            "spawn: /bin/prog32: not a 64-bit little-endian RISC-V file",
            "sh: prog32: not an executable"
        ]
    );
    assert_eq!(command_output(&out, "hello"), ["hello from user mode"]);
}

// ---- Threads, files and sockets ----

#[test]
fn threads_and_futexes_on_one_and_four_harts() {
    // threadtest: threads, joins, futexes, mutexes and spinlocks, sbrk and
    // pipes from several threads, a thread's fault ending its process.
    for harts in [1, 4] {
        let out = run(
            Setup {
                harts,
                ..Setup::default()
            },
            b"threadtest\nthreads\nexit 0\n",
        );
        let lines = command_output(&out, "threadtest");
        assert!(
            lines[0].starts_with("pid ") && lines[0].ends_with(" (threadtest) killed: store page fault"),
            "{out}"
        );
        all_checks_pass(&out, "threadtest", "threadtest");
        assert_eq!(
            command_output(&out, "threads"),
            ["threads: 4 threads, 500 rounds: counter 2000, spun 2000: ok"],
            "{harts} harts"
        );
    }
}

#[test]
fn files_pipes_and_64_bit_offsets_on_a_disk() {
    // filetest on the root filesystem: every file system call and its
    // errors, and lseek's 64-bit off_t (an offset past the end, past the
    // largest file or LONG_MAX fails with -EINVAL whatever its low 32 bits)
    // and stat's 24 bytes.
    let out = run(
        Setup {
            disks: vec![root_disk(4096, &[])],
            ..Setup::default()
        },
        b"filetest\nexit 0\n",
    );
    all_checks_pass(&out, "filetest", "filetest");
}

#[test]
fn sockets_over_loopback() {
    // socktest: every socket call and its errors, kernel pointers among
    // them (the kernel image's address).
    let out = run(
        Setup {
            nets: vec![NetConfig::user(Vec::new())],
            ..Setup::default()
        },
        b"socktest\nexit 0\n",
    );
    assert!(all_checks_pass(&out, "socktest", "socktest") > 200, "{out}");
}

/// A program whose .bss is above 4 GiB, so its heap starts there (the page
/// after its last segment, as Linux's brk follows the bss): sbrk must
/// return the 64-bit addresses whole. Exits 0 and prints `high: ok` if
/// sbrk(0) is 0x1_0000_1000, sbrk(8192) returns that and maps two pages
/// that take a store, sbrk(0) is then 0x1_0000_3000 and sbrk(-8192) gives
/// that back; else exits with the number of the check that failed.
const HIGH: &str = "
    .equ SYS_exit, 1
    .equ SYS_write, 2
    .equ SYS_sbrk, 9
    .text
    .globl _start
_start:
    li    s0, 1
    li    a0, 0
    li    a7, SYS_sbrk
    ecall
    li    s1, 0x100001000
    bne   a0, s1, fail
    li    s0, 2
    li    a0, 8192
    li    a7, SYS_sbrk
    ecall
    bne   a0, s1, fail
    li    s0, 3
    li    t0, 0x100002ff8
    li    t1, 0x123456789
    sd    t1, 0(t0)
    ld    t2, 0(t0)
    bne   t1, t2, fail
    li    s0, 4
    li    a0, 0
    li    a7, SYS_sbrk
    ecall
    li    s1, 0x100003000
    bne   a0, s1, fail
    li    s0, 5
    li    a0, -8192
    li    a7, SYS_sbrk
    ecall
    bne   a0, s1, fail
    li    a0, 1
    la    a1, ok
    li    a2, 9
    li    a7, SYS_write
    ecall
    li    s0, 0
fail:
    mv    a0, s0
    li    a7, SYS_exit
    ecall
ok:
    .ascii \"high: ok\\n\"
    .bss
    .zero 4096
";

/// Its linker script: the code at 0x1000, as user.ld puts it, the .bss at
/// 4 GiB.
const HIGH_LD: &str = "ENTRY(_start)
SECTIONS
{
    . = 0x1000;
    .text : { *(.text*) }
    . = 0x100000000;
    .bss : { *(.bss*) }
}
";

#[test]
fn the_calls_take_and_return_64_bit_values() {
    // sbrk above 4 GiB, from a program built here (merlin-as, merlin-ld)
    // and run from the disk; lseek's and stat's widths are filetest's
    // (files_pipes_and_64_bit_offsets_on_a_disk).
    let obj = xtask::toolchain::assemble("high.s", HIGH).unwrap();
    let input = merlin_ld::Input {
        name: "high.o".into(),
        data: obj,
    };
    let elf = xtask::toolchain::link_script("high.ld", HIGH_LD.into(), &[input], "high", false).unwrap();
    let parsed = elf::Elf::parse(&elf).unwrap();
    assert!(
        parsed.segments.iter().any(|s| s.vaddr == 0x1_0000_0000),
        "no segment at 4 GiB"
    );
    let out = run(
        Setup {
            disks: vec![root_disk(4096, &[("/bin/high", &elf)])],
            ..Setup::default()
        },
        b"high\nexit 0\n",
    );
    assert_eq!(command_output(&out, "high"), ["high: ok"]);
}
