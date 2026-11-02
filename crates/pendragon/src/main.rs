//! `pendragon [--listen ADDR] [--avalon ADDR] [--lake ADDR] [--avalon-max-ram MiB]
//! [--allow-host NAME]...`
//!
//! A thin wrapper: the server lives in the library (`pendragon::Server`).

use pendragon::{parse_args, Server, USAGE};
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let config = match parse_args(&args) {
        Ok(Some(config)) => config,
        Ok(None) => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Err(e) => {
            eprintln!("pendragon: {e}\nRun 'pendragon --help' for the options.");
            return ExitCode::from(2);
        }
    };
    let server = match Server::bind(&config) {
        Ok(server) => server,
        Err(e) => {
            eprintln!("pendragon: cannot listen on {}: {e}", config.listen);
            return ExitCode::FAILURE;
        }
    };
    // The URL alone on stdout, for scripts; the rest on stderr.
    println!("{}", server.url());
    eprintln!("pendragon: open the URL above in a browser; Ctrl-C stops Pendragon");
    match server.run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("pendragon: {e}");
            ExitCode::FAILURE
        }
    }
}
