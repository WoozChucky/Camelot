//! `merlin-dbg`: see [`merlin_dbg::USAGE`].

use std::process::ExitCode;
use std::sync::atomic::AtomicBool;

/// Set on Ctrl-C: a running machine is interrupted, a stopped one ignores it.
static INTERRUPTED: AtomicBool = AtomicBool::new(false);

fn main() -> ExitCode {
    ctrl_c::install();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let stdin = std::io::stdin();
    // Unlocked: the machine's console output, from another thread, goes to stdout too.
    let mut out = std::io::stdout();
    let code = merlin_dbg::run(&args, &mut stdin.lock(), &mut out, &INTERRUPTED);
    ExitCode::from(code as u8)
}

/// Ctrl-C does not end merlin-dbg: it sets [`INTERRUPTED`], which interrupts the
/// machine if it is running (`quit`, or the end of the input, ends merlin-dbg).
#[cfg(windows)]
mod ctrl_c {
    use std::sync::atomic::Ordering;

    type Bool = i32;
    type HandlerRoutine = Option<unsafe extern "system" fn(u32) -> Bool>;
    /// CTRL_C_EVENT and CTRL_BREAK_EVENT (wincon.h).
    const HANDLED: [u32; 2] = [0, 1];

    #[link(name = "kernel32")]
    extern "system" {
        fn SetConsoleCtrlHandler(handler: HandlerRoutine, add: Bool) -> Bool;
    }

    unsafe extern "system" fn on_ctrl(event: u32) -> Bool {
        if !HANDLED.contains(&event) {
            return 0;
        }
        super::INTERRUPTED.store(true, Ordering::SeqCst);
        1
    }

    pub fn install() {
        // SAFETY: registers a handler with the signature the API expects; it only
        // touches an atomic.
        unsafe {
            SetConsoleCtrlHandler(Some(on_ctrl), 1);
        }
    }
}

#[cfg(unix)]
mod ctrl_c {
    use std::sync::atomic::Ordering;

    const SIGINT: i32 = 2;

    extern "C" {
        /// `sighandler_t signal(int, sighandler_t)`; a handler is a function pointer.
        fn signal(signum: i32, handler: usize) -> usize;
    }

    extern "C" fn on_signal(_: i32) {
        super::INTERRUPTED.store(true, Ordering::SeqCst);
    }

    pub fn install() {
        // SAFETY: installs a handler that only touches an atomic.
        unsafe {
            signal(SIGINT, on_signal as extern "C" fn(i32) as usize);
        }
    }
}

#[cfg(not(any(windows, unix)))]
mod ctrl_c {
    pub fn install() {}
}
