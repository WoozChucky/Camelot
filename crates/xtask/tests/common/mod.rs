//! What the kernel tests share (kernel.rs, smp.rs, threads.rs, fs.rs,
//! processes.rs, sbi.rs): where the kernel image is and how a kernel pointer
//! reaches RAM, the kernel's layouts they read, loading the committed kernel,
//! running shell commands one step at a time, and the watcher that checks no
//! hart enters U-mode holding a lock.
//!
//! The kernel is a higher-half kernel (kernel/kernel.lds.S): linked at
//! [`KERNEL_LINK_ADDR`] and loaded at [`LOAD_ADDR`], after the firmware's 2
//! MiB, so a symbol's address is not where its bytes are in RAM:
//! [`image_pa`] is. A pointer the kernel computes at run time (into the
//! direct map, a kernel stack) is found through the hart's page table
//! ([`kpa`]), which maps the kernel's half in every address space.
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

/// The checks `maptest` makes (kernel/user/maptest.c), and `maptest
/// threads`: each must run every one.
pub const MAPTEST_CHECKS: u32 = 107;
pub const MAPTEST_THREADS_CHECKS: u32 = 15;

/// Where the kernel image is linked (its symbols' addresses) and where it is
/// loaded (where its bytes are in RAM).
pub const KERNEL_LINK_ADDR: u64 = 0xffff_ffff_8000_0000;
pub const LOAD_ADDR: u64 = 0x8020_0000;

/// The physical address of `va`, an address in the kernel image.
pub fn image_pa(va: u64) -> u64 {
    assert!(va >= KERNEL_LINK_ADDR, "{va:#x} is not in the kernel image");
    va - KERNEL_LINK_ADDR + LOAD_ADDR
}

/// The physical address of kernel pointer `va`: through the hart's page
/// table once paging is on (every address space maps the kernel's half),
/// else in the image.
pub fn kpa(cpu: &Cpu, va: u64) -> u64 {
    let paged = cpu.mode != Mode::M && cpu.csr.satp >> 60 != 0;
    if paged {
        cpu.debug_translate(&cpu.bus, va)
            .unwrap_or_else(|| panic!("{va:#x} is not mapped"))
    } else {
        image_pa(va)
    }
}

/// The u32 at kernel pointer `va`.
pub fn peek(cpu: &Cpu, va: u64) -> u32 {
    cpu.bus.peek(kpa(cpu, va)).unwrap()
}

/// The u64 at kernel pointer `va`.
pub fn peek64(cpu: &Cpu, va: u64) -> u64 {
    let pa = kpa(cpu, va);
    u64::from(cpu.bus.peek(pa).unwrap()) | u64::from(cpu.bus.peek(pa + 4).unwrap()) << 32
}

/// The byte at kernel pointer `va`.
pub fn peek_byte(cpu: &Cpu, va: u64) -> u8 {
    cpu.bus.read_mem(kpa(cpu, va), 1).unwrap() as u8
}

// Kernel layouts, from the headers named in each comment.

/// `sizeof(struct cpu)` (`CPU_SIZE` in kernel/include/spinlock.h): hart `h`'s
/// entry is at `cpus + h * CPU_SIZE`.
pub const CPU_SIZE: u64 = 168;
/// Offsets in `struct cpu`: the running thread's process, the push_off()
/// depth, interrupts as push_off() found them, the hart's id, the
/// scheduler's context, the page table in satp and the running thread. A
/// pointer after a u32 starts on the next 8-byte boundary.
pub const CPU_PROC: u64 = 0;
pub const CPU_NOFF: u64 = 8;
pub const CPU_INTENA: u64 = 12;
pub const CPU_HARTID: u64 = 16;
pub const CPU_CONTEXT: u64 = 24;
pub const CPU_PAGETABLE: u64 = 136;
pub const CPU_THREAD: u64 = 160;
/// Offset of the holding cpu in `struct spinlock`.
pub const LOCK_CPU: u64 = 8;
/// Offsets in `struct proc` (kernel/include/proc.h): its pid, its name (16
/// bytes), its parent and its page table.
pub const PROC_PID: u64 = 4;
pub const PROC_NAME: u64 = 8;
pub const PROC_PARENT: u64 = 24;
pub const PROC_PAGETABLE: u64 = 40;

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

/// The machine the kernel tests boot the kernel on, with `harts` harts:
/// Camelot's firmware and the RAM `cargo xtask kernel --run` gives it
/// (`xtask::kernel::machine_config`).
pub fn machine_config(harts: usize) -> MachineConfig {
    MachineConfig {
        harts,
        ..xtask::kernel::machine_config()
    }
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
    let mut m = Machine::with_backends(machine_config(harts), disks).unwrap();
    m.cpu.bus.uart.capture = true;
    m.load_elf(&file).unwrap();
    m.set_uart_input(input);
    (m, elf)
}

/// [`load_with`] without disks.
pub fn load(harts: usize, input: Receiver<u8>) -> (Machine, grail::elf::Elf) {
    load_with(harts, input, Vec::new())
}

/// `out` with what changes from run to run masked: `xcal.dbg`'s pc
/// (`xtask::kernel::mask_dbg_pc`), and the bytes of kernel stack the
/// deepest use took (`kstack: N of 16384 bytes used at most`).
pub fn mask(out: &str) -> String {
    xtask::kernel::mask_dbg_pc(out)
        .split_inclusive('\n')
        .map(|line| match line.strip_prefix("kstack: ") {
            Some(rest) if rest.contains(" of ") => {
                format!("kstack: N{}", &rest[rest.find(" of ").unwrap()..])
            }
            _ => line.to_string(),
        })
        .collect()
}

pub fn printed(m: &Machine) -> String {
    String::from_utf8_lossy(&m.cpu.bus.uart.captured).into_owned()
}

/// The address of symbol `name`: in the kernel image, a link address.
pub fn sym(elf: &grail::elf::Elf, name: &str) -> u64 {
    elf.symbol(name).unwrap_or_else(|| panic!("no symbol {name}"))
}

/// The addresses of function `name` in the committed kernel.
pub fn function(name: &str) -> Range<u64> {
    let file = committed_elf();
    let s = elf::Elf::parse(&file)
        .unwrap()
        .symbols()
        .unwrap()
        .into_iter()
        .find(|s| s.name == name)
        .unwrap_or_else(|| panic!("no symbol {name}"));
    assert!(s.size > 0, "{name} has no size");
    s.value..s.value + s.size
}

/// The name of the process whose `struct proc` is at `p`.
pub fn proc_name(cpu: &Cpu, p: u64) -> String {
    let bytes: Vec<u8> = (0..16)
        .map(|i| peek_byte(cpu, p + PROC_NAME + i))
        .take_while(|&b| b != 0)
        .collect();
    String::from_utf8(bytes).unwrap()
}

/// Pages free in the allocator (`nfree` in kernel/kalloc.c, a u32).
pub fn free_pages(m: &Machine, elf: &grail::elf::Elf) -> u32 {
    m.cpu.bus.peek(image_pa(sym(elf, "nfree"))).unwrap()
}

/// The pages of a kernel stack (`KSTACK_SIZE`, kernel/include/vm.h).
pub const KSTACK_PAGES: u32 = 4;
/// The freed kernel stacks each hart keeps mapped for its next thread
/// (`NR_CACHED_STACKS`, kernel/include/vm.h), and the harts there can be
/// (`NCPU`, kernel/include/spinlock.h).
pub const NR_CACHED_STACKS: u64 = 2;
pub const NCPU: u64 = 8;

/// The pages no process holds but the first, the shell, while it waits for
/// a line: the free ones ([`free_pages`]), with the kernel stacks the harts
/// keep cached (vm.c's `kstack_cached`: a cache, filled as threads end), and
/// the shell's own user pages and page tables, which grow on demand (its
/// stack, on its first commands). Every page a command's processes took is
/// back once this is what it was before the command.
pub fn spare_pages(m: &Machine, elf: &grail::elf::Elf) -> u32 {
    let cached_at = sym(elf, "kstack_cached");
    let cached = (0..NCPU * NR_CACHED_STACKS)
        .filter(|i| peek64(&m.cpu, cached_at + 8 * i) != 0)
        .count() as u32;
    let init = peek64(&m.cpu, sym(elf, "init_proc"));
    assert_ne!(init, 0, "the shell has not started");
    let root = kpa(&m.cpu, peek64(&m.cpu, init + PROC_PAGETABLE));
    free_pages(m, elf) + KSTACK_PAGES * cached + user_pages_under(m, root, 0..256)
}

/// The pages under root entries `entries` of the page table at physical
/// address `table`, its leaves' pages and the tables below it, whatever the
/// paging mode (a leaf is a PTE with R, W or X; the walk stops at 4 KiB
/// pages, three levels below the root at most).
fn user_pages_under(m: &Machine, table: u64, entries: Range<u64>) -> u32 {
    let mut pages = 0;
    for i in entries {
        let pa = table + 8 * i;
        let pte = u64::from(m.cpu.bus.peek(pa).unwrap()) | u64::from(m.cpu.bus.peek(pa + 4).unwrap()) << 32;
        if pte & 1 == 0 {
            continue;
        }
        pages += 1;
        if pte & 0b1110 == 0 {
            pages += user_pages_under(m, (pte >> 10 & 0xfff_ffff_ffff) << 12, 0..512);
        }
    }
    pages
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

/// A cell of the framebuffer console's screen (kernel/fbcon.c), read back
/// from the display's pixels by [`screen`]: its character and colours, as
/// 0x00RRGGBB.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScreenCell {
    pub ch: char,
    pub fg: u32,
    pub bg: u32,
}

/// The framebuffer console's grid.
pub const SCREEN_COLS: u32 = 80;
pub const SCREEN_ROWS: u32 = 30;

/// The framebuffer console's 80x30 cells as the display's VRAM shows them:
/// each 8x16 cell's pixels matched against the console's font
/// (`xtask::font`), with either of its two colours as the ink, so that
/// reverse video and the cursor read too. A cell of one colour is a space
/// (`fg` and `bg` that colour); one that is no glyph of printable ASCII is
/// U+FFFD.
pub fn screen(m: &Machine) -> Vec<Vec<ScreenCell>> {
    let font = xtask::font::psf2(&repo_root()).unwrap();
    let font = xtask::font::parse_psf2(&font).unwrap();
    let glyphs: std::collections::HashMap<Vec<u8>, char> = (0x21..=0x7e)
        .map(|c| (font.glyph(c).to_vec(), char::from(c as u8)))
        .collect();
    let fb = &m.cpu.bus.fb;
    let cell = |row: u32, col: u32| {
        let px = |x: u32, y: u32| fb.pixel(col * 8 + x, row * 16 + y);
        let first = px(0, 0);
        let other = (0..16)
            .flat_map(|y| (0..8).map(move |x| (x, y)))
            .map(|(x, y)| px(x, y))
            .find(|&p| p != first);
        let Some(other) = other else {
            return ScreenCell {
                ch: ' ',
                fg: first,
                bg: first,
            };
        };
        for (fg, bg) in [(first, other), (other, first)] {
            let bits: Vec<u8> = (0..16)
                .map(|y| (0..8).fold(0u8, |b, x| b << 1 | u8::from(px(x, y) == fg)))
                .collect();
            if let Some(&ch) = glyphs.get(&bits) {
                return ScreenCell { ch, fg, bg };
            }
        }
        ScreenCell {
            ch: '\u{fffd}',
            fg: first,
            bg: other,
        }
    };
    (0..SCREEN_ROWS)
        .map(|r| (0..SCREEN_COLS).map(|c| cell(r, c)).collect())
        .collect()
}

/// [`screen`]'s characters: a string a row, without its trailing spaces.
pub fn screen_text(m: &Machine) -> Vec<String> {
    screen(m)
        .iter()
        .map(|row| {
            row.iter()
                .map(|c| c.ch)
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect()
}

/// The lines the console shows for `out`, what the UART printed: each line
/// cut into rows of 80 columns, the last 30 rows (the last one is where the
/// cursor is, and may be empty). Text without tabs or escape sequences.
pub fn screen_rows_of(out: &str) -> Vec<String> {
    let mut rows = Vec::new();
    for line in out.replace('\r', "").split('\n') {
        let chars: Vec<char> = line.chars().collect();
        if chars.is_empty() {
            rows.push(String::new());
        }
        for chunk in chars.chunks(SCREEN_COLS as usize) {
            rows.push(chunk.iter().collect::<String>().trim_end().to_string());
        }
    }
    let skip = rows.len().saturating_sub(SCREEN_ROWS as usize);
    rows.split_off(skip)
}

/// Steps `m` until the display shows a new frame: the console's flush on
/// the next clock tick after it drew (fbcon.h).
pub fn step_to_frame(m: &mut Machine) {
    let before = m.cpu.bus.fb.shown();
    for _ in 0..RUN_MAX_STEPS {
        if m.cpu.bus.fb.shown() != before {
            return;
        }
        m.cpu.step();
    }
    panic!("no new frame:\n{}", printed(m));
}

/// Watches every hart's locking state: whenever a hart enters U-mode, it
/// holds no spinlock (no named lock's cpu is its own) and has no push_off()
/// pending. Other harts may hold locks meanwhile. A lock or a push_off
/// leaked on some path shows up here long before the deadlock it leads to.
pub struct LockWatch {
    cpus: u64,
    locks: Vec<(&'static str, u64)>,
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
            let me = self.cpus + u64::from(cpu.id) * CPU_SIZE;
            assert_eq!(
                cpu.bus.peek(image_pa(me + CPU_NOFF)),
                Some(0),
                "push_off() depth on hart {h} in U-mode at {:#x}",
                cpu.pc
            );
            for &(name, lock) in &self.locks {
                let pa = image_pa(lock + LOCK_CPU);
                let holder =
                    u64::from(cpu.bus.peek(pa).unwrap()) | u64::from(cpu.bus.peek(pa + 4).unwrap()) << 32;
                assert_ne!(holder, me, "{name} held by hart {h} in U-mode at {:#x}", cpu.pc);
            }
        }
        self.was_user[h] = is_user;
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

    /// A word on the stack: 8 bytes, a pointer's.
    const WORD: u64 = 8;

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

    /// The little-endian word at `va`.
    fn word(m: &Machine, va: u64) -> u64 {
        let b = read(m, va, WORD as usize);
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
    /// the current hart's sp: every register zero but sp (a0, the psABI's
    /// atexit function, is 0 too); at sp, 16-byte aligned, argc, then argv,
    /// a NULL, envp, a NULL, and the auxiliary vector in the kernel's
    /// order, every entry 8 bytes; the strings, argv's in order, then
    /// envp's, then the path and a zero word at the very top, as Linux's
    /// create_elf_tables lays them out; the 16 random bytes between the
    /// strings and the vector, aligned.
    pub fn check(m: &Machine, user_top: u64, elf: &[u8], path: &str, argv: &[&str], envp: &[&str]) {
        let w = |va: u64| word(m, va);
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
        at += WORD;
        // argv's pointers and a NULL, then envp's and a NULL: the strings
        // they point at, in order.
        let mut ptrs = Vec::new();
        for (strings, what) in [(argv, "argv"), (envp, "envp")] {
            for want in strings {
                let p = w(at);
                assert_eq!(&string(m, p), want, "{what}");
                ptrs.push(p);
                at += WORD;
            }
            assert_eq!(w(at), 0, "{what}'s NULL");
            at += WORD;
        }
        let mut auxv = Vec::new();
        loop {
            let (ty, val) = (w(at), w(at + WORD));
            auxv.push((ty, val));
            at += 2 * WORD;
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
        assert_eq!(execfn + path.len() as u64 + 1 + WORD, user_top);
        assert_eq!(w(user_top - WORD), 0, "the end marker");
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

/// The shell's environment (issue #216; kernel/user/sh.c, "Variables and
/// the environment"): a session typed without a disk, and what each command
/// must print. `env` (kernel/user/env.c) prints the environment the shell
/// gave it, after its own changes, and `printenv` (kernel/user/printenv.c)
/// the values getenv finds in it.
pub mod environment {
    use super::PROMPT;

    /// The session, one command a line. The last one replaces the shell,
    /// the first process, so the machine stops when it ends.
    pub const INPUT: &[u8] = b"env\n\
        X=1 env -u TERM\n\
        X=1 printenv X HOME\n\
        printenv X\n\
        export Y=2\n\
        Z=3\n\
        env -u HOME\n\
        HOME=/nope cd\n\
        export Z\n\
        export\n\
        export 1A\n\
        unset Y\n\
        Y=4 env -u TERM -u HOME\n\
        env -i A=1 B=two env\n\
        env -i\n\
        FOO=bar exec env -u TERM -u HOME -u Z\n";

    /// Checks the session's output, `out` from the shell's banner on, with
    /// `output`, the test file's command_output: the lines a command
    /// printed.
    pub fn check(out: &str, output: for<'a> fn(&'a str, &str) -> Vec<&'a str>) {
        // The first process's environment, Linux's for init, which the
        // shell passes on.
        assert_eq!(output(out, "env"), ["HOME=/", "TERM=linux"], "{out}");
        // An assignment before a program is in its environment alone.
        assert_eq!(output(out, "X=1 env -u TERM"), ["HOME=/", "X=1"], "{out}");
        // printenv: getenv of each name.
        assert_eq!(output(out, "X=1 printenv X HOME"), ["1", "/"], "{out}");
        assert_eq!(output(out, "printenv X"), ["printenv: exit 1"], "{out}");
        // export: in every program's from then on; a plain assignment is
        // the shell's own.
        assert_eq!(output(out, "env -u HOME"), ["TERM=linux", "Y=2"], "{out}");
        // A regular built-in sees the assignment, and leaves HOME as it
        // was: cd goes to HOME (no disk here, so nowhere).
        assert_eq!(
            output(out, "HOME=/nope cd"),
            ["sh: cd: /nope: no such file or directory"],
            "{out}"
        );
        assert_eq!(
            output(out, "export"),
            ["export HOME=/", "export TERM=linux", "export Y=2", "export Z=3"],
            "{out}"
        );
        assert_eq!(
            output(out, "export 1A"),
            ["sh: export: 1A: bad variable name"],
            "{out}"
        );
        // unset takes Y out; an assignment of a name that is no variable
        // comes after the others.
        assert_eq!(output(out, "Y=4 env -u TERM -u HOME"), ["Z=3", "Y=4"], "{out}");
        assert_eq!(output(out, "env -i A=1 B=two env"), ["A=1", "B=two"], "{out}");
        assert!(output(out, "env -i").is_empty(), "{out}");
        // Before exec, an assignment is exported to the program that
        // replaces the shell.
        let last = format!("{PROMPT}FOO=bar exec env -u TERM -u HOME -u Z\nFOO=bar\n");
        assert!(out.ends_with(&last), "{out}");
    }
}
