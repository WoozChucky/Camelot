//! `cargo xtask up` end to end: the real Lake, Avalon and Pendragon binaries started
//! as `up` starts them, on ephemeral ports, with a temporary images directory; each
//! one asked whether it answers; the demo's Camelot VM booted to its shell, with a
//! second one beside it; and everything stopped again. Nothing waits a fixed time:
//! every wait is for something to happen, with a generous limit.
//!
//! The binaries are built once, by `up`'s own build step (`cargo build --release -p
//! ...`, a no-op after `cargo test --release` has built the workspace), and so is the
//! root image, into a temporary file.

use avalon_client::Client;
use avalon_proto::{Connection, Message, VmRef, VmState};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
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

/// The root image `up` builds, built once (with the binaries) for every test.
fn root_image() -> &'static [u8] {
    static IMAGE: OnceLock<Vec<u8>> = OnceLock::new();
    IMAGE.get_or_init(|| {
        let dir = TempDir::new("build");
        let mut options = Options::new(&xtask::repo_root());
        options.image = dir.0.join("camelot-fs.img");
        let image = up::build(&options, &mut |line| eprintln!("{line}")).unwrap();
        assert_eq!(std::fs::read(&options.image).unwrap(), image);
        image
    })
}

/// `up`'s options for a test: ephemeral ports, everything under `dir`, nothing to
/// build (the image is [`root_image`]).
fn options(dir: &TempDir) -> Options {
    let mut o = Options::new(&xtask::repo_root());
    o.images = dir.0.join("images");
    o.image = dir.0.join("camelot-fs.img");
    std::fs::write(&o.image, root_image()).unwrap();
    o.build = false;
    let any = "127.0.0.1:0".parse().unwrap();
    (o.lake, o.avalon, o.pendragon) = (any, any, any);
    o
}

/// The token in Pendragon's URL.
fn token(url: &str) -> &str {
    url.rsplit_once("token=").unwrap().1
}

/// One HTTP request to Pendragon; the status and the body.
fn http(addr: SocketAddr, path: &str, token: &str) -> (u16, String) {
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(SLOW)).unwrap();
    write!(
        s,
        "GET {path} HTTP/1.1\r\nHost: {addr}\r\nX-Pendragon-Token: {token}\r\nConnection: close\r\n\r\n"
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
    assert!(disks.iter().any(|d| d.name == "camelot"), "{disks:?}");
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

#[test]
fn the_platform_comes_up_boots_camelot_and_stops() {
    let dir = TempDir::new("demo");
    let out = Output::default();
    let log = &mut |line: &str| out.push(line);
    let mut options = options(&dir);
    options.demo = true;

    up::prepare(&options, log).unwrap();
    assert_eq!(
        std::fs::read(options.images.join("camelot")).unwrap(),
        root_image()
    );
    let mut platform = Platform::start(&options, log).unwrap();
    for addr in [platform.lake, platform.avalon, platform.pendragon] {
        assert!(addr.ip().is_loopback() && addr.port() != 0, "{addr}");
    }
    all_answer(&platform);

    // The demo: a NAT switch and a VM that boots Camelot from its own copy of the
    // image, to the shell.
    let id = platform.demo(log).unwrap();
    let mut avalon = Client::connect(&platform.avalon.to_string(), "test").unwrap();
    let switches = avalon.list_switches().unwrap();
    assert_eq!(switches.len(), 1);
    assert_eq!(switches[0].name, "nat");
    assert!(switches[0].nat.is_some());
    let mut one = Console::attach(platform.avalon, id);
    one.wait_for_end(PROMPT);
    assert!(one.output.contains("Camelot v"), "{}", one.output);
    one.type_line("echo up from its own disk");
    one.wait_for_end("up from its own disk\n$ ");

    // A second Camelot VM beside it, on a copy of its own.
    let mut lake = Client::connect(&platform.lake.to_string(), "test").unwrap();
    let (two_id, disk) = avalon_client::create_camelot_vm(
        &mut avalon,
        &mut lake,
        &platform.lake.to_string(),
        "second",
        avalon_proto::HardwareProfile::default(),
    )
    .unwrap();
    assert_eq!(disk, "second-root");
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
    assert_eq!(writer("camelot-root"), Some(true));
    assert_eq!(writer("second-root"), Some(true));
    assert_eq!(writer("camelot"), Some(false));
    drop((avalon, lake, one, two));

    let summary = platform.summary(true);
    assert!(summary.contains(&platform.url), "{summary}");
    assert!(summary.contains(&format!(
        "cargo run --release -p herald -- --server {} camelot",
        platform.avalon
    )));
    // The Round Table commands it shows are ones Round Table takes.
    let mut commands = 0;
    for line in summary.lines() {
        if let Some(args) = line.trim().strip_prefix("cargo run --release -p round-table -- ") {
            let args: Vec<String> = args.split_whitespace().map(str::to_string).collect();
            round_table::parse_args(&args).unwrap_or_else(|e| panic!("{line}: {e}"));
            commands += 1;
        }
    }
    assert_eq!(commands, 2, "{summary}");

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
            .any(|l| l.starts_with("up: image camelot in ") && l.ends_with(": created")),
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
    let mut platform = Platform::start(&options, log).unwrap();
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
    let e = Platform::start(&options, log).err().unwrap();
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

/// With `--no-build`, `up` needs the binaries and an image to put in Lake's directory.
#[test]
fn no_build_needs_what_a_build_makes() {
    let dir = TempDir::new("nobuild");
    let mut options = options(&dir);
    let log = &mut |line: &str| eprintln!("{line}");
    options.image = dir.0.join("missing.img");
    let e = up::prepare(&options, log).unwrap_err();
    assert!(e.starts_with("there is no "), "{e}");
    // A base image already in the directory is enough.
    up::seed(&options.images, &[0; 512]).unwrap();
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
                // herald [--server ADDR] [--screen] <vm>: one VM, as its --help says.
                let vms: Vec<&&str> = args.iter().filter(|a| !a.starts_with('-')).collect();
                assert_eq!(vms.len(), 1, "{line}");
            }
            _ => continue,
        }
        checked += 1;
    }
    assert!(checked >= 4, "only {checked} commands in the quick start");
}
