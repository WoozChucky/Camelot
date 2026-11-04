//! Booting from a disk (spec section 7, "Boot from disk"): a machine started with no
//! program runs the boot ROM's disk loader, which reads the boot block of disk 0 (its
//! first two sectors, 1 KiB) into the last 64 KiB of RAM, checks its signature and
//! checksum, and jumps to it, like a PC's BIOS loading the master boot record.
//!
//! The loader is real code in the ROM, run by hart 0 like any program: its source,
//! `rom/diskboot.S`, is preprocessed for each width (merlin -E) and built with
//! merlin-as and merlin-ld into the committed `rom/diskboot.elf` for an RV32 machine
//! and `rom/diskboot-rv64.elf` for an RV64 one (`cargo xtask boot`), which Grail
//! includes here. What the
//! boot block does next is up to whoever wrote it; Camelot's (`kernel/boot/boot.s`)
//! loads the kernel ELF that `camelot-fs mkfs` installed after the filesystem.

use crate::bus::BOOT_ROM_BASE;
use excalibur::Xlen;

/// Where the disk loader starts in the boot ROM, after the stub (0x1000) and the park
/// loop (0x1020). The stub jumps here through the entry point when the machine boots
/// from disk.
pub const DISK_BOOT_PC: u64 = BOOT_ROM_BASE + 0x100;

/// The ROM's entry point (0x1010): where the stub and the park loop jump, a doubleword
/// (RV32: the word). It is the ROM's one writable location (8-byte stores; RV32: 4-byte
/// ones), so that a bootloader can point harts woken later at the kernel it started.
pub const BOOT_ENTRY_ADDR: u64 = BOOT_ROM_BASE + 0x10;

/// The boot block: sectors 0 and 1 of disk 0.
pub const BOOT_BLOCK_SIZE: usize = 1024;

/// The boot block's last word must be this ("BOOT" in ASCII, little-endian).
pub const BOOT_MAGIC: u32 = 0x544f_4f42;

/// The boot block is loaded at the start of the last `BOOT_AREA_SIZE` bytes of RAM, and
/// the whole area is the boot block's to use (for a stack, buffers).
pub const BOOT_AREA_SIZE: u32 = 0x1_0000;

/// The exit code of a machine whose boot ROM could not boot it (no boot block, a
/// read error, a bad checksum, a trap before the boot block set its own `mtvec`).
pub const EXIT_BOOT_FAILED: u32 = 0xfd;

/// The disk loader of RV32 machines, built from `rom/diskboot.S` (`cargo xtask boot`).
pub const DISKBOOT_ELF: &[u8] = include_bytes!("../rom/diskboot.elf");

/// The disk loader of RV64 machines, the RV64 build of `rom/diskboot.S` (`cargo xtask
/// boot`).
pub const DISKBOOT_RV64_ELF: &[u8] = include_bytes!("../rom/diskboot-rv64.elf");

/// The disk loader of an `xlen` machine: [`DISKBOOT_ELF`] or [`DISKBOOT_RV64_ELF`].
pub fn disk_loader_elf(xlen: Xlen) -> &'static [u8] {
    match xlen {
        Xlen::Rv32 => DISKBOOT_ELF,
        Xlen::Rv64 => DISKBOOT_RV64_ELF,
    }
}

/// The disk loader's code for an `xlen` machine: the bytes of [`disk_loader_elf`],
/// which go at [`DISK_BOOT_PC`].
pub fn disk_loader(xlen: Xlen) -> Vec<u8> {
    let elf = crate::elf::parse(disk_loader_elf(xlen)).expect("the disk loader is an ELF file");
    assert_eq!(elf.xlen, xlen, "the disk loader is built for the machine's width");
    let mut code = Vec::new();
    for seg in &elf.segments {
        // The ELF header and the program headers of an ELF32 file: they fit
        // below the loader in its page, so the linker loads them at the page's
        // start, 0x1000, in a segment of their own (ld.lld's rule, which
        // merlin-ld follows; an ELF64 file's are too big for the 0x100 bytes).
        // That is where the ROM has its stub; they are not the loader's.
        if seg.paddr < DISK_BOOT_PC {
            assert!(
                seg.data.starts_with(b"\x7fELF"),
                "only the headers go below the loader"
            );
            continue;
        }
        let at = (seg.paddr - DISK_BOOT_PC) as usize;
        code.resize(code.len().max(at + seg.mem_size as usize), 0);
        code[at..at + seg.data.len()].copy_from_slice(&seg.data);
    }
    code
}

/// Where the boot block goes on a machine with `ram_size` bytes of RAM: `None` if the
/// RAM is smaller than the boot area, which [`crate::Machine::boot_from_disk`] refuses.
pub fn boot_block_addr(ram_size: usize) -> Option<u64> {
    let ram = ram_size as u64;
    Some(crate::bus::RAM_BASE + ram.checked_sub(BOOT_AREA_SIZE.into())?)
}

/// Whether the boot ROM accepts `block` (at least [`BOOT_BLOCK_SIZE`] bytes; the rest
/// is ignored): its last word is [`BOOT_MAGIC`] and its words add up to 0.
pub fn is_bootable(block: &[u8]) -> bool {
    let Some(block) = block.get(..BOOT_BLOCK_SIZE) else {
        return false;
    };
    word(block, BOOT_BLOCK_SIZE - 4) == BOOT_MAGIC && checksum(block) == 0
}

/// Makes `block` bootable: writes [`BOOT_MAGIC`] in its last word and sets the word
/// before it so that all of them add up to 0, as Camelot's mkfs does. For tests and
/// hosts that write boot blocks of their own.
pub fn seal(block: &mut [u8; BOOT_BLOCK_SIZE]) {
    block[BOOT_BLOCK_SIZE - 4..].copy_from_slice(&BOOT_MAGIC.to_le_bytes());
    block[BOOT_BLOCK_SIZE - 8..BOOT_BLOCK_SIZE - 4].fill(0);
    let fix = 0u32.wrapping_sub(checksum(block));
    block[BOOT_BLOCK_SIZE - 8..BOOT_BLOCK_SIZE - 4].copy_from_slice(&fix.to_le_bytes());
}

/// The wrapping sum of the little-endian words of `block`.
fn checksum(block: &[u8]) -> u32 {
    (0..block.len() / 4).fold(0u32, |sum, i| sum.wrapping_add(word(block, 4 * i)))
}

fn word(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_loader_fits_the_rom() {
        for xlen in [Xlen::Rv32, Xlen::Rv64] {
            let code = disk_loader(xlen);
            assert!(code.len() > 100);
            assert!(DISK_BOOT_PC as usize - BOOT_ROM_BASE as usize + code.len() <= 0x1000);
            let elf = crate::elf::parse(disk_loader_elf(xlen)).unwrap();
            assert_eq!(elf.xlen, xlen);
            assert_eq!(elf.entry, DISK_BOOT_PC, "the loader starts at its first byte");
            // The headers' segment, at the start of the ROM, is left out. The
            // ELF64 headers don't fit below the loader, so they are not loaded.
            let below: Vec<u64> = elf
                .segments
                .iter()
                .map(|s| s.paddr)
                .filter(|&a| a < DISK_BOOT_PC)
                .collect();
            let headers: &[u64] = match xlen {
                Xlen::Rv32 => &[BOOT_ROM_BASE],
                Xlen::Rv64 => &[],
            };
            assert_eq!(below, headers, "{xlen:?}");
            assert_eq!(&code[..4], &0xf140_22f3u32.to_le_bytes(), "csrr t0, mhartid");
        }
        assert_ne!(disk_loader(Xlen::Rv32), disk_loader(Xlen::Rv64));
    }

    #[test]
    fn sealing_and_checking() {
        let mut block = [0u8; BOOT_BLOCK_SIZE];
        assert!(!is_bootable(&block), "an empty block has no signature");
        block[..4].copy_from_slice(&0x1234_5678u32.to_le_bytes());
        seal(&mut block);
        assert!(is_bootable(&block));
        assert!(
            is_bootable(&[&block[..], &[1, 2, 3]].concat()),
            "what follows is ignored"
        );
        assert!(!is_bootable(&block[..1000]));
        block[7] ^= 0x40;
        assert!(!is_bootable(&block), "one bit off");
        assert_eq!(boot_block_addr(128 << 20), Some(0x87ff_0000));
        assert_eq!(boot_block_addr(1 << 20), Some(0x800f_0000));
        assert_eq!(boot_block_addr(64 << 10), Some(0x8000_0000), "all of RAM");
        assert_eq!(boot_block_addr(60 << 10), None);
    }
}
