//! A native window that shows the framebuffer, through raw Win32 GDI calls, and turns
//! the keys typed into it into keyboard events (spec section 9.4) and the mouse over it
//! into pointer input (section 9.7).
//!
//! Single-threaded: the emulator runs on the thread that owns the window and calls
//! `pump` between slices of instructions. The window procedure cannot reach the
//! machine, so it only records what happened (repaint needed, window destroyed, keys,
//! mouse) in the window's own [`State`]; the owner redraws and forwards the keys and
//! pointer input it collects with [`Window::take_keys`] and [`Window::take_pointer`].
//!
//! Nothing here is shared between windows: each one finds its state through its
//! `GWLP_USERDATA` slot, and closing one does not post a thread-wide `WM_QUIT`, so a
//! host can show several machines' framebuffers at once. The only process-wide thing
//! is the window class, which Windows registers once per process anyway.

use crate::framebuffer::{FB_HEIGHT, FB_WIDTH};
use crate::keyboard::{code, KeyEvent};
use crate::pointer::{Motion, PointerInput, BUTTON_LEFT, BUTTON_MIDDLE, BUTTON_RIGHT};
use crate::win32::*;
use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::ptr::null_mut;
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
    pointer: RefCell<Vec<PointerInput>>,
    /// Buttons the last pointer input said were held, to release them if the window
    /// loses the mouse in the middle of a click.
    buttons: Cell<u8>,
    /// Wheel motion not yet reported: less than a notch (precise touchpads and wheels
    /// send fractions of `WHEEL_DELTA`).
    wheel: Cell<i32>,
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

impl State {
    /// The mouse moved to framebuffer pixel `pos` (`None`: it did not move), the
    /// buttons held are now `buttons`, and the wheel turned `wheel` Windows units
    /// (`WHEEL_DELTA` per notch, positive away from the user).
    fn mouse(&self, pos: Option<(i32, i32)>, buttons: u8, wheel: i32) {
        let total = self.wheel.get().saturating_add(wheel);
        let notches = total / WHEEL_DELTA;
        self.wheel.set(total - notches * WHEEL_DELTA);
        if wheel != 0 && notches == 0 && pos.is_none() && buttons == self.buttons.get() {
            return; // a fraction of a notch, nothing else
        }
        let motion = match pos {
            Some((x, y)) => Motion::To { x, y },
            None => Motion::None,
        };
        self.buttons.set(buttons);
        self.pointer.borrow_mut().push(PointerInput {
            motion,
            buttons,
            wheel: notches,
        });
    }

    /// The window lost the mouse (capture or focus went elsewhere), so the button-ups
    /// will go to another window: release the buttons the program saw go down.
    fn release_buttons(&self) {
        if self.buttons.get() != 0 {
            self.mouse(None, 0, 0);
        }
    }
}

/// The pointer buttons held, from a mouse message's `wParam` (`MK_*` flags).
fn mk_buttons(wparam: WPARAM) -> u8 {
    let flag = |mk: usize, button: u8| if wparam & mk != 0 { button } else { 0 };
    flag(MK_LBUTTON, BUTTON_LEFT) | flag(MK_RBUTTON, BUTTON_RIGHT) | flag(MK_MBUTTON, BUTTON_MIDDLE)
}

/// The framebuffer pixel shown at point (`x`, `y`) of a client area `width` × `height`
/// pixels, which the frame is stretched over (`Window::draw`). Points outside the client
/// area (a drag that leaves the window) give pixels outside the framebuffer; the pointer
/// device clamps them. `None` while the client area is empty (minimized).
fn client_to_fb(x: i32, y: i32, width: i32, height: i32) -> Option<(i32, i32)> {
    if width <= 0 || height <= 0 {
        return None;
    }
    // Rounded down, as the stretch maps whole pixels: client pixel `x` shows frame
    // pixel floor(x * FB_WIDTH / width).
    let scale = |v: i32, fb: u32, client: i32| (v as i64 * fb as i64).div_euclid(client as i64) as i32;
    Some((scale(x, FB_WIDTH, width), scale(y, FB_HEIGHT, height)))
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
        // The first message: remember the state `Window::open` passed to CreateWindowExW.
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
            // Alt chords still reach Windows, so Alt+F4 closes the window (menu mode
            // is stopped at WM_SYSCOMMAND below).
            if matches!(msg, WM_SYSKEYDOWN | WM_SYSKEYUP) {
                return DefWindowProcW(hwnd, msg, wparam, lparam);
            }
            0
        }
        // A lone Alt tap or F10 asks for keyboard menu mode, a modal loop inside
        // DispatchMessageW: the emulator would stop until a key ends it, and that key
        // would be swallowed. Refuse it; Alt+Space (lParam ' ') still opens the menu.
        WM_SYSCOMMAND if wparam & 0xfff0 == SC_KEYMENU && lparam == 0 => 0,
        WM_MOUSEMOVE | WM_LBUTTONDOWN | WM_LBUTTONUP | WM_RBUTTONDOWN | WM_RBUTTONUP | WM_MBUTTONDOWN
        | WM_MBUTTONUP => {
            // Client coordinates, signed (negative left of or above the client area
            // while the mouse is captured).
            let (x, y) = (lparam as u16 as i16 as i32, (lparam >> 16) as u16 as i16 as i32);
            let mut client = RECT::default();
            GetClientRect(hwnd, &mut client);
            let pos = client_to_fb(x, y, client.right - client.left, client.bottom - client.top);
            let buttons = mk_buttons(wparam);
            if pos.is_some() || buttons != st.buttons.get() {
                st.mouse(pos, buttons, 0);
            }
            // While a button is held, keep getting the mouse outside the window too, so
            // a drag follows it to the edge and its release is not lost.
            if buttons != 0 {
                if GetCapture() != hwnd {
                    SetCapture(hwnd);
                }
            } else if GetCapture() == hwnd {
                ReleaseCapture();
            }
            0
        }
        WM_MOUSEWHEEL => {
            // The position in lParam is in screen coordinates, and the pointer has not
            // moved: report the buttons and the wheel only.
            st.mouse(None, mk_buttons(wparam), (wparam >> 16) as u16 as i16 as i32);
            0
        }
        WM_CAPTURECHANGED => {
            st.release_buttons();
            0
        }
        WM_KILLFOCUS => {
            st.release_all();
            st.release_buttons();
            0
        }
        WM_DESTROY => {
            st.destroyed.set(true);
            0
        }
        WM_NCDESTROY => {
            // The last message this window gets.
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

pub struct Window {
    hwnd: HWND,
    /// Boxed so its address, stored in the window, stays put when the `Window` moves.
    state: Box<State>,
}

impl Window {
    /// Opens a window whose client area is the framebuffer's size.
    pub fn open(title: &str) -> Result<Window, String> {
        let class = wide(CLASS_NAME);
        let state = Box::new(State::new());
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
                (&*state as *const State).cast_mut().cast(),
            );
            if hwnd.is_null() {
                return Err("CreateWindowExW failed".into());
            }
            Ok(Window { hwnd, state })
        }
    }

    fn is_open(&self) -> bool {
        !self.state.destroyed.get()
    }

    /// Handles pending window messages without blocking (for every window of this
    /// thread: each one's procedure updates its own state). Returns false once this
    /// window has been closed.
    pub fn pump(&mut self) -> bool {
        // SAFETY: MSG is plain data; the calls follow the usual message-loop pattern.
        unsafe {
            let mut msg: MSG = std::mem::zeroed();
            while PeekMessageW(&mut msg, null_mut(), 0, 0, PM_REMOVE) != 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        self.is_open()
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

    /// What the mouse did over the window since the last call, oldest first, with
    /// positions in framebuffer pixels.
    pub fn take_pointer(&self) -> Vec<PointerInput> {
        self.state.pointer.take()
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
        if !self.is_open() {
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
        if self.is_open() {
            // SAFETY: the handle is still valid while the window is open. DestroyWindow
            // sends WM_DESTROY and WM_NCDESTROY before returning, so the procedure has
            // let go of `state` by the time the box is dropped.
            unsafe {
                DestroyWindow(self.hwnd);
            }
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

    #[test]
    fn client_points_map_to_framebuffer_pixels() {
        // The window's usual size: one to one.
        assert_eq!(client_to_fb(0, 0, 640, 480), Some((0, 0)));
        assert_eq!(client_to_fb(639, 479, 640, 480), Some((639, 479)));
        assert_eq!(client_to_fb(123, 45, 640, 480), Some((123, 45)));
        // Stretched to twice the size: two client pixels per frame pixel.
        assert_eq!(client_to_fb(1, 1, 1280, 960), Some((0, 0)));
        assert_eq!(client_to_fb(2, 3, 1280, 960), Some((1, 1)));
        assert_eq!(client_to_fb(1279, 959, 1280, 960), Some((639, 479)));
        // Shrunk, and stretched unevenly.
        assert_eq!(client_to_fb(319, 239, 320, 240), Some((638, 478)));
        assert_eq!(client_to_fb(100, 100, 1000, 200), Some((64, 240)));
        // Outside the client area (a captured drag): outside the frame, rounded down,
        // for the device to clamp.
        assert_eq!(client_to_fb(-1, -3, 1280, 960), Some((-1, -2)));
        assert_eq!(client_to_fb(1280, 2000, 1280, 960), Some((640, 1000)));
        // Minimized.
        assert_eq!(client_to_fb(5, 5, 0, 0), None);
        assert_eq!(client_to_fb(5, 5, 640, 0), None);
    }

    #[test]
    fn mouse_message_buttons() {
        assert_eq!(mk_buttons(0), 0);
        assert_eq!(mk_buttons(MK_LBUTTON), BUTTON_LEFT);
        assert_eq!(mk_buttons(MK_RBUTTON), BUTTON_RIGHT);
        assert_eq!(mk_buttons(MK_MBUTTON), BUTTON_MIDDLE);
        // Shift (0x4), Ctrl (0x8) and the X buttons (0x20, 0x40) are not pointer buttons.
        assert_eq!(mk_buttons(0x7f), BUTTON_LEFT | BUTTON_RIGHT | BUTTON_MIDDLE);
        assert_eq!(mk_buttons(0x6c), 0);
    }

    #[test]
    fn mouse_input_and_wheel_notches() {
        let st = State::new();
        st.mouse(Some((10, 20)), 0, 0);
        st.mouse(Some((11, 20)), BUTTON_LEFT, 0);
        st.mouse(None, BUTTON_LEFT, WHEEL_DELTA);
        st.mouse(None, BUTTON_LEFT, -2 * WHEEL_DELTA);
        // Fractions of a notch add up; on their own they send nothing.
        st.mouse(None, BUTTON_LEFT, 40);
        st.mouse(None, BUTTON_LEFT, 40);
        st.mouse(None, BUTTON_LEFT, 50);
        st.mouse(None, BUTTON_LEFT, -10);
        st.mouse(None, BUTTON_LEFT, -100);
        // A fraction with a button change still reports the button.
        st.mouse(None, 0, -30);
        assert_eq!(
            st.pointer.take(),
            [
                PointerInput::move_to(10, 20),
                PointerInput::move_to(11, 20).with_buttons(BUTTON_LEFT),
                PointerInput::buttons(BUTTON_LEFT).with_wheel(1),
                PointerInput::buttons(BUTTON_LEFT).with_wheel(-2),
                PointerInput::buttons(BUTTON_LEFT).with_wheel(1),
                PointerInput::buttons(0).with_wheel(-1),
            ]
        );
        assert_eq!(st.wheel.get(), -10, "the rest waits for the next notch");
    }

    #[test]
    fn losing_the_mouse_releases_held_buttons() {
        let st = State::new();
        st.mouse(Some((1, 2)), BUTTON_RIGHT | BUTTON_MIDDLE, 0);
        st.pointer.take();
        st.release_buttons();
        assert_eq!(st.pointer.take(), [PointerInput::buttons(0)]);
        st.release_buttons();
        assert!(st.pointer.take().is_empty(), "nothing held, nothing to release");
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

    /// Two machines' windows on one thread: each has its own state, and closing one
    /// leaves the other open. Needs a desktop session, like the test above.
    #[test]
    #[ignore]
    fn windows_are_independent() {
        let mut a = Window::open("Grail window A").unwrap();
        let mut b = Window::open("Grail window B").unwrap();
        assert!(a.pump() && b.pump());
        // Both got their first WM_PAINT, each in its own state.
        assert!(a.take_repaint());
        assert!(!a.take_repaint());
        assert!(b.take_repaint());

        // The user closes A (WM_CLOSE -> DestroyWindow, as the close button does).
        // SAFETY: A's handle is valid until it is destroyed.
        unsafe {
            DestroyWindow(a.hwnd);
        }
        assert!(!a.pump(), "A is closed");
        assert!(b.pump(), "B is still open");
        b.draw(None);
        drop(a);
        assert!(b.pump());
        drop(b);
    }

    /// Sets the calling thread's key state, which `GetKeyState` and `GetKeyboardState`
    /// read, to exactly `held` down. Sent messages do not change it, so tests hold the
    /// modifiers here, as real input would.
    fn hold_keys(held: &[u32]) {
        let mut state = [0u8; 256];
        for &vk in held {
            state[vk as usize] = 0x80;
        }
        // SAFETY: a 256-byte key state table.
        assert!(unsafe { SetKeyboardState(state.as_ptr()) } != 0);
    }

    /// The thread's key state, put back when dropped.
    struct SavedKeys([u8; 256]);

    impl SavedKeys {
        fn save() -> Self {
            let mut state = [0u8; 256];
            // SAFETY: a 256-byte key state table.
            unsafe { GetKeyboardState(state.as_mut_ptr()) };
            SavedKeys(state)
        }
    }

    impl Drop for SavedKeys {
        fn drop(&mut self) {
            // SAFETY: a 256-byte key state table.
            unsafe { SetKeyboardState(self.0.as_ptr()) };
        }
    }

    /// lParam of a key message: repeat count 1, the scan code in bits 16-23, bit 24 for
    /// the arrows (extended keys), bit 29 while Alt is down, bits 30-31 on key-up.
    fn key_lparam(msg: u32, vk: u32, scan: u32) -> LPARAM {
        let mut l = 1 | scan << 16;
        if (VK_LEFT..=VK_DOWN).contains(&vk) {
            l |= 1 << 24;
        }
        if vk == VK_MENU || key_down(VK_MENU) {
            l |= 1 << 29;
        }
        if matches!(msg, WM_KEYUP | WM_SYSKEYUP) {
            l |= 0xc000_0000;
        }
        l as i32 as LPARAM
    }

    /// Hands a key message to the window procedure at once.
    fn send(w: &Window, msg: u32, vk: u32, scan: u32) {
        // SAFETY: a valid window handle; key messages carry no pointers.
        unsafe { SendMessageW(w.hwnd, msg, vk as WPARAM, key_lparam(msg, vk, scan)) };
    }

    /// Queues a key message for the window, for `pump` to pick up as it does typed keys.
    fn post(w: &Window, msg: u32, vk: u32, scan: u32) {
        // SAFETY: a valid window handle; key messages carry no pointers.
        unsafe { PostMessageW(w.hwnd, msg, vk as WPARAM, key_lparam(msg, vk, scan)) };
    }

    /// Key messages sent to a real window come out of `take_keys`, translated, and two
    /// windows keep their own keys. Needs a desktop session (and a keyboard layout that
    /// types 'a' for VK 'A', which every Latin layout does):
    /// `cargo test -p grail --release -- --ignored window`.
    #[test]
    #[ignore]
    fn window_turns_key_messages_into_events() {
        let _saved = SavedKeys::save();
        hold_keys(&[]);
        let mut a = Window::open("Grail key test A").unwrap();
        let mut b = Window::open("Grail key test B").unwrap();
        assert!(a.pump() && b.pump());
        a.take_keys();
        b.take_keys();
        send(&a, WM_KEYDOWN, b'A' as u32, 0x1e);
        send(&a, WM_KEYUP, b'A' as u32, 0x1e);
        send(&b, WM_KEYDOWN, VK_F1 + 1, 0x3c);
        send(&a, WM_KEYDOWN, VK_RETURN, 0x1c);
        send(&a, WM_KEYDOWN, VK_SHIFT, 0x2a);
        send(&a, WM_KEYUP, VK_RETURN, 0x1c);
        send(&a, WM_KEYDOWN, VK_SPACE, 0x39);
        // F10 arrives as a system key.
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

    /// Shift picks the character through the layout, Ctrl only sets its bit (Ctrl+C is
    /// 'c', not 0x03), and Ctrl+Alt is AltGr, not a Ctrl chord. Needs a desktop session:
    /// `cargo test -p grail --release -- --ignored window`.
    #[test]
    #[ignore]
    fn window_reports_modifiers() {
        let _saved = SavedKeys::save();
        let mut w = Window::open("Grail modifier test").unwrap();
        assert!(w.pump());
        w.take_keys();
        let tap = |held: &[u32], vk: u32, scan: u32| {
            hold_keys(held);
            send(&w, WM_KEYDOWN, vk, scan);
            send(&w, WM_KEYUP, vk, scan);
            hold_keys(&[]);
        };
        let shift = [VK_SHIFT, VK_LSHIFT];
        let ctrl = [VK_CONTROL, VK_LCONTROL];
        let altgr = [VK_CONTROL, VK_LCONTROL, VK_MENU, VK_LMENU];

        tap(&shift, b'A' as u32, 0x1e);
        tap(&ctrl, b'C' as u32, 0x2e);
        tap(&[shift, ctrl].concat(), VK_LEFT, 0x4b);
        tap(&altgr, VK_RIGHT, 0x4d);
        assert_eq!(
            w.take_keys(),
            [
                KeyEvent::press(b'A').with_shift(true),
                KeyEvent::release(b'A').with_shift(true),
                KeyEvent::press(b'c').with_ctrl(true),
                KeyEvent::release(b'c').with_ctrl(true),
                KeyEvent::press(code::LEFT).with_shift(true).with_ctrl(true),
                KeyEvent::release(code::LEFT).with_shift(true).with_ctrl(true),
                KeyEvent::press(code::RIGHT),
                KeyEvent::release(code::RIGHT),
            ]
        );

        // What Ctrl+Alt+Q types depends on the layout ('@' on German, nothing on US),
        // but it is never a Ctrl chord.
        tap(&altgr, b'Q' as u32, 0x10);
        let keys = w.take_keys();
        assert!(keys.iter().all(|k| !k.ctrl), "{keys:?}");
    }

    /// A lone Alt tap and F10 must not put the window into keyboard menu mode: that
    /// modal loop would stop the emulator inside `pump` and swallow the next key. Needs
    /// a desktop session: `cargo test -p grail --release -- --ignored window`.
    #[test]
    #[ignore]
    fn menu_keys_do_not_stop_the_window() {
        let _saved = SavedKeys::save();
        hold_keys(&[]);
        let mut w = Window::open("Grail menu key test").unwrap();
        assert!(w.pump());
        w.take_keys();

        // Should menu mode start anyway, the Esc another thread posts every second
        // until the test is over ends it, so the test fails instead of hanging.
        let (done, wait) = std::sync::mpsc::channel::<()>();
        let hwnd = w.hwnd as usize;
        let rescue = std::thread::spawn(move || {
            use std::sync::mpsc::RecvTimeoutError::Timeout;
            while let Err(Timeout) = wait.recv_timeout(std::time::Duration::from_secs(1)) {
                for msg in [WM_KEYDOWN, WM_KEYUP] {
                    // SAFETY: posting a key message carries no pointers.
                    unsafe { PostMessageW(hwnd as HWND, msg, VK_ESCAPE as WPARAM, 1) };
                }
            }
        });

        let start = std::time::Instant::now();
        for (vk, scan, letter, scan2) in [(VK_MENU, 0x38, b'A', 0x1e), (VK_F10, 0x44, b'B', 0x30)] {
            if vk == VK_MENU {
                hold_keys(&[VK_MENU, VK_LMENU]);
            }
            send(&w, WM_SYSKEYDOWN, vk, scan);
            hold_keys(&[]);
            send(&w, WM_SYSKEYUP, vk, scan);
            post(&w, WM_KEYDOWN, letter as u32, scan2);
            post(&w, WM_KEYUP, letter as u32, scan2);
            assert!(w.pump());
            let c = letter.to_ascii_lowercase();
            let mut want = vec![KeyEvent::press(c), KeyEvent::release(c)];
            if vk == VK_F10 {
                want.splice(
                    0..0,
                    [KeyEvent::press(code::F1 + 9), KeyEvent::release(code::F1 + 9)],
                );
            }
            assert_eq!(w.take_keys(), want, "the key after vk {vk:#x}");
        }
        let took = start.elapsed();
        let _ = done.send(());
        rescue.join().unwrap();
        assert!(took < std::time::Duration::from_millis(500), "took {took:?}");
    }

    /// lParam of a client-area mouse message: x in the low word, y in the high word.
    fn mouse_lparam(x: i32, y: i32) -> LPARAM {
        (x as u16 as u32 | (y as u16 as u32) << 16) as i32 as LPARAM
    }

    /// Resizes `w` so that its client area is `width` x `height`.
    fn set_client_size(w: &Window, width: i32, height: i32) {
        // SAFETY: a valid window handle and RECTs it fills in.
        unsafe {
            let (mut outer, mut client) = (RECT::default(), RECT::default());
            GetWindowRect(w.hwnd, &mut outer);
            GetClientRect(w.hwnd, &mut client);
            let extra_w = (outer.right - outer.left) - client.right;
            let extra_h = (outer.bottom - outer.top) - client.bottom;
            MoveWindow(
                w.hwnd,
                outer.left,
                outer.top,
                width + extra_w,
                height + extra_h,
                1,
            );
        }
    }

    /// Mouse messages sent to a real window come out of `take_pointer` in framebuffer
    /// pixels, also when the window is stretched, and a drag captures the mouse until
    /// its release. Needs a desktop session (and no one moving the real mouse over the
    /// window): `cargo test -p grail --release -- --ignored window`.
    #[test]
    #[ignore]
    fn window_turns_mouse_messages_into_pointer_input() {
        let mut w = Window::open("Grail mouse test").unwrap();
        assert!(w.pump());
        w.take_pointer();
        let send = |w: &Window, msg: u32, wparam: usize, lparam: LPARAM| {
            // SAFETY: a valid window handle; mouse messages carry no pointers.
            unsafe { SendMessageW(w.hwnd, msg, wparam, lparam) };
        };
        // SAFETY: no arguments.
        let captured = |w: &Window| unsafe { GetCapture() } == w.hwnd;

        set_client_size(&w, FB_WIDTH as i32, FB_HEIGHT as i32);
        send(&w, WM_MOUSEMOVE, 0, mouse_lparam(100, 50));
        send(&w, WM_LBUTTONDOWN, MK_LBUTTON, mouse_lparam(100, 50));
        assert!(captured(&w), "a held button captures the mouse");
        send(&w, WM_MOUSEMOVE, MK_LBUTTON, mouse_lparam(-20, 600)); // dragged out
        send(
            &w,
            WM_RBUTTONDOWN,
            MK_LBUTTON | MK_RBUTTON,
            mouse_lparam(-20, 600),
        );
        send(&w, WM_LBUTTONUP, MK_RBUTTON, mouse_lparam(-20, 600));
        assert!(captured(&w));
        send(&w, WM_RBUTTONUP, 0, mouse_lparam(-20, 600));
        assert!(!captured(&w), "released with the last button");
        let wheel_down = (-WHEEL_DELTA as u16 as usize) << 16;
        send(&w, WM_MOUSEWHEEL, wheel_down, mouse_lparam(9999, 9999));
        send(&w, WM_MBUTTONDOWN, MK_MBUTTON, mouse_lparam(5, 6));
        assert!(w.pump());
        assert_eq!(
            w.take_pointer(),
            [
                PointerInput::move_to(100, 50),
                PointerInput::move_to(100, 50).with_buttons(BUTTON_LEFT),
                PointerInput::move_to(-20, 600).with_buttons(BUTTON_LEFT),
                PointerInput::move_to(-20, 600).with_buttons(BUTTON_LEFT | BUTTON_RIGHT),
                PointerInput::move_to(-20, 600).with_buttons(BUTTON_RIGHT),
                PointerInput::move_to(-20, 600),
                PointerInput::default().with_wheel(-1),
                PointerInput::move_to(5, 6).with_buttons(BUTTON_MIDDLE),
            ]
        );
        // The capture taken by the middle button goes away in the middle of the click
        // (another window took the mouse): the button is released.
        assert!(captured(&w));
        // SAFETY: no arguments.
        unsafe { ReleaseCapture() };
        assert_eq!(w.take_pointer(), [PointerInput::buttons(0)]);

        // Stretched to 1.5 times the size (small enough for any screen): three client
        // pixels show two frame pixels.
        set_client_size(&w, 960, 720);
        assert!(w.pump());
        let mut client = RECT::default();
        // SAFETY: a valid window handle and a RECT it fills in.
        unsafe { GetClientRect(w.hwnd, &mut client) };
        assert_eq!(
            (client.right, client.bottom),
            (960, 720),
            "the screen is too small"
        );
        w.take_pointer();
        send(&w, WM_MOUSEMOVE, 0, mouse_lparam(201, 101));
        send(&w, WM_MOUSEMOVE, 0, mouse_lparam(959, 719));
        assert_eq!(
            w.take_pointer(),
            [PointerInput::move_to(134, 67), PointerInput::move_to(639, 479)]
        );
    }
}
