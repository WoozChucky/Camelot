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

Ctrl-] detaches and leaves the VM running; so does the end of the input when it
is not a terminal (where Ctrl-] still detaches). Herald also ends when the VM
stops, exits or is destroyed. Ctrl-C leaves Herald at once, the VM running.
Any number of Heralds may attach to one VM: all of them see its output, and what
any of them types reaches it.

Exit status: 0 when detached or when the VM ended, 1 on an error (no server, no
such VM, connection lost, console output failed), 2 for bad arguments; Ctrl-C
gives the usual status of an interrupted program (130 on Linux,
STATUS_CONTROL_C_EXIT on Windows).";

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
                Some(s) => parse_server(&s).map(|addrs| server = Some((s, addrs))),
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
    let (server, addrs) = server.unwrap_or_else(|| {
        let default = DEFAULT_AVALON_ADDR.parse().expect("the default address parses");
        (DEFAULT_AVALON_ADDR.to_string(), vec![default])
    });
    match attach(&server, &addrs, &vm) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("herald: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Attaches to `vm` on `server` (as the person wrote it), which is at `addrs`: each is
/// tried in turn.
fn attach(server: &str, addrs: &[SocketAddr], vm: &VmRef) -> Result<(), String> {
    let conn = Connection::connect(addrs, AGENT).map_err(|e| format!("cannot connect to {server}: {e}"))?;
    // The address that answered, for the messages below.
    let server = conn
        .get_ref()
        .peer_addr()
        .map_or(server.to_string(), |a| a.to_string());
    let session = Session::attach(conn, vm).map_err(|e| format!("cannot attach to VM {vm}: {e}"))?;
    let summary = session.vm.clone();
    let terminal = io::stdin().is_terminal();
    // Not a terminal: the bytes go to the VM as they are, but a Ctrl-] among them
    // still detaches (a terminal that is a pipe, as mintty's, has no other way out).
    eprintln!(
        "herald: attached to VM {} '{}' ({}) on {server}; {} detaches",
        summary.id,
        summary.name,
        summary.state,
        if terminal {
            DETACH_KEY_NAME.to_string()
        } else {
            format!("{DETACH_KEY_NAME} or the end of the input")
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
    let mut out = LineEnd::new(console);
    let result = session.run(input, &mut out, &mut io::stderr());
    drop(raw);

    // The VM's output may have left the cursor in the middle of a line.
    if !out.at_start && !out.failed && io::stdout().is_terminal() {
        let _ = out.write_all(b"\r\n");
        let _ = out.flush();
    }
    match result {
        Ok(end) => {
            eprintln!("herald: {}", end.describe(&summary));
            Ok(())
        }
        // The session's I/O errors are the connection's, unless writing the console
        // is what failed.
        Err(e) if out.failed => Err(format!("cannot write the console output: {e}")),
        Err(e) => Err(format!("lost the connection to {server}: {e}")),
    }
}

/// Remembers whether the last byte written ended a line, and whether writing failed.
struct LineEnd<W> {
    inner: W,
    at_start: bool,
    failed: bool,
}

impl<W: Write> LineEnd<W> {
    fn new(inner: W) -> Self {
        LineEnd {
            inner,
            at_start: true,
            failed: false,
        }
    }
}

impl<W: Write> Write for LineEnd<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.inner.write(buf).inspect_err(|_| self.failed = true)?;
        if n > 0 {
            self.at_start = buf[n - 1] == b'\n';
        }
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush().inspect_err(|_| self.failed = true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_end_follows_the_last_byte() {
        let mut out = LineEnd::new(Vec::new());
        assert!(out.at_start);
        out.write_all(b"abc").unwrap();
        assert!(!out.at_start);
        out.write_all(b"def\n").unwrap();
        assert!(out.at_start);
        out.write_all(b"").unwrap();
        assert!(out.at_start);
        out.write_all(b"\r\nprompt> ").unwrap();
        assert!(!out.at_start);
        assert!(!out.failed);
        assert_eq!(out.inner, b"abcdef\n\r\nprompt> ");
    }

    /// A writer that takes `room` bytes, then fails.
    struct Full {
        room: usize,
    }

    impl Write for Full {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            if self.room == 0 {
                return Err(io::ErrorKind::BrokenPipe.into());
            }
            let n = buf.len().min(self.room);
            self.room -= n;
            Ok(n)
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn line_end_counts_only_what_was_written_and_notes_failures() {
        let mut out = LineEnd::new(Full { room: 4 });
        // Only "ab\nc" goes out: the line was not ended.
        assert!(out.write_all(b"ab\ncd\n").is_err());
        assert!(!out.at_start);
        assert!(out.failed);
    }
}
