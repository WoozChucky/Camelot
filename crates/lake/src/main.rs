//! `lake [--listen ADDR] --dir <images>`

use lake::{parse_listen, Config, Server};
use std::process::ExitCode;

const USAGE: &str = "usage: lake [--listen ADDR] --dir <images>

Serves the disk images in a directory over the Avalon protocol
(docs/avalon-protocol.md): Round Table creates, lists and deletes them, and VMs whose
profile names lake://ADDR/NAME use them as disks.

  --dir DIR       the images directory (created if it does not exist). Each disk is
                  a raw image file there, named after the disk
  --listen ADDR   where to listen: host:port, or a host or IP address for port 7401
                  (default 127.0.0.1:7401). Protocol v1 has no authentication, so
                  anything but a loopback address lets anyone who can reach it read,
                  write and delete the disks
  -h, --help      show this text

An image has one writer at a time, or any number of read-only users. Ctrl-C ends
Lake; every write it has answered is already in the image files.";

fn main() -> ExitCode {
    let mut listen = None;
    let mut dir = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let parsed = match a.as_str() {
            "-h" | "--help" => {
                println!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            "--listen" => value(&mut args, "--listen", parse_listen).map(|addr| listen = Some(addr)),
            "--dir" => value(&mut args, "--dir", |s| match s {
                "" => Err("empty directory name".to_string()),
                s => Ok(s.to_string()),
            })
            .map(|d| dir = Some(d)),
            other => Err(format!("unknown option '{other}'\n\n{USAGE}")),
        };
        if let Err(e) = parsed {
            eprintln!("lake: {e}");
            return ExitCode::from(2);
        }
    }
    let Some(dir) = dir else {
        eprintln!("lake: --dir is required\n\n{USAGE}");
        return ExitCode::from(2);
    };
    let mut config = Config::new(dir);
    if let Some(addr) = listen {
        config.listen = addr;
    }

    let server = match Server::bind(&config) {
        Ok(server) => server,
        Err(e) => {
            eprintln!(
                "lake: cannot serve {} on {}: {e}",
                config.dir.display(),
                config.listen
            );
            return ExitCode::FAILURE;
        }
    };
    match server.run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("lake: {e}");
            ExitCode::FAILURE
        }
    }
}

fn value<T>(
    args: &mut impl Iterator<Item = String>,
    flag: &str,
    parse: impl Fn(&str) -> Result<T, String>,
) -> Result<T, String> {
    let s = args.next().ok_or_else(|| format!("{flag} needs a value"))?;
    parse(&s).map_err(|e| format!("{flag}: {e}"))
}
