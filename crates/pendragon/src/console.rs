//! Live consoles for the browser.
//!
//! A browser cannot speak the Avalon protocol, so Pendragon attaches to a VM's console
//! on its behalf: a [`Tap`] is one protocol session attached to one VM
//! (`ConsoleAttach`), with a thread that reads its `ConsoleOutput` into a buffer. The
//! page asks for what is new with an offset, the way a file is read; a request with
//! nothing new waits a little for output (long polling), so text shows up as soon as
//! the guest writes it without the page asking many times a second.
//!
//! Offsets count bytes since the tap attached: the scrollback Avalon sends on
//! attaching starts at 0. A tap keeps the last [`ConsoleLimits::keep`] bytes; a page
//! that falls further behind gets what is left (and knows, since the reply starts
//! later than it asked). Each tap has a number of its own; a page that asks with
//! another tap's number (Pendragon restarted, or the tap was closed for being idle)
//! starts again from the oldest byte, with `reset` set so it clears what it shows.
//!
//! Every VM has at most one tap, shared by every page showing it, and there are at
//! most [`ConsoleLimits::max_taps`]. A tap nobody has read or written for
//! [`ConsoleLimits::idle`] is closed by a thread that looks every little while
//! ([`Hub::sweep_until_closed`]). Input from the page goes to the VM's UART as
//! `ConsoleInput` over the same session.

use avalon_client::{Client, Timeouts};
use avalon_proto::{Connection, Message, VmRef};
use std::collections::{HashMap, VecDeque};
use std::net::{Shutdown, TcpStream};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

/// Output kept per tap by default: as much as Avalon's scrollback.
pub const KEEP: usize = 64 * 1024;

/// By default, a tap nobody has read or written for this long is closed.
pub const IDLE: Duration = Duration::from_secs(60);

/// Most taps at once by default (each is a connection and a thread).
pub const MAX_TAPS: usize = 32;

/// Most bytes of input in one `ConsoleInput`.
const INPUT_CHUNK: usize = 4096;

/// How many consoles Pendragon keeps open, how much of each, and for how long
/// ([`crate::Config::consoles`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConsoleLimits {
    /// Output kept per console, in bytes.
    pub keep: usize,
    /// Most consoles open at once; more are refused (409, over budget).
    pub max_taps: usize,
    /// A console nobody has read or written for this long is closed.
    pub idle: Duration,
}

impl Default for ConsoleLimits {
    fn default() -> Self {
        ConsoleLimits {
            keep: KEEP,
            max_taps: MAX_TAPS,
            idle: IDLE,
        }
    }
}

pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// One session attached to one VM's console.
pub struct Tap {
    /// This tap's number, unique while Pendragon runs.
    pub id: u64,
    /// The VM's id.
    pub vm: u32,
    /// Bytes of output kept.
    keep: usize,
    /// Closed after this long unused.
    idle: Duration,
    state: Mutex<Output>,
    changed: Condvar,
    /// The write half of the session, for input.
    input: Mutex<Connection<TcpStream>>,
    /// To end the session from outside (idle, shutdown).
    socket: TcpStream,
    last_used: Mutex<Instant>,
}

struct Output {
    buf: VecDeque<u8>,
    /// The offset of `buf[0]`.
    start: u64,
    /// Why the session ended, once it has.
    ended: Option<String>,
}

impl Output {
    /// Appends `data`, dropping the oldest bytes past `keep`: `start` moves on by as
    /// many, so every byte keeps its offset.
    fn push(&mut self, data: &[u8], keep: usize) {
        self.buf.extend(data);
        let excess = self.buf.len().saturating_sub(keep);
        self.buf.drain(..excess);
        self.start += excess as u64;
    }
}

/// What [`Tap::read`] returns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    pub tap: u64,
    /// The offset of `data[0]`.
    pub from: u64,
    pub data: Vec<u8>,
    /// The offset after `data`: where to ask next.
    pub next: u64,
    /// The page asked with another tap's number: start over.
    pub reset: bool,
    pub ended: Option<String>,
}

impl Tap {
    /// A tap on VM `vm` over `input`, the session (`socket` is another handle on its
    /// socket), with nothing read yet.
    fn new(id: u64, vm: u32, input: Connection<TcpStream>, socket: TcpStream, limits: ConsoleLimits) -> Tap {
        Tap {
            id,
            vm,
            keep: limits.keep,
            idle: limits.idle,
            state: Mutex::new(Output {
                buf: VecDeque::new(),
                start: 0,
                ended: None,
            }),
            changed: Condvar::new(),
            input: Mutex::new(input),
            socket,
            last_used: Mutex::new(Instant::now()),
        }
    }

    /// What is buffered from offset `from` on (asked of tap `tap`), waiting up to
    /// `wait` for something new when there is nothing yet.
    pub fn read(&self, tap: Option<u64>, from: u64, wait: Duration) -> Chunk {
        self.touch();
        let reset = tap.is_some_and(|t| t != self.id);
        let mut out = lock(&self.state);
        let end = |o: &Output| o.start + o.buf.len() as u64;
        // A page that asks past the end (another tap's offsets) starts over too.
        let reset = reset || from > end(&out);
        let mut from = if reset { out.start } else { from.max(out.start) };
        let deadline = Instant::now() + wait;
        while from == end(&out) && out.ended.is_none() {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            out = self
                .changed
                .wait_timeout(out, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
            // Output may have pushed the start past where we were.
            from = from.max(out.start);
        }
        let skip = (from - out.start) as usize;
        let data: Vec<u8> = out.buf.iter().skip(skip).copied().collect();
        let chunk = Chunk {
            tap: self.id,
            from,
            next: from + data.len() as u64,
            data,
            reset,
            ended: out.ended.clone(),
        };
        drop(out);
        // A long poll is use until it ends, not only when it starts.
        self.touch();
        chunk
    }

    /// Sends `data` to the VM's UART.
    pub fn input(&self, data: &[u8]) -> Result<(), String> {
        self.touch();
        if let Some(why) = &lock(&self.state).ended {
            return Err(format!("the console session ended: {why}"));
        }
        let mut conn = lock(&self.input);
        for chunk in data.chunks(INPUT_CHUNK) {
            conn.send(&Message::ConsoleInput {
                vm: self.vm,
                data: chunk.to_vec(),
            })
            .map_err(|e| format!("cannot send to the console: {e}"))?;
        }
        Ok(())
    }

    fn touch(&self) {
        *lock(&self.last_used) = Instant::now();
    }

    fn idle(&self) -> bool {
        lock(&self.last_used).elapsed() >= self.idle
    }

    fn is_ended(&self) -> bool {
        lock(&self.state).ended.is_some()
    }

    /// Output from the VM: kept, and the pages waiting for it woken.
    fn output(&self, data: &[u8]) {
        lock(&self.state).push(data, self.keep);
        self.changed.notify_all();
    }

    /// The session ended, for `why`.
    fn end(&self, why: String) {
        lock(&self.state).ended = Some(why);
        self.changed.notify_all();
    }

    /// Ends the session; the reading thread sees the socket close and stops.
    fn close(&self) {
        let _ = self.socket.shutdown(Shutdown::Both);
    }

    /// The reading thread: output into the buffer until the session ends.
    fn pump(self: Arc<Self>, mut conn: Connection<TcpStream>) {
        let why = loop {
            match conn.recv() {
                Ok(Message::ConsoleOutput { vm, data }) if vm == self.vm => self.output(&data),
                Ok(Message::Detached { vm, .. }) if vm == self.vm => {
                    break "the VM was destroyed".to_string()
                }
                // State changes (the page asks Avalon for those), anything for
                // another VM, frames that do not decode: not output, skip them.
                Ok(_) => {}
                Err(e) if e.is_fatal() => break format!("the connection to Avalon ended ({e})"),
                Err(_) => {}
            }
        };
        self.end(why);
    }
}

/// Every open tap, by VM id.
pub struct Hub {
    avalon: String,
    timeouts: Timeouts,
    limits: ConsoleLimits,
    taps: Mutex<HashMap<u32, Arc<Tap>>>,
    next_id: AtomicU64,
    /// Set by [`Hub::close_all`]; the sweeping thread waits on `wake` for it.
    closed: Mutex<bool>,
    wake: Condvar,
}

impl Hub {
    /// Taps on the Avalon server at `avalon`; `timeouts` limit attaching.
    pub fn new(avalon: &str, timeouts: Timeouts, limits: ConsoleLimits) -> Hub {
        Hub {
            avalon: avalon.to_string(),
            timeouts,
            limits,
            taps: Mutex::new(HashMap::new()),
            // Start somewhere a page from an earlier run is unlikely to have.
            next_id: AtomicU64::new(1 + (std::process::id() as u64) * 1000),
            closed: Mutex::new(false),
            wake: Condvar::new(),
        }
    }

    /// The tap on VM `vm`, attaching one if there is none (or the last one ended).
    /// Returns the protocol client's error when Avalon refuses or cannot be reached.
    pub fn tap(&self, vm: &VmRef) -> Result<Arc<Tap>, avalon_client::Error> {
        self.sweep();
        if let VmRef::Id(id) = vm {
            if let Some(tap) = lock(&self.taps).get(id) {
                if !tap.is_ended() {
                    return Ok(Arc::clone(tap));
                }
            }
        }
        // Attach outside the lock: other VMs' pages carry on meanwhile.
        let mut client = Client::connect_with(&self.avalon, crate::AGENT, self.timeouts)?;
        let (id, scrollback) = client.attach_console(vm)?;
        let conn = client.into_connection();
        let io = |e: std::io::Error| avalon_client::Error::Request(e.into());
        // The pump waits for output as long as there is none; idle taps are closed
        // from outside. This must be set on the very handle the pump reads: on
        // Windows a cloned socket handle keeps timeouts of its own.
        conn.get_ref().set_read_timeout(None).map_err(io)?;
        let socket = conn.get_ref().try_clone().map_err(io)?;
        let input = conn.try_clone()?;
        let tap = Tap::new(
            self.next_id.fetch_add(1, Ordering::Relaxed),
            id,
            input,
            socket,
            self.limits,
        );
        tap.output(&scrollback);
        let tap = Arc::new(tap);
        let admitted = self.admit(Arc::clone(&tap))?;
        if !Arc::ptr_eq(&admitted, &tap) {
            // Another request's tap on this VM: this session is closed already.
            return Ok(admitted);
        }
        let pump = Arc::clone(&tap);
        let spawned = std::thread::Builder::new()
            .name(format!("console-{id}"))
            .spawn(move || pump.pump(conn));
        if let Err(e) = spawned {
            lock(&self.taps).remove(&id);
            tap.close();
            return Err(io(e));
        }
        Ok(tap)
    }

    /// Takes a newly attached tap into the table, unless another request attached
    /// one to the same VM meanwhile (that one is kept, and this one closed) or the
    /// table is full. Returns the tap to use.
    fn admit(&self, tap: Arc<Tap>) -> Result<Arc<Tap>, avalon_client::Error> {
        let mut taps = lock(&self.taps);
        if let Some(existing) = taps.get(&tap.vm) {
            if !existing.is_ended() {
                tap.close();
                return Ok(Arc::clone(existing));
            }
        }
        // An ended tap on this VM is about to be replaced: it does not count.
        let open = taps
            .iter()
            .filter(|(&vm, t)| vm != tap.vm && !t.is_ended())
            .count();
        if open >= self.limits.max_taps {
            tap.close();
            return Err(avalon_client::Error::Request(avalon_proto::Error::Remote {
                code: avalon_proto::ErrorCode::OverBudget,
                message: format!(
                    "Pendragon has {} consoles open already; try again in a minute",
                    self.limits.max_taps
                ),
            }));
        }
        if let Some(old) = taps.insert(tap.vm, Arc::clone(&tap)) {
            old.close();
        }
        Ok(tap)
    }

    /// How many taps are open.
    pub fn open(&self) -> usize {
        lock(&self.taps).values().filter(|t| !t.is_ended()).count()
    }

    /// Closes taps that ended or that nobody uses.
    fn sweep(&self) {
        lock(&self.taps).retain(|_, tap| {
            let keep = !tap.is_ended() && !tap.idle();
            if !keep {
                tap.close();
            }
            keep
        });
    }

    /// Sweeps every little while (a quarter of the idle time, within 10 ms and 5 s)
    /// until [`Hub::close_all`]: what the server's console thread runs.
    pub fn sweep_until_closed(&self) {
        let period = (self.limits.idle / 4).clamp(Duration::from_millis(10), Duration::from_secs(5));
        let mut closed = lock(&self.closed);
        while !*closed {
            closed = self
                .wake
                .wait_timeout(closed, period)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
            if *closed {
                break;
            }
            drop(closed);
            self.sweep();
            closed = lock(&self.closed);
        }
    }

    /// Closes every tap (at shutdown), and ends [`Hub::sweep_until_closed`].
    pub fn close_all(&self) {
        *lock(&self.closed) = true;
        self.wake.notify_all();
        for (_, tap) in lock(&self.taps).drain() {
            tap.close();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::net::TcpListener;

    const KEEP: usize = 16;

    fn limits(max_taps: usize, idle: Duration) -> ConsoleLimits {
        ConsoleLimits {
            keep: KEEP,
            max_taps,
            idle,
        }
    }

    /// A tap on VM `vm` over a loopback connection, without Avalon: its output comes
    /// from [`Tap::output`]. Returns the far end of the connection too.
    fn tap(id: u64, vm: u32, limits: ConsoleLimits) -> (Arc<Tap>, TcpStream) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let near = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (far, _) = listener.accept().unwrap();
        let socket = near.try_clone().unwrap();
        (
            Arc::new(Tap::new(id, vm, Connection::new(near), socket, limits)),
            far,
        )
    }

    fn text(chunk: &Chunk) -> &str {
        std::str::from_utf8(&chunk.data).unwrap()
    }

    #[test]
    fn output_past_the_limit_drops_the_oldest() {
        let (tap, _far) = tap(7, 1, limits(4, IDLE));
        let read = |from: u64| tap.read(Some(7), from, Duration::ZERO);
        tap.output(b"0123456789");
        let c = read(0);
        assert_eq!((c.from, text(&c), c.next, c.reset), (0, "0123456789", 10, false));
        // 20 bytes, 16 kept: offsets 0 to 3 are gone.
        tap.output(b"abcdefghij");
        let c = read(0);
        assert_eq!(
            (c.from, text(&c), c.next, c.reset),
            (4, "456789abcdefghij", 20, false)
        );
        let c = read(12);
        assert_eq!((c.from, text(&c), c.next), (12, "cdefghij", 20));
        assert_eq!((read(20).from, read(20).data.len()), (20, 0));
        // More than the limit at once: only its end is kept.
        tap.output(&[b'x'; 40]);
        let c = read(10);
        assert_eq!((c.from, c.data.len(), c.next), (44, KEEP, 60));
        // Another tap's number, or an offset past the end: from the oldest byte, reset.
        for (number, from) in [(Some(8), 50), (None, 61), (Some(7), 1000)] {
            let c = tap.read(number, from, Duration::ZERO);
            assert_eq!(
                (c.tap, c.from, c.next, c.reset),
                (7, 44, 60, true),
                "{number:?} {from}"
            );
        }
        // No number at all is a page that has not had one yet: no reset.
        assert!(!tap.read(None, 50, Duration::ZERO).reset);
    }

    #[test]
    fn a_read_waits_for_output() {
        let (tap, _far) = tap(1, 1, limits(4, IDLE));
        let writer = Arc::clone(&tap);
        let t = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            writer.output(b"late");
        });
        let c = tap.read(Some(1), 0, Duration::from_secs(10));
        assert_eq!(text(&c), "late");
        t.join().unwrap();
        tap.end("gone".into());
        let c = tap.read(Some(1), 4, Duration::from_secs(10));
        assert_eq!((c.data.len(), c.ended.as_deref()), (0, Some("gone")));
    }

    #[test]
    fn the_number_of_taps_is_limited() {
        let hub = Hub::new("127.0.0.1:1", Timeouts::default(), limits(2, IDLE));
        let (a, _fa) = tap(1, 1, hub.limits);
        let (b, _fb) = tap(2, 2, hub.limits);
        let (c, mut fc) = tap(3, 3, hub.limits);
        assert_eq!(hub.admit(Arc::clone(&a)).unwrap().id, 1);
        assert_eq!(hub.admit(b).unwrap().id, 2);
        let Err(refused) = hub.admit(c) else {
            panic!("a third tap was taken")
        };
        assert_eq!(refused.code(), Some(avalon_proto::ErrorCode::OverBudget));
        assert!(refused.to_string().contains("2 consoles open"), "{refused}");
        // The refused tap's session is closed.
        assert_eq!(fc.read(&mut [0; 1]).unwrap(), 0);
        assert_eq!(hub.open(), 2);
        // A second tap on a VM that has one is not taken: the first is shared.
        let (a2, mut fa2) = tap(4, 1, hub.limits);
        assert_eq!(hub.admit(a2).unwrap().id, 1);
        assert_eq!(fa2.read(&mut [0; 1]).unwrap(), 0);
        // An ended tap leaves room, and is replaced on its VM.
        a.end("gone".into());
        assert_eq!(hub.open(), 1);
        let (c, _fc) = tap(5, 3, hub.limits);
        assert_eq!(hub.admit(c).unwrap().id, 5);
        let (a3, _fa3) = tap(6, 1, hub.limits);
        assert!(hub.admit(a3).is_err());
    }

    #[test]
    fn unused_taps_are_closed_on_time() {
        let hub = Arc::new(Hub::new(
            "127.0.0.1:1",
            Timeouts::default(),
            limits(4, Duration::from_millis(100)),
        ));
        let start = Instant::now();
        let (unused, mut far) = tap(1, 1, hub.limits);
        let (used, _far) = tap(2, 2, hub.limits);
        hub.admit(unused).unwrap();
        hub.admit(Arc::clone(&used)).unwrap();
        let sweeper = Arc::clone(&hub);
        let sweeping = std::thread::spawn(move || sweeper.sweep_until_closed());
        // Nobody asks the hub for anything: its own thread closes the unused tap.
        while hub.open() == 2 {
            used.read(Some(2), 0, Duration::ZERO);
            assert!(
                start.elapsed() < Duration::from_secs(10),
                "the unused tap is still open"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(start.elapsed() >= Duration::from_millis(100));
        assert_eq!(hub.open(), 1);
        assert!(lock(&hub.taps).contains_key(&2));
        far.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        assert_eq!(far.read(&mut [0; 1]).unwrap(), 0);
        // Closing the hub ends the thread.
        hub.close_all();
        sweeping.join().unwrap();
        assert_eq!(hub.open(), 0);
    }
}
