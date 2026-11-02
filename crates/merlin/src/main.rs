//! `merlin [-o out.s] [--dump-ast] input.c`

use std::process::ExitCode;

const USAGE: &str = "usage: merlin [-o output.s] [--dump-ast] input.c

  -o FILE     output assembly (default: input with .s extension)
  --dump-ast  print the checked syntax tree to stdout instead of compiling";

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

fn run() -> ExitCode {
    let mut input = None;
    let mut output = None;
    let mut dump_ast = false;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "-o" => match args.next() {
                Some(o) => output = Some(o),
                None => {
                    eprintln!("merlin: -o needs a file name");
                    return ExitCode::from(2);
                }
            },
            "--dump-ast" => dump_ast = true,
            "-h" | "--help" => {
                println!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            _ if a.starts_with('-') => {
                eprintln!("merlin: unknown option {a}\n{USAGE}");
                return ExitCode::from(2);
            }
            _ if input.is_some() => {
                eprintln!("merlin: one input file per invocation\n{USAGE}");
                return ExitCode::from(2);
            }
            _ => input = Some(a),
        }
    }
    let Some(input) = input else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };

    let src = match std::fs::read_to_string(&input) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("merlin: cannot read {input}: {e}");
            return ExitCode::from(2);
        }
    };
    let result = if dump_ast {
        merlin::parse(&src).map(|p| merlin::ast::dump(&p))
    } else {
        merlin::compile(&src)
    };
    let text = match result {
        Ok(t) => t,
        Err(errors) => {
            for e in &errors {
                eprintln!("{}", e.render(&input));
            }
            return ExitCode::from(1);
        }
    };
    if dump_ast {
        print!("{text}");
        return ExitCode::SUCCESS;
    }
    let output = output.unwrap_or_else(|| {
        let stem = input.strip_suffix(".c").unwrap_or(&input);
        format!("{stem}.s")
    });
    if let Err(e) = std::fs::write(&output, text) {
        eprintln!("merlin: cannot write {output}: {e}");
        return ExitCode::from(2);
    }
    ExitCode::SUCCESS
}
