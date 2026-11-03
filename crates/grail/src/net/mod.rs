//! Xcal network card (spec section 9.8): an Ethernet card that moves raw Ethernet II
//! frames between two descriptor rings in guest RAM and a [`NetBackend`].
//!
//! The driver keeps two rings of descriptors in RAM, one for frames to send and one for
//! buffers to receive into, and tells the card about them through a few
//! registers. Each ring is described by a base address, a size (a power of two) and two
//! free-running 32-bit counters, `HEAD` and `TAIL`: the descriptors from `HEAD` up to
//! (not including) `TAIL` belong to the card, the rest to the driver, and the slot of
//! counter `n` is `n % SIZE`. The driver hands descriptors over by advancing `TAIL`; the
//! card hands them back by advancing `HEAD`. Because the counters run past `SIZE`
//! (wrapping at 2^32), "empty" (`HEAD == TAIL`) and "full" (`TAIL - HEAD == SIZE`) look
//! different and every slot can be used.
//!
//! - **Transmit** happens during the store to `NET_TX_TAIL`: the card sends every
//!   descriptor from `HEAD` to the new `TAIL`, writes each one's status and moves
//!   `HEAD` past it.
//! - **Receive** happens when the card polls its backend, every [`POLL_STEPS`] steps of
//!   the machine (and whenever the host calls [`NetDevice::poll`]). A frame goes into the
//!   buffer of the descriptor at `HEAD`; when `HEAD == TAIL` (no buffer) it is dropped
//!   and counted in `NET_RX_DROPPED`. Nothing queues without bound: the backend is
//!   drained at every poll whether or not the ring has room.
//!
//! Like the block device, every address is physical, every buffer must lie in RAM, and
//! a bad descriptor is an error status for the program, never a host panic.
//!
//! Two register layouts, as for the block device: **version 2**, which RV64 machines
//! present, with 64-bit ring bases and 16-byte descriptors holding a 64-bit address
//! ([`v2`]); and **version 1**, all 32-bit with 8-byte descriptors, which RV32 machines
//! keep (the constants at the top of this module).
//!
//! Where frames go is a [`NetBackend`], chosen per card by whoever builds the machine: a
//! port of an in-process [`Hub`] (tests; two machines on one hub), a NAT
//! [`gateway::Gateway`] (user networking, `user.rs`), wrapped or not in a [`Pcap`]
//! capture, or a backend of the host's own handed to [`crate::Machine::with_devices`].

mod hub;
mod pcap;
mod user;

pub use hub::{Hub, HubPort, HUB_QUEUE};
pub use pcap::{
    header as pcap_header, record as pcap_record, Clock, Pcap, LINKTYPE_ETHERNET, PCAP_MAGIC, PCAP_SNAPLEN,
};

use crate::bus::{BusFault, RAM_BASE};
use crate::mmio::{self, Field};
use excalibur::Xlen;
use std::fmt;
use std::io;

/// Shortest frame the card sends or delivers: an Ethernet II header (destination,
/// source, EtherType). Frames carry no FCS and need no padding to 60 bytes.
pub const MIN_FRAME: usize = 14;
/// Longest frame: a 1500-byte payload behind the 14-byte header.
pub const MAX_FRAME: usize = 1514;

/// Largest ring, in descriptors.
pub const MAX_RING: u32 = 1024;
/// Bytes per version 1 descriptor: address (u32), length (u16), status (u16).
pub const DESC_SIZE: u32 = 8;

/// Steps of the machine between two polls of the backend for received frames.
pub const POLL_STEPS: u32 = 256;
/// Most frames taken from the backend in one poll, so a backend that never runs dry
/// cannot stall the machine. The rest wait in the backend (which bounds its own queue).
pub const POLL_BUDGET: usize = 64;

// Register offsets from the start of a card's page: version 1's, all 32-bit. The ones
// up to `REG_RX_DROPPED` are version 2's too.
pub const REG_MAC_LO: u32 = 0x00;
pub const REG_MAC_HI: u32 = 0x04;
pub const REG_STATUS: u32 = 0x08;
pub const REG_ERROR: u32 = 0x0c;
pub const REG_IRQ_ENABLE: u32 = 0x10;
pub const REG_IRQ_STATUS: u32 = 0x14;
pub const REG_RX_DROPPED: u32 = 0x18;
pub const REG_TX_BASE: u32 = 0x20;
pub const REG_TX_SIZE: u32 = 0x24;
pub const REG_TX_HEAD: u32 = 0x28;
pub const REG_TX_TAIL: u32 = 0x2c;
pub const REG_RX_BASE: u32 = 0x30;
pub const REG_RX_SIZE: u32 = 0x34;
pub const REG_RX_HEAD: u32 = 0x38;
pub const REG_RX_TAIL: u32 = 0x3c;

/// Version 2 (RV64 machines, spec section 9.8): the ring registers' offsets, and the
/// 16-byte descriptor.
pub mod v2 {
    /// 64-bit.
    pub const REG_TX_BASE: u32 = 0x20;
    pub const REG_TX_SIZE: u32 = 0x28;
    pub const REG_TX_HEAD: u32 = 0x2c;
    pub const REG_TX_TAIL: u32 = 0x30;
    /// 64-bit.
    pub const REG_RX_BASE: u32 = 0x38;
    pub const REG_RX_SIZE: u32 = 0x40;
    pub const REG_RX_HEAD: u32 = 0x44;
    pub const REG_RX_TAIL: u32 = 0x48;
    /// Bytes per descriptor: address (u64), length (u16), status (u16), reserved (u32).
    /// A ring's base is a multiple of it.
    pub const DESC_SIZE: u32 = 16;
}

/// `NET_STATUS` bit 0: the link is up (the backend can carry frames).
pub const STATUS_LINK_UP: u32 = 1 << 0;

/// `NET_IRQ_STATUS` / `NET_IRQ_ENABLE` bit 0: the card put a frame (or an error status)
/// into a receive descriptor.
pub const IRQ_RX: u32 = 1 << 0;
/// Bit 1: the card finished transmit descriptors.
pub const IRQ_TX: u32 = 1 << 1;
/// Bit 2: a ring error was recorded in `NET_ERROR`.
pub const IRQ_ERROR: u32 = 1 << 2;
const IRQ_ALL: u32 = IRQ_RX | IRQ_TX | IRQ_ERROR;

/// `NET_ERROR` values: what went wrong with a ring as a whole. The latest one is kept
/// until software writes the register.
pub mod error {
    /// Nothing.
    pub const NONE: u32 = 0;
    /// A ring size that is neither 0 nor a power of two from 1 to [`super::MAX_RING`];
    /// the ring's size became 0.
    pub const BAD_SIZE: u32 = 1;
    /// A `TAIL` more than `SIZE` descriptors ahead of `HEAD`; the write was ignored.
    pub const BAD_TAIL: u32 = 2;
    /// A descriptor the card needed is not in RAM (`BASE` is wrong). Transmit stops at
    /// it; a received frame is dropped.
    pub const BAD_RING: u32 = 3;
    /// Version 2: a `BASE` that is not a multiple of 16 (the descriptor size); the
    /// ring's size became 0.
    pub const BAD_BASE: u32 = 4;
}

/// Descriptor status values, written by the card into the status field of every
/// descriptor it hands back.
pub mod status {
    /// The frame was sent, or received into the buffer.
    pub const OK: u32 = 0;
    /// Transmit: the length is not from [`super::MIN_FRAME`] to [`super::MAX_FRAME`].
    pub const BAD_LENGTH: u32 = 1;
    /// The buffer (address, length) is not entirely in RAM.
    pub const BAD_DMA: u32 = 2;
    /// Receive: the frame is longer than the buffer. Nothing was copied; the length field
    /// holds the frame's length.
    pub const TOO_SMALL: u32 = 3;
    /// Transmit: the backend failed to send the frame.
    pub const IO: u32 = 4;
}

/// A MAC address, in the order its bytes go on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct MacAddr(pub [u8; 6]);

impl MacAddr {
    pub const BROADCAST: MacAddr = MacAddr([0xff; 6]);

    /// Grail's default address for card number `n` (counted from 1) of a backend that
    /// has no better idea: `02:ca:00:` and `n` in the last three bytes. Bit 1 of the
    /// first byte makes it locally administered, so it never clashes with a real card's.
    pub fn local(n: u32) -> MacAddr {
        let [_, a, b, c] = n.to_be_bytes();
        MacAddr([0x02, 0xca, 0x00, a, b, c])
    }

    /// Group (multicast or broadcast) addresses have bit 0 of the first byte set. A card
    /// needs an individual one.
    pub fn is_multicast(&self) -> bool {
        self.0[0] & 1 != 0
    }

    /// Bit 1 of the first byte: assigned locally rather than by a manufacturer.
    pub fn is_local(&self) -> bool {
        self.0[0] & 2 != 0
    }

    /// Whether a card may have this address: an individual one (not a group address), and
    /// locally administered, so it never clashes with a real card's on a network the
    /// machine reaches through the host. The error says what is wrong.
    pub fn check_card(&self) -> Result<(), String> {
        if self.is_multicast() {
            Err(format!("{self} is a group address, not a card's"))
        } else if !self.is_local() {
            Err(format!(
                "{self} is not locally administered (bit 1 of the first byte, as in 02:..)"
            ))
        } else {
            Ok(())
        }
    }

    /// Parses `xx:xx:xx:xx:xx:xx` (or with `-`), hex digits in either case.
    pub fn parse(s: &str) -> Result<MacAddr, String> {
        let bad = || format!("'{s}' is not a MAC address (like 02:ca:00:00:00:01)");
        let sep = if s.contains('-') { '-' } else { ':' };
        let mut mac = [0; 6];
        let mut parts = s.split(sep);
        for byte in &mut mac {
            let part = parts.next().ok_or_else(bad)?;
            if part.len() != 2 || !part.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(bad());
            }
            *byte = u8::from_str_radix(part, 16).map_err(|_| bad())?;
        }
        match parts.next() {
            None => Ok(MacAddr(mac)),
            Some(_) => Err(bad()),
        }
    }
}

impl fmt::Display for MacAddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let [a, b, c, d, e, g] = self.0;
        write!(f, "{a:02x}:{b:02x}:{c:02x}:{d:02x}:{e:02x}:{g:02x}")
    }
}

/// Where a card's frames go and come from. One per card, owned by the machine, which
/// runs on its own thread, so it must be `Send`; the frames themselves are plain bytes,
/// so a backend can hand them to other threads (another machine, a switch, a socket).
///
/// Both calls are made from the machine's run loop and must not block: `send` queues or
/// drops, `recv` returns at once. Frames are Ethernet II without the FCS, from
/// [`MIN_FRAME`] to [`MAX_FRAME`] bytes; the card only sends such frames and drops any
/// other length it receives.
pub trait NetBackend: Send {
    /// Sends one frame. An error is reported to the program as [`status::IO`] for that
    /// descriptor; a backend with nowhere to put the frame may also just drop it.
    fn send(&mut self, frame: &[u8]) -> io::Result<()>;

    /// The next frame that arrived for the card, if any. Never blocks.
    fn recv(&mut self) -> Option<Vec<u8>>;

    /// Whether the link is up, for `NET_STATUS`. Frames still flow either way.
    fn link_up(&self) -> bool {
        true
    }
}

/// One descriptor ring's registers.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Ring {
    /// 32 bits in version 1.
    base: u64,
    /// 0 (no ring) or a power of two up to `MAX_RING`.
    size: u32,
    head: u32,
    tail: u32,
}

impl Ring {
    /// Descriptors the card owns.
    fn pending(&self) -> u32 {
        self.tail.wrapping_sub(self.head)
    }

    /// Address of the descriptor for counter `n`, if it is one (a base near 2^64 may
    /// put it past the top of the address space).
    fn desc(&self, n: u32, layout: Layout) -> Option<u64> {
        let slot = n & self.size.wrapping_sub(1);
        self.base
            .checked_add(u64::from(slot) * u64::from(layout.desc_size))
    }
}

/// The registers' contents. `bool`: the receive ring's (`true`) or the transmit ring's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reg {
    MacLo,
    MacHi,
    Status,
    Error,
    IrqEnable,
    IrqStatus,
    RxDropped,
    Base(bool),
    Size(bool),
    Head(bool),
    Tail(bool),
}

/// Where a descriptor's fields are: the version's descriptor format.
#[derive(Debug, Clone, Copy)]
struct Layout {
    desc_size: u32,
    /// Bytes of the address field, at +0.
    addr_size: usize,
    /// Offset of the length field; the status follows it.
    len_at: usize,
}

impl Layout {
    fn of(xlen: Xlen) -> Layout {
        match xlen {
            Xlen::Rv32 => Layout {
                desc_size: DESC_SIZE,
                addr_size: 4,
                len_at: 4,
            },
            Xlen::Rv64 => Layout {
                desc_size: v2::DESC_SIZE,
                addr_size: 8,
                len_at: 8,
            },
        }
    }

    /// A descriptor's address and length fields.
    fn read(&self, d: &[u8]) -> (u64, u16) {
        let mut addr = [0; 8];
        addr[..self.addr_size].copy_from_slice(&d[..self.addr_size]);
        let len = u16::from_le_bytes([d[self.len_at], d[self.len_at + 1]]);
        (u64::from_le_bytes(addr), len)
    }

    /// Writes a descriptor's length and status fields (the rest is left alone).
    fn write_result(&self, d: &mut [u8], len: u16, st: u16) {
        d[self.len_at..self.len_at + 2].copy_from_slice(&len.to_le_bytes());
        d[self.len_at + 2..self.len_at + 4].copy_from_slice(&st.to_le_bytes());
    }
}

/// One card: the registers of spec section 9.8 in front of a backend.
pub struct NetDevice {
    backend: Box<dyn NetBackend>,
    mac: MacAddr,
    irq_enable: u32,
    irq_status: u32,
    error: u32,
    rx_dropped: u32,
    tx: Ring,
    rx: Ring,
    /// Steps until the next poll of the backend.
    countdown: u32,
    /// Which registers and descriptors the card has: version 2 on RV64, 1 on RV32.
    xlen: Xlen,
}

impl NetDevice {
    pub fn new(mac: MacAddr, backend: Box<dyn NetBackend>) -> Self {
        NetDevice {
            backend,
            mac,
            irq_enable: 0,
            irq_status: 0,
            error: error::NONE,
            rx_dropped: 0,
            tx: Ring::default(),
            rx: Ring::default(),
            countdown: POLL_STEPS,
            xlen: Xlen::Rv32,
        }
    }

    /// Presents the registers and descriptors of an `xlen` machine's cards (spec section
    /// 9.8). The registers keep their values.
    pub fn set_xlen(&mut self, xlen: Xlen) {
        self.xlen = xlen;
    }

    pub fn mac(&self) -> MacAddr {
        self.mac
    }

    pub fn backend(&mut self) -> &mut dyn NetBackend {
        &mut *self.backend
    }

    /// `NET_RX_DROPPED`: frames lost on the way in since the last reset of the counter.
    pub fn rx_dropped(&self) -> u32 {
        self.rx_dropped
    }

    /// `NET_ERROR`.
    pub fn error(&self) -> u32 {
        self.error
    }

    /// Level of the card's PLIC line: high while an enabled `NET_IRQ_STATUS` bit is set.
    pub fn irq(&self) -> bool {
        self.irq_status & self.irq_enable != 0
    }

    /// Called once per machine step: polls the backend every [`POLL_STEPS`] steps.
    /// `dma` hears of every RAM write the card makes (see [`NetDevice::poll`]).
    pub fn tick(&mut self, ram: &mut [u8], dma: &mut dyn FnMut(u64, u32)) {
        self.skip(1, ram, dma);
    }

    /// Lets `steps` steps pass at once (a paced machine idling in `wfi` skips its idle
    /// ticks in bulk): polls once if a poll fell due among them.
    pub fn skip(&mut self, steps: u64, ram: &mut [u8], dma: &mut dyn FnMut(u64, u32)) {
        if steps >= self.countdown as u64 {
            self.countdown = POLL_STEPS;
            self.poll(ram, dma);
        } else {
            self.countdown -= steps as u32;
        }
    }

    /// Takes what the backend received (up to [`POLL_BUDGET`] frames) into the receive
    /// ring, dropping what does not fit.
    ///
    /// No hart store causes these writes, so the hart's LR/SC reservation would not see
    /// them: `dma` is called with the physical address and length of every range the
    /// card writes (a frame into its buffer, a descriptor's length and status), and the
    /// machine passes them on to [`crate::cpu::Reservation::snoop`] (spec section 4.1).
    pub fn poll(&mut self, ram: &mut [u8], dma: &mut dyn FnMut(u64, u32)) {
        for _ in 0..POLL_BUDGET {
            match self.backend.recv() {
                Some(frame) => self.receive(&frame, ram, dma),
                None => break,
            }
        }
    }

    /// Delivers one frame into the descriptor at `RX_HEAD`.
    fn receive(&mut self, frame: &[u8], ram: &mut [u8], dma: &mut dyn FnMut(u64, u32)) {
        if !(MIN_FRAME..=MAX_FRAME).contains(&frame.len()) || self.rx.pending() == 0 {
            self.rx_dropped = self.rx_dropped.wrapping_add(1);
            return;
        }
        let layout = Layout::of(self.xlen);
        let desc_addr = self.rx.desc(self.rx.head, layout);
        let Some((desc_addr, desc)) =
            desc_addr.and_then(|a| Some((a, ram_range(ram, a, layout.desc_size.into())?)))
        else {
            self.ring_error(error::BAD_RING);
            self.rx_dropped = self.rx_dropped.wrapping_add(1);
            return;
        };
        let (addr, cap) = layout.read(&ram[desc.clone()]);
        let st = match ram_range(ram, addr, cap.into()) {
            None => status::BAD_DMA,
            Some(_) if (cap as usize) < frame.len() => status::TOO_SMALL,
            Some(buf) => {
                ram[buf.start..buf.start + frame.len()].copy_from_slice(frame);
                dma(addr, frame.len() as u32);
                status::OK
            }
        };
        if st != status::OK {
            self.rx_dropped = self.rx_dropped.wrapping_add(1);
        }
        layout.write_result(&mut ram[desc], frame.len() as u16, st as u16);
        // The length and status fields.
        dma(desc_addr + layout.len_at as u64, 4);
        self.rx.head = self.rx.head.wrapping_add(1);
        self.irq_status |= IRQ_RX;
    }

    /// Sends every descriptor from `TX_HEAD` to `TX_TAIL`. Completing a descriptor
    /// writes its length and status fields, a RAM write no hart made: `dma` hears of
    /// each one, as in [`NetDevice::poll`].
    fn transmit(&mut self, ram: &mut [u8], dma: &mut dyn FnMut(u64, u32)) {
        let layout = Layout::of(self.xlen);
        let mut done = false;
        while self.tx.pending() != 0 {
            let desc_addr = self.tx.desc(self.tx.head, layout);
            let Some((desc_addr, desc)) =
                desc_addr.and_then(|a| Some((a, ram_range(ram, a, layout.desc_size.into())?)))
            else {
                self.ring_error(error::BAD_RING);
                break;
            };
            let (addr, len) = layout.read(&ram[desc.clone()]);
            let st = if !(MIN_FRAME..=MAX_FRAME).contains(&(len as usize)) {
                status::BAD_LENGTH
            } else {
                match ram_range(ram, addr, len.into()) {
                    None => status::BAD_DMA,
                    Some(buf) => match self.backend.send(&ram[buf]) {
                        Ok(()) => status::OK,
                        Err(_) => status::IO,
                    },
                }
            };
            layout.write_result(&mut ram[desc], len, st as u16);
            // The length and status fields.
            dma(desc_addr + layout.len_at as u64, 4);
            self.tx.head = self.tx.head.wrapping_add(1);
            done = true;
        }
        if done {
            self.irq_status |= IRQ_TX;
        }
    }

    fn ring_error(&mut self, code: u32) {
        self.error = code;
        self.irq_status |= IRQ_ERROR;
    }

    /// Writing a ring's `BASE` or `SIZE` starts it over: `HEAD` and `TAIL` go back to 0.
    /// In version 2 a ring whose `BASE` is not a multiple of 16 has no descriptors.
    fn set_ring(&mut self, rx: bool, base: Option<u64>, size: Option<u32>) {
        let mut ring = if rx { self.rx } else { self.tx };
        if let Some(base) = base {
            ring.base = base;
        }
        if let Some(size) = size {
            // 0 turns the ring off; anything else must be a power of two.
            ring.size = if size == 0 || (size.is_power_of_two() && size <= MAX_RING) {
                size
            } else {
                self.ring_error(error::BAD_SIZE);
                0
            };
        }
        let desc_size = Layout::of(self.xlen).desc_size;
        if self.xlen == Xlen::Rv64 && !ring.base.is_multiple_of(desc_size.into()) {
            // The register keeps what was written. Only the low bits count, so a base
            // written by halves is checked the same whichever half comes first.
            if base.is_some() || ring.size != 0 {
                self.ring_error(error::BAD_BASE);
            }
            ring.size = 0;
        }
        ring.head = 0;
        ring.tail = 0;
        if rx {
            self.rx = ring;
        } else {
            self.tx = ring;
        }
    }

    /// A new `TAIL` for a ring: refused (error `BAD_TAIL`) if that would give the card
    /// more descriptors than the ring has.
    fn set_tail(&mut self, rx: bool, tail: u32) -> bool {
        let ring = if rx { &mut self.rx } else { &mut self.tx };
        if tail.wrapping_sub(ring.head) > ring.size {
            self.ring_error(error::BAD_TAIL);
            return false;
        }
        ring.tail = tail;
        true
    }

    /// What `off` holds in this card's layout.
    fn field(&self, off: u32) -> Field<Reg> {
        use Field::{High, Low, Unused, Word};
        let common = match off {
            REG_MAC_LO => Word(Reg::MacLo),
            REG_MAC_HI => Word(Reg::MacHi),
            REG_STATUS => Word(Reg::Status),
            REG_ERROR => Word(Reg::Error),
            REG_IRQ_ENABLE => Word(Reg::IrqEnable),
            REG_IRQ_STATUS => Word(Reg::IrqStatus),
            REG_RX_DROPPED => Word(Reg::RxDropped),
            _ => Unused,
        };
        if common != Unused {
            return common;
        }
        match self.xlen {
            Xlen::Rv32 => match off {
                REG_TX_BASE => Word(Reg::Base(false)),
                REG_TX_SIZE => Word(Reg::Size(false)),
                REG_TX_HEAD => Word(Reg::Head(false)),
                REG_TX_TAIL => Word(Reg::Tail(false)),
                REG_RX_BASE => Word(Reg::Base(true)),
                REG_RX_SIZE => Word(Reg::Size(true)),
                REG_RX_HEAD => Word(Reg::Head(true)),
                REG_RX_TAIL => Word(Reg::Tail(true)),
                _ => Unused,
            },
            Xlen::Rv64 => match off {
                v2::REG_TX_BASE => Low(Reg::Base(false)),
                0x24 => High(Reg::Base(false)),
                v2::REG_TX_SIZE => Word(Reg::Size(false)),
                v2::REG_TX_HEAD => Word(Reg::Head(false)),
                v2::REG_TX_TAIL => Word(Reg::Tail(false)),
                v2::REG_RX_BASE => Low(Reg::Base(true)),
                0x3c => High(Reg::Base(true)),
                v2::REG_RX_SIZE => Word(Reg::Size(true)),
                v2::REG_RX_HEAD => Word(Reg::Head(true)),
                v2::REG_RX_TAIL => Word(Reg::Tail(true)),
                _ => Unused,
            },
        }
    }

    fn ring(&self, rx: bool) -> &Ring {
        if rx {
            &self.rx
        } else {
            &self.tx
        }
    }

    fn value(&self, reg: Reg) -> u64 {
        let m = self.mac.0;
        let word = match reg {
            Reg::MacLo => u32::from_le_bytes([m[0], m[1], m[2], m[3]]),
            Reg::MacHi => u32::from_le_bytes([m[4], m[5], 0, 0]),
            Reg::Status => self.backend.link_up() as u32 * STATUS_LINK_UP,
            Reg::Error => self.error,
            Reg::IrqEnable => self.irq_enable,
            Reg::IrqStatus => self.irq_status,
            Reg::RxDropped => self.rx_dropped,
            Reg::Base(rx) => return self.ring(rx).base,
            Reg::Size(rx) => self.ring(rx).size,
            Reg::Head(rx) => self.ring(rx).head,
            Reg::Tail(rx) => self.ring(rx).tail,
        };
        word.into()
    }

    /// Reads a register, at the widths of spec section 9 ([`mmio::access`]). Unknown
    /// offsets read as 0.
    pub fn read(&self, off: u32, size: u32) -> Result<u64, BusFault> {
        let access = mmio::access(off, size, self.xlen == Xlen::Rv64, |o| self.field(o))?;
        Ok(access.read(|reg| self.value(reg)))
    }

    /// Writing `NET_TX_TAIL` sends frames from `ram` (the machine's RAM, mapped at
    /// `RAM_BASE`) and completes their descriptors there: `dma` hears of every RAM range
    /// the card writes, as in [`NetDevice::poll`]. Writes to read-only registers and
    /// unknown offsets are ignored, and a write of one half of a ring base keeps the
    /// other half.
    pub fn write(
        &mut self,
        off: u32,
        size: u32,
        val: u64,
        ram: &mut [u8],
        dma: &mut dyn FnMut(u64, u32),
    ) -> Result<(), BusFault> {
        let access = mmio::access(off, size, self.xlen == Xlen::Rv64, |o| self.field(o))?;
        let Some(reg) = access.reg() else {
            return Ok(());
        };
        let word = val as u32;
        match reg {
            Reg::Error => self.error = error::NONE,
            Reg::IrqEnable => self.irq_enable = word & IRQ_ALL,
            Reg::IrqStatus => self.irq_status &= !word,
            Reg::RxDropped => self.rx_dropped = 0,
            Reg::Base(rx) => {
                let base = access.merge(self.ring(rx).base, val);
                self.set_ring(rx, Some(base), None);
            }
            Reg::Size(rx) => self.set_ring(rx, None, Some(word)),
            Reg::Tail(false) => {
                if self.set_tail(false, word) {
                    self.transmit(ram, dma);
                }
            }
            Reg::Tail(true) => {
                self.set_tail(true, word);
            }
            Reg::MacLo | Reg::MacHi | Reg::Status | Reg::Head(_) => {}
        }
        Ok(())
    }
}

/// The RAM indices of the physical range `[addr, addr + len)`, if it is all RAM.
/// No wrap-around: a range past 2^64 is not in RAM.
fn ram_range(ram: &[u8], addr: u64, len: u64) -> Option<std::ops::Range<usize>> {
    let start = addr.checked_sub(RAM_BASE)?;
    let end = start.checked_add(len)?;
    (end <= ram.len() as u64).then_some(start as usize..end as usize)
}

#[cfg(test)]
mod tests;
