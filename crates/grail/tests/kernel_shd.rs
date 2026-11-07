//! `shd`, Camelot's shell server (kernel/user/shd.c), on the user-mode gateway with
//! its port 23 forwarded from a host port (one the system picks): Rust clients on the
//! host each get a shell of their own, which runs commands while the others do, the
//! console's shell included, and ends with its connection. A client that goes away in
//! the middle of a command hangs its session up; one too many is turned away. After
//! every test no process, socket, page or packet buffer is left: shd is killed, the
//! console's shell exits, and the machine stops, which it does only once every
//! process has ended.
//!
//! As in kernel_sockets.rs, the machine runs in this thread by step counts with the
//! gateway on a clock that follows them. The clients are this thread's too, read
//! without blocking between the machine's turns; every wait is for something to happen.

mod common;

use common::*;
use grail::net::{MacAddr, NetBackend};
use grail::MachineConfig;
use std::io::{ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

/// What each session's shell prints first.
const GREETING: &str = "Type help for the list of commands.\n# ";

/// A machine on the user-mode gateway (DHCP gives it 10.0.2.15) with a host port
/// forwarded to its port 23, booted to its prompt; and that host port.
fn shd_vm() -> (Vm, SocketAddr) {
    shd_vm_on(23, gateway::Limits::default().tcp_closing)
}

/// [`shd_vm`] with the host port forwarded to the machine's `port`, and the gateway
/// resetting a connection closed on one side after `tcp_closing` with nothing moving
/// (a minute by default).
fn shd_vm_on(port: u16, tcp_closing: Duration) -> (Vm, SocketAddr) {
    let gw = gateway::Gateway::new(gateway::Config {
        hostfwd: vec![gateway::HostFwd::tcp(0, port)],
        limits: gateway::Limits {
            service_interval: Duration::ZERO,
            tcp_closing,
            ..gateway::Limits::default()
        },
        ..gateway::Config::default()
    })
    .unwrap();
    let addr = SocketAddr::V4(gw.hostfwd()[0].host);
    let backend: Box<dyn NetBackend> = Box::new(SteppedGateway::new(gw));
    let config = MachineConfig {
        ram_size: RAM,
        ..MachineConfig::default()
    };
    let mut vm = Vm::with_config(config, vec![(MacAddr(gateway::DEFAULT_GUEST_MAC), backend)], &[]);
    boot(&mut [&mut vm], None);
    assert!(vm.out().contains("net: 10.0.2.15/24"), "{}", vm.out());
    (vm, addr)
}

/// A client on the host: a connection to the forwarded port, read without blocking.
struct Client {
    stream: Option<TcpStream>,
    got: Vec<u8>,
    /// The far end closed the connection (or reset it).
    closed: bool,
}

impl Client {
    fn connect(addr: SocketAddr) -> Client {
        let stream = TcpStream::connect(addr).unwrap();
        stream.set_nonblocking(true).unwrap();
        Client {
            stream: Some(stream),
            got: Vec::new(),
            closed: false,
        }
    }

    /// Takes what has come.
    fn pump(&mut self) {
        let Some(stream) = self.stream.as_mut() else {
            return;
        };
        let mut buf = [0u8; 1024];
        while !self.closed {
            match stream.read(&mut buf) {
                Ok(0) => self.closed = true,
                Ok(n) => self.got.extend_from_slice(&buf[..n]),
                Err(e) if e.kind() == ErrorKind::WouldBlock => return,
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(_) => self.closed = true,
            }
        }
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.got).into_owned()
    }

    fn send(&mut self, bytes: &[u8]) {
        self.stream.as_mut().unwrap().write_all(bytes).unwrap();
    }

    /// Closes the connection from this end.
    fn close(&mut self) {
        self.stream = None;
    }
}

/// Runs the machine, the clients taking what comes between its turns, until `done`.
fn run(vm: &mut Vm, clients: &mut [&mut Client], done: impl Fn(&Vm, &[&mut Client]) -> bool) {
    let mut steps = 0;
    loop {
        for c in clients.iter_mut() {
            c.pump();
        }
        if done(vm, clients) {
            return;
        }
        vm.m.run_for(SLICE, &mut |_| {});
        assert_eq!(vm.m.exit_code(), None, "the machine stopped:\n{}", vm.out());
        steps += SLICE;
        let texts: Vec<String> = clients.iter().map(|c| c.text()).collect();
        assert!(
            steps < MAX_STEPS,
            "no end in sight:\n{}\nclients: {texts:#?}",
            vm.out()
        );
    }
}

/// Runs until the console's output has `text` `n` times.
fn wait_console(vm: &mut Vm, clients: &mut [&mut Client], text: &str, n: usize) {
    run(vm, clients, |vm, _| vm.out().matches(text).count() >= n);
}

/// A new session: connects and runs until its shell's greeting and prompt.
fn session(vm: &mut Vm, addr: SocketAddr) -> Client {
    let mut c = Client::connect(addr);
    run(vm, &mut [&mut c], |_, c| c[0].got.ends_with(GREETING.as_bytes()));
    assert_eq!(c.text(), GREETING);
    c
}

/// Sends `line` to client `i`'s shell and runs until its next prompt; returns what came
/// before it. The shell echoes nothing: the client does that.
fn command(vm: &mut Vm, clients: &mut [&mut Client], i: usize, line: &str) -> String {
    let before = clients[i].got.len();
    clients[i].send(format!("{line}\n").as_bytes());
    run(vm, clients, |_, c| {
        c[i].got.len() > before && c[i].got.ends_with(PROMPT.as_bytes())
    });
    let text = clients[i].text()[before..].to_string();
    text.strip_suffix(PROMPT).unwrap().to_string()
}

/// A command on the console's shell.
fn console(vm: &mut Vm, line: &str) -> String {
    common::command(&mut [vm], 0, None, line)
}

/// One line of `ps`: pid, parent, state, name.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Proc {
    pid: u32,
    ppid: u32,
    state: String,
    name: String,
}

fn parse_ps(out: &str) -> Vec<Proc> {
    let mut lines = out.lines();
    assert_eq!(
        lines
            .next()
            .map(str::split_whitespace)
            .map(Iterator::collect::<Vec<_>>),
        Some(vec!["PID", "PPID", "STATE", "SIZE", "NAME"]),
        "{out}"
    );
    lines
        .map(|l| {
            let w: Vec<&str> = l.split_whitespace().collect();
            Proc {
                pid: w[0].parse().unwrap(),
                ppid: w[1].parse().unwrap(),
                state: w[2].to_string(),
                name: w[4].to_string(),
            }
        })
        .collect()
}

/// The pid in a background job's `[PID]` line.
fn job_pid(out: &str) -> u32 {
    let line = out
        .lines()
        .find(|l| l.starts_with('['))
        .unwrap_or_else(|| panic!("{out}"));
    line[1..line.find(']').unwrap()].parse().unwrap()
}

/// shd's log lines (-v) in the console's output, with their pids and the clients' ports
/// (both vary) left out: `[] 10.0.2.2: connected`.
fn shd_log(out: &str) -> Vec<String> {
    out.lines()
        .filter_map(|l| l.find("shd: ").map(|at| &l[at + 5..]))
        .filter(|l| l.contains("10.0.2.2:"))
        .map(|l| {
            let words: Vec<&str> = l
                .split(' ')
                .map(|w| match w {
                    _ if w.starts_with('[') && w.ends_with(']') => "[]",
                    _ if w.starts_with("10.0.2.2:") => "10.0.2.2:",
                    _ => w,
                })
                .collect();
            words.join(" ")
        })
        .collect()
}

/// Starts `shd -v ARGS` in the background on the console; returns its pid once it
/// listens.
fn start_shd(vm: &mut Vm, args: &str) -> u32 {
    start_shd_on(vm, args, 23)
}

/// [`start_shd`], with ARGS making it listen on `port`.
fn start_shd_on(vm: &mut Vm, args: &str, port: u16) -> u32 {
    let before = vm.out().len();
    let prompts = vm.prompts();
    vm.type_line(&format!("shd -v{args} &"));
    let listening = format!("shd: listening on port {port}\n");
    run(vm, &mut [], |vm, _| {
        vm.out()[before..].contains(&listening) && vm.prompts() > prompts
    });
    job_pid(&vm.out()[before..])
}

/// Kills shd, collects it, waits for every TCP socket to be free (TIME-WAIT is 60
/// seconds) and every packet buffer back, checks that the free pages are `pages` again,
/// and leaves the console's shell: the machine stops, so no process is left.
fn finish(mut vm: Vm, shd: u32, pages: kstacks::Pages) {
    assert_eq!(console(&mut vm, &format!("kill {shd}")), "");
    assert_eq!(console(&mut vm, "wait"), format!("[{shd}] shd: exit -1\n"));
    let ps = parse_ps(&console(&mut vm, "ps"));
    let names: Vec<&str> = ps.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["sh", "ps"], "{ps:?}");
    let start = vm.read_var("ticks");
    while vm.read_var("tcp_in_use") != 0 {
        vm.m.run_for(SLICE, &mut |_| {});
        assert!(
            vm.read_var("ticks") - start < 700,
            "{} TCP sockets still in use:\n{}",
            vm.read_var("tcp_in_use"),
            vm.out()
        );
    }
    assert_buffers_back(&mut vm);
    assert_eq!(vm.pages(), pages, "pages leaked:\n{}", vm.out());
    vm.type_line("exit 0");
    assert_eq!(
        vm.m.run(Some(MAX_STEPS), |_| {}),
        grail::RunResult::Exited(0),
        "{}",
        vm.out()
    );
}

/// The free pages once the programs these tests run have each run once (whatever the
/// first run of one costs is then paid on both sides of the comparison).
fn warm_up(vm: &mut Vm) -> kstacks::Pages {
    assert_eq!(
        console(vm, "shd -x"),
        "usage: shd [-p PORT] [-n MAX] [-v]\nshd: exit 1\n"
    );
    assert_eq!(console(vm, "sleep 0"), "");
    assert_eq!(console(vm, "echo x | cat"), "x\n");
    parse_ps(&console(vm, "ps"));
    vm.pages()
}

#[test]
fn sessions_run_side_by_side_and_end_with_their_connections() {
    // Issue #117's first and second criteria: two sessions run commands at the same
    // time, one stuck in `sleep 5000` while the other keeps working and the console's
    // shell too; closing a connection ends its shell; a client that disconnects in the
    // middle of a command hangs up, and nothing is left behind.
    let (mut vm, addr) = shd_vm();
    let pages = warm_up(&mut vm);
    let shd = start_shd(&mut vm, "");
    let mut a = session(&mut vm, addr);
    let mut b = session(&mut vm, addr);
    wait_console(&mut vm, &mut [], " connected\n", 2);

    // A sleeps for over an hour; B looks until it sees A's sleep, as the processes go.
    a.send(b"sleep 5000\n");
    let ps = loop {
        let ps = parse_ps(&command(&mut vm, &mut [&mut a, &mut b], 1, "ps"));
        if ps.iter().any(|p| p.name == "sleep") {
            break ps;
        }
    };
    let find = |name: &str, ppid: u32| -> Vec<&Proc> {
        ps.iter().filter(|p| p.name == name && p.ppid == ppid).collect()
    };
    let console_sh = find("sh", 0);
    assert_eq!(console_sh.len(), 1, "{ps:#?}");
    assert_eq!(find("shd", console_sh[0].pid)[0].pid, shd, "{ps:#?}");
    let shells = find("sh", shd);
    assert_eq!(shells.len(), 2, "{ps:#?}");
    let (sh_a, sh_b) = (shells[0].pid, shells[1].pid);
    assert_eq!(find("sleep", sh_a).len(), 1, "{ps:#?}");
    assert_eq!(find("ps", sh_b)[0].state, "run", "{ps:#?}");
    assert_eq!(ps.len(), 6, "{ps:#?}");

    // B carries on: programs, a pipeline, errors on its own connection (stderr too),
    // built-ins, and two lines sent at once, each run in turn.
    let mut clients = [&mut a, &mut b];
    assert_eq!(
        command(&mut vm, &mut clients, 1, "echo hello from b"),
        "hello from b\n"
    );
    assert_eq!(command(&mut vm, &mut clients, 1, "echo one two | wc"), "1 2 8\n");
    assert_eq!(
        command(&mut vm, &mut clients, 1, "nosuch"),
        "sh: nosuch: no such program\n"
    );
    assert_eq!(
        command(&mut vm, &mut clients, 1, "help").lines().next(),
        Some("help            this list")
    );
    // A telnet client's line end.
    assert_eq!(command(&mut vm, &mut clients, 1, "echo crlf\r"), "crlf\n");
    let before = clients[1].got.len();
    clients[1].send(b"echo first\necho second\n");
    run(&mut vm, &mut clients, |_, c| {
        c[1].text()[before..].matches(PROMPT).count() == 2
    });
    assert_eq!(&clients[1].text()[before..], "first\n# second\n# ");
    // A program reads the same connection: what follows its line is its input, none of
    // it taken by the shell. It is killed from the console.
    let before = clients[1].got.len();
    clients[1].send(b"cat\nfor cat\n");
    run(&mut vm, &mut clients, |_, c| {
        c[1].text()[before..].contains("for cat\n")
    });
    let ps = parse_ps(&console(&mut vm, "ps"));
    let cat = ps.iter().find(|p| p.name == "cat" && p.ppid == sh_b).unwrap().pid;
    assert_eq!(console(&mut vm, &format!("kill {cat}")), "");
    run(&mut vm, &mut [&mut a, &mut b], |_, c| {
        c[1].got.ends_with(PROMPT.as_bytes())
    });
    assert_eq!(&b.text()[before..], "for cat\ncat: exit -1\n# ");

    // The console is unaffected throughout.
    assert_eq!(console(&mut vm, "echo console"), "console\n");
    let ps = parse_ps(&console(&mut vm, "ps"));
    assert_eq!(ps.iter().filter(|p| p.name == "sh").count(), 3, "{ps:#?}");

    // A is still asleep: nothing more came on its connection.
    assert_eq!(a.text(), GREETING);

    // B closes its connection between commands: its shell reads the end of its input
    // and ends, with the status of its last program, the killed cat.
    b.close();
    wait_console(&mut vm, &mut [&mut a], " ended, exit -1\n", 1);
    // A goes away in the middle of `sleep 5000`: shd hangs the session up, killing the
    // sleep; its shell then ends.
    a.close();
    wait_console(&mut vm, &mut [], " ended, exit -1\n", 2);
    let ps = parse_ps(&console(&mut vm, "ps"));
    let names: Vec<&str> = ps.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["sh", "shd", "ps"], "{ps:#?}");

    // C leaves between commands too.
    let mut c = session(&mut vm, addr);
    assert_eq!(command(&mut vm, &mut [&mut c], 0, "echo c"), "c\n");
    c.close();
    wait_console(&mut vm, &mut [], " ended, exit 0\n", 1);
    // D's shell ends with `exit 3`, a background program still running: the connection
    // ends with the shell, for the client too.
    let mut d = session(&mut vm, addr);
    let job = command(&mut vm, &mut [&mut d], 0, "sleep 1 &");
    job_pid(&job);
    d.send(b"exit 3\n");
    run(&mut vm, &mut [&mut d], |_, d| d[0].closed);
    assert_eq!(d.text(), format!("{GREETING}{job}# "));
    wait_console(&mut vm, &mut [], " ended, exit 3\n", 1);

    assert_eq!(
        shd_log(&vm.out()),
        [
            "[] 10.0.2.2: connected",
            "[] 10.0.2.2: connected",
            "[] 10.0.2.2: hung up",
            "[] 10.0.2.2: ended, exit -1",
            "[] 10.0.2.2: hung up",
            "[] 10.0.2.2: ended, exit -1",
            "[] 10.0.2.2: connected",
            "[] 10.0.2.2: hung up",
            "[] 10.0.2.2: ended, exit 0",
            "[] 10.0.2.2: connected",
            "[] 10.0.2.2: ended, exit 3",
        ]
    );

    // D's `sleep 1` outlives its shell, as a background job does: wait for it.
    loop {
        let ps = parse_ps(&console(&mut vm, "ps"));
        if !ps.iter().any(|p| p.name == "sleep") {
            break;
        }
    }
    finish(vm, shd, pages);
}

#[test]
fn the_session_limit_turns_connections_away() {
    // Issue #117's second criterion, the limit: with -n 1, a second connection is told
    // so and closed while the first session lasts, and gets in once it has ended.
    let (mut vm, addr) = shd_vm();
    let pages = warm_up(&mut vm);
    let shd = start_shd(&mut vm, " -n 1");
    let mut a = session(&mut vm, addr);
    let mut b = Client::connect(addr);
    run(&mut vm, &mut [&mut a, &mut b], |_, c| c[1].closed);
    assert_eq!(b.text(), "shd: too many sessions (1), try again later\n");
    assert_eq!(
        command(&mut vm, &mut [&mut a], 0, "echo still here"),
        "still here\n"
    );
    a.send(b"exit\n");
    run(&mut vm, &mut [&mut a], |_, c| c[0].closed);
    wait_console(&mut vm, &mut [], " ended, exit 0\n", 1);
    let mut c = session(&mut vm, addr);
    assert_eq!(command(&mut vm, &mut [&mut c], 0, "echo in"), "in\n");
    c.send(b"exit 0\n");
    run(&mut vm, &mut [&mut c], |_, c| c[0].closed);
    wait_console(&mut vm, &mut [], " ended, exit 0\n", 2);
    assert_eq!(
        shd_log(&vm.out()),
        [
            "[] 10.0.2.2: connected",
            "10.0.2.2: turned away: too many sessions",
            "[] 10.0.2.2: ended, exit 0",
            "[] 10.0.2.2: connected",
            "[] 10.0.2.2: ended, exit 0",
        ]
    );
    // Bad options.
    for args in ["-n 0", "-n 9", "-p 0", "-p", "-q"] {
        assert_eq!(
            console(&mut vm, &format!("shd {args}")),
            "usage: shd [-p PORT] [-n MAX] [-v]\nshd: exit 1\n",
            "{args}"
        );
    }
    // The port is taken: by shd itself.
    assert_eq!(
        console(&mut vm, "shd"),
        "shd: port 23: address in use\nshd: exit 1\n"
    );
    finish(vm, shd, pages);
}

/// Runs console `ps` until no process is named `name`; a few tries, each a command's
/// worth of time, are plenty for a killed process to end.
fn wait_gone(vm: &mut Vm, name: &str) {
    for _ in 0..20 {
        let ps = parse_ps(&console(vm, "ps"));
        if !ps.iter().any(|p| p.name == name) {
            return;
        }
    }
    panic!("a {name} is still there:\n{}", vm.out());
}

#[test]
fn a_hang_up_ends_background_jobs_and_typed_ahead_sessions() {
    // shd on another port (-p), forwarded to. The gateway gives up on a half-closed
    // connection where nothing moves after 3 seconds rather than a minute, to keep the
    // test short.
    let (mut vm, addr) = shd_vm_on(2323, Duration::from_secs(3));
    let pages = warm_up(&mut vm);
    let shd = start_shd_on(&mut vm, " -p 2323", 2323);

    // A starts a background job and leaves between commands: its shell reads the end
    // of its input at once and ends, too soon for shd to find the job by its parent,
    // so the shell kills it itself. Nothing of the session is left.
    let mut a = session(&mut vm, addr);
    wait_console(&mut vm, &mut [], " connected\n", 1);
    let job = command(&mut vm, &mut [&mut a], 0, "sleep 5000 &");
    let sleep_pid = job_pid(&job);
    let ps = parse_ps(&console(&mut vm, "ps"));
    assert!(
        ps.iter().any(|p| p.pid == sleep_pid && p.name == "sleep"),
        "{ps:#?}"
    );
    a.close();
    wait_console(&mut vm, &mut [], " ended, exit 0\n", 1);
    wait_gone(&mut vm, "sleep");
    let ps = parse_ps(&console(&mut vm, "ps"));
    let names: Vec<&str> = ps.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["sh", "shd", "ps"], "{ps:#?}");

    // B types `ls` while `sleep 5000` runs, then leaves: the line waits unread, so
    // after B's FIN the session looks like a script that still reads. Nothing moves on
    // the connection, and after a while (here 3 s) the gateway resets it: that drops the unread
    // line, shd hangs up and kills the sleep, and the shell, reading the reset, ends
    // without running `ls` (its status is still the killed sleep's).
    let mut b = session(&mut vm, addr);
    wait_console(&mut vm, &mut [], " connected\n", 2);
    b.send(b"sleep 5000\nls\n");
    loop {
        let ps = parse_ps(&console(&mut vm, "ps"));
        if ps.iter().any(|p| p.name == "sleep") {
            break;
        }
    }
    b.close();
    wait_console(&mut vm, &mut [], " ended, exit -1\n", 1);
    wait_gone(&mut vm, "sleep");
    let ps = parse_ps(&console(&mut vm, "ps"));
    let names: Vec<&str> = ps.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["sh", "shd", "ps"], "{ps:#?}");

    assert_eq!(
        shd_log(&vm.out()),
        [
            "[] 10.0.2.2: connected",
            "[] 10.0.2.2: hung up",
            "[] 10.0.2.2: ended, exit 0",
            "[] 10.0.2.2: connected",
            "[] 10.0.2.2: hung up",
            "[] 10.0.2.2: ended, exit -1",
        ]
    );
    finish(vm, shd, pages);
}

/// Turns a shell may outlive its client by, alone, before shd kills it, and how long a
/// turn is in timer ticks (`LINGER_TURNS` and `TEND_USEC` in kernel/user/shd.c; the
/// kernel ticks 10 times a second and rounds SO_RCVTIMEO up to whole ticks).
const LINGER_TURNS: u32 = 2;
const TURN_TICKS: u32 = 5;

#[test]
fn a_shell_left_alone_after_its_client_has_gone_is_killed() {
    // Issue #127: the session's shell outlives its client with nothing under it for
    // shd to kill, and neither reads its input nor ends: here it has replaced itself
    // with `exec sleep 5000` (POSIX's exec, kernel/user/sh.c), so the session's
    // process is a sleep with no children. Once the client has closed its side, shd
    // finds the connection gone with every byte read and no descendant to kill,
    // waits LINGER_TURNS turns, then logs the hang-up and kills the process itself;
    // the session ends with the killed status, and nothing of it is left behind.
    let (mut vm, addr) = shd_vm();
    let pages = warm_up(&mut vm);
    let shd = start_shd(&mut vm, "");
    let mut a = session(&mut vm, addr);
    wait_console(&mut vm, &mut [], " connected\n", 1);
    a.send(b"exec sleep 5000\n");
    let (session_pid, ps) = (0..20)
        .find_map(|_| {
            let ps = parse_ps(&console(&mut vm, "ps"));
            let pid = ps.iter().find(|p| p.ppid == shd && p.name == "sleep")?.pid;
            Some((pid, ps))
        })
        .unwrap_or_else(|| panic!("the session never became a sleep:\n{}", vm.out()));
    // The sleep is the session's process, shd's child, with nothing under it: no
    // shell is left but the console's.
    assert!(!ps.iter().any(|p| p.ppid == session_pid), "{ps:#?}");
    assert_eq!(ps.iter().filter(|p| p.name == "sh").count(), 1, "{ps:#?}");
    // exec printed nothing: no prompt, no error.
    a.pump();
    assert_eq!(a.text(), GREETING);

    let closed_at = vm.read_var("ticks");
    a.close();
    wait_console(&mut vm, &mut [], " hung up\n", 1);
    let lingered = vm.read_var("ticks") - closed_at;
    assert!(
        lingered >= LINGER_TURNS * TURN_TICKS,
        "hung up {lingered} ticks after the client left:\n{}",
        vm.out()
    );
    wait_console(&mut vm, &mut [], " ended, exit -1\n", 1);
    assert_eq!(
        shd_log(&vm.out()),
        [
            "[] 10.0.2.2: connected",
            "[] 10.0.2.2: hung up",
            "[] 10.0.2.2: ended, exit -1",
        ]
    );
    wait_gone(&mut vm, "sleep");
    let ps = parse_ps(&console(&mut vm, "ps"));
    let names: Vec<&str> = ps.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["sh", "shd", "ps"], "{ps:#?}");
    finish(vm, shd, pages);
}
