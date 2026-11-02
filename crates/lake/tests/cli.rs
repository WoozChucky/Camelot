//! The `lake` binary: flags, and serving a client from a directory.

mod common;

use avalon_proto::{Connection, DiskInfo, Message};
use common::*;
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};

fn lake_bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_lake"))
}

#[test]
fn help_and_bad_flags() {
    let out = lake_bin().arg("--help").output().unwrap();
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(
        text.starts_with("usage: lake [--listen ADDR] --dir <images>"),
        "{text}"
    );
    for args in [
        &[][..],
        &["--bogus"],
        &["--dir"],
        &["--dir", ""],
        &["--listen", "127.0.0.1:0"],
        &["--dir", "x", "--listen"],
        &["--dir", "x", "--listen", "not an address"],
    ] {
        let out = lake_bin().args(args).output().unwrap();
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        let err = String::from_utf8(out.stderr).unwrap();
        assert!(err.starts_with("lake: "), "{args:?}: {err}");
    }

    // The port is taken.
    let dir = TempDir::new("cli-taken");
    let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let out = lake_bin()
        .arg("--dir")
        .arg(dir.path())
        .args(["--listen", &taken.local_addr().unwrap().to_string()])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("cannot serve"));

    // The directory is a file.
    let dir = TempDir::new("cli-file");
    std::fs::write(dir.path(), b"not a directory").unwrap();
    let out = lake_bin()
        .arg("--dir")
        .arg(dir.path())
        .args(["--listen", "127.0.0.1:0"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    std::fs::remove_file(dir.path()).unwrap();
}

#[test]
fn serves_a_directory() {
    let dir = TempDir::new("cli-serve");
    let mut child = lake_bin()
        .arg("--dir")
        .arg(dir.path().join("images"))
        .args(["--listen", "127.0.0.1:0"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stderr = BufReader::new(child.stderr.take().unwrap());
    let mut line = String::new();
    stderr.read_line(&mut line).unwrap();
    let addr = line
        .trim()
        .strip_prefix("lake: listening on ")
        .unwrap_or_else(|| panic!("{line}"))
        .to_string();
    line.clear();
    stderr.read_line(&mut line).unwrap();
    assert!(line.starts_with("lake: images in "), "{line}");

    let mut rt = Connection::connect(addr.as_str(), "test").unwrap();
    rt.get_ref().set_read_timeout(Some(SLOW)).unwrap();
    create_disk(&mut rt, "root", 64);
    assert_eq!(
        list(&mut rt),
        [DiskInfo {
            name: "root".into(),
            sectors: 64,
            readers: 0,
            writer: false
        }]
    );
    assert_eq!(
        std::fs::metadata(dir.path().join("images/root")).unwrap().len(),
        64 * 512
    );
    rt.request_ok(&Message::DiskDelete { name: "root".into() })
        .unwrap();
    child.kill().unwrap();
    child.wait().unwrap();
}
