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
