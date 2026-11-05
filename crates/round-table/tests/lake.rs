//! Round Table against the real Lake server (and Avalon), both in process: the disk
//! commands end to end, and a VM created on a Lake disk holding it until it is
//! destroyed.

mod common;

use common::*;
use std::path::PathBuf;

/// A fresh images directory, removed when dropped.
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> TempDir {
        let dir = std::env::temp_dir().join(format!("round-table-lake-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        TempDir(dir)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The largest disk a client takes is Lake's largest image: avalon-client cannot
/// depend on Lake, so this ties the two.
const _: () = assert!(avalon_client::parse::MAX_DISK_SECTORS == lake::MAX_IMAGE_SECTORS);

#[test]
fn disk_commands_against_lake() {
    let dir = TempDir::new("disks");
    let config = lake::Config {
        listen: "127.0.0.1:0".parse().unwrap(),
        log: false,
        ..lake::Config::new(&dir.0)
    };
    let lake = lake::Server::bind(&config).unwrap().spawn().unwrap();
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

    assert_eq!(disk(&["list"]).unwrap(), "no disks\n");
    assert_eq!(
        disk(&["create", "data", "--size", "64MiB"]).unwrap(),
        "created disk data (64 MiB, 131072 sectors)\n"
    );
    assert_eq!(std::fs::metadata(dir.0.join("data")).unwrap().len(), 64 * MIB);
    assert_eq!(
        refused(&["create", "data", "--size", "1M"]),
        "already exists: a disk named 'data' already exists"
    );

    // A VM created on the disk holds it, read-write, until it is destroyed.
    let avalon = avalon(256);
    let source = format!("lake://{lake_addr}/data");
    ok(
        avalon.addr(),
        &["create", "--name", "vm", "--disk", &source, &example_str("disk")],
    );
    assert_eq!(
        disk(&["list"]).unwrap(),
        "\
NAME  SIZE    SECTORS  ATTACHED
data  64 MiB  131072   read-write
"
    );
    assert!(refused(&["delete", "data"]).starts_with("busy: "));
    ok(avalon.addr(), &["destroy", "vm"]);
    assert_eq!(
        disk(&["list"]).unwrap(),
        "\
NAME  SIZE    SECTORS  ATTACHED
data  64 MiB  131072   no
"
    );

    assert_eq!(disk(&["delete", "data"]).unwrap(), "deleted disk data\n");
    assert!(!dir.0.join("data").exists());
    assert_eq!(refused(&["delete", "data"]), "no such disk: no disk named 'data'");
    assert_eq!(disk(&["list"]).unwrap(), "no disks\n");
    avalon.stop().unwrap();
    lake.stop().unwrap();
}
