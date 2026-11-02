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
//! attaching starts at 0. A tap keeps the last [`KEEP`] bytes; a page that falls
//! further behind gets what is left (and knows, since the reply starts later than it
//! asked). Each tap has a number of its own; a page that asks with another tap's
//! number (Pendragon restarted, or the tap was closed for being idle) starts again
//! from the oldest byte, with `reset` set so it clears what it shows.
//!
//! Every VM has at most one tap, shared by every page showing it, and a tap nobody
//! has asked about for [`IDLE`] is closed. Input from the page goes to the VM's UART
//! as `ConsoleInput` over the same session.

use avalon_client::{Client, Timeouts};
use avalon_proto::{Connection, Message, VmRef};
use std::collections::{HashMap, VecDeque};
use std::net::{Shutdown, TcpStream};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

/// Output kept per tap: as much as Avalon's scrollback.
pub const KEEP: usize = 64 * 1024;

/// A tap nobody has read or written for this long is closed.
pub const IDLE: Duration = Duration::from_secs(60);

/// Most taps at once (each is a connection and a thread).
pub const MAX_TAPS: usize = 32;

/// Most bytes of input in one `ConsoleInput`.
const INPUT_CHUNK: usize = 4096;

pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// One session attached to one VM's console.
pub struct Tap {
    /// This tap's number, unique while Pendragon runs.
    pub id: u64,
    /// The VM's id.
    pub vm: u32,
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
        Chunk {
            tap: self.id,
            from,
            next: from + data.len() as u64,
            data,
            reset,
            ended: out.ended.clone(),
        }
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
        lock(&self.last_used).elapsed() >= IDLE
    }

    fn is_ended(&self) -> bool {
        lock(&self.state).ended.is_some()
    }

    /// Ends the session; the reading thread sees the socket close and stops.
    fn close(&self) {
        let _ = self.socket.shutdown(Shutdown::Both);
    }

    /// The reading thread: output into the buffer until the session ends.
    fn pump(self: Arc<Self>, mut conn: Connection<TcpStream>) {
        let why = loop {
            match conn.recv() {
                Ok(Message::ConsoleOutput { vm, data }) if vm == self.vm => {
                    let mut out = lock(&self.state);
                    out.buf.extend(data);
                    let excess = out.buf.len().saturating_sub(KEEP);
                    out.buf.drain(..excess);
                    out.start += excess as u64;
                    drop(out);
                    self.changed.notify_all();
                }
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
        lock(&self.state).ended = Some(why);
        self.changed.notify_all();
    }
}

/// Every open tap, by VM id.
pub struct Hub {
    avalon: String,
    timeouts: Timeouts,
    taps: Mutex<HashMap<u32, Arc<Tap>>>,
    next_id: AtomicU64,
}

impl Hub {
    /// Taps on the Avalon server at `avalon`; `timeouts` limit attaching.
    pub fn new(avalon: &str, timeouts: Timeouts) -> Hub {
        Hub {
            avalon: avalon.to_string(),
            timeouts,
            taps: Mutex::new(HashMap::new()),
            // Start somewhere a page from an earlier run is unlikely to have.
            next_id: AtomicU64::new(1 + (std::process::id() as u64) * 1000),
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
        let mut buf = VecDeque::from(scrollback);
        let excess = buf.len().saturating_sub(KEEP);
        buf.drain(..excess);
        let tap = Arc::new(Tap {
            id: self.next_id.fetch_add(1, Ordering::Relaxed),
            vm: id,
            state: Mutex::new(Output {
                buf,
                start: excess as u64,
                ended: None,
            }),
            changed: Condvar::new(),
            input: Mutex::new(input),
            socket,
            last_used: Mutex::new(Instant::now()),
        });

        let mut taps = lock(&self.taps);
        // Another request may have attached meanwhile: keep the first.
        if let Some(existing) = taps.get(&id) {
            if !existing.is_ended() {
                tap.close();
                return Ok(Arc::clone(existing));
            }
        }
        if taps.len() >= MAX_TAPS {
            tap.close();
            return Err(avalon_client::Error::Request(avalon_proto::Error::Remote {
                code: avalon_proto::ErrorCode::OverBudget,
                message: format!("Pendragon has {MAX_TAPS} consoles open already; try again in a minute"),
            }));
        }
        taps.insert(id, Arc::clone(&tap));
        drop(taps);
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

    /// Closes every tap (at shutdown).
    pub fn close_all(&self) {
        for (_, tap) in lock(&self.taps).drain() {
            tap.close();
        }
    }
}
