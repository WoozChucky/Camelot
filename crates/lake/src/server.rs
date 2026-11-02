//! The server: a TCP listener and one thread per client.
//!
//! A client's thread does the handshake, then reads its frames one by one and writes
//! each reply before it reads the next frame. Lake sends no events, so there is
//! nothing else to write and no queue: a client that stops reading its replies only
//! blocks its own thread, until the write times out ([`WRITE_TIMEOUT`]) and the
//! connection is closed. Nothing a client sends can stop the server: every frame that
//! is not an event gets exactly one reply, an `Error` when the request fails (protocol
//! doc, sections 4 and 5).
//!
//! A connection's attachments ([`Image`]s, by handle) belong to its thread: when the
//! connection ends, however it ends, they are dropped and their images released.

use crate::store::{Image, Store, MAX_IMAGE_SECTORS};
use crate::{lock, Fail};
use avalon_proto::{
    default_port, kind_of, type_name, Connection, Error, ErrorCode, Frame, Message, MessageKind,
    DEFAULT_LAKE_ADDR,
};
use std::collections::{BTreeMap, HashMap};
use std::io::{self, Read, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, Shutdown, SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// What the server says in `Welcome`.
const SERVER_NAME: &str = concat!("lake ", env!("CARGO_PKG_VERSION"));

/// How long a new connection has to send its `Hello`, by default.
pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// Most images one connection may have attached at once (each holds a file open on
/// the host). More gets `OverBudget`.
pub const MAX_HANDLES: usize = 64;

/// How long a reply may wait for a client that does not read before the connection
/// is closed.
const WRITE_TIMEOUT: Duration = Duration::from_secs(30);

/// How long shutdown waits for the clients' threads to end.
const DRAIN_TIMEOUT: Duration = Duration::from_secs(1);

/// How often a client's thread, waiting for the client, checks whether the connection
/// is being ended.
const POLL: Duration = Duration::from_millis(100);

/// How the server runs.
#[derive(Debug, Clone)]
pub struct Config {
    /// Where to listen. Loopback by default: the protocol has no authentication.
    pub listen: SocketAddr,
    /// The images directory (created if it does not exist).
    pub dir: PathBuf,
    /// Largest image `DiskCreate` makes, in sectors; bigger gets `OverBudget`.
    pub max_sectors: u64,
    /// Log connections, creates and deletes to stderr.
    pub log: bool,
    /// How long a new connection has to complete its `Hello` before it is closed.
    pub handshake_timeout: Duration,
}

impl Config {
    /// Serves the images in `dir` on the default address.
    pub fn new(dir: impl Into<PathBuf>) -> Config {
        Config {
            listen: DEFAULT_LAKE_ADDR.parse().expect("the default address parses"),
            dir: dir.into(),
            max_sectors: MAX_IMAGE_SECTORS,
            log: true,
            handshake_timeout: HANDSHAKE_TIMEOUT,
        }
    }
}

/// Parses a `--listen` value: `host:port`, or just a host or IP address (the default
/// port, 7401). IPv6 addresses with a port go in brackets: `[::1]:7401`.
pub fn parse_listen(s: &str) -> Result<SocketAddr, String> {
    avalon_proto::parse_listen(s, default_port(DEFAULT_LAKE_ADDR))
}

/// A bound server, ready to [`Server::run`].
pub struct Server {
    listener: TcpListener,
    shared: Arc<Shared>,
}

/// Stops a running server from another thread (a test, a Ctrl-C handler).
#[derive(Clone)]
pub struct ShutdownHandle {
    shared: Arc<Shared>,
}

/// A server running on a thread of its own ([`Server::spawn`]).
pub struct ServerThread {
    addr: SocketAddr,
    handle: ShutdownHandle,
    thread: JoinHandle<io::Result<()>>,
}

struct Shared {
    store: Arc<Store>,
    log: bool,
    handshake_timeout: Duration,
    addr: SocketAddr,
    stopping: AtomicBool,
    clients: Mutex<Clients>,
}

#[derive(Default)]
struct Clients {
    next_id: u64,
    live: HashMap<u64, Client>,
    threads: Vec<JoinHandle<()>>,
}

struct Client {
    /// To end the connection at shutdown, whatever its thread is doing.
    stream: TcpStream,
    /// Tells the thread to stop (see [`Socket`]).
    hang_up: Arc<AtomicBool>,
}

impl Server {
    /// Opens the images directory (creating it if needed) and binds the listening
    /// socket. Use port 0 for an ephemeral port ([`Server::local_addr`] says which).
    pub fn bind(config: &Config) -> io::Result<Server> {
        let store = Store::open(&config.dir, config.max_sectors)
            .map_err(|e| io::Error::new(e.kind(), format!("{}: {e}", config.dir.display())))?;
        let listener = TcpListener::bind(config.listen)?;
        let addr = listener.local_addr()?;
        Ok(Server {
            listener,
            shared: Arc::new(Shared {
                store: Arc::new(store),
                log: config.log,
                handshake_timeout: config.handshake_timeout,
                addr,
                stopping: AtomicBool::new(false),
                clients: Mutex::default(),
            }),
        })
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.shared.addr
    }

    pub fn shutdown_handle(&self) -> ShutdownHandle {
        ShutdownHandle {
            shared: Arc::clone(&self.shared),
        }
    }

    /// Accepts and serves clients until [`ShutdownHandle::shutdown`]; then closes every
    /// connection (which detaches every image) and returns.
    pub fn run(self) -> io::Result<()> {
        let shared = &self.shared;
        if shared.log {
            eprintln!("lake: listening on {}", shared.addr);
            eprintln!("lake: images in {}", shared.store.dir().display());
            if !shared.addr.ip().is_loopback() {
                eprintln!(
                    "lake: warning: {} is not a loopback address, and the protocol has no \
                     authentication: anyone who can reach it can read, write and delete the disks",
                    shared.addr
                );
            }
        }
        for stream in self.listener.incoming() {
            if shared.stopping.load(Ordering::SeqCst) {
                break;
            }
            match stream {
                Ok(stream) => Arc::clone(shared).accept(stream),
                Err(e) => {
                    if shared.log {
                        eprintln!("lake: accept failed: {e}");
                    }
                    // Out of sockets or similar: give it a moment rather than spin.
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
        }
        shared.shut_down();
        Ok(())
    }

    /// Runs the server on a new thread.
    pub fn spawn(self) -> io::Result<ServerThread> {
        let addr = self.local_addr();
        let handle = self.shutdown_handle();
        let thread = std::thread::Builder::new()
            .name("lake".into())
            .spawn(move || self.run())?;
        Ok(ServerThread { addr, handle, thread })
    }
}

impl ShutdownHandle {
    /// Asks the server to shut down: [`Server::run`] stops accepting, closes the
    /// connections and returns. Does not wait for that.
    pub fn shutdown(&self) {
        if self.shared.stopping.swap(true, Ordering::SeqCst) {
            return;
        }
        // Wake the accept loop with a connection of our own.
        let mut addr = self.shared.addr;
        if addr.ip().is_unspecified() {
            addr.set_ip(match addr {
                SocketAddr::V4(_) => IpAddr::V4(Ipv4Addr::LOCALHOST),
                SocketAddr::V6(_) => IpAddr::V6(Ipv6Addr::LOCALHOST),
            });
        }
        let _ = TcpStream::connect_timeout(&addr, Duration::from_secs(1));
    }
}

impl ServerThread {
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn shutdown_handle(&self) -> ShutdownHandle {
        self.handle.clone()
    }

    /// For tests: how many connections the server is serving (handshake or not).
    #[doc(hidden)]
    pub fn debug_connections(&self) -> usize {
        lock(&self.handle.shared.clients).live.len()
    }

    /// Shuts the server down and waits until it has.
    pub fn stop(self) -> io::Result<()> {
        self.handle.shutdown();
        self.thread
            .join()
            .unwrap_or_else(|_| Err(io::Error::other("the server thread panicked")))
    }
}

impl Shared {
    fn accept(self: Arc<Self>, stream: TcpStream) {
        let Ok(handle) = stream.try_clone() else { return };
        let mut clients = lock(&self.clients);
        if self.stopping.load(Ordering::SeqCst) {
            return;
        }
        let id = clients.next_id;
        clients.next_id += 1;
        let hang_up = Arc::new(AtomicBool::new(false));
        let socket = Socket {
            stream,
            hang_up: Arc::clone(&hang_up),
            deadline: Some(Instant::now() + self.handshake_timeout),
        };
        let shared = Arc::clone(&self);
        let spawned = std::thread::Builder::new()
            .name(format!("client-{id}"))
            .spawn(move || shared.serve(id, socket));
        if let Ok(thread) = spawned {
            clients.live.insert(
                id,
                Client {
                    stream: handle,
                    hang_up,
                },
            );
            // Collect the threads of clients that have gone.
            let (done, running) = std::mem::take(&mut clients.threads)
                .into_iter()
                .partition(|t: &JoinHandle<()>| t.is_finished());
            clients.threads = running;
            clients.threads.push(thread);
            drop(clients);
            for t in done {
                let _ = t.join();
            }
        }
    }

    /// A client's thread.
    fn serve(self: Arc<Self>, id: u64, socket: Socket) {
        let stream = &socket.stream;
        let peer = stream
            .peer_addr()
            .map_or_else(|_| "unknown peer".to_string(), |a| a.to_string());
        let _ = stream.set_nodelay(true);
        let _ = stream.set_read_timeout(Some(POLL));
        let _ = stream.set_write_timeout(Some(WRITE_TIMEOUT));
        let mut conn = Connection::new(socket);
        let agent = match conn.server_handshake(SERVER_NAME) {
            Ok(agent) => agent,
            Err(e) => {
                if self.log {
                    eprintln!("lake: {peer}: handshake failed: {e}");
                }
                let _ = conn.get_ref().stream.shutdown(Shutdown::Both);
                self.forget(id);
                return;
            }
        };
        conn.get_mut().deadline = None;
        let _ = conn.get_ref().stream.set_read_timeout(Some(POLL));
        if self.log {
            eprintln!("lake: {peer}: '{agent}' connected");
        }

        let mut session = Session {
            server: &self,
            handles: BTreeMap::new(),
            next_handle: 1,
        };
        loop {
            let reply = match conn.recv_frame() {
                Ok(frame) => session.frame(frame),
                Err(e) if e.is_fatal() => {
                    if let Error::FrameTooLarge(_) = e {
                        // The rest of the stream cannot be read; say why, then close.
                        let _ = conn.send(&Fail::new(e.code(), e.to_string()).into());
                    }
                    break;
                }
                // A frame too short for a type: consumed, and answered.
                Err(e) => Some(Fail::new(e.code(), e.to_string()).into()),
            };
            if let Some(reply) = reply {
                if conn.send(&reply).is_err() {
                    break;
                }
            }
        }

        // Detach everything this connection had attached.
        let attached = session.handles.len();
        drop(session);
        let _ = conn.get_ref().stream.shutdown(Shutdown::Both);
        self.forget(id);
        if self.log {
            match attached {
                0 => eprintln!("lake: {peer}: '{agent}' disconnected"),
                n => eprintln!("lake: {peer}: '{agent}' disconnected, detaching {n} disk(s)"),
            }
        }
    }

    fn forget(&self, id: u64) {
        lock(&self.clients).live.remove(&id);
    }

    /// Closes every connection and waits for their threads.
    fn shut_down(&self) {
        self.stopping.store(true, Ordering::SeqCst);
        let (live, threads) = {
            let mut clients = lock(&self.clients);
            (
                std::mem::take(&mut clients.live),
                std::mem::take(&mut clients.threads),
            )
        };
        for client in live.values() {
            client.hang_up.store(true, Ordering::SeqCst);
            let _ = client.stream.shutdown(Shutdown::Both);
        }
        // A thread blocked writing to a client that stopped reading is not woken by
        // the shutdown everywhere; it ends by itself once the write times out, or
        // with the process.
        let deadline = Instant::now() + DRAIN_TIMEOUT;
        for thread in threads {
            while !thread.is_finished() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(5));
            }
            if thread.is_finished() {
                let _ = thread.join();
            }
        }
        if self.log {
            eprintln!("lake: shut down");
        }
    }
}

/// A client's socket as its thread sees it. Shutting a socket down from another
/// thread does not wake a blocked read on every platform (on Windows it waits for the
/// peer), so reads time out every [`POLL`] and are retried, invisibly to the frame
/// reader, until the connection is told to hang up. Until the handshake is done, every
/// read also fails once the deadline has passed, data or not: a client that trickles
/// its `Hello` in byte by byte gets no more time than a silent one.
struct Socket {
    stream: TcpStream,
    hang_up: Arc<AtomicBool>,
    /// Until the handshake is done: when the client must have sent its `Hello`.
    deadline: Option<Instant>,
}

impl Read for Socket {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            if let Some(deadline) = self.deadline {
                let left = deadline.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    return Err(io::ErrorKind::TimedOut.into());
                }
                self.stream.set_read_timeout(Some(left.min(POLL)))?;
            }
            match self.stream.read(buf) {
                Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => {
                    if self.hang_up.load(Ordering::SeqCst) {
                        return Err(io::ErrorKind::ConnectionAborted.into());
                    }
                }
                result => return result,
            }
        }
    }
}

impl Write for Socket {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.stream.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.stream.flush()
    }
}

/// One client's requests, on its thread, and the images it has attached.
struct Session<'a> {
    server: &'a Shared,
    handles: BTreeMap<u32, Image>,
    /// Handles are never reused on a connection, so a stale one cannot reach another
    /// attachment.
    next_handle: u32,
}

impl Session<'_> {
    /// The reply to one frame: a request gets exactly one (an `Error` if it fails or
    /// does not decode), and so does any other frame that is not an event. Events are
    /// never answered; Lake has no use for any, so they are dropped.
    fn frame(&mut self, frame: Frame) -> Option<Message> {
        let fail = match kind_of(frame.ty) {
            None => Fail::new(
                ErrorCode::UnknownMessage,
                format!("unknown message type {:#06x}", frame.ty),
            ),
            Some(MessageKind::Reply) => Fail::new(
                ErrorCode::Unexpected,
                format!("{} is a reply; clients send requests", type_name(frame.ty)),
            ),
            Some(MessageKind::Event) => return None,
            Some(MessageKind::Request) => match frame.message() {
                Ok(msg) => match self.request(msg) {
                    Ok(reply) => return Some(reply),
                    Err(fail) => fail,
                },
                Err(e) => Fail::new(e.code(), format!("{}: {e}", type_name(frame.ty))),
            },
        };
        Some(fail.into())
    }

    fn image(&mut self, handle: u32) -> Result<&mut Image, Fail> {
        self.handles.get_mut(&handle).ok_or_else(|| {
            Fail::new(
                ErrorCode::NoSuchHandle,
                format!("no open block handle {handle} on this connection"),
            )
        })
    }

    fn request(&mut self, msg: Message) -> Result<Message, Fail> {
        let server = self.server;
        let store = &server.store;
        Ok(match msg {
            Message::Hello { .. } => {
                return Err(Fail::new(ErrorCode::Unexpected, "the session is already open"))
            }
            Message::Ping { token } => Message::Pong { token },

            Message::DiskCreate {
                name,
                sectors,
                source: None,
            } => {
                store.create(&name, sectors)?;
                if server.log {
                    eprintln!("lake: disk '{name}' created: {sectors} sectors");
                }
                Message::Ok
            }
            Message::DiskCreate {
                name,
                sectors,
                source: Some(source),
            } => {
                let sectors = store.clone_from(&name, &source, sectors)?;
                if server.log {
                    eprintln!("lake: disk '{name}' created from '{source}': {sectors} sectors");
                }
                Message::Ok
            }
            Message::ListDisks => Message::DiskList { disks: store.list()? },
            Message::DiskDelete { name } => {
                store.delete(&name)?;
                if server.log {
                    eprintln!("lake: disk '{name}' deleted");
                }
                Message::Ok
            }

            Message::BlockOpen { name, read_only } => {
                if self.handles.len() >= MAX_HANDLES {
                    return Err(Fail::new(
                        ErrorCode::OverBudget,
                        format!("a connection may have at most {MAX_HANDLES} disks attached"),
                    ));
                }
                let handle = self.next_handle;
                let next = handle.checked_add(1).ok_or_else(|| {
                    Fail::new(ErrorCode::Internal, "out of block handles on this connection")
                })?;
                let image = store.attach(&name, read_only)?;
                let sectors = image.sectors();
                self.next_handle = next;
                self.handles.insert(handle, image);
                Message::BlockOpened {
                    handle,
                    sectors,
                    read_only,
                }
            }
            Message::BlockRead {
                handle,
                sector,
                count,
            } => Message::BlockData {
                handle,
                sector,
                data: self.image(handle)?.read(sector, count)?,
            },
            Message::BlockWrite { handle, sector, data } => {
                self.image(handle)?.write(sector, &data)?;
                Message::Ok
            }
            Message::BlockFlush { handle } => {
                self.image(handle)?.flush()?;
                Message::Ok
            }
            Message::BlockSize { handle } => Message::BlockCapacity {
                handle,
                sectors: self.image(handle)?.sectors(),
            },
            Message::BlockClose { handle } => {
                self.image(handle)?;
                self.handles.remove(&handle);
                Message::Ok
            }

            other => return Err(unsupported(&other)),
        })
    }
}

/// The reply to a request from a group this server does not serve.
fn unsupported(msg: &Message) -> Fail {
    Fail::new(
        ErrorCode::Unsupported,
        format!(
            "{}: Lake serves disks; VMs, consoles, screens and switches are Avalon's",
            msg.name()
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listen_addresses_default_to_port_7401() {
        assert_eq!(parse_listen("0.0.0.0").unwrap(), "0.0.0.0:7401".parse().unwrap());
        assert_eq!(
            parse_listen("127.0.0.1:9000").unwrap(),
            "127.0.0.1:9000".parse().unwrap()
        );
        assert!(parse_listen("").is_err());
    }

    #[test]
    fn default_config() {
        let config = Config::new("images");
        assert_eq!(config.listen.to_string(), DEFAULT_LAKE_ADDR);
        assert!(config.listen.ip().is_loopback());
        assert_eq!(config.max_sectors, MAX_IMAGE_SECTORS);
        assert_eq!(config.dir, PathBuf::from("images"));
    }
}
