//! `merlin-ld [-T script] [-e entry] [-o out.elf] [--Map out.map] [-S] [-L dir] input.o ... [lib.a] [-lname]
//! [--start-lib lib.o ... --end-lib] [@file]`

use merlin_ld::{Input, Options, ScriptSource};
use std::process::ExitCode;

const USAGE: &str = "usage: merlin-ld [options] input.o ... [lib.a] [-lNAME]

  -o FILE        output executable (default: a.out)
  -T FILE        linker script (default: .text at 0x80000000, then .rodata, .data, .bss)
  -e SYMBOL      entry point symbol or address (overrides ENTRY in the script)
  --Map FILE     write a link map
  -S, --strip-debug
                 leave out the debug information (.debug_* sections)
  -L DIR         look for -l libraries in DIR (every -L, in order)
  -l NAME        the archive libNAME.a of the first -L directory that has it
                 (-l:FILE: FILE itself); an archive's members, as the objects
                 of --start-lib, are linked only if needed
  --start-lib    the objects up to --end-lib are a library: each is linked
                 only if it defines a symbol the others need (like ld.lld's)
  --end-lib      ends a library
  @FILE          more arguments, read from FILE (separated by white space)";

/// The arguments with each `@FILE` replaced by the words of FILE, as GNU ld
/// and ld.lld read response files (without their quoting): how a link
/// line longer than a shell takes is given, on Camelot as on the host.
fn expand_response_files(args: impl Iterator<Item = String>) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    for a in args {
        let Some(file) = a.strip_prefix('@').filter(|f| !f.is_empty()) else {
            out.push(a);
            continue;
        };
        let text = std::fs::read_to_string(file).map_err(|e| format!("cannot read {file}: {e}"))?;
        for word in text.split_whitespace() {
            if word.starts_with('@') {
                return Err(format!("{file}: a response file cannot name another ({word})"));
            }
            out.push(word.to_string());
        }
    }
    Ok(out)
}

/// An input of the command line: a file, or a library `-l` names.
enum Arg {
    File(String),
    Library(String),
}

fn main() -> ExitCode {
    // (file, or -l's name, whether it is a library member)
    let mut inputs: Vec<(Arg, bool)> = Vec::new();
    let mut dirs: Vec<String> = Vec::new();
    let mut in_lib = false;
    let mut output = "a.out".to_string();
    let mut script = None;
    let mut map = None;
    let mut strip = false;
    let mut opts = Options::default();
    let mut args = match expand_response_files(std::env::args().skip(1)) {
        Ok(args) => args.into_iter(),
        Err(e) => {
            eprintln!("merlin-ld: {e}");
            return ExitCode::from(2);
        }
    };
    while let Some(a) = args.next() {
        // `--Map=x`, `-Map=x`, `-o=x`... and the separate-argument forms.
        let (flag, inline) = match a.split_once('=') {
            Some((f, v)) if f.starts_with('-') => (f.to_string(), Some(v.to_string())),
            _ => (a.clone(), None),
        };
        let mut value = |name: &str| match inline.clone().or_else(|| args.next()) {
            Some(v) => Ok(v),
            None => Err(format!("{name} needs an argument")),
        };
        let r = match flag.as_str() {
            "-o" => value("-o").map(|v| output = v),
            "-T" => value("-T").map(|v| script = Some(v)),
            "-e" | "--entry" => value("-e").map(|v| opts.entry = Some(v)),
            "--Map" | "-Map" => value("--Map").map(|v| map = Some(v)),
            "-S" | "--strip-debug" => {
                strip = true;
                Ok(())
            }
            "--start-lib" if in_lib => Err("nested --start-lib".to_string()),
            "--start-lib" => {
                in_lib = true;
                Ok(())
            }
            "--end-lib" if !in_lib => Err("--end-lib without --start-lib".to_string()),
            "--end-lib" => {
                in_lib = false;
                Ok(())
            }
            "-h" | "--help" => {
                println!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            "-L" | "--library-path" => value("-L").map(|v| dirs.push(v)),
            "-l" | "--library" => value("-l").map(|v| inputs.push((Arg::Library(v), in_lib))),
            f if f.starts_with("-L") => {
                dirs.push(a[2..].to_string());
                Ok(())
            }
            f if f.starts_with("-l") => {
                inputs.push((Arg::Library(a[2..].to_string()), in_lib));
                Ok(())
            }
            f if f.starts_with('-') && f.len() > 1 => Err(format!("unknown option {f}\n{USAGE}")),
            _ => {
                inputs.push((Arg::File(a), in_lib));
                Ok(())
            }
        };
        if let Err(e) = r {
            eprintln!("merlin-ld: {e}");
            return ExitCode::from(2);
        }
    }
    if in_lib {
        eprintln!("merlin-ld: --start-lib without --end-lib");
        return ExitCode::from(2);
    }
    if inputs.iter().all(|(_, lib)| *lib) {
        eprintln!("merlin-ld: no input files\n{USAGE}");
        return ExitCode::from(2);
    }

    // Every -L counts for every -l, as in ld: the directories are all known
    // before a library is looked for.
    let mut missing = false;
    let mut found = Vec::new();
    for (arg, lib) in inputs {
        match arg {
            Arg::File(name) => found.push((name, lib)),
            Arg::Library(name) => {
                match merlin_ld::search_library(&name, &dirs, |p| std::path::Path::new(p).is_file()) {
                    Ok(path) => found.push((path, lib)),
                    Err(e) => {
                        eprintln!("merlin-ld: error: {e}");
                        missing = true;
                    }
                }
            }
        }
    }
    if missing {
        return ExitCode::from(1);
    }

    // In command-line order, which ld.lld reads them in, library members too.
    let mut files = Vec::new();
    for (name, lib) in found {
        match std::fs::read(&name) {
            Ok(data) => files.push((Input { name, data }, lib)),
            Err(e) => {
                eprintln!("merlin-ld: cannot read {name}: {e}");
                return ExitCode::from(2);
            }
        }
    }
    if let Some(name) = script {
        match std::fs::read_to_string(&name) {
            Ok(text) => opts.script = Some(ScriptSource { name, text }),
            Err(e) => {
                eprintln!("merlin-ld: cannot read {name}: {e}");
                return ExitCode::from(2);
            }
        }
    }

    let files: Vec<(&Input, bool)> = files.iter().map(|(i, lib)| (i, *lib)).collect();
    match merlin_ld::link_files(&files, &opts) {
        Ok(linked) => {
            for w in &linked.layout.warnings {
                eprintln!("merlin-ld: warning: {w}");
            }
            let elf = if strip {
                match merlin_ld::strip_debug(&linked.elf) {
                    Ok(elf) => elf,
                    Err(e) => {
                        eprintln!("merlin-ld: error: {e}");
                        return ExitCode::from(1);
                    }
                }
            } else {
                linked.elf
            };
            if let Err(e) = std::fs::write(&output, &elf) {
                eprintln!("merlin-ld: cannot write {output}: {e}");
                return ExitCode::from(2);
            }
            if let Some(m) = map {
                if let Err(e) = std::fs::write(&m, merlin_ld::map::map(&linked.layout)) {
                    eprintln!("merlin-ld: cannot write {m}: {e}");
                    return ExitCode::from(2);
                }
            }
            ExitCode::SUCCESS
        }
        Err(errors) => {
            for e in &errors {
                eprintln!("merlin-ld: error: {e}");
            }
            ExitCode::from(1)
        }
    }
}
