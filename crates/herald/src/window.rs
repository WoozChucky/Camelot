//! The [`View`] of `herald --screen` on Windows: Grail's own framebuffer window
//! (`grail::window`, hand-written Win32 FFI), shared rather than written again. It
//! shows the remote screen stretched over its client area as Grail shows a local one,
//! and its keys and mouse, already turned into keyboard events and pointer input in
//! framebuffer pixels by that window, go to the VM.

use crate::screen::{key_from_grail, pointer_from_grail, Input, View};
use avalon_proto::{Screen, VmState};
use grail::framebuffer::{FB_HEIGHT, FB_WIDTH};
use grail::window::Window;

/// A window onto a VM's screen.
pub struct WindowView {
    window: Window,
    title: String,
}

impl WindowView {
    /// Whether this window can show a `width` × `height` screen: Grail's window has the
    /// framebuffer's size, 640 × 480 (stretched as it is resized).
    pub fn fits(width: u32, height: u32) -> bool {
        (width, height) == (FB_WIDTH, FB_HEIGHT)
    }

    /// Opens the window, titled `title` (plus the VM's state when it is not running).
    pub fn open(title: &str) -> Result<WindowView, String> {
        Ok(WindowView {
            window: Window::open(title)?,
            title: title.to_string(),
        })
    }

    /// After the session: keeps showing `screen`, with `status` in the title, until the
    /// window is closed. Input goes nowhere: there is nothing left to take it.
    pub fn hold(&mut self, screen: &Screen, status: &str) {
        self.window
            .set_title(&format!("{} ({status}, close to quit)", self.title));
        self.draw(screen);
        while self.window.pump() {
            self.window.take_keys();
            self.window.take_pointer();
            if self.window.take_repaint() {
                self.draw(screen);
            }
            self.window.wait();
        }
    }
}

impl View for WindowView {
    fn pump(&mut self) -> bool {
        self.window.pump()
    }

    fn take_input(&mut self) -> Vec<Input> {
        let keys = self
            .window
            .take_keys()
            .into_iter()
            .map(|k| Input::Key(key_from_grail(k)));
        let pointer = self
            .window
            .take_pointer()
            .into_iter()
            .map(|p| Input::Pointer(pointer_from_grail(p, FB_WIDTH, FB_HEIGHT)));
        keys.chain(pointer).collect()
    }

    fn needs_redraw(&mut self) -> bool {
        self.window.take_repaint()
    }

    fn draw(&mut self, screen: &Screen) {
        assert!(Self::fits(screen.width(), screen.height()));
        self.window.draw(Some(screen.pixels()));
    }

    fn vm_state(&mut self, state: VmState) {
        match state {
            VmState::Running => self.window.set_title(&self.title),
            state => self.window.set_title(&format!("{} ({state})", self.title)),
        }
    }
}
