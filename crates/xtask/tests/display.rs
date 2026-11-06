//! The display server (#184, `kernel/user/compositor`) and the desktop
//! shell (#185: the panel, the terminal and the apps): Grail boots the disk
//! kernel on an image with the server, its test client `wltest`
//! (`kernel/user/wltest.c`), the shell's programs and the desktop's
//! `/usr/share`, on several harts; the shell starts the server and the
//! clients in the background.
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

/// The programs in `/bin`, besides the shell: the server, its clients, and
/// what the terminal's shell runs.
const BINS: &[&str] = &[
    "compositor",
    "wltest",
    "guidemo",
    "term",
    "ls",
    "cat",
    "echo",
    "wc",
    "primes",
    "stty",
    "panel",
    "files",
    "sysmon",
    "gears",
];

/// What `/test/escapes` holds, for the terminal to draw: colours and
/// attributes, erasing, inserting and deleting, the cursor saved, moved and
/// restored, and UTF-8.
const ESCAPES: &[u8] = b"\x1b[1;31mbold red\x1b[0m \x1b[30;42mblack on green\x1b[0m \x1b[7mreverse\x1b[27m \
\x1b[4munderlined\x1b[24m \x1b[94mbright blue\x1b[0m\n\
1234567890ABCDEFGHIJ\x1b[10D\x1b[K <- erased to the end\n\
abcdef\x1b[3D\x1b[2P <- two deleted\n\
abcdef\x1b[3D\x1b[2@XY <- two inserted\n\
\x1b7\x1b[12;50H\x1b[46m at 12, 50 \x1b[0m\x1b8saved and restored\n\
caf\xc3\xa9 \xe2\x94\x8c\xe2\x94\x80\xe2\x94\x90 \xc2\xb1\xc2\xb0\n";

/// Where the machines' clock starts, the time the panel shows: noon of
/// Grail's default day (Thursday 1 January 2026), so that it shows "12:00"
/// for a minute and more of the machine's time.
const NOON: u64 = grail::rtc::DEFAULT_RTC_TIME + 12 * 3600;

/// Where the panel's hart meters are on a machine of four (x, y, width,
/// height): how busy the harts were depends on how long everything took,
/// so screens with the panel leave them out of the comparison.
const PANEL_METERS: (u32, u32, u32, u32) = (476, 0, 36, 24);

/// Steps a frame takes, 60 a second at Grail's clock.
const FRAME: u64 = 166_667;
/// The most steps any one wait may take.
const WAIT_MAX: u64 = 3_000_000_000;

/// HID usages of the keys the tests press.
const KEY_A: u16 = 0x04;
const KEY_ESCAPE: u16 = 0x29;
const KEY_TAB: u16 = 0x2b;
const KEY_PAGE_UP: u16 = 0x4b;
const KEY_LEFT_CTRL: u16 = 0xe0;
const KEY_LEFT_SHIFT: u16 = 0xe1;
const KEY_LEFT_ALT: u16 = 0xe2;
const KEY_LEFT_GUI: u16 = 0xe3;

/// The HID usage of the key that types ASCII `c` on a US keyboard, and
/// whether Shift is held for it.
fn usage_of(c: char) -> (u16, bool) {
    const SHIFTED: &str = "!@#$%^&*()";
    match c {
        'a'..='z' => (0x04 + (c as u16 - 'a' as u16), false),
        'A'..='Z' => (0x04 + (c as u16 - 'A' as u16), true),
        '1'..='9' => (0x1e + (c as u16 - '1' as u16), false),
        '0' => (0x27, false),
        '\n' => (0x28, false),
        ' ' => (0x2c, false),
        '-' => (0x2d, false),
        '_' => (0x2d, true),
        '=' => (0x2e, false),
        '+' => (0x2e, true),
        '\\' => (0x31, false),
        '|' => (0x31, true),
        ';' => (0x33, false),
        ':' => (0x33, true),
        '\'' => (0x34, false),
        '"' => (0x34, true),
        ',' => (0x36, false),
        '<' => (0x36, true),
        '.' => (0x37, false),
        '>' => (0x37, true),
        '/' => (0x38, false),
        '?' => (0x38, true),
        _ => match SHIFTED.find(c) {
            Some(9) => (0x27, true),
            Some(i) => (0x1e + i as u16, true),
            None => panic!("no key types {c:?}"),
        },
    }
}

/// An image with `/dev`'s console, framebuffer, input devices and
/// pseudo-terminals, the shell and [`BINS`] in `/bin`, the terminal's text
/// in `/test`, the desktop's `/usr/share`, and an empty `/run`.
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
    fs.add_device("/dev/ptmx", xtask::fs::PTMX_MAJOR, 0).unwrap();
    fs.mkdir("/dev/pts").unwrap();
    for n in 0..xtask::fs::NPTY {
        fs.add_device(&format!("/dev/pts/{n}"), xtask::fs::PTS_MAJOR, n)
            .unwrap();
    }
    fs.mkdir("/bin").unwrap();
    let user = userprogs::user_dir(&root);
    let rt = userprogs::build_runtime(&user, CCompiler::Merlin).unwrap();
    let progs = userprogs::programs(&user).unwrap();
    for name in std::iter::once(&"sh").chain(BINS) {
        let src = progs.iter().find(|p| p.name() == *name).unwrap();
        let prog = userprogs::link_program(&user, src, CCompiler::Merlin, &rt).unwrap();
        fs.add_file(&format!("/bin/{name}"), &prog.elf).unwrap();
    }
    fs.mkdir("/test").unwrap();
    fs.add_file("/test/escapes", ESCAPES).unwrap();
    let lines: String = (1..=60).map(|n| format!("line {n}\n")).collect();
    fs.add_file("/test/lines", lines.as_bytes()).unwrap();
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
    /// How long the console's output was at the last input.
    input: usize,
}

impl Desk {
    fn boot(harts: usize) -> Desk {
        let config = MachineConfig {
            harts,
            bios: grail::Bios::Default,
            ram_size: 32 * 1024 * 1024 + grail::firmware::FW_SIZE as usize,
            rtc: grail::Rtc::At(NOON),
            ..MachineConfig::default()
        };
        let disk = MemDisk::from_image(image()).unwrap();
        let mut m = Machine::with_backends(config, vec![Box::new(disk)]).unwrap();
        m.cpu.bus.uart.capture = true;
        let elf = std::fs::read(repo_root().join("kernel").join(kernel::DISK_KERNEL)).unwrap();
        m.load_elf(&elf).unwrap();
        let (tx, rx) = channel();
        m.set_uart_input(rx);
        let mut d = Desk {
            m,
            tx,
            seen: 0,
            input: 0,
        };
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
        self.input = self.m.cpu.bus.uart.captured.len();
        assert!(self.m.key_event(ev));
        self.settle(2);
    }

    fn tap(&mut self, usage: u16) {
        self.key(usage, true);
        self.key(usage, false);
    }

    /// Types `text` on the keyboard, a key at a time.
    fn type_text(&mut self, text: &str) {
        for c in text.chars() {
            let (usage, shift) = usage_of(c);
            if shift {
                self.key(KEY_LEFT_SHIFT, true);
            }
            self.tap(usage);
            if shift {
                self.key(KEY_LEFT_SHIFT, false);
            }
        }
    }

    /// Steps until the terminal `title`, started with -v, has committed a
    /// frame with `line` on the cursor's line up to the cursor: what came
    /// before it is on the screen once the server is idle after it.
    fn term_drawn(&mut self, title: &str, line: &str) {
        self.wait_for(&format!("term \"{title}\": drawn \"{line}\"\n"));
        self.since_here();
    }

    /// The next screen waits for the server to be idle after what was
    /// printed so far, not just after the last input: a client's line says
    /// it sent something, which the server may not have drawn yet.
    fn since_here(&mut self) {
        self.input = self.seen;
    }

    /// Types a command line at the terminal `title`'s shell and steps until
    /// its next prompt is drawn.
    fn term_command(&mut self, title: &str, command: &str) {
        self.type_text(command);
        self.term_drawn(title, &format!("$ {command}"));
        self.type_text("\n");
        self.term_drawn(title, "$ ");
    }

    /// The pointer at x, y with `buttons` held (no event if neither
    /// changed).
    fn pointer(&mut self, x: i32, y: i32, buttons: u8) {
        self.input = self.m.cpu.bus.uart.captured.len();
        self.m
            .pointer_event(PointerInput::move_to(x, y).with_buttons(buttons));
        self.settle(2);
    }

    fn click(&mut self, x: i32, y: i32) {
        self.pointer(x, y, 0);
        self.pointer(x, y, BUTTON_LEFT);
        self.pointer(x, y, 0);
    }

    /// Steps until the server has drawn everything it was given: with -v it
    /// says "compositor: idle" once its last frame is on the screen and
    /// nothing is left to draw. The line counts if it comes after the last
    /// input and nothing more of the server's follows it (a line of its
    /// says it acted on something, which it may draw next).
    fn wait_idle(&mut self) {
        const IDLE: &[u8] = b"compositor: idle\n";
        let mut steps = 0;
        loop {
            let captured = &self.m.cpu.bus.uart.captured;
            let idle = (self.input..captured.len().saturating_sub(IDLE.len() - 1))
                .rev()
                .find(|&at| captured[at..].starts_with(IDLE));
            if let Some(at) = idle {
                if find(&captured[at + IDLE.len()..], b"compositor: ").is_none() {
                    return;
                }
            }
            assert!(steps < WAIT_MAX, "the server is never idle:\n{}", self.out());
            for _ in 0..10_000 {
                self.m.cpu.step();
            }
            steps += 10_000;
        }
    }

    /// The screen as RGB bytes, and its size.
    fn screen(&self) -> (u32, u32, Vec<u8>) {
        let frame = self.m.cpu.bus.frame();
        let pixels = frame.composited().expect("the display is on");
        let rgb = pixels.chunks(4).flat_map(|p| [p[2], p[1], p[0]]).collect();
        (frame.width, frame.height, rgb)
    }

    /// The screen is `tests/display/NAME.png`.
    fn expect_screen(&mut self, name: &str) {
        self.expect_screen_but(name, &[]);
    }

    /// The screen is `tests/display/NAME.png`, but for the rectangles
    /// (x, y, width, height) in `masked`, painted magenta in both.
    fn expect_screen_but(&mut self, name: &str, masked: &[(u32, u32, u32, u32)]) {
        self.wait_idle();
        let (w, h, mut rgb) = self.screen();
        for &(mx, my, mw, mh) in masked {
            for y in my..(my + mh).min(h) {
                for x in mx..(mx + mw).min(w) {
                    let i = ((y * w + x) * 3) as usize;
                    rgb[i..i + 3].copy_from_slice(&[0xff, 0x00, 0xff]);
                }
            }
        }
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
                    let n = want.chunks(3).zip(rgb.chunks(3)).filter(|(a, b)| a != b).count();
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

/// The server started, ready for clients.
fn start_server(d: &mut Desk, args: &str) {
    d.run(&format!("compositor -v{args} &"));
    d.wait_for("compositor: ready on /run/wayland-0 (640x480)\n");
}

#[test]
fn windows_are_composited_raised_moved_and_closed() {
    // Two clients on a machine of four harts: each window drawn where it
    // was placed, its title bar and close button the server's, the last
    // one on top with the keyboard (its title bar blue).
    let mut d = Desk::boot(4);
    start_server(&mut d, "");
    d.run("wltest window red ff0000 160 100 &");
    d.wait_for("wltest red: shown\n");
    d.run("wltest window green 00c000 160 100 &");
    d.wait_for("wltest green: shown\n");
    d.expect_screen("two-windows");

    // The keyboard goes to the window on top.
    let from = d.seen;
    d.tap(KEY_A);
    d.wait_for("wltest green: key 30 0\n");
    assert_eq!(said(&d.out()[from..], "green"), ["key 30 1", "key 30 0"]);
    assert_eq!(said(&d.out()[from..], "red"), Vec::<&str>::new());

    // The pointer goes to the surface under it, in its coordinates; a
    // click raises the window under it and gives it the keyboard, and is
    // the client's.
    let from = d.seen;
    d.pointer(60, 100, 0);
    d.click(60, 100);
    d.wait_for("wltest red: button 272 0\n");
    let out = d.out();
    assert_eq!(
        said(&out[from..], "red"),
        [
            "pointer 12 52",
            "focus 1",
            "modifiers 0 0",
            "button 272 1",
            "button 272 0"
        ]
    );
    assert_eq!(said(&out[from..], "green"), ["focus 0"]);
    d.expect_screen("raised");
    let from = d.seen;
    d.tap(KEY_A);
    d.wait_for("wltest red: key 30 0\n");
    assert_eq!(said(&d.out()[from..], "green"), Vec::<&str>::new());

    // Alt+Tab raises the lowest window and gives it the keyboard; Tab
    // never reaches a client, Alt does, as a modifier (Mod1, 8).
    let from = d.seen;
    d.key(KEY_LEFT_ALT, true);
    d.tap(KEY_TAB);
    d.key(KEY_LEFT_ALT, false);
    d.wait_for("wltest green: modifiers 0 0\n");
    let out = d.out();
    assert_eq!(
        said(&out[from..], "red"),
        ["key 56 1", "modifiers 8 0", "focus 0"]
    );
    assert_eq!(
        said(&out[from..], "green"),
        ["focus 1", "modifiers 8 0", "key 56 0", "modifiers 0 0"]
    );

    // A window dragged by its title bar follows the pointer; the client
    // sees the pointer leave and nothing of the drag.
    let from = d.seen;
    d.pointer(150, 70, 0);
    d.pointer(150, 70, BUTTON_LEFT);
    d.wait_for("compositor: moving \"green\"\n");
    d.pointer(220, 150, BUTTON_LEFT);
    d.pointer(300, 250, BUTTON_LEFT);
    d.pointer(300, 250, 0);
    d.expect_screen("moved");
    let out = d.out();
    assert_eq!(said(&out[from..], "red"), ["leave"]);
    assert_eq!(said(&out[from..], "green"), Vec::<&str>::new());

    // The close button asks the client to close its window, which it does.
    d.click(380, 250);
    d.wait_for("wltest green: close\n");
    d.wait_for(") gone\n");
    d.expect_screen("closed");

    // A client that won't close its window is killed.
    d.run("wltest window -n blue 0000ff 120 80 &");
    d.wait_for("wltest blue: shown\n");
    d.click(222, 102);
    d.wait_for("wltest blue: close\n");
    d.wait_for("didn't close its window: killed\n");
    d.expect_screen("killed");

    // The right button on a window's content asks the server to move it
    // (xdg_toplevel.move), as a client's own title bar does.
    d.pointer(100, 100, 0);
    d.pointer(100, 100, BUTTON_RIGHT);
    d.wait_for("compositor: moving \"red\"\n");
    d.pointer(400, 300, BUTTON_RIGHT);
    d.pointer(400, 300, 0);
    d.wait_for("wltest red: button 273 0\n");
    d.expect_screen("client-moved");

    // Escape ends the last client: the background alone.
    d.tap(KEY_ESCAPE);
    d.wait_for(") gone\n");
    d.expect_screen("empty");
}

#[test]
fn a_client_that_breaks_the_protocol_is_disconnected() {
    // Each way of breaking the protocol gets wl_display.error, naming what
    // was wrong, and the hang-up; the server goes on, and a well-behaved
    // client after them gets its window.
    let mut d = Desk::boot(2);
    start_server(&mut d, "");
    d.run("wltest probe");
    let out = d.wait_for("$ ");
    let globals: Vec<&str> = out.lines().filter(|l| l.starts_with("wltest:")).collect();
    assert_eq!(
        globals,
        [
            "wltest: global 1 wl_compositor 4",
            "wltest: global 2 wl_shm 1",
            "wltest: global 3 wl_seat 1",
            "wltest: global 4 wl_output 2",
            "wltest: global 5 xdg_wm_base 1",
            "wltest: global 6 wl_data_device_manager 1",
            "wltest: global 7 zwlr_layer_shell_v1 1",
            "wltest: global 8 zwlr_foreign_toplevel_manager_v1 1",
        ]
    );
    for (kind, error) in [
        ("object", "1 0: invalid object 99"),
        ("opcode", "1 1: invalid method 7 of wl_display@1"),
        ("size", "1 1: a message of a size that can't be"),
        ("newid", "1 0: wl_display.get_registry: invalid new id 2"),
        ("string", "1 1: wl_registry.bind: argument 2 can't be read"),
        ("type", "1 0: wl_surface.attach: invalid object 2"),
        ("trailing", "1 1: wl_display.sync: 4 bytes past its arguments"),
        ("fd", "1 1: wl_shm.create_pool: argument 2 can't be read"),
        ("fds", "1 1: descriptors no message takes"),
        ("pool", "4 2: can't map the pool's memory"),
        ("unsealed", "4 2: the pool's memory may shrink"),
        ("pools", "1 2: too many pools"),
        ("buffer", "5 1: the buffer is past the pool's end"),
    ] {
        d.run(&format!("wltest bad {kind}"));
        let out = d.wait_for("$ ");
        let said: Vec<&str> = out.lines().filter(|l| l.starts_with("wltest:")).collect();
        assert_eq!(
            said,
            [format!("wltest: error {error}").as_str(), "wltest: disconnected"],
            "{kind}:\n{out}"
        );
    }
    d.run("wltest window fine 808080 100 60 &");
    d.wait_for("wltest fine: shown\n");

    // Eight clients at once, and no more: the ninth is refused.
    for name in ["two", "three", "four", "five", "six", "seven", "eight"] {
        d.run(&format!("wltest window {name} 808080 60 40 &"));
        d.wait_for(&format!("wltest {name}: shown\n"));
    }
    d.run("wltest window nine 808080 60 40");
    d.wait_for("compositor: too many clients: one refused\n");
    d.wait_for("wltest nine: can't connect");
}

#[test]
fn a_move_needs_the_press_the_client_was_given() {
    // A client that asks for a move every frame, whatever the pointer does:
    // the server moves it for none of them, not even while a button is
    // held on another window, which goes on getting the pointer.
    let mut d = Desk::boot(2);
    start_server(&mut d, "");
    d.run("wltest window green 00c000 160 100 &");
    d.wait_for("wltest green: shown\n");
    d.run("wltest window -f -m pushy ff00ff 32 32 &");
    d.wait_for("wltest pushy: frame 16\n");
    let from = d.seen;
    d.pointer(60, 130, 0);
    d.pointer(60, 130, BUTTON_LEFT);
    d.pointer(130, 200, BUTTON_LEFT);
    d.pointer(200, 300, BUTTON_LEFT);
    d.wait_for("wltest pushy: frame 48\n");
    d.pointer(200, 300, 0);
    d.wait_for("wltest green: button 272 0\n");
    let out = d.out();
    assert!(!out.contains("compositor: moving"), "{out}");
    let green = said(&out[from..], "green");
    assert_eq!(green.first(), Some(&"pointer 12 82"), "{out}");
    assert!(green.contains(&"button 272 1"), "{out}");
    assert!(green.contains(&"pointer 152 252"), "{out}");
    // Let go of outside it, the pointer leaves it then.
    assert_eq!(green[green.len() - 2..], ["button 272 0", "leave"], "{out}");

    // A client with the press is moved for that press's serial, and not
    // for the one before it.
    d.run("wltest stale &");
    d.wait_for("wltest stale: committed\n");
    d.wait_idle();
    d.pointer(140, 130, 0);
    d.pointer(140, 130, BUTTON_RIGHT);
    let asked = d.wait_for("wltest stale: asked with the serial before\n");
    assert!(!asked.contains("compositor: moving"), "{}", d.out());
    d.wait_for("compositor: moving \"stale\"\n");
    d.pointer(140, 130, 0);
    d.wait_for("wltest stale: released\n");
}

#[test]
fn frames_flip_at_the_vertical_blank_whole() {
    // A client that draws a new colour every frame, as fast as the frame
    // callbacks come: they come once a vertical blank at most, and the
    // screen only ever shows whole frames, one colour or the other, never
    // the old one's rows below the new one's.
    let mut d = Desk::boot(4);
    start_server(&mut d, " -l /bin/guidemo");
    d.run("wltest window -f flash ff0000 64 48 &");
    d.wait_for("wltest flash: frame 16\n");
    let mut steps = 0u64;
    let mut seen = std::collections::BTreeSet::new();
    let mut changes = 0;
    let mut last = 0;
    while find(&d.m.cpu.bus.uart.captured[d.seen..], b"wltest flash: frame 48\n").is_none() {
        for _ in 0..FRAME / 4 {
            d.m.cpu.step();
        }
        steps += FRAME / 4;
        assert!(steps < 4000 * FRAME, "{}", d.out());
        let (w, _, rgb) = d.screen();
        let at = |x: u32, y: u32| {
            let i = ((y * w + x) * 3) as usize;
            u32::from_be_bytes([0, rgb[i], rgb[i + 1], rgb[i + 2]])
        };
        let colour = at(48, 48);
        for y in 48..48 + 48 {
            for x in [48, 79, 111] {
                assert_eq!(at(x, y), colour, "a torn frame at {x}, {y}");
            }
        }
        seen.insert(colour);
        if colour != last {
            changes += 1;
            last = colour;
        }
    }
    d.wait_for("wltest flash: frame 48\n");
    assert_eq!(seen.into_iter().collect::<Vec<_>>(), [0x00ffff, 0xff0000]);
    // 32 frames took 32 vertical blanks at least.
    assert!(steps >= 31 * FRAME, "{steps} steps for 32 frames");
    assert!(changes > 4, "{changes} changes seen");

    // The logo key starts the launcher, the GUI library's demo here.
    d.tap(KEY_LEFT_GUI);
    d.wait_for("compositor: focus \"Camelot\"\n");
}

#[test]
fn the_terminal_runs_a_shell() {
    // The terminal emulator (kernel/user/term.c): a window with the shell
    // on a pseudo-terminal, typed at with the keyboard.
    let mut d = Desk::boot(4);
    start_server(&mut d, "");
    d.run("term -v &");
    d.wait_for("compositor: focus \"Terminal\"\n");
    d.term_drawn("Terminal", "$ ");
    // (`ls /test`, not /bin, whose sizes change with every program.)
    for command in ["echo hello | wc -c", "ls /test", "primes 4 40000"] {
        d.term_command("Terminal", command);
    }
    d.expect_screen("term-shell");
}

/// Where the middle of the terminal's cell at `col`, `row` is on the
/// screen, its window where the server puts the first (its content's top
/// left at 1, 48, the grid 2 pixels in, cells of 8 x 16).
fn cell(col: i32, row: i32) -> (i32, i32) {
    (1 + 2 + col * 8 + 4, 48 + 2 + row * 16 + 8)
}

#[test]
fn the_terminal_draws_escapes_and_scrolls_back() {
    // The VT100's and ANSI's sequences programs write, and the lines that
    // went off the top, read back with Shift+PageUp; a key brings the view
    // back down.
    let mut d = Desk::boot(4);
    start_server(&mut d, "");
    d.run("term -v &");
    d.term_drawn("Terminal", "$ ");
    d.term_command("Terminal", "cat /test/escapes");
    d.expect_screen("term-escapes");

    d.term_command("Terminal", "cat /test/lines");
    d.expect_screen("term-lines");
    d.key(KEY_LEFT_SHIFT, true);
    d.tap(KEY_PAGE_UP);
    d.key(KEY_LEFT_SHIFT, false);
    d.term_drawn("Terminal", "$ ");
    d.expect_screen("term-scrolled-back");
    d.tap(KEY_A);
    d.term_drawn("Terminal", "$ a");
    d.expect_screen("term-back-down");
}

#[test]
fn terminals_resize_and_share_the_clipboard() {
    // A window made shorter by its bottom edge: the grid follows it, and
    // the shell's terminal says its new size (TIOCSWINSZ).
    let mut d = Desk::boot(4);
    start_server(&mut d, "");
    d.run("term -v -T one &");
    d.term_drawn("one", "$ ");
    d.term_command("one", "stty size");
    d.pointer(300, 439, 0);
    d.pointer(300, 439, BUTTON_LEFT);
    d.wait_for("compositor: resizing \"one\"\n");
    d.pointer(300, 359, BUTTON_LEFT);
    d.pointer(300, 279, BUTTON_LEFT);
    d.pointer(300, 279, 0);
    d.term_command("one", "stty size");
    d.term_command("one", "echo camelot");
    d.expect_screen("term-resized");

    // Text selected in one terminal with the pointer is pasted in another
    // with Ctrl+Shift+V.
    let (x0, y) = cell(0, 6);
    let (x1, _) = cell(6, 6);
    d.pointer(x0 - 3, y, 0);
    d.pointer(x0 - 3, y, BUTTON_LEFT);
    d.pointer(x1, y, BUTTON_LEFT);
    d.pointer(x1, y, 0);
    d.run("term -v -T two &");
    d.wait_for("compositor: focus \"two\"\n");
    d.term_drawn("two", "$ ");
    d.key(KEY_LEFT_CTRL, true);
    d.key(KEY_LEFT_SHIFT, true);
    d.tap(0x19); // V
    d.key(KEY_LEFT_SHIFT, false);
    d.key(KEY_LEFT_CTRL, false);
    d.term_drawn("two", "$ camelot");
    d.type_text("\n");
    d.term_drawn("two", "$ ");
    d.expect_screen("term-pasted");
}

/// How many pixels of the screen are mostly `channel` (0 red, 1 green, 2
/// blue): bright in it and dim in the others.
fn pixels_of(rgb: &[u8], channel: usize) -> usize {
    rgb.chunks(3)
        .filter(|p| p[channel] > 0x60 && (0..3).all(|c| c == channel || p[c] < p[channel] / 2))
        .count()
}

#[test]
fn the_panel_launches_lists_and_focuses_windows() {
    // The panel (kernel/user/panel.c) along the top: it starts the program
    // it is given, a terminal, which is placed under it; its launcher's
    // menu lists /usr/share/applications.
    let mut d = Desk::boot(4);
    start_server(&mut d, "");
    d.run("panel -v term -v &");
    d.wait_for("panel: ready, 4 applications\n");
    d.wait_for("panel: started term\n");
    d.term_drawn("Terminal", "$ ");
    d.expect_screen_but("panel", &[PANEL_METERS]);
    // The menu takes the keyboard once it is shown.
    d.click(40, 12);
    d.wait_for("panel: menu shown\n");
    d.wait_for("compositor: focus \"launcher\"\n");
    d.since_here();
    d.expect_screen_but("panel-menu", &[PANEL_METERS]);

    // The launcher starts every application, each a window with the
    // keyboard. The gears turn (glxgears' red, green and blue) until
    // Escape; the system monitor goes with Escape too.
    let apps = [
        ("files", "Files"),
        ("gears", "Gears"),
        ("sysmon", "System Monitor"),
        ("term", "Terminal"),
    ];
    for (i, (name, title)) in apps.into_iter().enumerate() {
        if i > 0 {
            d.click(40, 12);
            d.wait_for("panel: menu shown\n");
            d.wait_for("compositor: focus \"launcher\"\n");
        }
        d.click(60, 24 + 2 + 24 * i as i32 + 12);
        d.wait_for(&format!("panel: started {name}\n"));
        d.wait_for("panel: menu closed\n");
        d.wait_for(&format!("compositor: focus \"{title}\"\n"));
        match name {
            "gears" => {
                let fps = d.wait_for(" FPS\n");
                eprintln!("gears: {}", fps.lines().last().unwrap_or(""));
                let (w, h, rgb) = d.screen();
                for channel in 0..3 {
                    let n = pixels_of(&rgb, channel);
                    if n <= 1500 {
                        let actual = repo_root().join("target/display-actual");
                        std::fs::create_dir_all(&actual).unwrap();
                        std::fs::write(actual.join("gears.png"), png::encode(w, h, &rgb)).unwrap();
                        panic!(
                            "{n} pixels of channel {channel} (drawn in {}):\n{}",
                            actual.display(),
                            d.out()
                        );
                    }
                }
                d.tap(KEY_ESCAPE);
                d.wait_for(") gone\n");
            }
            "sysmon" => {
                d.settle(30);
                d.tap(KEY_ESCAPE);
                d.wait_for(") gone\n");
            }
            _ => {}
        }
    }

    // A button for each window (the first terminal, the files and the
    // second terminal, whose prompt a second is plenty for), the one with
    // the keyboard lit; a click on one raises its window and gives it the
    // keyboard.
    d.settle(60);
    d.since_here();
    d.expect_screen_but("panel-windows", &[PANEL_METERS]);
    d.click(120, 12);
    d.wait_for("compositor: focus \"Terminal\"\n");
    d.since_here();
    d.expect_screen_but("panel-focused", &[PANEL_METERS]);
}
