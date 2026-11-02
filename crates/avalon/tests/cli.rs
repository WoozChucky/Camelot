//! The `avalon` binary: flags, serving a client, and a graceful shutdown on Ctrl-C
//! (Windows: Ctrl-Break sent to its process group) or SIGTERM (Unix).

mod common;

use avalon_proto::{Connection, Message, VmRef, VmState};
use common::*;
use std::io::{BufRead, BufReader, Read};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

fn avalon() -> Command {
    Command::new(env!("CARGO_BIN_EXE_avalon"))
}

#[test]
fn help_and_bad_flags() {
    let out = avalon().arg("--help").output().unwrap();
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(
        text.starts_with("usage: avalon [--listen ADDR] [--max-ram MiB]"),
        "{text}"
    );
    for args in [
        &["--bogus"][..],
        &["--listen"],
        &["--listen", "not an address"],
        &["--max-ram", "0"],
        &["--max-ram", "lots"],
    ] {
        let out = avalon().args(args).output().unwrap();
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        let err = String::from_utf8(out.stderr).unwrap();
        assert!(err.starts_with("avalon: "), "{args:?}: {err}");
    }
    // The port is taken.
    let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let out = avalon()
        .args(["--listen", &taken.local_addr().unwrap().to_string()])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("cannot listen"));
}

/// Starts `avalon` on an ephemeral port and reads the address it listens on from its
/// first line of output.
fn start(extra: &[&str]) -> (Child, String, BufReader<std::process::ChildStderr>) {
    let mut cmd = avalon();
    cmd.args(["--listen", "127.0.0.1:0"])
        .args(extra)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // Its own process group, so a Ctrl-Break can be sent to it alone.
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        cmd.creation_flags(CREATE_NEW_PROCESS_GROUP);
    }
    let mut child = cmd.spawn().unwrap();
    let mut stderr = BufReader::new(child.stderr.take().unwrap());
    let mut line = String::new();
    stderr.read_line(&mut line).unwrap();
    let addr = line
        .trim()
        .strip_prefix("avalon: listening on ")
        .unwrap_or_else(|| panic!("{line}"))
        .to_string();
    (child, addr, stderr)
}

/// Asks the server to shut down as Ctrl-C would. False if this host cannot.
fn interrupt(child: &Child) -> bool {
    #[cfg(windows)]
    {
        #[link(name = "kernel32")]
        extern "system" {
            fn GenerateConsoleCtrlEvent(event: u32, group: u32) -> i32;
        }
        const CTRL_BREAK_EVENT: u32 = 1;
        assert_ne!(child.id(), 0);
        // SAFETY: sends Ctrl-Break to the child's own process group (never group 0,
        // which would be every process on this console, the tests included). Fails
        // when the tests run without a console.
        unsafe { GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, child.id()) != 0 }
    }
    #[cfg(unix)]
    {
        extern "C" {
            fn kill(pid: i32, sig: i32) -> i32;
        }
        const SIGTERM: i32 = 15;
        // SAFETY: a plain signal to our own child.
        unsafe { kill(child.id() as i32, SIGTERM) == 0 }
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = child;
        false
    }
}

#[test]
fn serves_clients_and_shuts_down_gracefully() {
    let (mut child, addr, mut stderr) = start(&["--max-ram", "8"]);
    let mut rt = Connection::connect(addr.as_str(), "test").unwrap();
    rt.get_ref().set_read_timeout(Some(SLOW)).unwrap();
    let id = create(&mut rt, "spin", profile(8, None), spinner());
    assert_eq!(
        error_of(
            &mut rt,
            &Message::CreateVm {
                name: "more".into(),
                profile: profile(1, None),
                program: spinner(),
            }
        ),
        avalon_proto::ErrorCode::OverBudget
    );
    rt.request_ok(&Message::StartVm { vm: VmRef::Id(id) }).unwrap();
    assert_eq!(state(&mut rt, id), VmState::Running);

    if !interrupt(&child) {
        eprintln!("cannot send Ctrl-C here; stopping avalon the hard way");
        child.kill().unwrap();
        child.wait().unwrap();
        return;
    }
    let start = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if start.elapsed() > SLOW {
            child.kill().unwrap();
            let mut log = String::new();
            stderr.read_to_string(&mut log).unwrap();
            panic!("avalon did not shut down: {log}");
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    assert!(status.success(), "{status:?}");
    let mut log = String::new();
    stderr.read_to_string(&mut log).unwrap();
    assert!(log.contains("shutting down"), "{log}");
    assert!(log.contains("VM 1 ('spin') stopped"), "{log}");
    assert!(log.contains("shut down (1 VMs stopped)"), "{log}");
    // The connection was closed.
    assert!(rt.request(&Message::ListVms).is_err());
}
