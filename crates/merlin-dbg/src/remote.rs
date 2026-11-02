//! The client side of the GDB remote serial protocol: what merlin-dbg says to a stub
//! (Grail's, `grail --gdb`) to read and change a stopped machine, and to run it until it
//! stops. The framing is Grail's own ([`grail::gdb::packet`]).

use grail::gdb::packet::{self, Decoder, Event};
use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// GDB's register numbers (see `grail::gdb::target`).
pub const PC: usize = 32;
pub const PRIV: usize = 65 + 4096;
pub fn csr(addr: u16) -> usize {
    65 + addr as usize
}

/// What a watchpoint watches for, as `Z2`-`Z4` say it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Watch {
    Write,
    Read,
    Access,
}

impl Watch {
    fn z_type(self) -> u8 {
        match self {
            Watch::Write => 2,
            Watch::Read => 3,
            Watch::Access => 4,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Watch::Write => "write",
            Watch::Read => "read",
            Watch::Access => "access",
        }
    }
}

/// Why the machine stopped, from a stop reply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopEvent {
    /// `T`: a signal (5 SIGTRAP for breakpoints, steps and watchpoints, 2 SIGINT for an
    /// interrupt), the hart, and a watchpoint's kind and address if one was hit.
    Stopped {
        signal: u8,
        hart: usize,
        watch: Option<(Watch, u32)>,
    },
    /// `W`: the program stopped the machine with this exit code (`X`: killed).
    Exited(u32),
}

/// A connection to a stub.
pub struct Remote {
    stream: TcpStream,
    decoder: Decoder,
    /// Events read past the one wanted.
    events: VecDeque<Event>,
    no_ack: bool,
    /// How many harts the machine has (its threads).
    pub harts: usize,
}

/// An error reply (`Exx`) or a failed connection.
pub type Result<T> = std::result::Result<T, String>;

impl Remote {
    /// Connects to the stub at `addr` and agrees on the protocol: `swbreak` stop
    /// reasons, no acks if the stub can, and the list of harts.
    pub fn connect(addr: SocketAddr) -> Result<Remote> {
        let stream = TcpStream::connect(addr).map_err(|e| format!("cannot connect to {addr}: {e}"))?;
        let _ = stream.set_nodelay(true);
        let mut r = Remote {
            stream,
            decoder: Decoder::default(),
            events: VecDeque::new(),
            no_ack: false,
            harts: 1,
        };
        let features = r.request("qSupported:swbreak+")?;
        if features.split(';').any(|f| f == "QStartNoAckMode+") && r.request("QStartNoAckMode")? == "OK" {
            r.no_ack = true;
        }
        let threads = r.request("qfThreadInfo")?;
        r.harts = threads
            .strip_prefix('m')
            .map_or(1, |list| list.split(',').count());
        Ok(r)
    }

    /// Sends a packet and returns the reply's payload. `E` replies are errors; an empty
    /// one means the stub does not support the packet, which is an error too.
    pub fn request(&mut self, payload: &str) -> Result<String> {
        self.send(payload.as_bytes()).map_err(lost)?;
        let reply = self.recv(None).map_err(lost)?;
        if reply.is_empty() {
            return Err(format!("the stub does not support '{}'", short(payload)));
        }
        if reply.len() == 3 && reply.starts_with('E') {
            return Err(format!("the stub refused '{}' ({reply})", short(payload)));
        }
        Ok(reply)
    }

    fn send(&mut self, payload: &[u8]) -> io::Result<()> {
        let frame = packet::frame(payload);
        loop {
            self.stream.write_all(&frame)?;
            if self.no_ack {
                return Ok(());
            }
            loop {
                match self.next_event()? {
                    Event::Ack => return Ok(()),
                    Event::Nack => break,
                    other => self.events.push_back(other),
                }
            }
        }
    }

    /// The next packet's payload. With `interrupt`, waits in short slices and sends a
    /// Ctrl-C (0x03) once the flag is set.
    fn recv(&mut self, interrupt: Option<&AtomicBool>) -> io::Result<String> {
        let mut sent = false;
        loop {
            let event = match self.events.pop_front() {
                Some(e) => e,
                None => {
                    if let Some(flag) = interrupt {
                        if !sent && flag.swap(false, Ordering::SeqCst) {
                            self.stream.write_all(&[0x03])?;
                            sent = true;
                        }
                        match self.poll_event(Duration::from_millis(50))? {
                            Some(e) => e,
                            None => continue,
                        }
                    } else {
                        self.next_event()?
                    }
                }
            };
            match event {
                Event::Packet(p) => {
                    if !self.no_ack {
                        self.stream.write_all(b"+")?;
                    }
                    return Ok(String::from_utf8_lossy(&p).into_owned());
                }
                Event::Bad if !self.no_ack => self.stream.write_all(b"-")?,
                _ => {}
            }
        }
    }

    fn next_event(&mut self) -> io::Result<Event> {
        loop {
            if let Some(e) = self.poll_event(Duration::ZERO)? {
                return Ok(e);
            }
        }
    }

    /// The next event, reading for at most `timeout` (forever if zero).
    fn poll_event(&mut self, timeout: Duration) -> io::Result<Option<Event>> {
        self.stream
            .set_read_timeout((!timeout.is_zero()).then_some(timeout))?;
        let mut buf = [0u8; 4096];
        loop {
            let n = match self.stream.read(&mut buf) {
                Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
                Ok(n) => n,
                Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => {
                    return Ok(None)
                }
                Err(e) => return Err(e),
            };
            for &b in &buf[..n] {
                if let Some(e) = self.decoder.push(b) {
                    self.events.push_back(e);
                }
            }
            if let Some(e) = self.events.pop_front() {
                return Ok(Some(e));
            }
        }
    }

    /// The selected hart's `x0`-`x31` and `pc`.
    pub fn registers(&mut self) -> Result<[u32; 33]> {
        let g = self.request("g")?;
        let bytes = unhex(&g).ok_or("a malformed register packet")?;
        let mut regs = [0u32; 33];
        for (r, w) in regs.iter_mut().zip(bytes.as_chunks::<4>().0) {
            *r = u32::from_le_bytes(*w);
        }
        Ok(regs)
    }

    /// Register `n` (GDB's numbering) of the selected hart.
    pub fn register(&mut self, n: usize) -> Result<u32> {
        let p = self.request(&format!("p{n:x}"))?;
        match unhex(&p).as_deref() {
            Some(&[a, b, c, d]) => Ok(u32::from_le_bytes([a, b, c, d])),
            _ => Err("a malformed register value".into()),
        }
    }

    pub fn set_register(&mut self, n: usize, v: u32) -> Result<()> {
        self.request(&format!("P{n:x}={}", hex(&v.to_le_bytes())))
            .map(drop)
    }

    /// Up to `len` bytes at `addr`, through the selected hart's translation: fewer when
    /// the memory ends, an error when not even the first byte can be read.
    pub fn read_memory(&mut self, addr: u32, len: usize) -> Result<Vec<u8>> {
        let mut out = Vec::with_capacity(len);
        while out.len() < len {
            let at = addr.wrapping_add(out.len() as u32);
            let want = (len - out.len()).min(0x800);
            let reply = match self.request(&format!("m{at:x},{want:x}")) {
                Ok(r) => r,
                Err(e) if out.is_empty() => return Err(format!("cannot read memory at {at:#x}: {e}")),
                Err(_) => break,
            };
            let bytes = unhex(&reply).ok_or("a malformed memory packet")?;
            let short = bytes.len() < want;
            out.extend(bytes);
            if short {
                break;
            }
        }
        Ok(out)
    }

    /// A word of memory, if it can be read.
    pub fn read_word(&mut self, addr: u32) -> Option<u32> {
        let b = self.read_memory(addr, 4).ok()?;
        (b.len() == 4).then(|| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    /// Selects the hart the register and memory packets use.
    pub fn select(&mut self, hart: usize) -> Result<()> {
        self.request(&format!("Hg{:x}", hart + 1)).map(drop)
    }

    /// The selected hart, from the stub.
    pub fn selected(&mut self) -> Result<usize> {
        let qc = self.request("qC")?;
        let t = qc
            .strip_prefix("QC")
            .and_then(|t| usize::from_str_radix(t, 16).ok());
        t.filter(|&t| t > 0)
            .map(|t| t - 1)
            .ok_or("a malformed qC reply".into())
    }

    /// The stub's description of a hart (`qThreadExtraInfo`).
    pub fn hart_info(&mut self, hart: usize) -> Result<String> {
        let r = self.request(&format!("qThreadExtraInfo,{:x}", hart + 1))?;
        let bytes = unhex(&r).ok_or("a malformed thread description")?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }

    pub fn set_breakpoint(&mut self, addr: u32) -> Result<()> {
        self.request(&format!("Z0,{addr:x},4")).map(drop)
    }

    pub fn clear_breakpoint(&mut self, addr: u32) -> Result<()> {
        self.request(&format!("z0,{addr:x},4")).map(drop)
    }

    pub fn set_watchpoint(&mut self, kind: Watch, addr: u32, len: u32) -> Result<()> {
        self.request(&format!("Z{},{addr:x},{len:x}", kind.z_type()))
            .map(drop)
    }

    pub fn clear_watchpoint(&mut self, kind: Watch, addr: u32, len: u32) -> Result<()> {
        self.request(&format!("z{},{addr:x},{len:x}", kind.z_type()))
            .map(drop)
    }

    /// Continues the machine (every hart), or steps `step`'s hart, and waits until it
    /// stops. Setting `interrupt` (a Ctrl-C) meanwhile asks the stub to stop it.
    pub fn resume(&mut self, step: Option<usize>, interrupt: &AtomicBool) -> Result<StopEvent> {
        let packet = match step {
            Some(h) => format!("vCont;s:{:x}", h + 1),
            None => "vCont;c".to_string(),
        };
        self.send(packet.as_bytes()).map_err(lost)?;
        let reply = self.recv(Some(interrupt)).map_err(lost)?;
        parse_stop(&reply).ok_or_else(|| format!("a malformed stop reply '{reply}'"))
    }

    /// Ends the machine (`k`): there is no reply.
    pub fn kill(&mut self) {
        let _ = self.send(b"k");
    }

    /// Leaves the machine running on its own, without breakpoints (`D`).
    pub fn detach(&mut self) -> Result<()> {
        self.request("D").map(drop)
    }
}

fn lost(e: io::Error) -> String {
    format!("lost the connection to the stub: {e}")
}

/// A packet, cut short for an error message.
fn short(p: &str) -> String {
    p.chars().take(40).collect()
}

/// Parses a stop reply: `T05thread:1;...`, `T05watch:addr;thread:2;`, `W00`, `X09`.
pub fn parse_stop(reply: &str) -> Option<StopEvent> {
    let kind = reply.chars().next()?;
    let code = u32::from_str_radix(reply.get(1..3)?, 16).ok()?;
    match kind {
        'W' | 'X' => Some(StopEvent::Exited(code)),
        'T' | 'S' => {
            let mut hart = 0;
            let mut watch = None;
            for field in reply[3..].split(';') {
                let Some((key, value)) = field.split_once(':') else {
                    continue;
                };
                let kind = match key {
                    "thread" => {
                        let t = usize::from_str_radix(value, 16).ok()?;
                        hart = t.checked_sub(1)?;
                        continue;
                    }
                    "watch" => Watch::Write,
                    "rwatch" => Watch::Read,
                    "awatch" => Watch::Access,
                    _ => continue,
                };
                watch = Some((kind, u32::from_str_radix(value, 16).ok()?));
            }
            Some(StopEvent::Stopped {
                signal: code as u8,
                hart,
                watch,
            })
        }
        _ => None,
    }
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn unhex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stop_replies() {
        assert_eq!(parse_stop("W00"), Some(StopEvent::Exited(0)));
        assert_eq!(parse_stop("X09"), Some(StopEvent::Exited(9)));
        assert_eq!(
            parse_stop("T05swbreak:;thread:2;20:00100000;"),
            Some(StopEvent::Stopped {
                signal: 5,
                hart: 1,
                watch: None
            })
        );
        assert_eq!(
            parse_stop("T05awatch:80000430;thread:1;"),
            Some(StopEvent::Stopped {
                signal: 5,
                hart: 0,
                watch: Some((Watch::Access, 0x8000_0430))
            })
        );
        assert_eq!(
            parse_stop("T02"),
            Some(StopEvent::Stopped {
                signal: 2,
                hart: 0,
                watch: None
            })
        );
        assert_eq!(parse_stop("T05thread:0;"), None);
        assert_eq!(parse_stop("OK"), None);
        assert_eq!(parse_stop(""), None);
        assert_eq!(parse_stop("W"), None);
    }

    #[test]
    fn hex_round_trips() {
        assert_eq!(hex(&[0, 0xab, 0x10]), "00ab10");
        assert_eq!(unhex("00ab10"), Some(vec![0, 0xab, 0x10]));
        assert_eq!(unhex("0"), None);
        assert_eq!(unhex("zz"), None);
    }
}
