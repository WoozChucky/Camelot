//! `merlin [-march=ISA] [-mabi=ABI] [-o out.s] [-I dir] [-D name[=value]] [-U name] [-g] [-x LANG] [-O LEVEL]
//! [-std=c99] [-w] [-W...] [-ffreestanding] [-E [-P] | --dump-ast] [@file] input.c...`

use merlin::{CmdMacro, Lang, Options};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const USAGE: &str = "usage: merlin [options] input.c...

  -march=ISA        rv64ima_zicsr_zifencei, the only one (and the default)
  -mabi=ABI         lp64, the only one (merlin has no floating point)
  -o FILE           output file (default: each input with a .s extension;
                    with -E, standard output; `-` is standard output); needs
                    a single input
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
  -x LANG           what the inputs after it are: c (or cpp-output, C
                    already preprocessed: a .i file), or assembler-with-cpp
                    (assembly for merlin-as, only preprocessed: -E); without
                    it, .S files are assembler-with-cpp, .i files C, and the
                    rest C
  -O LEVEL          -O0, -O1, -O2, -O3, -Os, -Oz, -Og or -Ofast, as clang
                    takes them; merlin does not optimize, so every level
                    compiles the same code
  -std=STD          the language: c99 (or gnu99, iso9899:1999), the only one
  -w                no warnings
  -Wno-FLAG         no warnings of -WFLAG; -WFLAG (and -Wall...) changes nothing,
                    every warning merlin gives being clang's default
  -Werror[=FLAG]    warnings (of FLAG) are errors; -Wno-error[=FLAG] undoes it
  -ffreestanding    changes nothing: merlin compiles for a freestanding
                    implementation (__STDC_HOSTED__ is 0)
  --dump-ast        print the checked syntax tree to stdout instead of compiling
  @FILE             more arguments, read from FILE (quoted as clang reads them:
                    excalibur::response)

Each input is a translation unit, compiled to its own assembly file. A .S
file is preprocessed (-E) as clang -E -x assembler-with-cpp does, with
__ASSEMBLER__ and the target's __riscv_xlen defined, for merlin-as. An input
`-` is standard input, named <stdin>.";

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

/// What `-w` and the `-W` options make of merlin's warnings, as clang's
/// make of the same warnings (merlin gives clang's default warnings only,
/// so turning one on changes nothing): left out, or made errors.
#[derive(Default)]
struct Warnings {
    /// `-w`: none at all.
    none: bool,
    /// `-Werror` (true) or `-Wno-error` (false), the last given.
    all_errors: bool,
    /// The flags `-Wno-FLAG` turned off (true) or `-WFLAG` on again
    /// (false), the last given for each.
    off: Vec<(String, bool)>,
    /// `-Werror=FLAG` (true) and `-Wno-error=FLAG` (false), the last given
    /// for each: over `-Werror`'s word.
    errors: Vec<(String, bool)>,
}

impl Warnings {
    /// Takes `-W` option `a` (after `-W`); every other `-W` changes nothing.
    fn take(&mut self, a: &str) {
        let set = |list: &mut Vec<(String, bool)>, flag: &str, v: bool| {
            list.retain(|(f, _)| f != flag);
            list.push((flag.to_string(), v));
        };
        if a == "error" {
            self.all_errors = true;
        } else if a == "no-error" {
            self.all_errors = false;
        } else if let Some(f) = a.strip_prefix("error=") {
            set(&mut self.errors, f, true);
        } else if let Some(f) = a.strip_prefix("no-error=") {
            set(&mut self.errors, f, false);
        } else if let Some(f) = a.strip_prefix("no-") {
            set(&mut self.off, f, true);
        } else {
            set(&mut self.off, a, false);
        }
    }

    /// `diags` as clang gives them under these options: warnings left out,
    /// or made errors (`error: ... [-Werror,-WFLAG]`). Errors and notes are
    /// as they were: an error clang makes of a warning by default
    /// (`-Wint-conversion`) stays one, which merlin can't compile otherwise.
    /// Also whether one was made an error, which fails the compilation.
    fn apply(&self, diags: Vec<merlin::Error>) -> (Vec<merlin::Error>, bool) {
        let mut failed = false;
        let mut out = Vec::new();
        for mut d in diags {
            if d.severity == merlin::Severity::Warning {
                let word = |list: &[(String, bool)]| {
                    d.flag
                        .and_then(|flag| list.iter().find(|(f, _)| f == flag))
                        .map(|(_, v)| *v)
                };
                if self.none || word(&self.off) == Some(true) {
                    continue;
                }
                if word(&self.errors).unwrap_or(self.all_errors) {
                    d.severity = merlin::Severity::Error;
                    // A flag is a 'static str, as merlin's are: this one
                    // lives as long as the process, as it would anyway.
                    if let Some(flag) = d.flag {
                        d.flag = Some(Box::leak(format!("error,-W{flag}").into_boxed_str()));
                    }
                    failed = true;
                }
            }
            out.push(d);
        }
        (out, failed)
    }
}

/// The `-std=` values merlin takes: C99's names, as clang spells them.
const C99: &[&str] = &["c99", "c9x", "iso9899:1999", "iso9899:199x", "gnu99", "gnu9x"];

/// The other C standards clang knows (`-std=` values merlin refuses, being
/// C99's compiler, but knows as standards).
const OTHER_C: &[&str] = &[
    "c89",
    "c90",
    "iso9899:1990",
    "iso9899:199409",
    "gnu89",
    "gnu90",
    "c11",
    "c1x",
    "iso9899:2011",
    "iso9899:201x",
    "gnu11",
    "gnu1x",
    "c17",
    "iso9899:2017",
    "c18",
    "iso9899:2018",
    "gnu17",
    "gnu18",
    "c23",
    "c2x",
    "iso9899:2024",
    "gnu23",
    "gnu2x",
    "c2y",
    "gnu2y",
];

/// What `-std=VALUE` says, as clang's `-cc1` says it for a C input: nothing
/// for C99, else the error.
fn check_std(value: &str) -> Option<String> {
    if C99.contains(&value) {
        None
    } else if OTHER_C.contains(&value) {
        Some(format!("merlin compiles C99: '-std={value}' is not supported"))
    } else if value.starts_with("c++") || value.starts_with("gnu++") {
        Some(format!("invalid argument '-std={value}' not allowed with 'C'"))
    } else {
        Some(format!("invalid value '{value}' in '-std={value}'"))
    }
}

/// What `-OLEVEL` says, as clang's `-cc1` says it: nothing for a level it
/// takes, a warning for a number above 3 (which is 3), else the error.
fn check_opt(level: &str) -> Result<Option<String>, String> {
    match level {
        "" | "0" | "1" | "2" | "3" | "s" | "z" | "g" | "fast" => Ok(None),
        n if !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()) => Ok(Some(format!(
            "optimization level '-O{n}' is not supported; using '-O3' instead"
        ))),
        n => Err(format!("invalid integral value '{n}' in '-O{n}'")),
    }
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
    let mut warnings = Warnings::default();
    // What -O and -std= say, clang's diagnostics of no file.
    let mut said: Vec<merlin::Error> = Vec::new();
    // `@FILE`: how the `cc` driver on Camelot gives cc1, merlin in C, a
    // command line longer than `exec` takes (16 arguments), as clang's
    // driver gives its `-cc1` one when a command line is too long for the
    // system; read as clang reads them.
    let mut args = match excalibur::response::expand(std::env::args().skip(1)) {
        Ok(args) => args.into_iter(),
        Err(e) => {
            eprintln!("merlin: {e}");
            return ExitCode::from(2);
        }
    };
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
            "-w" => warnings.none = true,
            "-ffreestanding" => {}
            _ if a.starts_with("-O") => match check_opt(&a[2..]) {
                Ok(None) => {}
                Ok(Some(w)) => {
                    said.push(merlin::Error::warning_unflagged(merlin::Pos::default(), w).unplaced())
                }
                Err(e) => said.push(merlin::Error::new(merlin::Pos::default(), e).unplaced()),
            },
            _ if a.starts_with("-std=") => {
                if let Some(e) = check_std(&a["-std=".len()..]) {
                    said.push(merlin::Error::new(merlin::Pos::default(), e).unplaced());
                }
            }
            _ if a.starts_with("-W") && a.len() > 2 => warnings.take(&a[2..]),
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
                // C already preprocessed, which merlin preprocesses again:
                // its line markers are taken, and its macros are gone.
                Some(l) if l == "cpp-output" => lang = Some(Lang::C),
                Some(l) => match Lang::from_name(&l) {
                    Some(l) => lang = Some(l),
                    None => return usage_error(&format!("language not recognized: '{l}'")),
                },
                None => return usage_error("-x needs a language"),
            },
            _ if a.starts_with('-') && a != "-" => return usage_error(&format!("unknown option {a}")),
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
    // -O's and -std='s diagnostics, before the inputs', as clang's -cc1
    // gives them; an error stops it there.
    let (said, _) = warnings.apply(said);
    for d in &said {
        eprintln!("{}", d.render(""));
    }
    if said.iter().any(|d| d.severity == merlin::Severity::Error) {
        return ExitCode::from(1);
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
        if let Err(code) = translate(input, output.as_deref(), mode, &opts, &warnings) {
            status = code;
        }
    }
    status
}

/// Compiles (or preprocesses, or dumps) one input file: `-` is standard
/// input, named `<stdin>` as clang names it.
fn translate(
    input: &str,
    output: Option<&str>,
    mode: Mode,
    opts: &Options,
    warnings: &Warnings,
) -> Result<(), ExitCode> {
    let read = if input == "-" {
        let mut s = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut s).map(|_| s)
    } else {
        std::fs::read_to_string(input)
    };
    let src = match read {
        Ok(s) => s,
        Err(e) => {
            eprintln!("merlin: cannot read {input}: {e}");
            return Err(ExitCode::from(2));
        }
    };
    let input = if input == "-" { "<stdin>" } else { input };
    // The text, and the warnings of a compilation that succeeds (those of
    // one that fails come with its errors).
    let result = match mode {
        Mode::Compile => merlin::compile_warned(input, &src, opts, &merlin::read_file),
        Mode::Preprocess if opts.lang == Lang::AssemblerWithCpp => {
            merlin::preprocess_asm(input, &src, opts, &merlin::read_file).map(|t| (t, Vec::new()))
        }
        Mode::Preprocess => merlin::preprocess(input, &src, opts, &merlin::read_file)
            .map(|pp| (merlin::preprocess::spell(&pp.tokens), pp.diagnostics)),
        Mode::DumpAst => merlin::parse_with(input, &src, opts, &merlin::read_file)
            .map(|p| (merlin::ast::dump(&p), p.warnings)),
    };
    // Diagnostics as clang prints them, on stderr; only errors fail.
    // -w and -W leave warnings out or make them errors, which fail it.
    let text = match result {
        Ok((t, said)) => {
            let (said, failed) = warnings.apply(said);
            for w in &said {
                eprintln!("{}", w.render(input));
            }
            if failed {
                return Err(ExitCode::from(1));
            }
            t
        }
        Err(errors) => {
            for e in &warnings.apply(errors).0 {
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
        (_, Some(o)) if o != "-" => o.to_string(),
        _ => {
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
