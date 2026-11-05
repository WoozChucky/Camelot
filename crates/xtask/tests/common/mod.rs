//! What the kernel tests that run the shell on several harts share
//! (kernel.rs, smp.rs, threads.rs): the kernel's layouts they read, loading
//! the committed kernel, running shell commands one step at a time, and
//! the watcher that checks no hart enters U-mode holding a lock.
//!
//! Each test file that uses it has `mod common;`, so each compiles its own
//! copy, and not every file uses every item.
#![allow(dead_code)]

use grail::block::BlockBackend;
use grail::cpu::{Cpu, Mode};
use grail::{Machine, MachineConfig, RunResult};
use std::ops::Range;
use std::sync::mpsc::{Receiver, Sender};
use xtask::repo_root;

pub const PROMPT: &str = "$ ";

/// Enough steps for a boot and a few programs. Each step runs every hart
/// one instruction.
pub const RUN_MAX_STEPS: u64 = 150_000_000;

// Kernel layouts, from the headers named in each comment.

/// `sizeof(struct cpu)` (`CPU_SIZE` in kernel/include/spinlock.h): hart `h`'s
/// entry is at `cpus + h * CPU_SIZE`.
pub const CPU_SIZE: u32 = 92;
/// Offsets in `struct cpu`: the running thread's process, the push_off()
/// depth, the page table in satp and the running thread.
pub const CPU_PROC: u32 = 0;
pub const CPU_NOFF: u32 = 4;
pub const CPU_PAGETABLE: u32 = 72;
pub const CPU_THREAD: u32 = 88;
/// Offset of the holding cpu in `struct spinlock`.
pub const LOCK_CPU: u32 = 4;
/// Offsets in `struct proc` (kernel/include/proc.h): its name (16 bytes) and
/// its parent.
pub const PROC_NAME: u32 = 8;
pub const PROC_PARENT: u32 = 24;

/// The same layouts in the RV64 kernel (`kernel/kernel-rv64.elf`, #154),
/// where pointers and saved registers are 8 bytes and a pointer after a u32
/// starts on the next 8-byte boundary (the `#if __riscv_xlen == 64` sides of
/// kernel/include/spinlock.h, whose `_Static_assert`s check them).
pub mod rv64 {
    /// `sizeof(struct cpu)`: hart `h`'s entry is at `cpus + h * CPU_SIZE`.
    pub const CPU_SIZE: u64 = 168;
    pub const CPU_PROC: u64 = 0;
    pub const CPU_NOFF: u64 = 8;
    pub const CPU_INTENA: u64 = 12;
    pub const CPU_HARTID: u64 = 16;
    pub const CPU_CONTEXT: u64 = 24;
    pub const CPU_PAGETABLE: u64 = 136;
    pub const CPU_THREAD: u64 = 160;
    /// Offset of the holding cpu in `struct spinlock`.
    pub const LOCK_CPU: u64 = 8;
}

/// The SBI's numbers (kernel/include/sbi.h, from the SBI spec v2.0): the
/// extensions and functions the kernel calls, the errors and the HSM states.
pub mod sbi {
    pub const EXT_BASE: u64 = 0x10;
    pub const EXT_TIME: u64 = 0x5449_4d45;
    pub const EXT_IPI: u64 = 0x73_5049;
    pub const EXT_HSM: u64 = 0x48_534d;
    pub const EXT_SRST: u64 = 0x5352_5354;
    pub const EXT_DBCN: u64 = 0x4442_434e;

    pub const BASE_GET_SPEC_VERSION: u64 = 0;
    pub const BASE_GET_IMPL_ID: u64 = 1;
    pub const BASE_GET_IMPL_VERSION: u64 = 2;
    pub const BASE_PROBE_EXT: u64 = 3;
    pub const BASE_GET_MVENDORID: u64 = 4;
    pub const BASE_GET_MARCHID: u64 = 5;
    pub const BASE_GET_MIMPID: u64 = 6;
    pub const TIME_SET_TIMER: u64 = 0;
    pub const IPI_SEND_IPI: u64 = 0;
    pub const HSM_HART_START: u64 = 0;
    pub const HSM_HART_STOP: u64 = 1;
    pub const HSM_HART_GET_STATUS: u64 = 2;
    pub const HSM_HART_SUSPEND: u64 = 3;
    pub const SRST_RESET: u64 = 0;
    pub const DBCN_WRITE: u64 = 0;
    pub const DBCN_READ: u64 = 1;
    pub const DBCN_WRITE_BYTE: u64 = 2;

    pub const SUCCESS: i64 = 0;
    pub const ERR_NOT_SUPPORTED: i64 = -2;
    pub const ERR_INVALID_PARAM: i64 = -3;
    pub const ERR_INVALID_ADDRESS: i64 = -5;
    pub const ERR_ALREADY_AVAILABLE: i64 = -6;

    pub const STATE_STARTED: i64 = 0;
    pub const STATE_STOPPED: i64 = 1;
    pub const STATE_SUSPENDED: i64 = 4;

    /// Version 2.0: the major number in bits 30:24.
    pub const SPEC_VERSION: i64 = 0x0200_0000;
    /// Camelot's SBI: "CMLT", unregistered, and its first version.
    pub const IMPL_ID: i64 = 0x434d_4c54;
    pub const IMPL_VERSION: i64 = 1;
}

/// The kernel's spinlocks that are variables of their own (the per-pipe,
/// per-disk and the log's live inside other structures).
pub const NAMED_LOCKS: [&str; 10] = [
    "kmem_lock",
    "uvm_lock",
    "proc_lock",
    "tickslock",
    "cons_lock",
    "uart_tx_lock",
    "ftable_lock",
    "bcache_lock",
    "itable_lock",
    "net_lock",
];

pub fn committed_elf() -> Vec<u8> {
    let path = repo_root().join("kernel/kernel.elf");
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// Loads the committed kernel on a machine of `harts` harts with `disks`,
/// the UART reading `input`.
pub fn load_with(
    harts: usize,
    input: Receiver<u8>,
    disks: Vec<Box<dyn BlockBackend>>,
) -> (Machine, grail::elf::Elf) {
    let file = committed_elf();
    let elf = grail::elf::parse(&file).unwrap();
    let mut m = Machine::with_backends(
        MachineConfig {
            harts,
            ..MachineConfig::default()
        },
        disks,
    )
    .unwrap();
    m.cpu.bus.uart.capture = true;
    m.load_elf(&file).unwrap();
    m.set_uart_input(input);
    (m, elf)
}

/// [`load_with`] without disks.
pub fn load(harts: usize, input: Receiver<u8>) -> (Machine, grail::elf::Elf) {
    load_with(harts, input, Vec::new())
}

pub fn printed(m: &Machine) -> String {
    String::from_utf8_lossy(&m.cpu.bus.uart.captured).into_owned()
}

pub fn sym(elf: &grail::elf::Elf, name: &str) -> u32 {
    elf.symbol(name).unwrap_or_else(|| panic!("no symbol {name}")) as u32
}

/// The addresses of function `name` in the committed kernel.
pub fn function(name: &str) -> Range<u32> {
    let file = committed_elf();
    let s = elf::Elf::parse(&file)
        .unwrap()
        .symbols()
        .unwrap()
        .into_iter()
        .find(|s| s.name == name)
        .unwrap_or_else(|| panic!("no symbol {name}"));
    assert!(s.size > 0, "{name} has no size");
    s.value as u32..(s.value + s.size) as u32
}

pub fn peek(cpu: &Cpu, addr: u32) -> u32 {
    cpu.bus.peek(addr.into()).unwrap()
}

pub fn peek_byte(bus: &grail::bus::Bus, addr: u32) -> u8 {
    (bus.peek((addr & !3).into()).unwrap() >> (8 * (addr & 3))) as u8
}

/// The name of the process whose `struct proc` is at `p`.
pub fn proc_name(cpu: &Cpu, p: u32) -> String {
    let bytes: Vec<u8> = (0..16)
        .map(|i| peek_byte(&cpu.bus, p + PROC_NAME + i))
        .take_while(|&b| b != 0)
        .collect();
    String::from_utf8(bytes).unwrap()
}

pub fn free_pages(m: &Machine, elf: &grail::elf::Elf) -> u32 {
    m.cpu.bus.peek(sym(elf, "nfree").into()).unwrap()
}

/// The lines between the last echo of `command` and the next prompt.
pub fn command_output<'a>(out: &'a str, command: &str) -> Vec<&'a str> {
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

/// Steps `m` until the shell has printed its first prompt.
pub fn step_to_prompt(m: &mut Machine) {
    for _ in 0..RUN_MAX_STEPS {
        if m.cpu.bus.uart.captured.ends_with(PROMPT.as_bytes()) {
            return;
        }
        m.cpu.step();
    }
    panic!("no prompt:\n{}", printed(m));
}

/// Types `line` and steps `m`, calling `each` before every hart's turn,
/// until the shell prompts again.
pub fn run_command(m: &mut Machine, tx: &Sender<u8>, line: &str, mut each: impl FnMut(&Cpu)) {
    for b in format!("{line}\n").bytes() {
        tx.send(b).unwrap();
    }
    let from = m.cpu.bus.uart.captured.len();
    let mut steps = 0;
    while !m.cpu.bus.uart.captured[from..].ends_with(PROMPT.as_bytes()) {
        assert!(steps < RUN_MAX_STEPS, "`{line}` never ended:\n{}", printed(m));
        m.cpu.step_traced(&mut each);
        steps += 1;
    }
}

/// Ends the shell and checks the machine stops with its status, 0.
pub fn exit_shell(m: &mut Machine, tx: &Sender<u8>) {
    for b in b"exit\n" {
        tx.send(*b).unwrap();
    }
    assert_eq!(
        m.run(Some(RUN_MAX_STEPS), |_| {}),
        RunResult::Exited(0),
        "{}",
        printed(m)
    );
}

/// Watches every hart's locking state: whenever a hart enters U-mode, it
/// holds no spinlock (no named lock's cpu is its own) and has no push_off()
/// pending. Other harts may hold locks meanwhile. A lock or a push_off
/// leaked on some path shows up here long before the deadlock it leads to.
pub struct LockWatch {
    cpus: u32,
    locks: Vec<(&'static str, u32)>,
    was_user: [bool; grail::MAX_HARTS],
}

impl LockWatch {
    pub fn new(elf: &grail::elf::Elf) -> LockWatch {
        LockWatch {
            cpus: sym(elf, "cpus"),
            locks: NAMED_LOCKS.iter().map(|&n| (n, sym(elf, n))).collect(),
            was_user: [false; grail::MAX_HARTS],
        }
    }

    /// Called before each hart's turn: checks the state on every entry to
    /// U-mode.
    pub fn step(&mut self, cpu: &Cpu) {
        let h = cpu.id as usize;
        let is_user = cpu.mode == Mode::U;
        if is_user && !self.was_user[h] {
            let me = self.cpus + cpu.id * CPU_SIZE;
            assert_eq!(
                cpu.bus.peek(u64::from(me + CPU_NOFF)),
                Some(0),
                "push_off() depth on hart {h} in U-mode at {:#x}",
                cpu.pc
            );
            for &(name, lock) in &self.locks {
                assert_ne!(
                    cpu.bus.peek(u64::from(lock + LOCK_CPU)),
                    Some(me),
                    "{name} held by hart {h} in U-mode at {:#x}",
                    cpu.pc
                );
            }
        }
        self.was_user[h] = is_user;
    }
}

/// The width of the kernel a test runs, for the tests that run on both
/// ([`width::on_both`]): the RV32 kernel and then the RV64 one (#156).
pub mod width {
    use std::cell::Cell;
    use xtask::kernel::Xlen;

    thread_local! {
        static XLEN: Cell<Xlen> = const { Cell::new(Xlen::Rv32) };
    }

    /// Runs `test` for RV32, then for RV64: [`get`] says which. A failure
    /// says which width failed: the panic is raised again with the width
    /// before its message.
    pub fn on_both(test: impl Fn()) {
        for xlen in xtask::kernel::XLENS {
            XLEN.with(|x| x.set(xlen));
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(&test));
            XLEN.with(|x| x.set(Xlen::Rv32));
            if let Err(payload) = result {
                let name = match xlen {
                    Xlen::Rv32 => "RV32",
                    Xlen::Rv64 => "RV64",
                };
                panic!("on {name}: {}", panic_message(&*payload));
            }
        }
    }

    /// What a panic said: its message, if it had one.
    fn panic_message(payload: &(dyn std::any::Any + Send)) -> &str {
        if let Some(s) = payload.downcast_ref::<&str>() {
            s
        } else if let Some(s) = payload.downcast_ref::<String>() {
            s
        } else {
            "(a panic without a message)"
        }
    }

    /// The width under test.
    pub fn get() -> Xlen {
        XLEN.with(|x| x.get())
    }

    /// The same, as Grail names it.
    pub fn machine() -> grail::Xlen {
        match get() {
            Xlen::Rv32 => grail::Xlen::Rv32,
            Xlen::Rv64 => grail::Xlen::Rv64,
        }
    }

    /// Where the RV64 kernel image is linked, and where it is loaded.
    const KERNEL_LINK_ADDR_RV64: u64 = 0xffff_ffff_8000_0000;
    const LOAD_ADDR: u64 = 0x8000_0000;

    /// The physical address of `va`, an address in the kernel image: the
    /// same on RV32; on RV64, linked in the upper half, its load address.
    pub fn image_pa(va: u64) -> u64 {
        if va >= KERNEL_LINK_ADDR_RV64 {
            va - KERNEL_LINK_ADDR_RV64 + LOAD_ADDR
        } else {
            va
        }
    }
}

/// A program's start, as the kernel puts it on the stack of a program it
/// starts (docs/camelot-syscalls.md, "Starting a program"; kernel/proc.c,
/// push_start), Linux's layout: read through the current hart's
/// translation, which must be the new program's, before it has run.
pub mod start {
    use grail::debug::Space;
    use grail::Machine;

    /// The auxiliary vector's types (kernel/include/syscall.h's `AT_*`,
    /// Linux's values).
    pub const AT_NULL: u64 = 0;
    pub const AT_PHDR: u64 = 3;
    pub const AT_PHENT: u64 = 4;
    pub const AT_PHNUM: u64 = 5;
    pub const AT_PAGESZ: u64 = 6;
    pub const AT_BASE: u64 = 7;
    pub const AT_FLAGS: u64 = 8;
    pub const AT_ENTRY: u64 = 9;
    pub const AT_UID: u64 = 11;
    pub const AT_EUID: u64 = 12;
    pub const AT_GID: u64 = 13;
    pub const AT_EGID: u64 = 14;
    pub const AT_HWCAP: u64 = 16;
    pub const AT_CLKTCK: u64 = 17;
    pub const AT_SECURE: u64 = 23;
    pub const AT_RANDOM: u64 = 25;
    pub const AT_EXECFN: u64 = 31;

    /// `n` bytes at user address `va`.
    fn read(m: &Machine, va: u64, n: usize) -> Vec<u8> {
        let mut buf = vec![0; n];
        assert_eq!(
            m.read_memory(va, &mut buf, Space::Virtual),
            n,
            "{va:#x} is not mapped"
        );
        buf
    }

    /// The `size`-byte little-endian word at `va`.
    fn word(m: &Machine, va: u64, size: u64) -> u64 {
        let b = read(m, va, size as usize);
        b.iter().rev().fold(0, |v, &x| v << 8 | u64::from(x))
    }

    /// The NUL-terminated string at `va`.
    fn string(m: &Machine, va: u64) -> String {
        let mut s = Vec::new();
        for a in va.. {
            match read(m, a, 1)[0] {
                0 => break,
                c => s.push(c),
            }
        }
        String::from_utf8(s).unwrap()
    }

    /// The environment the kernel starts the first process with: Linux's
    /// for init (init/main.c's envp_init), kernel/exec.c's exec_init.
    pub const INIT_ENV: &[&str] = &["HOME=/", "TERM=linux"];

    /// Checks the start of program `elf`, run as `path` with `argv` and
    /// the environment `envp`, on the stack under `user_top` that ends at
    /// the current hart's sp, with `word_size`-byte pointers: every
    /// register zero but sp (a0, the psABI's atexit function, is 0 too); at
    /// sp, 16-byte aligned, argc, then argv, a NULL, envp, a NULL, and the
    /// auxiliary vector in the kernel's order; the strings, argv's in
    /// order, then envp's, then the path and a zero word at the very top,
    /// as Linux's create_elf_tables lays them out; the 16 random bytes
    /// between the strings and the vector, aligned.
    pub fn check(
        m: &Machine,
        word_size: u64,
        user_top: u64,
        elf: &[u8],
        path: &str,
        argv: &[&str],
        envp: &[&str],
    ) {
        let w = |va: u64| word(m, va, word_size);
        let regs = m.cpu.x;
        let sp = regs[2];
        for (r, &v) in regs.iter().enumerate() {
            if r != 2 {
                assert_eq!(v, 0, "x{r} is not zero");
            }
        }
        assert_eq!(sp % 16, 0, "sp {sp:#x}");
        assert!(sp < user_top && sp > user_top - 2048, "sp {sp:#x}");
        let mut at = sp;
        assert_eq!(w(at), argv.len() as u64, "argc");
        at += word_size;
        // argv's pointers and a NULL, then envp's and a NULL: the strings
        // they point at, in order.
        let mut ptrs = Vec::new();
        for (strings, what) in [(argv, "argv"), (envp, "envp")] {
            for want in strings {
                let p = w(at);
                assert_eq!(&string(m, p), want, "{what}");
                ptrs.push(p);
                at += word_size;
            }
            assert_eq!(w(at), 0, "{what}'s NULL");
            at += word_size;
        }
        let mut auxv = Vec::new();
        loop {
            let (ty, val) = (w(at), w(at + word_size));
            auxv.push((ty, val));
            at += 2 * word_size;
            if ty == AT_NULL {
                break;
            }
        }
        // The strings: contiguous, argv's then envp's, then the path, then
        // the marker.
        let mut s = ptrs[0];
        for (p, want) in ptrs.iter().zip(argv.iter().chain(envp)) {
            assert_eq!(*p, s);
            s += want.len() as u64 + 1;
        }
        let execfn = s;
        assert_eq!(string(m, execfn), path);
        assert_eq!(execfn + path.len() as u64 + 1 + word_size, user_top);
        assert_eq!(w(user_top - word_size), 0, "the end marker");
        // The random bytes: below the strings, above the vector.
        let random = auxv.iter().find(|a| a.0 == AT_RANDOM).unwrap().1;
        assert_eq!(random % 16, 0);
        assert!(random >= at && random + 16 <= ptrs[0], "random {random:#x}");
        assert!(read(m, random, 16).iter().any(|&b| b != 0));
        // The vector: what the ELF says of the program, and Camelot's
        // constants. AT_PHDR is where a PT_LOAD loads the headers (as
        // Linux's load_elf_binary finds it), 0 if none does.
        let e = elf::Elf::parse(elf).unwrap();
        let phdr = e
            .segments
            .iter()
            .filter(|s| s.ty == 1 && s.offset <= e.header.phoff && e.header.phoff < s.offset + s.filesz)
            .map(|s| e.header.phoff - s.offset + s.vaddr)
            .next_back()
            .unwrap_or(0);
        let hwcap: u64 = b"IMA".iter().map(|&c| 1 << (c - b'A')).sum();
        assert_eq!(
            auxv,
            [
                (AT_HWCAP, hwcap),
                (AT_PAGESZ, 4096),
                (AT_CLKTCK, 10),
                (AT_PHDR, phdr),
                (AT_PHENT, u64::from(e.header.phentsize)),
                (AT_PHNUM, u64::from(e.header.phnum)),
                (AT_BASE, 0),
                (AT_FLAGS, 0),
                (AT_ENTRY, e.header.entry),
                (AT_UID, 0),
                (AT_EUID, 0),
                (AT_GID, 0),
                (AT_EGID, 0),
                (AT_SECURE, 0),
                (AT_RANDOM, random),
                (AT_EXECFN, execfn),
                (AT_NULL, 0),
            ]
        );
    }
}
