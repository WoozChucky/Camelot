//! `herald [--server ADDR] <vm>`

use avalon_proto::{Connection, VmRef, DEFAULT_AVALON_ADDR};
use grail::console::{self, RawMode, RawOptions};
use herald::{parse_server, Session, Utf8Lossy, AGENT, DETACH_KEY_NAME};
use std::io::{self, IsTerminal, Write};
use std::net::SocketAddr;
use std::process::ExitCode;

const USAGE: &str = "usage: herald [--server ADDR] <vm>

Attaches to the serial console of a VM on an Avalon server: prints its recent
output, then streams the output live and sends what you type to the VM. <vm> is a
VM id (all digits) or a VM name.

  --server ADDR   the Avalon server: host:port, or a host or IP address for port
                  7400 (default 127.0.0.1:7400)
  -h, --help      show this text

Ctrl-] detaches and leaves the VM running; so does Ctrl-C, and the end of the
input when it is not a terminal. Herald also ends when the VM stops, exits or is
destroyed. Any number of Heralds may attach to one VM: all of them see its output,
and what any of them types reaches it.";

fn main() -> ExitCode {
    let mut server = None;
    let mut vm = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let parsed = match a.as_str() {
            "-h" | "--help" => {
                println!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            "--server" => match args.next() {
                Some(s) => parse_server(&s).map(|addr| server = Some(addr)),
                None => Err("--server needs a value".to_string()),
            },
            _ if a.starts_with('-') && a.len() > 1 => Err(format!("unknown option '{a}'\n\n{USAGE}")),
            _ if vm.is_some() => Err(format!("one VM at a time ('{a}' is one too many)\n\n{USAGE}")),
            _ => {
                vm = Some(VmRef::parse(&a));
                Ok(())
            }
        };
        if let Err(e) = parsed {
            eprintln!("herald: {e}");
            return ExitCode::from(2);
        }
    }
    let Some(vm) = vm else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    let server = server.unwrap_or_else(|| DEFAULT_AVALON_ADDR.parse().expect("the default address parses"));
    match attach(server, &vm) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("herald: {e}");
            ExitCode::FAILURE
        }
    }
}

fn attach(server: SocketAddr, vm: &VmRef) -> Result<(), String> {
    let conn = Connection::connect(server, AGENT).map_err(|e| format!("cannot connect to {server}: {e}"))?;
    let session = Session::attach(conn, vm).map_err(|e| format!("cannot attach to VM {vm}: {e}"))?;
    let summary = session.vm.clone();
    let terminal = io::stdin().is_terminal();
    eprintln!(
        "herald: attached to VM {} '{}' ({}) on {server}; {} detaches",
        summary.id,
        summary.name,
        summary.state,
        if terminal {
            DETACH_KEY_NAME
        } else {
            "the end of the input"
        },
    );

    // Raw mode before the reading thread starts, so that its first read is a raw one
    // already. It is put back when `raw` drops: on every return below, on a panic, and
    // on Ctrl-C (its handler).
    let raw = RawMode::with(RawOptions { vt: true });
    let input = match &raw {
        Some(raw) => raw.stdin_channel(),
        None => console::stdin_channel(),
    };
    let stdout = io::stdout();
    let console: Box<dyn Write> = if cfg!(windows) && stdout.is_terminal() {
        Box::new(Utf8Lossy::new(stdout.lock()))
    } else {
        Box::new(stdout.lock())
    };
    let mut out = LineEnd {
        inner: console,
        at_start: true,
    };
    let result = session.run(input, &mut out, &mut io::stderr());
    drop(raw);

    // The VM's output may have left the cursor in the middle of a line.
    if !out.at_start && io::stdout().is_terminal() {
        let _ = out.write_all(b"\r\n");
        let _ = out.flush();
    }
    match result {
        Ok(end) => {
            eprintln!("herald: {}", end.describe(&summary));
            Ok(())
        }
        Err(e) => Err(format!("lost the connection to {server}: {e}")),
    }
}

/// Remembers whether the last byte written ended a line.
struct LineEnd<W> {
    inner: W,
    at_start: bool,
}

impl<W: Write> Write for LineEnd<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.inner.write(buf)?;
        if n > 0 {
            self.at_start = buf[n - 1] == b'\n';
        }
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}
