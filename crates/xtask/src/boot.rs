//! `cargo xtask boot`: the programs that run before the kernel, built with our
//! own tools, merlin (its preprocessor), merlin-as and merlin-ld, as the
//! examples are; no LLVM.
//!
//! * Grail's boot ROM disk loader, `crates/grail/rom/diskboot.S`: reads the
//!   boot block of disk 0 and jumps to it, on a machine with no firmware
//!   (spec section 7, "Boot from disk"). One source for both widths (#193),
//!   preprocessed for each as the examples are: Grail includes the RV32
//!   build, `diskboot.elf`, in the boot ROM of an RV32 machine, and the RV64
//!   one, `diskboot-rv64.elf`, in an RV64 machine's (#151).
//! * Camelot's firmware, `firmware/` (#174, spec section 7, "Firmware"): the
//!   M-mode SBI implementation, its entry and its disk loader, linked at
//!   0x8000_0000 into `crates/grail/rom/firmware.elf`, which Grail includes
//!   and loads with `--bios default`. RV64 only.
//! * Camelot's bootloader, `kernel/boot/boot.S`: the boot block, which loads
//!   the kernel ELF stored after the filesystem. `camelot-fs mkfs --boot`
//!   (and `cargo xtask fs`) install the linked `boot.elf`.
//!
//! The `.elf` files are committed, and `cargo test` checks they are what the
//! current sources and tools build.

use crate::examples::{self, Built, Xlen};
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
    /// The width it is assembled for.
    pub xlen: Xlen,
}

/// The boot ROM's disk loader, for RV32 machines.
pub const DISKBOOT: Program = Program {
    name: "diskboot",
    dir: "crates/grail/rom",
    sources: &["diskboot.S"],
    script: "link.ld",
    elf_dir: "crates/grail/rom",
    elf: "diskboot.elf",
    xlen: Xlen::Rv32,
};

/// The boot ROM's disk loader, for RV64 machines: the same source and linker
/// script.
pub const DISKBOOT_RV64: Program = Program {
    name: "diskboot-rv64",
    dir: "crates/grail/rom",
    sources: &["diskboot.S"],
    script: "link.ld",
    elf_dir: "crates/grail/rom",
    elf: "diskboot-rv64.elf",
    xlen: Xlen::Rv64,
};

/// Camelot's firmware, for RV64 machines: Grail's `--bios default`.
pub const FIRMWARE: Program = Program {
    name: "firmware",
    dir: "firmware",
    sources: &["entry.S", "sbi.S", "disk.S"],
    script: "firmware.ld",
    elf_dir: "crates/grail/rom",
    elf: "firmware.elf",
    xlen: Xlen::Rv64,
};

/// Camelot's bootloader, for RV32 machines and kernels.
pub const BOOTLOADER: Program = Program {
    name: "boot",
    dir: "kernel/boot",
    sources: &["boot.S"],
    script: "link.ld",
    elf_dir: "kernel/boot",
    elf: "boot.elf",
    xlen: Xlen::Rv32,
};

/// Camelot's bootloader for RV64 machines, which loads the RV64 kernel
/// (ELF64) in S-mode, the firmware's next stage: the same source and linker
/// script.
pub const BOOTLOADER_RV64: Program = Program {
    name: "boot-rv64",
    dir: "kernel/boot",
    sources: &["boot.S"],
    script: "link.ld",
    elf_dir: "kernel/boot",
    elf: "boot-rv64.elf",
    xlen: Xlen::Rv64,
};

pub const PROGRAMS: &[Program] = &[DISKBOOT, DISKBOOT_RV64, FIRMWARE, BOOTLOADER, BOOTLOADER_RV64];

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
    /// `target/boot`, and `target/boot/rv64` for RV64 (one source serves
    /// both widths).
    pub fn obj_dir(&self) -> &'static str {
        match self.xlen {
            Xlen::Rv32 => "target/boot",
            Xlen::Rv64 => "target/boot/rv64",
        }
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
                self.xlen,
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
        p.xlen,
    )
}

/// The bootloader's executable as the current source builds it.
pub fn bootloader(root: &Path) -> Result<Vec<u8>, String> {
    Ok(build(root, &BOOTLOADER)?.elf)
}

/// The RV64 bootloader's executable as the current source builds it.
pub fn bootloader_rv64(root: &Path) -> Result<Vec<u8>, String> {
    Ok(build(root, &BOOTLOADER_RV64)?.elf)
}

/// The firmware's executable as the current sources build it.
pub fn firmware(root: &Path) -> Result<Vec<u8>, String> {
    Ok(build(root, &FIRMWARE)?.elf)
}
