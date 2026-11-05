//! RV64 processes (#155): `kernel/kernel-rv64.elf` running 64-bit programs
//! in the lower half of Sv48 (or Sv39), loaded from ELF64 files, with the
//! LP64 system call ABI. The same programs as the RV32 kernel's tests run
//! (kernel/user, built for RV64 by `xtask::userprogs::build_embedded_for`),
//! and pass the same way: the process, thread, futex, file, pipe, socket and
//! stack-growth checks; then what is RV64's own:
//!
//! - a negative result arrives sign-extended in all 64 bits of a0
//!   (`negative_results_arrive_sign_extended`);
//! - a pointer into the kernel's half of the address space, or between the
//!   halves, is refused with -EFAULT (`pointers_into_the_kernel_half_are_refused`);
//! - the widths are LP64's: `lseek`'s 64-bit `off_t`, `stat`'s 24 bytes,
//!   `sbrk`'s pointers above 4 GiB (`the_calls_take_and_return_64_bit_values`);
//! - a 32-bit program is refused, with no compat mode
//!   (`an_elf32_program_is_refused`);
//! - a program starts as Linux starts one, with 8-byte words
//!   (`a_program_starts_as_linux_starts_one`).

mod common;

use grail::{DiskConfig, Machine, MachineConfig, NetConfig, RunResult};
use xtask::kernel::{self, BOOT_MAX_STEPS};
use xtask::repo_root;

const SHELL_BANNER: &str = "Type help for the list of commands.\n";
const PROMPT: &str = "$ ";
const MIB: usize = 1 << 20;
/// Enough steps for a boot and the commands of a test.
const MAX_STEPS: u64 = 2 * BOOT_MAX_STEPS;

fn committed() -> Vec<u8> {
    let path = repo_root().join("kernel").join(kernel::KERNEL_ELF_RV64);
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// A machine for the RV64 kernel.
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

/// Boots the committed RV64 kernel on `setup`'s machine with `input` typed
/// on the UART (ending in `exit 0`), runs it until the shell exits, and
/// returns what was printed from the shell's banner on. The machine must
/// stop with status 0: no self-test failed, and the shell exited 0.
fn run(setup: Setup, input: &[u8]) -> String {
    let mut m = Machine::new(MachineConfig {
        harts: setup.harts,
        ram_size: setup.ram,
        xlen: grail::Xlen::Rv64,
        paging: setup.paging,
        disks: setup.disks,
        nets: setup.nets,
        ..MachineConfig::default()
    })
    .unwrap();
    m.cpu.bus.uart.capture = true;
    m.load_elf(&committed()).unwrap();
    m.set_uart_input(kernel::typed(input));
    let r = m.run(Some(MAX_STEPS), |_| {});
    let out = String::from_utf8_lossy(&m.cpu.bus.uart.captured).into_owned();
    assert_eq!(r, RunResult::Exited(0), "output:\n{out}");
    let at = out
        .find(SHELL_BANNER)
        .unwrap_or_else(|| panic!("the shell never started:\n{out}"));
    out[at..].to_string()
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

/// A disk with the root filesystem the RV64 kernel's tests use
/// (`xtask::fs::build_for`: the programs built for RV64 in /bin), `blocks`
/// blocks, and `extra` files besides.
fn rv64_root_disk(blocks: u32, extra: &[(&str, &[u8])]) -> DiskConfig {
    let root = repo_root();
    let opts = camelot_fs::mkfs::Options {
        size: blocks,
        ..camelot_fs::mkfs::Options::default()
    };
    let image = if extra.is_empty() {
        xtask::fs::build_for(&root, &opts, kernel::Xlen::Rv64).unwrap()
    } else {
        // build_for's files, then the extra ones.
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
        let merlin = xtask::userprogs::CCompiler::Merlin;
        for p in xtask::userprogs::build_all_for(&user, merlin, kernel::Xlen::Rv64).unwrap() {
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

#[test]
fn a_program_starts_as_linux_starts_one() {
    // The first instruction in U-mode, the shell's _start, with Linux's
    // start (docs/camelot-syscalls.md, "Starting a program"): sp at argc,
    // a long, then argv, envp and the auxiliary vector, 8 bytes each, under
    // USER_TOP, which depends on the paging mode. The programs it runs get
    // the same: systest checks its own start, and its children's after a
    // spawn and an exec (the_system_calls_do_what_the_document_says).
    let user = xtask::userprogs::user_dir(&repo_root());
    let sh = xtask::userprogs::build_named_for(&user, "sh", kernel::Xlen::Rv64).unwrap();
    for (paging, top) in [
        (grail::Paging::Sv48, 1u64 << 47),
        (grail::Paging::Sv39, 1u64 << 38),
    ] {
        let mut m = Machine::new(MachineConfig {
            ram_size: 16 * MIB,
            xlen: grail::Xlen::Rv64,
            paging,
            ..MachineConfig::default()
        })
        .unwrap();
        m.load_elf(&committed()).unwrap();
        let mut steps = 0;
        while m.cpu.mode != grail::cpu::Mode::U {
            assert!(steps < MAX_STEPS, "never reached U-mode");
            m.cpu.step();
            steps += 1;
        }
        assert_eq!(m.cpu.pc, 0x1000, "{paging:?}");
        common::start::check(&m, 8, top, &sh.elf, "/bin/sh", &["sh"]);
    }
}

#[test]
fn the_shell_runs_64_bit_programs() {
    // The shell is the first process, as on RV32: it runs programs by name,
    // with their arguments, in pipelines, and itself.
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
fn the_system_calls_do_what_the_document_says() {
    // systest: every process and descriptor call from a 64-bit program,
    // the RV64 checks among them (a negative result as a long, all of a7
    // the call number, sbrk by LONG_MIN).
    let out = run(Setup::default(), b"systest\nexit 0\n");
    assert!(all_checks_pass(&out, "systest", "systest") > 200, "{out}");
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
        // A load from the kernel image in U-mode: mapped, without U.
        let badload = command_output(&out, "badload");
        assert!(
            badload[0].ends_with(" (badload) killed: load page fault"),
            "{out}"
        );
        assert!(badload[1].ends_with("stval 0xffffffff80000000"), "{out}");
        assert!(
            badload[2].starts_with("  va 0xffffffff80000000 maps to pa 0x0000000080000000")
                && badload[2].ends_with(" DAG-X-RV"),
            "{out}"
        );
        assert_eq!(badload[3], "badload: exit -1");
    }
}

#[test]
fn the_c_library_does_what_user_h_says() {
    // libtest: strings, numbers (strtol's 64-bit LONG_MAX), snprintf (%ld of
    // 64-bit values, %p's 16 digits), malloc's 16-byte blocks, readers.
    let out = run(Setup::default(), b"libtest\nexit 0\n");
    assert!(all_checks_pass(&out, "libtest", "libtest") > 70, "{out}");
}

#[test]
fn the_user_stack_grows_on_demand_up_to_its_limit() {
    // spacetest's checks, a 6 MiB heap, and a stack overflow that reaches
    // the guard page under the stack's 8 MiB: USER_STACK_LIMIT - 4 KiB to
    // USER_STACK_LIMIT, below 2^47 in Sv48 and 2^38 in Sv39.
    for (paging, top) in [
        (grail::Paging::Sv48, 1u64 << 47),
        (grail::Paging::Sv39, 1u64 << 38),
    ] {
        let out = run(
            Setup {
                paging,
                ..Setup::default()
            },
            b"spacetest\nspacetest heap 6\nspacetest overflow\necho alive\nexit 0\n",
        );
        assert_eq!(all_checks_pass(&out, "spacetest", "spacetest"), 14, "{out}");
        assert_eq!(all_checks_pass(&out, "spacetest heap 6", "spacetest"), 7, "{out}");
        let overflow = command_output(&out, "spacetest overflow");
        assert!(
            overflow[0].ends_with("(spacetest) killed: store page fault"),
            "{out}"
        );
        let stval = overflow[1].rsplit("stval 0x").next().unwrap();
        let stval = u64::from_str_radix(stval.trim(), 16).unwrap();
        let limit = top - (8 << 20);
        assert!((limit - 4096..limit).contains(&stval), "{paging:?}: {out}");
        assert_eq!(command_output(&out, "echo alive"), ["alive"]);
    }
}

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
fn registers_survive_switches_and_system_calls() {
    // regs spins beside ding and dong and checks every register after
    // (exit 0); the shell reports the background ones.
    let out = run(Setup::default(), b"regs & ding & dong\nwait\nexit 0\n");
    // "[PID] NAME: exit 0" for each, the pids whatever the boot left.
    let mut collected: Vec<&str> = command_output(&out, "wait")
        .iter()
        .map(|l| l.split_once("] ").map_or(*l, |(_, rest)| rest))
        .collect();
    collected.sort();
    assert_eq!(collected, ["ding: exit 0", "regs: exit 0"], "{out}");
}

#[test]
fn programs_get_their_memory_and_faults_kill_them() {
    // data: zeroed stack and .bss, initialized and writable .data. badstore
    // stores to its .rodata: killed, the PTE shows A set up front (RV64 user
    // leaves have A, and D if writable: vm.h). badhalt: an M-mode
    // instruction. The kernel carries on (stress).
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

#[test]
fn files_pipes_and_64_bit_offsets_on_a_disk() {
    // filetest on the RV64 root filesystem: every file system call and its
    // errors, and on RV64 lseek's 64-bit off_t (an offset past the end, past
    // the largest file or LONG_MAX fails with -EINVAL whatever its low 32
    // bits) and stat's 24 bytes.
    let out = run(
        Setup {
            disks: vec![rv64_root_disk(4096, &[])],
            ..Setup::default()
        },
        b"filetest\nexit 0\n",
    );
    all_checks_pass(&out, "filetest", "filetest");
}

#[test]
fn sockets_over_loopback() {
    // socktest: every socket call and its errors, kernel pointers among
    // them (the RV64 kernel image's address).
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
fn an_elf32_program_is_refused() {
    // The RV64 kernel loads ELF64 executables only, with no compat mode for
    // RV32 ones (spec, "RV64 processes", "Loading"): hello built for RV32,
    // an ELF32 file in /bin, does not start, for the reason the RV32
    // kernel gives an image of another class (kernel.rs's
    // the_elf_loader_refuses_bad_images), and the RV64 one runs.
    let user = xtask::userprogs::user_dir(&repo_root());
    let hello32 = xtask::userprogs::build_named_for(&user, "hello", kernel::Xlen::Rv32).unwrap();
    assert_eq!(hello32.elf[4], 1, "not ELFCLASS32");
    let out = run(
        Setup {
            disks: vec![rv64_root_disk(4096, &[("/bin/hello32", &hello32.elf)])],
            ..Setup::default()
        },
        b"hello32\nhello\nexit 0\n",
    );
    assert_eq!(
        command_output(&out, "hello32"),
        [
            "spawn: /bin/hello32: not a 64-bit little-endian RISC-V file",
            "sh: hello32: not an executable"
        ]
    );
    assert_eq!(command_output(&out, "hello"), ["hello from user mode"]);
}

#[test]
fn the_calls_take_and_return_64_bit_values() {
    // sbrk above 4 GiB, from a program built here (merlin-as, merlin-ld)
    // and run from the disk; lseek's and stat's widths are filetest's
    // (files_pipes_and_64_bit_offsets_on_a_disk).
    let obj = xtask::toolchain::assemble_for("high.s", HIGH, kernel::Xlen::Rv64).unwrap();
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
            disks: vec![rv64_root_disk(4096, &[("/bin/high", &elf)])],
            ..Setup::default()
        },
        b"high\nexit 0\n",
    );
    assert_eq!(command_output(&out, "high"), ["high: ok"]);
}
