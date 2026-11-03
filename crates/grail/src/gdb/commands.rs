//! What the stub does with each packet: the subset of the GDB remote protocol Grail
//! implements (`docs/merlin-dbg.md`, "The GDB stub"). Nothing here touches the
//! network: [`Stub::handle`] takes a packet's payload and the machine, and says what to
//! answer or do, so every command is tested without a socket.
//!
//! Harts are the protocol's threads: hart `i` is thread `i + 1` (thread ids are
//! positive; 0 means "any" and -1 "all").
//!
//! A packet the stub does not know gets an empty reply, which tells the client it is
//! not supported. A known packet with arguments it cannot parse gets `E01`; a memory
//! access that fails `E14` (EFAULT); a thread or hart that does not exist `E22`
//! (EINVAL).

use super::packet;
use super::target::{self, Reg, GENERAL_REGS};
use crate::cpu::Mode;
use crate::debug::{Space, Stop, WatchKind};
use crate::Machine;

/// What to do after a packet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Send this payload.
    Reply(Vec<u8>),
    /// Resume the machine; the reply is the stop reply when it stops.
    Continue,
    /// Run one instruction of the selected hart, then send the stop reply.
    Step,
    /// Stop the machine for good (`k`, `vKill`); the session replies `X09`.
    Kill,
    /// Reply `OK`, forget the breakpoints and let the machine run on its own (`D`).
    Detach,
}

/// Why the machine is stopped, as the stub reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    /// Something the machine ran into ([`Stop`]).
    Machine(Stop),
    /// The client asked it to stop (Ctrl-C), or it was stopped for a new client.
    Interrupted,
}

/// The most memory one `m` packet reads: its reply, two hex digits a byte, stays well
/// within the packet size the stub announces.
const MAX_READ: usize = 0x1000;
/// `PacketSize` in `qSupported`, in bytes (hex in the reply).
const PACKET_SIZE: usize = 0x4000;

pub struct Stub {
    /// The client asked for no-ack mode (`QStartNoAckMode`).
    no_ack: bool,
    /// The client understands the `swbreak` stop reason (`qSupported`).
    swbreak: bool,
    /// The target description, made once.
    xml: String,
    /// The hart `c` and `s` act on, from `Hc`; `None` is "any" (the selected hart).
    resume_hart: Option<usize>,
    last_stop: StopReason,
}

impl Stub {
    /// A stub for `m`, which is stopped.
    pub fn new(m: &Machine) -> Stub {
        Stub {
            no_ack: false,
            swbreak: false,
            xml: target::xml(m),
            resume_hart: None,
            last_stop: StopReason::Interrupted,
        }
    }

    /// Whether the client and the stub agreed to stop acknowledging packets.
    pub fn no_ack(&self) -> bool {
        self.no_ack
    }

    /// Records that the machine stopped for `reason` and returns the stop reply. The
    /// hart that stopped it becomes the selected one, as GDB expects.
    pub fn stopped(&mut self, m: &mut Machine, reason: StopReason) -> Vec<u8> {
        let hart = match reason {
            StopReason::Machine(Stop::Breakpoint { hart, .. } | Stop::Stepped { hart }) => Some(hart),
            StopReason::Machine(Stop::Watchpoint(hit)) => Some(hit.hart),
            StopReason::Machine(Stop::Exited(_)) => None,
            StopReason::Interrupted => {
                m.debug_pause();
                None
            }
        };
        if let Some(hart) = hart {
            let _ = m.select_hart(hart);
        }
        self.last_stop = reason;
        self.stop_reply(m)
    }

    /// The reply that reports the last stop: `T05` (SIGTRAP) with the thread and, for a
    /// watchpoint, which kind and the address; `T02` (SIGINT) for an interrupt; `W` and
    /// the exit code for a program that stopped the machine.
    fn stop_reply(&self, m: &Machine) -> Vec<u8> {
        let (signal, why) = match self.last_stop {
            StopReason::Machine(Stop::Exited(code)) => return format!("W{:02x}", code & 0xff).into_bytes(),
            StopReason::Machine(Stop::Breakpoint { .. }) if self.swbreak => (5, "swbreak:;".to_string()),
            StopReason::Machine(Stop::Watchpoint(hit)) => {
                let kind = match hit.kind {
                    WatchKind::Write => "watch",
                    WatchKind::Read => "rwatch",
                    WatchKind::Access => "awatch",
                };
                (5, format!("{kind}:{:x};", hit.addr))
            }
            StopReason::Machine(_) => (5, String::new()),
            StopReason::Interrupted => (2, String::new()),
        };
        // The pc comes along (register 0x20), saving the client a round trip.
        let pc = hex_word(m.pc());
        format!("T{signal:02x}{why}thread:{:x};20:{pc};", m.selected_hart() + 1).into_bytes()
    }

    /// Handles one packet's payload (unescaped by nobody yet: only `X` carries binary).
    pub fn handle(&mut self, m: &mut Machine, payload: &[u8]) -> Action {
        let Some((&cmd, args)) = payload.split_first() else {
            return reply("");
        };
        let result = match cmd {
            b'?' => Ok(Action::Reply(self.stop_reply(m))),
            b'g' => Ok(read_general(m)),
            b'G' => write_general(m, args),
            b'p' => read_register(m, args),
            b'P' => write_register(m, args),
            b'm' => read_memory(m, args),
            b'M' => write_memory(m, args, false),
            b'X' => write_memory(m, args, true),
            b'c' | b'C' | b's' | b'S' => self.resume(m, cmd, args),
            b'Z' | b'z' => point(m, cmd == b'Z', args),
            b'H' => self.set_thread(m, args),
            b'T' => thread(m, args).map(|_| reply("OK")),
            b'k' => Ok(Action::Kill),
            b'D' => Ok(Action::Detach),
            b'v' => self.v_packet(m, args),
            b'q' | b'Q' => Ok(self.query(m, payload)),
            _ => Ok(reply("")),
        };
        result.unwrap_or_else(Action::Reply)
    }

    /// `c [addr]`, `C sig[;addr]`, `s [addr]`, `S sig[;addr]`: resume at `addr` if given
    /// (the signal is ignored: there are no signals to deliver).
    fn resume(&mut self, m: &mut Machine, cmd: u8, args: &[u8]) -> Result<Action, Vec<u8>> {
        let addr = if cmd.is_ascii_uppercase() {
            match args.iter().position(|&b| b == b';') {
                Some(i) => &args[i + 1..],
                None => &[][..],
            }
        } else {
            args
        };
        if let Some(h) = self.resume_hart {
            m.select_hart(h).map_err(|_| error(22))?;
        }
        if !addr.is_empty() {
            m.set_pc(hex(addr).ok_or_else(malformed)?);
        }
        Ok(if cmd.eq_ignore_ascii_case(&b's') {
            Action::Step
        } else {
            Action::Continue
        })
    }

    /// `Hg tid` (the thread register and memory packets use) and `Hc tid` (the thread
    /// `c` and `s` resume).
    fn set_thread(&mut self, m: &mut Machine, args: &[u8]) -> Result<Action, Vec<u8>> {
        let (&op, tid) = args.split_first().ok_or_else(malformed)?;
        let hart = thread(m, tid)?;
        match op {
            b'g' => {
                if let Some(h) = hart {
                    m.select_hart(h).map_err(|_| error(22))?;
                }
            }
            b'c' => self.resume_hart = hart,
            _ => return Err(malformed()),
        }
        Ok(reply("OK"))
    }

    /// `vCont?`, `vCont;action[:tid]...`, `vKill`. Of a `vCont`'s actions, a step
    /// (`s`, `S`) steps its thread; otherwise the machine continues. Every hart runs
    /// either way: they take turns, so a step of one is a turn of each before it.
    fn v_packet(&mut self, m: &mut Machine, args: &[u8]) -> Result<Action, Vec<u8>> {
        if args == b"Cont?" {
            return Ok(reply("vCont;c;C;s;S"));
        }
        if args.starts_with(b"Kill") {
            return Ok(Action::Kill);
        }
        let Some(actions) = args.strip_prefix(b"Cont;") else {
            return Ok(reply(""));
        };
        let mut step = None;
        for action in actions.split(|&b| b == b';') {
            let (what, tid) = match action.iter().position(|&b| b == b':') {
                Some(i) => (&action[..i], Some(&action[i + 1..])),
                None => (action, None),
            };
            let hart = match tid {
                Some(tid) => thread(m, tid)?,
                None => None,
            };
            match what.first() {
                Some(b's' | b'S') if step.is_none() => step = Some(hart.unwrap_or(m.selected_hart())),
                Some(b'c' | b'C' | b's' | b'S') => {}
                _ => return Err(malformed()),
            }
        }
        Ok(match step {
            Some(h) => {
                m.select_hart(h).map_err(|_| error(22))?;
                Action::Step
            }
            None => Action::Continue,
        })
    }

    fn query(&mut self, m: &Machine, payload: &[u8]) -> Action {
        let text = String::from_utf8_lossy(payload);
        let (name, rest) = match text.find([':', ',']) {
            Some(i) => (&text[..i], &text[i + 1..]),
            None => (&text[..], ""),
        };
        match name {
            "qSupported" => {
                self.swbreak = rest.split(';').any(|f| f == "swbreak+");
                reply(&format!(
                    "PacketSize={PACKET_SIZE:x};qXfer:features:read+;QStartNoAckMode+;swbreak+;vContSupported+"
                ))
            }
            "QStartNoAckMode" => {
                self.no_ack = true;
                reply("OK")
            }
            "qXfer" => match rest.strip_prefix("features:read:target.xml:") {
                Some(range) => self.read_xml(range),
                None => reply(""),
            },
            "qfThreadInfo" => {
                let ids: Vec<String> = (1..=m.cpu.harts().len()).map(|t| format!("{t:x}")).collect();
                reply(&format!("m{}", ids.join(",")))
            }
            "qsThreadInfo" => reply("l"),
            "qC" => reply(&format!("QC{:x}", m.selected_hart() + 1)),
            "qAttached" => reply("1"),
            "qThreadExtraInfo" => match thread(m, rest.as_bytes()) {
                Ok(Some(h)) => {
                    let hart = m.cpu.hart(h);
                    let wfi = if hart.waiting { ", in wfi" } else { "" };
                    let info = format!("hart {h}, {:?}-mode{wfi}", hart.mode);
                    reply(&hex_bytes(info.as_bytes()))
                }
                _ => Action::Reply(error(22)),
            },
            // LLDB's register descriptions, one per packet. LLDB builds without XML
            // support (its Windows builds) ask for these instead of target.xml.
            _ if name.starts_with("qRegisterInfo") => match usize::from_str_radix(&name[13..], 16) {
                Ok(n) => match target::lldb_register_info(n) {
                    Some(info) => reply(&info),
                    None => Action::Reply(error(0x45)),
                },
                Err(_) => Action::Reply(malformed()),
            },
            // LLDB's: what the target is, since there is no host OS to ask.
            "qHostInfo" | "qProcessInfo" => {
                let triple = hex_bytes(b"riscv32-unknown-unknown-elf");
                let pid = if name == "qProcessInfo" { "pid:1;" } else { "" };
                reply(&format!("{pid}triple:{triple};endian:little;ptrsize:4;"))
            }
            _ => reply(""),
        }
    }

    /// `qXfer:features:read:target.xml:off,len`: `m` and a part of the description, or
    /// `l` and its last part.
    fn read_xml(&self, range: &str) -> Action {
        let Some((off, len)) = range.split_once(',') else {
            return Action::Reply(malformed());
        };
        let (Some(off), Some(len)) = (hex(off.as_bytes()), hex(len.as_bytes())) else {
            return Action::Reply(malformed());
        };
        let xml = self.xml.as_bytes();
        let start = (off as usize).min(xml.len());
        let end = start.saturating_add(len as usize).min(xml.len());
        let mut out = vec![if end == xml.len() { b'l' } else { b'm' }];
        out.extend(packet::escape(&xml[start..end]));
        Action::Reply(out)
    }
}

/// `g`: `x0`-`x31` and `pc`, each as 4 little-endian bytes in hex.
fn read_general(m: &Machine) -> Action {
    let mut s = String::with_capacity(GENERAL_REGS * 8);
    for r in 0..32 {
        s.push_str(&hex_word(m.reg(r)));
    }
    s.push_str(&hex_word(m.pc()));
    reply(&s)
}

/// `G data`: writes as many of `g`'s registers as `data` holds.
fn write_general(m: &mut Machine, args: &[u8]) -> Result<Action, Vec<u8>> {
    let bytes = unhex_bytes(args).ok_or_else(malformed)?;
    if !bytes.len().is_multiple_of(4) || bytes.len() > GENERAL_REGS * 4 {
        return Err(malformed());
    }
    for (i, w) in bytes.chunks(4).enumerate() {
        let v = u32::from_le_bytes([w[0], w[1], w[2], w[3]]);
        if i == target::PC {
            m.set_pc(v);
        } else {
            m.set_reg(i, v);
        }
    }
    Ok(reply("OK"))
}

/// `p n`: register `n` (numbered as in [`target`]).
fn read_register(m: &Machine, args: &[u8]) -> Result<Action, Vec<u8>> {
    let n = hex(args).ok_or_else(malformed)? as usize;
    let v = match Reg::from_number(n).ok_or_else(|| error(1))? {
        Reg::X(r) => m.reg(r),
        Reg::Pc => m.pc(),
        Reg::Csr(a) => m.csr(a).ok_or_else(|| error(1))?,
        Reg::Priv => m.mode() as u32,
    };
    Ok(reply(&hex_word(v)))
}

/// `P n=value`.
fn write_register(m: &mut Machine, args: &[u8]) -> Result<Action, Vec<u8>> {
    let i = args.iter().position(|&b| b == b'=').ok_or_else(malformed)?;
    let n = hex(&args[..i]).ok_or_else(malformed)? as usize;
    let bytes = unhex_bytes(&args[i + 1..]).ok_or_else(malformed)?;
    let [a, b, c, d] = bytes[..] else {
        return Err(malformed());
    };
    let v = u32::from_le_bytes([a, b, c, d]);
    match Reg::from_number(n).ok_or_else(|| error(1))? {
        Reg::X(r) => m.set_reg(r, v),
        Reg::Pc => m.set_pc(v),
        Reg::Csr(a) => {
            if !m.set_csr(a, v) {
                return Err(error(1));
            }
        }
        Reg::Priv => m.set_mode(Mode::from_bits(v.into())),
    }
    Ok(reply("OK"))
}

/// `addr,len` of the memory packets.
fn addr_len(args: &[u8]) -> Option<(u32, usize)> {
    let i = args.iter().position(|&b| b == b',')?;
    Some((hex(&args[..i])?, hex(&args[i + 1..])? as usize))
}

/// `m addr,len`: the bytes that can be read from `addr` on, through the selected hart's
/// translation; `E14` if not even the first can.
fn read_memory(m: &Machine, args: &[u8]) -> Result<Action, Vec<u8>> {
    let (addr, len) = addr_len(args).ok_or_else(malformed)?;
    let mut buf = vec![0; len.min(MAX_READ)];
    let n = m.read_memory(addr, &mut buf, Space::Virtual);
    if n == 0 && !buf.is_empty() {
        return Err(error(14));
    }
    Ok(reply(&hex_bytes(&buf[..n])))
}

/// `M addr,len:hex` and `X addr,len:binary`.
fn write_memory(m: &mut Machine, args: &[u8], binary: bool) -> Result<Action, Vec<u8>> {
    let colon = args.iter().position(|&b| b == b':').ok_or_else(malformed)?;
    let (addr, len) = addr_len(&args[..colon]).ok_or_else(malformed)?;
    let data = &args[colon + 1..];
    let data = if binary {
        packet::unescape(data)
    } else {
        unhex_bytes(data)
    }
    .ok_or_else(malformed)?;
    if data.len() != len {
        return Err(malformed());
    }
    if m.write_memory(addr, &data, Space::Virtual) != len {
        return Err(error(14));
    }
    Ok(reply("OK"))
}

/// `Z type,addr,kind` and `z type,addr,kind`: type 0 is a software breakpoint, 1 a
/// hardware one (the same thing here: neither patches memory), 2-4 a write, read and
/// access watchpoint, whose `kind` is the length watched. Conditions after a `;` are
/// not supported and ignored.
fn point(m: &mut Machine, set: bool, args: &[u8]) -> Result<Action, Vec<u8>> {
    let args = args.split(|&b| b == b';').next().unwrap_or_default();
    let mut fields = args.split(|&b| b == b',');
    let (Some(ty), Some(addr), Some(kind), None) =
        (fields.next(), fields.next(), fields.next(), fields.next())
    else {
        return Err(malformed());
    };
    let addr = hex(addr).ok_or_else(malformed)?;
    let len = hex(kind).ok_or_else(malformed)?;
    let watch = match ty {
        b"0" | b"1" => {
            if set {
                m.set_breakpoint(addr);
            } else {
                m.clear_breakpoint(addr);
            }
            return Ok(reply("OK"));
        }
        b"2" => WatchKind::Write,
        b"3" => WatchKind::Read,
        b"4" => WatchKind::Access,
        _ => return Ok(reply("")),
    };
    if set {
        m.set_watchpoint(addr, len, watch).map_err(|_| error(22))?;
    } else {
        m.clear_watchpoint(addr, len, watch);
    }
    Ok(reply("OK"))
}

/// The hart a thread id names: `None` for 0 ("any") and -1 ("all"), `E22` if there is
/// no such thread.
fn thread(m: &Machine, tid: &[u8]) -> Result<Option<usize>, Vec<u8>> {
    if tid == b"-1" || tid == b"0" {
        return Ok(None);
    }
    let t = hex(tid).ok_or_else(malformed)? as usize;
    if t == 0 || t > m.cpu.harts().len() {
        return Err(error(22));
    }
    Ok(Some(t - 1))
}

fn reply(s: &str) -> Action {
    Action::Reply(s.as_bytes().to_vec())
}

fn error(code: u8) -> Vec<u8> {
    format!("E{code:02x}").into_bytes()
}

fn malformed() -> Vec<u8> {
    error(1)
}

/// A hex number, at most 32 bits.
fn hex(s: &[u8]) -> Option<u32> {
    if s.is_empty() || s.len() > 8 {
        return None;
    }
    u32::from_str_radix(std::str::from_utf8(s).ok()?, 16).ok()
}

fn hex_bytes(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A register's value: its 4 bytes, little-endian, in hex.
fn hex_word(v: u32) -> String {
    hex_bytes(&v.to_le_bytes())
}

fn unhex_bytes(s: &[u8]) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    s.chunks(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MachineConfig;

    /// RAM's base as the debug interface takes addresses: 32 bits.
    const RAM_BASE: u32 = crate::bus::RAM_BASE as u32;

    /// A machine of `harts` harts with 64 KiB of RAM, and its stub.
    fn setup(harts: usize) -> (Machine, Stub) {
        let config = MachineConfig {
            harts,
            ..MachineConfig::with_ram(64 * 1024)
        };
        let m = Machine::new(config).unwrap();
        let stub = Stub::new(&m);
        (m, stub)
    }

    /// The payload of the reply to `packet`; panics if it was another action.
    fn ask(m: &mut Machine, stub: &mut Stub, packet: &str) -> String {
        match stub.handle(m, packet.as_bytes()) {
            Action::Reply(r) => String::from_utf8(r).unwrap(),
            other => panic!("{packet}: {other:?}"),
        }
    }

    #[test]
    fn unsupported_and_malformed_packets() {
        let (mut m, mut stub) = setup(1);
        for p in [
            "",
            "!",
            "vMustReplyEmpty",
            "qOffsets",
            "qTStatus",
            "x0,0",
            "Z9,0,4",
            "jThreadsInfo",
            "\u{7f}",
        ] {
            assert_eq!(ask(&mut m, &mut stub, p), "", "{p:?}");
        }
        for p in [
            "m",
            "m1000",
            "m1000,",
            "mzz,4",
            "m123456789,4",
            "M1000,4",
            "M1000,4:12",
            "M1000,2:1234zz",
            "Xbad",
            "p",
            "pzz",
            "P20",
            "P20=12",
            "G123",
            "Z0,1000",
            "Z0,zz,4",
            "Hx1",
            "H",
            "vCont;t",
            "qXfer:features:read:target.xml:zz",
            "czz",
        ] {
            assert_eq!(ask(&mut m, &mut stub, p), "E01", "{p:?}");
        }
        // Nothing changed.
        assert_eq!(m.pc(), crate::bus::BOOT_ROM_BASE as u32);
        assert_eq!(m.breakpoints().count(), 0);
    }

    #[test]
    fn negotiation_and_queries() {
        let (mut m, mut stub) = setup(2);
        assert_eq!(
            ask(
                &mut m,
                &mut stub,
                "qSupported:multiprocess+;swbreak+;hwbreak+;vContSupported+"
            ),
            "PacketSize=4000;qXfer:features:read+;QStartNoAckMode+;swbreak+;vContSupported+"
        );
        assert!(stub.swbreak);
        assert!(!stub.no_ack());
        assert_eq!(ask(&mut m, &mut stub, "QStartNoAckMode"), "OK");
        assert!(stub.no_ack());
        assert_eq!(ask(&mut m, &mut stub, "qAttached"), "1");
        assert_eq!(ask(&mut m, &mut stub, "qfThreadInfo"), "m1,2");
        assert_eq!(ask(&mut m, &mut stub, "qsThreadInfo"), "l");
        assert_eq!(ask(&mut m, &mut stub, "qC"), "QC1");
        assert_eq!(ask(&mut m, &mut stub, "T1"), "OK");
        assert_eq!(ask(&mut m, &mut stub, "T2"), "OK");
        assert_eq!(ask(&mut m, &mut stub, "T3"), "E16");
        assert_eq!(ask(&mut m, &mut stub, "vCont?"), "vCont;c;C;s;S");
        let info = ask(&mut m, &mut stub, "qThreadExtraInfo,2");
        assert_eq!(
            String::from_utf8(unhex_bytes(info.as_bytes()).unwrap()).unwrap(),
            "hart 1, M-mode"
        );
        assert_eq!(ask(&mut m, &mut stub, "qThreadExtraInfo,5"), "E16");
        assert_eq!(
            ask(&mut m, &mut stub, "qRegisterInfo0"),
            "name:zero;alt-name:x0;bitsize:32;offset:0;encoding:uint;format:hex;set:General Purpose Registers;dwarf:0;"
        );
        assert_eq!(
            ask(&mut m, &mut stub, "qRegisterInfo20"),
            "name:pc;bitsize:32;offset:128;encoding:uint;format:hex;set:General Purpose Registers;generic:pc;"
        );
        assert!(ask(&mut m, &mut stub, "qRegisterInfoa").contains("generic:arg1;"));
        assert_eq!(ask(&mut m, &mut stub, "qRegisterInfo21"), "E45");
        assert_eq!(ask(&mut m, &mut stub, "qRegisterInfozz"), "E01");
        let host = ask(&mut m, &mut stub, "qHostInfo");
        assert!(host.starts_with("triple:7269736376"), "{host}");
    }

    #[test]
    fn target_xml_comes_in_parts() {
        let (mut m, mut stub) = setup(1);
        let whole = target::xml(&m);
        let mut got = Vec::new();
        loop {
            let r = ask(
                &mut m,
                &mut stub,
                &format!("qXfer:features:read:target.xml:{:x},100", got.len()),
            );
            let (kind, data) = r.split_at(1);
            got.extend(packet::unescape(data.as_bytes()).unwrap());
            if kind == "l" {
                break;
            }
            assert_eq!(kind, "m");
        }
        assert_eq!(String::from_utf8(got).unwrap(), whole);
        // Past the end: an empty last part.
        let past = format!("qXfer:features:read:target.xml:{:x},10", whole.len() + 5);
        assert_eq!(ask(&mut m, &mut stub, &past), "l");
        assert_eq!(ask(&mut m, &mut stub, "qXfer:features:read:other.xml:0,10"), "");
    }

    #[test]
    fn registers() {
        let (mut m, mut stub) = setup(1);
        m.set_reg(1, 0x1122_3344);
        let g = ask(&mut m, &mut stub, "g");
        assert_eq!(g.len(), 33 * 8);
        assert_eq!(&g[..8], "00000000");
        assert_eq!(&g[8..16], "44332211");
        assert_eq!(&g[32 * 8..], "00100000", "pc = 0x1000");
        assert_eq!(ask(&mut m, &mut stub, "p1"), "44332211");
        assert_eq!(ask(&mut m, &mut stub, "p20"), "00100000");
        // mhartid, misa and priv.
        assert_eq!(ask(&mut m, &mut stub, &format!("p{:x}", 65 + 0xf14)), "00000000");
        let misa = ask(&mut m, &mut stub, &format!("p{:x}", 65 + 0x301));
        assert_eq!(misa, hex_word(crate::cpu::misa(crate::Xlen::Rv32) as u32));
        assert_eq!(ask(&mut m, &mut stub, "p1041"), "03000000", "M-mode");
        assert_eq!(ask(&mut m, &mut stub, "p21"), "E01", "no f0");
        assert_eq!(
            ask(&mut m, &mut stub, &format!("p{:x}", 65 + 0x7c0)),
            "E01",
            "no such CSR"
        );

        assert_eq!(ask(&mut m, &mut stub, "P5=78563412"), "OK");
        assert_eq!(m.reg(5), 0x1234_5678);
        assert_eq!(ask(&mut m, &mut stub, "P0=78563412"), "OK");
        assert_eq!(m.reg(0), 0, "x0 stays 0");
        assert_eq!(ask(&mut m, &mut stub, "P20=00000080"), "OK");
        assert_eq!(m.pc(), RAM_BASE);
        // mscratch, then a read-only CSR.
        assert_eq!(
            ask(&mut m, &mut stub, &format!("P{:x}=efbeadde", 65 + 0x340)),
            "OK"
        );
        assert_eq!(m.csr(0x340), Some(0xdead_beef));
        assert_eq!(
            ask(&mut m, &mut stub, &format!("P{:x}=01000000", 65 + 0xf14)),
            "E01"
        );
        assert_eq!(ask(&mut m, &mut stub, "P1041=01000000"), "OK");
        assert_eq!(m.mode(), Mode::S);

        let mut all = "00000000".to_string();
        for r in 1..32u32 {
            all.push_str(&hex_word(r * 0x100));
        }
        all.push_str(&hex_word(RAM_BASE + 8));
        assert_eq!(ask(&mut m, &mut stub, &format!("G{all}")), "OK");
        assert_eq!(m.reg(31), 0x1f00);
        assert_eq!(m.pc(), RAM_BASE + 8);
        assert_eq!(ask(&mut m, &mut stub, "g"), all);
        // A shorter G writes the registers it has.
        assert_eq!(ask(&mut m, &mut stub, "G00000000ffffffff"), "OK");
        assert_eq!(m.reg(1), 0xffff_ffff);
        assert_eq!(m.reg(2), 0x200);
    }

    #[test]
    fn memory() {
        let (mut m, mut stub) = setup(1);
        let a = RAM_BASE + 0x100;
        assert_eq!(ask(&mut m, &mut stub, &format!("M{a:x},4:01020304")), "OK");
        assert_eq!(ask(&mut m, &mut stub, &format!("m{a:x},4")), "01020304");
        assert_eq!(ask(&mut m, &mut stub, &format!("m{a:x},0")), "");
        // Binary write, with escaped bytes: '}' 0x5d is 0x7d, '#' 0x23.
        assert_eq!(ask(&mut m, &mut stub, &format!("X{a:x},3:a}}]}}\x03")), "OK");
        assert_eq!(ask(&mut m, &mut stub, &format!("m{a:x},4")), "617d2304");
        assert_eq!(ask(&mut m, &mut stub, &format!("X{a:x},0:")), "OK");
        assert_eq!(
            ask(&mut m, &mut stub, &format!("M{a:x},2:01")),
            "E01",
            "length mismatch"
        );
        // The boot ROM reads; device registers and holes do not.
        m.cpu.bus.set_boot_entry(crate::bus::RAM_BASE);
        assert_eq!(
            ask(&mut m, &mut stub, "m1000,4"),
            hex_word(m.cpu.bus.read_mem(0x1000, 4).unwrap() as u32)
        );
        assert_ne!(ask(&mut m, &mut stub, "m1000,4"), "00000000");
        assert_eq!(ask(&mut m, &mut stub, "m10000000,1"), "E0e", "UART");
        assert_eq!(ask(&mut m, &mut stub, "m0,4"), "E0e");
        assert_eq!(ask(&mut m, &mut stub, "M1000,1:00"), "E0e", "ROM");
        // A read that runs off the end of RAM stops there.
        let end = RAM_BASE + 64 * 1024;
        assert_eq!(ask(&mut m, &mut stub, &format!("m{:x},8", end - 2)), "0000");
        // A huge read is cut down.
        assert_eq!(
            ask(&mut m, &mut stub, &format!("m{RAM_BASE:x},ffffff")).len(),
            2 * MAX_READ
        );
    }

    #[test]
    fn memory_goes_through_the_selected_harts_translation() {
        let (mut m, mut stub) = setup(1);
        // A page table at RAM + 0x1000 whose one 4 MiB superpage maps virtual
        // 0x4000_0000 onto RAM (PPN 0x80000), readable, in S-mode with paging on.
        let root = RAM_BASE + 0x1000;
        let pte: u32 = (0x80000 << 10) | 0b1111; // V R W X
        let slot = root + (0x4000_0000u32 >> 22) * 4;
        m.write_memory(slot, &pte.to_le_bytes(), Space::Physical);
        m.write_memory(RAM_BASE, &[0xaa, 0xbb], Space::Physical);
        assert!(m.set_csr(0x180, (1 << 31) | (root >> 12)));
        m.set_mode(Mode::S);
        assert_eq!(ask(&mut m, &mut stub, "m40000000,2"), "aabb");
        assert_eq!(ask(&mut m, &mut stub, "m80000000,2"), "E0e", "not mapped");
        // Reading set no A bit.
        let mut back = [0; 4];
        m.read_memory(slot, &mut back, Space::Physical);
        assert_eq!(u32::from_le_bytes(back), pte);
        // In M-mode, addresses are physical.
        m.set_mode(Mode::M);
        assert_eq!(ask(&mut m, &mut stub, "m80000000,2"), "aabb");
    }

    #[test]
    fn breakpoints_and_watchpoints() {
        let (mut m, mut stub) = setup(1);
        assert_eq!(ask(&mut m, &mut stub, "Z0,80000010,4"), "OK");
        assert_eq!(ask(&mut m, &mut stub, "Z1,80000020,4"), "OK");
        assert_eq!(
            ask(&mut m, &mut stub, "Z0,80000030,4;X1,0a"),
            "OK",
            "condition ignored"
        );
        assert_eq!(
            m.breakpoints().collect::<Vec<_>>(),
            [RAM_BASE + 0x10, RAM_BASE + 0x20, RAM_BASE + 0x30]
        );
        assert_eq!(ask(&mut m, &mut stub, "z0,80000010,4"), "OK");
        assert_eq!(ask(&mut m, &mut stub, "z0,80000010,4"), "OK", "already gone");
        assert_eq!(m.breakpoints().count(), 2);

        assert_eq!(ask(&mut m, &mut stub, "Z2,80001000,4"), "OK");
        assert_eq!(ask(&mut m, &mut stub, "Z3,80001000,1"), "OK");
        assert_eq!(ask(&mut m, &mut stub, "Z4,80002000,8"), "OK");
        assert_eq!(ask(&mut m, &mut stub, "Z2,80001000,0"), "E16", "empty range");
        assert_eq!(m.watchpoints().len(), 3);
        assert_eq!(m.watchpoints()[2].kind, WatchKind::Access);
        assert_eq!(ask(&mut m, &mut stub, "z3,80001000,1"), "OK");
        assert_eq!(m.watchpoints().len(), 2);
    }

    #[test]
    fn threads_and_resumes() {
        let (mut m, mut stub) = setup(2);
        assert_eq!(ask(&mut m, &mut stub, "Hg2"), "OK");
        assert_eq!(m.selected_hart(), 1);
        assert_eq!(ask(&mut m, &mut stub, "qC"), "QC2");
        assert_eq!(
            ask(&mut m, &mut stub, "p20"),
            hex_word(crate::bus::PARK_PC as u32)
        );
        assert_eq!(ask(&mut m, &mut stub, "Hg0"), "OK", "any: unchanged");
        assert_eq!(m.selected_hart(), 1);
        assert_eq!(ask(&mut m, &mut stub, "Hg3"), "E16");
        assert_eq!(ask(&mut m, &mut stub, "Hg1"), "OK");
        assert_eq!(m.selected_hart(), 0);

        assert_eq!(stub.handle(&mut m, b"c"), Action::Continue);
        assert_eq!(stub.handle(&mut m, b"C05"), Action::Continue);
        assert_eq!(stub.handle(&mut m, b"s"), Action::Step);
        assert_eq!(stub.handle(&mut m, b"S05;80000000"), Action::Step);
        assert_eq!(m.pc(), RAM_BASE, "resumed at the address");
        assert_eq!(stub.handle(&mut m, b"Hc2"), Action::Reply(b"OK".to_vec()));
        assert_eq!(stub.handle(&mut m, b"s"), Action::Step);
        assert_eq!(m.selected_hart(), 1, "steps Hc's thread");
        assert_eq!(stub.handle(&mut m, b"Hc-1"), Action::Reply(b"OK".to_vec()));
        assert_eq!(stub.handle(&mut m, b"vCont;s:1;c"), Action::Step);
        assert_eq!(m.selected_hart(), 0);
        assert_eq!(stub.handle(&mut m, b"vCont;c"), Action::Continue);
        assert_eq!(stub.handle(&mut m, b"vCont;c:2"), Action::Continue);
        assert_eq!(stub.handle(&mut m, b"vCont;s:9"), Action::Reply(b"E16".to_vec()));
        assert_eq!(stub.handle(&mut m, b"k"), Action::Kill);
        assert_eq!(stub.handle(&mut m, b"vKill;1"), Action::Kill);
        assert_eq!(stub.handle(&mut m, b"D"), Action::Detach);
        assert_eq!(stub.handle(&mut m, b"D;1"), Action::Detach);
    }

    #[test]
    fn stop_replies() {
        let (mut m, mut stub) = setup(2);
        assert_eq!(ask(&mut m, &mut stub, "?"), "T02thread:1;20:00100000;");
        let r = stub.stopped(
            &mut m,
            StopReason::Machine(Stop::Breakpoint { hart: 1, pc: 0x1020 }),
        );
        assert_eq!(r, b"T05thread:2;20:20100000;");
        assert_eq!(m.selected_hart(), 1);
        ask(&mut m, &mut stub, "qSupported:swbreak+");
        assert_eq!(ask(&mut m, &mut stub, "?"), "T05swbreak:;thread:2;20:20100000;");
        let hit = crate::debug::WatchHit {
            hart: 0,
            kind: WatchKind::Read,
            addr: 0x8000_1000,
        };
        let r = stub.stopped(&mut m, StopReason::Machine(Stop::Watchpoint(hit)));
        assert_eq!(r, b"T05rwatch:80001000;thread:1;20:00100000;");
        let r = stub.stopped(&mut m, StopReason::Machine(Stop::Stepped { hart: 1 }));
        assert_eq!(r, b"T05thread:2;20:20100000;");
        assert_eq!(stub.stopped(&mut m, StopReason::Machine(Stop::Exited(3))), b"W03");
        assert_eq!(ask(&mut m, &mut stub, "?"), "W03");
    }
}
