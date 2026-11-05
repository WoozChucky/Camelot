//! Grail's GDB remote stub (`grail --gdb ADDR`): a machine debugged over TCP with the
//! GDB remote serial protocol, by GDB, LLDB or merlin-dbg (spec section 7,
//! "Debugging"; `docs/merlin-dbg.md`).
//!
//! The pieces: [`packet`] frames the bytes, [`commands`] says what each packet does to
//! the machine ([`crate::debug`]), [`target`] describes the registers, and [`Session`]
//! is a client's conversation with the stub, whatever carries its bytes. [`serve`] ties
//! them to a socket: it runs the machine on the calling thread, and a reader thread
//! turns what the client sends into events, so a Ctrl-C reaches a running machine.
//! Avalon carries a [`Session`] over its own connection instead
//! (`docs/avalon-protocol.md`, section 6.8).
//!
//! One client at a time, and the machine is stopped while one connects, as QEMU's stub
//! stops a VM for a new GDB connection. A client that detaches (`D`) leaves the machine
//! running without its breakpoints and watchpoints. One whose connection ends without
//! `D` leaves the machine as it was, as QEMU does: stopped, it stays stopped until the
//! next client resumes it; running, it runs on, and the breakpoints still stop it (the
//! next client's `?` removes them). `k` stops Grail.

pub mod commands;
pub mod packet;
pub mod target;

use crate::cpu::Cpu;
use crate::Machine;
pub use commands::StopReason;
use commands::{Action, Stub};
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
    // Whether the machine is stopped with no client: one whose connection ended left it
    // stopped, or a breakpoint it left has stopped it since.
    let mut stopped = false;
    loop {
        match client.take() {
            Some(stream) => match TcpSession::new(m, stream)?.run(m, trace) {
                Ended::Exited(code) => return Ok(Outcome::Exited(code)),
                Ended::Killed => return Ok(Outcome::Killed),
                // Free to run on its own again.
                Ended::Detached => {
                    m.clear_debug_points();
                    stopped = false;
                }
                // Left as it was, its breakpoints too.
                Ended::Dropped { running } => stopped = !running,
            },
            None if stopped => {
                // Nothing runs until a client comes.
                listener.set_nonblocking(false)?;
                let (stream, _) = listener.accept()?;
                listener.set_nonblocking(true)?;
                client = Some(stream);
            }
            None => {
                if let Some(code) = m.exit_code() {
                    return Ok(Outcome::Exited(code));
                }
                if let Some(stop) = m.debug_continue(SLICE, trace) {
                    // A breakpoint or watchpoint the last client left: the machine stops,
                    // as a QEMU VM stops in its run state debug with no debugger to tell.
                    stopped = !matches!(stop, crate::debug::Stop::Exited(_));
                    continue;
                }
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

/// A client's session with the stub, whatever carries its bytes: what to send back for
/// each [`Event`] the client's bytes make, and what the host does with the machine next.
/// [`serve`] runs one over TCP; Avalon runs one for each VM's debugger.
///
/// The host owns the machine and runs it. While it is stopped, the host passes every
/// event to [`Session::event`]. On [`Next::Step`] it runs one step with
/// [`Session::step`], which writes the stop reply. On [`Next::Continue`] it runs the machine with
/// [`Machine::debug_continue`] until that stops it or the client sends a Ctrl-C
/// ([`Session::interrupts`]; anything else the client sends meanwhile is dropped, as in
/// GDB's all-stop mode), and gives the reason to [`Session::stopped`], which writes the
/// stop reply.
pub struct Session {
    stub: Stub,
    /// The last packet sent, for a `-`.
    last: Vec<u8>,
}

/// What the host does after an event, once it has sent what [`Session::event`] wrote.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Next {
    /// Nothing: the machine stays stopped until the client's next event.
    Wait,
    /// Run the machine until it stops or the client interrupts it, then call
    /// [`Session::stopped`].
    Continue,
    /// Run one instruction of the selected hart with [`Session::step`] (`s`, `vCont;s`).
    /// A host with run states has the machine running for it, as QEMU's is for a step,
    /// and stopped again after it (debug).
    Step,
    /// The client killed the machine (`k`, `vKill`); `X09` was written.
    Kill,
    /// The client detached (`D`); `OK` was written. Its breakpoints and watchpoints are
    /// the host's to clear ([`Machine::clear_debug_points`]), and the machine runs on.
    Detach,
    /// A step reached the program's exit: the stop reply (`W`) was written, and the
    /// session is over.
    Exited(u32),
}

impl Session {
    /// A session with a new client, the machine stopped (as for an interrupt: its stop
    /// reply is `T02`).
    pub fn new(m: &mut Machine) -> Session {
        let mut stub = Stub::new();
        stub.stopped(m, StopReason::Interrupted);
        Session {
            stub,
            last: Vec::new(),
        }
    }

    /// An event from the client while the machine is stopped: appends what to send to
    /// `out` (an ack, a reply, a packet sent again) and says what to do next.
    pub fn event(&mut self, m: &mut Machine, event: Event, out: &mut Vec<u8>) -> Next {
        let payload = match event {
            Event::Packet(p) => p,
            Event::Bad => {
                if !self.stub.no_ack() {
                    out.push(b'-');
                }
                return Next::Wait;
            }
            Event::Nack => {
                out.extend_from_slice(&self.last);
                return Next::Wait;
            }
            // A Ctrl-C while stopped has nothing to stop.
            Event::Ack | Event::Interrupt => return Next::Wait,
        };
        if !self.stub.no_ack() {
            out.push(b'+');
        }
        match self.stub.handle(m, &payload) {
            Action::Reply(r) => {
                self.send(&r, out);
                Next::Wait
            }
            Action::Kill => {
                // GDB expects no reply to `k`; LLDB waits for one. `X09`, "ended by
                // SIGKILL", satisfies LLDB, and GDB never reads it.
                self.send(b"X09", out);
                Next::Kill
            }
            Action::Detach => {
                self.send(b"OK", out);
                Next::Detach
            }
            Action::Step => Next::Step,
            Action::Continue => Next::Continue,
        }
    }

    /// Runs the step [`Next::Step`] asked for and appends its stop reply to `out`. The
    /// program's exit code if the step reached the exit (the reply is `W`), which ends
    /// the session. `trace` is called before each hart's turn.
    pub fn step(&mut self, m: &mut Machine, trace: &mut impl FnMut(&Cpu), out: &mut Vec<u8>) -> Option<u32> {
        let stop = m.debug_step(trace);
        self.stopped(m, StopReason::Machine(stop), out)
    }

    /// Whether `event`, from a client whose machine is running, stops it: a Ctrl-C.
    pub fn interrupts(event: &Event) -> bool {
        *event == Event::Interrupt
    }

    /// The machine stopped for `reason`: appends the stop reply to `out`. The program's
    /// exit code if it exited, which ends the session.
    pub fn stopped(&mut self, m: &mut Machine, reason: StopReason, out: &mut Vec<u8>) -> Option<u32> {
        let r = self.stub.stopped(m, reason);
        self.send(&r, out);
        match reason {
            StopReason::Machine(crate::debug::Stop::Exited(code)) => Some(code),
            _ => None,
        }
    }

    /// The host ended the machine while the client waited for a stop reply: appends
    /// `X09` (ended by SIGKILL), as for `k`.
    pub fn killed(&mut self, out: &mut Vec<u8>) {
        self.send(b"X09", out);
    }

    fn send(&mut self, payload: &[u8], out: &mut Vec<u8>) {
        self.last = packet::frame(payload);
        out.extend_from_slice(&self.last);
    }
}

/// How a client's session over TCP ended.
enum Ended {
    Exited(u32),
    Killed,
    /// `D`.
    Detached,
    /// The connection ended without `D` (or writing to it failed), the machine running
    /// for the client or stopped.
    Dropped {
        running: bool,
    },
}

/// One client over TCP: its [`Session`], the socket to write to, and the events from the
/// reader thread.
struct TcpSession {
    session: Session,
    stream: TcpStream,
    events: Receiver<Event>,
}

impl TcpSession {
    /// Starts a session with the client on `stream`, the machine stopped.
    fn new(m: &mut Machine, stream: TcpStream) -> io::Result<TcpSession> {
        stream.set_nonblocking(false)?;
        let _ = stream.set_nodelay(true);
        let reader = stream.try_clone()?;
        let (tx, events) = mpsc::channel();
        std::thread::spawn(move || read_events(reader, tx));
        Ok(TcpSession {
            session: Session::new(m),
            stream,
            events,
        })
    }

    fn run(&mut self, m: &mut Machine, trace: &mut impl FnMut(&Cpu)) -> Ended {
        let ended = self.serve(m, trace);
        let _ = self.stream.shutdown(std::net::Shutdown::Both);
        ended
    }

    /// The session's loop. An error writing to the client ends it as its connection
    /// ending does.
    fn serve(&mut self, m: &mut Machine, trace: &mut impl FnMut(&Cpu)) -> Ended {
        const STOPPED: Ended = Ended::Dropped { running: false };
        // Stopped: wait for a packet.
        loop {
            let Ok(event) = self.events.recv() else {
                return STOPPED;
            };
            let mut out = Vec::new();
            let next = self.session.event(m, event, &mut out);
            let written = self.stream.write_all(&out);
            let exited = match next {
                // GDB never reads the reply to `k`, and may be gone already.
                Next::Kill => return Ended::Killed,
                _ if written.is_err() => return STOPPED,
                Next::Wait => continue,
                Next::Detach => return Ended::Detached,
                Next::Exited(code) => return Ended::Exited(code),
                Next::Step => {
                    let mut out = Vec::new();
                    let exited = self.session.step(m, trace, &mut out);
                    (exited, self.stream.write_all(&out))
                }
                Next::Continue => {
                    let Some(reason) = self.run_until_stopped(m, trace) else {
                        // Gone while the machine ran for it: it runs on.
                        return Ended::Dropped { running: true };
                    };
                    let mut out = Vec::new();
                    let exited = self.session.stopped(m, reason, &mut out);
                    (exited, self.stream.write_all(&out))
                }
            };
            match exited {
                (Some(code), _) => return Ended::Exited(code),
                (None, Err(_)) => return STOPPED,
                (None, Ok(())) => {}
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
            if event.as_ref().is_some_and(Session::interrupts) {
                return Some(StopReason::Interrupted);
            }
        }
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
