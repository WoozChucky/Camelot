//! `herald [--server ADDR] [--screen] <vm>`

#[cfg(windows)]
use avalon_proto::VmState;
use avalon_proto::{Connection, VmRef, DEFAULT_AVALON_ADDR};
use grail::console::{self, RawMode, RawOptions};
#[cfg(windows)]
use herald::End;
use herald::{parse_args, Args, Session, Utf8Lossy, AGENT, DETACH_KEY_NAME, USAGE};
use std::io::{self, IsTerminal, Write};
use std::net::{SocketAddr, TcpStream};
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Args { server, screen, vm } = match parse_args(&args) {
        Ok(Some(args)) => args,
        Ok(None) => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Err(e) => {
            eprintln!("herald: {e}");
            return ExitCode::from(2);
        }
    };
    let (server, addrs) = server.unwrap_or_else(|| {
        let default = DEFAULT_AVALON_ADDR.parse().expect("the default address parses");
        (DEFAULT_AVALON_ADDR.to_string(), vec![default])
    });
    let result = if screen {
        show_screen(&server, &addrs, &vm)
    } else {
        attach(&server, &addrs, &vm)
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("herald: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Connects to `server` (as the person wrote it), which is at `addrs`: each is tried
/// in turn. Returns the connection and the address that answered, for messages.
fn connect(server: &str, addrs: &[SocketAddr]) -> Result<(Connection<TcpStream>, String), String> {
    let conn = Connection::connect(addrs, AGENT).map_err(|e| format!("cannot connect to {server}: {e}"))?;
    let server = conn
        .get_ref()
        .peer_addr()
        .map_or(server.to_string(), |a| a.to_string());
    Ok((conn, server))
}

/// `--screen`: shows `vm`'s screen in a window until the window is closed.
#[cfg(windows)]
fn show_screen(server: &str, addrs: &[SocketAddr], vm: &VmRef) -> Result<(), String> {
    use herald::screen::ScreenSession;
    use herald::window::WindowView;

    let (conn, server) = connect(server, addrs)?;
    let mut session = ScreenSession::attach(conn, vm)
        .map_err(|e| format!("cannot attach to the screen of VM {vm}: {e}"))?;
    let (width, height) = (session.screen.width(), session.screen.height());
    if !WindowView::fits(width, height) {
        return Err(format!(
            "VM {vm} has a {width}x{height} screen; Herald's window shows 640x480 ones"
        ));
    }
    let summary = session.vm.clone();
    let mut view = WindowView::open(&format!("Herald: VM {} '{}'", summary.id, summary.name))?;
    eprintln!(
        "herald: showing the screen of VM {} '{}' ({}) on {server}; closing the window detaches",
        summary.id, summary.name, summary.state
    );
    match session.run(&mut view) {
        Ok(end) => {
            eprintln!("herald: {}", end.describe(&summary));
            let status = match end {
                End::Detached | End::InputEnded => return Ok(()),
                End::VmEnded(VmState::Exited(code)) => format!("exited with code {code}"),
                End::VmEnded(state) => state.to_string(),
                End::Destroyed => "destroyed".to_string(),
            };
            view.hold(&session.screen, &status);
            Ok(())
        }
        Err(e) => Err(format!("lost the connection to {server}: {e}")),
    }
}

/// `--screen` needs a window, which this build does not have.
#[cfg(not(windows))]
fn show_screen(_server: &str, _addrs: &[SocketAddr], _vm: &VmRef) -> Result<(), String> {
    Err("--screen shows the VM's screen in a window, which Herald only has on Windows".into())
}

/// Attaches to `vm`'s console on `server` (as the person wrote it), which is at
/// `addrs`: each is tried in turn.
fn attach(server: &str, addrs: &[SocketAddr], vm: &VmRef) -> Result<(), String> {
    let (conn, server) = connect(server, addrs)?;
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
