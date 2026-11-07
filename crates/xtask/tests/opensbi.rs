//! Camelot under the real OpenSBI (#174): the committed kernel,
//! `kernel/kernel.elf`, booted on Grail with OpenSBI's `fw_jump` for
//! the generic platform as its firmware (`--bios FILE`), as QEMU's `virt`
//! boots Linux with `-bios`.
//!
//! The firmware is `tests/opensbi/fw_jump.bin`, OpenSBI v1.9 built for
//! the ISA Excalibur has (no C, no F/D) by `scripts/build-opensbi.sh`;
//! `tests/opensbi/README.md` gives its commit, build and checksum. It
//! finds Grail's devices in the device tree Grail passes it in a1 (spec
//! section 7, "Firmware"): the CLINT as an ACLINT's MSWI and MTIMER, the
//! UART as an 8250, the test device through `syscon-poweroff`. It is a
//! reference, as LLVM is for the toolchain: the kernel must behave the
//! same on it as on Camelot's firmware, and any difference is a bug in
//! one of the two firmwares or an assumption in the kernel that no SBI
//! promises.
//!
//! Like the riscv-tests, it needs nothing a plain `cargo test` lacks:
//! the firmware and the kernel are committed, and building the firmware
//! again is the only step that needs LLVM. So it runs with the suite, and
//! in CI (`cargo xtask ci`).

use grail::{Bios, Machine, MachineConfig, RunResult};
use std::sync::mpsc::{self, Sender};
use xtask::kernel::{BOOT_MAX_STEPS, KERNEL_ELF};
use xtask::repo_root;

const PROMPT: &str = "# ";
const SHELL_BANNER: &str = "Type help for the list of commands.\n";
/// The kernel's first line, where its output starts after the firmware's.
const KERNEL_BANNER: &str = "Camelot v0.1.0 booting...\n";
/// RAM for the machine. OpenSBI's `fw_jump` on the generic platform copies
/// the device tree to 0x8220_0000 (`FW_JUMP_FDT_OFFSET`), 34 MiB into RAM,
/// and gives the kernel that copy: a machine needs more than 34 MiB.
const RAM: usize = 64 << 20;
/// Steps the machine runs between looks at what it printed.
const SLICE: u64 = 500_000;
/// The most steps one command may take.
const COMMAND_MAX_STEPS: u64 = 100_000_000;

/// The kernel's line for the SBI it found, on each firmware: OpenSBI v1.9
/// implements SBI v3.0 and is implementation 1 (the SBI spec's number for
/// it), version 0x10009 (1.9); Camelot's is "CMLT", version 1, of SBI
/// v2.0 (spec section 10, "Kernel traps").
const OPENSBI_SBI_LINE: &str = "sbi: v3.0, implementation 0x1 version 0x10009\n";
const CAMELOT_SBI_LINE: &str = "sbi: v2.0, implementation 0x434d4c54 version 0x1\n";

/// Camelot running on Grail.
struct Session {
    m: Machine,
    input: Sender<u8>,
}

impl Session {
    /// Boots the committed kernel on `harts` harts with `bios` as its
    /// firmware, and runs it to the shell's first prompt.
    fn boot(bios: Bios, harts: usize) -> Session {
        let mut m = Machine::new(MachineConfig {
            ram_size: RAM,
            harts,
            bios,
            ..MachineConfig::default()
        })
        .unwrap();
        m.cpu.bus.uart.capture = true;
        let elf = std::fs::read(repo_root().join("kernel").join(KERNEL_ELF)).unwrap();
        m.load_elf(&elf).unwrap();
        // Nothing is typed until the shell asks: OpenSBI's 8250 driver
        // reads the receive buffer once as it sets the UART up
        // (uart8250_init()), which would take a byte typed ahead.
        let (input, rx) = mpsc::channel();
        m.set_uart_input(rx);
        let mut s = Session { m, input };
        let ready = format!("{SHELL_BANNER}{PROMPT}");
        s.run_until(|out| out.ends_with(&ready), BOOT_MAX_STEPS * harts as u64);
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

    /// Types `line` for the shell and returns what it printed after the
    /// line's echo, up to its next prompt.
    fn command(&mut self, line: &str) -> String {
        let start = self.out().len();
        self.type_line(line);
        let echoed = format!("{line}\n");
        self.run_until(
            |out| out[start..].len() > echoed.len() && out[start..].ends_with(PROMPT),
            COMMAND_MAX_STEPS,
        );
        let new = self.out()[start..].to_string();
        let printed = new.strip_prefix(&echoed).unwrap_or_else(|| panic!("{new}"));
        printed[..printed.len() - PROMPT.len()].to_string()
    }

    fn type_line(&mut self, line: &str) {
        for b in line.bytes().chain(*b"\n") {
            self.input.send(b).unwrap();
        }
    }

    /// Types `exit CODE` and runs the machine until it stops. Returns how
    /// it stopped and what it printed after the line's echo.
    fn exit(mut self, code: u8) -> (RunResult, String) {
        let start = self.out().len();
        let line = format!("exit {code}");
        self.type_line(&line);
        let r = self.m.run(Some(COMMAND_MAX_STEPS), |_| {});
        let new = self.out()[start..].to_string();
        let after = new
            .strip_prefix(&format!("{line}\n"))
            .unwrap_or_else(|| panic!("{new}"));
        (r, after.to_string())
    }
}

/// What the kernel printed, from its banner to the shell's prompt, split
/// into the line for the SBI it found and the rest.
fn kernel_output(out: &str) -> (String, String) {
    let start = out
        .find(KERNEL_BANNER)
        .unwrap_or_else(|| panic!("no kernel banner in:\n{out}"));
    let kernel = &out[start..];
    let sbi = kernel
        .lines()
        .find(|l| l.starts_with("sbi: v"))
        .unwrap_or_else(|| panic!("no sbi line in:\n{kernel}"));
    let sbi = format!("{sbi}\n");
    (sbi.clone(), kernel.replacen(&sbi, "", 1))
}

#[test]
fn camelot_boots_under_opensbi() {
    let fw_jump = repo_root().join("tests/opensbi/fw_jump.bin");
    for harts in [1, 4, 8] {
        // OpenSBI starts, finds Grail's devices in the device tree and
        // enters the kernel at 0x8020_0000 in S-mode (its CRLF line ends
        // are its console's, sbi_puts()).
        let mut theirs = Session::boot(Bios::File(fw_jump.clone()), harts);
        let out = theirs.out();
        let firmware = &out[..out.find(KERNEL_BANNER).unwrap_or(out.len())];
        for line in [
            "OpenSBI v1.9\r\n".to_string(),
            format!("Platform HART Count         : {harts}\r\n"),
            "Platform IPI Device         : aclint-mswi\r\n".to_string(),
            "Platform Timer Device       : aclint-mtimer @ 10000000Hz\r\n".to_string(),
            "Platform Console Device     : uart8250\r\n".to_string(),
            "Platform Shutdown Device    : syscon-poweroff\r\n".to_string(),
            "Domain0 Next Address        : 0x0000000080200000\r\n".to_string(),
            "Domain0 Next Mode           : S-mode\r\n".to_string(),
        ] {
            assert!(
                firmware.contains(&line),
                "{harts} harts: no {line:?} in:\n{firmware}"
            );
        }

        // The kernel on Camelot's firmware, on the same machine.
        let mut ours = Session::boot(Bios::Default, harts);

        // The kernel's boot, its self-tests and its harts, line for line
        // the same on both but for the SBI it reports.
        let (their_sbi, their_boot) = kernel_output(&theirs.out());
        let (our_sbi, our_boot) = kernel_output(&ours.out());
        assert_eq!(their_sbi, OPENSBI_SBI_LINE, "{harts} harts");
        assert_eq!(our_sbi, CAMELOT_SBI_LINE, "{harts} harts");
        assert_eq!(their_boot, our_boot, "{harts} harts");
        assert!(!their_boot.contains("FAILED"), "{their_boot}");

        // The shell runs programs on both.
        assert_eq!(theirs.command("echo from OpenSBI"), "from OpenSBI\n");
        assert_eq!(ours.command("echo from OpenSBI"), "from OpenSBI\n");

        // `exit 3` shuts the machine down with the exit code as the
        // reason, which Camelot's firmware gives Grail as its exit code.
        // OpenSBI takes only the standard reasons: the kernel warns, as
        // Linux's sbi_srst_reset() does, and shuts down for "system
        // failure", which OpenSBI's syscon-poweroff does as every
        // shutdown, writing 0x5555 (pass) to the test device.
        let (r, after) = theirs.exit(3);
        assert_eq!(r, RunResult::Exited(0), "{after}");
        assert_eq!(after, "sbi_srst_reset: type=0x0 reason=0xe0000003 failed\n");
        let (r, after) = ours.exit(3);
        assert_eq!(r, RunResult::Exited(3), "{after}");
        assert_eq!(after, "");
    }
}
