//! The kernel's filesystem (issue #66, docs/camelot-fs.md), against the
//! host's implementation in the `camelot-fs` crate. The committed
//! `kernel/kernel.elf` boots on Grail with images built by mkfs on
//! in-memory disks that record every write; fsck and the reader check what
//! the kernel left, and the kernel's listing must be the host's.
//!
//! The crash tests stop the machine after each sector the self-test's
//! transactions write, check with fsck that the disk replayed is
//! consistent and holds all of each transaction or none of it, then reboot
//! the kernel on it and check it replays the log exactly as the host does.

use camelot_fs::mkfs::{Mkfs, Options};
use camelot_fs::read::Fs;
use camelot_fs::{block, fsck, Superblock, BSIZE, SECTOR_SIZE, T_FILE};
use grail::block::BlockBackend;
use grail::{Machine, MachineConfig, RunResult};
use std::sync::{Arc, Mutex};
use xtask::kernel::{self, BOOT_MAX_STEPS};
use xtask::repo_root;

/// Values of the kernel's `fs_test` (kernel/include/fstest.h).
const FS_TEST_READ: u32 = 1;
const FS_TEST_WRITE: u32 = 2;

/// Checks the self-test runs (kernel/fstest.c): the read checks, one fewer
/// without a /bin, and the transactions'.
const READ_CHECKS: u32 = 12;
const READ_CHECKS_NO_BIN: u32 = 11;
const WRITE_CHECKS: u32 = 34;
/// Transactions the write test commits (`COMMITS` in kernel/fstest.c).
const COMMITS: usize = 12;

/// The write test's files (kernel/fstest.c): lengths and pattern seeds.
const B_LEN: usize = 700;
const A_LEN: usize = 2500;
const BIG_LEN: usize = 13 * BSIZE + 100;
const P_LEN: usize = 1500;

/// RAM for these machines: less than the default, so they boot faster
/// (the allocator sets up every page).
const RAM: usize = 16 << 20;

/// What the shell prints first.
const SHELL_BANNER: &str = "Type help for the list of commands.\n";

/// Byte `i` of a test file with `seed` (`pattern_byte` in kernel/fstest.c).
fn pattern(len: usize, seed: u32) -> Vec<u8> {
    (0..len as u32).map(|i| (i * 7 + seed + i / 251) as u8).collect()
}

/// One thing a disk was asked to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    /// A sector written, and the first word written to it.
    Write {
        sector: u64,
        first_word: u32,
    },
    Flush,
}

#[derive(Debug, Default)]
struct Record {
    ops: Vec<Op>,
    /// The disk after each sector written, if asked for: where it would
    /// stand had the machine stopped right there.
    snapshots: Vec<Vec<u8>>,
}

/// An in-memory disk that records what it is asked to do.
struct Recorder {
    data: Vec<u8>,
    read_only: bool,
    snapshots: bool,
    record: Arc<Mutex<Record>>,
}

impl BlockBackend for Recorder {
    fn capacity(&self) -> u64 {
        (self.data.len() / SECTOR_SIZE) as u64
    }
    fn read_only(&self) -> bool {
        self.read_only
    }
    fn read(&mut self, sector: u64, buf: &mut [u8]) -> std::io::Result<()> {
        let range = grail::block::byte_range(self.capacity(), sector, buf.len())?;
        buf.copy_from_slice(&self.data[range.start as usize..range.end as usize]);
        Ok(())
    }
    fn write(&mut self, sector: u64, buf: &[u8]) -> std::io::Result<()> {
        grail::block::byte_range(self.capacity(), sector, buf.len())?;
        let mut record = self.record.lock().unwrap();
        // A sector at a time, as a disk that loses power mid-request would
        // leave it.
        for (i, s) in buf.chunks(SECTOR_SIZE).enumerate() {
            let sector = sector + i as u64;
            let at = sector as usize * SECTOR_SIZE;
            self.data[at..at + SECTOR_SIZE].copy_from_slice(s);
            let first_word = u32::from_le_bytes([s[0], s[1], s[2], s[3]]);
            record.ops.push(Op::Write { sector, first_word });
            if self.snapshots {
                record.snapshots.push(self.data.clone());
            }
        }
        Ok(())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.record.lock().unwrap().ops.push(Op::Flush);
        Ok(())
    }
}

/// How a boot went.
struct Run {
    result: RunResult,
    /// Everything printed, `xcal.dbg`'s pc masked.
    out: String,
    /// The disk afterwards.
    disk: Vec<u8>,
    record: Record,
}

#[derive(Clone, Copy)]
struct Boot {
    read_only: bool,
    fs_test: u32,
    snapshots: bool,
    /// Stop the machine once this many sectors have been written.
    stop_after: Option<usize>,
}

const READ: Boot = Boot {
    read_only: false,
    fs_test: FS_TEST_READ,
    snapshots: false,
    stop_after: None,
};

const WRITE: Boot = Boot {
    fs_test: FS_TEST_WRITE,
    ..READ
};

fn committed_elf() -> Vec<u8> {
    let path = repo_root().join("kernel").join("kernel.elf");
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// Boots the committed kernel with `image` as disk 0 and `exit` typed for
/// the shell.
fn boot(image: Vec<u8>, how: Boot) -> Run {
    let record = Arc::new(Mutex::new(Record::default()));
    let disk = Recorder {
        data: image,
        read_only: how.read_only,
        snapshots: how.snapshots,
        record: record.clone(),
    };
    let config = MachineConfig {
        ram_size: RAM,
        ..MachineConfig::default()
    };
    let mut m = Machine::with_backends(config, vec![Box::new(disk)]).unwrap();
    m.cpu.bus.uart.capture = true;
    let file = committed_elf();
    let elf = grail::elf::parse(&file).unwrap();
    m.load_elf(&file).unwrap();
    let fs_test = elf.symbol("fs_test").expect("no fs_test in the kernel");
    m.cpu.bus.load(fs_test, &how.fs_test.to_le_bytes()).unwrap();
    m.set_uart_input(kernel::typed(b"exit\n"));
    let result = match how.stop_after {
        None => m.run(Some(BOOT_MAX_STEPS), |_| {}),
        Some(n) => {
            let written = || {
                let r = record.lock().unwrap();
                r.ops.iter().filter(|o| matches!(o, Op::Write { .. })).count()
            };
            for _ in 0..BOOT_MAX_STEPS {
                if written() >= n {
                    break;
                }
                m.cpu.step();
            }
            assert_eq!(written(), n, "the machine never wrote {n} sectors");
            RunResult::StepLimit
        }
    };
    let out = kernel::mask_dbg_pc(&String::from_utf8_lossy(&m.cpu.bus.uart.captured));
    let d = &mut m.cpu.bus.disks[0];
    let mut disk = vec![0; d.capacity() as usize * SECTOR_SIZE];
    d.backend().read(0, &mut disk).unwrap();
    drop(m);
    let record = std::mem::take(&mut *record.lock().unwrap());
    Run {
        result,
        out,
        disk,
        record,
    }
}

/// What the kernel printed about the filesystem: from after the disk
/// self-test's line to the shell's banner.
fn fs_lines(out: &str) -> &str {
    let blk = out
        .find("blk self-test: ")
        .unwrap_or_else(|| panic!("no disk self-test:\n{out}"));
    let start = blk + out[blk..].find('\n').unwrap() + 1;
    let end = out
        .find(SHELL_BANNER)
        .unwrap_or_else(|| panic!("no shell:\n{out}"));
    &out[start..end]
}

/// The kernel's line about the mounted disk, for an image fsck reported on.
fn mount_line(r: &fsck::Report, read_only: bool) -> String {
    format!(
        "fs: disk 0 mounted: {} blocks ({} free), {} inodes ({} free), {} log blocks{}\n",
        r.sb.size,
        r.free_blocks,
        r.sb.ninodes,
        r.free_inodes,
        r.sb.nlog,
        if read_only { ", read-only" } else { "" }
    )
}

fn passed(n: u32) -> String {
    format!("fs self-test: {n}/{n} passed\n")
}

/// The root image `cargo xtask fs` builds, `size` blocks.
fn root_image(size: u32) -> Vec<u8> {
    xtask::fs::build(
        &repo_root(),
        &Options {
            size,
            ..Options::default()
        },
    )
    .unwrap()
}

/// A small image without /bin: a README and an empty directory. Small, so
/// that the crash tests' snapshots are cheap.
fn small_image() -> Vec<u8> {
    let mut fs = Mkfs::new(&Options {
        size: 160,
        ninodes: 32,
        nlog: 21,
    })
    .unwrap();
    fs.add_file("/README", b"a small disk\n").unwrap();
    fs.mkdir("/tmp").unwrap();
    fs.finish()
}

fn programs() -> usize {
    xtask::userprogs::program_names(&xtask::userprogs::user_dir(&repo_root()))
        .unwrap()
        .len()
}

/// A disk's state as the files see it: every path, its inode, type, links
/// and contents, after replaying the log.
fn state(image: &[u8]) -> Vec<(String, u32, u16, u16, Vec<u8>)> {
    let fs = Fs::open(image).unwrap();
    fs.walk()
        .unwrap()
        .into_iter()
        .map(|(path, e)| {
            let contents = if e.inode.kind == T_FILE {
                fs.contents(e.inum).unwrap()
            } else {
                Vec::new()
            };
            (path, e.inum, e.inode.kind, e.inode.nlink, contents)
        })
        .collect()
}

/// The block writes among `ops`, two sectors each, with the first word of
/// each block; panics on a lone sector.
fn block_ops(ops: &[Op]) -> Vec<Op> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < ops.len() {
        match ops[i] {
            Op::Flush => out.push(Op::Flush),
            Op::Write { sector, first_word } => {
                assert!(sector % 2 == 0, "a write starting at odd sector {sector}");
                assert!(
                    matches!(ops.get(i + 1), Some(Op::Write { sector: s, .. }) if *s == sector + 1),
                    "block {} written in half",
                    sector / 2
                );
                out.push(Op::Write {
                    sector: sector / 2,
                    first_word,
                });
                i += 1;
            }
        }
        i += 1;
    }
    out
}

/// Checks that the filesystem's writes in `ops` (after the disk self-test's,
/// to the disk's last block, which the filesystem doesn't use here) follow
/// the commit protocol of docs/camelot-fs.md, "The log", every time: slots
/// from the first, flush, header with their count, flush, as many home
/// blocks, flush, empty header, flush. Returns the number of commits.
fn check_commits(ops: &[Op], sb: &Superblock, disk_blocks: u32) -> usize {
    let ops = block_ops(ops);
    let scratch = disk_blocks - 1;
    let first = ops
        .iter()
        .position(|o| matches!(o, Op::Write { sector, .. } if *sector as u32 != scratch))
        .unwrap_or(ops.len());
    let ops = &ops[first..];
    let header = sb.logstart;
    let mut i = 0;
    let mut commits = 0;
    let write = |o: Option<&Op>| match o {
        Some(Op::Write { sector, first_word }) => Some((*sector as u32, *first_word)),
        _ => None,
    };
    while i < ops.len() {
        let mut slots = 0;
        while let Some((b, _)) = write(ops.get(i)).filter(|(b, _)| *b > header && *b < sb.inodestart) {
            assert_eq!(b, header + 1 + slots, "commit {commits}: slots out of order");
            slots += 1;
            i += 1;
        }
        assert!(slots > 0, "commit {commits}: no slots at op {i}: {:?}", &ops[i..]);
        assert_eq!(
            ops.get(i),
            Some(&Op::Flush),
            "commit {commits}: no flush after the slots"
        );
        assert_eq!(
            write(ops.get(i + 1)),
            Some((header, slots)),
            "commit {commits}: the header"
        );
        assert_eq!(
            ops.get(i + 2),
            Some(&Op::Flush),
            "commit {commits}: no flush after the header"
        );
        i += 3;
        for k in 0..slots {
            let (b, _) = write(ops.get(i)).unwrap_or_else(|| panic!("commit {commits}: home {k} missing"));
            assert!(
                b >= sb.inodestart && b < scratch,
                "commit {commits}: home block {b}"
            );
            i += 1;
        }
        assert_eq!(
            ops.get(i),
            Some(&Op::Flush),
            "commit {commits}: no flush after installing"
        );
        assert_eq!(
            write(ops.get(i + 1)),
            Some((header, 0)),
            "commit {commits}: the empty header"
        );
        assert_eq!(
            ops.get(i + 2),
            Some(&Op::Flush),
            "commit {commits}: no flush after emptying"
        );
        i += 3;
        commits += 1;
    }
    commits
}

#[test]
fn the_kernel_mounts_the_root_image_and_lists_it() {
    // Issue #66's acceptance test: mkfs builds the root image, the kernel
    // mounts it at boot and lists / in its self-test, as the host lists it;
    // every program in /bin reads back as the kernel's own copy.
    let image = root_image(2048);
    let report = fsck::check(&image).unwrap();
    let host = Fs::open(&image).unwrap();
    let run = boot(image.clone(), READ);
    assert_eq!(run.result, RunResult::Exited(0), "{}", run.out);
    let n = programs();
    assert_eq!(
        fs_lines(&run.out),
        format!(
            "{}fs: ls /\n{}fs: /bin holds {n} of the kernel's {n} programs\n{}",
            mount_line(&report, false),
            host.ls("/").unwrap(),
            passed(READ_CHECKS)
        )
    );
    assert!(host.ls("/").unwrap().contains(" README\n"));
    // Reading changes nothing; the only writes were the disk self-test's
    // to its scratch block (the last), which it put back.
    assert_eq!(run.disk, image);
    let last = (image.len() / BSIZE) as u64 - 1;
    for op in &run.record.ops {
        if let Op::Write { sector, .. } = op {
            assert_eq!(sector / 2, last, "{op:?}");
        }
    }
}

#[test]
fn the_kernels_transactions_are_what_the_host_reads_back() {
    // The write test's transactions on the root image: fsck finds the disk
    // consistent, the host reads the files the kernel wrote (one past its
    // direct blocks) and finds the removed ones gone, the kernel's listing
    // of /fstest is the host's, and every commit followed the protocol.
    let image = root_image(2048);
    let before = fsck::check(&image).unwrap();
    let root_listing = Fs::open(&image).unwrap().ls("/").unwrap();
    let run = boot(image, WRITE);
    assert_eq!(run.result, RunResult::Exited(0), "{}", run.out);
    let after = fsck::check(&run.disk).unwrap_or_else(|p| panic!("{p:#?}"));
    assert_eq!(after.replayed, 0);
    let fs = Fs::open(&run.disk).unwrap();
    let n = programs();
    assert_eq!(
        fs_lines(&run.out),
        format!(
            "{}fs: ls /\n{}fs: /bin holds {n} of the kernel's {n} programs\nfs: ls /fstest\n{}{}",
            mount_line(&before, false),
            root_listing,
            fs.ls("/fstest").unwrap(),
            passed(READ_CHECKS + WRITE_CHECKS)
        )
    );
    assert_eq!(fs.read_file("/fstest/b").unwrap(), pattern(B_LEN, 2));
    assert_eq!(fs.read_file("/fstest/big").unwrap(), pattern(BIG_LEN, 3));
    assert_eq!(fs.read_file("/fstest/p1").unwrap(), pattern(P_LEN, 4));
    assert_eq!(fs.read_file("/fstest/p2").unwrap(), pattern(P_LEN, 5));
    let big = fs.inode(fs.lookup("/fstest/big").unwrap()).unwrap();
    assert_ne!(big.indirect(), 0);
    assert!(fs.lookup("/fstest/a").is_err());
    assert!(fs.lookup("/fstest/d").is_err());
    let names: Vec<String> = fs.list("/fstest").unwrap().iter().map(|e| e.name()).collect();
    // big took a's free slot, p1 d's.
    assert_eq!(names, [".", "..", "big", "b", "p1", "p2"]);
    assert_eq!((after.dirs, after.files), (before.dirs + 1, before.files + 4));
    assert_eq!(after.free_blocks, before.free_blocks - 21);
    assert_eq!(after.free_inodes, before.free_inodes - 5);
    let disk_blocks = (run.disk.len() / BSIZE) as u32;
    assert_eq!(check_commits(&run.record.ops, &after.sb, disk_blocks), COMMITS);
}

#[test]
fn a_crash_after_any_write_leaves_whole_transactions() {
    // Issue #66's crash-recovery test, at every point: the machine stopped
    // after each sector the write test sends to the disk (a block is two
    // sectors, so also with a block half written). Replayed, every one of
    // those disks is consistent, and holds the state before or after each
    // transaction, never one in between: the state changes exactly when a
    // header with n > 0 is written, to the state of that commit, in order.
    // Then the kernel boots on each one, replays its log as the host did
    // (the disk afterwards is byte for byte the host's replay) and passes
    // its self-test.
    let image = small_image();
    let run = boot(
        image.clone(),
        Boot {
            snapshots: true,
            ..WRITE
        },
    );
    assert_eq!(run.result, RunResult::Exited(0), "{}", run.out);
    assert!(
        run.out.contains(&passed(READ_CHECKS_NO_BIN + WRITE_CHECKS)),
        "{}",
        run.out
    );
    let snapshots = run.record.snapshots;
    // Each commit writes at least a slot, the header, a home and the header
    // again: 8 sectors.
    assert!(snapshots.len() >= 8 * COMMITS, "{} snapshots", snapshots.len());
    assert_eq!(snapshots.last(), Some(&run.disk));

    let mut states = vec![state(&image)];
    let mut commits = Vec::new();
    let mut pending = 0;
    for (i, snap) in snapshots.iter().enumerate() {
        let r = fsck::check(snap).unwrap_or_else(|p| panic!("stopped after sector write {i}: {p:#?}"));
        let s = state(snap);
        if r.replayed > 0 {
            pending += 1;
            if commits.last() != Some(&s) {
                commits.push(s.clone());
            }
        }
        if states.last() != Some(&s) {
            states.push(s);
        }
    }
    assert!(pending > COMMITS, "{pending} disks with a transaction to replay");
    assert_eq!(commits.len(), COMMITS);
    assert_eq!(states[1..], commits[..]);
    assert_eq!(states.last().unwrap(), &state(&run.disk));

    // Reboot on every one, several machines at a time.
    let threads = std::thread::available_parallelism()
        .map_or(4, |n| n.get())
        .min(16);
    let chunk = snapshots.len().div_ceil(threads);
    std::thread::scope(|scope| {
        for (c, part) in snapshots.chunks(chunk).enumerate() {
            scope.spawn(move || {
                for (j, snap) in part.iter().enumerate() {
                    let i = c * chunk + j;
                    let replayed = fsck::check(snap).unwrap().replayed;
                    let mut want = snap.clone();
                    fsck::recover(&mut want).unwrap();
                    let again = boot(snap.clone(), READ);
                    assert_eq!(
                        again.result,
                        RunResult::Exited(0),
                        "after write {i}:\n{}",
                        again.out
                    );
                    let lines = fs_lines(&again.out);
                    assert_eq!(
                        lines.starts_with(&format!("fs: replayed {replayed} blocks from the log\n")),
                        replayed > 0,
                        "after write {i}:\n{lines}"
                    );
                    assert!(
                        lines.ends_with(&passed(READ_CHECKS_NO_BIN)),
                        "after write {i}:\n{lines}"
                    );
                    assert!(
                        again.disk == want,
                        "after write {i}: the kernel's replay is not the host's"
                    );
                }
            });
        }
    });
}

#[test]
fn stopping_the_machine_mid_commit_then_rebooting_completes_the_transaction() {
    // The same, step by step, for one point: a first run finds the write
    // that commits the first transaction (/fstest and /fstest/a); a second
    // stops the machine right after it, before any block went home. The
    // disk then holds the transaction only in its log. Rebooted, the kernel
    // replays it (home blocks, flush, empty header, flush, nothing else),
    // and the files are there.
    let image = small_image();
    let sb = camelot_fs::superblock(&image).unwrap();
    let full = boot(image.clone(), WRITE);
    let header = u64::from(sb.logstart) * 2;
    let mut sectors = 0;
    let mut n = 0;
    for op in &full.record.ops {
        if let Op::Write { sector, first_word } = *op {
            sectors += 1;
            if sector == header && first_word > 0 {
                n = first_word;
                // Both sectors of the header.
                sectors += 1;
                break;
            }
        }
    }
    assert!(n > 0, "no commit");

    let stopped = boot(
        image.clone(),
        Boot {
            stop_after: Some(sectors),
            ..WRITE
        },
    );
    let disk = stopped.disk;
    let log = fsck::log_header(&disk).unwrap();
    assert_eq!(log.blocks.len(), n as usize);
    for (i, &home) in log.blocks.iter().enumerate() {
        assert_ne!(
            block(&disk, home),
            block(&disk, sb.logstart + 1 + i as u32),
            "block {home} is home already"
        );
    }
    // As it stands (without the replay), /fstest doesn't exist.
    let mut unreplayed = disk.clone();
    camelot_fs::block_mut(&mut unreplayed, sb.logstart).fill(0);
    assert!(Fs::open(&unreplayed).unwrap().lookup("/fstest").is_err());

    let again = boot(disk, READ);
    assert_eq!(again.result, RunResult::Exited(0), "{}", again.out);
    let lines = fs_lines(&again.out);
    assert!(
        lines.starts_with(&format!(
            "fs: replayed {n} blocks from the log\nfs: disk 0 mounted: "
        )),
        "{lines}"
    );
    assert!(lines.ends_with(&passed(READ_CHECKS_NO_BIN)), "{lines}");
    let r = fsck::check(&again.disk).unwrap();
    assert_eq!(r.replayed, 0);
    let fs = Fs::open(&again.disk).unwrap();
    assert_eq!(fs.read_file("/fstest/a").unwrap(), pattern(A_LEN, 1));
    // The replay's writes, after the disk self-test's.
    let ops = block_ops(&again.record.ops);
    let scratch = (again.disk.len() / BSIZE) as u64 - 1;
    let fs_ops: Vec<Op> = ops
        .into_iter()
        .skip_while(|o| !matches!(o, Op::Write { sector, .. } if *sector != scratch))
        .collect();
    let homes: Vec<Op> = log
        .blocks
        .iter()
        .map(|&b| Op::Write {
            sector: u64::from(b),
            first_word: u32::from_le_bytes(block(&again.disk, b)[..4].try_into().unwrap()),
        })
        .collect();
    let mut want = homes;
    want.extend([
        Op::Flush,
        Op::Write {
            sector: u64::from(sb.logstart),
            first_word: 0,
        },
        Op::Flush,
    ]);
    assert_eq!(fs_ops, want);
}

/// Boots `image` and returns what the kernel said about the filesystem,
/// checking the disk is unchanged.
fn mount_message(image: Vec<u8>, read_only: bool) -> String {
    let run = boot(image.clone(), Boot { read_only, ..READ });
    assert_eq!(run.result, RunResult::Exited(0), "{}", run.out);
    assert!(run.disk == image, "the disk changed");
    fs_lines(&run.out).to_string()
}

#[test]
fn a_disk_that_cannot_be_mounted_is_left_alone() {
    // No filesystem, a broken superblock or log header, a root that is not
    // a directory, a log to replay on a read-only disk: the kernel says
    // what is wrong, doesn't mount it, runs no filesystem test and changes
    // nothing.
    assert_eq!(
        mount_message(vec![0; 64 * BSIZE], false),
        "fs: no filesystem on disk 0\n"
    );
    let sb = camelot_fs::superblock(&small_image()).unwrap();
    let broken = |f: &dyn Fn(&mut Vec<u8>)| {
        let mut image = small_image();
        f(&mut image);
        image
    };
    let set_u32 = |img: &mut Vec<u8>, block: u32, off: usize, v: u32| {
        let at = block as usize * BSIZE + off;
        img[at..at + 4].copy_from_slice(&v.to_le_bytes());
    };
    let not_mounted = |why: &str| format!("fs: disk 0 not mounted: {why}\n");
    for (image, why) in [
        (broken(&|i| set_u32(i, 1, 4, 2)), "unknown version"),
        (broken(&|i| set_u32(i, 1, 20, 1)), "bad log size"),
        (broken(&|i| set_u32(i, 1, 16, 1)), "bad inode count"),
        (
            broken(&|i| set_u32(i, 1, 36, sb.datastart + 1)),
            "the regions are misplaced",
        ),
        (broken(&|i| i.truncate(100 * BSIZE)), "bigger than the disk"),
        (
            broken(&|i| set_u32(i, sb.logstart, 0, sb.nlog)),
            "the log header counts more blocks than the log has",
        ),
        (
            broken(&|i| {
                set_u32(i, sb.logstart, 0, 1);
                set_u32(i, sb.logstart, 4, 1);
            }),
            "the log header names a block outside the filesystem",
        ),
        (
            broken(&|i| {
                set_u32(i, sb.logstart, 0, 2);
                set_u32(i, sb.logstart, 4, sb.datastart);
                set_u32(i, sb.logstart, 8, sb.datastart);
            }),
            "the log header names a block twice",
        ),
        (
            broken(&|i| {
                let (b, off) = sb.inode_pos(1);
                i[b as usize * BSIZE + off] = T_FILE as u8;
            }),
            "the root is not a directory",
        ),
    ] {
        assert_eq!(mount_message(image, false), not_mounted(why));
    }
    // A committed transaction (one block: the root's directory block, as it
    // is) on a read-only disk can't be replayed.
    let image = broken(&|i| {
        let root = camelot_fs::read_inode(i, &sb, 1).addrs[0];
        let copy = block(i, root).to_vec();
        camelot_fs::block_mut(i, sb.logstart + 1).copy_from_slice(&copy);
        set_u32(i, sb.logstart, 0, 1);
        set_u32(i, sb.logstart, 4, root);
    });
    assert_eq!(
        mount_message(image, true),
        not_mounted("the disk is read-only and its log holds a transaction to replay")
    );
}

#[test]
fn a_read_only_root_is_read_but_never_written() {
    let image = small_image();
    let report = fsck::check(&image).unwrap();
    let run = boot(
        image.clone(),
        Boot {
            read_only: true,
            ..WRITE
        },
    );
    assert_eq!(run.result, RunResult::Exited(0), "{}", run.out);
    assert_eq!(
        fs_lines(&run.out),
        format!(
            "{}fs: ls /\n{}fs: write test skipped: the disk is read-only\n{}",
            mount_line(&report, true),
            Fs::open(&image).unwrap().ls("/").unwrap(),
            passed(READ_CHECKS_NO_BIN)
        )
    );
    assert!(run.disk == image);
    // The device refuses writes before the backend sees them: nothing came.
    assert_eq!(run.record.ops, []);
}

#[test]
fn the_format_document_matches_the_kernel_and_the_host() {
    // The numbers docs/camelot-fs.md gives are the ones both sides use.
    let doc = std::fs::read_to_string(repo_root().join("docs/camelot-fs.md")).unwrap();
    let fs_h = std::fs::read_to_string(repo_root().join("kernel/include/fs.h")).unwrap();
    let log_h = std::fs::read_to_string(repo_root().join("kernel/include/log.h")).unwrap();
    let define = |text: &str, name: &str| -> String {
        let line = text
            .lines()
            .find(|l| l.starts_with(&format!("#define {name} ")))
            .unwrap_or_else(|| panic!("no #define {name}"));
        line.split_whitespace().nth(2).unwrap().to_string()
    };
    assert_eq!(define(&fs_h, "FS_MAGIC"), format!("{:#x}", camelot_fs::MAGIC));
    assert!(doc.contains(&format!("`{:#010x}`", camelot_fs::MAGIC)));
    assert_eq!(define(&fs_h, "FS_VERSION"), camelot_fs::VERSION.to_string());
    assert_eq!(define(&fs_h, "NDIRECT"), camelot_fs::NDIRECT.to_string());
    assert_eq!(define(&fs_h, "DIRSIZ"), camelot_fs::DIRSIZ.to_string());
    assert_eq!(define(&fs_h, "LOG_MAX"), camelot_fs::MAX_LOG.to_string());
    assert_eq!(define(&fs_h, "INODES_MAX"), camelot_fs::MAX_INODES.to_string());
    assert_eq!(define(&fs_h, "ROOTINO"), camelot_fs::ROOT_INODE.to_string());
    // mkfs's default log is the kernel's LOGSIZE slots and the header.
    assert_eq!(
        define(&log_h, "LOGSIZE").parse::<u32>().unwrap() + 1,
        camelot_fs::DEFAULT_LOG
    );
    assert!(doc.contains("`LOGSIZE` = 20"));
    assert!(doc.contains("`MAXOPBLOCKS` = 10"));
    assert_eq!(define(&log_h, "MAXOPBLOCKS"), "10");
    // The largest file, in the document's words.
    assert_eq!(camelot_fs::MAX_FILE_SIZE, 274_432);
    assert!(doc.contains("268\n  blocks, 274 432 bytes") || doc.contains("268 blocks, 274 432 bytes"));
}
