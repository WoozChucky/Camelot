//! `merlin-as [-o out.o] input.s`

use std::process::ExitCode;

const USAGE: &str = "usage: merlin-as [-o output.o] input.s

  -o FILE   output object (default: input with .o extension)";

fn main() -> ExitCode {
    let mut input = None;
    let mut output = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "-o" => match args.next() {
                Some(o) => output = Some(o),
                None => {
                    eprintln!("merlin-as: -o needs a file name");
                    return ExitCode::from(2);
                }
            },
            "-h" | "--help" => {
                println!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            _ if a.starts_with('-') => {
                eprintln!("merlin-as: unknown option {a}\n{USAGE}");
                return ExitCode::from(2);
            }
            _ => input = Some(a),
        }
    }
    let Some(input) = input else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    let output = output.unwrap_or_else(|| {
        let stem = input
            .strip_suffix(".s")
            .or_else(|| input.strip_suffix(".S"))
            .unwrap_or(&input);
        format!("{stem}.o")
    });

    let src = match std::fs::read_to_string(&input) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("merlin-as: cannot read {input}: {e}");
            return ExitCode::from(2);
        }
    };
    match merlin_as::assemble_to_elf(&src) {
        Ok(bytes) => {
            if let Err(e) = std::fs::write(&output, bytes) {
                eprintln!("merlin-as: cannot write {output}: {e}");
                return ExitCode::from(2);
            }
            ExitCode::SUCCESS
        }
        Err(errors) => {
            for e in &errors {
                // file:line: message, the format editors understand.
                if e.line > 0 {
                    eprintln!("{input}:{}: error: {}", e.line, e.msg);
                } else {
                    eprintln!("{input}: error: {}", e.msg);
                }
            }
            ExitCode::from(1)
        }
    }
}
