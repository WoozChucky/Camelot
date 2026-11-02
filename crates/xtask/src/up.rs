//! `cargo xtask up`: the whole Avalon platform with one command.
//!
//! 1. **Build** ([`build`]): the platform's binaries in release mode
//!    (`cargo build --release -p lake -p avalon -p pendragon -p round-table -p herald`),
//!    and the bootable Camelot root image, as `cargo xtask fs` builds it
//!    (`target/camelot-fs.img`). `--no-build` skips both and uses what is there.
//! 2. **Seed** ([`seed`]): Lake's images directory (`target/platform/images` by
//!    default) gets the root image (2 MiB) as the disk `os-base`, the image a Camelot
//!    VM's disk is a copy of (`avalon_client::BASE_IMAGE`). It is written when it is missing
//!    and replaced when it differs from the one just built; nothing else in the
//!    directory is touched.
//! 3. **Start** ([`Platform::start`]): Lake, Avalon and Pendragon, as child processes
//!    of the built binaries, one after the other, each once the one before it is
//!    listening. Their output is read by a thread per pipe and printed with the name of
//!    the server in front. Lake and Avalon say where they listen (`listening on ADDR`)
//!    and Pendragon prints its URL, so port 0 works too: the tests use ephemeral ports.
//!    A server listening on every address (`0.0.0.0` or `::`) is reached on loopback
//!    ([`reachable`]): that is the address Pendragon, the demo and the summary use,
//!    since Windows refuses to connect to an unspecified address.
//! 4. **Demo** (`--demo`, [`Platform::demo`]): a switch `nat` with a NAT gateway, and a
//!    VM `vm001` with the default hardware (4 MiB of RAM) that boots from `vm001-disk0`,
//!    its own copy of the image, plugged into it and started. The gateway forwards
//!    127.0.0.1:2323 (`--shd`) to the VM's port 23, and once the VM is at its prompt,
//!    `up` types `shd &` on its console: the shell server, which gives each connection
//!    (`herald --tcp 127.0.0.1:2323`) a shell of its own, beside the console's. `up`
//!    checks that a shell answers through the forward; if none does (an old disk
//!    without `/bin/shd`, say), it says so and how to start it, and goes on.
//! 5. **Wait** ([`Platform::wait`]) until Ctrl-C, or until one of the servers ends;
//!    then **stop** ([`Platform::stop`]) the others, Pendragon first and Lake last.
//!
//! The servers are put in a process group of their own, so the console's Ctrl-C
//! reaches only `up`, which stops them in order (and says so). On Windows they are
//! also in a job object that ends them when `up` ends, however it ends, so no server
//! is left behind holding its port. Lake has every write it has answered in its files
//! already, and a VM keeps nothing that is not on its disks, so stopping a server
//! loses nothing.

use avalon_client::{create_camelot_vm, vm_disk, CamelotError, Client, BASE_IMAGE};
use avalon_proto::{
    DeviceSpec, DiskSource, DiskSpec, ErrorCode, FwdProto, HardwareProfile, Message, NatConfig, NetSpec,
    PortForward, VmRef,
};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

/// Lake's images directory, by default (under the repository root).
pub const DEFAULT_IMAGES: &str = "target/platform/images";

/// The servers, in the order they start.
pub const SERVERS: [&str; 3] = ["lake", "avalon", "pendragon"];

/// The binaries `up` builds: the servers, and the clients its summary tells you to
/// use.
pub const BINARIES: [&str; 5] = ["lake", "avalon", "pendragon", "round-table", "herald"];

/// Where each server listens by default: the platform's default addresses.
pub const DEFAULT_LAKE: &str = avalon_proto::DEFAULT_LAKE_ADDR;
pub const DEFAULT_AVALON: &str = avalon_proto::DEFAULT_AVALON_ADDR;
pub const DEFAULT_PENDRAGON: &str = "127.0.0.1:7402";
/// Where the demo's gateway forwards to the demo VM's shell server by default.
pub const DEFAULT_SHD: &str = "127.0.0.1:2323";

/// The demo VM's address on its switch (the first a NAT gateway's DHCP server gives)
/// and the port `shd` listens on.
pub const DEMO_GUEST: SocketAddrV4 = SocketAddrV4::new(Ipv4Addr::new(10, 0, 2, 15), 23);

/// The first line of a Camelot shell's greeting.
const SHELL_BANNER: &str = "Type help for the list of commands.\n";

/// The demo's switch and VM.
pub const DEMO_SWITCH: &str = "nat";
pub const DEMO_VM: &str = "vm001";

/// How long a server may take to say it is listening.
pub const STARTUP_TIMEOUT: Duration = Duration::from_secs(60);

/// How often [`Platform::start`] and [`Platform::wait`] look at the stop flag between
/// lines of output.
const POLL: Duration = Duration::from_millis(100);

/// The error of a [`Platform::start`] that Ctrl-C ended.
pub const INTERRUPTED: &str = "interrupted";

pub const USAGE: &str = "usage: cargo xtask up [options]

Brings the Avalon platform up: builds it, puts the Camelot root image in Lake's
images directory as the image os-base, and runs Lake, Avalon and Pendragon until
Ctrl-C (or until one of them ends), their output prefixed by server.

  --images DIR       Lake's images directory (default target/platform/images).
                     Only the image os-base is written there: created when it is
                     missing, replaced when the build has changed it
  --lake ADDR        where Lake listens (default 127.0.0.1:7401)
  --avalon ADDR      where Avalon listens (default 127.0.0.1:7400)
  --pendragon ADDR   where Pendragon serves its page (default 127.0.0.1:7402)
                     ADDR is host:port, or a host alone for the default port
  --no-build         use the binaries and target/camelot-fs.img already built
  --demo             also create a switch nat with a NAT gateway and a VM vm001
                     that boots from its own copy of the image, vm001-disk0,
                     plugged into it, and start it, with shd, its shell server,
                     running: herald --tcp 127.0.0.1:2323 gives a shell of its own
  --shd ADDR         where the demo's gateway forwards to the VM's shd (port 23;
                     default 127.0.0.1:2323). IPv4 only. shd's shells run as
                     root with no password: keep it on loopback
  -h, --help         show this text";

/// What `up` does, from its command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    /// The repository root: where `cargo build` runs and the root image is built from.
    pub root: PathBuf,
    /// Where the built binaries are (`target/release`).
    pub bin_dir: PathBuf,
    /// The bootable root image, as `cargo xtask fs` writes it.
    pub image: PathBuf,
    /// Lake's images directory.
    pub images: PathBuf,
    pub lake: SocketAddr,
    pub avalon: SocketAddr,
    pub pendragon: SocketAddr,
    /// Where the demo's gateway forwards to the demo VM's `shd`.
    pub shd: SocketAddrV4,
    /// Build the binaries and the root image first.
    pub build: bool,
    pub demo: bool,
}

impl Options {
    /// The defaults, for the repository at `root`.
    pub fn new(root: &Path) -> Options {
        let target = match std::env::var_os("CARGO_TARGET_DIR") {
            Some(dir) => root.join(dir),
            None => root.join("target"),
        };
        Options {
            root: root.to_path_buf(),
            bin_dir: target.join("release"),
            image: root.join("target").join("camelot-fs.img"),
            images: DEFAULT_IMAGES
                .split('/')
                .fold(root.to_path_buf(), |path, part| path.join(part)),
            lake: DEFAULT_LAKE.parse().expect("the default address parses"),
            avalon: DEFAULT_AVALON.parse().expect("the default address parses"),
            pendragon: DEFAULT_PENDRAGON.parse().expect("the default address parses"),
            shd: DEFAULT_SHD.parse().expect("the default address parses"),
            build: true,
            demo: false,
        }
    }

    /// The path of binary `name` (`lake`, `round-table`, ...).
    pub fn binary(&self, name: &str) -> PathBuf {
        self.bin_dir
            .join(format!("{name}{}", std::env::consts::EXE_SUFFIX))
    }
}

/// Parses `up`'s arguments (after `up`). `Ok(None)` asks for the help text.
pub fn parse_args(args: &[String], root: &Path) -> Result<Option<Options>, String> {
    let mut o = Options::new(root);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = |name: &str| it.next().cloned().ok_or_else(|| format!("{name} needs a value"));
        let addr = |name: &str, s: String, default: &str| {
            avalon_proto::parse_listen(&s, avalon_proto::default_port(default))
                .map_err(|e| format!("{name}: {e}"))
        };
        match a.as_str() {
            "-h" | "--help" => return Ok(None),
            "--images" => match value("--images")? {
                dir if dir.is_empty() => return Err("--images: empty directory name".into()),
                dir => o.images = PathBuf::from(dir),
            },
            "--lake" => o.lake = addr("--lake", value("--lake")?, DEFAULT_LAKE)?,
            "--avalon" => o.avalon = addr("--avalon", value("--avalon")?, DEFAULT_AVALON)?,
            "--pendragon" => o.pendragon = addr("--pendragon", value("--pendragon")?, DEFAULT_PENDRAGON)?,
            "--shd" => match addr("--shd", value("--shd")?, DEFAULT_SHD)? {
                SocketAddr::V4(v4) => o.shd = v4,
                SocketAddr::V6(_) => return Err("--shd: the gateway forwards IPv4 addresses only".into()),
            },
            "--no-build" => o.build = false,
            "--demo" => o.demo = true,
            other => return Err(format!("unknown option {other}")),
        }
    }
    Ok(Some(o))
}

/// Where `up`'s own messages and the servers' output go, a line at a time.
pub type Log<'a> = &'a mut dyn FnMut(&str);

// ---- Build and seed ----

/// Builds the binaries (`cargo build --release -p ...`, its output on this process's
/// stderr) and the bootable root image, written to `options.image`. Returns the image.
pub fn build(options: &Options, log: Log) -> Result<Vec<u8>, String> {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let mut cmd = Command::new(cargo);
    cmd.current_dir(&options.root).args(["build", "--release"]);
    for b in BINARIES {
        cmd.args(["-p", b]);
    }
    log(&format!(
        "up: building {} (cargo build --release)",
        BINARIES.join(", ")
    ));
    let status = cmd
        .stdin(Stdio::null())
        .status()
        .map_err(|e| format!("cannot run cargo: {e}"))?;
    if !status.success() {
        return Err(format!("cargo build failed ({status})"));
    }
    log(&format!(
        "up: building the root image {}",
        options.image.display()
    ));
    let image = crate::fs::build_bootable(&options.root, crate::fs::IMAGE_SIZE)?;
    if let Some(dir) = options.image.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    }
    std::fs::write(&options.image, &image)
        .map_err(|e| format!("cannot write {}: {e}", options.image.display()))?;
    Ok(image)
}

/// What [`seed`] did with the image `os-base`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Seeded {
    Created,
    /// It was there, but not the image just built.
    Replaced,
    UpToDate,
}

/// Puts `image` in the images directory `dir` (created if needed) as the disk
/// [`BASE_IMAGE`], unless it is there already. The new contents go to a file whose
/// name is not a disk name and are renamed into place, so Lake never sees half an
/// image. Nothing else in the directory is touched.
pub fn seed(dir: &Path, image: &[u8]) -> io::Result<Seeded> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(BASE_IMAGE);
    let seeded = match std::fs::read(&path) {
        Ok(old) if old == image => return Ok(Seeded::UpToDate),
        Ok(_) => Seeded::Replaced,
        Err(e) if e.kind() == io::ErrorKind::NotFound => Seeded::Created,
        Err(e) => return Err(e),
    };
    let temp = dir.join(format!(".{BASE_IMAGE}.up"));
    std::fs::write(&temp, image)?;
    std::fs::rename(&temp, &path).inspect_err(|_| {
        let _ = std::fs::remove_file(&temp);
    })?;
    Ok(seeded)
}

/// The root image to seed: the one just built, or with `--no-build` the one at
/// `options.image`, if there is one (`None`: leave the images directory as it is).
fn image_to_seed(options: &Options, log: Log) -> Result<Option<Vec<u8>>, String> {
    if options.build {
        return build(options, log).map(Some);
    }
    match std::fs::read(&options.image) {
        Ok(image) => Ok(Some(image)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("cannot read {}: {e}", options.image.display())),
    }
}

/// Builds (unless `--no-build`) and seeds the images directory; with `--no-build`,
/// checks that the binaries are there and that Lake will have an image to copy.
///
/// With `--demo`, when the image has just been replaced and the demo VM's disk is
/// there from an earlier run, says that the disk is a copy of the old image: the demo
/// boots from it as it is (with what the VM wrote to it), not from the new build.
pub fn prepare(options: &Options, log: Log) -> Result<(), String> {
    let image = image_to_seed(options, log)?;
    for name in SERVERS {
        let path = options.binary(name);
        if !path.is_file() {
            return Err(format!(
                "{} is missing: run cargo xtask up without --no-build",
                path.display()
            ));
        }
    }
    let base = options.images.join(BASE_IMAGE);
    match image {
        Some(image) => {
            let seeded =
                seed(&options.images, &image).map_err(|e| format!("cannot write {}: {e}", base.display()))?;
            let what = match seeded {
                Seeded::Created => "created",
                Seeded::Replaced => "replaced with the new build",
                Seeded::UpToDate => "up to date",
            };
            log(&format!(
                "up: image {BASE_IMAGE} in {}: {what}",
                options.images.display()
            ));
            let demo_disk = options.images.join(vm_disk(DEMO_VM));
            if seeded == Seeded::Replaced && options.demo && demo_disk.is_file() {
                log(&format!(
                    "up: note: the demo VM's disk {} is a copy of the image {BASE_IMAGE} from \
                     before this build, and the demo boots it as it is; to boot the new build, \
                     delete {} while up is not running (what the VM wrote there goes with it)",
                    vm_disk(DEMO_VM),
                    demo_disk.display()
                ));
            }
        }
        None if base.is_file() => {}
        None => {
            return Err(format!(
                "there is no {} to put in {} as the image {BASE_IMAGE}: run cargo xtask up \
                 without --no-build",
                options.image.display(),
                options.images.display()
            ))
        }
    }
    Ok(())
}

// ---- The servers ----

/// One line of a server's output, or the end of one of its pipes.
enum Event {
    Line(usize, String),
    Closed(usize),
}

/// A running server.
struct Server {
    name: &'static str,
    child: Child,
    /// Its stdout and stderr have both ended.
    pipes_closed: u8,
}

/// How [`Platform::wait`] ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ending {
    /// The stop flag was set (Ctrl-C).
    Stopped,
    /// This server ended by itself, with this status.
    Exited(&'static str, Option<i32>),
}

/// Lake, Avalon and Pendragon, running as child processes.
pub struct Platform {
    servers: Vec<Server>,
    events: Receiver<Event>,
    sender: Sender<Event>,
    /// On Windows, the job object that ends the servers when `up` ends.
    job: sys::Job,
    pub lake: SocketAddr,
    pub avalon: SocketAddr,
    pub pendragon: SocketAddr,
    /// Pendragon's URL, with this launch's token.
    pub url: String,
    images: PathBuf,
    /// Where the demo's gateway is asked to forward to the demo VM's `shd`.
    shd_listen: SocketAddrV4,
    /// Where the demo's gateway forwards to its VM's `shd`, as bound and reachable,
    /// once [`Platform::demo`] has set the forward up.
    pub shd: Option<SocketAddr>,
}

impl Platform {
    /// Starts Lake on `options.images`, then Avalon, then Pendragon in front of both,
    /// each once the one before is listening; returns when all three are. Their output
    /// goes to `log`. Fails (stopping what it started) when a server ends or says
    /// nothing for [`STARTUP_TIMEOUT`], or with [`INTERRUPTED`] as soon as `stop` is
    /// set (Ctrl-C).
    ///
    /// The addresses it keeps are where the servers can be reached: a server that
    /// listens on every address is reached on loopback ([`reachable`]).
    pub fn start(options: &Options, stop: &AtomicBool, log: Log) -> Result<Platform, String> {
        let images =
            std::path::absolute(&options.images).map_err(|e| format!("{}: {e}", options.images.display()))?;
        let (sender, events) = mpsc::channel();
        let mut platform = Platform {
            servers: Vec::new(),
            events,
            sender,
            job: sys::Job::new(),
            lake: options.lake,
            avalon: options.avalon,
            pendragon: options.pendragon,
            url: String::new(),
            images,
            shd_listen: options.shd,
            shd: None,
        };
        let images = platform.images.to_string_lossy().into_owned();
        let lake = platform.spawn(
            options,
            "lake",
            &["--listen", &options.lake.to_string(), "--dir", &images],
        )?;
        platform.lake = reachable(platform.wait_for(lake, stop, log, listening)?);
        let avalon = platform.spawn(options, "avalon", &["--listen", &options.avalon.to_string()])?;
        platform.avalon = reachable(platform.wait_for(avalon, stop, log, listening)?);
        let args = [
            "--listen".to_string(),
            options.pendragon.to_string(),
            "--avalon".to_string(),
            platform.avalon.to_string(),
            "--lake".to_string(),
            platform.lake.to_string(),
        ];
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        let pendragon = platform.spawn(options, "pendragon", &args)?;
        // Pendragon's URL has a loopback address already when it listens on every one.
        let (addr, url) = platform.wait_for(pendragon, stop, log, pendragon_url)?;
        platform.pendragon = reachable(addr);
        platform.url = url;
        Ok(platform)
    }

    fn spawn(&mut self, options: &Options, name: &'static str, args: &[&str]) -> Result<usize, String> {
        let path = options.binary(name);
        let mut cmd = Command::new(&path);
        cmd.args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        sys::own_process_group(&mut cmd);
        let mut child = cmd
            .spawn()
            .map_err(|e| format!("cannot start {}: {e}", path.display()))?;
        self.job.add(&child);
        let index = self.servers.len();
        let stdout = child.stdout.take().expect("piped");
        let stderr = child.stderr.take().expect("piped");
        for pipe in [Box::new(stdout) as Box<dyn Read + Send>, Box::new(stderr)] {
            let sender = self.sender.clone();
            std::thread::Builder::new()
                .name(format!("{name}-output"))
                .spawn(move || forward(index, pipe, sender))
                .map_err(|e| format!("cannot read {name}'s output: {e}"))?;
        }
        self.servers.push(Server {
            name,
            child,
            pipes_closed: 0,
        });
        Ok(index)
    }

    /// Passes the servers' output to `log` until server `index` prints a line `ready`
    /// takes; returns what it made of it. Fails with [`INTERRUPTED`] once `stop` is
    /// set.
    fn wait_for<T>(
        &mut self,
        index: usize,
        stop: &AtomicBool,
        log: Log,
        ready: fn(&str) -> Option<T>,
    ) -> Result<T, String> {
        let name = self.servers[index].name;
        let deadline = Instant::now() + STARTUP_TIMEOUT;
        let mut last = String::new();
        loop {
            if stop.load(Ordering::SeqCst) {
                return Err(INTERRUPTED.into());
            }
            let left = deadline.saturating_duration_since(Instant::now());
            match self.events.recv_timeout(left.min(POLL)) {
                Ok(Event::Line(i, line)) => {
                    log(&self.prefixed(i, &line));
                    if i == index {
                        if let Some(found) = ready(&line) {
                            return Ok(found);
                        }
                        last = line;
                    }
                }
                Ok(Event::Closed(i)) => {
                    if self.closed(i) && i == index {
                        let status = self.servers[i].child.wait().ok();
                        let status = status.map_or_else(|| "?".into(), |s| s.to_string());
                        return Err(format!("{name} ended before it was ready ({status}): {last}"));
                    }
                }
                Err(RecvTimeoutError::Timeout) if left.is_zero() => {
                    return Err(format!(
                        "{name} did not say it was ready within {STARTUP_TIMEOUT:?}"
                    ))
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => unreachable!("the platform holds a sender"),
            }
        }
    }

    /// Counts a closed pipe of server `i`; true once both are.
    fn closed(&mut self, i: usize) -> bool {
        let server = &mut self.servers[i];
        server.pipes_closed += 1;
        server.pipes_closed == 2
    }

    /// A line of server `i`'s output with the server's name in front (and without its
    /// own `name: ` prefix, which would say it twice).
    fn prefixed(&self, i: usize, line: &str) -> String {
        let name = self.servers[i].name;
        let text = line
            .strip_prefix(name)
            .and_then(|l| l.strip_prefix(": "))
            .unwrap_or(line);
        format!("{name:<9} | {text}")
    }

    /// Passes the servers' output to `log` until `stop` is set or a server ends.
    pub fn wait(&mut self, stop: &AtomicBool, log: Log) -> Ending {
        loop {
            if stop.load(Ordering::SeqCst) {
                return Ending::Stopped;
            }
            match self.events.recv_timeout(POLL) {
                Ok(Event::Line(i, line)) => log(&self.prefixed(i, &line)),
                Ok(Event::Closed(i)) => {
                    if self.closed(i) {
                        let server = &mut self.servers[i];
                        let code = server.child.wait().ok().and_then(|s| s.code());
                        return Ending::Exited(server.name, code);
                    }
                }
                Err(_) => {
                    // No output: a server could still have ended with its pipes held
                    // open by something it started.
                    for server in &mut self.servers {
                        if let Ok(Some(status)) = server.child.try_wait() {
                            return Ending::Exited(server.name, status.code());
                        }
                    }
                }
            }
        }
    }

    /// Passes on the output that is waiting, without waiting for more.
    pub fn drain(&mut self, log: Log) {
        while let Ok(event) = self.events.try_recv() {
            match event {
                Event::Line(i, line) => log(&self.prefixed(i, &line)),
                Event::Closed(i) => {
                    self.closed(i);
                }
            }
        }
    }

    /// Stops the servers that still run, Pendragon first and Lake last, and waits for
    /// each. Returns how each ended.
    pub fn stop(mut self, log: Log) -> Vec<(&'static str, ExitStatus)> {
        let mut ended = Vec::new();
        for server in self.servers.iter_mut().rev() {
            let running = matches!(server.child.try_wait(), Ok(None));
            if running {
                log(&format!("up: stopping {}", server.name));
                let _ = server.child.kill();
            }
            if let Ok(status) = server.child.wait() {
                ended.push((server.name, status));
            }
        }
        self.drain(log);
        ended
    }

    /// For tests: ends server `name` as if it had crashed.
    #[doc(hidden)]
    pub fn kill(&mut self, name: &str) {
        if let Some(server) = self.servers.iter_mut().find(|s| s.name == name) {
            let _ = server.child.kill();
        }
    }

    /// The process ids of the servers, in starting order.
    pub fn pids(&self) -> Vec<(&'static str, u32)> {
        self.servers.iter().map(|s| (s.name, s.child.id())).collect()
    }

    /// `--demo`: a switch [`DEMO_SWITCH`] with a NAT gateway, and a VM [`DEMO_VM`]
    /// that boots from its own copy of the image, plugged into the switch and started.
    /// An existing copy (from an earlier run on the same images directory) is used
    /// again, with what the VM wrote to it. Returns the VM's id.
    ///
    /// The gateway forwards `--shd`'s address to the VM's port 23 ([`Platform::shd`]
    /// is where, as bound), and `shd` is started in the VM ([`start_shd`]). Neither
    /// failing fails the demo: `up` says so, and what to do.
    pub fn demo(&mut self, log: Log) -> Result<u32, String> {
        let lake_addr = self.lake.to_string();
        let connect = |what: &str, addr: &str| {
            Client::connect(addr, "xtask-up").map_err(|e| format!("demo: cannot reach {what}: {e}"))
        };
        let mut avalon = connect("Avalon", &self.avalon.to_string())?;
        let mut lake = connect("Lake", &lake_addr)?;
        let forward = PortForward {
            proto: FwdProto::Tcp,
            host: self.shd_listen,
            guest: DEMO_GUEST,
        };
        let nat = NatConfig {
            hostfwd: vec![forward],
        };
        match avalon.create_switch_with(DEMO_SWITCH, Some(nat)) {
            Ok(()) => log(&format!(
                "up: demo: created switch {DEMO_SWITCH} with a NAT gateway, forwarding {} to {DEMO_GUEST}",
                self.shd_listen
            )),
            Err(e) if e.code() == Some(ErrorCode::AlreadyExists) => {}
            // The address is taken, most likely: the demo goes on without the forward.
            Err(e) => {
                log(&format!(
                    "up: demo: cannot forward {} to the VM's shell server ({e}); no shd \
                     (--shd picks another address)",
                    self.shd_listen
                ));
                match avalon.create_switch_with(DEMO_SWITCH, Some(NatConfig::default())) {
                    Ok(()) => log(&format!(
                        "up: demo: created switch {DEMO_SWITCH} with a NAT gateway"
                    )),
                    Err(e) if e.code() == Some(ErrorCode::AlreadyExists) => {}
                    Err(e) => return Err(format!("demo: cannot create switch {DEMO_SWITCH}: {e}")),
                }
            }
        }
        // The forward as bound (port 0 picks one), if the switch has it.
        let switches = avalon
            .list_switches()
            .map_err(|e| format!("demo: cannot list the switches: {e}"))?;
        self.shd = switches
            .iter()
            .filter(|s| s.name == DEMO_SWITCH)
            .filter_map(|s| s.nat.as_ref())
            .flat_map(|nat| &nat.hostfwd)
            .find(|f| f.proto == FwdProto::Tcp && f.guest == DEMO_GUEST)
            .map(|f| reachable(SocketAddr::V4(f.host)));
        let profile = HardwareProfile {
            devices: vec![DeviceSpec::Net(NetSpec {
                switch: Some(DEMO_SWITCH.into()),
                mac: None,
            })],
            ..HardwareProfile::default()
        };
        let disk = vm_disk(DEMO_VM);
        let id = match create_camelot_vm(&mut avalon, &mut lake, &lake_addr, DEMO_VM, profile.clone()) {
            Ok((id, _)) => {
                log(&format!("up: demo: created disk {disk}, a copy of {BASE_IMAGE}"));
                id
            }
            Err(CamelotError::Lake { cause, .. }) if cause.code() == Some(ErrorCode::AlreadyExists) => {
                // Its disk from an earlier run.
                let source = DiskSource::parse(&format!("{}{lake_addr}/{disk}", avalon_proto::LAKE_SCHEME))
                    .map_err(|e| format!("demo: {e}"))?;
                let mut profile = profile;
                profile.devices.insert(
                    0,
                    DeviceSpec::Disk(DiskSpec {
                        source,
                        read_only: false,
                    }),
                );
                log(&format!("up: demo: using disk {disk} from an earlier run"));
                avalon
                    .create_vm(DEMO_VM, profile, Vec::new())
                    .map_err(|e| format!("demo: cannot create VM {DEMO_VM}: {e}"))?
            }
            Err(e) => return Err(format!("demo: cannot create VM {DEMO_VM}: {e}")),
        };
        avalon
            .start_vm(&VmRef::Id(id))
            .map_err(|e| format!("demo: cannot start VM {DEMO_VM}: {e}"))?;
        log(&format!(
            "up: demo: started VM {id} ({DEMO_VM}) on switch {DEMO_SWITCH}"
        ));
        if let Some(shd) = self.shd {
            match start_shd(self.avalon, id, shd) {
                Ok(()) => log(&format!(
                    "up: demo: started shd, the shell server, in VM {id}: each connection to {shd} \
                     is a shell of its own"
                )),
                Err(e) => log(&format!(
                    "up: demo: could not start shd in VM {id} ({e}); type shd & on its console"
                )),
            }
        }
        Ok(id)
    }

    /// What `up` prints once the platform is up: where everything is, the commands
    /// to use it, and how to stop it.
    pub fn summary(&self, demo: bool) -> String {
        let server = if self.avalon.to_string() == DEFAULT_AVALON {
            String::new()
        } else {
            format!("--server {} ", self.avalon)
        };
        let lake = if self.lake.to_string() == DEFAULT_LAKE {
            String::new()
        } else {
            format!("--lake {} ", self.lake)
        };
        // The VM to attach to: the demo's, or the one the create command makes.
        let vm = if demo { DEMO_VM } else { "web" };
        let shell = match self.shd {
            Some(shd) if demo => {
                format!("\x20 cargo run --release -p herald -- --tcp {shd}      (a shell of its own: shd)\n")
            }
            _ => String::new(),
        };
        format!(
            "\nThe Avalon platform is up.\n\
             \n  Pendragon  {}\n             open it in a browser (the token changes at every launch)\
             \n  Avalon     {}\n  Lake       {}, images in {}\n\
             \nFrom another terminal: a Camelot VM of your own, the list of VMs, a VM's console\n\
             \x20 cargo run --release -p round-table -- {server}{lake}create --name web --camelot --start\n\
             \x20 cargo run --release -p round-table -- {server}list\n\
             \x20 cargo run --release -p herald -- {server}{vm}      (Ctrl-] detaches)\n\
             {shell}\
             \nCtrl-C stops Pendragon, Avalon and Lake.\n",
            self.url,
            self.avalon,
            self.lake,
            self.images.display()
        )
    }
}

impl Drop for Platform {
    /// However `up` ends (a failed start, a panic), no server outlives it.
    fn drop(&mut self) {
        for server in self.servers.iter_mut().rev() {
            if matches!(server.child.try_wait(), Ok(None)) {
                let _ = server.child.kill();
                let _ = server.child.wait();
            }
        }
    }
}

/// How long [`start_shd`] waits for the demo VM's prompt, and then for a shell through
/// the forward.
pub const SHD_TIMEOUT: Duration = Duration::from_secs(60);

/// Starts `shd` in VM `vm` on the Avalon server at `avalon`: waits for the shell's
/// prompt on its console, types `shd &`, and waits until a shell answers at `shd`,
/// the forwarded port (then leaves it with `exit`). Fails if any of it does not
/// happen within [`SHD_TIMEOUT`].
pub fn start_shd(avalon: SocketAddr, vm: u32, shd: SocketAddr) -> Result<(), String> {
    let deadline = Instant::now() + SHD_TIMEOUT;
    let mut client =
        Client::connect(&avalon.to_string(), "xtask-up").map_err(|e| format!("cannot reach Avalon: {e}"))?;
    let (id, scrollback) = client
        .attach_console(&VmRef::Id(vm))
        .map_err(|e| format!("cannot attach to its console: {e}"))?;
    let conn = client.connection();
    conn.get_ref()
        .set_read_timeout(Some(POLL))
        .map_err(|e| e.to_string())?;
    let mut output = String::from_utf8_lossy(&scrollback).into_owned();
    while !output.ends_with("\n$ ") {
        if Instant::now() > deadline {
            return Err("its shell never showed a prompt".into());
        }
        match conn.recv() {
            Ok(Message::ConsoleOutput { data, .. }) => output += &String::from_utf8_lossy(&data),
            Ok(_) => {}
            Err(avalon_proto::Error::Io(e))
                if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => {}
            Err(e) => return Err(format!("its console: {e}")),
        }
    }
    conn.send(&Message::ConsoleInput {
        vm: id,
        data: b"shd &\n".to_vec(),
    })
    .map_err(|e| format!("its console: {e}"))?;
    // Until shd listens, the gateway's connection to the VM is refused, and so the
    // host's ends at once: try again a little later.
    loop {
        if shell_answers(shd, deadline) {
            return Ok(());
        }
        if Instant::now() > deadline {
            return Err(format!("no shell answered at {shd}"));
        }
        std::thread::sleep(POLL);
    }
}

/// Whether a Camelot shell greets a connection to `addr` before `deadline`; if one
/// does, it is left with `exit`.
fn shell_answers(addr: SocketAddr, deadline: Instant) -> bool {
    let Ok(mut stream) = TcpStream::connect_timeout(&addr, POLL * 10) else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(POLL));
    let mut got = Vec::new();
    let mut buf = [0u8; 256];
    while Instant::now() < deadline {
        match stream.read(&mut buf) {
            Ok(0) => return false,
            Ok(n) => {
                got.extend_from_slice(&buf[..n]);
                if String::from_utf8_lossy(&got).starts_with(SHELL_BANNER) && got.ends_with(b"$ ") {
                    let _ = stream.write_all(b"exit\n");
                    return true;
                }
            }
            Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => {}
            Err(_) => return false,
        }
    }
    false
}

/// Reads one pipe of server `index` line by line into `sender`, then says it ended.
fn forward(index: usize, pipe: Box<dyn Read + Send>, sender: Sender<Event>) {
    let mut reader = BufReader::new(pipe);
    let mut line = Vec::new();
    loop {
        line.clear();
        match reader.read_until(b'\n', &mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) => {
                let text = String::from_utf8_lossy(&line);
                let text = text.trim_end_matches(['\n', '\r']).to_string();
                if sender.send(Event::Line(index, text)).is_err() {
                    return;
                }
            }
        }
    }
    let _ = sender.send(Event::Closed(index));
}

/// Lake's and Avalon's `<name>: listening on ADDR`.
fn listening(line: &str) -> Option<SocketAddr> {
    let (_, rest) = line.split_once(": listening on ")?;
    rest.split_whitespace().next()?.parse().ok()
}

/// Pendragon's URL line, `http://ADDR/?token=...`: its address and the URL.
fn pendragon_url(line: &str) -> Option<(SocketAddr, String)> {
    let rest = line.trim().strip_prefix("http://")?;
    let addr = rest.split('/').next()?.parse().ok()?;
    Some((addr, line.trim().to_string()))
}

/// Where a server listening at `addr` is reached: `addr` itself, or loopback (of the
/// same family, on the same port) when `addr` is unspecified (`0.0.0.0`, `::`). A
/// server bound there accepts connections on every address, but connecting to the
/// unspecified address fails on Windows (WSAEADDRNOTAVAIL).
pub fn reachable(addr: SocketAddr) -> SocketAddr {
    let mut addr = addr;
    if addr.ip().is_unspecified() {
        addr.set_ip(match addr {
            SocketAddr::V4(_) => IpAddr::V4(Ipv4Addr::LOCALHOST),
            SocketAddr::V6(_) => IpAddr::V6(Ipv6Addr::LOCALHOST),
        });
    }
    addr
}

// ---- The command ----

/// `cargo xtask up`: [`prepare`], [`Platform::start`], the demo, the summary, then
/// [`Platform::wait`] until `stop` is set (Ctrl-C) or a server ends, and
/// [`Platform::stop`]. Everything it says goes to `log`.
///
/// Ctrl-C while the servers start or the demo is made stops what has started, and
/// `run` returns `Ok`.
pub fn run(options: &Options, stop: &AtomicBool, log: Log) -> Result<(), String> {
    prepare(options, log)?;
    let mut platform = match Platform::start(options, stop, log) {
        Ok(platform) => platform,
        Err(e) if e == INTERRUPTED => {
            // `start` has stopped what it started.
            log("up: interrupted while starting; stopped");
            return Ok(());
        }
        Err(e) => return Err(e),
    };
    if options.demo && !stop.load(Ordering::SeqCst) {
        platform.demo(log)?;
    }
    platform.drain(log);
    if !stop.load(Ordering::SeqCst) {
        log(&platform.summary(options.demo));
    }
    let ending = platform.wait(stop, log);
    let result = match &ending {
        Ending::Stopped => Ok(()),
        Ending::Exited(name, code) => {
            let code = code.map_or_else(|| "no exit code".to_string(), |c| format!("exit code {c}"));
            log(&format!("up: {name} ended ({code}); stopping the others"));
            Err(format!("{name} ended"))
        }
    };
    platform.stop(log);
    log("up: stopped");
    result
}

/// Sets the returned flag on Ctrl-C (and Ctrl-Break, closing the console window,
/// SIGINT, SIGTERM, SIGHUP), for [`run`]'s `stop`. A second one ends `up` at once,
/// without stopping the servers in order (on Windows the job object still ends them).
pub fn on_ctrl_c() -> &'static AtomicBool {
    sys::install();
    &sys::INTERRUPTED
}

#[cfg(windows)]
mod sys {
    use std::ffi::c_void;
    use std::os::windows::io::AsRawHandle;
    use std::os::windows::process::CommandExt;
    use std::process::{Child, Command};
    use std::sync::atomic::{AtomicBool, Ordering};

    pub static INTERRUPTED: AtomicBool = AtomicBool::new(false);

    type Bool = i32;
    type Handle = *mut c_void;
    type HandlerRoutine = Option<unsafe extern "system" fn(u32) -> Bool>;

    /// The child's own process group: the console's Ctrl-C does not reach it.
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    /// `JobObjectExtendedLimitInformation` (winnt.h).
    const JOB_OBJECT_EXTENDED_LIMIT_INFORMATION: i32 = 9;
    const JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE: u32 = 0x2000;
    /// CTRL_C_EVENT, CTRL_BREAK_EVENT and CTRL_CLOSE_EVENT (wincon.h).
    const HANDLED: [u32; 3] = [0, 1, 2];

    /// `JOBOBJECT_BASIC_LIMIT_INFORMATION`.
    #[repr(C)]
    #[derive(Default)]
    struct BasicLimits {
        per_process_user_time_limit: i64,
        per_job_user_time_limit: i64,
        limit_flags: u32,
        minimum_working_set_size: usize,
        maximum_working_set_size: usize,
        active_process_limit: u32,
        affinity: usize,
        priority_class: u32,
        scheduling_class: u32,
    }

    /// `JOBOBJECT_EXTENDED_LIMIT_INFORMATION`, with its `IO_COUNTERS` as six `u64`s.
    #[repr(C)]
    #[derive(Default)]
    struct ExtendedLimits {
        basic: BasicLimits,
        io_counters: [u64; 6],
        process_memory_limit: usize,
        job_memory_limit: usize,
        peak_process_memory_used: usize,
        peak_job_memory_used: usize,
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn SetConsoleCtrlHandler(handler: HandlerRoutine, add: Bool) -> Bool;
        fn CreateJobObjectW(attributes: *mut c_void, name: *const u16) -> Handle;
        fn SetInformationJobObject(job: Handle, class: i32, info: *const c_void, length: u32) -> Bool;
        fn AssignProcessToJobObject(job: Handle, process: Handle) -> Bool;
        fn CloseHandle(handle: Handle) -> Bool;
    }

    unsafe extern "system" fn on_ctrl(event: u32) -> Bool {
        if !HANDLED.contains(&event) {
            return 0;
        }
        if INTERRUPTED.swap(true, Ordering::SeqCst) {
            // The second Ctrl-C: the job object ends the servers with us.
            std::process::exit(130);
        }
        1
    }

    pub fn install() {
        // SAFETY: registers a handler with the signature the API expects.
        unsafe {
            SetConsoleCtrlHandler(Some(on_ctrl), 1);
        }
    }

    pub fn own_process_group(cmd: &mut Command) {
        cmd.creation_flags(CREATE_NEW_PROCESS_GROUP);
    }

    /// A job object that kills its processes when its last handle closes: when the
    /// [`super::Platform`] is dropped, or when `up` ends in any way.
    pub struct Job(Handle);

    // SAFETY: a job handle may be used and closed from any thread.
    unsafe impl Send for Job {}

    impl Job {
        /// The job, or one that does nothing where Windows will not make it.
        pub fn new() -> Job {
            // SAFETY: no security attributes, no name; the limits structure is
            // initialised and its size passed with it.
            unsafe {
                let job = CreateJobObjectW(std::ptr::null_mut(), std::ptr::null());
                if job.is_null() {
                    return Job(job);
                }
                let mut limits = ExtendedLimits::default();
                limits.basic.limit_flags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                let ok = SetInformationJobObject(
                    job,
                    JOB_OBJECT_EXTENDED_LIMIT_INFORMATION,
                    &limits as *const ExtendedLimits as *const c_void,
                    std::mem::size_of::<ExtendedLimits>() as u32,
                );
                if ok == 0 {
                    CloseHandle(job);
                    return Job(std::ptr::null_mut());
                }
                Job(job)
            }
        }

        /// Puts `child` in the job (best effort: without it, the servers are still
        /// stopped by `up` itself).
        pub fn add(&self, child: &Child) {
            if !self.0.is_null() {
                // SAFETY: both handles are open: the job is ours, the child's process
                // handle lives as long as `child`.
                unsafe {
                    AssignProcessToJobObject(self.0, child.as_raw_handle());
                }
            }
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            if !self.0.is_null() {
                // SAFETY: closes the handle CreateJobObjectW gave us, once.
                unsafe {
                    CloseHandle(self.0);
                }
            }
        }
    }
}

#[cfg(unix)]
mod sys {
    use std::os::unix::process::CommandExt;
    use std::process::{Child, Command};
    use std::sync::atomic::{AtomicBool, Ordering};

    pub static INTERRUPTED: AtomicBool = AtomicBool::new(false);

    const SIGHUP: i32 = 1;
    const SIGINT: i32 = 2;
    const SIGTERM: i32 = 15;

    extern "C" {
        /// `sighandler_t signal(int, sighandler_t)`; a handler is a function pointer.
        fn signal(signum: i32, handler: usize) -> usize;
        fn _exit(status: i32) -> !;
    }

    extern "C" fn on_signal(_: i32) {
        if INTERRUPTED.swap(true, Ordering::SeqCst) {
            // SAFETY: _exit is async-signal-safe.
            unsafe { _exit(130) }
        }
    }

    pub fn install() {
        // SAFETY: installs a handler that only touches an atomic (or calls _exit).
        unsafe {
            for sig in [SIGHUP, SIGINT, SIGTERM] {
                signal(sig, on_signal as extern "C" fn(i32) as usize);
            }
        }
    }

    pub fn own_process_group(cmd: &mut Command) {
        cmd.process_group(0);
    }

    /// Unix has no job objects: `up` stops the servers itself.
    pub struct Job;

    impl Job {
        pub fn new() -> Job {
            Job
        }

        pub fn add(&self, _child: &Child) {}
    }
}

#[cfg(not(any(windows, unix)))]
mod sys {
    use std::process::{Child, Command};
    use std::sync::atomic::AtomicBool;

    pub static INTERRUPTED: AtomicBool = AtomicBool::new(false);

    pub fn install() {}

    pub fn own_process_group(_cmd: &mut Command) {}

    pub struct Job;

    impl Job {
        pub fn new() -> Job {
            Job
        }

        pub fn add(&self, _child: &Child) {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(text: &str) -> Vec<String> {
        text.split_whitespace().map(str::to_string).collect()
    }

    #[test]
    fn options() {
        let root = Path::new("repo");
        let o = parse_args(&[], root).unwrap().unwrap();
        assert_eq!(o, Options::new(root));
        assert_eq!(o.images, root.join("target/platform/images"));
        assert_eq!(
            (o.lake.to_string(), o.avalon.to_string(), o.pendragon.to_string()),
            (
                "127.0.0.1:7401".into(),
                "127.0.0.1:7400".into(),
                "127.0.0.1:7402".into()
            )
        );
        assert!(o.build && !o.demo);
        assert_eq!(o.shd.to_string(), "127.0.0.1:2323");
        assert_eq!(o.image, root.join("target/camelot-fs.img"));
        let o = parse_args(
            &args("--images imgs --lake 127.0.0.1:0 --avalon 0.0.0.0 --pendragon localhost:9000 --no-build --demo"),
            root,
        )
        .unwrap()
        .unwrap();
        assert_eq!(o.images, PathBuf::from("imgs"));
        assert_eq!(o.lake.to_string(), "127.0.0.1:0");
        assert_eq!(o.avalon.to_string(), "0.0.0.0:7400");
        assert_eq!(o.pendragon.port(), 9000);
        assert!(!o.build && o.demo);
        let shd = |a: &str| parse_args(&args(&format!("--shd {a}")), root).map(|o| o.unwrap().shd);
        assert_eq!(shd("127.0.0.1:0").unwrap().to_string(), "127.0.0.1:0");
        assert_eq!(shd("0.0.0.0").unwrap().to_string(), "0.0.0.0:2323");
        assert!(shd("[::1]:23").unwrap_err().contains("IPv4 addresses only"));
        assert_eq!(parse_args(&args("--demo -h"), root), Ok(None));
        for (bad, why) in [
            ("--lake", "--lake needs a value"),
            ("--avalon x:y", "--avalon: "),
            ("--images", "--images needs a value"),
            ("--shd", "--shd needs a value"),
            ("--bogus", "unknown option --bogus"),
            ("up", "unknown option up"),
        ] {
            let e = parse_args(&args(bad), root).unwrap_err();
            assert!(e.starts_with(why), "{bad}: {e}");
        }
        assert!(parse_args(&["--images".into(), String::new()], root).is_err());
    }

    #[test]
    fn unspecified_addresses_are_reached_on_loopback() {
        for (listen, reached) in [
            ("0.0.0.0:7401", "127.0.0.1:7401"),
            ("[::]:7400", "[::1]:7400"),
            ("127.0.0.1:7402", "127.0.0.1:7402"),
            ("192.168.1.5:80", "192.168.1.5:80"),
            ("[::1]:9", "[::1]:9"),
        ] {
            assert_eq!(
                reachable(listen.parse().unwrap()).to_string(),
                reached,
                "{listen}"
            );
        }
    }

    #[test]
    fn readiness_lines() {
        assert_eq!(
            listening("lake: listening on 127.0.0.1:7401"),
            Some("127.0.0.1:7401".parse().unwrap())
        );
        assert_eq!(
            listening("avalon: listening on [::1]:50000"),
            Some("[::1]:50000".parse().unwrap())
        );
        assert_eq!(listening("lake: images in x"), None);
        assert_eq!(listening("listening on nowhere"), None);
        let (addr, url) = pendragon_url("http://127.0.0.1:55555/?token=00ff\r").unwrap();
        assert_eq!(addr.port(), 55555);
        assert_eq!(url, "http://127.0.0.1:55555/?token=00ff");
        assert_eq!(pendragon_url("pendragon: open the URL above"), None);
    }

    #[test]
    fn seeding() {
        let dir = std::env::temp_dir().join(format!("xtask-up-seed-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let images = dir.join("images");
        assert_eq!(seed(&images, &[1; 1024]).unwrap(), Seeded::Created);
        std::fs::write(images.join("mine"), [7; 512]).unwrap();
        assert_eq!(seed(&images, &[1; 1024]).unwrap(), Seeded::UpToDate);
        assert_eq!(seed(&images, &[2; 2048]).unwrap(), Seeded::Replaced);
        assert_eq!(std::fs::read(images.join("os-base")).unwrap(), [2; 2048]);
        // Nothing else was touched, and nothing was left behind.
        assert_eq!(std::fs::read(images.join("mine")).unwrap(), [7; 512]);
        assert_eq!(std::fs::read_dir(&images).unwrap().count(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
