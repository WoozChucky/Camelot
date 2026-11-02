//! The images directory: creating, listing and deleting disk images, and attaching
//! them for block I/O.
//!
//! A disk is a regular file directly in the directory whose name is a valid disk name
//! (`validate_name`: lowercase, no paths, no Windows devices) and whose size is a whole
//! number of 512-byte sectors; the file holds the raw sectors, as Grail's image files
//! do. Anything else in the directory is left alone: it is not listed, cannot be
//! attached or deleted, and its name cannot be used for a new disk. A disk is looked up
//! by its exact directory entry, never just by opening `dir/name`: on a filesystem that
//! ignores case (Windows, macOS), the path for disk `root` opens a file named `ROOT`,
//! which is not a disk.
//!
//! New images are sparse: they take no space until written, and on Windows (NTFS)
//! writing near the end of a fresh image does not first make the filesystem write zeros
//! over everything before it (see [`Store::create`]).
//!
//! Concurrency rule: an image has at most one writer, and a writer excludes readers
//! (exclusive attach); any number of read-only attachments may share an image that has
//! no writer. Attaching against the rule, or deleting an attached image, is `Busy`.
//! The table of attachments lives behind one lock, which every create, list, delete,
//! attach and detach takes; reads and writes go through the attachment's own file
//! handle without it, so one client's I/O never waits for another's.

use crate::{lock, Fail};
use avalon_proto::{validate_name, DiskInfo, ErrorCode, MAX_BLOCK_SECTORS, SECTOR_SIZE};
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Largest image Lake creates by default, in sectors: what one Grail disk can address
/// (its `BLK_CAPACITY` register is 32 bits), just under 2 TiB.
pub const MAX_IMAGE_SECTORS: u64 = u32::MAX as u64;

const SECTOR: u64 = SECTOR_SIZE as u64;

/// A directory of disk images and who has them attached.
#[derive(Debug)]
pub struct Store {
    dir: PathBuf,
    max_sectors: u64,
    attached: Mutex<BTreeMap<String, Attachments>>,
}

/// The attachments of one image. An image with none has no entry.
#[derive(Debug, Default, Clone, Copy)]
struct Attachments {
    readers: u32,
    writer: bool,
}

impl Store {
    /// Serves the images in `dir`, creating the directory if it does not exist. New
    /// images may be up to `max_sectors` sectors.
    pub fn open(dir: impl Into<PathBuf>, max_sectors: u64) -> io::Result<Store> {
        let dir = dir.into();
        fs::create_dir_all(&dir)?;
        if !fs::metadata(&dir)?.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{} is not a directory", dir.display()),
            ));
        }
        Ok(Store {
            dir,
            max_sectors,
            attached: Mutex::new(BTreeMap::new()),
        })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }

    /// `DiskCreate`: a new zero-filled image of `sectors` sectors.
    ///
    /// The image is a sparse file of that length: it reads as zeros without any being
    /// written. Unix filesystems make a file extended by `set_len` sparse by
    /// themselves. NTFS does not: it keeps a "valid data length", and the first write
    /// past it makes it write zeros up to that point, synchronously, inside that one
    /// write. For a write near the end of a fresh 48 GiB image that takes longer than
    /// Grail waits for an answer, so the guest's disk would fail. On Windows the file
    /// is therefore marked sparse first ([`make_sparse`]); where the filesystem cannot
    /// do that (FAT), the zeros are written here instead, so `DiskCreate` pays for
    /// them and not a guest's write.
    pub fn create(&self, name: &str, sectors: u64) -> Result<(), Fail> {
        check_name(name)?;
        let too_big = || {
            Fail::new(
                ErrorCode::OverBudget,
                format!("a disk is 1 to {} sectors, not {sectors}", self.max_sectors),
            )
        };
        if sectors == 0 || sectors > self.max_sectors {
            return Err(too_big());
        }
        // `max_sectors` is the server's to choose; no size may overflow a file length.
        let len = sectors.checked_mul(SECTOR).ok_or_else(too_big)?;
        let _table = lock(&self.attached);
        let path = self.path(name);
        let exists = || {
            Fail::new(
                ErrorCode::AlreadyExists,
                format!("a disk named '{name}' already exists"),
            )
        };
        // Anything by that name, a disk or not, is in the way. (Windows refuses to
        // create a file over a directory as "access denied", so look first.)
        if fs::symlink_metadata(&path).is_ok() {
            return Err(exists());
        }
        let file = match File::options().write(true).create_new(true).open(&path) {
            Ok(file) => file,
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => return Err(exists()),
            Err(e) => return Err(io_fail(&format!("cannot create disk '{name}'"), e)),
        };
        // A new sparse file of that length reads as zeros, without writing them.
        let sized = if make_sparse(&file).is_ok() {
            file.set_len(len)
        } else {
            // Not sparse: write the last sector, so the filesystem fills the rest in
            // now.
            file.set_len(len)
                .and_then(|_| (&file).seek(SeekFrom::Start(len - SECTOR)))
                .and_then(|_| (&file).write_all(&[0; SECTOR_SIZE]))
        };
        if let Err(e) = sized {
            drop(file);
            let _ = fs::remove_file(&path);
            return Err(io_fail(
                &format!("cannot make disk '{name}' {sectors} sectors long"),
                e,
            ));
        }
        Ok(())
    }

    /// The size in sectors of disk `name`, if there is one: the directory has an entry
    /// spelled exactly `name` (see the module comment), and it is an image.
    fn disk_sectors(&self, name: &str) -> Result<Option<u64>, Fail> {
        let entries = fs::read_dir(&self.dir).map_err(|e| io_fail("cannot read the images directory", e))?;
        for entry in entries {
            let entry = entry.map_err(|e| io_fail("cannot read the images directory", e))?;
            if entry.file_name() == name {
                return Ok(image_sectors(&entry.path()));
            }
        }
        Ok(None)
    }

    /// `ListDisks`: every image, in name order, with its attachments.
    pub fn list(&self) -> Result<Vec<DiskInfo>, Fail> {
        let table = lock(&self.attached);
        let entries = fs::read_dir(&self.dir).map_err(|e| io_fail("cannot read the images directory", e))?;
        let mut disks = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|e| io_fail("cannot read the images directory", e))?;
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            if validate_name(&name).is_err() {
                continue;
            }
            // Follows links, like attaching does.
            let Some(sectors) = image_sectors(&entry.path()) else {
                continue;
            };
            let a = table.get(&name).copied().unwrap_or_default();
            disks.push(DiskInfo {
                name,
                sectors,
                readers: a.readers,
                writer: a.writer,
            });
        }
        disks.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(disks)
    }

    /// `DiskDelete`: removes an image nobody has attached.
    pub fn delete(&self, name: &str) -> Result<(), Fail> {
        check_name(name)?;
        let table = lock(&self.attached);
        if self.disk_sectors(name)?.is_none() {
            return Err(no_such_disk(name));
        }
        if let Some(a) = table.get(name) {
            return Err(busy(name, a));
        }
        fs::remove_file(self.path(name)).map_err(|e| io_fail(&format!("cannot delete disk '{name}'"), e))
    }

    /// `BlockOpen`: attaches image `name`, read-write (the only attachment it may
    /// have) or read-only (shared with other readers). The attachment ends when the
    /// [`Image`] is dropped.
    pub fn attach(self: &Arc<Self>, name: &str, read_only: bool) -> Result<Image, Fail> {
        check_name(name)?;
        let mut table = lock(&self.attached);
        let Some(sectors) = self.disk_sectors(name)? else {
            return Err(no_such_disk(name));
        };
        let path = self.path(name);
        if let Some(a) = table.get(name) {
            if a.writer || !read_only {
                return Err(busy(name, a));
            }
        }
        let file = File::options()
            .read(true)
            .write(!read_only)
            .open(&path)
            .map_err(|e| match e.kind() {
                io::ErrorKind::NotFound => no_such_disk(name),
                _ => io_fail(&format!("cannot open disk '{name}'"), e),
            })?;
        let a = table.entry(name.to_string()).or_default();
        if read_only {
            a.readers += 1;
        } else {
            a.writer = true;
        }
        Ok(Image {
            file,
            sectors,
            lease: Lease {
                store: Arc::clone(self),
                name: name.to_string(),
                read_only,
            },
        })
    }

    /// Ends an attachment (an [`Image`] being dropped).
    fn release(&self, name: &str, read_only: bool) {
        let mut table = lock(&self.attached);
        if let Some(a) = table.get_mut(name) {
            if read_only {
                a.readers = a.readers.saturating_sub(1);
            } else {
                a.writer = false;
            }
            if a.readers == 0 && !a.writer {
                table.remove(name);
            }
        }
    }
}

/// One attachment of an image: its own file handle, for block I/O. Dropping it
/// detaches the image.
#[derive(Debug)]
pub struct Image {
    // Declared before `lease`, so the file is closed before the image is released.
    file: File,
    sectors: u64,
    lease: Lease,
}

/// Releases an attachment when dropped.
#[derive(Debug)]
struct Lease {
    store: Arc<Store>,
    name: String,
    read_only: bool,
}

impl Drop for Lease {
    fn drop(&mut self) {
        self.store.release(&self.name, self.read_only);
    }
}

impl Image {
    pub fn name(&self) -> &str {
        &self.lease.name
    }

    /// The image's size in sectors, as it was when attached. (Images never change
    /// size through Lake.)
    pub fn sectors(&self) -> u64 {
        self.sectors
    }

    pub fn read_only(&self) -> bool {
        self.lease.read_only
    }

    /// `OutOfRange` unless sectors `sector` to `sector + count` are on the image,
    /// computed without overflow. Returns the byte offset of `sector`.
    fn check_range(&self, sector: u64, count: u64) -> Result<u64, Fail> {
        match sector.checked_add(count) {
            Some(end) if end <= self.sectors => Ok(sector * SECTOR),
            _ => Err(Fail::new(
                ErrorCode::OutOfRange,
                format!(
                    "{count} sectors from sector {sector} run past the end of disk '{}' ({} sectors)",
                    self.name(),
                    self.sectors
                ),
            )),
        }
    }

    /// `BlockRead`: `count` sectors from `sector`.
    pub fn read(&mut self, sector: u64, count: u32) -> Result<Vec<u8>, Fail> {
        if count > MAX_BLOCK_SECTORS {
            return Err(Fail::new(
                ErrorCode::Malformed,
                format!("{count} sectors is more than one request may read ({MAX_BLOCK_SECTORS})"),
            ));
        }
        let offset = self.check_range(sector, count as u64)?;
        let mut data = vec![0; count as usize * SECTOR_SIZE];
        self.file
            .seek(SeekFrom::Start(offset))
            .and_then(|_| self.file.read_exact(&mut data))
            .map_err(|e| io_fail(&format!("cannot read disk '{}'", self.lease.name), e))?;
        Ok(data)
    }

    /// `BlockWrite`: `data` (whole sectors) from `sector`.
    pub fn write(&mut self, sector: u64, data: &[u8]) -> Result<(), Fail> {
        if self.read_only() {
            return Err(Fail::new(
                ErrorCode::ReadOnly,
                format!("disk '{}' is attached read-only here", self.name()),
            ));
        }
        if !data.len().is_multiple_of(SECTOR_SIZE) {
            return Err(Fail::new(
                ErrorCode::Malformed,
                format!("{} bytes is not a whole number of sectors", data.len()),
            ));
        }
        let offset = self.check_range(sector, (data.len() / SECTOR_SIZE) as u64)?;
        self.file
            .seek(SeekFrom::Start(offset))
            .and_then(|_| self.file.write_all(data))
            .map_err(|e| io_fail(&format!("cannot write disk '{}'", self.lease.name), e))
    }

    /// `BlockFlush`: once this returns, every earlier write through this attachment is
    /// on the host's own disk (as far as its OS can tell).
    pub fn flush(&mut self) -> Result<(), Fail> {
        if self.read_only() {
            return Ok(());
        }
        self.file
            .flush()
            .and_then(|_| self.file.sync_data())
            .map_err(|e| io_fail(&format!("cannot flush disk '{}'", self.lease.name), e))
    }
}

/// The size in sectors of the image at `path`, if it is one: a regular file of whole
/// sectors.
fn image_sectors(path: &Path) -> Option<u64> {
    let meta = fs::metadata(path).ok()?;
    (meta.is_file() && meta.len().is_multiple_of(SECTOR)).then_some(meta.len() / SECTOR)
}

/// Marks a new, empty file sparse, so that extending it ([`File::set_len`]) and then
/// writing anywhere in it costs only what is written (see [`Store::create`]). Fails
/// where the filesystem has no sparse files.
#[cfg(windows)]
fn make_sparse(file: &File) -> io::Result<()> {
    use std::ffi::c_void;
    use std::os::windows::io::AsRawHandle;
    use std::ptr;

    /// `CTL_CODE(FILE_DEVICE_FILE_SYSTEM, 49, METHOD_BUFFERED, FILE_SPECIAL_ACCESS)`.
    const FSCTL_SET_SPARSE: u32 = 0x0009_00c4;

    #[link(name = "kernel32")]
    extern "system" {
        fn DeviceIoControl(
            device: *mut c_void,
            code: u32,
            in_buffer: *const c_void,
            in_size: u32,
            out_buffer: *mut c_void,
            out_size: u32,
            returned: *mut u32,
            overlapped: *mut c_void,
        ) -> i32;
    }

    let mut returned = 0;
    // SAFETY: the handle is open for writing for the duration of the call (`file` is
    // borrowed), and it is synchronous (no OVERLAPPED). With no input buffer,
    // FSCTL_SET_SPARSE sets the attribute; it has no output.
    let ok = unsafe {
        DeviceIoControl(
            file.as_raw_handle(),
            FSCTL_SET_SPARSE,
            ptr::null(),
            0,
            ptr::null_mut(),
            0,
            &mut returned,
            ptr::null_mut(),
        )
    };
    if ok == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// Unix filesystems make an extended file sparse by themselves.
#[cfg(not(windows))]
fn make_sparse(_file: &File) -> io::Result<()> {
    Ok(())
}

fn check_name(name: &str) -> Result<(), Fail> {
    validate_name(name).map_err(|e| Fail::new(ErrorCode::InvalidName, e))
}

fn no_such_disk(name: &str) -> Fail {
    Fail::new(ErrorCode::NoSuchDisk, format!("no disk named '{name}'"))
}

fn busy(name: &str, a: &Attachments) -> Fail {
    let why = if a.writer {
        "attached read-write".to_string()
    } else {
        format!("attached read-only {} time(s)", a.readers)
    };
    Fail::new(ErrorCode::Busy, format!("disk '{name}' is {why}"))
}

fn io_fail(what: &str, e: io::Error) -> Fail {
    Fail::new(ErrorCode::Io, format!("{what}: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh images directory, removed when dropped.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> TempDir {
            let dir = std::env::temp_dir().join(format!("lake-store-{}-{tag}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            TempDir(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn store(dir: &TempDir) -> Arc<Store> {
        Arc::new(Store::open(&dir.0, 1000).unwrap())
    }

    fn code<T: std::fmt::Debug>(r: Result<T, Fail>) -> ErrorCode {
        r.unwrap_err().code
    }

    #[test]
    fn create_list_delete() {
        let dir = TempDir::new("cld");
        let s = store(&dir);
        assert!(s.list().unwrap().is_empty());
        s.create("root", 8).unwrap();
        s.create("a.img", 1).unwrap();
        assert_eq!(fs::read(dir.0.join("root")).unwrap(), vec![0; 8 * 512]);
        let names: Vec<_> = s
            .list()
            .unwrap()
            .into_iter()
            .map(|d| (d.name, d.sectors))
            .collect();
        assert_eq!(names, [("a.img".to_string(), 1), ("root".to_string(), 8)]);

        assert_eq!(code(s.create("root", 8)), ErrorCode::AlreadyExists);
        assert_eq!(code(s.create("Root", 8)), ErrorCode::InvalidName);
        assert_eq!(code(s.create("../root", 8)), ErrorCode::InvalidName);
        assert_eq!(code(s.create("nul", 8)), ErrorCode::InvalidName);
        assert_eq!(code(s.create("big", 1001)), ErrorCode::OverBudget);
        assert_eq!(code(s.create("empty", 0)), ErrorCode::OverBudget);
        assert!(!dir.0.join("big").exists() && !dir.0.join("empty").exists());

        s.delete("root").unwrap();
        assert_eq!(code(s.delete("root")), ErrorCode::NoSuchDisk);
        assert_eq!(code(s.delete("..")), ErrorCode::InvalidName);
        assert_eq!(s.list().unwrap().len(), 1);
    }

    #[test]
    fn other_files_are_left_alone() {
        let dir = TempDir::new("other");
        let s = store(&dir);
        fs::write(dir.0.join("odd"), [0; 100]).unwrap();
        fs::write(dir.0.join("UPPER"), [0; 512]).unwrap();
        fs::create_dir(dir.0.join("sub")).unwrap();
        fs::write(dir.0.join("ok"), [0; 1024]).unwrap();
        let names: Vec<_> = s.list().unwrap().into_iter().map(|d| d.name).collect();
        assert_eq!(names, ["ok"]);
        for name in ["odd", "sub"] {
            assert_eq!(code(s.attach(name, true)), ErrorCode::NoSuchDisk, "{name}");
            assert_eq!(code(s.delete(name)), ErrorCode::NoSuchDisk, "{name}");
            assert_eq!(code(s.create(name, 1)), ErrorCode::AlreadyExists, "{name}");
        }
        assert!(dir.0.join("odd").exists() && dir.0.join("sub").is_dir());

        // Where the filesystem ignores case, the path for disk "upper" opens "UPPER",
        // and the one for "data.img" opens "Data.img": those files are still not
        // disks.
        fs::write(dir.0.join("Data.img"), [0xcd; 512]).unwrap();
        assert_eq!(s.list().unwrap().len(), 1);
        for name in ["upper", "data.img"] {
            for read_only in [true, false] {
                assert_eq!(code(s.attach(name, read_only)), ErrorCode::NoSuchDisk, "{name}");
            }
            assert_eq!(code(s.delete(name)), ErrorCode::NoSuchDisk, "{name}");
        }
        assert_eq!(fs::read(dir.0.join("UPPER")).unwrap(), [0; 512]);
        assert_eq!(fs::read(dir.0.join("Data.img")).unwrap(), [0xcd; 512]);
    }

    #[test]
    fn new_images_are_sparse() {
        let dir = TempDir::new("sparse");
        let s = Arc::new(Store::open(&dir.0, MAX_IMAGE_SECTORS).unwrap());
        // 1 GiB: if it were not sparse, NTFS would write all of it in the write below
        // (or the fallback in `create`, at create time).
        let sectors = 1 << 21;
        s.create("big", sectors).unwrap();
        let mut w = s.attach("big", false).unwrap();
        w.write(sectors - 1, &[0xab; 512]).unwrap();
        w.flush().unwrap();
        assert_eq!(w.read(sectors - 1, 1).unwrap(), [0xab; 512]);
        assert_eq!(w.read(sectors / 2, 1).unwrap(), [0; 512]);
        drop(w);
        let meta = fs::metadata(dir.0.join("big")).unwrap();
        assert_eq!(meta.len(), sectors * SECTOR);
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            const FILE_ATTRIBUTE_SPARSE_FILE: u32 = 0x200;
            assert_ne!(meta.file_attributes() & FILE_ATTRIBUTE_SPARSE_FILE, 0);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            assert!(
                meta.blocks() * 512 < 1 << 20,
                "{} blocks allocated",
                meta.blocks()
            );
        }
    }

    #[test]
    fn sizes_never_overflow() {
        let dir = TempDir::new("overflow");
        let s = Arc::new(Store::open(&dir.0, u64::MAX).unwrap());
        for sectors in [u64::MAX, u64::MAX / SECTOR + 1, 1 << 63] {
            assert_eq!(
                code(s.create("huge", sectors)),
                ErrorCode::OverBudget,
                "{sectors}"
            );
        }
        assert!(!dir.0.join("huge").exists());
    }

    #[test]
    fn one_writer_or_many_readers() {
        let dir = TempDir::new("rw");
        let s = store(&dir);
        s.create("d", 4).unwrap();
        let info = |s: &Store| {
            let d = &s.list().unwrap()[0];
            (d.readers, d.writer)
        };

        let w = s.attach("d", false).unwrap();
        assert_eq!(info(&s), (0, true));
        assert_eq!(code(s.attach("d", false)), ErrorCode::Busy);
        assert_eq!(code(s.attach("d", true)), ErrorCode::Busy);
        assert_eq!(code(s.delete("d")), ErrorCode::Busy);
        drop(w);
        assert_eq!(info(&s), (0, false));

        let r1 = s.attach("d", true).unwrap();
        let r2 = s.attach("d", true).unwrap();
        assert_eq!(info(&s), (2, false));
        assert_eq!(code(s.attach("d", false)), ErrorCode::Busy);
        assert_eq!(code(s.delete("d")), ErrorCode::Busy);
        drop(r1);
        assert_eq!(info(&s), (1, false));
        assert_eq!(code(s.attach("d", false)), ErrorCode::Busy);
        drop(r2);
        let w = s.attach("d", false).unwrap();
        drop(w);
        s.delete("d").unwrap();
        assert_eq!(code(s.attach("d", true)), ErrorCode::NoSuchDisk);
        assert_eq!(code(s.attach("D", true)), ErrorCode::InvalidName);
    }

    #[test]
    fn block_io() {
        let dir = TempDir::new("io");
        let s = store(&dir);
        s.create("d", 4).unwrap();
        let mut w = s.attach("d", false).unwrap();
        assert_eq!((w.name(), w.sectors(), w.read_only()), ("d", 4, false));
        let data: Vec<u8> = (0..1024).map(|i| i as u8).collect();
        w.write(2, &data).unwrap();
        w.flush().unwrap();
        assert_eq!(w.read(2, 2).unwrap(), data);
        assert_eq!(w.read(0, 1).unwrap(), vec![0; 512]);
        assert_eq!(w.read(0, 4).unwrap().len(), 2048);

        // Ranges past the end fail and change nothing, without overflowing.
        for (sector, count) in [(4, 1), (3, 2), (0, 5), (u64::MAX, 1), (u64::MAX - 1, 2048)] {
            assert_eq!(
                code(w.read(sector, count)),
                ErrorCode::OutOfRange,
                "{sector} {count}"
            );
            let buf = vec![0xff; count as usize * 512];
            assert_eq!(
                code(w.write(sector, &buf)),
                ErrorCode::OutOfRange,
                "{sector} {count}"
            );
        }
        assert_eq!(code(w.write(0, &[1; 100])), ErrorCode::Malformed);
        assert_eq!(code(w.read(0, 2049)), ErrorCode::Malformed);
        assert_eq!(w.read(3, 1).unwrap(), data[512..]);
        drop(w);

        let mut r = s.attach("d", true).unwrap();
        assert!(r.read_only());
        assert_eq!(r.read(2, 2).unwrap(), data);
        assert_eq!(code(r.write(0, &[0; 512])), ErrorCode::ReadOnly);
        r.flush().unwrap();
        assert_eq!(fs::read(dir.0.join("d")).unwrap()[1024..], data[..]);
    }

    #[test]
    fn store_dir_must_be_a_directory() {
        let dir = TempDir::new("notdir");
        fs::write(&dir.0, b"file").unwrap();
        assert!(Store::open(&dir.0, 1).is_err());
        fs::remove_file(&dir.0).unwrap();
        // Created when missing.
        let s = Store::open(dir.0.join("a/b"), 1).unwrap();
        assert!(s.dir().is_dir());
    }
}
