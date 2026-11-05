//! `cargo xtask boot`: the programs that run before the kernel, built with our
//! own tools, merlin (its preprocessor), merlin-as and merlin-ld, as the
//! examples are; no LLVM.
//!
//! * Grail's boot ROM disk loader, `crates/grail/rom/diskboot.S`: reads the
//!   boot block of disk 0 and jumps to it, on a machine with no firmware
//!   (spec section 7, "Boot from disk"), preprocessed as the examples are:
//!   Grail includes `diskboot.elf` in its boot ROM.
//! * Camelot's firmware, `firmware/` (#174, spec section 7, "Firmware"): the
//!   M-mode SBI implementation, its entry and its disk loader, linked at
//!   0x8000_0000 into `crates/grail/rom/firmware.elf`, which Grail includes
//!   and loads with `--bios default`.
//! * Camelot's bootloader, `kernel/boot/boot.S`: the boot block, the
//!   firmware's next stage, which loads the kernel ELF stored after the
//!   filesystem. `camelot-fs mkfs --boot` (and `cargo xtask fs`) install the
//!   linked `boot.elf`.
//!
//! The `.elf` files are committed, and `cargo test` checks they are what the
//! current sources and tools build.

use crate::examples::{self, Built};
use std::path::{Path, PathBuf};

/// One program: sources and a linker script in `dir`, linked to `elf` in
/// `elf_dir`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Program {
    pub name: &'static str,
    /// Its sources' directory, relative to the repository root, with `/`.
    pub dir: &'static str,
    /// Its sources, in link order. The first one's comments give the
    /// commands that build it ([`Program::commands`]).
    pub sources: &'static [&'static str],
    pub script: &'static str,
    /// The directory of the linked executable, relative to the repository
    /// root, with `/`.
    pub elf_dir: &'static str,
    /// The linked executable's file name in `elf_dir`.
    pub elf: &'static str,
}

/// The boot ROM's disk loader.
pub const DISKBOOT: Program = Program {
    name: "diskboot",
    dir: "crates/grail/rom",
    sources: &["diskboot.S"],
    script: "link.ld",
    elf_dir: "crates/grail/rom",
    elf: "diskboot.elf",
};

/// Camelot's firmware: Grail's `--bios default`.
pub const FIRMWARE: Program = Program {
    name: "firmware",
    dir: "firmware",
    sources: &["entry.S", "sbi.S", "disk.S"],
    script: "firmware.ld",
    elf_dir: "crates/grail/rom",
    elf: "firmware.elf",
};

/// Camelot's bootloader, which loads the kernel (ELF64) in S-mode, the
/// firmware's next stage.
pub const BOOTLOADER: Program = Program {
    name: "boot",
    dir: "kernel/boot",
    sources: &["boot.S"],
    script: "link.ld",
    elf_dir: "kernel/boot",
    elf: "boot.elf",
};

pub const PROGRAMS: &[Program] = &[DISKBOOT, FIRMWARE, BOOTLOADER];

impl Program {
    /// The committed executable under `root`.
    pub fn elf_path(&self, root: &Path) -> PathBuf {
        root.join(self.elf_dir).join(self.elf)
    }

    /// The executable's path relative to the repository root, with `/`.
    pub fn elf_name(&self) -> String {
        format!("{}/{}", self.elf_dir, self.elf)
    }

    /// Where the commands leave the preprocessed sources and the objects:
    /// `target/boot`.
    pub fn obj_dir(&self) -> &'static str {
        "target/boot"
    }

    /// The equivalent command lines: each source preprocessed and
    /// assembled, then the objects linked.
    pub fn commands(&self) -> Vec<String> {
        let obj_dir = self.obj_dir();
        let mut cmds = Vec::new();
        for source in self.sources {
            cmds.extend(examples::assemble_commands(
                self.dir,
                source,
                &[examples::INCLUDE_DIR],
                obj_dir,
            ));
        }
        let objects: Vec<String> = self
            .sources
            .iter()
            .map(|s| format!("{obj_dir}/{}", examples::object_name(s)))
            .collect();
        cmds.push(format!(
            "merlin-ld -T {}/{} {} -o {}",
            self.dir,
            self.script,
            objects.join(" "),
            self.elf_name()
        ));
        cmds
    }
}

/// Builds `p` in memory.
pub fn build(root: &Path, p: &Program) -> Result<Built, String> {
    examples::assemble_and_link(
        root,
        p.dir,
        p.name,
        p.sources,
        &[examples::INCLUDE_DIR],
        Some(p.script),
    )
}

/// The bootloader's executable as the current source builds it.
pub fn bootloader(root: &Path) -> Result<Vec<u8>, String> {
    Ok(build(root, &BOOTLOADER)?.elf)
}

/// The firmware's executable as the current sources build it.
pub fn firmware(root: &Path) -> Result<Vec<u8>, String> {
    Ok(build(root, &FIRMWARE)?.elf)
}
