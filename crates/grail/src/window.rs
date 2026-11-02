//! A native window that shows the framebuffer, through raw Win32 GDI calls.
//!
//! Single-threaded: the emulator runs on the thread that owns the window and calls
//! `pump` between slices of instructions. The window procedure cannot reach the
//! framebuffer, so it only records what happened (repaint needed, window destroyed)
//! in thread-locals and the owner redraws.

use crate::framebuffer::{FB_HEIGHT, FB_WIDTH};
use crate::win32::*;
use std::cell::Cell;
use std::ffi::c_void;
use std::ptr::null_mut;
use std::sync::OnceLock;

const CLASS_NAME: &str = "GrailFramebuffer";

thread_local! {
    static REPAINT: Cell<bool> = const { Cell::new(false) };
    static DESTROYED: Cell<bool> = const { Cell::new(false) };
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => {
            let mut ps: PAINTSTRUCT = std::mem::zeroed();
            BeginPaint(hwnd, &mut ps);
            EndPaint(hwnd, &ps);
            REPAINT.with(|r| r.set(true));
            0
        }
        WM_DESTROY => {
            DESTROYED.with(|d| d.set(true));
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

pub struct Window {
    hwnd: HWND,
    open: bool,
}

impl Window {
    /// Opens a window whose client area is the framebuffer's size.
    pub fn open(title: &str) -> Result<Window, String> {
        let class = wide(CLASS_NAME);
        // SAFETY: plain Win32 calls with valid, NUL-terminated strings and zeroed structs.
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
            DESTROYED.with(|d| d.set(false));
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
                null_mut(),
            );
            if hwnd.is_null() {
                return Err("CreateWindowExW failed".into());
            }
            Ok(Window { hwnd, open: true })
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
        if DESTROYED.with(|d| d.get()) {
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
        REPAINT.with(|r| r.replace(false))
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
            // Drain the WM_DESTROY / WM_QUIT this produces so they do not leak into a
            // later window on this thread.
            self.pump();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
