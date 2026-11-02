//! Grail's GDB remote stub (`grail --gdb ADDR`): a machine debugged over TCP with the
//! GDB remote serial protocol, by GDB, LLDB or merlin-dbg (spec section 7,
//! "Debugging"; `docs/merlin-dbg.md`).
//!
//! The pieces: [`packet`] frames the bytes, [`commands`] says what each packet does to
//! the machine ([`crate::debug`]), [`target`] describes the registers. [`serve`] ties
//! them to a socket: it runs the machine on the calling thread, and a reader thread
//! turns what the client sends into events, so a Ctrl-C reaches a running machine.
//!
//! One client at a time. A client that detaches (`D`) or disconnects leaves the machine
//! running without breakpoints, and the next one to connect stops it. `k` stops Grail.

pub mod commands;
pub mod packet;
pub mod target;

use crate::cpu::Cpu;
use crate::Machine;
use commands::{Action, StopReason, Stub};
use packet::{Decoder, Event};
use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, TryRecvError};
use std::time::Duration;

/// Steps the machine runs between two looks at the client (for a Ctrl-C) or the
/// listener (for a new client).
const SLICE: u64 = 20_000;
/// The longest a real-time machine sleeps between two looks.
const MAX_WAIT: Duration = Duration::from_millis(10);

/// How a debugged run ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// The program stopped the machine with this exit code.
    Exited(u32),
    /// A client killed it (`k`).
    Killed,
}

/// Runs `m` under the stub until the program exits or a client kills it. With `wait`,
/// the machine does not start until a client has connected and resumed it (it is
/// stopped at its first instruction); without, it runs from the start and stops when a
/// client connects. `trace` is called before each hart's turn, as [`Machine::run_for`]'s.
pub fn serve(
    m: &mut Machine,
    listener: TcpListener,
    wait: bool,
    trace: &mut impl FnMut(&Cpu),
) -> io::Result<Outcome> {
    let mut client = if wait {
        let (stream, _) = listener.accept()?;
        Some(stream)
    } else {
        None
    };
    listener.set_nonblocking(true)?;
    loop {
        match client.take() {
            Some(stream) => match Session::new(m, stream)?.run(m, trace) {
                Ended::Exited(code) => return Ok(Outcome::Exited(code)),
                Ended::Killed => return Ok(Outcome::Killed),
                // Free to run on its own again.
                Ended::Detached => m.clear_debug_points(),
            },
            None => {
                if let Some(code) = m.exit_code() {
                    return Ok(Outcome::Exited(code));
                }
                m.debug_continue(SLICE, trace);
                if let Some(d) = m.pace_delay() {
                    std::thread::sleep(d.min(MAX_WAIT));
                }
                match listener.accept() {
                    Ok((stream, _)) => client = Some(stream),
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
                    Err(e) => return Err(e),
                }
            }
        }
    }
}

/// How a client's session ended.
enum Ended {
    Exited(u32),
    Killed,
    /// `D`, or the client went away.
    Detached,
}

/// One client: the stub's state, the socket to write to, and the events from the
/// reader thread.
struct Session {
    stub: Stub,
    stream: TcpStream,
    events: Receiver<Event>,
    /// The last packet sent, for a `-`.
    last: Vec<u8>,
}

impl Session {
    /// Starts a session with the client on `stream`, the machine stopped.
    fn new(m: &mut Machine, stream: TcpStream) -> io::Result<Session> {
        stream.set_nonblocking(false)?;
        let _ = stream.set_nodelay(true);
        let reader = stream.try_clone()?;
        let (tx, events) = mpsc::channel();
        std::thread::spawn(move || read_events(reader, tx));
        let mut stub = Stub::new(m);
        stub.stopped(m, StopReason::Interrupted);
        Ok(Session {
            stub,
            stream,
            events,
            last: Vec::new(),
        })
    }

    fn run(&mut self, m: &mut Machine, trace: &mut impl FnMut(&Cpu)) -> Ended {
        let ended = self.serve(m, trace).unwrap_or(Ended::Detached);
        let _ = self.stream.shutdown(std::net::Shutdown::Both);
        ended
    }

    /// The session's loop. An error writing to the client ends it, as a detach.
    fn serve(&mut self, m: &mut Machine, trace: &mut impl FnMut(&Cpu)) -> io::Result<Ended> {
        // Stopped: wait for a packet.
        loop {
            let Ok(event) = self.events.recv() else {
                return Ok(Ended::Detached);
            };
            let payload = match event {
                Event::Packet(p) => p,
                Event::Bad => {
                    if !self.stub.no_ack() {
                        self.stream.write_all(b"-")?;
                    }
                    continue;
                }
                Event::Nack => {
                    let last = self.last.clone();
                    self.stream.write_all(&last)?;
                    continue;
                }
                // A Ctrl-C while stopped has nothing to stop.
                Event::Ack | Event::Interrupt => continue,
            };
            if !self.stub.no_ack() {
                self.stream.write_all(b"+")?;
            }
            let reason = match self.stub.handle(m, &payload) {
                Action::Reply(r) => {
                    self.send(&r)?;
                    continue;
                }
                Action::Kill => return Ok(Ended::Killed),
                Action::Detach => {
                    self.send(b"OK")?;
                    return Ok(Ended::Detached);
                }
                Action::Step => StopReason::Machine(m.debug_step(trace)),
                Action::Continue => match self.run_until_stopped(m, trace) {
                    Some(reason) => reason,
                    None => return Ok(Ended::Detached),
                },
            };
            let r = self.stub.stopped(m, reason);
            self.send(&r)?;
            if let StopReason::Machine(crate::debug::Stop::Exited(code)) = reason {
                return Ok(Ended::Exited(code));
            }
        }
    }

    /// Runs the machine until it stops or the client interrupts it. `None` if the
    /// client went away meanwhile.
    fn run_until_stopped(&mut self, m: &mut Machine, trace: &mut impl FnMut(&Cpu)) -> Option<StopReason> {
        loop {
            if let Some(stop) = m.debug_continue(SLICE, trace) {
                return Some(StopReason::Machine(stop));
            }
            let event = match m.pace_delay() {
                Some(d) => match self.events.recv_timeout(d.min(MAX_WAIT)) {
                    Ok(e) => Some(e),
                    Err(RecvTimeoutError::Timeout) => None,
                    Err(RecvTimeoutError::Disconnected) => return None,
                },
                None => match self.events.try_recv() {
                    Ok(e) => Some(e),
                    Err(TryRecvError::Empty) => None,
                    Err(TryRecvError::Disconnected) => return None,
                },
            };
            // While the machine runs, a client sends nothing but a Ctrl-C (all-stop
            // mode); anything else waits for the stop, dropped.
            if event == Some(Event::Interrupt) {
                return Some(StopReason::Interrupted);
            }
        }
    }

    fn send(&mut self, payload: &[u8]) -> io::Result<()> {
        self.last = packet::frame(payload);
        self.stream.write_all(&self.last)
    }
}

/// The reader thread: decodes what the client sends until it closes the connection,
/// or the session drops the receiver.
fn read_events(mut stream: TcpStream, tx: mpsc::Sender<Event>) {
    let mut decoder = Decoder::default();
    let mut buf = [0u8; 4096];
    loop {
        let n = match stream.read(&mut buf) {
            Ok(0) | Err(_) => return,
            Ok(n) => n,
        };
        for &b in &buf[..n] {
            if let Some(e) = decoder.push(b) {
                if tx.send(e).is_err() {
                    return;
                }
            }
        }
    }
}
