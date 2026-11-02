//! Checking an image against every rule of `docs/camelot-fs.md`
//! ("Consistency rules"), and replaying its log.
//!
//! [`check`] looks at the disk as a mount would leave it: a committed
//! transaction still in the log is replayed into a copy first (and counted in
//! the report). [`recover`] does that replay in place, as the kernel does at
//! mount.

use crate::{
    block, block_mut, bmap, contents, dirents, read_inode, show_name, superblock, valid_name, LogHeader,
    Superblock, BITS_PER_BLOCK, BSIZE, DIRENT_SIZE, MAX_FILE_SIZE, NDIRECT, NINDIRECT, NO_INODE, ROOT_INODE,
    T_DEVICE, T_DIR, T_FILE, T_FREE,
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
    };
    c.inodes();
    c.bitmap();
    let refs = c.tree();
    c.links(&refs);
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
    /// Inodes whose type, size and pointers are sound: only those are read.
    sound: Vec<bool>,
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

    /// Rule 2 and 3: every inode's type, size and pointers.
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
            if d.size > MAX_FILE_SIZE {
                self.problem(format!(
                    "inode {inum}: size {} is more than the largest file",
                    d.size
                ));
                continue;
            }
            if d.kind == T_DEVICE && (d.size != 0 || d.addrs.iter().any(|&a| a != 0)) {
                self.problem(format!("inode {inum}: a device with contents"));
                continue;
            }
            if d.kind == T_DIR
                && (!(d.size as usize).is_multiple_of(DIRENT_SIZE) || (d.size as usize) < 2 * DIRENT_SIZE)
            {
                self.problem(format!(
                    "inode {inum}: a directory of {} bytes (not whole entries, or fewer than 2)",
                    d.size
                ));
                continue;
            }
            let mut ok = true;
            let n = d.content_blocks();
            for k in 0..NDIRECT {
                let a = d.addrs[k];
                if k < n {
                    if a == 0 {
                        self.problem(format!("inode {inum}: block {k} of its {n} is missing (a hole)"));
                        ok = false;
                    } else {
                        ok &= self.claim(inum, a, &format!("block {k}"));
                    }
                } else if a != 0 {
                    self.problem(format!(
                        "inode {inum}: a pointer past its {n} blocks (addrs[{k}] = {a})"
                    ));
                    ok = false;
                }
            }
            let ind = d.indirect();
            if n > NDIRECT {
                if ind == 0 {
                    self.problem(format!("inode {inum}: {n} blocks but no indirect block"));
                    ok = false;
                } else if self.claim(inum, ind, "the indirect block") {
                    for i in 0..NINDIRECT {
                        let k = NDIRECT + i;
                        let a = bmap(self.image, &self.sb, &d, k).unwrap_or(0);
                        if k < n {
                            if a == 0 {
                                self.problem(format!(
                                    "inode {inum}: block {k} of its {n} is missing (a hole)"
                                ));
                                ok = false;
                            } else {
                                ok &= self.claim(inum, a, &format!("block {k}"));
                            }
                        } else if a != 0 {
                            self.problem(format!(
                                "inode {inum}: indirect entry {i} past its {n} blocks is {a}"
                            ));
                            ok = false;
                        }
                    }
                } else {
                    ok = false;
                }
            } else if ind != 0 {
                self.problem(format!(
                    "inode {inum}: an indirect block ({ind}) with only {n} blocks"
                ));
                ok = false;
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

    /// Rule 5's reachability and rule 6, the link counts.
    fn links(&mut self, refs: &HashMap<u32, u32>) {
        for inum in 1..self.sb.ninodes {
            let d = read_inode(self.image, &self.sb, inum);
            if d.kind == T_FREE || d.kind > T_DEVICE {
                continue;
            }
            let n = refs.get(&inum).copied().unwrap_or(0);
            if n == 0 {
                self.problem(format!("inode {inum} ({}) is in no directory", type_name(d.kind)));
            } else if u32::from(d.nlink) != n {
                self.problem(format!(
                    "inode {inum}: nlink is {}, but {n} entries name it",
                    d.nlink
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mkfs::{Mkfs, Options};
    use crate::{put_u32, write_inode, Dinode, Dirent};

    fn opts() -> Options {
        Options {
            size: 300,
            ninodes: 48,
            nlog: 6,
        }
    }

    /// An image with a bit of everything: subdirectories, a file with an
    /// indirect block, an empty file, a device, a hard link.
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
        // Root, bin, sub: one block each; big: 21 + its indirect; README 1.
        assert_eq!(r.used_blocks, 3 + 22 + 1);
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

    #[test]
    fn every_kind_of_damage_is_found() {
        broken("no filesystem", |img, _| block_mut(img, 1)[0] = 0);
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
        broken("a hole", |img, sb| {
            let i = inode_of(img, "/bin/big");
            edit_inode(img, sb, i, |d| d.addrs[3] = 0)
        });
        broken("a pointer past its", |img, sb| {
            let i = inode_of(img, "/README");
            edit_inode(img, sb, i, |d| d.addrs[5] = 200)
        });
        broken("outside the data region", |img, sb| {
            let i = inode_of(img, "/README");
            edit_inode(img, sb, i, |d| d.addrs[0] = 3)
        });
        broken("uses too", |img, sb| {
            let readme = read_inode(img, sb, inode_of(img, "/README"));
            let i = inode_of(img, "/bin/big");
            edit_inode(img, sb, i, |d| d.addrs[0] = readme.addrs[0])
        });
        broken("an indirect block", |img, sb| {
            let i = inode_of(img, "/README");
            edit_inode(img, sb, i, |d| d.addrs[NDIRECT] = 250)
        });
        broken("indirect entry", |img, sb| {
            let big = read_inode(img, sb, inode_of(img, "/bin/big"));
            put_u32(block_mut(img, big.indirect()), 4 * 100, 251);
        });
        broken("marked free in the bitmap, but inode", |img, sb| {
            let readme = read_inode(img, sb, inode_of(img, "/README"));
            let (blk, byte, mask) = sb.bit_pos(readme.addrs[0]);
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
            block_mut(img, d.addrs[0])[at..at + DIRENT_SIZE].copy_from_slice(&e.encode());
            if at >= d.size as usize {
                edit_inode(img, sb, inum, |d| d.size = (at + DIRENT_SIZE) as u32);
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
        let mut dir_block = block(&image, root.addrs[0]).to_vec();
        let n = root.size as usize;
        dir_block[n..n + DIRENT_SIZE].copy_from_slice(&Dirent::new(readme as u16, b"again").encode());
        let mut inodes = image.clone();
        edit_inode(&mut inodes, &sb, readme, |d| d.nlink = 3);
        edit_inode(&mut inodes, &sb, ROOT_INODE, |d| d.size += DIRENT_SIZE as u32);
        let (iblock, _) = sb.inode_pos(readme);
        assert_eq!(sb.inode_pos(ROOT_INODE).0, iblock);
        let slots = [
            (root.addrs[0], dir_block),
            (iblock, block(&inodes, iblock).to_vec()),
        ];
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
