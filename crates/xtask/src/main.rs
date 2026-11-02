//! `cargo xtask <task> [options]`

use std::path::PathBuf;
use std::process::ExitCode;
use xtask::kernel::{Assembler, Build};
use xtask::{repo_root, tools};

const USAGE: &str = "usage: cargo xtask <task> [options]

tasks:
  kernel       build kernel/kernel.elf (clang + ld.lld from the official LLVM install)

kernel options:
  --clang PATH       clang to use (default: PATH, then C:\\Program Files\\LLVM\\bin)
  --lld PATH         ld.lld to use (default: next to clang, then the same search)
  --as clang|merlin  assembler for the .s files (default: clang)
  -o PATH            output file (default: kernel/kernel.elf)
  --run              run the result on Grail afterwards; exit with its exit code
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

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("kernel") => match parse_kernel_args(&args[1..]) {
            Ok(k) => kernel(k),
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
    let mut m = grail::Machine::new(grail::DEFAULT_RAM_SIZE);
    if let Err(e) = m.load_elf(&file) {
        eprintln!("xtask: {e}");
        return ExitCode::FAILURE;
    }
    match m.run(Some(xtask::kernel::BOOT_MAX_STEPS), |_| {}) {
        grail::RunResult::Exited(code) => ExitCode::from(code.min(255) as u8),
        grail::RunResult::StepLimit => {
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

    #[test]
    fn errors() {
        assert!(parse(&["--clang"]).unwrap_err().contains("needs a value"));
        assert!(parse(&["--as", "gas"]).unwrap_err().contains("unknown assembler"));
        assert!(parse(&["--frobnicate"]).unwrap_err().contains("unknown option"));
    }
}
