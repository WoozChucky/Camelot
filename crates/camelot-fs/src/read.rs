//! Reading an image: paths, directories and files, as the kernel would see
//! them after mounting. A committed transaction still in the log is replayed
//! into a private copy first, so a disk that crashed mid-transaction reads as
//! recovery will leave it.

use crate::{
    contents, dirents, fsck, path_names, read_inode, show_name, superblock, Dinode, Superblock, BSIZE,
    NO_INODE, ROOT_INODE, T_DEVICE, T_DIR, T_FILE, T_FREE, T_SOCKET,
};

/// A filesystem image opened for reading.
#[derive(Debug, Clone)]
pub struct Fs {
    image: Vec<u8>,
    sb: Superblock,
    replayed: usize,
}

/// A used directory entry, with what its inode says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: Vec<u8>,
    pub inum: u32,
    pub inode: Dinode,
}

impl Entry {
    /// The name as text.
    pub fn name(&self) -> String {
        show_name(&self.name)
    }
}

/// The letter `ls` shows for an inode type: `d`, `f`, `c` (device), `s`
/// (socket), `?`.
pub fn type_letter(kind: u16) -> char {
    match kind {
        T_DIR => 'd',
        T_FILE => 'f',
        T_DEVICE => 'c',
        T_SOCKET => 's',
        _ => '?',
    }
}

impl Fs {
    /// Opens `image`: checks the superblock and the log header, and replays a
    /// committed transaction into a copy.
    pub fn open(image: &[u8]) -> Result<Fs, String> {
        let sb = superblock(image)?;
        let mut copy = image[..sb.size as usize * BSIZE].to_vec();
        let replayed = fsck::recover(&mut copy)?;
        Ok(Fs {
            image: copy,
            sb,
            replayed,
        })
    }

    pub fn superblock(&self) -> &Superblock {
        &self.sb
    }

    /// Blocks replayed from the log when opening (0 if it was empty).
    pub fn replayed(&self) -> usize {
        self.replayed
    }

    /// The filesystem's blocks, as recovered.
    pub fn image(&self) -> &[u8] {
        &self.image
    }

    /// Inode `inum`.
    pub fn inode(&self, inum: u32) -> Result<Dinode, String> {
        if inum >= self.sb.ninodes {
            return Err(format!("inode {inum} is past the last ({})", self.sb.ninodes - 1));
        }
        Ok(read_inode(&self.image, &self.sb, inum))
    }

    /// The contents of inode `inum`.
    pub fn contents(&self, inum: u32) -> Result<Vec<u8>, String> {
        let d = self.inode(inum)?;
        contents(&self.image, &self.sb, &d).map_err(|e| format!("inode {inum}: {e}"))
    }

    /// The used entries of directory `inum`, `.` and `..` included, in order.
    pub fn dir(&self, inum: u32) -> Result<Vec<Entry>, String> {
        let d = self.inode(inum)?;
        if d.kind != T_DIR {
            return Err(format!("inode {inum} is not a directory"));
        }
        let mut out = Vec::new();
        for e in dirents(&self.contents(inum)?) {
            if u32::from(e.inum) == NO_INODE {
                continue;
            }
            let inum = u32::from(e.inum);
            out.push(Entry {
                name: e.name().to_vec(),
                inum,
                inode: self.inode(inum)?,
            });
        }
        Ok(out)
    }

    /// The inode `path` names, looked up from the root.
    pub fn lookup(&self, path: &str) -> Result<u32, String> {
        let mut at = ROOT_INODE;
        for name in path_names(path) {
            let entries = self
                .dir(at)
                .map_err(|_| format!("{path}: not a directory on the way"))?;
            at = entries
                .iter()
                .find(|e| e.name == name)
                .map(|e| e.inum)
                .ok_or_else(|| format!("{path}: no such file or directory"))?;
        }
        Ok(at)
    }

    /// The contents of the regular file at `path`.
    pub fn read_file(&self, path: &str) -> Result<Vec<u8>, String> {
        let inum = self.lookup(path)?;
        match self.inode(inum)?.kind {
            T_FILE => self.contents(inum),
            T_DIR => Err(format!("{path}: is a directory")),
            _ => Err(format!("{path}: not a regular file")),
        }
    }

    /// The used entries of the directory at `path`.
    pub fn list(&self, path: &str) -> Result<Vec<Entry>, String> {
        self.dir(self.lookup(path)?)
    }

    /// Data blocks the bitmap marks free.
    pub fn free_blocks(&self) -> u32 {
        (self.sb.datastart..self.sb.size)
            .filter(|&b| {
                let (blk, byte, mask) = self.sb.bit_pos(b);
                self.image[blk as usize * BSIZE + byte] & mask == 0
            })
            .count() as u32
    }

    /// Inodes of type 0 (inode 0, never used, not counted).
    pub fn free_inodes(&self) -> u32 {
        (1..self.sb.ninodes)
            .filter(|&i| read_inode(&self.image, &self.sb, i).kind == T_FREE)
            .count() as u32
    }

    /// Every path under the root, depth first in directory order, with its
    /// entry (`.` and `..` left out): the whole tree, e.g. to compare two
    /// states of a disk.
    pub fn walk(&self) -> Result<Vec<(String, Entry)>, String> {
        let mut out = Vec::new();
        self.walk_dir(ROOT_INODE, "", &mut out, 0)?;
        Ok(out)
    }

    fn walk_dir(
        &self,
        dir: u32,
        prefix: &str,
        out: &mut Vec<(String, Entry)>,
        depth: u32,
    ) -> Result<(), String> {
        if depth > self.sb.ninodes {
            return Err("the directories loop".into());
        }
        for e in self.dir(dir)? {
            if e.name == b"." || e.name == b".." {
                continue;
            }
            let path = format!("{prefix}/{}", e.name());
            let sub = (e.inode.kind == T_DIR).then_some(e.inum);
            out.push((path.clone(), e));
            if let Some(sub) = sub {
                self.walk_dir(sub, &path, out, depth + 1)?;
            }
        }
        Ok(())
    }

    /// `ls`'s listing of the directory at `path`: one line per entry, as the
    /// kernel's boot listing prints it (type, inode, links, size, name).
    pub fn ls(&self, path: &str) -> Result<String, String> {
        Ok(self.list(path)?.iter().map(ls_line).collect())
    }
}

/// One line of a listing: `d    1  3     128 .` (the kernel's `%c %4u %2u
/// %7u %s`).
pub fn ls_line(e: &Entry) -> String {
    format!(
        "{} {:4} {:2} {:7} {}\n",
        type_letter(e.inode.kind),
        e.inum,
        e.inode.nlink,
        e.inode.size,
        e.name()
    )
}
