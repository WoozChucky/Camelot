//! Round Table's VM commands against an Avalon server running in process: every
//! command's output, the state transitions it causes (checked over a separate
//! protocol connection), and the errors a server gives back.

mod common;

use avalon_client::{Client, VmAction};
use avalon_proto::{ErrorCode, VmRef, VmState};
use common::*;
use round_table::Failure;
use std::time::Duration;

/// The device table of Grail's machine without disks, as `info` shows it.
const DEVICES: &str = "\
devices  9
  test      0x00100000 + 0x00001000
  CLINT     0x02000000 + 0x00010000
  PLIC      0x0c000000 + 0x04000000
  UART      0x10000000 + 0x00000100  irq 1
  fb-ctrl   0x30000000 + 0x00001000
  keyboard  0x30001000 + 0x00001000  irq 2
  sysinfo   0x30002000 + 0x00001000
  pointer   0x30003000 + 0x00001000  irq 11
  fb-vram   0x31000000 + 0x0012c000
";

#[test]
fn life_cycle() {
    let server = avalon(512);
    let addr = server.addr();
    assert_eq!(ok(addr, &["list"]), "no VMs\n");

    // uart-echo sleeps until input arrives, so it runs until it is stopped; sysinfo
    // prints its machine and exits.
    let echo = example_str("uart-echo");
    let sysinfo = example_str("sysinfo");
    assert_eq!(
        ok(
            addr,
            &["create", "--name", "echo", "--ram", "64", "--clock", "1MHz", "--harts", "2", &echo]
        ),
        "created VM 1 (echo)\n"
    );
    assert_eq!(state(addr, "echo"), VmState::Created);
    assert_eq!(
        ok(addr, &["create", &sysinfo, "--name=sys", "--ram=32", "--start"]),
        "created VM 2 (sys)\nstarted VM 2 (sys)\n"
    );
    wait_for_state(addr, "sys", |s| s == VmState::Exited(0));

    assert_eq!(
        ok(addr, &["list"]),
        "\
ID  NAME  STATE      RAM     CLOCK       DISKS
1   echo  created    64 MiB  1 MHz       0
2   sys   exited(0)  32 MiB  full speed  0
"
    );
    assert_eq!(
        ok(addr, &["info", "echo"]),
        format!(
            "\
id       1
name     echo
state    created
RAM      64 MiB
clock    1 MHz (real time)
harts    2
steps    0
disks    none
{DEVICES}"
        )
    );
    let sys = ok(addr, &["info", "2"]);
    assert!(
        sys.starts_with("id       2\nname     sys\nstate    exited(0)\nRAM      32 MiB\n"),
        "{sys}"
    );
    assert!(
        sys.contains("\nclock    full speed (reports 10 MHz to the guest)\n"),
        "{sys}"
    );
    assert!(sys.ends_with(DEVICES), "{sys}");

    // created -> running -> paused -> running -> stopped, by name and by id.
    assert!(refused(addr, &["pause", "echo"]).starts_with("invalid state: "));
    assert!(refused(addr, &["resume", "echo"]).starts_with("invalid state: "));
    assert_eq!(ok(addr, &["start", "echo"]), "started VM echo\n");
    assert_eq!(state(addr, "1"), VmState::Running);
    assert!(refused(addr, &["start", "1"]).starts_with("invalid state: "));
    assert_eq!(ok(addr, &["pause", "1"]), "paused VM 1\n");
    assert_eq!(state(addr, "echo"), VmState::Paused);
    let paused_at = steps(addr, "echo");
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(steps(addr, "echo"), paused_at, "a paused VM runs no instructions");
    assert!(ok(addr, &["info", "echo"]).contains("\nstate    paused\n"));
    assert_eq!(ok(addr, &["resume", "echo"]), "resumed VM echo\n");
    assert_eq!(state(addr, "echo"), VmState::Running);
    assert_eq!(ok(addr, &["stop", "echo"]), "stopped VM echo\n");
    assert_eq!(state(addr, "echo"), VmState::Stopped);
    assert!(refused(addr, &["start", "echo"]).starts_with("invalid state: "));
    assert!(refused(addr, &["stop", "sys"]).starts_with("invalid state: "));

    // Destroying frees the name.
    assert_eq!(ok(addr, &["destroy", "echo"]), "destroyed VM echo\n");
    assert_eq!(refused(addr, &["info", "echo"]), "no such VM: no VM named 'echo'");
    assert_eq!(
        ok(addr, &["list"]),
        "\
ID  NAME  STATE      RAM     CLOCK       DISKS
2   sys   exited(0)  32 MiB  full speed  0
"
    );
    assert_eq!(
        ok(addr, &["create", "--name", "echo", &echo]),
        "created VM 3 (echo)\n"
    );
    assert!(ok(addr, &["list"]).contains("\n3   echo  created    128 MiB  full speed  0\n"));
    assert_eq!(ok(addr, &["destroy", "2"]), "destroyed VM 2\n");
    assert_eq!(ok(addr, &["destroy", "3"]), "destroyed VM 3\n");
    assert_eq!(ok(addr, &["list"]), "no VMs\n");
    server.stop().unwrap();
}

#[test]
fn disks_are_attached_at_create() {
    let server = avalon(512);
    let addr = server.addr();
    let dir = std::env::temp_dir().join(format!("round-table-disks-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let image = dir.join("disk.img");
    let examples = example("disk").parent().unwrap().to_path_buf();
    std::fs::copy(examples.join("disk.img"), &image).unwrap();
    let image = image.to_str().unwrap();

    // examples/disk reads the first disk, writes and restores its last sector, and
    // exits with 0 when everything worked.
    assert_eq!(
        ok(
            addr,
            &[
                "create",
                "--name",
                "disky",
                "--disk",
                image,
                "--start",
                &example_str("disk")
            ]
        ),
        "created VM 1 (disky)\nstarted VM 1 (disky)\n"
    );
    assert_eq!(
        wait_for_state(addr, "disky", |s| s != VmState::Running),
        VmState::Exited(0)
    );
    let info = ok(addr, &["info", "disky"]);
    assert!(
        info.contains(&format!("\ndisks    {image} (read-write)\ndevices  10\n")),
        "{info}"
    );
    assert!(info.contains("\n  block     0x"), "{info}");
    assert!(ok(addr, &["list"]).contains("\n1   disky  exited(0)  128 MiB  full speed  1\n"));

    // Two disks, in order, the second read-only.
    ok(
        addr,
        &[
            "create",
            "--name",
            "two",
            "--disk-ro",
            image,
            "--disk-ro",
            image,
            &example_str("disk"),
        ],
    );
    let info = ok(addr, &["info", "two"]);
    assert!(
        info.contains(&format!(
            "\ndisks    {image} (read-only)\n         {image} (read-only)\ndevices  11\n"
        )),
        "{info}"
    );

    // A disk image Avalon cannot open is a bad profile.
    let missing = dir.join("missing.img");
    let why = refused(
        addr,
        &[
            "create",
            "--name",
            "nodisk",
            "--disk",
            missing.to_str().unwrap(),
            &example_str("disk"),
        ],
    );
    assert!(why.starts_with("invalid hardware profile: "), "{why}");

    // So is a Lake disk on a server nobody runs (tests/lake.rs has a real one).
    let nobody = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap();
    let source = format!("lake://{nobody}/data");
    let why = refused(
        addr,
        &[
            "create",
            "--name",
            "lakey",
            "--disk",
            &source,
            &example_str("disk"),
        ],
    );
    assert!(why.starts_with("invalid hardware profile: "), "{why}");
    assert!(why.contains(&source), "{why}");
    assert!(refused(addr, &["info", "lakey"]).starts_with("no such VM: "));
    server.stop().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

/// A VM holds its disks while its machine exists: from create until it is stopped,
/// its program exits, or it is destroyed (docs/avalon-protocol.md, 6.6). A stopped or
/// exited VM still lists them.
#[test]
fn disks_are_released_when_the_machine_ends() {
    let server = avalon(512);
    let addr = server.addr();
    let dir = std::env::temp_dir().join(format!("round-table-release-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let source = example("disk").parent().unwrap().join("disk.img");
    let images: Vec<_> = ["run.img", "idle.img", "exit.img", "gone.img"]
        .iter()
        .map(|name| {
            let image = dir.join(name);
            std::fs::copy(&source, &image).unwrap();
            image
        })
        .collect();
    let [run, idle, exit, gone] = &images[..] else {
        unreachable!()
    };
    let arg = |p: &std::path::Path| p.to_str().unwrap().to_string();
    let echo = example_str("uart-echo");

    // Stopped while running.
    ok(addr, &["create", "--name", "run", "--disk", &arg(run), &echo]);
    wait_for_open(run, true);
    ok(addr, &["start", "run"]);
    wait_for_open(run, true);
    ok(addr, &["stop", "run"]);
    wait_for_open(run, false);
    assert!(ok(addr, &["list"]).contains("\n1   run   stopped  128 MiB  full speed  1\n"));
    let info = ok(addr, &["info", "run"]);
    assert!(
        info.contains(&format!("\ndisks    {} (read-write)\n", arg(run))),
        "{info}"
    );

    // Stopped before it ever ran.
    ok(
        addr,
        &["create", "--name", "idle", "--disk-ro", &arg(idle), &echo],
    );
    wait_for_open(idle, true);
    ok(addr, &["stop", "idle"]);
    wait_for_open(idle, false);

    // Its program exited.
    ok(
        addr,
        &[
            "create",
            "--name",
            "exit",
            "--disk",
            &arg(exit),
            "--start",
            &example_str("disk"),
        ],
    );
    wait_for_state(addr, "exit", |s| s == VmState::Exited(0));
    wait_for_open(exit, false);
    let info = ok(addr, &["info", "exit"]);
    assert!(
        info.contains(&format!("\ndisks    {} (read-write)\n", arg(exit))),
        "{info}"
    );

    // Destroyed while running.
    ok(
        addr,
        &["create", "--name", "gone", "--disk", &arg(gone), "--start", &echo],
    );
    wait_for_open(gone, true);
    ok(addr, &["destroy", "gone"]);
    wait_for_open(gone, false);

    server.stop().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

/// `create --start` when the VM is created but does not start: what did happen is
/// printed, then the failure. The real Avalon cannot be made to do this on demand, so
/// a stand-in answers.
#[test]
fn created_but_not_started() {
    use avalon_proto::Message;
    let addr = fake_server("fake avalon", |request| match request {
        Message::CreateVm { .. } => Message::VmCreated { id: 7 },
        other => Message::Error {
            code: ErrorCode::InvalidState,
            message: format!("{} refused", other.name()),
        },
    });
    let server = addr.to_string();
    let echo = example_str("uart-echo");
    let args: Vec<String> = ["--server", &server, "create", "--name", "web", "--start", &echo]
        .iter()
        .map(|a| a.to_string())
        .collect();
    let mut out = Vec::new();
    let failure = round_table::run(&args, &mut out).unwrap_err();
    assert_eq!(String::from_utf8(out).unwrap(), "created VM 7 (web)\n");
    assert_eq!(
        failure,
        Failure::Failed("VM 7 (web) was created, but did not start: invalid state: StartVm refused".into())
    );
    assert_eq!(failure.exit_code(), 1);
}

#[test]
fn refusals() {
    let server = avalon(256);
    let addr = server.addr();
    let echo = example_str("uart-echo");
    ok(addr, &["create", "--name", "a", "--ram", "128", &echo]);

    let why = refused(addr, &["create", "--name", "a", &echo]);
    assert_eq!(why, "already exists: a VM named 'a' already exists");
    let why = refused(addr, &["create", "--name", "b", "--ram", "256", &echo]);
    assert!(why.starts_with("over budget: "), "{why}");
    // Not an ELF file.
    let source = example("uart-echo").with_extension("S");
    let why = refused(addr, &["create", "--name", "b", source.to_str().unwrap()]);
    assert!(why.starts_with("invalid program: "), "{why}");
    // Not a file at all: read here, before anything is sent.
    let why = refused(addr, &["create", "--name", "b", "no-such-program.elf"]);
    assert!(why.starts_with("cannot read no-such-program.elf: "), "{why}");

    assert_eq!(refused(addr, &["info", "99"]), "no such VM: no VM with id 99");
    for action in VmAction::ALL {
        let why = refused(addr, &[action.name(), "ghost"]);
        assert_eq!(why, "no such VM: no VM named 'ghost'", "{action:?}");
    }
    // Avalon does not manage disks.
    let why = rt(&["--lake", &addr.to_string(), "disk", "list"]).unwrap_err();
    assert!(
        matches!(&why, Failure::Failed(m) if m.starts_with("unsupported: ListDisks: ")),
        "{why:?}"
    );
    // Nothing changed.
    assert_eq!(ok(addr, &["list"]).lines().count(), 2);
    server.stop().unwrap();

    // No server.
    let dead = dead_addr();
    match rt(&["--server", &dead.to_string(), "list"]) {
        Err(Failure::Failed(why)) => {
            assert!(
                why.starts_with(&format!("cannot connect to Avalon at {dead}: ")),
                "{why}"
            )
        }
        other => panic!("{other:?}"),
    }
}

/// The library layer on its own, as Pendragon uses it.
#[test]
fn client_library() {
    let server = avalon(256);
    let mut client = Client::connect(&server.addr().to_string(), "test").unwrap();
    assert!(client.server().starts_with("avalon "), "{}", client.server());
    client.ping().unwrap();
    let program = std::fs::read(example("uart-echo")).unwrap();
    let profile = avalon_proto::HardwareProfile::with_ram(16 << 20);
    let id = client.create_vm("lib", profile.clone(), program.clone()).unwrap();
    let vm = VmRef::Id(id);
    let err = client.create_vm("lib", profile, program).unwrap_err();
    assert_eq!(err.code(), Some(ErrorCode::AlreadyExists));
    client.start_vm(&vm).unwrap();
    client.pause_vm(&vm).unwrap();
    assert_eq!(client.vm_info(&vm).unwrap().vm.state, VmState::Paused);
    client.resume_vm(&vm).unwrap();
    client.stop_vm(&vm).unwrap();
    assert_eq!(client.list_vms().unwrap()[0].state, VmState::Stopped);
    client.destroy_vm(&vm).unwrap();
    assert!(client.list_vms().unwrap().is_empty());
    assert_eq!(client.vm_info(&vm).unwrap_err().code(), Some(ErrorCode::NoSuchVm));
    assert_eq!(
        client.list_disks().unwrap_err().code(),
        Some(ErrorCode::Unsupported)
    );
    // The session is still fine after refusals.
    client.ping().unwrap();
    server.stop().unwrap();

    let dead = dead_addr().to_string();
    let err = Client::connect(&dead, "test").err().unwrap();
    assert!(matches!(err, avalon_client::Error::Connect { .. }), "{err:?}");
    assert_eq!(err.code(), None);
    assert!(err
        .to_string()
        .starts_with(&format!("cannot connect to {dead}: ")));
}
