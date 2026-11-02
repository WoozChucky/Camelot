//! `merlin-ld [-T script] [-e entry] [-o out.elf] [--Map out.map] [-S] input.o ... [--start-lib lib.o ... --end-lib]`

use merlin_ld::{Input, Options, ScriptSource};
use std::process::ExitCode;

const USAGE: &str = "usage: merlin-ld [options] input.o ...

  -o FILE        output executable (default: a.out)
  -T FILE        linker script (default: .text at 0x80000000, then .rodata, .data, .bss)
  -e SYMBOL      entry point symbol or address (overrides ENTRY in the script)
  --Map FILE     write a link map
  -S, --strip-debug
                 leave out the debug information (.debug_* sections)
  --start-lib    the objects up to --end-lib are a library: each is linked
                 only if it defines a symbol the others need (like ld.lld's)
  --end-lib      ends a library";

fn main() -> ExitCode {
    // (file, whether it is a library member)
    let mut inputs: Vec<(String, bool)> = Vec::new();
    let mut in_lib = false;
    let mut output = "a.out".to_string();
    let mut script = None;
    let mut map = None;
    let mut strip = false;
    let mut opts = Options::default();
    let mut args = std::env::args().skip(1);
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
            f if f.starts_with('-') && f.len() > 1 => Err(format!("unknown option {f}\n{USAGE}")),
            _ => {
                inputs.push((a, in_lib));
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

    let mut files = Vec::new();
    let mut library = Vec::new();
    for (name, lib) in inputs {
        match std::fs::read(&name) {
            Ok(data) if lib => library.push(Input { name, data }),
            Ok(data) => files.push(Input { name, data }),
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

    match merlin_ld::link_with_library(&files, &library, &opts) {
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
