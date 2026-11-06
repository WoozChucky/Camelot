//! Helpers for Herald's tests: an in-process Avalon server with guest VMs, Herald
//! sessions driven through channels and buffers, and a scripted fake server.

#![allow(dead_code)]

use avalon::{Config, Server, ServerThread};
use avalon_proto::{Connection, HardwareProfile, Message, VmRef, VmState};
use herald::{End, Session};
use std::io::Write;
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

pub const MIB: u64 = 1024 * 1024;

/// Generous ceiling for anything that should take well under a second, so a loaded
/// machine does not make the tests flaky.
pub const SLOW: Duration = Duration::from_secs(20);

/// A quiet server on 127.0.0.1 and an ephemeral port.
pub fn server() -> ServerThread {
    let config = Config {
        listen: "127.0.0.1:0".parse().unwrap(),
        log: false,
        max_ram: None,
        ..Config::default()
    };
    Server::bind(&config).unwrap().spawn().unwrap()
}

pub fn connect(server: &ServerThread) -> Connection<TcpStream> {
    let conn = Connection::connect(server.addr(), "test").unwrap();
    conn.get_ref().set_read_timeout(Some(SLOW)).unwrap();
    conn
}

pub fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// `examples/<name>/<name>.elf`.
pub fn example(name: &str) -> Vec<u8> {
    std::fs::read(repo().join("examples").join(name).join(format!("{name}.elf"))).unwrap()
}

/// The Camelot kernel, `kernel/kernel.elf`.
pub fn kernel() -> Vec<u8> {
    std::fs::read(repo().join("kernel/kernel.elf")).unwrap()
}

/// Creates VM `name` running `program` with `ram_mib` MiB at full speed, and starts
/// it.
pub fn start_vm(server: &ServerThread, name: &str, ram_mib: u64, program: Vec<u8>) -> u32 {
    start_vm_with(server, name, HardwareProfile::with_ram(ram_mib * MIB), program)
}

/// Creates VM `name` running the Camelot kernel, and starts it: on Camelot's
/// firmware, whose 2 MiB and the image's 2.1 MiB from 0x8020_0000 take 8 MiB.
pub fn start_kernel_vm(server: &ServerThread, name: &str) -> u32 {
    let profile = HardwareProfile {
        bios: avalon_proto::Bios::Default,
        ..HardwareProfile::with_ram(8 * MIB)
    };
    start_vm_with(server, name, profile, kernel())
}

/// Creates VM `name` with `profile` running `program`, and starts it.
pub fn start_vm_with(server: &ServerThread, name: &str, profile: HardwareProfile, program: Vec<u8>) -> u32 {
    let mut rt = connect(server);
    let id = match rt.request(&Message::CreateVm {
        name: name.into(),
        profile,
        program,
    }) {
        Ok(Message::VmCreated { id }) => id,
        other => panic!("create {name}: {other:?}"),
    };
    rt.request_ok(&Message::StartVm { vm: VmRef::Id(id) }).unwrap();
    id
}

pub fn state(server: &ServerThread, vm: u32) -> VmState {
    match connect(server).request(&Message::GetVmInfo { vm: VmRef::Id(vm) }) {
        Ok(Message::VmInfo(details)) => details.vm.state,
        other => panic!("info {vm}: {other:?}"),
    }
}

/// Clients attached to VM `vm`'s console.
pub fn consoles(server: &ServerThread, vm: u32) -> usize {
    server.debug_vm(vm).map_or(0, |d| d.consoles)
}

/// Waits until `done` holds, polling.
pub fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
    let start = Instant::now();
    while !done() {
        assert!(start.elapsed() < SLOW, "still waiting for {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// A `Write` into a buffer the test can look at while the session writes.
#[derive(Clone, Default)]
pub struct Shared(pub Arc<Mutex<Vec<u8>>>);

impl Shared {
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }

    pub fn bytes(&self) -> Vec<u8> {
        self.0.lock().unwrap().clone()
    }
}

impl Write for Shared {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// A Herald session on a thread of its own: keys go in through a channel, the
/// console comes out into a buffer, as the binary does with a terminal.
pub struct Herald {
    pub vm: avalon_proto::VmSummary,
    pub scrollback: Vec<u8>,
    keys: Option<Sender<u8>>,
    pub output: Shared,
    pub notices: Shared,
    done: Receiver<Result<End, avalon_proto::Error>>,
    thread: Option<JoinHandle<()>>,
}

impl Herald {
    /// Attaches to `vm` on `server` and starts streaming.
    pub fn attach(server: &ServerThread, vm: VmRef) -> Herald {
        Herald::attach_at(&server.addr().to_string(), vm, |_| {})
    }

    /// Attaches to `vm` on the server at `addr`; `setup` may adjust the session first.
    pub fn attach_at(addr: &str, vm: VmRef, setup: impl FnOnce(&mut Session)) -> Herald {
        let conn = Connection::connect(addr, herald::AGENT).unwrap();
        let mut session = Session::attach(conn, &vm).unwrap_or_else(|e| panic!("attach {vm}: {e}"));
        setup(&mut session);
        let (keys, input) = mpsc::channel();
        let (output, notices) = (Shared::default(), Shared::default());
        let (vm, scrollback) = (session.vm.clone(), session.scrollback.clone());
        let (tx, done) = mpsc::channel();
        let (mut out, mut note) = (output.clone(), notices.clone());
        let thread = std::thread::spawn(move || {
            let _ = tx.send(session.run(input, &mut out, &mut note));
        });
        Herald {
            vm,
            scrollback,
            keys: Some(keys),
            output,
            notices,
            done,
            thread: Some(thread),
        }
    }

    /// Types `bytes`. Like a real terminal, keys typed after the session has
    /// ended (say, right after the detach key) go nowhere: the session drops
    /// its end of the channel when it returns, which may happen before the
    /// test has sent everything, so a failed send is not an error.
    pub fn type_keys(&self, bytes: &[u8]) {
        let keys = self.keys.as_ref().expect("the input is still open");
        for &b in bytes {
            if keys.send(b).is_err() {
                return;
            }
        }
    }

    /// Ends the input, as closing stdin does.
    pub fn close_input(&mut self) {
        self.keys = None;
    }

    /// Waits until the console output contains `needle`, and returns it.
    pub fn wait_for(&self, needle: &str) -> String {
        let start = Instant::now();
        loop {
            let text = self.output.text();
            if text.contains(needle) {
                return text;
            }
            assert!(
                start.elapsed() < SLOW,
                "waiting for {needle:?}; output so far {text:?}"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Waits for the session to end, and returns how.
    pub fn end(&mut self) -> Result<End, avalon_proto::Error> {
        let result = self
            .done
            .recv_timeout(SLOW)
            .unwrap_or_else(|_| panic!("the session is still running; output {:?}", self.output.text()));
        self.thread.take().unwrap().join().unwrap();
        result
    }

    /// Whether the session has ended (without waiting).
    pub fn has_ended(&self) -> bool {
        self.thread.as_ref().is_none_or(|t| t.is_finished())
    }
}

/// A fake Avalon server for one Herald: accepts one connection, does the handshake,
/// then hands it to `script` on a thread. Returns the address to attach to.
pub fn fake_server(script: impl FnOnce(Connection<TcpStream>) + Send + 'static) -> (String, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let thread = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        stream.set_read_timeout(Some(SLOW)).unwrap();
        let mut conn = Connection::new(stream);
        conn.server_handshake("fake").unwrap();
        script(conn);
    });
    (addr, thread)
}

/// The fake server's side of an attach: answers `ConsoleAttach` and `GetVmInfo` for
/// VM `id` named `name` in `state`, with `between` sent between the two replies (the
/// client queues it).
pub fn fake_attach(
    conn: &mut Connection<TcpStream>,
    id: u32,
    state: VmState,
    scrollback: &[u8],
    between: &[Message],
) {
    match conn.recv().unwrap() {
        Message::ConsoleAttach { .. } => {}
        other => panic!("expected ConsoleAttach, got {other:?}"),
    }
    conn.send(&Message::ConsoleAttached {
        vm: id,
        scrollback: scrollback.to_vec(),
    })
    .unwrap();
    for event in between {
        conn.send(event).unwrap();
    }
    match conn.recv().unwrap() {
        Message::GetVmInfo { vm: VmRef::Id(v) } if v == id => {}
        other => panic!("expected GetVmInfo, got {other:?}"),
    }
    conn.send(&Message::VmInfo(avalon_proto::VmDetails {
        vm: avalon_proto::VmSummary {
            id,
            name: "fake".into(),
            state,
            profile: HardwareProfile::default(),
        },
        devices: Vec::new(),
        steps: 0,
    }))
    .unwrap();
}

/// A fake server's side after an attach, for a client that leaves while the server's
/// frames are still coming (#234): sends `flood` without pause, so that some always
/// wait unread on the client's side when it closes, answers `detach` with `Ok` when it
/// comes, and reads what the client sends until its end. Then it stops flooding and
/// closes. Returns what it read, how the reading ended (`Error::Closed` is a clean
/// end, a reset is not), and how the flooding did.
pub fn flood_until_the_client_closes(
    mut conn: Connection<TcpStream>,
    flood: Message,
    detach: Message,
) -> (Vec<Message>, avalon_proto::Error, Result<(), avalon_proto::Error>) {
    use std::sync::atomic::{AtomicBool, Ordering};
    let detached = Arc::new(AtomicBool::new(false));
    let stop = Arc::new(AtomicBool::new(false));
    let mut writer = conn.try_clone().unwrap();
    let flooding = {
        let (detached, stop) = (Arc::clone(&detached), Arc::clone(&stop));
        std::thread::spawn(move || -> Result<(), avalon_proto::Error> {
            let mut replied = false;
            while !stop.load(Ordering::SeqCst) {
                if !replied && detached.load(Ordering::SeqCst) {
                    writer.send(&Message::Ok)?;
                    replied = true;
                }
                writer.send(&flood)?;
            }
            // The client waits for this end's close before it closes its own.
            writer.get_ref().shutdown(std::net::Shutdown::Write)?;
            Ok(())
        })
    };
    let mut got = Vec::new();
    let end = loop {
        match conn.recv() {
            Ok(msg) => {
                if msg == detach {
                    detached.store(true, Ordering::SeqCst);
                }
                got.push(msg);
            }
            Err(e) => break e,
        }
    };
    stop.store(true, Ordering::SeqCst);
    (got, end, flooding.join().unwrap())
}
