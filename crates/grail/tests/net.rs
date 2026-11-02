//! Xcal network card end to end (spec section 9.8): `examples/net` (ping and pong) on
//! two machines connected by a hub, in one thread and on two; a pcap capture checked
//! byte for byte against a committed golden file; cards in the sysinfo device table;
//! frames dropped while no buffer is free; and the `--net` / `--pcap` flags.

use grail::net::{self, Hub, MacAddr, NetBackend, Pcap};
use grail::{Machine, MachineConfig, NetConfig, RunResult};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const PING_OUT: &str = "net 0x10009000, irq 12, mac 02:ca:00:00:00:01, link up
sent 60 bytes to ff:ff:ff:ff:ff:ff
received 60 bytes from 02:ca:00:00:00:02, type 0x88b5: Hello back from pong!
";

const PONG_OUT: &str = "net 0x10009000, irq 12, mac 02:ca:00:00:00:02, link up
received 60 bytes from 02:ca:00:00:00:01, type 0x88b5: Hello from ping!
sent 60 bytes to 02:ca:00:00:00:01
";

/// The capture of ping's card in a ping/pong exchange, its records time-stamped by
/// [`fixed_clock`]. `GRAIL_WRITE_PCAP=1` rewrites it from the test.
const GOLDEN: &str = "tests/golden/net-ping.pcap";

fn examples() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples")
}

fn elf(name: &str) -> Vec<u8> {
    std::fs::read(examples().join(format!("net/{name}.elf"))).unwrap()
}

/// A machine with these cards running `examples/net/<name>.elf`, its UART captured.
fn machine(name: &str, config: MachineConfig, cards: Vec<(MacAddr, Box<dyn NetBackend>)>) -> Machine {
    let mut m = Machine::with_devices(config, Vec::new(), cards).unwrap();
    m.cpu.bus.uart.capture = true;
    m.load_elf(&elf(name)).unwrap();
    m
}

fn on_hub(hub: &Hub) -> MachineConfig {
    MachineConfig {
        nets: vec![NetConfig::hub(hub)],
        ..MachineConfig::default()
    }
}

fn output(m: &mut Machine) -> String {
    String::from_utf8(std::mem::take(&mut m.cpu.bus.uart.captured)).unwrap()
}

/// Runs pong until it listens, then both in turns of `slice` steps until both exit.
fn exchange(ping: &mut Machine, pong: &mut Machine, slice: u64) {
    pong.run_for(20_000, &mut |_| {});
    assert_eq!(pong.exit_code(), None, "pong waits for a frame");
    for _ in 0..10_000 {
        if ping.exit_code().is_some() && pong.exit_code().is_some() {
            return;
        }
        ping.run_for(slice, &mut |_| {});
        pong.run_for(slice, &mut |_| {});
    }
    panic!(
        "no exit: ping {:?}, pong {:?}",
        ping.exit_code(),
        pong.exit_code()
    );
}

#[test]
fn ping_and_pong_on_a_hub() {
    let hub = Hub::new();
    // Ping attaches first, so it gets the hub's port 1 and its MAC.
    let mut ping = machine("net-ping", on_hub(&hub), Vec::new());
    let mut pong = machine("net-pong", on_hub(&hub), Vec::new());
    exchange(&mut ping, &mut pong, 1_000);
    assert_eq!(
        (ping.exit_code(), output(&mut ping).as_str()),
        (Some(0), PING_OUT)
    );
    assert_eq!(
        (pong.exit_code(), output(&mut pong).as_str()),
        (Some(0), PONG_OUT)
    );
    for m in [&ping, &pong] {
        let card = &m.cpu.bus.nets[0];
        assert_eq!((card.rx_dropped(), card.error()), (0, net::error::NONE));
    }
    // The slice size changes nothing.
    let hub = Hub::new();
    let mut ping = machine("net-ping", on_hub(&hub), Vec::new());
    let mut pong = machine("net-pong", on_hub(&hub), Vec::new());
    exchange(&mut ping, &mut pong, 37);
    assert_eq!(output(&mut ping), PING_OUT);
    assert_eq!(output(&mut pong), PONG_OUT);
}

#[test]
fn ping_and_pong_on_two_threads() {
    let hub = Hub::new();
    let mut ping = machine("net-ping", on_hub(&hub), Vec::new());
    let mut pong = machine("net-pong", on_hub(&hub), Vec::new());
    let (listening, ready) = std::sync::mpsc::channel();
    let pong = std::thread::spawn(move || {
        pong.run_for(20_000, &mut |_| {});
        listening.send(()).unwrap();
        let result = pong.run(Some(100_000_000), |_| {});
        (result, output(&mut pong))
    });
    ready.recv().unwrap();
    assert_eq!(ping.run(Some(100_000_000), |_| {}), RunResult::Exited(0));
    assert_eq!(output(&mut ping), PING_OUT);
    let (result, out) = pong.join().unwrap();
    assert_eq!((result, out.as_str()), (RunResult::Exited(0), PONG_OUT));
}

/// With a real-time clock, an idle machine skips its `wfi` ticks in bulk; its card still
/// polls on time, so pong (sleeping in `wfi`) answers a frame from the host promptly.
#[test]
fn paced_pong_answers_a_host_frame() {
    let hub = Hub::new();
    let mut host = hub.attach();
    let config = MachineConfig {
        clock_hz: Some(2_000_000),
        ..on_hub(&hub)
    };
    let mut pong = machine("net-pong", config, Vec::new());
    let pong = std::thread::spawn(move || {
        let result = pong.run(Some(20_000_000), |_| {});
        (result, output(&mut pong))
    });
    // 100 ms of guest time is 200 000 instructions: pong has long been listening.
    std::thread::sleep(Duration::from_millis(100));
    let hello = frame([0xff; 6], [2, 0, 0, 0, 0, 1], "Hello from the host");
    let start = std::time::Instant::now();
    host.send(&hello).unwrap();
    let answer = loop {
        if let Some(f) = host.recv() {
            break f;
        }
        assert!(start.elapsed() < Duration::from_secs(5), "no answer");
        std::thread::sleep(Duration::from_millis(1));
    };
    assert_eq!(
        answer,
        frame([2, 0, 0, 0, 0, 1], MacAddr::local(2).0, "Hello back from pong!")
    );
    let (result, out) = pong.join().unwrap();
    assert_eq!(result, RunResult::Exited(0));
    assert!(
        out.contains("from 02:00:00:00:00:01, type 0x88b5: Hello from the host\n"),
        "{out}"
    );
}

/// A writer the test can still read after handing it over.
#[derive(Clone, Default)]
struct Shared(Arc<Mutex<Vec<u8>>>);

impl Write for Shared {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Time stamps 2024-01-01 00:00:00.250 UTC, then 250 ms later each time.
fn fixed_clock() -> net::Clock {
    let mut t = Duration::from_secs(1_704_067_200);
    Box::new(move || {
        t += Duration::from_millis(250);
        t
    })
}

/// A 60-byte frame with EtherType 0x88b5 and `text` (and a NUL) as payload.
fn frame(dst: [u8; 6], src: [u8; 6], text: &str) -> Vec<u8> {
    let mut f = [&dst[..], &src, &[0x88, 0xb5], text.as_bytes()].concat();
    f.resize(60, 0);
    f
}

#[test]
fn capture_matches_the_golden_file() {
    let hub = Hub::new();
    let capture = Shared::default();
    let ping_mac = MacAddr::local(1);
    let pcap = Pcap::new(Box::new(hub.attach()), capture.clone(), fixed_clock()).unwrap();
    let mut ping = machine(
        "net-ping",
        MachineConfig::default(),
        vec![(ping_mac, Box::new(pcap))],
    );
    let mut pong = machine("net-pong", on_hub(&hub), Vec::new());
    exchange(&mut ping, &mut pong, 1_000);
    assert_eq!(output(&mut ping), PING_OUT);
    let got = capture.0.lock().unwrap().clone();

    // What the file must hold: the header, the frame ping sent, pong's answer.
    let ping_frame = frame([0xff; 6], ping_mac.0, "Hello from ping!");
    let pong_frame = frame(ping_mac.0, MacAddr::local(2).0, "Hello back from pong!");
    let mut want = net::pcap_header().to_vec();
    want.extend(net::pcap_record(
        Duration::new(1_704_067_200, 250_000_000),
        &ping_frame,
    ));
    want.extend(net::pcap_record(
        Duration::new(1_704_067_200, 500_000_000),
        &pong_frame,
    ));
    assert_eq!(got, want);

    let golden = Path::new(env!("CARGO_MANIFEST_DIR")).join(GOLDEN);
    if std::env::var_os("GRAIL_WRITE_PCAP").is_some() {
        std::fs::write(&golden, &got).unwrap();
    }
    assert!(
        std::fs::read(&golden).unwrap() == got,
        "{GOLDEN} is out of date: rerun this test with GRAIL_WRITE_PCAP=1"
    );
}

/// A file in the temp directory, removed when dropped.
struct TempFile(PathBuf);

impl TempFile {
    fn new(name: &str) -> Self {
        TempFile(std::env::temp_dir().join(format!("grail-net-{}-{name}", std::process::id())))
    }

    fn path(&self) -> &str {
        self.0.to_str().unwrap()
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// The frames of a pcap file with the header checked: (seconds, microseconds, frame).
fn pcap_frames(file: &[u8]) -> Vec<(u32, u32, Vec<u8>)> {
    assert_eq!(&file[..24], &net::pcap_header());
    let word = |b: &[u8]| u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
    let mut rest = &file[24..];
    let mut frames = Vec::new();
    while !rest.is_empty() {
        let len = word(&rest[8..]) as usize;
        assert_eq!(word(&rest[12..]) as usize, len, "nothing cut");
        frames.push((word(rest), word(&rest[4..]), rest[16..16 + len].to_vec()));
        rest = &rest[16 + len..];
    }
    frames
}

#[test]
fn configured_capture_files() {
    let hub = Hub::new();
    let file = TempFile::new("pong.pcap");
    let pong_config = MachineConfig {
        nets: vec![NetConfig::hub(&hub)
            .with_mac(MacAddr([0x02, 0, 0, 0, 0, 0x99]))
            .with_pcap(&file.0)],
        ..MachineConfig::default()
    };
    // Pong attaches first here, but its MAC is the configured one; ping is port 2.
    let mut pong = machine("net-pong", pong_config, Vec::new());
    let mut ping = machine("net-ping", on_hub(&hub), Vec::new());
    exchange(&mut ping, &mut pong, 1_000);
    assert_eq!(
        output(&mut ping),
        "net 0x10009000, irq 12, mac 02:ca:00:00:00:02, link up
sent 60 bytes to ff:ff:ff:ff:ff:ff
received 60 bytes from 02:00:00:00:00:99, type 0x88b5: Hello back from pong!
"
    );
    drop(pong); // closes the capture
    let frames = pcap_frames(&std::fs::read(&file.0).unwrap());
    let ping_mac = MacAddr::local(2).0;
    assert_eq!(frames.len(), 2);
    assert_eq!(frames[0].2, frame([0xff; 6], ping_mac, "Hello from ping!"));
    assert_eq!(
        frames[1].2,
        frame(ping_mac, [2, 0, 0, 0, 0, 0x99], "Hello back from pong!")
    );
    // Wall-clock time stamps, in order.
    assert!(frames[0].0 > 1_700_000_000 && frames[0].1 < 1_000_000);
    assert!((frames[0].0, frames[0].1) <= (frames[1].0, frames[1].1));
}

#[test]
fn frames_are_dropped_while_no_buffer_is_free() {
    // Ping speaks before pong listens: its frame reaches pong's card before pong's
    // driver has given it a buffer, so the card drops it and counts it.
    let hub = Hub::new();
    let mut ping = machine("net-ping", on_hub(&hub), Vec::new());
    let mut pong = machine("net-pong", on_hub(&hub), Vec::new());
    assert_eq!(ping.run(Some(50_000_000), |_| {}), RunResult::Exited(3));
    assert_eq!(
        output(&mut ping),
        "net 0x10009000, irq 12, mac 02:ca:00:00:00:01, link up
sent 60 bytes to ff:ff:ff:ff:ff:ff
no answer
"
    );
    assert_eq!(pong.run(Some(1_000_000), |_| {}), RunResult::StepLimit);
    assert_eq!(pong.cpu.bus.nets[0].rx_dropped(), 1);
    assert_eq!(
        output(&mut pong),
        "net 0x10009000, irq 12, mac 02:ca:00:00:00:02, link up\n",
        "still waiting"
    );
}

#[test]
fn examples_without_a_card() {
    for name in ["net-ping", "net-pong"] {
        let mut m = machine(name, MachineConfig::default(), Vec::new());
        assert_eq!(m.run(Some(1_000_000), |_| {}), RunResult::Exited(1), "{name}");
        assert_eq!(output(&mut m), "no network card (try grail --net hub)\n");
    }
}

#[test]
fn sysinfo_lists_the_cards() {
    let hub = Hub::new();
    let config = MachineConfig {
        nets: vec![NetConfig::hub(&hub), NetConfig::hub(&hub)],
        disks: vec![grail::DiskConfig::memory(vec![0; 512])],
        ..MachineConfig::default()
    };
    // A third card from the host: it comes after the configured ones.
    let mut m = Machine::with_devices(
        config,
        Vec::new(),
        vec![(MacAddr::local(7), Box::new(hub.attach()))],
    )
    .unwrap();
    m.cpu.bus.uart.capture = true;
    m.load_elf(&std::fs::read(examples().join("sysinfo/sysinfo.elf")).unwrap())
        .unwrap();
    assert_eq!(m.run(Some(1_000_000), |_| {}), RunResult::Exited(0));
    let out = output(&mut m);
    let devices = out.split_once("13 devices:\n").expect(&out).1;
    assert_eq!(
        devices,
        "  test      0x00100000 + 0x00001000
  CLINT     0x02000000 + 0x00010000
  PLIC      0x0c000000 + 0x04000000
  UART      0x10000000 + 0x00000100  irq 1
  block     0x10001000 + 0x00001000  irq 3
  net       0x10009000 + 0x00001000  irq 12
  net       0x1000a000 + 0x00001000  irq 13
  net       0x1000b000 + 0x00001000  irq 14
  fb-ctrl   0x30000000 + 0x00001000
  keyboard  0x30001000 + 0x00001000  irq 2
  sysinfo   0x30002000 + 0x00001000
  pointer   0x30003000 + 0x00001000  irq 11
  fb-vram   0x31000000 + 0x0012c000
"
    );
    // The host's port attached first (port 1), so the configured cards got ports 2 and 3.
    let macs: Vec<MacAddr> = m.cpu.bus.nets.iter().map(|n| n.mac()).collect();
    assert_eq!(macs, [MacAddr::local(2), MacAddr::local(3), MacAddr::local(7)]);
}

#[test]
fn bad_cards_are_machine_errors() {
    let hub = Hub::new();
    let card = || (MacAddr::local(9), Box::new(hub.attach()) as Box<dyn NetBackend>);
    let e = Machine::with_devices(on_hub(&hub), Vec::new(), (0..4).map(|_| card()).collect())
        .err()
        .unwrap();
    assert_eq!(e, "5 network cards, at most 4");
    let e = Machine::with_devices(
        MachineConfig::default(),
        Vec::new(),
        vec![(MacAddr::BROADCAST, card().1)],
    )
    .err()
    .unwrap();
    assert_eq!(e, "ff:ff:ff:ff:ff:ff is a group address, not a card's");
    let config = MachineConfig {
        nets: vec![
            NetConfig::hub(&hub),
            NetConfig::hub(&hub).with_pcap("no/such/dir/x.pcap"),
        ],
        ..MachineConfig::default()
    };
    let e = Machine::new(config).err().unwrap();
    assert!(e.starts_with("network card 1: no/such/dir/x.pcap: "), "{e}");
}

fn grail(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_grail"))
        .args(args)
        .output()
        .expect("cannot run grail")
}

fn elf_path(name: &str) -> String {
    examples()
        .join(format!("net/{name}.elf"))
        .to_str()
        .unwrap()
        .to_string()
}

#[test]
fn cli_net_and_pcap_flags() {
    let file = TempFile::new("cli.pcap");
    let out = grail(&["--net", "hub", "--pcap", file.path(), &elf_path("net-ping")]);
    assert_eq!(out.status.code(), Some(3), "nobody answers");
    assert_eq!(
        String::from_utf8(out.stdout).unwrap(),
        "net 0x10009000, irq 12, mac 02:ca:00:00:00:01, link up
sent 60 bytes to ff:ff:ff:ff:ff:ff
no answer
"
    );
    let frames = pcap_frames(&std::fs::read(&file.0).unwrap());
    assert_eq!(frames.len(), 1);
    assert_eq!(
        frames[0].2,
        frame([0xff; 6], MacAddr::local(1).0, "Hello from ping!")
    );

    // Two cards on the process's hub: ping uses the first, the second hears it (and,
    // having no buffers, drops it). A MAC address from the command line.
    let out = grail(&[
        "--net",
        "hub,mac=02:00:00:00:00:42",
        "--net",
        "hub",
        "--pcap",
        file.path(),
        &elf_path("net-ping"),
    ]);
    assert_eq!(out.status.code(), Some(3));
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(
        stdout.starts_with("net 0x10009000, irq 12, mac 02:00:00:00:00:42, link up\n"),
        "{stdout}"
    );
    let frames = pcap_frames(&std::fs::read(&file.0).unwrap());
    assert_eq!(frames.len(), 1, "the second card received the frame");
    assert_eq!(frames[0].2[6..12], [2, 0, 0, 0, 0, 0x42]);
}

#[test]
fn cli_net_errors() {
    let elf = elf_path("net-ping");
    let fails = |args: &[&str], want: &str| {
        let out = grail(args);
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        let stderr = String::from_utf8(out.stderr).unwrap();
        assert!(stderr.contains(want), "{args:?}: {stderr}");
    };
    fails(&["--net", "tap", &elf], "grail: --net: 'tap' is not a network");
    fails(
        &["--net", "hub,mac=ff:ff:ff:ff:ff:ff", &elf],
        "is a group address",
    );
    fails(&["--net", "hub,mac=nope", &elf], "is not a MAC address");
    fails(&[&elf, "--net"], "--net needs a value");
    fails(
        &["--pcap", "x.pcap", "--net", "hub", &elf],
        "--pcap needs a --net before it",
    );
    fails(&["--net", "hub", &elf, "--pcap"], "--pcap needs a file");
    let file = TempFile::new("twice.pcap");
    fails(
        &["--net", "hub", "--pcap", file.path(), "--pcap", file.path(), &elf],
        "that card is already captured",
    );
    let five: Vec<&str> = (0..5)
        .flat_map(|_| ["--net", "hub"])
        .chain([elf.as_str()])
        .collect();
    fails(&five, "grail: 5 network cards, at most 4");
    fails(
        &["--net", "hub", "--pcap", "no/such/dir/x.pcap", &elf],
        "grail: network card 0: no/such/dir/x.pcap: ",
    );

    // No card at all: the example says so.
    let out = grail(&[&elf]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        String::from_utf8(out.stdout).unwrap(),
        "no network card (try grail --net hub)\n"
    );
}
