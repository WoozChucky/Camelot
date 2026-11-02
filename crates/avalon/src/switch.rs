//! Virtual Ethernet switches: the networks Avalon's VMs talk over (protocol doc,
//! section 6.7).
//!
//! A [`Switch`] is a learning Ethernet switch, like the box under a desk. Each port
//! has something plugged in, for now a VM's network card ([`Card`]), and a queue of
//! frames waiting for it. When a frame comes in on a port, the switch:
//!
//! 1. **learns** its source address: the sender's MAC is reachable through that port
//!    (the [`MacTable`]);
//! 2. **forwards** it: to the one port its destination was learned on, or, for a
//!    destination it does not know and for broadcast and multicast destinations, to
//!    every other port (**flooding**). A frame for an address learned on the port it
//!    came in on is dropped (**filtered**): its destination already has it.
//!
//! Learned addresses **age**: an entry not refreshed by a frame from that address for
//! [`MAC_AGEING`] is forgotten, so a card that moved to another port (or went away)
//! is found again by flooding. The table holds at most [`MAC_TABLE_SIZE`] addresses,
//! so a guest that makes up source addresses cannot make it grow without bound: when
//! it is full of fresh entries, new addresses are not learned and their frames flood.
//! Nor can it take another port's address: a card's frame whose source is the address
//! of another port (another card, or the gateway) goes nowhere, so the frames meant for
//! that port keep going to it.
//!
//! A switch may also have one port with a **NAT gateway** plugged in ([`Switch::set_nat`],
//! protocol doc section 6.7, "NAT"): the user-mode gateway of `crates/gateway`, which
//! gives the VMs on the switch DHCP leases, DNS, and TCP and UDP to the host and the
//! internet through host sockets, and forwards host ports to them (`hostfwd`). Its
//! port is like a card's (a bounded queue, counters, captured frames), but nothing
//! polls it from a machine's run loop: the gateway runs on a thread of its own
//! ([`run_gateway`]), woken when a frame is queued for it and every few milliseconds
//! to service its host sockets. One gateway serves every card on the switch, each a
//! machine of its own to it (a lease, and flow limits, per MAC address); the switch
//! passes it only frames whose source address is the sending card's own, so a guest
//! cannot pose as more machines than it has cards.
//!
//! **Nothing here blocks a VM.** A card's frames go through the switch on its VM's own
//! thread, in the middle of a slice, so every step is short and bounded: the switch's
//! lock is held only to copy a frame into the queues of the ports it goes to, a queue
//! holds at most [`PORT_QUEUE`] frames (the frames past that are dropped and counted
//! on that port: a slow, paused or stopped VM loses its own frames, not anybody
//! else's), and a capture hands the frames to a thread of its own over a bounded
//! channel. A card whose queue gets a frame rings its VM's bell, which wakes a VM
//! that is idling in `wfi` so it takes the frame at once rather than at its next look.
//!
//! Locks are taken in this order, never the other way round: the server's VM table,
//! the server's switch table, a VM's lock, a card's plug, a switch's state, a
//! gateway's leases. A card's bell is rung (which takes its VM's lock) only after the
//! plug and switch locks have been released. The gateway's thread holds no lock while
//! it runs the gateway, and none of the switch's while it rings bells.

use crate::{lock, Fail};
use avalon_proto::{
    ErrorCode, FwdProto, Lease, NatConfig, NatInfo, PortForward, PortOwner, SwitchInfo, SwitchPort,
};
use gateway::{Gateway, HostFwd, Proto, GATEWAY_MAC};
use grail::net::{pcap_header, pcap_record, MacAddr, NetBackend, MIN_FRAME};
use std::collections::{HashMap, VecDeque};
use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::net::Ipv4Addr;
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Condvar, Mutex, OnceLock, Weak};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Frames waiting for one port, at most. Past this the port's frames are dropped (and
/// counted in its `dropped`): its card is not taking them, and the others must not
/// wait for it.
pub const PORT_QUEUE: usize = 256;

/// How long a learned address is kept without a frame from it (the usual default of
/// hardware switches).
pub const MAC_AGEING: Duration = Duration::from_secs(300);

/// Most addresses a switch's table holds.
pub const MAC_TABLE_SIZE: usize = 4096;

/// Frames a capture can be behind its file before it drops them.
pub const CAPTURE_QUEUE: usize = 1024;

/// Most switches one Avalon server has.
pub const MAX_SWITCHES: usize = 64;

/// How often a gateway with open flows services its host sockets when no frame wakes
/// it: the gateway's own [`gateway::Limits::service_interval`].
pub const GATEWAY_BUSY_POLL: Duration = Duration::from_millis(1);

/// How often an idle gateway (no flows) looks at its sockets: for a host client of a
/// forwarded port, or a DNS answer.
pub const GATEWAY_IDLE_POLL: Duration = Duration::from_millis(10);

/// Frames the gateway's thread takes from the gateway in one round before it looks at
/// its port's queue again.
const GATEWAY_BUDGET: usize = 64;

/// The learning part of a switch: which port each source address was last seen on,
/// and when.
pub struct MacTable {
    entries: HashMap<MacAddr, Learned>,
    ageing: Duration,
    capacity: usize,
}

#[derive(Debug, Clone, Copy)]
struct Learned {
    port: u32,
    seen: Instant,
}

/// Where a switch sends a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Forward {
    /// To this one port.
    Port(u32),
    /// To every port but the one it came in on.
    Flood,
    /// Nowhere: its destination is on the port it came from.
    Filter,
}

impl MacTable {
    pub fn new(ageing: Duration, capacity: usize) -> Self {
        MacTable {
            entries: HashMap::new(),
            ageing,
            capacity,
        }
    }

    /// Addresses in the table, fresh or not yet expired.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Records that `mac` sent a frame through `port` at `now`. Group addresses are
    /// never the source of a valid frame, so they are not learned; neither is a new
    /// address while the table is full of fresh ones.
    pub fn learn(&mut self, mac: MacAddr, port: u32, now: Instant) {
        if mac.is_multicast() {
            return;
        }
        if let Some(entry) = self.entries.get_mut(&mac) {
            *entry = Learned { port, seen: now };
            return;
        }
        if self.entries.len() >= self.capacity {
            self.expire(now);
            if self.entries.len() >= self.capacity {
                return;
            }
        }
        self.entries.insert(mac, Learned { port, seen: now });
    }

    /// The port `mac` was learned on, unless that was [`MAC_AGEING`] (the table's
    /// ageing time) or more before `now`.
    pub fn lookup(&self, mac: MacAddr, now: Instant) -> Option<u32> {
        let entry = self.entries.get(&mac)?;
        (now.saturating_duration_since(entry.seen) < self.ageing).then_some(entry.port)
    }

    /// Forgets the addresses that have aged out by `now`.
    pub fn expire(&mut self, now: Instant) {
        let ageing = self.ageing;
        self.entries
            .retain(|_, e| now.saturating_duration_since(e.seen) < ageing);
    }

    /// Forgets every address learned on `port` (it was unplugged).
    pub fn forget_port(&mut self, port: u32) {
        self.entries.retain(|_, e| e.port != port);
    }

    /// Learns the source of `frame`, which came in on port `from`, and decides where
    /// it goes. `frame` is at least an Ethernet header (14 bytes): destination, then
    /// source.
    pub fn forward(&mut self, from: u32, frame: &[u8], now: Instant) -> Forward {
        let dst = mac_at(frame, 0);
        self.learn(mac_at(frame, 6), from, now);
        if dst.is_multicast() {
            return Forward::Flood;
        }
        match self.lookup(dst, now) {
            Some(port) if port == from => Forward::Filter,
            Some(port) => Forward::Port(port),
            None => Forward::Flood,
        }
    }
}

fn mac_at(frame: &[u8], at: usize) -> MacAddr {
    MacAddr(frame[at..at + 6].try_into().expect("6 bytes"))
}

/// A virtual switch. Clones are the same switch.
#[derive(Clone)]
pub struct Switch {
    inner: Arc<SwitchInner>,
}

struct SwitchInner {
    name: String,
    state: Mutex<State>,
}

struct State {
    /// Deleted: nothing can be plugged in, and nothing flows.
    closed: bool,
    /// Port numbers are never reused, so a stale number cannot reach a new port.
    next_port: u32,
    /// In the order they were plugged in.
    ports: Vec<Port>,
    table: MacTable,
    capture: Option<Capture>,
    /// The NAT gateway, while the switch has one.
    nat: Option<Nat>,
}

struct Port {
    id: u32,
    owner: PortOwner,
    mac: MacAddr,
    /// Frames for what is plugged in, oldest first.
    queue: VecDeque<Vec<u8>>,
    plugged: Plugged,
    in_frames: u64,
    in_bytes: u64,
    out_frames: u64,
    out_bytes: u64,
    dropped: u64,
}

/// What is plugged into a port, and so what to wake when a frame is queued for it.
enum Plugged {
    /// A VM's card: its bell wakes its VM, and the card is unplugged when the switch
    /// is deleted. Weak: the VM owns its cards.
    Card(Weak<Card>),
    /// The switch's NAT gateway, whose thread waits on this.
    Gateway(Arc<Wake>),
}

/// Something a frame was queued for, to wake once the switch's lock is released.
enum Ring {
    Card(Arc<Card>),
    Gateway(Arc<Wake>),
}

impl Ring {
    fn ring(&self) {
        match self {
            Ring::Card(card) => card.ring(),
            Ring::Gateway(wake) => wake.notify(false),
        }
    }
}

/// A switch's NAT gateway: its port, the thread that runs it, and what `SwitchList`
/// shows of it.
struct Nat {
    port: u32,
    /// The forwarding rules as bound.
    hostfwd: Vec<HostFwd>,
    /// The gateway's leases, as its thread last saw them.
    leases: Arc<Mutex<Vec<Lease>>>,
    wake: Arc<Wake>,
    thread: JoinHandle<()>,
}

impl Nat {
    /// Ends the gateway's thread and waits for it: when this returns, the gateway is
    /// gone and its host sockets are closed, so its ports can be bound again.
    fn stop(self) {
        self.wake.notify(true);
        // A panic there ended the gateway all the same.
        let _ = self.thread.join();
    }
}

/// How a gateway's thread is woken: a frame was queued for it, or it must stop.
#[derive(Default)]
struct Wake {
    state: Mutex<WakeState>,
    cond: Condvar,
}

#[derive(Default)]
struct WakeState {
    pending: bool,
    stop: bool,
}

impl Wake {
    fn notify(&self, stop: bool) {
        let mut st = lock(&self.state);
        st.pending = true;
        st.stop |= stop;
        self.cond.notify_one();
    }

    /// Waits until notified or `timeout` has passed. True when the thread must stop.
    fn wait(&self, timeout: Duration) -> bool {
        let mut st = lock(&self.state);
        if !st.pending {
            st = self
                .cond
                .wait_timeout(st, timeout)
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .0;
        }
        st.pending = false;
        st.stop
    }
}

impl Switch {
    /// A new switch with no ports, which forgets addresses after [`MAC_AGEING`].
    pub fn new(name: &str) -> Switch {
        Switch::with_ageing(name, MAC_AGEING)
    }

    pub fn with_ageing(name: &str, ageing: Duration) -> Switch {
        Switch {
            inner: Arc::new(SwitchInner {
                name: name.to_string(),
                state: Mutex::new(State {
                    closed: false,
                    next_port: 1,
                    ports: Vec::new(),
                    table: MacTable::new(ageing, MAC_TABLE_SIZE),
                    capture: None,
                    nat: None,
                }),
            }),
        }
    }

    pub fn name(&self) -> &str {
        &self.inner.name
    }

    /// Whether `self` and `other` are the same switch.
    pub fn same(&self, other: &Switch) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        lock(&self.inner.state)
    }

    fn no_such(&self) -> Fail {
        Fail::new(
            ErrorCode::NoSuchSwitch,
            format!("switch '{}' has been deleted", self.name()),
        )
    }

    /// Plugs `card` in, as `owner`, with the card's address. Refused if the switch has
    /// been deleted, or if another port has that address (two cards with one address
    /// on one network would steal each other's frames).
    fn attach(&self, owner: PortOwner, card: &Arc<Card>) -> Result<u32, Fail> {
        let mut st = self.state();
        if st.closed {
            return Err(self.no_such());
        }
        if let Some(other) = st.ports.iter().find(|p| p.mac == card.mac) {
            return Err(Fail::new(
                ErrorCode::AlreadyExists,
                format!(
                    "{} is on switch '{}' already ({}): two cards with one address cannot \
                     share a network",
                    card.mac,
                    self.name(),
                    owner_text(other.owner)
                ),
            ));
        }
        Ok(st.add_port(owner, card.mac, Plugged::Card(Arc::downgrade(card))))
    }

    /// Unplugs port `id`: its queued frames go with it, and so do the addresses
    /// learned on it.
    fn detach(&self, id: u32) {
        let mut st = self.state();
        st.ports.retain(|p| p.id != id);
        st.table.forget_port(id);
    }

    /// A frame sent by what is plugged into port `from`, now. Returns what it was
    /// queued for, which the caller wakes once it has let go of its locks.
    fn input(&self, from: u32, frame: &[u8]) -> Vec<Ring> {
        self.input_at(from, frame, Instant::now())
    }

    /// [`Switch::input`] at time `now` (tests make time pass without waiting).
    fn input_at(&self, from: u32, frame: &[u8], now: Instant) -> Vec<Ring> {
        let mut st = self.state();
        let st = &mut *st;
        if st.closed {
            return Vec::new();
        }
        let Some(port) = st.ports.iter_mut().find(|p| p.id == from) else {
            return Vec::new();
        };
        port.in_frames += 1;
        port.in_bytes += frame.len() as u64;
        // A card's frames reach the gateway only with the card's own source address
        // (below).
        let from_card = matches!(port.plugged, Plugged::Card(_)).then_some(port.mac);
        if let Some(capture) = &mut st.capture {
            capture.record(frame);
        }
        // Cards never send a frame shorter than its header; anything else might.
        if frame.len() < MIN_FRAME {
            return Vec::new();
        }
        // A card that sends from the address of another port (the gateway's, or another
        // card's) would move that address to its own port in the table, and receive
        // the frames meant for it: such frames go nowhere.
        let src = mac_at(frame, 6);
        if from_card.is_some() && st.ports.iter().any(|p| p.id != from && p.mac == src) {
            return Vec::new();
        }
        let forward = st.table.forward(from, frame, now);
        let mut rung = Vec::new();
        for port in &mut st.ports {
            let to_it = match forward {
                Forward::Port(id) => port.id == id,
                Forward::Flood => port.id != from,
                Forward::Filter => false,
            };
            if !to_it {
                continue;
            }
            // The gateway counts flows and gives leases per MAC address: a card
            // sending from addresses that are not its own could be many machines to
            // it, and use up what is meant for the others.
            if matches!(port.plugged, Plugged::Gateway(_)) && from_card.is_some_and(|mac| mac != src) {
                continue;
            }
            if port.queue.len() >= PORT_QUEUE {
                port.dropped += 1;
                continue;
            }
            port.queue.push_back(frame.to_vec());
            port.out_frames += 1;
            port.out_bytes += frame.len() as u64;
            match &port.plugged {
                Plugged::Card(card) => rung.extend(card.upgrade().map(Ring::Card)),
                Plugged::Gateway(wake) => rung.push(Ring::Gateway(Arc::clone(wake))),
            }
        }
        rung
    }

    /// The next frame for what is plugged into port `id`.
    fn take(&self, id: u32) -> Option<Vec<u8>> {
        self.state()
            .ports
            .iter_mut()
            .find(|p| p.id == id)?
            .queue
            .pop_front()
    }

    /// Every frame waiting for port `id`.
    fn take_all(&self, id: u32) -> Vec<Vec<u8>> {
        match self.state().ports.iter_mut().find(|p| p.id == id) {
            Some(port) => port.queue.drain(..).collect(),
            None => Vec::new(),
        }
    }

    /// Deletes the switch: nothing flows through it any more, its capture stops, every
    /// card plugged into it is unplugged (link down), and its gateway ends, its host
    /// sockets closed by the time this returns.
    pub fn close(&self) {
        let (cards, capture, nat) = {
            let mut st = self.state();
            st.closed = true;
            let cards: Vec<Arc<Card>> = st
                .ports
                .drain(..)
                .filter_map(|p| match p.plugged {
                    Plugged::Card(card) => card.upgrade(),
                    Plugged::Gateway(_) => None,
                })
                .collect();
            (cards, st.capture.take(), st.nat.take())
        };
        for card in cards {
            card.unplug_from(self);
        }
        if let Some(nat) = nat {
            nat.stop();
        }
        if let Some(capture) = capture {
            // The switch is gone, and its capture with it: nobody is left to tell of
            // a write that failed.
            let _ = capture.finish();
        }
    }

    /// Gives the switch a NAT gateway (`Some`), or takes its gateway away (`None`).
    ///
    /// A new gateway binds its forwarded host ports here, on the requesting thread, so
    /// a port that cannot be bound is the request's error (`Io`), then gets a port of
    /// the switch with its address ([`GATEWAY_MAC`]) and a thread of its own. Errors:
    /// `AlreadyExists` (the switch has a gateway already, or a card with the gateway's
    /// address is plugged in), `InvalidState` (taking away a gateway it does not have),
    /// `NoSuchSwitch` (it has been deleted). A gateway taken away is gone, its host
    /// sockets closed, when this returns.
    pub fn set_nat(&self, nat: Option<&NatConfig>) -> Result<(), Fail> {
        let Some(config) = nat else {
            let nat = {
                let mut st = self.state();
                let Some(nat) = st.nat.take() else {
                    return Err(Fail::new(
                        ErrorCode::InvalidState,
                        format!("switch '{}' has no NAT gateway", self.name()),
                    ));
                };
                st.ports.retain(|p| p.id != nat.port);
                st.table.forget_port(nat.port);
                nat
            };
            nat.stop();
            return Ok(());
        };
        // Said before binding anything, so that asking twice is not an I/O error.
        self.check_nat_free(&self.state())?;
        let gateway = Gateway::new(gateway::Config {
            hostfwd: config.hostfwd.iter().map(to_gateway_rule).collect(),
            ..gateway::Config::default()
        })
        .map_err(|e| {
            Fail::new(
                ErrorCode::Io,
                format!("switch '{}': cannot forward the port: {e}", self.name()),
            )
        })?;
        let hostfwd = gateway.hostfwd();
        let mut st = self.state();
        // Checked again: the gateway was made without the lock. A gateway dropped here
        // closes its sockets.
        self.check_nat_free(&st)?;
        let wake = Arc::new(Wake::default());
        let port = st.add_port(
            PortOwner::Gateway,
            MacAddr(GATEWAY_MAC),
            Plugged::Gateway(Arc::clone(&wake)),
        );
        let leases = Arc::new(Mutex::new(Vec::new()));
        let thread = {
            let (switch, wake, leases) = (
                Arc::downgrade(&self.inner),
                Arc::clone(&wake),
                Arc::clone(&leases),
            );
            std::thread::Builder::new()
                .name(format!("gateway {}", self.name()))
                .spawn(move || run_gateway(switch, port, gateway, wake, leases))
        };
        let thread = match thread {
            Ok(thread) => thread,
            Err(e) => {
                st.ports.retain(|p| p.id != port);
                return Err(Fail::new(
                    ErrorCode::Internal,
                    format!("cannot start the gateway of switch '{}': {e}", self.name()),
                ));
            }
        };
        st.nat = Some(Nat {
            port,
            hostfwd,
            leases,
            wake,
            thread,
        });
        Ok(())
    }

    /// Whether a gateway can be added to the switch as `st` is.
    fn check_nat_free(&self, st: &State) -> Result<(), Fail> {
        if st.closed {
            return Err(self.no_such());
        }
        if st.nat.is_some() {
            return Err(Fail::new(
                ErrorCode::AlreadyExists,
                format!(
                    "switch '{}' has a NAT gateway already; take it away first",
                    self.name()
                ),
            ));
        }
        if let Some(other) = st.ports.iter().find(|p| p.mac == MacAddr(GATEWAY_MAC)) {
            return Err(Fail::new(
                ErrorCode::AlreadyExists,
                format!(
                    "{} has the gateway's address {}: a gateway cannot share switch '{}' with it",
                    owner_text(other.owner),
                    MacAddr(GATEWAY_MAC),
                    self.name()
                ),
            ));
        }
        Ok(())
    }

    /// The switch, its ports and their counters, for `SwitchList`.
    pub fn info(&self) -> SwitchInfo {
        let st = self.state();
        SwitchInfo {
            name: self.name().to_string(),
            capture: st.capture.as_ref().map(|c| c.path.clone()),
            nat: st.nat.as_ref().map(|nat| NatInfo {
                hostfwd: nat.hostfwd.iter().map(from_gateway_rule).collect(),
                leases: lock(&nat.leases).clone(),
            }),
            ports: st
                .ports
                .iter()
                .map(|p| SwitchPort {
                    owner: p.owner,
                    mac: p.mac.0,
                    in_frames: p.in_frames,
                    in_bytes: p.in_bytes,
                    out_frames: p.out_frames,
                    out_bytes: p.out_bytes,
                    dropped: p.dropped,
                })
                .collect(),
        }
    }

    /// Addresses the switch has learned (expired ones included until they are
    /// cleaned out).
    pub fn learned(&self) -> usize {
        self.state().table.len()
    }

    /// Starts capturing every frame that enters the switch into a new pcap file at
    /// `path` on this host (replacing any). Errors: `Busy` (a capture is running),
    /// `Io` (the file cannot be created).
    pub fn start_capture(&self, path: &str) -> Result<(), Fail> {
        if let Some(c) = &self.state().capture {
            return Err(Fail::new(
                ErrorCode::Busy,
                format!(
                    "switch '{}' is being captured into {} already",
                    self.name(),
                    c.path
                ),
            ));
        }
        // Opened here, on the requesting thread, so that a bad path is the request's
        // error; the frames are written on the capture's own thread.
        let file = File::create(path).map_err(|e| Fail::new(ErrorCode::Io, format!("{path}: {e}")))?;
        self.capture_into(path, BufWriter::new(file))
    }

    /// Starts capturing into `out`, which [`Switch::start_capture`] opened as `path`.
    fn capture_into(&self, path: &str, out: impl Write + Send + 'static) -> Result<(), Fail> {
        let capture =
            Capture::start(path, out).map_err(|e| Fail::new(ErrorCode::Io, format!("{path}: {e}")))?;
        let mut st = self.state();
        if st.closed {
            drop(st);
            let _ = capture.finish();
            return Err(self.no_such());
        }
        if st.capture.is_some() {
            // Another request started one meanwhile.
            drop(st);
            let _ = capture.finish();
            return Err(Fail::new(
                ErrorCode::Busy,
                format!("switch '{}' is being captured already", self.name()),
            ));
        }
        st.capture = Some(capture);
        Ok(())
    }

    /// Stops the capture, once every frame captured so far is in the file. Errors:
    /// `InvalidState` (no capture is running), `Io` (writing the file failed, which
    /// ended the capture there: the frames after that are not in it). The capture is
    /// stopped either way.
    pub fn stop_capture(&self) -> Result<(), Fail> {
        let capture = self.state().capture.take();
        match capture {
            Some(capture) => {
                let path = capture.path.clone();
                capture.finish().map_err(|e| {
                    Fail::new(
                        ErrorCode::Io,
                        format!("{path}: {e}; the capture stopped there, and the frames after it are not in the file"),
                    )
                })
            }
            None => Err(Fail::new(
                ErrorCode::InvalidState,
                format!("switch '{}' is not being captured", self.name()),
            )),
        }
    }
}

impl State {
    /// Plugs `plugged` into a new port, as `owner`, with address `mac`.
    fn add_port(&mut self, owner: PortOwner, mac: MacAddr, plugged: Plugged) -> u32 {
        let id = self.next_port;
        self.next_port += 1;
        self.ports.push(Port {
            id,
            owner,
            mac,
            queue: VecDeque::new(),
            plugged,
            in_frames: 0,
            in_bytes: 0,
            out_frames: 0,
            out_bytes: 0,
            dropped: 0,
        });
        id
    }
}

fn owner_text(owner: PortOwner) -> String {
    match owner {
        PortOwner::Card { vm, card } => format!("card {card} of VM {vm}"),
        PortOwner::Gateway => "the switch's NAT gateway".into(),
    }
}

/// A forwarding rule as the gateway takes it.
fn to_gateway_rule(rule: &PortForward) -> HostFwd {
    HostFwd {
        proto: match rule.proto {
            FwdProto::Tcp => Proto::Tcp,
            FwdProto::Udp => Proto::Udp,
        },
        host: rule.host,
        guest: rule.guest,
    }
}

/// A forwarding rule as the protocol carries it.
fn from_gateway_rule(rule: &HostFwd) -> PortForward {
    PortForward {
        proto: match rule.proto {
            Proto::Tcp => FwdProto::Tcp,
            Proto::Udp => FwdProto::Udp,
        },
        host: rule.host,
        guest: rule.guest,
    }
}

/// A gateway's thread: passes the frames queued for its port `port` to the gateway,
/// and the gateway's frames into the switch, until it is told to stop or the switch
/// is gone. Between rounds it sleeps until a frame is queued for it, or for
/// [`GATEWAY_BUSY_POLL`] while the gateway has flows open ([`GATEWAY_IDLE_POLL`]
/// otherwise), when the gateway services its host sockets. The gateway is dropped,
/// closing its sockets, when the thread ends.
fn run_gateway(
    switch: Weak<SwitchInner>,
    port: u32,
    mut gateway: Gateway,
    wake: Arc<Wake>,
    leases: Arc<Mutex<Vec<Lease>>>,
) {
    let mut stop = false;
    while !stop {
        let Some(inner) = switch.upgrade() else { return };
        let switch = Switch { inner };
        let incoming = switch.take_all(port);
        for frame in &incoming {
            gateway.send(frame);
        }
        if !incoming.is_empty() {
            // Leases are only given in answer to a frame, and never change.
            let now = gateway.leases();
            let mut shown = lock(&leases);
            if now.len() != shown.len() {
                *shown = now
                    .into_iter()
                    .map(|(mac, addr): ([u8; 6], Ipv4Addr)| Lease { mac, addr })
                    .collect();
            }
        }
        let mut sent = 0;
        while sent < GATEWAY_BUDGET {
            let Some(frame) = gateway.recv() else { break };
            sent += 1;
            for ring in switch.input(port, &frame) {
                ring.ring();
            }
        }
        if sent == GATEWAY_BUDGET || !incoming.is_empty() {
            // More may be waiting either way: look again at once.
            stop = lock(&wake.state).stop;
            continue;
        }
        let poll = if gateway.flows() > 0 {
            GATEWAY_BUSY_POLL
        } else {
            GATEWAY_IDLE_POLL
        };
        drop(switch);
        stop = wake.wait(poll);
    }
}

/// A switch's capture: records go to a writer thread over a bounded channel, so a slow
/// disk never holds up the VMs. Records that do not fit are dropped.
struct Capture {
    path: String,
    tx: SyncSender<Vec<u8>>,
    /// Ends with the first write error, which [`Capture::finish`] hands back.
    thread: JoinHandle<io::Result<()>>,
}

impl Capture {
    /// Writes the pcap header to `out` and starts the writer thread.
    fn start(path: &str, mut out: impl Write + Send + 'static) -> io::Result<Capture> {
        out.write_all(&pcap_header())?;
        out.flush()?;
        let (tx, rx) = mpsc::sync_channel(CAPTURE_QUEUE);
        let thread = std::thread::Builder::new()
            .name("capture".into())
            .spawn(move || write_records(rx, out))?;
        Ok(Capture {
            path: path.to_string(),
            tx,
            thread,
        })
    }

    /// Queues `frame`, time-stamped with the wall clock, unless the writer is behind
    /// (or has failed).
    fn record(&mut self, frame: &[u8]) {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
        match self.tx.try_send(pcap_record(now, frame)) {
            Ok(()) | Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {}
        }
    }

    /// Ends the capture: the writer writes what is queued, flushes, and ends. An
    /// error is the one that ended the writer early.
    fn finish(self) -> io::Result<()> {
        drop(self.tx);
        self.thread
            .join()
            .unwrap_or_else(|_| Err(io::Error::other("the capture thread panicked")))
    }
}

/// The capture thread: writes records until the channel closes. A write error ends
/// the capture there (the frames keep flowing through the switch) and is returned;
/// the records queued after it are dropped.
fn write_records(rx: Receiver<Vec<u8>>, mut out: impl Write) -> io::Result<()> {
    while let Ok(record) = rx.recv() {
        out.write_all(&record)?;
        // Write what else is waiting, then flush once: the file is complete whenever
        // the switch is idle, without a flush per frame under load.
        while let Ok(more) = rx.try_recv() {
            out.write_all(&more)?;
        }
        out.flush()?;
    }
    Ok(())
}

/// What rings when a card's switch queues a frame for it: wakes the card's VM.
pub type Bell = Arc<dyn Fn() + Send + Sync>;

/// A VM's network card as the switches see it: its number and address, the switch
/// port its cable is plugged into, and the bell that wakes its VM. The machine holds
/// the card's [`NetBackend`] ([`Card::backend`]); the VM keeps the card, to plug it in
/// and out.
pub struct Card {
    /// The card's number in its VM, from 0.
    pub index: u8,
    pub mac: MacAddr,
    plug: Mutex<Plug>,
    bell: OnceLock<Bell>,
}

#[derive(Default)]
struct Plug {
    /// The switch and port number, while plugged in.
    port: Option<(Switch, u32)>,
    /// The machine is gone (its backend was dropped): the card cannot be plugged in
    /// again.
    ended: bool,
}

impl Card {
    pub fn new(index: u8, mac: MacAddr) -> Arc<Card> {
        Arc::new(Card {
            index,
            mac,
            plug: Mutex::default(),
            bell: OnceLock::new(),
        })
    }

    /// Sets what rings when a frame is queued for the card. Only the first call counts.
    pub fn set_bell(&self, bell: Bell) {
        let _ = self.bell.set(bell);
    }

    fn ring(&self) {
        if let Some(bell) = self.bell.get() {
            bell();
        }
    }

    /// The switch the card is plugged into.
    pub fn switch(&self) -> Option<Switch> {
        lock(&self.plug).port.as_ref().map(|(s, _)| s.clone())
    }

    /// Plugs the card into `switch`, as `owner`. Errors: `Busy` (it is plugged in
    /// already), `InvalidState` (its machine has ended), and `switch`'s own
    /// (`NoSuchSwitch`, `AlreadyExists`).
    pub fn plug(self: &Arc<Self>, owner: PortOwner, switch: &Switch) -> Result<(), Fail> {
        let mut plug = lock(&self.plug);
        if plug.ended {
            return Err(Fail::new(
                ErrorCode::InvalidState,
                format!("{} is gone: its VM's machine has ended", owner_text(owner)),
            ));
        }
        if let Some((on, _)) = &plug.port {
            return Err(Fail::new(
                ErrorCode::Busy,
                format!(
                    "{} is plugged into switch '{}'; detach it first",
                    owner_text(owner),
                    on.name()
                ),
            ));
        }
        let id = switch.attach(owner, self)?;
        plug.port = Some((switch.clone(), id));
        Ok(())
    }

    /// Unplugs the card. False if it was not plugged in.
    pub fn unplug(&self) -> bool {
        let port = lock(&self.plug).port.take();
        match port {
            Some((switch, id)) => {
                switch.detach(id);
                true
            }
            None => false,
        }
    }

    /// Ends the card for good, as its machine ends: it is unplugged, and cannot be
    /// plugged in again. The VM calls this before it tells anyone that its machine has
    /// ended (stopped or exited), so a client that has heard that never sees the card
    /// on a switch, nor plugs it in. Dropping the card's backend does it too.
    pub fn end(&self) {
        let port = {
            let mut plug = lock(&self.plug);
            plug.ended = true;
            plug.port.take()
        };
        if let Some((switch, id)) = port {
            switch.detach(id);
        }
    }

    /// Unplugs the card if it is plugged into `switch` (which is being deleted).
    fn unplug_from(&self, switch: &Switch) {
        let mut plug = lock(&self.plug);
        if plug.port.as_ref().is_some_and(|(s, _)| s.same(switch)) {
            plug.port = None;
        }
    }

    /// The backend for the card's machine. When the machine drops it, the card is
    /// unplugged for good.
    pub fn backend(self: &Arc<Self>) -> CardBackend {
        CardBackend(Arc::clone(self))
    }
}

/// A card's [`NetBackend`]: frames to and from the port the card is plugged into. An
/// unplugged card has its link down; what it sends is lost, as with no cable.
pub struct CardBackend(Arc<Card>);

impl NetBackend for CardBackend {
    fn send(&mut self, frame: &[u8]) -> io::Result<()> {
        let rung = match &lock(&self.0.plug).port {
            Some((switch, id)) => switch.input(*id, frame),
            None => Vec::new(),
        };
        for ring in rung {
            ring.ring();
        }
        Ok(())
    }

    fn recv(&mut self) -> Option<Vec<u8>> {
        let plug = lock(&self.0.plug);
        let (switch, id) = plug.port.as_ref()?;
        switch.take(*id)
    }

    fn link_up(&self) -> bool {
        lock(&self.0.plug).port.is_some()
    }
}

impl Drop for CardBackend {
    fn drop(&mut self) {
        self.0.end();
    }
}

/// The address Avalon gives card `card` of the VM named `vm` when its profile sets
/// none: `02` (locally administered, individual), then five bytes of a hash of the
/// name and the card's number. The same VM name always gets the same addresses, across
/// a VM's re-creation and the server's restarts, so the guests' ARP caches and any
/// DHCP leases stay valid; different names get different ones (a clash is a 1 in 2^40
/// chance, and is refused when the cards meet on a switch).
pub fn generated_mac(vm: &str, card: u8) -> MacAddr {
    // FNV-1a, 64 bits: simple and the same everywhere (std's hasher is randomized).
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in vm.as_bytes().iter().chain(&[0, card]) {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    let h = hash.to_be_bytes();
    MacAddr([0x02, h[0], h[1], h[2], h[3], h[4]])
}

#[cfg(test)]
mod tests;
