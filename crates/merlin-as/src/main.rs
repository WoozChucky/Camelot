//! `merlin-as [-march=ISA] [-o out.o] input.s`

use std::process::ExitCode;

const USAGE: &str = "usage: merlin-as [-march=ISA] [-o output.o] input.s

  -march=ISA  rv64ima_zicsr_zifencei, the one ISA (the default)
  -o FILE     output object (default: input with .o extension)";

/// What the command line asks for.
#[derive(Debug, PartialEq, Eq)]
enum Command {
    Help,
    Assemble { input: String, output: Option<String> },
}

/// Reads the arguments (without the program name). An error is the text to
/// print before exiting with status 2.
///
/// `-march` takes only the `-march=ISA` form, as clang does: clang calls a
/// bare `-march` an unknown argument, and so does merlin-as.
fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Command, String> {
    let mut input = None;
    let mut output = None;
    let mut args = args.into_iter();
    while let Some(a) = args.next() {
        match a.as_str() {
            "-o" => match args.next() {
                Some(o) => output = Some(o),
                None => return Err("merlin-as: -o needs a file name".to_string()),
            },
            "-h" | "--help" => return Ok(Command::Help),
            _ if a.starts_with("-march=") => {
                merlin_as::parse_march(&a["-march=".len()..]).map_err(|e| format!("merlin-as: {e}"))?;
            }
            _ if a.starts_with('-') => return Err(format!("merlin-as: unknown option {a}\n{USAGE}")),
            _ => input = Some(a),
        }
    }
    match input {
        Some(input) => Ok(Command::Assemble { input, output }),
        None => Err(USAGE.to_string()),
    }
}

fn main() -> ExitCode {
    let (input, output) = match parse_args(std::env::args().skip(1)) {
        Ok(Command::Help) => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Ok(Command::Assemble { input, output }) => (input, output),
        Err(msg) => {
            eprintln!("{msg}");
            return ExitCode::from(2);
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Command, String> {
        parse_args(args.iter().map(|a| a.to_string()))
    }

    fn assemble(input: &str, output: Option<&str>) -> Result<Command, String> {
        Ok(Command::Assemble {
            input: input.to_string(),
            output: output.map(str::to_string),
        })
    }

    #[test]
    fn march_may_name_the_isa() {
        assert_eq!(parse(&["a.s"]), assemble("a.s", None));
        assert_eq!(
            parse(&["-march=rv64ima_zicsr_zifencei", "-o", "b.o", "a.s"]),
            assemble("a.s", Some("b.o"))
        );
    }

    #[test]
    fn march_must_name_an_isa_merlin_as_assembles() {
        assert_eq!(
            parse(&["-march=rv64gc", "a.s"]),
            Err(
                "merlin-as: unsupported -march 'rv64gc' (merlin-as takes rv64ima_zicsr_zifencei)".to_string()
            )
        );
        // RV32 is not an ISA merlin-as assembles: the same message.
        assert_eq!(
            parse(&["-march=rv32ima_zicsr_zifencei", "a.s"]),
            Err("merlin-as: unsupported -march 'rv32ima_zicsr_zifencei' \
                 (merlin-as takes rv64ima_zicsr_zifencei)"
                .to_string())
        );
        // Only the `=` form: `-march ISA` is an unknown option.
        assert_eq!(
            parse(&["-march", "rv64ima_zicsr_zifencei", "a.s"]),
            Err(format!("merlin-as: unknown option -march\n{USAGE}"))
        );
    }

    #[test]
    fn other_arguments_read_as_before() {
        assert_eq!(parse(&["-h"]), Ok(Command::Help));
        assert_eq!(
            parse(&["a.s", "-o"]),
            Err("merlin-as: -o needs a file name".to_string())
        );
        assert_eq!(parse(&[]), Err(USAGE.to_string()));
    }
}
