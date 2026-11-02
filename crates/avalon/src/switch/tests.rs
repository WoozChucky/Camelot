use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

const A: MacAddr = MacAddr([0x02, 0, 0, 0, 0, 0xa]);
const B: MacAddr = MacAddr([0x02, 0, 0, 0, 0, 0xb]);
const C: MacAddr = MacAddr([0x02, 0, 0, 0, 0, 0xc]);
const D: MacAddr = MacAddr([0x02, 0, 0, 0, 0, 0xd]);
const GROUP: MacAddr = MacAddr([0x01, 0, 0x5e, 0, 0, 1]);

/// A 60-byte Ethernet frame from `src` to `dst`, its payload all `tag`.
fn frame(dst: MacAddr, src: MacAddr, tag: u8) -> Vec<u8> {
    let mut f = Vec::new();
    f.extend(dst.0);
    f.extend(src.0);
    f.extend([0x88, 0xb5]);
    f.resize(60, tag);
    f
}

// ---- The MAC table ----

#[test]
fn unknown_and_group_destinations_flood_known_ones_go_to_their_port() {
    let t0 = Instant::now();
    let mut table = MacTable::new(MAC_AGEING, MAC_TABLE_SIZE);
    // Nobody is known yet: flood, and learn A on port 1.
    assert_eq!(table.forward(1, &frame(B, A, 0), t0), Forward::Flood);
    assert_eq!(table.lookup(A, t0), Some(1));
    // B answers A from port 2: A is known, B is learned.
    assert_eq!(table.forward(2, &frame(A, B, 0), t0), Forward::Port(1));
    assert_eq!(table.forward(1, &frame(B, A, 0), t0), Forward::Port(2));
    // Broadcast and multicast always flood, known sender or not.
    assert_eq!(
        table.forward(1, &frame(MacAddr::BROADCAST, A, 0), t0),
        Forward::Flood
    );
    assert_eq!(table.forward(2, &frame(GROUP, B, 0), t0), Forward::Flood);
    // Still nobody knows C.
    assert_eq!(table.forward(2, &frame(C, B, 0), t0), Forward::Flood);
    // A frame for an address on the port it came from goes nowhere.
    assert_eq!(table.forward(1, &frame(A, D, 0), t0), Forward::Filter);
    assert_eq!(table.lookup(D, t0), Some(1));
    assert_eq!(table.len(), 3);
}

#[test]
fn group_sources_are_not_learned() {
    let t0 = Instant::now();
    let mut table = MacTable::new(MAC_AGEING, MAC_TABLE_SIZE);
    table.forward(1, &frame(A, GROUP, 0), t0);
    table.forward(1, &frame(A, MacAddr::BROADCAST, 0), t0);
    assert!(table.is_empty());
    assert_eq!(table.lookup(GROUP, t0), None);
}

#[test]
fn a_moved_address_is_learned_on_its_new_port() {
    let t0 = Instant::now();
    let mut table = MacTable::new(MAC_AGEING, MAC_TABLE_SIZE);
    table.learn(A, 1, t0);
    table.learn(A, 3, t0 + Duration::from_secs(1));
    assert_eq!(table.lookup(A, t0 + Duration::from_secs(1)), Some(3));
    assert_eq!(table.len(), 1);
}

#[test]
fn learned_addresses_age() {
    let t0 = Instant::now();
    let ageing = Duration::from_secs(300);
    let mut table = MacTable::new(ageing, MAC_TABLE_SIZE);
    table.learn(A, 1, t0);
    table.learn(B, 2, t0);
    let almost = t0 + ageing - Duration::from_millis(1);
    assert_eq!(table.lookup(A, almost), Some(1));
    // A frame from B refreshes it; A says nothing.
    table.learn(B, 2, t0 + Duration::from_secs(200));
    let later = t0 + ageing;
    assert_eq!(table.lookup(A, later), None, "aged out");
    assert_eq!(table.lookup(B, later), Some(2), "refreshed at 200 s");
    // An aged destination floods again, as if it had never been seen.
    assert_eq!(table.forward(2, &frame(A, B, 0), later), Forward::Flood);
    // Expiry cleans out the aged entries.
    assert_eq!(table.len(), 2);
    table.expire(later);
    assert_eq!(table.len(), 1);
    // B was refreshed again at 300 s, by the frame it sent then.
    table.expire(t0 + Duration::from_secs(600));
    assert!(table.is_empty());
    // A clock that went backwards (it cannot, but) keeps entries rather than panic.
    table.learn(C, 4, later);
    assert_eq!(table.lookup(C, t0), Some(4));
}

#[test]
fn the_table_is_bounded() {
    let t0 = Instant::now();
    let mut table = MacTable::new(Duration::from_secs(10), 3);
    for (i, mac) in [A, B, C].into_iter().enumerate() {
        table.learn(mac, i as u32, t0);
    }
    // Full of fresh entries: D is not learned, and frames for it flood.
    table.learn(D, 9, t0);
    assert_eq!(table.len(), 3);
    assert_eq!(table.lookup(D, t0), None);
    assert_eq!(table.forward(0, &frame(D, A, 0), t0), Forward::Flood);
    // Known addresses still refresh.
    table.learn(A, 7, t0 + Duration::from_secs(5));
    assert_eq!(table.lookup(A, t0 + Duration::from_secs(5)), Some(7));
    // Once B and C have aged, D finds room.
    let later = t0 + Duration::from_secs(12);
    table.learn(D, 9, later);
    assert_eq!(table.lookup(D, later), Some(9));
    assert_eq!(table.len(), 2, "A (refreshed at 5 s) and D");
}

#[test]
fn unplugged_ports_are_forgotten() {
    let t0 = Instant::now();
    let mut table = MacTable::new(MAC_AGEING, MAC_TABLE_SIZE);
    table.learn(A, 1, t0);
    table.learn(B, 1, t0);
    table.learn(C, 2, t0);
    table.forget_port(1);
    assert_eq!((table.lookup(A, t0), table.lookup(C, t0)), (None, Some(2)));
    assert_eq!(table.len(), 1);
}

// ---- Switches, ports and cards ----

/// Card 0 of a VM, with address `mac`: the card, its backend (what its machine would
/// hold), and how often its bell rang.
struct TestCard {
    card: Arc<Card>,
    backend: CardBackend,
    rings: Arc<AtomicUsize>,
}

impl TestCard {
    fn new(mac: MacAddr) -> TestCard {
        let card = Card::new(0, mac);
        let rings = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&rings);
        card.set_bell(Arc::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
        }));
        TestCard {
            backend: card.backend(),
            card,
            rings,
        }
    }

    /// Card 0 of VM `vm`, plugged into `switch`.
    fn plugged(vm: u32, mac: MacAddr, switch: &Switch) -> TestCard {
        let card = TestCard::new(mac);
        card.card.plug(owner(vm), switch).unwrap();
        card
    }

    fn send(&mut self, frame: &[u8]) {
        self.backend.send(frame).unwrap();
    }

    fn recv_all(&mut self) -> Vec<Vec<u8>> {
        std::iter::from_fn(|| self.backend.recv()).collect()
    }

    fn rings(&self) -> usize {
        self.rings.load(Ordering::SeqCst)
    }
}

fn owner(vm: u32) -> PortOwner {
    PortOwner::Card { vm, card: 0 }
}

fn port_of(switch: &Switch, vm: u32) -> SwitchPort {
    *switch
        .info()
        .ports
        .iter()
        .find(|p| p.owner == owner(vm))
        .expect("plugged in")
}

#[test]
fn two_cards_on_a_switch_exchange_frames_and_a_third_learns_to_stop_seeing_them() {
    let lan = Switch::new("lan");
    let mut a = TestCard::plugged(1, A, &lan);
    let mut b = TestCard::plugged(2, B, &lan);
    let mut c = TestCard::plugged(3, C, &lan);
    assert!(a.backend.link_up());

    // A broadcasts: everyone else hears it, A does not hear itself.
    let hello = frame(MacAddr::BROADCAST, A, 1);
    a.send(&hello);
    assert_eq!(b.recv_all(), vec![hello.clone()]);
    assert_eq!(c.recv_all(), vec![hello.clone()]);
    assert_eq!(a.recv_all(), Vec::<Vec<u8>>::new());
    assert_eq!((a.rings(), b.rings(), c.rings()), (0, 1, 1));

    // B answers A: the switch learned A, so only A gets it.
    let answer = frame(A, B, 2);
    b.send(&answer);
    assert_eq!(a.recv_all(), vec![answer.clone()]);
    assert!(c.recv_all().is_empty(), "C does not see a frame for A");
    // And now B is known too.
    let more = frame(B, A, 3);
    a.send(&more);
    assert_eq!(b.recv_all(), vec![more.clone()]);
    assert!(c.recv_all().is_empty());
    // A frame for an address nobody has sent from yet floods.
    let to_d = frame(D, C, 4);
    c.send(&to_d);
    assert_eq!(a.recv_all(), vec![to_d.clone()]);
    assert_eq!(b.recv_all(), vec![to_d.clone()]);

    // Counters: "in" is what the card sent, "out" what the switch passed to it.
    let n = 60;
    let pa = port_of(&lan, 1);
    assert_eq!(pa.mac, A.0);
    assert_eq!((pa.in_frames, pa.in_bytes), (2, 2 * n));
    assert_eq!((pa.out_frames, pa.out_bytes), (2, 2 * n));
    let pc = port_of(&lan, 3);
    assert_eq!((pc.in_frames, pc.out_frames, pc.dropped), (1, 1, 0));
    assert_eq!(lan.learned(), 3);
    let info = lan.info();
    assert_eq!(info.name, "lan");
    assert_eq!(info.capture, None);
    assert_eq!(
        info.ports.iter().map(|p| p.owner).collect::<Vec<_>>(),
        [owner(1), owner(2), owner(3)],
        "in the order they were plugged in"
    );
}

#[test]
fn switches_are_separate_networks() {
    let lan = Switch::new("lan");
    let dmz = Switch::new("dmz");
    let mut a = TestCard::plugged(1, A, &lan);
    let mut b = TestCard::plugged(2, B, &lan);
    let mut c = TestCard::plugged(3, C, &dmz);
    a.send(&frame(MacAddr::BROADCAST, A, 1));
    a.send(&frame(C, A, 2));
    b.send(&frame(MacAddr::BROADCAST, B, 3));
    assert!(c.recv_all().is_empty(), "nothing crosses to another switch");
    assert_eq!(c.rings(), 0);
    assert_eq!(port_of(&dmz, 3).out_frames, 0);
    assert_eq!(b.recv_all().len(), 2);
}

#[test]
fn a_full_queue_drops_only_its_own_frames() {
    let lan = Switch::new("lan");
    let mut a = TestCard::plugged(1, A, &lan);
    let mut slow = TestCard::plugged(2, B, &lan);
    let mut c = TestCard::plugged(3, C, &lan);
    // B never takes its frames; broadcasts pile up in its queue and C's.
    for i in 0..PORT_QUEUE + 5 {
        a.send(&frame(MacAddr::BROADCAST, A, i as u8));
        // C keeps up.
        assert_eq!(c.recv_all().len(), 1);
    }
    let pb = port_of(&lan, 2);
    assert_eq!((pb.out_frames, pb.dropped), (PORT_QUEUE as u64, 5));
    assert_eq!(port_of(&lan, 3).dropped, 0);
    let got = slow.recv_all();
    assert_eq!(got.len(), PORT_QUEUE);
    assert_eq!(got[0][14], 0, "the oldest are kept");
    // Room again.
    a.send(&frame(B, A, 0xee));
    assert_eq!(slow.recv_all(), [frame(B, A, 0xee)]);
    assert!(a.recv_all().is_empty());
}

#[test]
fn switch_entries_age_out() {
    let lan = Switch::with_ageing("lan", Duration::from_secs(60));
    let ports: Vec<TestCard> = [A, B, C]
        .into_iter()
        .enumerate()
        .map(|(i, mac)| TestCard::plugged(i as u32 + 1, mac, &lan))
        .collect();
    let id = |vm: u32| {
        let st = lan.state();
        st.ports.iter().find(|p| p.owner == owner(vm)).unwrap().id
    };
    let (pa, pb) = (id(1), id(2));
    let t0 = Instant::now();
    let queued = |vm: u32| {
        let st = lan.state();
        st.ports
            .iter()
            .find(|p| p.owner == owner(vm))
            .unwrap()
            .queue
            .len()
    };
    // A is learned at t0; B's frame to A goes to A alone.
    lan.input_at(pa, &frame(MacAddr::BROADCAST, A, 0), t0);
    lan.input_at(pb, &frame(A, B, 1), t0 + Duration::from_secs(59));
    assert_eq!((queued(1), queued(3)), (1, 1), "C saw only the broadcast");
    // A minute after A last spoke, the switch has forgotten where it is: flood.
    lan.input_at(pb, &frame(A, B, 2), t0 + Duration::from_secs(60));
    assert_eq!((queued(1), queued(3)), (2, 2));
    drop(ports);
}

#[test]
fn runt_frames_are_counted_but_not_forwarded() {
    let lan = Switch::new("lan");
    let mut a = TestCard::plugged(1, A, &lan);
    let mut b = TestCard::plugged(2, B, &lan);
    a.send(&[0xff; 13]);
    assert!(b.recv_all().is_empty());
    let pa = port_of(&lan, 1);
    assert_eq!((pa.in_frames, pa.in_bytes), (1, 13));
    assert_eq!(lan.learned(), 0);
}

#[test]
fn cards_plug_in_and_out() {
    let lan = Switch::new("lan");
    let dmz = Switch::new("dmz");
    let mut a = TestCard::new(A);
    let mut b = TestCard::plugged(2, B, &lan);
    // Unplugged: link down, frames go nowhere.
    assert!(!a.backend.link_up());
    assert!(a.card.switch().is_none());
    a.send(&frame(MacAddr::BROADCAST, A, 0));
    assert!(b.recv_all().is_empty());
    assert!(!a.card.unplug(), "not plugged in");

    a.card.plug(owner(1), &lan).unwrap();
    assert!(a.backend.link_up());
    assert_eq!(a.card.switch().unwrap().name(), "lan");
    // Once is enough.
    let e = a.card.plug(owner(1), &dmz).unwrap_err();
    assert_eq!(e.code, ErrorCode::Busy);
    assert_eq!(
        e.message,
        "card 0 of VM 1 is plugged into switch 'lan'; detach it first"
    );

    a.send(&frame(MacAddr::BROADCAST, A, 1));
    b.send(&frame(A, B, 2));
    assert_eq!(b.recv_all().len(), 1);
    assert_eq!(lan.learned(), 2);
    // Unplugging forgets the card's queue and its address.
    assert!(a.card.unplug());
    assert!(!a.backend.link_up());
    assert_eq!(a.recv_all(), Vec::<Vec<u8>>::new());
    assert_eq!(lan.learned(), 1);
    assert_eq!(lan.info().ports.len(), 1);

    // Plugged into another switch, it is on that network.
    let mut c = TestCard::plugged(3, C, &dmz);
    a.card.plug(owner(1), &dmz).unwrap();
    c.send(&frame(MacAddr::BROADCAST, C, 3));
    assert_eq!(a.recv_all().len(), 1);
    assert!(b.recv_all().is_empty());

    // When the machine goes (its backend is dropped), the card is unplugged for good.
    let TestCard { card, backend, .. } = a;
    drop(backend);
    assert!(card.switch().is_none());
    assert_eq!(dmz.info().ports.len(), 1);
    let e = card.plug(owner(1), &lan).unwrap_err();
    assert_eq!(e.code, ErrorCode::InvalidState);
}

#[test]
fn two_cards_with_one_address_cannot_share_a_switch() {
    let lan = Switch::new("lan");
    let _a = TestCard::plugged(1, A, &lan);
    let twin = TestCard::new(A);
    let e = twin.card.plug(owner(2), &lan).unwrap_err();
    assert_eq!(e.code, ErrorCode::AlreadyExists);
    assert!(e.message.contains("card 0 of VM 1"), "{}", e.message);
    assert!(twin.card.switch().is_none());
    // Another switch is another network.
    twin.card.plug(owner(2), &Switch::new("dmz")).unwrap();
}

#[test]
fn a_deleted_switch_unplugs_its_cards() {
    let lan = Switch::new("lan");
    let mut a = TestCard::plugged(1, A, &lan);
    let mut b = TestCard::plugged(2, B, &lan);
    a.send(&frame(MacAddr::BROADCAST, A, 0));
    lan.close();
    assert!(!a.backend.link_up() && !b.backend.link_up());
    assert!(b.recv_all().is_empty(), "queued frames go with the switch");
    assert!(lan.info().ports.is_empty());
    let late = TestCard::new(C);
    assert_eq!(
        late.card.plug(owner(3), &lan).unwrap_err().code,
        ErrorCode::NoSuchSwitch
    );
    // The cards can go on another switch.
    a.card.plug(owner(1), &Switch::new("dmz")).unwrap();
}

#[test]
fn ports_work_across_threads() {
    let lan = Switch::new("lan");
    let mut here = TestCard::plugged(1, A, &lan);
    let mut there = TestCard::plugged(2, B, &lan);
    let echo = std::thread::spawn(move || {
        // Echo 100 frames back to their sender, each with its tag incremented.
        let mut n = 0;
        while n < 100 {
            match there.backend.recv() {
                Some(f) => {
                    let back = frame(MacAddr(f[6..12].try_into().unwrap()), B, f[14] + 1);
                    there.send(&back);
                    n += 1;
                }
                None => std::thread::yield_now(),
            }
        }
    });
    for i in 0..100u8 {
        here.send(&frame(B, A, i));
    }
    let mut got = Vec::new();
    while got.len() < 100 {
        match here.backend.recv() {
            Some(f) => got.push(f[14]),
            None => std::thread::yield_now(),
        }
    }
    echo.join().unwrap();
    assert_eq!(got, (1..=100).collect::<Vec<u8>>());
    assert_eq!(here.rings(), 100);
}

// ---- Captures ----

fn temp_file(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("avalon-switch-{}-{name}.pcap", std::process::id()))
}

/// The frames in a pcap file, after checking its header.
fn pcap_frames(bytes: &[u8]) -> Vec<Vec<u8>> {
    assert_eq!(bytes[..24], pcap_header());
    let mut rest = &bytes[24..];
    let mut frames = Vec::new();
    while !rest.is_empty() {
        let field = |i: usize| u32::from_le_bytes(rest[4 * i..4 * i + 4].try_into().unwrap()) as usize;
        let (caplen, len) = (field(2), field(3));
        assert_eq!(caplen, len);
        frames.push(rest[16..16 + len].to_vec());
        rest = &rest[16 + len..];
    }
    frames
}

#[test]
fn captures_record_every_frame_that_enters() {
    let path = temp_file("capture");
    let path_text = path.to_str().unwrap();
    let lan = Switch::new("lan");
    let mut a = TestCard::plugged(1, A, &lan);
    let mut b = TestCard::plugged(2, B, &lan);
    a.send(&frame(MacAddr::BROADCAST, A, 0)); // before the capture

    lan.start_capture(path_text).unwrap();
    assert_eq!(lan.info().capture.as_deref(), Some(path_text));
    let e = lan.start_capture(path_text).unwrap_err();
    assert_eq!(e.code, ErrorCode::Busy);
    let sent = [
        frame(MacAddr::BROADCAST, A, 1),
        frame(A, B, 2),
        // Filtered by the switch, captured all the same.
        frame(A, A, 3),
    ];
    a.send(&sent[0]);
    b.send(&sent[1]);
    a.send(&sent[2]);
    lan.stop_capture().unwrap();
    a.send(&frame(MacAddr::BROADCAST, A, 4)); // after it
    assert_eq!(lan.info().capture, None);

    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(pcap_frames(&bytes), sent);
    std::fs::remove_file(&path).unwrap();

    assert_eq!(lan.stop_capture().unwrap_err().code, ErrorCode::InvalidState);
    let bad = temp_file("no-such-dir").join("x.pcap");
    let e = lan.start_capture(bad.to_str().unwrap()).unwrap_err();
    assert_eq!(e.code, ErrorCode::Io);
    assert_eq!(lan.info().capture, None);
    drop((a.recv_all(), b.recv_all()));
}

#[test]
fn deleting_a_switch_completes_its_capture() {
    let path = temp_file("close");
    let lan = Switch::new("lan");
    let mut a = TestCard::plugged(1, A, &lan);
    let _b = TestCard::plugged(2, B, &lan);
    lan.start_capture(path.to_str().unwrap()).unwrap();
    a.send(&frame(B, A, 7));
    lan.close();
    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(pcap_frames(&bytes), [frame(B, A, 7)]);
    std::fs::remove_file(&path).unwrap();
    assert_eq!(
        lan.start_capture(path.to_str().unwrap()).unwrap_err().code,
        ErrorCode::NoSuchSwitch
    );
    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_capture_that_falls_behind_drops_rather_than_waits() {
    /// A file that takes no writes until it is let go.
    struct Stuck(Arc<(Mutex<bool>, std::sync::Condvar)>);
    impl Write for Stuck {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            let (open, cv) = &*self.0;
            let mut open = open.lock().unwrap();
            while !*open {
                open = cv.wait(open).unwrap();
            }
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let gate = Arc::new((Mutex::new(true), std::sync::Condvar::new()));
    let mut capture = Capture::start("stuck", Stuck(Arc::clone(&gate))).unwrap();
    *gate.0.lock().unwrap() = false;
    // Far more records than the channel holds: none of these calls may block.
    for i in 0..CAPTURE_QUEUE * 3 {
        capture.record(&frame(B, A, i as u8));
    }
    *gate.0.lock().unwrap() = true;
    gate.1.notify_all();
    capture.finish().unwrap();
}

#[test]
fn a_capture_whose_file_fails_says_so_when_stopped() {
    /// A file that takes its header, then fails every write (a full disk).
    struct Full(bool);
    impl Write for Full {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            if std::mem::replace(&mut self.0, false) {
                Ok(buf.len())
            } else {
                Err(io::Error::other("disk full"))
            }
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let lan = Switch::new("lan");
    let mut a = TestCard::plugged(1, A, &lan);
    let _b = TestCard::plugged(2, B, &lan);
    lan.capture_into("full.pcap", Full(true)).unwrap();
    for i in 0..10 {
        a.send(&frame(B, A, i));
    }
    // The frames still flow; the capture's error comes with its stop.
    assert_eq!(port_of(&lan, 2).out_frames, 10);
    let e = lan.stop_capture().unwrap_err();
    assert_eq!(e.code, ErrorCode::Io);
    assert!(
        e.message
            .starts_with("full.pcap: disk full; the capture stopped there"),
        "{}",
        e.message
    );
    // And it is stopped: another can start.
    assert_eq!(lan.info().capture, None);
    lan.capture_into("again.pcap", io::sink()).unwrap();
    lan.stop_capture().unwrap();
}

#[test]
fn an_ended_card_is_unplugged_for_good() {
    let lan = Switch::new("lan");
    let a = TestCard::plugged(1, A, &lan);
    a.card.end();
    assert!(lan.info().ports.is_empty());
    assert!(a.card.switch().is_none());
    assert_eq!(
        a.card.plug(owner(1), &lan).unwrap_err().code,
        ErrorCode::InvalidState
    );
    // Its machine letting go of the backend later changes nothing.
    drop(a);
    assert!(lan.info().ports.is_empty());
}

// ---- Addresses ----

#[test]
fn generated_addresses_are_stable_cards_and_distinct() {
    assert_eq!(generated_mac("web", 0), generated_mac("web", 0));
    let mut seen = std::collections::BTreeSet::new();
    for vm in ["web", "db", "web-1", "web-2", "a", "b"] {
        for card in 0..4 {
            let mac = generated_mac(vm, card);
            assert_eq!(mac.check_card(), Ok(()), "{mac}");
            assert_eq!(mac.0[0], 0x02);
            assert!(seen.insert(mac), "{vm} {card}: {mac}");
        }
    }
    // Pinned, so a change of the hash (which would change every VM's address) shows.
    assert_eq!(generated_mac("web", 0).to_string(), GENERATED_WEB_0);
}

/// `generated_mac("web", 0)`: FNV-1a 64 of "web", 0, 0.
const GENERATED_WEB_0: &str = "02:66:b7:0f:2e:09";

// ---- The NAT gateway ----

use gateway::{dhcp, wire};

const GW: MacAddr = MacAddr(GATEWAY_MAC);

/// A DHCP DISCOVER for `chaddr`, broadcast from `src`.
fn discover(src: MacAddr, chaddr: MacAddr) -> Vec<u8> {
    let m = dhcp::client_message(dhcp::DISCOVER, 7, chaddr.0, None);
    let udp = wire::udp_datagram(Ipv4Addr::UNSPECIFIED, Ipv4Addr::BROADCAST, 68, 67, &m);
    let ip = wire::ipv4_packet(
        Ipv4Addr::UNSPECIFIED,
        Ipv4Addr::BROADCAST,
        wire::PROTO_UDP,
        0,
        &udp,
    );
    wire::eth_frame(wire::BROADCAST_MAC, src.0, wire::ETHERTYPE_IPV4, &ip)
}

/// An ARP request from `src` (at 10.0.2.`last`) for the gateway's address.
fn who_has_the_gateway(src: MacAddr, last: u8) -> Vec<u8> {
    let arp = wire::arp_packet(&wire::Arp {
        op: wire::ARP_REQUEST,
        sender_mac: src.0,
        sender_ip: Ipv4Addr::new(10, 0, 2, last),
        target_mac: [0; 6],
        target_ip: gateway::GATEWAY_ADDR,
    });
    wire::eth_frame(wire::BROADCAST_MAC, src.0, wire::ETHERTYPE_ARP, &arp)
}

/// Waits for the gateway's thread to send `card` a frame, and returns it (frames
/// from other cards are passed over).
fn from_gateway(card: &mut TestCard) -> Vec<u8> {
    let start = Instant::now();
    loop {
        if let Some(f) = card.recv_all().into_iter().find(|f| f[6..12] == GW.0) {
            return f;
        }
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "the gateway never answered"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn gateway_port(switch: &Switch) -> SwitchPort {
    *switch
        .info()
        .ports
        .iter()
        .find(|p| p.owner == PortOwner::Gateway)
        .expect("a gateway")
}

fn leases(switch: &Switch) -> Vec<(MacAddr, Ipv4Addr)> {
    switch
        .info()
        .nat
        .expect("a gateway")
        .leases
        .iter()
        .map(|l| (MacAddr(l.mac), l.addr))
        .collect()
}

#[test]
fn one_gateway_gives_each_card_its_own_lease() {
    let lan = Switch::new("lan");
    lan.set_nat(Some(&NatConfig::default())).unwrap();
    let mut a = TestCard::plugged(1, A, &lan);
    let mut b = TestCard::plugged(2, B, &lan);
    assert_eq!(lan.info().nat, Some(NatInfo::default()));
    let port = gateway_port(&lan);
    assert_eq!((port.mac, port.in_frames, port.out_frames), (GATEWAY_MAC, 0, 0));

    a.send(&discover(A, A));
    let offer = from_gateway(&mut a);
    assert_eq!(offer[..6], A.0, "unicast to the card that asked");
    assert!(a.rings() >= 1, "the gateway's frame rang the card's bell");
    // B heard A's broadcast, but not the gateway's answer to A.
    assert_eq!(b.recv_all(), [discover(A, A)]);
    b.send(&discover(B, B));
    let offer_b = from_gateway(&mut b);
    assert_eq!(offer_b[..6], B.0);
    assert_eq!(
        wire::ipv4_at(&offer, 14 + 20 + 8 + 16),
        Ipv4Addr::new(10, 0, 2, 15)
    );
    assert_eq!(
        wire::ipv4_at(&offer_b, 14 + 20 + 8 + 16),
        Ipv4Addr::new(10, 0, 2, 16)
    );
    assert_eq!(
        leases(&lan),
        [(A, Ipv4Addr::new(10, 0, 2, 15)), (B, Ipv4Addr::new(10, 0, 2, 16))]
    );
    // The gateway's port counts like a card's: the two DISCOVERs in, the two OFFERs
    // out (A's DISCOVER also went to B's port).
    let port = gateway_port(&lan);
    assert_eq!((port.out_frames, port.in_frames, port.dropped), (2, 2, 0));
    assert_eq!(port_of(&lan, 2).out_frames, 2);
}

#[test]
fn a_card_cannot_pose_as_another_machine_to_the_gateway() {
    let lan = Switch::new("lan");
    lan.set_nat(Some(&NatConfig::default())).unwrap();
    let mut a = TestCard::plugged(1, A, &lan);
    let mut b = TestCard::plugged(2, B, &lan);
    // A asks for a lease as C: the switch passes it to B (a card), not the gateway.
    a.send(&discover(C, C));
    a.send(&who_has_the_gateway(A, 15));
    let reply = from_gateway(&mut a);
    assert_eq!(wire::be16(&reply, 12), wire::ETHERTYPE_ARP);
    assert_eq!(gateway_port(&lan).out_frames, 1, "the ARP request only");
    assert_eq!(leases(&lan), []);
    assert_eq!(b.recv_all(), [discover(C, C), who_has_the_gateway(A, 15)]);
}

#[test]
fn a_gateway_taken_away_or_deleted_frees_its_host_ports() {
    let lan = Switch::new("lan");
    let fwd = |port| NatConfig {
        hostfwd: vec![PortForward {
            proto: FwdProto::Tcp,
            host: std::net::SocketAddrV4::new(Ipv4Addr::LOCALHOST, port),
            guest: std::net::SocketAddrV4::new(Ipv4Addr::new(10, 0, 2, 15), 80),
        }],
    };
    lan.set_nat(Some(&fwd(0))).unwrap();
    let bound = lan.info().nat.unwrap().hostfwd[0].host;
    assert_ne!(bound.port(), 0);
    assert!(std::net::TcpListener::bind(bound).is_err(), "the gateway has it");
    let mut a = TestCard::plugged(1, A, &lan);
    assert_eq!(lan.info().ports.len(), 2);

    // Taken away: its port goes, the port is free at once, and nothing reaches it.
    lan.set_nat(None).unwrap();
    assert_eq!(lan.info().nat, None);
    assert_eq!(lan.info().ports.len(), 1);
    drop(std::net::TcpListener::bind(bound).unwrap());
    a.send(&who_has_the_gateway(A, 15));
    assert!(a.recv_all().is_empty());
    assert_eq!(lan.set_nat(None).unwrap_err().code, ErrorCode::InvalidState);

    // Back with the same port, and then the switch deleted.
    lan.set_nat(Some(&fwd(bound.port()))).unwrap();
    assert_eq!(lan.info().nat.unwrap().hostfwd[0].host, bound);
    assert_eq!(
        lan.set_nat(Some(&fwd(0))).unwrap_err().code,
        ErrorCode::AlreadyExists
    );
    lan.close();
    drop(std::net::TcpListener::bind(bound).unwrap());
    assert_eq!(
        lan.set_nat(Some(&fwd(0))).unwrap_err().code,
        ErrorCode::NoSuchSwitch
    );
}

#[test]
fn a_port_that_cannot_be_bound_fails_and_changes_nothing() {
    let lan = Switch::new("lan");
    let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let host = match taken.local_addr().unwrap() {
        std::net::SocketAddr::V4(v4) => v4,
        other => panic!("{other}"),
    };
    let rule = PortForward {
        proto: FwdProto::Tcp,
        host,
        guest: std::net::SocketAddrV4::new(Ipv4Addr::new(10, 0, 2, 16), 22),
    };
    let fail = lan.set_nat(Some(&NatConfig { hostfwd: vec![rule] })).unwrap_err();
    assert_eq!(fail.code, ErrorCode::Io);
    assert!(
        fail.message.contains(&format!("hostfwd {rule}")),
        "{}",
        fail.message
    );
    assert_eq!(lan.info().nat, None);
    assert!(lan.info().ports.is_empty());
}
