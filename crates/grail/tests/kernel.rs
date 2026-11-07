//! The Camelot kernel (`kernel/kernel.elf`, committed) run through the `grail`
//! binary, on its firmware and in its 128 MiB of RAM (the command's defaults, QEMU
//! `virt`'s), with `--max-steps` as a guard: booting with paging on (with several
//! `--ram` sizes), the timer ticks, the user-space shell driven through
//! stdin, the panics from its deliberate faults, user programs taking turns or
//! getting killed, and image files attached with `--disk`. The kernel's own
//! tests (traps, delegation, the register dump, the page table, the console,
//! processes, disks) are in `crates/xtask/tests/kernel.rs`.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Far more than a run needs (a boot is about 16 million steps with the command's
/// 128 MiB of RAM, most of them setting up memory; `wfi` skips ahead to each tick),
/// so hitting it means the kernel hung. Generous because stdin reaches
/// the machine through a thread: until the bytes arrive the kernel idles
/// through timer ticks, which costs steps.
const MAX_STEPS: &str = "200000000";

// Kernel constants, from the kernel sources named in each comment. grail
// can't depend on xtask, so crates/xtask/tests/kernel.rs has its own copies:
// keep them in sync.

/// Exit code of a kernel panic (`PANIC_EXIT_CODE` in kernel/include/trap.h).
const PANIC_EXIT: i32 = 0xfe;

/// Values of the kernel's `boot_fault` (kernel/main.c).
const BOOT_FAULT_ILLEGAL: u32 = 2;
const BOOT_FAULT_LOAD: u32 = 3;
const BOOT_FAULT_STORE_TEXT: u32 = 4;

/// What the kernel prints once paging is on (kernel/main.c, kernel/vmtest.c).
fn memory_lines(ram_mib: u32) -> String {
    format!("ram: {ram_mib} MiB at 0x80000000\npaging: on (Sv48)\nvm self-test: 26/26 passed\n")
}

/// The command's RAM, QEMU `virt`'s (`grail::cli::COMMAND_RAM_SIZE`).
const RAM_MIB: u32 = 128;

/// What the kernel prints after the ticks and before the shell: its kernel stacks'
/// self-test, and how much of a stack it used (kernel/vm.c), taken from `out`.
fn kstack_lines(out: &str) -> &str {
    let at = out
        .find("kstack self-test: 3/3 passed\nkstack: ")
        .unwrap_or_else(|| panic!("no kernel stack self-test:\n{out}"));
    let end = at + out[at..].find(" bytes used at most\n").expect(out) + " bytes used at most\n".len();
    &out[at..end]
}

fn kernel_elf() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../kernel/kernel.elf")
}

/// Runs `elf` on the `grail` binary with `stdin` as its standard input.
/// Returns the exit code and stdout.
fn grail(elf: &Path, stdin: &[u8]) -> (Option<i32>, String) {
    grail_with(&[], elf, stdin)
}

/// [`grail`] with extra command-line `args` before the image.
fn grail_with(args: &[&str], elf: &Path, stdin: &[u8]) -> (Option<i32>, String) {
    // The clock at a fixed date, so the boot's `rtc:` line is the same on every run.
    let mut child = Command::new(env!("CARGO_BIN_EXE_grail"))
        .args(["--max-steps", MAX_STEPS, "--rtc", "2026-01-01"])
        .args(args)
        .arg(elf)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("cannot run grail");
    // A kernel that panics may exit before reading it all: ignore a broken pipe.
    let _ = child.stdin.take().unwrap().write_all(stdin);
    let out = child.wait_with_output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!stderr.contains("step limit"), "kernel hung:\n{stdout}{stderr}");
    (out.status.code(), stdout)
}

/// A copy of the kernel image with `boot_fault` set to `fault`, written to a
/// scratch file.
fn kernel_with_fault(fault: u32) -> PathBuf {
    let mut file = std::fs::read(kernel_elf()).unwrap();
    let elf = elf::Elf::parse(&file).unwrap();
    let addr = elf
        .symbols()
        .unwrap()
        .into_iter()
        .find(|s| s.name == "boot_fault")
        .expect("no boot_fault symbol")
        .value;
    // The file offset of `addr`: in the loadable segment that holds it.
    let seg = elf
        .segments
        .iter()
        .find(|p| p.ty == elf::PT_LOAD && (p.vaddr..p.vaddr + p.filesz).contains(&addr))
        .expect("boot_fault is not in the image");
    let off = (seg.offset + (addr - seg.vaddr)) as usize;
    assert_eq!(
        file[off..off + 4],
        1u32.to_le_bytes(),
        "boot_fault should ship as BOOT_FAULT_NONE"
    );
    file[off..off + 4].copy_from_slice(&fault.to_le_bytes());

    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("grail-kernel");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("kernel-fault-{fault}.elf"));
    std::fs::write(&path, file).unwrap();
    path
}

/// The ten tick lines the kernel prints before it starts the shell (or faults).
fn ticks() -> String {
    (1..=10).map(|n| format!("tick {n}\n")).collect()
}

/// What the kernel says before the timer line of the display Grail's
/// machines have (kernel/fb.c).
const FB: &str = "fb: 640x480, 6 modes, irq 16\n";

/// What the kernel says after the timer line when no disk is attached
/// (kernel/main.c).
const NO_DISK: &str = "disk: none\n";
/// The kernel's line for the real-time clock, which `grail_with` starts at 2026-01-01.
const RTC: &str = "rtc: 2026-01-01 00:00 UTC\n";

/// What the shell prints before its first prompt, and the prompt
/// (kernel/user/sh.c).
const SHELL_BANNER: &str = "Type help for the list of commands.\n";
const PROMPT: &str = "$ ";

/// `out` from the shell's banner on.
fn shell_output(out: &str) -> &str {
    let at = out
        .find(SHELL_BANNER)
        .unwrap_or_else(|| panic!("the shell never started:\n{out}"));
    &out[at..]
}

#[test]
fn kernel_ticks_ten_times_then_starts_the_shell() {
    let (code, out) = grail(&kernel_elf(), b"exit\n");
    assert_eq!(code, Some(0), "output:\n{out}");
    // The banner (its exact text is checked in xtask's kernel tests), then the version.
    assert!(out.starts_with("\n    ____ "), "{out}");
    assert!(out.contains("|_|\n\nCamelot v0.1.0 booting...\n"), "{out}");
    assert!(
        out.ends_with(&format!(
            "{}{FB}timer: 10 Hz tick, 10000000 Hz timebase\n{RTC}{NO_DISK}{}{}{SHELL_BANNER}{PROMPT}exit\n",
            memory_lines(RAM_MIB),
            ticks(),
            kstack_lines(&out)
        )),
        "{out}"
    );
}

#[test]
fn shell_runs_ticks_then_exit_from_stdin() {
    // Issue #18's acceptance test, now with the shell in user space. The
    // kernel's console echoes each command, then the program prints its
    // output.
    let (code, out) = grail(&kernel_elf(), b"ticks\nexit\n");
    assert_eq!(code, Some(0), "output:\n{out}");
    let rest = shell_output(&out)
        .strip_prefix(&format!("{SHELL_BANNER}{PROMPT}ticks\nticks: "))
        .unwrap_or_else(|| panic!("{out}"));
    let (n, rest) = rest.split_once('\n').unwrap();
    // At least the ten boot ticks; more if stdin was slow to reach the machine.
    let n: u32 = n.parse().unwrap_or_else(|_| panic!("ticks: {n:?}"));
    assert!(n >= 10, "{out}");
    assert_eq!(rest, format!("{PROMPT}exit\n"));
}

#[test]
fn booting_drops_into_the_user_shell() {
    // Issue #21's acceptance test: the shell is the first process; it runs
    // hello and exits, and the machine stops with its status.
    let (code, out) = grail(&kernel_elf(), b"hello\nexit\n");
    assert_eq!(code, Some(0), "output:\n{out}");
    assert_eq!(
        shell_output(&out),
        format!("{SHELL_BANNER}{PROMPT}hello\nhello from user mode\n{PROMPT}exit\n")
    );
    let (code, out) = grail(&kernel_elf(), b"exit 42\n");
    assert_eq!(code, Some(42), "output:\n{out}");
}

#[test]
fn shell_accepts_windows_line_ends() {
    // Text piped on Windows may end its lines with CR LF, and a raw console
    // sends CR alone for Enter: both must end a line exactly once.
    let (code, out) = grail(&kernel_elf(), b"help\r\nnope\recho a b\r\nexit\r\n");
    assert_eq!(code, Some(0), "output:\n{out}");
    let want = format!(
        "{SHELL_BANNER}{PROMPT}help\n\
         help            this list\n\
         exit [N]        leave the shell, with status N or the last command's\n\
         wait            wait for every background program\n\
         cd [DIR]        change the working directory, to HOME (or /) without DIR\n\
         exec [PROG ...] replace the shell with PROG; without one, keep its redirections\n\
         export [NAME..] pass variables (NAME or NAME=VALUE) to programs; alone, list them\n\
         unset NAME...   remove variables\n\
         NAME=VALUE      set a variable; before a command, for that command alone\n\
         PROG [ARG...]   run /bin/PROG (e.g. ls, cat, echo, wc, sh), or the program at PROG if it has a /\n\
         CMD < FILE      read the input from FILE\n\
         CMD > FILE      write the output to FILE\n\
         CMD | CMD       send the first command's output to the second one\n\
         CMD & CMD       run the first command in the background\n\
         {PROMPT}nope\n\
         sh: nope: no such program\n\
         {PROMPT}echo a b\n\
         a b\n\
         {PROMPT}exit\n"
    );
    assert_eq!(shell_output(&out), want);
}

#[test]
fn exec_replaces_the_shell() {
    // POSIX's `exec`: alone, it does nothing (no redirections to keep); a program that
    // can't be started is reported and the shell carries on, as is a redirection
    // that can't be opened (there are no files without a disk). Its NAME is always a
    // program, never a built-in: `exec cd /bin` is /bin/cd, which isn't there. In a
    // pipeline or a background job, which POSIX runs in a subshell, it replaces the
    // command's subshell: one `exec` goes and the rest runs the program NAME, still
    // never a built-in (`exec exit 7 &` doesn't end the shell, `exec exec echo hi &`
    // is /bin/exec), and `exec` alone only makes its redirections. A shell reading a
    // pipe is not interactive: a failed `exec` ends it, with 127 for no such program.
    // Then `exec PROG` replaces the first process, so the machine stops with PROG's
    // status: `systest fds` exits 0 only with descriptors 0, 1 and 2 and no other (the
    // shell's copies above 2 are not inherited), `systest exit 3` with 3.
    let lines = "exec\nexec nosuch\nexec > /f\nexec cd /bin\nexec cd /bin &\nexec exit 7 &\n\
                 exec exec echo hi &\nexec &\nexec > /f | cat\necho piped | exec cat\n\
                 echo exec nosuch | sh\nexec echo behind &\nwait\n";
    let (code, out) = grail(&kernel_elf(), format!("{lines}exec systest fds\n").as_bytes());
    assert_eq!(code, Some(0), "output:\n{out}");
    let shell = shell_output(&out);
    let want_start = format!(
        "{SHELL_BANNER}{PROMPT}exec\n\
         {PROMPT}exec nosuch\n\
         sh: nosuch: no such program\n\
         {PROMPT}exec > /f\n\
         sh: /f: no such file or directory\n\
         {PROMPT}exec cd /bin\n\
         sh: cd: no such program\n\
         {PROMPT}exec cd /bin &\n\
         sh: cd: no such program\n\
         {PROMPT}exec exit 7 &\n\
         sh: exit: no such program\n\
         {PROMPT}exec exec echo hi &\n\
         sh: exec: no such program\n\
         {PROMPT}exec &\n\
         {PROMPT}exec > /f | cat\n\
         sh: /f: no such file or directory\n\
         {PROMPT}echo piped | exec cat\n\
         piped\n\
         {PROMPT}echo exec nosuch | sh\n\
         {SHELL_BANNER}{PROMPT}sh: nosuch: no such program\n\
         sh: exit 127\n\
         {PROMPT}exec echo behind &\n"
    );
    assert!(shell.starts_with(&want_start), "{shell}");
    // The background echo's [PID], its line and `wait`'s report, in whatever order
    // its output and the prompt come.
    let rest = &shell[want_start.len()..];
    assert!(rest.contains("behind\n"), "{rest}");
    assert!(rest.contains("] echo: exit 0\n"), "{rest}");
    assert!(rest.ends_with(&format!("{PROMPT}exec systest fds\n")), "{rest}");

    let (code, out) = grail(&kernel_elf(), b"exec systest exit 3\n");
    assert_eq!(code, Some(3), "output:\n{out}");
}

#[test]
fn kernel_boots_with_paging_on_any_ram_size() {
    // The page allocator takes the RAM size from sysinfo, so --ram changes
    // what the kernel manages; the self-tests and the ticks run with paging
    // on either way, and so does the shell. 8 MiB is the smallest machine the
    // kernel boots in: the firmware's 2 MiB, then its image, with the user
    // programs it embeds, a little over 2 MiB, then the device tree, at a 2 MiB
    // boundary (QEMU's place).
    for mib in ["8", "16", "300"] {
        let (code, out) = grail_with(&["--ram", mib], &kernel_elf(), b"exit\n");
        assert_eq!(code, Some(0), "--ram {mib}, output:\n{out}");
        let want = format!(
            "{}{FB}timer: 10 Hz tick, 10000000 Hz timebase\n{RTC}{NO_DISK}{}{}{SHELL_BANNER}{PROMPT}exit\n",
            memory_lines(mib.parse().unwrap()),
            ticks(),
            kstack_lines(&out)
        );
        assert!(out.ends_with(&want), "--ram {mib}:\n{out}");
    }
}

#[test]
fn illegal_instruction_panics() {
    let (code, out) = grail(&kernel_with_fault(BOOT_FAULT_ILLEGAL), b"");
    assert_eq!(code, Some(PANIC_EXIT), "output:\n{out}");
    let report = format!(
        "{}panic: illegal instruction\n  scause 0x0000000000000002  sepc 0x",
        ticks()
    );
    assert!(out.contains(&report), "{out}");
    assert!(out.contains("  stval 0x00000000c0001073  "), "{out}");
    // The register dump, with the values illegal_instruction put there.
    assert!(out.contains("   ra 0101010101010101"), "{out}");
    assert!(out.contains("  t6 1f1f1f1f1f1f1f1f\n"), "{out}");
}

#[test]
fn unmapped_load_panics() {
    let (code, out) = grail(&kernel_with_fault(BOOT_FAULT_LOAD), b"");
    assert_eq!(code, Some(PANIC_EXIT), "output:\n{out}");
    // Paging is on: an address the page table doesn't map is a page fault.
    let report = format!(
        "{}panic: load page fault\n  scause 0x000000000000000d  sepc 0x",
        ticks()
    );
    assert!(out.contains(&report), "{out}");
    assert!(out.contains("  stval 0x0000000000000100  "), "{out}");
    assert!(out.contains("\n  va 0x0000000000000100 is not mapped\n"), "{out}");
}

#[test]
fn store_to_kernel_text_panics() {
    let (code, out) = grail(&kernel_with_fault(BOOT_FAULT_STORE_TEXT), b"");
    assert_eq!(code, Some(PANIC_EXIT), "output:\n{out}");
    let report = format!(
        "{}panic: store page fault\n  scause 0x000000000000000f  sepc 0x",
        ticks()
    );
    assert!(out.contains(&report), "{out}");
    // The page is mapped, read and execute only, and the line shows the PTE
    // as the kernel page table held it when the store faulted. (Whether A is
    // set depends on whether code on that page ran with paging on, which is
    // up to the layout: the xtask test of the same name works it out. The
    // store set neither A nor D, since it faulted.) The same kernel, run
    // in-process, shows that PTE in RAM as the trap is taken.
    let va = out
        .lines()
        .find_map(|l| l.strip_prefix("  va 0x")?.split_once(' ').map(|(va, _)| va))
        .map(|va| u64::from_str_radix(va, 16).unwrap())
        .unwrap_or_else(|| panic!("no mapping line:\n{out}"));
    let file = std::fs::read(kernel_with_fault(BOOT_FAULT_STORE_TEXT)).unwrap();
    let mut m = grail::Machine::new(grail::MachineConfig {
        bios: grail::Bios::Default,
        ..grail::MachineConfig::with_ram(8 << 20)
    })
    .unwrap();
    m.cpu.bus.uart.capture = true;
    m.load_elf(&file).unwrap();
    const STORE_PAGE_FAULT: u64 = 15;
    // (The vm self-test's probes make store page faults too, elsewhere.) The walk
    // from satp's root, Sv48 or Sv39, down to the leaf that maps `va`, a 4 KiB page
    // or a superpage.
    let mut at_fault = None;
    let r = m.run(Some(MAX_STEPS.parse().unwrap()), |cpu| {
        if at_fault.is_none() && cpu.csr.scause == STORE_PAGE_FAULT && cpu.csr.stval == va {
            let read = |addr: u64| cpu.bus.read_mem(addr, 8).unwrap();
            let levels = if cpu.csr.satp >> 60 == 9 { 4 } else { 3 };
            let mut table = (cpu.csr.satp & ((1 << 44) - 1)) << 12;
            for level in (0..levels).rev() {
                let pte = read(table + 8 * (va >> (12 + 9 * level) & 0x1ff));
                if pte & 0b1010 != 0 {
                    // R or X: a leaf.
                    let offset = va & ((1 << (12 + 9 * level)) - 1);
                    at_fault = Some((pte, (pte >> 10) << 12 | offset));
                    break;
                }
                table = (pte >> 10) << 12;
            }
        }
    });
    assert_eq!(r, grail::RunResult::Exited(PANIC_EXIT as u32));
    let (pte, pa) = at_fault.expect("no store page fault on that page in-process");
    let flags: String = "DAGUXWRV"
        .chars()
        .enumerate()
        .map(|(i, c)| if pte >> (7 - i) & 1 != 0 { c } else { '-' })
        .collect();
    // R+X, and A, D and G, which the kernel sets on its own leaves (vm.h's
    // PTE_KERNEL): no access through them waits for the hardware to.
    assert_eq!(flags, "DAG-X-RV", "pte {pte:#018x}");
    let line = format!("  va {va:#018x} maps to pa {pa:#018x}, pte {pte:#018x} {flags}\n");
    assert!(out.contains(&line), "want {line}in:\n{out}");
}

// ---- User processes (issues #20 and #21) ----

/// The lines between the shell's echo of `command` and its next prompt: what
/// the programs and the kernel printed meanwhile.
fn command_output<'a>(out: &'a str, command: &str) -> Vec<&'a str> {
    let start = format!("{PROMPT}{command}\n");
    let at = out
        .find(&start)
        .unwrap_or_else(|| panic!("no `{command}` in:\n{out}"))
        + start.len();
    let rest = &out[at..];
    rest[..rest
        .find(PROMPT)
        .unwrap_or_else(|| panic!("`{command}` never ended:\n{out}"))]
        .lines()
        .collect()
}

#[test]
fn user_programs_run_interleaved() {
    // ding and dong each print four lines about half a tick apart, from
    // U-mode in their own address spaces. The scheduler switches on every
    // timer tick, so their lines alternate in pairs. The shell runs ding in
    // the background and waits for dong, then collects ding with `wait`.
    let (code, out) = grail(&kernel_elf(), b"ding & dong\nwait\nexit\n");
    assert_eq!(code, Some(0), "output:\n{out}");
    // The shell's `[PID]` comes first, or after ding's first line if a tick
    // lands in the shell's fork (likely with merlin's unoptimized kernel). (The
    // kernel stacks' self-test took the first pids, its threads'.)
    let lines = command_output(&out, "ding & dong");
    let is_job = |l: &str| l.starts_with('[') && l.ends_with(']');
    assert_eq!(lines.iter().filter(|&&l| is_job(l)).count(), 1, "{out}");
    let job = *lines[..2].iter().find(|&&l| is_job(l)).expect(&out);
    let printed: Vec<&str> = lines.iter().copied().filter(|&l| l != job).collect();
    assert_eq!(printed.len(), 8, "{out}");
    // Each in order, and they take turns more than once.
    for name in ["ding", "dong"] {
        let own: Vec<&str> = printed.iter().copied().filter(|l| l.starts_with(name)).collect();
        let want: Vec<String> = (1..=4).map(|i| format!("{name} {i}")).collect();
        assert_eq!(own, want, "{out}");
    }
    let turns = printed.windows(2).filter(|w| w[0][..4] != w[1][..4]).count();
    assert!(turns >= 3, "only {turns} switches:\n{out}");
    assert_eq!(
        command_output(&out, "wait"),
        [format!("{job} ding: exit 0")],
        "{out}"
    );
}

#[test]
fn faulting_user_programs_are_killed_and_the_kernel_carries_on() {
    // badload reads kernel memory (mapped in its page table, but without U),
    // badhalt executes xcal.halt (M-mode only). Both die, the kernel says
    // why, the shell reports their status, hello still runs, and so does the
    // shell afterwards. The kernel's first page is in a 2 MiB leaf (the image is
    // mapped with them): the report shows its PTE (A, D and G, as the kernel makes
    // its leaves) and the physical address, the image's load address, 0x8020_0000.
    // (The pids follow the kernel stacks' self-test's threads.)
    let (code, out) = grail(&kernel_elf(), b"badload\nbadhalt\nhello\nticks\nexit\n");
    assert_eq!(code, Some(0), "output:\n{out}");
    let lines = command_output(&out, "badload");
    assert_eq!(lines.len(), 4, "{out}");
    let pid = |l: &str| -> u32 {
        l.strip_prefix("pid ")
            .and_then(|r| r.split_once(' '))
            .unwrap()
            .0
            .parse()
            .unwrap()
    };
    let badload = pid(lines[0]);
    assert_eq!(
        lines[0],
        format!("pid {badload} (badload) killed: load page fault")
    );
    assert!(
        lines[1].starts_with("  scause 0x000000000000000d  sepc 0x"),
        "{out}"
    );
    assert!(lines[1].ends_with("  stval 0xffffffff80000000"), "{out}");
    assert_eq!(
        lines[2],
        "  va 0xffffffff80000000 maps to pa 0x0000000080200000, pte 0x00000000200800eb DAG-X-RV"
    );
    assert_eq!(lines[3], "badload: exit -1");
    let lines = command_output(&out, "badhalt");
    assert_eq!(lines.len(), 3, "{out}");
    assert_eq!(
        lines[0],
        format!("pid {} (badhalt) killed: illegal instruction", badload + 1)
    );
    assert!(lines[1].ends_with("  stval 0x000000000005100b"), "{out}");
    assert_eq!(lines[2], "badhalt: exit -1");
    assert_eq!(command_output(&out, "hello"), ["hello from user mode"]);
    assert!(out.contains(&format!("{PROMPT}ticks\nticks: ")), "{out}");
    assert!(out.ends_with(&format!("{PROMPT}exit\n")), "{out}");
}

#[test]
fn bad_pointers_to_the_system_calls_are_refused() {
    // Issue #21's second criterion, end to end: badptr hands the system
    // calls pointers to kernel memory, page 0, unmapped and read-only pages,
    // and every call fails with -EFAULT instead of faulting.
    let (code, out) = grail(&kernel_elf(), b"badptr\nexit\n");
    assert_eq!(code, Some(0), "output:\n{out}");
    assert_eq!(
        command_output(&out, "badptr"),
        ["badptr: every bad pointer refused"]
    );
}

// ---- Disks (issue #65) ----

/// A scratch disk image of `sectors` sectors, every byte different from its
/// neighbours, written next to the other scratch files. Returns its path and
/// contents.
fn disk_image(name: &str, sectors: usize) -> (PathBuf, Vec<u8>) {
    let bytes: Vec<u8> = (0..sectors * 512).map(|i| (i * 13 + i / 512) as u8).collect();
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("grail-kernel");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, &bytes).unwrap();
    (path, bytes)
}

/// Checks `out` has `blk self-test: N/N passed` and returns N.
fn blk_selftest_passed(out: &str) -> u32 {
    let at = out
        .find("blk self-test: ")
        .unwrap_or_else(|| panic!("no disk self-test:\n{out}"));
    let line = out[at..].lines().next().unwrap();
    let (passed, total) = line
        .strip_prefix("blk self-test: ")
        .and_then(|r| r.strip_suffix(" passed"))
        .and_then(|r| r.split_once('/'))
        .unwrap_or_else(|| panic!("{line}"));
    assert_eq!(passed, total, "{out}");
    passed.parse().unwrap()
}

#[test]
fn kernel_tests_an_image_file_disk_and_leaves_it_as_it_was() {
    // The boot self-test writes a scratch block of the disk and puts it back.
    let (path, bytes) = disk_image("rw.img", 128);
    let (code, out) = grail_with(&["--disk", path.to_str().unwrap()], &kernel_elf(), b"exit\n");
    assert_eq!(code, Some(0), "output:\n{out}");
    assert!(
        out.contains("timebase\nrtc: 2026-01-01 00:00 UTC\ndisk 0: 128 sectors (64 KiB), irq 3\ntick 1\n"),
        "{out}"
    );
    // After the ticks, before the shell, and before the kernel looks for a
    // filesystem on the disk (it holds none).
    assert!(out.contains("tick 10\nblk self-test: "), "{out}");
    assert!(blk_selftest_passed(&out) > 10, "{out}");
    assert!(
        out.ends_with(&format!(
            " passed\nfs: no filesystem on disk 0\n{}{SHELL_BANNER}{PROMPT}exit\n",
            kstack_lines(&out)
        )),
        "{out}"
    );
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
}

#[test]
fn kernel_lists_every_disk_and_leaves_a_read_only_one_alone() {
    let (ro, bytes) = disk_image("ro.img", 64);
    let (rw, _) = disk_image("second.img", 8);
    let (code, out) = grail_with(
        &["--disk-ro", ro.to_str().unwrap(), "--disk", rw.to_str().unwrap()],
        &kernel_elf(),
        b"exit\n",
    );
    assert_eq!(code, Some(0), "output:\n{out}");
    assert!(
        out.contains(
            "timebase\n\
             rtc: 2026-01-01 00:00 UTC\n\
             disk 0: 64 sectors (32 KiB), irq 3, read-only\n\
             disk 1: 8 sectors (4 KiB), irq 4\n\
             tick 1\n"
        ),
        "{out}"
    );
    // Disk 0 is the one tested: its write must fail with BLK_ERR_READ_ONLY.
    blk_selftest_passed(&out);
    assert_eq!(std::fs::read(&ro).unwrap(), bytes);
}
