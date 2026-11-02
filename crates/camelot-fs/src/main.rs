//! `camelot-fs`: builds, checks and reads Camelot filesystem images
//! (docs/camelot-fs.md) on the host.

use camelot_fs::boot;
use camelot_fs::mkfs::{self, Options};
use camelot_fs::read::Fs;
use camelot_fs::{fsck, BSIZE};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const USAGE: &str = "usage: camelot-fs <command> ...

commands:
  mkfs IMAGE [DIR] [--size N] [--inodes N] [--log N] [--boot BOOT --kernel KERNEL]
        write a new filesystem image, holding a copy of the host directory DIR
        (its files and subdirectories) if given. --size is in 1 KiB blocks, or
        with a K or M suffix in KiB or MiB (default 2048 blocks, 2 MiB);
        --inodes counts inode slots (default 256), --log the log's blocks with
        its header (default 21; the kernel mounts a log of fewer than 11
        blocks read-only). --boot and --kernel make it bootable: the
        bootloader BOOT (an ELF, kernel/boot/boot.elf) goes in the boot block
        and the kernel ELF KERNEL after the filesystem, so `grail --disk
        IMAGE` boots it
  fsck [--recover] IMAGE
        check every consistency rule; a committed transaction still in the log
        is checked as replayed, and --recover writes the replay to IMAGE, then
        frees the inodes a crash left with no links, as the kernel's mount
        does
  ls IMAGE [PATH]
        list a directory (default /): type, inode, links, size, name
  cat IMAGE PATH
        write a file's contents to stdout";

/// A size: blocks, or KiB/MiB with a K/M suffix.
fn parse_size(s: &str) -> Result<u32, String> {
    let bad = || format!("bad size {s} (blocks, or a number with K or M)");
    let (digits, unit) = match s.char_indices().last() {
        Some((i, 'K' | 'k')) => (&s[..i], 1024u64),
        Some((i, 'M' | 'm')) => (&s[..i], 1024 * 1024),
        _ => (s, BSIZE as u64),
    };
    let n: u64 = digits.parse().map_err(|_| bad())?;
    let bytes = n.checked_mul(unit).ok_or_else(bad)?;
    if bytes % BSIZE as u64 != 0 {
        return Err(format!("{s} is not a whole number of 1 KiB blocks"));
    }
    u32::try_from(bytes / BSIZE as u64).map_err(|_| bad())
}

#[derive(Debug, PartialEq, Eq)]
struct MkfsArgs {
    image: PathBuf,
    dir: Option<PathBuf>,
    opts: Options,
    /// The bootloader and the kernel to install, if any.
    boot: Option<(PathBuf, PathBuf)>,
}

fn parse_mkfs(args: &[String]) -> Result<MkfsArgs, String> {
    let mut opts = Options::default();
    let mut paths = Vec::new();
    let (mut boot, mut kernel) = (None, None);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = |name: &str| it.next().ok_or_else(|| format!("{name} needs a value"));
        match a.as_str() {
            "--size" => opts.size = parse_size(value("--size")?)?,
            "--inodes" => {
                let v = value("--inodes")?;
                opts.ninodes = v.parse().map_err(|_| format!("bad inode count {v}"))?;
            }
            "--log" => {
                let v = value("--log")?;
                opts.nlog = v.parse().map_err(|_| format!("bad log size {v}"))?;
            }
            "--boot" => boot = Some(PathBuf::from(value("--boot")?)),
            "--kernel" => kernel = Some(PathBuf::from(value("--kernel")?)),
            other if other.starts_with('-') => return Err(format!("unknown option {other}")),
            path => paths.push(PathBuf::from(path)),
        }
    }
    let mut paths = paths.into_iter();
    let image = paths.next().ok_or("mkfs needs an image path")?;
    let dir = paths.next();
    if paths.next().is_some() {
        return Err("mkfs takes an image and at most one directory".into());
    }
    let boot = match (boot, kernel) {
        (Some(b), Some(k)) => Some((b, k)),
        (None, None) => None,
        _ => return Err("--boot and --kernel go together".into()),
    };
    Ok(MkfsArgs {
        image,
        dir,
        opts,
        boot,
    })
}

fn read_image(path: &Path) -> Result<Vec<u8>, String> {
    std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))
}

/// Says where a bootable image's kernel is; nothing for an image with no signed
/// boot block. A signed boot block that is not a good one of Camelot's (a bad
/// checksum, no kernel, a kernel past the image) gets a warning, not an error:
/// the filesystem is fine all the same, and the block may be another author's
/// bootloader, which the boot ROM boots as well (it only checks the signature
/// and the checksum).
fn print_boot(image: &[u8]) {
    match boot::boot_info(image) {
        Ok(None) => {}
        Ok(Some(info)) if info.kernel_sectors == 0 => {
            println!("not bootable: the boot block names no kernel")
        }
        Ok(Some(info)) => println!(
            "bootable: kernel at sector {}, {} sectors ({} KiB)",
            info.kernel_sector,
            info.kernel_sectors,
            info.kernel_sectors / 2
        ),
        Err(e) => println!("warning: {e}"),
    }
}

fn run(args: &[String]) -> Result<ExitCode, String> {
    let words: Vec<&str> = args.iter().map(String::as_str).collect();
    match words.as_slice() {
        ["mkfs", ..] => {
            let a = parse_mkfs(&args[1..])?;
            let mut image = mkfs::mkfs(&a.opts, a.dir.as_deref())?;
            if let Some((b, k)) = &a.boot {
                boot::install(&mut image, &read_image(b)?, &read_image(k)?)?;
            }
            std::fs::write(&a.image, &image).map_err(|e| format!("{}: {e}", a.image.display()))?;
            let r = fsck::check(&image).map_err(|p| p.join("\n"))?;
            println!(
                "{}: {} blocks ({} free), {} inodes ({} free), {} directories, {} files",
                a.image.display(),
                r.sb.size,
                r.free_blocks,
                r.sb.ninodes,
                r.free_inodes,
                r.dirs,
                r.files
            );
            print_boot(&image);
            Ok(ExitCode::SUCCESS)
        }
        ["fsck", rest @ ..] => {
            let (recover, path) = match rest {
                ["--recover", path] => (true, path),
                [path] => (false, path),
                _ => return Err("fsck takes [--recover] IMAGE".into()),
            };
            let path = Path::new(path);
            let mut image = read_image(path)?;
            match fsck::check(&image) {
                Ok(mut r) => {
                    if recover && (r.replayed > 0 || r.orphans > 0) {
                        let replayed = fsck::recover(&mut image)?;
                        let freed = fsck::free_orphans(&mut image)?;
                        std::fs::write(path, &image).map_err(|e| format!("{}: {e}", path.display()))?;
                        if replayed > 0 {
                            println!("replayed {replayed} blocks from the log");
                        }
                        if freed > 0 {
                            println!("freed {freed} orphaned inodes");
                        }
                        r = fsck::check(&image).map_err(|p| p.join("\n"))?;
                    } else {
                        if r.replayed > 0 {
                            println!("the log holds a committed transaction of {} blocks", r.replayed);
                        }
                        if r.orphans > 0 {
                            println!(
                                "{} orphaned inodes (no links, in no directory) for the next mount to free",
                                r.orphans
                            );
                        }
                    }
                    println!(
                        "{}: clean: {} blocks ({} used, {} free), {} inodes ({} free), \
                         {} directories, {} files, {} devices",
                        path.display(),
                        r.sb.size,
                        r.used_blocks,
                        r.free_blocks,
                        r.sb.ninodes,
                        r.free_inodes,
                        r.dirs,
                        r.files,
                        r.devices
                    );
                    print_boot(&image);
                    Ok(ExitCode::SUCCESS)
                }
                Err(problems) => {
                    for p in &problems {
                        println!("{p}");
                    }
                    println!("{}: {} problems", path.display(), problems.len());
                    Ok(ExitCode::FAILURE)
                }
            }
        }
        ["ls", image, rest @ ..] if rest.len() <= 1 => {
            let fs = Fs::open(&read_image(Path::new(image))?)?;
            print!("{}", fs.ls(rest.first().copied().unwrap_or("/"))?);
            Ok(ExitCode::SUCCESS)
        }
        ["cat", image, path] => {
            let fs = Fs::open(&read_image(Path::new(image))?)?;
            let data = fs.read_file(path)?;
            std::io::stdout().write_all(&data).map_err(|e| e.to_string())?;
            Ok(ExitCode::SUCCESS)
        }
        ["-h" | "--help" | "help"] => {
            println!("{USAGE}");
            Ok(ExitCode::SUCCESS)
        }
        _ => {
            eprintln!("{USAGE}");
            Ok(ExitCode::from(2))
        }
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("camelot-fs: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes() {
        assert_eq!(parse_size("2048"), Ok(2048));
        assert_eq!(parse_size("64K"), Ok(64));
        assert_eq!(parse_size("2M"), Ok(2048));
        assert!(parse_size("1.5M").is_err());
        assert!(parse_size("x").is_err());
        assert!(parse_size("99999999999M").is_err());
    }

    #[test]
    fn mkfs_args() {
        let args = |v: &[&str]| parse_mkfs(&v.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        assert_eq!(
            args(&["disk.img"]).unwrap(),
            MkfsArgs {
                image: "disk.img".into(),
                dir: None,
                opts: Options::default(),
                boot: None,
            }
        );
        let a = args(&["--size", "1M", "d.img", "root", "--inodes", "64", "--log", "8"]).unwrap();
        assert_eq!(a.dir, Some("root".into()));
        assert_eq!(
            a.opts,
            Options {
                size: 1024,
                ninodes: 64,
                nlog: 8
            }
        );
        assert!(args(&[]).unwrap_err().contains("needs an image"));
        assert!(args(&["a", "b", "c"]).unwrap_err().contains("at most one"));
        assert!(args(&["--size"]).unwrap_err().contains("needs a value"));
        assert!(args(&["--frob", "a"]).unwrap_err().contains("unknown option"));
        let a = args(&["d.img", "--boot", "b.elf", "--kernel", "k.elf"]).unwrap();
        assert_eq!(a.boot, Some(("b.elf".into(), "k.elf".into())));
        assert!(args(&["d.img", "--boot", "b.elf"])
            .unwrap_err()
            .contains("go together"));
        assert!(args(&["d.img", "--kernel", "k.elf"])
            .unwrap_err()
            .contains("go together"));
    }
}
