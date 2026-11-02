//! `round-table [--server ADDR] [--lake ADDR] <command> [arguments]`
//!
//! A thin wrapper: the commands live in the library (`round_table::run`), over the
//! shared client layer in `avalon-client`.

use round_table::{run, USAGE};
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    }
    match run(&args, &mut std::io::stdout().lock()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(failure) => {
            eprintln!("round-table: {failure}");
            ExitCode::from(failure.exit_code())
        }
    }
}
