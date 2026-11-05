//! The server: a TCP listener, the VM table, and two threads per client.
//!
//! Each client connection gets a reading thread, which does the handshake and then
//! answers the client's frames one by one, and a writing thread, which empties the
//! client's [`Outbox`] (its replies and the events of the VMs it is attached to). VMs
//! run on threads of their own ([`crate::vm`]). Nothing a client sends can stop the
//! server: every frame that is not an event gets exactly one reply, an `Error` when the
//! request fails (protocol doc, sections 4 and 5).
//!
//! The server also owns the virtual switches its VMs' network cards plug into
//! ([`crate::switch`]), by name.

use crate::outbox::Outbox;
use crate::profile::{card_configs, CardConfig};
use crate::switch::{Card, Switch, MAX_SWITCHES};
use crate::vm::Vm;
use crate::{lock, machine_config, Fail};
use avalon_proto::{
    close_gracefully, default_port, kind_of, type_name, validate_name, Connection, Error, ErrorCode, Frame,
    HardwareProfile, Message, MessageKind, NatConfig, PortOwner, SwitchInfo, VmRef, VmSummary,
    DEFAULT_AVALON_ADDR,
};
use std::collections::{BTreeMap, HashMap};
use std::io::{self, Read, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const MIB: u64 = 1024 * 1024;

/// Total RAM of all VMs when `--max-ram` is not given: 4 GiB.
pub const DEFAULT_MAX_RAM: u64 = 4096 * MIB;

/// What the server says in `Welcome`.
const SERVER_NAME: &str = concat!("avalon ", env!("CARGO_PKG_VERSION"));

/// How long a new connection has to send its `Hello`, by default.
pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// How long shutdown waits for a client's last frames to go out before it closes the
/// socket anyway, and then for its threads to end.
const DRAIN_TIMEOUT: Duration = Duration::from_secs(1);

/// How often a client's reading thread, waiting for the client, checks whether the
/// connection is being ended.
const POLL: Duration = Duration::from_millis(100);

/// How the server runs.
#[derive(Debug, Clone)]
pub struct Config {
    /// Where to listen. Loopback by default: the protocol has no authentication.
    pub listen: SocketAddr,
    /// Total RAM of all VMs in bytes; creating a VM that would exceed it is refused
    /// with `OverBudget`. `None` is no limit.
    pub max_ram: Option<u64>,
    /// Log connections and VM state changes to stderr.
    pub log: bool,
    /// How long a new connection has to complete its `Hello` (all of it, however
    /// slowly it trickles in) before it is closed.
    pub handshake_timeout: Duration,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            listen: DEFAULT_AVALON_ADDR.parse().expect("the default address parses"),
            max_ram: Some(DEFAULT_MAX_RAM),
            log: true,
            handshake_timeout: HANDSHAKE_TIMEOUT,
        }
    }
}

/// Parses a `--listen` value: `host:port`, or just a host or IP address (the default
/// port, 7400). IPv6 addresses with a port go in brackets: `[::1]:7400`.
pub fn parse_listen(s: &str) -> Result<SocketAddr, String> {
    avalon_proto::parse_listen(s, default_port(DEFAULT_AVALON_ADDR))
}

/// Parses a `--max-ram` value: MiB, or a number with a unit, `M`/`MiB`, `G`/`GiB` or
/// `T`/`TiB` (`4096`, `64G`, `2T`), at least 1 MiB: `--ram`'s syntax without its 1 TiB
/// limit (Grail's `parse_size`). Returns bytes.
pub fn parse_max_ram(s: &str) -> Result<u64, String> {
    grail::parse_size(s)
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
    handshake_timeout: Duration,
    addr: SocketAddr,
    stopping: AtomicBool,
    table: Mutex<Table>,
    /// The virtual switches, by name. Taken after `table` when both are needed.
    switches: Mutex<BTreeMap<String, Switch>>,
    clients: Mutex<Clients>,
}

struct Table {
    vms: BTreeMap<u32, Arc<Vm>>,
    /// VMs being built (outside the lock): their names and RAM are taken already.
    pending: Vec<(String, u64)>,
    next_id: u32,
    /// The server is shutting down: no more VMs.
    closed: bool,
}

#[derive(Default)]
struct Clients {
    next_id: u64,
    live: HashMap<u64, Client>,
    threads: Vec<JoinHandle<()>>,
    /// Connections whose threads have finished with them, their sockets closed: only
    /// ever grows (for tests, [`ServerThread::debug_connections_finished`]).
    finished: u64,
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
                handshake_timeout: config.handshake_timeout,
                addr,
                stopping: AtomicBool::new(false),
                table: Mutex::new(Table {
                    vms: BTreeMap::new(),
                    pending: Vec::new(),
                    next_id: 1,
                    closed: false,
                }),
                switches: Mutex::default(),
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
                    "avalon: warning: {} is not a loopback address, and the protocol has no \
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

    /// For tests: how VM `id`'s thread is doing.
    #[doc(hidden)]
    pub fn debug_vm(&self, id: u32) -> Option<VmDebug> {
        let vm = lock(&self.handle.shared.table).vms.get(&id).cloned()?;
        let (keys, pointer) = vm.queued_input();
        Some(VmDebug {
            slices: vm.slices(),
            bells: vm.bells(),
            consoles: vm.console_count(),
            viewers: vm.viewer_count(),
            debugged: vm.debugged(),
            behind: vm.behind_count(),
            keys,
            pointer,
        })
    }

    /// For tests: how many connections the server is serving (handshake or not).
    #[doc(hidden)]
    pub fn debug_connections(&self) -> usize {
        lock(&self.handle.shared.clients).live.len()
    }

    /// For tests: how many connections the server has finished with and closed, ever.
    /// Unlike [`ServerThread::debug_connections`], which is also 0 before the server has
    /// accepted a connection, this says that a connection has been closed.
    #[doc(hidden)]
    pub fn debug_connections_finished(&self) -> u64 {
        lock(&self.handle.shared.clients).finished
    }

    /// Shuts the server down and waits until it has.
    pub fn stop(self) -> io::Result<()> {
        self.handle.shutdown();
        self.thread
            .join()
            .unwrap_or_else(|_| Err(io::Error::other("the server thread panicked")))
    }
}

/// What [`ServerThread::debug_vm`] reports.
#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VmDebug {
    /// Calls of Grail's `run_for` so far, empty ones included.
    pub slices: u64,
    /// Times a frame for one of its network cards rang its bell (woke it).
    pub bells: u64,
    /// Clients attached to the console.
    pub consoles: usize,
    /// Clients attached to the screen.
    pub viewers: usize,
    /// It has a debugger.
    pub debugged: bool,
    /// Viewers held back for being over their backlog (`screen_backlog`), owed a full
    /// frame and the cursor once they catch up.
    pub behind: usize,
    /// Keys waiting for the machine (at most [`crate::MAX_INPUT_QUEUE`]).
    pub keys: usize,
    /// Pointer changes waiting for the machine (at most [`crate::MAX_INPUT_QUEUE`]).
    pub pointer: usize,
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
            .spawn(move || {
                Arc::clone(&shared).serve(id, socket);
                // The socket is closed now: `serve`'s handles are gone, and so is the
                // one in `live`, which it removed (unless its writer was left blocked,
                // in which case the writer's handle closes it when the writer ends).
                lock(&shared.clients).finished += 1;
            });
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
        // Closing waits for the client a moment (`close_gracefully`), unless the
        // connection is being hung up (server shutdown or cut-off).
        let stopped = Arc::clone(&socket.hang_up);
        let stopping = || stopped.load(Ordering::SeqCst);
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
                // The Welcome or Error just sent must not be lost to a reset over
                // what the client sent after its first frame.
                close_gracefully(&cut, stopping);
                self.forget(id);
                return;
            }
        };
        conn.get_mut().deadline = None;
        let _ = conn.get_ref().stream.set_read_timeout(Some(POLL));
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
        // A writer blocked on a client that stopped reading (one that was cut off, say)
        // is not woken by its socket's shutdown everywhere: on Windows it waits for the
        // peer. Give it a moment; past that it is left to end by itself, once the
        // client reads or goes, and the connection is forgotten now.
        if outbox.wait_done(DRAIN_TIMEOUT) {
            let _ = writer.join();
            // Everything queued has been written. After an oversize frame its body is
            // still unread behind the Error: close so that the Error is not lost to a
            // reset. (A connection cut off, or ended by a failed write, is shut down
            // already, and this returns at once.)
            close_gracefully(&conn.get_ref().stream, stopping);
        }
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
        // Ends the switches' captures, with every frame in the files.
        let switches = std::mem::take(&mut *lock(&self.switches));
        for switch in switches.values() {
            switch.close();
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

    /// `CreateVm`: checks the name, the profile (its cards' switches included), the RAM
    /// budget and the program, in that order, and adds the VM in state created, its
    /// cards plugged into their switches.
    ///
    /// The name and the RAM are reserved under the table's lock, so two creates cannot
    /// both take the last of the budget or the same name; the machine is built without
    /// it, so a slow build (a disk image on a slow path, gigabytes of RAM) holds up no
    /// other request.
    fn create(&self, name: String, profile: HardwareProfile, program: &[u8]) -> Result<u32, Fail> {
        validate_name(&name).map_err(|e| Fail::new(ErrorCode::InvalidName, e))?;
        let config =
            machine_config(&profile, program).map_err(|e| Fail::new(ErrorCode::InvalidProfile, e))?;
        let cards = card_configs(&name, &profile).map_err(|e| Fail::new(ErrorCode::InvalidProfile, e))?;
        // Checked again when the cards are plugged in, in case a switch is deleted
        // meanwhile; this saves building a machine that cannot be plugged in.
        for card in &cards {
            if let Some(switch) = &card.switch {
                self.switch(switch)?;
            }
        }
        self.reserve(&name, profile.ram_size)?;
        let built = self.build(name.clone(), profile, config, &cards, program);
        let mut table = lock(&self.table);
        if let Some(i) = table.pending.iter().position(|(n, _)| *n == name) {
            table.pending.swap_remove(i);
        }
        let mut vm = built?;
        if table.closed {
            return Err(shutting_down());
        }
        let id = table.next_id;
        let next = id
            .checked_add(1)
            .ok_or_else(|| Fail::new(ErrorCode::Internal, "out of VM ids"))?;
        vm.id = id;
        let vm = Arc::new(vm);
        // A frame for one of the cards wakes the VM.
        vm.ring_on_frames();
        // Plugged in under the table's lock, so the VM is never listed with its
        // cards half plugged in. If one cannot be, the VM is not created.
        if let Err(fail) = self.plug_cards(&vm, &cards) {
            drop(table);
            for card in vm.cards() {
                card.unplug();
            }
            // Its machine (disks included) goes with it, outside the lock.
            drop(vm);
            return Err(fail);
        }
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
        table.vms.insert(id, vm);
        Ok(id)
    }

    /// Plugs the cards of a new VM into the switches `cards` name.
    fn plug_cards(&self, vm: &Arc<Vm>, cards: &[CardConfig]) -> Result<(), Fail> {
        for (card, config) in vm.cards().iter().zip(cards) {
            if let Some(switch) = &config.switch {
                let owner = PortOwner::Card {
                    vm: vm.id,
                    card: card.index,
                };
                card.plug(owner, &self.switch(switch)?)?;
            }
        }
        Ok(())
    }

    /// Takes `name` and `ram` bytes of the budget for a VM about to be built.
    fn reserve(&self, name: &str, ram: u64) -> Result<(), Fail> {
        let mut table = lock(&self.table);
        if table.closed {
            return Err(shutting_down());
        }
        let taken =
            table.vms.values().any(|vm| vm.name == name) || table.pending.iter().any(|(n, _)| n == name);
        if taken {
            return Err(Fail::new(
                ErrorCode::AlreadyExists,
                format!("a VM named '{name}' already exists"),
            ));
        }
        let used: u64 = table.vms.values().map(|vm| vm.profile.ram_size).sum::<u64>()
            + table.pending.iter().map(|(_, ram)| ram).sum::<u64>();
        if let Some(max) = self.max_ram {
            if used.saturating_add(ram) > max {
                return Err(Fail::new(
                    ErrorCode::OverBudget,
                    format!(
                        "the VM needs {} of RAM, and {} of the server's {} are in use",
                        show_size(ram),
                        show_size(used),
                        show_size(max)
                    ),
                ));
            }
        }
        table.pending.push((name.to_string(), ram));
        Ok(())
    }

    /// Builds a reserved VM, without the table's lock.
    fn build(
        &self,
        name: String,
        profile: HardwareProfile,
        config: grail::MachineConfig,
        cards: &[CardConfig],
        program: &[u8],
    ) -> Result<Vm, Fail> {
        // Grail backs RAM lazily (spec section 7, "RAM"): the size is reserved in the
        // host's address space, not allocated, and a size the host cannot reserve is
        // refused. Reserved here, and let go at once, so that refusal is the budget's
        // answer, OverBudget, rather than a bad profile when the machine is built.
        // Committing the whole size first (as an allocation would) would undo the lazy
        // backing, and refuse any VM bigger than the host's memory.
        if let Err(e) = grail::ram::Ram::new(config.ram_size) {
            return Err(Fail::new(
                ErrorCode::OverBudget,
                format!("{} of RAM: {e}", show_size(profile.ram_size)),
            ));
        }
        let cards = cards
            .iter()
            .enumerate()
            .map(|(i, c)| Card::new(i as u8, c.mac))
            .collect();
        Vm::new(name, profile, config, cards, program, self.log)
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

    // ---- Virtual network ----

    /// The switch named `name`.
    fn switch(&self, name: &str) -> Result<Switch, Fail> {
        lock(&self.switches)
            .get(name)
            .cloned()
            .ok_or_else(|| Fail::new(ErrorCode::NoSuchSwitch, format!("no switch named '{name}'")))
    }

    /// `CreateSwitch`: a new switch with no cards plugged in, and its NAT gateway if
    /// `nat` asks for one.
    fn create_switch(&self, name: String, nat: Option<NatConfig>) -> Result<(), Fail> {
        validate_name(&name).map_err(|e| Fail::new(ErrorCode::InvalidName, e))?;
        if self.stopping.load(Ordering::SeqCst) {
            return Err(shutting_down());
        }
        let room = |switches: &BTreeMap<String, Switch>| {
            if switches.contains_key(&name) {
                return Err(Fail::new(
                    ErrorCode::AlreadyExists,
                    format!("a switch named '{name}' already exists"),
                ));
            }
            if switches.len() >= MAX_SWITCHES {
                return Err(Fail::new(
                    ErrorCode::OverBudget,
                    format!("this server has {MAX_SWITCHES} switches, its most"),
                ));
            }
            Ok(())
        };
        // Checked before the gateway binds its ports, and again once it has (without
        // the table's lock, so other requests do not wait for it).
        room(&lock(&self.switches))?;
        let switch = Switch::new(&name);
        if let Some(nat) = &nat {
            switch.set_nat(Some(nat))?;
        }
        let mut switches = lock(&self.switches);
        // A shutdown that began meanwhile has closed the switches it took from the
        // table; this one, inserted after, would never be closed (its gateway's thread
        // and host ports would outlive the server). `stopping` is set before the
        // shutdown takes the table's lock, so seeing it false here, under that lock,
        // means the shutdown will find this switch.
        let refused = if self.stopping.load(Ordering::SeqCst) {
            Err(shutting_down())
        } else {
            room(&switches)
        };
        if let Err(fail) = refused {
            drop(switches);
            switch.close();
            return Err(fail);
        }
        if self.log {
            match &nat {
                None => eprintln!("avalon: switch '{name}' created"),
                Some(_) => eprintln!(
                    "avalon: switch '{name}' created with a NAT gateway{}",
                    forwards_text(&switch)
                ),
            }
        }
        switches.insert(name, switch);
        Ok(())
    }

    /// `SetSwitchNat`.
    fn set_switch_nat(&self, name: &str, nat: Option<NatConfig>) -> Result<(), Fail> {
        let switch = self.switch(name)?;
        switch.set_nat(nat.as_ref())?;
        if self.log {
            match nat {
                Some(_) => eprintln!(
                    "avalon: switch '{name}' has a NAT gateway{}",
                    forwards_text(&switch)
                ),
                None => eprintln!("avalon: switch '{name}' has no NAT gateway any more"),
            }
        }
        Ok(())
    }

    /// `DeleteSwitch`: unplugs every card from the switch (their links go down) and
    /// ends its capture.
    fn delete_switch(&self, name: &str) -> Result<(), Fail> {
        let switch = lock(&self.switches).remove(name);
        let Some(switch) = switch else {
            return Err(Fail::new(
                ErrorCode::NoSuchSwitch,
                format!("no switch named '{name}'"),
            ));
        };
        switch.close();
        if self.log {
            eprintln!("avalon: switch '{name}' deleted");
        }
        Ok(())
    }

    /// `ListSwitches`, in name order.
    fn list_switches(&self) -> Vec<SwitchInfo> {
        let switches: Vec<Switch> = lock(&self.switches).values().cloned().collect();
        switches.iter().map(Switch::info).collect()
    }

    /// `AttachCard`.
    fn attach_card(&self, vm: &VmRef, card: u8, switch: &str) -> Result<(), Fail> {
        let vm = self.find(vm)?;
        let card = vm.card(card)?;
        let switch = self.switch(switch)?;
        card.plug(
            PortOwner::Card {
                vm: vm.id,
                card: card.index,
            },
            &switch,
        )
    }

    /// `DetachCard`.
    fn detach_card(&self, vm: &VmRef, card: u8) -> Result<(), Fail> {
        let vm = self.find(vm)?;
        let card = vm.card(card)?;
        if card.unplug() {
            Ok(())
        } else {
            Err(Fail::new(
                ErrorCode::InvalidState,
                format!(
                    "card {} of VM {} ('{}') is not plugged into a switch",
                    card.index, vm.id, vm.name
                ),
            ))
        }
    }

    /// `CaptureSwitch`.
    fn capture_switch(&self, switch: &str, path: Option<&str>) -> Result<(), Fail> {
        let switch = self.switch(switch)?;
        match path {
            Some(path) => switch.start_capture(path),
            None => switch.stop_capture(),
        }
    }
}

/// The ports a switch's gateway forwards, for the log: `, forwarding tcp:...-...`.
fn forwards_text(switch: &Switch) -> String {
    let rules: Vec<String> = switch
        .info()
        .nat
        .map(|nat| nat.hostfwd.iter().map(|r| r.to_string()).collect())
        .unwrap_or_default();
    if rules.is_empty() {
        String::new()
    } else {
        format!(", forwarding {}", rules.join(", "))
    }
}

/// A client's socket as its reading thread sees it. Shutting a socket down from
/// another thread does not wake a blocked read on every platform (on Windows it waits
/// for the peer), so reads time out every [`POLL`] and are retried, invisibly to the
/// frame reader, until the connection is told to hang up. Until the handshake is done,
/// every read also fails once the deadline has passed, data or not: a client that
/// trickles its `Hello` in byte by byte gets no more time than a silent one.
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

fn shutting_down() -> Fail {
    Fail::new(ErrorCode::Internal, "the server is shutting down")
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
            Message::ScreenAttach { vm } => {
                server.find(&vm)?.attach_screen(self.outbox)?;
                return Ok(None);
            }
            Message::ScreenDetach { vm } => {
                if !server.find(&VmRef::Id(vm))?.detach_screen(self.outbox.id) {
                    return Err(Fail::new(
                        ErrorCode::InvalidState,
                        format!("not attached to the screen of VM {vm}"),
                    ));
                }
                Message::Ok
            }
            Message::DebugAttach { vm } => {
                let vm = server.find(&vm)?;
                vm.attach_debugger(self.outbox)?;
                Message::DebugAttached { vm: vm.id }
            }
            Message::DebugDetach { vm } => {
                if !server.find(&VmRef::Id(vm))?.detach_debugger(self.outbox.id) {
                    return Err(Fail::new(
                        ErrorCode::InvalidState,
                        format!("not the debugger of VM {vm}"),
                    ));
                }
                Message::Ok
            }
            Message::CreateSwitch { name, nat } => {
                server.create_switch(name, nat)?;
                Message::Ok
            }
            Message::SetSwitchNat { switch, nat } => {
                server.set_switch_nat(&switch, nat)?;
                Message::Ok
            }
            Message::DeleteSwitch { name } => {
                server.delete_switch(&name)?;
                Message::Ok
            }
            Message::ListSwitches => Message::SwitchList {
                switches: server.list_switches(),
            },
            Message::AttachCard { vm, card, switch } => {
                server.attach_card(&vm, card, &switch)?;
                Message::Ok
            }
            Message::DetachCard { vm, card } => {
                server.detach_card(&vm, card)?;
                Message::Ok
            }
            Message::CaptureSwitch { switch, path } => {
                server.capture_switch(&switch, path.as_deref())?;
                Message::Ok
            }
            other => return Err(unsupported(&other)),
        };
        Ok(Some(reply))
    }

    /// Serves one event: console input for a VM whose console this client is attached
    /// to, keys and pointer changes for a VM whose screen it is attached to, the GDB
    /// remote protocol's bytes for a VM whose debugger it is. Every other
    /// event is dropped: input for a VM it is not attached to (or that does not exist),
    /// and events only servers send.
    fn event(&self, msg: Message) {
        let id = self.outbox.id;
        let find = |vm: u32| self.server.find(&VmRef::Id(vm)).ok();
        match msg {
            Message::ConsoleInput { vm, data } => {
                if let Some(vm) = find(vm) {
                    vm.console_input(id, &data);
                }
            }
            Message::KeyInput { vm, key } => {
                if let Some(vm) = find(vm) {
                    vm.key_input(id, key);
                }
            }
            Message::PointerInput { vm, pointer } => {
                if let Some(vm) = find(vm) {
                    vm.pointer_input(id, pointer);
                }
            }
            Message::DebugInput { vm, data } => {
                if let Some(vm) = find(vm) {
                    vm.debug_input(id, &data);
                }
            }
            _ => {}
        }
    }

    /// The connection is ending: detach it from every console and screen, and from the
    /// VMs it debugs (which run on without its breakpoints).
    fn detach_everywhere(&self) {
        let vms: Vec<Arc<Vm>> = lock(&self.server.table).vms.values().cloned().collect();
        for vm in vms {
            vm.detach_console(self.outbox.id);
            vm.detach_screen(self.outbox.id);
            vm.detach_debugger(self.outbox.id);
        }
    }
}

/// The reply to a request from a group this server does not serve.
fn unsupported(msg: &Message) -> Fail {
    let why = match msg.type_code() >> 8 {
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
        assert_eq!(parse_max_ram("64MiB").unwrap(), 64 * MIB);
        assert_eq!(parse_max_ram("2T").unwrap(), 2 << 40);
        for bad in ["0", "-1", "+5", "1.5", "64MB", "", "99999999999999999999"] {
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
