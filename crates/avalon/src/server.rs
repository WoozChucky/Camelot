//! The server: a TCP listener, the VM table, and two threads per client.
//!
//! Each client connection gets a reading thread, which does the handshake and then
//! answers the client's frames one by one, and a writing thread, which empties the
//! client's [`Outbox`] (its replies and the events of the VMs it is attached to). VMs
//! run on threads of their own ([`crate::vm`]). Nothing a client sends can stop the
//! server: every frame that is not an event gets exactly one reply, an `Error` when the
//! request fails (protocol doc, sections 4 and 5).

use crate::outbox::Outbox;
use crate::vm::Vm;
use crate::{lock, machine_config, Fail};
use avalon_proto::{
    kind_of, type_name, validate_name, Connection, Error, ErrorCode, Frame, HardwareProfile, Message,
    MessageKind, VmRef, VmSummary, DEFAULT_AVALON_ADDR,
};
use std::collections::{BTreeMap, HashMap};
use std::io::{self, Read, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, Shutdown, SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const MIB: u64 = 1024 * 1024;

/// Total RAM of all VMs when `--max-ram` is not given: 4 GiB.
pub const DEFAULT_MAX_RAM: u64 = 4096 * MIB;

/// What the server says in `Welcome`.
const SERVER_NAME: &str = concat!("avalon ", env!("CARGO_PKG_VERSION"));

/// How long a new connection has to send its `Hello`.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// How long shutdown waits for a client's last frames to go out before it closes the
/// socket anyway, and then for its threads to end.
const DRAIN_TIMEOUT: Duration = Duration::from_secs(1);

/// How often a client's reading thread, waiting for the client, checks whether the
/// connection is being ended.
const POLL: Duration = Duration::from_millis(100);

/// How the server runs.
#[derive(Debug, Clone)]
pub struct Config {
    /// Where to listen. Loopback by default: v1 of the protocol has no authentication.
    pub listen: SocketAddr,
    /// Total RAM of all VMs in bytes; creating a VM that would exceed it is refused
    /// with `OverBudget`. `None` is no limit.
    pub max_ram: Option<u64>,
    /// Log connections and VM state changes to stderr.
    pub log: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            listen: DEFAULT_AVALON_ADDR.parse().expect("the default address parses"),
            max_ram: Some(DEFAULT_MAX_RAM),
            log: true,
        }
    }
}

/// Parses a `--listen` value: `host:port`, or just a host or IP address (the default
/// port, 7400). IPv6 addresses with a port go in brackets: `[::1]:7400`.
pub fn parse_listen(s: &str) -> Result<SocketAddr, String> {
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
            "'{s}' is not an address to listen on (examples: 127.0.0.1:7400, 0.0.0.0, localhost:7500)"
        )),
    }
}

/// Parses a `--max-ram` value, a whole number of MiB (at least 1). Returns bytes.
pub fn parse_max_ram(s: &str) -> Result<u64, String> {
    let s = s.trim();
    match s.parse::<u64>() {
        Ok(mib) if mib > 0 && s.bytes().all(|b| b.is_ascii_digit()) => mib
            .checked_mul(MIB)
            .ok_or_else(|| format!("{mib} MiB is too large")),
        _ => Err(format!("'{s}' is not a whole number of MiB (at least 1)")),
    }
}

/// A bound server, ready to [`Server::run`].
pub struct Server {
    listener: TcpListener,
    shared: Arc<Shared>,
}

/// Stops a running server from another thread (a Ctrl-C handler, a test).
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
    max_ram: Option<u64>,
    log: bool,
    addr: SocketAddr,
    stopping: AtomicBool,
    table: Mutex<Table>,
    clients: Mutex<Clients>,
}

struct Table {
    vms: BTreeMap<u32, Arc<Vm>>,
    next_id: u32,
    /// The server is shutting down: no more VMs.
    closed: bool,
}

#[derive(Default)]
struct Clients {
    next_id: u64,
    live: HashMap<u64, Client>,
    threads: Vec<JoinHandle<()>>,
}

struct Client {
    /// To end the connection at shutdown, whatever its threads are doing.
    stream: TcpStream,
    /// Tells the reading thread to stop (see [`Socket`]).
    hang_up: Arc<AtomicBool>,
    /// Once the handshake is done.
    outbox: Option<Arc<Outbox>>,
}

impl Server {
    /// Binds the listening socket. Use port 0 for an ephemeral port
    /// ([`Server::local_addr`] says which).
    pub fn bind(config: &Config) -> io::Result<Server> {
        let listener = TcpListener::bind(config.listen)?;
        let addr = listener.local_addr()?;
        Ok(Server {
            listener,
            shared: Arc::new(Shared {
                max_ram: config.max_ram,
                log: config.log,
                addr,
                stopping: AtomicBool::new(false),
                table: Mutex::new(Table {
                    vms: BTreeMap::new(),
                    next_id: 1,
                    closed: false,
                }),
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

    /// Accepts and serves clients until [`ShutdownHandle::shutdown`]; then stops every
    /// VM, closes every connection (after sending what was queued for it) and returns.
    pub fn run(self) -> io::Result<()> {
        let shared = &self.shared;
        if shared.log {
            eprintln!("avalon: listening on {}", shared.addr);
            if !shared.addr.ip().is_loopback() {
                eprintln!(
                    "avalon: warning: {} is not a loopback address, and protocol v1 has no \
                     authentication: anyone who can reach it can control the VMs",
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
                        eprintln!("avalon: accept failed: {e}");
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
            .name("avalon".into())
            .spawn(move || self.run())?;
        Ok(ServerThread { addr, handle, thread })
    }
}

impl ShutdownHandle {
    /// Asks the server to shut down: [`Server::run`] stops accepting, stops the VMs,
    /// closes the connections and returns. Does not wait for that.
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
            deadline: Some(Instant::now() + HANDSHAKE_TIMEOUT),
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
                    outbox: None,
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

    /// A client's reading thread.
    fn serve(self: Arc<Self>, id: u64, socket: Socket) {
        let stream = &socket.stream;
        let peer = stream
            .peer_addr()
            .map_or_else(|_| "unknown peer".to_string(), |a| a.to_string());
        let _ = stream.set_nodelay(true);
        // Reads wake up this often to see whether the connection is being ended.
        let _ = stream.set_read_timeout(Some(POLL));
        let hang_up = Arc::clone(&socket.hang_up);
        let (Ok(cut), Ok(out)) = (stream.try_clone(), stream.try_clone()) else {
            let _ = stream.shutdown(Shutdown::Both);
            self.forget(id);
            return;
        };
        let mut conn = Connection::new(socket);
        let agent = match conn.server_handshake(SERVER_NAME) {
            Ok(agent) => agent,
            Err(e) => {
                if self.log {
                    eprintln!("avalon: {peer}: handshake failed: {e}");
                }
                let _ = cut.shutdown(Shutdown::Both);
                self.forget(id);
                return;
            }
        };
        conn.get_mut().deadline = None;
        let outbox = Arc::new(Outbox::new(id, move || {
            hang_up.store(true, Ordering::SeqCst);
            let _ = cut.shutdown(Shutdown::Both);
        }));
        match lock(&self.clients).live.get_mut(&id) {
            Some(client) => client.outbox = Some(Arc::clone(&outbox)),
            // Shutdown has begun.
            None => {
                let _ = out.shutdown(Shutdown::Both);
                return;
            }
        }
        let writer = {
            let outbox = Arc::clone(&outbox);
            std::thread::Builder::new()
                .name(format!("client-{id}-out"))
                .spawn(move || outbox.run_writer(out))
        };
        let Ok(writer) = writer else {
            let _ = conn.get_ref().stream.shutdown(Shutdown::Both);
            self.forget(id);
            return;
        };
        if self.log {
            eprintln!("avalon: {peer}: '{agent}' connected");
        }

        let session = Session {
            server: &self,
            outbox: &outbox,
        };
        loop {
            match conn.recv_frame() {
                Ok(frame) => session.frame(frame),
                Err(e) if e.is_fatal() => {
                    if let Error::FrameTooLarge(_) = e {
                        // The rest of the stream cannot be read; say why, then close.
                        outbox.reply(&Fail::new(e.code(), e.to_string()).into());
                    }
                    break;
                }
                // A frame too short for a type: consumed, and answered.
                Err(e) => outbox.reply(&Fail::new(e.code(), e.to_string()).into()),
            }
            if outbox.is_closed() {
                break;
            }
        }

        session.detach_everywhere();
        outbox.close();
        let _ = writer.join();
        self.forget(id);
        if self.log {
            eprintln!("avalon: {peer}: '{agent}' disconnected");
        }
    }

    fn forget(&self, id: u64) {
        lock(&self.clients).live.remove(&id);
    }

    /// Stops every VM, then closes every connection, and waits for their threads.
    fn shut_down(&self) {
        self.stopping.store(true, Ordering::SeqCst);
        let vms = {
            let mut table = lock(&self.table);
            table.closed = true;
            std::mem::take(&mut table.vms)
        };
        for vm in vms.values() {
            vm.shut_down();
        }
        let (live, threads) = {
            let mut clients = lock(&self.clients);
            (
                std::mem::take(&mut clients.live),
                std::mem::take(&mut clients.threads),
            )
        };
        // Last frames first (the VMs' final state changes), then hang up.
        for client in live.values() {
            if let Some(outbox) = &client.outbox {
                outbox.close();
            }
        }
        for client in live.values() {
            if let Some(outbox) = &client.outbox {
                outbox.wait_done(DRAIN_TIMEOUT);
            }
            client.hang_up.store(true, Ordering::SeqCst);
            let _ = client.stream.shutdown(Shutdown::Both);
        }
        // A writer can stay blocked on a client that stopped reading (shutting a socket
        // down does not wake a blocked send everywhere); such threads are left to end
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
            eprintln!("avalon: shut down ({} VMs stopped)", vms.len());
        }
    }

    fn find(&self, vm: &VmRef) -> Result<Arc<Vm>, Fail> {
        let table = lock(&self.table);
        let found = match vm {
            VmRef::Id(id) => table.vms.get(id),
            VmRef::Name(name) => table.vms.values().find(|v| v.name == *name),
        };
        match found {
            Some(vm) => Ok(Arc::clone(vm)),
            None => Err(no_such_vm(vm)),
        }
    }

    fn list(&self) -> Vec<VmSummary> {
        let vms: Vec<Arc<Vm>> = lock(&self.table).vms.values().cloned().collect();
        vms.iter().map(|vm| vm.summary()).collect()
    }

    /// `CreateVm`: checks the name, the profile, the RAM budget and the program, in
    /// that order, and adds the VM in state created.
    fn create(&self, name: String, profile: HardwareProfile, program: &[u8]) -> Result<u32, Fail> {
        validate_name(&name).map_err(|e| Fail::new(ErrorCode::InvalidName, e))?;
        let config = machine_config(&profile).map_err(|e| Fail::new(ErrorCode::InvalidProfile, e))?;
        // Held to the end, so two creates cannot both take the last of the budget or
        // the same name.
        let mut table = lock(&self.table);
        if table.closed {
            return Err(Fail::new(ErrorCode::Internal, "the server is shutting down"));
        }
        if table.vms.values().any(|vm| vm.name == name) {
            return Err(Fail::new(
                ErrorCode::AlreadyExists,
                format!("a VM named '{name}' already exists"),
            ));
        }
        let used: u64 = table.vms.values().map(|vm| vm.profile.ram_size).sum();
        if let Some(max) = self.max_ram {
            if used + profile.ram_size > max {
                return Err(Fail::new(
                    ErrorCode::OverBudget,
                    format!(
                        "the VM needs {} of RAM, and {} of the server's {} are in use",
                        show_size(profile.ram_size),
                        show_size(used),
                        show_size(max)
                    ),
                ));
            }
        }
        // Grail allocates RAM with no way to fail but aborting the process, so make
        // sure the host can give it first.
        if Vec::<u8>::new().try_reserve_exact(config.ram_size).is_err() {
            return Err(Fail::new(
                ErrorCode::OverBudget,
                format!("the host cannot allocate {} of RAM", show_size(profile.ram_size)),
            ));
        }
        let id = table.next_id;
        let next = id
            .checked_add(1)
            .ok_or_else(|| Fail::new(ErrorCode::Internal, "out of VM ids"))?;
        let vm = Vm::new(id, name, profile, config, program, self.log)?;
        if self.log {
            eprintln!(
                "avalon: VM {id} ('{}') created: {} RAM, {}",
                vm.name,
                show_size(vm.profile.ram_size),
                match vm.profile.clock_hz {
                    Some(hz) => format!("{hz} Hz real-time clock"),
                    None => "full speed".to_string(),
                }
            );
        }
        table.next_id = next;
        table.vms.insert(id, Arc::new(vm));
        Ok(id)
    }

    fn destroy(&self, vm: &VmRef) -> Result<(), Fail> {
        let vm = {
            let mut table = lock(&self.table);
            let id = match vm {
                VmRef::Id(id) => table.vms.contains_key(id).then_some(*id),
                VmRef::Name(name) => table.vms.values().find(|v| v.name == *name).map(|v| v.id),
            };
            match id.and_then(|id| table.vms.remove(&id)) {
                Some(vm) => vm,
                None => return Err(no_such_vm(vm)),
            }
        };
        vm.destroy();
        Ok(())
    }
}

/// A client's socket as its reading thread sees it. Shutting a socket down from
/// another thread does not wake a blocked read on every platform (on Windows it waits
/// for the peer), so reads time out every [`POLL`] and are retried, invisibly to the
/// frame reader, until the connection is told to hang up or the handshake deadline
/// passes.
struct Socket {
    stream: TcpStream,
    hang_up: Arc<AtomicBool>,
    /// Until the handshake is done: when the client must have sent its `Hello`.
    deadline: Option<Instant>,
}

impl Read for Socket {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            match self.stream.read(buf) {
                Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => {
                    if self.hang_up.load(Ordering::SeqCst) {
                        return Err(io::ErrorKind::ConnectionAborted.into());
                    }
                    if self.deadline.is_some_and(|d| Instant::now() >= d) {
                        return Err(io::ErrorKind::TimedOut.into());
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

fn no_such_vm(vm: &VmRef) -> Fail {
    Fail::new(
        ErrorCode::NoSuchVm,
        match vm {
            VmRef::Id(id) => format!("no VM with id {id}"),
            VmRef::Name(name) => format!("no VM named '{name}'"),
        },
    )
}

/// A size in bytes for messages: whole MiB when it is one, else bytes.
fn show_size(bytes: u64) -> String {
    if bytes.is_multiple_of(MIB) {
        format!("{} MiB", bytes / MIB)
    } else {
        format!("{bytes} bytes")
    }
}

/// One client's requests and events, on its reading thread.
struct Session<'a> {
    server: &'a Shared,
    outbox: &'a Arc<Outbox>,
}

impl Session<'_> {
    /// Answers one frame: a request gets exactly one reply (an `Error` if it fails or
    /// does not decode), and so does any other frame that is not an event. Events are
    /// never answered.
    fn frame(&self, frame: Frame) {
        let reply = match kind_of(frame.ty) {
            None => Some(Fail::new(
                ErrorCode::UnknownMessage,
                format!("unknown message type {:#06x}", frame.ty),
            )),
            Some(MessageKind::Reply) => Some(Fail::new(
                ErrorCode::Unexpected,
                format!("{} is a reply; clients send requests", type_name(frame.ty)),
            )),
            Some(MessageKind::Event) => {
                // An event that does not decode is dropped like any other bad event.
                if let Ok(msg) = frame.message() {
                    self.event(msg);
                }
                None
            }
            Some(MessageKind::Request) => match frame.message() {
                Ok(msg) => match self.request(msg) {
                    Ok(Some(reply)) => {
                        self.outbox.reply(&reply);
                        None
                    }
                    Ok(None) => None,
                    Err(fail) => Some(fail),
                },
                Err(e) => Some(Fail::new(e.code(), format!("{}: {e}", type_name(frame.ty)))),
            },
        };
        if let Some(fail) = reply {
            self.outbox.reply(&fail.into());
        }
    }

    /// Serves one request. `Ok(None)` when the reply has already been queued.
    fn request(&self, msg: Message) -> Result<Option<Message>, Fail> {
        let server = self.server;
        let reply = match msg {
            Message::Hello { .. } => {
                return Err(Fail::new(ErrorCode::Unexpected, "the session is already open"))
            }
            Message::Ping { token } => Message::Pong { token },
            Message::CreateVm {
                name,
                profile,
                program,
            } => Message::VmCreated {
                id: server.create(name, profile, &program)?,
            },
            Message::ListVms => Message::VmList { vms: server.list() },
            Message::GetVmInfo { vm } => Message::VmInfo(server.find(&vm)?.details()),
            Message::StartVm { vm } => {
                server.find(&vm)?.start()?;
                Message::Ok
            }
            Message::PauseVm { vm } => {
                server.find(&vm)?.pause()?;
                Message::Ok
            }
            Message::ResumeVm { vm } => {
                server.find(&vm)?.resume()?;
                Message::Ok
            }
            Message::StopVm { vm } => {
                server.find(&vm)?.stop()?;
                Message::Ok
            }
            Message::DestroyVm { vm } => {
                server.destroy(&vm)?;
                Message::Ok
            }
            Message::ConsoleAttach { vm } => {
                server.find(&vm)?.attach_console(self.outbox)?;
                return Ok(None);
            }
            Message::ConsoleDetach { vm } => {
                if !server.find(&VmRef::Id(vm))?.detach_console(self.outbox.id) {
                    return Err(Fail::new(
                        ErrorCode::InvalidState,
                        format!("not attached to the console of VM {vm}"),
                    ));
                }
                Message::Ok
            }
            other => return Err(unsupported(&other)),
        };
        Ok(Some(reply))
    }

    /// Serves one event: console input for a VM this client is attached to. Every
    /// other event is dropped: input for a VM it is not attached to, key input (it
    /// needs a screen attachment, which this version does not serve), and events only
    /// servers send.
    fn event(&self, msg: Message) {
        if let Message::ConsoleInput { vm, data } = msg {
            if let Ok(vm) = self.server.find(&VmRef::Id(vm)) {
                vm.console_input(self.outbox.id, &data);
            }
        }
    }

    /// The connection is ending: detach it from every console.
    fn detach_everywhere(&self) {
        let vms: Vec<Arc<Vm>> = lock(&self.server.table).vms.values().cloned().collect();
        for vm in vms {
            vm.detach_console(self.outbox.id);
        }
    }
}

/// The reply to a request from a group this server does not serve.
fn unsupported(msg: &Message) -> Fail {
    let why = match msg.type_code() >> 8 {
        0x03 => "this Avalon does not stream screens yet",
        0x04 | 0x05 => "Avalon runs VMs; block I/O and disk management are served by Lake",
        _ => "not served by Avalon",
    };
    Fail::new(ErrorCode::Unsupported, format!("{}: {why}", msg.name()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listen_addresses() {
        assert_eq!(
            parse_listen("127.0.0.1:7400").unwrap(),
            "127.0.0.1:7400".parse().unwrap()
        );
        assert_eq!(parse_listen("0.0.0.0").unwrap(), "0.0.0.0:7400".parse().unwrap());
        assert_eq!(parse_listen("::1").unwrap(), "[::1]:7400".parse().unwrap());
        assert_eq!(parse_listen("[::]:9000").unwrap(), "[::]:9000".parse().unwrap());
        assert_eq!(parse_listen("localhost:7500").unwrap().port(), 7500);
        assert!(parse_listen("localhost").unwrap().ip().is_loopback());
        for bad in ["", "127.0.0.1:", "127.0.0.1:99999", "host name with spaces", ":"] {
            assert!(parse_listen(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn max_ram_values() {
        assert_eq!(parse_max_ram("1").unwrap(), MIB);
        assert_eq!(parse_max_ram(" 4096 ").unwrap(), DEFAULT_MAX_RAM);
        for bad in ["0", "-1", "+5", "1.5", "64MiB", "", "99999999999999999999"] {
            assert!(parse_max_ram(bad).is_err(), "{bad}");
        }
        assert!(parse_max_ram(&(u64::MAX / MIB + 1).to_string()).is_err());
    }

    #[test]
    fn sizes() {
        assert_eq!(show_size(64 * MIB), "64 MiB");
        assert_eq!(show_size(4096), "4096 bytes");
    }

    #[test]
    fn default_config() {
        let config = Config::default();
        assert_eq!(config.listen.to_string(), DEFAULT_AVALON_ADDR);
        assert!(config.listen.ip().is_loopback());
        assert_eq!(config.max_ram, Some(DEFAULT_MAX_RAM));
    }
}
