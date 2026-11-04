//! The [`View`] of `herald --screen` on Windows: Grail's own framebuffer window
//! (`grail::window`, hand-written Win32 FFI), shared rather than written again. It
//! shows the remote screen stretched over its client area as Grail shows a local one,
//! and its keys and mouse, already turned into keyboard events and pointer input in
//! framebuffer pixels by that window, go to the VM. When the guest changes modes the
//! screen comes in another size, and the window takes it, as Grail's does. The guest's
//! cursor, which comes apart from the frames, is blended over the screen as Grail's
//! window blends a local machine's cursor plane.

use crate::screen::{frame, key_from_grail, pointer_from_grail, Input, View};
use avalon_proto::{Cursor, Screen, VmState};
use grail::window::Window;

/// A window onto a VM's screen.
pub struct WindowView {
    window: Window,
    title: String,
}

impl WindowView {
    /// Opens the window, titled `title` (plus the VM's state when it is not running).
    pub fn open(title: &str) -> Result<WindowView, String> {
        Ok(WindowView {
            window: Window::open(title)?,
            title: title.to_string(),
        })
    }

    /// After the session: keeps showing `screen` and `cursor`, with `status` in the
    /// title, until the window is closed. Input goes nowhere: there is nothing left to
    /// take it.
    pub fn hold(&mut self, screen: &Screen, cursor: &Cursor, status: &str) {
        self.window
            .set_title(&format!("{} ({status}, close to quit)", self.title));
        self.draw(screen, cursor);
        while self.window.pump() {
            self.window.take_keys();
            self.window.take_pointer();
            if self.window.take_repaint() {
                self.draw(screen, cursor);
            }
            self.window.wait();
        }
    }
}

impl View for WindowView {
    fn pump(&mut self) -> bool {
        self.window.pump()
    }

    /// Keys first, then pointer changes: Grail's window keeps the two apart (they are
    /// for two devices with a FIFO each), so their order within one poll is not known.
    /// Each keeps its own order, which is all the server keeps too (protocol doc,
    /// section 6.4). Positions are in the pixels of the screen last drawn, the one the
    /// person sees.
    fn take_input(&mut self) -> Vec<Input> {
        let keys = self
            .window
            .take_keys()
            .into_iter()
            .map(|k| Input::Key(key_from_grail(k)));
        let (width, height) = self.window.frame_size();
        let pointer = self
            .window
            .take_pointer()
            .into_iter()
            .map(|p| Input::Pointer(pointer_from_grail(p, width, height)));
        keys.chain(pointer).collect()
    }

    fn needs_redraw(&mut self) -> bool {
        self.window.take_repaint()
    }

    fn draw(&mut self, screen: &Screen, cursor: &Cursor) {
        self.window.draw(frame(screen, cursor));
    }

    fn vm_state(&mut self, state: VmState) {
        match state {
            VmState::Running => self.window.set_title(&self.title),
            state => self.window.set_title(&format!("{} ({state})", self.title)),
        }
    }
}
