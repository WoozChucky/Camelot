//! `avalon [--listen ADDR] [--max-ram MiB]`

use avalon::{parse_listen, parse_max_ram, Config, Server, ShutdownHandle};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

const USAGE: &str = "usage: avalon [--listen ADDR] [--max-ram MiB]

Runs Grail virtual machines and serves the Avalon protocol (docs/avalon-protocol.md):
Round Table creates and controls the VMs, Herald attaches to their consoles.

  --listen ADDR   where to listen: host:port, or a host or IP address for port 7400
                  (default 127.0.0.1:7400). Protocol v1 has no authentication, so
                  anything but a loopback address lets anyone who can reach it create,
                  control and read VMs, and open files on this host as their disks
  --max-ram MiB   total RAM of all VMs, in MiB (default 4096). Creating a VM that
                  would go over it is refused; destroying a VM gives its RAM back
  -h, --help      show this text

Ctrl-C stops every VM, closes the connections and exits.";

fn main() -> ExitCode {
    let mut config = Config::default();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let parsed = match a.as_str() {
            "-h" | "--help" => {
                println!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            "--listen" => value(&mut args, "--listen", parse_listen).map(|addr| config.listen = addr),
            "--max-ram" => value(&mut args, "--max-ram", parse_max_ram).map(|max| config.max_ram = Some(max)),
            other => Err(format!("unknown option '{other}'\n\n{USAGE}")),
        };
        if let Err(e) = parsed {
            eprintln!("avalon: {e}");
            return ExitCode::from(2);
        }
    }

    let server = match Server::bind(&config) {
        Ok(server) => server,
        Err(e) => {
            eprintln!("avalon: cannot listen on {}: {e}", config.listen);
            return ExitCode::FAILURE;
        }
    };
    on_ctrl_c(server.shutdown_handle());
    let result = server.run();
    SHUT_DOWN.store(true, Ordering::SeqCst);
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("avalon: {e}");
            ExitCode::FAILURE
        }
    }
}

fn value<T>(
    args: &mut impl Iterator<Item = String>,
    flag: &str,
    parse: impl Fn(&str) -> Result<T, String>,
) -> Result<T, String> {
    let s = args.next().ok_or_else(|| format!("{flag} needs a value"))?;
    parse(&s).map_err(|e| format!("{flag}: {e}"))
}

/// Set by the Ctrl-C handler.
static INTERRUPTED: AtomicBool = AtomicBool::new(false);

/// Set once the server has shut down (the Windows close handler waits for it).
#[cfg_attr(not(windows), allow(dead_code))]
static SHUT_DOWN: AtomicBool = AtomicBool::new(false);

/// Shuts the server down on Ctrl-C (and SIGTERM on Unix). The handler only sets a
/// flag, the one thing a Unix signal handler may safely do; a thread watches it. A
/// second Ctrl-C while shutting down exits at once.
fn on_ctrl_c(handle: ShutdownHandle) {
    sys::install();
    std::thread::spawn(move || {
        while !INTERRUPTED.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(50));
        }
        eprintln!("avalon: shutting down");
        handle.shutdown();
    });
}

#[cfg(windows)]
mod sys {
    use super::{INTERRUPTED, SHUT_DOWN};
    use std::sync::atomic::Ordering;
    use std::time::{Duration, Instant};

    type Bool = i32;
    type HandlerRoutine = Option<unsafe extern "system" fn(u32) -> Bool>;

    #[link(name = "kernel32")]
    extern "system" {
        fn SetConsoleCtrlHandler(handler: HandlerRoutine, add: Bool) -> Bool;
    }

    /// CTRL_C_EVENT, CTRL_BREAK_EVENT and CTRL_CLOSE_EVENT (wincon.h).
    const HANDLED: [u32; 3] = [0, 1, CTRL_CLOSE_EVENT];
    const CTRL_CLOSE_EVENT: u32 = 2;

    /// How long the handler holds a closing console window open for the shutdown.
    /// Windows ends the process anyway some 5 s after a close event.
    const CLOSE_GRACE: Duration = Duration::from_millis(4500);

    /// Runs on a thread Windows creates for the event.
    unsafe extern "system" fn on_ctrl(event: u32) -> Bool {
        if !HANDLED.contains(&event) {
            return 0;
        }
        if event == CTRL_CLOSE_EVENT {
            // Windows ends the process as soon as this returns, so wait here for the
            // shutdown the watcher thread starts.
            INTERRUPTED.store(true, Ordering::SeqCst);
            let start = Instant::now();
            while !SHUT_DOWN.load(Ordering::SeqCst) && start.elapsed() < CLOSE_GRACE {
                std::thread::sleep(Duration::from_millis(10));
            }
            return 1;
        }
        if INTERRUPTED.swap(true, Ordering::SeqCst) {
            std::process::exit(130);
        }
        1
    }

    pub fn install() {
        // SAFETY: registers a handler with the signature the API expects.
        unsafe {
            SetConsoleCtrlHandler(Some(on_ctrl), 1);
        }
    }
}

#[cfg(unix)]
mod sys {
    use super::INTERRUPTED;
    use std::sync::atomic::Ordering;

    const SIGINT: i32 = 2;
    const SIGTERM: i32 = 15;

    extern "C" {
        /// `sighandler_t signal(int, sighandler_t)`; a handler is a function pointer.
        fn signal(signum: i32, handler: usize) -> usize;
        fn _exit(status: i32) -> !;
    }

    extern "C" fn on_signal(_: i32) {
        if INTERRUPTED.swap(true, Ordering::SeqCst) {
            // SAFETY: _exit is async-signal-safe.
            unsafe { _exit(130) }
        }
    }

    pub fn install() {
        // SAFETY: installs a handler that only touches an atomic (or calls _exit).
        unsafe {
            signal(SIGINT, on_signal as extern "C" fn(i32) as usize);
            signal(SIGTERM, on_signal as extern "C" fn(i32) as usize);
        }
    }
}

#[cfg(not(any(windows, unix)))]
mod sys {
    pub fn install() {}
}
