//! Herald: the client of the Avalon platform that attaches to one VM to interact with
//! it directly. This version does the VM's serial console (its UART): the recent output
//! first, then the output live, and what is typed goes to the VM. The screen and the
//! keyboard device come later, over the same connection.
//!
//! - [`Session`] is the streaming layer, and knows nothing of terminals: it attaches
//!   over an Avalon [`Connection`], then runs one loop over everything that can happen
//!   (output and events from the server, input bytes from a channel) until the detach
//!   key, the end of the input, or the end of the VM. Output goes to any `Write`. The
//!   tests drive it that way, with no terminal.
//! - [`Utf8Lossy`] keeps a Windows console fed with valid UTF-8, which Rust's console
//!   stdout insists on.
//! - [`parse_server`] reads `--server` values.
//!
//! The `herald` binary wires a session to the terminal: stdin in raw mode (Grail's
//! [`grail::console::RawMode`], shared with Grail's own console) and stdout.

use avalon_proto::{
    AttachKind, Connection, Error, Message, MessageKind, VmRef, VmState, VmSummary, DEFAULT_AVALON_ADDR,
};
use std::io::{self, Write};
use std::net::{IpAddr, Shutdown, SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

/// Herald's name in the `Hello` it sends.
pub const AGENT: &str = "herald";

/// The key that detaches: Ctrl-] (ASCII GS), as in telnet. It never reaches the VM.
pub const DETACH_KEY: u8 = 0x1d;

/// How [`DETACH_KEY`] is written for people.
pub const DETACH_KEY_NAME: &str = "Ctrl-]";

/// How long a session waits for the server to confirm a detach before it ends anyway.
pub const DETACH_TIMEOUT: Duration = Duration::from_secs(5);

/// Most input bytes sent in one `ConsoleInput`. Keys come one or a few at a time;
/// this only bounds a burst (a paste, or input piped in).
const MAX_INPUT_BATCH: usize = 4096;

/// Why a session ended. Every one of them leaves the VM as it is: detaching never
/// pauses or stops it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum End {
    /// The detach key was pressed.
    Detached,
    /// The input ended (stdin closed, or not a terminal and read to its end), which
    /// detaches as well.
    InputEnded,
    /// The VM stopped or exited (on attaching, or later). Nothing more can come from
    /// it: a stopped VM cannot be restarted.
    VmEnded(VmState),
    /// The VM was destroyed (`Detached` from the server).
    Destroyed,
}

impl End {
    /// A sentence for the person at the terminal.
    pub fn describe(&self, vm: &VmSummary) -> String {
        let name = format!("VM {} '{}'", vm.id, vm.name);
        match self {
            End::Detached => format!("detached from {name}, which carries on"),
            End::InputEnded => format!("end of input: detached from {name}, which carries on"),
            End::VmEnded(VmState::Exited(code)) => format!("{name} exited with code {code}"),
            End::VmEnded(state) => format!("{name} is {state}"),
            End::Destroyed => format!("{name} was destroyed"),
        }
    }
}

/// Whether a VM in `state` can still run: stopped and exited are final.
fn ended(state: VmState) -> bool {
    matches!(state, VmState::Stopped | VmState::Exited(_))
}

/// What the session loop waits for: the server and the input, on one channel, so a
/// single thread handles both in the order they happen.
enum Event {
    Server(Result<Message, Error>),
    Input(Vec<u8>),
    InputEnded,
}

/// A console attached to one VM over an Avalon connection.
///
/// [`Session::attach`] attaches and learns the VM's name and state; [`Session::run`]
/// writes the scrollback and streams until the session ends. Several sessions (in one
/// process or many) may attach to one VM: all of them get its output, and input from
/// any of them reaches it.
pub struct Session {
    conn: Connection<TcpStream>,
    /// The VM, as the server described it right after the attach.
    pub vm: VmSummary,
    /// Its recent output, from before the attach (Avalon keeps the last 64 KiB).
    pub scrollback: Vec<u8>,
    /// The byte that detaches (default [`DETACH_KEY`]); `None` for none, so every byte
    /// reaches the VM and only the end of the input detaches.
    pub detach_key: Option<u8>,
    /// How long to wait for the server to confirm a detach (default [`DETACH_TIMEOUT`]).
    pub detach_timeout: Duration,
    /// Set once a detach is under way: why the session will end.
    detaching: Option<End>,
}

impl Session {
    /// Attaches to `vm`'s console on `conn` (a connection after its handshake), then
    /// asks for the VM's name and state. Fails with the server's error if there is no
    /// such VM.
    pub fn attach(mut conn: Connection<TcpStream>, vm: &VmRef) -> Result<Session, Error> {
        let (id, scrollback) = match conn.request(&Message::ConsoleAttach { vm: vm.clone() })? {
            Message::ConsoleAttached { vm, scrollback } => (vm, scrollback),
            other => return Err(Error::Unexpected(other.type_code())),
        };
        // Output that arrives meanwhile is queued by `request`; `run` hands it on.
        let vm = match conn.request(&Message::GetVmInfo { vm: VmRef::Id(id) })? {
            Message::VmInfo(details) => details.vm,
            other => return Err(Error::Unexpected(other.type_code())),
        };
        Ok(Session {
            conn,
            vm,
            scrollback,
            detach_key: Some(DETACH_KEY),
            detach_timeout: DETACH_TIMEOUT,
            detaching: None,
        })
    }

    /// Writes the scrollback to `out`, then streams: console output to `out` (flushed
    /// as it comes), each burst of `input` to the VM, and a line on `notices` when the
    /// VM is paused or resumed. Returns when the detach key comes in, the input ends,
    /// or the VM stops, exits or is destroyed; a VM that had already stopped or exited
    /// ends the session right after its scrollback.
    ///
    /// Losing the connection, or failing to write `out`, is an error. Events this
    /// session does not need (other VMs', ones that fail to decode) are passed over, as
    /// the protocol says a client should.
    ///
    /// `input` is read on a thread of its own, which stays blocked on it after the
    /// session ends until its next byte or its end (a terminal cannot be interrupted).
    pub fn run(
        mut self,
        input: Receiver<u8>,
        out: &mut dyn Write,
        notices: &mut dyn Write,
    ) -> Result<End, Error> {
        out.write_all(&self.scrollback)?;
        out.flush()?;

        // Events read while attaching come first.
        while self.conn.queued_events() > 0 {
            let event = self.conn.recv();
            if let Some(end) = self.on_server(event, out, notices)? {
                return Ok(end);
            }
        }
        if ended(self.vm.state) {
            return Ok(End::VmEnded(self.vm.state));
        }

        let (tx, events) = mpsc::channel();
        let mut reader = self.conn.try_clone()?;
        // The reading thread waits for as long as the session lasts.
        reader.get_ref().set_read_timeout(None)?;
        let server_tx = tx.clone();
        let reading = std::thread::spawn(move || loop {
            let msg = reader.recv();
            let fatal = matches!(&msg, Err(e) if e.is_fatal());
            if server_tx.send(Event::Server(msg)).is_err() || fatal {
                break;
            }
        });
        std::thread::spawn(move || forward_input(input, tx));

        let result = self.stream(events, out, notices);
        // Unblocks the reading thread (and tells the server we are gone).
        let _ = self.conn.get_ref().shutdown(Shutdown::Both);
        let _ = reading.join();
        result
    }

    /// The session loop.
    fn stream(
        &mut self,
        events: Receiver<Event>,
        out: &mut dyn Write,
        notices: &mut dyn Write,
    ) -> Result<End, Error> {
        let mut deadline: Option<Instant> = None;
        loop {
            let event = match deadline {
                None => events.recv().map_err(|_| Error::Closed)?,
                Some(deadline) => {
                    let left = deadline.saturating_duration_since(Instant::now());
                    match events.recv_timeout(left) {
                        Ok(event) => event,
                        // No answer in time, or nothing left to hear one on: the detach
                        // is done as far as we are concerned, closing the connection
                        // ends the attachment anyway.
                        Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => {
                            return Ok(self.detaching.expect("a deadline is set while detaching"))
                        }
                    }
                }
            };
            match event {
                Event::Server(msg) => {
                    if let Some(end) = self.on_server(msg, out, notices)? {
                        return Ok(end);
                    }
                }
                Event::Input(_) | Event::InputEnded if self.detaching.is_some() => {}
                Event::Input(bytes) => {
                    let key = self.detach_key;
                    let (keys, detach) = match bytes.iter().position(|&b| Some(b) == key) {
                        Some(at) => (&bytes[..at], true),
                        None => (&bytes[..], false),
                    };
                    if !keys.is_empty() {
                        self.conn.send(&Message::ConsoleInput {
                            vm: self.vm.id,
                            data: keys.to_vec(),
                        })?;
                    }
                    if detach {
                        deadline = Some(self.detach(End::Detached)?);
                    }
                }
                Event::InputEnded => deadline = Some(self.detach(End::InputEnded)?),
            }
        }
    }

    /// Sends `ConsoleDetach`; the session ends with `end` when the reply comes.
    /// Returns how long to wait for it.
    fn detach(&mut self, end: End) -> Result<Instant, Error> {
        self.conn.send(&Message::ConsoleDetach { vm: self.vm.id })?;
        self.detaching = Some(end);
        Ok(Instant::now() + self.detach_timeout)
    }

    /// Handles what the server sent. Returns the end of the session, if it is one.
    fn on_server(
        &mut self,
        msg: Result<Message, Error>,
        out: &mut dyn Write,
        notices: &mut dyn Write,
    ) -> Result<Option<End>, Error> {
        let id = self.vm.id;
        match msg {
            Ok(Message::ConsoleOutput { vm, data }) if vm == id => {
                out.write_all(&data)?;
                out.flush()?;
            }
            Ok(Message::VmStateChanged { vm, state }) if vm == id => {
                self.vm.state = state;
                if ended(state) {
                    return Ok(Some(End::VmEnded(state)));
                }
                // Not fatal if stderr is gone: the console is what matters.
                let _ = writeln!(notices, "herald: VM {id} is {state}");
            }
            Ok(Message::Detached {
                vm,
                kind: AttachKind::Console,
            }) if vm == id => return Ok(Some(End::Destroyed)),
            // The only request a running session sends is ConsoleDetach, so a reply
            // (Ok, or an Error if the server had already let go) is its answer.
            Ok(reply) if reply.kind() == MessageKind::Reply => match self.detaching {
                Some(end) => return Ok(Some(end)),
                None => return Err(Error::Unexpected(reply.type_code())),
            },
            Ok(_) => {}
            Err(Error::Closed) if self.detaching.is_some() => return Ok(self.detaching),
            Err(e) if e.is_fatal() => return Err(e),
            // An event that does not decode: set aside, never answered.
            Err(_) => {}
        }
        Ok(None)
    }
}

/// Passes input bytes on to the session loop, a burst at a time, then its end.
fn forward_input(input: Receiver<u8>, events: Sender<Event>) {
    while let Ok(first) = input.recv() {
        let mut burst = vec![first];
        while burst.len() < MAX_INPUT_BATCH {
            match input.try_recv() {
                Ok(b) => burst.push(b),
                Err(_) => break,
            }
        }
        if events.send(Event::Input(burst)).is_err() {
            return;
        }
    }
    let _ = events.send(Event::InputEnded);
}

/// A writer that passes on only valid UTF-8: each invalid byte becomes U+FFFD, and a
/// character split across two writes is held back until its last byte comes.
///
/// A Windows console needs this: Rust's stdout writes to it as UTF-16 and refuses
/// bytes that are not UTF-8, and its buffer then keeps failing on them. A guest's
/// UART output is raw bytes, so Herald puts this between the console and a Windows
/// terminal. Pipes and files get the bytes as they are.
pub struct Utf8Lossy<W: Write> {
    inner: W,
    /// The start of a character whose other bytes have not come yet.
    pending: Vec<u8>,
}

impl<W: Write> Utf8Lossy<W> {
    pub fn new(inner: W) -> Self {
        Utf8Lossy {
            inner,
            pending: Vec::new(),
        }
    }

    pub fn into_inner(self) -> W {
        self.inner
    }
}

impl<W: Write> Write for Utf8Lossy<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut bytes = std::mem::take(&mut self.pending);
        bytes.extend_from_slice(buf);
        let mut clean = Vec::with_capacity(bytes.len() + 3);
        let mut rest = &bytes[..];
        loop {
            match std::str::from_utf8(rest) {
                Ok(text) => {
                    clean.extend_from_slice(text.as_bytes());
                    rest = &[];
                    break;
                }
                Err(e) => {
                    let (valid, after) = rest.split_at(e.valid_up_to());
                    clean.extend_from_slice(valid);
                    match e.error_len() {
                        Some(bad) => {
                            clean.extend_from_slice("\u{fffd}".as_bytes());
                            rest = &after[bad..];
                        }
                        // The start of a character, cut off by the end of the buffer.
                        None => {
                            rest = after;
                            break;
                        }
                    }
                }
            }
        }
        self.pending = rest.to_vec();
        self.inner.write_all(&clean)?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

/// Parses a `--server` value: `host:port`, or just a host or IP address (Avalon's
/// default port, 7400). IPv6 addresses with a port go in brackets: `[::1]:7400`.
pub fn parse_server(s: &str) -> Result<SocketAddr, String> {
    let default_port = DEFAULT_AVALON_ADDR
        .parse::<SocketAddr>()
        .expect("the default address parses")
        .port();
    if let Ok(addr) = s.parse::<SocketAddr>() {
        return Ok(addr);
    }
    if let Ok(ip) = s.parse::<IpAddr>() {
        return Ok(SocketAddr::new(ip, default_port));
    }
    let resolved = if s.is_empty() || s.contains(char::is_whitespace) {
        Err(io::ErrorKind::InvalidInput.into())
    } else if s.contains(':') {
        s.to_socket_addrs()
    } else {
        (s, default_port).to_socket_addrs()
    };
    match resolved.map(|mut addrs| addrs.next()) {
        Ok(Some(addr)) => Ok(addr),
        _ => Err(format!(
            "'{s}' is not a server address (examples: 127.0.0.1:7400, localhost, 10.0.0.5:7500)"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lossy(chunks: &[&[u8]]) -> Vec<u8> {
        let mut w = Utf8Lossy::new(Vec::new());
        for c in chunks {
            assert_eq!(w.write(c).unwrap(), c.len());
        }
        w.into_inner()
    }

    #[test]
    fn utf8_passes_through() {
        assert_eq!(
            lossy(&[b"hello\r\n", "olá ✓".as_bytes()]),
            "hello\r\nolá ✓".as_bytes()
        );
        assert_eq!(lossy(&[b""]), b"");
    }

    #[test]
    fn invalid_bytes_become_replacement_characters() {
        assert_eq!(lossy(&[b"a\xffb\xfe"]), "a\u{fffd}b\u{fffd}".as_bytes());
        // A lone continuation byte, and a cut-off character followed by ASCII.
        assert_eq!(lossy(&[b"\x80x", b"\xc3y"]), "\u{fffd}x\u{fffd}y".as_bytes());
    }

    #[test]
    fn a_character_split_across_writes_is_kept_whole() {
        let check = "✓".as_bytes(); // e2 9c 93
        assert_eq!(
            lossy(&[b"a", &check[..1], &check[1..2], &check[2..], b"b"]),
            "a✓b".as_bytes()
        );
        // Nothing of it goes out until it is complete.
        let mut w = Utf8Lossy::new(Vec::new());
        w.write_all(&check[..2]).unwrap();
        assert!(w.into_inner().is_empty());
    }

    #[test]
    fn server_addresses() {
        assert_eq!(
            parse_server("127.0.0.1:7500").unwrap(),
            "127.0.0.1:7500".parse().unwrap()
        );
        assert_eq!(
            parse_server("10.1.2.3").unwrap(),
            "10.1.2.3:7400".parse().unwrap()
        );
        assert_eq!(parse_server("::1").unwrap(), "[::1]:7400".parse().unwrap());
        assert_eq!(parse_server("[::1]:9").unwrap(), "[::1]:9".parse().unwrap());
        assert_eq!(parse_server("localhost").unwrap().port(), 7400);
        assert_eq!(parse_server("localhost:7600").unwrap().port(), 7600);
        for bad in ["", "two words", "127.0.0.1:port", "host:99999"] {
            assert!(parse_server(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn ends_describe_themselves() {
        let vm = VmSummary {
            id: 3,
            name: "demo".into(),
            state: VmState::Running,
            profile: Default::default(),
        };
        assert_eq!(
            End::Detached.describe(&vm),
            "detached from VM 3 'demo', which carries on"
        );
        assert_eq!(
            End::VmEnded(VmState::Exited(7)).describe(&vm),
            "VM 3 'demo' exited with code 7"
        );
        assert_eq!(
            End::VmEnded(VmState::Stopped).describe(&vm),
            "VM 3 'demo' is stopped"
        );
        assert_eq!(End::Destroyed.describe(&vm), "VM 3 'demo' was destroyed");
    }
}
