//! The GUI library (#183, `kernel/user/gui`, `libgui.a`): Grail boots the
//! disk kernel on an image with the desktop's `/usr/share` (the font, the
//! images), and `guitest` (`kernel/user/guitest.c`) draws each scene into a
//! buffer in memory and writes it to `/out` as a PPM file. The host reads
//! them back from the disk and compares each with its expected image,
//! `tests/gui/NAME.png`, by encoding it as a PNG ([`xtask::png`]) and
//! comparing the bytes. The parts that draw nothing (damage lists, the
//! keyboard, UTF-8) print what they did, which is compared too.
//!
//! When the drawing changes on purpose, `GUI_BLESS=1 cargo test --release
//! -p xtask --test gui` writes the new expected images; look at them before
//! committing. A failing comparison leaves the image drawn in
//! `target/gui-actual/NAME.png`.

use camelot_fs::mkfs::{Mkfs, Options};
use camelot_fs::read::Fs;
use grail::block::MemDisk;
use grail::{Machine, MachineConfig, RunResult};
use std::path::PathBuf;
use std::sync::OnceLock;
use xtask::{kernel, png, repo_root, userprogs};

const PROMPT: &str = "$ ";

/// What the scenes' run printed, and its disk afterwards.
struct Run {
    out: String,
    disk: Vec<u8>,
}

/// The programs in `/bin`, besides the shell.
const BINS: &[&str] = &["guitest", "guidemo"];

/// The commands the shell runs, in order.
const COMMANDS: &[&str] = &[
    "guitest draw /out",
    "guitest text /out",
    "guitest image /out",
    "guitest widgets /out",
    "guitest wayland /out",
    "guitest damage",
    "guitest keys",
    "guitest utf8",
];

/// The images the scenes write to `/out`.
const IMAGES: &[&str] = &[
    "draw", "text", "image", "widgets", "input", "wayland1", "wayland2", "wayland3",
];

/// An image with `/dev/console`, the shell and [`BINS`] in `/bin`, the
/// desktop's `/usr/share`, and an empty `/run` and `/out`.
fn image() -> Vec<u8> {
    let root = repo_root();
    let share = xtask::fs::share_files(&root).unwrap();
    let mut fs = Mkfs::new(&Options {
        size: 4096,
        ..Options::default()
    })
    .unwrap();
    fs.mkdir("/dev").unwrap();
    fs.add_device("/dev/console", xtask::fs::CONSOLE_MAJOR, 0)
        .unwrap();
    fs.mkdir("/bin").unwrap();
    let user = userprogs::user_dir(&root);
    for name in std::iter::once(&"sh").chain(BINS) {
        let prog = userprogs::build_named(&user, name).unwrap();
        fs.add_file(&format!("/bin/{name}"), &prog.elf).unwrap();
    }
    fs.mkdir("/usr").unwrap();
    xtask::fs::add_share(&mut fs, &share).unwrap();
    fs.mkdir("/run").unwrap();
    fs.mkdir("/out").unwrap();
    fs.finish()
}

/// Boots the disk kernel on [`image`] (16 MiB of RAM besides the
/// firmware's), runs [`COMMANDS`] and `exit 0`; once for every test.
fn run() -> &'static Run {
    static RUN: OnceLock<Run> = OnceLock::new();
    RUN.get_or_init(|| {
        let config = MachineConfig {
            bios: grail::Bios::Default,
            ram_size: 16 * 1024 * 1024 + grail::firmware::FW_SIZE as usize,
            ..MachineConfig::default()
        };
        let disk = MemDisk::from_image(image()).unwrap();
        let mut m = Machine::with_backends(config, vec![Box::new(disk)]).unwrap();
        m.cpu.bus.uart.capture = true;
        let elf = std::fs::read(repo_root().join("kernel").join(kernel::DISK_KERNEL)).unwrap();
        m.load_elf(&elf).unwrap();
        let input = format!("{}\nexit 0\n", COMMANDS.join("\n"));
        m.set_uart_input(kernel::typed(input.as_bytes()));
        let r = m.run(Some(4_000_000_000), |_| {});
        let out = String::from_utf8_lossy(&m.cpu.bus.uart.captured).into_owned();
        assert_eq!(r, RunResult::Exited(0), "{out}");
        let d = &mut m.cpu.bus.disks[0];
        let mut data = vec![0; d.capacity() as usize * camelot_fs::SECTOR_SIZE];
        d.backend().read(0, &mut data).unwrap();
        Run { out, disk: data }
    })
}

/// What the shell printed after `command`, up to its next prompt.
fn output<'a>(out: &'a str, command: &str) -> &'a str {
    let typed = format!("{PROMPT}{command}\n");
    let start = out
        .find(&typed)
        .unwrap_or_else(|| panic!("no `{command}` in:\n{out}"))
        + typed.len();
    let end = out[start..].find(PROMPT).map_or(out.len(), |e| start + e);
    &out[start..end]
}

/// A PPM (`P6`, 255) as the width, the height and the RGB bytes.
fn read_ppm(b: &[u8]) -> (u32, u32, Vec<u8>) {
    let mut fields = Vec::new();
    let mut at = 0;
    while fields.len() < 4 {
        while b[at].is_ascii_whitespace() {
            at += 1;
        }
        let start = at;
        while !b[at].is_ascii_whitespace() {
            at += 1;
        }
        fields.push(String::from_utf8_lossy(&b[start..at]).into_owned());
    }
    assert_eq!((fields[0].as_str(), fields[3].as_str()), ("P6", "255"));
    let (w, h): (u32, u32) = (fields[1].parse().unwrap(), fields[2].parse().unwrap());
    let rgb = b[at + 1..].to_vec();
    assert_eq!(rgb.len(), (w * h * 3) as usize);
    (w, h, rgb)
}

fn expected_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/gui")
}

/// Every scene's image is its expected one.
#[test]
fn the_scenes_draw_the_expected_images() {
    let run = run();
    for c in COMMANDS {
        assert_eq!(
            output(&run.out, c).lines().find(|l| l.starts_with("guitest:")),
            None,
            "{c}"
        );
    }
    let fs = Fs::open(&run.disk).unwrap();
    let bless = std::env::var_os("GUI_BLESS").is_some();
    let actual = repo_root().join("target/gui-actual");
    let mut wrong = Vec::new();
    for name in IMAGES {
        let ppm = fs.read_file(&format!("/out/{name}.ppm")).unwrap();
        let (w, h, rgb) = read_ppm(&ppm);
        let got = png::encode(w, h, &rgb);
        let path = expected_dir().join(format!("{name}.png"));
        if bless {
            std::fs::create_dir_all(expected_dir()).unwrap();
            std::fs::write(&path, &got).unwrap();
            continue;
        }
        let want = std::fs::read(&path).unwrap_or_default();
        if want != got {
            std::fs::create_dir_all(&actual).unwrap();
            std::fs::write(actual.join(format!("{name}.png")), &got).unwrap();
            let differ = png::decode(&want).map_or("no expected image".to_string(), |(ww, wh, want)| {
                if (ww, wh) != (w, h) {
                    format!("{ww}x{wh}, drawn {w}x{h}")
                } else {
                    let n = want.chunks(3).zip(rgb.chunks(3)).filter(|(a, b)| a != b).count();
                    format!("{n} pixels differ")
                }
            });
            wrong.push(format!("{name}: {differ}"));
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?} (drawn in {})", actual.display());
}

/// What guidemo asks of guitest's display server (`guitest wayland`): the
/// registry and the globals bound, the seat's pointer and keyboard (a
/// second roundtrip), the window, the first commit with no buffer, then
/// the pool and its two buffers at the configured size, the configure
/// acknowledged, the first frame all damaged; the ping answered; the
/// second frame in the other buffer, only the field and the greeting
/// damaged; the third in the first buffer again (released when the second
/// was committed), the field and the list; then, closed, everything
/// destroyed. Ids are reused once the server has deleted them (3, the
/// first sync's callback, is the surface; 15, the frame callback, each
/// frame's).
const WAYLAND_LOG: &str = "\
> wl_display@1.get_registry 2
> wl_display@1.sync 3
> wl_registry@2.bind 1 wl_compositor v4 -> 4
> wl_registry@2.bind 2 wl_shm v1 -> 5
> wl_registry@2.bind 3 xdg_wm_base v1 -> 6
> wl_registry@2.bind 4 wl_seat v1 -> 7
> wl_display@1.sync 3
> wl_seat@7.get_pointer 8
> wl_seat@7.get_keyboard 9
> wl_compositor@4.create_surface 3
> xdg_wm_base@6.get_xdg_surface 10, surface 3
> xdg_surface@10.get_toplevel 11
> xdg_toplevel@11.set_title \"Camelot\"
> wl_surface@3.commit
> wl_shm@5.create_pool 12, 457600 bytes
> wl_shm_pool@12.create_buffer 13, at 0, 260x220, stride 1040, format 1
> wl_shm_pool@12.create_buffer 14, at 228800, 260x220, stride 1040, format 1
> xdg_surface@10.ack_configure 100
> wl_surface@3.attach 13, 0, 0
> wl_surface@3.damage_buffer 0,0 260x220
> wl_surface@3.frame 15
> wl_surface@3.commit
> xdg_wm_base@6.pong 77
> wl_surface@3.attach 14, 0, 0
> wl_surface@3.damage_buffer 52,30 204x24
> wl_surface@3.damage_buffer 4,168 252x20
> wl_surface@3.frame 15
> wl_surface@3.commit
> wl_surface@3.attach 13, 0, 0
> wl_surface@3.damage_buffer 52,30 204x24
> wl_surface@3.damage_buffer 4,58 252x106
> wl_surface@3.frame 15
> wl_surface@3.commit
> wl_buffer@13.destroy
> wl_buffer@14.destroy
> wl_shm_pool@12.destroy
> xdg_toplevel@11.destroy
> xdg_surface@10.destroy
> wl_surface@3.destroy
frames: 3
guidemo exited 0
";

/// The text the scenes print: the widgets' layout, actions and damage
/// (only what changed is drawn again), the font's size and advance, the
/// damage lists, the keyboard's characters, UTF-8 decoded and encoded.
#[test]
fn the_scenes_print_what_they_did() {
    let run = run();
    assert_eq!(output(&run.out, "guitest wayland /out"), WAYLAND_LOG);
    assert_eq!(
        output(&run.out, "guitest widgets /out"),
        "list 4,58 180x82, 4 lines; ok 164,144 32x24\n\
         first: 0,0 200x172 (bounds 0,0 200x172)\n\
         again: (bounds 0,0 0x0)\n\
         action: field 0\n\
         field: \"Arthur\", cursor 1\n\
         typed: 52,30 144x24 (bounds 52,30 144x24)\n\
         action: list 0\n\
         action: list 1\n\
         list: selected 1, top 3, bar 3\n\
         action: list 5\n\
         list: selected 5\n\
         list: 52,30 144x24 4,58 192x82 (bounds 4,30 192x110)\n\
         action: button OK 0\n\
         action: button Cancel 0\n\
         menu: 96,144 64x24 164,144 32x24 4,4 180x136 (bounds 4,4 192x164)\n\
         action: menu File 2\n\
         closed: 4,4 180x136 (bounds 4,4 180x136)\n\
         menu open: 0\n"
    );
    assert_eq!(
        output(&run.out, "guitest text /out"),
        "font 8x16, 512 glyphs\nadvance 120, width 120\nheight 16, large 32\n"
    );
    assert_eq!(
        output(&run.out, "guitest damage"),
        "empty: (bounds 0,0 0x0)\n\
         apart: 10,10 10x10 40,10 10x10 (bounds 10,10 40x10)\n\
         inside: 40,10 10x10 10,10 10x10 (bounds 10,10 40x10)\n\
         touching: 40,10 10x10 10,10 15x10 (bounds 10,10 40x10)\n\
         bridging: 10,10 40x10 (bounds 10,10 40x10)\n\
         full: 0,0 5x5 20,3 5x5 40,6 5x5 60,9 5x5 80,12 5x5 100,15 5x5 120,18 5x5 140,21 45x11 (bounds 0,0 185x32)\n"
    );
    assert_eq!(
        output(&run.out, "guitest keys"),
        "hello: \"Hello, world!\\n\" mods 0\n\
         caps: \"A1b@C\" mods 2\n\
         caps off: \"d\" mods 0\n\
         ctrl alt: \"z\" mods 0\n\
         two shifts: \"~`\\b\" mods 40\n"
    );
    assert_eq!(
        output(&run.out, "guitest utf8"),
        "utf8 0: 41 e9 20ac 1f600\n\
         utf8 1: fffd fffd fffd fffd fffd fffd fffd fffd\n\
         utf8 2: fffd fffd fffd fffd fffd fffd\n\
         encoded: 41 c3a9 e282ac f09f9880 efbfbd efbfbd\n"
    );
}
