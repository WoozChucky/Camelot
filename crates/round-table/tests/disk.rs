//! Round Table's disk commands against a stand-in Lake that speaks the protocol's
//! disk-management messages (tests/common). The real Lake server has an end-to-end
//! test of its own with Round Table.

mod common;

use avalon_client::Client;
use avalon_proto::ErrorCode;
use common::*;
use round_table::Failure;

fn lake_ok(lake: &FakeLake, args: &[&str]) -> String {
    let addr = lake.addr.to_string();
    let mut full = vec!["--lake", &addr];
    full.extend_from_slice(args);
    rt(&full).unwrap_or_else(|e| panic!("{args:?}: {e}"))
}

fn lake_refused(lake: &FakeLake, args: &[&str]) -> String {
    let addr = lake.addr.to_string();
    let mut full = vec!["--lake", &addr];
    full.extend_from_slice(args);
    match rt(&full) {
        Err(Failure::Failed(why)) => why,
        other => panic!("{args:?} should be refused, got {other:?}"),
    }
}

#[test]
fn create_list_delete() {
    let lake = FakeLake::start();
    assert_eq!(lake_ok(&lake, &["disk", "list"]), "no disks\n");
    assert_eq!(
        lake_ok(&lake, &["disk", "create", "data", "--size", "64MiB"]),
        "created disk data (64 MiB, 131072 sectors)\n"
    );
    assert_eq!(
        lake_ok(&lake, &["disk", "create", "--size=8s", "boot.img"]),
        "created disk boot.img (4 KiB, 8 sectors)\n"
    );
    assert_eq!(lake.names(), ["boot.img", "data"]);
    assert_eq!(lake.disks.lock().unwrap()["data"].sectors, 131_072);
    assert_eq!(lake.agents.lock().unwrap()[0], "round-table");

    // Images attached by VMs.
    lake.add("shared", 2048, 3, false);
    lake.add("system", 2 << 21, 0, true);
    assert_eq!(
        lake_ok(&lake, &["disk", "list"]),
        "\
NAME      SIZE    SECTORS  ATTACHED
boot.img  4 KiB   8        no
data      64 MiB  131072   no
shared    1 MiB   2048     3 readers
system    2 GiB   4194304  read-write
"
    );

    assert_eq!(
        lake_refused(&lake, &["disk", "create", "data", "--size", "1M"]),
        "already exists: a disk named 'data' already exists"
    );
    assert_eq!(
        lake_refused(&lake, &["disk", "delete", "system"]),
        "busy: disk 'system' is attached"
    );
    assert_eq!(
        lake_refused(&lake, &["disk", "delete", "shared"]),
        "busy: disk 'shared' is attached"
    );
    assert_eq!(
        lake_refused(&lake, &["disk", "delete", "nothing"]),
        "no such disk: no disk named 'nothing'"
    );
    // Names are checked by the server too (Round Table only checks the ones it
    // creates).
    assert!(lake_refused(&lake, &["disk", "delete", "Data"]).starts_with("no such disk: "));
    assert_eq!(lake.names(), ["boot.img", "data", "shared", "system"]);

    assert_eq!(lake_ok(&lake, &["disk", "delete", "data"]), "deleted disk data\n");
    assert_eq!(
        lake_ok(&lake, &["disk", "delete", "boot.img"]),
        "deleted disk boot.img\n"
    );
    assert_eq!(lake.names(), ["shared", "system"]);
    assert_eq!(
        lake_ok(&lake, &["disk", "list"]),
        "\
NAME    SIZE   SECTORS  ATTACHED
shared  1 MiB  2048     3 readers
system  2 GiB  4194304  read-write
"
    );
}

#[test]
fn wrong_or_missing_server() {
    let lake = FakeLake::start();
    // VM commands go to Avalon, not Lake.
    let addr = lake.addr.to_string();
    match rt(&["--server", &addr, "list"]) {
        Err(Failure::Failed(why)) => assert!(why.starts_with("unsupported: ListVms: "), "{why}"),
        other => panic!("{other:?}"),
    }
    let dead = dead_addr();
    match rt(&["--lake", &dead.to_string(), "disk", "create", "x", "--size", "1M"]) {
        Err(Failure::Failed(why)) => {
            assert!(
                why.starts_with(&format!("cannot connect to Lake at {dead}: ")),
                "{why}"
            )
        }
        other => panic!("{other:?}"),
    }
    // A command-line mistake never reaches the server.
    assert!(matches!(
        rt(&["--lake", &addr, "disk", "create", "x", "--size", "1.5M"]),
        Err(Failure::Usage(_))
    ));
    assert!(lake.names().is_empty());
}

/// The library layer on its own, as Pendragon uses it.
#[test]
fn client_library() {
    let lake = FakeLake::start();
    let mut client = Client::connect(&lake.addr.to_string(), "test").unwrap();
    assert_eq!(client.server(), "fake lake");
    client.ping().unwrap();
    client.create_disk("one", 1).unwrap();
    assert_eq!(
        client.create_disk("one", 1).unwrap_err().code(),
        Some(ErrorCode::AlreadyExists)
    );
    lake.add("held", 1, 0, true);
    assert_eq!(
        client.delete_disk("held").unwrap_err().code(),
        Some(ErrorCode::Busy)
    );
    let names: Vec<String> = client.list_disks().unwrap().into_iter().map(|d| d.name).collect();
    assert_eq!(names, ["held", "one"]);
    client.delete_disk("one").unwrap();
    assert_eq!(
        client.delete_disk("one").unwrap_err().code(),
        Some(ErrorCode::NoSuchDisk)
    );
    assert_eq!(
        client.list_vms().unwrap_err().code(),
        Some(ErrorCode::Unsupported)
    );
}
