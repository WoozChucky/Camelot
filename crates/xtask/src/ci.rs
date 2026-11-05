//! `cargo xtask ci`: the tests GitHub's CI runs, on pull requests and on main.
//!
//! Not the whole suite: that is `cargo test --release`, run locally on Windows
//! before a pull request is opened (CLAUDE.md, "Before pushing"). CI runs the
//! checks that are both important and quick, so that a pull request's run takes
//! minutes:
//!
//! - every crate's unit tests;
//! - the toolchain against its references: excalibur's decoder and
//!   disassembler against llvm-objdump's text, the elf crate against
//!   llvm-readelf's and llvm-dwarfdump's, merlin-as's encodings, round
//!   trips and mapping symbols, merlin-ld's links on both widths (riscv-tests relinked, the
//!   fixtures current),
//!   merlin-objdump's goldens, the compiler's end-to-end harness, riscv-tests
//!   on Grail;
//! - Grail's display controller, and its keyboard in both of its modes, through
//!   `examples/kbd-echo`;
//! - the committed binaries are what the tools build now: the kernel, the
//!   examples, the boot programs, hello, the disk image; and the kernel boots;
//! - RV64 processes' system call ABI: results sign-extended, pointers into the
//!   kernel's half refused, 64-bit values through the calls;
//! - how Avalon and Lake end connections: bad handshakes, and a server's last
//!   frame arriving whole though the client sent more (Linux's TCP stack is not
//!   Windows's here);
//! - a VM's debugger over Avalon: the stub and its run states, and merlin-dbg
//!   breaking in the kernel of a VM with Herald attached;
//! - the tests that check the documents against the code (instant).
//!
//! Left to the local run: the kernel's filesystem integration tests and their
//! crash tests, the multi-hart, threads and self-hosting runs, networking, the
//! Avalon platform and its clients. They boot Camelot on Grail, many of them
//! hundreds of times, and take most of the suite's time.

use std::process::Command;

/// One `cargo test --release` invocation: its arguments after `--release`.
struct Step {
    what: &'static str,
    args: &'static [&'static str],
}

const STEPS: &[Step] = &[
    Step {
        what: "unit tests, every crate",
        args: &["--workspace", "--lib", "--bins"],
    },
    Step {
        what: "decoder and disassembler against llvm-objdump, both widths",
        args: &["-p", "excalibur", "--test", "disasm"],
    },
    Step {
        what: "the elf crate against llvm-readelf and llvm-dwarfdump, both classes",
        args: &["-p", "elf", "--test", "llvm"],
    },
    Step {
        what: "merlin-as: encodings, round trips, line tables, mapping symbols, hello",
        args: &["-p", "merlin-as", "--tests"],
    },
    Step {
        what: "merlin-ld: links, riscv-tests relinked, fixtures current, both widths",
        args: &["-p", "merlin-ld", "--tests"],
    },
    Step {
        what: "merlin-objdump against llvm-objdump, both widths",
        args: &["-p", "merlin-objdump", "--tests"],
    },
    Step {
        what: "merlin: the compiler end to end, both widths",
        args: &["-p", "merlin", "--tests"],
    },
    Step {
        what: "camelot-fs: mkfs, fsck, the command",
        args: &["-p", "camelot-fs", "--tests"],
    },
    Step {
        what: "Avalon protocol",
        args: &["-p", "avalon-proto", "--tests"],
    },
    // How the servers close connections depends on the host's TCP stack (Linux and
    // Windows answer a close with unread input differently), and CI runs on Linux.
    Step {
        what: "Avalon: bad handshakes, and the last frame before a close arrives whole",
        args: &[
            "-p",
            "avalon",
            "--test",
            "errors",
            "--",
            "bad_handshakes",
            "the_last_frame_survives_unread_input",
        ],
    },
    Step {
        what: "Avalon: a VM's GDB stub over the connection, its run states",
        args: &["-p", "avalon", "--test", "debug"],
    },
    Step {
        what: "merlin-dbg on an Avalon VM: the kernel's write system call, Herald attached",
        args: &["-p", "merlin-dbg", "--test", "avalon"],
    },
    Step {
        what: "Lake: bad handshakes, and the last frame before a close arrives whole",
        args: &[
            "-p",
            "lake",
            "--test",
            "protocol",
            "--",
            "bad_handshakes",
            "the_last_frame_survives_unread_input",
            "a_handshake_out_of_time_gets_no_reply",
        ],
    },
    Step {
        what: "Grail: riscv-tests, RV64, paging and atomics semantics, disk boot, the committed disk \
               image, the display controller, the keyboard's two modes",
        args: &[
            "-p",
            "grail",
            "--test",
            "riscv_tests",
            "--test",
            "rv64",
            "--test",
            "paging",
            "--test",
            "atomics",
            "--test",
            "disk_boot",
            "--test",
            "block",
            "--test",
            "framebuffer",
            "--test",
            "keyboard",
        ],
    },
    Step {
        what: "the committed kernel is current and boots",
        args: &[
            "-p",
            "xtask",
            "--test",
            "kernel",
            "--",
            "committed_kernel_is_what_our_toolchain_builds",
            "committed_kernel_boots",
            "committed_kernel_layout",
        ],
    },
    Step {
        what: "the committed RV64 kernel is current, boots on 1-8 harts and panics as it should",
        args: &["-p", "xtask", "--test", "kernel_rv64"],
    },
    Step {
        what: "RV64 processes: the LP64 system call ABI, sign-extended results, the kernel half refused",
        args: &[
            "-p",
            "xtask",
            "--test",
            "processes_rv64",
            "--",
            "negative_results_arrive_sign_extended",
            "pointers_into_the_kernel_half_are_refused",
            "the_calls_take_and_return_64_bit_values",
        ],
    },
    Step {
        what: "the committed examples, boot programs and hello are current; an RV64 boot block boots",
        args: &[
            "-p",
            "xtask",
            "--test",
            "examples",
            "--test",
            "boot",
            "--test",
            "hello",
            "--",
            "committed_examples_are_current",
            "committed_boot_programs_are_up_to_date",
            "an_rv64_boot_block_boots_through_the_rv64_rom",
            "hello",
        ],
    },
    Step {
        what: "the documents agree with the code",
        args: &[
            "-p",
            "xtask",
            "--test",
            "boot",
            "--test",
            "fs",
            "--test",
            "kernel",
            "--test",
            "up",
            "--",
            "the_rom_the_bootloader_mkfs_and_the_documents_agree",
            "the_spec_gives_the_root_images_size",
            "the_format_document_matches",
            "the_system_call_document_matches",
            "the_readme_quick_start",
        ],
    },
    Step {
        what: "the README's Avalon client example",
        args: &[
            "-p",
            "avalon",
            "--test",
            "server",
            "--",
            "the_readme_client_example",
        ],
    },
    Step {
        what: "the README's Lake example",
        args: &["-p", "lake", "--test", "avalon", "--", "the_readme_lake_example"],
    },
];

/// Runs every step, all of them even after one fails, and says which failed.
/// Returns whether all passed.
pub fn run() -> bool {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let mut failed = Vec::new();
    for (n, step) in STEPS.iter().enumerate() {
        println!("xtask ci: [{}/{}] {}", n + 1, STEPS.len(), step.what);
        let ok = Command::new(&cargo)
            .args(["test", "--release"])
            .args(step.args)
            .status()
            .is_ok_and(|s| s.success());
        if !ok {
            failed.push(step.what);
        }
    }
    if failed.is_empty() {
        println!("xtask ci: all {} steps passed", STEPS.len());
    } else {
        println!("xtask ci: {} of {} steps failed:", failed.len(), STEPS.len());
        for what in &failed {
            println!("  {what}");
        }
    }
    failed.is_empty()
}
