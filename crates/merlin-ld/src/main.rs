//! `merlin-ld [-T script] [-e entry] [-o out.elf] [--Map out.map] input.o ...`

use merlin_ld::{Input, Options, ScriptSource};
use std::process::ExitCode;

const USAGE: &str = "usage: merlin-ld [options] input.o ...

  -o FILE        output executable (default: a.out)
  -T FILE        linker script (default: .text at 0x80000000, then .rodata, .data, .bss)
  -e SYMBOL      entry point symbol or address (overrides ENTRY in the script)
  --Map FILE     write a link map";

fn main() -> ExitCode {
    let mut inputs = Vec::new();
    let mut output = "a.out".to_string();
    let mut script = None;
    let mut map = None;
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
            "-h" | "--help" => {
                println!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            f if f.starts_with('-') && f.len() > 1 => Err(format!("unknown option {f}\n{USAGE}")),
            _ => {
                inputs.push(a);
                Ok(())
            }
        };
        if let Err(e) = r {
            eprintln!("merlin-ld: {e}");
            return ExitCode::from(2);
        }
    }
    if inputs.is_empty() {
        eprintln!("merlin-ld: no input files\n{USAGE}");
        return ExitCode::from(2);
    }

    let mut files = Vec::new();
    for name in inputs {
        match std::fs::read(&name) {
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

    match merlin_ld::link(&files, &opts) {
        Ok(linked) => {
            for w in &linked.layout.warnings {
                eprintln!("merlin-ld: warning: {w}");
            }
            if let Err(e) = std::fs::write(&output, &linked.elf) {
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
