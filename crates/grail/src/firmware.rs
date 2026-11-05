//! Firmware (spec section 7, "Firmware"; `--bios`, QEMU's `-bios`): the M-mode
//! program a machine runs before its program, which then runs in S-mode, as QEMU's
//! `virt` machine runs OpenSBI before Linux.
//!
//! The layout is OpenSBI's `fw_jump` on `virt`: the firmware has the first 2 MiB of
//! RAM, [`FW_BASE`] to [`FW_JUMP_ADDR`], and the next stage, the program (a kernel
//! linked to load there) or a disk's boot block, starts at [`FW_JUMP_ADDR`]. Grail
//! makes a device tree for the machine ([`crate::devicetree`]) and puts it near the top
//! of RAM where QEMU would ([`fdt_addr`]); the boot ROM sends every hart to the
//! firmware's entry with what QEMU's reset vector hands OpenSBI ([`Handoff`]):
//!
//! - a0 = the hart's id;
//! - a1 = the device tree's physical address;
//! - a2 = a `struct fw_dynamic_info` in the ROM (OpenSBI's `include/sbi/fw_dynamic.h`),
//!   which says where the next stage starts and in which mode, 0 for "no program" as
//!   QEMU says it when it has no kernel. A `fw_jump` firmware ignores it.
//!
//! Camelot's own firmware is `firmware/` in the repository, built by `cargo xtask boot`
//! into the committed `rom/firmware.elf`, which Grail includes ([`FIRMWARE_ELF`]). It is
//! RV64 only: RV32 machines boot without firmware until #159.

use crate::bus::RAM_BASE;
use crate::config::Bios;
use crate::elf;
use excalibur::Xlen;

/// Where the firmware is: the start of RAM.
pub const FW_BASE: u64 = RAM_BASE;
/// The RAM the firmware has, from [`FW_BASE`]: 2 MiB, as OpenSBI's `fw_jump` keeps on
/// QEMU `virt`. The device tree's `/reserved-memory` and sysinfo's `SYS_RESERVED_BASE`
/// and `SYS_RESERVED_SIZE` give it to the next stage, which must leave it alone.
pub const FW_SIZE: u64 = 0x20_0000;
/// Where the next stage loads and starts: OpenSBI's `FW_JUMP_ADDR` on `virt`, the
/// RV64 Camelot kernel's physical load address, and where the firmware puts a disk's
/// boot block.
pub const FW_JUMP_ADDR: u64 = FW_BASE + FW_SIZE;

/// Camelot's firmware, built from `firmware/` (`cargo xtask boot`).
pub const FIRMWARE_ELF: &[u8] = include_bytes!("../rom/firmware.elf");

/// `fw_dynamic_info.magic`: "OSBI".
pub const FW_DYNAMIC_INFO_MAGIC: u64 = 0x4942_534f;
/// The `fw_dynamic_info` version the ROM gives: 2, the one with `boot_hart`.
pub const FW_DYNAMIC_INFO_VERSION: u64 = 2;
/// `fw_dynamic_info.next_mode`: the next stage runs in S-mode.
pub const FW_DYNAMIC_INFO_NEXT_MODE_S: u64 = 1;

/// A firmware image, ready to load: its bytes at their physical addresses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    /// (physical address, bytes, size in memory): the rest of each is zeros.
    pub segments: Vec<(u64, Vec<u8>, u64)>,
    /// Where every hart enters it.
    pub entry: u64,
}

impl Image {
    /// The firmware `bios` names for an `xlen` machine: `None` for [`Bios::None`]. An
    /// error if the file cannot be read, is an ELF file of the other width or is not an
    /// executable, or does not fit in the firmware's 2 MiB.
    pub fn of(bios: &Bios, xlen: Xlen) -> Result<Option<Image>, String> {
        let image = match bios {
            Bios::None => return Ok(None),
            Bios::Default => Image::from_elf(FIRMWARE_ELF, xlen)?,
            Bios::File(path) => {
                let file = std::fs::read(path)
                    .map_err(|e| format!("cannot read the firmware {}: {e}", path.display()))?;
                let image = if file.starts_with(b"\x7fELF") {
                    Image::from_elf(&file, xlen)
                } else {
                    Image::raw(file)
                };
                image.map_err(|e| format!("the firmware {}: {e}", path.display()))?
            }
        };
        Ok(Some(image))
    }

    /// A firmware ELF executable: its `PT_LOAD` segments at their physical addresses,
    /// entered at the physical address of its entry point.
    pub fn from_elf(file: &[u8], xlen: Xlen) -> Result<Image, String> {
        let elf = elf::parse(file).map_err(|e| e.to_string())?;
        if elf.xlen != xlen {
            return Err(format!(
                "an {:?} firmware cannot run on an {xlen:?} machine",
                elf.xlen
            ));
        }
        let entry = elf.physical_entry().map_err(|e| e.to_string())?;
        let segments = elf
            .segments
            .iter()
            .map(|s| (s.paddr, s.data.clone(), s.mem_size))
            .collect();
        Image::checked(Image { segments, entry })
    }

    /// A raw binary, loaded at [`FW_BASE`] and entered at its first byte, as QEMU loads
    /// a `-bios` file that is no ELF (OpenSBI's `fw_jump.bin`).
    pub fn raw(bytes: Vec<u8>) -> Result<Image, String> {
        let len = bytes.len() as u64;
        Image::checked(Image {
            segments: vec![(FW_BASE, bytes, len)],
            entry: FW_BASE,
        })
    }

    /// The image if every byte of it, and its entry, is in the firmware's 2 MiB.
    fn checked(image: Image) -> Result<Image, String> {
        for &(at, _, size) in &image.segments {
            if at < FW_BASE || at.checked_add(size).is_none_or(|end| end > FW_JUMP_ADDR) {
                return Err(format!(
                    "a segment at {at:#x} of {size:#x} bytes is outside the firmware's RAM, \
                     {FW_BASE:#x}-{:#x}",
                    FW_JUMP_ADDR - 1
                ));
            }
        }
        if !(FW_BASE..FW_JUMP_ADDR).contains(&image.entry) {
            return Err(format!("its entry point {:#x} is outside its RAM", image.entry));
        }
        Ok(image)
    }
}

/// What the boot ROM hands the firmware ([`crate::bus::Bus::set_firmware_boot`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Handoff {
    /// The firmware's entry point.
    pub entry: u64,
    /// The device tree's physical address, in a1.
    pub fdt: u64,
    /// `fw_dynamic_info.next_addr`: the program's entry, or 0 if there is none (the
    /// machine boots from disk).
    pub next_addr: u64,
}

impl Handoff {
    /// The `fw_dynamic_info` the ROM holds, as QEMU's `riscv_rom_copy_firmware_info()`
    /// fills it: the magic, the version, the next stage's address and mode (S), no
    /// options, and the boot hart, 0. Each field a doubleword (an `unsigned long` of
    /// RV64).
    pub fn dynamic_info(&self) -> [u64; 6] {
        [
            FW_DYNAMIC_INFO_MAGIC,
            FW_DYNAMIC_INFO_VERSION,
            self.next_addr,
            FW_DYNAMIC_INFO_NEXT_MODE_S,
            0,
            0,
        ]
    }
}

/// Where the device tree goes on a machine with `ram_size` bytes of RAM, for a tree of
/// `fdt_size` bytes: as QEMU's `riscv_compute_fdt_addr()` places it on an RV64
/// machine, as far up as it can, so that the next stage does not overwrite it, at the
/// last 2 MiB boundary that leaves it room before the end of RAM. `None` if that is
/// below RAM.
pub fn fdt_addr(ram_size: u64, fdt_size: u64) -> Option<u64> {
    let end = RAM_BASE + ram_size;
    let at = end.checked_sub(fdt_size)? & !(FW_SIZE - 1);
    (at >= RAM_BASE).then_some(at)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_built_in_firmware_fits_its_ram() {
        let image = Image::of(&Bios::Default, Xlen::Rv64).unwrap().unwrap();
        assert_eq!(image.entry, FW_BASE, "the firmware starts at its first byte");
        assert_eq!(image.segments[0].0, FW_BASE);
        assert!(Image::of(&Bios::Default, Xlen::Rv32).is_err(), "RV64 only");
        assert_eq!(Image::of(&Bios::None, Xlen::Rv64), Ok(None));
    }

    #[test]
    fn raw_firmware_loads_at_the_start_of_ram() {
        let image = Image::raw(vec![0x6f, 0, 0, 0]).unwrap();
        assert_eq!(image.segments, [(FW_BASE, vec![0x6f, 0, 0, 0], 4)]);
        assert_eq!(image.entry, FW_BASE);
        // 2 MiB at most: the next stage starts after it.
        assert!(Image::raw(vec![0; FW_SIZE as usize]).is_ok());
        let err = Image::raw(vec![0; FW_SIZE as usize + 1]).unwrap_err();
        assert!(err.contains("outside the firmware's RAM"), "{err}");
        let missing = Bios::File("no such firmware.bin".into());
        assert!(Image::of(&missing, Xlen::Rv64)
            .unwrap_err()
            .contains("cannot read"));
    }

    #[test]
    fn the_device_tree_goes_where_qemu_puts_it() {
        // The last 2 MiB boundary with room for it.
        assert_eq!(fdt_addr(128 << 20, 0x1000), Some(0x87e0_0000));
        assert_eq!(fdt_addr(8 << 20, 0x1000), Some(0x8060_0000));
        assert_eq!(fdt_addr(6 << 20, 0x1000), Some(0x8040_0000));
        assert_eq!(fdt_addr(4 << 20, 0x1000), Some(0x8020_0000));
        // In the firmware's own RAM: the machine refuses that (lib.rs).
        assert_eq!(fdt_addr(1 << 20, 0x1000), Some(0x8000_0000));
        assert_eq!(fdt_addr(0x1000, 0x2000), None);
        let info = Handoff {
            entry: FW_BASE,
            fdt: 0x8060_0000,
            next_addr: FW_JUMP_ADDR,
        }
        .dynamic_info();
        assert_eq!(info, [0x4942_534f, 2, 0x8020_0000, 1, 0, 0]);
    }
}
