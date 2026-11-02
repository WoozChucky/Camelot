//! User networking end to end (spec section 9.8, "User networking"): the bare-metal
//! `examples/net/net-user` gets its address from the gateway by DHCP, opens a TCP
//! connection through 10.0.2.2 to a listener of this test on the host's 127.0.0.1, and
//! accepts a connection the test makes to a forwarded host port (`hostfwd`). Then the
//! same through the `grail --net user` command line.
//!
//! Nothing here needs the internet. The guest runs on its own thread at full speed
//! while the test plays the host side with real sockets; every wait polls with a
//! deadline, so a broken exchange fails instead of hanging.

use gateway::{Config, Gateway, HostFwd, DEFAULT_GUEST_MAC};
use grail::net::MacAddr;
use grail::{Machine, MachineConfig};
use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// How long any step may take before the test fails.
const DEADLINE: Duration = Duration::from_secs(30);

fn elf_path() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/net/net-user.elf")
}

/// The UART's output, shared with the machine's thread.
#[derive(Clone, Default)]
struct Output(Arc<Mutex<Vec<u8>>>);

impl Write for Output {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Output {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }

    /// Waits until the guest has printed `what`.
    fn wait_for(&self, what: &str) {
        let start = Instant::now();
        while !self.text().contains(what) {
            assert!(
                start.elapsed() < DEADLINE,
                "no {what:?}; output so far:\n{}",
                self.text()
            );
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

/// `net-user` on a machine whose card is `gw`, reading `input` from its UART, running
/// on its own thread until it exits (its exit code) or the deadline passes (`None`).
fn boot(gw: Gateway, input: &str) -> (JoinHandle<Option<u32>>, Output) {
    let cards: Vec<(MacAddr, Box<dyn grail::net::NetBackend>)> =
        vec![(MacAddr(DEFAULT_GUEST_MAC), Box::new(gw))];
    let mut m = Machine::with_devices(MachineConfig::default(), Vec::new(), cards).unwrap();
    m.load_elf(&std::fs::read(elf_path()).unwrap()).unwrap();
    let (tx, rx) = channel();
    for b in input.bytes() {
        tx.send(b).unwrap();
    }
    m.set_uart_input(rx);
    let out = Output::default();
    m.set_uart_output(out.clone());
    let guest = std::thread::spawn(move || {
        let start = Instant::now();
        while m.exit_code().is_none() && start.elapsed() < DEADLINE {
            m.run_for(1_000_000, &mut |_| {});
        }
        drop(tx);
        m.exit_code()
    });
    (guest, out)
}

/// Accepts one connection on `listener`, giving up at the deadline.
fn accept(listener: &TcpListener) -> TcpStream {
    listener.set_nonblocking(true).unwrap();
    let start = Instant::now();
    loop {
        match listener.accept() {
            Ok((s, _)) => {
                s.set_nonblocking(false).unwrap();
                s.set_read_timeout(Some(DEADLINE)).unwrap();
                return s;
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                assert!(start.elapsed() < DEADLINE, "the guest never connected");
                std::thread::sleep(Duration::from_millis(1));
            }
            Err(e) => panic!("accept: {e}"),
        }
    }
}

/// The host side of `c PORT`: the guest's greeting in, an answer out, then close.
fn serve_the_guest(listener: &TcpListener) {
    let mut s = accept(listener);
    let mut greeting = [0; 22];
    s.read_exact(&mut greeting).unwrap();
    assert_eq!(&greeting, b"Hello from the guest!\n");
    s.write_all(b"Hello from the host!\n").unwrap();
}

#[test]
fn guest_reaches_the_host_and_the_host_reaches_the_guest() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let gw = Gateway::new(Config {
        hostfwd: vec![HostFwd::tcp(0, 7)],
        ..Config::default()
    })
    .unwrap();
    let forwarded = gw.hostfwd()[0].host;
    let (guest, out) = boot(gw, &format!("c {port}\nl 7\nq\n"));

    // Guest to host, through 10.0.2.2.
    serve_the_guest(&listener);
    // Host to guest, through the forwarded port, once the guest listens.
    out.wait_for("listen 7\n");
    let mut s = TcpStream::connect(forwarded).unwrap();
    s.set_read_timeout(Some(DEADLINE)).unwrap();
    s.write_all(b"ping over hostfwd\n").unwrap();
    let mut echo = [0; 18];
    s.read_exact(&mut echo).unwrap();
    assert_eq!(&echo, b"ping over hostfwd\n");
    s.shutdown(std::net::Shutdown::Write).unwrap();
    let mut rest = Vec::new();
    s.read_to_end(&mut rest).unwrap();
    assert_eq!(rest, b"", "the guest closes after the host");

    assert_eq!(guest.join().unwrap(), Some(0), "output:\n{}", out.text());
    assert_eq!(
        out.text(),
        format!(
            "net 0x10009000, irq 12, mac 52:54:00:12:34:56, link up\n\
             dhcp: address 10.0.2.15, server 10.0.2.2\n\
             connect 10.0.2.2:{port}\n\
             received: Hello from the host!\n\
             closed\n\
             listen 7\n\
             accepted from 10.0.2.2:49152\n\
             echoed 18 bytes\n\
             closed\n"
        )
    );
}

#[test]
fn a_refused_connection_is_reported() {
    // A port nobody listens on: bind one, then close it.
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let (guest, out) = boot(Gateway::new(Config::default()).unwrap(), &format!("c {port}\n"));
    assert_eq!(guest.join().unwrap(), Some(2), "output:\n{}", out.text());
    assert!(
        out.text()
            .ends_with(&format!("connect 10.0.2.2:{port}\nconnection refused\n")),
        "{}",
        out.text()
    );
}

#[test]
fn grail_command_line_user_networking() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let mut grail = Command::new(env!("CARGO_BIN_EXE_grail"))
        .args(["--net", "user", "--max-steps", "50000000000"])
        .arg(elf_path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    grail
        .stdin
        .take()
        .unwrap()
        .write_all(format!("c {port}\nq\n").as_bytes())
        .unwrap();
    serve_the_guest(&listener);
    let start = Instant::now();
    while grail.try_wait().unwrap().is_none() {
        if start.elapsed() > DEADLINE {
            let _ = grail.kill();
            panic!("grail did not exit");
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let out = grail.wait_with_output().unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        format!(
            "net 0x10009000, irq 12, mac 52:54:00:12:34:56, link up\n\
             dhcp: address 10.0.2.15, server 10.0.2.2\n\
             connect 10.0.2.2:{port}\n\
             received: Hello from the host!\n\
             closed\n"
        )
    );
}

#[test]
fn grail_command_line_rejects_bad_rules() {
    let out = Command::new(env!("CARGO_BIN_EXE_grail"))
        .args(["--net", "user,hostfwd=tcp::80"])
        .arg(elf_path())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("--net: 'tcp::80' is not a forwarding rule"), "{err}");
}
