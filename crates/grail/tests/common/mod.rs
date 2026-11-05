//! What the tests of the Camelot kernel's network stack share (kernel_net.rs,
//! kernel_tcp.rs, kernel_sockets.rs): machines run in turns by step counts with the shell driven through
//! the UART, frames crafted and read back, a host played by the test on a hub port,
//! and a gateway on a clock that follows the machine's steps.
//!
//! The kernel is `kernel/kernel.elf`, on Camelot's firmware. Its variables are
//! read and set through their load addresses ([`image_pa`]): the image is
//! linked in the upper half and loaded at 0x8020_0000.
#![allow(dead_code)]

pub mod kstacks;

use grail::net::{Hub, HubPort, MacAddr, NetBackend, POLL_STEPS};
use grail::{Machine, MachineConfig, NetConfig, RunResult};
use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// 16 MiB rather than the default 4: room to spare for the tests that run several
/// programs at once.
pub const RAM: usize = 16 << 20;
/// Steps a machine runs per turn.
pub const SLICE: u64 = 2_000;
/// More than any wait here needs: a boot is about 10 million steps with 16 MiB, a
/// command a few million at most.
pub const MAX_STEPS: u64 = 400_000_000;

pub const SHELL_BANNER: &str = "Type help for the list of commands.\n";
pub const PROMPT: &str = "$ ";

/// The committed kernel, as a path from the repository root.
pub fn kernel_path() -> &'static str {
    "kernel/kernel.elf"
}

pub fn kernel_elf() -> Vec<u8> {
    std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(kernel_path()),
    )
    .unwrap()
}

/// The firmware the kernel boots on: Camelot's, as the kernel is S-mode only.
pub fn bios() -> grail::Bios {
    grail::Bios::Default
}

/// Where the kernel image is linked, and where it is loaded: after the
/// firmware's 2 MiB.
const KERNEL_LINK_ADDR: u64 = 0xffff_ffff_8000_0000;
const LOAD_ADDR: u64 = 0x8020_0000;

/// The physical address of `va`, an address in the kernel image: its load
/// address.
pub fn image_pa(va: u64) -> u64 {
    assert!(va >= KERNEL_LINK_ADDR, "{va:#x} is not in the kernel image");
    va - KERNEL_LINK_ADDR + LOAD_ADDR
}

/// Addresses as the kernel's variables hold them: in host order.
pub fn ip(a: [u8; 4]) -> u32 {
    u32::from_be_bytes(a)
}

/// The kernel's static network configuration (kernel/net/net.c), to patch into a
/// machine's RAM before it boots: no DHCP, this address and mask, no gateway.
pub fn static_config(addr: [u8; 4], mask: [u8; 4]) -> Vec<(&'static str, u32)> {
    vec![
        ("net_dhcp", 0),
        ("net_static_addr", ip(addr)),
        ("net_static_mask", ip(mask)),
        ("net_static_gateway", 0),
    ]
}

/// A Camelot machine: the kernel with its UART captured and fed through `input`.
pub struct Vm {
    pub m: Machine,
    pub input: Sender<u8>,
    /// The address of the packet buffer pool's free count (`free_count` in
    /// kernel/net/pbuf.c).
    pub free_count: u64,
}

impl Vm {
    /// The kernel on a machine with `config`'s cards and `cards`, with the kernel
    /// variables in `patches` set.
    pub fn new(
        nets: Vec<NetConfig>,
        cards: Vec<(MacAddr, Box<dyn NetBackend>)>,
        patches: &[(&str, u32)],
    ) -> Vm {
        let config = MachineConfig {
            ram_size: RAM,
            nets,
            ..MachineConfig::default()
        };
        Vm::with_config(config, cards, patches)
    }

    /// The kernel on a machine built from `config` (its disks, say), with `cards`
    /// and `patches` as for [`Vm::new`].
    pub fn with_config(
        config: MachineConfig,
        cards: Vec<(MacAddr, Box<dyn NetBackend>)>,
        patches: &[(&str, u32)],
    ) -> Vm {
        let config = MachineConfig {
            bios: bios(),
            ..config
        };
        let mut m = Machine::with_devices(config, Vec::new(), cards).unwrap();
        m.cpu.bus.uart.capture = true;
        let file = kernel_elf();
        m.load_elf(&file).unwrap();
        let elf = grail::elf::parse(&file).unwrap();
        for (name, value) in patches {
            let addr = elf.symbol(name).unwrap_or_else(|| panic!("no symbol {name}"));
            m.cpu.bus.load(image_pa(addr), &value.to_le_bytes()).unwrap();
        }
        let free_count = image_pa(elf.symbol("free_count").expect("no symbol free_count"));
        let (input, rx) = channel();
        m.set_uart_input(rx);
        Vm { m, input, free_count }
    }

    pub fn out(&self) -> String {
        String::from_utf8_lossy(&self.m.cpu.bus.uart.captured).into_owned()
    }

    /// Prompts printed so far: one when the shell starts, one after each command.
    pub fn prompts(&self) -> usize {
        self.out().matches(&format!("\n{PROMPT}")).count()
    }

    pub fn type_line(&self, line: &str) {
        for b in line.bytes().chain(std::iter::once(b'\n')) {
            self.input.send(b).unwrap();
        }
    }

    /// The 32-bit kernel variable `name`, read from RAM.
    pub fn read_var(&self, name: &str) -> u32 {
        self.m.cpu.bus.read_mem(var_addr(name), 4).unwrap() as u32
    }

    /// Sets the 32-bit kernel variable `name` in RAM (one the kernel clears while it
    /// boots, in .bss, once it has booted).
    pub fn write_var(&mut self, name: &str, value: u32) {
        self.m.cpu.bus.load(var_addr(name), &value.to_le_bytes()).unwrap();
    }

    /// The pages the kernel has not lost ([`kstacks::pages`]): the free ones
    /// (`nfree`, kernel/kalloc.c), the pages of every mapped kernel stack, and
    /// the stacks in use.
    pub fn pages(&self) -> kstacks::Pages {
        kstacks::pages(&self.m, &kernel_elf())
    }
}

/// The physical address of the kernel variable `name`, looked up once.
pub fn var_addr(name: &str) -> u64 {
    static ADDRS: OnceLock<Mutex<HashMap<String, u64>>> = OnceLock::new();
    let mut addrs = ADDRS.get_or_init(Default::default).lock().unwrap();
    *addrs.entry(name.to_string()).or_insert_with(|| {
        let file = kernel_elf();
        let elf = grail::elf::parse(&file).unwrap();
        image_pa(elf.symbol(name).unwrap_or_else(|| panic!("no symbol {name}")))
    })
}

/// Runs the machines in turns, letting `host` (if any) see what they send between
/// turns, until `done` holds. Panics if a machine stops or the steps run out.
pub fn run_until(vms: &mut [&mut Vm], mut host: Option<&mut Host>, done: impl Fn(&[&mut Vm]) -> bool) {
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
pub fn boot(vms: &mut [&mut Vm], host: Option<&mut Host>) {
    run_until(vms, host, |vms| vms.iter().all(|vm| vm.prompts() >= 1));
}

/// Types `line` into machine `i`'s shell and runs until its next prompt. Returns what
/// the command printed (after the echoed line, up to the prompt).
pub fn command(vms: &mut [&mut Vm], i: usize, host: Option<&mut Host>, line: &str) -> String {
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

/// Buffers the kernel's pool holds (`NPBUF`, kernel/include/net.h) and the ones its
/// card's receive ring always has (`NIC_RX_RING`, kernel/include/nic.h).
pub const NPBUF: u32 = 64;
pub const NIC_RX_RING: u32 = 16;

/// Checks that every packet buffer is back in the pool but the receive ring's: a path
/// that forgets to free one would drain the pool, slowly, until the network stops. The
/// machine runs a little first, idle at its prompt, so the card's last interrupt has
/// reclaimed what it sent.
pub fn assert_buffers_back(vm: &mut Vm) {
    for _ in 0..50 {
        vm.m.run_for(SLICE, &mut |_| {});
    }
    let free = vm.m.cpu.bus.read_mem(vm.free_count, 4).unwrap();
    assert_eq!(
        free,
        u64::from(NPBUF - NIC_RX_RING),
        "packet buffers leaked:\n{}",
        vm.out()
    );
}

/// Leaves the shell; the machine must stop with status 0.
pub fn exit(vm: &mut Vm) {
    vm.type_line("exit");
    assert_eq!(
        vm.m.run(Some(MAX_STEPS), |_| {}),
        RunResult::Exited(0),
        "{}",
        vm.out()
    );
}

/// The counters `ifconfig` prints ("name N" after the colon of its counter lines).
pub fn counters(ifconfig: &str) -> HashMap<String, u32> {
    let mut map = HashMap::new();
    for line in ifconfig.lines() {
        let Some((head, list)) = line.split_once(": ") else {
            continue;
        };
        if !(head.ends_with("rx dropped") || head.ends_with("other") || head.trim() == "tcp") {
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
pub fn traffic(ifconfig: &str) -> (u32, u32) {
    let line = ifconfig
        .lines()
        .find(|l| l.starts_with("  rx ") && l.contains("; tx "))
        .unwrap();
    let words: Vec<&str> = line.split_whitespace().collect();
    (words[1].parse().unwrap(), words[6].parse().unwrap())
}

// ---- crafting frames ----
//
// gateway::wire has builders and parsers for all of these, but the test keeps its own,
// written from the RFCs: the kernel's checksums and layouts are checked against a second,
// independent implementation, so a mistake shared with the gateway's can't hide.

pub const IP_TYPE: u16 = 0x0800;
pub const ARP_TYPE: u16 = 0x0806;

/// The Internet checksum of `data` (RFC 1071), starting from `sum`.
pub fn checksum(data: &[u8], mut sum: u32) -> u16 {
    for c in data.chunks(2) {
        sum += (u32::from(c[0]) << 8) | u32::from(*c.get(1).unwrap_or(&0));
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    !(sum as u16)
}

pub fn ether(dst: [u8; 6], src: [u8; 6], ty: u16, payload: &[u8]) -> Vec<u8> {
    let mut f = Vec::new();
    f.extend_from_slice(&dst);
    f.extend_from_slice(&src);
    f.extend_from_slice(&ty.to_be_bytes());
    f.extend_from_slice(payload);
    f
}

/// An IPv4 header without options and `payload`, with a right checksum.
pub fn ipv4(src: [u8; 4], dst: [u8; 4], proto: u8, payload: &[u8]) -> Vec<u8> {
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
pub fn fix_ip_checksum(p: &mut [u8]) {
    let hlen = usize::from(p[0] & 0xf) * 4;
    p[10..12].fill(0);
    let sum = checksum(&p[..hlen], 0);
    p[10..12].copy_from_slice(&sum.to_be_bytes());
}

pub fn icmp_echo(ty: u8, id: u16, seq: u16, data: &[u8]) -> Vec<u8> {
    let mut m = vec![ty, 0, 0, 0];
    m.extend_from_slice(&id.to_be_bytes());
    m.extend_from_slice(&seq.to_be_bytes());
    m.extend_from_slice(data);
    let sum = checksum(&m, 0);
    m[2..4].copy_from_slice(&sum.to_be_bytes());
    m
}

/// The sum of UDP's pseudo-header, for [`checksum`].
pub fn pseudo(src: [u8; 4], dst: [u8; 4], len: usize) -> u32 {
    let w = |a: [u8; 4]| {
        u32::from(u16::from_be_bytes([a[0], a[1]])) + u32::from(u16::from_be_bytes([a[2], a[3]]))
    };
    w(src) + w(dst) + 17 + len as u32
}

pub fn udp(src: [u8; 4], dst: [u8; 4], sport: u16, dport: u16, data: &[u8]) -> Vec<u8> {
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

pub fn arp(op: u16, sha: [u8; 6], spa: [u8; 4], tha: [u8; 6], tpa: [u8; 4]) -> Vec<u8> {
    let mut a = vec![0, 1, 8, 0, 6, 4];
    a.extend_from_slice(&op.to_be_bytes());
    a.extend_from_slice(&sha);
    a.extend_from_slice(&spa);
    a.extend_from_slice(&tha);
    a.extend_from_slice(&tpa);
    a
}

/// An IPv4 packet read from a frame, its header checksum checked.
pub struct Packet {
    pub src: [u8; 4],
    pub dst: [u8; 4],
    pub proto: u8,
    pub ttl: u8,
    pub payload: Vec<u8>,
}

pub fn parse_ipv4(frame: &[u8]) -> Option<Packet> {
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

pub const HOST_MAC: [u8; 6] = [0x02, 0, 0, 0, 0, 0x99];
pub const HOST_IP: [u8; 4] = [10, 0, 9, 1];
pub const VM_IP: [u8; 4] = [10, 0, 9, 2];
/// The first card attached to a hub gets port 1's address.
pub const VM_MAC: [u8; 6] = [0x02, 0xca, 0, 0, 0, 1];
pub const MASK: [u8; 4] = [255, 255, 255, 0];

pub struct Host {
    pub port: HubPort,
    /// Every frame the machine sent, in order, but its ARP requests for HOST_IP.
    pub seen: Vec<Vec<u8>>,
    /// Frames taken from `seen` by [`Host::take`] so far.
    pub taken: usize,
    /// The machine's ARP requests for HOST_IP, all answered. When they come depends on
    /// how long its cache keeps an entry and how many ticks the test's turns take
    /// (an idle machine skips to its next timer interrupt), so they are kept apart.
    pub asked: Vec<Vec<u8>>,
    /// Frames this host sent with a length the card takes (14 to 1514 bytes).
    pub sent: u32,
}

impl Host {
    /// A host on a new port of `hub`.
    pub fn new(hub: &Hub) -> Host {
        Host {
            port: hub.attach(),
            seen: Vec::new(),
            taken: 0,
            asked: Vec::new(),
            sent: 0,
        }
    }

    pub fn send(&mut self, frame: Vec<u8>) {
        if (14..=1514).contains(&frame.len()) {
            self.sent += 1;
        }
        self.port.send(&frame).unwrap();
    }

    /// Takes what arrived, answering ARP requests for HOST_IP and echo requests to it,
    /// as a host would.
    pub fn service(&mut self) {
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
    pub fn take(&mut self) -> Vec<Vec<u8>> {
        let new = self.seen[self.taken..].to_vec();
        self.taken = self.seen.len();
        new
    }
}

/// Sends `frame` to the machine, lets it run for a while (time enough to answer)
/// and returns what it sent back.
pub fn exchange(vm: &mut Vm, host: &mut Host, frame: Vec<u8>) -> Vec<Vec<u8>> {
    host.take();
    host.send(frame);
    for _ in 0..100 {
        vm.m.run_for(SLICE, &mut |_| {});
        host.service();
    }
    assert_eq!(vm.m.exit_code(), None, "the kernel stopped:\n{}", vm.out());
    host.take()
}

/// A [`gateway::Gateway`] on a clock of its own rather than the wall clock: each poll of
/// the card moves it on by the time [`POLL_STEPS`] take at 10 MHz, so when it services
/// its host sockets (and every timer it has) follows the machine's steps. The test keeps
/// a handle to it to wait for the host's datagram and to see what the machine sends.
#[derive(Clone)]
pub struct SteppedGateway(Arc<Mutex<Stepped>>);

pub struct Stepped {
    pub gw: gateway::Gateway,
    pub now: Instant,
    /// Frames for the machine taken from the gateway by [`SteppedGateway::wait_for_host`].
    pub early: VecDeque<Vec<u8>>,
    /// Every frame the machine sent.
    pub from_guest: Vec<Vec<u8>>,
}

/// The time one poll of the card stands for: [`POLL_STEPS`] steps at 10 MHz.
pub const POLL_TIME: Duration = Duration::from_nanos(POLL_STEPS as u64 * 100);

impl SteppedGateway {
    pub fn new(gw: gateway::Gateway) -> SteppedGateway {
        SteppedGateway(Arc::new(Mutex::new(Stepped {
            gw,
            now: Instant::now(),
            early: VecDeque::new(),
            from_guest: Vec::new(),
        })))
    }

    /// Services the gateway's host sockets, with the machine stopped, until what a host
    /// program sent has reached them and become a frame for the machine. The gateway's
    /// clock stands still meanwhile (its service interval is 0, so each call reads the
    /// sockets): only the host's own loopback delivery is left to the wall clock here,
    /// and it is bounded.
    pub fn wait_for_host(&self) {
        let start = Instant::now();
        loop {
            let mut s = self.0.lock().unwrap();
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

    pub fn guest_frames(&self) -> Vec<Vec<u8>> {
        self.0.lock().unwrap().from_guest.clone()
    }
}

/// Whether `frame` is a DNS query: UDP to port 53.
pub fn is_dns_query(frame: &[u8]) -> bool {
    if frame.len() < 34 || frame[12..14] != IP_TYPE.to_be_bytes() || frame[23] != 17 {
        return false;
    }
    let u = 14 + usize::from(frame[14] & 0xf) * 4;
    frame.len() >= u + 8 && frame[u + 2..u + 4] == 53u16.to_be_bytes()
}

/// Whether `frame` is a TCP SYN opening a connection (no ACK).
pub fn is_syn(frame: &[u8]) -> bool {
    if frame.len() < 34 || frame[12..14] != IP_TYPE.to_be_bytes() || frame[23] != 6 {
        return false;
    }
    let t = 14 + usize::from(frame[14] & 0xf) * 4;
    frame.len() >= t + 20 && frame[t + 13] & 0x12 == 0x02
}

impl NetBackend for SteppedGateway {
    /// A SYN from the machine makes the gateway connect a host socket on a thread of its
    /// own, and a DNS query makes it ask its resolver on another; the machine waits
    /// here, stopped, until the answer (SYN-ACK or reset, DNS answer) is ready, a
    /// bounded wait of the wall clock. Otherwise the machine, idle, would see its clock
    /// race ahead (wfi skips to the next tick) and send again, or give up, for nothing.
    fn send(&mut self, frame: &[u8]) -> std::io::Result<()> {
        let mut s = self.0.lock().unwrap();
        s.from_guest.push(frame.to_vec());
        let now = s.now;
        s.gw.send_at(frame, now);
        if is_syn(frame) || is_dns_query(frame) {
            let start = Instant::now();
            loop {
                if let Some(answer) = s.gw.recv_at(now) {
                    s.early.push_back(answer);
                    break;
                }
                assert!(
                    start.elapsed() < Duration::from_secs(30),
                    "the gateway never answered a SYN or a DNS query"
                );
                drop(s);
                std::thread::yield_now();
                s = self.0.lock().unwrap();
            }
        }
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
