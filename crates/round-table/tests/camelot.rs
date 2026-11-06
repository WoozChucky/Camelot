//! `disk create --from` and `create --camelot`/`--profile` against the real Lake and Avalon, in
//! process: copies of an image, and VMs that boot from copies of Lake's base image
//! (`os-base`) of their own, two of them running at once.
//!
//! The base images here are small bootable disks whose boot block spins forever
//! (`j .`), so a VM booted from one stays running and holds its disk; the real Camelot images are booted by `cargo xtask
//! up`'s tests (crates/xtask/tests/up.rs).

mod common;

use common::*;
use std::path::PathBuf;

/// A fresh images directory, removed when dropped.
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> TempDir {
        let dir = std::env::temp_dir().join(format!("round-table-camelot-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        TempDir(dir)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A bootable image of `sectors` sectors whose boot code is `j .`: it runs until it
/// is stopped.
pub fn spinning_image(sectors: usize) -> Vec<u8> {
    let mut block = [0u8; grail::boot::BOOT_BLOCK_SIZE];
    block[..4].copy_from_slice(&0x0000_006fu32.to_le_bytes()); // jal x0, 0
    grail::boot::seal(&mut block);
    let mut image = block.to_vec();
    image.resize(sectors * 512, 0);
    image
}

fn lake(dir: &TempDir) -> lake::ServerThread {
    let config = lake::Config {
        listen: "127.0.0.1:0".parse().unwrap(),
        log: false,
        ..lake::Config::new(&dir.0)
    };
    lake::Server::bind(&config).unwrap().spawn().unwrap()
}

#[test]
fn copies_of_a_disk() {
    let dir = TempDir::new("copies");
    std::fs::write(dir.0.join("os-base"), spinning_image(16)).unwrap();
    let lake = lake(&dir);
    let lake_addr = lake.addr().to_string();
    let disk = |args: &[&str]| {
        let mut full = vec!["--lake", &lake_addr, "disk"];
        full.extend_from_slice(args);
        rt(&full)
    };
    let refused = |args: &[&str]| match disk(args) {
        Err(round_table::Failure::Failed(why)) => why,
        other => panic!("{args:?} should be refused, got {other:?}"),
    };

    assert_eq!(
        disk(&["create", "copy", "--from", "os-base"]).unwrap(),
        "created disk copy (a copy of os-base, 8 KiB, 16 sectors)\n"
    );
    let base = std::fs::read(dir.0.join("os-base")).unwrap();
    assert_eq!(std::fs::read(dir.0.join("copy")).unwrap(), base);
    assert_eq!(
        disk(&["create", "bigger", "--from=os-base", "--size", "64K"]).unwrap(),
        "created disk bigger (a copy of os-base, 64 KiB, 128 sectors)\n"
    );
    let bigger = std::fs::read(dir.0.join("bigger")).unwrap();
    assert_eq!((&bigger[..base.len()], bigger.len()), (&base[..], 65536));

    assert_eq!(
        refused(&["create", "x", "--from", "missing"]),
        "no such disk: no disk named 'missing'"
    );
    assert_eq!(
        refused(&["create", "copy", "--from", "os-base"]),
        "already exists: a disk named 'copy' already exists"
    );
    assert_eq!(
        refused(&["create", "x", "--from", "os-base", "--size", "4K"]),
        "out of range: a copy of disk 'os-base' needs at least its 16 sectors, not 8"
    );
    assert_eq!(
        disk(&["list"]).unwrap(),
        "\
NAME     SIZE    SECTORS  ATTACHED
bigger   64 KiB  128      no
copy     8 KiB   16       no
os-base  8 KiB   16       no
"
    );
    lake.stop().unwrap();
}

#[test]
fn camelot_vms_boot_from_copies_of_their_own() {
    // The base image.
    let base = avalon_client::BASE_IMAGE;
    let dir = TempDir::new("vms");
    std::fs::write(dir.0.join(base), spinning_image(16)).unwrap();
    let lake = lake(&dir);
    let lake_addr = lake.addr().to_string();
    let avalon = avalon(256);
    let server = avalon.addr().to_string();
    let run = |args: &[&str]| {
        let mut full = vec!["--server", &server, "--lake", &lake_addr];
        full.extend_from_slice(args);
        rt(&full)
    };
    let refused = |args: &[&str]| match run(args) {
        Err(round_table::Failure::Failed(why)) => why,
        other => panic!("{args:?} should be refused, got {other:?}"),
    };

    // Two VMs, each on a copy of its own, running at the same time.
    assert_eq!(
        run(&["create", "--name", "one", "--camelot", "--ram", "16", "--start"]).unwrap(),
        format!("created disk one-disk0 (a copy of {base})\ncreated VM 1 (one)\nstarted VM 1 (one)\n")
    );
    assert_eq!(
        run(&["create", "--camelot", "--name", "two", "--ram", "16"]).unwrap(),
        format!("created disk two-disk0 (a copy of {base})\ncreated VM 2 (two)\n")
    );
    ok(avalon.addr(), &["start", "two"]);
    let addr = avalon.addr();
    let running = |s| s == avalon_proto::VmState::Running;
    wait_for_state(addr, "one", running);
    wait_for_state(addr, "two", running);
    // Both run on: their steps go up.
    for vm in ["one", "two"] {
        let before = steps(addr, vm);
        let start = std::time::Instant::now();
        while steps(addr, vm) == before {
            assert!(start.elapsed() < SLOW, "VM {vm} does not run");
        }
    }
    assert_eq!(
        ok(addr, &["list"]),
        "\
ID  NAME  STATE    RAM     CLOCK       DISKS  NETS
1   one   running  16 MiB  full speed  1      0
2   two   running  16 MiB  full speed  1      0
"
    );
    let info = ok(addr, &["info", "one"]);
    assert!(
        info.contains(&format!("disks    lake://{lake_addr}/one-disk0 (read-write)\n")),
        "{info}"
    );
    // Each holds its own copy; the images they were copied from stay free.
    let disks = |lake_addr: &str| rt(&["--lake", lake_addr, "disk", "list"]).unwrap();
    assert_eq!(
        disks(&lake_addr),
        "\
NAME       SIZE   SECTORS  ATTACHED
one-disk0  8 KiB  16       read-write
os-base    8 KiB  16       no
two-disk0  8 KiB  16       read-write
"
    );

    // A copy whose name is taken.
    assert_eq!(
        refused(&["create", "--name", "one", "--camelot"]),
        "cannot create disk one-disk0: already exists: a disk named 'one-disk0' already exists"
    );
    // A VM name Avalon has, but Lake not its disk: the copy is made, Avalon refuses
    // the VM, and the copy is deleted again. (Readers of the base image do not stop
    // a copy.)
    ok(
        addr,
        &[
            "create",
            "--name",
            "three",
            "--disk-ro",
            &format!("lake://{lake_addr}/{base}"),
        ],
    );
    let why = refused(&["create", "--name", "three", "--camelot"]);
    assert!(why.starts_with("already exists: "), "{why}");
    assert!(!dir.0.join("three-disk0").exists());
    // The base image with a writer cannot be copied.
    ok(addr, &["destroy", "three"]);
    ok(
        addr,
        &[
            "create",
            "--name",
            "w",
            "--disk",
            &format!("lake://{lake_addr}/{base}"),
        ],
    );
    assert_eq!(
        refused(&["create", "--name", "four", "--camelot"]),
        format!("cannot create disk four-disk0: busy: disk '{base}' is attached read-write")
    );
    ok(addr, &["destroy", "w"]);

    // A Lake without the base image says what to do.
    let empty = TempDir::new("empty");
    let bare = self::lake(&empty);
    let bare_addr = bare.addr().to_string();
    let args = [
        "--server",
        &server,
        "--lake",
        &bare_addr,
        "create",
        "--name",
        "five",
        "--camelot",
    ];
    match rt(&args) {
        Err(round_table::Failure::Failed(why)) => assert_eq!(
            why,
            format!(
                "the Lake at {bare_addr} has no '{base}' image to copy (start the platform with \
             'cargo xtask up', which puts it in Lake's images directory)"
            )
        ),
        other => panic!("{other:?}"),
    }
    assert_eq!(disks(&bare_addr), "no disks\n");
    // A name too long for its disk is a mistake on the command line.
    let long = "a".repeat(60);
    match rt(&["create", "--name", &long, "--camelot"]) {
        Err(round_table::Failure::Usage(why)) => assert!(why.contains("too long"), "{why}"),
        other => panic!("{other:?}"),
    }

    // Destroying the VMs frees their copies.
    ok(addr, &["destroy", "one"]);
    ok(addr, &["destroy", "two"]);
    let list = disks(&lake_addr);
    assert!(
        list.lines()
            .any(|l| l.split_whitespace().eq(["one-disk0", "8", "KiB", "16", "no"])),
        "{list}"
    );
    avalon.stop().unwrap();
    bare.stop().unwrap();
    lake.stop().unwrap();
}

#[test]
fn the_profiles_and_a_desktop() {
    // The profiles, with no server to ask.
    assert_eq!(
        rt(&["profiles"]).unwrap(),
        "\
NAME     IMAGE       RAM     BIOS     ABOUT
server   os-base     6 MiB   default  a Camelot server: the console's shell and shd, its shells over TCP
desktop  os-desktop  64 MiB  default  a Camelot desktop: its own image, and RAM for the largest display mode
"
    );
    let desktop = avalon_client::DESKTOP_IMAGE;
    let dir = TempDir::new("desktop");
    std::fs::write(dir.0.join(desktop), spinning_image(16)).unwrap();
    let lake = lake(&dir);
    let lake_addr = lake.addr().to_string();
    let avalon = avalon(256);
    let server = avalon.addr().to_string();
    let run = |args: &[&str]| {
        let mut full = vec!["--server", &server, "--lake", &lake_addr];
        full.extend_from_slice(args);
        rt(&full)
    };
    // A desktop: a copy of the desktop's image, the desktop's RAM.
    assert_eq!(
        run(&["create", "--name", "desk", "--profile", "desktop", "--start"]).unwrap(),
        format!("created disk desk-disk0 (a copy of {desktop})\ncreated VM 1 (desk)\nstarted VM 1 (desk)\n")
    );
    let addr = avalon.addr();
    wait_for_state(addr, "desk", |s| s == avalon_proto::VmState::Running);
    assert_eq!(
        ok(addr, &["list"]),
        "\
ID  NAME  STATE    RAM     CLOCK       DISKS  NETS
1   desk  running  64 MiB  full speed  1      0
"
    );
    // This Lake has no server image.
    match run(&["create", "--name", "srv", "--profile", "server"]) {
        Err(round_table::Failure::Failed(why)) => {
            assert!(
                why.starts_with(&format!("the Lake at {lake_addr} has no 'os-base' image")),
                "{why}"
            )
        }
        other => panic!("{other:?}"),
    }
    ok(addr, &["destroy", "desk"]);
    avalon.stop().unwrap();
    lake.stop().unwrap();
}
