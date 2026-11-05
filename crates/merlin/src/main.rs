//! `merlin [-march=ISA] [-mabi=ABI] [-o out.s] [-I dir] [-D name[=value]] [-U name] [-g] [-x LANG] [-E [-P] | --dump-ast] input.c...`

use merlin::{CmdMacro, Lang, Options};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const USAGE: &str = "usage: merlin [options] input.c...

  -march=ISA        rv64ima_zicsr_zifencei, the only one (and the default)
  -mabi=ABI         lp64, the only one (merlin has no floating point)
  -o FILE           output file (default: each input with a .s extension;
                    with -E, standard output); needs a single input
  -I DIR            look for #include files in DIR (after the including
                    file's own directory for #include \"...\"); repeatable
  -D NAME[=VALUE]   define a macro, as #define NAME VALUE (VALUE is 1 if
                    left out); -D 'F(x)=VALUE' defines a function-like one
  -U NAME           undefine a macro (-D and -U apply in order)
  -g                debug information: which source line each instruction
                    comes from (.file, .loc), and a DWARF compile unit with
                    the functions, their parameters and locals, the globals
                    and the types, for debuggers (merlin-dbg, lldb)
  -fdebug-compilation-dir=DIR
                    the directory -g says file names are relative to
                    (default: the current directory)
  -E                preprocess only: print the tokens after preprocessing
  -P                with -E, no line markers in preprocessed assembly (for a
                    linker script: kernel/kernel.lds.S); C's -E has none
  -x LANG           what the inputs after it are: c, or assembler-with-cpp
                    (assembly for merlin-as, only preprocessed: -E); without
                    it, .S files are assembler-with-cpp and the rest C
  --dump-ast        print the checked syntax tree to stdout instead of compiling

Each input is a translation unit, compiled to its own assembly file. A .S
file is preprocessed (-E) as clang -E -x assembler-with-cpp does, with
__ASSEMBLER__ and the target's __riscv_xlen defined, for merlin-as.";

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
    // `-x LANG` applies to the inputs after it, as in clang; `None` goes by
    // each file's name.
    let mut lang: Option<Lang> = None;
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
            "-P" => opts.line_markers = false,
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
                Ok(()) => {}
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
            _ if a.starts_with("-x") => match value("-x") {
                Some(l) if l == "none" => lang = None,
                Some(l) => match Lang::from_name(&l) {
                    Some(l) => lang = Some(l),
                    None => return usage_error(&format!("language not recognized: '{l}'")),
                },
                None => return usage_error("-x needs a language"),
            },
            _ if a.starts_with('-') => return usage_error(&format!("unknown option {a}")),
            _ => {
                let l = lang.unwrap_or_else(|| Lang::of_file(&a));
                inputs.push((a, l));
            }
        }
    }
    // The ABI must be the ISA's; clang calls any other an unknown one
    // (`-mabi=ilp32` too).
    if let Some(abi) = abi.filter(|a| a != merlin::ABI) {
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
    // Assembly is only preprocessed: merlin-as assembles it.
    if let Some((a, _)) = inputs.iter().find(|(_, l)| *l == Lang::AssemblerWithCpp) {
        if mode != Mode::Preprocess {
            return usage_error(&format!(
                "{a}: assembler-with-cpp input is only preprocessed (-E); merlin-as assembles the output"
            ));
        }
    }
    let mut status = ExitCode::SUCCESS;
    for (input, lang) in &inputs {
        let opts = Options {
            lang: *lang,
            ..opts.clone()
        };
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
    // The text, and the warnings of a compilation that succeeds (those of
    // one that fails come with its errors).
    let result = match mode {
        Mode::Compile => merlin::compile_warned(input, &src, opts, &merlin::read_file),
        Mode::Preprocess if opts.lang == Lang::AssemblerWithCpp => {
            merlin::preprocess_asm(input, &src, opts, &merlin::read_file).map(|t| (t, Vec::new()))
        }
        Mode::Preprocess => merlin::preprocess(input, &src, opts, &merlin::read_file)
            .map(|pp| (merlin::preprocess::spell(&pp.tokens), Vec::new())),
        Mode::DumpAst => merlin::parse_with(input, &src, opts, &merlin::read_file)
            .map(|p| (merlin::ast::dump(&p), p.warnings)),
    };
    // Diagnostics as clang prints them, on stderr; only errors fail.
    let text = match result {
        Ok((t, warnings)) => {
            for w in &warnings {
                eprintln!("{}", w.render(input));
            }
            t
        }
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
