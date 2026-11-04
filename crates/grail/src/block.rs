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
//!
//! Two register layouts: **version 2**, which RV64 machines present, with 64-bit
//! `BLK_CAPACITY`, `BLK_SECTOR` and `BLK_DMA` ([`v2`]); and **version 1**, all 32-bit,
//! which RV32 machines keep (the constants at the top of this module).

use crate::bus::{BusFault, RAM_BASE};
use crate::mmio::{self, Field};
use excalibur::Xlen;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;

/// Bytes per sector.
pub const SECTOR_SIZE: usize = 512;

// Version 1's register offsets from the start of a disk's page, all 32-bit.
pub const REG_CAPACITY: u32 = 0x00;
pub const REG_FLAGS: u32 = 0x04;
pub const REG_SECTOR: u32 = 0x08;
pub const REG_DMA: u32 = 0x0c;
pub const REG_COUNT: u32 = 0x10;
pub const REG_CMD: u32 = 0x14;
pub const REG_STATUS: u32 = 0x18;
pub const REG_ERROR: u32 = 0x1c;
pub const REG_CTRL: u32 = 0x20;

/// Version 2's register offsets (RV64 machines, spec section 9.6).
pub mod v2 {
    /// 64-bit.
    pub const REG_CAPACITY: u32 = 0x00;
    /// 64-bit.
    pub const REG_SECTOR: u32 = 0x08;
    /// 64-bit.
    pub const REG_DMA: u32 = 0x10;
    pub const REG_COUNT: u32 = 0x18;
    pub const REG_FLAGS: u32 = 0x1c;
    pub const REG_CMD: u32 = 0x20;
    pub const REG_STATUS: u32 = 0x24;
    pub const REG_ERROR: u32 = 0x28;
    pub const REG_CTRL: u32 = 0x2c;
}

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

/// Disk images are whole sectors, at most [`MAX_SECTORS`] of them.
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

/// Largest disk: the most sectors whose bytes a 64-bit offset reaches. Version 2's
/// `BLK_CAPACITY` is 64 bits, so the limit is the backend's, never the register's.
/// Version 1's is 32 bits: a bigger disk shows [`V1_MAX_SECTORS`] there.
pub const MAX_SECTORS: u64 = u64::MAX / SECTOR_SIZE as u64;

/// The most sectors version 1's 32-bit `BLK_CAPACITY` shows (RV32 machines), just under
/// 2 TiB: what a 32-bit `BLK_SECTOR` reaches.
pub const V1_MAX_SECTORS: u64 = u32::MAX as u64;

/// The registers' contents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reg {
    Capacity,
    Flags,
    Sector,
    Dma,
    Count,
    Cmd,
    Status,
    Error,
    Ctrl,
}

/// One disk: the registers of spec section 9.6 in front of a backend.
pub struct BlockDevice {
    backend: Box<dyn BlockBackend>,
    /// `backend.capacity()`, read once: at most [`MAX_SECTORS`] (checked by
    /// [`BlockDevice::new`]).
    capacity: u64,
    read_only: bool,
    pub sector: u64,
    pub dma: u64,
    pub count: u32,
    pub ctrl: u32,
    done: bool,
    error: u32,
    /// Which layout the registers have: version 2 on RV64, 1 on RV32.
    xlen: Xlen,
}

impl BlockDevice {
    /// Fails if the backend is bigger than the device can address ([`MAX_SECTORS`]).
    /// The registers are version 1's until [`BlockDevice::set_xlen`] says otherwise.
    pub fn new(backend: Box<dyn BlockBackend>) -> Result<Self, String> {
        let capacity = backend.capacity();
        if capacity > MAX_SECTORS {
            return Err(format!(
                "disk of {capacity} sectors is too big (at most {MAX_SECTORS})"
            ));
        }
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
            xlen: Xlen::Rv32,
        })
    }

    /// Presents the registers of an `xlen` machine's disks (spec section 9.6). The
    /// registers keep their values.
    pub fn set_xlen(&mut self, xlen: Xlen) {
        self.xlen = xlen;
    }

    pub fn backend(&mut self) -> &mut dyn BlockBackend {
        &mut *self.backend
    }

    /// Size in sectors (version 1's `BLK_CAPACITY` shows at most 2^32 - 1 of them).
    pub fn capacity(&self) -> u64 {
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

    /// What `off` holds in this disk's layout.
    fn field(&self, off: u32) -> Field<Reg> {
        use Field::{High, Low, Unused, Word};
        match self.xlen {
            Xlen::Rv32 => match off {
                REG_CAPACITY => Word(Reg::Capacity),
                REG_FLAGS => Word(Reg::Flags),
                REG_SECTOR => Word(Reg::Sector),
                REG_DMA => Word(Reg::Dma),
                REG_COUNT => Word(Reg::Count),
                REG_CMD => Word(Reg::Cmd),
                REG_STATUS => Word(Reg::Status),
                REG_ERROR => Word(Reg::Error),
                REG_CTRL => Word(Reg::Ctrl),
                _ => Unused,
            },
            Xlen::Rv64 => match off {
                v2::REG_CAPACITY => Low(Reg::Capacity),
                0x04 => High(Reg::Capacity),
                v2::REG_SECTOR => Low(Reg::Sector),
                0x0c => High(Reg::Sector),
                v2::REG_DMA => Low(Reg::Dma),
                0x14 => High(Reg::Dma),
                v2::REG_COUNT => Word(Reg::Count),
                v2::REG_FLAGS => Word(Reg::Flags),
                v2::REG_CMD => Word(Reg::Cmd),
                v2::REG_STATUS => Word(Reg::Status),
                v2::REG_ERROR => Word(Reg::Error),
                v2::REG_CTRL => Word(Reg::Ctrl),
                _ => Unused,
            },
        }
    }

    /// Whether `off` is `BLK_CMD`, whose writes run commands.
    pub fn is_command(&self, off: u32) -> bool {
        self.field(off) == Field::Word(Reg::Cmd)
    }

    fn value(&self, reg: Reg) -> u64 {
        match reg {
            Reg::Capacity => match self.xlen {
                Xlen::Rv32 => self.capacity.min(V1_MAX_SECTORS),
                Xlen::Rv64 => self.capacity,
            },
            Reg::Flags => (self.read_only as u32 * FLAG_READ_ONLY).into(),
            Reg::Sector => self.sector,
            Reg::Dma => self.dma,
            Reg::Count => self.count.into(),
            Reg::Cmd => 0,
            Reg::Status => (self.done as u32 * STATUS_DONE).into(),
            Reg::Error => self.error.into(),
            Reg::Ctrl => self.ctrl.into(),
        }
    }

    /// Reads a register, at the widths of spec section 9 ([`mmio::access`]). Unknown
    /// offsets, and `BLK_CMD`, read as 0.
    pub fn read(&self, off: u32, size: u32) -> Result<u64, BusFault> {
        let access = mmio::access(off, size, self.xlen == Xlen::Rv64, |o| self.field(o))?;
        Ok(access.read(|reg| self.value(reg)))
    }

    /// Writing `BLK_CMD` runs the command against `ram` (the machine's RAM, mapped at
    /// `RAM_BASE`). Writes to read-only registers and unknown offsets are ignored, and
    /// a write of one half of a 64-bit register keeps the other half.
    pub fn write(&mut self, off: u32, size: u32, val: u64, ram: &mut [u8]) -> Result<(), BusFault> {
        let access = mmio::access(off, size, self.xlen == Xlen::Rv64, |o| self.field(o))?;
        let Some(reg) = access.reg() else {
            return Ok(());
        };
        let word = val as u32;
        match reg {
            Reg::Sector => self.sector = access.merge(self.sector, val),
            Reg::Dma => self.dma = access.merge(self.dma, val),
            Reg::Count => self.count = word,
            Reg::Cmd => {
                self.error = self.run(word, ram);
                self.done = true;
            }
            Reg::Status if word & STATUS_DONE != 0 => self.done = false,
            Reg::Ctrl => self.ctrl = word & CTRL_IRQ_ENABLE,
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
        // No wrap-around: a sum past 2^64 is out of range too.
        let end = self.sector.checked_add(self.count.into());
        if end.is_none_or(|end| end > self.capacity) {
            return error::OUT_OF_RANGE;
        }
        // The buffer must lie entirely in RAM, which starts at RAM_BASE.
        let len = u64::from(self.count) * SECTOR_SIZE as u64;
        let start = self.dma.wrapping_sub(RAM_BASE);
        if self.dma < RAM_BASE || start.checked_add(len).is_none_or(|end| end > ram.len() as u64) {
            return error::BAD_DMA;
        }
        if cmd == CMD_WRITE && self.read_only {
            return error::READ_ONLY;
        }
        if len == 0 {
            return error::NONE;
        }
        let buf = &mut ram[start as usize..(start + len) as usize];
        // The backend may hand the buffer to the host's kernel (a file's ReadFile or
        // read), which cannot take a first touch of lazily backed RAM itself.
        crate::ram::touch(buf);
        let result = if cmd == CMD_READ {
            self.backend.read(self.sector, buf)
        } else {
            self.backend.write(self.sector, buf)
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

    /// RAM's base as the DMA register holds it: a 32-bit physical address.
    const RAM_BASE: u32 = crate::bus::RAM_BASE as u32;
    const RAM_SIZE: usize = 4 * SECTOR_SIZE;

    /// A 4-sector disk whose sector `n` is filled with byte `n + 1`, and a RAM of 4 sectors.
    fn setup() -> (BlockDevice, Vec<u8>) {
        let image: Vec<u8> = (0..4u8).flat_map(|n| [n + 1; SECTOR_SIZE]).collect();
        let disk = MemDisk::from_image(image).unwrap();
        (BlockDevice::new(Box::new(disk)).unwrap(), vec![0; RAM_SIZE])
    }

    fn w(d: &mut BlockDevice, ram: &mut [u8], off: u32, val: u32) {
        d.write(off, 4, val.into(), ram).unwrap();
    }

    fn r(d: &BlockDevice, off: u32) -> u32 {
        d.read(off, 4).unwrap() as u32
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

    /// A backend that claims this many sectors.
    struct Huge(u64);

    impl BlockBackend for Huge {
        fn capacity(&self) -> u64 {
            self.0
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
    fn disks_must_fit_64_bit_byte_offsets() {
        let e = BlockDevice::new(Box::new(Huge(MAX_SECTORS + 1))).err().unwrap();
        assert!(e.contains("too big"), "{e}");
        assert!(BlockDevice::new(Box::new(Huge(MAX_SECTORS))).is_ok());
    }

    /// Version 1's 32-bit `BLK_CAPACITY` shows a disk of 2^32 sectors or more as the
    /// 2^32 - 1 its `BLK_SECTOR` reaches; version 2 shows all of it.
    #[test]
    fn a_big_disk_is_whole_only_in_version_2() {
        let mut d = BlockDevice::new(Box::new(Huge(1 << 33))).unwrap();
        assert_eq!(d.read(REG_CAPACITY, 4), Ok(u32::MAX.into()));
        d.set_xlen(Xlen::Rv64);
        assert_eq!(d.read(v2::REG_CAPACITY, 8), Ok(1 << 33));
        assert_eq!(d.capacity(), 1 << 33);
    }

    /// An RV64 machine's disk: 64-bit CAPACITY, SECTOR and DMA, whole or by halves.
    #[test]
    fn version_2_registers() {
        let (mut d, mut ram) = setup();
        d.set_xlen(Xlen::Rv64);
        assert_eq!(d.read(v2::REG_CAPACITY, 8), Ok(4));
        assert_eq!(d.read(v2::REG_CAPACITY, 4), Ok(4));
        assert_eq!(d.read(v2::REG_CAPACITY + 4, 4), Ok(0));
        assert_eq!(d.read(v2::REG_FLAGS, 4), Ok(0));
        // The halves of a 64-bit register, each keeping the other.
        d.write(v2::REG_SECTOR, 8, 0x1_0000_0002, &mut ram).unwrap();
        assert_eq!(d.sector, 0x1_0000_0002);
        d.write(v2::REG_SECTOR, 4, 3, &mut ram).unwrap();
        assert_eq!(d.read(v2::REG_SECTOR, 8), Ok(0x1_0000_0003));
        d.write(v2::REG_SECTOR + 4, 4, 0, &mut ram).unwrap();
        assert_eq!(d.read(v2::REG_SECTOR, 8), Ok(3));
        assert_eq!(d.read(v2::REG_SECTOR + 4, 4), Ok(0));
        // A command with 64-bit DMA.
        d.write(v2::REG_DMA, 8, crate::bus::RAM_BASE + 512, &mut ram)
            .unwrap();
        d.write(v2::REG_COUNT, 4, 1, &mut ram).unwrap();
        d.write(v2::REG_CTRL, 4, 0xff, &mut ram).unwrap();
        d.write(v2::REG_CMD, 4, CMD_READ.into(), &mut ram).unwrap();
        assert_eq!(d.read(v2::REG_STATUS, 4), Ok(STATUS_DONE.into()));
        assert_eq!(d.read(v2::REG_ERROR, 4), Ok(0));
        assert!(d.irq(), "BLK_CTRL is at +0x2C");
        assert_eq!(d.read(v2::REG_CTRL, 4), Ok(CTRL_IRQ_ENABLE.into()));
        assert!(ram[512..1024].iter().all(|&b| b == 4), "sector 3");
        assert!(d.is_command(v2::REG_CMD) && !d.is_command(REG_CMD));
        d.write(v2::REG_STATUS, 4, STATUS_DONE.into(), &mut ram).unwrap();
        assert!(!d.done());
        // 32-bit registers take no 8-byte access; a faulting write runs nothing.
        assert_eq!(d.read(v2::REG_COUNT, 8), Err(BusFault));
        assert_eq!(d.write(v2::REG_CMD, 8, CMD_READ.into(), &mut ram), Err(BusFault));
        assert_eq!(d.read(v2::REG_SECTOR + 4, 8), Err(BusFault), "misaligned");
        assert!(!d.done());
        assert_eq!(d.read(0x30, 8), Ok(0), "unused");
    }

    /// Huge 64-bit sectors and buffers are errors, never a wrap-around or a panic.
    #[test]
    fn version_2_checks_do_not_wrap() {
        let (mut d, mut ram) = setup();
        d.set_xlen(Xlen::Rv64);
        let mut run = |sector: u64, count: u32, dma: u64| {
            d.write(v2::REG_SECTOR, 8, sector, &mut ram).unwrap();
            d.write(v2::REG_COUNT, 4, count.into(), &mut ram).unwrap();
            d.write(v2::REG_DMA, 8, dma, &mut ram).unwrap();
            d.write(v2::REG_CMD, 4, CMD_READ.into(), &mut ram).unwrap();
            d.read(v2::REG_ERROR, 4).unwrap() as u32
        };
        let base = crate::bus::RAM_BASE;
        assert_eq!(run(1 << 32, 1, base), error::OUT_OF_RANGE);
        assert_eq!(run(u64::MAX, 1, base), error::OUT_OF_RANGE);
        assert_eq!(run(1, u32::MAX, base), error::OUT_OF_RANGE);
        assert_eq!(run(0, 1, base + (1 << 32)), error::BAD_DMA);
        assert_eq!(run(0, 1, u64::MAX), error::BAD_DMA);
        assert_eq!(run(0, 1, u64::MAX - 511), error::BAD_DMA);
        assert_eq!(
            run(0, 0, base + RAM_SIZE as u64),
            error::NONE,
            "just past the end"
        );
        assert!(ram.iter().all(|&b| b == 0));
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
        // Version 1 takes no 8-byte access either.
        for size in [1, 2, 8] {
            assert_eq!(d.read(REG_CAPACITY, size), Err(BusFault));
            assert_eq!(d.write(REG_CMD, size, CMD_READ.into(), &mut ram), Err(BusFault));
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
    #[should_panic(expected = "at most 36028797018963967")]
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
