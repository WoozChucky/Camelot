//! The host terminal as a machine's console: a thread that feeds a byte stream (stdin)
//! to the UART, and [`RawMode`], which makes an interactive terminal behave like a
//! serial line.
//!
//! A terminal normally edits a whole line itself, echoing it, and hands it over only
//! when Enter is pressed. A guest that does its own line editing (the Camelot kernel's
//! console) needs each key as it is typed instead, and must do the echoing itself, or
//! everything would appear twice. [`RawMode`] switches stdin to that while it lives.
//! Ctrl-C still stops Grail, and the terminal is put back on drop and on Ctrl-C.
//! Herald (`crates/herald`) uses the same guard for a VM's console on an Avalon
//! server, with [`RawOptions::vt`] on.

use std::io::Read;
use std::sync::mpsc::{self, Receiver};

/// Bytes read from `src` on a background thread, until it ends or fails. Hand the
/// receiver to [`crate::Machine::set_uart_input`].
pub fn input_channel(src: impl Read + Send + 'static) -> Receiver<u8> {
    reader(src, None)
}

/// [`input_channel`] reading this process's stdin. For a terminal in raw mode, use
/// [`RawMode::stdin_channel`] instead.
pub fn stdin_channel() -> Receiver<u8> {
    input_channel(std::io::stdin())
}

/// The reader thread behind the channels. An empty read is the end of `src`, unless
/// `empty_read` is a byte: then it stands for that byte and reading goes on.
fn reader(mut src: impl Read + Send + 'static, empty_read: Option<u8>) -> Receiver<u8> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = [0u8; 256];
        loop {
            let n = match src.read(&mut buf) {
                Ok(0) => match empty_read {
                    Some(b) => {
                        buf[0] = b;
                        1
                    }
                    None => return,
                },
                Ok(n) => n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => return,
            };
            for &b in &buf[..n] {
                if tx.send(b).is_err() {
                    return;
                }
            }
        }
    });
    rx
}

/// While alive, stdin delivers each key as soon as it is typed, without the terminal
/// echoing it: Enter arrives as CR (Windows) or LF (Linux), backspace as BS (Windows) or
/// DEL (most Unix terminals). Ctrl-C keeps stopping the process. Dropping it restores
/// the terminal, and so does Ctrl-C.
///
/// Only Windows consoles and Linux terminals are switched; elsewhere, and whenever
/// stdin is not a terminal (a pipe or a file), [`RawMode::enable`] returns `None` and
/// changes nothing.
///
/// Read the terminal through [`RawMode::stdin_channel`], which can only start once
/// raw mode is on: a read already waiting on a Windows console keeps the mode it
/// started in, so the first line would still be edited and echoed by the console.
pub struct RawMode {
    saved: sys::Saved,
}

/// What [`RawMode::with`] switches on besides raw input.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RawOptions {
    /// Make a Windows console a VT terminal, as Unix terminals always are: keys with
    /// no character of their own (arrows, Home, Delete, F1, ...) arrive as their
    /// escape sequences (`ESC [ A` for Up) instead of not at all, and the escape
    /// sequences written to stdout (colours, cursor moves) are carried out instead of
    /// shown. Enter still arrives as CR, Ctrl-C still stops the process. Both modes
    /// are put back with the rest. A console that cannot (before Windows 10) is
    /// switched to plain raw mode. Nothing changes elsewhere.
    pub vt: bool,
}

impl RawMode {
    /// Raw mode with the default [`RawOptions`] (what Grail uses).
    pub fn enable() -> Option<RawMode> {
        RawMode::with(RawOptions::default())
    }

    /// Raw mode with `options`. `None` when stdin is not a terminal (or not one this
    /// platform can switch); nothing has changed then.
    pub fn with(options: RawOptions) -> Option<RawMode> {
        sys::enable(options).map(|saved| RawMode { saved })
    }

    /// [`stdin_channel`] for the terminal in raw mode: every key, as typed, until the
    /// terminal goes away. On Windows a lone Ctrl-Z reads as nothing at all (Rust's
    /// console stdin drops a Ctrl-Z that ends a read), which is not the end of the
    /// input there: it is passed on as the 0x1A it was, the way Ctrl-D reaches the
    /// guest as 0x04 from a raw Linux terminal.
    pub fn stdin_channel(&self) -> Receiver<u8> {
        reader(std::io::stdin(), sys::EMPTY_READ)
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        sys::restore(&self.saved);
    }
}

#[cfg(windows)]
mod sys {
    use super::RawOptions;
    use crate::win32::{
        GetConsoleMode, GetStdHandle, SetConsoleCtrlHandler, SetConsoleMode, BOOL, ENABLE_ECHO_INPUT,
        ENABLE_LINE_INPUT, ENABLE_VIRTUAL_TERMINAL_INPUT, ENABLE_VIRTUAL_TERMINAL_PROCESSING,
        STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
    };
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

    /// The console modes before [`enable`]: stdin's, and stdout's if it was changed.
    pub struct Saved {
        input: u32,
        output: Option<u32>,
    }

    /// What an empty read of the raw console means: a lone Ctrl-Z, removed by Rust's
    /// console stdin (see [`super::RawMode::stdin_channel`]).
    pub const EMPTY_READ: Option<u8> = Some(0x1a);

    /// What the Ctrl-C handler restores. One console per process, so one copy.
    static SAVED_MODE: AtomicU32 = AtomicU32::new(0);
    static ACTIVE: AtomicBool = AtomicBool::new(false);
    static SAVED_OUTPUT_MODE: AtomicU32 = AtomicU32::new(0);
    static OUTPUT_ACTIVE: AtomicBool = AtomicBool::new(false);

    /// The raw input mode: no line editing, no echo, and with `vt` keys as VT
    /// sequences. `ENABLE_PROCESSED_INPUT` stays as it was, so Ctrl-C still raises a
    /// Ctrl-C event instead of reaching the guest.
    pub fn raw(mode: u32, vt: bool) -> u32 {
        let raw = mode & !(ENABLE_LINE_INPUT | ENABLE_ECHO_INPUT);
        if vt {
            raw | ENABLE_VIRTUAL_TERMINAL_INPUT
        } else {
            raw
        }
    }

    /// The output mode with VT sequences carried out.
    pub fn vt_output(mode: u32) -> u32 {
        mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING
    }

    pub fn enable(options: RawOptions) -> Option<Saved> {
        // SAFETY: plain Win32 calls on this process's standard handles. GetConsoleMode
        // fails (returns 0) when the handle is not a console, e.g. a pipe.
        unsafe {
            let h = GetStdHandle(STD_INPUT_HANDLE);
            let mut mode = 0;
            if h.is_null() || GetConsoleMode(h, &mut mode) == 0 {
                return None;
            }
            SAVED_MODE.store(mode, Ordering::SeqCst);
            ACTIVE.store(true, Ordering::SeqCst);
            SetConsoleCtrlHandler(Some(on_ctrl), 1);
            // VT input is refused by consoles that predate it: then plain raw mode.
            let switched = (options.vt && SetConsoleMode(h, raw(mode, true)) != 0)
                || SetConsoleMode(h, raw(mode, false)) != 0;
            if !switched {
                ACTIVE.store(false, Ordering::SeqCst);
                return None;
            }
            let output = if options.vt { enable_vt_output() } else { None };
            Some(Saved { input: mode, output })
        }
    }

    /// Switches stdout to VT processing, if it is a console that can. Returns the mode
    /// to restore.
    fn enable_vt_output() -> Option<u32> {
        // SAFETY: as in `enable`.
        unsafe {
            let h = GetStdHandle(STD_OUTPUT_HANDLE);
            let mut mode = 0;
            if h.is_null() || GetConsoleMode(h, &mut mode) == 0 {
                return None;
            }
            SAVED_OUTPUT_MODE.store(mode, Ordering::SeqCst);
            OUTPUT_ACTIVE.store(true, Ordering::SeqCst);
            if SetConsoleMode(h, vt_output(mode)) == 0 {
                OUTPUT_ACTIVE.store(false, Ordering::SeqCst);
                return None;
            }
            Some(mode)
        }
    }

    pub fn restore(saved: &Saved) {
        ACTIVE.store(false, Ordering::SeqCst);
        OUTPUT_ACTIVE.store(false, Ordering::SeqCst);
        // SAFETY: as in `enable`.
        unsafe {
            SetConsoleMode(GetStdHandle(STD_INPUT_HANDLE), saved.input);
            if let Some(mode) = saved.output {
                SetConsoleMode(GetStdHandle(STD_OUTPUT_HANDLE), mode);
            }
        }
    }

    /// Ctrl-C, Ctrl-Break or the console closing: put the console back, then return
    /// FALSE so the default handler ends the process as usual. Runs on a thread the
    /// system creates for it.
    unsafe extern "system" fn on_ctrl(_event: u32) -> BOOL {
        // SAFETY: as in `enable`.
        unsafe {
            if ACTIVE.swap(false, Ordering::SeqCst) {
                SetConsoleMode(GetStdHandle(STD_INPUT_HANDLE), SAVED_MODE.load(Ordering::SeqCst));
            }
            if OUTPUT_ACTIVE.swap(false, Ordering::SeqCst) {
                SetConsoleMode(
                    GetStdHandle(STD_OUTPUT_HANDLE),
                    SAVED_OUTPUT_MODE.load(Ordering::SeqCst),
                );
            }
        }
        0
    }
}

#[cfg(all(target_os = "linux", any(target_arch = "x86_64", target_arch = "aarch64")))]
mod sys {
    use std::ffi::c_int;
    use std::sync::OnceLock;

    /// `struct termios` of glibc and musl on x86-64 and AArch64 (asm-generic
    /// `termbits.h`): 60 bytes.
    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct Termios {
        pub iflag: u32,
        pub oflag: u32,
        pub cflag: u32,
        pub lflag: u32,
        pub line: u8,
        pub cc: [u8; NCCS],
        pub ispeed: u32,
        pub ospeed: u32,
    }

    const NCCS: usize = 32;
    pub const ICANON: u32 = 0o2;
    pub const ECHO: u32 = 0o10;
    pub const VTIME: usize = 5;
    pub const VMIN: usize = 6;
    const TCSANOW: c_int = 0;
    const STDIN: c_int = 0;
    const SIGHUP: c_int = 1;
    const SIGINT: c_int = 2;
    const SIGTERM: c_int = 15;

    extern "C" {
        fn isatty(fd: c_int) -> c_int;
        fn tcgetattr(fd: c_int, t: *mut Termios) -> c_int;
        fn tcsetattr(fd: c_int, action: c_int, t: *const Termios) -> c_int;
        fn signal(sig: c_int, handler: usize) -> usize;
        fn _exit(code: c_int) -> !;
    }

    pub type Saved = Termios;

    /// A raw terminal's reads wait for at least one byte (`VMIN` 1), so an empty one
    /// is the end: the terminal hung up.
    pub const EMPTY_READ: Option<u8> = None;

    /// What the signal handler restores (set once; the first saved state is the
    /// terminal's real one).
    static SAVED: OnceLock<Termios> = OnceLock::new();

    /// The raw settings: no line editing (`ICANON`), no echo, and reads that return as
    /// soon as one byte is there. Signals (`ISIG`) and output processing stay, so
    /// Ctrl-C still works and `\n` still starts a new line.
    pub fn raw(t: &Termios) -> Termios {
        let mut r = *t;
        r.lflag &= !(ICANON | ECHO);
        r.cc[VMIN] = 1;
        r.cc[VTIME] = 0;
        r
    }

    /// Linux terminals already speak VT both ways, so the options change nothing.
    pub fn enable(_: super::RawOptions) -> Option<Saved> {
        // SAFETY: libc calls on fd 0 with a properly sized termios.
        unsafe {
            if isatty(STDIN) == 0 {
                return None;
            }
            let mut t = std::mem::zeroed::<Termios>();
            if tcgetattr(STDIN, &mut t) != 0 {
                return None;
            }
            let _ = SAVED.set(t);
            let handler = on_signal as extern "C" fn(c_int) as usize;
            for sig in [SIGHUP, SIGINT, SIGTERM] {
                signal(sig, handler);
            }
            if tcsetattr(STDIN, TCSANOW, &raw(&t)) != 0 {
                return None;
            }
            Some(t)
        }
    }

    pub fn restore(saved: &Saved) {
        // SAFETY: as in `enable`.
        unsafe {
            tcsetattr(STDIN, TCSANOW, saved);
        }
    }

    /// Ctrl-C and friends: restore the terminal and exit like the default action would
    /// have (128 + signal). Only async-signal-safe calls.
    extern "C" fn on_signal(sig: c_int) {
        if let Some(t) = SAVED.get() {
            restore(t);
        }
        // SAFETY: _exit is async-signal-safe and ends the process.
        unsafe { _exit(128 + sig) }
    }
}

#[cfg(not(any(
    windows,
    all(target_os = "linux", any(target_arch = "x86_64", target_arch = "aarch64"))
)))]
mod sys {
    pub type Saved = ();

    pub const EMPTY_READ: Option<u8> = None;

    pub fn enable(_: super::RawOptions) -> Option<Saved> {
        None
    }

    pub fn restore(_: &Saved) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_channel_delivers_every_byte_then_disconnects() {
        let data: Vec<u8> = (0..=255).cycle().take(1000).collect();
        let rx = input_channel(std::io::Cursor::new(data.clone()));
        let got: Vec<u8> = rx.iter().collect();
        assert_eq!(got, data);
    }

    /// Reads that return `chunks` in turn, then an error. An `Err` chunk is an
    /// interrupted read.
    struct Chunks(std::collections::VecDeque<Result<&'static [u8], ()>>);

    impl Read for Chunks {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            match self.0.pop_front() {
                Some(Ok(c)) => {
                    buf[..c.len()].copy_from_slice(c);
                    Ok(c.len())
                }
                Some(Err(())) => Err(std::io::ErrorKind::Interrupted.into()),
                None => Err(std::io::ErrorKind::BrokenPipe.into()),
            }
        }
    }

    fn chunks() -> Chunks {
        Chunks([Ok(&b"ab"[..]), Err(()), Ok(b""), Ok(b"c")].into())
    }

    #[test]
    fn an_empty_read_ends_the_input() {
        let got: Vec<u8> = reader(chunks(), None).iter().collect();
        assert_eq!(got, b"ab");
    }

    #[test]
    fn an_empty_read_of_a_raw_console_is_a_key() {
        // A lone Ctrl-Z on Windows: passed on, and reading goes on.
        let got: Vec<u8> = reader(chunks(), Some(0x1a)).iter().collect();
        assert_eq!(got, b"ab\x1ac");
    }

    #[cfg(windows)]
    #[test]
    fn raw_console_mode_drops_line_input_and_echo_only() {
        use crate::win32::{ENABLE_ECHO_INPUT, ENABLE_LINE_INPUT, ENABLE_VIRTUAL_TERMINAL_INPUT};
        const ENABLE_PROCESSED_INPUT: u32 = 0x1;
        const ENABLE_QUICK_EDIT_MODE: u32 = 0x40;
        let cooked = ENABLE_PROCESSED_INPUT | ENABLE_LINE_INPUT | ENABLE_ECHO_INPUT | ENABLE_QUICK_EDIT_MODE;
        assert_eq!(
            sys::raw(cooked, false),
            ENABLE_PROCESSED_INPUT | ENABLE_QUICK_EDIT_MODE
        );
        assert_eq!(
            sys::raw(cooked, true),
            ENABLE_PROCESSED_INPUT | ENABLE_QUICK_EDIT_MODE | ENABLE_VIRTUAL_TERMINAL_INPUT
        );
    }

    #[cfg(windows)]
    #[test]
    fn vt_output_mode_keeps_the_rest() {
        const ENABLE_PROCESSED_OUTPUT: u32 = 0x1;
        const ENABLE_WRAP_AT_EOL_OUTPUT: u32 = 0x2;
        let cooked = ENABLE_PROCESSED_OUTPUT | ENABLE_WRAP_AT_EOL_OUTPUT;
        assert_eq!(sys::vt_output(cooked), cooked | 0x4);
        assert_eq!(sys::vt_output(cooked | 0x4), cooked | 0x4);
    }

    #[cfg(all(target_os = "linux", any(target_arch = "x86_64", target_arch = "aarch64")))]
    #[test]
    fn raw_termios_keeps_signals_and_output() {
        use sys::*;
        assert_eq!(std::mem::size_of::<Termios>(), 60);
        const ISIG: u32 = 0o1;
        const IEXTEN: u32 = 0o100000;
        let mut t: Termios = unsafe { std::mem::zeroed() };
        t.lflag = ISIG | ICANON | ECHO | IEXTEN;
        t.oflag = 0o5; // OPOST | ONLCR
        t.cc[VMIN] = 0;
        t.cc[VTIME] = 3;
        let r = raw(&t);
        assert_eq!(r.lflag, ISIG | IEXTEN);
        assert_eq!(r.oflag, t.oflag);
        assert_eq!((r.cc[VMIN], r.cc[VTIME]), (1, 0));
    }
}
