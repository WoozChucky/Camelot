//! The kernel's filesystem (issue #66, docs/camelot-fs.md), against the
//! host's implementation in the `camelot-fs` crate. The committed
//! `kernel/kernel.elf` boots on Grail with images built by mkfs on
//! in-memory disks that record every write; fsck and the reader check what
//! the kernel left, and the kernel's listing must be the host's.
//!
//! The crash tests stop the machine after each sector the self-test's
//! transactions write, check with fsck that the disk replayed is
//! consistent and holds all of each transaction or none of it, then reboot
//! the kernel on it and check it replays the log (and frees the inodes left
//! with no links) exactly as the host does.
//!
//! The file system calls (issue #67, docs/camelot-syscalls.md) are tested
//! the same way, from the shell: programs run from /bin on the disk, files
//! written and read through redirections and pipes, filetest's checks, and
//! a crash at every point of a sequence of calls that change the disk.

use camelot_fs::extent::{self, Blocks};
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
const FS_TEST_FULL: u32 = 3;
const FS_TEST_TREE: u32 = 4;
const FS_TEST_GROW: u32 = 5;

/// Checks the self-test runs (kernel/fstest.c): the read checks, one fewer
/// without a /bin, and the transactions'.
const READ_CHECKS: u32 = 12;
const READ_CHECKS_NO_BIN: u32 = 11;
const WRITE_CHECKS: u32 = 62;
const FULL_CHECKS: u32 = 8;
const TREE_CHECKS: u32 = 6;
const GROW_CHECKS: u32 = 7;
/// Transactions the write test commits (`COMMITS` in kernel/fstest.c).
const COMMITS: usize = 24;

/// The write test's files (kernel/fstest.c): lengths and pattern seeds.
const B_LEN: usize = 700;
const A_LEN: usize = 2500;
const BIG_LEN: usize = 13 * BSIZE + 100;
const P_LEN: usize = 1500;
/// The tree test's files, /t1 and /t2: blocks each (`T_LEN` in
/// kernel/fstest.c is this many blocks).
const T_BLOCKS: usize = 1200;
/// The grow test's files, /g1 and /g2: blocks each (`G_BLOCKS`).
const G_BLOCKS: usize = 340;

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
    /// Each sector written and its bytes, if asked for: the same, for disks
    /// too big to copy at every write (`at_each_write` replays them).
    writes: Vec<(u64, Vec<u8>)>,
}

/// An in-memory disk that records what it is asked to do.
struct Recorder {
    data: Vec<u8>,
    read_only: bool,
    snapshots: bool,
    writes: bool,
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
            if self.writes {
                record.writes.push((sector, s.to_vec()));
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
    /// Record each sector written (`Record::writes`).
    writes: bool,
    /// Stop the machine once this many sectors have been written.
    stop_after: Option<usize>,
    /// What is typed for the shell, which runs until it reads `exit`.
    input: &'static [u8],
    max_steps: u64,
}

const READ: Boot = Boot {
    read_only: false,
    fs_test: FS_TEST_READ,
    snapshots: false,
    writes: false,
    stop_after: None,
    input: b"exit\n",
    max_steps: BOOT_MAX_STEPS,
};

const WRITE: Boot = Boot {
    fs_test: FS_TEST_WRITE,
    ..READ
};

fn committed_elf() -> Vec<u8> {
    let path = repo_root().join("kernel").join("kernel.elf");
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// Boots the committed kernel with `image` as disk 0 and `how.input` typed
/// for the shell.
fn boot(image: Vec<u8>, how: Boot) -> Run {
    let record = Arc::new(Mutex::new(Record::default()));
    let disk = Recorder {
        data: image,
        read_only: how.read_only,
        snapshots: how.snapshots,
        writes: how.writes,
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
    m.set_uart_input(kernel::typed(how.input));
    let result = match how.stop_after {
        None => m.run(Some(how.max_steps), |_| {}),
        Some(n) => {
            let written = || {
                let r = record.lock().unwrap();
                r.ops.iter().filter(|o| matches!(o, Op::Write { .. })).count()
            };
            for _ in 0..how.max_steps {
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

/// The root filesystem `cargo xtask fs` builds, laid out as in the bootable
/// root image (`xtask::fs::IMAGE_SIZE` blocks less the kernel's), without
/// the kernel: the room the shell really has, which filetest's largest file
/// must fit in beside `/bin`.
fn root_image() -> Vec<u8> {
    let root = repo_root();
    let kernel = std::fs::read(root.join(xtask::fs::KERNEL)).unwrap();
    let opts = xtask::fs::bootable_layout(xtask::fs::IMAGE_SIZE, &kernel).unwrap();
    xtask::fs::build(&root, &opts).unwrap()
}

/// The layout of the small images, with mkfs's log (the kernel's LOGSIZE).
const SMALL: Options = Options {
    size: 160,
    ninodes: 32,
    nlog: camelot_fs::DEFAULT_LOG,
};

/// A small image without /bin: a README and an empty directory. Small, so
/// that the crash tests' snapshots are cheap.
fn small_image() -> Vec<u8> {
    small_image_with(&SMALL)
}

fn small_image_with(opts: &Options) -> Vec<u8> {
    let mut fs = Mkfs::new(opts).unwrap();
    fs.add_file("/README", b"a small disk\n").unwrap();
    fs.mkdir("/tmp").unwrap();
    fs.finish()
}

/// A small image holding only /filler, a file that leaves exactly `free`
/// data blocks free. mkfs allocates in order, so those are the last ones.
fn image_with_free_blocks(free: u32) -> Vec<u8> {
    let have = fsck::check(&Mkfs::new(&SMALL).unwrap().finish())
        .unwrap()
        .free_blocks;
    // The filler's blocks: one extent, in the root, so no tree block.
    let blocks = have - free;
    let mut fs = Mkfs::new(&SMALL).unwrap();
    fs.add_file("/filler", &pattern(blocks as usize * BSIZE, 9))
        .unwrap();
    let image = fs.finish();
    assert_eq!(fsck::check(&image).unwrap().free_blocks, free);
    image
}

/// `image` with its log slots zeroed: what is left to compare once a
/// transaction has gone through them.
fn without_log_slots(image: &[u8]) -> Vec<u8> {
    let sb = camelot_fs::superblock(image).unwrap();
    let mut out = image.to_vec();
    for b in sb.logstart + 1..sb.inodestart {
        camelot_fs::block_mut(&mut out, b).fill(0);
    }
    out
}

/// The programs the kernel builds in (not the disk-only ones), which its
/// self-test looks for in /bin.
fn programs() -> usize {
    xtask::userprogs::embedded_names(&xtask::userprogs::user_dir(&repo_root()))
        .unwrap()
        .len()
}

/// A disk's state, after replaying the log: every path, its inode, type,
/// links and contents, and the free blocks and inodes (which an inode with
/// no links, in no directory, still holds until it is freed).
type State = (Vec<(String, u32, u16, u16, Vec<u8>)>, u32, u32);

fn state(image: &[u8]) -> State {
    let fs = Fs::open(image).unwrap();
    let files = fs
        .walk()
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
        .collect();
    let r = fsck::check(image).unwrap();
    (files, r.free_blocks, r.free_inodes)
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
    commit_sizes(ops, sb, disk_blocks, disk_blocks - 1).len()
}

/// check_commits(), for a filesystem whose home blocks may reach `top` - 1
/// (the disk's last block, the disk self-test's scratch block, for one the
/// test fills to the end): the number of blocks of each commit, in order.
fn commit_sizes(ops: &[Op], sb: &Superblock, disk_blocks: u32, top: u32) -> Vec<u32> {
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
    let mut sizes = Vec::new();
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
            assert!(b >= sb.inodestart && b < top, "commit {commits}: home block {b}");
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
        sizes.push(slots);
    }
    sizes
}

#[test]
fn the_kernel_mounts_the_root_image_and_lists_it() {
    // Issue #66's acceptance test: mkfs builds the root image, the kernel
    // mounts it at boot and lists / in its self-test, as the host lists it;
    // every program in /bin reads back as the kernel's own copy.
    let image = root_image();
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
    // to its scratch block (the last), which on a disk with a filesystem
    // it writes with its own contents.
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
    // consistent, the host reads the files the kernel wrote and finds the
    // removed ones gone (one past its direct blocks, whose blocks all came
    // back), the kernel's listing of /fstest is the host's, and every commit
    // followed the protocol.
    let image = root_image();
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
    assert_eq!(fs.read_file("/fstest/p1").unwrap(), pattern(P_LEN, 4));
    assert_eq!(fs.read_file("/fstest/p2").unwrap(), pattern(P_LEN, 5));
    for gone in ["a", "x", "big", "d"] {
        assert!(fs.lookup(&format!("/fstest/{gone}")).is_err(), "{gone}");
    }
    let names: Vec<String> = fs.list("/fstest").unwrap().iter().map(|e| e.name()).collect();
    // Free slots are reused: x took a's, then d x's, then p1 d's.
    assert_eq!(
        names,
        [".", "..", "p1", "b", "p2", "r1", "r2", "j3", "j2", "c1", "c2"]
    );
    assert_eq!((after.dirs, after.files), (before.dirs + 1, before.files + 9));
    assert_eq!(after.orphans, 0);
    // /fstest's block, b's, and p1's and p2's two each.
    assert_eq!(after.free_blocks, before.free_blocks - 6);
    assert_eq!(after.free_inodes, before.free_inodes - 10);
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
    // Then the kernel boots on each one, replays its log and frees an inode
    // left with no links (the write test removes /fstest/x while holding a
    // reference to it, and frees it a transaction later) as the host does
    // (the disk afterwards is byte for byte the host's, but for the log
    // slots that freeing went through) and passes its self-test.
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
    assert!(
        snapshots.iter().any(|s| fsck::check(s).unwrap().orphans == 1),
        "no disk with an orphan"
    );
    assert_eq!(commits.len(), COMMITS);
    assert_eq!(states[1..], commits[..]);
    // The host reads /fstest/big, past its direct blocks, as the kernel
    // wrote it, before it is removed.
    let big = pattern(BIG_LEN, 3);
    assert!(states
        .iter()
        .any(|(files, _, _)| files.iter().any(|f| f.0 == "/fstest/big" && f.4 == big)));
    assert_eq!(states.last().unwrap(), &state(&run.disk));

    reboot_each(&snapshots, READ_CHECKS_NO_BIN);
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
        (broken(&|i| set_u32(i, 1, 4, 1)), "unknown version"),
        (broken(&|i| set_u32(i, 1, 20, 1)), "bad log size"),
        (broken(&|i| set_u32(i, 1, 16, 1)), "bad inode count"),
        (
            broken(&|i| set_u32(i, 1, 36, sb.datastart + 1)),
            "the regions are misplaced",
        ),
        (broken(&|i| i.truncate(100 * BSIZE)), "bigger than the disk"),
        (
            broken(&|i| i[BSIZE + 100] = 0xff),
            "the superblock's bytes past its fields are not zero",
        ),
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
        let root = camelot_fs::data_blocks(i, &sb, &camelot_fs::read_inode(i, &sb, 1)).unwrap()[0];
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
fn a_full_disk_leaves_the_extent_tree_as_it_was() {
    // 13 free blocks: /full and /pad take one each in turn, four times, so
    // /full is four extents, and /pad all but one of the rest. /full's next
    // block needs that one and a tree block for the root's entries to move
    // to, which there isn't: the write fails and changes nothing, the block
    // it took given back. Then, with /pad gone, /full fills the disk with
    // its tree a level deeper (kernel/fstest.c, full_test).
    let image = image_with_free_blocks(13);
    let before = fsck::check(&image).unwrap();
    let run = boot(
        image,
        Boot {
            fs_test: FS_TEST_FULL,
            ..READ
        },
    );
    assert_eq!(run.result, RunResult::Exited(0), "{}", run.out);
    let lines = fs_lines(&run.out);
    // All 13 blocks: 12 of data, and the tree block.
    assert!(
        lines.ends_with(&format!(
            "fs: the disk filled up at {} bytes\n{}",
            12 * BSIZE,
            passed(READ_CHECKS_NO_BIN + FULL_CHECKS)
        )),
        "{lines}"
    );
    let after = fsck::check(&run.disk).unwrap_or_else(|p| panic!("{p:#?}"));
    assert_eq!(after.free_blocks, 0);
    assert_eq!(after.files, before.files + 1);
    let fs = Fs::open(&run.disk).unwrap();
    assert_eq!(fs.read_file("/full").unwrap(), pattern(12 * BSIZE, 7));
    let d = fs.inode(fs.lookup("/full").unwrap()).unwrap();
    let map = extent::walk(fs.image(), fs.superblock(), &d.root).unwrap();
    assert_eq!(extent::Header::decode(&d.root).depth, 1);
    assert_eq!(map.tree_blocks.len(), 1);
}

/// The blocks of an image, for growing extent trees on it as mkfs would
/// but in any order: blocks come from the first free one.
struct Image<'a> {
    image: &'a mut [u8],
    sb: Superblock,
}

impl Blocks for Image<'_> {
    fn block(&self, n: u32) -> &[u8] {
        block(self.image, n)
    }
    fn block_mut(&mut self, n: u32) -> &mut [u8] {
        camelot_fs::block_mut(self.image, n)
    }
    fn alloc(&mut self, _goal: u32) -> Result<u32, String> {
        for b in self.sb.datastart..self.sb.size {
            let (blk, byte, mask) = self.sb.bit_pos(b);
            if block(self.image, blk)[byte] & mask == 0 {
                camelot_fs::block_mut(self.image, blk)[byte] |= mask;
                camelot_fs::block_mut(self.image, b).fill(0);
                return Ok(b);
            }
        }
        Err("full".into())
    }
    fn free(&mut self, b: u32) {
        let (blk, byte, mask) = self.sb.bit_pos(b);
        camelot_fs::block_mut(self.image, blk)[byte] &= !mask;
    }
}

/// The tree test's disk (kernel/fstest.c, tree_test): a README, then /t1 and
/// /t2, T_BLOCKS blocks each of the pattern with seeds 11 and 12, whose
/// blocks alternate, so every block is an extent of its own and each tree
/// is two deep. The host's extent code grows them, so the kernel works on
/// trees it did not build.
fn tree_image() -> Vec<u8> {
    tree_image_with_log(camelot_fs::DEFAULT_LOG, &[])
}

/// The same, with a log of `nlog` blocks, and `programs` in /bin.
fn tree_image_with_log(nlog: u32, programs: &[&str]) -> Vec<u8> {
    let opts = Options {
        size: 2 * T_BLOCKS as u32 + 200,
        ninodes: 16,
        nlog,
    };
    let mut fs = Mkfs::new(&opts).unwrap();
    fs.add_file("/README", b"trees\n").unwrap();
    if !programs.is_empty() {
        fs.mkdir("/bin").unwrap();
        for name in programs {
            fs.add_file(&format!("/bin/{name}"), &user_program(name)).unwrap();
        }
    }
    let files = [
        (fs.add_file("/t1", b"").unwrap(), pattern(T_BLOCKS * BSIZE, 11)),
        (fs.add_file("/t2", b"").unwrap(), pattern(T_BLOCKS * BSIZE, 12)),
    ];
    let mut image = fs.finish();
    let sb = camelot_fs::superblock(&image).unwrap();
    let mut disk = Image {
        image: &mut image,
        sb,
    };
    for k in 0..T_BLOCKS {
        for (inum, data) in &files {
            let mut d = camelot_fs::read_inode(disk.image, &sb, *inum);
            let b = disk.alloc(0).unwrap();
            disk.block_mut(b)
                .copy_from_slice(&data[k * BSIZE..(k + 1) * BSIZE]);
            extent::append(&mut disk, &mut d.root, k as u32, b).unwrap();
            d.size += BSIZE as u64;
            camelot_fs::write_inode(disk.image, &sb, *inum, &d);
        }
    }
    let r = fsck::check(&image).unwrap_or_else(|p| panic!("{p:#?}"));
    assert_eq!(r.files, 3 + programs.len() as u32);
    let fs = Fs::open(&image).unwrap();
    for (inum, data) in &files {
        let d = fs.inode(*inum).unwrap();
        assert_eq!(extent::Header::decode(&d.root).depth, 2);
        assert_eq!(&fs.contents(*inum).unwrap(), data);
    }
    image
}

const TREE: Boot = Boot {
    // It reads 2.4 MB back through the disk, block by block, and frees 2400
    // blocks: far longer than a boot.
    max_steps: 20 * BOOT_MAX_STEPS,
    fs_test: FS_TEST_TREE,
    ..READ
};

#[test]
fn the_kernel_reads_grows_and_frees_trees_the_host_built() {
    // kernel/fstest.c's tree test: /t1 and /t2 read back, a block added to
    // /t1 too, removing /t1 takes more than one transaction (each leaf it
    // empties takes a log slot, and there are 15), emptying /t2 as O_TRUNC
    // does leaves it empty. Afterwards only the README and the root's blocks
    // are in use, and every commit followed the protocol.
    let image = tree_image();
    let before = fsck::check(&image).unwrap();
    let run = boot(image, TREE);
    assert_eq!(run.result, RunResult::Exited(0), "{}", run.out);
    assert!(
        fs_lines(&run.out).ends_with(&passed(READ_CHECKS_NO_BIN + TREE_CHECKS)),
        "{}",
        run.out
    );
    let after = fsck::check(&run.disk).unwrap_or_else(|p| panic!("{p:#?}"));
    assert_eq!((after.orphans, after.truncating, after.replayed), (0, 0, 0));
    assert_eq!((after.files, after.used_blocks), (before.files - 1, 2));
    let fs = Fs::open(&run.disk).unwrap();
    assert!(fs.lookup("/t1").is_err());
    assert_eq!(fs.read_file("/t2").unwrap(), b"");
    let disk_blocks = (run.disk.len() / BSIZE) as u32;
    assert!(check_commits(&run.record.ops, &after.sb, disk_blocks) >= 5);
}

/// Calls `f` with the disk as it stood after each sector of `writes`, from
/// `base`, in parallel: each thread replays the writes up to the start of
/// its share, then one at a time.
fn at_each_write(base: &[u8], writes: &[(u64, Vec<u8>)], f: impl Fn(usize, &[u8]) + Sync) {
    let threads = std::thread::available_parallelism()
        .map_or(4, |n| n.get())
        .min(16);
    let chunk = writes.len().div_ceil(threads);
    let f = &f;
    std::thread::scope(|scope| {
        for c in 0..threads {
            scope.spawn(move || {
                let mut disk = base.to_vec();
                for (i, (sector, data)) in writes.iter().enumerate().take((c + 1) * chunk) {
                    let at = *sector as usize * SECTOR_SIZE;
                    disk[at..at + SECTOR_SIZE].copy_from_slice(data);
                    if i >= c * chunk {
                        f(i, &disk);
                    }
                }
            });
        }
    });
}

#[test]
fn a_crash_during_a_truncation_leaves_it_for_the_next_mount() {
    // The tree test stopped after every sector it writes. Replayed, every
    // disk passes fsck and holds the state of a commit. Some hold /t2 being
    // truncated (TRUNCATING set, a size of 0, blocks still to free) and
    // /t1 an orphan with blocks still to free; the kernel, booted on each
    // disk, finishes the truncation and frees the orphan as fsck --recover
    // does.
    let image = tree_image();
    let run = boot(image.clone(), Boot { writes: true, ..TREE });
    assert_eq!(run.result, RunResult::Exited(0), "{}", run.out);
    let writes = run.record.writes;
    let seen = Mutex::new(Vec::new());
    at_each_write(&image, &writes, |i, disk| {
        let r = fsck::check(disk).unwrap_or_else(|p| panic!("stopped after sector write {i}: {p:#?}"));
        seen.lock()
            .unwrap()
            .push((i, r.replayed > 0, r.truncating, r.orphans, state(disk)));
        reboot_one(i, disk, READ_CHECKS_NO_BIN);
    });
    let mut seen = seen.into_inner().unwrap();
    seen.sort_by_key(|s| s.0);
    // The state changes only at a commit point: when a header with n > 0
    // is written, to that commit's state.
    let mut states = vec![state(&image)];
    let mut commits = Vec::new();
    for (_, pending, _, _, s) in &seen {
        if *pending && commits.last() != Some(s) {
            commits.push(s.clone());
        }
        if states.last() != Some(s) {
            states.push(s.clone());
        }
    }
    assert_eq!(states[1..], commits[..]);
    assert!(
        seen.iter().any(|s| s.2 > 0 && s.3 == 0),
        "no disk with a file part way through its truncation"
    );
    assert!(seen.iter().any(|s| s.3 > 0), "no disk with an orphan");
    assert_eq!(states.last().unwrap(), &state(&run.disk));
}

/// The grow test's disk (kernel/fstest.c, grow_test): one bitmap block, a
/// README, and room for /g1 and /g2 and /pad.
fn grow_image() -> Vec<u8> {
    let mut fs = Mkfs::new(&Options {
        size: 900,
        ninodes: 16,
        nlog: camelot_fs::DEFAULT_LOG,
    })
    .unwrap();
    fs.add_file("/README", b"growing\n").unwrap();
    fs.finish()
}

const GROW: Boot = Boot {
    // 680 appends and as many transactions, and /pad's.
    max_steps: 20 * BOOT_MAX_STEPS,
    fs_test: FS_TEST_GROW,
    ..READ
};

#[test]
fn the_kernel_grows_trees_through_every_split() {
    // kernel/fstest.c's grow test: /g1 and /g2 written a block at a time in
    // turn grow from depth 0 to 1, split a full leaf, and grow to depth 2,
    // each append logging what docs/camelot-fs.md counts for it; the growth
    // to depth 2 fails cleanly first with a block too few. The host reads
    // both back and finds two trees of depth 2.
    let image = grow_image();
    let run = boot(image, GROW);
    assert_eq!(run.result, RunResult::Exited(0), "{}", run.out);
    assert!(
        fs_lines(&run.out).ends_with(&passed(READ_CHECKS_NO_BIN + GROW_CHECKS)),
        "{}",
        run.out
    );
    let after = fsck::check(&run.disk).unwrap_or_else(|p| panic!("{p:#?}"));
    assert_eq!((after.orphans, after.truncating, after.files), (0, 0, 3));
    let fs = Fs::open(&run.disk).unwrap();
    assert!(fs.lookup("/pad").is_err());
    for (name, seed) in [("/g1", 21), ("/g2", 22)] {
        assert_eq!(
            fs.read_file(name).unwrap(),
            pattern(G_BLOCKS * BSIZE, seed),
            "{name}"
        );
        let d = fs.inode(fs.lookup(name).unwrap()).unwrap();
        let map = extent::walk(fs.image(), fs.superblock(), &d.root).unwrap();
        assert_eq!(extent::Header::decode(&d.root).depth, 2, "{name}");
        assert_eq!(map.extents.len(), G_BLOCKS, "{name}: an extent per block");
    }
    let disk_blocks = (run.disk.len() / BSIZE) as u32;
    // The grow test fills the disk to its last block, which the disk
    // self-test wrote back as it was before the mount.
    assert!(commit_sizes(&run.record.ops, &after.sb, disk_blocks, disk_blocks).len() > 2 * G_BLOCKS);
}

#[test]
fn a_crash_while_a_tree_grows_leaves_whole_transactions() {
    // The grow test stopped after every sector it writes: replayed, every
    // disk passes fsck and holds the state of a commit. Around each growth
    // (the trees' 5th, 85th and 337th extents: depth 0 to 1, a leaf split,
    // depth 1 to 2), the kernel boots on each disk and recovers it as the
    // host does.
    let image = grow_image();
    let run = boot(image.clone(), Boot { writes: true, ..GROW });
    assert_eq!(run.result, RunResult::Exited(0), "{}", run.out);
    let writes = run.record.writes;
    let near_growth = |s: &State| {
        s.0.iter().any(|f| {
            (f.0 == "/g1" || f.0 == "/g2")
                && [4, 84, 336]
                    .iter()
                    .any(|&k| (k * BSIZE..=(k + 2) * BSIZE).contains(&f.4.len()))
        })
    };
    let seen = Mutex::new(Vec::new());
    let rebooted = std::sync::atomic::AtomicUsize::new(0);
    at_each_write(&image, &writes, |i, disk| {
        let r = fsck::check(disk).unwrap_or_else(|p| panic!("stopped after sector write {i}: {p:#?}"));
        let s = state(disk);
        if near_growth(&s) {
            reboot_one(i, disk, READ_CHECKS_NO_BIN);
            rebooted.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        seen.lock().unwrap().push((i, r.replayed > 0, s));
    });
    let mut seen = seen.into_inner().unwrap();
    seen.sort_by_key(|s| s.0);
    let mut states = vec![state(&image)];
    let mut commits = Vec::new();
    for (_, pending, s) in &seen {
        if *pending && commits.last() != Some(s) {
            commits.push(s.clone());
        }
        if states.last() != Some(s) {
            states.push(s.clone());
        }
    }
    assert_eq!(states[1..], commits[..]);
    assert_eq!(states.last().unwrap(), &state(&run.disk));
    // Both files, through all three growths, a dozen points or more each.
    assert!(rebooted.into_inner() > 6 * 12);
}

#[test]
fn a_file_unlinked_on_the_smallest_writable_log_is_freed_in_transactions_of_its_own() {
    // The tree test's disk with the smallest log the kernel writes to (a
    // header and MAXOPBLOCKS slots), and its files removed by the shell, open
    // nowhere: the unlink and the freeing of /t1's blocks are separate
    // transactions (as Linux's unlink and eviction are), each step of the
    // freeing a full transaction of its own (its 15 leaves, emptied, take a
    // slot each), so none overflows the log.
    let image = tree_image_with_log(23, &["rm"]);
    let run = boot(
        image,
        Boot {
            input: b"rm /t1 /t2\nexit\n",
            max_steps: 20 * BOOT_MAX_STEPS,
            ..READ
        },
    );
    assert_eq!(run.result, RunResult::Exited(0), "{}", run.out);
    assert_eq!(shell_output(&run.out, "rm /t1 /t2"), "");
    let after = fsck::check(&run.disk).unwrap_or_else(|p| panic!("{p:#?}"));
    let rm = Fs::open(&run.disk)
        .unwrap()
        .read_file("/bin/rm")
        .unwrap()
        .len()
        .div_ceil(BSIZE) as u32;
    // The README's block, /bin's and the root's, and rm's.
    assert_eq!((after.files, after.used_blocks, after.orphans), (2, 3 + rm, 0));
    let disk_blocks = (run.disk.len() / BSIZE) as u32;
    // The unlink of /t1 commits alone (its entry's block and the inode block
    // holding the root's and /t1's inodes), before any of the freeing, which
    // takes more than one transaction, its first full (a leaf a slot, and
    // the bitmap): had they shared one, it would hold them all.
    let sizes = commit_sizes(&run.record.ops, &after.sb, disk_blocks, disk_blocks - 1);
    assert_eq!(sizes[0], 2, "{sizes:?}");
    assert!(sizes[1] > 12, "{sizes:?}");
    assert!(sizes.len() >= 6, "{sizes:?}");
}

#[test]
fn a_file_past_what_the_kernels_offsets_reach_fails_to_open_with_eoverflow() {
    // A file whose size passes 2^31 - 1 bytes, which the kernel's 32-bit
    // offsets cannot reach (docs/camelot-syscalls.md, open). A real one
    // would need a 2 GiB disk; the kernel checks the size before it reads
    // a block, so an inode that only says so is enough (fsck would call the
    // disk broken: the size needs blocks it does not have).
    let mut fs = Mkfs::new(&Options { size: 320, ..SMALL }).unwrap();
    fs.add_file("/README", b"a small disk\n").unwrap();
    fs.mkdir("/bin").unwrap();
    fs.add_file("/bin/cat", &user_program("cat")).unwrap();
    let huge = fs.add_file("/huge", b"").unwrap();
    let mut image = fs.finish();
    let sb = camelot_fs::superblock(&image).unwrap();
    let mut d = camelot_fs::read_inode(&image, &sb, huge);
    d.size = 1 << 31;
    camelot_fs::write_inode(&mut image, &sb, huge, &d);
    let run = boot(
        image,
        Boot {
            input: b"cat /huge\ncat /README\nexit\n",
            max_steps: SHELL_MAX_STEPS,
            ..READ
        },
    );
    assert_eq!(run.result, RunResult::Exited(0), "{}", run.out);
    assert_eq!(
        shell_output(&run.out, "cat /huge"),
        "cat: /huge: value too large for defined data type\ncat: exit 1\n"
    );
}

#[test]
fn a_log_too_small_for_an_operation_is_mounted_read_only() {
    // The format allows 1 to 127 log slots; an operation may change
    // MAXOPBLOCKS (22) blocks. With fewer slots the kernel only reads the
    // disk, rather than panic on the first big transaction.
    let image = small_image_with(&Options { nlog: 22, ..SMALL });
    let report = fsck::check(&image).unwrap();
    let run = boot(image.clone(), WRITE);
    assert_eq!(run.result, RunResult::Exited(0), "{}", run.out);
    assert_eq!(
        fs_lines(&run.out),
        format!(
            "{}fs: ls /\n{}fs: write test skipped: the log is too small to write\n{}",
            mount_line(&report, false).replace("\n", ", read-only: the log is too small to write\n"),
            Fs::open(&image).unwrap().ls("/").unwrap(),
            passed(READ_CHECKS_NO_BIN)
        )
    );
    assert!(run.disk == image);
    // 23 blocks, a header and MAXOPBLOCKS slots, is enough (for one
    // operation at a time): the disk is filled a transaction at a time.
    let image = small_image_with(&Options { nlog: 23, ..SMALL });
    let run = boot(
        image,
        Boot {
            fs_test: FS_TEST_FULL,
            ..READ
        },
    );
    assert_eq!(run.result, RunResult::Exited(0), "{}", run.out);
    assert!(
        run.out.contains(&passed(READ_CHECKS_NO_BIN + FULL_CHECKS)),
        "{}",
        run.out
    );
    let r = fsck::check(&run.disk).unwrap_or_else(|p| panic!("{p:#?}"));
    assert_eq!(r.free_blocks, 0);
}

#[test]
fn the_disk_self_test_never_changes_a_filesystems_blocks() {
    // The disk self-test writes its scratch block, the disk's last, before
    // the filesystem is mounted. Here that block is /filler's: wherever the
    // machine stops, the disk holds what it held, so the file is intact.
    let image = image_with_free_blocks(0);
    let last = (image.len() / BSIZE) as u64 - 1;
    let full = boot(image.clone(), READ);
    assert_eq!(full.result, RunResult::Exited(0), "{}", full.out);
    assert!(full.disk == image);
    let writes: Vec<u64> = full
        .record
        .ops
        .iter()
        .filter_map(|o| match o {
            Op::Write { sector, .. } => Some(*sector),
            Op::Flush => None,
        })
        .collect();
    assert_eq!(
        writes,
        [2 * last, 2 * last + 1, 2 * last, 2 * last + 1],
        "the scratch block, written twice"
    );
    // A block's two sectors go to the disk in one request.
    for n in (2..=writes.len()).step_by(2) {
        let run = boot(
            image.clone(),
            Boot {
                stop_after: Some(n),
                ..READ
            },
        );
        assert!(
            run.disk == image,
            "stopped after sector write {n}: the disk changed"
        );
    }
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
    assert!(doc.starts_with(&format!(
        "# The Camelot filesystem (CMFS), version {}\n",
        camelot_fs::VERSION
    )));
    assert_eq!(define(&fs_h, "DIRSIZ"), camelot_fs::DIRSIZ.to_string());
    // The extent trees: ext4's magic and sizes, on both sides and in the
    // document.
    assert_eq!(define(&fs_h, "EXT_MAGIC"), format!("{:#x}", extent::MAGIC));
    assert!(doc.contains("0xF30A"));
    assert_eq!(define(&fs_h, "EXT_ROOT_MAX"), extent::ROOT_MAX.to_string());
    assert_eq!(define(&fs_h, "EXT_MAX_DEPTH"), extent::MAX_DEPTH.to_string());
    assert_eq!(define(&fs_h, "EXT_MAX_LEN"), extent::MAX_LEN.to_string());
    assert!(doc.contains(&format!("(1024 - 12) / 12 = {}", extent::BLOCK_MAX)));
    assert!(doc.contains("blocks it maps, 1 to 32 768"));
    assert!(doc.contains(&format!("{} bytes, 8 per block", camelot_fs::INODE_SIZE)));
    assert_eq!(
        define(&fs_h, "INODE_TRUNCATING"),
        camelot_fs::TRUNCATING.to_string()
    );
    assert_eq!(define(&fs_h, "LOG_MAX"), camelot_fs::MAX_LOG.to_string());
    assert_eq!(define(&fs_h, "INODES_MAX"), camelot_fs::MAX_INODES.to_string());
    assert_eq!(define(&fs_h, "ROOTINO"), camelot_fs::ROOT_INODE.to_string());
    // mkfs's default log is the kernel's LOGSIZE slots and the header.
    assert_eq!(
        define(&log_h, "LOGSIZE").parse::<u32>().unwrap() + 1,
        camelot_fs::DEFAULT_LOG
    );
    assert!(doc.contains("`LOGSIZE` = 44"));
    assert!(doc.contains("`MAXOPBLOCKS` = 22"));
    assert_eq!(define(&log_h, "MAXOPBLOCKS"), "22");
    assert!(doc.contains("(`nlog` < 23)"));
    // The largest file: the format's, and the kernel's for now.
    assert_eq!(camelot_fs::MAX_FILE_SIZE, 4 << 40);
    assert!(doc.contains("2^32 blocks (its logical block numbers are\n  u32), 4 TiB"));
    assert_eq!(define(&fs_h, "MAXFILE_BYTES"), "0x7fffffffu");
    assert!(doc.contains("writes files of at most 2^31 - 1\n  bytes"));
}

// ---- The file system calls (issue #67) ----

/// Steps for a boot whose shell runs programs from the disk.
const SHELL_MAX_STEPS: u64 = 400_000_000;

/// The shell's prompt.
const PROMPT: &str = "$ ";

/// Checks `filetest` makes on the root image, and `filetest ro` on a
/// read-only one (kernel/user/filetest.c).
const FILETEST_CHECKS: u32 = 168;
const FILETEST_RO_CHECKS: u32 = 14;

/// What the shell printed after reading `command`, up to its next prompt.
fn shell_output<'a>(out: &'a str, command: &str) -> &'a str {
    let typed = format!("{PROMPT}{command}\n");
    let start = out
        .find(&typed)
        .unwrap_or_else(|| panic!("no `{command}` in:\n{out}"))
        + typed.len();
    let end = out[start..].find(PROMPT).map_or(out.len(), |e| start + e);
    &out[start..end]
}

/// A program of kernel/user, as `cargo xtask fs` puts it in /bin.
fn user_program(name: &str) -> Vec<u8> {
    xtask::userprogs::build_named(&xtask::userprogs::user_dir(&repo_root()), name)
        .unwrap()
        .elf
}

/// A small image with /README, /dev/console, a device node with no driver,
/// and `programs` in /bin.
fn small_image_with_programs(programs: &[&str]) -> Vec<u8> {
    let mut fs = Mkfs::new(&Options { size: 320, ..SMALL }).unwrap();
    fs.add_file("/README", b"a small disk\n").unwrap();
    fs.mkdir("/dev").unwrap();
    fs.add_device("/dev/console", xtask::fs::CONSOLE_MAJOR, 0)
        .unwrap();
    fs.add_device("/dev/nodriver", 7, 0).unwrap();
    fs.mkdir("/bin").unwrap();
    for name in programs {
        fs.add_file(&format!("/bin/{name}"), &user_program(name)).unwrap();
    }
    fs.finish()
}

/// Boots the kernel on each of `snapshots` (disks a machine stopped
/// writing to), several machines at a time, and checks each one: the kernel
/// replays the log and frees the inodes no directory names as the host
/// does (byte for byte, but for the log slots that freeing went through),
/// passes its read checks (`read_checks` of them), and leaves nothing to
/// recover.
fn reboot_each(snapshots: &[Vec<u8>], read_checks: u32) {
    let threads = std::thread::available_parallelism()
        .map_or(4, |n| n.get())
        .min(16);
    let chunk = snapshots.len().div_ceil(threads);
    std::thread::scope(|scope| {
        for (c, part) in snapshots.chunks(chunk).enumerate() {
            scope.spawn(move || {
                for (j, snap) in part.iter().enumerate() {
                    reboot_one(c * chunk + j, snap, read_checks);
                }
            });
        }
    });
}

/// Boots the kernel on `snap`, a disk as the machine left it after sector
/// write `i`, and checks that its mount recovers it as the host does:
/// replays the log, finishes the truncations, frees the orphans, says so,
/// and leaves the host's disk (but for the log slots and the free blocks'
/// bytes, which the kernel's transactions may have gone through), which
/// fsck finds clean.
fn reboot_one(i: usize, snap: &[u8], read_checks: u32) {
    let replayed = fsck::check(snap).unwrap().replayed;
    let mut want = snap.to_vec();
    fsck::recover(&mut want).unwrap();
    let truncations = fsck::finish_truncations(&mut want).unwrap();
    let orphans = fsck::free_orphans(&mut want).unwrap();
    let again = boot(snap.to_vec(), READ);
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
    assert_eq!(
        lines.contains(&format!("fs: finished {truncations} truncations\n")),
        truncations > 0,
        "after write {i}:\n{lines}"
    );
    assert_eq!(
        lines.contains(&format!("fs: freed {orphans} inodes that no directory named\n")),
        orphans > 0,
        "after write {i}:\n{lines}"
    );
    assert!(lines.ends_with(&passed(read_checks)), "after write {i}:\n{lines}");
    if orphans == 0 && truncations == 0 {
        assert!(
            again.disk == want,
            "after write {i}: the kernel's replay is not the host's"
        );
    } else {
        assert!(
            without_free_blocks(&again.disk) == without_free_blocks(&want),
            "after write {i}: the kernel's recovery is not the host's"
        );
    }
    let r = fsck::check(&again.disk).unwrap();
    assert_eq!(
        (r.replayed, r.orphans, r.truncating),
        (0, 0, 0),
        "after write {i}"
    );
}

/// `image` with its log slots and the blocks its bitmap marks free zeroed:
/// what two recoveries must agree on, when one went through transactions
/// (which use the slots, and may have changed a tree block before freeing
/// it) and the other did not.
fn without_free_blocks(image: &[u8]) -> Vec<u8> {
    let mut out = without_log_slots(image);
    let sb = camelot_fs::superblock(image).unwrap();
    for b in sb.datastart..sb.size {
        let (blk, byte, mask) = sb.bit_pos(b);
        if block(image, blk)[byte] & mask == 0 {
            camelot_fs::block_mut(&mut out, b).fill(0);
        }
    }
    out
}

#[test]
fn the_shell_runs_programs_from_bin_and_redirects_through_files_and_pipes() {
    // Issue #67's acceptance test: on the root image, the shell runs ls from
    // /bin, writes a file with `>`, reads it back through a pipe and with
    // `<`, and runs a program by a path of its own (a link to /bin/echo,
    // which only the disk has). The host reads what the kernel wrote, fsck
    // finds the disk consistent, and every change was a whole transaction.
    let image = root_image();
    let before = fsck::check(&image).unwrap();
    let bin = Fs::open(&image).unwrap().ls("/bin").unwrap();
    let run = boot(
        image,
        Boot {
            input: b"ls /bin\n\
                echo hello world > /hello.txt\n\
                cat /hello.txt | wc\n\
                cat < /hello.txt | cat\n\
                wc < /hello.txt > /count\n\
                ln /bin/echo /greet\n\
                /greet from the root\n\
                cat /missing | wc\n\
                echo hi > /hello.txt\n\
                exit\n",
            max_steps: SHELL_MAX_STEPS,
            ..READ
        },
    );
    assert_eq!(run.result, RunResult::Exited(0), "{}", run.out);
    let out = &run.out;
    assert_eq!(shell_output(out, "ls /bin"), bin);
    assert_eq!(shell_output(out, "echo hello world > /hello.txt"), "");
    assert_eq!(shell_output(out, "cat /hello.txt | wc"), "1 2 12\n");
    assert_eq!(shell_output(out, "cat < /hello.txt | cat"), "hello world\n");
    assert_eq!(shell_output(out, "wc < /hello.txt > /count"), "");
    assert_eq!(shell_output(out, "ln /bin/echo /greet"), "");
    assert_eq!(shell_output(out, "/greet from the root"), "from the root\n");
    // cat's error goes to the console; wc reads nothing.
    let missing = shell_output(out, "cat /missing | wc");
    assert!(
        missing.contains("cat: /missing: no such file or directory\n") && missing.contains("0 0 0\n"),
        "{missing}"
    );

    let after = fsck::check(&run.disk).unwrap_or_else(|p| panic!("{p:#?}"));
    assert_eq!((after.replayed, after.orphans), (0, 0));
    assert_eq!((after.dirs, after.files), (before.dirs, before.files + 2));
    let fs = Fs::open(&run.disk).unwrap();
    // `>` empties the file it writes: nothing of the longer line is left.
    assert_eq!(shell_output(out, "echo hi > /hello.txt"), "");
    assert_eq!(fs.read_file("/hello.txt").unwrap(), b"hi\n");
    assert_eq!(fs.read_file("/count").unwrap(), b"1 2 12\n");
    let echo = fs.lookup("/bin/echo").unwrap();
    assert_eq!(fs.lookup("/greet").unwrap(), echo);
    assert_eq!(fs.inode(echo).unwrap().nlink, 2);
    let disk_blocks = (run.disk.len() / BSIZE) as u32;
    assert!(check_commits(&run.record.ops, &after.sb, disk_blocks) > 0);
}

#[test]
fn filetest_passes_on_the_root_image_and_leaves_it_as_it_was() {
    // Every file system call and its errors (kernel/user/filetest.c), on
    // the root image. filetest removes what it made: the disk ends as it
    // began, consistent, with every block and inode back.
    let image = root_image();
    let run = boot(
        image.clone(),
        Boot {
            input: b"filetest\nexit\n",
            max_steps: SHELL_MAX_STEPS,
            ..READ
        },
    );
    assert_eq!(run.result, RunResult::Exited(0), "{}", run.out);
    let out = shell_output(&run.out, "filetest");
    assert!(
        out.ends_with(&format!("filetest: {FILETEST_CHECKS} checks, 0 failed\n")),
        "{out}"
    );
    // The kernel says why a file it was asked to run is not a program.
    assert!(out.contains("spawn: /ft/notelf: not an ELF file\n"), "{out}");
    assert!(out.contains("exec: /ft/notelf: not an ELF file\n"), "{out}");
    let after = fsck::check(&run.disk).unwrap_or_else(|p| panic!("{p:#?}"));
    assert_eq!((after.replayed, after.orphans), (0, 0));
    assert_eq!(state(&run.disk), state(&image));
    let disk_blocks = (run.disk.len() / BSIZE) as u32;
    assert!(check_commits(&run.record.ops, &after.sb, disk_blocks) > 0);
}

#[test]
fn stress_on_the_root_image_leaves_it_as_it_was() {
    // Issue #61's stress program with a filesystem: six workers, each
    // also writing, reading back and removing a file of its own every
    // round, side by side with the pipe and the page allocator, under
    // timer preemption. The transactions interleave in the log; the disk
    // ends consistent and as it began. That ticks really do preempt the
    // workers mid-round is checked by kernel.rs's
    // stress_runs_many_processes_through_the_locks, which watches the hart.
    let image = root_image();
    let run = boot(
        image.clone(),
        Boot {
            input: b"stress 6 3\nexit\n",
            max_steps: SHELL_MAX_STEPS,
            ..READ
        },
    );
    assert_eq!(run.result, RunResult::Exited(0), "{}", run.out);
    let out = shell_output(&run.out, "stress 6 3");
    assert!(
        out.ends_with("stress: 6 workers, 3 rounds, 3600 bytes: ok\n"),
        "{out}"
    );
    for w in 1..=6 {
        for r in 1..=3 {
            assert_eq!(
                out.matches(&format!("stress {w}: round {r}\n")).count(),
                1,
                "{out}"
            );
        }
    }
    let after = fsck::check(&run.disk).unwrap_or_else(|p| panic!("{p:#?}"));
    assert_eq!((after.replayed, after.orphans), (0, 0));
    assert_eq!(state(&run.disk), state(&image));
    let disk_blocks = (run.disk.len() / BSIZE) as u32;
    // A file, its directory entry and its inode each round: 18 at least.
    assert!(check_commits(&run.record.ops, &after.sb, disk_blocks) >= 18);
}

#[test]
fn a_crash_during_the_file_system_calls_leaves_whole_transactions() {
    // The crash test of issue #66, for the calls that change the disk:
    // `filetest ops` makes directories and files, links, truncates and
    // removes them (one while it is open, which leaves an orphan until it
    // is closed), from the shell. The machine is stopped after each sector
    // written; every disk replayed is consistent and holds the state before
    // or after each commit, never one in between; and the kernel, rebooted
    // on each one, recovers it as the host does.
    let image = small_image_with_programs(&["filetest"]);
    let before = fsck::check(&image).unwrap();
    let run = boot(
        image.clone(),
        Boot {
            snapshots: true,
            input: b"filetest ops\nexit\n",
            max_steps: SHELL_MAX_STEPS,
            ..READ
        },
    );
    assert_eq!(run.result, RunResult::Exited(0), "{}", run.out);
    assert_eq!(shell_output(&run.out, "filetest ops"), "filetest: ops done\n");
    let after = fsck::check(&run.disk).unwrap_or_else(|p| panic!("{p:#?}"));
    let disk_blocks = (run.disk.len() / BSIZE) as u32;
    let commits_made = check_commits(&run.record.ops, &after.sb, disk_blocks);
    let snapshots = run.record.snapshots;
    assert_eq!(snapshots.last(), Some(&run.disk));

    let mut states = vec![state(&image)];
    let mut commits = Vec::new();
    for (i, snap) in snapshots.iter().enumerate() {
        let r = fsck::check(snap).unwrap_or_else(|p| panic!("stopped after sector write {i}: {p:#?}"));
        let s = state(snap);
        if r.replayed > 0 && commits.last() != Some(&s) {
            commits.push(s.clone());
        }
        if states.last() != Some(&s) {
            states.push(s);
        }
    }
    assert_eq!(commits.len(), commits_made);
    assert_eq!(states[1..], commits[..]);
    assert!(
        snapshots.iter().any(|s| fsck::check(s).unwrap().orphans == 1),
        "no disk with an orphan"
    );
    // What each step leaves, as the host reads it.
    let holds = |path: &str, nlink: u16, contents: &[u8]| {
        states.iter().any(|(files, _, _)| {
            files
                .iter()
                .any(|f| f.0 == path && f.3 == nlink && f.4 == contents)
        })
    };
    assert!(holds("/ops/a", 1, &pattern(5000, 11)), "/ops/a");
    assert!(holds("/ops/b", 2, &pattern(5000, 11)), "/ops/b, a link to a");
    assert!(holds("/ops/d/c", 1, &pattern(100, 12)), "/ops/d/c");
    assert!(holds("/ops/b", 2, &[]), "/ops/b emptied");
    assert!(holds("/ops/a", 2, &pattern(10, 13)), "/ops/a written through b");
    // In the end, /ops and an empty /ops/e: everything else came back.
    let (files, free_blocks, free_inodes) = states.last().unwrap();
    let ops: Vec<&str> = files
        .iter()
        .map(|f| f.0.as_str())
        .filter(|p| p.starts_with("/ops"))
        .collect();
    assert_eq!(ops, ["/ops", "/ops/e"]);
    assert_eq!(
        (*free_blocks, *free_inodes),
        (before.free_blocks - 1, before.free_inodes - 2)
    );

    reboot_each(&snapshots, READ_CHECKS);
}

#[test]
fn on_a_read_only_root_every_change_fails_and_reading_works() {
    // `filetest ro`: every call that would write the disk fails with
    // EROFS, reading, running programs and changing directory work, and
    // nothing reaches the disk.
    let image = small_image_with_programs(&["sh", "filetest"]);
    let run = boot(
        image.clone(),
        Boot {
            read_only: true,
            input: b"filetest ro\nexit\n",
            max_steps: SHELL_MAX_STEPS,
            ..READ
        },
    );
    assert_eq!(run.result, RunResult::Exited(0), "{}", run.out);
    assert_eq!(
        shell_output(&run.out, "filetest ro"),
        format!("filetest: {FILETEST_RO_CHECKS} checks, 0 failed\n")
    );
    assert!(run.disk == image);
    assert_eq!(run.record.ops, []);
}

#[test]
fn ls_reports_an_entry_it_cannot_open_rather_than_leaving_it_out() {
    // ls looks each entry up by opening it: a device node with no driver
    // can't be, and ls says so where the host lists it, and fails.
    let image = small_image_with_programs(&["sh", "ls"]);
    let host = Fs::open(&image).unwrap().ls("/dev").unwrap();
    let run = boot(
        image,
        Boot {
            input: b"ls /dev\nexit\n",
            max_steps: SHELL_MAX_STEPS,
            ..READ
        },
    );
    assert_eq!(run.result, RunResult::Exited(1), "{}", run.out);
    let mut want: String = host
        .lines()
        .map(|l| {
            if l.ends_with(" nodriver") {
                "ls: /dev/nodriver: no such device\n".to_string()
            } else {
                format!("{l}\n")
            }
        })
        .collect();
    assert!(want.contains("nodriver"), "{host}");
    want.push_str("ls: exit 1\n");
    assert_eq!(shell_output(&run.out, "ls /dev"), want);
}
