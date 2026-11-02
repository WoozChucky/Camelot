//! A native window that shows the framebuffer, through raw Win32 GDI calls, and turns
//! the keys typed into it into keyboard events (spec section 9.4).
//!
//! Single-threaded: the emulator runs on the thread that owns the window and calls
//! `pump` between slices of instructions. The window procedure cannot reach the
//! machine, so it only records what happened (repaint needed, window destroyed, keys)
//! in the window's own [`State`]; the owner redraws and forwards the keys it collects
//! with [`Window::take_keys`]. Windows share nothing, so several can coexist.

use crate::framebuffer::{FB_HEIGHT, FB_WIDTH};
use crate::keyboard::{code, KeyEvent};
use crate::win32::*;
use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::ptr::null_mut;
use std::rc::Rc;
use std::sync::OnceLock;

const CLASS_NAME: &str = "GrailFramebuffer";

/// What the window procedure saw, for the owner to act on. One per window, reached from
/// the window procedure through `GWLP_USERDATA`.
#[derive(Default)]
struct State {
    repaint: Cell<bool>,
    destroyed: Cell<bool>,
    keys: RefCell<Vec<KeyEvent>>,
    /// Key code reported for each virtual key that is down (0 = up, or a key with no
    /// code), so a release carries the same code as its press even if Shift changed.
    down: RefCell<Vec<u8>>,
}

impl State {
    fn new() -> Self {
        State {
            down: RefCell::new(vec![0; 256]),
            ..State::default()
        }
    }

    /// Virtual key `vk` went down (`press`) or up. `char_code` gives the printable
    /// character it types, for keys that are not in the spec's table of special keys.
    fn key(&self, vk: u32, press: bool, shift: bool, ctrl: bool, char_code: impl FnOnce() -> Option<u8>) {
        let mut down = self.down.borrow_mut();
        let Some(slot) = down.get_mut(vk as usize) else {
            return;
        };
        let code = if press {
            // Auto-repeat sends more key-downs: each one is another press of the code
            // the key got when it first went down.
            if *slot == 0 {
                *slot = special_code(vk).or_else(char_code).unwrap_or(0);
            }
            *slot
        } else {
            std::mem::take(slot)
        };
        if code != 0 {
            self.keys.borrow_mut().push(KeyEvent {
                code,
                release: !press,
                shift,
                ctrl,
            });
        }
    }

    /// Focus moved away, so the key-ups will go to another window: release every key the
    /// program saw go down instead of leaving it stuck.
    fn release_all(&self) {
        let mut keys = self.keys.borrow_mut();
        for c in self.down.borrow_mut().iter_mut().filter(|c| **c != 0) {
            keys.push(KeyEvent::release(std::mem::take(c)));
        }
    }
}

/// Spec key code of a key that is not a printable character, from its virtual-key code.
fn special_code(vk: u32) -> Option<u8> {
    Some(match vk {
        VK_RETURN => code::ENTER,
        VK_ESCAPE => code::ESC,
        VK_BACK => code::BACKSPACE,
        VK_TAB => code::TAB,
        VK_UP => code::UP,
        VK_DOWN => code::DOWN,
        VK_LEFT => code::LEFT,
        VK_RIGHT => code::RIGHT,
        VK_F1..=VK_F12 => code::F1 + (vk - VK_F1) as u8,
        _ => return None,
    })
}

fn key_down(vk: u32) -> bool {
    // SAFETY: no pointers involved.
    unsafe { GetKeyState(vk as i32) < 0 }
}

/// The printable ASCII character (0x20-0x7E) the key types with the current Shift and
/// Caps Lock state in the host's keyboard layout, ignoring Ctrl (so Ctrl+C reports 'c',
/// not 0x03). Ctrl+Alt is AltGr on Windows, so Ctrl is kept when Alt is down.
fn printable(vk: u32, scan: u32) -> Option<u8> {
    let mut state = [0u8; 256];
    let mut buf = [0u16; 4];
    // SAFETY: both buffers have the sizes the calls are told.
    let n = unsafe {
        if GetKeyboardState(state.as_mut_ptr()) == 0 {
            return None;
        }
        if state[VK_MENU as usize] & 0x80 == 0 {
            for vk in [VK_CONTROL, VK_LCONTROL, VK_RCONTROL] {
                state[vk as usize] = 0;
            }
        }
        ToUnicode(
            vk,
            scan,
            state.as_ptr(),
            buf.as_mut_ptr(),
            buf.len() as i32,
            TOUNICODE_KEEP_STATE,
        )
    };
    match (n, buf[0]) {
        (1, c @ 0x20..=0x7e) => Some(c as u8),
        _ => None, // dead keys, non-ASCII characters, keys that type nothing
    }
}

/// The window's state, or `None` before `WM_NCCREATE` and after `WM_NCDESTROY`.
///
/// # Safety
/// `hwnd` must be one of our windows: its `GWLP_USERDATA` is 0 or points at the `State`
/// its `Window` keeps alive until the window is destroyed.
unsafe fn state<'a>(hwnd: HWND) -> Option<&'a State> {
    (GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const State).as_ref()
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if msg == WM_NCCREATE {
        let cs = &*(lparam as *const CREATESTRUCTW);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, cs.lpCreateParams as isize);
        return DefWindowProcW(hwnd, msg, wparam, lparam);
    }
    let Some(st) = state(hwnd) else {
        return DefWindowProcW(hwnd, msg, wparam, lparam);
    };
    match msg {
        WM_PAINT => {
            let mut ps: PAINTSTRUCT = std::mem::zeroed();
            BeginPaint(hwnd, &mut ps);
            EndPaint(hwnd, &ps);
            st.repaint.set(true);
            0
        }
        WM_KEYDOWN | WM_KEYUP | WM_SYSKEYDOWN | WM_SYSKEYUP => {
            let vk = wparam as u32;
            let press = matches!(msg, WM_KEYDOWN | WM_SYSKEYDOWN);
            // Ctrl+Alt is AltGr: the character it types is the event, not a Ctrl chord.
            let ctrl = key_down(VK_CONTROL) && !key_down(VK_MENU);
            let scan = (lparam as u32 >> 16) & 0x1ff;
            st.key(vk, press, key_down(VK_SHIFT), ctrl, || printable(vk, scan));
            // Alt chords still reach Windows (Alt+F4 closes the window), except F10,
            // which would put the window into menu mode and swallow the next keys.
            if matches!(msg, WM_SYSKEYDOWN | WM_SYSKEYUP) && vk != VK_F10 {
                return DefWindowProcW(hwnd, msg, wparam, lparam);
            }
            0
        }
        WM_KILLFOCUS => {
            st.release_all();
            0
        }
        WM_DESTROY => {
            st.destroyed.set(true);
            PostQuitMessage(0);
            0
        }
        WM_NCDESTROY => {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

pub struct Window {
    hwnd: HWND,
    open: bool,
    /// Outlives the Win32 window: `Drop` destroys the window before this is released.
    state: Rc<State>,
}

impl Window {
    /// Opens a window whose client area is the framebuffer's size.
    pub fn open(title: &str) -> Result<Window, String> {
        let class = wide(CLASS_NAME);
        // SAFETY: plain Win32 calls with valid, NUL-terminated strings and zeroed structs.
        // The window keeps a pointer to `state`, which lives as long as the `Window`.
        unsafe {
            let hinst = GetModuleHandleW(std::ptr::null());
            static REGISTERED: OnceLock<bool> = OnceLock::new();
            let registered = *REGISTERED.get_or_init(|| {
                let wc = WNDCLASSEXW {
                    cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                    style: CS_HREDRAW | CS_VREDRAW,
                    lpfnWndProc: Some(wndproc),
                    cbClsExtra: 0,
                    cbWndExtra: 0,
                    hInstance: hinst,
                    hIcon: null_mut(),
                    hCursor: LoadCursorW(null_mut(), IDC_ARROW as usize as *const u16),
                    hbrBackground: null_mut(),
                    lpszMenuName: std::ptr::null(),
                    lpszClassName: class.as_ptr(),
                    hIconSm: null_mut(),
                };
                RegisterClassExW(&wc) != 0
            });
            if !registered {
                return Err("RegisterClassExW failed".into());
            }

            let style = WS_OVERLAPPEDWINDOW | WS_VISIBLE;
            let mut r = RECT {
                left: 0,
                top: 0,
                right: FB_WIDTH as i32,
                bottom: FB_HEIGHT as i32,
            };
            AdjustWindowRect(&mut r, style, 0);
            let state = Rc::new(State::new());
            let title = wide(title);
            let hwnd = CreateWindowExW(
                0,
                class.as_ptr(),
                title.as_ptr(),
                style,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                r.right - r.left,
                r.bottom - r.top,
                null_mut(),
                null_mut(),
                hinst,
                Rc::as_ptr(&state) as *mut c_void,
            );
            if hwnd.is_null() {
                return Err("CreateWindowExW failed".into());
            }
            Ok(Window {
                hwnd,
                open: true,
                state,
            })
        }
    }

    /// Handles pending window messages without blocking. Returns false once the
    /// window has been closed.
    pub fn pump(&mut self) -> bool {
        // SAFETY: MSG is plain data; the calls follow the usual message-loop pattern.
        unsafe {
            let mut msg: MSG = std::mem::zeroed();
            while PeekMessageW(&mut msg, null_mut(), 0, 0, PM_REMOVE) != 0 {
                if msg.message == WM_QUIT {
                    self.open = false;
                    break;
                }
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        if self.state.destroyed.get() {
            self.open = false;
        }
        self.open
    }

    /// Blocks until a message arrives (or returns at once if one is queued).
    pub fn wait(&self) {
        // SAFETY: no arguments.
        unsafe {
            WaitMessage();
        }
    }

    /// Returns whether Windows asked for a repaint since the last call.
    pub fn take_repaint(&self) -> bool {
        self.state.repaint.replace(false)
    }

    /// Key events typed into the window since the last call, oldest first.
    pub fn take_keys(&self) -> Vec<KeyEvent> {
        self.state.keys.take()
    }

    pub fn set_title(&self, title: &str) {
        let title = wide(title);
        // SAFETY: valid window handle and NUL-terminated string.
        unsafe {
            SetWindowTextW(self.hwnd, title.as_ptr());
        }
    }

    /// Stretches the frame over the client area. `pixels` is top-down XRGB8888 rows of
    /// `FB_WIDTH` pixels (the VRAM layout); `None` paints the display black.
    pub fn draw(&self, pixels: Option<&[u8]>) {
        if !self.open {
            return;
        }
        const BLACK: [u8; 4] = [0; 4];
        let (bits, w, h): (*const c_void, i32, i32) = match pixels {
            Some(p) => {
                assert_eq!(p.len(), (FB_WIDTH * FB_HEIGHT * 4) as usize);
                (p.as_ptr().cast(), FB_WIDTH as i32, FB_HEIGHT as i32)
            }
            None => (BLACK.as_ptr().cast(), 1, 1),
        };
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB,
                biSizeImage: 0,
                biXPelsPerMeter: 0,
                biYPelsPerMeter: 0,
                biClrUsed: 0,
                biClrImportant: 0,
            },
            bmiColors: [0],
        };
        // SAFETY: `bits` points at w*h 32-bit pixels described by `bmi`; the DC is
        // released before returning.
        unsafe {
            let mut client = RECT::default();
            GetClientRect(self.hwnd, &mut client);
            let hdc = GetDC(self.hwnd);
            if hdc.is_null() {
                return;
            }
            SetStretchBltMode(hdc, COLORONCOLOR);
            StretchDIBits(
                hdc,
                0,
                0,
                client.right - client.left,
                client.bottom - client.top,
                0,
                0,
                w,
                h,
                bits,
                &bmi,
                DIB_RGB_COLORS,
                SRCCOPY,
            );
            ReleaseDC(self.hwnd, hdc);
        }
    }
}

impl Drop for Window {
    fn drop(&mut self) {
        if self.open {
            // SAFETY: the handle is still valid while the window is open.
            unsafe {
                DestroyWindow(self.hwnd);
            }
            // Drain the WM_QUIT this produces so it does not leak into a later window
            // on this thread.
            self.pump();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn special_keys_map_to_spec_codes() {
        assert_eq!(special_code(VK_RETURN), Some(0x0a));
        assert_eq!(special_code(VK_ESCAPE), Some(0x1b));
        assert_eq!(special_code(VK_BACK), Some(0x08));
        assert_eq!(special_code(VK_TAB), Some(0x09));
        assert_eq!(special_code(VK_UP), Some(0x80));
        assert_eq!(special_code(VK_DOWN), Some(0x81));
        assert_eq!(special_code(VK_LEFT), Some(0x82));
        assert_eq!(special_code(VK_RIGHT), Some(0x83));
        for n in 0..12 {
            assert_eq!(special_code(VK_F1 + n), Some(0x90 + n as u8), "F{}", n + 1);
        }
        // Characters go through the keyboard layout; modifiers alone send nothing.
        for vk in [
            VK_SPACE,
            b'A' as u32,
            b'0' as u32,
            VK_SHIFT,
            VK_CONTROL,
            VK_MENU,
            0x7c,
            0xff,
        ] {
            assert_eq!(special_code(vk), None, "vk {vk:#x}");
        }
    }

    #[test]
    fn release_reports_the_code_of_the_press() {
        let st = State::new();
        // Shift+A down, Shift up, A up: the release is still 'A'.
        st.key(b'A' as u32, true, true, false, || Some(b'A'));
        st.key(b'A' as u32, false, false, false, || Some(b'a'));
        assert_eq!(
            st.keys.take(),
            [KeyEvent::press(b'A').with_shift(true), KeyEvent::release(b'A')]
        );
    }

    #[test]
    fn auto_repeat_and_flags() {
        let st = State::new();
        st.key(VK_LEFT, true, false, true, || unreachable!("special key"));
        st.key(VK_LEFT, true, false, true, || unreachable!("special key"));
        st.key(VK_LEFT, false, false, false, || unreachable!("special key"));
        assert_eq!(
            st.keys.take(),
            [
                KeyEvent::press(code::LEFT).with_ctrl(true),
                KeyEvent::press(code::LEFT).with_ctrl(true),
                KeyEvent::release(code::LEFT),
            ]
        );
    }

    #[test]
    fn keys_without_a_code_send_nothing() {
        let st = State::new();
        st.key(VK_SHIFT, true, true, false, || None);
        st.key(0xde, true, false, false, || None); // a dead key, say
        st.key(0xde, false, false, false, || panic!("no lookup on release"));
        st.key(VK_SHIFT, false, false, false, || None);
        // A release with no press seen (key held when the window got focus).
        st.key(b'Q' as u32, false, false, false, || Some(b'q'));
        st.key(0x1_0000, true, false, false, || Some(b'x')); // out of range
        assert!(st.keys.take().is_empty());
    }

    #[test]
    fn losing_focus_releases_held_keys() {
        let st = State::new();
        st.key(b'W' as u32, true, false, false, || Some(b'w'));
        st.key(VK_UP, true, false, false, || None);
        st.key(VK_UP, false, false, false, || None);
        st.key(VK_F1 + 4, true, false, false, || None);
        st.keys.take();
        st.release_all();
        assert_eq!(
            st.keys.take(),
            [KeyEvent::release(b'w'), KeyEvent::release(code::F1 + 4)]
        );
        st.release_all();
        assert!(st.keys.take().is_empty());
    }

    /// Needs a desktop session, so it does not run in CI:
    /// `cargo test -p grail --release -- --ignored window`.
    #[test]
    #[ignore]
    fn window_opens_draws_and_closes() {
        let mut w = Window::open("Grail window test").unwrap();
        let mut pixels = vec![0u8; (FB_WIDTH * FB_HEIGHT * 4) as usize];
        for (i, px) in pixels.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let (x, y) = (i as u32 % FB_WIDTH, i as u32 / FB_WIDTH);
            *px = ((x * 255 / FB_WIDTH) << 16 | (y * 255 / FB_HEIGHT) << 8).to_le_bytes();
        }
        let start = std::time::Instant::now();
        while start.elapsed() < std::time::Duration::from_millis(500) {
            assert!(w.pump());
            w.draw(Some(&pixels));
            std::thread::sleep(std::time::Duration::from_millis(16));
        }
        w.draw(None);
        assert!(w.pump());
        drop(w);
        // A second window on the same thread still works after the first was destroyed.
        let mut w = Window::open("Grail window test 2").unwrap();
        assert!(w.pump());
    }

    /// Key messages sent to a real window come out of `take_keys`, translated, and two
    /// windows keep their own keys. Needs a desktop session (and a keyboard layout that
    /// types 'a' for VK 'A', which every Latin layout does):
    /// `cargo test -p grail --release -- --ignored window`.
    #[test]
    #[ignore]
    fn window_turns_key_messages_into_events() {
        let mut a = Window::open("Grail key test A").unwrap();
        let mut b = Window::open("Grail key test B").unwrap();
        assert!(a.pump() && b.pump());
        a.take_keys();
        b.take_keys();
        // lParam: repeat count 1, scan code in bits 16-23, bits 30-31 set on key-up.
        let send = |w: &Window, msg: u32, vk: u32, scan: isize| {
            let up = if msg == WM_KEYUP {
                0xc000_0000u32 as i32 as isize
            } else {
                0
            };
            // SAFETY: a valid window handle; key messages carry no pointers.
            unsafe { SendMessageW(w.hwnd, msg, vk as WPARAM, 1 | scan << 16 | up) };
        };
        send(&a, WM_KEYDOWN, b'A' as u32, 0x1e);
        send(&a, WM_KEYUP, b'A' as u32, 0x1e);
        send(&b, WM_KEYDOWN, VK_F1 + 1, 0x3c);
        send(&a, WM_KEYDOWN, VK_RETURN, 0x1c);
        send(&a, WM_KEYDOWN, VK_SHIFT, 0x2a);
        send(&a, WM_KEYUP, VK_RETURN, 0x1c);
        send(&a, WM_KEYDOWN, VK_SPACE, 0x39);
        // F10 arrives as a system key; the window must not enter menu mode.
        send(&b, WM_SYSKEYDOWN, VK_F10, 0x44);
        assert!(a.pump() && b.pump());
        assert_eq!(
            a.take_keys(),
            [
                KeyEvent::press(b'a'),
                KeyEvent::release(b'a'),
                KeyEvent::press(code::ENTER),
                KeyEvent::release(code::ENTER),
                KeyEvent::press(b' '),
            ]
        );
        assert_eq!(
            b.take_keys(),
            [KeyEvent::press(code::F1 + 1), KeyEvent::press(code::F1 + 9)]
        );
        assert!(a.take_keys().is_empty());
        // SAFETY: a valid window handle.
        unsafe { SendMessageW(a.hwnd, WM_KILLFOCUS, 0, 0) };
        assert_eq!(a.take_keys(), [KeyEvent::release(b' ')]);
    }
}
