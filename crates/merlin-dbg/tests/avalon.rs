//! merlin-dbg on a VM of an Avalon server (`merlin-dbg --vm NAME --server ADDR`,
//! protocol doc, section 6.8), an RV32 VM and then an RV64 one: the server runs in this
//! process, the VM runs the Camelot kernel of its width to its shell, and Herald is
//! attached to its console the whole time. The
//! debugger breaks in the kernel's `write` system call, shows where, continues to the
//! next one and leaves; the console goes on working, and Herald sees nothing but the
//! VM's state changes, few of them (a VM with a debugger has its notices rate-limited).

use avalon::{Config, Server, ServerThread};
use avalon_proto::{Connection, HardwareProfile, Message, VmRef, VmState, Xlen};
use herald::{End, Session};
use std::io::Write;
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const SLOW: Duration = Duration::from_secs(60);

fn path(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").join(rel)
}

fn server() -> ServerThread {
    let config = Config {
        listen: "127.0.0.1:0".parse().unwrap(),
        log: false,
        max_ram: None,
        ..Config::default()
    };
    Server::bind(&config).unwrap().spawn().unwrap()
}

fn connect(server: &ServerThread) -> Connection<TcpStream> {
    let conn = Connection::connect(server.addr(), "test").unwrap();
    conn.get_ref().set_read_timeout(Some(SLOW)).unwrap();
    conn
}

fn state(conn: &mut Connection<TcpStream>, vm: u32) -> VmState {
    match conn.request(&Message::GetVmInfo { vm: VmRef::Id(vm) }) {
        Ok(Message::VmInfo(details)) => details.vm.state,
        other => panic!("{other:?}"),
    }
}

/// sys_write's first line after its prologue, where `break sys_write` stops: the
/// line that takes the file, the first after the function's own.
fn sys_write_line() -> usize {
    let text = std::fs::read_to_string(path("kernel/sysfile.c")).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    let start = lines
        .iter()
        .position(|l| l.starts_with("long sys_write("))
        .expect("no sys_write");
    let take = lines[start..]
        .iter()
        .position(|l| l.trim_start().starts_with("struct file *file = fd_get"))
        .expect("sys_write takes no file");
    start + take + 1
}

/// A buffer the test reads while a session writes to it.
#[derive(Clone, Default)]
struct Shared(Arc<Mutex<Vec<u8>>>);

impl Shared {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
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

/// Herald's console session on a thread, as the binary runs it on a terminal.
struct Herald {
    keys: Sender<u8>,
    output: Shared,
    notices: Shared,
    done: Receiver<End>,
}

impl Herald {
    fn attach(server: &ServerThread, vm: VmRef) -> Herald {
        let conn = Connection::connect(server.addr(), herald::AGENT).unwrap();
        let session = Session::attach(conn, &vm).unwrap();
        let (keys, input) = mpsc::channel();
        let (output, notices) = (Shared::default(), Shared::default());
        let (mut out, mut note) = (output.clone(), notices.clone());
        let (tx, done) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(session.run(input, &mut out, &mut note).unwrap());
        });
        Herald {
            keys,
            output,
            notices,
            done,
        }
    }

    fn type_keys(&self, bytes: &[u8]) {
        for &b in bytes {
            self.keys.send(b).unwrap();
        }
    }

    /// Waits until the console shows `needle` `times` times.
    fn wait_for(&self, needle: &str, times: usize) -> String {
        let start = Instant::now();
        loop {
            let text = self.output.text();
            if text.matches(needle).count() >= times {
                return text;
            }
            assert!(start.elapsed() < SLOW, "waiting for {needle:?}: {text:?}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

#[test]
fn debugging_a_vm_on_avalon_with_herald_attached() {
    // An RV32 VM and an RV64 one, each running the kernel of its width: the debugger
    // learns the width from the stub's target description.
    for (kernel, xlen) in [
        ("kernel/kernel.elf", Xlen::Rv32),
        ("kernel/kernel-rv64.elf", Xlen::Rv64),
    ] {
        let run = std::panic::catch_unwind(|| debug_a_vm(kernel, xlen));
        if let Err(payload) = run {
            let message = payload
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| payload.downcast_ref::<&str>().map(|s| s.to_string()))
                .unwrap_or_default();
            panic!("on {xlen}: {message}");
        }
    }
}

/// The test on a VM of width `xlen` running `kernel`.
fn debug_a_vm(kernel: &str, xlen: Xlen) {
    let server = server();
    let mut control = connect(&server);
    let id = match control.request(&Message::CreateVm {
        name: "vm001".into(),
        profile: HardwareProfile {
            xlen,
            ..HardwareProfile::with_ram(4 * 1024 * 1024)
        },
        program: std::fs::read(path(kernel)).unwrap(),
    }) {
        Ok(Message::VmCreated { id }) => id,
        other => panic!("{other:?}"),
    };
    control
        .request_ok(&Message::StartVm { vm: VmRef::Id(id) })
        .unwrap();
    let herald = Herald::attach(&server, VmRef::Name("vm001".into()));
    herald.wait_for("$ ", 1);

    // The debugger attaches, the VM stops for it; what is typed meanwhile waits.
    let mut dbg = Command::new(env!("CARGO_BIN_EXE_merlin-dbg"))
        .args(["--vm", "vm001", "--server", &server.addr().to_string(), kernel])
        .current_dir(path("."))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let start = Instant::now();
    let attaching = start;
    // Paused for it, as QEMU's stub pauses a VM for a GDB connection.
    while !(server.debug_vm(id).unwrap().debugged && state(&mut control, id) == VmState::Paused) {
        assert!(start.elapsed() < SLOW, "the debugger never attached");
        std::thread::sleep(Duration::from_millis(10));
    }
    herald.type_keys(b"echo camelot\r");
    // Break in the kernel's write system call: the shell's echo writes, and so does
    // its next prompt. Continue from the first to the second, then leave.
    dbg.stdin
        .take()
        .unwrap()
        .write_all(b"break sys_write\ncontinue\nbt\nprint f->regs[17]\ncontinue\ndelete\nquit\n")
        .unwrap();
    let out = dbg.wait_with_output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        out.status.success(),
        "{text}{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let hits = text.lines().filter(|l| *l == "breakpoint 1").count();
    assert_eq!(hits, 2, "{text}");
    assert!(text.contains("=> 0x"), "{text}");
    let bt: Vec<&str> = text
        .lines()
        .skip_while(|l| *l != "(merlin-dbg) bt")
        .skip(1)
        .take_while(|l| !l.starts_with("(merlin-dbg)"))
        .collect();
    assert!(bt[0].starts_with("#0  sys_write (f=0x"), "{text}");
    let line = sys_write_line();
    assert!(bt[0].ends_with(&format!("at kernel/sysfile.c:{line}")), "{text}");
    assert!(bt[1].contains(" in syscall (f=0x"), "{text}");
    // a7 is the system call's number: write, 2.
    assert!(text.contains("\n$1 = 2\n"), "{text}");

    // Left alone, the VM runs on: the shell answers, and goes on answering.
    herald.wait_for("camelot\n", 1);
    herald.wait_for("$ ", 2);
    let start = Instant::now();
    while state(&mut control, id) != VmState::Running {
        assert!(start.elapsed() < SLOW, "the VM is not running again");
        std::thread::sleep(Duration::from_millis(10));
    }
    herald.type_keys(b"echo again\r");
    herald.wait_for("again\n", 1);
    herald.wait_for("$ ", 3);
    // Herald saw the console as if nobody had debugged it, and the state changes.
    let console = herald.output.text();
    assert!(
        console.contains("$ echo camelot\ncamelot\n$ echo again\nagain\n$ "),
        "{console:?}"
    );
    // Paused for the debugger as it attached (QEMU's run states); the run states its
    // continues and breakpoints went through after that come at most once a second
    // (avalon::vm::NOTICE_RATE), so Herald hears of few of them.
    let notices = herald.notices.text();
    assert!(
        notices.starts_with(&format!("herald: VM {id} is paused\n")),
        "{notices}"
    );
    // Paused, at most one a second while the debugger held the VM, the last held one,
    // and the running after it went.
    let most = 3 + attaching.elapsed().as_secs() as usize + 1;
    assert!(notices.lines().count() <= most, "{notices}");
    assert!(
        notices.ends_with(&format!("herald: VM {id} is running\n")),
        "{notices}"
    );
    assert!(herald.done.try_recv().is_err(), "Herald's session ended");
    herald.type_keys(b"exit\r");
    assert_eq!(
        herald.done.recv_timeout(SLOW).unwrap(),
        End::VmEnded(VmState::Exited(0))
    );
    server.stop().unwrap();
}
