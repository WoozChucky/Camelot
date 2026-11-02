//! The `pendragon` binary: its usage, its mistakes, and a real launch that prints the
//! URL to open and serves it.

mod common;

use common::*;
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};

fn pendragon() -> Command {
    Command::new(env!("CARGO_BIN_EXE_pendragon"))
}

#[test]
fn help_and_mistakes() {
    let out = pendragon().arg("--help").output().unwrap();
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    assert_eq!(text, format!("{}\n", pendragon::USAGE));

    for (args, why) in [
        (&["--bogus"][..], "unknown option '--bogus'"),
        (&["--listen"][..], "--listen needs a value"),
        (&["--avalon", "a:b:c"][..], "--avalon: "),
        (&["--avalon-max-ram", "0"][..], "--avalon-max-ram: "),
    ] {
        let out = pendragon().args(args).output().unwrap();
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        let err = String::from_utf8(out.stderr).unwrap();
        assert!(err.starts_with(&format!("pendragon: {why}")), "{err}");
        assert!(err.contains("pendragon --help"), "{err}");
    }

    // An address already taken.
    let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = taken.local_addr().unwrap().to_string();
    let out = pendragon().args(["--listen", &addr]).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8(out.stderr)
        .unwrap()
        .starts_with(&format!("pendragon: cannot listen on {addr}")));
}

/// Kills the child when dropped, whatever the test does.
struct Running(std::process::Child);

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn serves_the_url_it_prints() {
    let dir = TempDir::new("cli");
    let lake = lake::Server::bind(&lake::Config {
        listen: "127.0.0.1:0".parse().unwrap(),
        log: false,
        ..lake::Config::new(dir.path())
    })
    .unwrap()
    .spawn()
    .unwrap();
    let lake_addr = lake.addr().to_string();
    let avalon = hang_up_addr();
    let mut child = Running(
        pendragon()
            .args([
                "--listen",
                "127.0.0.1:0",
                "--avalon",
                &avalon,
                "--lake",
                &lake_addr,
            ])
            .args(["--avalon-max-ram", "512"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let mut line = String::new();
    BufReader::new(child.0.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    let url = line.trim();
    let rest = url
        .strip_prefix("http://127.0.0.1:")
        .unwrap_or_else(|| panic!("{url}"));
    let (port, token) = rest.split_once("/?token=").unwrap();
    assert_eq!(token.len(), 64);
    let addr = format!("127.0.0.1:{port}").parse().unwrap();

    assert_eq!(request(addr, "GET", "/", &[], b"").status, 200);
    let api = Api {
        addr,
        token: token.to_string(),
    };
    let o = api.expect(200, api.get("overview"));
    assert_eq!(s(o.get("lake").unwrap(), "addr"), lake_addr);
    assert_eq!(
        o.get("lake").unwrap().get("ok"),
        Some(&pendragon::json::Json::Bool(true))
    );
    assert_eq!(
        o.get("avalon").unwrap().get("ok"),
        Some(&pendragon::json::Json::Bool(false))
    );
    api.expect(
        201,
        api.post("disks", &obj([("name", "cli".into()), ("size", "4K".into())])),
    );
    assert!(dir.path().join("cli").exists());
    assert_eq!(api.get("disks").status, 200);
    // The token is the launch's: a made-up one is refused.
    let wrong = Api {
        addr,
        token: "0".repeat(64),
    };
    assert_eq!(wrong.get("overview").status, 403);

    drop(child);
    lake.stop().unwrap();
}
