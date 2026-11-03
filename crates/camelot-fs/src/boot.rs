//! Bootable images (docs/camelot-fs.md, "Booting"): [`install`] puts a
//! bootloader in the boot block and a kernel right after the filesystem, so
//! that Grail started with the image as its disk 0 and no program boots it.
//!
//! Grail's boot ROM reads the boot block (block 0, 1 KiB) into RAM, checks
//! that its last word is [`BOOT_MAGIC`] and that its 256 words add up to 0,
//! and jumps to its first byte (spec section 7, "Boot from disk"). The
//! bootloader, `kernel/boot/boot.s`, then reads the kernel ELF from the
//! sectors the boot block names and loads its segments. The kernel is not a
//! file of the filesystem: a region of its own is far simpler to read for a
//! bootloader of under 1 KiB than a directory, an inode and an extent tree.
//! Neither the boot block nor the blocks past `size` belong to the
//! filesystem, so a bootable image is still a valid filesystem, and fsck,
//! the reader and the kernel see no difference.
//!
//! Only what the bootloader loads is stored ([`loadable`]): the ELF header, the
//! program headers and the bytes of the loadable segments, at their offsets. The
//! section headers, the symbols and their names stay in the kernel's file on the
//! host, for the disassembler and the tests.

use crate::{Superblock, BSIZE, SECTOR_SIZE, SUPER_BLOCK};

/// The boot block's size: block 0.
pub const BOOT_BLOCK_SIZE: usize = BSIZE;
/// The last word of a bootable boot block ("BOOT" in ASCII, little-endian).
pub const BOOT_MAGIC: u32 = 0x544f_4f42;
/// Where the boot block's fields are, all `u32`: the bootloader's code comes
/// first, then these, which mkfs fills in.
pub const KERNEL_SECTOR_AT: usize = 1008;
pub const KERNEL_SECTORS_AT: usize = 1012;
pub const CHECKSUM_AT: usize = 1016;
pub const MAGIC_AT: usize = 1020;
/// The most code a bootloader can have.
pub const MAX_BOOT_CODE: usize = KERNEL_SECTOR_AT;
/// The bootloader reads the kernel's first 1 KiB; its ELF and program
/// headers must be in there.
pub const KERNEL_HEADERS: usize = 1024;
/// Where RAM starts (spec section 7): the kernel's segments go there or above.
pub const RAM_BASE: u32 = 0x8000_0000;

/// Where a bootable image keeps its kernel, as its boot block says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BootInfo {
    /// The kernel ELF's first sector: the one after the filesystem.
    pub kernel_sector: u32,
    /// Its length in sectors, padded to a whole block.
    pub kernel_sectors: u32,
}

/// Parses `elf`, the bootloader's or the kernel's (`what` names it in errors).
/// Both are RV32 programs, ELF32, which is what the bootloader loads and the
/// boot block's layout assumes, until the RV64 migration reaches them.
fn parse32<'a>(what: &str, elf: &'a [u8]) -> Result<elf::Elf<'a>, String> {
    let e = elf::Elf::parse(elf).map_err(|e| format!("{what}: {e}"))?;
    if e.class != elf::Class::Elf32 {
        return Err(format!("{what}: not ELF32, the class the bootloader loads"));
    }
    Ok(e)
}

/// The code of a bootloader linked as an ELF executable (`kernel/boot/boot.elf`):
/// its loadable bytes, from the lowest address, where its entry point must be.
/// It must fit in [`MAX_BOOT_CODE`] bytes and have no `.bss` (nothing would
/// zero it).
pub fn boot_code(elf: &[u8]) -> Result<Vec<u8>, String> {
    let e = parse32("bootloader", elf)?;
    let segs: Vec<_> = e.segments.iter().filter(|p| p.ty == elf::PT_LOAD).collect();
    let base = segs
        .iter()
        .map(|p| p.vaddr)
        .min()
        .ok_or("bootloader: nothing to load")?;
    if e.header.entry != base {
        return Err(format!(
            "bootloader: the entry point ({:#x}) must be its first byte ({base:#x})",
            e.header.entry
        ));
    }
    let mut code = Vec::new();
    for p in segs {
        if p.memsz != p.filesz {
            return Err("bootloader: it has .bss, which nothing would zero".into());
        }
        let at = (p.vaddr - base) as usize;
        let end = at + p.filesz as usize;
        if end > MAX_BOOT_CODE {
            return Err(format!(
                "bootloader: {end} bytes, more than the {MAX_BOOT_CODE} a boot block has room for"
            ));
        }
        code.resize(code.len().max(end), 0);
        code[at..end].copy_from_slice(e.segment_data(p).map_err(|e| format!("bootloader: {e}"))?);
    }
    Ok(code)
}

/// Checks that the bootloader can load the kernel ELF `elf`: an Excalibur
/// executable (ELF32, little-endian, RISC-V, `ET_EXEC`) whose ELF header and
/// program headers are in its first [`KERNEL_HEADERS`] bytes, and whose
/// loadable segments go in RAM and, when they have bytes in the file, start at
/// a sector boundary of it (a `.bss`-only segment reads nothing, so its file
/// offset does not matter). (Whether they fit below the bootloader depends on
/// the machine's RAM: the bootloader checks that.)
pub fn check_kernel(elf: &[u8]) -> Result<(), String> {
    let e = parse32("kernel", elf)?;
    let h = &e.header;
    if h.ty != elf::ET_EXEC || h.machine != elf::EM_RISCV {
        return Err("kernel: not a RISC-V executable".into());
    }
    if h.phentsize as usize != elf::PHDR_SIZE
        || h.phoff as usize + h.phnum as usize * elf::PHDR_SIZE > KERNEL_HEADERS
    {
        return Err(format!(
            "kernel: the program headers must be in the first {KERNEL_HEADERS} bytes"
        ));
    }
    for p in e.segments.iter().filter(|p| p.ty == elf::PT_LOAD) {
        if p.filesz != 0 && !(p.offset as usize).is_multiple_of(SECTOR_SIZE) {
            return Err(format!(
                "kernel: the segment at {:#x} is at file offset {:#x}, not a multiple of {SECTOR_SIZE}",
                p.paddr, p.offset
            ));
        }
        // ELF32 values: the end must not pass 2^32 - 1.
        if p.memsz < p.filesz || p.paddr < u64::from(RAM_BASE) || p.paddr + p.memsz > u64::from(u32::MAX) {
            return Err(format!("kernel: the segment at {:#x} is not in RAM", p.paddr));
        }
    }
    Ok(())
}

/// What of the kernel ELF `elf` a bootable image stores: the ELF header and the
/// program headers, and each loadable segment's bytes at their offset in the file,
/// up to the end of the last one; the gaps between them are zeros. The section
/// headers (and with them the symbols) are gone: the header says there are none.
/// The bootloader reads only these parts, and the result is still an ELF file that
/// loads the same.
pub fn loadable(elf: &[u8]) -> Result<Vec<u8>, String> {
    // ELF32 only: the header fields cleared below are at ELF32's offsets.
    let e = parse32("kernel", elf)?;
    let h = &e.header;
    let headers = h.phoff as usize + h.phnum as usize * elf::PHDR_SIZE;
    let mut out = elf
        .get(..headers.max(elf::EHDR_SIZE))
        .ok_or("kernel: the program headers run past the file")?
        .to_vec();
    for p in e
        .segments
        .iter()
        .filter(|p| p.ty == elf::PT_LOAD && p.filesz != 0)
    {
        let data = e.segment_data(p).map_err(|e| format!("kernel: {e}"))?;
        let at = p.offset as usize;
        out.resize(out.len().max(at + data.len()), 0);
        out[at..at + data.len()].copy_from_slice(data);
    }
    // No section headers: e_shoff, then e_shentsize, e_shnum and e_shstrndx.
    out[32..36].fill(0);
    out[46..52].fill(0);
    Ok(out)
}

/// Makes the filesystem `image` bootable: the bootloader's code (from its ELF,
/// see [`boot_code`]) in the boot block, the kernel ELF (see [`check_kernel`];
/// only its [`loadable`] part) appended after the filesystem's last block and
/// padded to a whole block, and the boot block's fields and checksum filled in.
/// `image` must hold exactly the filesystem, as [`crate::mkfs`] makes it, with
/// nothing in the boot block.
pub fn install(image: &mut Vec<u8>, boot_elf: &[u8], kernel_elf: &[u8]) -> Result<BootInfo, String> {
    let code = boot_code(boot_elf)?;
    check_kernel(kernel_elf)?;
    let kernel_elf = &loadable(kernel_elf)?;
    let sb = Superblock::decode(crate::block(image, SUPER_BLOCK));
    sb.validate((image.len() / BSIZE) as u64)?;
    if image.len() != sb.size as usize * BSIZE {
        return Err("the image must end with its filesystem".into());
    }
    if image[..BOOT_BLOCK_SIZE].iter().any(|&b| b != 0) {
        return Err("the boot block is in use".into());
    }
    let blocks = kernel_elf.len().div_ceil(BSIZE);
    let sectors_per_block = (BSIZE / SECTOR_SIZE) as u64;
    let info = BootInfo {
        kernel_sector: u32::try_from(sb.size as u64 * sectors_per_block)
            .map_err(|_| "the filesystem is too big to boot from")?,
        kernel_sectors: u32::try_from(blocks as u64 * sectors_per_block)
            .map_err(|_| "the kernel is too big")?,
    };
    image.extend_from_slice(kernel_elf);
    image.resize((sb.size as usize + blocks) * BSIZE, 0);

    let block = &mut image[..BOOT_BLOCK_SIZE];
    block[..code.len()].copy_from_slice(&code);
    put(block, KERNEL_SECTOR_AT, info.kernel_sector);
    put(block, KERNEL_SECTORS_AT, info.kernel_sectors);
    put(block, MAGIC_AT, BOOT_MAGIC);
    let fix = 0u32.wrapping_sub(checksum(block));
    put(block, CHECKSUM_AT, fix);
    Ok(info)
}

/// What the boot block of `image` says, if Grail's boot ROM would boot it:
/// `None` when it has no signature, an error when it has one but a bad
/// checksum or names sectors past the image. `kernel_sectors` 0 means no
/// kernel (Camelot's bootloader refuses it).
pub fn boot_info(image: &[u8]) -> Result<Option<BootInfo>, String> {
    let Some(block) = image.get(..BOOT_BLOCK_SIZE) else {
        return Ok(None);
    };
    if get(block, MAGIC_AT) != BOOT_MAGIC {
        return Ok(None);
    }
    if checksum(block) != 0 {
        return Err("the boot block is corrupt (bad checksum)".into());
    }
    let info = BootInfo {
        kernel_sector: get(block, KERNEL_SECTOR_AT),
        kernel_sectors: get(block, KERNEL_SECTORS_AT),
    };
    if info.kernel_sectors == 0 {
        return Ok(Some(info));
    }
    let end = info.kernel_sector as u64 + info.kernel_sectors as u64;
    if end * SECTOR_SIZE as u64 > image.len() as u64 {
        return Err(format!(
            "the boot block's kernel (sectors {} to {}) runs past the image",
            info.kernel_sector,
            end - 1
        ));
    }
    Ok(Some(info))
}

/// The kernel ELF of a bootable image (with the padding after it).
pub fn kernel_image(image: &[u8]) -> Result<&[u8], String> {
    let info = boot_info(image)?.ok_or("the image is not bootable")?;
    if info.kernel_sectors == 0 {
        return Err("the boot block names no kernel".into());
    }
    let at = info.kernel_sector as usize * SECTOR_SIZE;
    Ok(&image[at..at + info.kernel_sectors as usize * SECTOR_SIZE])
}

/// The wrapping sum of the block's little-endian words.
fn checksum(block: &[u8]) -> u32 {
    (0..block.len() / 4).fold(0u32, |sum, i| sum.wrapping_add(get(block, 4 * i)))
}

fn get(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

fn put(b: &mut [u8], at: usize, v: u32) {
    b[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mkfs::{self, Options};

    fn small() -> Options {
        Options {
            size: 300,
            ninodes: 32,
            nlog: 12,
        }
    }

    /// A minimal ELF32 RISC-V executable: the header, `phdrs` program headers
    /// (type, offset, paddr, filesz, memsz) right after it, and `size` bytes
    /// in all.
    fn elf(entry: u32, phdrs: &[(u32, u32, u32, u32, u32)], size: usize) -> Vec<u8> {
        let mut f = vec![0u8; size];
        f[..4].copy_from_slice(b"\x7fELF");
        f[4] = 1; // ELFCLASS32
        f[5] = 1; // little-endian
        f[6] = 1; // EV_CURRENT
        f[16..18].copy_from_slice(&elf::ET_EXEC.to_le_bytes());
        f[18..20].copy_from_slice(&elf::EM_RISCV.to_le_bytes());
        f[20..24].copy_from_slice(&1u32.to_le_bytes());
        f[24..28].copy_from_slice(&entry.to_le_bytes());
        f[28..32].copy_from_slice(&52u32.to_le_bytes());
        f[40..42].copy_from_slice(&52u16.to_le_bytes());
        f[42..44].copy_from_slice(&32u16.to_le_bytes());
        f[44..46].copy_from_slice(&(phdrs.len() as u16).to_le_bytes());
        for (i, &(ty, off, paddr, filesz, memsz)) in phdrs.iter().enumerate() {
            let at = 52 + 32 * i;
            for (j, v) in [ty, off, paddr, paddr, filesz, memsz, 5, 0x1000]
                .iter()
                .enumerate()
            {
                f[at + 4 * j..at + 4 * j + 4].copy_from_slice(&v.to_le_bytes());
            }
        }
        f
    }

    fn bootloader(len: u32) -> Vec<u8> {
        let mut f = elf(0, &[(elf::PT_LOAD, 0x200, 0, len, len)], 0x200 + len as usize);
        for (i, b) in f[0x200..].iter_mut().enumerate() {
            *b = i as u8 | 1;
        }
        f
    }

    fn kernel_elf() -> Vec<u8> {
        let mut f = elf(RAM_BASE, &[(elf::PT_LOAD, 0x400, RAM_BASE, 0x300, 0x2000)], 0x700);
        f[0x400..].fill(0xab);
        f
    }

    /// A kernel with two segments, a gap between them, and after them what a
    /// linker puts there: section headers (which the ELF header points to) and
    /// symbols.
    fn kernel_with_sections() -> Vec<u8> {
        let mut f = elf(
            RAM_BASE,
            &[
                (elf::PT_LOAD, 0x200, RAM_BASE, 0x100, 0x100),
                (elf::PT_NULL, 0, 0, 0, 0),
                (elf::PT_LOAD, 0x600, RAM_BASE + 0x1000, 0x80, 0x400),
            ],
            0x900,
        );
        f[0x200..0x300].fill(0x11);
        f[0x300..0x600].fill(0xee); // not part of any segment
        f[0x600..0x680].fill(0x22);
        f[0x680..].fill(0x33); // symbols, names, ...
        f[32..36].copy_from_slice(&0x800u32.to_le_bytes()); // e_shoff
        f[46..48].copy_from_slice(&40u16.to_le_bytes()); // e_shentsize
        f[48..50].copy_from_slice(&0u16.to_le_bytes()); // e_shnum: none to parse
        f[50..52].copy_from_slice(&7u16.to_le_bytes()); // e_shstrndx
        f
    }

    /// An RV64 program (ELF64): what the bootloader cannot load yet, as
    /// bootloader or as kernel, and whose header `loadable` must not rewrite
    /// at ELF32's offsets.
    #[test]
    fn elf64_files_are_refused() {
        let rv64 = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../elf/tests/llvm/rv64/prog.elf"
        ))
        .unwrap();
        let want = |what: &str| format!("{what}: not ELF32, the class the bootloader loads");
        assert_eq!(boot_code(&rv64), Err(want("bootloader")));
        assert_eq!(check_kernel(&rv64), Err(want("kernel")));
        assert_eq!(loadable(&rv64), Err(want("kernel")));
        let mut image = mkfs::mkfs(&small(), None).unwrap();
        let err = install(&mut image, &rv64, &kernel_elf()).unwrap_err();
        assert_eq!(err, want("bootloader"));
        let err = install(&mut image, &bootloader(64), &rv64).unwrap_err();
        assert_eq!(err, want("kernel"));
    }

    #[test]
    fn only_what_the_bootloader_loads_is_kept() {
        let full = kernel_with_sections();
        let kept = loadable(&full).unwrap();
        // Up to the end of the last segment's bytes.
        assert_eq!(kept.len(), 0x680);
        // The headers, but no section headers.
        assert_eq!(&kept[..32], &full[..32]);
        assert_eq!(&kept[36..46], &full[36..46]);
        assert!(kept[32..36].iter().chain(&kept[46..52]).all(|&b| b == 0));
        assert_eq!(&kept[52..52 + 3 * 32], &full[52..52 + 3 * 32]);
        // The segments where they were; nothing else.
        assert!(kept[0x200..0x300].iter().all(|&b| b == 0x11));
        assert!(kept[0x300..0x600].iter().all(|&b| b == 0));
        assert!(kept[0x600..0x680].iter().all(|&b| b == 0x22));
        // It loads the same.
        let (a, b) = (elf::Elf::parse(&full).unwrap(), elf::Elf::parse(&kept).unwrap());
        assert_eq!(a.header.entry, b.header.entry);
        assert_eq!(a.segments, b.segments);
        for p in a.segments.iter().filter(|p| p.ty == elf::PT_LOAD) {
            assert_eq!(a.segment_data(p).unwrap(), b.segment_data(p).unwrap());
        }
        assert!(b.sections.is_empty());
        assert_eq!(check_kernel(&kept), Ok(()));
        // Twice is once.
        assert_eq!(loadable(&kept).unwrap(), kept);

        // install stores just that.
        let mut image = mkfs::mkfs(&small(), None).unwrap();
        let info = install(&mut image, &bootloader(100), &full).unwrap();
        assert_eq!(
            info.kernel_sectors, 4,
            "0x680 bytes, two blocks (the whole file would take three)"
        );
        assert_eq!(&kernel_image(&image).unwrap()[..kept.len()], &kept[..]);
        assert!(loadable(b"not an elf").unwrap_err().starts_with("kernel:"));
    }

    #[test]
    fn installs_after_the_filesystem() {
        let mut image = mkfs::mkfs(&small(), None).unwrap();
        assert_eq!(boot_info(&image), Ok(None));
        let boot = bootloader(600);
        let elf = kernel_elf();
        let info = install(&mut image, &boot, &elf).unwrap();
        assert_eq!(
            info,
            BootInfo {
                kernel_sector: 600,
                kernel_sectors: 4
            }
        );
        assert_eq!(image.len(), 302 * BSIZE, "the kernel padded to whole blocks");
        assert_eq!(boot_info(&image), Ok(Some(info)));
        assert_eq!(&kernel_image(&image).unwrap()[..elf.len()], &elf[..]);
        assert!(kernel_image(&image).unwrap()[elf.len()..].iter().all(|&b| b == 0));
        assert_eq!(&image[..600], &boot[0x200..]);
        assert!(image[600..KERNEL_SECTOR_AT].iter().all(|&b| b == 0));
        assert_eq!(get(&image, MAGIC_AT), BOOT_MAGIC);
        assert_eq!(checksum(&image[..BOOT_BLOCK_SIZE]), 0);
        // Still the same filesystem.
        let r = crate::fsck::check(&image).unwrap();
        assert_eq!(r.sb.size, 300);

        // Corruption is noticed; an image too short for its kernel too.
        let mut bad = image.clone();
        bad[100] ^= 1;
        assert!(boot_info(&bad).unwrap_err().contains("checksum"));
        assert_eq!(
            boot_info(&image[..301 * BSIZE]),
            Err("the boot block's kernel (sectors 600 to 603) runs past the image".into())
        );
        assert!(kernel_image(&mkfs::mkfs(&small(), None).unwrap()).is_err());
        // A boot block that names no kernel: no range to check, and no kernel.
        let mut none = image.clone();
        put(&mut none, KERNEL_SECTOR_AT, 0xdead_beef);
        put(&mut none, KERNEL_SECTORS_AT, 0);
        put(&mut none, CHECKSUM_AT, 0);
        let fix = 0u32.wrapping_sub(checksum(&none[..BOOT_BLOCK_SIZE]));
        put(&mut none, CHECKSUM_AT, fix);
        assert_eq!(
            boot_info(&none),
            Ok(Some(BootInfo {
                kernel_sector: 0xdead_beef,
                kernel_sectors: 0
            }))
        );
        assert!(kernel_image(&none).unwrap_err().contains("no kernel"));
    }

    #[test]
    fn refuses_what_would_not_boot() {
        let fresh = || mkfs::mkfs(&small(), None).unwrap();
        let kernel = kernel_elf();
        let err = |boot: &[u8], kernel: &[u8]| install(&mut fresh(), boot, kernel).unwrap_err();

        assert!(err(&bootloader(1009), &kernel).contains("1009 bytes"));
        assert!(install(&mut fresh(), &bootloader(1008), &kernel).is_ok());
        let mut entry = bootloader(100);
        entry[24] = 4;
        assert!(err(&entry, &kernel).contains("entry point"));
        let bss = elf(0, &[(elf::PT_LOAD, 0x200, 0, 16, 32)], 0x210);
        assert!(err(&bss, &kernel).contains(".bss"));
        assert!(err(b"not an elf", &kernel).starts_with("bootloader:"));

        let boot = bootloader(100);
        assert!(err(&boot, b"nope").starts_with("kernel:"));
        let mut rel = kernel.clone();
        rel[16] = elf::ET_REL as u8;
        assert!(err(&boot, &rel).contains("not a RISC-V executable"));
        let odd = elf(RAM_BASE, &[(elf::PT_LOAD, 0x204, RAM_BASE, 4, 4)], 0x300);
        assert!(err(&boot, &odd).contains("not a multiple of 512"));
        // A .bss-only segment reads nothing from the file: its offset is not checked
        // (merlin-ld gives it one congruent to its address modulo 4 KiB).
        let bss_only = elf(
            RAM_BASE,
            &[
                (elf::PT_LOAD, 0x200, RAM_BASE, 4, 4),
                (elf::PT_LOAD, 0x210, RAM_BASE + 0x1010, 0, 0x100),
            ],
            0x300,
        );
        assert!(install(&mut fresh(), &boot, &bss_only).is_ok());
        let low = elf(0, &[(elf::PT_LOAD, 0x200, 0x1000, 4, 4)], 0x300);
        assert!(err(&boot, &low).contains("not in RAM"));
        let wraps = elf(RAM_BASE, &[(elf::PT_LOAD, 0x200, 0xffff_f000, 4, 0x2000)], 0x300);
        assert!(err(&boot, &wraps).contains("not in RAM"));
        let many: Vec<_> = (0..31).map(|_| (elf::PT_NULL, 0, 0, 0, 0)).collect();
        assert!(err(&boot, &elf(RAM_BASE, &many, 0x800)).contains("first 1024 bytes"));

        let mut used = fresh();
        used[5] = 1;
        assert!(install(&mut used, &boot, &kernel)
            .unwrap_err()
            .contains("boot block is in use"));
        let mut twice = fresh();
        install(&mut twice, &boot, &kernel).unwrap();
        assert!(install(&mut twice, &boot, &kernel).is_err());
        assert!(
            install(&mut vec![0; 4 * BSIZE], &boot, &kernel).is_err(),
            "no filesystem"
        );
    }
}
