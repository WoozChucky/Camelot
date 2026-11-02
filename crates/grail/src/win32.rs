//! The few Win32 (user32/gdi32/kernel32) declarations the framebuffer window and the
//! console's raw mode need, written by hand instead of pulling in a bindings crate.
//! Names and layouts follow the Windows SDK headers (winuser.h, wingdi.h, consoleapi.h).

#![allow(non_snake_case, clippy::upper_case_acronyms)]

use std::ffi::c_void;

pub type HANDLE = *mut c_void;
pub type HWND = HANDLE;
pub type HDC = HANDLE;
pub type HINSTANCE = HANDLE;
pub type HICON = HANDLE;
pub type HCURSOR = HANDLE;
pub type HBRUSH = HANDLE;
pub type HMENU = HANDLE;
pub type WPARAM = usize;
pub type LPARAM = isize;
pub type LRESULT = isize;
pub type BOOL = i32;
pub type WNDPROC = Option<unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT>;

pub const CS_VREDRAW: u32 = 0x0001;
pub const CS_HREDRAW: u32 = 0x0002;
pub const WS_OVERLAPPEDWINDOW: u32 = 0x00cf_0000;
pub const WS_VISIBLE: u32 = 0x1000_0000;
pub const CW_USEDEFAULT: i32 = 0x8000_0000_u32 as i32;
pub const IDC_ARROW: u16 = 32512;

pub const WM_DESTROY: u32 = 0x0002;
pub const WM_KILLFOCUS: u32 = 0x0008;
pub const WM_CLOSE: u32 = 0x0010;
pub const WM_PAINT: u32 = 0x000f;
pub const WM_NCCREATE: u32 = 0x0081;
pub const WM_NCDESTROY: u32 = 0x0082;
pub const WM_KEYDOWN: u32 = 0x0100;
pub const WM_KEYUP: u32 = 0x0101;
pub const WM_SYSKEYDOWN: u32 = 0x0104;
pub const WM_SYSKEYUP: u32 = 0x0105;
pub const WM_SYSCOMMAND: u32 = 0x0112;
pub const WM_MOUSEMOVE: u32 = 0x0200;
pub const WM_LBUTTONDOWN: u32 = 0x0201;
pub const WM_LBUTTONUP: u32 = 0x0202;
pub const WM_RBUTTONDOWN: u32 = 0x0204;
pub const WM_RBUTTONUP: u32 = 0x0205;
pub const WM_MBUTTONDOWN: u32 = 0x0207;
pub const WM_MBUTTONUP: u32 = 0x0208;
pub const WM_MOUSEWHEEL: u32 = 0x020a;
pub const WM_CAPTURECHANGED: u32 = 0x0215;
/// Mouse message `wParam` flags: the buttons held.
pub const MK_LBUTTON: usize = 0x0001;
pub const MK_RBUTTON: usize = 0x0002;
pub const MK_MBUTTON: usize = 0x0010;
/// `WM_MOUSEWHEEL` units in one notch of a wheel.
pub const WHEEL_DELTA: i32 = 120;
/// `WM_SYSCOMMAND` request to open the window menu from the keyboard. Its `lParam` is
/// the character of an Alt chord (Alt+Space), or 0 for a lone Alt tap or F10.
pub const SC_KEYMENU: usize = 0xf100;
pub const PM_REMOVE: u32 = 0x0001;

pub const GWLP_USERDATA: i32 = -21;

// Virtual-key codes (winuser.h). Letters and digits are their ASCII uppercase codes.
pub const VK_BACK: u32 = 0x08;
pub const VK_TAB: u32 = 0x09;
pub const VK_RETURN: u32 = 0x0d;
pub const VK_SHIFT: u32 = 0x10;
pub const VK_CONTROL: u32 = 0x11;
pub const VK_MENU: u32 = 0x12;
pub const VK_ESCAPE: u32 = 0x1b;
#[cfg(test)]
pub const VK_SPACE: u32 = 0x20;
#[cfg(test)]
pub const VK_LSHIFT: u32 = 0xa0;
#[cfg(test)]
pub const VK_LMENU: u32 = 0xa4;
pub const VK_LEFT: u32 = 0x25;
pub const VK_UP: u32 = 0x26;
pub const VK_RIGHT: u32 = 0x27;
pub const VK_DOWN: u32 = 0x28;
pub const VK_F1: u32 = 0x70;
#[cfg(test)]
pub const VK_F10: u32 = 0x79;
pub const VK_F12: u32 = 0x7b;
pub const VK_LCONTROL: u32 = 0xa2;
pub const VK_RCONTROL: u32 = 0xa3;

/// `ToUnicode` flag: do not change the keyboard state (dead keys), Windows 10 1607+.
pub const TOUNICODE_KEEP_STATE: u32 = 1 << 2;

pub const BI_RGB: u32 = 0;
pub const DIB_RGB_COLORS: u32 = 0;
pub const SRCCOPY: u32 = 0x00cc_0020;
pub const COLORONCOLOR: i32 = 3;

// Console input (consoleapi.h, processenv.h).
/// `GetStdHandle` argument: `(DWORD)-10`.
pub const STD_INPUT_HANDLE: u32 = -10i32 as u32;
/// `GetStdHandle` argument: `(DWORD)-11`.
pub const STD_OUTPUT_HANDLE: u32 = -11i32 as u32;
pub const ENABLE_LINE_INPUT: u32 = 0x0002;
pub const ENABLE_ECHO_INPUT: u32 = 0x0004;
/// Input mode: keys arrive as the VT sequences a Unix terminal sends (Windows 10+).
pub const ENABLE_VIRTUAL_TERMINAL_INPUT: u32 = 0x0200;
/// Output mode (same bit as `ENABLE_ECHO_INPUT`, on the other buffer): VT escape
/// sequences written to the console are carried out (Windows 10+).
pub const ENABLE_VIRTUAL_TERMINAL_PROCESSING: u32 = 0x0004;
/// A `SetConsoleCtrlHandler` routine: gets the event, returns TRUE if it handled it.
#[allow(non_camel_case_types)]
pub type PHANDLER_ROUTINE = Option<unsafe extern "system" fn(u32) -> BOOL>;

#[repr(C)]
pub struct WNDCLASSEXW {
    pub cbSize: u32,
    pub style: u32,
    pub lpfnWndProc: WNDPROC,
    pub cbClsExtra: i32,
    pub cbWndExtra: i32,
    pub hInstance: HINSTANCE,
    pub hIcon: HICON,
    pub hCursor: HCURSOR,
    pub hbrBackground: HBRUSH,
    pub lpszMenuName: *const u16,
    pub lpszClassName: *const u16,
    pub hIconSm: HICON,
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
pub struct POINT {
    pub x: i32,
    pub y: i32,
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
pub struct RECT {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

#[repr(C)]
pub struct MSG {
    pub hwnd: HWND,
    pub message: u32,
    pub wParam: WPARAM,
    pub lParam: LPARAM,
    pub time: u32,
    pub pt: POINT,
}

/// What `CreateWindowExW` was called with; `lParam` of `WM_NCCREATE` points at it.
#[repr(C)]
pub struct CREATESTRUCTW {
    pub lpCreateParams: *mut c_void,
    pub hInstance: HINSTANCE,
    pub hMenu: HMENU,
    pub hwndParent: HWND,
    pub cy: i32,
    pub cx: i32,
    pub y: i32,
    pub x: i32,
    pub style: i32,
    pub lpszName: *const u16,
    pub lpszClass: *const u16,
    pub dwExStyle: u32,
}

#[repr(C)]
pub struct PAINTSTRUCT {
    pub hdc: HDC,
    pub fErase: BOOL,
    pub rcPaint: RECT,
    pub fRestore: BOOL,
    pub fIncUpdate: BOOL,
    pub rgbReserved: [u8; 32],
}

#[repr(C)]
pub struct BITMAPINFOHEADER {
    pub biSize: u32,
    pub biWidth: i32,
    /// Negative height = top-down rows.
    pub biHeight: i32,
    pub biPlanes: u16,
    pub biBitCount: u16,
    pub biCompression: u32,
    pub biSizeImage: u32,
    pub biXPelsPerMeter: i32,
    pub biYPelsPerMeter: i32,
    pub biClrUsed: u32,
    pub biClrImportant: u32,
}

#[repr(C)]
pub struct BITMAPINFO {
    pub bmiHeader: BITMAPINFOHEADER,
    /// RGBQUAD color table; unused for 32-bpp BI_RGB.
    pub bmiColors: [u32; 1],
}

#[link(name = "kernel32")]
extern "system" {
    pub fn GetModuleHandleW(lpModuleName: *const u16) -> HINSTANCE;
    pub fn GetStdHandle(nStdHandle: u32) -> HANDLE;
    pub fn GetConsoleMode(hConsoleHandle: HANDLE, lpMode: *mut u32) -> BOOL;
    pub fn SetConsoleMode(hConsoleHandle: HANDLE, dwMode: u32) -> BOOL;
    pub fn SetConsoleCtrlHandler(HandlerRoutine: PHANDLER_ROUTINE, Add: BOOL) -> BOOL;
}

#[link(name = "user32")]
extern "system" {
    pub fn RegisterClassExW(wc: *const WNDCLASSEXW) -> u16;
    pub fn CreateWindowExW(
        dwExStyle: u32,
        lpClassName: *const u16,
        lpWindowName: *const u16,
        dwStyle: u32,
        x: i32,
        y: i32,
        nWidth: i32,
        nHeight: i32,
        hWndParent: HWND,
        hMenu: HMENU,
        hInstance: HINSTANCE,
        lpParam: *mut c_void,
    ) -> HWND;
    pub fn DestroyWindow(hwnd: HWND) -> BOOL;
    pub fn DefWindowProcW(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT;
    pub fn PeekMessageW(msg: *mut MSG, hwnd: HWND, min: u32, max: u32, remove: u32) -> BOOL;
    pub fn TranslateMessage(msg: *const MSG) -> BOOL;
    pub fn DispatchMessageW(msg: *const MSG) -> LRESULT;
    pub fn WaitMessage() -> BOOL;
    pub fn AdjustWindowRect(rect: *mut RECT, style: u32, menu: BOOL) -> BOOL;
    pub fn GetClientRect(hwnd: HWND, rect: *mut RECT) -> BOOL;
    pub fn SetCapture(hwnd: HWND) -> HWND;
    pub fn ReleaseCapture() -> BOOL;
    pub fn GetCapture() -> HWND;
    #[cfg(test)]
    pub fn MoveWindow(hwnd: HWND, x: i32, y: i32, width: i32, height: i32, repaint: BOOL) -> BOOL;
    #[cfg(test)]
    pub fn GetWindowRect(hwnd: HWND, rect: *mut RECT) -> BOOL;
    pub fn SetWindowTextW(hwnd: HWND, text: *const u16) -> BOOL;
    pub fn LoadCursorW(hinst: HINSTANCE, name: *const u16) -> HCURSOR;
    pub fn GetDC(hwnd: HWND) -> HDC;
    pub fn ReleaseDC(hwnd: HWND, hdc: HDC) -> i32;
    pub fn BeginPaint(hwnd: HWND, ps: *mut PAINTSTRUCT) -> HDC;
    pub fn EndPaint(hwnd: HWND, ps: *const PAINTSTRUCT) -> BOOL;
    #[cfg(test)]
    pub fn SendMessageW(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT;
    pub fn PostMessageW(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> BOOL;
    pub fn FindWindowW(class: *const u16, title: *const u16) -> HWND;
    pub fn SetKeyboardState(state: *const u8) -> BOOL;
    pub fn GetKeyState(vkey: i32) -> i16;
    pub fn GetKeyboardState(state: *mut u8) -> BOOL;
    pub fn ToUnicode(vkey: u32, scan: u32, state: *const u8, buf: *mut u16, len: i32, flags: u32) -> i32;
    #[cfg(target_pointer_width = "64")]
    pub fn SetWindowLongPtrW(hwnd: HWND, index: i32, value: isize) -> isize;
    #[cfg(target_pointer_width = "64")]
    pub fn GetWindowLongPtrW(hwnd: HWND, index: i32) -> isize;
    #[cfg(target_pointer_width = "32")]
    #[link_name = "SetWindowLongW"]
    pub fn SetWindowLongPtrW(hwnd: HWND, index: i32, value: isize) -> isize;
    #[cfg(target_pointer_width = "32")]
    #[link_name = "GetWindowLongW"]
    pub fn GetWindowLongPtrW(hwnd: HWND, index: i32) -> isize;
}

#[link(name = "gdi32")]
extern "system" {
    #[cfg(test)]
    pub fn CreateCompatibleDC(hdc: HDC) -> HDC;
    #[cfg(test)]
    pub fn DeleteDC(hdc: HDC) -> BOOL;
    #[cfg(test)]
    pub fn CreateDIBSection(
        hdc: HDC,
        pbmi: *const BITMAPINFO,
        usage: u32,
        ppvBits: *mut *mut c_void,
        hSection: HANDLE,
        offset: u32,
    ) -> HANDLE;
    #[cfg(test)]
    pub fn SelectObject(hdc: HDC, h: HANDLE) -> HANDLE;
    #[cfg(test)]
    pub fn DeleteObject(h: HANDLE) -> BOOL;
    pub fn SetStretchBltMode(hdc: HDC, mode: i32) -> i32;
    pub fn StretchDIBits(
        hdc: HDC,
        xDest: i32,
        yDest: i32,
        DestWidth: i32,
        DestHeight: i32,
        xSrc: i32,
        ySrc: i32,
        SrcWidth: i32,
        SrcHeight: i32,
        lpBits: *const c_void,
        lpbmi: *const BITMAPINFO,
        iUsage: u32,
        rop: u32,
    ) -> i32;
}

/// NUL-terminated UTF-16, for the `W` functions.
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::size_of;

    /// Sizes from the Windows SDK headers; a mismatch means a wrong declaration.
    #[test]
    fn struct_layouts_match_the_sdk() {
        #[cfg(target_pointer_width = "64")]
        {
            assert_eq!(size_of::<WNDCLASSEXW>(), 80);
            assert_eq!(size_of::<MSG>(), 48);
            assert_eq!(size_of::<PAINTSTRUCT>(), 72);
            assert_eq!(size_of::<CREATESTRUCTW>(), 80);
        }
        #[cfg(target_pointer_width = "32")]
        {
            assert_eq!(size_of::<WNDCLASSEXW>(), 48);
            assert_eq!(size_of::<MSG>(), 28);
            assert_eq!(size_of::<PAINTSTRUCT>(), 64);
            assert_eq!(size_of::<CREATESTRUCTW>(), 48);
        }
        assert_eq!(size_of::<RECT>(), 16);
        assert_eq!(size_of::<BITMAPINFOHEADER>(), 40);
        assert_eq!(size_of::<BITMAPINFO>(), 44);
    }

    #[test]
    fn wide_strings_are_nul_terminated() {
        assert_eq!(wide("Grail"), [71, 114, 97, 105, 108, 0]);
    }
}
