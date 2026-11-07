//! `merlin [-march=ISA] [-mabi=ABI] [-o out.s] [-I dir] [-D name[=value]] [-U name] [-g] [-x LANG] [-O LEVEL]
//! [-std=c99] [-v] [-w] [-W...] [-ffreestanding] [-E [-P] | --dump-ast] [@file] input.c...`

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
  -v                say the version and the #include search list, as clang
                    -cc1 -v does
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

/// The standards of the other languages clang's `-cc1` knows by name but
/// not for C: OpenCL's and HLSL's (C++'s are every `c++` and `gnu++`).
const OTHER_LANGUAGES: &[&str] = &[
    "cl",
    "CL",
    "cl1.0",
    "CL1.0",
    "cl1.1",
    "CL1.1",
    "cl1.2",
    "CL1.2",
    "cl2.0",
    "CL2.0",
    "cl3.0",
    "CL3.0",
    "clc++",
    "CLC++",
    "clc++1.0",
    "CLC++1.0",
    "clc++2021",
    "CLC++2021",
    "hlsl",
    "hlsl2015",
    "hlsl2016",
    "hlsl2017",
    "hlsl2018",
    "hlsl2021",
    "hlsl202x",
    "hlsl202y",
];

/// The notes clang's `-cc1` gives after `invalid value 'X' in '-std=X'`: the
/// C standards, by name.
const STD_NOTES: &[&str] = &[
    "use 'c89', 'c90', or 'iso9899:1990' for 'ISO C 1990' standard",
    "use 'iso9899:199409' for 'ISO C 1990 with amendment 1' standard",
    "use 'gnu89' or 'gnu90' for 'ISO C 1990 with GNU extensions' standard",
    "use 'c99' or 'iso9899:1999' for 'ISO C 1999' standard",
    "use 'gnu99' for 'ISO C 1999 with GNU extensions' standard",
    "use 'c11' or 'iso9899:2011' for 'ISO C 2011' standard",
    "use 'gnu11' for 'ISO C 2011 with GNU extensions' standard",
    "use 'c17', 'iso9899:2017', 'c18', or 'iso9899:2018' for 'ISO C 2017' standard",
    "use 'gnu17' or 'gnu18' for 'ISO C 2017 with GNU extensions' standard",
    "use 'c23' or 'iso9899:2024' for 'ISO C 2023' standard",
    "use 'gnu23' for 'ISO C 2023 with GNU extensions' standard",
    "use 'c2y' for 'Working Draft for ISO C2y' standard",
    "use 'gnu2y' for 'Working Draft for ISO C2y with GNU extensions' standard",
];

/// A diagnostic of no file (`error: ...`, `warning: ...`, `note: ...`), as
/// clang's `-cc1` gives one of its command line.
fn unplaced(severity: merlin::Severity, msg: impl Into<String>) -> merlin::Error {
    merlin::Error {
        severity,
        ..merlin::Error::new(merlin::Pos::default(), msg).unplaced()
    }
}

/// What `-std=VALUE` says, as clang's `-cc1` says it for a C input: nothing
/// for C99; merlin's own error for another C standard; clang's for another
/// language's (C++, OpenCL, HLSL) and, with its notes naming the C
/// standards, for what is none.
fn check_std(value: &str) -> Option<merlin::Error> {
    use merlin::Severity::{Error, Note};
    if C99.contains(&value) {
        return None;
    }
    if OTHER_C.contains(&value) {
        return Some(unplaced(
            Error,
            format!("merlin compiles C99: '-std={value}' is not supported"),
        ));
    }
    if value.starts_with("c++") || value.starts_with("gnu++") || OTHER_LANGUAGES.contains(&value) {
        return Some(unplaced(
            Error,
            format!("invalid argument '-std={value}' not allowed with 'C'"),
        ));
    }
    let mut e = unplaced(Error, format!("invalid value '{value}' in '-std={value}'"));
    e.notes = STD_NOTES.iter().map(|n| unplaced(Note, *n)).collect();
    Some(e)
}

/// What `-OLEVEL` says, as clang's `-cc1` says it: nothing for `s`, `z`, `g`,
/// `fast` or a level of 0 to 4 (`-O4` is clang's -O3, without a word); for
/// another level, a 32-bit decimal number (a `-` but no `+` before it), the
/// warning that it is not supported; else the error. merlin does not
/// optimize, so no level changes the code.
fn check_opt(level: &str) -> Option<merlin::Error> {
    use merlin::Severity::{Error, Warning};
    if matches!(level, "" | "s" | "z" | "g" | "fast") {
        return None;
    }
    let digits = level.strip_prefix('-').unwrap_or(level);
    let number = if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) {
        level.parse::<i32>().ok()
    } else {
        None
    };
    match number {
        Some(0..=4) => None,
        Some(_) => Some(unplaced(
            Warning,
            format!("optimization level '-O{level}' is not supported; using '-O3' instead"),
        )),
        None => Some(unplaced(
            Error,
            format!("invalid integral value '{level}' in '-O{level}'"),
        )),
    }
}

/// An input's language and whether it is preprocessed already: C
/// preprocessed (`-x cpp-output`, a `.i` file).
const CPP_OUTPUT: (Lang, bool) = (Lang::C, true);

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
    let mut lang: Option<(Lang, bool)> = None;
    // -w and the -W options, in their order (merlin::warnings).
    let mut warnings: Vec<String> = Vec::new();
    let mut verbose = false;
    // What -O and -std= say, clang's diagnostics of no file.
    let mut said: Vec<merlin::Error> = Vec::new();
    // `@FILE`: how the `cc` driver on Camelot gives cc1, merlin in C, a
    // command line longer than `exec` takes (16 arguments), as clang's
    // driver gives its `-cc1` one when a command line is too long for the
    // system; read as clang reads them.
    let mut args = match excalibur::response::expand(std::env::args().skip(1)) {
        Ok(args) => args.into_iter(),
        Err(e) => {
            // LLVM's tools say it alone, and exit 1.
            eprintln!("{e}");
            return ExitCode::from(1);
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
            "-v" => verbose = true,
            "-ffreestanding" => {}
            _ if a.starts_with("-O") => said.extend(check_opt(&a[2..])),
            _ if a.starts_with("-std=") => said.extend(check_std(&a["-std=".len()..])),
            // -w, and -W but for -Wl, -Wp, and -Wa, (the driver's, which
            // merlin, as clang's -cc1, does not take).
            _ if merlin::warnings::is_option(&a) => warnings.push(a.clone()),
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
                // C already preprocessed (a .i file): no directive but line
                // markers run, and no macro is expanded, as clang's -cc1
                // takes it.
                Some(l) if l == "cpp-output" => lang = Some(CPP_OUTPUT),
                Some(l) => match Lang::from_name(&l) {
                    Some(l) => lang = Some((l, false)),
                    None => return usage_error(&format!("language not recognized: '{l}'")),
                },
                None => return usage_error("-x needs a language"),
            },
            _ if a.starts_with('-') && a != "-" => return usage_error(&format!("unknown option {a}")),
            _ => {
                let l = lang.unwrap_or_else(|| {
                    if a.ends_with(".i") {
                        CPP_OUTPUT
                    } else {
                        (Lang::of_file(&a), false)
                    }
                });
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
    // -O's and -std='s diagnostics, before anything else, as clang's -cc1
    // gives them as it reads its command line (and as they are: -w and -W
    // are not applied yet); an error stops it there.
    for d in &said {
        eprintln!("{}", d.render(""));
    }
    if said.iter().any(|d| d.severity == merlin::Severity::Error) {
        return ExitCode::from(1);
    }
    // -v: the version, as clang's -cc1 says it once it starts.
    if verbose {
        eprintln!(
            "merlin version {} default target riscv64",
            env!("CARGO_PKG_VERSION")
        );
    }
    // Assembly is only preprocessed: merlin-as assembles it.
    if let Some((a, _)) = inputs.iter().find(|(_, (l, _))| *l == Lang::AssemblerWithCpp) {
        if mode != Mode::Preprocess {
            return usage_error(&format!(
                "{a}: assembler-with-cpp input is only preprocessed (-E); merlin-as assembles the output"
            ));
        }
    }
    let mut status = ExitCode::SUCCESS;
    for (input, (lang, preprocessed)) in &inputs {
        let opts = Options {
            lang: *lang,
            preprocessed: *preprocessed,
            ..opts.clone()
        };
        if verbose {
            eprint!("{}", search_list(&opts.include_dirs));
        }
        if let Err(code) = translate(input, output.as_deref(), mode, &opts, &warnings) {
            status = code;
        }
    }
    status
}

/// What -v says of the `#include` search list before a translation unit,
/// as clang's -cc1 -v says it: the `-I` directories that are not there and
/// those given twice left out (and said so), then the list. merlin
/// searches a quoted name in the including file's directory first, which
/// clang's list does not show either, then the `-I` directories in order,
/// as it searches an angled one: they are both lists' (clang's `-iquote`
/// directories, which merlin has none of, would be the first's alone).
/// What it leaves out changes no search: a directory that isn't there has
/// no file, and one given again has none the first did not.
fn search_list(dirs: &[PathBuf]) -> String {
    let mut missing = String::new();
    let mut twice = String::new();
    let mut kept: Vec<(&PathBuf, PathBuf)> = Vec::new();
    for d in dirs {
        match std::fs::canonicalize(d) {
            Ok(real) if real.is_dir() => {
                if kept.iter().any(|(_, k)| *k == real) {
                    twice.push_str(&format!("ignoring duplicate directory \"{}\"\n", d.display()));
                } else {
                    kept.push((d, real));
                }
            }
            _ => missing.push_str(&format!("ignoring nonexistent directory \"{}\"\n", d.display())),
        }
    }
    let mut out = missing + &twice;
    out.push_str("#include \"...\" search starts here:\n#include <...> search starts here:\n");
    for (d, _) in kept {
        out.push_str(&format!(" {}\n", d.display()));
    }
    out.push_str("End of search list.\n");
    out
}

/// Compiles (or preprocesses, or dumps) one input file: `-` is standard
/// input, named `<stdin>` as clang names it.
fn translate(
    input: &str,
    output: Option<&str>,
    mode: Mode,
    opts: &Options,
    warnings: &[String],
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
    let stdin = input == "-";
    let input = if stdin { "<stdin>" } else { input };
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
            let (said, failed) = merlin::warnings::apply(warnings, said);
            for w in &said {
                eprintln!("{}", w.render(input));
            }
            if failed {
                return Err(ExitCode::from(1));
            }
            t
        }
        Err(errors) => {
            for e in &merlin::warnings::apply(warnings, errors).0 {
                eprintln!("{}", e.render(input));
            }
            return Err(ExitCode::from(1));
        }
    };
    // Standard input's assembly is named `-` (its STT_FILE symbol), as
    // clang's -main-file-name names it; its diagnostics `<stdin>`.
    let text = if stdin && mode == Mode::Compile {
        text.replacen("    .file \"<stdin>\"\n", "    .file \"-\"\n", 1)
    } else {
        text
    };
    // Without -o: the input's name with `.s` for its `.c` or `.i` (or
    // added), as clang's -cc1 names it, and standard output for standard
    // input.
    let output = match (mode, output) {
        (Mode::Compile, None) if !stdin => {
            let stem = input
                .strip_suffix(".c")
                .or_else(|| input.strip_suffix(".i"))
                .unwrap_or(input);
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
