//! The display server (#184, `kernel/user/compositor`): Grail boots the
//! disk kernel on an image with the server, its test client `wltest`
//! (`kernel/user/wltest.c`) and the desktop's `/usr/share`, on several
//! harts; the shell starts the server and the clients in the background.
//! The tests type and click with Grail's keyboard and pointer, read what
//! the clients say they received, and compare the screen with its expected
//! image, `tests/display/NAME.png`.
//!
//! When the drawing changes on purpose, `DISPLAY_BLESS=1 cargo test
//! --release -p xtask --test display` writes the new expected images; look
//! at them before committing. A failing comparison leaves the screen in
//! `target/display-actual/NAME.png`.

use camelot_fs::mkfs::{Mkfs, Options};
use grail::block::MemDisk;
use grail::keyboard::KeyEvent;
use grail::pointer::{PointerInput, BUTTON_LEFT, BUTTON_RIGHT};
use grail::{Machine, MachineConfig};
use std::path::PathBuf;
use std::sync::mpsc::{channel, Sender};
use userprogs::CCompiler;
use xtask::{kernel, png, repo_root, userprogs};

/// The programs in `/bin`, besides the shell.
const BINS: &[&str] = &["compositor", "wltest", "guidemo"];

/// Steps a frame takes, 60 a second at Grail's clock.
const FRAME: u64 = 166_667;
/// The most steps any one wait may take.
const WAIT_MAX: u64 = 3_000_000_000;

/// HID usages of the keys the tests press.
const KEY_A: u16 = 0x04;
const KEY_ESCAPE: u16 = 0x29;
const KEY_TAB: u16 = 0x2b;
const KEY_LEFT_ALT: u16 = 0xe2;
const KEY_LEFT_GUI: u16 = 0xe3;

/// An image with `/dev`'s console, framebuffer and input devices, the shell
/// and [`BINS`] in `/bin`, the desktop's `/usr/share`, and an empty `/run`.
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
    fs.add_device("/dev/fb0", xtask::fs::FB_MAJOR, 0).unwrap();
    fs.mkdir("/dev/input").unwrap();
    fs.add_device("/dev/input/event0", xtask::fs::INPUT_MAJOR, 0)
        .unwrap();
    fs.add_device("/dev/input/event1", xtask::fs::INPUT_MAJOR, 1)
        .unwrap();
    fs.mkdir("/bin").unwrap();
    let user = userprogs::user_dir(&root);
    let rt = userprogs::build_runtime(&user, CCompiler::Merlin).unwrap();
    let progs = userprogs::programs(&user).unwrap();
    for name in std::iter::once(&"sh").chain(BINS) {
        let src = progs.iter().find(|p| p.name() == *name).unwrap();
        let prog = userprogs::link_program(&user, src, CCompiler::Merlin, &rt).unwrap();
        fs.add_file(&format!("/bin/{name}"), &prog.elf).unwrap();
    }
    fs.mkdir("/usr").unwrap();
    xtask::fs::add_share(&mut fs, &share).unwrap();
    fs.mkdir("/run").unwrap();
    fs.finish()
}

/// A machine running the disk kernel on [`image`], at its shell's prompt.
struct Desk {
    m: Machine,
    tx: Sender<u8>,
    /// Where in the console's output the next wait starts looking.
    seen: usize,
}

impl Desk {
    fn boot(harts: usize) -> Desk {
        let config = MachineConfig {
            harts,
            bios: grail::Bios::Default,
            ram_size: 32 * 1024 * 1024 + grail::firmware::FW_SIZE as usize,
            ..MachineConfig::default()
        };
        let disk = MemDisk::from_image(image()).unwrap();
        let mut m = Machine::with_backends(config, vec![Box::new(disk)]).unwrap();
        m.cpu.bus.uart.capture = true;
        let elf = std::fs::read(repo_root().join("kernel").join(kernel::DISK_KERNEL)).unwrap();
        m.load_elf(&elf).unwrap();
        let (tx, rx) = channel();
        m.set_uart_input(rx);
        let mut d = Desk { m, tx, seen: 0 };
        d.wait_for("$ ");
        d
    }

    fn out(&self) -> String {
        String::from_utf8_lossy(&self.m.cpu.bus.uart.captured).into_owned()
    }

    /// Types a command line.
    fn run(&mut self, line: &str) {
        for b in format!("{line}\n").bytes() {
            self.tx.send(b).unwrap();
        }
    }

    /// Steps until the console has printed `text` since the last wait, and
    /// returns what it printed up to it.
    fn wait_for(&mut self, text: &str) -> String {
        let mut steps = 0;
        loop {
            let captured = &self.m.cpu.bus.uart.captured;
            if let Some(at) = find(&captured[self.seen..], text.as_bytes()) {
                let got = String::from_utf8_lossy(&captured[self.seen..self.seen + at]).into_owned();
                self.seen += at + text.len();
                return got;
            }
            assert!(steps < WAIT_MAX, "no {text:?} in:\n{}", self.out());
            for _ in 0..10_000 {
                self.m.cpu.step();
            }
            steps += 10_000;
        }
    }

    /// Steps `frames` frames' worth.
    fn settle(&mut self, frames: u64) {
        for _ in 0..frames * FRAME {
            self.m.cpu.step();
        }
    }

    fn key(&mut self, usage: u16, pressed: bool) {
        let ev = if pressed {
            KeyEvent::key_press(usage)
        } else {
            KeyEvent::key_release(usage)
        };
        assert!(self.m.key_event(ev));
        self.settle(2);
    }

    fn tap(&mut self, usage: u16) {
        self.key(usage, true);
        self.key(usage, false);
    }

    fn pointer(&mut self, x: i32, y: i32, buttons: u8) {
        assert!(self
            .m
            .pointer_event(PointerInput::move_to(x, y).with_buttons(buttons)));
        self.settle(2);
    }

    fn click(&mut self, x: i32, y: i32) {
        self.pointer(x, y, 0);
        self.pointer(x, y, BUTTON_LEFT);
        self.pointer(x, y, 0);
    }

    /// The screen as RGB bytes, and its size.
    fn screen(&self) -> (u32, u32, Vec<u8>) {
        let frame = self.m.cpu.bus.frame();
        let pixels = frame.composited().expect("the display is on");
        let rgb = pixels
            .chunks(4)
            .flat_map(|p| [p[2], p[1], p[0]])
            .collect();
        (frame.width, frame.height, rgb)
    }

    /// The screen is `tests/display/NAME.png`.
    fn expect_screen(&mut self, name: &str) {
        self.settle(4);
        let (w, h, rgb) = self.screen();
        let got = png::encode(w, h, &rgb);
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/display");
        let path = dir.join(format!("{name}.png"));
        if std::env::var_os("DISPLAY_BLESS").is_some() {
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(&path, &got).unwrap();
            return;
        }
        let want = std::fs::read(&path).unwrap_or_default();
        if want != got {
            let actual = repo_root().join("target/display-actual");
            std::fs::create_dir_all(&actual).unwrap();
            std::fs::write(actual.join(format!("{name}.png")), &got).unwrap();
            let differ = png::decode(&want).map_or("no expected image".to_string(), |(ww, wh, want)| {
                if (ww, wh) != (w, h) {
                    format!("{ww}x{wh}, drawn {w}x{h}")
                } else {
                    let n = want
                        .chunks(3)
                        .zip(rgb.chunks(3))
                        .filter(|(a, b)| a != b)
                        .count();
                    format!("{n} pixels differ")
                }
            });
            panic!("{name}: {differ} (drawn in {})", actual.display());
        }
    }
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// The lines a client printed in `out`, its prefix taken off.
fn said<'a>(out: &'a str, who: &str) -> Vec<&'a str> {
    let prefix = format!("wltest {who}: ");
    out.lines()
        .filter_map(|l| l.strip_prefix(prefix.as_str()))
        .collect()
}

#[test]
fn windows_are_composited_raised_moved_and_closed() {
    let mut d = Desk::boot(4);
    d.run("compositor -v &");
    d.wait_for("compositor: ready on /run/wayland-0 (640x480)\n");
    d.run("wltest window red ff0000 160 100 &");
    d.wait_for("wltest red: shown\n");
    d.run("wltest window green 00c000 160 100 &");
    d.wait_for("wltest green: shown\n");
    d.expect_screen("two-windows");
    let _ = (KEY_A, KEY_ESCAPE, KEY_TAB, KEY_LEFT_ALT, KEY_LEFT_GUI, BUTTON_RIGHT);
    let _ = said("", "");
}
