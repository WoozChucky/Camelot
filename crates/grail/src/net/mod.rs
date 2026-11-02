//! Xcal network card (spec section 9.8): an Ethernet card that moves raw Ethernet II
//! frames between two descriptor rings in guest RAM and a [`NetBackend`].
//!
//! The driver keeps two rings of 8-byte descriptors in RAM, one for frames to send and
//! one for buffers to receive into, and tells the card about them through a few
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
//! Where frames go is a [`NetBackend`], chosen per card by whoever builds the machine: a
//! port of an in-process [`Hub`] (tests; two machines on one hub), wrapped or not in a
//! [`Pcap`] capture, or a backend of the host's own handed to
//! [`crate::Machine::with_devices`].

mod hub;
mod pcap;

pub use hub::{Hub, HubPort, HUB_QUEUE};
pub use pcap::{
    header as pcap_header, record as pcap_record, Clock, Pcap, LINKTYPE_ETHERNET, PCAP_MAGIC, PCAP_SNAPLEN,
};

use crate::bus::{BusFault, RAM_BASE};
use std::fmt;
use std::io;

/// Shortest frame the card sends or delivers: an Ethernet II header (destination,
/// source, EtherType). Frames carry no FCS and need no padding to 60 bytes.
pub const MIN_FRAME: usize = 14;
/// Longest frame: a 1500-byte payload behind the 14-byte header.
pub const MAX_FRAME: usize = 1514;

/// Largest ring, in descriptors.
pub const MAX_RING: u32 = 1024;
/// Bytes per descriptor: address (u32), length (u16), status (u16).
pub const DESC_SIZE: u32 = 8;

/// Steps of the machine between two polls of the backend for received frames.
pub const POLL_STEPS: u32 = 256;
/// Most frames taken from the backend in one poll, so a backend that never runs dry
/// cannot stall the machine. The rest wait in the backend (which bounds its own queue).
pub const POLL_BUDGET: usize = 64;

// Register offsets from the start of a card's page.
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
    base: u32,
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

    /// Address of the descriptor for counter `n`.
    fn desc(&self, n: u32) -> u64 {
        self.base as u64 + (n & self.size.wrapping_sub(1)) as u64 * DESC_SIZE as u64
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
        }
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
    pub fn tick(&mut self, ram: &mut [u8]) {
        self.skip(1, ram);
    }

    /// Lets `steps` steps pass at once (a paced machine idling in `wfi` skips its idle
    /// ticks in bulk): polls once if a poll fell due among them.
    pub fn skip(&mut self, steps: u64, ram: &mut [u8]) {
        if steps >= self.countdown as u64 {
            self.countdown = POLL_STEPS;
            self.poll(ram);
        } else {
            self.countdown -= steps as u32;
        }
    }

    /// Takes what the backend received (up to [`POLL_BUDGET`] frames) into the receive
    /// ring, dropping what does not fit.
    pub fn poll(&mut self, ram: &mut [u8]) {
        for _ in 0..POLL_BUDGET {
            match self.backend.recv() {
                Some(frame) => self.receive(&frame, ram),
                None => break,
            }
        }
    }

    /// Delivers one frame into the descriptor at `RX_HEAD`.
    fn receive(&mut self, frame: &[u8], ram: &mut [u8]) {
        if !(MIN_FRAME..=MAX_FRAME).contains(&frame.len()) || self.rx.pending() == 0 {
            self.rx_dropped = self.rx_dropped.wrapping_add(1);
            return;
        }
        let Some(desc) = ram_range(ram, self.rx.desc(self.rx.head), DESC_SIZE as u64) else {
            self.ring_error(error::BAD_RING);
            self.rx_dropped = self.rx_dropped.wrapping_add(1);
            return;
        };
        let (addr, cap) = read_desc(&ram[desc.clone()]);
        let st = match ram_range(ram, addr as u64, cap as u64) {
            None => status::BAD_DMA,
            Some(_) if (cap as usize) < frame.len() => status::TOO_SMALL,
            Some(buf) => {
                ram[buf.start..buf.start + frame.len()].copy_from_slice(frame);
                status::OK
            }
        };
        if st != status::OK {
            self.rx_dropped = self.rx_dropped.wrapping_add(1);
        }
        write_desc_result(&mut ram[desc], frame.len() as u16, st as u16);
        self.rx.head = self.rx.head.wrapping_add(1);
        self.irq_status |= IRQ_RX;
    }

    /// Sends every descriptor from `TX_HEAD` to `TX_TAIL`.
    fn transmit(&mut self, ram: &mut [u8]) {
        let mut done = false;
        while self.tx.pending() != 0 {
            let Some(desc) = ram_range(ram, self.tx.desc(self.tx.head), DESC_SIZE as u64) else {
                self.ring_error(error::BAD_RING);
                break;
            };
            let (addr, len) = read_desc(&ram[desc.clone()]);
            let st = if !(MIN_FRAME..=MAX_FRAME).contains(&(len as usize)) {
                status::BAD_LENGTH
            } else {
                match ram_range(ram, addr as u64, len as u64) {
                    None => status::BAD_DMA,
                    Some(buf) => match self.backend.send(&ram[buf]) {
                        Ok(()) => status::OK,
                        Err(_) => status::IO,
                    },
                }
            };
            write_desc_result(&mut ram[desc], len, st as u16);
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
    fn set_ring(&mut self, rx: bool, base: Option<u32>, size: Option<u32>) {
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

    /// Registers are 32-bit only. Unknown offsets read as 0.
    pub fn read(&self, off: u32, size: u32) -> Result<u32, BusFault> {
        if size != 4 {
            return Err(BusFault);
        }
        let m = self.mac.0;
        Ok(match off {
            REG_MAC_LO => u32::from_le_bytes([m[0], m[1], m[2], m[3]]),
            REG_MAC_HI => u32::from_le_bytes([m[4], m[5], 0, 0]),
            REG_STATUS => self.backend.link_up() as u32 * STATUS_LINK_UP,
            REG_ERROR => self.error,
            REG_IRQ_ENABLE => self.irq_enable,
            REG_IRQ_STATUS => self.irq_status,
            REG_RX_DROPPED => self.rx_dropped,
            REG_TX_BASE => self.tx.base,
            REG_TX_SIZE => self.tx.size,
            REG_TX_HEAD => self.tx.head,
            REG_TX_TAIL => self.tx.tail,
            REG_RX_BASE => self.rx.base,
            REG_RX_SIZE => self.rx.size,
            REG_RX_HEAD => self.rx.head,
            REG_RX_TAIL => self.rx.tail,
            _ => 0,
        })
    }

    /// Writing `NET_TX_TAIL` sends frames from `ram` (the machine's RAM, mapped at
    /// `RAM_BASE`). Writes to read-only registers and unknown offsets are ignored.
    pub fn write(&mut self, off: u32, size: u32, val: u32, ram: &mut [u8]) -> Result<(), BusFault> {
        if size != 4 {
            return Err(BusFault);
        }
        match off {
            REG_ERROR => self.error = error::NONE,
            REG_IRQ_ENABLE => self.irq_enable = val & IRQ_ALL,
            REG_IRQ_STATUS => self.irq_status &= !val,
            REG_RX_DROPPED => self.rx_dropped = 0,
            REG_TX_BASE => self.set_ring(false, Some(val), None),
            REG_TX_SIZE => self.set_ring(false, None, Some(val)),
            REG_TX_TAIL => {
                if self.set_tail(false, val) {
                    self.transmit(ram);
                }
            }
            REG_RX_BASE => self.set_ring(true, Some(val), None),
            REG_RX_SIZE => self.set_ring(true, None, Some(val)),
            REG_RX_TAIL => {
                self.set_tail(true, val);
            }
            _ => {}
        }
        Ok(())
    }
}

/// The RAM indices of the physical range `[addr, addr + len)`, if it is all RAM.
/// 64-bit arithmetic: no wrap-around.
fn ram_range(ram: &[u8], addr: u64, len: u64) -> Option<std::ops::Range<usize>> {
    let start = addr.checked_sub(RAM_BASE as u64)?;
    let end = start + len;
    (end <= ram.len() as u64).then_some(start as usize..end as usize)
}

/// A descriptor's address and length fields.
fn read_desc(d: &[u8]) -> (u32, u16) {
    (
        u32::from_le_bytes([d[0], d[1], d[2], d[3]]),
        u16::from_le_bytes([d[4], d[5]]),
    )
}

/// Writes a descriptor's length and status fields (the address is left alone).
fn write_desc_result(d: &mut [u8], len: u16, st: u16) {
    d[4..6].copy_from_slice(&len.to_le_bytes());
    d[6..8].copy_from_slice(&st.to_le_bytes());
}

#[cfg(test)]
mod tests;
