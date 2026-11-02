//! The `herald` binary with pipes for stdin, stdout and stderr (a terminal is
//! `tests/terminal.rs`): flags, errors, a session with the detach key, the end of
//! the input, and a VM that has already exited.

mod common;

use avalon_proto::VmState;
use common::*;
use herald::DETACH_KEY;
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, Command, ExitStatus, Output, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

fn herald() -> Command {
    Command::new(env!("CARGO_BIN_EXE_herald"))
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn help_and_bad_flags() {
    let out = run(&["--help"]);
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(
        text.starts_with("usage: herald [--server ADDR] [--screen] <vm>"),
        "{text}"
    );
    assert!(text.contains("--screen"), "{text}");
    assert!(text.contains("Ctrl-] detaches"), "{text}");
    for args in [
        &[][..],
        &["--bogus", "vm"],
        &["--server"],
        &["--server", "not an address", "vm"],
        &["one", "two"],
    ] {
        let out = run(args);
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        let err = stderr(&out);
        assert!(
            err.starts_with("herald: ") || err.starts_with("usage: "),
            "{args:?}: {err}"
        );
    }
}

#[test]
fn nothing_to_connect_to() {
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let out = run(&["--server", &format!("127.0.0.1:{port}"), "vm"]);
    assert_eq!(out.status.code(), Some(1));
    let err = stderr(&out);
    assert!(
        err.starts_with(&format!("herald: cannot connect to 127.0.0.1:{port}: ")),
        "{err}"
    );
}

/// `--screen` with nothing to show it on: Herald has its window on Windows only, and
/// says so elsewhere before it connects; on Windows, no server is an error as for the
/// console.
#[test]
fn screen_without_a_window_or_a_server() {
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let out = run(&["--screen", "--server", &format!("127.0.0.1:{port}"), "vm"]);
    assert_eq!(out.status.code(), Some(1));
    let err = stderr(&out);
    let expected = if cfg!(windows) {
        format!("herald: cannot connect to 127.0.0.1:{port}: ")
    } else {
        "herald: --screen shows the VM's screen in a window, which Herald only has on Windows".to_string()
    };
    assert!(err.starts_with(&expected), "{err}");
}

/// `--screen` for a VM the server does not have fails before any window opens.
#[cfg(windows)]
#[test]
fn no_such_vm_to_show() {
    let server = server();
    let out = run(&["--server", &server.addr().to_string(), "--screen", "ghost"]);
    assert_eq!(out.status.code(), Some(1));
    let err = stderr(&out);
    assert!(
        err.starts_with("herald: cannot attach to the screen of VM ghost: no such VM"),
        "{err}"
    );
    server.stop().unwrap();
}

#[test]
fn no_such_vm() {
    let server = server();
    let out = run(&["--server", &server.addr().to_string(), "ghost"]);
    assert_eq!(out.status.code(), Some(1));
    let err = stderr(&out);
    assert!(
        err.starts_with("herald: cannot attach to VM ghost: no such VM"),
        "{err}"
    );
    server.stop().unwrap();
}

/// A host name is tried at every address it resolves to: `localhost` reaches a
/// server on 127.0.0.1 (Avalon's default) where it resolves to `[::1]` first.
#[test]
fn localhost_reaches_an_ipv4_server() {
    let server = server();
    let port = server.addr().port();
    let id = start_vm(&server, "echo", 1, example("uart-echo"));
    let out = run(&["--server", &format!("localhost:{port}"), "echo"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let err = stderr(&out);
    assert!(
        err.starts_with(&format!(
            "herald: attached to VM {id} 'echo' (running) on 127.0.0.1:{port};"
        )),
        "{err}"
    );
    // A server that is not there is named as it was given.
    server.stop().unwrap();
    let out = run(&["--server", &format!("localhost:{port}"), "echo"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).starts_with(&format!("herald: cannot connect to localhost:{port}: ")),
        "{}",
        stderr(&out)
    );
}

/// Spawns `herald <vm>` against `server` with all three streams piped, and waits for
/// its "attached" line on stderr.
fn spawn(server: &avalon::ServerThread, vm: &str) -> (Child, String) {
    let mut child = herald()
        .args(["--server", &server.addr().to_string(), vm])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut line = String::new();
    let mut stderr = BufReader::new(child.stderr.take().unwrap());
    stderr.read_line(&mut line).unwrap();
    // The rest of stderr, read on a thread so the child never blocks on it.
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut rest = String::new();
        let _ = stderr.read_to_string(&mut rest);
        let _ = tx.send(rest);
    });
    REST.with(|r| *r.borrow_mut() = Some(rx));
    (child, line)
}

thread_local! {
    /// The rest of the last spawned child's stderr.
    static REST: std::cell::RefCell<Option<mpsc::Receiver<String>>> = const { std::cell::RefCell::new(None) };
}

fn rest_of_stderr() -> String {
    REST.with(|r| r.borrow_mut().take().unwrap().recv_timeout(SLOW).unwrap())
}

/// Waits for the child to exit, for at most `SLOW`: a Herald that does not end is
/// killed and fails the test rather than hang it.
fn wait(child: &mut Child) -> ExitStatus {
    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        if start.elapsed() > SLOW {
            let _ = child.kill();
            let _ = child.wait();
            panic!("herald is still running after {SLOW:?}");
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Like `Child::wait_with_output` (stdin, if still held, stays open), with `wait`'s
/// deadline. A stream already taken comes back empty.
fn wait_with_output(mut child: Child) -> Output {
    fn drain(stream: Option<impl Read + Send + 'static>) -> std::thread::JoinHandle<Vec<u8>> {
        std::thread::spawn(move || {
            let mut seen = Vec::new();
            if let Some(mut stream) = stream {
                let _ = stream.read_to_end(&mut seen);
            }
            seen
        })
    }
    let stdout = drain(child.stdout.take());
    let stderr = drain(child.stderr.take());
    let status = wait(&mut child);
    Output {
        status,
        stdout: stdout.join().unwrap(),
        stderr: stderr.join().unwrap(),
    }
}

/// Runs `herald args` with stdin at its end, and collects its output.
fn run(args: &[&str]) -> Output {
    let child = herald()
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    wait_with_output(child)
}

/// Reads the child's stdout until it holds `needle`.
fn read_until(stdout: &mut impl Read, seen: &mut Vec<u8>, needle: &str) {
    let start = Instant::now();
    let mut buf = [0u8; 256];
    while !String::from_utf8_lossy(seen).contains(needle) {
        assert!(start.elapsed() < SLOW, "waiting for {needle:?}, got {seen:?}");
        let n = stdout.read(&mut buf).unwrap();
        assert!(n > 0, "stdout ended waiting for {needle:?}, got {seen:?}");
        seen.extend_from_slice(&buf[..n]);
    }
}

/// Typing at the VM through the binary, then the detach key: Herald exits, the VM
/// carries on.
#[test]
fn a_session_through_pipes() {
    let server = server();
    let id = start_vm(&server, "echo", 1, example("uart-echo"));
    let (mut child, line) = spawn(&server, "echo");
    assert_eq!(
        line.trim_end(),
        format!(
            "herald: attached to VM {id} 'echo' (running) on {}; Ctrl-] or the end of the input detaches",
            server.addr()
        )
    );
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = child.stdout.take().unwrap();
    let mut seen = Vec::new();
    stdin.write_all(b"hi there\n").unwrap();
    read_until(&mut stdout, &mut seen, "hi there\n");
    stdin.write_all(b"!").unwrap();
    read_until(&mut stdout, &mut seen, "!");
    // The detach key works through a pipe too.
    stdin.write_all(&[DETACH_KEY]).unwrap();
    let status = wait(&mut child);
    assert!(status.success());
    stdout.read_to_end(&mut seen).unwrap();
    assert_eq!(seen, b"hi there\n!");
    let err = rest_of_stderr();
    assert_eq!(
        err.trim_end(),
        format!("herald: detached from VM {id} 'echo', which carries on")
    );
    assert_eq!(state(&server, id), VmState::Running);

    // With stdin at its end at once: the scrollback, and out.
    let out = run(&["--server", &server.addr().to_string(), &id.to_string()]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(out.stdout, b"hi there\n!");
    assert!(
        stderr(&out).ends_with(&format!(
            "herald: end of input: detached from VM {id} 'echo', which carries on\n"
        )),
        "{}",
        stderr(&out)
    );
    server.stop().unwrap();
}

/// A VM that exits ends Herald with it; one that has already exited shows its
/// scrollback and ends at once, with input still open.
#[test]
fn the_vm_exiting_ends_herald() {
    let server = server();
    let id = start_vm(&server, "echo", 1, example("uart-echo"));
    let (mut child, _) = spawn(&server, "echo");
    let mut stdin = child.stdin.take().unwrap();
    stdin.write_all(b"bye").unwrap();
    stdin.write_all(&[4]).unwrap(); // EOT halts uart-echo
    let out = wait_with_output(child);
    assert!(out.status.success());
    assert_eq!(out.stdout, b"bye");
    assert_eq!(
        rest_of_stderr().trim_end(),
        format!("herald: VM {id} 'echo' exited with code 0")
    );
    assert_eq!(state(&server, id), VmState::Exited(0));

    let (mut child, line) = spawn(&server, &id.to_string());
    assert!(line.contains("(exited(0))"), "{line}");
    // Held open: Herald ends because of the VM, not the input.
    let _stdin = child.stdin.take().unwrap();
    let out = wait_with_output(child);
    assert!(out.status.success());
    assert_eq!(out.stdout, b"bye");
    assert_eq!(
        rest_of_stderr().trim_end(),
        format!("herald: VM {id} 'echo' exited with code 0")
    );
    server.stop().unwrap();
}
