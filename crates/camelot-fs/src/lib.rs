//! The Camelot filesystem (CMFS) on the host, the second implementation of
//! `docs/camelot-fs.md` (the kernel's, in `kernel/fs.c` and `kernel/log.c`, is
//! the first):
//!
//! * this module: the format itself, as constants and the three on-disk
//!   structures ([`Superblock`], [`Dinode`], [`Dirent`]) with their encodings;
//! * [`extent`]: the extent trees that map a file's contents (reading,
//!   checking and growing them);
//! * [`mkfs`]: builds an image, empty or filled from a host directory;
//! * [`read`]: reads one back ([`read::Fs`]: paths, directories, files), seeing
//!   a pending log as recovery would;
//! * [`boot`]: makes an image bootable (a bootloader and a kernel);
//! * [`fsck`]: checks every consistency rule of the document, replays the
//!   log ([`fsck::recover`]) and frees orphaned inodes
//!   ([`fsck::free_orphans`]), as the kernel's mount does.
//!
//! An image is a byte slice holding the disk from its first block. Everything
//! here works on images in memory; the `camelot-fs` binary reads and writes
//! files.

pub mod boot;
pub mod extent;
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
pub const VERSION: u32 = 3;

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
pub const INODE_SIZE: usize = 128;
pub const INODES_PER_BLOCK: u32 = (BSIZE / INODE_SIZE) as u32;
/// Where the inode's fields are: `i_block`, the extent tree's root, then
/// `flags`, the owner's `uid` and `gid`, the `mode`, then the reserved
/// bytes to the end.
pub const IBLOCK_AT: usize = 16;
pub const FLAGS_AT: usize = IBLOCK_AT + extent::ROOT_SIZE;
pub const UID_AT: usize = FLAGS_AT + 4;
pub const GID_AT: usize = UID_AT + 4;
pub const MODE_AT: usize = GID_AT + 4;
pub const RESERVED_AT: usize = MODE_AT + 2;
pub const RESERVED_SIZE: usize = INODE_SIZE - RESERVED_AT;
/// `mode`'s bits, Unix's (Linux's `S_I*` without the type, which is the
/// inode's `type`): read, write and execute (search, for a directory) for
/// the owner (0o700), the group (0o070) and everyone else (0o007), and
/// above them set-user-ID, set-group-ID and sticky. No other bit is
/// defined.
pub const S_ISUID: u16 = 0o4000;
pub const S_ISGID: u16 = 0o2000;
pub const S_ISVTX: u16 = 0o1000;
pub const MODE_BITS: u16 = 0o7777;
/// The superuser's user and group ids, which own what mkfs makes.
pub const ROOT_UID: u32 = 0;
pub const ROOT_GID: u32 = 0;
/// `flags`: the inode's blocks are being freed ("Truncation"). No other bit
/// is defined.
pub const TRUNCATING: u32 = 1;
/// The most blocks a file can have (its logical block numbers are u32),
/// and so its largest size in bytes.
pub const MAX_FILE_BLOCKS: u64 = 1 << 32;
pub const MAX_FILE_SIZE: u64 = MAX_FILE_BLOCKS * BSIZE as u64;

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
/// the kernel's `LOGSIZE` (44) slots.
pub const DEFAULT_SIZE: u32 = 2048;
pub const DEFAULT_INODES: u32 = 256;
pub const DEFAULT_LOG: u32 = 45;

/// Inode types (`Dinode::kind`).
pub const T_FREE: u16 = 0;
pub const T_DIR: u16 = 1;
pub const T_FILE: u16 = 2;
pub const T_DEVICE: u16 = 3;
/// A local socket's name (`bind`): no contents, as a device. 4 is never an
/// inode's type: it is `fstat`'s for a pipe, and 5 the same for a socket.
pub const T_SOCKET: u16 = 5;

/// Whether `kind` is an allocated inode's type.
pub fn valid_kind(kind: u16) -> bool {
    matches!(kind, T_DIR | T_FILE | T_DEVICE | T_SOCKET)
}

/// Whether an inode of `kind` has no contents: a device or a socket, whose
/// size is 0 and `i_block` all zeros.
pub fn no_contents(kind: u16) -> bool {
    kind == T_DEVICE || kind == T_SOCKET
}

/// The mode mkfs gives a new inode of `kind`: 0o755 for a directory, 0o644
/// for a file (0o755 for an executable: `Mkfs::add_file`), 0o666 for a
/// device, 0o777 for a socket (Linux's with no umask).
pub fn default_mode(kind: u16) -> u16 {
    match kind {
        T_DIR => 0o755,
        T_DEVICE => 0o666,
        T_SOCKET => 0o777,
        _ => 0o644,
    }
}

/// `mode` as `ls -l` shows it, after the type's letter: `rwxr-xr-x`, with
/// `s`/`S` for set-user-ID and set-group-ID and `t`/`T` for sticky in the
/// execute places (lower case when the execute bit is set too), as
/// coreutils' `ls` shows them.
pub fn mode_string(mode: u16) -> String {
    let mut s = String::new();
    for (shift, special, letter) in [(6, S_ISUID, 's'), (3, S_ISGID, 's'), (0, S_ISVTX, 't')] {
        let bits = (mode >> shift) & 7;
        s.push(if bits & 4 != 0 { 'r' } else { '-' });
        s.push(if bits & 2 != 0 { 'w' } else { '-' });
        let x = bits & 1 != 0;
        s.push(match (mode & special != 0, x) {
            (true, true) => letter,
            (true, false) => letter.to_ascii_uppercase(),
            (false, true) => 'x',
            (false, false) => '-',
        });
    }
    s
}

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
            return Err(if self.version < VERSION {
                format!(
                    "version {}, an older format (this reads version {VERSION}): rebuild the image",
                    self.version
                )
            } else {
                format!("version {} (this reads version {VERSION})", self.version)
            });
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

/// An on-disk inode (128 bytes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Dinode {
    /// `T_FREE`, `T_DIR`, `T_FILE`, `T_DEVICE` or `T_SOCKET`.
    pub kind: u16,
    pub major: u16,
    pub minor: u16,
    pub nlink: u16,
    pub size: u64,
    /// `i_block`: the root of the extent tree ([`extent`]); zeros for a
    /// device or a socket.
    pub root: [u8; extent::ROOT_SIZE],
    /// `TRUNCATING`, or 0.
    pub flags: u32,
    /// The owner: a user and a group id.
    pub uid: u32,
    pub gid: u32,
    /// The permission bits ([`MODE_BITS`]).
    pub mode: u16,
    /// The reserved bytes, which must be 0 (kept to check that they are).
    pub reserved: [u8; RESERVED_SIZE],
}

impl Default for Dinode {
    /// All zero: a free inode.
    fn default() -> Self {
        Dinode {
            kind: T_FREE,
            major: 0,
            minor: 0,
            nlink: 0,
            size: 0,
            root: [0; extent::ROOT_SIZE],
            flags: 0,
            uid: 0,
            gid: 0,
            mode: 0,
            reserved: [0; RESERVED_SIZE],
        }
    }
}

impl Dinode {
    /// A new inode of `kind`, empty: a directory or file with an empty
    /// extent tree, a device or a socket with an `i_block` of zeros; root's,
    /// with the kind's default mode.
    pub fn new(kind: u16) -> Dinode {
        Dinode {
            kind,
            uid: ROOT_UID,
            gid: ROOT_GID,
            mode: default_mode(kind),
            root: if no_contents(kind) {
                [0; extent::ROOT_SIZE]
            } else {
                extent::empty_root()
            },
            ..Dinode::default()
        }
    }

    pub fn decode(b: &[u8]) -> Dinode {
        let mut root = [0; extent::ROOT_SIZE];
        root.copy_from_slice(&b[IBLOCK_AT..FLAGS_AT]);
        let mut reserved = [0; RESERVED_SIZE];
        reserved.copy_from_slice(&b[RESERVED_AT..INODE_SIZE]);
        Dinode {
            kind: u16_at(b, 0),
            major: u16_at(b, 2),
            minor: u16_at(b, 4),
            nlink: u16_at(b, 6),
            size: u64::from(u32_at(b, 8)) | (u64::from(u32_at(b, 12)) << 32),
            root,
            flags: u32_at(b, FLAGS_AT),
            uid: u32_at(b, UID_AT),
            gid: u32_at(b, GID_AT),
            mode: u16_at(b, MODE_AT),
            reserved,
        }
    }

    pub fn encode(&self, b: &mut [u8]) {
        put_u16(b, 0, self.kind);
        put_u16(b, 2, self.major);
        put_u16(b, 4, self.minor);
        put_u16(b, 6, self.nlink);
        put_u32(b, 8, self.size as u32);
        put_u32(b, 12, (self.size >> 32) as u32);
        b[IBLOCK_AT..FLAGS_AT].copy_from_slice(&self.root);
        put_u32(b, FLAGS_AT, self.flags);
        put_u32(b, UID_AT, self.uid);
        put_u32(b, GID_AT, self.gid);
        put_u16(b, MODE_AT, self.mode);
        b[RESERVED_AT..INODE_SIZE].copy_from_slice(&self.reserved);
    }

    /// Blocks the contents take: ceil(size / BSIZE).
    pub fn content_blocks(&self) -> u64 {
        self.size.div_ceil(BSIZE as u64)
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

/// The disk blocks of a file's or directory's contents, in order, checked:
/// its extent tree is valid ([`extent::walk`]) and maps exactly the blocks
/// its size needs.
pub fn data_blocks(image: &[u8], sb: &Superblock, d: &Dinode) -> Result<Vec<u32>, String> {
    let map = extent::walk(image, sb, &d.root)?;
    if map.blocks() != d.content_blocks() {
        return Err(format!(
            "the size ({} bytes) needs {} blocks, the extent tree maps {}",
            d.size,
            d.content_blocks(),
            map.blocks()
        ));
    }
    Ok(map.content_blocks().collect())
}

/// The contents of a file or directory: its `size` bytes. A file being
/// truncated (`TRUNCATING`) has none: the blocks it still holds are being
/// freed, and the next mount finishes that.
pub fn contents(image: &[u8], sb: &Superblock, d: &Dinode) -> Result<Vec<u8>, String> {
    if d.flags & TRUNCATING != 0 {
        extent::walk(image, sb, &d.root)?;
        return Ok(Vec::new());
    }
    if d.size > MAX_FILE_SIZE {
        return Err(format!("size {} is more than the largest file", d.size));
    }
    let blocks = data_blocks(image, sb, d)?;
    let size = usize::try_from(d.size).map_err(|_| format!("size {} does not fit in memory", d.size))?;
    let mut out = Vec::with_capacity(size);
    for (k, &b) in blocks.iter().enumerate() {
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
        // 2 + 45 = 47; 256 inodes are 32 blocks; 2048 blocks one bitmap block.
        assert_eq!(
            (sb.logstart, sb.inodestart, sb.bmapstart, sb.datastart, sb.nblocks),
            (2, 47, 79, 80, 1968)
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
            "8192 inode blocks don't fit"
        );
        // The smallest: boot, super, 2 log, 1 inode block, 1 bitmap, 1 data.
        let sb = Superblock::layout(7, 2, 2).unwrap();
        assert_eq!(sb.datastart, 6);
        assert!(Superblock::layout(6, 2, 2).is_err());
        // 8 inodes to a block, and one bitmap block per 8192 blocks.
        assert_eq!(Superblock::layout(8192, 8, 2).unwrap().datastart, 6);
        assert_eq!(Superblock::layout(8192, 9, 2).unwrap().datastart, 7);
        assert_eq!(Superblock::layout(8193, 16, 2).unwrap().datastart, 8);
    }

    #[test]
    fn superblock_round_trips_and_validates() {
        let sb = Superblock::layout(300, 40, 10).unwrap();
        let b = sb.encode();
        assert_eq!(&b[..4], b"CMFS");
        assert!(b[SUPERBLOCK_SIZE..].iter().all(|&x| x == 0));
        assert_eq!(Superblock::decode(&b), sb);
        let mut bad = sb;
        bad.version = 1;
        assert!(bad
            .validate(300)
            .unwrap_err()
            .contains("version 1, an older format (this reads version 3): rebuild the image"));
        let mut bad = sb;
        bad.datastart += 1;
        assert!(bad.validate(300).unwrap_err().contains("misplaced"));
        let mut bad = sb;
        bad.magic = 0;
        assert!(bad.validate(300).unwrap_err().contains("no filesystem"));
    }

    #[test]
    fn dinode_layout() {
        let mut d = Dinode {
            kind: T_FILE,
            major: 0x1122,
            minor: 0x3344,
            nlink: 2,
            size: 0x0506_0708_0102_0304,
            flags: TRUNCATING,
            uid: 1000,
            gid: 0x0102_0304,
            mode: 0o4755,
            ..Dinode::new(T_FILE)
        };
        d.reserved[RESERVED_SIZE - 1] = 9;
        let mut b = [0; INODE_SIZE];
        d.encode(&mut b);
        assert_eq!(
            &b[..16],
            &[2, 0, 0x22, 0x11, 0x44, 0x33, 2, 0, 4, 3, 2, 1, 8, 7, 6, 5]
        );
        assert_eq!(&b[16..18], &[0x0a, 0xf3], "i_block's header");
        assert_eq!(&b[76..80], &[1, 0, 0, 0]);
        assert_eq!(&b[80..90], &[0xe8, 3, 0, 0, 4, 3, 2, 1, 0xed, 9]);
        assert_eq!(b[127], 9);
        assert_eq!(Dinode::decode(&b), d);
        assert_eq!(
            d.content_blocks(),
            0x0506_0708_0102_0304u64.div_ceil(BSIZE as u64)
        );
        assert_eq!(Dinode::new(T_DEVICE).root, [0; extent::ROOT_SIZE]);
        assert_eq!(
            (FLAGS_AT, UID_AT, GID_AT, MODE_AT, RESERVED_AT, RESERVED_SIZE),
            (76, 80, 84, 88, 90, 38)
        );
        let new = Dinode::new(T_DIR);
        assert_eq!((new.uid, new.gid, new.mode), (0, 0, 0o755));
        assert_eq!(MAX_FILE_SIZE, 1 << 42);
    }

    #[test]
    fn modes_read_as_ls_shows_them() {
        assert_eq!(mode_string(0o755), "rwxr-xr-x");
        assert_eq!(mode_string(0o600), "rw-------");
        assert_eq!(mode_string(0o4755), "rwsr-xr-x");
        assert_eq!(mode_string(0o2644), "rw-r-Sr--");
        assert_eq!(mode_string(0o1777), "rwxrwxrwt");
        assert_eq!(mode_string(0o1666), "rw-rw-rwT");
        assert_eq!(mode_string(0), "---------");
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
