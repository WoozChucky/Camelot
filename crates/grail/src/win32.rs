//! The few Win32 (user32/gdi32/kernel32) declarations the framebuffer window needs,
//! written by hand instead of pulling in a bindings crate. Names and layouts follow
//! the Windows SDK headers (winuser.h, wingdi.h).

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
pub const WM_PAINT: u32 = 0x000f;
pub const WM_NCCREATE: u32 = 0x0081;
pub const WM_NCDESTROY: u32 = 0x0082;
pub const PM_REMOVE: u32 = 0x0001;
pub const GWLP_USERDATA: i32 = -21;

pub const BI_RGB: u32 = 0;
pub const DIB_RGB_COLORS: u32 = 0;
pub const SRCCOPY: u32 = 0x00cc_0020;
pub const COLORONCOLOR: i32 = 3;

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
    pub lPrivate: u32,
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
    pub fn SetWindowTextW(hwnd: HWND, text: *const u16) -> BOOL;
    pub fn LoadCursorW(hinst: HINSTANCE, name: *const u16) -> HCURSOR;
    pub fn GetDC(hwnd: HWND) -> HDC;
    pub fn ReleaseDC(hwnd: HWND, hdc: HDC) -> i32;
    pub fn BeginPaint(hwnd: HWND, ps: *mut PAINTSTRUCT) -> HDC;
    pub fn EndPaint(hwnd: HWND, ps: *const PAINTSTRUCT) -> BOOL;
}

// The `...LongPtrW` functions exist only in 64-bit user32; in 32-bit builds the SDK
// headers map them to the plain `...LongW` ones, which take a LONG.
#[cfg(target_pointer_width = "64")]
#[link(name = "user32")]
extern "system" {
    pub fn GetWindowLongPtrW(hwnd: HWND, index: i32) -> isize;
    pub fn SetWindowLongPtrW(hwnd: HWND, index: i32, value: isize) -> isize;
}

#[cfg(target_pointer_width = "32")]
#[link(name = "user32")]
extern "system" {
    #[link_name = "GetWindowLongW"]
    pub fn GetWindowLongPtrW(hwnd: HWND, index: i32) -> isize;
    #[link_name = "SetWindowLongW"]
    pub fn SetWindowLongPtrW(hwnd: HWND, index: i32, value: isize) -> isize;
}

#[link(name = "gdi32")]
extern "system" {
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
            assert_eq!(size_of::<MSG>(), 32);
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
