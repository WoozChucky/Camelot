//! `merlin-objdump <options> file...`

use merlin_objdump::cli::{self, Command};
use std::io::Write;
use std::process::ExitCode;

fn main() -> ExitCode {
    let (opts, files) = match cli::parse_args(std::env::args().skip(1)) {
        Ok(Command::Dump { opts, files }) => (opts, files),
        Ok(Command::Help) => {
            println!("{}", cli::USAGE);
            return ExitCode::SUCCESS;
        }
        Err(e) => {
            eprintln!("merlin-objdump: error: {e}\n\n{}", cli::USAGE);
            return ExitCode::from(2);
        }
    };

    let mut status = ExitCode::SUCCESS;
    let mut stdout = std::io::stdout().lock();
    for file in &files {
        let result = std::fs::read(file)
            .map_err(|e| e.to_string())
            .and_then(|bytes| merlin_objdump::dump(file, &bytes, &opts).map_err(|e| e.to_string()));
        match result {
            Ok(text) => {
                if stdout.write_all(text.as_bytes()).is_err() {
                    // Closed pipe (`merlin-objdump -d x | head`): nothing left to do.
                    return status;
                }
            }
            Err(e) => {
                let _ = stdout.flush();
                eprintln!("merlin-objdump: error: '{file}': {e}");
                status = ExitCode::from(1);
            }
        }
    }
    status
}
