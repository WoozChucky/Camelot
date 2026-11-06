//! `cargo xtask up` end to end: the real Lake, Avalon and Pendragon binaries started
//! as `up` starts them, on ephemeral ports, with a temporary images directory; each
//! one asked whether it answers; the demo's Camelot VM booted to its shell, with a
//! shell of its own reached through the port its switch forwards to its `shd`, and a
//! second VM beside it; everything stopped, and started again on every address with
//! the demo VM on its old disk; and `xtask up` itself ended from outside. Nothing
//! waits a fixed time: every wait is for something to happen, with a generous limit.
//!
//! The binaries are built once, by `up`'s own build step (`cargo build --release -p
//! ...`, a no-op after `cargo test --release` has built the workspace), and so is the
//! root image, into a temporary file.

use avalon_client::Client;
use avalon_proto::{Connection, Message, VmRef, VmState};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{mpsc, Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use xtask::up::{self, Ending, Options, Platform};

/// The longest anything here may take: a Camelot boot on a loaded machine.
const SLOW: Duration = Duration::from_secs(120);

/// A fresh directory under the system's temporary directory, removed when dropped.
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> TempDir {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let n = NEXT.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("xtask-up-{}-{tag}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        TempDir(dir)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// What `up` says, kept for the test to look at (and shown when it fails).
#[derive(Clone, Default)]
struct Output(Arc<Mutex<Vec<String>>>);

impl Output {
    fn push(&self, line: &str) {
        eprintln!("{line}");
        self.0.lock().unwrap().push(line.to_string());
    }

    fn lines(&self) -> Vec<String> {
        self.0.lock().unwrap().clone()
    }
}

/// The root images `up` builds, the server's and the desktop's, built once (with
/// the binaries) for every test.
fn root_images() -> &'static [Vec<u8>] {
    static IMAGES: OnceLock<Vec<Vec<u8>>> = OnceLock::new();
    IMAGES.get_or_init(|| {
        let dir = TempDir::new("build");
        let mut options = Options::new(&xtask::repo_root());
        options.image = dir.0.join("camelot-fs.img");
        options.desktop_image = dir.0.join("camelot-desktop.img");
        let images = up::build(&options, &mut |line| eprintln!("{line}")).unwrap();
        assert_eq!(images.len(), 2);
        assert_eq!(std::fs::read(&options.image).unwrap(), images[0]);
        assert_eq!(std::fs::read(&options.desktop_image).unwrap(), images[1]);
        images
    })
}

/// The server's root image, the base image ([`root_images`]).
fn root_image() -> &'static [u8] {
    &root_images()[0]
}

/// A line of `round-table disk list` for a copy of the root image
/// (`xtask::fs::IMAGE_SIZE` blocks of 1 KiB, two sectors each), in the columns
/// the list has for this test's disks: names up to `pdvm-disk0`.
fn image_line(name: &str, attached: &str) -> String {
    let blocks = xtask::fs::IMAGE_SIZE;
    let size = format!("{} MiB", blocks / 1024);
    format!("{name:<12}{size:<7}{:<9}{attached}\n", blocks * 2)
}

/// A Camelot VM's hardware, as Round Table's `--camelot` and `up`'s demo make it:
/// Camelot's firmware, and its 2 MiB on top of the default 4 MiB of RAM.
fn camelot_profile() -> avalon_proto::HardwareProfile {
    avalon_proto::HardwareProfile {
        ram_size: avalon_client::CAMELOT_RAM,
        bios: avalon_client::camelot_bios(),
        ..avalon_proto::HardwareProfile::default()
    }
}

/// `up`'s options for a test: ephemeral ports, everything under `dir`, nothing to
/// build (the image is [`root_image`]).
fn options(dir: &TempDir) -> Options {
    let mut o = Options::new(&xtask::repo_root());
    o.images = dir.0.join("images");
    o.image = dir.0.join("camelot-fs.img");
    std::fs::write(&o.image, root_image()).unwrap();
    o.desktop_image = dir.0.join("camelot-desktop.img");
    std::fs::write(&o.desktop_image, &root_images()[1]).unwrap();
    o.build = false;
    let any = "127.0.0.1:0".parse().unwrap();
    (o.lake, o.avalon, o.pendragon) = (any, any, any);
    o.shd = "127.0.0.1:0".parse().unwrap();
    o
}

/// The token in Pendragon's URL.
fn token(url: &str) -> &str {
    url.rsplit_once("token=").unwrap().1
}

/// One HTTP request to Pendragon; the status and the body.
fn http(addr: SocketAddr, path: &str, token: &str) -> (u16, String) {
    http_request(addr, "GET", path, token, "")
}

/// One HTTP request to Pendragon with a JSON body (none if empty), as the page sends
/// it; the status and the body.
fn http_request(addr: SocketAddr, method: &str, path: &str, token: &str, body: &str) -> (u16, String) {
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(SLOW)).unwrap();
    let json = if body.is_empty() {
        String::new()
    } else {
        format!(
            "Content-Type: application/json\r\nContent-Length: {}\r\n",
            body.len()
        )
    };
    write!(
        s,
        "{method} {path} HTTP/1.1\r\nHost: {addr}\r\nX-Pendragon-Token: {token}\r\n{json}Connection: close\r\n\r\n{body}"
    )
    .unwrap();
    let mut reply = String::new();
    s.read_to_string(&mut reply).unwrap();
    let status = reply[9..12].parse().unwrap();
    let body = reply.split_once("\r\n\r\n").unwrap().1.to_string();
    (status, body)
}

/// Checks that all three servers answer: Lake and Avalon a handshake and a request,
/// Pendragon its page and its overview (which asks both of them).
fn all_answer(platform: &Platform) {
    let mut lake = Client::connect(&platform.lake.to_string(), "test").unwrap();
    assert!(lake.server().starts_with("lake "), "{}", lake.server());
    let disks = lake.list_disks().unwrap();
    assert!(disks.iter().any(|d| d.name == "os-base"), "{disks:?}");
    let mut avalon = Client::connect(&platform.avalon.to_string(), "test").unwrap();
    assert!(avalon.server().starts_with("avalon "), "{}", avalon.server());
    avalon.list_vms().unwrap();
    let (status, page) = http(platform.pendragon, "/", "");
    assert_eq!(status, 200);
    assert!(page.contains("<script src=\"/app.js\""), "{page}");
    let (status, overview) = http(platform.pendragon, "/api/overview", token(&platform.url));
    assert_eq!(status, 200, "{overview}");
    assert_eq!(overview.matches("\"ok\":true").count(), 2, "{overview}");
}

/// A VM's console: what it printed so far, and what is typed into it.
struct Console {
    conn: Connection<TcpStream>,
    vm: u32,
    output: String,
}

impl Console {
    fn attach(avalon: SocketAddr, vm: u32) -> Console {
        let mut conn = Connection::connect(avalon, "test").unwrap();
        conn.get_ref()
            .set_read_timeout(Some(Duration::from_millis(500)))
            .unwrap();
        let reply = conn
            .request(&Message::ConsoleAttach { vm: VmRef::Id(vm) })
            .unwrap();
        let Message::ConsoleAttached { scrollback, .. } = reply else {
            panic!("{reply:?}")
        };
        Console {
            conn,
            vm,
            output: String::from_utf8_lossy(&scrollback).into_owned(),
        }
    }

    /// Reads the console until its output ends with `end`.
    fn wait_for_end(&mut self, end: &str) {
        let start = Instant::now();
        while !self.output.ends_with(end) {
            assert!(
                start.elapsed() < SLOW,
                "VM {} never printed {end:?}:\n{}",
                self.vm,
                self.output
            );
            match self.conn.recv() {
                Ok(Message::ConsoleOutput { data, .. }) => self.output += &String::from_utf8_lossy(&data),
                Ok(_) => {}
                Err(avalon_proto::Error::Io(e))
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) => {}
                Err(e) => panic!("VM {}: {e}\n{}", self.vm, self.output),
            }
        }
    }

    fn type_line(&mut self, line: &str) {
        self.conn
            .send(&Message::ConsoleInput {
                vm: self.vm,
                data: format!("{line}\n").into_bytes(),
            })
            .unwrap();
    }
}

/// Camelot's shell prompt.
const PROMPT: &str = "$ ";

/// A shell from the demo VM's `shd`, over a TCP connection to the forwarded port.
struct Shell {
    stream: TcpStream,
    output: Vec<u8>,
}

impl Shell {
    /// Connects and reads until the shell's greeting and prompt.
    fn connect(addr: SocketAddr) -> Shell {
        let stream = TcpStream::connect(addr).unwrap();
        stream.set_read_timeout(Some(SLOW)).unwrap();
        let mut shell = Shell {
            stream,
            output: Vec::new(),
        };
        shell.read_until(b"Type help for the list of commands.\n$ ");
        shell
    }

    fn read_until(&mut self, end: &[u8]) {
        let mut buf = [0u8; 512];
        while !self.output.ends_with(end) {
            let n = self.stream.read(&mut buf).unwrap();
            assert!(
                n > 0,
                "the shell ended: {:?}",
                String::from_utf8_lossy(&self.output)
            );
            self.output.extend_from_slice(&buf[..n]);
        }
    }

    /// Runs `line` and checks that it prints `want`.
    fn command(&mut self, line: &str, want: &str) {
        self.output.clear();
        self.stream.write_all(format!("{line}\n").as_bytes()).unwrap();
        self.read_until(PROMPT.as_bytes());
        assert_eq!(String::from_utf8_lossy(&self.output), format!("{want}{PROMPT}"));
    }

    /// `exit`: the shell ends, and so does the connection.
    fn leave(mut self) {
        self.stream.write_all(b"exit\n").unwrap();
        let mut rest = Vec::new();
        self.stream.read_to_end(&mut rest).unwrap();
        assert!(rest.is_empty(), "{rest:?}");
    }
}

/// Never set: no Ctrl-C.
static RUNNING: AtomicBool = AtomicBool::new(false);

/// Checks the Herald command in `line` (its arguments `args`) with Herald's own parser.
fn check_herald(line: &str, args: &str) {
    let args: Vec<String> = args.split_whitespace().map(str::to_string).collect();
    let parsed = herald::parse_args(&args).unwrap_or_else(|e| panic!("{line}: {e}"));
    assert!(parsed.is_some(), "{line}");
}

/// Checks that every Round Table and Herald command in `up`'s summary is one they
/// take (`commands` of them), and that none names an unspecified address.
fn check_summary(summary: &str, want: usize) {
    let mut commands = 0;
    for line in summary.lines() {
        let line = line.trim();
        if let Some(args) = line.strip_prefix("cargo run --release -p round-table -- ") {
            let args: Vec<String> = args.split_whitespace().map(str::to_string).collect();
            round_table::parse_args(&args).unwrap_or_else(|e| panic!("{line}: {e}"));
            commands += 1;
        } else if let Some(args) = line.strip_prefix("cargo run --release -p herald -- ") {
            // The command, without the "(Ctrl-] detaches)" after it.
            check_herald(line, args.split("  ").next().unwrap());
            commands += 1;
        }
    }
    assert_eq!(commands, want, "{summary}");
    assert!(
        !summary.contains("0.0.0.0") && !summary.contains("[::]"),
        "{summary}"
    );
}

#[test]
fn the_platform_comes_up_boots_camelot_and_stops() {
    let dir = TempDir::new("demo");
    let out = Output::default();
    let log = &mut |line: &str| out.push(line);
    let mut options = options(&dir);
    options.demo = true;

    up::prepare(&options, log).unwrap();
    assert_eq!(
        std::fs::read(options.images.join("os-base")).unwrap(),
        root_image()
    );
    let mut platform = Platform::start(&options, &RUNNING, log).unwrap();
    for addr in [platform.lake, platform.avalon, platform.pendragon] {
        assert!(addr.ip().is_loopback() && addr.port() != 0, "{addr}");
    }
    all_answer(&platform);

    // The demo: a NAT switch and a VM, vm001, that boots Camelot from its own copy
    // of the root image, to the shell, with a Camelot VM's hardware: the firmware and
    // 6 MiB of RAM.
    assert_eq!(
        root_image().len(),
        xtask::fs::IMAGE_SIZE as usize * camelot_fs::BSIZE
    );
    let id = platform.demo(log).unwrap();
    let shd = platform.shd.expect("the demo forwards a port to its VM's shd");
    assert!(shd.ip().is_loopback() && shd.port() != 0, "{shd}");
    assert!(
        out.lines().iter().any(|l| l.starts_with("up: demo: started shd")),
        "{:#?}",
        out.lines()
    );
    let mut avalon = Client::connect(&platform.avalon.to_string(), "test").unwrap();
    let vm = avalon.vm_info(&VmRef::Id(id)).unwrap();
    assert_eq!(vm.vm.name, "vm001");
    assert_eq!(vm.vm.profile.ram_size, 6 << 20);
    // A Camelot VM's hardware, but for its disk and its card.
    assert_eq!(vm.vm.profile.devices.len(), 2);
    assert_eq!(
        avalon_proto::HardwareProfile {
            devices: Vec::new(),
            ..vm.vm.profile.clone()
        },
        camelot_profile()
    );
    let switches = avalon.list_switches().unwrap();
    assert_eq!(switches.len(), 1);
    assert_eq!(switches[0].name, "nat");
    assert!(switches[0].nat.is_some());
    let mut one = Console::attach(platform.avalon, id);
    one.wait_for_end(PROMPT);
    assert!(one.output.contains("Camelot v"), "{}", one.output);
    assert!(
        one.output.contains("\nram: 6 MiB at 0x80000000\n"),
        "{}",
        one.output
    );
    assert!(one.output.contains(", the kernel none\n"), "{}", one.output);
    one.type_line("echo up from its own disk");
    one.wait_for_end("up from its own disk\n$ ");
    // A shell of its own through the forwarded port, beside the console's.
    let mut shell = Shell::connect(shd);
    shell.command("echo through shd", "through shd\n");
    one.type_line("echo the console still");
    one.wait_for_end("the console still\n$ ");
    shell.command("ps | grep -c shd", "1\n");
    shell.leave();
    // Something for the next run to find on the disk.
    one.type_line("echo kept > /kept");
    one.wait_for_end(PROMPT);

    // A second Camelot VM beside it, on a copy of its own.
    let mut lake = Client::connect(&platform.lake.to_string(), "test").unwrap();
    let (two_id, disk) = avalon_client::create_camelot_vm(
        &mut avalon,
        &mut lake,
        &platform.lake.to_string(),
        "second",
        avalon_client::BASE_IMAGE,
        camelot_profile(),
    )
    .unwrap();
    assert_eq!(disk, "second-disk0");
    avalon.start_vm(&VmRef::Id(two_id)).unwrap();
    let mut two = Console::attach(platform.avalon, two_id);
    two.wait_for_end(PROMPT);
    two.type_line("ls /");
    two.wait_for_end(PROMPT);
    let vms = avalon.list_vms().unwrap();
    assert_eq!(vms.len(), 2);
    assert!(vms.iter().all(|vm| vm.state == VmState::Running), "{vms:?}");
    let disks = lake.list_disks().unwrap();
    let writer = |name: &str| disks.iter().find(|d| d.name == name).map(|d| d.writer);
    assert_eq!(writer("vm001-disk0"), Some(true));
    assert_eq!(writer("second-disk0"), Some(true));
    assert_eq!(writer("os-base"), Some(false));
    // Nothing is named after the OS.
    let mut names: Vec<&str> = disks.iter().map(|d| d.name.as_str()).collect();
    names.sort();
    assert_eq!(names, ["os-base", "second-disk0", "vm001-disk0"]);
    let mut names: Vec<String> = vms.iter().map(|vm| vm.name.clone()).collect();
    names.sort();
    assert_eq!(names, ["second", "vm001"]);
    drop((avalon, lake, one, two));

    let summary = platform.summary(true);
    assert!(summary.contains(&platform.url), "{summary}");
    assert!(summary.contains(&format!(
        "cargo run --release -p herald -- --server {} vm001",
        platform.avalon
    )));
    assert!(summary.contains(&format!("cargo run --release -p herald -- --tcp {shd}")));
    assert!(
        summary.contains("create --name web --camelot --start\n"),
        "{summary}"
    );
    check_summary(&summary, 4);

    // Ctrl-C: the flag `up`'s handler sets.
    let stop = AtomicBool::new(true);
    assert_eq!(platform.wait(&stop, log), Ending::Stopped);
    let (lake_addr, avalon_addr, pendragon_addr) = (platform.lake, platform.avalon, platform.pendragon);
    let ended = platform.stop(log);
    let names: Vec<&str> = ended.iter().map(|(name, _)| *name).collect();
    assert_eq!(names, ["pendragon", "avalon", "lake"]);
    for addr in [lake_addr, avalon_addr, pendragon_addr] {
        assert!(
            TcpStream::connect_timeout(&addr, Duration::from_secs(2)).is_err(),
            "{addr} still answers"
        );
    }
    // Every server's output came through, prefixed by its name.
    let lines = out.lines();
    for prefix in [
        "lake      | listening on",
        "avalon    | listening on",
        "pendragon | http://",
    ] {
        assert!(
            lines.iter().any(|l| l.starts_with(prefix)),
            "no {prefix:?} in {lines:#?}"
        );
    }

    // `cargo xtask up --demo` again on the same images directory, this time with
    // every server listening on every address (0.0.0.0). `up` reaches them on
    // loopback (connecting to 0.0.0.0 fails on Windows): Pendragon reaches Lake and
    // Avalon, the demo VM's disk is on a Lake Avalon can reach, and the summary's
    // commands work. The demo VM is made again on its disk from the first run, with
    // what it wrote there.
    let mut again = options.clone();
    let every = "0.0.0.0:0".parse().unwrap();
    (again.lake, again.avalon, again.pendragon) = (every, every, every);
    up::prepare(&again, log).unwrap();
    let mut platform = Platform::start(&again, &RUNNING, log).unwrap();
    for addr in [platform.lake, platform.avalon, platform.pendragon] {
        assert!(addr.ip().is_loopback() && addr.port() != 0, "{addr}");
    }
    assert!(platform.url.starts_with("http://127.0.0.1:"), "{}", platform.url);
    all_answer(&platform);
    let id = platform.demo(log).unwrap();
    assert!(
        out.lines()
            .contains(&"up: demo: using disk vm001-disk0 from an earlier run".to_string()),
        "{:#?}",
        out.lines()
    );
    let mut one = Console::attach(platform.avalon, id);
    one.wait_for_end(PROMPT);
    one.type_line("cat /kept");
    one.wait_for_end("kept\n$ ");
    drop(one);
    check_summary(&platform.summary(true), 4);
    check_summary(&platform.summary(false), 3);
    platform.stop(log);
}

/// What Round Table says to `args` on the platform: the disk commands to its Lake,
/// the others to its Avalon (and `create --camelot` to both).
fn round_table(platform: &Platform, args: &[&str]) -> String {
    let (avalon, lake) = (platform.avalon.to_string(), platform.lake.to_string());
    let mut full: Vec<String> = match args.first() {
        Some(&"disk") => vec!["--lake".into(), lake],
        _ if args.contains(&"--camelot") => vec!["--server".into(), avalon, "--lake".into(), lake],
        _ => vec!["--server".into(), avalon],
    };
    full.extend(args.iter().map(|a| a.to_string()));
    let mut out = Vec::new();
    round_table::run(&full, &mut out).unwrap_or_else(|e| panic!("round-table {args:?}: {e}"));
    String::from_utf8(out).unwrap()
}

/// Attaches to VM `vm`'s screen and returns the size it says and the first frame,
/// which is the whole screen.
fn first_frame(avalon: SocketAddr, vm: u32) -> (u32, u32, Vec<u8>) {
    let mut conn = Connection::connect(avalon, "test").unwrap();
    conn.get_ref().set_read_timeout(Some(SLOW)).unwrap();
    let reply = conn
        .request(&Message::ScreenAttach { vm: VmRef::Id(vm) })
        .unwrap();
    let Message::ScreenAttached {
        width,
        height,
        format,
        ..
    } = reply
    else {
        panic!("{reply:?}")
    };
    assert_eq!(format, avalon_proto::PIXEL_FORMAT_XRGB8888);
    loop {
        match conn.recv().unwrap() {
            Message::ScreenFrame {
                x,
                y,
                width: w,
                height: h,
                pixels,
                ..
            } => {
                assert_eq!((x, y, w, h), (0, 0, width, height), "the first frame is whole");
                return (width, height, pixels);
            }
            // The cursor, state changes: not the frame.
            _ => continue,
        }
    }
}

/// Camelot VMs on the platform `up` brings up, made the two ways a person makes one:
/// Round Table's `create --camelot` (Sv48 and Sv39, the default) and Pendragon's
/// form, through its API (here with Sv39 alone). Each boots Camelot from its own copy
/// of Lake's `os-base`, which `up` seeded, on Camelot's firmware with 6 MiB of RAM,
/// its 2 MiB on top of the default 4 MiB (a Camelot VM's defaults), and works as the
/// demo VM does (the test above): its console runs the shell, its screen is served,
/// its card on a switch with a NAT gateway reaches its `shd` through a forwarded
/// port, and what it writes to its disk is in Lake's image once it is stopped.
#[test]
fn camelot_vms_from_round_table_and_pendragon_boot_camelot() {
    let dir = TempDir::new("camelot");
    let out = Output::default();
    let log = &mut |line: &str| out.push(line);
    let options = options(&dir);
    up::prepare(&options, log).unwrap();
    assert!(
        out.lines()
            .iter()
            .any(|l| l.starts_with("up: image os-base in ") && l.ends_with(": created")),
        "{:#?}",
        out.lines()
    );
    let platform = Platform::start(&options, &RUNNING, log).unwrap();
    let rt = |args: &[&str]| round_table(&platform, args);

    // A switch with a NAT gateway for each VM, each forwarding a port of the host to
    // its VM's shd: 10.0.2.15, the first address its DHCP server gives, port 23.
    for switch in ["rt", "pd"] {
        rt(&[
            "switch",
            "create",
            switch,
            "--nat",
            "--hostfwd",
            "tcp:127.0.0.1:0-10.0.2.15:23",
        ]);
    }
    // Round Table's VM, with Sv48 and Sv39 (the default).
    assert_eq!(
        rt(&["create", "--name", "rtvm", "--camelot", "--net", "rt", "--start"]),
        "created disk rtvm-disk0 (a copy of os-base)\ncreated VM 1 (rtvm)\nstarted VM 1 (rtvm)\n"
    );
    // Pendragon's, with Sv39 alone.
    let token = token(&platform.url);
    let (status, body) = http_request(
        platform.pendragon,
        "POST",
        "/api/vms",
        token,
        r#"{"name": "pdvm", "camelot": true, "paging": "sv39", "nets": [{"switch": "pd"}], "start": true}"#,
    );
    assert_eq!(status, 201, "{body}");
    assert!(
        body.contains(r#""id":2"#) && body.contains(r#""disk":"pdvm-disk0""#),
        "{body}"
    );
    let vms = [("rtvm", 1, "rt", "Sv48"), ("pdvm", 2, "pd", "Sv39")];

    // What the clients show of them: each one's paging.
    assert_eq!(
        rt(&["list"]),
        "\
ID  NAME  STATE    RAM    CLOCK       DISKS  NETS
1   rtvm  running  6 MiB  full speed  1      1
2   pdvm  running  6 MiB  full speed  1      1
"
    );
    let info = rt(&["info", "rtvm"]);
    assert!(
        info.contains("\narch     RV64 (Sv48 and Sv39 paging)\nfirmware default\nRAM      6 MiB\n"),
        "{info}"
    );
    let (status, pd) = http(platform.pendragon, "/api/vms/pdvm", token);
    assert_eq!(status, 200, "{pd}");
    assert!(
        pd.contains(r#""paging":"sv39","arch_text":"RV64 (Sv39 paging)","bios":"default""#),
        "{pd}"
    );
    let (status, disks) = http(platform.pendragon, "/api/disks", token);
    assert_eq!(status, 200, "{disks}");
    assert!(disks.contains(r#""camelot_image":"os-base""#), "{disks}");
    let lake_disks = rt(&["disk", "list"]);
    for line in [
        image_line("os-base", "no"),
        image_line("pdvm-disk0", "read-write"),
        image_line("rtvm-disk0", "read-write"),
    ] {
        assert!(lake_disks.contains(&line), "{line:?} in {lake_disks}");
    }

    // The console: Camelot booted to its shell, with the paging the VM has (and
    // 64-bit pointers in the kernel's self-test).
    let mut avalon = Client::connect(&platform.avalon.to_string(), "test").unwrap();
    let switches = avalon.list_switches().unwrap();
    for (name, id, switch, paging) in vms {
        let mut console = Console::attach(platform.avalon, id);
        console.wait_for_end(PROMPT);
        for line in [
            "Camelot v",
            "p=0x0000000080000000 ",
            "\nram: 6 MiB at 0x80000000\n",
            &format!("\npaging: on ({paging})\n"),
            ", the kernel none\n",
        ] {
            assert!(
                console.output.contains(line),
                "{name}: {line:?} in\n{}",
                console.output
            );
        }
        console.type_line(&format!("echo kept by {name} > /kept"));
        console.wait_for_end(PROMPT);
        console.type_line("cat /kept");
        console.wait_for_end(&format!("kept by {name}\n$ "));
        drop(console);

        // The screen: served from the first frame, the whole screen in the mode a
        // machine starts in, black with the display off (Camelot draws nothing).
        let (width, height, pixels) = first_frame(platform.avalon, id);
        assert_eq!((width, height), (640, 480), "{name}");
        assert_eq!(pixels.len(), 640 * 480 * 4);
        assert!(pixels.iter().all(|&b| b == 0), "{name}: the screen is black");

        // The network: shd answers through the port the VM's switch forwards.
        let forward = switches
            .iter()
            .find(|s| s.name == switch)
            .and_then(|s| s.nat.as_ref())
            .map(|nat| nat.hostfwd[0].host)
            .unwrap_or_else(|| panic!("switch {switch} forwards nothing: {switches:?}"));
        let shd = SocketAddr::V4(forward);
        up::start_shd(platform.avalon, id, shd, SLOW).unwrap_or_else(|e| panic!("{name}: {e}"));
        let mut shell = Shell::connect(shd);
        shell.command("echo through shd", "through shd\n");
        shell.command("cat /kept", &format!("kept by {name}\n"));
        shell.leave();
    }

    // The disks: stopped, each VM lets go of its copy, which holds what it wrote.
    assert_eq!(rt(&["stop", "rtvm"]), "stopped VM rtvm\n");
    let (status, body) = http_request(platform.pendragon, "POST", "/api/vms/pdvm/stop", token, "");
    assert_eq!(status, 200, "{body}");
    let start = Instant::now();
    while !rt(&["disk", "list"]).contains(&image_line("pdvm-disk0", "no"))
        || !rt(&["disk", "list"]).contains(&image_line("rtvm-disk0", "no"))
    {
        assert!(start.elapsed() < SLOW, "the disks are still attached");
        std::thread::sleep(Duration::from_millis(20));
    }
    for (name, ..) in vms {
        let image = std::fs::read(options.images.join(format!("{name}-disk0"))).unwrap();
        let fs = camelot_fs::read::Fs::open(&image).unwrap();
        assert_eq!(
            fs.read_file("/kept").unwrap(),
            format!("kept by {name}\n").into_bytes()
        );
    }
    drop(avalon);
    platform.stop(log);
}

/// Whether the summary tells how to reach shd: the `herald --tcp` line.
fn tells_of_shd(summary: &str) -> bool {
    summary.contains("herald -- --tcp")
}

/// The demo when its gateway cannot bind `--shd`'s address (something else listens
/// there): `up` says so, the switch and the VM come up without the forward, there is
/// no shd to reach ([`Platform::shd`] is None) and the summary says nothing of one.
/// And `start_shd` giving up when no shell answers within its time.
#[test]
fn a_taken_shd_address_leaves_the_demo_without_shd() {
    let dir = TempDir::new("shd-taken");
    let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let taken_addr = taken.local_addr().unwrap();
    let out = Output::default();
    let log = &mut |line: &str| out.push(line);
    let mut options = options(&dir);
    options.demo = true;
    options.shd = match taken_addr {
        SocketAddr::V4(v4) => v4,
        SocketAddr::V6(_) => unreachable!(),
    };
    up::prepare(&options, log).unwrap();
    let mut platform = Platform::start(&options, &RUNNING, log).unwrap();
    let id = platform.demo(log).unwrap();
    let lines = out.lines();
    let cannot = format!("up: demo: cannot forward {taken_addr} to the VM's shell server (");
    let said = lines
        .iter()
        .find(|l| l.starts_with(&cannot))
        .unwrap_or_else(|| panic!("{lines:#?}"));
    assert!(
        said.ends_with("); no shd (--shd picks another address)"),
        "{said}"
    );
    assert!(
        lines.contains(&"up: demo: created switch nat with a NAT gateway".to_string()),
        "{lines:#?}"
    );
    assert!(
        !lines
            .iter()
            .any(|l| l.contains("shd, the shell server") || l.contains("start shd")),
        "{lines:#?}"
    );
    assert_eq!(platform.shd, None);
    let mut avalon = Client::connect(&platform.avalon.to_string(), "test").unwrap();
    let switches = avalon.list_switches().unwrap();
    assert_eq!(switches.len(), 1);
    assert_eq!(switches[0].nat.as_ref().map(|n| n.hostfwd.len()), Some(0));
    // The platform carries on: the VM boots to its shell, the servers answer.
    let mut console = Console::attach(platform.avalon, id);
    console.wait_for_end(PROMPT);
    console.type_line("echo no shd");
    console.wait_for_end("no shd\n$ ");
    all_answer(&platform);
    let summary = platform.summary(true);
    assert!(!tells_of_shd(&summary), "{summary}");
    check_summary(&summary, 3);

    // start_shd with nobody's shell at the address it is given (the listener, which
    // accepts and says nothing): it types `shd &`, which starts, and gives up when its
    // time is out.
    let start = Instant::now();
    let e = up::start_shd(platform.avalon, id, taken_addr, Duration::from_secs(3)).unwrap_err();
    assert_eq!(e, format!("no shell answered at {taken_addr}"));
    assert!(start.elapsed() < SLOW, "{:?}", start.elapsed());
    console.wait_for_end("]\n$ ");
    assert!(console.output.contains("shd &\n["), "{}", console.output);
    drop((console, taken));
    platform.stop(log);
}

/// The demo on an older disk, one without `/bin/shd`: the console says so at once
/// (`sh: shd: no such program`), and `up` says shd could not start and how to start
/// it, leaves `herald --tcp` out of its summary, and carries on.
#[test]
fn a_disk_without_shd_leaves_the_demo_without_shd() {
    let dir = TempDir::new("no-shd");
    let out = Output::default();
    let log = &mut |line: &str| out.push(line);
    let mut options = options(&dir);
    options.demo = true;
    up::prepare(&options, log).unwrap();
    let mut platform = Platform::start(&options, &RUNNING, log).unwrap();
    // The demo VM's disk from an earlier run, where /bin/shd was removed: made by a
    // VM of that name, which is then destroyed (the VMs end with Avalon; the disk
    // stays).
    let mut avalon = Client::connect(&platform.avalon.to_string(), "test").unwrap();
    let mut lake = Client::connect(&platform.lake.to_string(), "test").unwrap();
    let (old, disk) = avalon_client::create_camelot_vm(
        &mut avalon,
        &mut lake,
        &platform.lake.to_string(),
        "vm001",
        avalon_client::BASE_IMAGE,
        camelot_profile(),
    )
    .unwrap();
    assert_eq!(disk, "vm001-disk0");
    avalon.start_vm(&VmRef::Id(old)).unwrap();
    let mut console = Console::attach(platform.avalon, old);
    console.wait_for_end(PROMPT);
    console.type_line("rm /bin/shd");
    console.wait_for_end("rm /bin/shd\n$ ");
    drop(console);
    avalon.stop_vm(&VmRef::Id(old)).unwrap();
    avalon.destroy_vm(&VmRef::Id(old)).unwrap();

    let start = Instant::now();
    let id = platform.demo(log).unwrap();
    // The console told at once: no waiting for SHD_TIMEOUT.
    assert!(start.elapsed() < up::SHD_TIMEOUT, "{:?}", start.elapsed());
    let lines = out.lines();
    for line in [
        "up: demo: using disk vm001-disk0 from an earlier run".to_string(),
        format!(
            "up: demo: could not start shd in VM {id} (sh: shd: no such program); type shd & on its \
             console"
        ),
    ] {
        assert!(lines.contains(&line), "no {line:?} in {lines:#?}");
    }
    // The forward is there, for a shd started by hand.
    assert!(platform.shd.is_some());
    let summary = platform.summary(true);
    assert!(!tells_of_shd(&summary), "{summary}");
    check_summary(&summary, 3);
    // The platform carries on.
    let mut console = Console::attach(platform.avalon, id);
    console.type_line("echo still up");
    console.wait_for_end("still up\n$ ");
    all_answer(&platform);
    drop((console, avalon, lake));
    platform.stop(log);
}

/// `up::run`, the command itself: it comes up, prints its summary, and stops when the
/// flag is set (here by the log, once it has seen the summary and every server
/// answered).
#[test]
fn run_comes_up_and_stops_on_ctrl_c() {
    let dir = TempDir::new("run");
    let options = options(&dir);
    // An image of the user's own beside the base image is left alone.
    std::fs::create_dir_all(&options.images).unwrap();
    std::fs::write(options.images.join("mine"), [7; 1024]).unwrap();
    let stop = AtomicBool::new(false);
    let out = Output::default();
    let mut log = |line: &str| {
        out.push(line);
        if line.contains("The Avalon platform is up.") {
            let addr = |label: &str| -> SocketAddr {
                let at = line.find(label).unwrap() + label.len();
                line[at..]
                    .split([',', '\n'])
                    .next()
                    .unwrap()
                    .trim()
                    .parse()
                    .unwrap()
            };
            let (lake, avalon) = (addr("\n  Lake "), addr("\n  Avalon "));
            Client::connect(&lake.to_string(), "test")
                .unwrap()
                .list_disks()
                .unwrap();
            Client::connect(&avalon.to_string(), "test")
                .unwrap()
                .list_vms()
                .unwrap();
            stop.store(true, Ordering::SeqCst);
        }
    };
    up::run(&options, &stop, &mut log).unwrap();
    assert!(stop.load(Ordering::SeqCst));
    let lines = out.lines();
    assert_eq!(lines.last().map(String::as_str), Some("up: stopped"));
    for server in ["pendragon", "avalon", "lake"] {
        assert!(lines.contains(&format!("up: stopping {server}")), "{lines:#?}");
    }
    assert_eq!(std::fs::read(options.images.join("mine")).unwrap(), [7; 1024]);
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("up: image os-base in ") && l.ends_with(": created")),
        "{lines:#?}"
    );
}

/// When one server ends by itself, `wait` says which, and `stop` ends the others.
#[test]
fn one_server_ending_stops_the_others() {
    let dir = TempDir::new("crash");
    let out = Output::default();
    let log = &mut |line: &str| out.push(line);
    let options = options(&dir);
    up::prepare(&options, log).unwrap();
    let mut platform = Platform::start(&options, &RUNNING, log).unwrap();
    platform.kill("avalon");
    let never = AtomicBool::new(false);
    assert!(matches!(platform.wait(&never, log), Ending::Exited("avalon", _)));
    let ended = platform.stop(log);
    assert_eq!(ended.len(), 3);
    let lines = out.lines();
    let stopping: Vec<&str> = lines
        .iter()
        .filter_map(|l| l.strip_prefix("up: stopping "))
        .collect();
    assert_eq!(stopping, ["pendragon", "lake"]);
}

/// A server that cannot start (its port is taken) fails the start, saying why, and
/// leaves nothing running.
#[test]
fn a_taken_port_fails_the_start() {
    let dir = TempDir::new("taken");
    let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let mut options = options(&dir);
    options.avalon = taken.local_addr().unwrap();
    let out = Output::default();
    let log = &mut |line: &str| out.push(line);
    up::prepare(&options, log).unwrap();
    let e = Platform::start(&options, &RUNNING, log).err().unwrap();
    assert!(e.starts_with("avalon ended before it was ready"), "{e}");
    // Lake, which had started, was stopped: its port is free again.
    let lake = out
        .lines()
        .iter()
        .find_map(|l| l.strip_prefix("lake      | listening on ").map(str::to_string))
        .unwrap();
    let start = Instant::now();
    while TcpStream::connect_timeout(&lake.parse().unwrap(), Duration::from_secs(1)).is_ok() {
        assert!(start.elapsed() < SLOW, "lake still answers on {lake}");
    }
}

/// Ctrl-C while the servers start: `start` stops at once, and what it started ends.
#[test]
fn ctrl_c_while_starting_stops_what_started() {
    let dir = TempDir::new("interrupted");
    let options = options(&dir);
    let out = Output::default();
    let log = &mut |line: &str| out.push(line);
    up::prepare(&options, log).unwrap();
    let stop = AtomicBool::new(true);
    let e = Platform::start(&options, &stop, log).err().unwrap();
    assert_eq!(e, up::INTERRUPTED);
    // `run` says so, and succeeds (Ctrl-C is how `up` is meant to end).
    let out = Output::default();
    up::run(&options, &stop, &mut |line: &str| out.push(line)).unwrap();
    let lines = out.lines();
    assert_eq!(
        lines.last().map(String::as_str),
        Some("up: interrupted while starting; stopped"),
        "{lines:#?}"
    );
    assert!(!lines.iter().any(|l| l.contains("The Avalon platform is up.")));
}

/// `xtask up` ended from outside once it is up: on Windows killed outright, and the
/// job object ends the servers with it; on Unix with SIGTERM, which it handles by
/// stopping them in order. Either way no server is left holding its port.
#[test]
fn ending_xtask_up_ends_the_servers() {
    let dir = TempDir::new("process");
    let options = options(&dir);
    // `--no-build` seeds target/camelot-fs.img when it is there; without it, the image
    // already in the directory does.
    up::seed(&options.images, "os-base", root_image()).unwrap();
    let any = "127.0.0.1:0";
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_xtask"))
        .args([
            "up",
            "--no-build",
            "--lake",
            any,
            "--avalon",
            any,
            "--pendragon",
            any,
        ])
        .arg("--images")
        .arg(&options.images)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let stdout = BufReader::new(child.stdout.take().unwrap());
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in stdout.lines().map_while(Result::ok) {
            eprintln!("{line}");
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    // The summary's addresses: "  Avalon     ADDR", "  Lake       ADDR, images in ...",
    // "  Pendragon  http://ADDR/?token=...".
    let mut servers: Vec<SocketAddr> = Vec::new();
    while servers.len() < 3 {
        let line = rx
            .recv_timeout(SLOW)
            .expect("xtask up says where its servers are");
        let words: Vec<&str> = line.split_whitespace().collect();
        match words.as_slice() {
            ["Avalon" | "Lake", addr, ..] => servers.push(addr.trim_end_matches(',').parse().unwrap()),
            ["Pendragon", url] => {
                let addr = url.strip_prefix("http://").unwrap().split('/').next().unwrap();
                servers.push(addr.parse().unwrap());
            }
            _ => {}
        }
    }
    for addr in &servers {
        TcpStream::connect_timeout(addr, Duration::from_secs(5)).unwrap();
    }
    #[cfg(windows)]
    child.kill().unwrap();
    #[cfg(unix)]
    assert!(std::process::Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .status()
        .unwrap()
        .success());
    let status = child.wait().unwrap();
    // On Unix, SIGTERM is handled as Ctrl-C: an orderly stop, and success.
    #[cfg(unix)]
    assert!(status.success(), "{status}");
    #[cfg(not(unix))]
    let _ = status;
    let start = Instant::now();
    for addr in &servers {
        while TcpStream::connect_timeout(addr, Duration::from_secs(1)).is_ok() {
            assert!(
                start.elapsed() < SLOW,
                "{addr} still answers after xtask up ended"
            );
        }
    }
}

/// `--demo` after a build changed the image: the demo VM's disk from an earlier run is
/// a copy of the old one, and `up` says so (and how to get a fresh copy).
#[test]
fn a_new_image_says_the_demo_disk_is_older() {
    let dir = TempDir::new("stale");
    let mut options = options(&dir);
    options.demo = true;
    let out = Output::default();
    let log = &mut |line: &str| out.push(line);
    let note = |out: &Output| {
        out.lines()
            .iter()
            .any(|l| l.starts_with("up: note: the demo VM's disk"))
    };
    // The image changed, but there is no demo disk yet: nothing to say.
    up::seed(&options.images, "os-base", &[0; 512]).unwrap();
    up::prepare(&options, log).unwrap();
    assert!(!note(&out));
    // Unchanged image, old demo disk: nothing to say either.
    std::fs::write(options.images.join("vm001-disk0"), [0; 512]).unwrap();
    up::prepare(&options, log).unwrap();
    assert!(!note(&out));
    // A new image and the old demo disk.
    up::seed(&options.images, "os-base", &[0; 512]).unwrap();
    up::prepare(&options, log).unwrap();
    assert!(note(&out), "{:#?}", out.lines());
    // Said after the image's line.
    let lines = out.lines();
    let at = lines
        .iter()
        .rposition(|l| l.starts_with("up: note: the demo VM's disk"))
        .unwrap();
    assert!(
        lines[at - 1].starts_with("up: image os-base in ")
            && lines[at - 1].ends_with(": replaced with the new build"),
        "{lines:#?}"
    );
    assert_eq!(
        lines[at],
        format!(
            "up: note: the demo VM's disk vm001-disk0 is a copy of the image os-base from before this \
             build, and the demo boots it as it is; to boot the new build, delete {} while up is not \
             running (what the VM wrote there goes with it)",
            options.images.join("vm001-disk0").display()
        )
    );
}

/// With `--no-build`, `up` needs the binaries and an image to put in Lake's directory.
#[test]
fn no_build_needs_what_a_build_makes() {
    let dir = TempDir::new("nobuild");
    let mut options = options(&dir);
    let log = &mut |line: &str| eprintln!("{line}");
    options.image = dir.0.join("missing.img");
    let e = up::prepare(&options, log).unwrap_err();
    assert_eq!(
        e,
        format!(
            "there is no {} to put in {} as the image os-base: run cargo xtask up without --no-build",
            options.image.display(),
            options.images.display()
        )
    );
    // A base image already in the directory is enough.
    up::seed(&options.images, "os-base", &[0; 512]).unwrap();
    up::prepare(&options, log).unwrap();
    options.bin_dir = dir.0.join("nowhere");
    let e = up::prepare(&options, log).unwrap_err();
    assert!(
        e.contains("is missing: run cargo xtask up without --no-build"),
        "{e}"
    );
}

#[test]
fn the_command_line() {
    let xtask = env!("CARGO_BIN_EXE_xtask");
    let run = |args: &[&str]| std::process::Command::new(xtask).args(args).output().unwrap();
    let help = run(&["up", "--help"]);
    assert!(help.status.success());
    assert_eq!(String::from_utf8(help.stdout).unwrap().trim_end(), up::USAGE);
    let bad = run(&["up", "--bogus"]);
    assert_eq!(bad.status.code(), Some(2));
    assert!(String::from_utf8(bad.stderr)
        .unwrap()
        .starts_with("xtask: unknown option --bogus"));
    let tasks = run(&["--help"]);
    assert!(String::from_utf8(tasks.stdout).unwrap().contains("\n  up "));
}

/// The README's quick start: each command in it is one this repository's tools take.
#[test]
fn the_readme_quick_start_is_right() {
    let readme = std::fs::read_to_string(xtask::repo_root().join("README.md"))
        .unwrap()
        .replace("\r\n", "\n");
    let section = &readme[readme.find("## Quick start").expect("a Quick start section")..];
    let section = &section[..section[3..].find("\n## ").map_or(section.len(), |i| i + 3)];
    let mut checked = 0;
    let mut in_block = false;
    for line in section.lines() {
        if line.starts_with("```") {
            in_block = !in_block;
            continue;
        }
        if !in_block {
            continue;
        }
        let words: Vec<String> = line.split_whitespace().map(str::to_string).collect();
        let strs: Vec<&str> = words.iter().map(String::as_str).collect();
        match strs.as_slice() {
            ["cargo", "xtask", "up", args @ ..] => {
                let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
                assert!(up::parse_args(&args, Path::new(".")).unwrap().is_some(), "{line}");
            }
            ["cargo", "run", "--release", "-p", "round-table", "--", args @ ..] => {
                let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
                round_table::parse_args(&args).unwrap_or_else(|e| panic!("{line}: {e}"));
            }
            ["cargo", "run", "--release", "-p", "herald", "--", args @ ..] => {
                check_herald(line, &args.join(" "))
            }
            _ => continue,
        }
        checked += 1;
    }
    assert!(checked >= 4, "only {checked} commands in the quick start");
}
