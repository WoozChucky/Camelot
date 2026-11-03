//! The kernel and its programs are built with `-g` (line tables for merlin-dbg),
//! and that changes nothing the machine loads: the committed kernels' segments
//! are byte for byte those of a build without debug information, the programs
//! the kernel embeds and `/bin` holds have none (merlin-ld strips it), and so the
//! root image is the same either way.

use std::path::{Path, PathBuf};
use xtask::{fs, kernel, toolchain, userprogs};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The entry point and every segment: type, addresses, sizes, flags and the
/// bytes it loads.
type Loaded = (u32, Vec<(u32, u32, u32, u32, u32, Vec<u8>)>);

fn loaded(elf: &[u8]) -> Loaded {
    let e = elf::Elf::parse(elf).unwrap();
    let segments = e
        .segments
        .iter()
        .map(|p| {
            (
                p.ty,
                p.vaddr,
                p.filesz,
                p.memsz,
                p.flags,
                e.segment_data(p).unwrap().to_vec(),
            )
        })
        .collect();
    (e.header.entry, segments)
}

fn has_line_table(elf: &[u8]) -> bool {
    let e = elf::Elf::parse(elf).unwrap();
    e.sections.iter().any(|s| s.name == ".debug_line")
}

fn committed(name: &str) -> Vec<u8> {
    let path = repo_root().join("kernel").join(name);
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

#[test]
fn debug_information_changes_no_loaded_byte() {
    let kernel_dir = repo_root().join("kernel");
    let plain = toolchain::without_debug_info(|| kernel::build_merlin(&kernel_dir, None).unwrap());
    let plain_disk = toolchain::without_debug_info(|| kernel::disk_kernel(&kernel_dir, &plain).unwrap());
    assert!(!has_line_table(&plain.elf));
    // The committed kernels (what `cargo xtask kernel` builds, with -g: the
    // test in kernel.rs checks that) carry line tables, and load the same.
    for (name, plain) in [
        (kernel::KERNEL_ELF, &plain.elf),
        (kernel::DISK_KERNEL, &plain_disk),
    ] {
        let with = committed(name);
        assert!(has_line_table(&with), "{name} has no line table");
        assert!(
            loaded(&with) == loaded(plain),
            "{name} loads differently without -g"
        );
    }

    // A program: its debug ELF has the line tables, the one it is installed
    // as does not, and both load the same; without -g it is the installed one.
    let user = userprogs::user_dir(&repo_root());
    let prog = userprogs::build_named(&user, "echo").unwrap();
    let plain_prog = toolchain::without_debug_info(|| userprogs::build_named(&user, "echo").unwrap());
    assert!(has_line_table(&prog.debug) && !has_line_table(&prog.elf));
    assert!(loaded(&prog.debug) == loaded(&prog.elf));
    assert!(plain_prog.elf == prog.elf && plain_prog.debug == prog.elf);

    // The root image, programs and kernel together.
    let image = fs::build_bootable(&repo_root(), fs::IMAGE_SIZE).unwrap();
    let plain_image =
        toolchain::without_debug_info(|| fs::build_bootable(&repo_root(), fs::IMAGE_SIZE).unwrap());
    assert!(image == plain_image, "the root image differs without -g");
}

#[test]
fn the_kernel_line_table_names_its_sources() {
    // Paths relative to the repository root: a debugger run there finds them.
    let elf = committed(kernel::KERNEL_ELF);
    let e = elf::Elf::parse(&elf).unwrap();
    let tables = e.line_tables().unwrap();
    let files: Vec<&str> = tables
        .iter()
        .flat_map(|t| t.files.iter().map(|f| f.as_str()))
        .collect();
    for f in ["kernel/main.c", "kernel/net/arp.c", "kernel/include/mmio.h"] {
        assert!(files.contains(&f), "no {f} in {files:?}");
    }
    assert!(files.iter().all(|f| f.starts_with("kernel/")), "{files:?}");
    let main = repo_root().join("kernel/main.c");
    assert!(main.exists());
}
