//! Booting from a disk (spec section 7, "Boot from disk"): a machine started with no
//! program runs the boot ROM's disk loader, which reads the boot block of disk 0 (its
//! first two sectors, 1 KiB) into the last 64 KiB of RAM, checks its signature and
//! checksum, and jumps to it, like a PC's BIOS loading the master boot record.
//!
//! The loader is real code in the ROM, run by hart 0 like any program: its source is
//! `rom/diskboot.s`, built with merlin-as and merlin-ld into the committed
//! `rom/diskboot.elf` (`cargo xtask boot`), which Grail includes here. What the boot
//! block does next is up to whoever wrote it; Camelot's (`kernel/boot/boot.s`) loads
//! the kernel ELF that `camelot-fs mkfs` installed after the filesystem.

use crate::bus::BOOT_ROM_BASE;

/// Where the disk loader starts in the boot ROM, after the stub (0x1000) and the park
/// loop (0x1020). The stub jumps here through the entry word when the machine boots
/// from disk.
pub const DISK_BOOT_PC: u32 = BOOT_ROM_BASE + 0x100;

/// The ROM's entry word (0x1010): where the stub and the park loop jump. It is the
/// ROM's one writable word (32-bit stores), so that a bootloader can point harts woken
/// later at the kernel it started.
pub const BOOT_ENTRY_ADDR: u32 = BOOT_ROM_BASE + 0x10;

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

/// The disk loader, built from `rom/diskboot.s` (`cargo xtask boot`).
pub const DISKBOOT_ELF: &[u8] = include_bytes!("../rom/diskboot.elf");

/// The disk loader's code: the bytes of [`DISKBOOT_ELF`], which go at
/// [`DISK_BOOT_PC`].
pub fn disk_loader() -> Vec<u8> {
    let elf = crate::elf::parse(DISKBOOT_ELF).expect("rom/diskboot.elf is an ELF file");
    let mut code = Vec::new();
    for seg in &elf.segments {
        let at = (seg.paddr - u64::from(DISK_BOOT_PC)) as usize;
        code.resize(code.len().max(at + seg.mem_size as usize), 0);
        code[at..at + seg.data.len()].copy_from_slice(&seg.data);
    }
    code
}

/// Where the boot block goes on a machine with `ram_size` bytes of RAM: `None` if the
/// RAM is smaller than the boot area, which [`crate::Machine::boot_from_disk`] refuses.
pub fn boot_block_addr(ram_size: usize) -> Option<u32> {
    let ram = u32::try_from(ram_size).ok()?;
    Some(crate::bus::RAM_BASE + ram.checked_sub(BOOT_AREA_SIZE)?)
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
        let code = disk_loader();
        assert!(code.len() > 100);
        assert!(DISK_BOOT_PC as usize - BOOT_ROM_BASE as usize + code.len() <= 0x1000);
        let elf = crate::elf::parse(DISKBOOT_ELF).unwrap();
        assert_eq!(
            elf.entry,
            u64::from(DISK_BOOT_PC),
            "the loader starts at its first byte"
        );
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
