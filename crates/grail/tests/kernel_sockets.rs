//! The Camelot kernel's socket system calls (`kernel/net/socket.c`, docs/camelot-syscalls.md
//! "Sockets") and the network programs on them (kernel/user), on the user-mode gateway:
//!
//! - `socktest` checks, from user space and over loopback, every call and its errors:
//!   bad descriptors, bad pointers, bad addresses, ports in use, timeouts, shutdown,
//!   running out of sockets;
//! - `httpd` serves files from the machine's disk to clients on the host through a
//!   forwarded port (`--net user,hostfwd=tcp:8080-:80`; here the host port is one the
//!   system picks, so parallel tests can't collide), and to `fetch` in the machine
//!   itself over loopback;
//! - `fetch` downloads from an HTTP server on the host (10.0.2.2), by address and by a
//!   name the gateway's DNS server answers (a test resolver, so no network is needed),
//!   and reports a 404 and a name nobody knows; `ping` takes the name too;
//! - `nc` talks to a host server, and listens for a host client, both ways at once.
//!
//! `nc` between two machines on an Avalon switch is in crates/avalon/tests/sockets.rs.
//! As in kernel_tcp.rs, the machine runs in this thread by step counts, with the
//! gateway on a clock that follows them; only the host's programs, on threads of their
//! own, follow the wall clock, and every wait for them is bounded.

mod common;

use common::*;
use grail::net::{MacAddr, NetBackend};
use grail::{DiskConfig, MachineConfig};
use std::io::{Read, Write};
use std::net::{Ipv4Addr, Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

/// The name the test resolver knows: the host, as the guest sees it.
const WEB_NAME: &str = "web.test";

/// A resolver that knows [`WEB_NAME`] only: 10.0.2.2, the gateway's address for the
/// host's loopback.
fn test_resolver() -> gateway::Resolver {
    Arc::new(|name: &str| {
        if name == WEB_NAME {
            Ok(vec![Ipv4Addr::new(10, 0, 2, 2)])
        } else {
            Err(std::io::Error::new(std::io::ErrorKind::NotFound, "unknown"))
        }
    })
}

/// A machine on the user-mode gateway (DHCP gives it 10.0.2.15), with `hostfwd`'s rules,
/// the test resolver, and `disks`. Returns it and the forwarding rules as bound.
fn user_vm(hostfwd: Vec<gateway::HostFwd>, disks: Vec<DiskConfig>) -> (Vm, Vec<gateway::HostFwd>) {
    let (vm, fwd, _) = user_vm_losing_dns(hostfwd, disks);
    (vm, fwd)
}

/// The gateway, but DNS queries from the machine are lost while the budget (the
/// second counter) lasts; the first counts every query the machine sent.
struct LosingDns {
    inner: SteppedGateway,
    dns: Arc<(AtomicUsize, AtomicUsize)>,
}

impl NetBackend for LosingDns {
    fn send(&mut self, frame: &[u8]) -> std::io::Result<()> {
        if is_dns_query(frame) {
            self.dns.0.fetch_add(1, Ordering::SeqCst);
            let left = self.dns.1.load(Ordering::SeqCst);
            if left > 0 {
                self.dns.1.store(left - 1, Ordering::SeqCst);
                return Ok(());
            }
        }
        self.inner.send(frame)
    }

    fn recv(&mut self) -> Option<Vec<u8>> {
        self.inner.recv()
    }
}

/// [`user_vm`], on [`LosingDns`]: also returns its counters (queries sent, queries left
/// to lose; none at first).
fn user_vm_losing_dns(
    hostfwd: Vec<gateway::HostFwd>,
    disks: Vec<DiskConfig>,
) -> (Vm, Vec<gateway::HostFwd>, Arc<(AtomicUsize, AtomicUsize)>) {
    let gw = gateway::Gateway::new(gateway::Config {
        hostfwd,
        limits: gateway::Limits {
            service_interval: Duration::ZERO,
            ..gateway::Limits::default()
        },
        resolver: test_resolver(),
    })
    .unwrap();
    let fwd = gw.hostfwd();
    let dns = Arc::new((AtomicUsize::new(0), AtomicUsize::new(0)));
    let backend: Box<dyn NetBackend> = Box::new(LosingDns {
        inner: SteppedGateway::new(gw),
        dns: dns.clone(),
    });
    let config = MachineConfig {
        ram_size: RAM,
        disks,
        ..MachineConfig::default()
    };
    let mut vm = Vm::with_config(config, vec![(MacAddr(gateway::DEFAULT_GUEST_MAC), backend)], &[]);
    boot(&mut [&mut vm], None);
    assert!(vm.out().contains("net: 10.0.2.15/24"), "{}", vm.out());
    (vm, fwd, dns)
}

/// What the host thread `h` returns, the machine running meanwhile.
fn join<T>(vm: &mut Vm, h: JoinHandle<T>) -> T {
    run_until(&mut [vm], None, |_| h.is_finished());
    h.join().unwrap()
}

/// Leaves the shell with status 0.
fn leave(vm: &mut Vm) {
    vm.type_line("exit 0");
    assert_eq!(
        vm.m.run(Some(MAX_STEPS), |_| {}),
        grail::RunResult::Exited(0),
        "{}",
        vm.out()
    );
}

/// Text of `len` bytes, lines of digits, for files: easy to read in a failure. It ends
/// with a newline, so the shell's next prompt starts a line.
fn text(len: usize, seed: usize) -> Vec<u8> {
    (0..len)
        .map(|i| {
            if i % 64 == 63 || i == len - 1 {
                b'\n'
            } else {
                b'0' + ((i / 64 + seed + i) % 10) as u8
            }
        })
        .collect()
}

#[test]
fn socktest_passes() {
    // Issue #85's third criterion: bad addresses, ports in use, bad descriptors and bad
    // user pointers return errors, as does everything else socktest checks.
    let (mut vm, _) = user_vm(Vec::new(), Vec::new());
    let out = command(&mut [&mut vm], 0, None, "socktest");
    let last = out.lines().last().unwrap_or("");
    assert!(
        last.starts_with("socktest: ") && last.ends_with(" checks, 0 failed"),
        "{out}"
    );
    let checks: u32 = last["socktest: ".len()..last.find(" checks").unwrap()]
        .parse()
        .unwrap();
    assert!(checks > 200, "{out}");
    assert_eq!(out.lines().count(), 1, "{out}");
    leave(&mut vm);
}

// ---- httpd ----

/// The user programs the kernel embeds, as (name, ELF), read from the loaded kernel's
/// `user_programs` table (the userprogs.s `cargo xtask kernel` generates from kernel/user,
/// crates/xtask/src/userprogs.rs): `{ name, elf, size }` each, two pointers and a u32,
/// 24 bytes, the pointers 8 bytes and into the image, which is loaded below where it is
/// linked ([`image_pa`]).
fn embedded_programs(vm: &Vm) -> Vec<(String, Vec<u8>)> {
    let mem = &vm.m.cpu.bus;
    let word = |a: u64| mem.read_mem(a, 4).unwrap();
    let ptr = |a: u64| image_pa(mem.read_mem(a, 8).unwrap());
    let (size_of, ptr_size) = (24, 8);
    let table = var_addr("user_programs");
    let count = vm.read_var("user_program_count");
    (0..u64::from(count))
        .map(|i| {
            let entry = table + size_of * i;
            let (name, elf, size) = (ptr(entry), ptr(entry + ptr_size), word(entry + 2 * ptr_size));
            let name: Vec<u8> = (name..)
                .map(|a| mem.read_mem(a, 1).unwrap() as u8)
                .take_while(|&b| b != 0)
                .collect();
            let elf = (elf..elf + size)
                .map(|a| mem.read_mem(a, 1).unwrap() as u8)
                .collect();
            (String::from_utf8(name).unwrap(), elf)
        })
        .collect()
}

/// A root filesystem with `/dev/console`, the programs `programs` in `/bin` (taken from
/// the kernel, as `cargo xtask fs` would build them) and `files` under `/www` (a name
/// with a `/` in its own directory, made first).
fn root_image(programs: &[&str], files: &[(&str, &[u8])]) -> Vec<u8> {
    // A machine only to read the kernel's programs from: loaded, never run.
    let vm = Vm::new(Vec::new(), Vec::new(), &[]);
    let embedded = embedded_programs(&vm);
    let mut fs = camelot_fs::mkfs::Mkfs::new(&camelot_fs::mkfs::Options::default()).unwrap();
    fs.mkdir("/dev").unwrap();
    fs.add_device("/dev/console", 1, 0).unwrap();
    fs.mkdir("/bin").unwrap();
    for name in programs {
        let (_, elf) = embedded.iter().find(|(n, _)| n == name).unwrap();
        fs.add_file(&format!("/bin/{name}"), elf).unwrap();
    }
    fs.mkdir("/www").unwrap();
    let mut dirs = Vec::new();
    for (name, data) in files {
        if let Some((dir, _)) = name.rsplit_once('/') {
            if !dirs.contains(&dir) {
                fs.mkdir(&format!("/www/{dir}")).unwrap();
                dirs.push(dir);
            }
        }
        fs.add_file(&format!("/www/{name}"), data).unwrap();
    }
    fs.finish()
}

/// An HTTP/1.0 request from the host: sends `request`, reads the whole answer.
fn http_client(addr: SocketAddr, request: String) -> JoinHandle<Vec<u8>> {
    thread::spawn(move || {
        let mut stream = TcpStream::connect(addr).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(60))).unwrap();
        stream.write_all(request.as_bytes()).unwrap();
        let mut answer = Vec::new();
        stream.read_to_end(&mut answer).unwrap();
        answer
    })
}

/// httpd's log in `out`: its lines (`CLIENT:PORT METHOD TARGET STATUS BYTES`), as
/// `CLIENT METHOD TARGET STATUS BYTES`, since the gateway's port varies.
fn httpd_log(out: &str) -> Vec<String> {
    out.lines()
        .filter_map(|l| {
            let at = l.find("10.0.2.2:").or_else(|| l.find("127.0.0.1:"))?;
            let (client, rest) = l[at..].split_once(' ')?;
            let host = client.split(':').next().unwrap();
            Some(format!("{host} {rest}"))
        })
        .collect()
}

/// An answer split into its status line, its headers and its body.
fn parse_answer(answer: &[u8]) -> (String, Vec<String>, Vec<u8>) {
    let end = answer
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .unwrap_or_else(|| panic!("no end of headers: {:?}", String::from_utf8_lossy(answer)));
    let head = String::from_utf8(answer[..end].to_vec()).unwrap();
    let mut lines = head.split("\r\n").map(str::to_string);
    let status = lines.next().unwrap();
    (status, lines.collect(), answer[end + 4..].to_vec())
}

#[test]
fn httpd_serves_files_from_the_disk_through_a_forwarded_port() {
    // Issue #85's first criterion: httpd runs in Camelot with its port 80 forwarded
    // from the host; a client on the host gets a file from the machine's disk.
    let big = text(20_000, 1);
    let index = b"<html><body>Camelot</body></html>\n".to_vec();
    let image = root_image(
        &["sh", "httpd", "fetch", "cat"],
        &[
            ("big.txt", &big),
            ("index.html", &index),
            ("sub/a.txt", b"a\n"),
            ("sub/x y&z.txt", b"xyz\n"),
        ],
    );
    let (mut vm, fwd) = user_vm(
        vec![gateway::HostFwd::tcp(0, 80)],
        vec![DiskConfig::memory(image)],
    );
    let addr = SocketAddr::V4(fwd[0].host);
    let prompts = vm.prompts();
    vm.type_line("httpd -n 12 /www &");
    run_until(&mut [&mut vm], None, |vms| {
        vms[0].out().contains("httpd: serving /www on port 80\n") && vms[0].prompts() > prompts
    });
    let mut ask = |request: &str| parse_answer(&join(&mut vm, http_client(addr, request.to_string())));

    // A file, byte for byte, with its length.
    let (status, headers, body) = ask("GET /big.txt HTTP/1.0\r\nHost: localhost\r\n\r\n");
    assert_eq!(status, "HTTP/1.0 200 OK");
    assert!(
        headers.contains(&"Content-Length: 20000".to_string()),
        "{headers:?}"
    );
    assert!(
        headers.contains(&"Content-Type: text/plain".to_string()),
        "{headers:?}"
    );
    assert!(body == big, "the body differs ({} bytes)", body.len());
    // A directory: its index.html.
    let (status, headers, body) = ask("GET / HTTP/1.0\r\n\r\n");
    assert_eq!(status, "HTTP/1.0 200 OK");
    assert!(
        headers.contains(&"Content-Type: text/html".to_string()),
        "{headers:?}"
    );
    assert_eq!(body, index);
    // A name that isn't there.
    let (status, _, body) = ask("GET /nothing HTTP/1.0\r\n\r\n");
    assert_eq!(status, "HTTP/1.0 404 Not Found");
    assert_eq!(body, b"404 Not Found\n");
    // Nothing above the served directory, spelled out or %-encoded.
    for target in ["/../bin/sh", "/%2e%2e/bin/sh"] {
        let (status, _, body) = ask(&format!("GET {target} HTTP/1.0\r\n\r\n"));
        assert_eq!(status, "HTTP/1.0 403 Forbidden", "{target}");
        assert_eq!(body, b"403 Forbidden\n", "{target}");
    }
    // HEAD: the headers of GET, no body.
    let (status, headers, body) = ask("HEAD /big.txt HTTP/1.0\r\n\r\n");
    assert_eq!(status, "HTTP/1.0 200 OK");
    assert!(
        headers.contains(&"Content-Length: 20000".to_string()),
        "{headers:?}"
    );
    assert!(body.is_empty(), "{} bytes", body.len());
    // Another method; a request that isn't one.
    let (status, _, body) = ask("POST / HTTP/1.0\r\n\r\n");
    assert_eq!(status, "HTTP/1.0 405 Method Not Allowed");
    assert_eq!(body, b"405 Method Not Allowed\n");
    let (status, _, body) = ask("garbage\r\n\r\n");
    assert_eq!(status, "HTTP/1.0 400 Bad Request");
    assert_eq!(body, b"400 Bad Request\n");
    // A directory without index.html: a list of its entries, links to them (%-encoded,
    // the names themselves as HTML), the parent's too.
    let listing = "<html><body><h1>/sub</h1><ul>\n\
               <li><a href=\"/\">..</a></li>\n\
               <li><a href=\"/sub/a.txt\">a.txt</a></li>\n\
               <li><a href=\"/sub/x%20y%26z.txt\">x y&amp;z.txt</a></li>\n\
               </ul></body></html>\n";
    let (status, headers, body) = ask("GET /sub HTTP/1.0\r\n\r\n");
    assert_eq!(status, "HTTP/1.0 200 OK");
    assert!(
        headers.contains(&format!("Content-Length: {}", listing.len())),
        "{headers:?}"
    );
    assert_eq!(String::from_utf8_lossy(&body), listing);
    // The same with a long query, which is not part of the path: the page is built in
    // the heap, as long as it needs, and the links leave the query out.
    let query = "A".repeat(2000);
    let long_listing = listing.replace("<h1>/sub</h1>", "<h1>/sub/</h1>");
    let (status, headers, body) = ask(&format!("GET /sub/?{query} HTTP/1.0\r\n\r\n"));
    assert_eq!(status, "HTTP/1.0 200 OK");
    assert!(
        headers.contains(&format!("Content-Length: {}", long_listing.len())),
        "{headers:?}"
    );
    assert_eq!(String::from_utf8_lossy(&body), long_listing);
    // A path longer than the filesystem's.
    let long_path = format!("/{}", "a".repeat(200));
    let (status, _, body) = ask(&format!("GET {long_path} HTTP/1.0\r\n\r\n"));
    assert_eq!(status, "HTTP/1.0 414 Request-URI Too Long");
    assert_eq!(body, b"414 Request-URI Too Long\n");
    // httpd logs a request once the client is done with the connection, which may be
    // after the client has all it wanted.
    let logged = |n: usize| move |vms: &[&mut Vm]| httpd_log(&vms[0].out()).len() == n;
    run_until(&mut [&mut vm], None, logged(11));

    // fetch in the machine, over loopback, into a file on the disk; httpd logs it too,
    // before or after the shell's next prompt.
    let (before, prompts) = (vm.out().len(), vm.prompts());
    vm.type_line("fetch http://127.0.0.1/index.html > copy.html");
    run_until(&mut [&mut vm], None, |vms| {
        logged(12)(vms) && vms[0].prompts() > prompts
    });
    let out = &vm.out()[before..];
    assert!(!out.contains("fetch:"), "{out}");
    assert_eq!(
        command(&mut [&mut vm], 0, None, "cat copy.html"),
        String::from_utf8(index.clone()).unwrap()
    );

    // httpd's log: one line per request, from the gateway (its port varies) or from
    // loopback; then, after its twelfth, it exits.
    let out = vm.out();
    assert_eq!(
        httpd_log(&out),
        [
            "10.0.2.2 GET /big.txt 200 20000".to_string(),
            format!("10.0.2.2 GET / 200 {}", index.len()),
            "10.0.2.2 GET /nothing 404 14".to_string(),
            "10.0.2.2 GET /../bin/sh 403 14".to_string(),
            "10.0.2.2 GET /%2e%2e/bin/sh 403 14".to_string(),
            "10.0.2.2 HEAD /big.txt 200 20000".to_string(),
            "10.0.2.2 POST / 405 23".to_string(),
            "10.0.2.2 - - 400 16".to_string(),
            format!("10.0.2.2 GET /sub 200 {}", listing.len()),
            format!("10.0.2.2 GET /sub/?{query} 200 {}", long_listing.len()),
            format!("10.0.2.2 GET {long_path} 414 25"),
            format!("127.0.0.1 GET /index.html 200 {}", index.len()),
        ],
        "{out}"
    );
    let done = command(&mut [&mut vm], 0, None, "wait");
    assert!(done.ends_with("] httpd: exit 0\n"), "{done}");
    leave(&mut vm);
}

// ---- fetch, ping and nc against the host ----

/// An HTTP server on the host's loopback (10.0.2.2 to the guest) for `connections`
/// connections: `/a.txt` and `/b.txt` are files, anything else a 404. Returns its port
/// and the request lines it got.
fn host_web_server(connections: usize, a: Vec<u8>, b: Vec<u8>) -> (u16, JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || {
        let mut requests = Vec::new();
        for _ in 0..connections {
            let (mut stream, _) = listener.accept().unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(60))).unwrap();
            let mut request = Vec::new();
            let mut byte = [0u8; 1];
            while !request.ends_with(b"\r\n\r\n") {
                assert_eq!(stream.read(&mut byte).unwrap(), 1, "the request ended early");
                request.push(byte[0]);
            }
            let request = String::from_utf8(request).unwrap();
            let line = request.lines().next().unwrap().to_string();
            let path = line.split(' ').nth(1).unwrap();
            let (status, body) = match path {
                "/a.txt" => ("200 OK", a.clone()),
                "/b.txt" => ("200 OK", b.clone()),
                _ => ("404 Not Found", b"no such file\n".to_vec()),
            };
            write!(
                stream,
                "HTTP/1.0 {status}\r\nContent-Type: text/plain\r\nContent-Length: {}\r\n\r\n",
                body.len()
            )
            .unwrap();
            stream.write_all(&body).unwrap();
            stream.shutdown(Shutdown::Write).unwrap();
            requests.push(format!("{line} | {}", request.lines().nth(1).unwrap()));
        }
        requests
    });
    (port, server)
}

/// `time=1.234 ms` as `time=T ms`.
fn mask_times(s: &str) -> String {
    s.lines()
        .map(|l| match (l.find("time="), l.find(" ms")) {
            (Some(a), Some(b)) if a < b => format!("{}time=T{}\n", &l[..a], &l[b..]),
            _ => format!("{l}\n"),
        })
        .collect()
}

#[test]
fn fetch_downloads_from_a_host_server_by_address_and_by_name() {
    // Issue #85's second criterion, first half: fetch in the machine downloads from a
    // server on the host, through the gateway.
    let (a, b) = (text(9000, 2), text(300, 3));
    let (port, server) = host_web_server(3, a.clone(), b.clone());
    let (mut vm, _) = user_vm(Vec::new(), Vec::new());
    let vms = &mut [&mut vm];
    let by_address = format!("http://10.0.2.2:{port}/a.txt");
    assert_eq!(
        command(vms, 0, None, &format!("fetch {by_address}")).as_bytes(),
        a
    );
    // By name: the gateway's DNS server (10.0.2.3, from DHCP) asks the test resolver.
    let by_name = format!("http://{WEB_NAME}:{port}/b.txt");
    assert_eq!(command(vms, 0, None, &format!("fetch {by_name}")).as_bytes(), b);
    let missing = format!("http://10.0.2.2:{port}/c.txt");
    assert_eq!(
        command(vms, 0, None, &format!("fetch {missing}")),
        format!("fetch: {missing}: 404 Not Found\nfetch: exit 1\n")
    );
    let requests = join(vms[0], server);
    assert_eq!(
        requests,
        [
            "GET /a.txt HTTP/1.0 | Host: 10.0.2.2".to_string(),
            format!("GET /b.txt HTTP/1.0 | Host: {WEB_NAME}"),
            "GET /c.txt HTTP/1.0 | Host: 10.0.2.2".to_string(),
        ]
    );
    // A name nobody knows; a host that refuses the connection.
    assert_eq!(
        command(vms, 0, None, "fetch http://nowhere.test/"),
        "fetch: nowhere.test: unknown host\nfetch: exit 1\n"
    );
    assert_eq!(
        command(vms, 0, None, &format!("fetch {by_address}")),
        format!("fetch: {by_address}: connection refused\nfetch: exit 1\n")
    );
    // ping resolves names the same way.
    assert_eq!(
        mask_times(&command(vms, 0, None, &format!("ping {WEB_NAME} 1"))),
        format!(
            "PING {WEB_NAME} (10.0.2.2): 56 data bytes\n64 bytes from 10.0.2.2: seq=1 ttl=64 time=T ms\n\
         1 sent, 1 received\n"
        )
    );
    leave(&mut vm);
}

#[test]
fn the_resolver_asks_again_when_a_query_is_lost() {
    // resolve() sends a query up to three times, a second apart: two lost, the third
    // is answered; three lost, the name is not found in time.
    let (mut vm, _, dns) = user_vm_losing_dns(Vec::new(), Vec::new());
    let vms = &mut [&mut vm];
    dns.1.store(2, Ordering::SeqCst);
    let start = vms[0].read_var("ticks");
    assert_eq!(
        mask_times(&command(vms, 0, None, &format!("ping {WEB_NAME} 1"))),
        format!(
            "PING {WEB_NAME} (10.0.2.2): 56 data bytes\n64 bytes from 10.0.2.2: seq=1 ttl=64 time=T ms\n\
         1 sent, 1 received\n"
        )
    );
    assert_eq!(dns.0.load(Ordering::SeqCst), 3);
    let took = vms[0].read_var("ticks") - start;
    assert!((20..30).contains(&took), "{took} ticks");
    dns.0.store(0, Ordering::SeqCst);
    dns.1.store(3, Ordering::SeqCst);
    assert_eq!(
        command(vms, 0, None, &format!("ping {WEB_NAME} 1")),
        format!("ping: {WEB_NAME}: timed out\nping: exit 1\n")
    );
    assert_eq!(dns.0.load(Ordering::SeqCst), 3);
    leave(&mut vm);
}

#[test]
fn nc_talks_to_host_programs_both_ways() {
    // A host server: nc sends its input and prints what comes back.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(60))).unwrap();
        let mut got = Vec::new();
        stream.read_to_end(&mut got).unwrap();
        stream.write_all(b"hi there\n").unwrap();
        got
    });
    let (mut vm, fwd) = user_vm(vec![gateway::HostFwd::tcp(0, 7000)], Vec::new());
    let out = command(
        &mut [&mut vm],
        0,
        None,
        &format!("echo hello | nc 10.0.2.2 {port}"),
    );
    assert_eq!(out, "hi there\n");
    assert_eq!(join(&mut vm, server), b"hello\n");

    // nc listening, a host client through the forwarded port.
    let before = vm.out().len();
    let prompts = vm.prompts();
    vm.type_line("echo from guest | nc -lv 7000");
    run_until(&mut [&mut vm], None, |vms| {
        vms[0].out()[before..].contains("listening on port 7000\n")
    });
    let addr = SocketAddr::V4(fwd[0].host);
    let client = thread::spawn(move || {
        let mut stream = TcpStream::connect(addr).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(60))).unwrap();
        stream.write_all(b"from host\n").unwrap();
        stream.shutdown(Shutdown::Write).unwrap();
        let mut got = Vec::new();
        stream.read_to_end(&mut got).unwrap();
        got
    });
    assert_eq!(join(&mut vm, client), b"from guest\n");
    run_until(&mut [&mut vm], None, |vms| vms[0].prompts() > prompts);
    let out = &vm.out()[before..];
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines[0], "echo from guest | nc -lv 7000", "{out}");
    assert_eq!(lines[1], "listening on port 7000", "{out}");
    assert!(lines[2].starts_with("connection from 10.0.2.2:"), "{out}");
    assert_eq!(lines[3..], ["from host", "# "], "{out}");
    leave(&mut vm);
}
