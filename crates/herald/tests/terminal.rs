//! The `herald` binary on a real terminal, made by the test: a pseudoconsole
//! (ConPTY) on Windows, a pseudoterminal on Linux. The terminal is in raw mode while
//! Herald runs, and its modes are back as they were after Herald ends: by detaching,
//! by losing its server (an error), and by Ctrl-C.
//!
//! On Windows a console's modes can only be read by a process attached to it, so a
//! probe does that: this test binary itself, run in the pseudoconsole as the test
//! `probe` with `HERALD_PROBE` naming a file to write the modes to. A console resets
//! its modes once no process is attached to it, which would hide a mode left behind,
//! so another probe stays attached throughout (the keeper); a control run shows that
//! a mode a process leaves behind is then seen.

mod common;

use common::*;

#[cfg(any(
    windows,
    all(target_os = "linux", any(target_arch = "x86_64", target_arch = "aarch64"))
))]
use avalon_proto::{Connection, VmRef};
#[cfg(any(
    windows,
    all(target_os = "linux", any(target_arch = "x86_64", target_arch = "aarch64"))
))]
use std::net::TcpStream;
#[cfg(any(
    windows,
    all(target_os = "linux", any(target_arch = "x86_64", target_arch = "aarch64"))
))]
use std::sync::mpsc;

/// How Herald is made to leave.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Leave {
    /// The detach key, typed on the terminal.
    Detach,
    /// The server hangs up: Herald ends with an error.
    ServerGone,
    /// Ctrl-C.
    Interrupt,
}

/// A server for one scenario: a real Avalon with an echoing VM, or (for
/// [`Leave::ServerGone`]) a fake one that hangs up when told to.
#[cfg(any(
    windows,
    all(target_os = "linux", any(target_arch = "x86_64", target_arch = "aarch64"))
))]
enum Backend {
    Avalon(avalon::ServerThread, u32),
    Fake {
        addr: String,
        attached: mpsc::Receiver<()>,
        hang_up: mpsc::Sender<()>,
        thread: std::thread::JoinHandle<()>,
    },
}

#[cfg(any(
    windows,
    all(target_os = "linux", any(target_arch = "x86_64", target_arch = "aarch64"))
))]
impl Backend {
    fn new(leave: Leave) -> Backend {
        if leave != Leave::ServerGone {
            let server = server();
            let id = start_vm(&server, "echo", 1, example("uart-echo"));
            return Backend::Avalon(server, id);
        }
        let (attached_tx, attached) = mpsc::channel();
        let (hang_up, hang_up_rx) = mpsc::channel::<()>();
        let (addr, thread) = fake_server(move |mut conn: Connection<TcpStream>| {
            fake_attach(&mut conn, 7, avalon_proto::VmState::Running, b"", &[]);
            attached_tx.send(()).unwrap();
            let _ = hang_up_rx.recv();
        });
        Backend::Fake {
            addr,
            attached,
            hang_up,
            thread,
        }
    }

    fn addr(&self) -> String {
        match self {
            Backend::Avalon(server, _) => server.addr().to_string(),
            Backend::Fake { addr, .. } => addr.clone(),
        }
    }

    fn vm(&self) -> String {
        match self {
            Backend::Avalon(_, id) => id.to_string(),
            Backend::Fake { .. } => "7".into(),
        }
    }

    /// Waits until Herald has attached.
    fn wait_attached(&self) {
        match self {
            Backend::Avalon(server, id) => wait_until("Herald to attach", || consoles(server, *id) == 1),
            Backend::Fake { attached, .. } => attached.recv_timeout(SLOW).unwrap(),
        }
    }

    /// For [`Leave::ServerGone`]: hangs up.
    fn hang_up(&self) {
        if let Backend::Fake { hang_up, .. } = self {
            hang_up.send(()).unwrap();
        }
    }

    /// Checks that a VM is untouched by Herald leaving, and shuts down.
    fn finish(self) {
        match self {
            Backend::Avalon(server, id) => {
                wait_until("the server to let go", || consoles(&server, id) == 0);
                assert_eq!(state(&server, id), avalon_proto::VmState::Running);
                // The VM got what was typed on the terminal.
                let mut check = Herald::attach(&server, VmRef::Id(id));
                assert_eq!(check.scrollback, b"typed");
                check.close_input();
                check.end().unwrap();
                server.stop().unwrap();
            }
            Backend::Fake { thread, .. } => thread.join().unwrap(),
        }
    }
}

#[cfg(windows)]
mod conpty {
    //! Just enough of the pseudoconsole API (consoleapi.h, processthreadsapi.h) to run
    //! a program in a console of the test's making, written by hand like the rest of
    //! Camelot's Win32 code.

    #![allow(clippy::upper_case_acronyms)]

    use std::ffi::c_void;
    use std::fs::File;
    use std::io::{Read, Write};
    use std::os::windows::io::FromRawHandle;
    use std::ptr::{null, null_mut};
    use std::sync::{Arc, Mutex};
    use std::thread::JoinHandle;
    use std::time::Duration;

    type HANDLE = *mut c_void;
    type BOOL = i32;

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct COORD {
        x: i16,
        y: i16,
    }

    #[repr(C)]
    struct STARTUPINFOW {
        cb: u32,
        reserved: *mut u16,
        desktop: *mut u16,
        title: *mut u16,
        x: u32,
        y: u32,
        x_size: u32,
        y_size: u32,
        x_count_chars: u32,
        y_count_chars: u32,
        fill_attribute: u32,
        flags: u32,
        show_window: u16,
        reserved2_len: u16,
        reserved2: *mut u8,
        std_input: HANDLE,
        std_output: HANDLE,
        std_error: HANDLE,
    }

    #[repr(C)]
    struct STARTUPINFOEXW {
        startup_info: STARTUPINFOW,
        attribute_list: *mut c_void,
    }

    #[repr(C)]
    struct PROCESS_INFORMATION {
        process: HANDLE,
        thread: HANDLE,
        process_id: u32,
        thread_id: u32,
    }

    const PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE: usize = 0x0002_0016;
    const EXTENDED_STARTUPINFO_PRESENT: u32 = 0x0008_0000;
    const CREATE_UNICODE_ENVIRONMENT: u32 = 0x0000_0400;
    const STARTF_USESTDHANDLES: u32 = 0x0000_0100;
    const INVALID_HANDLE_VALUE: HANDLE = -1isize as HANDLE;
    const WAIT_OBJECT_0: u32 = 0;
    const STD_INPUT_HANDLE: u32 = -10i32 as u32;
    const STD_OUTPUT_HANDLE: u32 = -11i32 as u32;
    pub const ENABLE_LINE_INPUT: u32 = 0x0002;
    pub const ENABLE_ECHO_INPUT: u32 = 0x0004;
    pub const ENABLE_VIRTUAL_TERMINAL_INPUT: u32 = 0x0200;
    pub const ENABLE_VIRTUAL_TERMINAL_PROCESSING: u32 = 0x0004;
    /// The exit code of a console process ended by Ctrl-C.
    pub const STATUS_CONTROL_C_EXIT: u32 = 0xc000_013a;

    #[link(name = "kernel32")]
    extern "system" {
        fn CreatePipe(read: *mut HANDLE, write: *mut HANDLE, attrs: *const c_void, size: u32) -> BOOL;
        fn CreatePseudoConsole(
            size: COORD,
            input: HANDLE,
            output: HANDLE,
            flags: u32,
            pc: *mut HANDLE,
        ) -> i32;
        fn ClosePseudoConsole(pc: HANDLE);
        fn InitializeProcThreadAttributeList(
            list: *mut c_void,
            count: u32,
            flags: u32,
            size: *mut usize,
        ) -> BOOL;
        fn UpdateProcThreadAttribute(
            list: *mut c_void,
            flags: u32,
            attribute: usize,
            value: *const c_void,
            size: usize,
            previous: *mut c_void,
            returned: *mut usize,
        ) -> BOOL;
        fn DeleteProcThreadAttributeList(list: *mut c_void);
        fn CreateProcessW(
            application: *const u16,
            command_line: *mut u16,
            process_attrs: *const c_void,
            thread_attrs: *const c_void,
            inherit: BOOL,
            flags: u32,
            environment: *const c_void,
            directory: *const u16,
            startup: *const STARTUPINFOW,
            info: *mut PROCESS_INFORMATION,
        ) -> BOOL;
        fn WaitForSingleObject(handle: HANDLE, ms: u32) -> u32;
        fn GetExitCodeProcess(process: HANDLE, code: *mut u32) -> BOOL;
        fn TerminateProcess(process: HANDLE, code: u32) -> BOOL;
        fn CloseHandle(handle: HANDLE) -> BOOL;
        fn GetStdHandle(which: u32) -> HANDLE;
        fn GetConsoleMode(handle: HANDLE, mode: *mut u32) -> BOOL;
        fn SetConsoleMode(handle: HANDLE, mode: u32) -> BOOL;
        fn SetConsoleCtrlHandler(handler: Option<unsafe extern "system" fn(u32) -> BOOL>, add: BOOL) -> BOOL;
    }

    /// Lets Ctrl-C end this process (`ignore` false) or not. Processes inherit the
    /// setting from the one that starts them, and a test runner may have Ctrl-C
    /// off.
    pub fn ignore_ctrl_c(ignore: bool) {
        // SAFETY: a null handler sets or clears the process's ignore flag.
        assert!(unsafe { SetConsoleCtrlHandler(None, ignore as BOOL) } != 0);
    }

    /// This process's console modes (stdin, stdout), if they are a console.
    pub fn my_modes() -> (Option<u32>, Option<u32>) {
        let mode = |which| {
            let mut mode = 0;
            // SAFETY: plain Win32 calls on a standard handle.
            unsafe { (GetConsoleMode(GetStdHandle(which), &mut mode) != 0).then_some(mode) }
        };
        (mode(STD_INPUT_HANDLE), mode(STD_OUTPUT_HANDLE))
    }

    /// Sets this process's console input mode.
    pub fn set_input_mode(mode: u32) {
        // SAFETY: as in `my_modes`.
        assert!(unsafe { SetConsoleMode(GetStdHandle(STD_INPUT_HANDLE), mode) } != 0);
    }

    /// Sets this process's console output mode.
    pub fn set_output_mode(mode: u32) {
        // SAFETY: as in `my_modes`.
        assert!(unsafe { SetConsoleMode(GetStdHandle(STD_OUTPUT_HANDLE), mode) } != 0);
    }

    /// A pseudoconsole, its input, and everything it has output so far.
    pub struct PseudoConsole {
        pc: HANDLE,
        input: File,
        pub output: Arc<Mutex<Vec<u8>>>,
        drain: Option<JoinHandle<()>>,
    }

    fn pipe() -> (HANDLE, HANDLE) {
        let (mut read, mut write) = (null_mut(), null_mut());
        // SAFETY: two out-pointers to handles.
        assert!(unsafe { CreatePipe(&mut read, &mut write, null(), 0) } != 0);
        (read, write)
    }

    impl PseudoConsole {
        /// A new pseudoconsole, or `None` if this Windows has none (before 10 1809).
        pub fn new() -> Option<PseudoConsole> {
            let (in_read, in_write) = pipe();
            let (out_read, out_write) = pipe();
            let mut pc = null_mut();
            // SAFETY: the pipe ends were just made; the pseudoconsole duplicates the
            // ends it keeps, so ours are closed afterwards.
            let hr = unsafe {
                let hr = CreatePseudoConsole(COORD { x: 120, y: 30 }, in_read, out_write, 0, &mut pc);
                CloseHandle(in_read);
                CloseHandle(out_write);
                hr
            };
            // SAFETY: handles we own, each wrapped once.
            let (input, mut out) =
                unsafe { (File::from_raw_handle(in_write), File::from_raw_handle(out_read)) };
            if hr != 0 {
                return None;
            }
            // The pseudoconsole writes everything shown on it here; it must be read,
            // or the programs in it block.
            let output = Arc::new(Mutex::new(Vec::new()));
            let sink = Arc::clone(&output);
            let drain = std::thread::spawn(move || {
                let mut buf = [0u8; 4096];
                while let Ok(n @ 1..) = out.read(&mut buf) {
                    sink.lock().unwrap().extend_from_slice(&buf[..n]);
                }
            });
            Some(PseudoConsole {
                pc,
                input,
                output,
                drain: Some(drain),
            })
        }

        /// Types `bytes` on the console (VT input: 0x03 is Ctrl-C).
        pub fn type_keys(&mut self, bytes: &[u8]) {
            self.input.write_all(bytes).unwrap();
            self.input.flush().unwrap();
        }

        /// Starts `program` with `args` in this console, with `env` added to the
        /// environment.
        pub fn spawn(&self, program: &std::path::Path, args: &[&str], env: &[(&str, &str)]) -> Process {
            let mut line = quote(&program.to_string_lossy());
            for a in args {
                line.push(' ');
                line.push_str(&quote(a));
            }
            let mut line: Vec<u16> = line.encode_utf16().chain([0]).collect();
            let mut block: Vec<u16> = Vec::new();
            let vars = std::env::vars().filter(|(k, _)| !env.iter().any(|(e, _)| e.eq_ignore_ascii_case(k)));
            for (k, v) in vars.chain(env.iter().map(|(k, v)| (k.to_string(), v.to_string()))) {
                block.extend(format!("{k}={v}").encode_utf16());
                block.push(0);
            }
            block.push(0);

            // SAFETY: the attribute list is sized as the first call asks, in a buffer
            // aligned for pointers, and lives until CreateProcessW has returned.
            unsafe {
                let mut size = 0;
                InitializeProcThreadAttributeList(null_mut(), 1, 0, &mut size);
                let mut list = vec![0usize; size.div_ceil(std::mem::size_of::<usize>())];
                let list = list.as_mut_ptr().cast::<c_void>();
                assert!(InitializeProcThreadAttributeList(list, 1, 0, &mut size) != 0);
                // The value is the pseudoconsole handle itself, not a pointer to it.
                assert!(
                    UpdateProcThreadAttribute(
                        list,
                        0,
                        PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE,
                        self.pc,
                        std::mem::size_of::<HANDLE>(),
                        null_mut(),
                        null_mut(),
                    ) != 0
                );
                let mut si: STARTUPINFOEXW = std::mem::zeroed();
                si.startup_info.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
                si.attribute_list = list;
                // Invalid standard handles, so the child cannot inherit this test's
                // (redirected) ones: it gets the pseudoconsole's.
                si.startup_info.flags = STARTF_USESTDHANDLES;
                si.startup_info.std_input = INVALID_HANDLE_VALUE;
                si.startup_info.std_output = INVALID_HANDLE_VALUE;
                si.startup_info.std_error = INVALID_HANDLE_VALUE;
                let mut info: PROCESS_INFORMATION = std::mem::zeroed();
                let ok = CreateProcessW(
                    null(),
                    line.as_mut_ptr(),
                    null(),
                    null(),
                    0,
                    EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT,
                    block.as_ptr().cast(),
                    null(),
                    &si.startup_info,
                    &mut info,
                );
                DeleteProcThreadAttributeList(list);
                assert!(ok != 0, "cannot start {}", program.display());
                CloseHandle(info.thread);
                Process(info.process)
            }
        }
    }

    impl Drop for PseudoConsole {
        fn drop(&mut self) {
            // SAFETY: our handle, closed once. Closing it ends the output pipe.
            unsafe { ClosePseudoConsole(self.pc) };
            if let Some(drain) = self.drain.take() {
                let _ = drain.join();
            }
        }
    }

    /// Quotes a command-line argument the way the C runtime parses it back.
    fn quote(arg: &str) -> String {
        if !arg.is_empty() && !arg.contains([' ', '\t', '"']) {
            return arg.to_string();
        }
        let mut out = String::from('"');
        let mut slashes = 0;
        for c in arg.chars() {
            match c {
                '\\' => slashes += 1,
                '"' => {
                    out.extend(std::iter::repeat_n('\\', slashes * 2 + 1));
                    out.push('"');
                    slashes = 0;
                    continue;
                }
                _ => {}
            }
            if c != '\\' {
                out.extend(std::iter::repeat_n('\\', slashes));
                slashes = 0;
                out.push(c);
            }
        }
        out.extend(std::iter::repeat_n('\\', slashes * 2));
        out.push('"');
        out
    }

    /// A process started in a pseudoconsole.
    pub struct Process(HANDLE);

    impl Process {
        /// Waits for the process to end, up to `limit`. Returns its exit code.
        pub fn wait(&self, limit: Duration) -> Option<u32> {
            // SAFETY: our process handle.
            unsafe {
                if WaitForSingleObject(self.0, limit.as_millis() as u32) != WAIT_OBJECT_0 {
                    return None;
                }
                let mut code = 0;
                assert!(GetExitCodeProcess(self.0, &mut code) != 0);
                Some(code)
            }
        }
    }

    impl Drop for Process {
        fn drop(&mut self) {
            // SAFETY: our process handle, closed once. Ends it if a test failed
            // before it did.
            unsafe {
                TerminateProcess(self.0, 1);
                CloseHandle(self.0);
            }
        }
    }
}

/// Not a test of its own: the probe that runs inside a pseudoconsole (see the module
/// comment). It writes this process's console modes to the file `HERALD_PROBE`
/// names, after setting the input mode to `HERALD_PROBE_SET` if that is given. With
/// `HERALD_PROBE_KEEP` it turns VT processing off for the console's output first,
/// and stays, ignoring Ctrl-C, until it is ended.
#[cfg(windows)]
#[test]
fn probe() {
    let Ok(path) = std::env::var("HERALD_PROBE") else {
        return;
    };
    let keep = std::env::var("HERALD_PROBE_KEEP").is_ok();
    if keep {
        // A pseudoconsole starts with VT processing on; off, Herald has it to switch.
        conpty::set_output_mode(conpty::my_modes().1.unwrap() & !conpty::ENABLE_VIRTUAL_TERMINAL_PROCESSING);
    }
    if let Ok(mode) = std::env::var("HERALD_PROBE_SET") {
        conpty::set_input_mode(mode.parse().unwrap());
    }
    let (input, output) = conpty::my_modes();
    let show = |m: Option<u32>| m.map_or("none".to_string(), |m| m.to_string());
    std::fs::write(path, format!("{} {}", show(input), show(output))).unwrap();
    if keep {
        conpty::ignore_ctrl_c(true);
        std::thread::sleep(SLOW * 10);
    }
}

/// Console modes (input, output) as the probe sees them.
#[cfg(windows)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Modes(u32, u32);

#[cfg(windows)]
fn probe_modes(console: &conpty::PseudoConsole, set_input: Option<u32>) -> Modes {
    let set = set_input.map(|m| m.to_string());
    let env: Vec<(&str, &str)> = set.iter().map(|m| ("HERALD_PROBE_SET", m.as_str())).collect();
    let (probe, path) = start_probe(console, &env);
    assert_eq!(probe.wait(SLOW), Some(0), "the probe failed");
    read_probe(&path)
}

/// Starts the probe in `console` with `env` added. Returns it and its file.
#[cfg(windows)]
fn start_probe(
    console: &conpty::PseudoConsole,
    env: &[(&str, &str)],
) -> (conpty::Process, std::path::PathBuf) {
    static COUNT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = COUNT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let path = std::env::temp_dir().join(format!("herald-probe-{}-{n}.txt", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let mut env = env.to_vec();
    env.push(("HERALD_PROBE", path.to_str().unwrap()));
    let me = std::env::current_exe().unwrap();
    let probe = console.spawn(&me, &["probe", "--exact", "--test-threads=1", "-q"], &env);
    (probe, path)
}

/// A console with a process that stays attached to it, so that it keeps its modes
/// between the processes that come and go: a console with no process left resets
/// them.
#[cfg(windows)]
fn kept_console() -> Option<(conpty::PseudoConsole, conpty::Process)> {
    let console = conpty::PseudoConsole::new()?;
    let (keeper, path) = start_probe(&console, &[("HERALD_PROBE_KEEP", "1")]);
    wait_until("the keeper to start", || path.exists());
    let _ = std::fs::remove_file(&path);
    Some((console, keeper))
}

/// The modes a probe wrote to `path`.
#[cfg(windows)]
fn read_probe(path: &std::path::Path) -> Modes {
    let text = std::fs::read_to_string(path).unwrap();
    let _ = std::fs::remove_file(path);
    let modes: Vec<u32> = text
        .split(' ')
        .map(|m| {
            m.parse()
                .unwrap_or_else(|_| panic!("the probe saw no console: {text}"))
        })
        .collect();
    Modes(modes[0], modes[1])
}

/// Runs Herald in a fresh pseudoconsole and makes it leave by `leave`; checks the
/// console modes before, during and after.
#[cfg(windows)]
fn conpty_session(leave: Leave) {
    use conpty::*;
    let Some((mut console, keeper)) = kept_console() else {
        eprintln!("no pseudoconsole on this Windows: skipped");
        return;
    };
    let before = probe_modes(&console, None);
    assert_ne!(
        before.0 & ENABLE_LINE_INPUT,
        0,
        "a fresh console edits lines: {before:?}"
    );
    assert_eq!(before.1 & ENABLE_VIRTUAL_TERMINAL_PROCESSING, 0, "{before:?}");

    let backend = Backend::new(leave);
    let herald = std::path::Path::new(env!("CARGO_BIN_EXE_herald"));
    // Herald inherits whether Ctrl-C is ignored from this process.
    ignore_ctrl_c(false);
    let process = console.spawn(herald, &["--server", &backend.addr(), &backend.vm()], &[]);
    backend.wait_attached();
    // Raw mode is switched on right after the attach; wait for it.
    let start = std::time::Instant::now();
    let during = loop {
        let modes = probe_modes(&console, None);
        if modes.0 & ENABLE_LINE_INPUT == 0 || start.elapsed() > SLOW {
            break modes;
        }
    };
    assert_eq!(
        during.0 & (ENABLE_LINE_INPUT | ENABLE_ECHO_INPUT),
        0,
        "{during:?}"
    );
    assert_ne!(during.0 & ENABLE_VIRTUAL_TERMINAL_INPUT, 0, "{during:?}");
    assert_ne!(during.1 & ENABLE_VIRTUAL_TERMINAL_PROCESSING, 0, "{during:?}");

    let code = match leave {
        Leave::Detach => {
            console.type_keys(b"typed");
            if let Backend::Avalon(server, id) = &backend {
                let mut watch = Herald::attach(server, VmRef::Id(*id));
                watch.wait_for("typed");
                watch.close_input();
                watch.end().unwrap();
            }
            console.type_keys(&[herald::DETACH_KEY]);
            0
        }
        Leave::ServerGone => {
            backend.hang_up();
            1
        }
        Leave::Interrupt => {
            console.type_keys(b"typed");
            if let Backend::Avalon(server, id) = &backend {
                let mut watch = Herald::attach(server, VmRef::Id(*id));
                watch.wait_for("typed");
                watch.close_input();
                watch.end().unwrap();
            }
            console.type_keys(&[0x03]);
            STATUS_CONTROL_C_EXIT
        }
    };
    let output = || String::from_utf8_lossy(&console.output.lock().unwrap()).into_owned();
    assert_eq!(process.wait(SLOW), Some(code), "{leave:?}; console: {}", output());
    let after = probe_modes(&console, None);
    assert_eq!(after, before, "{leave:?}; console: {}", output());
    drop((process, keeper));
    backend.finish();
}

#[cfg(windows)]
#[test]
fn console_modes_are_restored_after_a_detach() {
    conpty_session(Leave::Detach);
}

#[cfg(windows)]
#[test]
fn console_modes_are_restored_after_an_error() {
    conpty_session(Leave::ServerGone);
}

#[cfg(windows)]
#[test]
fn console_modes_are_restored_after_ctrl_c() {
    conpty_session(Leave::Interrupt);
}

/// The method can fail: a mode a process leaves behind is still there for the next.
#[cfg(windows)]
#[test]
fn a_mode_left_behind_is_seen() {
    use conpty::*;
    let Some((console, _keeper)) = kept_console() else {
        return;
    };
    let before = probe_modes(&console, None);
    let raw = before.0 & !(ENABLE_LINE_INPUT | ENABLE_ECHO_INPUT);
    let set = probe_modes(&console, Some(raw));
    assert_eq!(set.0, raw);
    assert_eq!(probe_modes(&console, None).0, raw);
}

#[cfg(all(target_os = "linux", any(target_arch = "x86_64", target_arch = "aarch64")))]
mod pty {
    //! A pseudoterminal pair, through libc.

    use std::ffi::{c_char, c_int, CStr};
    use std::fs::File;
    use std::os::fd::{AsRawFd, FromRawFd};

    /// `struct termios` on x86-64 and AArch64 Linux: 60 bytes.
    #[repr(C)]
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct Termios {
        pub iflag: u32,
        pub oflag: u32,
        pub cflag: u32,
        pub lflag: u32,
        pub rest: [u8; 44],
    }

    pub const ICANON: u32 = 0o2;
    pub const ECHO: u32 = 0o10;
    const O_RDWR: c_int = 2;
    const O_NOCTTY: c_int = 0o400;
    pub const SIGINT: c_int = 2;

    extern "C" {
        fn posix_openpt(flags: c_int) -> c_int;
        fn grantpt(fd: c_int) -> c_int;
        fn unlockpt(fd: c_int) -> c_int;
        fn ptsname_r(fd: c_int, buf: *mut c_char, len: usize) -> c_int;
        fn tcgetattr(fd: c_int, t: *mut Termios) -> c_int;
        pub fn kill(pid: c_int, sig: c_int) -> c_int;
    }

    /// Opens a pseudoterminal: (master, slave).
    pub fn open() -> (File, File) {
        // SAFETY: libc calls with a buffer of the length given.
        unsafe {
            let master = posix_openpt(O_RDWR | O_NOCTTY);
            assert!(master >= 0);
            assert_eq!(grantpt(master), 0);
            assert_eq!(unlockpt(master), 0);
            let mut name = [0 as c_char; 128];
            assert_eq!(ptsname_r(master, name.as_mut_ptr(), name.len()), 0);
            let path = CStr::from_ptr(name.as_ptr()).to_str().unwrap().to_string();
            let master = File::from_raw_fd(master);
            use std::os::unix::fs::OpenOptionsExt;
            let slave = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .custom_flags(O_NOCTTY)
                .open(path)
                .unwrap();
            (master, slave)
        }
    }

    pub fn attrs(tty: &File) -> Termios {
        // SAFETY: a properly sized termios.
        unsafe {
            let mut t = std::mem::zeroed::<Termios>();
            assert_eq!(tcgetattr(tty.as_raw_fd(), &mut t), 0);
            t
        }
    }
}

/// Runs Herald with a pseudoterminal as stdin and makes it leave by `leave`; checks
/// the terminal's settings before, during and after.
#[cfg(all(target_os = "linux", any(target_arch = "x86_64", target_arch = "aarch64")))]
fn pty_session(leave: Leave) {
    use pty::*;
    use std::io::{Read, Write};
    use std::process::{Command, Stdio};

    assert_eq!(std::mem::size_of::<Termios>(), 60);
    let (mut master, slave) = open();
    let before = attrs(&slave);
    assert_ne!(
        before.lflag & ICANON,
        0,
        "a fresh terminal edits lines: {before:?}"
    );
    // Echoes and output go to the master side, which must be read.
    let mut reader = master.try_clone().unwrap();
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        while let Ok(1..) = reader.read(&mut buf) {}
    });

    let backend = Backend::new(leave);
    let mut child = Command::new(env!("CARGO_BIN_EXE_herald"))
        .args(["--server", &backend.addr(), &backend.vm()])
        .stdin(Stdio::from(slave.try_clone().unwrap()))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    backend.wait_attached();
    wait_until("raw mode", || attrs(&slave).lflag & ICANON == 0);
    let during = attrs(&slave);
    assert_eq!(during.lflag & (ICANON | ECHO), 0, "{during:?}");

    let code = match leave {
        Leave::Detach | Leave::Interrupt => {
            master.write_all(b"typed").unwrap();
            if let Backend::Avalon(server, id) = &backend {
                let mut watch = Herald::attach(server, VmRef::Id(*id));
                watch.wait_for("typed");
                watch.close_input();
                watch.end().unwrap();
            }
            if leave == Leave::Detach {
                master.write_all(&[herald::DETACH_KEY]).unwrap();
                0
            } else {
                // SAFETY: signals our own child.
                assert_eq!(unsafe { kill(child.id() as i32, SIGINT) }, 0);
                128 + SIGINT
            }
        }
        Leave::ServerGone => {
            backend.hang_up();
            1
        }
    };
    let start = std::time::Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        assert!(start.elapsed() < SLOW, "herald is still running ({leave:?})");
        std::thread::sleep(std::time::Duration::from_millis(5));
    };
    assert_eq!(status.code(), Some(code), "{leave:?}");
    assert_eq!(attrs(&slave), before, "{leave:?}");
    backend.finish();
}

#[cfg(all(target_os = "linux", any(target_arch = "x86_64", target_arch = "aarch64")))]
#[test]
fn terminal_is_restored_after_a_detach() {
    pty_session(Leave::Detach);
}

#[cfg(all(target_os = "linux", any(target_arch = "x86_64", target_arch = "aarch64")))]
#[test]
fn terminal_is_restored_after_an_error() {
    pty_session(Leave::ServerGone);
}

#[cfg(all(target_os = "linux", any(target_arch = "x86_64", target_arch = "aarch64")))]
#[test]
fn terminal_is_restored_after_ctrl_c() {
    pty_session(Leave::Interrupt);
}

/// Keeps `Leave` and the helpers used where no terminal test runs.
#[test]
fn leave_kinds() {
    assert_ne!(Leave::Detach, Leave::ServerGone);
    assert_ne!(Leave::ServerGone, Leave::Interrupt);
    let _ = SLOW;
}
