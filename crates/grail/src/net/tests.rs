use super::*;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

const RAM_SIZE: usize = 16 * 1024;
const MAC: MacAddr = MacAddr([0x02, 0xca, 0x11, 0x22, 0x33, 0x44]);
/// RAM's and VRAM's bases as the card's registers and descriptors hold them: 32-bit
/// physical addresses.
const RAM_BASE: u32 = crate::bus::RAM_BASE as u32;
const VRAM_BASE: u32 = crate::bus::VRAM_BASE as u32;

// A layout in the test RAM: the transmit ring, the receive ring, then buffers.
const TX_RING: u32 = RAM_BASE;
const RX_RING: u32 = RAM_BASE + 0x100;
const BUFS: u32 = RAM_BASE + 0x1000;

/// What the card sent, and what the test wants it to receive, shared with the test.
#[derive(Default)]
struct Wire {
    sent: Vec<Vec<u8>>,
    incoming: VecDeque<Vec<u8>>,
    fail: bool,
    link_down: bool,
}

#[derive(Clone, Default)]
struct TestBackend(Arc<Mutex<Wire>>);

impl NetBackend for TestBackend {
    fn send(&mut self, frame: &[u8]) -> io::Result<()> {
        let mut w = self.0.lock().unwrap();
        if w.fail {
            return Err(io::Error::other("unplugged"));
        }
        w.sent.push(frame.to_vec());
        Ok(())
    }

    fn recv(&mut self) -> Option<Vec<u8>> {
        self.0.lock().unwrap().incoming.pop_front()
    }

    fn link_up(&self) -> bool {
        !self.0.lock().unwrap().link_down
    }
}

impl TestBackend {
    fn sent(&self) -> Vec<Vec<u8>> {
        std::mem::take(&mut self.0.lock().unwrap().sent)
    }

    fn arrive(&self, frame: Vec<u8>) {
        self.0.lock().unwrap().incoming.push_back(frame);
    }

    fn waiting(&self) -> usize {
        self.0.lock().unwrap().incoming.len()
    }
}

fn setup() -> (NetDevice, TestBackend, Vec<u8>) {
    let wire = TestBackend::default();
    (
        NetDevice::new(MAC, Box::new(wire.clone())),
        wire,
        vec![0; RAM_SIZE],
    )
}

fn w(d: &mut NetDevice, ram: &mut [u8], off: u32, val: u32) {
    d.write(off, 4, val.into(), ram, &mut |_, _| {}).unwrap();
}

fn r(d: &NetDevice, off: u32) -> u32 {
    d.read(off, 4).unwrap() as u32
}

/// The RAM bytes at physical address `addr`.
fn at(ram: &mut [u8], addr: u32, len: usize) -> &mut [u8] {
    let off = (addr - RAM_BASE) as usize;
    &mut ram[off..off + len]
}

/// Writes descriptor `slot` of the ring at `ring`: buffer address and length; status 0xffff
/// so the test sees that the card wrote it.
fn set_desc(ram: &mut [u8], ring: u32, slot: u32, addr: u32, len: u16) {
    let d = at(ram, ring + slot * DESC_SIZE, 8);
    d[..4].copy_from_slice(&addr.to_le_bytes());
    d[4..6].copy_from_slice(&len.to_le_bytes());
    d[6..].copy_from_slice(&0xffffu16.to_le_bytes());
}

/// Descriptor `slot`'s length and status fields.
fn desc(ram: &mut [u8], ring: u32, slot: u32) -> (u16, u16) {
    let d = at(ram, ring + slot * DESC_SIZE, 8);
    (u16::from_le_bytes([d[4], d[5]]), u16::from_le_bytes([d[6], d[7]]))
}

/// A frame of `len` bytes from MAC, every payload byte `fill`.
fn frame(len: usize, fill: u8) -> Vec<u8> {
    let mut f = vec![fill; len];
    f[..6].copy_from_slice(&MacAddr::BROADCAST.0);
    f[6..12].copy_from_slice(&MAC.0);
    f[12..14].copy_from_slice(&[0x88, 0xb5]);
    f
}

/// Rings of `size` descriptors at TX_RING and RX_RING.
fn rings(d: &mut NetDevice, ram: &mut [u8], size: u32) {
    w(d, ram, REG_TX_BASE, TX_RING);
    w(d, ram, REG_TX_SIZE, size);
    w(d, ram, REG_RX_BASE, RX_RING);
    w(d, ram, REG_RX_SIZE, size);
}

/// Puts `f` in buffer `n` (2 KiB each) and queues it in the transmit descriptor for
/// counter `tail`, without ringing the doorbell. Returns the next tail.
fn queue_tx(d: &NetDevice, ram: &mut [u8], tail: u32, n: u32, f: &[u8]) -> u32 {
    let buf = BUFS + n * 0x800;
    at(ram, buf, f.len()).copy_from_slice(f);
    set_desc(ram, TX_RING, tail % r(d, REG_TX_SIZE), buf, f.len() as u16);
    tail.wrapping_add(1)
}

/// Gives the card receive buffer `n` (2 KiB) in the descriptor for counter `tail`.
fn give_rx(d: &mut NetDevice, ram: &mut [u8], tail: u32, n: u32) -> u32 {
    set_desc(ram, RX_RING, tail % r(d, REG_RX_SIZE), BUFS + n * 0x800, 0x800);
    w(d, ram, REG_RX_TAIL, tail.wrapping_add(1));
    tail.wrapping_add(1)
}

#[test]
fn reset_state() {
    let (d, wire, _) = setup();
    assert_eq!(r(&d, REG_MAC_LO), 0x2211_ca02, "bytes 0-3, first in bits 7:0");
    assert_eq!(r(&d, REG_MAC_HI), 0x4433);
    assert_eq!(r(&d, REG_STATUS), STATUS_LINK_UP);
    for off in [
        REG_ERROR,
        REG_IRQ_ENABLE,
        REG_IRQ_STATUS,
        REG_RX_DROPPED,
        0x1c,
        REG_TX_BASE,
        REG_TX_SIZE,
        REG_TX_HEAD,
        REG_TX_TAIL,
        REG_RX_BASE,
        REG_RX_SIZE,
        REG_RX_HEAD,
        REG_RX_TAIL,
        0x40,
        0xffc,
    ] {
        assert_eq!(r(&d, off), 0, "offset {off:#x}");
    }
    assert!(!d.irq());
    assert_eq!(d.mac(), MAC);
    wire.0.lock().unwrap().link_down = true;
    assert_eq!(r(&d, REG_STATUS), 0, "the backend says the link is down");
}

#[test]
fn read_only_registers_ignore_writes() {
    let (mut d, _, mut ram) = setup();
    rings(&mut d, &mut ram, 4);
    for off in [
        REG_MAC_LO,
        REG_MAC_HI,
        REG_STATUS,
        REG_TX_HEAD,
        REG_RX_HEAD,
        0x1c,
        0x40,
        0xffc,
    ] {
        w(&mut d, &mut ram, off, 0xdead_beef);
    }
    assert_eq!(r(&d, REG_MAC_LO), 0x2211_ca02);
    assert_eq!(r(&d, REG_MAC_HI), 0x4433);
    assert_eq!(r(&d, REG_STATUS), STATUS_LINK_UP);
    assert_eq!([r(&d, REG_TX_HEAD), r(&d, REG_RX_HEAD)], [0, 0]);
    assert_eq!([r(&d, 0x1c), r(&d, 0x40), r(&d, 0xffc)], [0, 0, 0]);
    assert!(ram.iter().all(|&b| b == 0), "no DMA");
    // Reserved interrupt bits read 0.
    w(&mut d, &mut ram, REG_IRQ_ENABLE, u32::MAX);
    assert_eq!(r(&d, REG_IRQ_ENABLE), IRQ_RX | IRQ_TX | IRQ_ERROR);
}

#[test]
fn ring_registers() {
    let (mut d, _, mut ram) = setup();
    rings(&mut d, &mut ram, 8);
    assert_eq!([r(&d, REG_TX_BASE), r(&d, REG_TX_SIZE)], [TX_RING, 8]);
    assert_eq!([r(&d, REG_RX_BASE), r(&d, REG_RX_SIZE)], [RX_RING, 8]);
    // TAIL may run up to SIZE ahead of HEAD.
    w(&mut d, &mut ram, REG_RX_TAIL, 8);
    assert_eq!(r(&d, REG_RX_TAIL), 8);
    w(&mut d, &mut ram, REG_RX_TAIL, 3);
    assert_eq!(r(&d, REG_RX_TAIL), 3, "and back, while HEAD has not passed it");
    assert_eq!(r(&d, REG_ERROR), error::NONE);
    // Further, or behind HEAD, is refused.
    for bad in [9, u32::MAX, 0x8000_0000] {
        w(&mut d, &mut ram, REG_RX_TAIL, bad);
        assert_eq!(r(&d, REG_RX_TAIL), 3, "{bad:#x}");
        assert_eq!(r(&d, REG_ERROR), error::BAD_TAIL);
    }
    assert_eq!(r(&d, REG_IRQ_STATUS), IRQ_ERROR);
    // Writing NET_ERROR clears it, whatever the value.
    w(&mut d, &mut ram, REG_ERROR, 0x1234);
    assert_eq!(r(&d, REG_ERROR), error::NONE);

    // Writing BASE or SIZE starts the ring over.
    w(&mut d, &mut ram, REG_RX_BASE, RX_RING + 8);
    assert_eq!([r(&d, REG_RX_HEAD), r(&d, REG_RX_TAIL)], [0, 0]);
    assert_eq!(r(&d, REG_RX_SIZE), 8, "the size stays");
    w(&mut d, &mut ram, REG_RX_TAIL, 2);
    w(&mut d, &mut ram, REG_RX_SIZE, 4);
    assert_eq!([r(&d, REG_RX_HEAD), r(&d, REG_RX_TAIL)], [0, 0]);
    assert_eq!(r(&d, REG_RX_BASE), RX_RING + 8, "the base stays");
    assert_eq!(r(&d, REG_TX_SIZE), 8, "the other ring is untouched");

    // Sizes are powers of two from 1 to MAX_RING; anything else leaves no ring.
    for good in [1, 2, 64, MAX_RING] {
        w(&mut d, &mut ram, REG_TX_SIZE, good);
        assert_eq!(r(&d, REG_TX_SIZE), good);
    }
    assert_eq!(r(&d, REG_ERROR), error::NONE);
    for bad in [0, 3, 12, MAX_RING * 2, u32::MAX, 0x8000_0000] {
        w(&mut d, &mut ram, REG_TX_SIZE, 4);
        w(&mut d, &mut ram, REG_ERROR, 0);
        w(&mut d, &mut ram, REG_TX_SIZE, bad);
        assert_eq!(r(&d, REG_TX_SIZE), 0, "{bad:#x}");
        let want = if bad == 0 { error::NONE } else { error::BAD_SIZE };
        assert_eq!(r(&d, REG_ERROR), want, "{bad:#x}");
        // With no ring, TAIL cannot move.
        w(&mut d, &mut ram, REG_TX_TAIL, 1);
        assert_eq!(r(&d, REG_TX_TAIL), 0);
    }
}

#[test]
fn transmit_sends_the_frame_and_completes_the_descriptor() {
    let (mut d, wire, mut ram) = setup();
    rings(&mut d, &mut ram, 4);
    let f = frame(60, 0x5a);
    let tail = queue_tx(&d, &mut ram, 0, 0, &f);
    assert!(wire.sent().is_empty(), "nothing goes before the doorbell");
    w(&mut d, &mut ram, REG_TX_TAIL, tail);
    assert_eq!(wire.sent(), [f]);
    assert_eq!([r(&d, REG_TX_HEAD), r(&d, REG_TX_TAIL)], [1, 1]);
    assert_eq!(desc(&mut ram, TX_RING, 0), (60, status::OK as u16));
    assert_eq!(r(&d, REG_IRQ_STATUS), IRQ_TX);
    assert_eq!(r(&d, REG_ERROR), error::NONE);

    // Shortest and longest frames; a buffer at any byte address, up to the end of RAM.
    let short = frame(MIN_FRAME, 1);
    let long = frame(MAX_FRAME, 2);
    let end = RAM_BASE + RAM_SIZE as u32;
    at(&mut ram, BUFS + 3, MIN_FRAME).copy_from_slice(&short);
    set_desc(&mut ram, TX_RING, 1, BUFS + 3, MIN_FRAME as u16);
    at(&mut ram, end - MAX_FRAME as u32, MAX_FRAME).copy_from_slice(&long);
    set_desc(&mut ram, TX_RING, 2, end - MAX_FRAME as u32, MAX_FRAME as u16);
    w(&mut d, &mut ram, REG_TX_TAIL, 3);
    assert_eq!(wire.sent(), [short, long], "in ring order");
    assert_eq!(desc(&mut ram, TX_RING, 1), (MIN_FRAME as u16, 0));
    assert_eq!(desc(&mut ram, TX_RING, 2), (MAX_FRAME as u16, 0));
    assert_eq!(r(&d, REG_TX_HEAD), 3);
}

#[test]
fn transmit_ring_wraps_around() {
    let (mut d, wire, mut ram) = setup();
    rings(&mut d, &mut ram, 4);
    // 4 at once fills the ring; then batches of 3 cross the end of the ring each time.
    let mut tail = 0;
    let mut n = 0u8;
    for batch in [4, 3, 3, 3, 1, 2] {
        let mut want = Vec::new();
        for _ in 0..batch {
            let f = frame(20 + n as usize, n);
            tail = queue_tx(&d, &mut ram, tail, n as u32 % 4, &f);
            want.push(f);
            n += 1;
        }
        w(&mut d, &mut ram, REG_TX_TAIL, tail);
        assert_eq!(wire.sent(), want, "batch of {batch}");
        assert_eq!(r(&d, REG_TX_HEAD), tail);
    }
    assert_eq!(tail, 16);
    assert_eq!(r(&d, REG_ERROR), error::NONE);

    // The counters wrap at 2^32 too: slots still follow on.
    d.tx.head = u32::MAX - 1;
    d.tx.tail = u32::MAX - 1;
    let mut tail = u32::MAX - 1;
    let mut want = Vec::new();
    for i in 0..4u8 {
        let f = frame(30, 0xa0 + i);
        tail = queue_tx(&d, &mut ram, tail, i as u32, &f);
        want.push(f);
    }
    assert_eq!(tail, 2);
    w(&mut d, &mut ram, REG_TX_TAIL, tail);
    assert_eq!(wire.sent(), want);
    assert_eq!(r(&d, REG_TX_HEAD), 2);
    // u32::MAX - 1 and u32::MAX are slots 2 and 3; 0 and 1 are slots 0 and 1.
    for slot in 0..4 {
        assert_eq!(desc(&mut ram, TX_RING, slot), (30, 0));
    }
}

#[test]
fn bad_transmit_descriptors_are_errors_not_panics() {
    let (mut d, wire, mut ram) = setup();
    rings(&mut d, &mut ram, 16);
    let end = RAM_BASE + RAM_SIZE as u32;
    let cases: [(u32, u16, u32); 11] = [
        (BUFS, 0, status::BAD_LENGTH),
        (BUFS, MIN_FRAME as u16 - 1, status::BAD_LENGTH),
        (BUFS, MAX_FRAME as u16 + 1, status::BAD_LENGTH),
        (BUFS, u16::MAX, status::BAD_LENGTH),
        (0, 60, status::BAD_DMA),            // not RAM
        (RAM_BASE - 1, 60, status::BAD_DMA), // starts just below RAM
        (end - 59, 60, status::BAD_DMA),     // one byte past the end
        (end, 60, status::BAD_DMA),          // starts at the end
        (0xffff_fff0, 60, status::BAD_DMA),  // would wrap around
        (VRAM_BASE, 60, status::BAD_DMA),    // memory, but not RAM
        (BUFS, 60, status::OK),              // and the next good one goes
    ];
    for (i, &(addr, len, _)) in cases.iter().enumerate() {
        set_desc(&mut ram, TX_RING, i as u32, addr, len);
    }
    at(&mut ram, BUFS, 60).copy_from_slice(&frame(60, 9));
    w(&mut d, &mut ram, REG_TX_TAIL, cases.len() as u32);
    for (i, &(addr, len, st)) in cases.iter().enumerate() {
        assert_eq!(
            desc(&mut ram, TX_RING, i as u32),
            (len, st as u16),
            "addr {addr:#x} len {len}"
        );
    }
    assert_eq!(wire.sent(), [frame(60, 9)], "only the good one");
    assert_eq!(r(&d, REG_TX_HEAD), cases.len() as u32);
    assert_eq!(
        r(&d, REG_ERROR),
        error::NONE,
        "descriptor errors are not ring errors"
    );
}

#[test]
fn a_transmit_ring_outside_ram_stops_transmit() {
    let (mut d, wire, mut ram) = setup();
    let end = RAM_BASE + RAM_SIZE as u32;
    for base in [0, RAM_BASE - 8, end - 4, end, 0xffff_fffc] {
        w(&mut d, &mut ram, REG_TX_BASE, base);
        w(&mut d, &mut ram, REG_TX_SIZE, 2);
        w(&mut d, &mut ram, REG_IRQ_STATUS, u32::MAX);
        w(&mut d, &mut ram, REG_TX_TAIL, 1);
        assert_eq!(r(&d, REG_ERROR), error::BAD_RING, "{base:#x}");
        assert_eq!(r(&d, REG_IRQ_STATUS), IRQ_ERROR, "{base:#x}: no TX completion");
        assert_eq!([r(&d, REG_TX_HEAD), r(&d, REG_TX_TAIL)], [0, 1]);
    }
    // A ring whose first slot is in RAM but whose second is not: the first goes.
    w(&mut d, &mut ram, REG_ERROR, 0);
    w(&mut d, &mut ram, REG_TX_BASE, end - 8);
    at(&mut ram, BUFS, 20).copy_from_slice(&frame(20, 1));
    set_desc(&mut ram, end - 8, 0, BUFS, 20);
    w(&mut d, &mut ram, REG_TX_TAIL, 2);
    assert_eq!(wire.sent().len(), 1);
    assert_eq!([r(&d, REG_TX_HEAD), r(&d, REG_ERROR)], [1, error::BAD_RING]);
    // The driver recovers by setting the ring up again.
    w(&mut d, &mut ram, REG_TX_BASE, TX_RING);
    assert_eq!([r(&d, REG_TX_HEAD), r(&d, REG_TX_TAIL)], [0, 0]);
}

#[test]
fn backend_failures_are_io_errors() {
    let (mut d, wire, mut ram) = setup();
    rings(&mut d, &mut ram, 2);
    wire.0.lock().unwrap().fail = true;
    let tail = queue_tx(&d, &mut ram, 0, 0, &frame(60, 0));
    w(&mut d, &mut ram, REG_TX_TAIL, tail);
    assert_eq!(desc(&mut ram, TX_RING, 0), (60, status::IO as u16));
    assert_eq!(r(&d, REG_TX_HEAD), 1);
    wire.0.lock().unwrap().fail = false;
    let tail = queue_tx(&d, &mut ram, tail, 1, &frame(60, 1));
    w(&mut d, &mut ram, REG_TX_TAIL, tail);
    assert_eq!(desc(&mut ram, TX_RING, 1), (60, status::OK as u16));
    assert_eq!(wire.sent(), [frame(60, 1)]);
}

#[test]
fn receive_fills_the_buffer_at_head() {
    let (mut d, wire, mut ram) = setup();
    rings(&mut d, &mut ram, 4);
    give_rx(&mut d, &mut ram, 0, 0);
    let f = frame(60, 0x77);
    wire.arrive(f.clone());
    assert_eq!(r(&d, REG_RX_HEAD), 0, "nothing until the card polls");
    d.poll(&mut ram, &mut |_, _| {});
    assert_eq!(r(&d, REG_RX_HEAD), 1);
    assert_eq!(at(&mut ram, BUFS, 60), &f[..]);
    assert_eq!(at(&mut ram, BUFS + 60, 1), [0], "nothing past the frame");
    assert_eq!(desc(&mut ram, RX_RING, 0), (60, status::OK as u16));
    assert_eq!(r(&d, REG_IRQ_STATUS), IRQ_RX);
    assert_eq!(r(&d, REG_RX_DROPPED), 0);

    // A buffer exactly as long as the frame, at the very end of RAM.
    let end = RAM_BASE + RAM_SIZE as u32;
    set_desc(&mut ram, RX_RING, 1, end - MAX_FRAME as u32, MAX_FRAME as u16);
    w(&mut d, &mut ram, REG_RX_TAIL, 2);
    let long = frame(MAX_FRAME, 3);
    wire.arrive(long.clone());
    d.poll(&mut ram, &mut |_, _| {});
    assert_eq!(desc(&mut ram, RX_RING, 1), (MAX_FRAME as u16, 0));
    assert_eq!(at(&mut ram, end - MAX_FRAME as u32, MAX_FRAME), &long[..]);
}

/// Receive DMA is not a hart store, so the card reports every range it writes (for the
/// LR/SC reservation): the frame in its buffer and the descriptor's length and status.
#[test]
fn receive_reports_the_ram_it_writes() {
    let (mut d, wire, mut ram) = setup();
    rings(&mut d, &mut ram, 4);
    let tail = give_rx(&mut d, &mut ram, 0, 0);
    // The second buffer is too small: only its descriptor is written.
    set_desc(&mut ram, RX_RING, 1, BUFS + 0x800, 10);
    w(&mut d, &mut ram, REG_RX_TAIL, tail + 1);
    wire.arrive(frame(60, 1));
    wire.arrive(frame(60, 2));
    // No buffer left: dropped without a write.
    wire.arrive(frame(60, 3));
    let mut writes = Vec::new();
    d.poll(&mut ram, &mut |addr, len| writes.push((addr as u32, len)));
    assert_eq!(
        writes,
        [(BUFS, 60), (RX_RING + 4, 4), (RX_RING + DESC_SIZE + 4, 4)]
    );
    // Nothing arrives, nothing is written; tick and skip poll the same way.
    writes.clear();
    d.skip(POLL_STEPS as u64, &mut ram, &mut |addr, len| {
        writes.push((addr as u32, len))
    });
    assert_eq!(writes, []);
    give_rx(&mut d, &mut ram, tail + 1, 0);
    wire.arrive(frame(20, 4));
    for _ in 0..POLL_STEPS {
        d.tick(&mut ram, &mut |addr, len| writes.push((addr as u32, len)));
    }
    assert_eq!(writes, [(BUFS, 20), (RX_RING + 2 * DESC_SIZE + 4, 4)]);
}

/// Completing a transmit descriptor is a RAM write too: the card reports each
/// descriptor's length and status, whether the frame went or not, and nothing else.
#[test]
fn transmit_reports_the_ram_it_writes() {
    let (mut d, wire, mut ram) = setup();
    rings(&mut d, &mut ram, 4);
    let tail = queue_tx(&d, &mut ram, 0, 0, &frame(60, 1));
    // A bad length: not sent, but its descriptor still gets a status.
    set_desc(&mut ram, TX_RING, 1, BUFS, 3);
    let mut writes = Vec::new();
    d.write(REG_TX_TAIL, 4, (tail + 1).into(), &mut ram, &mut |addr, len| {
        writes.push((addr as u32, len))
    })
    .unwrap();
    assert_eq!(wire.sent().len(), 1);
    assert_eq!(writes, [(TX_RING + 4, 4), (TX_RING + DESC_SIZE + 4, 4)]);
    // Other registers write no RAM.
    writes.clear();
    d.write(REG_IRQ_STATUS, 4, IRQ_ALL.into(), &mut ram, &mut |addr, len| {
        writes.push((addr as u32, len))
    })
    .unwrap();
    assert_eq!(writes, []);
}

#[test]
fn a_full_receive_ring_drops_and_counts() {
    let (mut d, wire, mut ram) = setup();
    // No ring yet: everything that arrives is dropped (and taken from the backend).
    for i in 0..3 {
        wire.arrive(frame(20, i));
    }
    d.poll(&mut ram, &mut |_, _| {});
    assert_eq!(wire.waiting(), 0);
    assert_eq!(r(&d, REG_RX_DROPPED), 3);
    assert_eq!(
        r(&d, REG_IRQ_STATUS),
        0,
        "drops without a descriptor do not interrupt"
    );

    rings(&mut d, &mut ram, 2);
    let mut tail = 0;
    tail = give_rx(&mut d, &mut ram, tail, 0);
    tail = give_rx(&mut d, &mut ram, tail, 1);
    for i in 0..5 {
        wire.arrive(frame(20, 10 + i));
    }
    d.poll(&mut ram, &mut |_, _| {});
    assert_eq!(r(&d, REG_RX_HEAD), 2, "two buffers, two frames");
    assert_eq!(r(&d, REG_RX_DROPPED), 6);
    assert_eq!(at(&mut ram, BUFS + 19, 1), [10]);
    assert_eq!(at(&mut ram, BUFS + 0x800 + 19, 1), [11]);

    // Once the driver gives the buffers back, frames get through again: the ring wraps.
    for round in 0..3u8 {
        tail = give_rx(&mut d, &mut ram, tail, 0);
        wire.arrive(frame(20, 0x40 + round));
        d.poll(&mut ram, &mut |_, _| {});
        assert_eq!(r(&d, REG_RX_HEAD), tail);
        assert_eq!(at(&mut ram, BUFS + 19, 1), [0x40 + round]);
        let slot = (tail - 1) % 2;
        assert_eq!(desc(&mut ram, RX_RING, slot), (20, 0));
    }
    assert_eq!(r(&d, REG_RX_DROPPED), 6);
    // Any write resets the counter.
    w(&mut d, &mut ram, REG_RX_DROPPED, 99);
    assert_eq!(r(&d, REG_RX_DROPPED), 0);
}

#[test]
fn bad_receive_descriptors_are_errors_not_panics() {
    let (mut d, wire, mut ram) = setup();
    rings(&mut d, &mut ram, 8);
    let end = RAM_BASE + RAM_SIZE as u32;
    // (buffer, capacity, status) for a 100-byte frame.
    let cases: [(u32, u16, u32); 7] = [
        (BUFS, 99, status::TOO_SMALL),
        (BUFS, 0, status::TOO_SMALL),
        (0, 0x800, status::BAD_DMA),
        (end - 0x7ff, 0x800, status::BAD_DMA), // the whole buffer must be RAM
        (0xffff_ff00, 0x800, status::BAD_DMA),
        (VRAM_BASE, 0x800, status::BAD_DMA),
        (BUFS, 100, status::OK),
    ];
    for (i, &(addr, cap, _)) in cases.iter().enumerate() {
        set_desc(&mut ram, RX_RING, i as u32, addr, cap);
        wire.arrive(frame(100, i as u8));
    }
    w(&mut d, &mut ram, REG_RX_TAIL, cases.len() as u32);
    d.poll(&mut ram, &mut |_, _| {});
    for (i, &(addr, cap, st)) in cases.iter().enumerate() {
        assert_eq!(
            desc(&mut ram, RX_RING, i as u32),
            (100, st as u16),
            "buffer {addr:#x} + {cap}"
        );
    }
    assert_eq!(
        r(&d, REG_RX_HEAD),
        cases.len() as u32,
        "every one was handed back"
    );
    assert_eq!(
        r(&d, REG_RX_DROPPED),
        cases.len() as u32 - 1,
        "the frames in them were lost"
    );
    assert_eq!(
        at(&mut ram, BUFS, 100),
        &frame(100, 6)[..],
        "only the good one copied"
    );
    assert_eq!(r(&d, REG_ERROR), error::NONE);
    assert_eq!(r(&d, REG_IRQ_STATUS), IRQ_RX);

    // Frames of a length the card never sends are dropped without using a buffer.
    give_rx(&mut d, &mut ram, cases.len() as u32, 0);
    for len in [0, 1, MIN_FRAME - 1, MAX_FRAME + 1, 9000] {
        wire.arrive(vec![0xee; len]);
    }
    d.poll(&mut ram, &mut |_, _| {});
    assert_eq!(r(&d, REG_RX_HEAD), cases.len() as u32);
    assert_eq!(r(&d, REG_RX_DROPPED), cases.len() as u32 - 1 + 5);
}

#[test]
fn a_receive_ring_outside_ram_drops_frames() {
    let (mut d, wire, mut ram) = setup();
    w(&mut d, &mut ram, REG_RX_BASE, 0x1000_0000);
    w(&mut d, &mut ram, REG_RX_SIZE, 4);
    w(&mut d, &mut ram, REG_RX_TAIL, 4);
    w(&mut d, &mut ram, REG_IRQ_ENABLE, IRQ_ERROR);
    wire.arrive(frame(60, 1));
    wire.arrive(frame(60, 2));
    d.poll(&mut ram, &mut |_, _| {});
    assert_eq!(r(&d, REG_ERROR), error::BAD_RING);
    assert_eq!(r(&d, REG_RX_DROPPED), 2);
    assert_eq!(r(&d, REG_RX_HEAD), 0);
    assert_eq!(r(&d, REG_IRQ_STATUS), IRQ_ERROR);
    assert!(d.irq());
    assert!(ram.iter().all(|&b| b == 0));
}

#[test]
fn polls_take_a_bounded_number_of_frames() {
    let (mut d, wire, mut ram) = setup();
    for i in 0..POLL_BUDGET + 10 {
        wire.arrive(frame(20, i as u8));
    }
    d.poll(&mut ram, &mut |_, _| {});
    assert_eq!(wire.waiting(), 10, "the rest wait for the next poll");
    assert_eq!(r(&d, REG_RX_DROPPED), POLL_BUDGET as u32);
    d.poll(&mut ram, &mut |_, _| {});
    assert_eq!(wire.waiting(), 0);
}

#[test]
fn ticks_poll_every_poll_steps() {
    let (mut d, wire, mut ram) = setup();
    rings(&mut d, &mut ram, 4);
    give_rx(&mut d, &mut ram, 0, 0);
    wire.arrive(frame(20, 1));
    for _ in 0..POLL_STEPS - 1 {
        d.tick(&mut ram, &mut |_, _| {});
    }
    assert_eq!(r(&d, REG_RX_HEAD), 0);
    d.tick(&mut ram, &mut |_, _| {});
    assert_eq!(r(&d, REG_RX_HEAD), 1, "the {POLL_STEPS}th tick polled");
    wire.arrive(frame(20, 2));
    for _ in 0..POLL_STEPS {
        d.tick(&mut ram, &mut |_, _| {});
    }
    assert_eq!(r(&d, REG_RX_DROPPED), 1, "and the next {POLL_STEPS} did again");

    // Steps skipped in bulk count like ticks: a poll falls due within them.
    wire.arrive(frame(20, 3));
    d.skip(POLL_STEPS as u64 - 1, &mut ram, &mut |_, _| {});
    assert_eq!(wire.waiting(), 1);
    d.skip(1, &mut ram, &mut |_, _| {});
    assert_eq!(wire.waiting(), 0);
    wire.arrive(frame(20, 4));
    d.skip(1_000_000, &mut ram, &mut |_, _| {});
    assert_eq!(wire.waiting(), 0, "one poll for many steps");
    d.tick(&mut ram, &mut |_, _| {});
    wire.arrive(frame(20, 5));
    d.skip(POLL_STEPS as u64 - 2, &mut ram, &mut |_, _| {});
    assert_eq!(wire.waiting(), 1, "the count started over after that poll");
    d.tick(&mut ram, &mut |_, _| {});
    assert_eq!(wire.waiting(), 0);
}

#[test]
fn interrupt_follows_enable_and_status() {
    let (mut d, wire, mut ram) = setup();
    rings(&mut d, &mut ram, 4);
    give_rx(&mut d, &mut ram, 0, 0);
    wire.arrive(frame(20, 1));
    d.poll(&mut ram, &mut |_, _| {});
    let tail = queue_tx(&d, &mut ram, 0, 1, &frame(20, 2));
    w(&mut d, &mut ram, REG_TX_TAIL, tail);
    assert_eq!(r(&d, REG_IRQ_STATUS), IRQ_RX | IRQ_TX);
    assert!(!d.irq(), "nothing enabled");

    w(&mut d, &mut ram, REG_IRQ_ENABLE, IRQ_TX);
    assert!(d.irq());
    w(&mut d, &mut ram, REG_IRQ_STATUS, IRQ_RX);
    assert!(d.irq(), "acknowledging RX leaves TX");
    assert_eq!(r(&d, REG_IRQ_STATUS), IRQ_TX);
    w(&mut d, &mut ram, REG_IRQ_STATUS, 0);
    assert!(d.irq(), "writing 0 acknowledges nothing");
    w(&mut d, &mut ram, REG_IRQ_STATUS, IRQ_TX);
    assert!(!d.irq());
    assert_eq!(r(&d, REG_IRQ_STATUS), 0);

    // Enabled first, then the event: the line rises with it.
    w(&mut d, &mut ram, REG_IRQ_ENABLE, IRQ_RX);
    give_rx(&mut d, &mut ram, 1, 1);
    assert!(!d.irq());
    wire.arrive(frame(20, 3));
    d.poll(&mut ram, &mut |_, _| {});
    assert!(d.irq());
    w(&mut d, &mut ram, REG_IRQ_ENABLE, 0);
    assert!(!d.irq());
    assert_eq!(r(&d, REG_IRQ_STATUS), IRQ_RX, "still set, just not interrupting");
}

#[test]
fn registers_are_word_only() {
    let (mut d, wire, mut ram) = setup();
    rings(&mut d, &mut ram, 4);
    queue_tx(&d, &mut ram, 0, 0, &frame(20, 0));
    for size in [1, 2] {
        assert_eq!(d.read(REG_MAC_LO, size), Err(BusFault));
        let no_dma = &mut |_, _| {};
        assert_eq!(d.write(REG_TX_TAIL, size, 1, &mut ram, no_dma), Err(BusFault));
        assert_eq!(d.write(REG_TX_BASE, size, 0, &mut ram, no_dma), Err(BusFault));
    }
    assert!(wire.sent().is_empty(), "a faulting write sends nothing");
    assert_eq!(r(&d, REG_TX_TAIL), 0);
    assert_eq!(r(&d, REG_TX_BASE), TX_RING);
}

#[test]
fn mac_addresses() {
    let mac = MacAddr::parse("02:CA:00:0a:ff:01").unwrap();
    assert_eq!(mac, MacAddr([2, 0xca, 0, 10, 0xff, 1]));
    assert_eq!(mac.to_string(), "02:ca:00:0a:ff:01");
    assert_eq!(MacAddr::parse("02-ca-00-0a-ff-01"), Ok(mac));
    assert!(mac.is_local() && !mac.is_multicast());
    assert!(MacAddr::BROADCAST.is_multicast());
    assert!(!MacAddr([0x00, 0x1b, 0x21, 1, 2, 3]).is_local());
    // A card's address is individual and locally administered.
    assert_eq!(mac.check_card(), Ok(()));
    assert!(MacAddr::BROADCAST
        .check_card()
        .unwrap_err()
        .contains("group address"));
    assert!(MacAddr([0x03, 0, 0, 0, 0, 1])
        .check_card()
        .unwrap_err()
        .contains("group address"));
    assert!(MacAddr([0x00, 0x1b, 0x21, 1, 2, 3])
        .check_card()
        .unwrap_err()
        .contains("not locally administered"));
    for bad in [
        "",
        "02:ca:00:0a:ff",
        "02:ca:00:0a:ff:01:02",
        "02:ca:00:0a:ff:1",
        "02:ca:00:0a:ff:001",
        "02:ca:00:0a:ff:zz",
        "02:ca-00:0a:ff:01",
        "02ca000aff01",
        "+2:ca:00:0a:ff:01",
    ] {
        assert!(MacAddr::parse(bad).is_err(), "{bad:?}");
    }
    assert_eq!(MacAddr::local(1).to_string(), "02:ca:00:00:00:01");
    assert_eq!(MacAddr::local(0x01_0203).to_string(), "02:ca:00:01:02:03");
}

#[test]
fn devices_and_backends_are_send() {
    fn assert_send<T: Send + ?Sized>() {}
    assert_send::<dyn NetBackend>();
    assert_send::<NetDevice>();
    assert_send::<Hub>();
    assert_send::<HubPort>();
    assert_send::<Pcap>();
}

// Version 2: an RV64 machine's card, with 64-bit ring bases and 16-byte descriptors.

fn setup_v2() -> (NetDevice, TestBackend, Vec<u8>) {
    let (mut d, wire, ram) = setup();
    d.set_xlen(Xlen::Rv64);
    (d, wire, ram)
}

/// Writes `val` to `off` with a `size`-byte access, which must not fault.
fn w2(d: &mut NetDevice, ram: &mut [u8], off: u32, size: u32, val: u64) {
    d.write(off, size, val, ram, &mut |_, _| {}).unwrap();
}

/// The RAM bytes of 16-byte descriptor `slot` of the ring at `ring`.
fn desc_v2_bytes(ram: &mut [u8], ring: u32, slot: u32) -> &mut [u8] {
    at(ram, ring + slot * v2::DESC_SIZE, 16)
}

/// Writes 16-byte descriptor `slot`: buffer address and length; status 0xffff and the
/// reserved word 0xaaaa_aaaa, so the test sees what the card writes.
fn set_desc_v2(ram: &mut [u8], ring: u32, slot: u32, addr: u64, len: u16) {
    let d = desc_v2_bytes(ram, ring, slot);
    d[..8].copy_from_slice(&addr.to_le_bytes());
    d[8..10].copy_from_slice(&len.to_le_bytes());
    d[10..12].copy_from_slice(&0xffffu16.to_le_bytes());
    d[12..].copy_from_slice(&0xaaaa_aaaau32.to_le_bytes());
}

/// Descriptor `slot`'s length, status and reserved fields.
fn desc_v2(ram: &mut [u8], ring: u32, slot: u32) -> (u16, u16, u32) {
    let d = desc_v2_bytes(ram, ring, slot);
    (
        u16::from_le_bytes([d[8], d[9]]),
        u16::from_le_bytes([d[10], d[11]]),
        u32::from_le_bytes([d[12], d[13], d[14], d[15]]),
    )
}

#[test]
fn version_2_registers() {
    let (mut d, _, mut ram) = setup_v2();
    // The registers before the rings are version 1's.
    assert_eq!(d.read(REG_MAC_LO, 4), Ok(0x2211_ca02));
    assert_eq!(d.read(REG_STATUS, 4), Ok(STATUS_LINK_UP.into()));
    // The ring bases are 64-bit: whole, or by halves keeping the other half.
    w2(&mut d, &mut ram, v2::REG_TX_BASE, 8, 0x1_2345_6780);
    assert_eq!(d.read(v2::REG_TX_BASE, 8), Ok(0x1_2345_6780));
    assert_eq!(d.read(v2::REG_TX_BASE, 4), Ok(0x2345_6780));
    assert_eq!(d.read(v2::REG_TX_BASE + 4, 4), Ok(1));
    w2(&mut d, &mut ram, v2::REG_RX_BASE, 4, RX_RING.into());
    w2(&mut d, &mut ram, v2::REG_RX_BASE + 4, 4, 2);
    assert_eq!(d.read(v2::REG_RX_BASE, 8), Ok(0x2_0000_0000 | u64::from(RX_RING)));
    w2(&mut d, &mut ram, v2::REG_RX_BASE + 4, 4, 0);
    assert_eq!(d.read(v2::REG_RX_BASE, 8), Ok(RX_RING.into()));
    // The 32-bit ring registers, at their new offsets.
    w2(&mut d, &mut ram, v2::REG_TX_SIZE, 4, 8);
    w2(&mut d, &mut ram, v2::REG_RX_SIZE, 4, 4);
    w2(&mut d, &mut ram, v2::REG_RX_TAIL, 4, 3);
    assert_eq!(d.read(v2::REG_TX_SIZE, 4), Ok(8));
    assert_eq!(d.read(v2::REG_RX_SIZE, 4), Ok(4));
    assert_eq!(d.read(v2::REG_RX_TAIL, 4), Ok(3));
    assert_eq!(d.read(v2::REG_RX_HEAD, 4), Ok(0));
    assert_eq!(d.read(v2::REG_TX_HEAD, 4), Ok(0));
    assert_eq!(d.read(0x34, 4), Ok(0), "reserved");
    assert_eq!(d.read(0x50, 8), Ok(0), "unused");
    // 32-bit registers take no 8-byte access, and a faulting write does nothing.
    assert_eq!(d.read(v2::REG_RX_SIZE, 8), Err(BusFault));
    assert_eq!(
        d.read(REG_RX_DROPPED, 8),
        Err(BusFault),
        "a counter and reserved space"
    );
    assert_eq!(
        d.write(v2::REG_RX_SIZE, 8, 1, &mut ram, &mut |_, _| {}),
        Err(BusFault)
    );
    assert_eq!(d.read(v2::REG_RX_SIZE, 4), Ok(4));
    assert_eq!(d.read(v2::REG_TX_BASE + 4, 8), Err(BusFault), "misaligned");
}

/// A version 2 ring's base is a multiple of 16: another is kept in the register, but
/// the ring has no descriptors and NET_ERROR says why.
#[test]
fn version_2_ring_bases_are_aligned() {
    let (mut d, _, mut ram) = setup_v2();
    w2(&mut d, &mut ram, v2::REG_RX_SIZE, 4, 4);
    w2(&mut d, &mut ram, v2::REG_RX_BASE, 8, u64::from(RX_RING) + 8);
    assert_eq!(
        d.read(v2::REG_RX_BASE, 8),
        Ok(u64::from(RX_RING) + 8),
        "it reads back"
    );
    assert_eq!(d.read(v2::REG_RX_SIZE, 4), Ok(0));
    assert_eq!(d.read(REG_ERROR, 4), Ok(error::BAD_BASE.into()));
    assert_eq!(d.read(REG_IRQ_STATUS, 4), Ok(IRQ_ERROR.into()));
    // A SIZE is refused the same way while the base is wrong; turning the ring off is not.
    w2(&mut d, &mut ram, REG_ERROR, 4, 0);
    w2(&mut d, &mut ram, v2::REG_RX_SIZE, 4, 0);
    assert_eq!(d.read(REG_ERROR, 4), Ok(0));
    w2(&mut d, &mut ram, v2::REG_RX_SIZE, 4, 4);
    assert_eq!(d.read(v2::REG_RX_SIZE, 4), Ok(0));
    assert_eq!(d.read(REG_ERROR, 4), Ok(error::BAD_BASE.into()));
    // Fixed, by the low half alone; then the size takes.
    w2(&mut d, &mut ram, REG_ERROR, 4, 0);
    w2(&mut d, &mut ram, v2::REG_RX_BASE, 4, RX_RING.into());
    w2(&mut d, &mut ram, v2::REG_RX_SIZE, 4, 4);
    assert_eq!(d.read(v2::REG_RX_SIZE, 4), Ok(4));
    assert_eq!(d.read(REG_ERROR, 4), Ok(0));
    // Only the low bits count: the high half never misaligns a base.
    w2(&mut d, &mut ram, v2::REG_TX_SIZE, 4, 2);
    w2(&mut d, &mut ram, v2::REG_TX_BASE + 4, 4, 0xffff_ffff);
    assert_eq!(d.read(REG_ERROR, 4), Ok(0));
    assert_eq!(d.read(v2::REG_TX_SIZE, 4), Ok(2));
    // Version 1's descriptors may be anywhere.
    let (mut d, _, mut ram) = setup();
    rings(&mut d, &mut ram, 4);
    w(&mut d, &mut ram, REG_RX_BASE, RX_RING + 8);
    w(&mut d, &mut ram, REG_RX_SIZE, 4);
    assert_eq!(r(&d, REG_RX_SIZE), 4);
    assert_eq!(r(&d, REG_ERROR), error::NONE);
}

#[test]
fn version_2_descriptors_move_frames() {
    let (mut d, wire, mut ram) = setup_v2();
    w2(&mut d, &mut ram, v2::REG_TX_BASE, 8, TX_RING.into());
    w2(&mut d, &mut ram, v2::REG_TX_SIZE, 4, 4);
    w2(&mut d, &mut ram, v2::REG_RX_BASE, 8, RX_RING.into());
    w2(&mut d, &mut ram, v2::REG_RX_SIZE, 4, 4);

    // Transmit: one good frame, then buffers above 4 GiB and at the top of the
    // address space, which are not RAM.
    let f = frame(60, 9);
    at(&mut ram, BUFS, 60).copy_from_slice(&f);
    set_desc_v2(&mut ram, TX_RING, 0, BUFS.into(), 60);
    set_desc_v2(&mut ram, TX_RING, 1, u64::from(BUFS) + (1 << 32), 60);
    set_desc_v2(&mut ram, TX_RING, 2, u64::MAX - 10, 60);
    let mut writes = Vec::new();
    d.write(v2::REG_TX_TAIL, 4, 3, &mut ram, &mut |addr, len| {
        writes.push((addr, len))
    })
    .unwrap();
    assert_eq!(wire.sent(), [f]);
    assert_eq!(
        desc_v2(&mut ram, TX_RING, 0),
        (60, status::OK as u16, 0xaaaa_aaaa)
    );
    assert_eq!(
        desc_v2(&mut ram, TX_RING, 1),
        (60, status::BAD_DMA as u16, 0xaaaa_aaaa)
    );
    assert_eq!(
        desc_v2(&mut ram, TX_RING, 2),
        (60, status::BAD_DMA as u16, 0xaaaa_aaaa)
    );
    assert_eq!(d.read(v2::REG_TX_HEAD, 4), Ok(3));
    let tx = u64::from(TX_RING);
    assert_eq!(writes, [(tx + 8, 4), (tx + 24, 4), (tx + 40, 4)]);

    // Receive into the buffer of the descriptor at RX_HEAD.
    set_desc_v2(&mut ram, RX_RING, 0, u64::from(BUFS) + 0x800, 0x800);
    w2(&mut d, &mut ram, v2::REG_RX_TAIL, 4, 1);
    wire.arrive(frame(60, 7));
    writes.clear();
    d.poll(&mut ram, &mut |addr, len| writes.push((addr, len)));
    assert_eq!(at(&mut ram, BUFS + 0x800, 60), frame(60, 7));
    assert_eq!(
        desc_v2(&mut ram, RX_RING, 0),
        (60, status::OK as u16, 0xaaaa_aaaa)
    );
    assert_eq!(d.read(v2::REG_RX_HEAD, 4), Ok(1));
    let rx = u64::from(RX_RING);
    assert_eq!(writes, [(u64::from(BUFS) + 0x800, 60), (rx + 8, 4)]);
}

/// A ring whose descriptors would run past 2^64 is a ring error, not a wrap-around.
#[test]
fn a_version_2_ring_at_the_top_of_the_address_space() {
    let (mut d, wire, mut ram) = setup_v2();
    w2(&mut d, &mut ram, v2::REG_TX_BASE, 8, u64::MAX - 15);
    w2(&mut d, &mut ram, v2::REG_TX_SIZE, 4, 4);
    w2(&mut d, &mut ram, v2::REG_TX_TAIL, 4, 2);
    assert_eq!(d.read(REG_ERROR, 4), Ok(error::BAD_RING.into()));
    assert_eq!(d.read(v2::REG_TX_HEAD, 4), Ok(0));
    assert!(wire.sent().is_empty());
}
