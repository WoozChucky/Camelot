//! Herald: the client of the Avalon platform that attaches to one VM to interact with
//! it directly: its serial console (its UART) in the terminal, the recent output first,
//! then the output live, with what is typed going to the VM; or its screen (the
//! framebuffer) in a window, with the window's keys and mouse going to the VM's keyboard
//! and pointer devices.
//!
//! - [`Session`] is the console's streaming layer, and knows nothing of terminals: it
//!   attaches over an Avalon [`Connection`], then runs one loop over everything that
//!   can happen (output and events from the server, input bytes from a channel) until
//!   the detach key, the end of the input, or the end of the VM. Output goes to any
//!   `Write`. The tests drive it that way, with no terminal.
//! - [`screen::ScreenSession`] is the screen's, and knows nothing of windows: it
//!   rebuilds the VM's screen from the server's frames and shows it in any
//!   [`screen::View`]. The tests drive it with a view made of vectors; on Windows the
//!   view is [`window::WindowView`], Grail's own framebuffer window.
//! - [`Utf8Lossy`] keeps a Windows console fed with valid UTF-8, which Rust's console
//!   stdout insists on.
//! - [`parse_args`] reads Herald's command line, and [`parse_server`] its `--server`
//!   values.
//!
//! The `herald` binary wires a console session to the terminal: stdin in raw mode
//! (Grail's [`grail::console::RawMode`], shared with Grail's own console) and stdout;
//! and with `--screen`, a screen session to a window.

pub mod screen;
#[cfg(windows)]
pub mod window;

use avalon_proto::{
    kind_of, AttachKind, Connection, Error, Frame, Message, MessageKind, VmRef, VmState, VmSummary,
    DEFAULT_AVALON_ADDR,
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

/// Why a session (console or screen) ended. Every one of them leaves the VM as it is:
/// detaching never pauses or stops it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum End {
    /// The detach key was pressed (the console), or the window closed (the screen).
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
pub(crate) fn ended(state: VmState) -> bool {
    matches!(state, VmState::Stopped | VmState::Exited(_))
}

/// What the session loop waits for: the server and the input, on one channel, so a
/// single thread handles both in the order they happen.
enum Event {
    /// A frame, still undecoded so that its type is known even when it does not
    /// decode, or the error that ended the reading.
    Server(Result<Frame, Error>),
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

        // Events read while attaching come first. `request` queued only events, and
        // the ones that failed to decode as their errors: those are passed over.
        while self.conn.queued_events() > 0 {
            if let Ok(event) = self.conn.recv() {
                if let Some(end) = self.on_server(event.type_code(), Ok(event), out, notices)? {
                    return Ok(end);
                }
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
        // Whole frames, not messages: a frame that fails to decode must still be placed
        // by its type (a reply that fails to decode is still the reply).
        let reading = std::thread::spawn(move || loop {
            let frame = reader.recv_frame();
            let fatal = matches!(&frame, Err(e) if e.is_fatal());
            if server_tx.send(Event::Server(frame)).is_err() || fatal {
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
                Event::Server(frame) => {
                    if let Some(end) = self.on_frame(frame, out, notices)? {
                        return Ok(end);
                    }
                }
                // Keys typed while a detach waits for its answer go nowhere: the
                // session is ending, and one ConsoleDetach is enough.
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

    /// Handles a frame the reading thread got, or the error that ended its reading.
    /// Returns the end of the session, if it is one.
    fn on_frame(
        &mut self,
        frame: Result<Frame, Error>,
        out: &mut dyn Write,
        notices: &mut dyn Write,
    ) -> Result<Option<End>, Error> {
        match frame {
            Ok(frame) => self.on_server(frame.ty, frame.message(), out, notices),
            // The server hanging up on a detach: the detach is done all the same.
            Err(Error::Closed) if self.detaching.is_some() => Ok(self.detaching),
            Err(e) if e.is_fatal() => Err(e),
            // A frame too short to hold a type (consumed whole). It is no event, so it
            // cannot be placed while an answer is due; otherwise it is set aside.
            Err(e) => match self.detaching {
                Some(_) => Err(out_of_step(e)),
                None => Ok(None),
            },
        }
    }

    /// Handles a frame of type `ty` from the server, decoded as `msg`. Returns the end
    /// of the session, if it is one.
    ///
    /// The only request a running session sends is `ConsoleDetach`. While its answer
    /// is due, frames are placed as `Connection::request` places them (protocol
    /// section 4): events are handled, a reply-type frame is the answer even when it
    /// fails to decode, and anything else (a request, a type not in the catalogue)
    /// means the two sides are out of step.
    fn on_server(
        &mut self,
        ty: u16,
        msg: Result<Message, Error>,
        out: &mut dyn Write,
        notices: &mut dyn Write,
    ) -> Result<Option<End>, Error> {
        let id = self.vm.id;
        match (kind_of(ty), msg) {
            (Some(MessageKind::Event), Ok(Message::ConsoleOutput { vm, data })) if vm == id => {
                out.write_all(&data)?;
                out.flush()?;
            }
            (Some(MessageKind::Event), Ok(Message::VmStateChanged { vm, state })) if vm == id => {
                self.vm.state = state;
                if ended(state) {
                    return Ok(Some(End::VmEnded(state)));
                }
                // Not fatal if stderr is gone: the console is what matters.
                let _ = writeln!(notices, "herald: VM {id} is {state}");
            }
            (
                Some(MessageKind::Event),
                Ok(Message::Detached {
                    vm,
                    kind: AttachKind::Console,
                }),
            ) if vm == id => return Ok(Some(End::Destroyed)),
            // Other events (other VMs', the screen's) and events that do not decode:
            // set aside, never answered.
            (Some(MessageKind::Event), _) => {}
            // A reply (Ok, or an Error if the server had already let go) is the answer
            // to the ConsoleDetach, whatever it holds.
            (Some(MessageKind::Reply), _) => match self.detaching {
                Some(end) => return Ok(Some(end)),
                None => return Err(Error::Unexpected(ty)),
            },
            // A request, or a type this client does not know.
            (Some(MessageKind::Request) | None, msg) => {
                if self.detaching.is_some() {
                    return Err(out_of_step(msg.err().unwrap_or(Error::Unexpected(ty))));
                }
            }
        }
        Ok(None)
    }
}

/// The error for a frame that cannot be placed while the answer to a request is due
/// (as `Connection::request` reports it).
pub(crate) fn out_of_step(cause: Error) -> Error {
    Error::OutOfStep(format!("waiting for a reply, got: {cause}"))
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

/// Herald's command line, as `herald --help` prints it.
pub const USAGE: &str = "usage: herald [--server ADDR] [--screen] <vm>

Attaches to the serial console of a VM on an Avalon server: prints its recent
output, then streams the output live and sends what you type to the VM. <vm> is a
VM id (all digits) or a VM name.

  --server ADDR   the Avalon server: host:port, or a host or IP address for port
                  7400 (default 127.0.0.1:7400)
  --screen        show the VM's screen (its framebuffer) in a window instead, and
                  send the keys typed and the mouse moved over it to the VM's
                  keyboard and pointer devices. Closing the window detaches; when
                  the VM ends, the window stays on its last frame until closed.
                  Windows only
  -h, --help      show this text

On the console, Ctrl-] detaches and leaves the VM running; so does the end of
the input when it is not a terminal (where Ctrl-] still detaches). Herald also
ends when the VM stops, exits or is destroyed. Ctrl-C leaves Herald at once, the
VM running. Any number of Heralds may attach to one VM, to its console or its
screen: all of them see its output, and what any of them types reaches it.

Exit status: 0 when detached or when the VM ended, 1 on an error (no server, no
such VM, connection lost, console output failed, --screen without a window), 2
for bad arguments; Ctrl-C gives the usual status of an interrupted program (130
on Linux, STATUS_CONTROL_C_EXIT on Windows).";

/// What Herald's command line asks for ([`parse_args`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Args {
    /// `--server`: the value as written (for messages) and the addresses it names.
    pub server: Option<(String, Vec<SocketAddr>)>,
    /// `--screen`: the VM's screen in a window instead of its console.
    pub screen: bool,
    pub vm: VmRef,
}

/// Parses Herald's arguments (without the program name): `[--server ADDR] [--screen]
/// <vm>`. `Ok(None)` asks for the help text ([`USAGE`]); an error is a bad command
/// line (exit status 2).
pub fn parse_args(args: &[String]) -> Result<Option<Args>, String> {
    let mut server = None;
    let mut vm = None;
    let mut screen = false;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "-h" | "--help" => return Ok(None),
            "--screen" => screen = true,
            "--server" => match it.next() {
                Some(s) => server = Some((s.clone(), parse_server(s)?)),
                None => return Err("--server needs a value".to_string()),
            },
            _ if a.starts_with('-') && a.len() > 1 => {
                return Err(format!(
                    "unknown option '{a}'

{USAGE}"
                ))
            }
            _ if vm.is_some() => {
                return Err(format!(
                    "one VM at a time ('{a}' is one too many)

{USAGE}"
                ))
            }
            _ => vm = Some(VmRef::parse(a)),
        }
    }
    match vm {
        Some(vm) => Ok(Some(Args { server, screen, vm })),
        None => Err(format!(
            "which VM? (a VM id or name)

{USAGE}"
        )),
    }
}

/// Parses a `--server` value: `host:port`, or just a host or IP address (Avalon's
/// default port, 7400). IPv6 addresses with a port go in brackets: `[::1]:7400`.
///
/// Returns every address a host name resolves to, for `Connection::connect` to try in
/// turn: `localhost` is often `[::1]` first, while Avalon listens on `127.0.0.1` by
/// default. IPv4 addresses go first for that reason (Windows takes about two seconds
/// to give up on a refused `[::1]`); the resolver's order is kept otherwise.
pub fn parse_server(s: &str) -> Result<Vec<SocketAddr>, String> {
    let default_port = DEFAULT_AVALON_ADDR
        .parse::<SocketAddr>()
        .expect("the default address parses")
        .port();
    if let Ok(addr) = s.parse::<SocketAddr>() {
        return Ok(vec![addr]);
    }
    if let Ok(ip) = s.parse::<IpAddr>() {
        return Ok(vec![SocketAddr::new(ip, default_port)]);
    }
    let resolved = if s.is_empty() || s.contains(char::is_whitespace) {
        Err(io::ErrorKind::InvalidInput.into())
    } else if s.contains(':') {
        s.to_socket_addrs()
    } else {
        (s, default_port).to_socket_addrs()
    };
    match resolved.map(|addrs| addrs.collect::<Vec<_>>()) {
        Ok(mut addrs) if !addrs.is_empty() => {
            // A stable sort: false (IPv4) before true.
            addrs.sort_by_key(SocketAddr::is_ipv6);
            Ok(addrs)
        }
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
        let one = |s: &str| vec![s.parse::<SocketAddr>().unwrap()];
        assert_eq!(parse_server("127.0.0.1:7500").unwrap(), one("127.0.0.1:7500"));
        assert_eq!(parse_server("10.1.2.3").unwrap(), one("10.1.2.3:7400"));
        assert_eq!(parse_server("::1").unwrap(), one("[::1]:7400"));
        assert_eq!(parse_server("[::1]:9").unwrap(), one("[::1]:9"));
        // A host name keeps every address it resolves to, IPv4 first: IPv4 loopback
        // leads, whatever the resolver put first.
        for (s, port) in [("localhost", 7400), ("localhost:7600", 7600)] {
            let addrs = parse_server(s).unwrap();
            assert!(addrs.iter().all(|a| a.port() == port), "{s}: {addrs:?}");
            assert_eq!(
                addrs.len(),
                ("localhost", port).to_socket_addrs().unwrap().count()
            );
            assert_eq!(
                addrs[0],
                format!("127.0.0.1:{port}").parse().unwrap(),
                "{s}: {addrs:?}"
            );
        }
        for bad in ["", "two words", "127.0.0.1:port", "host:99999"] {
            assert!(parse_server(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn command_lines() {
        let parse = |s: &str| parse_args(&s.split_whitespace().map(str::to_string).collect::<Vec<_>>());
        let args = parse("web").unwrap().unwrap();
        assert_eq!(
            (args.server, args.screen, args.vm),
            (None, false, VmRef::Name("web".into()))
        );
        let args = parse("--screen --server 127.0.0.1:7500 7").unwrap().unwrap();
        assert_eq!(
            (args.server, args.screen, args.vm),
            (
                Some((
                    "127.0.0.1:7500".to_string(),
                    vec!["127.0.0.1:7500".parse().unwrap()]
                )),
                true,
                VmRef::Id(7)
            )
        );
        assert_eq!(parse("web --help"), Ok(None));
        for (bad, why) in [
            ("", "which VM?"),
            ("--screen", "which VM?"),
            ("--bogus web", "unknown option '--bogus'"),
            ("web --server", "--server needs a value"),
            ("--server 127.0.0.1:port web", "is not a server address"),
            ("one two", "one VM at a time"),
        ] {
            let e = parse(bad).unwrap_err();
            assert!(e.contains(why), "{bad:?}: {e}");
        }
    }

    /// `localhost` reaches a server listening on 127.0.0.1 only (Avalon's default),
    /// even where it resolves to `[::1]` first.
    #[test]
    fn localhost_reaches_an_ipv4_server() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let addrs = parse_server(&format!("localhost:{port}")).unwrap();
        let stream = TcpStream::connect(&addrs[..]).unwrap();
        assert_eq!(stream.peer_addr().unwrap(), listener.local_addr().unwrap());
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
