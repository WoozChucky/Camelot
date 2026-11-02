//! Xcal block device (spec section 9.6): a disk of 512-byte sectors behind a few
//! registers. Software names a sector range and a buffer in RAM, writes a command, and
//! the device copies the sectors between the disk and RAM itself (DMA), then sets
//! `BLK_STATUS.DONE` and, if enabled, raises its PLIC line.
//!
//! The device only does the register protocol and the checks. Where the sectors live is
//! a [`BlockBackend`], chosen per disk by whoever builds the machine: a [`MemDisk`]
//! (tests, scratch disks), a [`FileDisk`] (an image file on the host), or a backend of
//! the host's own (a network disk, say) handed to [`crate::Machine::with_backends`].
//!
//! Commands run to completion during the write to `BLK_CMD`, so `BLK_STATUS.BUSY` is
//! never seen set. Every check happens before any data moves, and a backend failure is
//! an error code for the program: nothing a program does can make the host panic.

use crate::bus::{BusFault, RAM_BASE};
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;

/// Bytes per sector.
pub const SECTOR_SIZE: usize = 512;

// Register offsets from the start of a disk's page.
pub const REG_CAPACITY: u32 = 0x00;
pub const REG_FLAGS: u32 = 0x04;
pub const REG_SECTOR: u32 = 0x08;
pub const REG_DMA: u32 = 0x0c;
pub const REG_COUNT: u32 = 0x10;
pub const REG_CMD: u32 = 0x14;
pub const REG_STATUS: u32 = 0x18;
pub const REG_ERROR: u32 = 0x1c;
pub const REG_CTRL: u32 = 0x20;

/// `BLK_FLAGS` bit 0: the disk is read-only; writes fail with [`error::READ_ONLY`].
pub const FLAG_READ_ONLY: u32 = 1 << 0;

/// `BLK_CMD` values.
pub const CMD_READ: u32 = 1;
pub const CMD_WRITE: u32 = 2;
pub const CMD_FLUSH: u32 = 3;

/// `BLK_STATUS` bit 0: a command is running. Grail completes commands immediately, so it
/// always reads 0, but drivers should wait for it to clear (or for `DONE`).
pub const STATUS_BUSY: u32 = 1 << 0;
/// `BLK_STATUS` bit 1: a command finished (with or without an error) and has not been
/// acknowledged. Write 1 to clear it.
pub const STATUS_DONE: u32 = 1 << 1;

/// `BLK_CTRL` bit 0: raise the disk's PLIC source while `BLK_STATUS.DONE` is set.
pub const CTRL_IRQ_ENABLE: u32 = 1 << 0;

/// `BLK_ERROR` values: the outcome of the last command.
pub mod error {
    /// It worked.
    pub const NONE: u32 = 0;
    /// `BLK_CMD` was not a known command.
    pub const BAD_COMMAND: u32 = 1;
    /// `BLK_SECTOR + BLK_COUNT` is past the end of the disk.
    pub const OUT_OF_RANGE: u32 = 2;
    /// The buffer (`BLK_DMA`, `BLK_COUNT` sectors long) is not entirely inside RAM.
    pub const BAD_DMA: u32 = 3;
    /// A write to a read-only disk.
    pub const READ_ONLY: u32 = 4;
    /// The backend failed (a host I/O error). Some sectors may have been transferred.
    pub const IO: u32 = 5;
}

/// Where a disk's sectors live. One per disk, owned by the machine, so it must be
/// `Send` (machines run on their own threads); it is used through `dyn BlockBackend`.
///
/// The device only calls [`read`](BlockBackend::read) and [`write`](BlockBackend::write)
/// with whole sectors inside the capacity. Implementations should still return an error
/// rather than panic if that is not the case.
pub trait BlockBackend: Send {
    /// Size of the disk in sectors. Must not change while the machine runs.
    fn capacity(&self) -> u64;

    /// Whether writes are refused. The device checks this before calling `write`.
    fn read_only(&self) -> bool {
        false
    }

    /// Fills `buf` (a whole number of sectors) from the disk, starting at `sector`.
    fn read(&mut self, sector: u64, buf: &mut [u8]) -> io::Result<()>;

    /// Writes `buf` (a whole number of sectors) to the disk, starting at `sector`.
    fn write(&mut self, sector: u64, buf: &[u8]) -> io::Result<()>;

    /// Makes every completed write durable (as far as the backend can).
    fn flush(&mut self) -> io::Result<()>;
}

/// The byte range `[sector * 512, + len)` of a disk with `capacity` sectors, if `len`
/// is whole sectors and the range is on the disk. Backends use it to validate requests.
/// Never overflows: a range whose byte offsets do not fit in a `u64` (a backend with a
/// capacity above 2^55 sectors) is an error, like one past the end.
pub fn byte_range(capacity: u64, sector: u64, len: usize) -> io::Result<std::ops::Range<u64>> {
    let invalid = |msg: &str| io::Error::new(io::ErrorKind::InvalidInput, msg.to_string());
    if !len.is_multiple_of(SECTOR_SIZE) {
        return Err(invalid("not a whole number of sectors"));
    }
    let count = (len / SECTOR_SIZE) as u64;
    let bytes = |sectors: u64| sectors.checked_mul(SECTOR_SIZE as u64);
    match sector.checked_add(count) {
        Some(end) if end <= capacity => match (bytes(sector), bytes(end)) {
            (Some(start), Some(end)) => Ok(start..end),
            _ => Err(invalid("sector out of range")),
        },
        _ => Err(invalid("sector out of range")),
    }
}

/// A disk held in host memory: for tests, and for scratch disks that vanish with the
/// machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemDisk {
    data: Vec<u8>,
    read_only: bool,
}

impl MemDisk {
    /// A zero-filled disk of `sectors` sectors.
    ///
    /// # Panics
    ///
    /// If `sectors` is more than the device can address ([`MAX_SECTORS`]), or its size
    /// in bytes does not fit in a `usize`: a host's mistake, like asking `vec!` for too
    /// much. [`MemDisk::from_image`] returns an error instead.
    pub fn new(sectors: usize) -> Self {
        assert!(
            sectors as u64 <= MAX_SECTORS,
            "MemDisk of {sectors} sectors: at most {MAX_SECTORS}"
        );
        let size = sectors
            .checked_mul(SECTOR_SIZE)
            .unwrap_or_else(|| panic!("MemDisk of {sectors} sectors does not fit in memory"));
        MemDisk {
            data: vec![0; size],
            read_only: false,
        }
    }

    /// A disk holding `image`, which must be a whole number of sectors.
    pub fn from_image(image: Vec<u8>) -> io::Result<Self> {
        check_image_size(image.len() as u64)?;
        Ok(MemDisk {
            data: image,
            read_only: false,
        })
    }

    pub fn with_read_only(self, read_only: bool) -> Self {
        MemDisk { read_only, ..self }
    }

    /// The disk's contents.
    pub fn data(&self) -> &[u8] {
        &self.data
    }
}

impl BlockBackend for MemDisk {
    fn capacity(&self) -> u64 {
        (self.data.len() / SECTOR_SIZE) as u64
    }

    fn read_only(&self) -> bool {
        self.read_only
    }

    fn read(&mut self, sector: u64, buf: &mut [u8]) -> io::Result<()> {
        let r = byte_range(self.capacity(), sector, buf.len())?;
        buf.copy_from_slice(&self.data[r.start as usize..r.end as usize]);
        Ok(())
    }

    fn write(&mut self, sector: u64, buf: &[u8]) -> io::Result<()> {
        if self.read_only {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, "read-only disk"));
        }
        let r = byte_range(self.capacity(), sector, buf.len())?;
        self.data[r.start as usize..r.end as usize].copy_from_slice(buf);
        Ok(())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// A disk image file on the host: sector `n` is bytes `n * 512 ..` of the file. Writes go
/// straight to the file; `flush` asks the host OS to put them on its own disk.
#[derive(Debug)]
pub struct FileDisk {
    file: File,
    sectors: u64,
    read_only: bool,
}

impl FileDisk {
    /// Opens an existing image, read-write or read-only. Its size must be a whole number
    /// of sectors; it never grows.
    pub fn open(path: impl AsRef<Path>, read_only: bool) -> io::Result<Self> {
        let file = File::options().read(true).write(!read_only).open(path)?;
        let len = file.metadata()?.len();
        check_image_size(len)?;
        Ok(FileDisk {
            file,
            sectors: len / SECTOR_SIZE as u64,
            read_only,
        })
    }
}

impl BlockBackend for FileDisk {
    fn capacity(&self) -> u64 {
        self.sectors
    }

    fn read_only(&self) -> bool {
        self.read_only
    }

    fn read(&mut self, sector: u64, buf: &mut [u8]) -> io::Result<()> {
        let r = byte_range(self.sectors, sector, buf.len())?;
        self.file.seek(SeekFrom::Start(r.start))?;
        self.file.read_exact(buf)
    }

    fn write(&mut self, sector: u64, buf: &[u8]) -> io::Result<()> {
        if self.read_only {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, "read-only disk"));
        }
        let r = byte_range(self.sectors, sector, buf.len())?;
        self.file.seek(SeekFrom::Start(r.start))?;
        self.file.write_all(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        if self.read_only {
            return Ok(());
        }
        self.file.flush()?;
        self.file.sync_data()
    }
}

/// Disk images are whole sectors, and the device's 32-bit `BLK_CAPACITY` limits a disk
/// to 2^32 - 1 sectors (just under 2 TiB).
fn check_image_size(len: u64) -> io::Result<()> {
    let invalid = |msg: String| Err(io::Error::new(io::ErrorKind::InvalidData, msg));
    if !len.is_multiple_of(SECTOR_SIZE as u64) {
        return invalid(format!(
            "disk image size {len} is not a multiple of {SECTOR_SIZE} bytes"
        ));
    }
    if len / SECTOR_SIZE as u64 > MAX_SECTORS {
        return invalid(format!(
            "disk image of {len} bytes is too big (at most {MAX_SECTORS} sectors)"
        ));
    }
    Ok(())
}

/// Largest disk the device can describe: `BLK_CAPACITY` is 32 bits.
pub const MAX_SECTORS: u64 = u32::MAX as u64;

/// One disk: the registers of spec section 9.6 in front of a backend.
pub struct BlockDevice {
    backend: Box<dyn BlockBackend>,
    /// `backend.capacity()`, read once: it fits in 32 bits (checked by [`BlockDevice::new`]).
    capacity: u32,
    read_only: bool,
    pub sector: u32,
    pub dma: u32,
    pub count: u32,
    pub ctrl: u32,
    done: bool,
    error: u32,
}

impl BlockDevice {
    /// Fails if the backend is bigger than the device can address ([`MAX_SECTORS`]).
    pub fn new(backend: Box<dyn BlockBackend>) -> Result<Self, String> {
        let sectors = backend.capacity();
        let capacity = u32::try_from(sectors)
            .map_err(|_| format!("disk of {sectors} sectors is too big (at most {MAX_SECTORS})"))?;
        Ok(BlockDevice {
            read_only: backend.read_only(),
            backend,
            capacity,
            sector: 0,
            dma: 0,
            count: 0,
            ctrl: 0,
            done: false,
            error: error::NONE,
        })
    }

    pub fn backend(&mut self) -> &mut dyn BlockBackend {
        &mut *self.backend
    }

    /// Size in sectors, as `BLK_CAPACITY` reports it.
    pub fn capacity(&self) -> u32 {
        self.capacity
    }

    /// `BLK_STATUS.DONE`.
    pub fn done(&self) -> bool {
        self.done
    }

    /// `BLK_ERROR`.
    pub fn error(&self) -> u32 {
        self.error
    }

    /// Level of the disk's PLIC line: high while `BLK_CTRL.0` and `BLK_STATUS.DONE` are
    /// both set, so acknowledging `DONE` lowers it.
    pub fn irq(&self) -> bool {
        self.ctrl & CTRL_IRQ_ENABLE != 0 && self.done
    }

    /// Registers are 32-bit only. Unknown offsets, and `BLK_CMD`, read as 0.
    pub fn read(&self, off: u32, size: u32) -> Result<u32, BusFault> {
        if size != 4 {
            return Err(BusFault);
        }
        Ok(match off {
            REG_CAPACITY => self.capacity,
            REG_FLAGS => self.read_only as u32 * FLAG_READ_ONLY,
            REG_SECTOR => self.sector,
            REG_DMA => self.dma,
            REG_COUNT => self.count,
            REG_STATUS => self.done as u32 * STATUS_DONE,
            REG_ERROR => self.error,
            REG_CTRL => self.ctrl,
            _ => 0,
        })
    }

    /// Writing `BLK_CMD` runs the command against `ram` (the machine's RAM, mapped at
    /// `RAM_BASE`). Writes to read-only registers and unknown offsets are ignored.
    pub fn write(&mut self, off: u32, size: u32, val: u32, ram: &mut [u8]) -> Result<(), BusFault> {
        if size != 4 {
            return Err(BusFault);
        }
        match off {
            REG_SECTOR => self.sector = val,
            REG_DMA => self.dma = val,
            REG_COUNT => self.count = val,
            REG_CMD => {
                self.error = self.run(val, ram);
                self.done = true;
            }
            REG_STATUS if val & STATUS_DONE != 0 => self.done = false,
            REG_CTRL => self.ctrl = val & CTRL_IRQ_ENABLE,
            _ => {}
        }
        Ok(())
    }

    /// Runs command `cmd` and returns its `BLK_ERROR` code.
    fn run(&mut self, cmd: u32, ram: &mut [u8]) -> u32 {
        if cmd == CMD_FLUSH {
            return match self.backend.flush() {
                Ok(()) => error::NONE,
                Err(_) => error::IO,
            };
        }
        if cmd != CMD_READ && cmd != CMD_WRITE {
            return error::BAD_COMMAND;
        }
        if self.sector as u64 + self.count as u64 > self.capacity as u64 {
            return error::OUT_OF_RANGE;
        }
        // The buffer must lie entirely in RAM. 64-bit arithmetic: no wrap-around.
        let len = self.count as u64 * SECTOR_SIZE as u64;
        let start = (self.dma as u64).wrapping_sub(RAM_BASE as u64);
        if self.dma < RAM_BASE || start + len > ram.len() as u64 {
            return error::BAD_DMA;
        }
        if cmd == CMD_WRITE && self.read_only {
            return error::READ_ONLY;
        }
        if len == 0 {
            return error::NONE;
        }
        let buf = &mut ram[start as usize..(start + len) as usize];
        let sector = self.sector as u64;
        let result = if cmd == CMD_READ {
            self.backend.read(sector, buf)
        } else {
            self.backend.write(sector, buf)
        };
        match result {
            Ok(()) => error::NONE,
            Err(_) => error::IO,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RAM_SIZE: usize = 4 * SECTOR_SIZE;

    /// A 4-sector disk whose sector `n` is filled with byte `n + 1`, and a RAM of 4 sectors.
    fn setup() -> (BlockDevice, Vec<u8>) {
        let image: Vec<u8> = (0..4u8).flat_map(|n| [n + 1; SECTOR_SIZE]).collect();
        let disk = MemDisk::from_image(image).unwrap();
        (BlockDevice::new(Box::new(disk)).unwrap(), vec![0; RAM_SIZE])
    }

    fn w(d: &mut BlockDevice, ram: &mut [u8], off: u32, val: u32) {
        d.write(off, 4, val, ram).unwrap();
    }

    fn r(d: &BlockDevice, off: u32) -> u32 {
        d.read(off, 4).unwrap()
    }

    /// Runs `cmd` on `count` sectors from `sector`, with the buffer at `dma`; returns BLK_ERROR.
    fn cmd(d: &mut BlockDevice, ram: &mut [u8], cmd: u32, sector: u32, count: u32, dma: u32) -> u32 {
        w(d, ram, REG_SECTOR, sector);
        w(d, ram, REG_COUNT, count);
        w(d, ram, REG_DMA, dma);
        w(d, ram, REG_CMD, cmd);
        assert_eq!(r(d, REG_STATUS), STATUS_DONE, "every command completes");
        w(d, ram, REG_STATUS, STATUS_DONE);
        r(d, REG_ERROR)
    }

    /// The disk's sector `n`, read through the backend.
    fn sector(d: &mut BlockDevice, n: u64) -> Vec<u8> {
        let mut buf = vec![0; SECTOR_SIZE];
        d.backend().read(n, &mut buf).unwrap();
        buf
    }

    #[test]
    fn reset_state() {
        let (d, _) = setup();
        assert_eq!(r(&d, REG_CAPACITY), 4);
        assert_eq!(r(&d, REG_FLAGS), 0);
        for off in [
            REG_SECTOR, REG_DMA, REG_COUNT, REG_CMD, REG_STATUS, REG_ERROR, REG_CTRL,
        ] {
            assert_eq!(r(&d, off), 0, "offset {off:#x}");
        }
        assert!(!d.irq());
    }

    #[test]
    fn parameter_registers_hold_their_values() {
        let (mut d, mut ram) = setup();
        w(&mut d, &mut ram, REG_SECTOR, 0xdead_beef);
        w(&mut d, &mut ram, REG_DMA, 0x8000_1234);
        w(&mut d, &mut ram, REG_COUNT, 0xffff_ffff);
        assert_eq!(r(&d, REG_SECTOR), 0xdead_beef);
        assert_eq!(r(&d, REG_DMA), 0x8000_1234);
        assert_eq!(r(&d, REG_COUNT), 0xffff_ffff);
        // Read-only registers and unknown offsets ignore writes.
        w(&mut d, &mut ram, REG_CAPACITY, 99);
        w(&mut d, &mut ram, REG_FLAGS, 1);
        w(&mut d, &mut ram, REG_ERROR, 7);
        w(&mut d, &mut ram, 0x24, 7);
        w(&mut d, &mut ram, 0xffc, 7);
        assert_eq!(r(&d, REG_CAPACITY), 4);
        assert_eq!(r(&d, REG_FLAGS), 0);
        assert_eq!(r(&d, REG_ERROR), 0);
        assert_eq!(r(&d, 0x24), 0);
        assert_eq!(r(&d, 0xffc), 0);
        assert!(!d.done(), "none of that ran a command");
        w(&mut d, &mut ram, REG_CTRL, 0xffff_ffff);
        assert_eq!(r(&d, REG_CTRL), CTRL_IRQ_ENABLE, "reserved bits read 0");
    }

    #[test]
    fn read_copies_sectors_into_ram() {
        let (mut d, mut ram) = setup();
        assert_eq!(cmd(&mut d, &mut ram, CMD_READ, 1, 2, RAM_BASE + 512), error::NONE);
        assert!(ram[..512].iter().all(|&b| b == 0), "before the buffer");
        assert!(ram[512..1024].iter().all(|&b| b == 2));
        assert!(ram[1024..1536].iter().all(|&b| b == 3));
        assert!(ram[1536..].iter().all(|&b| b == 0), "after the buffer");
        // Registers keep their values, ready for the next command.
        assert_eq!([r(&d, REG_SECTOR), r(&d, REG_COUNT)], [1, 2]);
        assert_eq!(r(&d, REG_DMA), RAM_BASE + 512);
    }

    #[test]
    fn write_copies_ram_to_the_disk_and_reads_back() {
        let (mut d, mut ram) = setup();
        for (i, b) in ram.iter_mut().enumerate() {
            *b = (i * 7) as u8;
        }
        let pattern = ram[3..3 + 1024].to_vec();
        // Any byte address in RAM works, aligned or not.
        assert_eq!(cmd(&mut d, &mut ram, CMD_WRITE, 2, 2, RAM_BASE + 3), error::NONE);
        assert_eq!(sector(&mut d, 1), [2; 512], "untouched");
        assert_eq!(sector(&mut d, 2), pattern[..512]);
        assert_eq!(sector(&mut d, 3), pattern[512..]);

        ram.fill(0);
        assert_eq!(
            cmd(&mut d, &mut ram, CMD_READ, 2, 2, RAM_BASE + 1024),
            error::NONE
        );
        assert_eq!(ram[1024..2048], pattern[..]);
        assert_eq!(cmd(&mut d, &mut ram, CMD_FLUSH, 0, 0, 0), error::NONE);
    }

    #[test]
    fn whole_disk_and_whole_ram() {
        let (mut d, mut ram) = setup();
        assert_eq!(cmd(&mut d, &mut ram, CMD_READ, 0, 4, RAM_BASE), error::NONE);
        for (n, chunk) in ram.chunks(SECTOR_SIZE).enumerate() {
            assert!(chunk.iter().all(|&b| b == n as u8 + 1), "sector {n}");
        }
    }

    #[test]
    fn zero_sectors_is_a_successful_no_op() {
        let (mut d, mut ram) = setup();
        for c in [CMD_READ, CMD_WRITE] {
            assert_eq!(
                cmd(&mut d, &mut ram, c, 4, 0, RAM_BASE + RAM_SIZE as u32),
                error::NONE
            );
        }
        assert!(ram.iter().all(|&b| b == 0));
        // The checks still apply.
        assert_eq!(
            cmd(&mut d, &mut ram, CMD_READ, 5, 0, RAM_BASE),
            error::OUT_OF_RANGE
        );
        assert_eq!(cmd(&mut d, &mut ram, CMD_READ, 0, 0, 0), error::BAD_DMA);
        assert_eq!(cmd(&mut d, &mut ram, 7, 0, 0, RAM_BASE), error::BAD_COMMAND);
    }

    #[test]
    fn out_of_range_sectors_are_an_error_not_a_panic() {
        let (mut d, mut ram) = setup();
        let cases = [
            (4, 1),        // first sector past the end
            (3, 2),        // runs off the end
            (0, 5),        // more sectors than the disk
            (u32::MAX, 1), // would wrap in 32 bits
            (1, u32::MAX), // so would this
            (u32::MAX, u32::MAX),
        ];
        for (sector, count) in cases {
            for c in [CMD_READ, CMD_WRITE] {
                let e = cmd(&mut d, &mut ram, c, sector, count, RAM_BASE);
                assert_eq!(e, error::OUT_OF_RANGE, "cmd {c} sector {sector} count {count}");
            }
        }
        assert!(ram.iter().all(|&b| b == 0), "nothing was read");
        assert_eq!(sector(&mut d, 3), [4; 512], "nothing was written");
    }

    #[test]
    fn buffers_outside_ram_are_an_error() {
        let (mut d, mut ram) = setup();
        let end = RAM_BASE + RAM_SIZE as u32;
        let cases = [
            (0, 1),              // not RAM at all
            (RAM_BASE - 1, 1),   // starts just below RAM
            (end - 511, 1),      // runs one byte past the end
            (end, 1),            // starts at the end
            (RAM_BASE + 512, 4), // too long
            (0xffff_ff00, 1),    // would wrap around the address space
        ];
        for (dma, count) in cases {
            for c in [CMD_READ, CMD_WRITE] {
                let e = cmd(&mut d, &mut ram, c, 0, count, dma);
                assert_eq!(e, error::BAD_DMA, "cmd {c} dma {dma:#x} count {count}");
            }
        }
        assert!(ram.iter().all(|&b| b == 0));
        assert_eq!(sector(&mut d, 0), [1; 512]);
        // The last whole sector of RAM is fine.
        assert_eq!(cmd(&mut d, &mut ram, CMD_READ, 0, 1, end - 512), error::NONE);
    }

    #[test]
    fn unknown_commands_are_an_error() {
        let (mut d, mut ram) = setup();
        for c in [0, 4, 0x100 | CMD_READ, u32::MAX] {
            assert_eq!(
                cmd(&mut d, &mut ram, c, 0, 1, RAM_BASE),
                error::BAD_COMMAND,
                "{c:#x}"
            );
        }
        assert!(ram.iter().all(|&b| b == 0));
        // The next good command clears the error.
        assert_eq!(cmd(&mut d, &mut ram, CMD_READ, 0, 1, RAM_BASE), error::NONE);
    }

    #[test]
    fn read_only_disks_refuse_writes() {
        let disk = MemDisk::new(2).with_read_only(true);
        let mut d = BlockDevice::new(Box::new(disk)).unwrap();
        let mut ram = vec![0xaa; RAM_SIZE];
        assert_eq!(r(&d, REG_FLAGS), FLAG_READ_ONLY);
        assert_eq!(cmd(&mut d, &mut ram, CMD_WRITE, 0, 1, RAM_BASE), error::READ_ONLY);
        assert_eq!(sector(&mut d, 0), [0; 512]);
        // Range checks come first; reads and flushes work.
        assert_eq!(
            cmd(&mut d, &mut ram, CMD_WRITE, 2, 1, RAM_BASE),
            error::OUT_OF_RANGE
        );
        assert_eq!(cmd(&mut d, &mut ram, CMD_READ, 1, 1, RAM_BASE), error::NONE);
        assert!(ram[..512].iter().all(|&b| b == 0));
        assert_eq!(cmd(&mut d, &mut ram, CMD_FLUSH, 0, 0, 0), error::NONE);
    }

    /// A backend whose every operation fails, as a broken file or network disk would.
    struct Broken;

    impl BlockBackend for Broken {
        fn capacity(&self) -> u64 {
            8
        }
        fn read(&mut self, _: u64, _: &mut [u8]) -> io::Result<()> {
            Err(io::Error::other("unplugged"))
        }
        fn write(&mut self, _: u64, _: &[u8]) -> io::Result<()> {
            Err(io::Error::other("unplugged"))
        }
        fn flush(&mut self) -> io::Result<()> {
            Err(io::Error::other("unplugged"))
        }
    }

    #[test]
    fn backend_failures_are_io_errors() {
        let mut d = BlockDevice::new(Box::new(Broken)).unwrap();
        let mut ram = vec![0; RAM_SIZE];
        assert_eq!(cmd(&mut d, &mut ram, CMD_READ, 0, 1, RAM_BASE), error::IO);
        assert_eq!(cmd(&mut d, &mut ram, CMD_WRITE, 7, 1, RAM_BASE), error::IO);
        assert_eq!(cmd(&mut d, &mut ram, CMD_FLUSH, 0, 0, 0), error::IO);
        // Checks the device makes itself still come first.
        assert_eq!(
            cmd(&mut d, &mut ram, CMD_READ, 8, 1, RAM_BASE),
            error::OUT_OF_RANGE
        );
        assert_eq!(cmd(&mut d, &mut ram, 9, 0, 1, RAM_BASE), error::BAD_COMMAND);
    }

    /// A backend that claims more sectors than the registers can describe.
    struct Huge;

    impl BlockBackend for Huge {
        fn capacity(&self) -> u64 {
            MAX_SECTORS + 1
        }
        fn read(&mut self, _: u64, _: &mut [u8]) -> io::Result<()> {
            Ok(())
        }
        fn write(&mut self, _: u64, _: &[u8]) -> io::Result<()> {
            Ok(())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn disks_must_fit_32_bit_sector_numbers() {
        let e = BlockDevice::new(Box::new(Huge)).err().unwrap();
        assert!(e.contains("too big"), "{e}");
    }

    #[test]
    fn completion_interrupt_follows_ctrl_and_done() {
        let (mut d, mut ram) = setup();
        w(&mut d, &mut ram, REG_COUNT, 1);
        w(&mut d, &mut ram, REG_DMA, RAM_BASE);
        w(&mut d, &mut ram, REG_CMD, CMD_READ);
        assert!(d.done());
        assert!(!d.irq(), "interrupts disabled");
        w(&mut d, &mut ram, REG_CTRL, CTRL_IRQ_ENABLE);
        assert!(d.irq(), "enabling with DONE set raises the line");
        w(&mut d, &mut ram, REG_STATUS, 0);
        assert!(d.irq(), "writing 0 does not acknowledge");
        w(&mut d, &mut ram, REG_STATUS, STATUS_BUSY);
        assert!(d.irq(), "neither does writing BUSY");
        w(&mut d, &mut ram, REG_STATUS, STATUS_DONE);
        assert!(!d.irq(), "acknowledged");
        assert_eq!(r(&d, REG_STATUS), 0);

        // Failed commands complete (and interrupt) too.
        w(&mut d, &mut ram, REG_SECTOR, 100);
        w(&mut d, &mut ram, REG_CMD, CMD_READ);
        assert!(d.irq());
        assert_eq!(r(&d, REG_ERROR), error::OUT_OF_RANGE);
        w(&mut d, &mut ram, REG_CTRL, 0);
        assert!(!d.irq());
        assert_eq!(
            r(&d, REG_STATUS),
            STATUS_DONE,
            "still done, just not interrupting"
        );
    }

    #[test]
    fn registers_are_word_only() {
        let (mut d, mut ram) = setup();
        w(&mut d, &mut ram, REG_COUNT, 1);
        w(&mut d, &mut ram, REG_DMA, RAM_BASE);
        for size in [1, 2] {
            assert_eq!(d.read(REG_CAPACITY, size), Err(BusFault));
            assert_eq!(d.write(REG_CMD, size, CMD_READ, &mut ram), Err(BusFault));
            assert_eq!(d.write(REG_SECTOR, size, 3, &mut ram), Err(BusFault));
        }
        assert!(!d.done(), "a faulting write runs nothing");
        assert_eq!(r(&d, REG_SECTOR), 0);
        assert!(ram.iter().all(|&b| b == 0));
    }

    #[test]
    fn mem_disk_backend() {
        let mut disk = MemDisk::new(3);
        assert_eq!(disk.capacity(), 3);
        assert!(!disk.read_only());
        disk.write(1, &[7; 1024]).unwrap();
        let mut buf = [0; 512];
        disk.read(2, &mut buf).unwrap();
        assert_eq!(buf, [7; 512]);
        assert_eq!(&disk.data()[..512], &[0; 512]);
        // Bad requests are errors, never panics.
        assert!(disk.read(3, &mut buf).is_err());
        assert!(disk.read(2, &mut [0; 1024]).is_err());
        assert!(disk.read(0, &mut [0; 100]).is_err());
        assert!(disk.write(u64::MAX, &buf).is_err());
        assert!(MemDisk::from_image(vec![0; 513]).is_err());
        assert_eq!(MemDisk::from_image(Vec::new()).unwrap().capacity(), 0);
        let mut ro = MemDisk::new(1).with_read_only(true);
        assert_eq!(
            ro.write(0, &buf).unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
    }

    #[test]
    fn byte_ranges() {
        assert_eq!(byte_range(4, 0, 512).unwrap(), 0..512);
        assert_eq!(byte_range(4, 3, 512).unwrap(), 1536..2048);
        assert_eq!(byte_range(4, 4, 0).unwrap(), 2048..2048);
        assert!(byte_range(4, 4, 512).is_err());
        assert!(byte_range(4, 0, 511).is_err());
        assert!(byte_range(u64::MAX, u64::MAX, 512).is_err());
        // In range, but the byte offsets would overflow: an error, not a panic or a
        // wrapped range.
        let huge = u64::MAX / SECTOR_SIZE as u64;
        let err = byte_range(u64::MAX, huge, 512).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        assert!(byte_range(u64::MAX, huge + 1, 0).is_err());
        assert_eq!(
            byte_range(u64::MAX, huge - 1, 512).unwrap(),
            (huge - 1) * 512..huge * 512
        );
    }

    #[test]
    #[should_panic(expected = "at most 4294967295")]
    fn a_mem_disk_bigger_than_the_device_can_address_panics() {
        MemDisk::new(MAX_SECTORS as usize + 1);
    }

    #[test]
    fn backends_are_object_safe_and_send() {
        fn assert_send<T: Send + ?Sized>() {}
        assert_send::<dyn BlockBackend>();
        assert_send::<BlockDevice>();
        let disks: Vec<Box<dyn BlockBackend>> = vec![Box::new(MemDisk::new(1)), Box::new(Broken)];
        assert_eq!(disks.iter().map(|d| d.capacity()).sum::<u64>(), 9);
    }
}
