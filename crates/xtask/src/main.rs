//! `cargo xtask <task> [options]`

use std::path::PathBuf;
use std::process::ExitCode;
use xtask::kernel::{Assembler, Build, Toolchain};
use xtask::{examples, repo_root, tools};

/// The usage text, with `{image_size}` where [`usage`] puts the root image's default
/// size, from `xtask::fs::IMAGE_SIZE`, so the text cannot go stale.
const USAGE: &str = "usage: cargo xtask <task> [options]

tasks:
  up           bring the Avalon platform up: build it, put the Camelot root image in Lake's
               images directory (target/platform/images) as the image os-base, and run Lake,
               Avalon and Pendragon until Ctrl-C. --demo also starts a Camelot VM, vm001;
               cargo xtask up --help lists the options
  examples     build every example (or the ones named) with merlin-as + merlin-ld into
               examples/<dir>/<name>.elf; no LLVM needed. Objects go to target/examples.
               --check: build in memory and report stale .elf files instead of writing
               -v: print the equivalent merlin-as/merlin-ld command lines
  kernel       build kernel/kernel.elf with our own toolchain (merlin, merlin-as, merlin-ld;
               no LLVM), with the user programs of kernel/user built the same way and embedded
               as data (their generated userprogs.s goes to target/kernel/obj with the objects),
               and kernel/kernel-disk.elf, the same kernel without the user programs built in
               (it runs them from /bin: the kernel of a bootable root image).
               --cc clang: the reference build instead (clang -O2 + ld.lld from the official
               LLVM install, the user programs' C compiled by clang too), in target/kernel/clang
  fs           build the root filesystem image (target/camelot-fs.img): kernel/fs/ at /,
               the user programs in /bin (merlin, merlin-as, merlin-ld; no LLVM), /dev/console,
               made bootable with kernel/boot/boot.s and kernel/kernel-disk.elf
               (grail --disk target/camelot-fs.img boots it, in the default 4 MiB of RAM)
               -o PATH: output file; --size N: the image's size in blocks (1 KiB each, default
               {image_size}), the filesystem filling what the kernel leaves;
               --no-boot: the filesystem alone, N blocks
  boot         build the boot ROM's disk loader (crates/grail/rom/diskboot.elf) and
               Camelot's bootloader (kernel/boot/boot.elf) with merlin-as + merlin-ld
               --check: build in memory and report stale .elf files instead of writing
               -v: print the equivalent merlin-as/merlin-ld command lines
  hello        reference build: assemble examples/hello/hello.S with merlin-as, link it
               with ld.lld into crates/merlin-as/tests/golden/hello.elf; takes --lld and -v
  ld-fixtures  regenerate crates/merlin-ld/tests/fixtures (clang, ld.lld, llvm-readobj, llvm-objdump);
               takes --clang, --lld and -v like kernel

kernel options:
  --cc merlin|clang  C compiler (default: merlin); clang is the reference build
  --clang PATH       clang to use (default: PATH, then C:\\Program Files\\LLVM\\bin)
  --lld PATH         ld.lld to use (default: next to clang, then the same search)
  --as clang|merlin  assembler for the .s files with --cc clang (default: clang)
  -o PATH            output file (default: kernel/kernel.elf; with --cc clang,
                     target/kernel/clang/kernel.elf). The merlin build writes the disk
                     kernel next to it, named with -disk (kernel-disk.elf)
  --run              run the result on Grail afterwards, with this terminal as its
                     console (type exit to stop it); exit with its exit code
  -v, --verbose      print each command (for merlin, its command-line equivalent)";

/// [`USAGE`] with the default root image size filled in.
fn usage() -> String {
    let blocks = xtask::fs::IMAGE_SIZE;
    USAGE.replace("{image_size}", &format!("{blocks}: {} MiB", blocks / 1024))
}

/// `--cc`: which C compiler builds the kernel.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Cc {
    /// Our own toolchain: how kernel/kernel.elf is built.
    #[default]
    Merlin,
    /// The reference build.
    Clang,
}

#[derive(Debug, Default, PartialEq, Eq)]
struct KernelArgs {
    cc: Option<Cc>,
    clang: Option<PathBuf>,
    lld: Option<PathBuf>,
    assembler: Option<Assembler>,
    out: Option<PathBuf>,
    run: bool,
    verbose: bool,
}

fn parse_kernel_args(args: &[String]) -> Result<KernelArgs, String> {
    let mut k = KernelArgs::default();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = |name: &str| it.next().cloned().ok_or_else(|| format!("{name} needs a value"));
        match a.as_str() {
            "--clang" => k.clang = Some(value("--clang")?.into()),
            "--lld" => k.lld = Some(value("--lld")?.into()),
            "-o" => k.out = Some(value("-o")?.into()),
            "--as" => {
                k.assembler = Some(match value("--as")?.as_str() {
                    "clang" => Assembler::Clang,
                    "merlin" | "merlin-as" => Assembler::Merlin,
                    other => return Err(format!("unknown assembler {other} (clang or merlin)")),
                })
            }
            "--cc" => {
                k.cc = Some(match value("--cc")?.as_str() {
                    "merlin" => Cc::Merlin,
                    "clang" => Cc::Clang,
                    other => return Err(format!("unknown compiler {other} (merlin or clang)")),
                })
            }
            "--run" => k.run = true,
            "-v" | "--verbose" => k.verbose = true,
            other => return Err(format!("unknown option {other}")),
        }
    }
    Ok(k)
}

/// Checks the options `cargo xtask kernel` got fit together: LLVM's tools
/// only matter to the reference build.
fn check_kernel_args(k: &KernelArgs) -> Result<(), String> {
    if k.cc.unwrap_or_default() == Cc::Merlin {
        for (given, name) in [
            (k.clang.is_some(), "--clang"),
            (k.lld.is_some(), "--lld"),
            (k.assembler.is_some(), "--as"),
        ] {
            if given {
                return Err(format!(
                    "{name} is for the reference build only (--cc clang); merlin needs no LLVM"
                ));
            }
        }
    }
    Ok(())
}

#[derive(Debug, Default, PartialEq, Eq)]
struct FsArgs {
    out: Option<PathBuf>,
    size: Option<u32>,
    no_boot: bool,
}

fn parse_fs_args(args: &[String]) -> Result<FsArgs, String> {
    let mut f = FsArgs::default();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = |name: &str| it.next().cloned().ok_or_else(|| format!("{name} needs a value"));
        match a.as_str() {
            "-o" => f.out = Some(value("-o")?.into()),
            "--size" => {
                let v = value("--size")?;
                f.size = Some(v.parse().map_err(|_| format!("bad size {v} (blocks)"))?);
            }
            "--no-boot" => f.no_boot = true,
            other => return Err(format!("unknown option {other}")),
        }
    }
    Ok(f)
}

#[derive(Debug, Default, PartialEq, Eq)]
struct ExamplesArgs {
    /// Examples named on the command line; empty for all of them.
    names: Vec<String>,
    check: bool,
    verbose: bool,
}

fn parse_examples_args(args: &[String]) -> Result<ExamplesArgs, String> {
    let mut e = ExamplesArgs::default();
    for a in args {
        match a.as_str() {
            "--check" => e.check = true,
            "-v" | "--verbose" => e.verbose = true,
            other if other.starts_with('-') => return Err(format!("unknown option {other}")),
            name if examples::find(name).is_some() => e.names.push(name.to_string()),
            name => {
                let known: Vec<&str> = examples::EXAMPLES.iter().map(|e| e.name).collect();
                return Err(format!("unknown example {name} (one of: {})", known.join(", ")));
            }
        }
    }
    Ok(e)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("examples") => match parse_examples_args(&args[1..]) {
            Ok(e) => build_examples(e),
            Err(e) => {
                eprintln!("xtask: {e}\n{}", usage());
                ExitCode::from(2)
            }
        },
        Some("kernel") => match parse_kernel_args(&args[1..]).and_then(|k| check_kernel_args(&k).map(|()| k))
        {
            Ok(k) => kernel(k),
            Err(e) => {
                eprintln!("xtask: {e}\n{}", usage());
                ExitCode::from(2)
            }
        },
        Some("fs") => match parse_fs_args(&args[1..]) {
            Ok(f) => build_fs(f),
            Err(e) => {
                eprintln!("xtask: {e}\n{}", usage());
                ExitCode::from(2)
            }
        },
        Some("boot") => match parse_examples_args(&args[1..]) {
            Ok(e) if e.names.is_empty() => build_boot(e),
            Ok(_) | Err(_) => {
                eprintln!("xtask: boot takes only --check and -v\n{}", usage());
                ExitCode::from(2)
            }
        },
        Some("hello") => match parse_kernel_args(&args[1..]) {
            Ok(k)
                if k.clang.is_none()
                    && k.assembler.is_none()
                    && k.cc.is_none()
                    && k.out.is_none()
                    && !k.run =>
            {
                hello(k)
            }
            Ok(_) => {
                eprintln!("xtask: hello takes only --lld and -v\n{}", usage());
                ExitCode::from(2)
            }
            Err(e) => {
                eprintln!("xtask: {e}\n{}", usage());
                ExitCode::from(2)
            }
        },
        Some("ld-fixtures") => match parse_kernel_args(&args[1..]) {
            Ok(k) if k.assembler.is_none() && k.cc.is_none() && k.out.is_none() && !k.run => ld_fixtures(k),
            Ok(_) => {
                eprintln!("xtask: ld-fixtures takes only --clang, --lld and -v\n{}", usage());
                ExitCode::from(2)
            }
            Err(e) => {
                eprintln!("xtask: {e}\n{}", usage());
                ExitCode::from(2)
            }
        },
        Some("up") => match xtask::up::parse_args(&args[1..], &repo_root()) {
            Ok(Some(options)) => up(options),
            Ok(None) => {
                println!("{}", xtask::up::USAGE);
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("xtask: {e}\n{}", xtask::up::USAGE);
                ExitCode::from(2)
            }
        },
        Some("-h" | "--help" | "help") => {
            println!("{}", usage());
            ExitCode::SUCCESS
        }
        Some(other) => {
            eprintln!("xtask: unknown task {other}\n{}", usage());
            ExitCode::from(2)
        }
        None => {
            eprintln!("{}", usage());
            ExitCode::from(2)
        }
    }
}

/// `cargo xtask up`: the platform until Ctrl-C, its output on stdout.
fn up(options: xtask::up::Options) -> ExitCode {
    let stop = xtask::up::on_ctrl_c();
    let mut print = |line: &str| println!("{line}");
    match xtask::up::run(&options, stop, &mut print) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("xtask: up: {e}");
            ExitCode::FAILURE
        }
    }
}

/// `cargo xtask examples`: merlin-as + merlin-ld for every example (or the
/// named ones). Writes each object to `target/examples/<dir>/` and each
/// executable over the committed `examples/<dir>/<name>.elf`; with `--check`,
/// writes nothing and fails if a committed executable is not what the current
/// tools build.
fn build_examples(args: ExamplesArgs) -> ExitCode {
    let root = repo_root();
    let selected: Vec<&examples::Example> = if args.names.is_empty() {
        examples::EXAMPLES.iter().collect()
    } else {
        args.names.iter().filter_map(|n| examples::find(n)).collect()
    };
    let mut failed = false;
    for ex in selected {
        if args.verbose {
            for cmd in examples::commands(ex, &format!("target/examples/{}", ex.dir)) {
                eprintln!("{cmd}");
            }
        }
        let built = match examples::build(&root, ex) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("{e}");
                failed = true;
                continue;
            }
        };
        for w in &built.warnings {
            eprintln!("merlin-ld: warning: {w}");
        }
        if args.check {
            match examples::check(&root, ex, &built) {
                examples::Status::UpToDate => eprintln!("{} is up to date", ex.elf_name()),
                examples::Status::Stale => {
                    eprintln!(
                        "{} is out of date: run `cargo xtask examples {}`",
                        ex.elf_name(),
                        ex.name
                    );
                    failed = true;
                }
                examples::Status::Unreadable(e) => {
                    eprintln!("{e}");
                    failed = true;
                }
            }
            continue;
        }
        match examples::write(&root, ex, &built) {
            Ok(()) => eprintln!("built {}", ex.elf_name()),
            Err(e) => {
                eprintln!("xtask: cannot write {}: {e}", ex.name);
                failed = true;
            }
        }
    }
    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

/// `cargo xtask boot`: the boot ROM's disk loader and Camelot's bootloader,
/// written over their committed `.elf` files; with `--check`, writes nothing
/// and fails if a committed file is not what the current tools build.
fn build_boot(args: ExamplesArgs) -> ExitCode {
    let root = repo_root();
    let mut failed = false;
    for p in xtask::boot::PROGRAMS {
        if args.verbose {
            for cmd in p.commands() {
                eprintln!("{cmd}");
            }
        }
        let built = match xtask::boot::build(&root, p) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("{e}");
                failed = true;
                continue;
            }
        };
        for w in &built.warnings {
            eprintln!("merlin-ld: warning: {w}");
        }
        let path = p.elf_path(&root);
        if args.check {
            match std::fs::read(&path) {
                Ok(committed) if committed == built.elf => eprintln!("{} is up to date", p.elf_name()),
                Ok(_) => {
                    eprintln!("{} is out of date: run `cargo xtask boot`", p.elf_name());
                    failed = true;
                }
                Err(e) => {
                    eprintln!("cannot read {}: {e}", path.display());
                    failed = true;
                }
            }
            continue;
        }
        match std::fs::write(&path, &built.elf) {
            Ok(()) => eprintln!("built {}", p.elf_name()),
            Err(e) => {
                eprintln!("xtask: cannot write {}: {e}", path.display());
                failed = true;
            }
        }
    }
    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

/// `cargo xtask fs`: the root filesystem image, bootable unless `--no-boot`,
/// checked with fsck after it is built.
fn build_fs(f: FsArgs) -> ExitCode {
    let root = repo_root();
    let out = f.out.unwrap_or_else(|| xtask::fs::default_out(&root));
    let built = if f.no_boot {
        let mut opts = camelot_fs::mkfs::Options::default();
        if let Some(size) = f.size {
            opts.size = size;
        }
        xtask::fs::build(&root, &opts)
    } else {
        xtask::fs::build_bootable(&root, f.size.unwrap_or(xtask::fs::IMAGE_SIZE))
    };
    let image = match built {
        Ok(image) => image,
        Err(e) => {
            eprintln!("xtask: {e}");
            return ExitCode::FAILURE;
        }
    };
    let report = match camelot_fs::fsck::check(&image) {
        Ok(r) => r,
        Err(problems) => {
            eprintln!("xtask: the image fails fsck:\n{}", problems.join("\n"));
            return ExitCode::FAILURE;
        }
    };
    if let Some(dir) = out.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Err(e) = std::fs::write(&out, &image) {
        eprintln!("xtask: cannot write {}: {e}", out.display());
        return ExitCode::FAILURE;
    }
    let (what, kernel) = match camelot_fs::boot::boot_info(&image) {
        Ok(Some(info)) => (
            format!(" (bootable, {} KiB)", image.len() / 1024),
            format!(", then a {} KiB kernel", info.kernel_sectors / 2),
        ),
        _ => (String::new(), String::new()),
    };
    eprintln!(
        "built {}{what}: a filesystem of {} blocks ({} free) with {} files{kernel}",
        out.display(),
        report.sb.size,
        report.free_blocks,
        report.files,
    );
    ExitCode::SUCCESS
}

fn hello(k: KernelArgs) -> ExitCode {
    let Some(lld) = k.lld.or_else(|| tools::find("ld.lld")) else {
        eprintln!("xtask: ld.lld not found; pass --lld PATH");
        return ExitCode::FAILURE;
    };
    let root = repo_root();
    let work = root.join("target").join("hello");
    let elf = match xtask::hello::build(&root, &lld, &work, k.verbose) {
        Ok(elf) => elf,
        Err(e) => {
            eprintln!("xtask: {e}");
            return ExitCode::FAILURE;
        }
    };
    let out = root.join(xtask::hello::GOLDEN);
    if let Err(e) = std::fs::write(&out, elf) {
        eprintln!("xtask: cannot write {}: {e}", out.display());
        return ExitCode::FAILURE;
    }
    eprintln!("built {}", out.display());
    ExitCode::SUCCESS
}

fn ld_fixtures(k: KernelArgs) -> ExitCode {
    let Some(clang) = k.clang.or_else(|| tools::find("clang")) else {
        eprintln!("xtask: clang not found; pass --clang PATH");
        return ExitCode::FAILURE;
    };
    let (Some(lld), Some(readobj), Some(objdump)) = (
        k.lld.or_else(|| tools::find_beside("ld.lld", &clang)),
        tools::find_beside("llvm-readobj", &clang),
        tools::find_beside("llvm-objdump", &clang),
    ) else {
        eprintln!(
            "xtask: ld.lld, llvm-readobj and llvm-objdump must be next to {} or on PATH",
            clang.display()
        );
        return ExitCode::FAILURE;
    };
    let root = repo_root();
    let t = xtask::ld_fixtures::Tools {
        clang,
        lld,
        readobj,
        objdump,
        verbose: k.verbose,
    };
    let fixtures = root.join("crates/merlin-ld/tests/fixtures");
    match xtask::ld_fixtures::run(&fixtures, &root.join("examples"), &t) {
        Ok(files) => {
            eprintln!("wrote {} files in {}", files.len(), fixtures.display());
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("xtask: {e}");
            ExitCode::FAILURE
        }
    }
}

fn kernel(k: KernelArgs) -> ExitCode {
    let root = repo_root();
    let kernel_dir = root.join("kernel");
    // The C assembler's tables, from the host tools' (as_tables.rs): before
    // the user programs are built.
    match xtask::as_tables::write(&root) {
        Ok(true) => eprintln!("wrote {}", xtask::as_tables::TABLES_H),
        Ok(false) => {}
        Err(e) => {
            eprintln!("xtask: {e}");
            return ExitCode::from(1);
        }
    }
    // The tools run inside kernel/, so a relative -o must be made absolute here.
    let out = match k.out.map(std::path::absolute).transpose() {
        Ok(out) => out,
        Err(e) => {
            eprintln!("xtask: bad output path: {e}");
            return ExitCode::from(2);
        }
    };
    let built = match k.cc.unwrap_or_default() {
        Cc::Merlin => {
            // The user programs' assembly (userprogs.s) is generated into the
            // object directory first: their images are part of the kernel.
            Build {
                out: out.unwrap_or_else(|| kernel_dir.join("kernel.elf")),
                obj_dir: root.join("target").join("kernel").join("obj"),
                kernel_dir,
                toolchain: Toolchain::Merlin,
                userprogs: None,
                verbose: k.verbose,
            }
            .run()
        }
        Cc::Clang => {
            let Some(clang) = k.clang.or_else(|| tools::find("clang")) else {
                eprintln!(
                    "xtask: clang not found. Install LLVM (https://github.com/llvm/llvm-project/releases) \
                     or pass --clang PATH"
                );
                return ExitCode::FAILURE;
            };
            let Some(lld) = k.lld.or_else(|| tools::find_beside("ld.lld", &clang)) else {
                eprintln!(
                    "xtask: ld.lld not found next to {} or on PATH; pass --lld PATH",
                    clang.display()
                );
                return ExitCode::FAILURE;
            };
            match xtask::kernel::clang_version(&clang) {
                Ok(v) => eprintln!("reference build with {v}"),
                Err(e) => {
                    eprintln!("xtask: {e}");
                    return ExitCode::FAILURE;
                }
            }
            let work = root.join(xtask::kernel::REFERENCE_DIR);
            let out = out.unwrap_or_else(|| work.join("kernel.elf"));
            let assembler = k.assembler.unwrap_or(Assembler::Clang);
            xtask::kernel::build_reference(&root, &clang, &lld, assembler, &work, &out, k.verbose)
        }
    };
    let out = match built {
        Ok(out) => out,
        Err(e) => {
            eprintln!("xtask: {e}");
            return ExitCode::FAILURE;
        }
    };
    eprintln!("built {}", out.display());
    if k.cc.unwrap_or_default() == Cc::Merlin {
        eprintln!("built {}", xtask::kernel::disk_kernel_path(&out).display());
    }
    if !k.run {
        return ExitCode::SUCCESS;
    }

    let file = match std::fs::read(&out) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("xtask: cannot read {}: {e}", out.display());
            return ExitCode::FAILURE;
        }
    };
    // Interactive: the kernel's shell reads this terminal, in raw mode like the
    // grail command's, and runs until the shell exits (or Ctrl-C).
    let _raw = grail::console::RawMode::enable();
    let run = xtask::kernel::run_with_output(
        &file,
        None,
        grail::console::stdin_channel(),
        std::io::stdout(),
        std::io::stderr(),
    );
    match run {
        Err(e) => {
            eprintln!("xtask: {e}");
            ExitCode::FAILURE
        }
        Ok((grail::RunResult::Exited(code), _)) => ExitCode::from(code.min(255) as u8),
        Ok((grail::RunResult::StepLimit, m)) => {
            eprintln!("xtask: step limit reached at pc={:#010x}", m.cpu.pc);
            ExitCode::from(124)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<KernelArgs, String> {
        parse_kernel_args(&args.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }

    #[test]
    fn defaults() {
        assert_eq!(parse(&[]).unwrap(), KernelArgs::default());
    }

    #[test]
    fn usage_descriptions_line_up() {
        // Every task's description, and every continuation line, starts in
        // the same column.
        let usage = usage();
        let tasks = usage
            .split("tasks:\n")
            .nth(1)
            .unwrap()
            .split("\n\n")
            .next()
            .unwrap();
        for line in tasks.lines() {
            // A task's line: its name, then the description.
            let rest = match line.strip_prefix("  ") {
                Some(task) if !task.starts_with(' ') => task.trim_start_matches(|c: char| c != ' '),
                _ => line,
            };
            let column = line.len() - rest.trim_start().len();
            assert_eq!(column, 15, "{line:?}");
        }
    }

    #[test]
    fn all_options() {
        let k = parse(&[
            "--cc", "clang", "--clang", "c", "--lld", "l", "--as", "merlin", "-o", "k.elf", "--run", "-v",
        ])
        .unwrap();
        assert_eq!(
            k,
            KernelArgs {
                cc: Some(Cc::Clang),
                clang: Some("c".into()),
                lld: Some("l".into()),
                assembler: Some(Assembler::Merlin),
                out: Some("k.elf".into()),
                run: true,
                verbose: true,
            }
        );
        assert_eq!(check_kernel_args(&k), Ok(()));
        assert_eq!(
            parse(&["--as", "clang"]).unwrap().assembler,
            Some(Assembler::Clang)
        );
        assert_eq!(parse(&["--cc", "merlin"]).unwrap().cc, Some(Cc::Merlin));
    }

    #[test]
    fn llvm_options_need_the_reference_build() {
        for args in [
            &["--clang", "c"][..],
            &["--lld", "l"],
            &["--as", "merlin"],
            &["--cc", "merlin", "--as", "clang"],
        ] {
            let k = parse(args).unwrap();
            let err = check_kernel_args(&k).unwrap_err();
            assert!(err.contains("--cc clang"), "{args:?}: {err}");
        }
        assert_eq!(check_kernel_args(&parse(&["-o", "k", "--run"]).unwrap()), Ok(()));
    }

    fn parse_examples(args: &[&str]) -> Result<ExamplesArgs, String> {
        parse_examples_args(&args.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }

    #[test]
    fn examples_args() {
        assert_eq!(parse_examples(&[]).unwrap(), ExamplesArgs::default());
        assert_eq!(
            parse_examples(&["hello", "--check", "multiobj", "-v"]).unwrap(),
            ExamplesArgs {
                names: vec!["hello".into(), "multiobj".into()],
                check: true,
                verbose: true,
            }
        );
        let err = parse_examples(&["helo"]).unwrap_err();
        assert!(err.starts_with("unknown example helo (one of: hello, "), "{err}");
        assert!(parse_examples(&["--run"])
            .unwrap_err()
            .contains("unknown option --run"));
    }

    #[test]
    fn fs_args() {
        let parse = |v: &[&str]| parse_fs_args(&v.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        assert_eq!(parse(&[]).unwrap(), FsArgs::default());
        assert_eq!(
            parse(&["-o", "d.img", "--size", "512"]).unwrap(),
            FsArgs {
                out: Some("d.img".into()),
                size: Some(512),
                no_boot: false,
            }
        );
        assert!(parse(&["--no-boot"]).unwrap().no_boot);
        assert!(parse(&["--size", "1M"]).unwrap_err().contains("bad size"));
        assert!(parse(&["-v"]).unwrap_err().contains("unknown option"));
    }

    #[test]
    fn errors() {
        assert!(parse(&["--clang"]).unwrap_err().contains("needs a value"));
        assert!(parse(&["--as", "gas"]).unwrap_err().contains("unknown assembler"));
        assert!(parse(&["--cc", "gcc"])
            .unwrap_err()
            .contains("unknown compiler gcc"));
        assert!(parse(&["--frobnicate"]).unwrap_err().contains("unknown option"));
    }
}
