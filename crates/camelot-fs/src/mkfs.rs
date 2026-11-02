//! Building images. [`Mkfs::new`] formats an empty filesystem (superblock,
//! empty log, root directory with `.` and `..`); [`Mkfs::mkdir`],
//! [`Mkfs::add_file`] and [`Mkfs::add_host_dir`] fill it; [`Mkfs::finish`]
//! returns the image. Nothing else uses the image while it is built, so mkfs
//! writes blocks in place, without the log.
//!
//! Inodes are handed out in order from 2 (1 is the root) and blocks in order
//! from `datastart`, so the same inputs always give the same bytes. A file's
//! blocks come right after its indirect block, if it has one.

use crate::{
    block_mut, contents, dirents, path_names, read_inode, show_name, valid_name, write_inode, Dinode, Dirent,
    Superblock, BSIZE, DEFAULT_INODES, DEFAULT_LOG, DEFAULT_SIZE, DIRENT_SIZE, MAX_FILE_SIZE, NDIRECT,
    ROOT_INODE, SUPER_BLOCK, T_DEVICE, T_DIR, T_FILE,
};
use std::path::Path;

/// The shape of a new filesystem.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Options {
    /// Blocks in the filesystem, and in the image.
    pub size: u32,
    /// Inode slots (inode 0 included).
    pub ninodes: u32,
    /// Log blocks, the header included.
    pub nlog: u32,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            size: DEFAULT_SIZE,
            ninodes: DEFAULT_INODES,
            nlog: DEFAULT_LOG,
        }
    }
}

/// A filesystem being built.
#[derive(Debug, Clone)]
pub struct Mkfs {
    image: Vec<u8>,
    sb: Superblock,
    /// The next inode and data block to hand out.
    next_inode: u32,
    next_block: u32,
}

impl Mkfs {
    /// An empty filesystem: the superblock, an empty log (all zero), the
    /// metadata blocks marked in use and the root directory.
    pub fn new(opts: &Options) -> Result<Mkfs, String> {
        let sb = Superblock::layout(opts.size, opts.ninodes, opts.nlog)?;
        let mut fs = Mkfs {
            image: vec![0; sb.size as usize * BSIZE],
            sb,
            next_inode: ROOT_INODE,
            next_block: sb.datastart,
        };
        block_mut(&mut fs.image, SUPER_BLOCK).copy_from_slice(&sb.encode());
        for b in 0..sb.datastart {
            fs.mark_used(b);
        }
        let root = fs.alloc_inode(T_DIR)?;
        assert_eq!(root, ROOT_INODE);
        // The root's `..` is itself, and counts as its one link.
        fs.append(root, &dot_entries(root, root))?;
        fs.set_nlink(root, 1);
        Ok(fs)
    }

    pub fn superblock(&self) -> &Superblock {
        &self.sb
    }

    /// Data blocks and inodes not handed out yet.
    pub fn free_blocks(&self) -> u32 {
        self.sb.size - self.next_block
    }

    pub fn free_inodes(&self) -> u32 {
        self.sb.ninodes - self.next_inode
    }

    /// The image as built so far.
    pub fn finish(self) -> Vec<u8> {
        self.image
    }

    fn mark_used(&mut self, b: u32) {
        let (blk, byte, mask) = self.sb.bit_pos(b);
        block_mut(&mut self.image, blk)[byte] |= mask;
    }

    fn alloc_block(&mut self) -> Result<u32, String> {
        if self.next_block >= self.sb.size {
            return Err(format!(
                "no space left: all {} data blocks are used",
                self.sb.nblocks
            ));
        }
        let b = self.next_block;
        self.next_block += 1;
        self.mark_used(b);
        Ok(b)
    }

    fn alloc_inode(&mut self, kind: u16) -> Result<u32, String> {
        if self.next_inode >= self.sb.ninodes {
            return Err(format!("no inode left: all {} are used", self.sb.ninodes - 1));
        }
        let inum = self.next_inode;
        self.next_inode += 1;
        let d = Dinode {
            kind,
            ..Dinode::default()
        };
        write_inode(&mut self.image, &self.sb, inum, &d);
        Ok(inum)
    }

    fn inode(&self, inum: u32) -> Dinode {
        read_inode(&self.image, &self.sb, inum)
    }

    fn set_nlink(&mut self, inum: u32, nlink: u16) {
        let mut d = self.inode(inum);
        d.nlink = nlink;
        write_inode(&mut self.image, &self.sb, inum, &d);
    }

    /// Adds `data` at the end of inode `inum`'s contents, allocating blocks
    /// (and the indirect block) as it goes.
    fn append(&mut self, inum: u32, data: &[u8]) -> Result<(), String> {
        let mut d = self.inode(inum);
        let mut off = d.size as usize;
        let end = off + data.len();
        if end > MAX_FILE_SIZE as usize {
            return Err(format!(
                "{end} bytes is more than the largest file ({MAX_FILE_SIZE})"
            ));
        }
        let mut from = 0;
        while off < end {
            let k = off / BSIZE;
            // A block boundary at the end of the file: block k is new.
            if off.is_multiple_of(BSIZE) {
                let b = self.alloc_block()?;
                if k < NDIRECT {
                    d.addrs[k] = b;
                } else {
                    if d.addrs[NDIRECT] == 0 {
                        // The indirect block comes before the block it points to.
                        let ind = b;
                        d.addrs[NDIRECT] = ind;
                        let b = self.alloc_block()?;
                        set_indirect(&mut self.image, ind, k - NDIRECT, b);
                    } else {
                        set_indirect(&mut self.image, d.addrs[NDIRECT], k - NDIRECT, b);
                    }
                }
            }
            let b = crate::bmap(&self.image, &self.sb, &d, k)?;
            let n = (BSIZE - off % BSIZE).min(end - off);
            let at = off % BSIZE;
            block_mut(&mut self.image, b)[at..at + n].copy_from_slice(&data[from..from + n]);
            off += n;
            from += n;
        }
        d.size = end as u32;
        write_inode(&mut self.image, &self.sb, inum, &d);
        Ok(())
    }

    /// The inode `path` names, if any.
    fn lookup(&self, path: &str) -> Result<Option<u32>, String> {
        let mut at = ROOT_INODE;
        for name in path_names(path) {
            match self.dir_lookup(at, name)? {
                Some(next) => at = next,
                None => return Ok(None),
            }
        }
        Ok(Some(at))
    }

    /// `name` in directory `dir`.
    fn dir_lookup(&self, dir: u32, name: &[u8]) -> Result<Option<u32>, String> {
        let d = self.inode(dir);
        if d.kind != T_DIR {
            return Err("not a directory".into());
        }
        let data = contents(&self.image, &self.sb, &d)?;
        Ok(dirents(&data)
            .iter()
            .find(|e| e.inum != 0 && e.name() == name)
            .map(|e| u32::from(e.inum)))
    }

    /// The directory `path` should go in, and its last name, checked: the
    /// directory exists and has nothing by that name yet.
    fn parent_of<'p>(&self, path: &'p str) -> Result<(u32, &'p [u8]), String> {
        let names = path_names(path);
        let Some((&name, dirs)) = names.split_last() else {
            return Err(format!("{path}: the root already exists"));
        };
        if !valid_name(name) {
            return Err(format!(
                "{path}: `{}` can't be a name (1 to 14 bytes, no `/`, not `.` or `..`)",
                show_name(name)
            ));
        }
        let mut dir = ROOT_INODE;
        for (i, &n) in dirs.iter().enumerate() {
            let so_far = || {
                format!(
                    "/{}",
                    dirs[..=i]
                        .iter()
                        .map(|n| show_name(n))
                        .collect::<Vec<_>>()
                        .join("/")
                )
            };
            dir = self
                .dir_lookup(dir, n)
                .map_err(|e| format!("{path}: {e}"))?
                .ok_or_else(|| format!("{path}: no directory {}", so_far()))?;
            if self.inode(dir).kind != T_DIR {
                return Err(format!("{path}: {} is not a directory", so_far()));
            }
        }
        if self.dir_lookup(dir, name)?.is_some() {
            return Err(format!("{path}: already exists"));
        }
        Ok((dir, name))
    }

    /// Adds entry `name` → `inum` to directory `dir`.
    fn link(&mut self, dir: u32, name: &[u8], inum: u32) -> Result<(), String> {
        let inum16 = u16::try_from(inum).map_err(|_| format!("inode {inum} doesn't fit an entry"))?;
        self.append(dir, &Dirent::new(inum16, name).encode())
    }

    /// Makes the directory `path` (its parent must exist) and returns its
    /// inode.
    pub fn mkdir(&mut self, path: &str) -> Result<u32, String> {
        let (parent, name) = self.parent_of(path)?;
        let inum = self.alloc_inode(T_DIR)?;
        self.append(inum, &dot_entries(inum, parent))?;
        self.link(parent, name, inum)?;
        // Its entry in the parent, and its `..` naming the parent.
        self.set_nlink(inum, 1);
        let up = self.inode(parent).nlink + 1;
        self.set_nlink(parent, up);
        Ok(inum)
    }

    /// Makes the file `path` (its directory must exist) holding `data`, and
    /// returns its inode.
    pub fn add_file(&mut self, path: &str, data: &[u8]) -> Result<u32, String> {
        let (parent, name) = self.parent_of(path)?;
        if data.len() > MAX_FILE_SIZE as usize {
            return Err(format!(
                "{path}: {} bytes is more than the largest file ({MAX_FILE_SIZE})",
                data.len()
            ));
        }
        let inum = self.alloc_inode(T_FILE)?;
        self.append(inum, data).map_err(|e| format!("{path}: {e}"))?;
        self.link(parent, name, inum)?;
        self.set_nlink(inum, 1);
        Ok(inum)
    }

    /// Makes the device node `path` for device `major`, `minor`.
    pub fn add_device(&mut self, path: &str, major: u16, minor: u16) -> Result<u32, String> {
        let (parent, name) = self.parent_of(path)?;
        let inum = self.alloc_inode(T_DEVICE)?;
        let d = Dinode {
            kind: T_DEVICE,
            major,
            minor,
            nlink: 1,
            ..Dinode::default()
        };
        write_inode(&mut self.image, &self.sb, inum, &d);
        self.link(parent, name, inum)?;
        Ok(inum)
    }

    /// Gives an existing file `existing` another name, `path`.
    pub fn link_file(&mut self, existing: &str, path: &str) -> Result<(), String> {
        let inum = self
            .lookup(existing)?
            .ok_or_else(|| format!("{existing}: no such file"))?;
        if self.inode(inum).kind == T_DIR {
            return Err(format!("{existing}: a directory can't have a second name"));
        }
        let (parent, name) = self.parent_of(path)?;
        self.link(parent, name, inum)?;
        let n = self.inode(inum).nlink + 1;
        self.set_nlink(inum, n);
        Ok(())
    }

    /// Copies the host directory `host` into the directory `at` (which must
    /// exist): its files and, recursively, its subdirectories, in name order.
    /// Names must be valid CMFS names.
    pub fn add_host_dir(&mut self, host: &Path, at: &str) -> Result<(), String> {
        let read = std::fs::read_dir(host).map_err(|e| format!("{}: {e}", host.display()))?;
        let mut entries = Vec::new();
        for e in read {
            let e = e.map_err(|e| format!("{}: {e}", host.display()))?;
            entries.push(e.path());
        }
        entries.sort();
        for path in entries {
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .ok_or_else(|| format!("{}: the name is not UTF-8", path.display()))?
                .to_string();
            let inside = format!("{}/{name}", at.trim_end_matches('/'));
            let meta = std::fs::metadata(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            if meta.is_dir() {
                self.mkdir(&inside)?;
                self.add_host_dir(&path, &inside)?;
            } else if meta.is_file() {
                let data = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
                self.add_file(&inside, &data)?;
            } else {
                return Err(format!("{}: not a file or a directory", path.display()));
            }
        }
        Ok(())
    }
}

/// Sets entry `i` of indirect block `ind` to `b`.
fn set_indirect(image: &mut [u8], ind: u32, i: usize, b: u32) {
    block_mut(image, ind)[4 * i..4 * i + 4].copy_from_slice(&b.to_le_bytes());
}

/// A new directory's first two entries: `.` (itself) and `..` (its parent).
fn dot_entries(me: u32, parent: u32) -> Vec<u8> {
    let mut v = Vec::with_capacity(2 * DIRENT_SIZE);
    v.extend_from_slice(&Dirent::new(me as u16, b".").encode());
    v.extend_from_slice(&Dirent::new(parent as u16, b"..").encode());
    v
}

/// mkfs in one call: a filesystem shaped by `opts`, holding a copy of the
/// host directory `root` (if any) as `/`.
pub fn mkfs(opts: &Options, root: Option<&Path>) -> Result<Vec<u8>, String> {
    let mut fs = Mkfs::new(opts)?;
    if let Some(root) = root {
        fs.add_host_dir(root, "/")?;
    }
    Ok(fs.finish())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::read::Fs;

    fn small() -> Options {
        Options {
            size: 400,
            ninodes: 32,
            nlog: 8,
        }
    }

    #[test]
    fn an_empty_filesystem() {
        let fs = Mkfs::new(&small()).unwrap();
        let sb = *fs.superblock();
        // One block for the root's entries, one inode for the root.
        assert_eq!(fs.free_blocks(), sb.nblocks - 1);
        assert_eq!(fs.free_inodes(), 32 - 2);
        let image = fs.finish();
        assert_eq!(image.len(), 400 * BSIZE);
        let r = Fs::open(&image).unwrap();
        let root = r.list("/").unwrap();
        let names: Vec<String> = root.iter().map(|e| e.name()).collect();
        assert_eq!(names, [".", ".."]);
        assert!(root.iter().all(|e| e.inum == ROOT_INODE));
        assert_eq!(r.inode(ROOT_INODE).unwrap().nlink, 1);
        assert_eq!(r.inode(ROOT_INODE).unwrap().size, 32);
        assert_eq!(r.free_blocks(), sb.nblocks - 1);
    }

    #[test]
    fn files_of_every_shape() {
        let mut fs = Mkfs::new(&small()).unwrap();
        fs.mkdir("/a").unwrap();
        fs.mkdir("/a/b").unwrap();
        let sizes = [
            0,
            1,
            BSIZE - 1,
            BSIZE,
            BSIZE + 1,
            NDIRECT * BSIZE,
            NDIRECT * BSIZE + 1,
            100 * BSIZE + 7,
        ];
        for (i, &n) in sizes.iter().enumerate() {
            let data: Vec<u8> = (0..n).map(|j| (j * 31 + i) as u8).collect();
            fs.add_file(&format!("/a/b/f{i}"), &data).unwrap();
        }
        let image = fs.finish();
        let r = Fs::open(&image).unwrap();
        for (i, &n) in sizes.iter().enumerate() {
            let data: Vec<u8> = (0..n).map(|j| (j * 31 + i) as u8).collect();
            assert_eq!(r.read_file(&format!("a/b//f{i}/")).unwrap(), data, "f{i}");
        }
        // `/a` has one subdirectory, the root one too.
        assert_eq!(r.inode(r.lookup("/a").unwrap()).unwrap().nlink, 2);
        assert_eq!(r.inode(ROOT_INODE).unwrap().nlink, 2);
        assert_eq!(r.lookup("/a/b/..").unwrap(), r.lookup("/a").unwrap());
        assert_eq!(r.lookup("/..").unwrap(), ROOT_INODE);
    }

    #[test]
    fn the_largest_file_fits_and_one_byte_more_does_not() {
        let opts = Options { size: 400, ..small() };
        let mut fs = Mkfs::new(&opts).unwrap();
        let big = vec![0x5a; MAX_FILE_SIZE as usize];
        fs.add_file("/big", &big).unwrap();
        let err = fs
            .add_file("/bigger", &vec![0; MAX_FILE_SIZE as usize + 1])
            .unwrap_err();
        assert!(err.contains("largest file"), "{err}");
        let image = fs.finish();
        assert_eq!(Fs::open(&image).unwrap().read_file("/big").unwrap(), big);
    }

    #[test]
    fn errors() {
        let mut fs = Mkfs::new(&small()).unwrap();
        fs.add_file("/f", b"x").unwrap();
        for (path, want) in [
            ("/f", "already exists"),
            ("/", "root already exists"),
            ("/nope/g", "no directory /nope"),
            ("/f/g", "/f is not a directory"),
            ("/abcdefghijklmno", "can't be a name"),
            ("/a/..", "can't be a name"),
        ] {
            let err = fs.add_file(path, b"").unwrap_err();
            assert!(err.contains(want), "{path}: {err}");
        }
        assert!(fs.mkdir("/f").unwrap_err().contains("already exists"));
        assert!(fs.link_file("/", "/r").unwrap_err().contains("directory"));
        // Out of inodes, then out of blocks.
        let mut fs = Mkfs::new(&small()).unwrap();
        for i in 0..30 {
            fs.add_file(&format!("/{i}"), b"").unwrap();
        }
        assert!(fs
            .add_file("/one-more", b"")
            .unwrap_err()
            .contains("no inode left"));
        let mut fs = Mkfs::new(&small()).unwrap();
        fs.add_file("/big", &vec![1; MAX_FILE_SIZE as usize]).unwrap();
        let err = fs.add_file("/huge", &vec![1; 150 * BSIZE]).unwrap_err();
        assert!(err.contains("no space left"), "{err}");
    }

    #[test]
    fn devices_and_links() {
        let mut fs = Mkfs::new(&small()).unwrap();
        fs.mkdir("/dev").unwrap();
        fs.add_device("/dev/console", 1, 0).unwrap();
        fs.add_file("/f", b"hi").unwrap();
        fs.link_file("/f", "/dev/g").unwrap();
        let image = fs.finish();
        let r = Fs::open(&image).unwrap();
        let c = r.inode(r.lookup("/dev/console").unwrap()).unwrap();
        assert_eq!(
            (c.kind, c.major, c.minor, c.size, c.nlink),
            (T_DEVICE, 1, 0, 0, 1)
        );
        assert_eq!(r.lookup("/f").unwrap(), r.lookup("/dev/g").unwrap());
        assert_eq!(r.inode(r.lookup("/f").unwrap()).unwrap().nlink, 2);
        assert!(r
            .read_file("/dev/console")
            .unwrap_err()
            .contains("not a regular file"));
        assert!(r.read_file("/dev").unwrap_err().contains("is a directory"));
    }

    #[test]
    fn the_same_inputs_give_the_same_bytes() {
        let build = || {
            let mut fs = Mkfs::new(&small()).unwrap();
            fs.mkdir("/d").unwrap();
            fs.add_file("/d/x", &[7; 3000]).unwrap();
            fs.finish()
        };
        assert_eq!(build(), build());
    }
}
