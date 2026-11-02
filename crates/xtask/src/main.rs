//! `cargo xtask <task> [options]`

use std::path::PathBuf;
use std::process::ExitCode;
use xtask::kernel::{Assembler, Build};
use xtask::{examples, repo_root, tools};

const USAGE: &str = "usage: cargo xtask <task> [options]

tasks:
  examples     build every example (or the ones named) with merlin-as + merlin-ld into
               examples/<dir>/<name>.elf; no LLVM needed. Objects go to target/examples.
               --check: build in memory and report stale .elf files instead of writing
               -v: print the equivalent merlin-as/merlin-ld command lines
  kernel       build kernel/kernel.elf (clang + ld.lld from the official LLVM install),
               after compiling kernel/user/*.c to kernel/user/clang/*.s (clang) and
               regenerating kernel/userprogs.s from kernel/user (merlin-as + merlin-ld)
  fs           build the root filesystem image (target/camelot-fs.img): kernel/fs/ at /,
               the user programs in /bin (merlin-as + merlin-ld; no LLVM needed), /dev/console
               -o PATH: output file; --size N: blocks (1 KiB each, default 2048)
  hello        reference build: assemble examples/hello/hello.S with merlin-as, link it
               with ld.lld into crates/merlin-as/tests/golden/hello.elf; takes --lld and -v
  ld-fixtures  regenerate crates/merlin-ld/tests/fixtures (clang, ld.lld, llvm-readobj, llvm-objdump);
               takes --clang, --lld and -v like kernel

kernel options:
  --clang PATH       clang to use (default: PATH, then C:\\Program Files\\LLVM\\bin)
  --lld PATH         ld.lld to use (default: next to clang, then the same search)
  --as clang|merlin  assembler for the .s files (default: clang)
  -o PATH            output file (default: kernel/kernel.elf)
  --run              run the result on Grail afterwards, with this terminal as its
                     console (type exit to stop it); exit with its exit code
  -v, --verbose      print each command";

#[derive(Debug, Default, PartialEq, Eq)]
struct KernelArgs {
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
            "--run" => k.run = true,
            "-v" | "--verbose" => k.verbose = true,
            other => return Err(format!("unknown option {other}")),
        }
    }
    Ok(k)
}

#[derive(Debug, Default, PartialEq, Eq)]
struct FsArgs {
    out: Option<PathBuf>,
    size: Option<u32>,
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
                eprintln!("xtask: {e}\n{USAGE}");
                ExitCode::from(2)
            }
        },
        Some("kernel") => match parse_kernel_args(&args[1..]) {
            Ok(k) => kernel(k),
            Err(e) => {
                eprintln!("xtask: {e}\n{USAGE}");
                ExitCode::from(2)
            }
        },
        Some("fs") => match parse_fs_args(&args[1..]) {
            Ok(f) => build_fs(f),
            Err(e) => {
                eprintln!("xtask: {e}\n{USAGE}");
                ExitCode::from(2)
            }
        },
        Some("hello") => match parse_kernel_args(&args[1..]) {
            Ok(k) if k.clang.is_none() && k.assembler.is_none() && k.out.is_none() && !k.run => hello(k),
            Ok(_) => {
                eprintln!("xtask: hello takes only --lld and -v\n{USAGE}");
                ExitCode::from(2)
            }
            Err(e) => {
                eprintln!("xtask: {e}\n{USAGE}");
                ExitCode::from(2)
            }
        },
        Some("ld-fixtures") => match parse_kernel_args(&args[1..]) {
            Ok(k) if k.assembler.is_none() && k.out.is_none() && !k.run => ld_fixtures(k),
            Ok(_) => {
                eprintln!("xtask: ld-fixtures takes only --clang, --lld and -v\n{USAGE}");
                ExitCode::from(2)
            }
            Err(e) => {
                eprintln!("xtask: {e}\n{USAGE}");
                ExitCode::from(2)
            }
        },
        Some("-h" | "--help" | "help") => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        Some(other) => {
            eprintln!("xtask: unknown task {other}\n{USAGE}");
            ExitCode::from(2)
        }
        None => {
            eprintln!("{USAGE}");
            ExitCode::from(2)
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

/// `cargo xtask fs`: the root filesystem image, checked with fsck after it
/// is built.
fn build_fs(f: FsArgs) -> ExitCode {
    let root = repo_root();
    let mut opts = camelot_fs::mkfs::Options::default();
    if let Some(size) = f.size {
        opts.size = size;
    }
    let out = f.out.unwrap_or_else(|| xtask::fs::default_out(&root));
    let image = match xtask::fs::build(&root, &opts) {
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
    eprintln!(
        "built {}: {} blocks ({} free), {} files",
        out.display(),
        report.sb.size,
        report.free_blocks,
        report.files
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
    let root = repo_root();
    let kernel_dir = root.join("kernel");
    // The user programs first: clang turns their C into assembly, then
    // merlin-as + merlin-ld build them. Their images are part of the
    // kernel's sources.
    match xtask::userprogs::write_clang(&root, &clang, k.verbose) {
        Ok(changed) => {
            for f in changed {
                eprintln!("regenerated {f}");
            }
        }
        Err(e) => {
            eprintln!("xtask: {e}");
            return ExitCode::FAILURE;
        }
    }
    match xtask::userprogs::write(&root) {
        Ok(true) => eprintln!("regenerated {}", xtask::userprogs::GENERATED),
        Ok(false) => {}
        Err(e) => {
            eprintln!("xtask: {e}");
            return ExitCode::FAILURE;
        }
    }
    let default_out = k.out.is_none();
    // The tools run inside kernel/, so a relative -o must be made absolute here.
    let out = match k.out.map(std::path::absolute).transpose() {
        Ok(out) => out.unwrap_or_else(|| kernel_dir.join("kernel.elf")),
        Err(e) => {
            eprintln!("xtask: bad output path: {e}");
            return ExitCode::from(2);
        }
    };
    let build = Build {
        out,
        obj_dir: root.join("target").join("kernel").join("obj"),
        kernel_dir,
        clang,
        lld,
        assembler: k.assembler.unwrap_or(Assembler::Clang),
        verbose: k.verbose,
    };
    let out = match build.run() {
        Ok(out) => out,
        Err(e) => {
            eprintln!("xtask: {e}");
            return ExitCode::FAILURE;
        }
    };
    eprintln!("built {}", out.display());
    // Record which clang produced the committed image (see VERSION_FILE).
    if default_out && build.assembler == Assembler::Clang {
        let file = build.kernel_dir.join(xtask::kernel::VERSION_FILE);
        let written = build
            .clang_version()
            .and_then(|v| std::fs::write(&file, format!("{v}\n")).map_err(|e| e.to_string()));
        if let Err(e) = written {
            eprintln!(
                "xtask: cannot record the clang version in {}: {e}",
                file.display()
            );
            return ExitCode::FAILURE;
        }
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
    fn all_options() {
        let k = parse(&[
            "--clang", "c", "--lld", "l", "--as", "merlin", "-o", "k.elf", "--run", "-v",
        ])
        .unwrap();
        assert_eq!(
            k,
            KernelArgs {
                clang: Some("c".into()),
                lld: Some("l".into()),
                assembler: Some(Assembler::Merlin),
                out: Some("k.elf".into()),
                run: true,
                verbose: true,
            }
        );
        assert_eq!(
            parse(&["--as", "clang"]).unwrap().assembler,
            Some(Assembler::Clang)
        );
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
                size: Some(512)
            }
        );
        assert!(parse(&["--size", "1M"]).unwrap_err().contains("bad size"));
        assert!(parse(&["-v"]).unwrap_err().contains("unknown option"));
    }

    #[test]
    fn errors() {
        assert!(parse(&["--clang"]).unwrap_err().contains("needs a value"));
        assert!(parse(&["--as", "gas"]).unwrap_err().contains("unknown assembler"));
        assert!(parse(&["--frobnicate"]).unwrap_err().contains("unknown option"));
    }
}
