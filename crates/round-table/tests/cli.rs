//! The `round-table` binary: usage, exit status, and messages on stderr.

mod common;

use common::*;
use std::process::{Command, Output};

fn round_table(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_round-table"))
        .args(args)
        .output()
        .unwrap()
}

fn stdout(out: &Output) -> String {
    String::from_utf8(out.stdout.clone()).unwrap()
}

fn stderr(out: &Output) -> String {
    // Windows' console newlines are not involved: output goes to a pipe.
    String::from_utf8(out.stderr.clone()).unwrap()
}

#[test]
fn usage() {
    let out = round_table(&["--help"]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(stdout(&out), format!("{}\n", round_table::USAGE));
    assert!(stdout(&out).starts_with("usage: round-table [--server ADDR] <command> [arguments]\n"));

    // Nothing at all: the usage, on stderr.
    let out = round_table(&[]);
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(stderr(&out), format!("{}\n", round_table::USAGE));
    assert!(out.stdout.is_empty());

    for args in [
        &["bogus"][..],
        &["list", "--bogus"],
        &["create", "prog.elf"],
        &["create", "--name", "x", "--clock", "0", "prog.elf"],
        &["--server", "no where", "list"],
        &["disk", "create", "d", "--size", "7"],
    ] {
        let out = round_table(args);
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        let err = stderr(&out);
        assert!(err.starts_with("round-table: "), "{args:?}: {err}");
        assert!(
            err.ends_with("\nRun 'round-table --help' for the commands and options.\n"),
            "{args:?}: {err}"
        );
        assert!(out.stdout.is_empty());
    }
}

#[test]
fn against_avalon() {
    let server = avalon(256);
    let addr = server.addr().to_string();
    let echo = example_str("uart-echo");

    let out = round_table(&["--server", &addr, "create", "--name", "cli", "--ram", "16", &echo]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(stdout(&out), "created VM 1 (cli)\n");
    assert!(out.stderr.is_empty());

    let out = round_table(&["list", "--server", &addr]);
    assert_eq!(
        stdout(&out),
        "\
ID  NAME  STATE    RAM     CLOCK       DISKS
1   cli   created  16 MiB  full speed  0
"
    );
    let out = round_table(&["--server", &addr, "start", "cli"]);
    assert_eq!(
        (out.status.code(), stdout(&out)),
        (Some(0), "started VM cli\n".into())
    );

    // A refusal: exit status 1, the server's reason on stderr.
    let out = round_table(&["--server", &addr, "start", "cli"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(out.stdout.is_empty());
    let err = stderr(&out);
    assert!(err.starts_with("round-table: invalid state: "), "{err}");
    assert_eq!(err.lines().count(), 1, "{err}");

    let out = round_table(&["--server", &addr, "info", "nope"]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(stderr(&out), "round-table: no such VM: no VM named 'nope'\n");

    let out = round_table(&["--server", &addr, "destroy", "1"]);
    assert_eq!(
        (out.status.code(), stdout(&out)),
        (Some(0), "destroyed VM 1\n".into())
    );
    server.stop().unwrap();

    // No server: exit status 1.
    let dead = dead_addr().to_string();
    let out = round_table(&["--server", &dead, "list"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).starts_with(&format!("round-table: cannot connect to Avalon at {dead}: ")),
        "{}",
        stderr(&out)
    );
}

#[test]
fn against_lake() {
    let lake = FakeLake::start();
    let addr = lake.addr.to_string();
    let out = round_table(&["--lake", &addr, "disk", "create", "scratch", "--size", "1G"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(stdout(&out), "created disk scratch (1 GiB, 2097152 sectors)\n");
    let out = round_table(&["--lake", &addr, "disk", "list"]);
    assert_eq!(
        stdout(&out),
        "NAME     SIZE   SECTORS  ATTACHED\nscratch  1 GiB  2097152  no\n"
    );
    let out = round_table(&["--lake", &addr, "disk", "delete", "gone"]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(stderr(&out), "round-table: no such disk: no disk named 'gone'\n");
}
