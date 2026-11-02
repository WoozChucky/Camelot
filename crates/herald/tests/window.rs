//! `herald --screen`'s window (Grail's framebuffer window) against an in-process
//! Avalon, on Windows. It needs a desktop session, and the real mouse kept off the
//! window, so it does not run by default:
//! `cargo test -p herald --release -- --ignored window`.

#![cfg(windows)]

mod common;

use avalon_proto::{Connection, Message, VmRef, VmState};
use common::*;
use grail::win32::*;
use herald::screen::ScreenSession;
use herald::window::WindowView;
use herald::End;
use std::ptr::null;
use std::time::{Duration, Instant};

/// Waits up to `limit` for a top-level window titled `title`.
fn find_window(title: &str, limit: Duration) -> Option<usize> {
    let title = wide(title);
    let start = Instant::now();
    loop {
        // SAFETY: a NUL-terminated title; no class filter.
        let h = unsafe { FindWindowW(null(), title.as_ptr()) };
        if !h.is_null() {
            return Some(h as usize);
        }
        if start.elapsed() > limit {
            return None;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// The pointer example on Avalon, in Herald's window: another thread moves the mouse
/// to (100, 50) over the window and presses the left button there, then presses Esc.
/// The guest's cursor turns red there (so the window's mouse reached its pointer
/// device, and the screen came back), and Esc ends it (its keyboard device).
#[test]
#[ignore]
fn window_shows_the_screen_and_sends_input() {
    let server = server();
    let mut rt = connect(&server);
    let id = match rt.request(&Message::CreateVm {
        name: "ptr".into(),
        profile: avalon_proto::HardwareProfile::with_ram(16 * MIB),
        program: example("pointer"),
    }) {
        Ok(Message::VmCreated { id }) => id,
        other => panic!("{other:?}"),
    };
    rt.request_ok(&Message::StartVm { vm: VmRef::Id(id) }).unwrap();

    let conn = Connection::connect(server.addr(), herald::AGENT).unwrap();
    let mut session = ScreenSession::attach(conn, &VmRef::Id(id)).unwrap();
    let title = format!("Herald window test {}", std::process::id());
    let mut view = WindowView::open(&title).unwrap();

    let user = std::thread::spawn(move || {
        let hwnd = find_window(&title, Duration::from_secs(10)).expect("window") as HWND;
        // Let the first frames arrive, so the click lands on a drawn screen.
        std::thread::sleep(Duration::from_millis(300));
        let at = (100 | 50 << 16) as LPARAM; // the window is 640x480: 1:1
        for (msg, wparam, lparam) in [
            (WM_MOUSEMOVE, 0, at),
            (WM_LBUTTONDOWN, MK_LBUTTON, at),
            (WM_KEYDOWN, 0x1b, (1 | 0x01 << 16) as LPARAM),
        ] {
            // SAFETY: posting mouse and key messages carries no pointers.
            unsafe { PostMessageW(hwnd, msg, wparam, lparam) };
        }
    });

    // Typed keys carry this thread's Shift and Ctrl state, so start with none held.
    // SAFETY: a 256-byte key state table.
    unsafe { SetKeyboardState([0u8; 256].as_ptr()) };
    let end = session.run(&mut view).unwrap();
    user.join().unwrap();
    assert_eq!(end, End::VmEnded(VmState::Exited(0)));
    assert_eq!(session.screen.pixel(100, 50), 0xff0000, "the cursor's tip, red");
    assert_eq!(
        session.screen.pixel(0, 0),
        0x203048,
        "background where it started"
    );
    drop(view);
    server.stop().unwrap();
}
