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

use crate::framebuffer::{Frame, FB_HEIGHT, FB_WIDTH};
use crate::keyboard::{code, usage, KeyEvent};
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
    /// The keys that are down, in the order they went down, with what their press
    /// reported, so a release carries the same code as its press even if Shift changed.
    down: RefCell<Vec<Held>>,
    /// The program reads physical keys (raw mode, spec section 9.4): Alt chords and F10
    /// are its keys, not the window's ([`Window::set_raw_keys`]).
    raw: Cell<bool>,
    pointer: RefCell<Vec<PointerInput>>,
    /// Buttons the last pointer input said were held, to release them if the window
    /// loses the mouse in the middle of a click.
    buttons: Cell<u8>,
    /// Wheel motion not yet reported: less than a notch (precise touchpads and wheels
    /// send fractions of `WHEEL_DELTA`).
    wheel: Cell<i32>,
    /// The size of the frame last drawn, in pixels: the framebuffer's mode, which the
    /// mouse's positions are mapped to.
    frame: Cell<(u32, u32)>,
}

/// A key that is down, and what its press reported.
#[derive(Debug, Clone, Copy)]
struct Held {
    /// Which physical key: its scan code as [`usage::Key::win`] gives it, or, for a
    /// message with no scan code, 0x1_0000 plus the virtual key.
    id: u32,
    code: u8,
    usage: u16,
    /// Its press was reported with its release at once (a key Windows may send no
    /// key-up for, [`taps`]): the key-up, if one comes, reports nothing.
    tapped: bool,
}

/// Whether the key of usage `u` is one the program sees as a press and a release at
/// once, whatever messages Windows sends: Lang1 and Lang2, the Korean Hangul and Hanja
/// keys, are make-only on a PC keyboard (scan code set 1 sends 0xF2 and 0xF1 with no
/// break, and Windows' HID translation sends them so); Linux's `atkbd` likewise makes
/// up their releases (its `force_release_mask`).
fn taps(u: u16) -> bool {
    matches!(u, usage::LANG1 | usage::LANG2)
}

/// The virtual key `GetKeyState` reads for modifier `u` (by side), `None` for the
/// other keys.
fn modifier_vk(u: u16) -> Option<u32> {
    Some(match u {
        usage::LEFT_CTRL => VK_LCONTROL,
        usage::LEFT_SHIFT => VK_LSHIFT,
        usage::LEFT_ALT => VK_LMENU,
        usage::LEFT_GUI => VK_LWIN,
        usage::RIGHT_CTRL => VK_RCONTROL,
        usage::RIGHT_SHIFT => VK_RSHIFT,
        usage::RIGHT_ALT => VK_RMENU,
        usage::RIGHT_GUI => VK_RWIN,
        _ => return None,
    })
}

impl State {
    fn new() -> Self {
        State {
            frame: Cell::new((FB_WIDTH, FB_HEIGHT)),
            ..State::default()
        }
    }

    /// The key with virtual key `vk` and scan code `scan` (0xe0xx for extended keys,
    /// as [`usage::Key::win`]) went down (`press`) or up. `char_code` gives the
    /// printable character it types, for keys that are not in the spec's table of
    /// special keys. Each event carries both forms of the key, the ASCII-mode code and
    /// the HID usage of its scan code, for the mode the program chose.
    fn key(
        &self,
        vk: u32,
        scan: u16,
        press: bool,
        shift: bool,
        ctrl: bool,
        char_code: impl FnOnce() -> Option<u8>,
    ) {
        if vk > 0xff {
            return; // not a virtual key
        }
        // Keys are told apart by scan code, so left and right Shift, or Enter and the
        // keypad's Enter, are separate keys with separate presses and releases.
        let id = if scan != 0 { scan as u32 } else { 0x1_0000 | vk };
        let mut down = self.down.borrow_mut();
        let at = down.iter().position(|h| h.id == id);
        let (held, repeat) = match (press, at) {
            // A key that was reported as a tap: nothing more until it is up again.
            (true, Some(i)) if down[i].tapped => return,
            (false, Some(i)) if down[i].tapped => {
                down.remove(i);
                return;
            }
            // Auto-repeat sends more key-downs: each one is another press of what the
            // key got when it first went down, marked as a repeat.
            (true, Some(i)) => (down[i], true),
            (true, None) => {
                let usage = usage::from_win_scan(scan).unwrap_or(0);
                let held = Held {
                    id,
                    code: special_code(vk).or_else(char_code).unwrap_or(0),
                    usage,
                    tapped: taps(usage),
                };
                if held.code == 0 && held.usage == 0 {
                    return; // nothing to report in either mode
                }
                down.push(held);
                if held.tapped {
                    drop(down);
                    self.tap(held.code, held.usage);
                    return;
                }
                (held, false)
            }
            (false, Some(i)) => (down.remove(i), false),
            (false, None) => {
                // Print Screen's key-down goes to the system's hotkey, so only its
                // key-up comes: it is a press and a release, as SDL makes it. So is a
                // make-only key whose message is a key-up.
                let usage = usage::from_win_scan(scan).unwrap_or(0);
                if usage == usage::PRINT_SCREEN || taps(usage) {
                    drop(down);
                    self.tap(0, usage);
                }
                // Any other release with no press seen (the key was held when the
                // window got the focus): the program never saw it go down.
                return;
            }
        };
        self.keys.borrow_mut().push(KeyEvent {
            code: held.code,
            usage: held.usage,
            release: !press,
            repeat,
            shift,
            ctrl,
        });
    }

    /// Reports a press of the key with `code` and `usage` and its release at once.
    fn tap(&self, code: u8, usage: u16) {
        let press = KeyEvent::press(code).with_usage(usage);
        self.keys.borrow_mut().extend([
            press,
            KeyEvent {
                release: true,
                ..press
            },
        ]);
    }

    /// Focus moved away, so the key-ups will go to another window: release every key the
    /// program saw go down, modifiers included, instead of leaving it stuck.
    fn release_all(&self) {
        let mut keys = self.keys.borrow_mut();
        for held in self.down.borrow_mut().drain(..) {
            if !held.tapped {
                keys.push(KeyEvent::release(held.code).with_usage(held.usage));
            }
        }
    }

    /// Releases every modifier the program saw go down that `is_down` (`GetKeyState`
    /// for its side's virtual key) says is up. Windows loses key-ups: with both Shifts
    /// down, letting go of one sends no key-up, and letting go of the other sends one,
    /// for that key alone; the shell may keep the Windows key's key-up for its
    /// hotkeys. The window's thread key state is right all the same (it changes as
    /// key messages are read), so after each pump the window looks at it, as SDL does
    /// (`WIN_PumpEvents`), and a modifier is never left stuck.
    fn resync_modifiers(&self, is_down: impl Fn(u32) -> bool) {
        let mut keys = self.keys.borrow_mut();
        self.down
            .borrow_mut()
            .retain(|held| match modifier_vk(held.usage) {
                Some(vk) if !held.tapped && !is_down(vk) => {
                    keys.push(KeyEvent::release(held.code).with_usage(held.usage));
                    false
                }
                _ => true,
            });
    }
}

impl State {
    /// The mouse moved to framebuffer pixel `pos` (`None`: it did not move), the
    /// buttons held are now `buttons`, and the wheel turned `wheel` Windows units
    /// (`WHEEL_DELTA` per notch, positive away from the user).
    fn mouse(&self, pos: Option<(i32, i32)>, buttons: u8, wheel: i32) {
        // A turn the other way starts afresh: what was left of the last direction
        // must not eat into a notch of the new one.
        let rest = self.wheel.get();
        let rest = if wheel != 0 && (rest < 0) != (wheel < 0) {
            0
        } else {
            rest
        };
        let total = rest.saturating_add(wheel);
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

    /// Records what a mouse message, or the window losing the mouse capture or the
    /// focus, means for the program, with the client area `width` x `height` pixels.
    /// For a move or a button message, returns whether the window should hold the
    /// mouse capture now (while a button is held, so a drag follows the mouse out of
    /// the window and its release is not lost); `None` for the others.
    fn mouse_message(
        &self,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
        width: i32,
        height: i32,
    ) -> Option<bool> {
        match msg {
            WM_MOUSEMOVE | WM_LBUTTONDOWN | WM_LBUTTONUP | WM_RBUTTONDOWN | WM_RBUTTONUP | WM_MBUTTONDOWN
            | WM_MBUTTONUP => {
                // Client coordinates, signed (negative left of or above the client
                // area while the mouse is captured).
                let (x, y) = (lparam as u16 as i16 as i32, (lparam >> 16) as u16 as i16 as i32);
                let pos = client_to_fb(x, y, width, height, self.frame.get());
                let buttons = mk_buttons(wparam);
                if pos.is_some() || buttons != self.buttons.get() {
                    self.mouse(pos, buttons, 0);
                }
                Some(buttons != 0)
            }
            WM_MOUSEWHEEL => {
                // The position in lParam is in screen coordinates, and the pointer has
                // not moved: report the buttons and the wheel only.
                self.mouse(None, mk_buttons(wparam), (wparam >> 16) as u16 as i16 as i32);
                None
            }
            WM_CAPTURECHANGED => {
                self.release_buttons();
                None
            }
            WM_KILLFOCUS => {
                self.release_all();
                self.release_buttons();
                None
            }
            _ => None,
        }
    }
}

/// The pointer buttons held, from a mouse message's `wParam` (`MK_*` flags).
fn mk_buttons(wparam: WPARAM) -> u8 {
    let flag = |mk: usize, button: u8| if wparam & mk != 0 { button } else { 0 };
    flag(MK_LBUTTON, BUTTON_LEFT) | flag(MK_RBUTTON, BUTTON_RIGHT) | flag(MK_MBUTTON, BUTTON_MIDDLE)
}

/// The framebuffer pixel shown at point (`x`, `y`) of a client area `width` × `height`
/// pixels, which a frame of `frame` (width, height) pixels is stretched over
/// (`Window::draw`). Points outside the client area (a drag that leaves the window) give
/// pixels outside the framebuffer; the pointer device clamps them. `None` while the
/// client area is empty (minimized).
fn client_to_fb(x: i32, y: i32, width: i32, height: i32, frame: (u32, u32)) -> Option<(i32, i32)> {
    if width <= 0 || height <= 0 {
        return None;
    }
    // The stretch (COLORONCOLOR) gives each client pixel the frame pixel under its
    // centre: client pixel `x` shows frame pixel floor((x + 1/2) * frame width / width).
    let scale =
        |v: i32, fb: u32, client: i32| ((2 * v as i64 + 1) * fb as i64).div_euclid(2 * client as i64) as i32;
    Some((scale(x, frame.0, width), scale(y, frame.1, height)))
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

/// The scan code of a key message, as [`usage::Key::win`] gives it: `lParam` bits
/// 16-23, plus 0xe000 for an extended key (bit 24). A message with none (synthesized
/// from a virtual key alone, as some remote-control programs send) gets the scan code
/// the keyboard layout gives the virtual key. `VK_PACKET`, a character sent as a key
/// (`SendInput` with `KEYEVENTF_UNICODE`), has none: its `wScan` is the character's
/// UTF-16 unit, which lands in the scan code field and names no key.
fn win_scan(vk: u32, lparam: LPARAM) -> u16 {
    if vk == VK_PACKET {
        return 0;
    }
    let scan = (lparam >> 16) as u16 & 0xff;
    match (scan, lparam & KF_EXTENDED_LPARAM != 0) {
        // The layout gives no key of the table for these: Num Lock is 0x45 there,
        // which Windows' messages give Pause; Pause is 0xE11D (its set 1 E1 1D 45)
        // and Print Screen 0x54 (SysRq). Ctrl+Pause comes as VK_CANCEL. They get what
        // their key messages carry.
        (0, _) if vk == VK_NUMLOCK => 0xe045,
        (0, _) if vk == VK_PAUSE || vk == VK_CANCEL => 0x45,
        (0, _) if vk == VK_SNAPSHOT => 0xe037,
        (0, _) => {
            // SAFETY: no pointers involved.
            let scan = unsafe { MapVirtualKeyW(vk, MAPVK_VK_TO_VSC_EX) } as u16;
            // The arrows, Insert, Delete, Home, End, Page Up and Page Down are also
            // the keypad's keys with Num Lock off, and the layout gives their keypad
            // scan code: a virtual key alone means the grey key, extended.
            const VK_PRIOR: u32 = 0x21;
            const VK_INSERT: u32 = 0x2d;
            const VK_DELETE: u32 = 0x2e;
            if scan != 0 && matches!(vk, VK_PRIOR..=VK_DOWN | VK_INSERT | VK_DELETE) {
                0xe000 | scan
            } else {
                scan
            }
        }
        (scan, true) => 0xe000 | scan,
        (scan, false) => scan,
    }
}

/// Whether key-down message `msg` (`wparam`, `lparam`, sent at `time`) is the Left Ctrl
/// that Windows makes up for AltGr: on a layout with AltGr, the Right Alt key comes as
/// a Left Ctrl message and then the Right Alt one, both at the same time, though no
/// Ctrl key moved. `next` is the message after it in the queue. The physical keyboard
/// sent Right Alt alone, so that is all raw mode reports, as SDL does (its
/// `SkipAltGrLeftControl`). A Left Ctrl really pressed has no Right Alt queued behind
/// it at the same millisecond.
///
/// Key-ups are never skipped: the made-up Left Ctrl's key-up is a release of a key the
/// window did not see go down, which reports nothing anyway. Should a made-up press go
/// through (a posted message queued before the Right Alt one hides it from `next`), its
/// key-up then releases it, instead of leaving Left Ctrl held.
fn is_altgr_ctrl(msg: u32, wparam: WPARAM, lparam: LPARAM, time: u32, next: Option<&MSG>) -> bool {
    let Some(next) = next else {
        return false;
    };
    matches!(msg, WM_KEYDOWN | WM_SYSKEYDOWN)
        && wparam as u32 == VK_CONTROL
        && lparam & KF_EXTENDED_LPARAM == 0
        && matches!(next.message, WM_KEYDOWN | WM_SYSKEYDOWN)
        && next.wParam as u32 == VK_MENU
        && next.lParam & KF_EXTENDED_LPARAM != 0
        && next.time == time
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
            let mut next: MSG = std::mem::zeroed();
            let next = (PeekMessageW(&mut next, hwnd, 0, 0, PM_NOREMOVE) != 0).then_some(&next);
            if !is_altgr_ctrl(msg, wparam, lparam, GetMessageTime() as u32, next) {
                st.key(vk, win_scan(vk, lparam), press, key_down(VK_SHIFT), ctrl, || {
                    printable(vk, scan)
                });
            }
            // In ASCII mode Alt chords still reach Windows, so Alt+F4 closes the window
            // (menu mode is stopped at WM_SYSCOMMAND below). In raw mode they are the
            // program's keys, as in a virtual machine's window that has the keyboard:
            // Alt+F4 is for the guest's own windows, and the close button still works.
            if matches!(msg, WM_SYSKEYDOWN | WM_SYSKEYUP) && !st.raw.get() {
                return DefWindowProcW(hwnd, msg, wparam, lparam);
            }
            0
        }
        // In raw mode an Alt chord's character is no menu mnemonic (nor a beep).
        WM_SYSCHAR if st.raw.get() => 0,
        // A lone Alt tap or F10 asks for keyboard menu mode, a modal loop inside
        // DispatchMessageW: the emulator would stop until a key ends it, and that key
        // would be swallowed. Refuse it; in ASCII mode Alt+Space (lParam ' ') still
        // opens the menu.
        WM_SYSCOMMAND if wparam & 0xfff0 == SC_KEYMENU && (lparam == 0 || st.raw.get()) => 0,
        WM_MOUSEMOVE | WM_LBUTTONDOWN | WM_LBUTTONUP | WM_RBUTTONDOWN | WM_RBUTTONUP | WM_MBUTTONDOWN
        | WM_MBUTTONUP | WM_MOUSEWHEEL | WM_CAPTURECHANGED | WM_KILLFOCUS => {
            let mut client = RECT::default();
            GetClientRect(hwnd, &mut client);
            let (width, height) = (client.right - client.left, client.bottom - client.top);
            // Taken or let go after the state is updated: ReleaseCapture sends
            // WM_CAPTURECHANGED back here before it returns.
            match st.mouse_message(msg, wparam, lparam, width, height) {
                Some(true) if GetCapture() != hwnd => {
                    SetCapture(hwnd);
                }
                Some(false) if GetCapture() == hwnd => {
                    ReleaseCapture();
                }
                _ => {}
            }
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
    /// thread: each one's procedure updates its own state), then releases the modifiers
    /// whose key-ups Windows lost (`State::resync_modifiers`). Returns false once this
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
        self.state.resync_modifiers(key_down);
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

    /// Tells the window whether the program reads physical keys (the keyboard's raw
    /// mode, spec section 9.4). In raw mode Alt chords and F10 are the program's keys:
    /// the window no longer gives them to Windows, so Alt+F4 does not close it and no
    /// key opens the window menu. Off (ASCII mode) when the window opens.
    pub fn set_raw_keys(&self, raw: bool) {
        self.state.raw.set(raw);
    }

    /// Key events typed into the window since the last call, oldest first. Each one
    /// carries the key's ASCII-mode code and its HID usage (spec section 9.4).
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

    /// The size of the frame last drawn: the mouse's positions are in its pixels.
    pub fn frame_size(&self) -> (u32, u32) {
        self.state.frame.get()
    }

    /// Stretches the frame over the client area; a frame with no pixels (the display
    /// off) paints it black. A frame of another size than the last one (the guest
    /// changed modes) resizes the window to show it pixel for pixel, as QEMU's window
    /// does, unless the user has maximized or minimized it; the mouse is mapped to the
    /// new size from then on.
    pub fn draw(&self, frame: Frame) {
        if !self.is_open() {
            return;
        }
        if self.state.frame.replace((frame.width, frame.height)) != (frame.width, frame.height) {
            self.fit_to(frame.width, frame.height);
        }
        if let Some(pixels) = frame.pixels {
            // A scanout buffer may be RAM the guest has never touched, which the host's
            // GDI could not read on Windows until it is committed.
            crate::ram::touch(pixels);
        }
        // SAFETY: a valid window handle; the DC is released before returning.
        unsafe {
            let mut client = RECT::default();
            GetClientRect(self.hwnd, &mut client);
            let hdc = GetDC(self.hwnd);
            if hdc.is_null() {
                return;
            }
            stretch(hdc, client.right - client.left, client.bottom - client.top, frame);
            ReleaseDC(self.hwnd, hdc);
        }
    }

    /// Gives the window a client area of `width` × `height` pixels, keeping its top
    /// left corner, unless it is maximized or minimized. It stays on its monitor's work
    /// area ([`fitted`]): a mode bigger than the screen (1920x1080 on a 1080p monitor,
    /// once the borders and the title bar are counted) gets a smaller window, which the
    /// frame is stretched to as always.
    fn fit_to(&self, width: u32, height: u32) {
        // SAFETY: a valid window handle, and structs the calls fill in (MONITORINFO with
        // its size set, as GetMonitorInfoW requires).
        unsafe {
            if IsZoomed(self.hwnd) != 0 || IsIconic(self.hwnd) != 0 {
                return;
            }
            let (mut outer, mut client) = (RECT::default(), RECT::default());
            GetWindowRect(self.hwnd, &mut outer);
            GetClientRect(self.hwnd, &mut client);
            let extra = (
                (outer.right - outer.left) - client.right,
                (outer.bottom - outer.top) - client.bottom,
            );
            let mut monitor = MONITORINFO {
                cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                ..MONITORINFO::default()
            };
            let on = MonitorFromWindow(self.hwnd, MONITOR_DEFAULTTONEAREST);
            let work = (GetMonitorInfoW(on, &mut monitor) != 0).then_some(monitor.rcWork);
            let to = fitted(outer, extra, width, height, work);
            MoveWindow(
                self.hwnd,
                to.left,
                to.top,
                to.right - to.left,
                to.bottom - to.top,
                1,
            );
        }
    }
}

/// Where a window whose outer rectangle is `outer` goes for a client area of `width` ×
/// `height`, `extra` being what its frame adds (borders, title bar): at its top left
/// corner, but inside `work`, its monitor's work area, when that is known. There it is
/// no bigger than the work area, and moved up or left as far as needed for its right
/// and bottom edges to be in it, so that the title bar stays on screen.
fn fitted(outer: RECT, extra: (i32, i32), width: u32, height: u32, work: Option<RECT>) -> RECT {
    let mut w = (width as i32).saturating_add(extra.0);
    let mut h = (height as i32).saturating_add(extra.1);
    let (mut left, mut top) = (outer.left, outer.top);
    if let Some(work) = work {
        w = w.min(work.right - work.left);
        h = h.min(work.bottom - work.top);
        left = left.min(work.right - w).max(work.left);
        top = top.min(work.bottom - h).max(work.top);
    }
    RECT {
        left,
        top,
        right: left + w,
        bottom: top + h,
    }
}

/// Stretches `frame` (`Window::draw`'s) over `width` x `height` pixels at the top left
/// of `hdc`. `client_to_fb` inverts this mapping.
///
/// # Safety
/// `hdc` must be a valid device context.
unsafe fn stretch(hdc: HDC, width: i32, height: i32, frame: Frame) {
    const BLACK: [u8; 4] = [0; 4];
    let (bits, w, h): (*const c_void, i32, i32) = match frame.pixels {
        Some(p) => {
            assert_eq!(p.len(), frame.width as usize * frame.height as usize * 4);
            (p.as_ptr().cast(), frame.width as i32, frame.height as i32)
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
    // SAFETY: `bits` points at w*h 32-bit pixels described by `bmi`.
    SetStretchBltMode(hdc, COLORONCOLOR);
    StretchDIBits(
        hdc,
        0,
        0,
        width,
        height,
        0,
        0,
        w,
        h,
        bits,
        &bmi,
        DIB_RGB_COLORS,
        SRCCOPY,
    );
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

    // Scan codes (as `usage::Key::win`) of the keys the tests press.
    const SC_A: u16 = 0x1e;
    const SC_W: u16 = 0x11;
    const SC_Q: u16 = 0x10;
    const SC_F5: u16 = 0x3f;
    const SC_ENTER: u16 = 0x1c;
    const SC_KP_ENTER: u16 = 0xe01c;
    const SC_LEFT: u16 = 0xe04b;
    const SC_UP: u16 = 0xe048;
    const SC_HOME: u16 = 0xe047;
    const SC_KP7: u16 = 0x47;
    const SC_LSHIFT: u16 = 0x2a;
    const SC_RSHIFT: u16 = 0x36;
    const SC_LCTRL: u16 = 0x1d;
    const SC_RCTRL: u16 = 0xe01d;
    const SC_LALT: u16 = 0x38;
    const SC_RALT: u16 = 0xe038;
    const SC_LWIN: u16 = 0xe05b;
    const SC_RWIN: u16 = 0xe05c;
    const SC_PRTSC: u16 = 0xe037;
    const SC_HANGUL: u16 = 0x72;
    const SC_HANJA: u16 = 0x71;
    const VK_HOME: u32 = 0x24;
    const VK_NUMPAD7: u32 = 0x67;
    const VK_HANGUL: u32 = 0x15;
    const VK_HANJA: u32 = 0x19;

    #[test]
    fn release_reports_the_code_of_the_press() {
        let st = State::new();
        // Shift+A down, Shift up, A up: the release is still 'A', of the same key.
        st.key(b'A' as u32, SC_A, true, true, false, || Some(b'A'));
        st.key(b'A' as u32, SC_A, false, false, false, || Some(b'a'));
        assert_eq!(
            st.keys.take(),
            [
                KeyEvent::press(b'A').with_usage(usage::A).with_shift(true),
                KeyEvent::release(b'A').with_usage(usage::A),
            ]
        );
    }

    #[test]
    fn auto_repeat_and_flags() {
        let st = State::new();
        st.key(VK_LEFT, SC_LEFT, true, false, true, || {
            unreachable!("special key")
        });
        st.key(VK_LEFT, SC_LEFT, true, false, true, || {
            unreachable!("special key")
        });
        st.key(VK_LEFT, SC_LEFT, false, false, false, || {
            unreachable!("special key")
        });
        let left = KeyEvent::press(code::LEFT).with_usage(usage::LEFT);
        assert_eq!(
            st.keys.take(),
            [
                left.with_ctrl(true),
                left.with_ctrl(true).with_repeat(true),
                KeyEvent::release(code::LEFT).with_usage(usage::LEFT),
            ]
        );
    }

    #[test]
    fn keys_without_a_code_or_a_usage_send_nothing() {
        let st = State::new();
        st.key(0xde, 0, true, false, false, || None); // a dead key, say, of no scan code
        st.key(0xde, 0, true, false, false, || None);
        st.key(0xde, 0, false, false, false, || panic!("no lookup on release"));
        st.key(0xe5, 0xe0ff, true, false, false, || None); // a scan code of no key
                                                           // A release with no press seen (key held when the window got focus).
        st.key(b'Q' as u32, SC_Q, false, false, false, || Some(b'q'));
        st.key(0x1_0000, SC_A, true, false, false, || Some(b'x')); // out of range
        assert!(st.keys.take().is_empty());
        // A key with a code but no scan code still reports its code.
        st.key(b'Q' as u32, 0, true, false, false, || Some(b'q'));
        st.key(b'Q' as u32, 0, false, false, false, || Some(b'q'));
        assert_eq!(st.keys.take(), [KeyEvent::press(b'q'), KeyEvent::release(b'q')]);
    }

    #[test]
    fn modifiers_are_keys_of_their_own() {
        let st = State::new();
        // Alone, each modifier is a press and a release with its usage and no code,
        // left and right apart.
        let mods = [
            (VK_SHIFT, SC_LSHIFT, usage::LEFT_SHIFT),
            (VK_SHIFT, SC_RSHIFT, usage::RIGHT_SHIFT),
            (VK_CONTROL, SC_LCTRL, usage::LEFT_CTRL),
            (VK_CONTROL, SC_RCTRL, usage::RIGHT_CTRL),
            (VK_MENU, SC_LALT, usage::LEFT_ALT),
            (VK_MENU, SC_RALT, usage::RIGHT_ALT),
            (VK_LWIN, SC_LWIN, usage::LEFT_GUI),
            (VK_RWIN, SC_RWIN, usage::RIGHT_GUI),
        ];
        for (vk, scan, _) in mods {
            st.key(vk, scan, true, false, false, || None);
            st.key(vk, scan, false, false, false, || None);
        }
        let want: Vec<KeyEvent> = mods
            .iter()
            .flat_map(|&(_, _, u)| [KeyEvent::key_press(u), KeyEvent::key_release(u)])
            .collect();
        assert_eq!(st.keys.take(), want);

        // Both Shifts down (one virtual key, VK_SHIFT): two keys.
        st.key(VK_SHIFT, SC_LSHIFT, true, true, false, || None);
        st.key(VK_SHIFT, SC_RSHIFT, true, true, false, || None);
        assert_eq!(
            st.keys.take(),
            [
                KeyEvent::key_press(usage::LEFT_SHIFT).with_shift(true),
                KeyEvent::key_press(usage::RIGHT_SHIFT).with_shift(true),
            ]
        );
    }

    #[test]
    fn modifiers_whose_key_up_windows_lost_are_released() {
        let st = State::new();
        let held = |vks: &'static [u32]| move |vk: u32| vks.contains(&vk);
        // Both Shifts down, then Left Shift let go: Windows sends no key-up for it,
        // but the key state has it up. The pump after it releases it.
        st.key(VK_SHIFT, SC_LSHIFT, true, true, false, || None);
        st.key(VK_SHIFT, SC_RSHIFT, true, true, false, || None);
        st.key(b'A' as u32, SC_A, true, true, false, || Some(b'A'));
        st.resync_modifiers(held(&[VK_LSHIFT, VK_RSHIFT]));
        st.resync_modifiers(held(&[VK_RSHIFT]));
        // Right Shift's key-up does come. 'A' is no modifier: the key state is not
        // asked about it.
        st.key(VK_SHIFT, SC_RSHIFT, false, false, false, || None);
        st.resync_modifiers(held(&[]));
        assert_eq!(
            st.keys.take(),
            [
                KeyEvent::key_press(usage::LEFT_SHIFT).with_shift(true),
                KeyEvent::key_press(usage::RIGHT_SHIFT).with_shift(true),
                KeyEvent::press(b'A').with_usage(usage::A).with_shift(true),
                KeyEvent::key_release(usage::LEFT_SHIFT),
                KeyEvent::key_release(usage::RIGHT_SHIFT),
            ]
        );
        // Every modifier by its side's virtual key: the Windows key whose key-up the
        // shell kept, Right Alt, Left Ctrl.
        for (vk, scan, side, u) in [
            (VK_LWIN, SC_LWIN, VK_LWIN, usage::LEFT_GUI),
            (VK_RWIN, SC_RWIN, VK_RWIN, usage::RIGHT_GUI),
            (VK_MENU, SC_LALT, VK_LMENU, usage::LEFT_ALT),
            (VK_MENU, SC_RALT, VK_RMENU, usage::RIGHT_ALT),
            (VK_CONTROL, SC_LCTRL, VK_LCONTROL, usage::LEFT_CTRL),
            (VK_CONTROL, SC_RCTRL, VK_RCONTROL, usage::RIGHT_CTRL),
            (VK_SHIFT, SC_LSHIFT, VK_LSHIFT, usage::LEFT_SHIFT),
            (VK_SHIFT, SC_RSHIFT, VK_RSHIFT, usage::RIGHT_SHIFT),
        ] {
            st.key(vk, scan, true, false, false, || None);
            // Its side down, every other key up: still held.
            st.resync_modifiers(|v| v == side);
            assert_eq!(st.keys.take(), [KeyEvent::key_press(u)], "vk {side:#x}");
            st.resync_modifiers(|v| v != side);
            assert_eq!(st.keys.take(), [KeyEvent::key_release(u)], "vk {side:#x}");
        }
        // 'A' is still down; only focus loss releases it.
        st.release_all();
        assert_eq!(st.keys.take(), [KeyEvent::release(b'A').with_usage(usage::A)]);
    }

    #[test]
    fn print_screen_comes_as_its_key_up_alone() {
        let st = State::new();
        // Windows keeps Print Screen's key-down for its hotkey: the key-up alone is a
        // press and a release.
        st.key(VK_SNAPSHOT, SC_PRTSC, false, false, false, || None);
        let prtsc = KeyEvent::key_press(usage::PRINT_SCREEN);
        assert_eq!(
            st.keys.take(),
            [
                prtsc,
                KeyEvent {
                    release: true,
                    ..prtsc
                }
            ]
        );
        // Should its key-down come (no hotkey), it is a key as any other.
        st.key(VK_SNAPSHOT, SC_PRTSC, true, false, false, || None);
        st.key(VK_SNAPSHOT, SC_PRTSC, false, false, false, || None);
        assert_eq!(
            st.keys.take(),
            [
                prtsc,
                KeyEvent {
                    release: true,
                    ..prtsc
                }
            ]
        );
    }

    #[test]
    fn hangul_and_hanja_are_taps() {
        let st = State::new();
        let tap = |u: u16| [KeyEvent::key_press(u), KeyEvent::key_release(u)];
        // A key-down: the press and the release at once; its repeats and key-up, if
        // they come, report nothing.
        st.key(VK_HANGUL, SC_HANGUL, true, false, false, || None);
        st.key(VK_HANGUL, SC_HANGUL, true, false, false, || None);
        assert_eq!(st.keys.take(), tap(usage::LANG1));
        st.release_all();
        st.resync_modifiers(|_| false);
        assert!(st.keys.take().is_empty(), "released already");
        st.key(VK_HANJA, SC_HANJA, true, false, false, || None);
        st.key(VK_HANJA, SC_HANJA, false, false, false, || None);
        // A key-up alone: the same.
        st.key(VK_HANGUL, SC_HANGUL, false, false, false, || None);
        assert_eq!(st.keys.take(), [tap(usage::LANG2), tap(usage::LANG1)].concat());
        // Pressed again after its key-up: another tap.
        st.key(VK_HANJA, SC_HANJA, true, false, false, || None);
        assert_eq!(st.keys.take(), tap(usage::LANG2));
    }

    #[test]
    fn extended_keys_are_the_grey_keys() {
        let st = State::new();
        // Enter and the keypad's Enter: the same virtual key and code, two usages.
        st.key(VK_RETURN, SC_ENTER, true, false, false, || None);
        st.key(VK_RETURN, SC_KP_ENTER, true, false, false, || None);
        st.key(VK_RETURN, SC_ENTER, false, false, false, || None);
        st.key(VK_RETURN, SC_KP_ENTER, false, false, false, || None);
        // Home and the keypad's 7 with Num Lock off (VK_HOME too): the scan code tells.
        st.key(VK_HOME, SC_HOME, true, false, false, || None);
        st.key(VK_HOME, SC_HOME, false, false, false, || None);
        st.key(VK_HOME, SC_KP7, true, false, false, || None);
        // Num Lock on: the same key is VK_NUMPAD7 and types '7'.
        st.key(VK_NUMPAD7, SC_KP7, false, false, false, || Some(b'7'));
        let enter = KeyEvent::press(code::ENTER);
        assert_eq!(
            st.keys.take(),
            [
                enter.with_usage(usage::ENTER),
                enter.with_usage(usage::KP_ENTER),
                KeyEvent::release(code::ENTER).with_usage(usage::ENTER),
                KeyEvent::release(code::ENTER).with_usage(usage::KP_ENTER),
                KeyEvent::key_press(usage::HOME),
                KeyEvent::key_release(usage::HOME),
                KeyEvent::key_press(usage::KP1 + 6),
                KeyEvent::key_release(usage::KP1 + 6),
            ]
        );
    }

    #[test]
    fn scan_codes_from_lparam() {
        let lp = |scan: isize, ext: bool| (scan << 16 | if ext { KF_EXTENDED_LPARAM } else { 0 }) | 1;
        assert_eq!(win_scan(b'A' as u32, lp(0x1e, false)), SC_A);
        assert_eq!(win_scan(VK_CONTROL, lp(0x1d, true)), SC_RCTRL);
        // Key-up messages set bits 30 and 31; the scan code is the same.
        assert_eq!(
            win_scan(VK_UP, lp(0x48, true) | 0xc000_0000u32 as i32 as isize),
            SC_UP
        );
        // A message with no scan code gets the layout's, extended keys included.
        assert_eq!(win_scan(b'A' as u32, 1), SC_A);
        assert_eq!(win_scan(VK_UP, 1), SC_UP);
        assert_eq!(win_scan(VK_HOME, 1), SC_HOME);
        assert_eq!(win_scan(VK_NUMPAD7, 1), SC_KP7);
        assert_eq!(usage::from_win_scan(win_scan(VK_LWIN, 1)), Some(usage::LEFT_GUI));
        for (vk, u) in [
            (VK_NUMLOCK, usage::NUM_LOCK),
            (VK_PAUSE, usage::PAUSE),
            (VK_CANCEL, usage::PAUSE),
            (VK_SNAPSHOT, usage::PRINT_SCREEN),
        ] {
            assert_eq!(usage::from_win_scan(win_scan(vk, 1)), Some(u), "vk {vk:#x}");
        }
        // With their scan codes, as the keyboard sends them: Num Lock is extended,
        // Ctrl+Pause (Break) is E0 46, Alt+Print Screen (SysRq) is 54.
        assert_eq!(
            usage::from_win_scan(win_scan(VK_NUMLOCK, lp(0x45, true))),
            Some(usage::NUM_LOCK)
        );
        assert_eq!(
            usage::from_win_scan(win_scan(VK_PAUSE, lp(0x45, false))),
            Some(usage::PAUSE)
        );
        assert_eq!(
            usage::from_win_scan(win_scan(VK_CANCEL, lp(0x46, true))),
            Some(usage::PAUSE)
        );
        assert_eq!(
            usage::from_win_scan(win_scan(VK_SNAPSHOT, lp(0x54, false))),
            Some(usage::PRINT_SCREEN)
        );
        // A character sent as a key: whatever its scan code field holds, no key.
        assert_eq!(win_scan(VK_PACKET, lp(0x41, false)), 0);
        assert_eq!(win_scan(VK_PACKET, lp(0xe9, true)), 0);
        assert_eq!(win_scan(VK_PACKET, 1), 0);
    }

    #[test]
    fn the_left_ctrl_of_altgr_is_not_a_key() {
        let lp = |scan: isize, ext: bool| (scan << 16 | if ext { KF_EXTENDED_LPARAM } else { 0 }) | 1;
        let msg = |message: u32, vk: u32, lparam: LPARAM, time: u32| MSG {
            hwnd: null_mut(),
            message,
            wParam: vk as WPARAM,
            lParam: lparam,
            time,
            pt: POINT::default(),
        };
        let ctrl = lp(0x1d, false);
        let ralt = msg(WM_SYSKEYDOWN, VK_MENU, lp(0x38, true), 500);
        // AltGr down: Left Ctrl and Right Alt at the same time. Up likewise.
        assert!(is_altgr_ctrl(
            WM_KEYDOWN,
            VK_CONTROL as WPARAM,
            ctrl,
            500,
            Some(&ralt)
        ));
        // Its key-up is not skipped: with no press seen, it reports nothing anyway.
        let ralt_up = msg(WM_KEYUP, VK_MENU, lp(0x38, true), 640);
        assert!(!is_altgr_ctrl(
            WM_KEYUP,
            VK_CONTROL as WPARAM,
            ctrl,
            640,
            Some(&ralt_up)
        ));
        // A real Left Ctrl: nothing queued, a later Right Alt, a Left Alt, a key-up
        // after a key-down, or Right Ctrl.
        assert!(!is_altgr_ctrl(WM_KEYDOWN, VK_CONTROL as WPARAM, ctrl, 500, None));
        assert!(!is_altgr_ctrl(
            WM_KEYDOWN,
            VK_CONTROL as WPARAM,
            ctrl,
            499,
            Some(&ralt)
        ));
        let lalt = msg(WM_SYSKEYDOWN, VK_MENU, lp(0x38, false), 500);
        assert!(!is_altgr_ctrl(
            WM_KEYDOWN,
            VK_CONTROL as WPARAM,
            ctrl,
            500,
            Some(&lalt)
        ));
        assert!(!is_altgr_ctrl(
            WM_KEYDOWN,
            VK_CONTROL as WPARAM,
            ctrl,
            640,
            Some(&ralt_up)
        ));
        let rctrl = lp(0x1d, true);
        assert!(!is_altgr_ctrl(
            WM_KEYDOWN,
            VK_CONTROL as WPARAM,
            rctrl,
            500,
            Some(&ralt)
        ));
        assert!(!is_altgr_ctrl(
            WM_KEYDOWN,
            VK_SHIFT as WPARAM,
            lp(0x2a, false),
            500,
            Some(&ralt)
        ));
    }

    #[test]
    fn a_missed_altgr_press_does_not_hold_left_ctrl() {
        // The made-up Left Ctrl press went through (a posted message hid the Right Alt
        // behind it); its key-up, never skipped, releases it.
        let st = State::new();
        st.key(VK_CONTROL, SC_LCTRL, true, false, true, || None);
        st.key(VK_MENU, SC_RALT, true, false, false, || None);
        st.key(VK_CONTROL, SC_LCTRL, false, false, false, || None);
        st.key(VK_MENU, SC_RALT, false, false, false, || None);
        assert_eq!(
            st.keys.take(),
            [
                KeyEvent::key_press(usage::LEFT_CTRL).with_ctrl(true),
                KeyEvent::key_press(usage::RIGHT_ALT),
                KeyEvent::key_release(usage::LEFT_CTRL),
                KeyEvent::key_release(usage::RIGHT_ALT),
            ]
        );
        assert!(st.down.borrow().is_empty());
        // Caught, it is skipped, and its key-up then has no press to release.
        st.key(VK_MENU, SC_RALT, true, false, false, || None);
        st.key(VK_CONTROL, SC_LCTRL, false, false, false, || None);
        st.key(VK_MENU, SC_RALT, false, false, false, || None);
        assert_eq!(
            st.keys.take(),
            [
                KeyEvent::key_press(usage::RIGHT_ALT),
                KeyEvent::key_release(usage::RIGHT_ALT),
            ]
        );
    }

    #[test]
    fn losing_focus_releases_held_keys() {
        let st = State::new();
        st.key(b'W' as u32, SC_W, true, false, false, || Some(b'w'));
        st.key(VK_CONTROL, SC_RCTRL, true, false, true, || None);
        st.key(VK_UP, SC_UP, true, false, true, || None);
        st.key(VK_UP, SC_UP, false, false, true, || None);
        st.key(VK_F1 + 4, SC_F5, true, false, true, || None);
        st.key(VK_LWIN, SC_LWIN, true, false, true, || None);
        st.keys.take();
        st.release_all();
        // Modifiers included, in the order they went down, with no flags.
        assert_eq!(
            st.keys.take(),
            [
                KeyEvent::release(b'w').with_usage(usage::A + 22),
                KeyEvent::key_release(usage::RIGHT_CTRL),
                KeyEvent::release(code::F1 + 4).with_usage(usage::F1 + 4),
                KeyEvent::key_release(usage::LEFT_GUI),
            ]
        );
        st.release_all();
        assert!(st.keys.take().is_empty());
        // Up after the focus came back: no release, it was given already.
        st.key(b'W' as u32, SC_W, false, false, false, || Some(b'w'));
        assert!(st.keys.take().is_empty());
    }

    /// A window resized for a new mode keeps its corner when it fits on the monitor's
    /// work area, and otherwise shrinks to it and moves back onto it.
    #[test]
    fn a_resized_window_stays_on_its_monitor() {
        let rect = |left, top, right, bottom| RECT {
            left,
            top,
            right,
            bottom,
        };
        let pos = |r: RECT| (r.left, r.top, r.right - r.left, r.bottom - r.top);
        // A 1080p monitor with a 48-pixel taskbar; the frame adds 16 x 39.
        let work = Some(rect(0, 0, 1920, 1032));
        let extra = (16, 39);
        let outer = rect(100, 50, 100 + 656, 50 + 519);
        // 800x600 fits where the window is.
        assert_eq!(pos(fitted(outer, extra, 800, 600, work)), (100, 50, 816, 639));
        // 1280x1024 fits across but not down: as tall as the work area, from its top.
        assert_eq!(pos(fitted(outer, extra, 1280, 1024, work)), (100, 0, 1296, 1032));
        // 1920x1080 fits neither way: the whole work area.
        assert_eq!(pos(fitted(outer, extra, 1920, 1080, work)), (0, 0, 1920, 1032));
        // Near the right edge, it moves left as far as it must.
        let right = rect(1500, 50, 1500 + 656, 50 + 519);
        assert_eq!(pos(fitted(right, extra, 800, 600, work)), (1104, 50, 816, 639));
        // A second monitor left of the first, at negative coordinates.
        let left_monitor = Some(rect(-1280, 0, 0, 984));
        let on_it = rect(-1200, 100, -1200 + 656, 100 + 519);
        assert_eq!(
            pos(fitted(on_it, extra, 1280, 720, left_monitor)),
            (-1280, 100, 1280, 759)
        );
        // No monitor known: just the size asked for.
        assert_eq!(
            pos(fitted(right, extra, 1920, 1080, None)),
            (1500, 50, 1936, 1119)
        );
    }

    #[test]
    fn client_points_map_to_framebuffer_pixels() {
        // The window's usual size: one to one.
        assert_eq!(client_to_fb(0, 0, 640, 480, VGA), Some((0, 0)));
        assert_eq!(client_to_fb(639, 479, 640, 480, VGA), Some((639, 479)));
        assert_eq!(client_to_fb(123, 45, 640, 480, VGA), Some((123, 45)));
        // Stretched to twice the size: two client pixels per frame pixel.
        assert_eq!(client_to_fb(1, 1, 1280, 960, VGA), Some((0, 0)));
        assert_eq!(client_to_fb(2, 3, 1280, 960, VGA), Some((1, 1)));
        assert_eq!(client_to_fb(1279, 959, 1280, 960, VGA), Some((639, 479)));
        // Shrunk, and stretched unevenly: the frame pixel under each client pixel's
        // centre.
        assert_eq!(client_to_fb(0, 0, 320, 240, VGA), Some((1, 1)));
        assert_eq!(client_to_fb(319, 239, 320, 240, VGA), Some((639, 479)));
        assert_eq!(client_to_fb(100, 100, 1000, 200, VGA), Some((64, 241)));
        assert_eq!(client_to_fb(1, 0, 960, 720, VGA), Some((1, 0)));
        // Outside the client area (a captured drag): outside the frame, rounded down,
        // for the device to clamp.
        assert_eq!(client_to_fb(-1, -3, 1280, 960, VGA), Some((-1, -2)));
        assert_eq!(client_to_fb(1280, 2000, 1280, 960, VGA), Some((640, 1000)));
        // Minimized.
        assert_eq!(client_to_fb(5, 5, 0, 0, VGA), None);
        assert_eq!(client_to_fb(5, 5, 640, 0, VGA), None);
        // Another mode: its pixels, whatever the window's size.
        assert_eq!(client_to_fb(639, 479, 640, 480, (1920, 1080)), Some((1918, 1078)));
        assert_eq!(client_to_fb(799, 599, 800, 600, (800, 600)), Some((799, 599)));
    }

    /// The size of the default mode's frame.
    const VGA: (u32, u32) = (FB_WIDTH, FB_HEIGHT);

    /// `client_to_fb` is what `stretch` (the drawing) shows: a frame whose pixels hold
    /// their own coordinates is stretched into a memory bitmap of each size, and every
    /// pixel of it must be the frame pixel `client_to_fb` names, for a frame of the
    /// default mode and of the largest. Needs no desktop.
    #[test]
    fn client_to_fb_matches_the_stretch() {
        for (fw, fh) in [VGA, (1920, 1080)] {
            let mut pixels = Vec::with_capacity((fw * fh * 4) as usize);
            for y in 0..fh {
                for x in 0..fw {
                    pixels.extend_from_slice(&(x | y << 12).to_le_bytes());
                }
            }
            let frame = Frame {
                width: fw,
                height: fh,
                pixels: Some(&pixels),
            };
            stretch_matches(frame);
        }
    }

    fn stretch_matches(frame: Frame) {
        for (width, height) in [
            (640, 480),
            (1280, 960),
            (960, 720),
            (320, 240),
            (1000, 200),
            (700, 500),
            (641, 481),
            (1366, 705),
            (213, 1001),
        ] {
            let bmi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: width,
                    biHeight: -height, // top-down, like the frame
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
            // SAFETY: a memory DC and a DIB section of the size `bmi` gives, both freed
            // before the next size; `bits` is read only while the section exists.
            unsafe {
                let dc = CreateCompatibleDC(null_mut());
                assert!(!dc.is_null());
                let mut bits: *mut c_void = null_mut();
                let bitmap = CreateDIBSection(dc, &bmi, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
                assert!(!bitmap.is_null() && !bits.is_null());
                let old = SelectObject(dc, bitmap);
                stretch(dc, width, height, frame);
                let shown = std::slice::from_raw_parts(bits as *const u32, (width * height) as usize);
                let mut wrong = 0;
                let mut first = None;
                for (i, &px) in shown.iter().enumerate() {
                    let (x, y) = (i as i32 % width, i as i32 / width);
                    let at = (px as i32 & 0xfff, px as i32 >> 12);
                    if client_to_fb(x, y, width, height, (frame.width, frame.height)) != Some(at) {
                        wrong += 1;
                        first.get_or_insert((x, y, at));
                    }
                }
                SelectObject(dc, old);
                DeleteObject(bitmap);
                DeleteDC(dc);
                assert_eq!(
                    wrong, 0,
                    "{}x{} on {width}x{height}: first (client x, client y, frame pixel shown) {first:?}",
                    frame.width, frame.height
                );
            }
        }
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
        // A turn the other way drops what was left of the last direction (10).
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
        assert_eq!(st.wheel.get(), -20, "the rest waits for the next notch");
    }

    #[test]
    fn turning_the_wheel_back_is_never_swallowed() {
        let st = State::new();
        // Most of a notch up on a precise touchpad, then one standard notch down.
        st.mouse(None, 0, 100);
        st.mouse(None, 0, -WHEEL_DELTA);
        assert_eq!(st.pointer.take(), [PointerInput::default().with_wheel(-1)]);
        st.mouse(None, 0, -100);
        st.mouse(None, 0, WHEEL_DELTA);
        assert_eq!(st.pointer.take(), [PointerInput::default().with_wheel(1)]);
        assert_eq!(st.wheel.get(), 0);
    }

    /// lParam of a client-area mouse message: x in the low word, y in the high word.
    fn mouse_lparam(x: i32, y: i32) -> LPARAM {
        (x as u16 as u32 | (y as u16 as u32) << 16) as i32 as LPARAM
    }

    /// The window procedure's mouse arm, without a window: which messages become which
    /// pointer input, and when the window wants the mouse captured.
    #[test]
    fn mouse_messages_become_pointer_input() {
        let st = State::new();
        let msg = |m: u32, wparam: usize, lparam: LPARAM| st.mouse_message(m, wparam, lparam, 1280, 960);
        assert_eq!(msg(WM_MOUSEMOVE, 0, mouse_lparam(201, 101)), Some(false));
        assert_eq!(
            msg(WM_LBUTTONDOWN, MK_LBUTTON, mouse_lparam(201, 101)),
            Some(true)
        );
        // Dragged out of the window: negative client coordinates.
        assert_eq!(msg(WM_MOUSEMOVE, MK_LBUTTON, mouse_lparam(-3, 2000)), Some(true));
        assert_eq!(
            msg(WM_RBUTTONDOWN, MK_LBUTTON | MK_RBUTTON, mouse_lparam(-3, 2000)),
            Some(true)
        );
        assert_eq!(msg(WM_LBUTTONUP, MK_RBUTTON, mouse_lparam(-3, 2000)), Some(true));
        assert_eq!(msg(WM_RBUTTONUP, 0, mouse_lparam(-3, 2000)), Some(false));
        let wheel_up = (2 * WHEEL_DELTA as usize) << 16 | MK_MBUTTON;
        assert_eq!(msg(WM_MOUSEWHEEL, wheel_up, mouse_lparam(9999, 9999)), None);
        assert_eq!(msg(WM_MBUTTONDOWN, MK_MBUTTON, mouse_lparam(0, 0)), Some(true));
        // Other messages are not pointer input.
        assert_eq!(msg(WM_PAINT, MK_LBUTTON, mouse_lparam(7, 7)), None);
        assert_eq!(
            st.pointer.take(),
            [
                PointerInput::move_to(100, 50),
                PointerInput::move_to(100, 50).with_buttons(BUTTON_LEFT),
                PointerInput::move_to(-2, 1000).with_buttons(BUTTON_LEFT),
                PointerInput::move_to(-2, 1000).with_buttons(BUTTON_LEFT | BUTTON_RIGHT),
                PointerInput::move_to(-2, 1000).with_buttons(BUTTON_RIGHT),
                PointerInput::move_to(-2, 1000),
                PointerInput::buttons(BUTTON_MIDDLE).with_wheel(2),
                PointerInput::move_to(0, 0).with_buttons(BUTTON_MIDDLE),
            ]
        );

        // Minimized: no position, but a button change still counts.
        let min = |m: u32, wparam: usize| st.mouse_message(m, wparam, mouse_lparam(3, 3), 0, 0);
        assert_eq!(min(WM_MOUSEMOVE, MK_MBUTTON), Some(true));
        assert!(st.pointer.take().is_empty(), "nothing changed");
        assert_eq!(min(WM_RBUTTONDOWN, MK_MBUTTON | MK_RBUTTON), Some(true));
        assert_eq!(
            st.pointer.take(),
            [PointerInput::buttons(BUTTON_MIDDLE | BUTTON_RIGHT)]
        );
    }

    #[test]
    fn losing_the_capture_or_the_focus_releases_buttons() {
        let st = State::new();
        let msg = |m: u32, wparam: usize| st.mouse_message(m, wparam, mouse_lparam(10, 10), 640, 480);
        msg(WM_LBUTTONDOWN, MK_LBUTTON);
        st.pointer.take();
        assert_eq!(msg(WM_CAPTURECHANGED, 0), None);
        assert_eq!(st.pointer.take(), [PointerInput::buttons(0)]);

        msg(WM_RBUTTONDOWN, MK_RBUTTON);
        st.key(VK_UP, SC_UP, true, false, false, || None);
        st.pointer.take();
        st.keys.take();
        assert_eq!(msg(WM_KILLFOCUS, 0), None);
        assert_eq!(st.pointer.take(), [PointerInput::buttons(0)]);
        assert_eq!(
            st.keys.take(),
            [KeyEvent::release(code::UP).with_usage(usage::UP)],
            "keys go too"
        );
        msg(WM_KILLFOCUS, 0);
        msg(WM_CAPTURECHANGED, 0);
        assert!(st.pointer.take().is_empty(), "nothing held, nothing to release");
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

    /// A `width` × `height` frame of a display that is off.
    fn off(width: u32, height: u32) -> Frame<'static> {
        Frame {
            width,
            height,
            pixels: None,
        }
    }

    /// The size of `w`'s client area.
    fn client_size(w: &Window) -> (i32, i32) {
        let mut client = RECT::default();
        // SAFETY: a valid window handle and a RECT it fills in.
        unsafe { GetClientRect(w.hwnd, &mut client) };
        (client.right - client.left, client.bottom - client.top)
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
            w.draw(Frame {
                width: FB_WIDTH,
                height: FB_HEIGHT,
                pixels: Some(&pixels),
            });
            std::thread::sleep(std::time::Duration::from_millis(16));
        }
        w.draw(off(FB_WIDTH, FB_HEIGHT));
        assert!(w.pump());
        // A frame of another mode resizes the window to it, and maps the mouse to it.
        w.draw(off(800, 600));
        assert!(w.pump());
        assert_eq!(client_size(&w), (800, 600));
        assert_eq!(w.frame_size(), (800, 600));
        // One bigger than any monitor stays on the monitor's work area.
        w.draw(off(8000, 8000));
        assert!(w.pump());
        // SAFETY: a valid window handle, and structs the calls fill in.
        let (outer, work) = unsafe {
            let mut outer = RECT::default();
            GetWindowRect(w.hwnd, &mut outer);
            let mut info = MONITORINFO {
                cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                ..MONITORINFO::default()
            };
            let on = MonitorFromWindow(w.hwnd, MONITOR_DEFAULTTONEAREST);
            assert_ne!(GetMonitorInfoW(on, &mut info), 0);
            (outer, info.rcWork)
        };
        assert!(
            outer.left >= work.left
                && outer.top >= work.top
                && outer.right <= work.right
                && outer.bottom <= work.bottom,
            "window {:?} outside the work area {:?}",
            (outer.left, outer.top, outer.right, outer.bottom),
            (work.left, work.top, work.right, work.bottom)
        );
        assert_eq!(w.frame_size(), (8000, 8000), "the mouse maps to the frame");
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
        b.draw(off(FB_WIDTH, FB_HEIGHT));
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
        // Left Shift is down in the key state too, or the pump would release it.
        hold_keys(&[VK_SHIFT, VK_LSHIFT]);
        send(&a, WM_KEYDOWN, VK_SHIFT, 0x2a);
        send(&a, WM_KEYUP, VK_RETURN, 0x1c);
        send(&a, WM_KEYDOWN, VK_SPACE, 0x39);
        // F10 arrives as a system key.
        send(&b, WM_SYSKEYDOWN, VK_F10, 0x44);
        assert!(a.pump() && b.pump());
        assert_eq!(
            a.take_keys(),
            [
                KeyEvent::press(b'a').with_usage(usage::A),
                KeyEvent::release(b'a').with_usage(usage::A),
                KeyEvent::press(code::ENTER).with_usage(usage::ENTER),
                KeyEvent::key_press(usage::LEFT_SHIFT).with_shift(true),
                KeyEvent::release(code::ENTER)
                    .with_usage(usage::ENTER)
                    .with_shift(true),
                KeyEvent::press(b' ').with_usage(usage::SPACE).with_shift(true),
            ]
        );
        assert_eq!(
            b.take_keys(),
            [
                KeyEvent::press(code::F1 + 1).with_usage(usage::F1 + 1),
                // Shift is down in the thread's key state, which both windows read.
                KeyEvent::press(code::F1 + 9)
                    .with_usage(usage::F1 + 9)
                    .with_shift(true),
            ]
        );
        assert!(a.take_keys().is_empty());
        // SAFETY: a valid window handle.
        unsafe { SendMessageW(a.hwnd, WM_KILLFOCUS, 0, 0) };
        assert_eq!(
            a.take_keys(),
            [
                KeyEvent::key_release(usage::LEFT_SHIFT),
                KeyEvent::release(b' ').with_usage(usage::SPACE),
            ]
        );
        hold_keys(&[]);
    }

    /// The pump releases a modifier the thread's key state has up though its key-up
    /// never came, as Windows does with both Shifts down. Needs a desktop session:
    /// `cargo test -p grail --release -- --ignored window`.
    #[test]
    #[ignore]
    fn window_releases_modifiers_windows_lost() {
        let _saved = SavedKeys::save();
        let mut w = Window::open("Grail lost key-up test").unwrap();
        assert!(w.pump());
        w.take_keys();
        hold_keys(&[VK_SHIFT, VK_LSHIFT, VK_RSHIFT]);
        send(&w, WM_KEYDOWN, VK_SHIFT, 0x2a);
        send(&w, WM_KEYDOWN, VK_SHIFT, 0x36);
        assert!(w.pump());
        // Left Shift let go: no key-up, only the key state.
        hold_keys(&[VK_SHIFT, VK_RSHIFT]);
        assert!(w.pump());
        hold_keys(&[]);
        send(&w, WM_KEYUP, VK_SHIFT, 0x36);
        assert!(w.pump());
        assert_eq!(
            w.take_keys(),
            [
                KeyEvent::key_press(usage::LEFT_SHIFT).with_shift(true),
                KeyEvent::key_press(usage::RIGHT_SHIFT).with_shift(true),
                KeyEvent::key_release(usage::LEFT_SHIFT),
                KeyEvent::key_release(usage::RIGHT_SHIFT),
            ]
        );
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
        let (a, c) = (usage::A, usage::A + 2);
        assert_eq!(
            w.take_keys(),
            [
                KeyEvent::press(b'A').with_usage(a).with_shift(true),
                KeyEvent::release(b'A').with_usage(a).with_shift(true),
                KeyEvent::press(b'c').with_usage(c).with_ctrl(true),
                KeyEvent::release(b'c').with_usage(c).with_ctrl(true),
                KeyEvent::press(code::LEFT)
                    .with_usage(usage::LEFT)
                    .with_shift(true)
                    .with_ctrl(true),
                KeyEvent::release(code::LEFT)
                    .with_usage(usage::LEFT)
                    .with_shift(true)
                    .with_ctrl(true),
                KeyEvent::press(code::RIGHT).with_usage(usage::RIGHT),
                KeyEvent::release(code::RIGHT).with_usage(usage::RIGHT),
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
        // In ASCII mode, and in raw mode, where the window keeps no key for itself.
        for raw in [false, true] {
            w.set_raw_keys(raw);
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
                let u = usage::from_win_scan(scan2 as u16).unwrap();
                let first = match vk {
                    VK_F10 => KeyEvent::press(code::F1 + 9).with_usage(usage::F1 + 9),
                    _ => KeyEvent::key_press(usage::LEFT_ALT),
                };
                let want = [
                    first,
                    KeyEvent {
                        release: true,
                        ..first
                    },
                    KeyEvent::press(c).with_usage(u),
                    KeyEvent::release(c).with_usage(u),
                ];
                assert_eq!(w.take_keys(), want, "the key after vk {vk:#x}, raw {raw}");
            }
        }
        let took = start.elapsed();
        let _ = done.send(());
        rescue.join().unwrap();
        assert!(took < std::time::Duration::from_millis(500), "took {took:?}");
    }

    /// In raw mode Alt+F4 is the program's key; in ASCII mode it closes the window, as
    /// before. Needs a desktop session: `cargo test -p grail --release -- --ignored
    /// window`.
    #[test]
    #[ignore]
    fn window_alt_f4_is_the_programs_in_raw_mode() {
        let _saved = SavedKeys::save();
        let mut w = Window::open("Grail Alt+F4 test").unwrap();
        assert!(w.pump());
        w.take_keys();
        let f4 = KeyEvent::press(code::F1 + 3).with_usage(usage::F1 + 3);
        let alt_f4 = |w: &Window| {
            hold_keys(&[VK_MENU, VK_LMENU]);
            send(w, WM_SYSKEYDOWN, VK_F1 + 3, 0x3e);
            send(w, WM_SYSKEYUP, VK_F1 + 3, 0x3e);
            hold_keys(&[]);
        };
        w.set_raw_keys(true);
        alt_f4(&w);
        assert!(w.pump(), "still open");
        assert_eq!(w.take_keys(), [f4, KeyEvent { release: true, ..f4 }]);
        w.set_raw_keys(false);
        alt_f4(&w);
        assert!(!w.pump(), "Alt+F4 closes the window in ASCII mode");
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
        // Losing the focus in the middle of a click releases the button too.
        send(&w, WM_RBUTTONDOWN, MK_RBUTTON, mouse_lparam(5, 6));
        w.take_pointer();
        send(&w, WM_KILLFOCUS, 0, 0);
        assert_eq!(w.take_pointer(), [PointerInput::buttons(0)]);
        send(&w, WM_RBUTTONUP, 0, mouse_lparam(5, 6));
        assert!(!captured(&w));
        w.take_pointer();

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
        send(&w, WM_MOUSEMOVE, 0, mouse_lparam(1, 1));
        send(&w, WM_MOUSEMOVE, 0, mouse_lparam(201, 101));
        send(&w, WM_MOUSEMOVE, 0, mouse_lparam(959, 719));
        assert_eq!(
            w.take_pointer(),
            [
                PointerInput::move_to(1, 1),
                PointerInput::move_to(134, 67),
                PointerInput::move_to(639, 479)
            ]
        );
    }
}
