//! The kernel and its programs are built with `-g` (line tables for merlin-dbg),
//! and that changes nothing the machine loads: the committed kernels' segments
//! are byte for byte those of a build without debug information, the programs
//! the kernel embeds and `/bin` holds have none (merlin-ld strips it), and so the
//! root image holds the same programs and kernel either way. The objects
//! programs are linked from on Camelot, `/usr/lib/crt0.o` and `libc.a`, are
//! installed stripped (#240), which leaves none of their line tables; but an
//! object compiled with `-g` keeps a section symbol or two its relocations
//! of the line tables needed (llvm-strip keeps them too), so they are not
//! the objects of a build without `-g`. The programs linked from either are
//! the same.

use std::path::{Path, PathBuf};
use xtask::{fs, kernel, toolchain, userprogs};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The entry point and every segment: type, addresses, sizes, flags and the
/// bytes it loads.
type Loaded = (u64, Vec<(u32, u64, u64, u64, u32, Vec<u8>)>);

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
    // tests in kernel.rs check that) carry line tables, and load the same.
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
    // as does not, and both load the same; without -g it is the installed
    // one, and its debug ELF loads the same and has no line table.
    let user = userprogs::user_dir(&repo_root());
    let prog = userprogs::build_named(&user, "echo").unwrap();
    let plain_prog = toolchain::without_debug_info(|| userprogs::build_named(&user, "echo").unwrap());
    let echo = userprogs::programs(&user)
        .unwrap()
        .into_iter()
        .find(|p| p.stem == "echo")
        .unwrap();
    let prog_objects = userprogs::program_objects(&user, &echo, userprogs::CCompiler::Merlin).unwrap();
    assert!(has_line_table(&prog.debug) && !has_line_table(&prog.elf));
    assert!(loaded(&prog.debug) == loaded(&prog.elf));
    assert!(plain_prog.elf == prog.elf);
    assert!(!has_line_table(&plain_prog.debug) && loaded(&plain_prog.debug) == loaded(&prog.elf));

    // The root image: the same kernel, and every file the same but the
    // objects of /usr/lib, which have the same members, none with a line
    // table, and link the same program.
    let image = fs::build_bootable(&repo_root(), fs::IMAGE_SIZE).unwrap();
    let plain_image =
        toolchain::without_debug_info(|| fs::build_bootable(&repo_root(), fs::IMAGE_SIZE).unwrap());
    assert!(
        camelot_fs::boot::kernel_image(&image).unwrap()
            == camelot_fs::boot::kernel_image(&plain_image).unwrap(),
        "the root image's kernel differs without -g"
    );
    assert_eq!(
        image[..camelot_fs::BSIZE],
        plain_image[..camelot_fs::BSIZE],
        "the boot block"
    );
    let (with, without) = (
        camelot_fs::read::Fs::open(&image).unwrap(),
        camelot_fs::read::Fs::open(&plain_image).unwrap(),
    );
    let paths =
        |f: &camelot_fs::read::Fs| -> Vec<String> { f.walk().unwrap().into_iter().map(|(p, _)| p).collect() };
    assert_eq!(paths(&with), paths(&without));
    let objects = ["/usr/lib/crt0.o", "/usr/lib/libc.a"];
    for p in paths(&with) {
        if objects.contains(&p.as_str()) || with.read_file(&p).is_err() {
            continue;
        }
        assert!(
            with.read_file(&p) == without.read_file(&p),
            "{p} differs without -g"
        );
    }
    let members = |f: &camelot_fs::read::Fs| -> Vec<String> {
        let a = f.read_file("/usr/lib/libc.a").unwrap();
        elf::archive::parse(&a)
            .unwrap()
            .members
            .iter()
            .map(|m| m.name.clone())
            .collect()
    };
    assert_eq!(members(&with), members(&without));
    for f in [&with, &without] {
        let a = f.read_file("/usr/lib/libc.a").unwrap();
        for m in elf::archive::parse(&a).unwrap().members {
            assert!(!has_line_table(m.data), "{} has a line table", m.name);
        }
        assert!(!has_line_table(&f.read_file("/usr/lib/crt0.o").unwrap()));
    }
    let link = |f: &camelot_fs::read::Fs| {
        let input = |p: &str| merlin_ld::Input {
            name: p.into(),
            data: f.read_file(p).unwrap(),
        };
        let mut inputs = vec![input("/usr/lib/crt0.o")];
        inputs.extend(prog_objects.clone());
        inputs.push(input("/usr/lib/libc.a"));
        let files: Vec<(&merlin_ld::Input, bool)> = inputs.iter().map(|i| (i, false)).collect();
        let opts = merlin_ld::Options {
            script: Some(merlin_ld::ScriptSource {
                name: "user.ld".into(),
                text: String::from_utf8(f.read_file("/usr/lib/user.ld").unwrap()).unwrap(),
            }),
            entry: None,
        };
        merlin_ld::strip_debug(&merlin_ld::link_files(&files, &opts).unwrap().elf).unwrap()
    };
    assert!(
        link(&with) == prog.elf && link(&without) == prog.elf,
        "echo links differently"
    );
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
