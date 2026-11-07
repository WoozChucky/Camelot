//! The Camelot kernel's TCP (`kernel/net/tcp.c`, docs/camelot-net.md "TCP"), driven
//! through `nettest` (kernel/user/nettest.c), a program on the socket system calls:
//!
//! - through the user-mode gateway (`--net user`): a guest client sends a large buffer
//!   to a Rust server on the host (10.0.2.2) while that server sends one back; a guest
//!   echo server answers host clients through a forwarded port, one at a time with the
//!   others waiting in its backlog; and both again through a backend that drops,
//!   duplicates and reorders TCP frames by a fixed pattern;
//! - on a hub, with this test playing the other host segment by segment: resets for
//!   ports nobody listens on, malformed segments counted, the handshake and its MSS
//!   option, RFC 5961's challenge ACKs, a full backlog, out-of-order and duplicate data,
//!   closes from either side and at once, refused and reset connections, retries that
//!   give up and probes of a zero window (their timeouts doubling, timed by the kernel's
//!   `ticks` at every step), a socket closed into a window that stays shut given up,
//!   sequence numbers wrapping past 2^32 both ways, silly window avoidance both ways,
//!   resets for data unread at a close or arriving after it, FIN-WAIT-2's timeout, and
//!   processes killed while they wait in a call;
//! - loopback connections, and many connections in a row, after which every socket,
//!   page and packet buffer is back (counted from the kernel's variables).
//!
//! Like kernel_net.rs, the machines run in this thread by step counts. The gateway runs
//! on a clock that follows the machine's steps; only the host sockets behind it (a
//! server or clients on threads of their own) follow the wall clock, and every wait for
//! them is bounded.

mod common;

use common::*;
use grail::net::{Hub, MacAddr, NetBackend};
use grail::NetConfig;
use std::collections::VecDeque;
use std::io::{Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

// ---- what nettest sends and reports ----

/// Byte `i` of what `nettest connect` sends (kernel/user/nettest.c).
fn client_pattern(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i ^ (i >> 8) ^ (i >> 16)) as u8).collect()
}

/// What the host's programs send: unlike the client's, so nothing can pass for it.
fn host_pattern(len: usize, seed: usize) -> Vec<u8> {
    (0..len)
        .map(|i| ((i.wrapping_mul(7) + seed) ^ (i >> 5)) as u8)
        .collect()
}

/// FNV-1a, as nettest reports what it received.
fn fnv1a(data: &[u8]) -> u32 {
    data.iter().fold(0x811c_9dc5u32, |h, &b| {
        (h ^ u32::from(b)).wrapping_mul(0x0100_0193)
    })
}

/// Compares two byte strings without printing them whole.
fn assert_same(got: &[u8], want: &[u8], what: &str) {
    assert_eq!(got.len(), want.len(), "{what}: length");
    if let Some(i) = got.iter().zip(want).position(|(a, b)| a != b) {
        panic!("{what}: byte {i} is {:#x}, not {:#x}", got[i], want[i]);
    }
}

/// Counters that must be 0 when nothing is lost and nobody misbehaves.
const CLEAN: [&str; 6] = [
    "tcp",
    "tcp-checksum",
    "no-socket",
    "backlog",
    "out-of-order",
    "resets",
];

fn assert_clean(ifconfig: &str) {
    let c = counters(ifconfig);
    for name in CLEAN {
        assert_eq!(c[name], 0, "{name}:\n{ifconfig}");
    }
}

/// Runs `vm` (and `host`) until its output contains `text` `n` times.
fn wait_for(vm: &mut Vm, host: Option<&mut Host>, text: &str, n: usize) {
    run_until(&mut [vm], host, |vms| vms[0].out().matches(text).count() >= n);
}

/// Leaves the shell with status 0 (`exit` alone would return the last command's,
/// and some tests end with a nettest that failed on purpose).
fn leave(vm: &mut Vm) {
    vm.type_line("exit 0");
    assert_eq!(
        vm.m.run(Some(MAX_STEPS), |_| {}),
        grail::RunResult::Exited(0),
        "{}",
        vm.out()
    );
}

/// Starts `line` in the background and runs until it printed `ready` and the shell its
/// next prompt (which may come before or after).
fn background(vm: &mut Vm, host: Option<&mut Host>, line: &str, ready: &str) {
    let prompts = vm.prompts();
    vm.type_line(&format!("{line} &"));
    run_until(&mut [vm], host, |vms| {
        vms[0].out().contains(ready) && vms[0].prompts() > prompts
    });
}

/// Runs the machine idle until TIME-WAIT is over everywhere, and checks that every TCP
/// socket is free again (`tcp_in_use`, kernel/net/tcp.c) and every packet buffer back.
fn assert_all_free(vm: &mut Vm, mut host: Option<&mut Host>) {
    let start = vm.read_var("ticks");
    // TIME-WAIT (and FIN-WAIT-2 for a socket nobody holds) last 600 ticks.
    while vm.read_var("tcp_in_use") != 0 {
        vm.m.run_for(SLICE, &mut |_| {});
        if let Some(h) = host.as_deref_mut() {
            h.service();
        }
        assert!(
            vm.read_var("ticks") - start < 700,
            "{} TCP sockets still in use:\n{}",
            vm.read_var("tcp_in_use"),
            vm.out()
        );
    }
    assert_buffers_back(vm);
}

// ---- the gateway, and a backend that loses frames ----

/// Whether `frame` carries TCP.
fn is_tcp(frame: &[u8]) -> bool {
    frame.len() >= 34 && frame[12..14] == [8, 0] && frame[23] == 6
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fate {
    Pass,
    Drop,
    Duplicate,
    Hold,
}

/// What happens to the `n`th TCP frame (from 1) one way: dropped when n % 7 is 3, sent
/// twice when n % 11 is 5, held back to go after the next one when n % 13 is 8.
fn fate(n: u64) -> Fate {
    if n % 7 == 3 {
        Fate::Drop
    } else if n % 11 == 5 {
        Fate::Duplicate
    } else if n % 13 == 8 {
        Fate::Hold
    } else {
        Fate::Pass
    }
}

/// One direction of [`Lossy`].
#[derive(Default)]
struct Way {
    count: u64,
    held: Option<Vec<u8>>,
    /// Polls the frame held back going up has waited.
    waited: u32,
    dropped: u32,
    duplicated: u32,
    /// Frames held back that did go after a later one (not those let go because none
    /// came).
    reordered: u32,
}

/// Polls a frame held back going up waits for the next frame up before it goes alone:
/// 400 polls are 10.24 ms of the machine's time, far less than any timeout of the
/// gateway's or the kernel's (a tick is 100 ms), and long enough that the kernel,
/// whose segments up are often more than a few ms apart, usually sends another
/// before the held one goes alone.
const HOLD_POLLS: u32 = 400;

/// Toward the gateway, toward the machine, and the frames due to the machine next
/// (duplicates, and frames that were held back).
type Ways = (Way, Way, VecDeque<Vec<u8>>);

/// A backend between the card and the gateway that drops, duplicates and reorders TCP
/// frames both ways, by [`fate`]: a fixed pattern over the frames' count, no randomness
/// (which frames it hits still varies a little between runs, since the host's sockets
/// follow the wall clock). Other frames (ARP, DHCP) pass untouched. A frame held back
/// goes after the next one that way, or alone if none comes: going down, when a poll
/// finds no other; going up, after [`HOLD_POLLS`] polls.
#[derive(Clone)]
struct Lossy {
    inner: SteppedGateway,
    ways: Arc<Mutex<Ways>>,
}

impl Lossy {
    fn new(inner: SteppedGateway) -> Lossy {
        Lossy {
            inner,
            ways: Arc::new(Mutex::new((Way::default(), Way::default(), VecDeque::new()))),
        }
    }

    /// (dropped, duplicated, reordered) toward the gateway and toward the machine.
    fn stats(&self) -> [(u32, u32, u32); 2] {
        let w = self.ways.lock().unwrap();
        [
            (w.0.dropped, w.0.duplicated, w.0.reordered),
            (w.1.dropped, w.1.duplicated, w.1.reordered),
        ]
    }
}

impl NetBackend for Lossy {
    fn send(&mut self, frame: &[u8]) -> std::io::Result<()> {
        if !is_tcp(frame) {
            return self.inner.send(frame);
        }
        let mut guard = self.ways.lock().unwrap();
        let up = &mut guard.0;
        up.count += 1;
        match fate(up.count) {
            Fate::Drop => {
                up.dropped += 1;
                return Ok(());
            }
            Fate::Hold if up.held.is_none() => {
                up.held = Some(frame.to_vec());
                up.waited = 0;
                return Ok(());
            }
            Fate::Duplicate => {
                up.duplicated += 1;
                self.inner.send(frame)?;
            }
            _ => {}
        }
        self.inner.send(frame)?;
        if let Some(held) = up.held.take() {
            up.reordered += 1;
            self.inner.send(&held)?;
        }
        Ok(())
    }

    fn recv(&mut self) -> Option<Vec<u8>> {
        let mut guard = self.ways.lock().unwrap();
        let (up, down, queue) = &mut *guard;
        // A frame held back going up that waited long enough for another goes alone.
        if up.held.is_some() {
            up.waited += 1;
            if up.waited >= HOLD_POLLS {
                self.inner.send(&up.held.take().unwrap()).unwrap();
            }
        }
        if let Some(next) = queue.pop_front() {
            return Some(next);
        }
        loop {
            let Some(frame) = self.inner.recv() else {
                return down.held.take();
            };
            if !is_tcp(&frame) {
                return Some(frame);
            }
            down.count += 1;
            match fate(down.count) {
                Fate::Drop => down.dropped += 1,
                Fate::Hold if down.held.is_none() => down.held = Some(frame),
                Fate::Duplicate => {
                    down.duplicated += 1;
                    queue.push_back(frame.clone());
                    return Some(frame);
                }
                _ => {
                    if let Some(held) = down.held.take() {
                        down.reordered += 1;
                        queue.push_back(held);
                    }
                    return Some(frame);
                }
            }
        }
    }
}

/// A machine on the user-mode gateway (DHCP gives it 10.0.2.15), on the machine's
/// clock, with `hostfwd`'s rules; through a [`Lossy`] backend if `lossy`. Returns the
/// machine, the forwarding rules as bound, and the lossy backend.
fn user_vm(hostfwd: Vec<gateway::HostFwd>, lossy: bool) -> (Vm, Vec<gateway::HostFwd>, Option<Lossy>) {
    let gw = gateway::Gateway::new(gateway::Config {
        hostfwd,
        limits: gateway::Limits {
            service_interval: Duration::ZERO,
            ..gateway::Limits::default()
        },
        ..gateway::Config::default()
    })
    .unwrap();
    let fwd = gw.hostfwd();
    let gw = SteppedGateway::new(gw);
    let lossy = lossy.then(|| Lossy::new(gw.clone()));
    let backend: Box<dyn NetBackend> = match &lossy {
        Some(l) => Box::new(l.clone()),
        None => Box::new(gw),
    };
    let mut vm = Vm::new(
        Vec::new(),
        vec![(MacAddr(gateway::DEFAULT_GUEST_MAC), backend)],
        &[],
    );
    boot(&mut [&mut vm], None);
    assert!(vm.out().contains("net: 10.0.2.15/24"), "{}", vm.out());
    (vm, fwd, lossy)
}

/// A host server on 127.0.0.1 (10.0.2.2 to the guest) for `connections` connections,
/// one after the other: on each it sends `reply` while it reads, then closes; returns
/// what each connection sent it.
fn host_server(connections: usize, reply: Vec<u8>) -> (u16, JoinHandle<Vec<Vec<u8>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || {
        (0..connections)
            .map(|_| {
                let (stream, _) = listener.accept().unwrap();
                stream.set_read_timeout(Some(Duration::from_secs(60))).unwrap();
                let mut w = stream.try_clone().unwrap();
                let reply = reply.clone();
                let writer = thread::spawn(move || {
                    w.write_all(&reply).unwrap();
                    w.shutdown(Shutdown::Write).unwrap();
                });
                let mut got = Vec::new();
                (&stream).read_to_end(&mut got).unwrap();
                writer.join().unwrap();
                got
            })
            .collect()
    });
    (port, server)
}

/// What the host thread `h` returns, the machine running meanwhile: until the thread
/// has everything, the kernel may still have segments to send again (its FIN, say,
/// when the program that sent it is gone).
fn join<T>(vm: &mut Vm, h: JoinHandle<T>) -> T {
    run_until(&mut [vm], None, |_| h.is_finished());
    h.join().unwrap()
}

/// A host client of a forwarded port: sends `data` while it reads, ends its side, and
/// returns what came back once the guest ended its own.
fn host_client(addr: SocketAddr, data: Vec<u8>) -> JoinHandle<Vec<u8>> {
    thread::spawn(move || {
        let stream = TcpStream::connect(addr).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(60))).unwrap();
        let mut w = stream.try_clone().unwrap();
        let writer = thread::spawn(move || {
            w.write_all(&data).unwrap();
            w.shutdown(Shutdown::Write).unwrap();
        });
        let mut got = Vec::new();
        (&stream).read_to_end(&mut got).unwrap();
        writer.join().unwrap();
        got
    })
}

/// `nettest connect`'s line for a connection that went through.
fn connect_line(lport: u16, port: u16, sent: usize, reply: &[u8]) -> String {
    format!(
        "10.0.2.15:{lport} > 10.0.2.2:{port}: sent {sent}, received {}, hash {:08x}\n",
        reply.len(),
        fnv1a(reply)
    )
}

/// The client's transfer of the gateway tests: `bytes` up, `reply` down, checked.
fn client_transfer(vm: &mut Vm, lport: u16, bytes: usize, reply_len: usize) {
    let reply = host_pattern(reply_len, 1);
    let (port, server) = host_server(1, reply.clone());
    let out = command(
        &mut [vm],
        0,
        None,
        &format!("nettest connect 10.0.2.2 {port} {bytes}"),
    );
    assert_eq!(out, connect_line(lport, port, bytes, &reply));
    let got = join(vm, server);
    assert_same(&got[0], &client_pattern(bytes), "what the host server got");
}

/// The echo server's transfers of the gateway tests: one connection of `big` bytes, then
/// two at once, the second waiting in the backlog while the first is served.
fn echo_transfers(vm: &mut Vm, fwd: &gateway::HostFwd, big: usize) {
    let prompts = vm.prompts();
    vm.type_line("nettest serve 7000 3 2");
    wait_for(vm, None, "listening on port 7000\n", 1);
    let addr = SocketAddr::V4(fwd.host);
    let datas = [host_pattern(big, 2), host_pattern(5000, 3), host_pattern(7000, 4)];
    let first = host_client(addr, datas[0].clone());
    assert_same(&join(vm, first), &datas[0], "echo 1");
    let others: Vec<_> = datas[1..].iter().map(|d| host_client(addr, d.clone())).collect();
    for (i, (h, d)) in others.into_iter().zip(&datas[1..]).enumerate() {
        assert_same(&join(vm, h), d, &format!("echo {}", i + 2));
    }
    run_until(&mut [vm], None, |vms| vms[0].prompts() > prompts);
    // One line per connection, from the gateway's ports (in the order the guest
    // accepted them, which for the last two is the order the host connected).
    let out = vm.out();
    let lines: Vec<&str> = out.lines().filter(|l| l.contains(": echoed ")).collect();
    assert_eq!(lines.len(), 3, "{out}");
    let mut got: Vec<String> = lines
        .iter()
        .map(|l| {
            assert!(
                l.starts_with("10.0.2.2:") && l.contains(" > 10.0.2.15:7000: echoed "),
                "{l}"
            );
            l[l.find(": echoed").unwrap()..].to_string()
        })
        .collect();
    let mut want: Vec<String> = datas
        .iter()
        .map(|d| format!(": echoed {}, hash {:08x}", d.len(), fnv1a(d)))
        .collect();
    assert_eq!(got[0], want[0]);
    got.sort();
    want.sort();
    assert_eq!(got, want);
}

#[test]
fn a_guest_client_moves_a_large_buffer_both_ways_through_the_gateway() {
    // Issue #84's first criterion: through the gateway (10.0.2.2 is the host's
    // 127.0.0.1), a Rust server on the host and the guest's client each send the
    // other 192 KiB at the same time, byte for byte. The guest's 4 KiB receive window
    // closes while it is still sending, and opens as it reads.
    let (mut vm, _, _) = user_vm(Vec::new(), false);
    client_transfer(&mut vm, 49152, 192 * 1024, 192 * 1024 + 333);
    // An empty connection: SYN, FIN, and nothing between.
    client_transfer(&mut vm, 49153, 0, 0);
    let ifconfig = command(&mut [&mut vm], 0, None, "ifconfig");
    assert_clean(&ifconfig);
    assert_all_free(&mut vm, None);
    leave(&mut vm);
}

#[test]
fn a_guest_echo_server_answers_host_clients_through_a_forwarded_port() {
    // Issue #84's second criterion: `nettest serve` listens on port 7000, forwarded
    // from a host port; host clients send to it and read back their bytes, one large,
    // then two at once (backlog 2).
    let (mut vm, fwd, _) = user_vm(vec![gateway::HostFwd::tcp(0, 7000)], false);
    echo_transfers(&mut vm, &fwd[0], 160 * 1024);
    let ifconfig = command(&mut [&mut vm], 0, None, "ifconfig");
    assert_clean(&ifconfig);
    assert_all_free(&mut vm, None);
    leave(&mut vm);
}

#[test]
fn transfers_survive_lost_duplicated_and_reordered_frames() {
    // Issue #84's third criterion: both transfers again, through a backend that drops,
    // duplicates and reorders TCP frames both ways by a fixed pattern. The kernel
    // resends on its timer; the gateway does the same toward it; what arrives out of
    // order is dropped and resent; duplicates are acknowledged and ignored.
    let (mut vm, fwd, lossy) = user_vm(vec![gateway::HostFwd::tcp(0, 7000)], true);
    let lossy = lossy.unwrap();
    client_transfer(&mut vm, 49152, 64 * 1024, 48 * 1024);
    echo_transfers(&mut vm, &fwd[0], 48 * 1024);
    // Most frames held back go alone (nothing else came that way in time); the few that
    // went after a later one are the reordering each way.
    for (way, (dropped, duplicated, reordered)) in ["up", "down"].iter().zip(lossy.stats()) {
        assert!(
            dropped > 10 && duplicated > 5 && reordered >= 3,
            "{way}: {dropped} dropped, {duplicated} duplicated, {reordered} reordered"
        );
    }
    let ifconfig = command(&mut [&mut vm], 0, None, "ifconfig");
    let c = counters(&ifconfig);
    assert!(c["retransmits"] > 0 && c["out-of-order"] > 0, "{ifconfig}");
    // Not "resets": the gateway keeps no TIME-WAIT, so when its last ACK is lost, the
    // FIN the kernel sends again finds no connection and is answered with a reset,
    // which ends the kernel's side as well as the ACK would have. Nor "no-socket": a
    // duplicate of the last ACK comes after the connection is gone.
    for name in ["tcp", "tcp-checksum"] {
        assert_eq!(c[name], 0, "{name}:\n{ifconfig}");
    }
    assert_all_free(&mut vm, None);
    leave(&mut vm);
}

#[test]
fn many_connections_leave_no_socket_page_or_buffer_behind() {
    // Issue #84's fourth criterion, the counting part: 40 connections through the
    // gateway and 20 over loopback, more than the 16 sockets there are (connections in
    // TIME-WAIT give up their slot when one is needed), and afterwards every socket,
    // every page of their buffers and every packet buffer is free again.
    let (mut vm, _, _) = user_vm(Vec::new(), false);
    // The program once, so what it costs to start one is the same before and after.
    assert_eq!(
        command(&mut [&mut vm], 0, None, "nettest"),
        "usage: nettest connect ADDR PORT BYTES [TIMES]\n       nettest serve PORT CONNECTIONS [BACKLOG]\n\
     nettest: exit 1\n"
    );
    let pages = vm.pages();
    let reply = host_pattern(1500, 5);
    let (port, server) = host_server(40, reply.clone());
    let out = command(
        &mut [&mut vm],
        0,
        None,
        &format!("nettest connect 10.0.2.2 {port} 3000 40"),
    );
    let want: String = (0..40)
        .map(|i| connect_line(49152 + i, port, 3000, &reply))
        .collect();
    assert_eq!(out, want);
    for got in join(&mut vm, server) {
        assert_same(&got, &client_pattern(3000), "a connection's bytes");
    }
    // Loopback: the server and the client are both this machine. (Small transfers:
    // the client reads only once it has sent everything, so an echo bigger than the
    // buffers on the way, 16 KiB, would stop both.)
    background(&mut vm, None, "nettest serve 6000 20", "listening on port 6000\n");
    let echo = client_pattern(2000);
    let echoed = format!(": echoed 2000, hash {:08x}\n", fnv1a(&echo));
    let prompts = vm.prompts();
    vm.type_line("nettest connect 127.0.0.1 6000 2000 20");
    run_until(&mut [&mut vm], None, |vms| {
        vms[0].prompts() > prompts && vms[0].out().matches(&echoed).count() == 20
    });
    let want: String = (0..20)
        .map(|i| {
            format!(
                "127.0.0.1:{} > 127.0.0.1:6000: sent 2000, received 2000, hash {:08x}\n",
                49192 + i,
                fnv1a(&echo)
            )
        })
        .collect();
    let out = vm.out();
    assert_eq!(out.matches(": echoed 2000, hash ").count(), 20, "{out}");
    for line in want.lines() {
        assert!(out.contains(line), "{line}\n{out}");
    }
    let done = command(&mut [&mut vm], 0, None, "wait");
    assert!(
        done.starts_with('[') && done.ends_with("] nettest: exit 0\n"),
        "{done}"
    );
    let ifconfig = command(&mut [&mut vm], 0, None, "ifconfig");
    assert_clean(&ifconfig);
    assert_all_free(&mut vm, None);
    assert_eq!(vm.pages(), pages, "pages leaked");
    leave(&mut vm);
}

// ---- the other host, segment by segment ----

const FIN: u8 = 0x01;
const SYN: u8 = 0x02;
const RST: u8 = 0x04;
const PSH: u8 = 0x08;
const ACK: u8 = 0x10;

/// A TCP segment, as this test sends it or read it from the machine.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Seg {
    sport: u16,
    dport: u16,
    seq: u32,
    ack: u32,
    flags: u8,
    wnd: u16,
    mss: Option<u16>,
    data: Vec<u8>,
}

/// The sum of TCP's pseudo-header, for [`checksum`].
fn tcp_pseudo(src: [u8; 4], dst: [u8; 4], len: usize) -> u32 {
    let w = |a: [u8; 4]| {
        u32::from(u16::from_be_bytes([a[0], a[1]])) + u32::from(u16::from_be_bytes([a[2], a[3]]))
    };
    w(src) + w(dst) + 6 + len as u32
}

/// The bytes of `s` from `src` to `dst`, with `opts` (whole words) before the MSS
/// option, and a right checksum.
fn tcp_bytes(src: [u8; 4], dst: [u8; 4], s: &Seg, opts: &[u8]) -> Vec<u8> {
    let mut o = opts.to_vec();
    if let Some(mss) = s.mss {
        o.extend_from_slice(&[2, 4]);
        o.extend_from_slice(&mss.to_be_bytes());
    }
    assert_eq!(o.len() % 4, 0);
    let mut t = Vec::new();
    t.extend_from_slice(&s.sport.to_be_bytes());
    t.extend_from_slice(&s.dport.to_be_bytes());
    t.extend_from_slice(&s.seq.to_be_bytes());
    t.extend_from_slice(&s.ack.to_be_bytes());
    t.push((((20 + o.len()) / 4) << 4) as u8);
    t.push(s.flags);
    t.extend_from_slice(&s.wnd.to_be_bytes());
    t.extend_from_slice(&[0, 0, 0, 0]);
    t.extend_from_slice(&o);
    t.extend_from_slice(&s.data);
    let sum = checksum(&t, tcp_pseudo(src, dst, t.len()));
    t[16..18].copy_from_slice(&sum.to_be_bytes());
    t
}

/// The frame of `s` from the host to the machine.
fn frame(s: &Seg) -> Vec<u8> {
    raw_frame(&tcp_bytes(HOST_IP, VM_IP, s, &[]))
}

/// A frame from the host to the machine around the TCP bytes `t`.
fn raw_frame(t: &[u8]) -> Vec<u8> {
    ether(VM_MAC, HOST_MAC, IP_TYPE, &ipv4(HOST_IP, VM_IP, 6, t))
}

/// The machine's segment in `frame`, if it is one, its checksum checked.
fn parse_seg(frame: &[u8]) -> Option<Seg> {
    let p = parse_ipv4(frame)?;
    if p.proto != 6 {
        return None;
    }
    assert_eq!((p.src, p.dst), (VM_IP, HOST_IP));
    let t = &p.payload;
    assert_eq!(
        checksum(t, tcp_pseudo(p.src, p.dst, t.len())),
        0,
        "bad TCP checksum from the kernel"
    );
    let hlen = usize::from(t[12] >> 4) * 4;
    let mut mss = None;
    let mut i = 20;
    while i < hlen {
        match t[i] {
            0 => break,
            1 => i += 1,
            kind => {
                if kind == 2 {
                    mss = Some(u16::from_be_bytes([t[i + 2], t[i + 3]]));
                }
                i += usize::from(t[i + 1]);
            }
        }
    }
    Some(Seg {
        sport: u16::from_be_bytes([t[0], t[1]]),
        dport: u16::from_be_bytes([t[2], t[3]]),
        seq: u32::from_be_bytes(t[4..8].try_into().unwrap()),
        ack: u32::from_be_bytes(t[8..12].try_into().unwrap()),
        flags: t[13],
        wnd: u16::from_be_bytes([t[14], t[15]]),
        mss,
        data: t[hlen..].to_vec(),
    })
}

/// A segment from the host's port `sport` to the machine's `dport`, window 8192.
fn seg(sport: u16, dport: u16, seq: u32, ack: u32, flags: u8, data: &[u8]) -> Seg {
    Seg {
        sport,
        dport,
        seq,
        ack,
        flags,
        wnd: 8192,
        mss: None,
        data: data.to_vec(),
    }
}

/// A machine on a hub with the test as the host at HOST_IP, booted.
fn hub_vm() -> (Vm, Host) {
    let hub = Hub::new();
    // The machine first: its card gets port 1's address, VM_MAC.
    let mut vm = Vm::new(
        vec![NetConfig::hub(&hub)],
        Vec::new(),
        &static_config(VM_IP, MASK),
    );
    let mut host = Host::new(&hub);
    boot(&mut [&mut vm], Some(&mut host));
    (vm, host)
}

/// Runs the machine until the segments it sent since the last call satisfy `done`;
/// returns them.
fn segs_until(vm: &mut Vm, host: &mut Host, what: &str, done: impl Fn(&[Seg]) -> bool) -> Vec<Seg> {
    let mut got = Vec::new();
    let mut steps = 0;
    loop {
        got.extend(host.take().iter().filter_map(|f| parse_seg(f)));
        if done(&got) {
            return got;
        }
        vm.m.run_for(SLICE, &mut |_| {});
        assert_eq!(vm.m.exit_code(), None, "the kernel stopped:\n{}", vm.out());
        host.service();
        steps += SLICE;
        assert!(steps < 100_000_000, "{what}: only {got:?}\n{}", vm.out());
    }
}

/// Sends `s` and returns the `n` segments the machine answers with.
fn send(vm: &mut Vm, host: &mut Host, s: &Seg, n: usize) -> Vec<Seg> {
    host.take();
    host.send(frame(s));
    let got = segs_until(vm, host, &format!("{s:?}"), |g| g.len() >= n);
    assert_eq!(got.len(), n, "{s:?}: {got:?}");
    got
}

/// Sends `s` (a frame of raw TCP bytes, or a segment) and checks the machine says
/// nothing for a while (a dozen slices: under 10 ticks, so no timer of its fires).
fn quiet(vm: &mut Vm, host: &mut Host, what: &str, frame: Vec<u8>) {
    host.take();
    host.send(frame);
    for _ in 0..12 {
        vm.m.run_for(SLICE, &mut |_| {});
        host.service();
    }
    let got: Vec<Seg> = host.take().iter().filter_map(|f| parse_seg(f)).collect();
    assert!(got.is_empty(), "{what}: the kernel answered {got:?}");
}

/// The machine's output once the shell has printed more than `prompts` prompts (the
/// command running has ended).
fn finish(vm: &mut Vm, host: &mut Host, prompts: usize) -> String {
    run_until(&mut [vm], Some(host), |vms| vms[0].prompts() > prompts);
    vm.out()
}

/// A segment the machine sent, with the tick (the kernel's `ticks`) it was sent in.
type Timed = (u32, Seg);

/// Runs the machine until `done` holds for it and the segments it sent since the last
/// call, and returns them, each with its tick. The host takes frames at every step, so
/// the tick is exact, and `answer` replies to each segment as it comes (with the
/// segments it returns).
fn timed_until(
    vm: &mut Vm,
    host: &mut Host,
    what: &str,
    mut answer: impl FnMut(&Seg) -> Vec<Seg>,
    done: impl Fn(&Vm, &[Timed]) -> bool,
) -> Vec<Timed> {
    let ticks = var_addr("ticks");
    let mut got: Vec<Timed> = host
        .take()
        .iter()
        .filter_map(|f| parse_seg(f))
        .map(|s| (vm.read_var("ticks"), s))
        .collect();
    let mut steps = 0;
    while !done(vm, &got) {
        vm.m.run_for(SLICE, &mut |cpu| {
            host.service();
            for f in host.take() {
                if let Some(s) = parse_seg(&f) {
                    for a in answer(&s) {
                        host.send(frame(&a));
                    }
                    got.push((cpu.bus.read_mem(ticks, 4).unwrap() as u32, s));
                }
            }
        });
        assert_eq!(
            vm.m.exit_code(),
            None,
            "the kernel stopped:
{}",
            vm.out()
        );
        steps += SLICE;
        assert!(
            steps < MAX_STEPS,
            "{what}: only {got:?}
{}",
            vm.out()
        );
    }
    got
}

/// The ticks between the segments of `timed` that satisfy `pick`.
fn gaps(timed: &[Timed], pick: impl Fn(&Seg) -> bool) -> Vec<u32> {
    let at: Vec<u32> = timed.iter().filter(|(_, s)| pick(s)).map(|&(t, _)| t).collect();
    at.windows(2).map(|w| w[1] - w[0]).collect()
}

/// The process id in the shell's "[PID]" line for a background job, if `text` has one.
fn pid_in(text: &str) -> Option<u32> {
    text.lines()
        .find_map(|l| l.strip_prefix('[')?.strip_suffix(']')?.parse().ok())
}

/// Starts `line` in the background and returns its process id once the shell gave it
/// and printed its next prompt. Waiting for the prompt too matters: the job can run
/// (and send its first segment) between the "[PID]" line and the prompt, and a
/// `command` typed before that prompt would take it for its own.
fn start(vm: &mut Vm, host: &mut Host, line: &str) -> u32 {
    let (before, prompts) = (vm.out().len(), vm.prompts());
    vm.type_line(&format!("{line} &"));
    run_until(&mut [vm], Some(host), |vms| {
        pid_in(&vms[0].out()[before..]).is_some() && vms[0].prompts() > prompts
    });
    pid_in(&vm.out()[before..]).unwrap()
}

/// Kills `pid`, a nettest waiting in a call, and collects it; returns how many ticks that
/// took, from typing `kill` to the end of `wait`.
fn kill(vm: &mut Vm, host: &mut Host, pid: u32) -> u32 {
    let start = vm.read_var("ticks");
    assert_eq!(command(&mut [vm], 0, Some(host), &format!("kill {pid}")), "");
    assert_eq!(
        command(&mut [vm], 0, Some(host), "wait"),
        format!(
            "[{pid}] nettest: exit -1
"
        )
    );
    vm.read_var("ticks") - start
}

#[test]
fn segments_for_no_socket_get_resets_and_malformed_ones_are_counted() {
    let (mut vm, mut host) = hub_vm();
    // A SYN for a port nobody listens on: a reset acknowledging it (RFC 9293 3.10.7.1).
    let got = send(&mut vm, &mut host, &seg(40000, 81, 1000, 0, SYN, b""), 1);
    assert_eq!(got[0], seg(81, 40000, 0, 1001, RST | ACK, b"").with_wnd(0));
    // With data and a FIN, all of it is acknowledged.
    let got = send(&mut vm, &mut host, &seg(40000, 81, 2000, 0, SYN | FIN, b"abc"), 1);
    assert_eq!(got[0], seg(81, 40000, 0, 2005, RST | ACK, b"").with_wnd(0));
    // A segment with an ACK: a reset at the number it acknowledges.
    let got = send(&mut vm, &mut host, &seg(40000, 81, 5, 777, ACK, b"x"), 1);
    assert_eq!(got[0], seg(81, 40000, 777, 0, RST, b"").with_wnd(0));
    // Never a reset for a reset.
    quiet(
        &mut vm,
        &mut host,
        "a reset",
        frame(&seg(40000, 81, 5, 0, RST, b"")),
    );

    // Malformed: dropped and counted, never answered.
    quiet(&mut vm, &mut host, "10 bytes", raw_frame(&[0; 10]));
    let good = tcp_bytes(HOST_IP, VM_IP, &seg(40000, 81, 1, 0, SYN, b""), &[]);
    let mut bad = good.clone();
    bad[12] = 4 << 4; // a 16-byte header
    quiet(&mut vm, &mut host, "header length 16", raw_frame(&bad));
    let mut bad = good.clone();
    bad[12] = 15 << 4; // a 60-byte header in 20 bytes
    quiet(&mut vm, &mut host, "header length 60", raw_frame(&bad));
    let mut bad = good.clone();
    bad[16] ^= 0x40;
    quiet(&mut vm, &mut host, "checksum", raw_frame(&bad));
    // TCP to the subnet's broadcast address: not for one host, so no reset either.
    let bcast = [10, 0, 9, 255];
    let t = tcp_bytes(HOST_IP, bcast, &seg(40000, 81, 1, 0, SYN, b""), &[]);
    quiet(
        &mut vm,
        &mut host,
        "to broadcast",
        ether(VM_MAC, HOST_MAC, IP_TYPE, &ipv4(HOST_IP, bcast, 6, &t)),
    );

    let ifconfig = command(&mut [&mut vm], 0, Some(&mut host), "ifconfig");
    let mut c = counters(&ifconfig);
    for (name, n) in [("tcp", 3), ("tcp-checksum", 1), ("no-socket", 4), ("filtered", 1)] {
        assert_eq!(c.remove(name), Some(n), "{name}:\n{ifconfig}");
    }
    assert!(c.values().all(|&n| n == 0), "{ifconfig}");
    assert_all_free(&mut vm, Some(&mut host));
    leave(&mut vm);
}

impl Seg {
    fn with_wnd(mut self, wnd: u16) -> Seg {
        self.wnd = wnd;
        self
    }

    fn with_mss(mut self, mss: u16) -> Seg {
        self.mss = Some(mss);
        self
    }
}

#[test]
fn a_listener_handles_the_handshake_resets_syns_and_a_full_backlog() {
    let (mut vm, mut host) = hub_vm();
    background(
        &mut vm,
        Some(&mut host),
        "nettest serve 7000 2 1",
        "listening on port 7000\n",
    );

    // The handshake: the SYN-ACK acknowledges the SYN, offers the 4 KiB buffer and
    // says the MSS is 1460.
    let syn = seg(40001, 7000, 1000, 0, SYN, b"").with_mss(1000);
    let sa = send(&mut vm, &mut host, &syn, 1).remove(0);
    let iss = sa.seq;
    assert_eq!(
        sa,
        seg(7000, 40001, iss, 1001, SYN | ACK, b"")
            .with_wnd(4096)
            .with_mss(1460)
    );
    // Its SYN again (as if the SYN-ACK was lost): the same SYN-ACK.
    assert_eq!(send(&mut vm, &mut host, &syn, 1), vec![sa.clone()]);
    let a = |seq: u32, flags: u8, data: &[u8]| seg(40001, 7000, seq, iss + 1, flags, data);
    quiet(
        &mut vm,
        &mut host,
        "the handshake's ACK",
        frame(&a(1001, ACK, b"")),
    );

    // Data comes back: acknowledged at once, then echoed by the server. Once the
    // server read the 5 bytes, the window's right edge stays where it was (4091 more
    // bytes): 5 freed are less than the MSS that would move it (silly window
    // avoidance).
    let got = segs_until_sent(&mut vm, &mut host, &a(1001, PSH | ACK, b"hello"), |g| {
        g.iter().any(|s| !s.data.is_empty())
    });
    assert!(got.iter().all(|s| s.ack == 1006 && s.wnd == 4091), "{got:?}");
    assert_eq!(
        got.last().unwrap(),
        &seg(7000, 40001, iss + 1, 1006, PSH | ACK, b"hello").with_wnd(4091)
    );
    let challenge = seg(7000, 40001, iss + 6, 1006, ACK, b"").with_wnd(4091);
    let b = |seq: u32, flags: u8, data: &[u8]| seg(40001, 7000, seq, iss + 6, flags, data);
    quiet(&mut vm, &mut host, "the echo's ACK", frame(&b(1006, ACK, b"")));

    // RFC 5961: a reset outside the window is ignored; one inside it but not at the
    // next number expected may be forged, and gets a challenge ACK; so does a SYN.
    quiet(
        &mut vm,
        &mut host,
        "a reset past the window",
        frame(&b(1006 + 100_000, RST, b"")),
    );
    assert_eq!(
        send(&mut vm, &mut host, &b(1010, RST, b""), 1),
        vec![challenge.clone()]
    );
    assert_eq!(
        send(&mut vm, &mut host, &b(77_777, SYN, b""), 1),
        vec![challenge.clone()]
    );
    // A duplicate is acknowledged again; data past a gap is dropped, and the ACK
    // says what is missing.
    assert_eq!(
        send(&mut vm, &mut host, &b(1001, PSH | ACK, b"hello"), 1),
        vec![challenge.clone()]
    );
    assert_eq!(
        send(&mut vm, &mut host, &b(1011, PSH | ACK, b"world"), 1),
        vec![challenge.clone()]
    );
    // So is a FIN past a gap, with no data: dropped and counted like data.
    assert_eq!(
        send(&mut vm, &mut host, &b(1011, FIN | ACK, b""), 1),
        vec![challenge.clone()]
    );

    // The backlog is 1: while the server is busy with this connection, one more is
    // taken (and waits for accept()), the next SYN is dropped.
    let sb = send(&mut vm, &mut host, &seg(40002, 7000, 2000, 0, SYN, b""), 1).remove(0);
    assert_eq!((sb.flags, sb.ack), (SYN | ACK, 2001));
    quiet(
        &mut vm,
        &mut host,
        "a SYN past the backlog",
        frame(&seg(40003, 7000, 3000, 0, SYN, b"")),
    );
    quiet(
        &mut vm,
        &mut host,
        "the second handshake's ACK",
        frame(&seg(40002, 7000, 2001, sb.seq + 1, ACK, b"")),
    );

    // The reset at exactly the next number ends the connection: the server fails
    // with it, and closing its listener resets the connection still waiting.
    let prompts = vm.prompts();
    let got = send(&mut vm, &mut host, &b(1006, RST, b""), 1);
    assert_eq!(got, [seg(7000, 40002, sb.seq + 1, 0, RST, b"").with_wnd(0)]);
    vm.type_line("wait");
    let out = finish(&mut vm, &mut host, prompts);
    assert!(
        out.contains(&format!(
            "10.0.9.1:40001 > 10.0.9.2:7000: echoed 5, hash {:08x}\n\
         nettest: connection reset\n",
            fnv1a(b"hello")
        )) && out.ends_with("] nettest: exit 1\n# "),
        "{out}"
    );
    // The connection is gone: a segment for it gets a reset, and so does a SYN now
    // that nobody listens.
    assert_eq!(
        send(&mut vm, &mut host, &b(1006, PSH | ACK, b"late"), 1),
        [seg(7000, 40001, iss + 6, 0, RST, b"").with_wnd(0)]
    );
    assert_eq!(
        send(&mut vm, &mut host, &seg(40003, 7000, 3000, 0, SYN, b""), 1),
        [seg(7000, 40003, 0, 3001, RST | ACK, b"").with_wnd(0)]
    );

    let ifconfig = command(&mut [&mut vm], 0, Some(&mut host), "ifconfig");
    let mut c = counters(&ifconfig);
    for (name, n) in [
        ("backlog", 1),
        ("resets", 1),
        ("out-of-order", 2),
        ("no-socket", 2),
    ] {
        assert_eq!(c.remove(name), Some(n), "{name}:\n{ifconfig}");
    }
    assert!(c.values().all(|&n| n == 0), "{ifconfig}");
    assert_all_free(&mut vm, Some(&mut host));
    leave(&mut vm);
}

/// Sends `s` and runs until the segments that come satisfy `done`.
fn segs_until_sent(vm: &mut Vm, host: &mut Host, s: &Seg, done: impl Fn(&[Seg]) -> bool) -> Vec<Seg> {
    host.take();
    host.send(frame(s));
    segs_until(vm, host, &format!("{s:?}"), done)
}

/// Starts `nettest connect 10.0.9.1 PORT BYTES` and returns its SYN, checked.
fn connect(vm: &mut Vm, host: &mut Host, port: u16, bytes: usize, lport: u16) -> Seg {
    host.take();
    vm.type_line(&format!("nettest connect 10.0.9.1 {port} {bytes}"));
    let syn = segs_until(vm, host, "the SYN", |g| !g.is_empty()).remove(0);
    assert_eq!(
        syn,
        Seg {
            seq: syn.seq,
            ..seg(lport, port, 0, 0, SYN, b"").with_wnd(4096).with_mss(1460)
        }
    );
    syn
}

/// The bytes of `segs` that carry data, put together by sequence number from `first`
/// (duplicates overlap, gaps would show).
fn assemble(segs: &[Seg], first: u32) -> Vec<u8> {
    let mut out = Vec::new();
    for s in segs.iter().filter(|s| !s.data.is_empty()) {
        let off = s.seq.wrapping_sub(first) as usize;
        if out.len() < off + s.data.len() {
            out.resize(off + s.data.len(), 0);
        }
        out[off..off + s.data.len()].copy_from_slice(&s.data);
    }
    out
}

#[test]
fn connections_close_from_either_side_or_both_and_can_be_refused_or_reset() {
    let (mut vm, mut host) = hub_vm();
    let empty = 0x811c_9dc5u32;

    // The guest closes first: FIN-WAIT-1, FIN-WAIT-2, then TIME-WAIT once the host's
    // own FIN comes.
    let prompts = vm.prompts();
    let syn = connect(&mut vm, &mut host, 80, 100, 49152);
    let iss = syn.seq;
    let sa = seg(80, 49152, 7000, iss + 1, SYN | ACK, b"").with_mss(1460);
    let got = segs_until_sent(&mut vm, &mut host, &sa, |g| g.iter().any(|s| s.flags & FIN != 0));
    assert!(got.iter().all(|s| s.ack == 7001 && s.flags & ACK != 0), "{got:?}");
    assert_eq!(assemble(&got, iss + 1), client_pattern(100));
    let fin = got.last().unwrap();
    assert_eq!(
        (fin.seq + fin.data.len() as u32, fin.flags & FIN),
        (iss + 101, FIN)
    );
    let h = |seq: u32, flags: u8, data: &[u8]| seg(80, 49152, seq, iss + 102, flags, data);
    quiet(
        &mut vm,
        &mut host,
        "the ACK of its FIN",
        frame(&h(7001, ACK, b"")),
    );
    // Acknowledged on arrival, the window's right edge where it was: the 3 bytes and
    // the FIN took 4 of it.
    let last_ack = seg(49152, 80, iss + 102, 7005, ACK, b"").with_wnd(4092);
    assert_eq!(
        send(&mut vm, &mut host, &h(7001, FIN | PSH | ACK, b"bye"), 1),
        vec![last_ack.clone()]
    );
    let out = finish(&mut vm, &mut host, prompts);
    assert!(
        out.ends_with(&format!(
            "10.0.9.2:49152 > 10.0.9.1:80: sent 100, received 3, hash {:08x}\n# ",
            fnv1a(b"bye")
        )),
        "{out}"
    );
    // In TIME-WAIT the host's FIN again (our ACK "lost") is acknowledged again.
    assert_eq!(
        send(&mut vm, &mut host, &h(7001, FIN | PSH | ACK, b"bye"), 1),
        [last_ack.with_wnd(0)]
    );

    // Both close at once: the host's FIN comes before the ACK of the guest's, so the
    // guest goes through CLOSING. (The window's right edge stays put: the FIN took 1.)
    let prompts = vm.prompts();
    let iss = connect(&mut vm, &mut host, 80, 100, 49153).seq;
    let sa = seg(80, 49153, 9000, iss + 1, SYN | ACK, b"").with_mss(1460);
    segs_until_sent(&mut vm, &mut host, &sa, |g| g.iter().any(|s| s.flags & FIN != 0));
    let h = |seq: u32, ack: u32, flags: u8| seg(80, 49153, seq, ack, flags, b"");
    assert_eq!(
        send(&mut vm, &mut host, &h(9001, iss + 101, FIN | ACK), 1),
        [seg(49153, 80, iss + 102, 9002, ACK, b"").with_wnd(4095)]
    );
    quiet(
        &mut vm,
        &mut host,
        "the ACK of its FIN",
        frame(&h(9002, iss + 102, ACK)),
    );
    let out = finish(&mut vm, &mut host, prompts);
    assert!(
        out.ends_with(&format!(
            "10.0.9.2:49153 > 10.0.9.1:80: sent 100, received 0, hash {empty:08x}\n# "
        )),
        "{out}"
    );

    // Refused: a reset answers the SYN.
    let prompts = vm.prompts();
    let iss = connect(&mut vm, &mut host, 80, 100, 49154).seq;
    host.send(frame(&seg(80, 49154, 0, iss + 1, RST | ACK, b"")));
    let out = finish(&mut vm, &mut host, prompts);
    assert!(
        out.ends_with(&format!(
            "10.0.9.2:49154 > 10.0.9.1:80: sent 0, received 0, hash {empty:08x}\n\
         nettest: connection refused\nnettest: exit 1\n# "
        )),
        "{out}"
    );

    // Reset once established: the host sends a reset at the next number expected.
    let prompts = vm.prompts();
    let iss = connect(&mut vm, &mut host, 80, 100, 49155).seq;
    let sa = seg(80, 49155, 11000, iss + 1, SYN | ACK, b"").with_mss(1460);
    segs_until_sent(&mut vm, &mut host, &sa, |g| g.iter().any(|s| s.flags & FIN != 0));
    host.send(frame(&seg(80, 49155, 11001, 0, RST, b"")));
    let out = finish(&mut vm, &mut host, prompts);
    assert!(
        out.ends_with(&format!(
            "10.0.9.2:49155 > 10.0.9.1:80: sent 100, received 0, hash {empty:08x}\n\
         nettest: connection reset\nnettest: exit 1\n# "
        )),
        "{out}"
    );

    let ifconfig = command(&mut [&mut vm], 0, Some(&mut host), "ifconfig");
    let mut c = counters(&ifconfig);
    assert_eq!(c.remove("resets"), Some(2), "{ifconfig}");
    assert!(c.values().all(|&n| n == 0), "{ifconfig}");
    assert_all_free(&mut vm, Some(&mut host));
    leave(&mut vm);
}

#[test]
fn unanswered_segments_are_sent_again_then_given_up() {
    let (mut vm, mut host) = hub_vm();
    // Nobody answers the SYN: it goes 6 times (TCP_SYN_RETRIES more), the timeout
    // doubling from 1 s (10 ticks) each time, then connect() fails.
    let prompts = vm.prompts();
    host.take();
    vm.type_line("nettest connect 10.0.9.1 81 100");
    let got = timed_until(
        &mut vm,
        &mut host,
        "the SYNs",
        |_| vec![],
        |vm, _| vm.prompts() > prompts,
    );
    let syn = got[0].1.clone();
    assert_eq!(
        syn,
        Seg {
            seq: syn.seq,
            ..seg(49152, 81, 0, 0, SYN, b"").with_wnd(4096).with_mss(1460)
        }
    );
    assert!(got.iter().all(|(_, s)| *s == syn), "{got:?}");
    assert_eq!(gaps(&got, |_| true), [10, 20, 40, 80, 160], "{got:?}");
    assert!(
        vm.out().ends_with(
            "10.0.9.2:49152 > 10.0.9.1:81: sent 0, received 0, hash 811c9dc5\n\
         nettest: timed out\nnettest: exit 1\n# "
        ),
        "{}",
        vm.out()
    );

    // The handshake completes (the SYN-ACK at once, so the round trip measured is
    // under a tick and the timeout 2 or 3 ticks), then the host goes silent: the data
    // (and the FIN with it) goes 9 times (TCP_RETRIES more), the timeout doubling each
    // time, then the connection is reset and given up once the last timeout (up to
    // 60 s) is over.
    let prompts = vm.prompts();
    host.take();
    vm.type_line("nettest connect 10.0.9.1 80 100");
    let synack = |s: &Seg| {
        if s.flags & SYN != 0 {
            vec![seg(80, 49153, 5000, s.seq + 1, SYN | ACK, b"").with_mss(1460)]
        } else {
            vec![]
        }
    };
    let timed = timed_until(&mut vm, &mut host, "the data", synack, |vm, _| {
        vm.prompts() > prompts
    });
    let got: Vec<Seg> = timed.iter().map(|(_, s)| s.clone()).collect();
    let iss = got[0].seq;
    let data: Vec<&Seg> = got.iter().filter(|s| !s.data.is_empty()).collect();
    assert_eq!(data.len(), 9, "{got:?}");
    assert!(
        data.iter()
            .all(|s| s.seq == iss + 1 && s.data == client_pattern(100)),
        "{got:?}"
    );
    let gaps = gaps(&timed, |s| !s.data.is_empty() || s.flags & RST != 0);
    let rto = gaps[0];
    assert!(rto == 2 || rto == 3, "{gaps:?}");
    let want: Vec<u32> = (0..9).map(|i| (rto << i).min(600)).collect();
    assert_eq!(gaps, want, "{timed:?}");
    assert_eq!(
        got.last().unwrap(),
        &seg(49153, 80, iss + 102, 0, RST, b"").with_wnd(0),
        "{got:?}"
    );
    assert!(
        vm.out().ends_with(
            "10.0.9.2:49153 > 10.0.9.1:80: sent 100, received 0, hash 811c9dc5\n\
         nettest: timed out\nnettest: exit 1\n# "
        ),
        "{}",
        vm.out()
    );
    let ifconfig = command(&mut [&mut vm], 0, Some(&mut host), "ifconfig");
    let mut c = counters(&ifconfig);
    assert_eq!(c.remove("retransmits"), Some(5 + 8), "{ifconfig}");
    assert!(c.values().all(|&n| n == 0), "{ifconfig}");
    assert_all_free(&mut vm, Some(&mut host));
    leave(&mut vm);
}

#[test]
fn a_zero_window_is_probed_until_it_opens() {
    let (mut vm, mut host) = hub_vm();
    let prompts = vm.prompts();
    let iss = connect(&mut vm, &mut host, 80, 3000, 49152).seq;
    // The host's SYN-ACK offers no window at all (its MSS 1000, after options this
    // stack skips: two NOPs and a 10-byte timestamp).
    let mut sa = seg(80, 49152, 5000, iss + 1, SYN | ACK, b"")
        .with_mss(1000)
        .with_wnd(0);
    let mut opts = vec![1, 1, 8, 10];
    opts.extend_from_slice(&[0; 8]);
    host.take();
    host.send(raw_frame(&tcp_bytes(HOST_IP, VM_IP, &sa, &opts)));
    sa.mss = None;
    // The handshake's ACK, then probes: one byte past the window, again and again,
    // each answered at once by an ACK that takes nothing. They come on the persist
    // timer, its timeout doubling each time: never a retransmission timeout in between
    // (which would send nothing into the closed window, and double the timeout twice
    // per probe).
    let zero = seg(80, 49152, 5001, iss + 1, ACK, b"").with_wnd(0);
    let answer = |s: &Seg| {
        if s.data.is_empty() {
            vec![]
        } else {
            vec![zero.clone()]
        }
    };
    let all = timed_until(&mut vm, &mut host, "the probes", answer, |_, g| {
        g.iter().filter(|(_, s)| !s.data.is_empty()).count() >= 6
    });
    for (_, s) in all.iter().filter(|(_, s)| !s.data.is_empty()) {
        assert_eq!((s.seq, s.data.clone()), (iss + 1, client_pattern(1)), "{all:?}");
    }
    assert_eq!(all[0].1, seg(49152, 80, iss + 1, 5001, ACK, b"").with_wnd(4096));
    // Each probe is answered within the tick it went in or the next, so the gaps are
    // the doubling timeout, or a tick more.
    let gaps = gaps(&all, |s| !s.data.is_empty());
    for w in gaps.windows(2) {
        assert!(w[1].abs_diff(2 * w[0]) <= 2, "{gaps:?}");
    }
    assert!(gaps[4] >= 32, "{gaps:?}");
    // The window opens: everything goes, in segments of the MSS asked for, the FIN
    // with the last.
    let got = segs_until_sent(&mut vm, &mut host, &zero.clone().with_wnd(8192), |g| {
        g.iter().any(|s| s.flags & FIN != 0)
    });
    assert_eq!(assemble(&got, iss + 1), client_pattern(3000));
    assert!(got.iter().all(|s| s.data.len() <= 1000), "{got:?}");
    let end = iss + 3001;
    let h = |seq: u32, flags: u8| seg(80, 49152, seq, end + 1, flags, b"");
    send(&mut vm, &mut host, &h(5001, FIN | ACK), 1);
    let out = finish(&mut vm, &mut host, prompts);
    assert!(
        out.ends_with("10.0.9.2:49152 > 10.0.9.1:80: sent 3000, received 0, hash 811c9dc5\n# "),
        "{out}"
    );
    let ifconfig = command(&mut [&mut vm], 0, Some(&mut host), "ifconfig");
    assert_clean(&ifconfig);
    assert_eq!(counters(&ifconfig)["retransmits"], 0, "{ifconfig}");
    assert_all_free(&mut vm, Some(&mut host));
    leave(&mut vm);
}

#[test]
fn a_socket_closed_into_a_window_that_stays_shut_is_given_up() {
    // A peer that offers no window and answers every probe can't keep a socket nobody
    // holds: its probes count as retries, and after TCP_RETRIES of them the stack
    // resets the connection and frees the slot. (A socket a process holds is probed
    // for as long as the peer answers: a_zero_window_is_probed_until_it_opens.)
    let (mut vm, mut host) = hub_vm();
    let prompts = vm.prompts();
    vm.type_line("nettest serve 7000 1");
    wait_for(&mut vm, Some(&mut host), "listening on port 7000\n", 1);
    let syn = seg(40001, 7000, 1000, 0, SYN, b"").with_mss(1460).with_wnd(0);
    let iss = send(&mut vm, &mut host, &syn, 1)[0].seq;
    // "hello" and the FIN at once, the window shut: the server reads to the end, puts
    // its echo in the send buffer (where it stays), closes and is gone.
    host.take();
    host.send(frame(
        &seg(40001, 7000, 1001, iss + 1, PSH | FIN | ACK, b"hello").with_wnd(0),
    ));
    let zero = seg(40001, 7000, 1007, iss + 1, ACK, b"").with_wnd(0);
    let answer = |s: &Seg| {
        if s.data.is_empty() {
            vec![]
        } else {
            vec![zero.clone()]
        }
    };
    let got = timed_until(&mut vm, &mut host, "the reset", answer, |_, g| {
        g.iter().any(|(_, s)| s.flags & RST != 0)
    });
    assert!(
        vm.out().ends_with(&format!(
            "10.0.9.1:40001 > 10.0.9.2:7000: echoed 5, hash {:08x}\n# ",
            fnv1a(b"hello")
        )),
        "{}",
        vm.out()
    );
    assert!(vm.prompts() > prompts, "{}", vm.out());
    let probes: Vec<&Seg> = got
        .iter()
        .map(|(_, s)| s)
        .filter(|s| !s.data.is_empty())
        .collect();
    assert_eq!(probes.len(), 8, "{got:?}");
    assert!(
        probes.iter().all(|s| s.seq == iss + 1 && s.data == b"h"),
        "{got:?}"
    );
    assert_eq!(
        got.last().unwrap().1,
        seg(7000, 40001, iss + 1, 0, RST, b"").with_wnd(0)
    );
    assert_eq!(vm.read_var("tcp_in_use"), 0);
    // The probes came on the persist timer alone, doubling.
    let gaps = gaps(&got, |s| !s.data.is_empty());
    for w in gaps.windows(2) {
        assert!(w[1].abs_diff(2 * w[0]) <= 2, "{gaps:?}");
    }
    let ifconfig = command(&mut [&mut vm], 0, Some(&mut host), "ifconfig");
    assert_clean(&ifconfig);
    assert_eq!(counters(&ifconfig)["retransmits"], 0, "{ifconfig}");
    assert_all_free(&mut vm, Some(&mut host));
    leave(&mut vm);
}

#[test]
fn sequence_numbers_wrap_around_both_ways() {
    // The guest's initial sequence number is put 4 KiB below 2^32 (tcp_isn_next) and
    // the host's 2 KiB below: 8 KiB echoed each way crosses 0 in both directions,
    // the host's data received across it and the guest's acknowledged across it.
    let guest_isn = 0xffff_f000u32;
    let (mut vm, mut host) = hub_vm();
    vm.write_var("tcp_isn_next", guest_isn);
    let prompts = vm.prompts();
    vm.type_line("nettest serve 7000 1");
    wait_for(&mut vm, Some(&mut host), "listening on port 7000\n", 1);
    let isn = 0xffff_f800u32;
    let sa = send(
        &mut vm,
        &mut host,
        &seg(40001, 7000, isn, 0, SYN, b"").with_mss(1460),
        1,
    )
    .remove(0);
    assert_eq!((sa.seq, sa.ack), (guest_isn, isn + 1));

    // The host: sends what the guest's window lets it, acknowledges what comes back,
    // sends its FIN once all its data is acknowledged, until the guest's FIN.
    let data = host_pattern(8192, 9);
    let end = isn.wrapping_add(1 + data.len() as u32);
    assert!(end < isn, "the host's numbers wrap");
    let (mut snd_una, mut snd_nxt, mut wnd) = (isn + 1, isn + 1, u32::from(sa.wnd));
    let mut rcv_nxt = guest_isn + 1;
    let mut echo = Vec::new();
    let (mut fin_sent, mut guest_fin) = (false, false);
    let mut steps = 0;
    host.take();
    while !(guest_fin && snd_una == end.wrapping_add(1)) {
        while !fin_sent && snd_nxt != end && snd_nxt.wrapping_sub(snd_una) < wnd {
            let off = snd_nxt.wrapping_sub(isn + 1) as usize;
            let room = (wnd - snd_nxt.wrapping_sub(snd_una)) as usize;
            let n = room.min(1000).min(data.len() - off);
            let s = seg(40001, 7000, snd_nxt, rcv_nxt, PSH | ACK, &data[off..off + n]);
            host.send(frame(&s));
            snd_nxt = snd_nxt.wrapping_add(n as u32);
        }
        if snd_una == end && !fin_sent {
            host.send(frame(&seg(40001, 7000, end, rcv_nxt, FIN | ACK, b"")));
            snd_nxt = end.wrapping_add(1);
            fin_sent = true;
        }
        vm.m.run_for(SLICE, &mut |_| {});
        host.service();
        let mut ack = false;
        for s in host.take().iter().filter_map(|f| parse_seg(f)) {
            let acked = s.ack.wrapping_sub(snd_una);
            if s.flags & ACK != 0 && acked <= snd_nxt.wrapping_sub(snd_una) {
                snd_una = s.ack;
                wnd = u32::from(s.wnd);
            }
            if !s.data.is_empty() || s.flags & FIN != 0 {
                if s.seq == rcv_nxt {
                    echo.extend_from_slice(&s.data);
                    rcv_nxt = rcv_nxt.wrapping_add(s.data.len() as u32);
                    if s.flags & FIN != 0 {
                        rcv_nxt = rcv_nxt.wrapping_add(1);
                        guest_fin = true;
                    }
                }
                ack = true;
            }
        }
        if ack {
            host.send(frame(&seg(40001, 7000, snd_nxt, rcv_nxt, ACK, b"")));
        }
        steps += SLICE;
        assert!(
            steps < MAX_STEPS,
            "stuck at {snd_una:#x}/{rcv_nxt:#x}:\n{}",
            vm.out()
        );
    }
    assert_same(&echo, &data, "the echo");
    assert_eq!(rcv_nxt, guest_isn.wrapping_add(8192 + 2));
    let out = finish(&mut vm, &mut host, prompts);
    assert!(
        out.ends_with(&format!(
            "10.0.9.1:40001 > 10.0.9.2:7000: echoed 8192, hash {:08x}\n# ",
            fnv1a(&data)
        )),
        "{out}"
    );
    let ifconfig = command(&mut [&mut vm], 0, Some(&mut host), "ifconfig");
    assert_clean(&ifconfig);
    assert_eq!(counters(&ifconfig)["retransmits"], 0, "{ifconfig}");
    assert_all_free(&mut vm, Some(&mut host));
    leave(&mut vm);
}

#[test]
fn blocked_calls_end_when_the_process_is_killed() {
    // accept(), connect(), send() and recv() wait in the kernel; a process killed
    // while it waits ends at once (-EINTR), long before any TCP timer would have
    // ended the wait, and leaves no socket behind.
    let (mut vm, mut host) = hub_vm();

    // accept(): nobody connects.
    let pid = start(&mut vm, &mut host, "nettest serve 7000 1");
    wait_for(&mut vm, Some(&mut host), "listening on port 7000\n", 1);
    let took = kill(&mut vm, &mut host, pid);
    assert!(took < 20, "{took} ticks");
    assert_eq!(vm.read_var("tcp_in_use"), 0);
    // The listener went with it.
    assert_eq!(
        send(&mut vm, &mut host, &seg(40001, 7000, 1000, 0, SYN, b""), 1),
        [seg(7000, 40001, 0, 1001, RST | ACK, b"").with_wnd(0)]
    );

    // connect(): nobody answers the SYN (the first time it would go again is in 10
    // ticks, the last wait would end in 310).
    host.take();
    let pid = start(&mut vm, &mut host, "nettest connect 10.0.9.1 81 100");
    let iss = segs_until(&mut vm, &mut host, "the SYN", |g| !g.is_empty())[0].seq;
    let took = kill(&mut vm, &mut host, pid);
    assert!(took < 20, "{took} ticks");
    assert_eq!(vm.read_var("tcp_in_use"), 0);
    // The connection went with it: a late SYN-ACK finds none.
    let sa = seg(81, 49152, 5000, iss + 1, SYN | ACK, b"");
    assert_eq!(
        send(&mut vm, &mut host, &sa, 1),
        [seg(49152, 81, iss + 1, 0, RST, b"").with_wnd(0)]
    );

    // send() with the peer's window shut and the send buffer full, recv() with nothing
    // to read: closing_with_data_unread_resets_and_an_orphan_in_fin_wait_2_ends.
    let ifconfig = command(&mut [&mut vm], 0, Some(&mut host), "ifconfig");
    let mut c = counters(&ifconfig);
    assert_eq!(c.remove("no-socket"), Some(2), "{ifconfig}");
    assert!(c.values().all(|&n| n == 0), "{ifconfig}");
    assert_all_free(&mut vm, Some(&mut host));
    leave(&mut vm);
}

#[test]
fn closing_with_data_unread_resets_and_an_orphan_in_fin_wait_2_ends() {
    let (mut vm, mut host) = hub_vm();

    // The server, killed while it waits in send(), with data it has not read: its
    // close resets the connection, so the peer can't think that data was taken. The
    // host keeps its window shut, so the echo stays in the send buffer: 4 KiB fill it,
    // and of the 1000 bytes more, the server reads 512 and waits to send them.
    let pid = start(&mut vm, &mut host, "nettest serve 7000 1");
    wait_for(&mut vm, Some(&mut host), "listening on port 7000\n", 1);
    let syn = seg(40001, 7000, 1000, 0, SYN, b"").with_mss(1460).with_wnd(0);
    let iss = send(&mut vm, &mut host, &syn, 1)[0].seq;
    let data = host_pattern(4096 + 1000, 6);
    let h = |seq: u32, data: &[u8]| seg(40001, 7000, seq, iss + 1, PSH | ACK, data).with_wnd(0);
    host.take();
    for (off, n) in [(0, 1460), (1460, 1460), (2920, 1176)] {
        host.send(frame(&h(1001 + off, &data[off as usize..(off + n) as usize])));
    }
    // Read, the window opens again (the threshold: an MSS).
    segs_until(&mut vm, &mut host, "the window update", |g| {
        g.iter().any(|s| s.ack == 1001 + 4096 && s.wnd >= 1460)
    });
    host.take();
    host.send(frame(&h(1001 + 4096, &data[4096..])));
    segs_until(&mut vm, &mut host, "the ACK", |g| {
        g.iter().any(|s| s.ack == 1001 + 5096)
    });
    for _ in 0..20 {
        vm.m.run_for(SLICE, &mut |_| {});
        host.service();
    }
    host.take();
    let took = kill(&mut vm, &mut host, pid);
    assert!(took < 20, "{took} ticks");
    let got: Vec<Seg> = host.take().iter().filter_map(|f| parse_seg(f)).collect();
    let rst = got
        .iter()
        .find(|s| s.flags & RST != 0)
        .unwrap_or_else(|| panic!("{got:?}"));
    assert_eq!(
        (rst.sport, rst.dport, rst.flags, rst.ack),
        (7000, 40001, RST, 0),
        "{got:?}"
    );
    assert_eq!(vm.read_var("tcp_in_use"), 0);

    // The client, killed in recv() in FIN-WAIT-2 (its FIN acknowledged, the host's
    // not sent): the stack finishes the close on its own, and data that comes now
    // gets a reset, since nobody will ever read it.
    let fin_wait_2 = |vm: &mut Vm, host: &mut Host, lport: u16| {
        host.take();
        let pid = start(vm, host, "nettest connect 10.0.9.1 80 100");
        let iss = segs_until(vm, host, "the SYN", |g| !g.is_empty())[0].seq;
        let sa = seg(80, lport, 7000, iss + 1, SYN | ACK, b"").with_mss(1460);
        segs_until_sent(vm, host, &sa, |g| g.iter().any(|s| s.flags & FIN != 0));
        let ack = seg(80, lport, 7001, iss + 102, ACK, b"");
        quiet(vm, host, "the ACK of its FIN", frame(&ack));
        (pid, iss)
    };
    let (pid, iss) = fin_wait_2(&mut vm, &mut host, 49152);
    let took = kill(&mut vm, &mut host, pid);
    assert!(took < 20, "{took} ticks");
    assert_eq!(vm.read_var("tcp_in_use"), 1);
    assert_eq!(
        send(
            &mut vm,
            &mut host,
            &seg(80, 49152, 7001, iss + 102, PSH | ACK, b"late"),
            1
        ),
        [seg(49152, 80, iss + 102, 0, RST, b"").with_wnd(0)]
    );
    // The reset goes out before the slot is freed: let the kernel finish, and say
    // nothing more.
    run_until(&mut [&mut vm], Some(&mut host), |vms| {
        vms[0].read_var("tcp_in_use") == 0
    });
    assert!(host.take().is_empty());

    // Again, but nothing more comes: the stack waits TCP_FIN_TIMEOUT (600 ticks) from
    // the close for the host's FIN, then frees the slot without a word.
    let (pid, _) = fin_wait_2(&mut vm, &mut host, 49153);
    let before = vm.read_var("ticks");
    let took = kill(&mut vm, &mut host, pid);
    let got = timed_until(
        &mut vm,
        &mut host,
        "the end of FIN-WAIT-2",
        |_| vec![],
        |vm, _| vm.read_var("tcp_in_use") == 0,
    );
    assert!(got.is_empty(), "{got:?}");
    let ended = vm.read_var("ticks") - before;
    assert!(
        (600..=602 + took).contains(&ended),
        "{ended} ticks, the kill {took}"
    );

    let ifconfig = command(&mut [&mut vm], 0, Some(&mut host), "ifconfig");
    let c = counters(&ifconfig);
    for name in [
        "tcp",
        "tcp-checksum",
        "no-socket",
        "backlog",
        "out-of-order",
        "resets",
    ] {
        assert_eq!(c[name], 0, "{name}:\n{ifconfig}");
    }
    assert_all_free(&mut vm, Some(&mut host));
    leave(&mut vm);
}

#[test]
fn silly_windows_are_avoided_both_ways() {
    let (mut vm, mut host) = hub_vm();

    // Sending: once the host offered 4 KiB, a window of 300 bytes is too small for a
    // segment that does not end the data queued (under half the largest window
    // offered): nothing goes until the persist timer forces it.
    let prompts = vm.prompts();
    let iss = connect(&mut vm, &mut host, 80, 10_000, 49152).seq;
    let sa = seg(80, 49152, 5000, iss + 1, SYN | ACK, b"")
        .with_mss(1460)
        .with_wnd(4096);
    segs_until_sent(&mut vm, &mut host, &sa, |g| assemble(g, iss + 1).len() >= 4096);
    let before = vm.read_var("ticks");
    host.take();
    host.send(frame(
        &seg(80, 49152, 5001, iss + 1 + 4096, ACK, b"").with_wnd(300),
    ));
    let got = timed_until(
        &mut vm,
        &mut host,
        "the forced segment",
        |_| vec![],
        |_, g| g.iter().any(|(_, s)| !s.data.is_empty()),
    );
    let (at, s) = got.iter().find(|(_, s)| !s.data.is_empty()).unwrap();
    assert_eq!((s.seq, s.data.len()), (iss + 1 + 4096, 300), "{got:?}");
    assert!(at - before >= 2, "{got:?} from tick {before}");
    host.send(frame(&seg(80, 49152, 5001, 0, RST, b"")));
    let out = finish(&mut vm, &mut host, prompts);
    assert!(
        out.ends_with("nettest: connection reset\nnettest: exit 1\n# "),
        "{out}"
    );

    // Receiving: the host fills the 4 KiB window; as nettest reads, the guest says the
    // window opened on its own (the host sends nothing more, and has nothing to probe
    // with), once it opened by the threshold, an MSS here.
    let prompts = vm.prompts();
    let iss = connect(&mut vm, &mut host, 80, 0, 49153).seq;
    let sa = seg(80, 49153, 7000, iss + 1, SYN | ACK, b"").with_mss(1460);
    segs_until_sent(&mut vm, &mut host, &sa, |g| g.iter().any(|s| s.flags & FIN != 0));
    let data = host_pattern(4096, 7);
    host.take();
    for (off, n) in [(0, 1460), (1460, 1460), (2920, 1176)] {
        let s = seg(
            80,
            49153,
            7001 + off,
            iss + 2,
            PSH | ACK,
            &data[off as usize..(off + n) as usize],
        );
        host.send(frame(&s));
    }
    let end = 7001 + 4096;
    let got = segs_until(&mut vm, &mut host, "the window update", |g| {
        g.iter().any(|s| s.ack == end && s.wnd > 0)
    });
    let first = got.iter().find(|s| s.ack == end).unwrap();
    assert_eq!(first.wnd, 0, "{got:?}");
    let update = got.last().unwrap();
    assert!(update.data.is_empty() && update.wnd >= 1460, "{got:?}");
    send(
        &mut vm,
        &mut host,
        &seg(80, 49153, end, iss + 2, FIN | ACK, b""),
        1,
    );
    let out = finish(&mut vm, &mut host, prompts);
    assert!(
        out.ends_with(&format!(
            "10.0.9.2:49153 > 10.0.9.1:80: sent 0, received 4096, hash {:08x}\n# ",
            fnv1a(&data)
        )),
        "{out}"
    );
    assert_all_free(&mut vm, Some(&mut host));
    leave(&mut vm);
}
