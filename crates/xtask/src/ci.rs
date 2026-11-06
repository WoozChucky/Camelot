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
//!   trips and mapping symbols, merlin-ld's links (riscv-tests relinked, the
//!   fixtures current), merlin-objdump's goldens, the compiler's end-to-end
//!   harness, riscv-tests on Grail;
//! - Grail's display controller, and its keyboard in both of its modes, through
//!   `examples/kbd-echo`;
//! - the committed binaries are what the tools build now: the kernel, the
//!   examples, the boot programs, hello, the disk image; and the kernel boots,
//!   on 1 to 8 harts, under Sv48 and Sv39, from disk, and panics as it should;
//! - processes' system call ABI: results sign-extended, pointers into the
//!   kernel's half refused, 64-bit values through the calls; memory mappings
//!   (maptest);
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
        what: "decoder and disassembler against llvm-objdump, RV64's and RV32's",
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
        what: "merlin-ld: links, riscv-tests relinked, fixtures current",
        args: &["-p", "merlin-ld", "--tests"],
    },
    Step {
        what: "merlin-objdump against llvm-objdump, RV64's and RV32's",
        args: &["-p", "merlin-objdump", "--tests"],
    },
    Step {
        what: "merlin: the compiler end to end",
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
        what: "Avalon: every VM is RV64 (an ELF32 program refused), and its RAM",
        args: &[
            "-p",
            "avalon",
            "--test",
            "server",
            "--",
            "every_vm_is_rv64",
            "a_vm_has_more_ram_than_the_host",
        ],
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
        what: "Grail: riscv-tests, RV64, paging and atomics semantics, disk boot, firmware and the \
               device tree, the committed disk image, the display controller, the keyboard's two modes",
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
            "firmware",
            "--test",
            "block",
            "--test",
            "framebuffer",
            "--test",
            "keyboard",
        ],
    },
    Step {
        what: "the committed kernel is current, linked high, boots on 1-8 harts, under Sv39 and from \
               disk, panics as it should, and the shell's built-in statuses are dash's",
        args: &[
            "-p",
            "xtask",
            "--test",
            "kernel",
            "--",
            "committed_kernel_is_what_our_toolchain_builds",
            "committed_kernel_boots",
            "the_kernel_is_linked_high_and_loaded_low",
            "the_kernels_have_no_m_mode_code",
            "kmain_runs_in_s_mode_with_traps_delegated",
            "the_kernel_boots_through_its_self_tests",
            "the_kernel_runs_on_up_to_eight_harts",
            "illegal_instruction_panics_with_a_64_bit_register_dump",
            "a_kernel_stack_overflow_hits_the_guard_page_and_is_reported",
            "the_kernel_page_table_maps_the_kernel_half_only",
            "the_kernel_falls_back_to_sv39",
            "the_kernel_boots_from_disk",
            "a_failed_builtin_gives_dashs_status",
        ],
    },
    Step {
        what: "the kernel's SBI: every call and error, a fence before every IPI (#170)",
        args: &["-p", "xtask", "--test", "sbi"],
    },
    Step {
        what: "the kernel on OpenSBI v1.9's fw_jump, the reference firmware, on 1, 4 and 8 harts",
        args: &["-p", "xtask", "--test", "opensbi"],
    },
    Step {
        what: "processes: the LP64 system call ABI, sign-extended results, the kernel half refused, \
               Linux's start as pid 1, the environment (#216), memory mappings (#179)",
        args: &[
            "-p",
            "xtask",
            "--test",
            "processes",
            "--",
            "negative_results_arrive_sign_extended",
            "pointers_into_the_kernel_half_are_refused",
            "the_calls_take_and_return_64_bit_values",
            "a_program_starts_as_linux_starts_one",
            "the_shell_passes_its_environment_on",
        ],
    },
    Step {
        what: "the committed examples, boot programs and hello are current; a boot block and the root \
               image boot",
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
            "a_boot_block_boots_through_the_rom",
            "the_root_image_boots_from_disk_and_runs_programs_from_bin",
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
            "the_firmware_grail_the_kernel_and_the_documents_agree",
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
