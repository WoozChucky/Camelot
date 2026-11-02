//! The Camelot kernel's network stack (`kernel/net/`, docs/camelot-net.md) on Grail's
//! network cards, driven through the shell's `ifconfig` and `ping` built-ins:
//!
//! - with user networking (`--net user`) it gets 10.0.2.15 from the gateway by DHCP and
//!   pings 10.0.2.2, itself and 127.0.0.1; its UDP echo service answers a host program
//!   through a forwarded port;
//! - two kernels on a hub, with static addresses (one from the kernel's configuration,
//!   one from `ifconfig` after DHCP found no server), ping each other;
//! - on a hub with this test as the other host, crafted frames (bad checksums,
//!   truncated headers, wrong lengths, fragments, unknown protocols) are dropped and
//!   counted, never a panic, and the stack still answers afterwards; ARP gives up on an
//!   address nobody has.
//!
//! The machines run in this thread, in turns of a few thousand steps, so what they do is
//! deterministic: time is Grail's steps, the forwarded port's gateway included. Only
//! that test goes through real host sockets, and its waits for them are bounded.

use grail::net::{Hub, HubPort, MacAddr, NetBackend, POLL_STEPS};
use grail::{Machine, MachineConfig, NetConfig, RunResult};
use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// 16 MiB boots faster than the default 128 (the kernel puts every page on its free
/// list) and is plenty.
const RAM: usize = 16 << 20;
/// Steps a machine runs per turn.
const SLICE: u64 = 2_000;
/// More than any wait here needs: a boot is about 10 million steps with 16 MiB, a
/// command a few million at most.
const MAX_STEPS: u64 = 400_000_000;

const SHELL_BANNER: &str = "Type help for the list of commands.\n";
const PROMPT: &str = "$ ";

fn kernel_elf() -> Vec<u8> {
    std::fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../kernel/kernel.elf")).unwrap()
}

/// Addresses as the kernel's variables hold them: in host order.
fn ip(a: [u8; 4]) -> u32 {
    u32::from_be_bytes(a)
}

/// The kernel's static network configuration (kernel/net/net.c), to patch into a
/// machine's RAM before it boots: no DHCP, this address and mask, no gateway.
fn static_config(addr: [u8; 4], mask: [u8; 4]) -> Vec<(&'static str, u32)> {
    vec![
        ("net_dhcp", 0),
        ("net_static_addr", ip(addr)),
        ("net_static_mask", ip(mask)),
        ("net_static_gateway", 0),
    ]
}

/// A Camelot machine: the kernel with its UART captured and fed through `input`.
struct Vm {
    m: Machine,
    input: Sender<u8>,
}

impl Vm {
    /// The kernel on a machine with `config`'s cards and `cards`, with the kernel
    /// variables in `patches` set.
    fn new(nets: Vec<NetConfig>, cards: Vec<(MacAddr, Box<dyn NetBackend>)>, patches: &[(&str, u32)]) -> Vm {
        let config = MachineConfig {
            ram_size: RAM,
            nets,
            ..MachineConfig::default()
        };
        let mut m = Machine::with_devices(config, Vec::new(), cards).unwrap();
        m.cpu.bus.uart.capture = true;
        let file = kernel_elf();
        m.load_elf(&file).unwrap();
        let elf = grail::elf::parse(&file).unwrap();
        for (name, value) in patches {
            let addr = elf.symbol(name).unwrap_or_else(|| panic!("no symbol {name}"));
            m.cpu.bus.load(addr, &value.to_le_bytes()).unwrap();
        }
        let (input, rx) = channel();
        m.set_uart_input(rx);
        Vm { m, input }
    }

    fn out(&self) -> String {
        String::from_utf8_lossy(&self.m.cpu.bus.uart.captured).into_owned()
    }

    /// Prompts printed so far: one when the shell starts, one after each command.
    fn prompts(&self) -> usize {
        self.out().matches(&format!("\n{PROMPT}")).count()
    }

    fn type_line(&self, line: &str) {
        for b in line.bytes().chain(std::iter::once(b'\n')) {
            self.input.send(b).unwrap();
        }
    }
}

/// Runs the machines in turns, letting `host` (if any) see what they send between
/// turns, until `done` holds. Panics if a machine stops or the steps run out.
fn run_until(vms: &mut [&mut Vm], mut host: Option<&mut Host>, done: impl Fn(&[&mut Vm]) -> bool) {
    let mut steps = 0;
    while !done(vms) {
        for vm in vms.iter_mut() {
            vm.m.run_for(SLICE, &mut |_| {});
            assert_eq!(vm.m.exit_code(), None, "a machine stopped:\n{}", vm.out());
        }
        if let Some(h) = host.as_deref_mut() {
            h.service();
        }
        steps += SLICE;
        assert!(steps < MAX_STEPS, "no end in sight:\n{}", vms[0].out());
    }
}

/// Boots the machines until each shows its first prompt.
fn boot(vms: &mut [&mut Vm], host: Option<&mut Host>) {
    run_until(vms, host, |vms| vms.iter().all(|vm| vm.prompts() >= 1));
}

/// Types `line` into machine `i`'s shell and runs until its next prompt. Returns what
/// the command printed (after the echoed line, up to the prompt).
fn command(vms: &mut [&mut Vm], i: usize, host: Option<&mut Host>, line: &str) -> String {
    let (before, prompts) = (vms[i].out().len(), vms[i].prompts());
    vms[i].type_line(line);
    run_until(vms, host, |vms| vms[i].prompts() > prompts);
    let out = vms[i].out();
    let text = &out[before..];
    let text = text
        .strip_prefix(&format!("{line}\n"))
        .unwrap_or_else(|| panic!("{line:?} not echoed:\n{out}"));
    text.strip_suffix(PROMPT).unwrap().to_string()
}

/// Leaves the shell; the machine must stop with status 0.
fn exit(vm: &mut Vm) {
    vm.type_line("exit");
    assert_eq!(
        vm.m.run(Some(MAX_STEPS), |_| {}),
        RunResult::Exited(0),
        "{}",
        vm.out()
    );
}

/// `time=1.234 ms` (which depends on how the machines take turns) as `time=T ms`.
fn mask_times(s: &str) -> String {
    s.lines()
        .map(|l| match (l.find("time="), l.find(" ms")) {
            (Some(a), Some(b)) if a < b => format!("{}time=T{}\n", &l[..a], &l[b..]),
            _ => format!("{l}\n"),
        })
        .collect()
}

/// What `ping ADDR N` prints when every reply comes.
fn ping_ok(addr: &str, n: u32) -> String {
    let mut s = format!("PING {addr}: 56 data bytes\n");
    for seq in 1..=n {
        s += &format!("64 bytes from {addr}: seq={seq} ttl=64 time=T ms\n");
    }
    s + &format!("{n} sent, {n} received\n")
}

/// The counters `ifconfig` prints ("name N" after the colon of its counter lines).
fn counters(ifconfig: &str) -> HashMap<String, u32> {
    let mut map = HashMap::new();
    for line in ifconfig.lines() {
        let Some((head, list)) = line.split_once(": ") else {
            continue;
        };
        if !(head.ends_with("rx dropped") || head.ends_with("other")) {
            continue;
        }
        for item in list.split(", ") {
            let (name, n) = item.rsplit_once(' ').unwrap();
            map.insert(name.to_string(), n.parse().unwrap());
        }
    }
    map
}

/// The frame and byte counts of `ifconfig`'s `rx ... ; tx ...` line.
fn traffic(ifconfig: &str) -> (u32, u32) {
    let line = ifconfig
        .lines()
        .find(|l| l.starts_with("  rx ") && l.contains("; tx "))
        .unwrap();
    let words: Vec<&str> = line.split_whitespace().collect();
    (words[1].parse().unwrap(), words[6].parse().unwrap())
}

// ---- crafting frames ----

const IP_TYPE: u16 = 0x0800;
const ARP_TYPE: u16 = 0x0806;

/// The Internet checksum of `data` (RFC 1071), starting from `sum`.
fn checksum(data: &[u8], mut sum: u32) -> u16 {
    for c in data.chunks(2) {
        sum += (u32::from(c[0]) << 8) | u32::from(*c.get(1).unwrap_or(&0));
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    !(sum as u16)
}

fn ether(dst: [u8; 6], src: [u8; 6], ty: u16, payload: &[u8]) -> Vec<u8> {
    let mut f = Vec::new();
    f.extend_from_slice(&dst);
    f.extend_from_slice(&src);
    f.extend_from_slice(&ty.to_be_bytes());
    f.extend_from_slice(payload);
    f
}

/// An IPv4 header without options and `payload`, with a right checksum.
fn ipv4(src: [u8; 4], dst: [u8; 4], proto: u8, payload: &[u8]) -> Vec<u8> {
    let mut p = vec![0x45, 0];
    p.extend_from_slice(&(20 + payload.len() as u16).to_be_bytes());
    p.extend_from_slice(&[0, 1, 0x40, 0, 64, proto, 0, 0]);
    p.extend_from_slice(&src);
    p.extend_from_slice(&dst);
    let sum = checksum(&p, 0);
    p[10..12].copy_from_slice(&sum.to_be_bytes());
    p.extend_from_slice(payload);
    p
}

/// Recomputes the header checksum of the IPv4 packet `p` after a change to its header.
fn fix_ip_checksum(p: &mut [u8]) {
    let hlen = usize::from(p[0] & 0xf) * 4;
    p[10..12].fill(0);
    let sum = checksum(&p[..hlen], 0);
    p[10..12].copy_from_slice(&sum.to_be_bytes());
}

fn icmp_echo(ty: u8, id: u16, seq: u16, data: &[u8]) -> Vec<u8> {
    let mut m = vec![ty, 0, 0, 0];
    m.extend_from_slice(&id.to_be_bytes());
    m.extend_from_slice(&seq.to_be_bytes());
    m.extend_from_slice(data);
    let sum = checksum(&m, 0);
    m[2..4].copy_from_slice(&sum.to_be_bytes());
    m
}

/// The sum of UDP's pseudo-header, for [`checksum`].
fn pseudo(src: [u8; 4], dst: [u8; 4], len: usize) -> u32 {
    let w = |a: [u8; 4]| {
        u32::from(u16::from_be_bytes([a[0], a[1]])) + u32::from(u16::from_be_bytes([a[2], a[3]]))
    };
    w(src) + w(dst) + 17 + len as u32
}

fn udp(src: [u8; 4], dst: [u8; 4], sport: u16, dport: u16, data: &[u8]) -> Vec<u8> {
    let mut u = Vec::new();
    u.extend_from_slice(&sport.to_be_bytes());
    u.extend_from_slice(&dport.to_be_bytes());
    u.extend_from_slice(&(8 + data.len() as u16).to_be_bytes());
    u.extend_from_slice(&[0, 0]);
    u.extend_from_slice(data);
    let sum = checksum(&u, pseudo(src, dst, u.len()));
    u[6..8].copy_from_slice(&sum.to_be_bytes());
    u
}

fn arp(op: u16, sha: [u8; 6], spa: [u8; 4], tha: [u8; 6], tpa: [u8; 4]) -> Vec<u8> {
    let mut a = vec![0, 1, 8, 0, 6, 4];
    a.extend_from_slice(&op.to_be_bytes());
    a.extend_from_slice(&sha);
    a.extend_from_slice(&spa);
    a.extend_from_slice(&tha);
    a.extend_from_slice(&tpa);
    a
}

/// An IPv4 packet read from a frame, its header checksum checked.
struct Packet {
    src: [u8; 4],
    dst: [u8; 4],
    proto: u8,
    ttl: u8,
    payload: Vec<u8>,
}

fn parse_ipv4(frame: &[u8]) -> Option<Packet> {
    if frame.len() < 34 || frame[12..14] != IP_TYPE.to_be_bytes() {
        return None;
    }
    let p = &frame[14..];
    let hlen = usize::from(p[0] & 0xf) * 4;
    let total = usize::from(u16::from_be_bytes([p[2], p[3]]));
    assert_eq!(checksum(&p[..hlen], 0), 0, "bad IPv4 checksum from the kernel");
    assert_eq!(p[6] & 0x20, 0, "the kernel never fragments");
    Some(Packet {
        src: p[12..16].try_into().unwrap(),
        dst: p[16..20].try_into().unwrap(),
        proto: p[9],
        ttl: p[8],
        payload: p[hlen..total].to_vec(),
    })
}

// ---- the other host, played by this test on a hub port ----

const HOST_MAC: [u8; 6] = [0x02, 0, 0, 0, 0, 0x99];
const HOST_IP: [u8; 4] = [10, 0, 9, 1];
const VM_IP: [u8; 4] = [10, 0, 9, 2];
/// The first card attached to a hub gets port 1's address.
const VM_MAC: [u8; 6] = [0x02, 0xca, 0, 0, 0, 1];
const MASK: [u8; 4] = [255, 255, 255, 0];

struct Host {
    port: HubPort,
    /// Every frame the machine sent, in order, but its ARP requests for HOST_IP.
    seen: Vec<Vec<u8>>,
    /// Frames taken from `seen` by [`Host::take`] so far.
    taken: usize,
    /// The machine's ARP requests for HOST_IP, all answered. When they come depends on
    /// how long its cache keeps an entry and how many ticks the test's turns take
    /// (an idle machine skips to its next timer interrupt), so they are kept apart.
    asked: Vec<Vec<u8>>,
    /// Frames this host sent with a length the card takes (14 to 1514 bytes).
    sent: u32,
}

impl Host {
    /// A host on a new port of `hub`.
    fn new(hub: &Hub) -> Host {
        Host {
            port: hub.attach(),
            seen: Vec::new(),
            taken: 0,
            asked: Vec::new(),
            sent: 0,
        }
    }

    fn send(&mut self, frame: Vec<u8>) {
        if (14..=1514).contains(&frame.len()) {
            self.sent += 1;
        }
        self.port.send(&frame).unwrap();
    }

    /// Takes what arrived, answering ARP requests for HOST_IP and echo requests to it,
    /// as a host would.
    fn service(&mut self) {
        while let Some(f) = self.port.recv() {
            if f.len() >= 42
                && f[12..14] == ARP_TYPE.to_be_bytes()
                && f[20..22] == [0, 1]
                && f[38..42] == HOST_IP
            {
                let (sha, spa) = (f[22..28].try_into().unwrap(), f[28..32].try_into().unwrap());
                self.send(ether(
                    sha,
                    HOST_MAC,
                    ARP_TYPE,
                    &arp(2, HOST_MAC, HOST_IP, sha, spa),
                ));
                self.asked.push(f);
                continue;
            }
            if let Some(p) = parse_ipv4(&f) {
                if p.dst == HOST_IP && p.proto == 1 && p.payload[0] == 8 {
                    let id = u16::from_be_bytes([p.payload[4], p.payload[5]]);
                    let seq = u16::from_be_bytes([p.payload[6], p.payload[7]]);
                    let reply = icmp_echo(0, id, seq, &p.payload[8..]);
                    let src_mac = f[6..12].try_into().unwrap();
                    self.send(ether(
                        src_mac,
                        HOST_MAC,
                        IP_TYPE,
                        &ipv4(HOST_IP, p.src, 1, &reply),
                    ));
                }
            }
            self.seen.push(f);
        }
    }

    /// The frames that came since the last call.
    fn take(&mut self) -> Vec<Vec<u8>> {
        let new = self.seen[self.taken..].to_vec();
        self.taken = self.seen.len();
        new
    }
}

/// Sends `frame` to the machine, lets it run for a while (time enough to answer)
/// and returns what it sent back.
fn exchange(vm: &mut Vm, host: &mut Host, frame: Vec<u8>) -> Vec<Vec<u8>> {
    host.take();
    host.send(frame);
    for _ in 0..100 {
        vm.m.run_for(SLICE, &mut |_| {});
        host.service();
    }
    assert_eq!(vm.m.exit_code(), None, "the kernel stopped:\n{}", vm.out());
    host.take()
}

/// Sends a frame the machine must drop without a word.
fn ignored(vm: &mut Vm, host: &mut Host, what: &str, frame: Vec<u8>) {
    let back = exchange(vm, host, frame);
    assert!(back.is_empty(), "{what}: the kernel answered {back:x?}");
}

/// An echo request from the host to the machine, and a check of its reply.
fn echo(vm: &mut Vm, host: &mut Host, seq: u16) {
    let data = b"Camelot echo data";
    let req = ether(
        VM_MAC,
        HOST_MAC,
        IP_TYPE,
        &ipv4(HOST_IP, VM_IP, 1, &icmp_echo(8, 0x4242, seq, data)),
    );
    let back = exchange(vm, host, req);
    assert_eq!(back.len(), 1, "{back:x?}");
    assert_eq!(back[0][..12], [HOST_MAC, VM_MAC].concat());
    let p = parse_ipv4(&back[0]).unwrap();
    assert_eq!((p.src, p.dst, p.proto, p.ttl), (VM_IP, HOST_IP, 1, 64));
    assert_eq!(checksum(&p.payload, 0), 0, "bad ICMP checksum");
    assert_eq!(p.payload, icmp_echo(0, 0x4242, seq, data));
}

/// A UDP datagram from the host's port `sport` to the machine's echo service, and a
/// check of what came back.
fn udp_echo(vm: &mut Vm, host: &mut Host, sport: u16, data: &[u8], with_checksum: bool) {
    let mut u = udp(HOST_IP, VM_IP, sport, 7, data);
    if !with_checksum {
        u[6..8].fill(0);
    }
    let back = exchange(
        vm,
        host,
        ether(VM_MAC, HOST_MAC, IP_TYPE, &ipv4(HOST_IP, VM_IP, 17, &u)),
    );
    assert_eq!(back.len(), 1, "{back:x?}");
    let p = parse_ipv4(&back[0]).unwrap();
    assert_eq!((p.src, p.dst, p.proto), (VM_IP, HOST_IP, 17));
    assert_eq!(
        p.payload,
        udp(VM_IP, HOST_IP, 7, sport, data),
        "a checksum or a byte is wrong"
    );
}

// ---- the tests ----

#[test]
fn user_networking_gets_a_lease_by_dhcp_and_pings_the_gateway() {
    // Issue #83's first criterion.
    let mut vm = Vm::new(vec![NetConfig::user(Vec::new())], Vec::new(), &[]);
    boot(&mut [&mut vm], None);
    let out = vm.out();
    assert!(
        out.contains("disk: none\nnet 0: mac 52:54:00:12:34:56, irq 12, link up\ntick 1\n"),
        "{out}"
    );
    assert!(
        out.contains(&format!(
            "tick 10\nnet: 10.0.2.15/24, gateway 10.0.2.2, dns 10.0.2.3 (dhcp)\n{SHELL_BANNER}"
        )),
        "{out}"
    );
    let vms = &mut [&mut vm];
    assert_eq!(
        mask_times(&command(vms, 0, None, "ping 10.0.2.2 3")),
        ping_ok("10.0.2.2", 3)
    );
    // Loopback, by name and by the interface's own address.
    assert_eq!(
        mask_times(&command(vms, 0, None, "ping 127.0.0.1 2")),
        ping_ok("127.0.0.1", 2)
    );
    assert_eq!(
        mask_times(&command(vms, 0, None, "ping 10.0.2.15 1")),
        ping_ok("10.0.2.15", 1)
    );
    let ifconfig = command(vms, 0, None, "ifconfig");
    assert!(
        ifconfig.starts_with(
            "net0: mac 52:54:00:12:34:56, link up\n  \
             inet 10.0.2.15 netmask 255.255.255.0 gateway 10.0.2.2 dns 10.0.2.3 (dhcp)\n"
        ),
        "{ifconfig}"
    );
    assert!(
        ifconfig.ends_with("lo: inet 127.0.0.1 netmask 255.0.0.0\n"),
        "{ifconfig}"
    );
    // Nothing was dropped. Sent: two DHCP messages, an ARP request for the gateway and
    // three echo requests; received their answers. Loopback never reaches the card.
    assert!(counters(&ifconfig).values().all(|&n| n == 0), "{ifconfig}");
    assert_eq!(traffic(&ifconfig), (6, 6), "{ifconfig}");
    exit(&mut vm);
}

#[test]
fn grail_net_user_boots_camelot_onto_the_network() {
    // The same through the command line, as a user would run it.
    let kernel = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../kernel/kernel.elf");
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_grail"))
        .args(["--max-steps", "400000000", "--ram", "16", "--net", "user"])
        .arg(kernel)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    use std::io::Write;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"ping 10.0.2.2 2\nexit\n")
        .unwrap();
    let out = child.wait_with_output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{stdout}{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let shell = &stdout[stdout.find(SHELL_BANNER).unwrap()..];
    assert_eq!(
        mask_times(shell),
        format!(
            "{SHELL_BANNER}{PROMPT}ping 10.0.2.2 2\n{}{PROMPT}exit\n",
            ping_ok("10.0.2.2", 2)
        )
    );
}

/// A [`gateway::Gateway`] on a clock of its own rather than the wall clock: each poll of
/// the card moves it on by the time [`POLL_STEPS`] take at 10 MHz, so when it services
/// its host sockets (and every timer it has) follows the machine's steps. The test keeps
/// a handle to it to wait for the host's datagram and to see what the machine sends.
#[derive(Clone)]
struct SteppedGateway(Arc<Mutex<Stepped>>);

struct Stepped {
    gw: gateway::Gateway,
    now: Instant,
    /// Frames for the machine taken from the gateway by [`SteppedGateway::wait_for_host`].
    early: VecDeque<Vec<u8>>,
    /// Every frame the machine sent.
    from_guest: Vec<Vec<u8>>,
}

/// The time one poll of the card stands for: [`POLL_STEPS`] steps at 10 MHz.
const POLL_TIME: Duration = Duration::from_nanos(POLL_STEPS as u64 * 100);

impl SteppedGateway {
    fn new(gw: gateway::Gateway) -> SteppedGateway {
        SteppedGateway(Arc::new(Mutex::new(Stepped {
            gw,
            now: Instant::now(),
            early: VecDeque::new(),
            from_guest: Vec::new(),
        })))
    }

    /// Services the gateway's host sockets, with the machine stopped, until what a host
    /// program sent has reached them and become a frame for the machine. Only the
    /// host's own loopback delivery is left to the wall clock here, and it is bounded.
    fn wait_for_host(&self) {
        let start = Instant::now();
        loop {
            let mut s = self.0.lock().unwrap();
            // Past the service interval, so each call reads the sockets.
            s.now += Duration::from_millis(1);
            let now = s.now;
            if let Some(frame) = s.gw.recv_at(now) {
                s.early.push_back(frame);
                return;
            }
            drop(s);
            assert!(
                start.elapsed() < Duration::from_secs(30),
                "the datagram never reached the gateway"
            );
            std::thread::yield_now();
        }
    }

    fn guest_frames(&self) -> Vec<Vec<u8>> {
        self.0.lock().unwrap().from_guest.clone()
    }
}

impl NetBackend for SteppedGateway {
    fn send(&mut self, frame: &[u8]) -> std::io::Result<()> {
        let mut s = self.0.lock().unwrap();
        s.from_guest.push(frame.to_vec());
        let now = s.now;
        s.gw.send_at(frame, now);
        Ok(())
    }

    fn recv(&mut self) -> Option<Vec<u8>> {
        let mut s = self.0.lock().unwrap();
        s.now += POLL_TIME;
        let now = s.now;
        match s.early.pop_front() {
            Some(frame) => Some(frame),
            None => s.gw.recv_at(now),
        }
    }
}

#[test]
fn the_udp_echo_service_answers_through_a_forwarded_port() {
    // UDP end to end through the gateway, which checks every checksum: a host program
    // sends to a forwarded port, the gateway passes the datagram to the kernel's echo
    // service on port 7, and the answer comes back the same way.
    //
    // The gateway runs on the machine's steps (SteppedGateway), so the machine's side
    // is deterministic. The two hops through host sockets are bounded instead: the
    // datagram is in the gateway's frame queue before the machine runs again, and the
    // machine's answer is handed to the host (a synchronous send) before this waits
    // for it, with a read timeout.
    let gw = gateway::Gateway::new(gateway::Config {
        hostfwd: vec![gateway::HostFwd::udp(0, 7)],
        ..gateway::Config::default()
    })
    .unwrap();
    let forwarded = gw.hostfwd()[0].host;
    let gw = SteppedGateway::new(gw);
    let cards: Vec<(MacAddr, Box<dyn NetBackend>)> =
        vec![(MacAddr(gateway::DEFAULT_GUEST_MAC), Box::new(gw.clone()))];
    let mut vm = Vm::new(Vec::new(), cards, &[]);
    boot(&mut [&mut vm], None);
    assert!(vm.out().contains("net: 10.0.2.15/24"), "{}", vm.out());

    let sock = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    sock.send_to(b"Hello, echo service!", forwarded).unwrap();
    gw.wait_for_host();

    // The echo, from 10.0.2.15 port 7, is what the machine sends next.
    let is_echo = |f: &Vec<u8>| {
        parse_ipv4(f).is_some_and(|p| p.src == [10, 0, 2, 15] && p.proto == 17 && p.payload[..2] == [0, 7])
    };
    let sent = gw.guest_frames().len();
    let mut steps = 0;
    while !gw.guest_frames()[sent..].iter().any(is_echo) {
        vm.m.run_for(SLICE, &mut |_| {});
        assert_eq!(vm.m.exit_code(), None, "the kernel stopped:\n{}", vm.out());
        steps += SLICE;
        assert!(steps < 10_000_000, "no echo from the kernel:\n{}", vm.out());
    }
    sock.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    let mut buf = [0u8; 64];
    let (n, from) = sock.recv_from(&mut buf).unwrap();
    assert_eq!(
        (&buf[..n], from),
        (&b"Hello, echo service!"[..], forwarded.into())
    );

    let ifconfig = command(&mut [&mut vm], 0, None, "ifconfig");
    assert!(counters(&ifconfig).values().all(|&n| n == 0), "{ifconfig}");
    exit(&mut vm);
}

#[test]
fn two_kernels_on_a_hub_ping_each_other() {
    // Issue #83's second criterion. `a` boots with a static address from the kernel's
    // configuration; `b` tries DHCP, finds no server on the hub, falls back to the
    // default static address, and gets its own with ifconfig.
    let hub = Hub::new();
    let mut a = Vm::new(
        vec![NetConfig::hub(&hub)],
        Vec::new(),
        &static_config([192, 168, 7, 1], [255, 255, 255, 0]),
    );
    let mut b = Vm::new(vec![NetConfig::hub(&hub)], Vec::new(), &[]);
    let vms = &mut [&mut a, &mut b];
    boot(vms, None);
    assert!(
        vms[0].out().contains("tick 10\nnet: 192.168.7.1/24 (static)\n"),
        "{}",
        vms[0].out()
    );
    assert!(
        vms[1].out().contains(
            "net 0: mac 02:ca:00:00:00:02, irq 12, link up\n\
             tick 1\n"
        ),
        "{}",
        vms[1].out()
    );
    assert!(
        vms[1].out().contains(
            "tick 10\nnet: no answer from DHCP\n\
             net: 10.0.2.15/24, gateway 10.0.2.2 (static)\n"
        ),
        "{}",
        vms[1].out()
    );
    assert_eq!(command(vms, 1, None, "ifconfig 192.168.7.2"), "");
    let ifconfig = command(vms, 1, None, "ifconfig");
    assert!(
        ifconfig.contains("\n  inet 192.168.7.2 netmask 255.255.255.0 (static)\n"),
        "{ifconfig}"
    );

    assert_eq!(
        mask_times(&command(vms, 0, None, "ping 192.168.7.2 3")),
        ping_ok("192.168.7.2", 3)
    );
    assert_eq!(
        mask_times(&command(vms, 1, None, "ping 192.168.7.1 3")),
        ping_ok("192.168.7.1", 3)
    );
    // Off the subnet with no gateway: no route.
    assert_eq!(
        command(vms, 0, None, "ping 10.1.2.3"),
        "PING 10.1.2.3: 56 data bytes\nsh: ping: network unreachable\n"
    );

    for i in 0..2 {
        let ifconfig = command(vms, i, None, "ifconfig");
        let mut c = counters(&ifconfig);
        // `a` got b's four DHCP discovers, for a port it has no socket on; ARP requests
        // for the other were filtered; nothing else went wrong.
        assert_eq!(
            c.remove("no-port"),
            Some(if i == 0 { 4 } else { 0 }),
            "{ifconfig}"
        );
        c.remove("filtered");
        assert!(c.values().all(|&n| n == 0), "{ifconfig}");
    }
    exit(&mut a);
    exit(&mut b);
}

#[test]
fn malformed_frames_are_dropped_and_counted() {
    // Issue #83's third criterion: this test is the other host on the hub and sends
    // the kernel every kind of broken frame; each is dropped and counted under its
    // reason, nothing panics, and the stack still answers afterwards.
    let hub = Hub::new();
    let mut vm = Vm::new(
        vec![NetConfig::hub(&hub)],
        Vec::new(),
        &static_config(VM_IP, MASK),
    );
    let mut host = Host::new(&hub);
    boot(&mut [&mut vm], Some(&mut host));
    assert!(vm.out().contains("net: 10.0.9.2/24 (static)\n"), "{}", vm.out());
    assert!(host.take().is_empty(), "a static configuration sends nothing");

    // The kernel pings the host: it asks for the host's MAC by ARP, the host answers
    // that and the echo requests.
    let out = command(&mut [&mut vm], 0, Some(&mut host), "ping 10.0.9.1 2");
    assert_eq!(mask_times(&out), ping_ok("10.0.9.1", 2));
    let request = ether(
        [0xff; 6],
        VM_MAC,
        ARP_TYPE,
        &arp(1, VM_MAC, VM_IP, [0; 6], HOST_IP),
    );
    assert_eq!(host.asked, [request], "the ARP request");
    let sent = host.take();
    assert_eq!(sent.len(), 2, "two echo requests: {sent:x?}");

    // ARP from the host: answered, from the address asked about.
    let back = exchange(
        &mut vm,
        &mut host,
        ether(
            [0xff; 6],
            HOST_MAC,
            ARP_TYPE,
            &arp(1, HOST_MAC, HOST_IP, [0; 6], VM_IP),
        ),
    );
    assert_eq!(
        back,
        [ether(
            HOST_MAC,
            VM_MAC,
            ARP_TYPE,
            &arp(2, VM_MAC, VM_IP, HOST_MAC, HOST_IP)
        )]
    );
    echo(&mut vm, &mut host, 1);
    udp_echo(&mut vm, &mut host, 40000, b"UDP echo, with a checksum", true);
    udp_echo(&mut vm, &mut host, 40001, b"UDP echo, without one", false);

    let to_vm = |payload: &[u8]| ether(VM_MAC, HOST_MAC, IP_TYPE, payload);
    let good_icmp = icmp_echo(8, 7, 7, b"x");
    let good_ip = ipv4(HOST_IP, VM_IP, 1, &good_icmp);
    let mut want: HashMap<&str, u32> = HashMap::new();
    let mut drop = |vm: &mut Vm, host: &mut Host, counter: &'static str, what: &str, frame: Vec<u8>| {
        ignored(vm, host, what, frame);
        *want.entry(counter).or_default() += 1;
    };

    // Ethernet: for another MAC, an unknown EtherType.
    drop(
        &mut vm,
        &mut host,
        "filtered",
        "another MAC",
        ether([2, 0, 0, 0, 0, 0x55], HOST_MAC, IP_TYPE, &good_ip),
    );
    drop(
        &mut vm,
        &mut host,
        "ether",
        "IPv6",
        ether(VM_MAC, HOST_MAC, 0x86dd, &[0; 40]),
    );
    // ARP: truncated, not Ethernet, an unknown operation.
    let request = arp(1, HOST_MAC, HOST_IP, [0; 6], VM_IP);
    drop(
        &mut vm,
        &mut host,
        "arp",
        "short ARP",
        ether(VM_MAC, HOST_MAC, ARP_TYPE, &request[..20]),
    );
    let mut bad = request.clone();
    bad[1] = 6;
    drop(
        &mut vm,
        &mut host,
        "arp",
        "ARP for IEEE 802",
        ether(VM_MAC, HOST_MAC, ARP_TYPE, &bad),
    );
    let mut bad = request.clone();
    bad[7] = 9;
    drop(
        &mut vm,
        &mut host,
        "arp",
        "ARP operation 9",
        ether(VM_MAC, HOST_MAC, ARP_TYPE, &bad),
    );
    // IPv4 headers: nothing at all, truncated, version 6, a header length of 4 words,
    // a total length past the frame and one shorter than the header.
    drop(&mut vm, &mut host, "ip", "empty IPv4", to_vm(&[]));
    drop(
        &mut vm,
        &mut host,
        "ip",
        "10 bytes of IPv4",
        to_vm(&good_ip[..10]),
    );
    let mut bad = good_ip.clone();
    bad[0] = 0x65;
    fix_ip_checksum(&mut bad);
    drop(&mut vm, &mut host, "ip", "version 6", to_vm(&bad));
    let mut bad = good_ip.clone();
    bad[0] = 0x44;
    drop(&mut vm, &mut host, "ip", "IHL 4", to_vm(&bad));
    let mut bad = good_ip.clone();
    bad[2..4].copy_from_slice(&(good_ip.len() as u16 + 1).to_be_bytes());
    fix_ip_checksum(&mut bad);
    drop(
        &mut vm,
        &mut host,
        "ip",
        "total length past the frame",
        to_vm(&bad),
    );
    let mut bad = good_ip.clone();
    bad[2..4].copy_from_slice(&19u16.to_be_bytes());
    fix_ip_checksum(&mut bad);
    drop(&mut vm, &mut host, "ip", "total length 19", to_vm(&bad));
    let mut bad = good_ip.clone();
    bad[10] ^= 0x10;
    drop(&mut vm, &mut host, "ip-checksum", "header checksum", to_vm(&bad));
    // Fragments: the first (MF), and a later one (an offset).
    let mut bad = good_ip.clone();
    bad[6] = 0x20;
    fix_ip_checksum(&mut bad);
    drop(&mut vm, &mut host, "fragment", "first fragment", to_vm(&bad));
    let mut bad = good_ip.clone();
    bad[6..8].copy_from_slice(&[0, 0x10]);
    fix_ip_checksum(&mut bad);
    drop(&mut vm, &mut host, "fragment", "last fragment", to_vm(&bad));
    // For another address on the subnet; TCP.
    drop(
        &mut vm,
        &mut host,
        "filtered",
        "another address",
        to_vm(&ipv4(HOST_IP, [10, 0, 9, 77], 1, &good_icmp)),
    );
    drop(
        &mut vm,
        &mut host,
        "protocol",
        "TCP",
        to_vm(&ipv4(HOST_IP, VM_IP, 6, &[0; 20])),
    );
    // ICMP: a bad checksum, truncated.
    let mut bad = good_icmp.clone();
    bad[8] ^= 1;
    drop(
        &mut vm,
        &mut host,
        "icmp",
        "ICMP checksum",
        to_vm(&ipv4(HOST_IP, VM_IP, 1, &bad)),
    );
    drop(
        &mut vm,
        &mut host,
        "icmp",
        "4 bytes of ICMP",
        to_vm(&ipv4(HOST_IP, VM_IP, 1, &good_icmp[..4])),
    );
    // UDP: truncated, a length past the packet, one shorter than the header, a bad
    // checksum; then good ones to a port nobody has, and from a port below 1024 to the
    // echo service (which must not answer it).
    let good_udp = udp(HOST_IP, VM_IP, 40000, 7, b"hello");
    drop(
        &mut vm,
        &mut host,
        "udp",
        "4 bytes of UDP",
        to_vm(&ipv4(HOST_IP, VM_IP, 17, &good_udp[..4])),
    );
    let mut bad = good_udp.clone();
    bad[4..6].copy_from_slice(&(good_udp.len() as u16 + 1).to_be_bytes());
    drop(
        &mut vm,
        &mut host,
        "udp",
        "UDP length past the packet",
        to_vm(&ipv4(HOST_IP, VM_IP, 17, &bad)),
    );
    let mut bad = good_udp.clone();
    bad[4..6].copy_from_slice(&7u16.to_be_bytes());
    drop(
        &mut vm,
        &mut host,
        "udp",
        "UDP length 7",
        to_vm(&ipv4(HOST_IP, VM_IP, 17, &bad)),
    );
    let mut bad = good_udp.clone();
    bad[9] ^= 1;
    drop(
        &mut vm,
        &mut host,
        "udp-checksum",
        "UDP checksum",
        to_vm(&ipv4(HOST_IP, VM_IP, 17, &bad)),
    );
    let to_port = udp(HOST_IP, VM_IP, 40000, 9999, b"anyone?");
    drop(
        &mut vm,
        &mut host,
        "no-port",
        "port 9999",
        to_vm(&ipv4(HOST_IP, VM_IP, 17, &to_port)),
    );
    let from_7 = udp(HOST_IP, VM_IP, 7, 7, b"loop?");
    drop(
        &mut vm,
        &mut host,
        "no-port",
        "echo from port 7",
        to_vm(&ipv4(HOST_IP, VM_IP, 17, &from_7)),
    );
    // Lengths the card itself refuses: shorter than a header, longer than 1514.
    drop(&mut vm, &mut host, "card", "10-byte frame", vec![0xff; 10]);
    drop(
        &mut vm,
        &mut host,
        "card",
        "1600-byte frame",
        ether(VM_MAC, HOST_MAC, IP_TYPE, &[0; 1586]),
    );

    // Still alive: it answers as before.
    echo(&mut vm, &mut host, 2);
    udp_echo(&mut vm, &mut host, 40002, b"still here", true);

    let ifconfig = command(&mut [&mut vm], 0, Some(&mut host), "ifconfig");
    let got = counters(&ifconfig);
    for (name, n) in &got {
        assert_eq!(
            *n,
            want.get(name.as_str()).copied().unwrap_or(0),
            "{name}:\n{ifconfig}"
        );
    }
    assert_eq!(got.len(), 16, "{ifconfig}");
    assert!(want.keys().all(|name| got.contains_key(*name)), "{want:?}");
    // Every frame the card took counts as received; what it sent: the two echo requests
    // of its ping, the replies (ARP, two echoes, three UDP) and its ARP requests.
    assert_eq!(
        traffic(&ifconfig),
        (host.sent, 8 + host.asked.len() as u32),
        "{ifconfig}"
    );
    exit(&mut vm);
}

#[test]
fn arp_gives_up_on_an_address_nobody_has() {
    // A ping to an address with no host behind it: ARP asks three times, a second
    // apart, then gives up; the pings time out, the packets waiting for an answer are
    // dropped and counted, and for a while the address is unreachable at once.
    let hub = Hub::new();
    let mut vm = Vm::new(
        vec![NetConfig::hub(&hub)],
        Vec::new(),
        &static_config(VM_IP, MASK),
    );
    let mut host = Host::new(&hub);
    boot(&mut [&mut vm], Some(&mut host));
    let out = command(&mut [&mut vm], 0, Some(&mut host), "ping 10.0.9.50 3");
    assert_eq!(
        out,
        "PING 10.0.9.50: 56 data bytes\nseq=1: timed out\nseq=2: timed out\nseq=3: timed out\n3 sent, 0 received\n"
    );
    let request = ether(
        [0xff; 6],
        VM_MAC,
        ARP_TYPE,
        &arp(1, VM_MAC, VM_IP, [0; 6], [10, 0, 9, 50]),
    );
    assert_eq!(
        host.take(),
        [request.clone(), request.clone(), request],
        "three ARP requests"
    );
    // ARP has given up by now: the next ping fails without waiting.
    let out = command(&mut [&mut vm], 0, Some(&mut host), "ping 10.0.9.50 1");
    assert_eq!(
        out,
        "PING 10.0.9.50: 56 data bytes\nseq=1: host unreachable\n1 sent, 0 received\n"
    );
    assert!(host.take().is_empty());
    let ifconfig = command(&mut [&mut vm], 0, Some(&mut host), "ifconfig");
    // Each ping's packet but the last was replaced in the ARP entry by the next one;
    // the last was dropped when ARP gave up; the fourth was refused.
    assert_eq!(counters(&ifconfig)["arp-failed"], 4, "{ifconfig}");
    exit(&mut vm);
}
