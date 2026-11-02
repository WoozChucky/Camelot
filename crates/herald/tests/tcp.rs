//! `herald --tcp`: the TCP session ([`herald::tcp::TcpSession`]) against small servers
//! on loopback, with no terminal: input from a channel, output to a buffer. Line
//! editing and its echo, Ctrl-] to leave, Ctrl-D and the end of piped input shutting
//! the sending side down, and the far end closing the connection. Then the binary
//! itself, through pipes.

use herald::tcp::{TcpEnd, TcpSession};
use herald::DETACH_KEY;
use std::io::{Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// The longest any wait here may take.
const SLOW: Duration = Duration::from_secs(20);

/// Output shared between the session's thread and the test.
#[derive(Clone, Default)]
struct Shared(Arc<Mutex<Vec<u8>>>);

impl Write for Shared {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Shared {
    fn bytes(&self) -> Vec<u8> {
        self.0.lock().unwrap().clone()
    }

    /// Waits until the output ends with `end`.
    fn wait_for_end(&self, end: &[u8]) {
        let start = Instant::now();
        while !self.bytes().ends_with(end) {
            assert!(
                start.elapsed() < SLOW,
                "waiting for {:?}, got {:?}",
                String::from_utf8_lossy(end),
                String::from_utf8_lossy(&self.bytes())
            );
            thread::yield_now();
        }
    }
}

/// A server on loopback for one connection, running `serve` on it; returns its address
/// and what `serve` returns.
fn serve_once<T: Send + 'static>(
    serve: impl FnOnce(TcpStream) -> T + Send + 'static,
) -> (SocketAddr, JoinHandle<T>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        stream.set_read_timeout(Some(SLOW)).unwrap();
        serve(stream)
    });
    (addr, handle)
}

/// Echoes what comes until the client's end; returns all of it.
fn echo(mut stream: TcpStream) -> Vec<u8> {
    let mut got = Vec::new();
    let mut buf = [0u8; 512];
    loop {
        match stream.read(&mut buf) {
            Ok(0) | Err(_) => return got,
            Ok(n) => {
                got.extend_from_slice(&buf[..n]);
                if stream.write_all(&buf[..n]).is_err() {
                    return got;
                }
            }
        }
    }
}

/// A session to `addr` on a thread of its own, editing or not: its keys, its output,
/// and how it ends.
fn start_session(addr: SocketAddr, edit: bool) -> (Sender<u8>, Shared, JoinHandle<TcpEnd>) {
    let mut session = TcpSession::new(TcpStream::connect(addr).unwrap());
    session.edit = edit;
    let (keys, input) = mpsc::channel();
    let out = Shared::default();
    let mut writer = out.clone();
    let handle = thread::spawn(move || session.run(input, &mut writer).unwrap());
    (keys, out, handle)
}

fn type_keys(keys: &Sender<u8>, text: &[u8]) {
    for &b in text {
        keys.send(b).unwrap();
    }
}

/// Waits for the thread `h`, for at most [`SLOW`].
fn join<T>(h: JoinHandle<T>) -> T {
    let start = Instant::now();
    while !h.is_finished() {
        assert!(start.elapsed() < SLOW, "the thread never ended");
        thread::yield_now();
    }
    h.join().unwrap()
}

#[test]
fn lines_are_edited_here_and_ctrl_close_bracket_leaves() {
    let (addr, server) = serve_once(echo);
    let (keys, out, session) = start_session(addr, true);
    // A typo fixed with Backspace: the echo shows it, the server gets the line fixed,
    // with its newline, only at Enter.
    type_keys(&keys, b"helo\x7flo");
    out.wait_for_end(b"helo\x08 \x08lo");
    type_keys(&keys, b"\r");
    out.wait_for_end(b"helo\x08 \x08lo\r\nhello\n");
    // Ctrl-W and Ctrl-U; an arrow key, ignored; \r\n is one Enter.
    type_keys(&keys, b"cat one two\x17\x15ls \x1b[A/bin\r\n");
    out.wait_for_end(b"ls /bin\r\nls /bin\n");
    // Ctrl-] leaves at once, the line being typed unsent; the server sees the end.
    type_keys(&keys, b"unsent");
    keys.send(DETACH_KEY).unwrap();
    assert_eq!(join(session), TcpEnd::Left);
    assert_eq!(join(server), b"hello\nls /bin\n");
}

#[test]
fn the_far_end_closing_ends_the_session() {
    let (addr, server) = serve_once(|mut stream: TcpStream| {
        let mut line = Vec::new();
        let mut b = [0u8];
        while line.last() != Some(&b'\n') {
            stream.read_exact(&mut b).unwrap();
            line.push(b[0]);
        }
        stream.write_all(b"bye\n").unwrap();
        line
    });
    let (keys, out, session) = start_session(addr, true);
    type_keys(&keys, b"quit\r");
    assert_eq!(join(session), TcpEnd::Closed);
    assert_eq!(out.bytes(), b"quit\r\nbye\n");
    assert_eq!(join(server), b"quit\n");
}

/// Reads to the client's end, then answers with how many bytes came, and closes.
fn count(mut stream: TcpStream) -> Vec<u8> {
    let mut got = Vec::new();
    stream.read_to_end(&mut got).unwrap();
    writeln!(stream, "got {} bytes", got.len()).unwrap();
    stream.shutdown(Shutdown::Both).unwrap();
    got
}

#[test]
fn ctrl_d_on_an_empty_line_ends_the_input_and_the_answer_still_comes() {
    let (addr, server) = serve_once(count);
    let (keys, out, session) = start_session(addr, true);
    // Ctrl-D on a line with something in it does nothing; on an empty one it ends the
    // input: what comes after it goes nowhere.
    type_keys(&keys, b"abc\x04\r\x04more\r");
    assert_eq!(join(session), TcpEnd::Closed);
    assert_eq!(join(server), b"abc\n");
    assert_eq!(out.bytes(), b"abc\r\ngot 4 bytes\n");
}

#[test]
fn ctrl_close_bracket_still_leaves_after_ctrl_d() {
    // A server that reads to the client's end and then keeps the connection open, until
    // the client is gone: Ctrl-] must not wait for it.
    let (ended_tx, ended) = mpsc::channel();
    let (addr, server) = serve_once(move |mut stream: TcpStream| {
        let mut got = Vec::new();
        stream.read_to_end(&mut got).unwrap();
        ended_tx.send(()).unwrap();
        // Until the client closes or resets its side: a write then fails, or the
        // test's wait runs out.
        let start = Instant::now();
        while start.elapsed() < SLOW && stream.write_all(b".").is_ok() {
            thread::sleep(Duration::from_millis(20));
        }
        got
    });
    let (keys, _out, session) = start_session(addr, true);
    type_keys(&keys, b"ls\r\x04");
    ended.recv_timeout(SLOW).unwrap();
    keys.send(DETACH_KEY).unwrap();
    assert_eq!(join(session), TcpEnd::Left);
    assert_eq!(join(server), b"ls\n");
}

#[test]
fn piped_input_goes_as_it_is() {
    let (addr, server) = serve_once(count);
    let (keys, out, session) = start_session(addr, false);
    // Nothing edited, nothing echoed; the end of the input shuts the sending side down
    // and the session waits for the far end.
    type_keys(&keys, b"x\x7fy\r\n\x04z");
    drop(keys);
    assert_eq!(join(session), TcpEnd::Closed);
    assert_eq!(join(server), b"x\x7fy\r\n\x04z");
    assert_eq!(out.bytes(), b"got 7 bytes\n");

    // The detach key still leaves, what came before it sent.
    let (addr, server) = serve_once(echo);
    let (keys, out, session) = start_session(addr, false);
    type_keys(&keys, b"one\n");
    out.wait_for_end(b"one\n");
    type_keys(&keys, b"two\x1dthree");
    assert_eq!(join(session), TcpEnd::Left);
    assert_eq!(join(server), b"one\ntwo");
}

// ---- the binary ----

fn herald() -> Command {
    Command::new(env!("CARGO_BIN_EXE_herald"))
}

/// Runs `herald --tcp ADDR` with `input` piped in (then its end, unless `keep_open`);
/// returns its exit code, stdout and stderr.
fn run_herald(addr: &str, input: &[u8], keep_open: bool) -> (Option<i32>, Vec<u8>, String) {
    let mut child = herald()
        .args(["--tcp", addr])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    stdin.write_all(input).unwrap();
    let stdin = if keep_open {
        Some(stdin)
    } else {
        drop(stdin);
        None
    };
    let mut stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let out = thread::spawn(move || {
        let mut v = Vec::new();
        stdout.read_to_end(&mut v).unwrap();
        v
    });
    let err = thread::spawn(move || {
        let mut s = String::new();
        stderr.read_to_string(&mut s).unwrap();
        s
    });
    let start = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if start.elapsed() > SLOW {
            let _ = child.kill();
            panic!("herald is still running after {SLOW:?}");
        }
        thread::sleep(Duration::from_millis(5));
    };
    drop(stdin);
    (status.code(), out.join().unwrap(), err.join().unwrap())
}

#[test]
fn the_binary_leaves_on_the_detach_key_in_piped_input() {
    // stdin stays open meanwhile: the key alone ends Herald.
    let (addr, server) = serve_once(echo);
    let (code, out, err) = run_herald(&addr.to_string(), b"hello\n\x1d", true);
    assert_eq!(code, Some(0), "{err}");
    assert_eq!(join(server), b"hello\n");
    // The echo may or may not have come back before Herald left.
    assert!(out == b"hello\n" || out.is_empty(), "{out:?}");
    assert_eq!(
        err,
        format!("herald: connected to {addr}; Ctrl-] leaves\nherald: left {addr}, closing the connection\n")
    );
}

#[test]
fn the_binary_waits_for_the_far_end_after_the_input() {
    // The end of the input: the server answers and closes, Herald prints the answer.
    let (addr, server) = serve_once(count);
    let (code, out, err) = run_herald(&format!("localhost:{}", addr.port()), b"ls\nps\n", false);
    assert_eq!(code, Some(0), "{err}");
    assert_eq!(join(server), b"ls\nps\n");
    assert_eq!(out, b"got 6 bytes\n");
    assert!(
        err.ends_with(&format!("herald: {addr} closed the connection\n")),
        "{err}"
    );
}

#[test]
fn the_binary_reports_bad_addresses_and_refused_connections() {
    for (args, code, want) in [
        (&["--tcp"][..], 2, "herald: --tcp needs a value"),
        (&["--tcp", "127.0.0.1"], 2, "is not HOST:PORT"),
        (&["--tcp", "127.0.0.1:23", "vm"], 2, "no --server, --screen or VM"),
    ] {
        let out = herald().args(args).stdin(Stdio::null()).output().unwrap();
        assert_eq!(out.status.code(), Some(code), "{args:?}");
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains(want), "{args:?}: {err}");
    }
    // A port nothing listens on.
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let out = herald()
        .args(["--tcp", &format!("127.0.0.1:{port}")])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.starts_with(&format!("herald: cannot connect to 127.0.0.1:{port}: ")),
        "{err}"
    );
    // The help mentions it.
    let out = herald().arg("--help").output().unwrap();
    let help = String::from_utf8_lossy(&out.stdout);
    assert!(help.contains("herald --tcp HOST:PORT"), "{help}");
}
