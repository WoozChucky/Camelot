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
use std::os::windows::io::AsRawHandle;
use std::path::Path;
use std::process::{Command, Stdio};
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
    let child = Command::new(env!("CARGO_BIN_EXE_grail"))
        .args(["--max-steps", "2000000000"])
        .arg(&kernel)
        .stdin(conin.try_clone().unwrap())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("cannot run grail");

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

    // Enter is a lone CR in a raw console; backspace is BS.
    type_keys(&conin, "ticks\rhalx\x08t\r");
    let out = child.wait_with_output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let restored = mode(&conin);
    set_mode(&conin, original);

    assert_eq!(out.status.code(), Some(0), "stdout:\n{stdout}");
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
