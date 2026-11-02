//! Interactive use of the `grail` command in a real Windows console: keys typed
//! into the console reach the kernel's shell one by one, unechoed by the console,
//! and the console's mode is put back when Grail exits.
//!
//! The test uses the console `cargo test` runs in (or opens one if there is
//! none) and types into it with `WriteConsoleInputW`, so keys typed while it
//! runs would mix in. Windows only: CI runs on Linux, so this is checked by the
//! local Windows test run.
#![cfg(windows)]
#![allow(non_snake_case, clippy::upper_case_acronyms)]

use std::ffi::c_void;
use std::fs::{File, OpenOptions};
use std::io::Read;
use std::os::windows::io::AsRawHandle;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

type HANDLE = *mut c_void;
type BOOL = i32;

const ENABLE_PROCESSED_INPUT: u32 = 0x0001;
const ENABLE_LINE_INPUT: u32 = 0x0002;
const ENABLE_ECHO_INPUT: u32 = 0x0004;
const KEY_EVENT: u16 = 0x0001;
const VK_BACK: u16 = 0x08;
const VK_RETURN: u16 = 0x0d;

/// `INPUT_RECORD` holding a `KEY_EVENT_RECORD` (wincontypes.h): 20 bytes.
#[repr(C)]
struct KeyInputRecord {
    EventType: u16,
    _pad: u16,
    bKeyDown: BOOL,
    wRepeatCount: u16,
    wVirtualKeyCode: u16,
    wVirtualScanCode: u16,
    UnicodeChar: u16,
    dwControlKeyState: u32,
}

#[link(name = "kernel32")]
extern "system" {
    fn AllocConsole() -> BOOL;
    fn GetConsoleMode(h: HANDLE, mode: *mut u32) -> BOOL;
    fn SetConsoleMode(h: HANDLE, mode: u32) -> BOOL;
    fn WriteConsoleInputW(h: HANDLE, records: *const KeyInputRecord, len: u32, written: *mut u32) -> BOOL;
}

fn mode(conin: &File) -> u32 {
    let mut m = 0;
    // SAFETY: a console input handle and a valid out pointer.
    assert_ne!(unsafe { GetConsoleMode(conin.as_raw_handle(), &mut m) }, 0);
    m
}

fn set_mode(conin: &File, m: u32) {
    // SAFETY: a console input handle.
    assert_ne!(unsafe { SetConsoleMode(conin.as_raw_handle(), m) }, 0);
}

/// Types `text` into the console: a key press and release per character.
fn type_keys(conin: &File, text: &str) {
    let mut records = Vec::new();
    for c in text.encode_utf16() {
        let vk = match c {
            0x0d => VK_RETURN,
            0x08 => VK_BACK,
            _ => 0,
        };
        for down in [1, 0] {
            records.push(KeyInputRecord {
                EventType: KEY_EVENT,
                _pad: 0,
                bKeyDown: down,
                wRepeatCount: 1,
                wVirtualKeyCode: vk,
                wVirtualScanCode: 0,
                UnicodeChar: c,
                dwControlKeyState: 0,
            });
        }
    }
    let mut written = 0;
    // SAFETY: `records` holds `len` valid INPUT_RECORDs.
    let ok = unsafe {
        WriteConsoleInputW(
            conin.as_raw_handle(),
            records.as_ptr(),
            records.len() as u32,
            &mut written,
        )
    };
    assert_ne!(ok, 0);
    assert_eq!(written as usize, records.len());
}

#[test]
fn input_record_layout_matches_the_sdk() {
    assert_eq!(std::mem::size_of::<KeyInputRecord>(), 20);
}

/// How long the test waits for grail to answer.
const TIMEOUT_SECS: u64 = 20;

/// A `grail` run reading the console. Dropping it kills grail if it is still
/// running and puts the console's mode back, so a failed assertion leaves
/// neither a grail reading the console nor a raw console behind.
struct Session {
    conin: File,
    original: u32,
    child: Child,
    stdout: Arc<Mutex<Vec<u8>>>,
}

impl Session {
    /// What grail has printed so far.
    fn stdout(&self) -> String {
        String::from_utf8_lossy(&self.stdout.lock().unwrap()).into_owned()
    }

    /// Waits for `done` to hold on the output so far; fails with `what`
    /// after `TIMEOUT_SECS`.
    fn wait_for_output(&self, what: &str, done: impl Fn(&str) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(TIMEOUT_SECS);
        while !done(&self.stdout()) {
            assert!(Instant::now() < deadline, "{what}; stdout:\n{}", self.stdout());
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        // SAFETY: a console input handle. Failure is ignored: this may run
        // while a panic unwinds.
        unsafe { SetConsoleMode(self.conin.as_raw_handle(), self.original) };
    }
}

#[test]
fn kernel_shell_in_a_raw_console() {
    let open = || OpenOptions::new().read(true).write(true).open("CONIN$");
    let conin = open().unwrap_or_else(|_| {
        // No console attached: open one.
        // SAFETY: a plain Win32 call.
        assert_ne!(unsafe { AllocConsole() }, 0, "cannot open a console");
        open().expect("cannot open CONIN$")
    });
    let original = mode(&conin);
    // Start from a normal, line-editing console.
    let cooked = original | ENABLE_PROCESSED_INPUT | ENABLE_LINE_INPUT | ENABLE_ECHO_INPUT;
    set_mode(&conin, cooked);

    let kernel = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../kernel/kernel.elf");
    let mut child = Command::new(env!("CARGO_BIN_EXE_grail"))
        .args(["--max-steps", "2000000000"])
        .arg(&kernel)
        .stdin(conin.try_clone().unwrap())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("cannot run grail");
    let mut pipe = child.stdout.take().unwrap();
    let mut s = Session {
        conin: conin.try_clone().unwrap(),
        original,
        child,
        stdout: Arc::new(Mutex::new(Vec::new())),
    };
    let sink = Arc::clone(&s.stdout);
    let reader = std::thread::spawn(move || {
        let mut buf = [0u8; 256];
        while let Ok(n @ 1..) = pipe.read(&mut buf) {
            sink.lock().unwrap().extend_from_slice(&buf[..n]);
        }
    });

    // Grail turns line input and echo off as soon as it starts.
    let deadline = Instant::now() + Duration::from_secs(10);
    while mode(&conin) & (ENABLE_LINE_INPUT | ENABLE_ECHO_INPUT) != 0 {
        assert!(
            Instant::now() < deadline,
            "grail never switched the console to raw mode"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    // Ctrl-C must keep working.
    assert_ne!(mode(&conin) & ENABLE_PROCESSED_INPUT, 0);

    // Keys arrive one by one, before Enter: the kernel echoes them as they are
    // typed. (Had grail's first read of the console started before raw mode,
    // the console would hold the whole line back until Enter.)
    type_keys(&conin, "ticks");
    s.wait_for_output("the keys never reached the kernel before Enter", |out| {
        out.contains("camelot> ticks")
    });
    // Enter is a lone CR in a raw console.
    type_keys(&conin, "\r");
    s.wait_for_output("no prompt after the first command", |out| {
        out.matches("camelot> ").count() == 2
    });
    // Ctrl-Z on its own is a key like any other, which the line editor
    // ignores, not the end of the input. Give grail time to read it alone.
    type_keys(&conin, "\x1a");
    std::thread::sleep(Duration::from_millis(500));
    // Backspace is BS.
    type_keys(&conin, "halx\x08t\r");
    let deadline = Instant::now() + Duration::from_secs(TIMEOUT_SECS);
    let status = loop {
        if let Some(status) = s.child.try_wait().unwrap() {
            break status;
        }
        assert!(
            Instant::now() < deadline,
            "grail did not halt; stdout:\n{}",
            s.stdout()
        );
        std::thread::sleep(Duration::from_millis(10));
    };
    let restored = mode(&conin);
    reader.join().unwrap();
    let stdout = s.stdout();

    assert_eq!(status.code(), Some(0), "stdout:\n{stdout}");
    let shell = &stdout[stdout.find("camelot> ").expect("no prompt")..];
    let rest = shell
        .strip_prefix("camelot> ticks\nticks: ")
        .unwrap_or_else(|| panic!("{stdout}"));
    let (n, rest) = rest.split_once('\n').unwrap();
    assert!(n.parse::<u32>().unwrap() >= 10, "{stdout}");
    assert_eq!(rest, "camelot> halx\x08 \x08t\n");
    // And the console is back to line editing.
    assert_eq!(restored, cooked);
}
