//! Checking an image against every rule of `docs/camelot-fs.md`
//! ("Consistency rules"), and replaying its log.
//!
//! [`check`] looks at the disk as a mount would leave it: a committed
//! transaction still in the log is replayed into a copy first (and counted in
//! the report). [`recover`] does that replay in place, as the kernel does at
//! mount; [`finish_truncations`] then frees the blocks of the inodes a crash
//! left part way through a truncation, and [`free_orphans`] the inodes it
//! left allocated with no links, as the kernel's mount does next. [`check`]
//! counts both in its report: they are allowed (docs/camelot-fs.md,
//! "Truncation" and "Link count").

use crate::extent::{self, empty_root};
use crate::{
    block, block_mut, contents, dirents, read_inode, show_name, superblock, valid_name, write_inode, Dinode,
    LogHeader, Superblock, BITS_PER_BLOCK, BSIZE, DIRENT_SIZE, MAX_FILE_SIZE, NO_INODE, ROOT_INODE,
    TRUNCATING, T_DEVICE, T_DIR, T_FILE, T_FREE,
};
use std::collections::{HashMap, VecDeque};

/// The log header of `image`, checked.
pub fn log_header(image: &[u8]) -> Result<LogHeader, String> {
    let sb = superblock(image)?;
    LogHeader::decode(block(image, sb.logstart), &sb)
}

/// Replays the committed transaction in `image`'s log, if there is one:
/// installs each slot to its home block, then empties the header (all
/// zeros). Returns how many blocks it installed. Errors if the superblock or
/// the log header is invalid, leaving `image` as it was.
pub fn recover(image: &mut [u8]) -> Result<usize, String> {
    let sb = superblock(image)?;
    let header = LogHeader::decode(block(image, sb.logstart), &sb)?;
    for (i, &home) in header.blocks.iter().enumerate() {
        let slot = block(image, sb.logstart + 1 + i as u32).to_vec();
        block_mut(image, home).copy_from_slice(&slot);
    }
    if !header.blocks.is_empty() {
        block_mut(image, sb.logstart).fill(0);
    }
    Ok(header.blocks.len())
}

/// Frees every block of inode `inum`, as the kernel's truncation leaves an
/// inode: its tree empty, `size` 0, `TRUNCATING` clear. The blocks are
/// marked free in the bitmap (their contents stay). Errors, changing
/// nothing, if its tree is broken.
fn free_blocks(image: &mut [u8], sb: &Superblock, inum: u32) -> Result<Dinode, String> {
    let mut d = read_inode(image, sb, inum);
    if d.kind == T_DEVICE {
        return Ok(d);
    }
    let map = extent::walk(image, sb, &d.root).map_err(|e| format!("inode {inum}: {e}"))?;
    for b in map.content_blocks().chain(map.tree_blocks.iter().copied()) {
        let (blk, byte, mask) = sb.bit_pos(b);
        block_mut(image, blk)[byte] &= !mask;
    }
    d.root = empty_root();
    d.size = 0;
    d.flags = 0;
    write_inode(image, sb, inum, &d);
    Ok(d)
}

/// Finishes every truncation a crash interrupted, as the kernel's mount
/// does first: frees the blocks left of each file with `TRUNCATING` set that
/// still has links. An orphan being freed is left to [`free_orphans`],
/// which frees it whole. Run it on a replayed image ([`recover`]). Returns
/// how many it finished; errors if the superblock or one of their trees is
/// invalid (the ones before it are finished).
pub fn finish_truncations(image: &mut [u8]) -> Result<usize, String> {
    let sb = superblock(image)?;
    let mut n = 0;
    for inum in ROOT_INODE..sb.ninodes {
        let d = read_inode(image, &sb, inum);
        if d.kind != T_FREE && d.nlink != 0 && d.flags & TRUNCATING != 0 {
            free_blocks(image, &sb, inum)?;
            n += 1;
        }
    }
    Ok(n)
}

/// Frees every orphaned inode of `image` (allocated, with `nlink` 0) as the
/// kernel's mount does (`free_orphans` in kernel/fs.c, through `iput`): its
/// blocks, tree blocks included, are marked free in the bitmap, its tree
/// emptied, its `size` and `flags` set to 0 and then its `type` (`major`,
/// `minor` and `nlink` stay as they were; the blocks' contents too). The root
/// is never freed. Run it on a replayed image ([`recover`]). Returns how
/// many it freed; errors if the superblock or an orphan's tree is invalid
/// (the ones before it are freed).
pub fn free_orphans(image: &mut [u8]) -> Result<usize, String> {
    let sb = superblock(image)?;
    let mut n = 0;
    for inum in ROOT_INODE + 1..sb.ninodes {
        let d = read_inode(image, &sb, inum);
        if d.kind == T_FREE || d.nlink != 0 {
            continue;
        }
        let mut d = free_blocks(image, &sb, inum).map_err(|e| format!("orphan {e}"))?;
        d.kind = T_FREE;
        write_inode(image, &sb, inum, &d);
        n += 1;
    }
    Ok(n)
}

/// What a consistent image holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub sb: Superblock,
    /// Blocks the log held (replayed in the checked copy, not in the image).
    pub replayed: usize,
    pub dirs: u32,
    pub files: u32,
    pub devices: u32,
    /// Inodes of type 0, inode 0 not counted.
    pub free_inodes: u32,
    /// Orphans: allocated inodes with no links and in no directory, which
    /// the next mount (or [`free_orphans`]) frees. Counted in `dirs`, `files`
    /// or `devices` too, and their blocks in `used_blocks`.
    pub orphans: u32,
    /// Files with links and `TRUNCATING` set, whose remaining blocks the next
    /// mount (or [`finish_truncations`]) frees. Their blocks count in
    /// `used_blocks`. (An orphan with it set counts as an orphan.)
    pub truncating: u32,
    /// Data blocks in use, and free.
    pub used_blocks: u32,
    pub free_blocks: u32,
}

/// Checks `image`. Returns what it holds, or every problem found.
pub fn check(image: &[u8]) -> Result<Report, Vec<String>> {
    let sb = superblock(image).map_err(|e| vec![e])?;
    let mut copy = image[..sb.size as usize * BSIZE].to_vec();
    let replayed = recover(&mut copy).map_err(|e| vec![e])?;
    let mut c = Checker {
        image: &copy,
        sb,
        problems: Vec::new(),
        owner: HashMap::new(),
        sound: vec![false; sb.ninodes as usize],
        truncating: 0,
    };
    c.inodes();
    c.bitmap();
    let refs = c.tree();
    let orphans = c.links(&refs);
    if !c.problems.is_empty() {
        return Err(c.problems);
    }
    let mut r = Report {
        sb,
        replayed,
        dirs: 0,
        files: 0,
        devices: 0,
        free_inodes: 0,
        orphans,
        truncating: c.truncating,
        used_blocks: c.owner.len() as u32,
        free_blocks: sb.nblocks - c.owner.len() as u32,
    };
    for i in 1..sb.ninodes {
        match read_inode(&copy, &sb, i).kind {
            T_DIR => r.dirs += 1,
            T_FILE => r.files += 1,
            T_DEVICE => r.devices += 1,
            _ => r.free_inodes += 1,
        }
    }
    Ok(r)
}

struct Checker<'a> {
    image: &'a [u8],
    sb: Superblock,
    problems: Vec<String>,
    /// Each data block in use, and the inode using it.
    owner: HashMap<u32, u32>,
    /// Inodes whose fields and tree are sound: only those are read.
    sound: Vec<bool>,
    /// Inodes with `TRUNCATING` set.
    truncating: u32,
}

fn type_name(kind: u16) -> &'static str {
    match kind {
        T_DIR => "directory",
        T_FILE => "file",
        T_DEVICE => "device",
        _ => "inode",
    }
}

impl Checker<'_> {
    fn problem(&mut self, p: String) {
        self.problems.push(p);
    }

    /// Records that inode `inum` uses block `b` (as `what`).
    fn claim(&mut self, inum: u32, b: u32, what: &str) -> bool {
        if !self.sb.is_data(b) {
            self.problem(format!(
                "inode {inum}: {what} is block {b}, outside the data region"
            ));
            return false;
        }
        if let Some(&other) = self.owner.get(&b) {
            self.problem(format!(
                "inode {inum}: {what} is block {b}, which inode {other} uses too"
            ));
            return false;
        }
        self.owner.insert(b, inum);
        true
    }

    /// Rule 2 and 3: every inode's fields and extent tree.
    fn inodes(&mut self) {
        for inum in 0..self.sb.ninodes {
            let d = read_inode(self.image, &self.sb, inum);
            if d.kind == T_FREE {
                continue;
            }
            if inum == NO_INODE {
                self.problem(format!("inode 0 has type {}, but is never used", d.kind));
                continue;
            }
            if d.kind > T_DEVICE {
                self.problem(format!("inode {inum}: type {} is not 0 to 3", d.kind));
                continue;
            }
            if d.kind != T_DEVICE && (d.major != 0 || d.minor != 0) {
                self.problem(format!(
                    "inode {inum}: a {} with device numbers",
                    type_name(d.kind)
                ));
            }
            if let Some(i) = d.reserved.iter().position(|&x| x != 0) {
                self.problem(format!(
                    "inode {inum}: reserved byte {} is not 0",
                    crate::RESERVED_AT + i
                ));
            }
            if d.flags & !TRUNCATING != 0 {
                self.problem(format!("inode {inum}: flags {:#x} has undefined bits", d.flags));
                continue;
            }
            let truncating = d.flags & TRUNCATING != 0;
            if d.size > MAX_FILE_SIZE {
                self.problem(format!(
                    "inode {inum}: size {} is more than the largest file",
                    d.size
                ));
                continue;
            }
            if d.kind == T_DEVICE {
                if d.size != 0 || d.root.iter().any(|&x| x != 0) || truncating {
                    self.problem(format!("inode {inum}: a device with contents"));
                    continue;
                }
                self.sound[inum as usize] = true;
                continue;
            }
            if truncating {
                // An orphan being freed is the next mount's to free whole: an
                // orphan, not a truncation to finish.
                if d.nlink != 0 {
                    self.truncating += 1;
                }
                if d.size != 0 {
                    self.problem(format!(
                        "inode {inum}: being truncated, with a size of {} (it must be 0)",
                        d.size
                    ));
                    continue;
                }
                if d.kind == T_DIR && d.nlink != 0 {
                    self.problem(format!(
                        "inode {inum}: a directory being truncated, with links (only orphans are)"
                    ));
                    continue;
                }
            } else if d.kind == T_DIR
                && (!d.size.is_multiple_of(DIRENT_SIZE as u64) || d.size < 2 * DIRENT_SIZE as u64)
            {
                self.problem(format!(
                    "inode {inum}: a directory of {} bytes (not whole entries, or fewer than 2)",
                    d.size
                ));
                continue;
            }
            let map = match extent::walk(self.image, &self.sb, &d.root) {
                Ok(map) => map,
                Err(e) => {
                    self.problem(format!("inode {inum}: {e}"));
                    continue;
                }
            };
            // A file being truncated maps the blocks still to free, any number
            // of them; any other the blocks its size needs.
            if !truncating && map.blocks() != d.content_blocks() {
                self.problem(format!(
                    "inode {inum}: its size ({} bytes) needs {} blocks, its extent tree maps {}",
                    d.size,
                    d.content_blocks(),
                    map.blocks()
                ));
                continue;
            }
            let mut ok = true;
            for &b in &map.tree_blocks {
                ok &= self.claim(inum, b, "a tree block");
            }
            for (k, b) in map.content_blocks().enumerate() {
                ok &= self.claim(inum, b, &format!("block {k}"));
            }
            self.sound[inum as usize] = ok;
        }
    }

    /// Rule 4: the bitmap marks the metadata and the blocks in use, nothing
    /// else.
    fn bitmap(&mut self) {
        let sb = self.sb;
        let bit = |b: u32| {
            let (blk, byte, mask) = sb.bit_pos(b);
            block(self.image, blk)[byte] & mask != 0
        };
        let mut found = Vec::new();
        for b in 0..sb.size {
            let used = b < sb.datastart || self.owner.contains_key(&b);
            match (used, bit(b)) {
                (true, false) if b < sb.datastart => {
                    found.push(format!("block {b} (metadata) is marked free in the bitmap"))
                }
                (true, false) => found.push(format!(
                    "block {b} is marked free in the bitmap, but inode {} uses it",
                    self.owner[&b]
                )),
                (false, true) => found.push(format!("block {b} is marked in use, but nothing uses it")),
                _ => {}
            }
        }
        let bitmap_bits = (sb.datastart - sb.bmapstart) * BITS_PER_BLOCK;
        for b in sb.size..bitmap_bits {
            if bit(b) {
                found.push(format!("the bitmap marks block {b}, past the end ({})", sb.size));
                break;
            }
        }
        self.problems.extend(found);
    }

    /// Rule 5: walks the directory tree from the root. Returns how many
    /// entries (other than `.`) name each inode, and which were reached.
    fn tree(&mut self) -> HashMap<u32, u32> {
        let mut refs: HashMap<u32, u32> = HashMap::new();
        let root = read_inode(self.image, &self.sb, ROOT_INODE);
        if root.kind != T_DIR {
            self.problem(format!(
                "inode 1, the root, is not a directory (type {})",
                root.kind
            ));
            return refs;
        }
        // Directories reached, with their parent: each directory may be
        // reached once.
        let mut parent_of: HashMap<u32, u32> = HashMap::from([(ROOT_INODE, ROOT_INODE)]);
        let mut queue = VecDeque::from([(ROOT_INODE, "/".to_string())]);
        while let Some((dir, path)) = queue.pop_front() {
            if !self.sound[dir as usize] {
                continue;
            }
            let d = read_inode(self.image, &self.sb, dir);
            let data = match contents(self.image, &self.sb, &d) {
                Ok(data) => data,
                Err(e) => {
                    self.problem(format!("{path}: {e}"));
                    continue;
                }
            };
            let mut names: Vec<Vec<u8>> = Vec::new();
            for (i, e) in dirents(&data).iter().enumerate() {
                let inum = u32::from(e.inum);
                let shown = show_name(e.name());
                let want_dot = match i {
                    0 => Some((&b"."[..], dir)),
                    1 => Some((&b".."[..], parent_of[&dir])),
                    _ => None,
                };
                if let Some((name, want)) = want_dot {
                    if e.name() != name || inum != want || !e.padded() {
                        self.problem(format!(
                            "{path}: entry {i} is `{shown}` → {inum}, not `{}` → {want}",
                            show_name(name)
                        ));
                    }
                    if i == 1 && inum == want {
                        *refs.entry(inum).or_default() += 1;
                    }
                    continue;
                }
                if inum == NO_INODE {
                    continue;
                }
                let child_path = format!(
                    "{}{shown}",
                    if path == "/" {
                        "/".into()
                    } else {
                        format!("{path}/")
                    }
                );
                if !valid_name(e.name()) || !e.padded() {
                    self.problem(format!("{path}: entry {i} has a bad name {:?}", e.name));
                }
                if names.iter().any(|n| n == e.name()) {
                    self.problem(format!("{path}: two entries called `{shown}`"));
                }
                names.push(e.name().to_vec());
                if inum >= self.sb.ninodes {
                    self.problem(format!("{child_path}: inode {inum} is past the last"));
                    continue;
                }
                let child = read_inode(self.image, &self.sb, inum);
                if child.kind == T_FREE {
                    self.problem(format!("{child_path}: names inode {inum}, which is free"));
                    continue;
                }
                *refs.entry(inum).or_default() += 1;
                if child.kind == T_DIR {
                    if inum == ROOT_INODE || parent_of.contains_key(&inum) {
                        self.problem(format!("{child_path}: directory {inum} has a second name"));
                        continue;
                    }
                    parent_of.insert(inum, dir);
                    queue.push_back((inum, child_path));
                }
            }
        }
        refs
    }

    /// Rule 5's reachability and rule 6, the link counts. Returns how many
    /// orphans there are: inodes in no directory with `nlink` 0, which are
    /// allowed (the entries of an orphaned directory are not walked, so its
    /// `..` doesn't count for its old parent, as in the kernel).
    fn links(&mut self, refs: &HashMap<u32, u32>) -> u32 {
        let mut orphans = 0;
        for inum in 1..self.sb.ninodes {
            let d = read_inode(self.image, &self.sb, inum);
            if d.kind == T_FREE || d.kind > T_DEVICE {
                continue;
            }
            let n = refs.get(&inum).copied().unwrap_or(0);
            if n == 0 && d.nlink == 0 && inum != ROOT_INODE {
                orphans += 1;
            } else if n == 0 {
                self.problem(format!("inode {inum} ({}) is in no directory", type_name(d.kind)));
            } else if u32::from(d.nlink) != n {
                self.problem(format!(
                    "inode {inum}: nlink is {}, but {n} entries name it",
                    d.nlink
                ));
            }
        }
        orphans
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extent::{Extent, Header, ENTRY_SIZE, HEADER_SIZE};
    use crate::mkfs::{Mkfs, Options};
    use crate::{data_blocks, put_u32, write_inode, Dinode, Dirent};

    fn opts() -> Options {
        Options {
            size: 300,
            ninodes: 48,
            nlog: 6,
        }
    }

    /// An image with a bit of everything: subdirectories, a file of 21
    /// blocks, an empty file, a device, a hard link.
    fn sample() -> Vec<u8> {
        let mut fs = Mkfs::new(&opts()).unwrap();
        fs.mkdir("/bin").unwrap();
        fs.mkdir("/bin/sub").unwrap();
        fs.add_file("/bin/big", &vec![3; 20 * BSIZE + 5]).unwrap();
        fs.add_file("/empty", b"").unwrap();
        fs.add_device("/console", 1, 0).unwrap();
        fs.add_file("/README", b"hello\n").unwrap();
        fs.link_file("/README", "/bin/readme").unwrap();
        fs.finish()
    }

    #[test]
    fn a_fresh_image_is_consistent() {
        let r = check(&sample()).unwrap();
        assert_eq!((r.dirs, r.files, r.devices, r.replayed), (3, 3, 1, 0));
        assert_eq!(r.free_inodes, 48 - 1 - 7);
        // Root, bin, sub: one block each; big: 21, one extent; README 1.
        assert_eq!(r.used_blocks, 3 + 21 + 1);
        assert_eq!(r.truncating, 0);
        assert_eq!(r.free_blocks, r.sb.nblocks - r.used_blocks);
    }

    /// Checks `image` after `f` broke it: one of the problems must contain
    /// `want`.
    fn broken(want: &str, f: impl FnOnce(&mut Vec<u8>, &Superblock)) {
        let mut image = sample();
        let sb = superblock(&image).unwrap();
        f(&mut image, &sb);
        let problems = check(&image).expect_err(want);
        assert!(
            problems.iter().any(|p| p.contains(want)),
            "want `{want}`, got {problems:#?}"
        );
    }

    fn inode_of(image: &[u8], path: &str) -> u32 {
        crate::read::Fs::open(image).unwrap().lookup(path).unwrap()
    }

    fn edit_inode(image: &mut [u8], sb: &Superblock, inum: u32, f: impl FnOnce(&mut Dinode)) {
        let mut d = read_inode(image, sb, inum);
        f(&mut d);
        write_inode(image, sb, inum, &d);
    }

    /// The first data block of inode `inum`.
    fn first_block(image: &[u8], sb: &Superblock, inum: u32) -> u32 {
        data_blocks(image, sb, &read_inode(image, sb, inum)).unwrap()[0]
    }

    /// Changes the first extent in an inode's root (a tree of depth 0).
    fn edit_extent(d: &mut Dinode, f: impl FnOnce(&mut Extent)) {
        assert_eq!(Header::decode(&d.root).depth, 0);
        let mut e = Extent::decode(&d.root[HEADER_SIZE..]);
        f(&mut e);
        e.encode(&mut d.root[HEADER_SIZE..HEADER_SIZE + ENTRY_SIZE]);
    }

    #[test]
    fn every_kind_of_damage_is_found() {
        broken("no filesystem", |img, _| block_mut(img, 1)[0] = 0);
        broken("superblock byte 140 is not zero", |img, _| {
            block_mut(img, 1)[140] = 0xff
        });
        broken("misplaced", |img, _| block_mut(img, 1)[36] += 1);
        broken("inode 0 has type", |img, sb| {
            edit_inode(img, sb, 0, |d| d.kind = T_FILE)
        });
        broken("type 7", |img, sb| edit_inode(img, sb, 2, |d| d.kind = 7));
        broken("not a directory", |img, sb| {
            edit_inode(img, sb, 1, |d| d.kind = T_FILE)
        });
        broken("a device with contents", |img, sb| {
            let i = inode_of(img, "/console");
            edit_inode(img, sb, i, |d| d.size = 1)
        });
        broken("with device numbers", |img, sb| {
            let i = inode_of(img, "/README");
            edit_inode(img, sb, i, |d| d.major = 1)
        });
        broken("largest file", |img, sb| {
            let i = inode_of(img, "/README");
            edit_inode(img, sb, i, |d| d.size = MAX_FILE_SIZE + 1)
        });
        broken("not whole entries", |img, sb| {
            edit_inode(img, sb, 1, |d| d.size += 1)
        });
        // A hole: the size needs more blocks than the tree maps; and a tree
        // that maps more than the size needs.
        broken("needs 30 blocks, its extent tree maps 21", |img, sb| {
            let i = inode_of(img, "/bin/big");
            edit_inode(img, sb, i, |d| d.size = 30 * BSIZE as u64)
        });
        broken("needs 1 blocks, its extent tree maps 2", |img, sb| {
            let i = inode_of(img, "/README");
            edit_inode(img, sb, i, |d| edit_extent(d, |e| e.len = 2))
        });
        broken("outside the data region", |img, sb| {
            let i = inode_of(img, "/README");
            edit_inode(img, sb, i, |d| edit_extent(d, |e| e.start = 3))
        });
        broken("uses too", |img, sb| {
            let readme = first_block(img, sb, inode_of(img, "/README"));
            let i = inode_of(img, "/bin/big");
            edit_inode(img, sb, i, |d| edit_extent(d, |e| e.start = u64::from(readme)))
        });
        broken("the root's magic", |img, sb| {
            let i = inode_of(img, "/README");
            edit_inode(img, sb, i, |d| d.root[0] = 0)
        });
        broken("a device with contents", |img, sb| {
            let i = inode_of(img, "/console");
            edit_inode(img, sb, i, |d| d.root[20] = 1)
        });
        broken("reserved byte 127 is not 0", |img, sb| {
            let i = inode_of(img, "/README");
            edit_inode(img, sb, i, |d| d.reserved[47] = 1)
        });
        broken("flags 0x2 has undefined bits", |img, sb| {
            let i = inode_of(img, "/README");
            edit_inode(img, sb, i, |d| d.flags = 2)
        });
        broken("being truncated, with a size of 6", |img, sb| {
            let i = inode_of(img, "/README");
            edit_inode(img, sb, i, |d| d.flags = TRUNCATING)
        });
        broken("a directory being truncated, with links", |img, sb| {
            let i = inode_of(img, "/bin/sub");
            edit_inode(img, sb, i, |d| {
                d.flags = TRUNCATING;
                d.size = 0;
            })
        });
        broken("a device with contents", |img, sb| {
            let i = inode_of(img, "/console");
            edit_inode(img, sb, i, |d| d.flags = TRUNCATING)
        });
        broken("marked free in the bitmap, but inode", |img, sb| {
            let readme = first_block(img, sb, inode_of(img, "/README"));
            let (blk, byte, mask) = sb.bit_pos(readme);
            block_mut(img, blk)[byte] &= !mask;
        });
        broken("(metadata) is marked free", |img, sb| {
            let (blk, byte, mask) = sb.bit_pos(sb.logstart);
            block_mut(img, blk)[byte] &= !mask;
        });
        broken("marked in use, but nothing uses it", |img, sb| {
            let (blk, byte, mask) = sb.bit_pos(sb.size - 1);
            block_mut(img, blk)[byte] |= mask;
        });
        broken("past the end", |img, sb| {
            let (blk, byte, mask) = sb.bit_pos(sb.size + 3);
            block_mut(img, blk)[byte] |= mask;
        });
        broken("is in no directory", |img, sb| {
            edit_inode(img, sb, 40, |d| {
                d.kind = T_FILE;
                d.nlink = 1;
            })
        });
        broken("nlink is 1, but 2 entries", |img, sb| {
            let i = inode_of(img, "/README");
            edit_inode(img, sb, i, |d| d.nlink = 1)
        });
        broken("nlink is 1, but 2 entries", |img, sb| {
            edit_inode(img, sb, 1, |d| d.nlink = 1)
        });

        // Directory entries.
        // Sets entry `i` of a directory, growing it if needed.
        let set_entry = |img: &mut Vec<u8>, sb: &Superblock, dir: &str, i: usize, e: Dirent| {
            let inum = inode_of(img, dir);
            let d = read_inode(img, sb, inum);
            let at = i * DIRENT_SIZE;
            let first = first_block(img, sb, inum);
            block_mut(img, first)[at..at + DIRENT_SIZE].copy_from_slice(&e.encode());
            if at as u64 >= d.size {
                edit_inode(img, sb, inum, |d| d.size = (at + DIRENT_SIZE) as u64);
            }
        };
        broken("entry 0 is `x`", |img, sb| {
            set_entry(img, sb, "/bin", 0, Dirent::new(2, b"x"))
        });
        broken("entry 1 is `..` → 2, not `..` → 1", |img, sb| {
            set_entry(img, sb, "/bin", 1, Dirent::new(2, b".."))
        });
        broken("two entries called `big`", |img, sb| {
            let sub = inode_of(img, "/bin/sub") as u16;
            set_entry(img, sb, "/bin", 2, Dirent::new(sub, b"big"))
        });
        broken("bad name", |img, sb| {
            let readme = inode_of(img, "/README") as u16;
            set_entry(img, sb, "/bin", 4, Dirent::new(readme, b"a/b"))
        });
        broken("which is free", |img, sb| {
            set_entry(img, sb, "/bin", 4, Dirent::new(45, b"ghost"))
        });
        broken("past the last", |img, sb| {
            set_entry(img, sb, "/bin", 4, Dirent::new(4000, b"far"))
        });
        broken("has a second name", |img, sb| {
            let sub = inode_of(img, "/bin/sub") as u16;
            set_entry(img, sb, "/", 6, Dirent::new(sub, b"again"))
        });
        // A cycle: /bin/sub names the root as a subdirectory.
        broken("has a second name", |img, sb| {
            set_entry(img, sb, "/bin/sub", 2, Dirent::new(1, b"up"))
        });
    }

    #[test]
    fn a_pending_log_is_checked_as_replayed() {
        let mut image = sample();
        let sb = superblock(&image).unwrap();
        // A committed transaction that adds a second name for /README: the
        // new entry in the root's block and the new link count.
        let readme = inode_of(&image, "/README");
        let root = read_inode(&image, &sb, ROOT_INODE);
        let root_block = first_block(&image, &sb, ROOT_INODE);
        let mut dir_block = block(&image, root_block).to_vec();
        let n = root.size as usize;
        dir_block[n..n + DIRENT_SIZE].copy_from_slice(&Dirent::new(readme as u16, b"again").encode());
        let mut inodes = image.clone();
        edit_inode(&mut inodes, &sb, readme, |d| d.nlink = 3);
        edit_inode(&mut inodes, &sb, ROOT_INODE, |d| d.size += DIRENT_SIZE as u64);
        let (iblock, _) = sb.inode_pos(readme);
        assert_eq!(sb.inode_pos(ROOT_INODE).0, iblock);
        let slots = [(root_block, dir_block), (iblock, block(&inodes, iblock).to_vec())];
        for (i, (_, data)) in slots.iter().enumerate() {
            block_mut(&mut image, sb.logstart + 1 + i as u32).copy_from_slice(data);
        }
        // Not committed yet: the image is the old one.
        let before = check(&image).unwrap();
        assert_eq!(before.replayed, 0);
        let header = LogHeader {
            blocks: slots.iter().map(|(h, _)| *h).collect(),
        };
        block_mut(&mut image, sb.logstart).copy_from_slice(&header.encode());
        // Committed: checked as replayed, the image itself unchanged.
        let snapshot = image.clone();
        let r = check(&image).unwrap();
        assert_eq!(r.replayed, 2);
        assert_eq!(image, snapshot);
        let fs = crate::read::Fs::open(&image).unwrap();
        assert_eq!(fs.read_file("/again").unwrap(), b"hello\n");
        assert_eq!(fs.replayed(), 2);
        // Recovery installs it and empties the log; doing it again changes
        // nothing.
        assert_eq!(recover(&mut image), Ok(2));
        assert!(block(&image, sb.logstart).iter().all(|&b| b == 0));
        assert_eq!(check(&image).unwrap().replayed, 0);
        let once = image.clone();
        assert_eq!(recover(&mut image), Ok(0));
        assert_eq!(image, once);
        assert_eq!(
            crate::read::Fs::open(&image)
                .unwrap()
                .read_file("/again")
                .unwrap(),
            b"hello\n"
        );
    }

    /// Removes the entry `name` from directory `dir` (its first block), as
    /// the kernel's unlink does: the slot's inode number becomes 0.
    fn remove_entry(image: &mut [u8], sb: &Superblock, dir: &str, name: &[u8]) {
        let first = first_block(image, sb, inode_of(image, dir));
        let b = block_mut(image, first);
        let i = dirents(b)
            .iter()
            .position(|e| e.inum != 0 && e.name() == name)
            .unwrap();
        b[i * DIRENT_SIZE..i * DIRENT_SIZE + 2].fill(0);
    }

    #[test]
    fn orphans_are_allowed_and_freed() {
        // A crash between unlinking a file still in use and its last user
        // letting go leaves it allocated, with no links and in no directory:
        // an orphan. That disk is consistent; free_orphans() frees it as the
        // kernel's next mount does.
        let mut image = sample();
        let sb = superblock(&image).unwrap();
        let before = check(&image).unwrap();
        let big = inode_of(&image, "/bin/big");
        let empty = inode_of(&image, "/empty");
        let big_d = read_inode(&image, &sb, big);
        remove_entry(&mut image, &sb, "/bin", b"big");
        edit_inode(&mut image, &sb, big, |d| d.nlink = 0);
        remove_entry(&mut image, &sb, "/", b"empty");
        edit_inode(&mut image, &sb, empty, |d| d.nlink = 0);
        let r = check(&image).unwrap();
        assert_eq!(r.orphans, 2);
        assert_eq!((r.files, r.used_blocks), (before.files, before.used_blocks));

        // Named by an entry, a count of 0 is still wrong.
        let mut named = sample();
        edit_inode(&mut named, &sb, empty, |d| d.nlink = 0);
        let problems = check(&named).unwrap_err();
        assert!(
            problems.iter().any(|p| p.contains("nlink is 0, but 1 entries")),
            "{problems:#?}"
        );

        assert_eq!(free_orphans(&mut image), Ok(2));
        let r = check(&image).unwrap();
        assert_eq!(r.orphans, 0);
        assert_eq!(r.files, before.files - 2);
        assert_eq!(r.free_inodes, before.free_inodes + 2);
        // big's 21 blocks.
        assert_eq!(r.used_blocks, before.used_blocks - 21);
        let d = read_inode(&image, &sb, big);
        assert_eq!(
            d,
            Dinode {
                kind: T_FREE,
                nlink: 0,
                size: 0,
                root: empty_root(),
                ..big_d
            }
        );
        // Nothing left to free.
        let once = image.clone();
        assert_eq!(free_orphans(&mut image), Ok(0));
        assert_eq!(image, once);
    }

    #[test]
    fn truncations_are_allowed_and_finished() {
        // A crash part way through freeing a file's blocks leaves it with
        // TRUNCATING set, a size of 0, and the blocks still to free: a valid
        // disk, on which the next mount finishes the job. Here none were
        // freed yet, and the file still has its name.
        let mut image = sample();
        let sb = superblock(&image).unwrap();
        let before = check(&image).unwrap();
        let big = inode_of(&image, "/bin/big");
        edit_inode(&mut image, &sb, big, |d| {
            d.flags = TRUNCATING;
            d.size = 0;
        });
        let r = check(&image).unwrap();
        assert_eq!((r.truncating, r.used_blocks), (1, before.used_blocks));
        // It reads as the mount will leave it: empty.
        let fs = crate::read::Fs::open(&image).unwrap();
        assert_eq!(fs.read_file("/bin/big").unwrap(), b"");
        assert_eq!(finish_truncations(&mut image), Ok(1));
        let r = check(&image).unwrap();
        assert_eq!((r.truncating, r.used_blocks), (0, before.used_blocks - 21));
        let d = read_inode(&image, &sb, big);
        assert_eq!(
            (d.kind, d.size, d.flags, d.nlink, d.root),
            (T_FILE, 0, 0, 1, empty_root())
        );
        let once = image.clone();
        assert_eq!(finish_truncations(&mut image), Ok(0));
        assert_eq!(image, once);
    }

    #[test]
    fn tree_blocks_are_claimed() {
        // A directory of 6 blocks, each after the files in the block before:
        // 6 extents, which need a tree block.
        let mut fs = Mkfs::new(&Options {
            size: 1200,
            ninodes: 512,
            nlog: 8,
        })
        .unwrap();
        for i in 0..6 * 64 - 2 {
            fs.add_file(&format!("/f{i}"), b"x").unwrap();
        }
        let mut image = fs.finish();
        let sb = superblock(&image).unwrap();
        let root = read_inode(&image, &sb, ROOT_INODE);
        let tree = extent::walk(&image, &sb, &root.root).unwrap().tree_blocks;
        assert_eq!(tree.len(), 1);
        assert_eq!(check(&image).unwrap().used_blocks, 6 + 1 + 6 * 64 - 2);
        // A broken tree block is found through its inode.
        let mut broken_tree = image.clone();
        put_u32(block_mut(&mut broken_tree, tree[0]), 8, 7);
        let problems = check(&broken_tree).unwrap_err();
        assert!(
            problems
                .iter()
                .any(|p| p.starts_with("inode 1: a tree block's eh_generation")),
            "{problems:#?}"
        );
        // A file's block that is the root's tree block too.
        let f = inode_of(&image, "/f7");
        edit_inode(&mut image, &sb, f, |d| {
            edit_extent(d, |e| e.start = u64::from(tree[0]))
        });
        let problems = check(&image).unwrap_err();
        assert!(
            problems
                .iter()
                .any(|p| p.contains(&format!("is block {}, which inode 1 uses too", tree[0]))),
            "{problems:#?}"
        );
    }

    #[test]
    fn a_bad_log_header_is_refused() {
        let mut image = sample();
        let sb = superblock(&image).unwrap();
        put_u32(block_mut(&mut image, sb.logstart), 0, sb.nlog);
        let before = image.clone();
        let err = recover(&mut image).unwrap_err();
        assert!(err.contains("slots"), "{err}");
        assert_eq!(image, before);
        assert!(check(&image).unwrap_err()[0].contains("slots"));
        assert!(crate::read::Fs::open(&image).is_err());
    }
}
