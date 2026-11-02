//! `cargo xtask boot`: the two programs that run before the kernel when a
//! machine boots from its disk (spec section 7, "Boot from disk"), built with
//! our own tools, merlin-as and merlin-ld, as the examples are; no LLVM.
//!
//! * Grail's boot ROM disk loader, `crates/grail/rom/diskboot.s`: reads the
//!   boot block of disk 0 and jumps to it. Grail includes the linked
//!   `diskboot.elf` in its boot ROM.
//! * Camelot's bootloader, `kernel/boot/boot.s`: the boot block, which loads
//!   the kernel ELF stored after the filesystem. `camelot-fs mkfs --boot`
//!   (and `cargo xtask fs`) install the linked `boot.elf`.
//!
//! Both `.elf` files are committed, and `cargo test` checks they are what the
//! current sources and tools build.

use crate::examples::{self, Built};
use std::path::{Path, PathBuf};

/// One program: a source and a linker script in `dir`, linked to `elf` there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Program {
    pub name: &'static str,
    /// Its directory, relative to the repository root, with `/`.
    pub dir: &'static str,
    pub source: &'static str,
    pub script: &'static str,
    /// The linked executable's file name in `dir`.
    pub elf: &'static str,
}

/// The boot ROM's disk loader.
pub const DISKBOOT: Program = Program {
    name: "diskboot",
    dir: "crates/grail/rom",
    source: "diskboot.s",
    script: "link.ld",
    elf: "diskboot.elf",
};

/// Camelot's bootloader.
pub const BOOTLOADER: Program = Program {
    name: "boot",
    dir: "kernel/boot",
    source: "boot.s",
    script: "link.ld",
    elf: "boot.elf",
};

pub const PROGRAMS: &[Program] = &[DISKBOOT, BOOTLOADER];

impl Program {
    /// The committed executable under `root`.
    pub fn elf_path(&self, root: &Path) -> PathBuf {
        root.join(self.dir).join(self.elf)
    }

    /// The executable's path relative to the repository root, with `/`.
    pub fn elf_name(&self) -> String {
        format!("{}/{}", self.dir, self.elf)
    }

    /// The equivalent command lines.
    pub fn commands(&self) -> [String; 2] {
        let obj = format!("target/boot/{}", examples::object_name(self.source));
        [
            format!("merlin-as {}/{} -o {obj}", self.dir, self.source),
            format!(
                "merlin-ld -T {}/{} {obj} -o {}",
                self.dir,
                self.script,
                self.elf_name()
            ),
        ]
    }
}

/// Builds `p` in memory.
pub fn build(root: &Path, p: &Program) -> Result<Built, String> {
    examples::assemble_and_link(&root.join(p.dir), p.dir, p.name, &[p.source], Some(p.script))
}

/// The bootloader's executable as the current source builds it.
pub fn bootloader(root: &Path) -> Result<Vec<u8>, String> {
    Ok(build(root, &BOOTLOADER)?.elf)
}
