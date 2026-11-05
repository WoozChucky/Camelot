//! The basic user programs (issue #68, kernel/user/README.md): each one,
//! its output and its errors, from the shell of Camelot booted with the
//! root image `cargo xtask fs` builds. What the programs print is checked
//! against what the host reads from the same image (camelot-fs), and what
//! they leave on the disk against the host's reading of it afterwards.
//!
//! The session is typed one command at a time, each once the shell has
//! printed its prompt for it, so a command can use what an earlier one
//! printed (the pid of a job sent to the background).
//!
//! The programs are compiled by merlin; the same session runs, in a test
//! that needs clang and is ignored otherwise, with every program compiled
//! by clang instead, so that a program that builds with only one of the
//! compilers, or behaves differently with each, is caught. Both run on
//! RV64 too (#156): the RV64 kernel, `kernel/kernel-rv64.elf`, on the RV64
//! root image, whose programs give the same output.

use camelot_fs::read::Fs;
use camelot_fs::{fsck, SECTOR_SIZE};
use grail::{DiskConfig, Machine, MachineConfig, RunResult};
use std::sync::mpsc::{self, Sender};
use xtask::kernel::{Xlen, BOOT_MAX_STEPS};
use xtask::repo_root;
use xtask::userprogs::CCompiler;

const PROMPT: &str = "$ ";
const SHELL_BANNER: &str = "Type help for the list of commands.\n";
/// RAM for the machine: less than the default, so it boots faster.
const RAM: usize = 16 << 20;
/// Steps the machine runs between looks at what it printed.
const SLICE: u64 = 500_000;
/// The most steps one command may take. merlin's code is several times
/// slower than clang's; the slowest command here takes about 20 million.
const COMMAND_MAX_STEPS: u64 = 200_000_000;
/// The ticks that `ticks`, the shell and `sleep` itself may take besides
/// the ones `sleep` waits for, on both widths. RV64's 4-level page tables
/// are seven pages a process where RV32's are three: zeroed four bytes at a
/// time, the 36 pages of `ticks`, `sleep` and `ticks` (24 on RV32) took
/// RV64 a million steps, a tick, more; kzalloc() zeroes with memset's
/// register-wide stores now, as Linux's clear_page() does.
const SLEEP_SLACK: u32 = 3;

/// Camelot running on Grail, with its shell waiting for a command.
struct Session {
    m: Machine,
    input: Sender<u8>,
}

impl Session {
    /// Boots the committed `xlen` kernel with `image` as disk 0 and waits
    /// for the shell's first prompt.
    fn boot(image: Vec<u8>, xlen: Xlen) -> Session {
        // The RV64 kernel runs in S-mode, on Camelot's firmware.
        let (machine_xlen, bios, kernel) = match xlen {
            Xlen::Rv32 => (grail::Xlen::Rv32, grail::Bios::None, "kernel.elf"),
            Xlen::Rv64 => (
                grail::Xlen::Rv64,
                grail::Bios::Default,
                xtask::kernel::KERNEL_ELF_RV64,
            ),
        };
        let config = MachineConfig {
            ram_size: RAM,
            xlen: machine_xlen,
            bios,
            disks: vec![DiskConfig::memory(image)],
            ..MachineConfig::default()
        };
        let mut m = Machine::new(config).unwrap();
        m.cpu.bus.uart.capture = true;
        let elf = std::fs::read(repo_root().join("kernel").join(kernel)).unwrap();
        m.load_elf(&elf).unwrap();
        let (input, rx) = mpsc::channel();
        m.set_uart_input(rx);
        let mut s = Session { m, input };
        let banner = format!("{SHELL_BANNER}{PROMPT}");
        s.run_until(|out| out.ends_with(&banner), BOOT_MAX_STEPS);
        s
    }

    /// Everything printed so far.
    fn out(&self) -> String {
        String::from_utf8_lossy(&self.m.cpu.bus.uart.captured).into_owned()
    }

    /// Runs the machine until `done` says yes to what it printed.
    fn run_until(&mut self, done: impl Fn(&str) -> bool, max_steps: u64) {
        let mut steps = 0;
        while !done(&self.out()) {
            assert!(steps < max_steps, "gave up after {steps} steps:\n{}", self.out());
            let r = self.m.run(Some(SLICE), |_| {});
            assert_eq!(r, RunResult::StepLimit, "the machine stopped:\n{}", self.out());
            steps += SLICE;
        }
    }

    /// Types `line` for the shell and returns what was printed after it (the
    /// console echoes it) up to the next prompt.
    fn run(&mut self, line: &str) -> String {
        let start = self.out().len();
        for b in line.bytes().chain(*b"\n") {
            self.input.send(b).unwrap();
        }
        let echoed = format!("{line}\n");
        self.run_until(
            |out| out[start..].len() > echoed.len() && out[start..].ends_with(PROMPT),
            COMMAND_MAX_STEPS,
        );
        let out = self.out();
        let new = &out[start..];
        assert!(new.starts_with(&echoed), "{new}");
        new[echoed.len()..new.len() - PROMPT.len()].to_string()
    }

    /// Runs `line` between two `ticks`, all three typed at once, and returns
    /// what `line` printed and the timer ticks from one `ticks` to the
    /// other. Typed ahead, the commands follow each other with no wait for
    /// the next line, during which the idle machine would skip ticks ahead.
    fn ticks_around(&mut self, line: &str) -> (String, u32) {
        let start = self.out().len();
        for b in format!("ticks\n{line}\nticks\n").bytes() {
            self.input.send(b).unwrap();
        }
        self.run_until(
            |out| out[start..].matches(PROMPT).count() == 3 && out.ends_with(PROMPT),
            COMMAND_MAX_STEPS,
        );
        let out = self.out()[start..].to_string();
        let parts: Vec<&str> = out.split(PROMPT).collect();
        let ticks = |part: &str| -> u32 {
            part.strip_prefix("ticks\nticks: ")
                .and_then(|n| n.strip_suffix('\n'))
                .and_then(|n| n.parse().ok())
                .unwrap_or_else(|| panic!("{out}"))
        };
        let printed = parts[1]
            .strip_prefix(&format!("{line}\n"))
            .unwrap_or_else(|| panic!("{out}"));
        (printed.to_string(), ticks(parts[2]) - ticks(parts[0]))
    }

    /// Types `exit` and returns how the machine stopped and the disk.
    fn exit(mut self) -> (RunResult, Vec<u8>) {
        for b in b"exit\n" {
            self.input.send(*b).unwrap();
        }
        let r = self.m.run(Some(COMMAND_MAX_STEPS), |_| {});
        let d = &mut self.m.cpu.bus.disks[0];
        let mut disk = vec![0; d.capacity() as usize * SECTOR_SIZE];
        d.backend().read(0, &mut disk).unwrap();
        (r, disk)
    }
}

/// The `xlen` root image, its programs compiled by `cc`, laid out as the
/// real one is (`cargo xtask fs`: [`xtask::fs::IMAGE_SIZE`] blocks with the
/// kernel's left out), so the programs have the room they have on Camelot.
fn root_image(cc: CCompiler, xlen: Xlen) -> Vec<u8> {
    let root = repo_root();
    let kernel = std::fs::read(root.join(xtask::fs::kernel_for(xlen))).unwrap();
    let opts = xtask::fs::bootable_layout(xtask::fs::IMAGE_SIZE, &kernel).unwrap();
    xtask::fs::build_for_with(&root, &opts, xlen, cc).unwrap()
}

/// What `wc` counts in `text`: lines, words, bytes.
fn counts(text: &[u8]) -> (usize, usize, usize) {
    let lines = text.iter().filter(|&&c| c == b'\n').count();
    let words = text
        .split(|c| matches!(c, b' ' | b'\n' | b'\t' | b'\r'))
        .filter(|w| !w.is_empty())
        .count();
    (lines, words, text.len())
}

/// The lines of `text` grep selects for `pat` (or, with `invert`, those
/// it doesn't), with their numbers, without their newlines.
fn grep<'a>(text: &'a str, pat: &str, invert: bool) -> Vec<(usize, &'a str)> {
    text.split_inclusive('\n')
        .map(|l| l.strip_suffix('\n').unwrap_or(l))
        .enumerate()
        .filter(|(_, l)| l.contains(pat) != invert)
        .map(|(i, l)| (i + 1, l))
        .collect()
}

/// The end of the heap of a program that never grows it: the page after its
/// highest segment, which `ps` reports as its size.
fn size_of(fs: &Fs, prog: &str) -> u32 {
    let elf = fs.read_file(&format!("/bin/{prog}")).unwrap();
    let elf = elf::Elf::parse(&elf).unwrap();
    let end = elf
        .segments
        .iter()
        .filter(|p| p.ty == elf::PT_LOAD)
        .map(|p| p.vaddr + p.memsz)
        .max()
        .unwrap();
    end.next_multiple_of(4096) as u32
}

/// The shell's line after a program that exited with `status`.
fn exit_line(prog: &str, status: i32) -> String {
    format!("{prog}: exit {status}\n")
}

/// Runs every basic program, its errors included, on `image`, with the
/// `xlen` kernel, and checks what it printed and what it left on the disk.
/// `same_programs` says whether `/bin` holds the programs the committed
/// kernel embeds: the kernel's boot self-test checks that they are the
/// same, and otherwise reports one failure, which becomes the machine's
/// exit status.
fn coreutils_session(image: Vec<u8>, same_programs: bool, xlen: Xlen) {
    let host = Fs::open(&image).unwrap();
    let readme_bytes = host.read_file("/README").unwrap();
    let readme = String::from_utf8(readme_bytes.clone()).unwrap();
    let (lines, words, bytes) = counts(&readme_bytes);
    let enoent = "no such file or directory";
    let mut s = Session::boot(image.clone(), xlen);
    let boot = s.out();
    let failures: Vec<&str> = boot.lines().filter(|l| l.contains("FAILED")).collect();
    let expected_failures: &[&str] = if same_programs {
        &[]
    } else {
        &["fs self-test FAILED: the programs in /bin are the kernel's"]
    };
    assert_eq!(failures, expected_failures, "{boot}");

    // ls and cat show what the host reads.
    assert_eq!(s.run("ls /bin"), host.ls("/bin").unwrap());
    assert_eq!(s.run("ls /"), host.ls("/").unwrap());
    assert_eq!(
        s.run("ls /nope"),
        format!("ls: /nope: {enoent}\n{}", exit_line("ls", 1))
    );
    assert_eq!(s.run("cat /README"), readme);
    assert_eq!(
        s.run("cat /README /nope"),
        format!("{readme}cat: /nope: {enoent}\n{}", exit_line("cat", 1))
    );
    // A directory is not copied as if it were text (nor counted, nor
    // searched: wc and grep below).
    assert_eq!(
        s.run("cat /bin"),
        format!("cat: /bin: is a directory\n{}", exit_line("cat", 1))
    );

    // echo.
    assert_eq!(s.run("echo a  b   c"), "a b c\n");
    assert_eq!(s.run("echo"), "\n");
    assert_eq!(s.run("echo -n no newline"), "no newline");

    // wc, all counts or some.
    assert_eq!(s.run("wc /README"), format!("{lines} {words} {bytes} /README\n"));
    assert_eq!(s.run("wc -l /README"), format!("{lines} /README\n"));
    assert_eq!(s.run("wc -cl /README"), format!("{lines} {bytes} /README\n"));
    assert_eq!(s.run("wc -w < /README"), format!("{words}\n"));
    assert_eq!(
        s.run("wc /README /nope"),
        format!(
            "{lines} {words} {bytes} /README\nwc: /nope: {enoent}\n{}",
            exit_line("wc", 1)
        )
    );
    assert_eq!(
        s.run("wc /bin /README"),
        format!(
            "wc: /bin: is a directory\n{lines} {words} {bytes} /README\n{}",
            exit_line("wc", 1)
        )
    );
    assert_eq!(
        s.run("wc -q"),
        format!(
            "wc: unknown option -q\nusage: wc [-l] [-w] [-c] [FILE...]\n{}",
            exit_line("wc", 1)
        )
    );

    // grep: fixed strings, its options, several files, exit statuses.
    let camelot = grep(&readme, "Camelot", false);
    assert!(!camelot.is_empty());
    let want: String = camelot.iter().map(|(_, l)| format!("{l}\n")).collect();
    assert_eq!(s.run("grep Camelot /README"), want);
    let want: String = grep(&readme, "the", false)
        .iter()
        .map(|(n, l)| format!("{n}:{l}\n"))
        .collect();
    assert_eq!(s.run("grep -n the /README"), want);
    let want: String = grep(&readme, "e", true)
        .iter()
        .map(|(_, l)| format!("{l}\n"))
        .collect();
    assert_eq!(s.run("grep -v e /README"), want);
    let a = grep(&readme, "a", false).len();
    assert_eq!(s.run("grep -c a /README"), format!("{a}\n"));
    let want: String = camelot
        .iter()
        .map(|(_, l)| format!("/README:{l}\n"))
        .chain(camelot.iter().map(|(_, l)| format!("/bin/../README:{l}\n")))
        .collect();
    assert_eq!(s.run("grep Camelot /README /bin/../README"), want);
    assert_eq!(s.run("grep Excalibur /README"), exit_line("grep", 1));
    assert_eq!(
        s.run("grep -c Excalibur /README"),
        format!("0\n{}", exit_line("grep", 1))
    );
    assert_eq!(
        s.run("grep x /nope"),
        format!("grep: /nope: {enoent}\n{}", exit_line("grep", 2))
    );
    assert_eq!(
        s.run("grep x /bin"),
        format!("grep: /bin: is a directory\n{}", exit_line("grep", 2))
    );
    let usage = "usage: grep [-v] [-c] [-n] PATTERN [FILE...]\n";
    assert_eq!(
        s.run("grep -z x"),
        format!("grep: unknown option -z\n{usage}{}", exit_line("grep", 2))
    );
    assert_eq!(s.run("grep"), format!("{usage}{}", exit_line("grep", 2)));

    // The issue's pipeline, and grep reading its input.
    assert_eq!(
        s.run("cat /README | grep Camelot | wc -l"),
        format!("{}\n", camelot.len())
    );
    assert_eq!(s.run("grep -c Camelot < /README"), format!("{}\n", camelot.len()));

    // mkdir.
    assert_eq!(s.run("mkdir /t /t/u"), "");
    assert_eq!(
        s.run("mkdir /t"),
        format!("mkdir: /t: file exists\n{}", exit_line("mkdir", 1))
    );
    assert_eq!(
        s.run("mkdir /nope/x"),
        format!("mkdir: /nope/x: {enoent}\n{}", exit_line("mkdir", 1))
    );
    assert_eq!(
        s.run("mkdir"),
        format!("usage: mkdir DIR...\n{}", exit_line("mkdir", 1))
    );

    // cp: to a file, into a directory, over a file; what it refuses.
    assert_eq!(s.run("cp /README /t/a"), "");
    assert_eq!(s.run("cat /t/a"), readme);
    assert_eq!(s.run("cp /README /bin/echo /t/u"), "");
    assert_eq!(s.run("echo short > /t/c"), "");
    assert_eq!(s.run("cp /t/c /t/u/README"), "");
    assert_eq!(s.run("cat /t/u/README"), "short\n");
    assert_eq!(
        s.run("cp /nope /t/b"),
        format!("cp: /nope: {enoent}\n{}", exit_line("cp", 1))
    );
    assert_eq!(
        s.run("cp /t /t/b"),
        format!("cp: /t: is a directory\n{}", exit_line("cp", 1))
    );
    assert_eq!(
        s.run("cp /t/a /t/a"),
        format!("cp: /t/a and /t/a are the same file\n{}", exit_line("cp", 1))
    );
    assert_eq!(
        s.run("cp /README /t/a /t/c"),
        format!("cp: /t/c: not a directory\n{}", exit_line("cp", 1))
    );
    assert_eq!(
        s.run("cp /README /nope/x"),
        format!("cp: /nope/x: {enoent}\n{}", exit_line("cp", 1))
    );
    let usage = "usage: cp SRC DST, or cp SRC... DIR\n";
    assert_eq!(s.run("cp /README"), format!("{usage}{}", exit_line("cp", 1)));

    // mv: renames, moves into a directory, replaces a file.
    assert_eq!(s.run("mv /t/a /t/b"), "");
    assert_eq!(
        s.run("cat /t/a"),
        format!("cat: /t/a: {enoent}\n{}", exit_line("cat", 1))
    );
    assert_eq!(s.run("mv /t/b /t/u"), "");
    assert_eq!(s.run("mv /t/c /t/u/echo"), "");
    assert_eq!(
        s.run("mv /nope /t/x"),
        format!("mv: /nope: {enoent}\n{}", exit_line("mv", 1))
    );
    assert_eq!(
        s.run("mv /t/u /t/v"),
        format!("mv: /t/u: is a directory\n{}", exit_line("mv", 1))
    );
    assert_eq!(
        s.run("mv /t/u/b /t/u/b"),
        format!("mv: /t/u/b and /t/u/b are the same file\n{}", exit_line("mv", 1))
    );
    assert_eq!(
        s.run("mv /t/u/b /t/u/echo /nope"),
        format!("mv: /nope: not a directory\n{}", exit_line("mv", 1))
    );
    // However it is spelled, one name is still the same file...
    assert_eq!(
        s.run("mv /t/u/b /t/u/./b"),
        format!(
            "mv: /t/u/b and /t/u/./b are the same file\n{}",
            exit_line("mv", 1)
        )
    );
    // ...but a second name of the file (a hard link) is already what the
    // first would become: only the first goes.
    assert_eq!(s.run("ln /t/u/b /t/h"), "");
    assert_eq!(s.run("mv /t/h /t/u/b"), "");
    assert_eq!(
        s.run("cat /t/h"),
        format!("cat: /t/h: {enoent}\n{}", exit_line("cat", 1))
    );
    assert_eq!(s.run("cat /t/u/b"), readme);
    let usage = "usage: mv SRC DST, or mv SRC... DIR\n";
    assert_eq!(s.run("mv /t/u/b"), format!("{usage}{}", exit_line("mv", 1)));

    // rm.
    assert_eq!(s.run("rm /t/u/README"), "");
    assert_eq!(
        s.run("rm /t/u/README"),
        format!("rm: /t/u/README: {enoent}\n{}", exit_line("rm", 1))
    );
    assert_eq!(
        s.run("rm /t"),
        format!("rm: /t: directory not empty\n{}", exit_line("rm", 1))
    );
    assert_eq!(s.run("rm"), format!("usage: rm PATH...\n{}", exit_line("rm", 1)));
    let listing = s.run("ls /t/u");

    // sleep, for as many ticks as it was asked (10 a second), and not many
    // more: the commands around it take a tick or two of their own.
    for (time, want) in [("0.3", 3), ("1", 10)] {
        let (printed, slept) = s.ticks_around(&format!("sleep {time}"));
        assert_eq!(printed, "");
        assert!(
            (want..want + SLEEP_SLACK).contains(&slept),
            "sleep {time}: {slept} ticks"
        );
    }
    assert_eq!(
        s.run("sleep 1x"),
        format!("sleep: 1x: invalid time\n{}", exit_line("sleep", 1))
    );
    assert_eq!(
        s.run("sleep"),
        format!("usage: sleep SECONDS\n{}", exit_line("sleep", 1))
    );

    // ps, with a sleeper in the background; kill ends it. The sleeps here
    // are long: while everything sleeps, Grail skips ahead to the next tick,
    // so thousands of ticks can pass between two commands.
    let started = s.run("sleep 100000 &");
    let pid: u32 = started
        .strip_prefix('[')
        .and_then(|r| r.strip_suffix("]\n"))
        .and_then(|n| n.parse().ok())
        .unwrap_or_else(|| panic!("{started}"));
    let ps = s.run("ps");
    let mut rows = ps.lines();
    assert_eq!(rows.next(), Some("  PID  PPID STATE      SIZE NAME"), "{ps}");
    let rows: Vec<Vec<&str>> = rows.map(|r| r.split_whitespace().collect()).collect();
    assert_eq!(rows.len(), 3, "{ps}");
    let sh = rows[0][0];
    let shell_pid: u32 = sh.parse().unwrap();
    let size = |prog: &str| size_of(&host, prog).to_string();
    let (pid_s, ps_pid) = (pid.to_string(), (pid + 1).to_string());
    // The shell sleeps in wait() for ps, unless a tick preempted it between
    // starting ps and waiting: then it is still ready to run.
    let sh_state = if rows[0][2] == "ready" { "ready" } else { "sleep" };
    // The shell's size is its program's and a page of heap: malloc's
    // first sbrk, for the variables it copied from its environment.
    let sh_size = (size_of(&host, "sh") + 4096).to_string();
    assert_eq!(rows[0], [sh, "0", sh_state, &sh_size, "sh"], "{ps}");
    assert_eq!(rows[1], [&pid_s, sh, "sleep", &size("sleep"), "sleep"], "{ps}");
    assert_eq!(rows[2], [&ps_pid, sh, "run", &size("ps"), "ps"], "{ps}");
    assert!(shell_pid < pid);
    assert_eq!(s.run("ps x"), format!("usage: ps\n{}", exit_line("ps", 1)));
    assert_eq!(s.run(&format!("kill {pid}")), "");
    assert_eq!(s.run("wait"), format!("[{pid}] sleep: exit -1\n"));
    assert_eq!(
        s.run(&format!("kill {pid}")),
        format!("kill: {pid}: no such process\n{}", exit_line("kill", 1))
    );
    assert_eq!(
        s.run(&format!("kill abc {pid} 0")),
        format!(
            "kill: abc: not a process id\nkill: {pid}: no such process\nkill: 0: not a process id\n{}",
            exit_line("kill", 1)
        )
    );
    assert_eq!(
        s.run("kill"),
        format!("usage: kill PID...\n{}", exit_line("kill", 1))
    );
    // A number too big for a pid is not one, rather than wrapping around
    // to some other process's (2^32 + the shell's pid would be the shell).
    let wrapped = (1u64 << 32) + u64::from(shell_pid);
    assert_eq!(
        s.run(&format!("kill {wrapped}")),
        format!("kill: {wrapped}: not a process id\n{}", exit_line("kill", 1))
    );
    // The shell can't be killed: its end would be Camelot's.
    assert_eq!(
        s.run(&format!("kill {shell_pid}")),
        format!(
            "kill: {shell_pid}: operation not permitted\n{}",
            exit_line("kill", 1)
        )
    );
    // A reader asleep on the console ends at once too. systest does it (it
    // kills a child of its own), here where nothing is typed ahead for the
    // reader to take.
    assert_eq!(s.run("systest killreader"), "");
    // A pipeline in the background, its reader asleep on the pipe and its
    // writer asleep in sleep. Killing the reader alone ends it while the
    // writer, which would end its input, still sleeps; then the writer.
    let started = s.run("sleep 50000 | cat &");
    let pids: Vec<u32> = started
        .lines()
        .map(|l| l.trim_matches(['[', ']']).parse().unwrap())
        .collect();
    assert_eq!(pids.len(), 2, "{started}");
    assert_eq!(s.run(&format!("kill {}", pids[1])), "");
    let ps = s.run("ps");
    let states: Vec<(&str, &str)> = ps
        .lines()
        .skip(1)
        .map(|r| r.split_whitespace().collect::<Vec<_>>())
        .map(|r| (r[2], r[4]))
        .collect();
    // The shell may be ready rather than asleep, as above.
    let sh_state = if states.first() == Some(&("ready", "sh")) {
        "ready"
    } else {
        "sleep"
    };
    assert_eq!(
        states,
        [
            (sh_state, "sh"),
            ("sleep", "sleep"),
            ("zombie", "cat"),
            ("run", "ps")
        ],
        "{ps}"
    );
    assert_eq!(s.run(&format!("kill {}", pids[0])), "");
    let waited = s.run("wait");
    assert!(
        waited.contains(&format!("[{}] sleep: exit -1\n", pids[0]))
            && waited.contains(&format!("[{}] cat: exit -1\n", pids[1])),
        "{waited}"
    );

    // env (#216): the environment the shell passes on (the kernel's for
    // init), emptied, changed, and a program run with it; and cd's HOME.
    assert_eq!(s.run("env"), "HOME=/\nTERM=linux\n");
    assert_eq!(s.run("env -i A=1 B=2 env"), "A=1\nB=2\n");
    assert_eq!(s.run("env -u HOME TERM=vt100"), "TERM=vt100\n");
    assert_eq!(
        s.run("env =x"),
        format!("env: cannot set =x: invalid argument\n{}", exit_line("env", 125))
    );
    let usage = "usage: env [-i] [-u NAME]... [NAME=VALUE]... [PROG [ARG...]]\n";
    assert_eq!(s.run("env -x"), format!("{usage}{}", exit_line("env", 125)));
    assert_eq!(
        s.run("env nope"),
        format!("env: nope: {enoent}\n{}", exit_line("env", 127))
    );
    assert_eq!(
        s.run("env /bin"),
        format!("env: /bin: not a regular file\n{}", exit_line("env", 126))
    );
    assert_eq!(s.run("printenv TERM HOME"), "linux\n/\n");
    assert_eq!(s.run("printenv"), "HOME=/\nTERM=linux\n");
    assert_eq!(s.run("printenv HOME=/"), exit_line("printenv", 1));
    assert_eq!(s.run("HOME=/t cd"), "");
    assert_eq!(s.run("ls u"), listing);
    assert_eq!(s.run("cd"), "");
    assert_eq!(s.run("ls t/u"), listing);

    let (r, disk) = s.exit();
    // The shell's status is the last program's (0); a failed self-test
    // comes first.
    assert_eq!(r, RunResult::Exited(expected_failures.len() as u32));
    // The disk is consistent, and holds what the programs left.
    let report = fsck::check(&disk).unwrap_or_else(|p| panic!("{p:#?}"));
    assert_eq!((report.replayed, report.orphans), (0, 0));
    let after = Fs::open(&disk).unwrap();
    assert_eq!(after.ls("/t/u").unwrap(), listing);
    assert_eq!(after.read_file("/t/u/b").unwrap(), readme_bytes);
    assert_eq!(after.read_file("/t/u/echo").unwrap(), b"short\n");
    assert!(after.lookup("/t/u/README").is_err());
    assert!(after.lookup("/t/a").is_err());
    assert!(after.lookup("/t/c").is_err());
    assert!(after.lookup("/t/h").is_err());
    let names: Vec<String> = after
        .ls("/t/u")
        .unwrap()
        .lines()
        .map(|l| l.rsplit(' ').next().unwrap().to_string())
        .collect();
    assert_eq!(names, [".", "..", "echo", "b"]);
}

#[test]
fn each_basic_program_does_its_job_and_reports_its_errors() {
    coreutils_session(root_image(CCompiler::Merlin, Xlen::Rv32), true, Xlen::Rv32);
}

#[test]
fn each_basic_program_does_the_same_on_rv64() {
    coreutils_session(root_image(CCompiler::Merlin, Xlen::Rv64), true, Xlen::Rv64);
}

/// The same session with every program compiled by clang (the issue's
/// "with each available compiler"), for `xlen`: the kernel is the committed
/// one, and `/bin` holds clang's programs, linked by merlin-ld (not the
/// ones the kernel embeds, which its boot self-test reports).
fn clang_session(xlen: Xlen) {
    let clang = xtask::tools::find("clang").expect("clang (the official LLVM install, or on PATH)");
    let dir = std::env::temp_dir().join(format!("xtask-coreutils-clang-{xlen:?}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let user = xtask::userprogs::user_dir(&repo_root());
    xtask::userprogs::compile_clang_for(&clang, &user, &dir, false, xlen).unwrap();
    coreutils_session(root_image(CCompiler::Clang(&dir), xlen), false, xlen);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
#[ignore = "needs clang"]
fn each_basic_program_does_the_same_compiled_by_clang() {
    clang_session(Xlen::Rv32);
}

#[test]
#[ignore = "needs clang"]
fn each_basic_program_does_the_same_compiled_by_clang_on_rv64() {
    clang_session(Xlen::Rv64);
}
