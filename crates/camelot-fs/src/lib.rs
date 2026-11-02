//! The Camelot filesystem (CMFS) on the host, the second implementation of
//! `docs/camelot-fs.md` (the kernel's, in `kernel/fs.c` and `kernel/log.c`, is
//! the first):
//!
//! * this module: the format itself, as constants and the three on-disk
//!   structures ([`Superblock`], [`Dinode`], [`Dirent`]) with their encodings;
//! * [`mkfs`]: builds an image, empty or filled from a host directory;
//! * [`read`]: reads one back ([`read::Fs`]: paths, directories, files), seeing
//!   a pending log as recovery would;
//! * [`fsck`]: checks every consistency rule of the document, replays the
//!   log ([`fsck::recover`]) and frees orphaned inodes
//!   ([`fsck::free_orphans`]), as the kernel's mount does.
//!
//! An image is a byte slice holding the disk from its first block. Everything
//! here works on images in memory; the `camelot-fs` binary reads and writes
//! files.

pub mod fsck;
pub mod mkfs;
pub mod read;

/// Bytes in a block: two 512-byte sectors (the kernel's `BSIZE`).
pub const BSIZE: usize = 1024;
/// Bytes in a disk sector (spec section 9.6).
pub const SECTOR_SIZE: usize = 512;

/// The superblock's `magic`: the bytes `C M F S`.
pub const MAGIC: u32 = 0x5346_4d43;
/// The format version this crate reads and writes.
pub const VERSION: u32 = 1;

/// The boot block, which the filesystem leaves alone.
pub const BOOT_BLOCK: u32 = 0;
/// The superblock.
pub const SUPER_BLOCK: u32 = 1;
/// Where the log starts: right after the superblock.
pub const LOG_START: u32 = 2;

/// Inode 0 is never used (a directory entry naming it is a free slot).
pub const NO_INODE: u32 = 0;
/// The root directory's inode.
pub const ROOT_INODE: u32 = 1;

/// Bytes in an on-disk inode, and inodes per block.
pub const INODE_SIZE: usize = 64;
pub const INODES_PER_BLOCK: u32 = (BSIZE / INODE_SIZE) as u32;
/// Direct block pointers in an inode.
pub const NDIRECT: usize = 12;
/// Pointers in the indirect block.
pub const NINDIRECT: usize = BSIZE / 4;
/// The most blocks a file has, and its largest size in bytes.
pub const MAX_FILE_BLOCKS: usize = NDIRECT + NINDIRECT;
pub const MAX_FILE_SIZE: u32 = (MAX_FILE_BLOCKS * BSIZE) as u32;

/// Bytes in a directory entry, and the longest name.
pub const DIRENT_SIZE: usize = 16;
pub const DIRSIZ: usize = 14;

/// Blocks one bitmap block covers.
pub const BITS_PER_BLOCK: u32 = (BSIZE * 8) as u32;

/// Limits on `nlog` (the header included) and `ninodes`.
pub const MIN_LOG: u32 = 2;
pub const MAX_LOG: u32 = 128;
pub const MIN_INODES: u32 = 2;
pub const MAX_INODES: u32 = 65536;
/// The largest filesystem: the kernel's buffer cache stops at block 2^31.
pub const MAX_SIZE: u32 = 1 << 31;

/// What mkfs makes unless told otherwise: 2 MiB, 256 inodes, a header and
/// the kernel's `LOGSIZE` (20) slots.
pub const DEFAULT_SIZE: u32 = 2048;
pub const DEFAULT_INODES: u32 = 256;
pub const DEFAULT_LOG: u32 = 21;

/// Inode types (`Dinode::kind`).
pub const T_FREE: u16 = 0;
pub const T_DIR: u16 = 1;
pub const T_FILE: u16 = 2;
pub const T_DEVICE: u16 = 3;

/// `ceil(a / b)`.
fn div_ceil(a: u32, b: u32) -> u32 {
    a.div_ceil(b)
}

fn u16_at(b: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([b[off], b[off + 1]])
}

fn u32_at(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}

fn put_u16(b: &mut [u8], off: usize, v: u16) {
    b[off..off + 2].copy_from_slice(&v.to_le_bytes());
}

fn put_u32(b: &mut [u8], off: usize, v: u32) {
    b[off..off + 4].copy_from_slice(&v.to_le_bytes());
}

/// Block `n` of `image` (which must hold it).
pub fn block(image: &[u8], n: u32) -> &[u8] {
    let at = n as usize * BSIZE;
    &image[at..at + BSIZE]
}

/// Block `n` of `image`, to change.
pub fn block_mut(image: &mut [u8], n: u32) -> &mut [u8] {
    let at = n as usize * BSIZE;
    &mut image[at..at + BSIZE]
}

/// The superblock (block 1). Every field is a u32; see the document's table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Superblock {
    pub magic: u32,
    pub version: u32,
    pub size: u32,
    pub nblocks: u32,
    pub ninodes: u32,
    pub nlog: u32,
    pub logstart: u32,
    pub inodestart: u32,
    pub bmapstart: u32,
    pub datastart: u32,
}

/// Bytes the superblock's fields take.
pub const SUPERBLOCK_SIZE: usize = 40;

impl Superblock {
    /// The superblock of a filesystem of `size` blocks with `ninodes` inodes
    /// and `nlog` log blocks: every region placed as the document says.
    pub fn layout(size: u32, ninodes: u32, nlog: u32) -> Result<Superblock, String> {
        if !(MIN_LOG..=MAX_LOG).contains(&nlog) {
            return Err(format!(
                "the log must be {MIN_LOG} to {MAX_LOG} blocks, not {nlog}"
            ));
        }
        if !(MIN_INODES..=MAX_INODES).contains(&ninodes) {
            return Err(format!(
                "there must be {MIN_INODES} to {MAX_INODES} inodes, not {ninodes}"
            ));
        }
        if size > MAX_SIZE {
            return Err(format!(
                "{size} blocks is more than the {MAX_SIZE} a filesystem can have"
            ));
        }
        let logstart = LOG_START;
        let inodestart = logstart + nlog;
        let bmapstart = inodestart + div_ceil(ninodes, INODES_PER_BLOCK);
        let datastart = bmapstart + div_ceil(size, BITS_PER_BLOCK);
        if datastart >= size {
            return Err(format!(
                "{size} blocks is too small: the metadata alone takes {datastart}, \
                 and the root directory needs a data block"
            ));
        }
        Ok(Superblock {
            magic: MAGIC,
            version: VERSION,
            size,
            nblocks: size - datastart,
            ninodes,
            nlog,
            logstart,
            inodestart,
            bmapstart,
            datastart,
        })
    }

    /// Reads the superblock from its block's bytes, without checking it.
    pub fn decode(b: &[u8]) -> Superblock {
        Superblock {
            magic: u32_at(b, 0),
            version: u32_at(b, 4),
            size: u32_at(b, 8),
            nblocks: u32_at(b, 12),
            ninodes: u32_at(b, 16),
            nlog: u32_at(b, 20),
            logstart: u32_at(b, 24),
            inodestart: u32_at(b, 28),
            bmapstart: u32_at(b, 32),
            datastart: u32_at(b, 36),
        }
    }

    /// The superblock's block: its fields, then zeros.
    pub fn encode(&self) -> [u8; BSIZE] {
        let mut b = [0; BSIZE];
        for (i, v) in [
            self.magic,
            self.version,
            self.size,
            self.nblocks,
            self.ninodes,
            self.nlog,
            self.logstart,
            self.inodestart,
            self.bmapstart,
            self.datastart,
        ]
        .into_iter()
        .enumerate()
        {
            put_u32(&mut b, i * 4, v);
        }
        b
    }

    /// Checks every rule of the document's superblock section: the magic, the
    /// version, the limits and every derived field. `disk_blocks` is how many
    /// blocks the disk holds.
    pub fn validate(&self, disk_blocks: u64) -> Result<(), String> {
        if self.magic != MAGIC {
            return Err(format!("no filesystem (magic {:#010x})", self.magic));
        }
        if self.version != VERSION {
            return Err(format!("version {} (this reads version {VERSION})", self.version));
        }
        let want = Superblock::layout(self.size, self.ninodes, self.nlog)?;
        if *self != want {
            return Err(format!("the regions are misplaced: {self:?}, want {want:?}"));
        }
        if u64::from(self.size) > disk_blocks {
            return Err(format!(
                "the filesystem is {} blocks, the disk only {disk_blocks}",
                self.size
            ));
        }
        Ok(())
    }

    /// Where inode `inum` is: (block, byte offset in it).
    pub fn inode_pos(&self, inum: u32) -> (u32, usize) {
        (
            self.inodestart + inum / INODES_PER_BLOCK,
            (inum % INODES_PER_BLOCK) as usize * INODE_SIZE,
        )
    }

    /// Where block `b`'s bitmap bit is: (block, byte offset, bit mask).
    pub fn bit_pos(&self, b: u32) -> (u32, usize, u8) {
        let i = b % BITS_PER_BLOCK;
        (
            self.bmapstart + b / BITS_PER_BLOCK,
            (i / 8) as usize,
            1 << (i % 8),
        )
    }

    /// Whether `b` is in the data region.
    pub fn is_data(&self, b: u32) -> bool {
        (self.datastart..self.size).contains(&b)
    }
}

/// An on-disk inode (64 bytes).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Dinode {
    /// `T_FREE`, `T_DIR`, `T_FILE` or `T_DEVICE`.
    pub kind: u16,
    pub major: u16,
    pub minor: u16,
    pub nlink: u16,
    pub size: u32,
    /// The 12 direct pointers, then the indirect one.
    pub addrs: [u32; NDIRECT + 1],
}

impl Dinode {
    pub fn decode(b: &[u8]) -> Dinode {
        let mut addrs = [0; NDIRECT + 1];
        for (i, a) in addrs.iter_mut().enumerate() {
            *a = u32_at(b, 12 + 4 * i);
        }
        Dinode {
            kind: u16_at(b, 0),
            major: u16_at(b, 2),
            minor: u16_at(b, 4),
            nlink: u16_at(b, 6),
            size: u32_at(b, 8),
            addrs,
        }
    }

    pub fn encode(&self, b: &mut [u8]) {
        put_u16(b, 0, self.kind);
        put_u16(b, 2, self.major);
        put_u16(b, 4, self.minor);
        put_u16(b, 6, self.nlink);
        put_u32(b, 8, self.size);
        for (i, a) in self.addrs.iter().enumerate() {
            put_u32(b, 12 + 4 * i, *a);
        }
    }

    /// The indirect pointer.
    pub fn indirect(&self) -> u32 {
        self.addrs[NDIRECT]
    }

    /// Blocks the contents take: ceil(size / BSIZE).
    pub fn content_blocks(&self) -> usize {
        (self.size as usize).div_ceil(BSIZE)
    }
}

/// A directory entry (16 bytes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Dirent {
    /// The inode, or 0 for a free slot.
    pub inum: u16,
    /// The name, NUL-padded.
    pub name: [u8; DIRSIZ],
}

impl Dirent {
    /// An entry naming `inum` `name`, which must be a valid name.
    pub fn new(inum: u16, name: &[u8]) -> Dirent {
        assert!(name.len() <= DIRSIZ);
        let mut n = [0; DIRSIZ];
        n[..name.len()].copy_from_slice(name);
        Dirent { inum, name: n }
    }

    pub fn decode(b: &[u8]) -> Dirent {
        let mut name = [0; DIRSIZ];
        name.copy_from_slice(&b[2..DIRENT_SIZE]);
        Dirent {
            inum: u16_at(b, 0),
            name,
        }
    }

    pub fn encode(&self) -> [u8; DIRENT_SIZE] {
        let mut b = [0; DIRENT_SIZE];
        put_u16(&mut b, 0, self.inum);
        b[2..].copy_from_slice(&self.name);
        b
    }

    /// The name without its padding: the bytes up to the first NUL.
    pub fn name(&self) -> &[u8] {
        let end = self.name.iter().position(|&c| c == 0).unwrap_or(DIRSIZ);
        &self.name[..end]
    }

    /// Whether the padding after the name is all NUL (it must be).
    pub fn padded(&self) -> bool {
        let n = self.name().len();
        self.name[n..].iter().all(|&c| c == 0)
    }
}

/// Whether `name` may name a directory entry: 1 to 14 bytes, no NUL or `/`,
/// and not `.` or `..` (which only the first two entries are).
pub fn valid_name(name: &[u8]) -> bool {
    !name.is_empty()
        && name.len() <= DIRSIZ
        && !name.iter().any(|&c| c == 0 || c == b'/')
        && name != b"."
        && name != b".."
}

/// The log header (block `logstart`): the home blocks of the committed
/// transaction, empty when there is none.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LogHeader {
    pub blocks: Vec<u32>,
}

impl LogHeader {
    /// Reads the header and checks it against `sb`: `n` fits the log, the
    /// home blocks are distinct and each in `inodestart..size`.
    pub fn decode(b: &[u8], sb: &Superblock) -> Result<LogHeader, String> {
        let n = u32_at(b, 0);
        if n > sb.nlog - 1 {
            return Err(format!(
                "the log header says {n} blocks, the log has {} slots",
                sb.nlog - 1
            ));
        }
        let blocks: Vec<u32> = (0..n as usize).map(|i| u32_at(b, 4 + 4 * i)).collect();
        for (i, &home) in blocks.iter().enumerate() {
            if !(sb.inodestart..sb.size).contains(&home) {
                return Err(format!(
                    "log slot {i} belongs to block {home}, outside the filesystem's writable blocks"
                ));
            }
            if blocks[..i].contains(&home) {
                return Err(format!("log slots for block {home} twice"));
            }
        }
        Ok(LogHeader { blocks })
    }

    /// The header's block: `n`, the entries, then zeros.
    pub fn encode(&self) -> [u8; BSIZE] {
        let mut b = [0; BSIZE];
        put_u32(&mut b, 0, self.blocks.len() as u32);
        for (i, &home) in self.blocks.iter().enumerate() {
            put_u32(&mut b, 4 + 4 * i, home);
        }
        b
    }
}

/// Inode `inum` of `image` (which must be below `sb.ninodes`).
pub fn read_inode(image: &[u8], sb: &Superblock, inum: u32) -> Dinode {
    let (b, off) = sb.inode_pos(inum);
    Dinode::decode(&block(image, b)[off..off + INODE_SIZE])
}

/// Writes inode `inum` of `image`.
pub fn write_inode(image: &mut [u8], sb: &Superblock, inum: u32, d: &Dinode) {
    let (b, off) = sb.inode_pos(inum);
    d.encode(&mut block_mut(image, b)[off..off + INODE_SIZE]);
}

/// The block holding logical block `k` of a file, 0 if it has none (the
/// pointer, or the indirect block, is 0). `k` must be below
/// `MAX_FILE_BLOCKS`. An indirect pointer outside the data region is an
/// error, so a corrupt inode can't send a reader outside the image.
pub fn bmap(image: &[u8], sb: &Superblock, d: &Dinode, k: usize) -> Result<u32, String> {
    if k < NDIRECT {
        return Ok(d.addrs[k]);
    }
    let ind = d.indirect();
    if ind == 0 {
        return Ok(0);
    }
    if !sb.is_data(ind) {
        return Err(format!("indirect block {ind} is outside the data region"));
    }
    let i = k - NDIRECT;
    Ok(u32_at(block(image, ind), 4 * i))
}

/// The contents of a file or directory: its `size` bytes. Every block below
/// the size must be allocated and in the data region.
pub fn contents(image: &[u8], sb: &Superblock, d: &Dinode) -> Result<Vec<u8>, String> {
    if d.size > MAX_FILE_SIZE {
        return Err(format!("size {} is more than the largest file", d.size));
    }
    let size = d.size as usize;
    let mut out = Vec::with_capacity(size);
    for k in 0..d.content_blocks() {
        let b = bmap(image, sb, d, k)?;
        if !sb.is_data(b) {
            return Err(format!(
                "block {k} of the contents is {b}, outside the data region"
            ));
        }
        let n = (size - k * BSIZE).min(BSIZE);
        out.extend_from_slice(&block(image, b)[..n]);
    }
    Ok(out)
}

/// The entries of a directory's contents, free slots included.
pub fn dirents(data: &[u8]) -> Vec<Dirent> {
    data.as_chunks::<DIRENT_SIZE>()
        .0
        .iter()
        .map(|e| Dirent::decode(e))
        .collect()
}

/// Splits `path` into its names: `/` separates them, repeated and trailing
/// slashes count for nothing, a leading one is optional.
pub fn path_names(path: &str) -> Vec<&[u8]> {
    path.split('/')
        .filter(|s| !s.is_empty())
        .map(str::as_bytes)
        .collect()
}

/// Shows a name as text (names are bytes; Camelot's own are ASCII).
pub fn show_name(name: &[u8]) -> String {
    String::from_utf8_lossy(name).into_owned()
}

/// Reads and checks the superblock of `image`.
pub fn superblock(image: &[u8]) -> Result<Superblock, String> {
    if image.len() < 2 * BSIZE {
        return Err(format!("{} bytes is too small to hold a superblock", image.len()));
    }
    if !image.len().is_multiple_of(SECTOR_SIZE) {
        return Err(format!("{} bytes is not a whole number of sectors", image.len()));
    }
    let b = block(image, SUPER_BLOCK);
    let sb = Superblock::decode(b);
    sb.validate((image.len() / BSIZE) as u64)?;
    if let Some(i) = b[SUPERBLOCK_SIZE..].iter().position(|&x| x != 0) {
        return Err(format!(
            "superblock byte {} is not zero (bytes {SUPERBLOCK_SIZE} to {} must be)",
            SUPERBLOCK_SIZE + i,
            BSIZE - 1
        ));
    }
    Ok(sb)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_layout() {
        let sb = Superblock::layout(DEFAULT_SIZE, DEFAULT_INODES, DEFAULT_LOG).unwrap();
        // 2 + 21 = 23; 256 inodes are 16 blocks; 2048 blocks one bitmap block.
        assert_eq!(
            (sb.logstart, sb.inodestart, sb.bmapstart, sb.datastart, sb.nblocks),
            (2, 23, 39, 40, 2008)
        );
        assert_eq!(sb.validate(2048), Ok(()));
        assert!(sb.validate(2047).unwrap_err().contains("the disk only 2047"));
    }

    #[test]
    fn layout_limits() {
        assert!(Superblock::layout(100, 16, 1).is_err());
        assert!(Superblock::layout(100, 16, 129).is_err());
        assert!(Superblock::layout(100, 1, 2).is_err());
        assert!(Superblock::layout(100, 65537, 2).is_err());
        assert!(
            Superblock::layout(100, 65536, 2).is_err(),
            "4096 inode blocks don't fit"
        );
        // The smallest: boot, super, 2 log, 1 inode block, 1 bitmap, 1 data.
        let sb = Superblock::layout(7, 2, 2).unwrap();
        assert_eq!(sb.datastart, 6);
        assert!(Superblock::layout(6, 2, 2).is_err());
        // Bitmap blocks: one per 8192 blocks.
        assert_eq!(Superblock::layout(8192, 16, 2).unwrap().datastart, 6);
        assert_eq!(Superblock::layout(8193, 16, 2).unwrap().datastart, 7);
    }

    #[test]
    fn superblock_round_trips_and_validates() {
        let sb = Superblock::layout(300, 40, 10).unwrap();
        let b = sb.encode();
        assert_eq!(&b[..4], b"CMFS");
        assert!(b[SUPERBLOCK_SIZE..].iter().all(|&x| x == 0));
        assert_eq!(Superblock::decode(&b), sb);
        let mut bad = sb;
        bad.version = 2;
        assert!(bad.validate(300).unwrap_err().contains("version 2"));
        let mut bad = sb;
        bad.datastart += 1;
        assert!(bad.validate(300).unwrap_err().contains("misplaced"));
        let mut bad = sb;
        bad.magic = 0;
        assert!(bad.validate(300).unwrap_err().contains("no filesystem"));
    }

    #[test]
    fn dinode_layout() {
        let d = Dinode {
            kind: T_FILE,
            major: 0x1122,
            minor: 0x3344,
            nlink: 2,
            size: 0x01020304,
            addrs: [7; NDIRECT + 1],
        };
        let mut b = [0; INODE_SIZE];
        d.encode(&mut b);
        assert_eq!(&b[..12], &[2, 0, 0x22, 0x11, 0x44, 0x33, 2, 0, 4, 3, 2, 1]);
        assert_eq!(&b[60..64], &[7, 0, 0, 0]);
        assert_eq!(Dinode::decode(&b), d);
        assert_eq!(d.content_blocks(), 0x01020304usize.div_ceil(BSIZE));
    }

    #[test]
    fn dirent_names() {
        let e = Dirent::new(5, b"README");
        let b = e.encode();
        assert_eq!(&b[..8], b"\x05\x00README");
        assert_eq!(Dirent::decode(&b).name(), b"README");
        let full = Dirent::new(1, b"abcdefghijklmn");
        assert_eq!(full.name(), b"abcdefghijklmn");
        assert!(full.padded());
        let mut gap = Dirent::new(1, b"ab");
        gap.name[5] = b'x';
        assert!(!gap.padded());
        assert!(valid_name(b"a.b"));
        for bad in [&b""[..], b".", b"..", b"a/b", b"a\0", b"abcdefghijklmno"] {
            assert!(!valid_name(bad), "{bad:?}");
        }
    }

    #[test]
    fn log_header_rules() {
        let sb = Superblock::layout(100, 16, 4).unwrap();
        let h = LogHeader {
            blocks: vec![sb.inodestart, 99],
        };
        assert_eq!(LogHeader::decode(&h.encode(), &sb), Ok(h));
        for (blocks, want) in [
            (vec![1, 2, 3, 4], "3 slots"),
            (vec![sb.logstart], "outside"),
            (vec![100], "outside"),
            (vec![50, 50], "twice"),
        ] {
            let err = LogHeader::decode(&LogHeader { blocks }.encode(), &sb).unwrap_err();
            assert!(err.contains(want), "{err}");
        }
    }
}
