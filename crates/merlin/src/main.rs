//! `merlin [-march=ISA] [-mabi=ABI] [-o out.s] [-I dir] [-D name[=value]] [-U name] [-g] [-E | --dump-ast] input.c...`

use merlin::{CmdMacro, Options};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const USAGE: &str = "usage: merlin [options] input.c...

  -march=ISA        rv32ima_zicsr_zifencei (default) or rv64ima_zicsr_zifencei
  -mabi=ABI         ilp32 for rv32 (the default), lp64 for rv64: the one
                    the ISA has (merlin has no floating point)
  -o FILE           output file (default: each input with a .s extension;
                    with -E, standard output); needs a single input
  -I DIR            look for #include files in DIR (after the including
                    file's own directory for #include \"...\"); repeatable
  -D NAME[=VALUE]   define a macro, as #define NAME VALUE (VALUE is 1 if
                    left out); -D 'F(x)=VALUE' defines a function-like one
  -U NAME           undefine a macro (-D and -U apply in order)
  -g                debug information: which source line each instruction
                    comes from (.file, .loc), and a DWARF compile unit
  -fdebug-compilation-dir=DIR
                    the directory -g says file names are relative to
                    (default: the current directory)
  -E                preprocess only: print the tokens after preprocessing
  --dump-ast        print the checked syntax tree to stdout instead of compiling

Each input is a translation unit, compiled to its own assembly file.";

/// The compiler recurses once per nesting level (at most
/// [`merlin::parser::MAX_DEPTH`]). Unoptimized builds use several KiB per level,
/// more than the main thread's 1 MiB on Windows, so run on a thread with room.
const STACK_SIZE: usize = 64 << 20;

fn main() -> ExitCode {
    std::thread::Builder::new()
        .stack_size(STACK_SIZE)
        .spawn(run)
        .expect("cannot start the compiler thread")
        .join()
        .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Compile,
    Preprocess,
    DumpAst,
}

fn usage_error(msg: &str) -> ExitCode {
    eprintln!("merlin: {msg}\n{USAGE}");
    ExitCode::from(2)
}

fn run() -> ExitCode {
    let mut inputs = Vec::new();
    let mut output = None;
    let mut mode = Mode::Compile;
    let mut opts = Options::default();
    let mut debug = false;
    let mut comp_dir = None;
    let mut abi = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        // `-I dir` or `-Idir`, and the same for -D, -U and -o.
        let mut value = |flag: &str| -> Option<String> {
            match a.strip_prefix(flag) {
                Some("") => args.next(),
                Some(v) => Some(v.to_string()),
                None => unreachable!("matched below"),
            }
        };
        match a.as_str() {
            "-E" => mode = Mode::Preprocess,
            "-g" => debug = true,
            _ if a.starts_with("-fdebug-compilation-dir") => {
                match a.strip_prefix("-fdebug-compilation-dir").unwrap() {
                    "" => match args.next() {
                        Some(d) => comp_dir = Some(d),
                        None => return usage_error("-fdebug-compilation-dir needs a directory"),
                    },
                    v => match v.strip_prefix('=') {
                        Some(d) => comp_dir = Some(d.to_string()),
                        None => return usage_error(&format!("unknown option {a}")),
                    },
                }
            }
            "--dump-ast" => mode = Mode::DumpAst,
            // `-march=ISA` and `-mabi=ABI` only, as clang takes them.
            _ if a.starts_with("-march=") => match merlin::parse_march(&a["-march=".len()..]) {
                Ok(x) => opts.xlen = x,
                Err(e) => return usage_error(&e),
            },
            _ if a.starts_with("-mabi=") => abi = Some(a["-mabi=".len()..].to_string()),
            "-h" | "--help" => {
                println!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            _ if a.starts_with("-o") => match value("-o") {
                Some(o) => output = Some(o),
                None => return usage_error("-o needs a file name"),
            },
            _ if a.starts_with("-I") => match value("-I") {
                Some(d) => opts.include_dirs.push(PathBuf::from(d)),
                None => return usage_error("-I needs a directory"),
            },
            _ if a.starts_with("-D") => match value("-D") {
                Some(d) if !d.is_empty() && !d.starts_with('=') => opts.macros.push(CmdMacro::Define(d)),
                _ => return usage_error("-D needs a macro name"),
            },
            _ if a.starts_with("-U") => match value("-U") {
                Some(u) if !u.is_empty() => opts.macros.push(CmdMacro::Undef(u)),
                _ => return usage_error("-U needs a macro name"),
            },
            _ if a.starts_with('-') => return usage_error(&format!("unknown option {a}")),
            _ => inputs.push(a),
        }
    }
    // The ABI must be the ISA's, whichever of the two came first; clang calls
    // any other an unknown one (`-mabi=lp64` with an RV32 `-march` too).
    if let Some(abi) = abi.filter(|a| a != merlin::abi_name(opts.xlen)) {
        return usage_error(&format!("unknown target ABI '{abi}'"));
    }
    if debug {
        let comp_dir = comp_dir.unwrap_or_else(|| {
            std::env::current_dir()
                .map(|d| d.to_string_lossy().into_owned())
                .unwrap_or_else(|_| ".".into())
        });
        opts.debug = Some(merlin::DebugInfo { comp_dir });
    }
    if inputs.is_empty() {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    }
    if output.is_some() && inputs.len() > 1 {
        return usage_error("-o needs a single input file");
    }
    let mut status = ExitCode::SUCCESS;
    for input in &inputs {
        if let Err(code) = translate(input, output.as_deref(), mode, &opts) {
            status = code;
        }
    }
    status
}

/// Compiles (or preprocesses, or dumps) one input file.
fn translate(input: &str, output: Option<&str>, mode: Mode, opts: &Options) -> Result<(), ExitCode> {
    let src = match std::fs::read_to_string(input) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("merlin: cannot read {input}: {e}");
            return Err(ExitCode::from(2));
        }
    };
    let result = match mode {
        Mode::Compile => merlin::compile_with(input, &src, opts, &merlin::read_file),
        Mode::Preprocess => merlin::preprocess(input, &src, opts, &merlin::read_file)
            .map(|pp| merlin::preprocess::spell(&pp.tokens)),
        Mode::DumpAst => {
            merlin::parse_with(input, &src, opts, &merlin::read_file).map(|p| merlin::ast::dump(&p))
        }
    };
    let text = match result {
        Ok(t) => t,
        Err(errors) => {
            for e in &errors {
                eprintln!("{}", e.render(input));
            }
            return Err(ExitCode::from(1));
        }
    };
    let output = match (mode, output) {
        (Mode::Compile, None) => {
            let stem = input.strip_suffix(".c").unwrap_or(input);
            format!("{stem}.s")
        }
        (_, Some(o)) => o.to_string(),
        (_, None) => {
            print!("{text}");
            return Ok(());
        }
    };
    if let Err(e) = std::fs::write(Path::new(&output), text) {
        eprintln!("merlin: cannot write {output}: {e}");
        return Err(ExitCode::from(2));
    }
    Ok(())
}
