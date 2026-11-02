//! The `camelot-fs` binary: mkfs from a host directory, then fsck, ls and
//! cat read it back (the round trip), and fsck --recover replays a log.

use camelot_fs::read::Fs;
use camelot_fs::{block, block_mut, fsck, superblock, LogHeader, BSIZE};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("camelot-fs-cli")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_camelot-fs"))
        .args(args)
        .output()
        .unwrap()
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

/// A host tree: files of several sizes (one past the direct blocks), an
/// empty file, nested directories, an empty directory.
fn host_tree(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let files = vec![
        ("README".to_string(), b"hello from the host\n".to_vec()),
        ("empty".to_string(), Vec::new()),
        (
            "bin/big".to_string(),
            (0..20_000u32).map(|i| (i * 13 % 251) as u8).collect(),
        ),
        ("bin/deeper/x".to_string(), vec![b'x'; 1024]),
    ];
    for (name, data) in &files {
        let path = dir.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, data).unwrap();
    }
    std::fs::create_dir_all(dir.join("void")).unwrap();
    files
}

#[test]
fn mkfs_then_read_back() {
    let dir = scratch("round-trip");
    let root = dir.join("root");
    let files = host_tree(&root);
    let img = dir.join("disk.img");
    let img_s = img.to_str().unwrap();

    let o = run(&[
        "mkfs",
        img_s,
        root.to_str().unwrap(),
        "--size",
        "512K",
        "--inodes",
        "48",
    ]);
    assert!(o.status.success(), "{o:?}");
    assert!(stdout(&o).contains(": 512 blocks ("), "{}", stdout(&o));
    let image = std::fs::read(&img).unwrap();
    assert_eq!(image.len(), 512 * BSIZE);

    // The library sees what the host had.
    let fs = Fs::open(&image).unwrap();
    for (name, data) in &files {
        assert_eq!(&fs.read_file(&format!("/{name}")).unwrap(), data, "{name}");
    }
    assert!(fs.list("/void").unwrap().len() == 2);
    let report = fsck::check(&image).unwrap();
    assert_eq!((report.dirs, report.files), (4, 4));

    // And so do the commands.
    let o = run(&["fsck", img_s]);
    assert!(o.status.success(), "{o:?}");
    assert!(stdout(&o).contains(": clean: 512 blocks"), "{}", stdout(&o));
    let o = run(&["ls", img_s]);
    assert_eq!(stdout(&o), fs.ls("/").unwrap());
    assert!(stdout(&o).contains(" README\n") && stdout(&o).contains(" bin\n"));
    let o = run(&["ls", img_s, "/bin/deeper"]);
    assert!(stdout(&o).ends_with("    1024 x\n"), "{}", stdout(&o));
    let o = run(&["cat", img_s, "/bin/big"]);
    assert!(o.status.success());
    assert_eq!(o.stdout, files[2].1);
}

#[test]
fn an_empty_filesystem_by_default() {
    let dir = scratch("empty");
    let img = dir.join("disk.img");
    let o = run(&["mkfs", img.to_str().unwrap()]);
    assert!(o.status.success(), "{o:?}");
    let image = std::fs::read(&img).unwrap();
    assert_eq!(image.len(), camelot_fs::DEFAULT_SIZE as usize * BSIZE);
    let sb = superblock(&image).unwrap();
    assert_eq!(
        (sb.ninodes, sb.nlog),
        (camelot_fs::DEFAULT_INODES, camelot_fs::DEFAULT_LOG)
    );
    let o = run(&["ls", img.to_str().unwrap()]);
    assert_eq!(stdout(&o), "d    1  1      32 .\nd    1  1      32 ..\n");
}

#[test]
fn fsck_reports_damage_and_recover_replays_the_log() {
    let dir = scratch("fsck");
    let root = dir.join("root");
    host_tree(&root);
    let img = dir.join("disk.img");
    let img_s = img.to_str().unwrap();
    assert!(run(&["mkfs", img_s, root.to_str().unwrap(), "--size", "300"])
        .status
        .success());
    let good = std::fs::read(&img).unwrap();
    let sb = superblock(&good).unwrap();

    // A block marked free that a file uses.
    let mut bad = good.clone();
    let fs = Fs::open(&good).unwrap();
    let readme = fs.inode(fs.lookup("/README").unwrap()).unwrap();
    let (b, byte, mask) = sb.bit_pos(readme.addrs[0]);
    block_mut(&mut bad, b)[byte] &= !mask;
    std::fs::write(&img, &bad).unwrap();
    let o = run(&["fsck", img_s]);
    assert!(!o.status.success());
    assert!(
        stdout(&o).contains("is marked free in the bitmap"),
        "{}",
        stdout(&o)
    );
    assert!(stdout(&o).ends_with(": 1 problems\n"), "{}", stdout(&o));

    // A committed transaction: README's block with new contents.
    let mut pending = good.clone();
    let mut new = block(&good, readme.addrs[0]).to_vec();
    new[..5].copy_from_slice(b"HELLO");
    block_mut(&mut pending, sb.logstart + 1).copy_from_slice(&new);
    let header = LogHeader {
        blocks: vec![readme.addrs[0]],
    };
    block_mut(&mut pending, sb.logstart).copy_from_slice(&header.encode());
    std::fs::write(&img, &pending).unwrap();
    let o = run(&["fsck", img_s]);
    assert!(o.status.success(), "{o:?}");
    assert!(stdout(&o).starts_with("the log holds a committed transaction of 1 blocks\n"));
    assert_eq!(std::fs::read(&img).unwrap(), pending, "fsck alone writes nothing");
    let o = run(&["cat", img_s, "/README"]);
    assert_eq!(o.stdout, b"HELLO from the host\n", "readers see the log replayed");
    let o = run(&["fsck", "--recover", img_s]);
    assert!(o.status.success());
    assert!(stdout(&o).starts_with("replayed 1 blocks from the log\n"));
    let after = std::fs::read(&img).unwrap();
    assert!(block(&after, sb.logstart).iter().all(|&b| b == 0));
    assert_eq!(&block(&after, readme.addrs[0])[..5], b"HELLO");
    assert_eq!(fsck::check(&after).unwrap().replayed, 0);

    // An orphan: README's entry gone and its count 0, as a crash leaves a
    // file unlinked while in use. Clean, reported; --recover frees it.
    let mut orphan = after.clone();
    let root = camelot_fs::read_inode(&orphan, &sb, camelot_fs::ROOT_INODE);
    let inum = fs.lookup("/README").unwrap();
    let dir = block_mut(&mut orphan, root.addrs[0]);
    let at = (0..dir.len())
        .step_by(camelot_fs::DIRENT_SIZE)
        .find(|&i| u32::from(u16::from_le_bytes([dir[i], dir[i + 1]])) == inum)
        .unwrap();
    dir[at..at + 2].fill(0);
    let mut d = camelot_fs::read_inode(&orphan, &sb, inum);
    d.nlink = 0;
    camelot_fs::write_inode(&mut orphan, &sb, inum, &d);
    std::fs::write(&img, &orphan).unwrap();
    let o = run(&["fsck", img_s]);
    assert!(o.status.success(), "{o:?}");
    assert!(
        stdout(&o).starts_with("1 orphaned inodes (no links, in no directory) for the next mount to free\n"),
        "{}",
        stdout(&o)
    );
    let o = run(&["fsck", "--recover", img_s]);
    assert!(o.status.success(), "{o:?}");
    assert!(
        stdout(&o).starts_with("freed 1 orphaned inodes\n"),
        "{}",
        stdout(&o)
    );
    let freed = std::fs::read(&img).unwrap();
    let r = fsck::check(&freed).unwrap();
    assert_eq!(r.orphans, 0);
    assert_eq!(r.free_blocks, fsck::check(&after).unwrap().free_blocks + 1);
    assert_eq!(camelot_fs::read_inode(&freed, &sb, inum).kind, camelot_fs::T_FREE);
}

#[test]
fn usage_and_errors() {
    let o = run(&[]);
    assert_eq!(o.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&o.stderr).contains("usage: camelot-fs"));
    let o = run(&["--help"]);
    assert!(o.status.success() && stdout(&o).contains("mkfs IMAGE"));

    let dir = scratch("errors");
    let img = dir.join("disk.img");
    let img_s = img.to_str().unwrap();
    // Names that can't be CMFS names.
    let root = dir.join("root");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("a-name-of-15-ch"), b"").unwrap();
    let o = run(&["mkfs", img_s, root.to_str().unwrap()]);
    assert!(!o.status.success());
    assert!(
        String::from_utf8_lossy(&o.stderr).contains("can't be a name"),
        "{o:?}"
    );
    // Too small, missing files, not a filesystem.
    let o = run(&["mkfs", img_s, "--size", "5"]);
    assert!(String::from_utf8_lossy(&o.stderr).contains("too small"), "{o:?}");
    let o = run(&["ls", dir.join("missing.img").to_str().unwrap()]);
    assert!(!o.status.success());
    std::fs::write(&img, vec![0; 8 * BSIZE]).unwrap();
    let o = run(&["fsck", img_s]);
    assert!(stdout(&o).contains("no filesystem"), "{o:?}");
    assert!(run(&["mkfs", img_s]).status.success());
    let o = run(&["cat", img_s, "/nope"]);
    assert!(
        String::from_utf8_lossy(&o.stderr).contains("no such file"),
        "{o:?}"
    );
}

/// `mkfs --boot --kernel` with Camelot's committed bootloader and kernel: the
/// boot block, then the kernel after the filesystem, which fsck still finds
/// clean and says is bootable.
#[test]
fn mkfs_makes_a_bootable_image() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let boot = repo.join("kernel/boot/boot.elf");
    let kernel = repo.join("kernel/kernel.elf");
    let dir = scratch("bootable");
    let img = dir.join("root.img");
    let img_s = img.to_str().unwrap();
    let o = run(&[
        "mkfs",
        img_s,
        "--size",
        "1M",
        "--boot",
        boot.to_str().unwrap(),
        "--kernel",
        kernel.to_str().unwrap(),
    ]);
    assert!(o.status.success(), "{o:?}");
    let image = std::fs::read(&img).unwrap();
    let kernel = std::fs::read(&kernel).unwrap();
    let info = camelot_fs::boot::boot_info(&image).unwrap().expect("bootable");
    assert_eq!(info.kernel_sector, 2048, "right after the 1 MiB filesystem");
    assert_eq!(image.len(), 1024 * BSIZE + kernel.len().div_ceil(BSIZE) * BSIZE);
    assert_eq!(
        &camelot_fs::boot::kernel_image(&image).unwrap()[..kernel.len()],
        &kernel[..]
    );
    let line = format!("bootable: kernel at sector 2048, {} sectors", info.kernel_sectors);
    assert!(stdout(&o).contains(&line), "{}", stdout(&o));

    let o = run(&["fsck", img_s]);
    assert!(o.status.success(), "{o:?}");
    assert!(stdout(&o).contains(": clean: 1024 blocks"), "{}", stdout(&o));
    assert!(stdout(&o).contains(&line), "{}", stdout(&o));

    // Half of the pair is an error; so is a kernel that is not an ELF.
    let o = run(&["mkfs", img_s, "--boot", boot.to_str().unwrap()]);
    assert!(!o.status.success());
    assert!(String::from_utf8_lossy(&o.stderr).contains("go together"));
    let o = run(&[
        "mkfs",
        img_s,
        "--boot",
        boot.to_str().unwrap(),
        "--kernel",
        boot.to_str().unwrap().replace("boot.elf", "boot.s").as_str(),
    ]);
    assert!(!o.status.success());
    assert!(String::from_utf8_lossy(&o.stderr).contains("kernel:"), "{o:?}");
}
