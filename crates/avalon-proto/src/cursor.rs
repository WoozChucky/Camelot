//! A remote screen's cursor as both ends of a screen attachment keep it (protocol doc,
//! section 6.4): the VM's cursor plane (Excalibur spec section 9.3), an image blended
//! over the screen where the guest puts it, sent apart from the screen's frames, as
//! VNC's cursor pseudo-encoding and SPICE's cursor channel send a cursor. Its image goes
//! out when it changes (`CursorShape`) and its place when it moves or shows or hides
//! (`CursorPosition`), so a cursor that moves costs a few bytes and no frame.
//!
//! The server keeps one [`Cursor`] per VM, what its viewers were last sent, and
//! [`Cursor::update`] turns the cursor the framebuffer shows now into the messages that
//! bring the viewers to it. A viewer starts with no cursor and applies each message with
//! [`Cursor::apply`]; after the same messages, both ends hold the same cursor.

use crate::Message;

/// A cursor's image: `width` × `height` pixels of ARGB8888 with premultiplied alpha
/// (bytes B, G, R, A, each colour already multiplied by A), rows top to bottom, its
/// hotspot (the pixel that points) at (`hot_x`, `hot_y`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorImage {
    pub width: u32,
    pub height: u32,
    pub hot_x: u32,
    pub hot_y: u32,
    pub pixels: Vec<u8>,
}

/// What [`Cursor::update`] is given: the cursor shown now, its image borrowed (from the
/// guest's RAM, say) and its image's top left pixel at (`x`, `y`) of the screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CursorView<'a> {
    pub width: u32,
    pub height: u32,
    pub hot_x: u32,
    pub hot_y: u32,
    pub pixels: &'a [u8],
    pub x: i32,
    pub y: i32,
}

impl CursorView<'_> {
    /// Whether this cursor's image is `image`.
    fn has_image(&self, image: &CursorImage) -> bool {
        (self.width, self.height, self.hot_x, self.hot_y)
            == (image.width, image.height, image.hot_x, image.hot_y)
            && self.pixels == image.pixels
    }
}

/// A screen's cursor: its image (none until one is sent), and whether and where it is
/// shown, the image's top left pixel at (`x`, `y`) of the screen.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Cursor {
    pub image: Option<CursorImage>,
    pub visible: bool,
    pub x: i32,
    pub y: i32,
}

impl Cursor {
    /// What a viewer shows: the image and its place, while the cursor is visible and has
    /// an image.
    pub fn shown(&self) -> Option<(&CursorImage, i32, i32)> {
        let image = self.image.as_ref()?;
        self.visible.then_some((image, self.x, self.y))
    }

    /// Server side: takes in the cursor shown now (`None`: hidden) and returns the
    /// messages for VM `vm` that bring a viewer holding `self` to it: a `CursorShape`
    /// if the image changed, then a `CursorPosition` if the cursor moved, showed or
    /// hid. An unchanged cursor gives none; a hidden one keeps its image, so showing it
    /// again sends only its place.
    pub fn update(&mut self, vm: u32, now: Option<CursorView>) -> Vec<Message> {
        let mut out = Vec::new();
        let Some(now) = now else {
            if self.visible {
                self.visible = false;
                out.push(self.position(vm));
            }
            return out;
        };
        if !self.image.as_ref().is_some_and(|image| now.has_image(image)) {
            self.image = Some(CursorImage {
                width: now.width,
                height: now.height,
                hot_x: now.hot_x,
                hot_y: now.hot_y,
                pixels: now.pixels.to_vec(),
            });
            out.push(self.shape(vm).expect("an image"));
        }
        if !self.visible || (self.x, self.y) != (now.x, now.y) {
            (self.visible, self.x, self.y) = (true, now.x, now.y);
            out.push(self.position(vm));
        }
        out
    }

    /// The messages that give a viewer that has no cursor yet (one attaching) this
    /// one: its image, if it has one, then its place, if it is visible.
    pub fn messages(&self, vm: u32) -> Vec<Message> {
        let mut out: Vec<Message> = self.shape(vm).into_iter().collect();
        if self.visible {
            out.push(self.position(vm));
        }
        out
    }

    /// The `CursorShape` of this cursor's image, if it has one.
    fn shape(&self, vm: u32) -> Option<Message> {
        let image = self.image.as_ref()?;
        Some(Message::CursorShape {
            vm,
            width: image.width,
            height: image.height,
            hot_x: image.hot_x,
            hot_y: image.hot_y,
            pixels: image.pixels.clone(),
        })
    }

    /// The `CursorPosition` of this cursor.
    fn position(&self, vm: u32) -> Message {
        Message::CursorPosition {
            vm,
            visible: self.visible,
            x: self.x,
            y: self.y,
        }
    }

    /// Viewer side: applies a `CursorShape` or a `CursorPosition` (of whichever VM: the
    /// caller picks its own). Returns whether `msg` was one of them; other messages
    /// change nothing.
    pub fn apply(&mut self, msg: &Message) -> bool {
        match msg {
            Message::CursorShape {
                width,
                height,
                hot_x,
                hot_y,
                pixels,
                ..
            } => {
                self.image = Some(CursorImage {
                    width: *width,
                    height: *height,
                    hot_x: *hot_x,
                    hot_y: *hot_y,
                    pixels: pixels.clone(),
                });
                true
            }
            Message::CursorPosition { visible, x, y, .. } => {
                (self.visible, self.x, self.y) = (*visible, *x, *y);
                true
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(pixels: &[u8], x: i32, y: i32) -> CursorView<'_> {
        CursorView {
            width: 2,
            height: 1,
            hot_x: 1,
            hot_y: 0,
            pixels,
            x,
            y,
        }
    }

    /// The server's updates, applied in order by a viewer that started with nothing,
    /// give it the server's cursor; only what changed is sent.
    #[test]
    fn updates_send_only_what_changed() {
        let (a, b) = ([0xffu8; 8], [0x80u8; 8]);
        let mut server = Cursor::default();
        let mut viewer = Cursor::default();
        let mut send = |server: &mut Cursor, now| {
            let msgs = server.update(7, now);
            for m in &msgs {
                assert!(viewer.apply(m));
                m.check().unwrap();
            }
            assert_eq!(&viewer, server);
            msgs
        };
        assert!(send(&mut server, None).is_empty(), "hidden from the start");
        let msgs = send(&mut server, Some(view(&a, 10, -5)));
        assert!(matches!(
            msgs[..],
            [
                Message::CursorShape {
                    vm: 7,
                    width: 2,
                    height: 1,
                    hot_x: 1,
                    hot_y: 0,
                    ..
                },
                Message::CursorPosition {
                    vm: 7,
                    visible: true,
                    x: 10,
                    y: -5
                }
            ]
        ));
        assert!(send(&mut server, Some(view(&a, 10, -5))).is_empty(), "unchanged");
        let moved = send(&mut server, Some(view(&a, 11, -5)));
        assert!(
            matches!(moved[..], [Message::CursorPosition { x: 11, .. }]),
            "a move"
        );
        let reshaped = send(&mut server, Some(view(&b, 11, -5)));
        assert!(
            matches!(reshaped[..], [Message::CursorShape { .. }]),
            "a new image"
        );
        let hidden = send(&mut server, None);
        assert!(matches!(
            hidden[..],
            [Message::CursorPosition { visible: false, .. }]
        ));
        assert!(send(&mut server, None).is_empty());
        assert_eq!(server.shown(), None);
        let back = send(&mut server, Some(view(&b, 11, -5)));
        assert!(
            matches!(back[..], [Message::CursorPosition { visible: true, .. }]),
            "the image kept"
        );
        // The viewer holds the same (`send` checks it).
        let (image, x, y) = server.shown().unwrap();
        assert_eq!((&image.pixels[..], x, y), (&b[..], 11, -5));
        // A hotspot of its own is another image.
        let hot = CursorView {
            hot_x: 0,
            ..view(&b, 11, -5)
        };
        assert!(matches!(
            send(&mut server, Some(hot))[..],
            [Message::CursorShape { hot_x: 0, .. }]
        ));
    }

    /// A viewer that attaches is sent the image and, while it shows, its place.
    #[test]
    fn a_new_viewer_gets_the_image_and_the_place() {
        let mut server = Cursor::default();
        assert!(server.messages(1).is_empty(), "nothing yet");
        server.update(1, Some(view(&[1; 8], 3, 4)));
        let mut viewer = Cursor::default();
        for m in server.messages(1) {
            viewer.apply(&m);
        }
        assert_eq!(viewer, server);
        server.update(1, None);
        let msgs = server.messages(1);
        assert!(
            matches!(msgs[..], [Message::CursorShape { .. }]),
            "hidden: the image only"
        );
        assert!(!viewer.apply(&Message::ConsoleDetach { vm: 1 }));
    }
}
