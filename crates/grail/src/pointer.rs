//! Xcal pointer (spec section 9.7): a mouse, or rather a tablet. A FIFO of pointer
//! events behind a page of registers, with an interrupt line to PLIC source 11.
//!
//! The device reports positions in framebuffer pixels by default (**absolute mode**),
//! like a USB tablet: the guest's cursor is exactly where the host's is, with no drift
//! and no need for a remote viewer to grab the host mouse. **Relative mode** (`PTR_CTRL`
//! bit 1) reports the motion since the previous event instead, for software that wants
//! deltas.
//!
//! Like the keyboard, the device only holds state. Input comes from whoever owns the
//! machine through [`Pointer::push`] (or [`crate::Machine::pointer_event`]): the
//! framebuffer window on Windows, a test, or a remote viewer. The host says where the
//! pointer is (or how far it moved), which buttons are held and how far the wheel
//! turned ([`PointerInput`]); the device turns that into an event in the mode the
//! guest asked for ([`PointerEvent`]) and queues it.

use crate::bus::BusFault;
use crate::framebuffer::{FB_HEIGHT, FB_WIDTH};
use std::collections::VecDeque;

// Register offsets from the start of the pointer page.
pub const REG_STATUS: u32 = 0x00;
pub const REG_EVENT: u32 = 0x04;
pub const REG_EVENT_XY: u32 = 0x08;
pub const REG_CTRL: u32 = 0x0c;
pub const REG_POS: u32 = 0x10;
pub const REG_BUTTONS: u32 = 0x14;

/// `PTR_STATUS` bit 0: at least one event is waiting in `PTR_EVENT` (read-only).
pub const STATUS_EVENT: u32 = 1 << 0;
/// `PTR_STATUS` bit 1: an event was dropped because the FIFO was full. Sticky until
/// software writes 1 to it.
pub const STATUS_OVERFLOW: u32 = 1 << 1;

/// `PTR_CTRL` bit 0: raise PLIC source 11 while an event is waiting.
pub const CTRL_IRQ_ENABLE: u32 = 1 << 0;
/// `PTR_CTRL` bit 1: relative mode (events carry dx, dy); clear = absolute (x, y).
pub const CTRL_RELATIVE: u32 = 1 << 1;
const CTRL_MASK: u32 = CTRL_IRQ_ENABLE | CTRL_RELATIVE;

/// Button bits, in events (`PTR_EVENT` bits 2:0), in `PTR_BUTTONS` and in
/// [`PointerInput::buttons`].
pub const BUTTON_LEFT: u8 = 1 << 0;
pub const BUTTON_RIGHT: u8 = 1 << 1;
pub const BUTTON_MIDDLE: u8 = 1 << 2;
pub const BUTTONS_MASK: u8 = BUTTON_LEFT | BUTTON_RIGHT | BUTTON_MIDDLE;

/// `PTR_EVENT` bit 3: the event's x and y are a relative motion (dx, dy).
pub const EVENT_RELATIVE: u32 = 1 << 3;
/// `PTR_EVENT` bits 15:8: wheel motion, signed, in notches.
pub const EVENT_WHEEL_SHIFT: u32 = 8;
/// `PTR_EVENT` bit 31: the word is an event (so 0 means the FIFO was empty).
pub const EVENT_VALID: u32 = 1 << 31;

/// Events the FIFO holds. Events that arrive while it is full are dropped.
pub const FIFO_LEN: usize = 64;

/// How the host's pointer moved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Motion {
    /// It did not (a button or wheel change only).
    #[default]
    None,
    /// To this framebuffer pixel. Values outside the framebuffer are clamped to its
    /// edges, so a host may pass positions past them (a drag that leaves the window).
    To { x: i32, y: i32 },
    /// By this many pixels (a host that only has deltas, such as a viewer that has
    /// captured the mouse).
    By { dx: i32, dy: i32 },
}

/// What the host reports: one change of its pointer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PointerInput {
    pub motion: Motion,
    /// The buttons held after this change ([`BUTTON_LEFT`], ...). Other bits are ignored.
    pub buttons: u8,
    /// Wheel notches turned, positive away from the user (up). Clamped to -128..=127.
    pub wheel: i32,
}

impl PointerInput {
    /// The pointer is now at framebuffer pixel (`x`, `y`), with no buttons held.
    pub fn move_to(x: i32, y: i32) -> Self {
        PointerInput {
            motion: Motion::To { x, y },
            ..Self::default()
        }
    }

    /// The pointer moved by (`dx`, `dy`) pixels, with no buttons held.
    pub fn move_by(dx: i32, dy: i32) -> Self {
        PointerInput {
            motion: Motion::By { dx, dy },
            ..Self::default()
        }
    }

    /// No motion: the buttons held are now `buttons`.
    pub fn buttons(buttons: u8) -> Self {
        PointerInput {
            buttons,
            ..Self::default()
        }
    }

    pub fn with_buttons(self, buttons: u8) -> Self {
        PointerInput { buttons, ..self }
    }

    pub fn with_wheel(self, wheel: i32) -> Self {
        PointerInput { wheel, ..self }
    }
}

/// One event, as the program reads it from `PTR_EVENT` and `PTR_EVENT_XY`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PointerEvent {
    /// Absolute: the pixel (0..FB_WIDTH, 0..FB_HEIGHT). Relative: the motion since the
    /// previous event (dx, dy), saturated to 16 bits.
    pub x: i16,
    pub y: i16,
    pub relative: bool,
    /// Buttons held after the event.
    pub buttons: u8,
    /// Wheel notches, positive away from the user.
    pub wheel: i8,
}

impl PointerEvent {
    /// The `PTR_EVENT` word: bits 2:0 buttons, bit 3 relative, bits 15:8 wheel, bit 31
    /// valid.
    pub fn encode(self) -> u32 {
        EVENT_VALID
            | (self.buttons & BUTTONS_MASK) as u32
            | if self.relative { EVENT_RELATIVE } else { 0 }
            | (self.wheel as u8 as u32) << EVENT_WHEEL_SHIFT
    }

    /// The `PTR_EVENT_XY` word: x in bits 15:0, y in bits 31:16 (two's complement in
    /// relative mode).
    pub fn encode_xy(self) -> u32 {
        self.x as u16 as u32 | (self.y as u16 as u32) << 16
    }

    /// Inverse of [`PointerEvent::encode`] and [`PointerEvent::encode_xy`]; `None` if
    /// the valid bit is clear or a reserved bit is set.
    pub fn decode(event: u32, xy: u32) -> Option<Self> {
        let known = EVENT_VALID | EVENT_RELATIVE | BUTTONS_MASK as u32 | 0xff << EVENT_WHEEL_SHIFT;
        if event & EVENT_VALID == 0 || event & !known != 0 {
            return None;
        }
        Some(PointerEvent {
            x: xy as u16 as i16,
            y: (xy >> 16) as u16 as i16,
            relative: event & EVENT_RELATIVE != 0,
            buttons: event as u8 & BUTTONS_MASK,
            wheel: (event >> EVENT_WHEEL_SHIFT) as u8 as i8,
        })
    }
}

/// Clamps a position into the framebuffer.
fn clamp_pos(x: i64, y: i64) -> (u16, u16) {
    (
        x.clamp(0, FB_WIDTH as i64 - 1) as u16,
        y.clamp(0, FB_HEIGHT as i64 - 1) as u16,
    )
}

fn saturate_i16(v: i64) -> i16 {
    v.clamp(i16::MIN as i64, i16::MAX as i64) as i16
}

#[derive(Default)]
pub struct Pointer {
    fifo: VecDeque<PointerEvent>,
    /// `PTR_EVENT_XY`: the position word of the event last popped from `PTR_EVENT`.
    latched_xy: u32,
    pub ctrl: u32,
    overflow: bool,
    /// Where the host's pointer is and which buttons it holds (`PTR_POS`,
    /// `PTR_BUTTONS`), including changes whose events were dropped.
    pos: (u16, u16),
    buttons: u8,
    /// Events dropped because the FIFO was full.
    pub dropped: u64,
}

impl Pointer {
    /// Applies a change of the host's pointer and queues the event it makes, in the
    /// mode `PTR_CTRL` selects now. Returns true if an event was queued. Returns false
    /// if it changed nothing the program can see (no motion in the current mode, the
    /// same buttons, no wheel), or if the FIFO is full: then the event is dropped,
    /// counted in `dropped` and flagged in `PTR_STATUS.1`, but `PTR_POS` and
    /// `PTR_BUTTONS` still follow it.
    pub fn push(&mut self, input: PointerInput) -> bool {
        let (old_x, old_y) = (self.pos.0 as i64, self.pos.1 as i64);
        // The new position, and the motion a relative event reports.
        let (pos, delta) = match input.motion {
            Motion::None => (self.pos, (0, 0)),
            Motion::To { x, y } => {
                let pos = clamp_pos(x as i64, y as i64);
                (pos, (pos.0 as i64 - old_x, pos.1 as i64 - old_y))
            }
            // A relative device reports the motion the host saw, even against an edge.
            Motion::By { dx, dy } => (
                clamp_pos(old_x + dx as i64, old_y + dy as i64),
                (dx as i64, dy as i64),
            ),
        };
        let relative = self.ctrl & CTRL_RELATIVE != 0;
        let buttons = input.buttons & BUTTONS_MASK;
        let wheel = input.wheel.clamp(i8::MIN as i32, i8::MAX as i32) as i8;
        let moved = if relative {
            delta != (0, 0)
        } else {
            pos != self.pos
        };
        if !moved && buttons == self.buttons && wheel == 0 {
            return false;
        }
        self.pos = pos;
        self.buttons = buttons;
        let (x, y) = if relative {
            (saturate_i16(delta.0), saturate_i16(delta.1))
        } else {
            (pos.0 as i16, pos.1 as i16)
        };
        if self.fifo.len() >= FIFO_LEN {
            self.dropped += 1;
            self.overflow = true;
            return false;
        }
        self.fifo.push_back(PointerEvent {
            x,
            y,
            relative,
            buttons,
            wheel,
        });
        true
    }

    /// Events waiting to be read.
    pub fn len(&self) -> usize {
        self.fifo.len()
    }

    pub fn is_empty(&self) -> bool {
        self.fifo.is_empty()
    }

    /// Where the pointer is, in framebuffer pixels (`PTR_POS`).
    pub fn position(&self) -> (u32, u32) {
        (self.pos.0 as u32, self.pos.1 as u32)
    }

    /// The buttons held (`PTR_BUTTONS`).
    pub fn held(&self) -> u8 {
        self.buttons
    }

    /// Level of the pointer's PLIC line: high while `PTR_CTRL.0` is set and an event is
    /// waiting, so reading `PTR_EVENT` until `PTR_STATUS.0` clears lowers it.
    pub fn irq(&self) -> bool {
        self.ctrl & CTRL_IRQ_ENABLE != 0 && !self.fifo.is_empty()
    }

    /// Registers are 32-bit only. Reading `PTR_EVENT` pops the oldest event (0 if none)
    /// and latches its position into `PTR_EVENT_XY`. Unknown offsets read as 0.
    pub fn read(&mut self, off: u32, size: u32) -> Result<u32, BusFault> {
        if size != 4 {
            return Err(BusFault);
        }
        Ok(match off {
            REG_STATUS => {
                let flag = |on: bool, bit: u32| if on { bit } else { 0 };
                flag(!self.fifo.is_empty(), STATUS_EVENT) | flag(self.overflow, STATUS_OVERFLOW)
            }
            REG_EVENT => {
                let ev = self.fifo.pop_front();
                self.latched_xy = ev.map_or(0, PointerEvent::encode_xy);
                ev.map_or(0, PointerEvent::encode)
            }
            REG_EVENT_XY => self.latched_xy,
            REG_CTRL => self.ctrl,
            REG_POS => self.pos.0 as u32 | (self.pos.1 as u32) << 16,
            REG_BUTTONS => self.buttons as u32,
            _ => 0,
        })
    }

    /// `PTR_CTRL` is writable, and writing 1 to `PTR_STATUS` bit 1 clears the overflow
    /// flag; other writes are ignored.
    pub fn write(&mut self, off: u32, size: u32, val: u32) -> Result<(), BusFault> {
        if size != 4 {
            return Err(BusFault);
        }
        match off {
            REG_STATUS if val & STATUS_OVERFLOW != 0 => self.overflow = false,
            REG_CTRL => self.ctrl = val & CTRL_MASK,
            _ => {}
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(p: &mut Pointer, off: u32) -> u32 {
        p.read(off, 4).unwrap()
    }

    /// Pops one event through the registers, as a program does.
    fn pop(p: &mut Pointer) -> Option<PointerEvent> {
        let ev = r(p, REG_EVENT);
        let xy = r(p, REG_EVENT_XY);
        let decoded = PointerEvent::decode(ev, xy);
        if decoded.is_none() {
            assert_eq!((ev, xy), (0, 0), "an empty pop reads 0 in both words");
        }
        decoded
    }

    fn abs(x: i16, y: i16, buttons: u8) -> PointerEvent {
        PointerEvent {
            x,
            y,
            buttons,
            ..PointerEvent::default()
        }
    }

    fn rel(x: i16, y: i16, buttons: u8) -> PointerEvent {
        PointerEvent {
            relative: true,
            ..abs(x, y, buttons)
        }
    }

    fn relative() -> Pointer {
        let mut p = Pointer::default();
        p.write(REG_CTRL, 4, CTRL_RELATIVE).unwrap();
        p
    }

    #[test]
    fn reset_state() {
        let mut p = Pointer::default();
        for off in [
            REG_STATUS,
            REG_EVENT,
            REG_EVENT_XY,
            REG_CTRL,
            REG_POS,
            REG_BUTTONS,
        ] {
            assert_eq!(r(&mut p, off), 0, "offset {off:#x}");
        }
        assert_eq!(pop(&mut p), None);
        assert!(!p.irq());
        assert_eq!(p.position(), (0, 0));
    }

    #[test]
    fn absolute_events() {
        let mut p = Pointer::default();
        assert!(p.push(PointerInput::move_to(10, 20)));
        assert!(p.push(PointerInput::move_to(11, 20).with_buttons(BUTTON_LEFT)));
        assert!(p.push(PointerInput::buttons(0)));
        assert_eq!(p.len(), 3);
        assert_eq!(r(&mut p, REG_STATUS), STATUS_EVENT);
        assert_eq!(r(&mut p, REG_STATUS), STATUS_EVENT, "status does not pop");
        assert_eq!(r(&mut p, REG_EVENT), EVENT_VALID);
        assert_eq!(r(&mut p, REG_EVENT_XY), 10 | 20 << 16);
        assert_eq!(r(&mut p, REG_EVENT_XY), 10 | 20 << 16, "XY does not pop");
        assert_eq!(pop(&mut p), Some(abs(11, 20, BUTTON_LEFT)));
        // A button release without motion still carries the position.
        assert_eq!(pop(&mut p), Some(abs(11, 20, 0)));
        assert_eq!(r(&mut p, REG_STATUS), 0);
        assert_eq!(pop(&mut p), None);
        assert_eq!(r(&mut p, REG_EVENT_XY), 0, "an empty pop clears the latch");
        assert_eq!(r(&mut p, REG_POS), 11 | 20 << 16);
    }

    #[test]
    fn absolute_motion_by_deltas() {
        let mut p = Pointer::default();
        assert!(p.push(PointerInput::move_by(5, 7)));
        assert!(p.push(PointerInput::move_by(-2, 1)));
        assert_eq!(pop(&mut p), Some(abs(5, 7, 0)));
        assert_eq!(pop(&mut p), Some(abs(3, 8, 0)));
    }

    #[test]
    fn positions_are_clamped_to_the_framebuffer() {
        let (w, h) = (FB_WIDTH as i16, FB_HEIGHT as i16);
        let mut p = Pointer::default();
        for (input, want) in [
            (PointerInput::move_to(-5, -1), (0, 0)),
            (PointerInput::move_to(10_000, 3), (w - 1, 3)),
            (PointerInput::move_to(4, 480), (4, h - 1)),
            (PointerInput::move_to(i32::MAX, i32::MIN), (w - 1, 0)),
            (PointerInput::move_to(639, 479), (w - 1, h - 1)),
            (PointerInput::move_by(i32::MIN, i32::MIN), (0, 0)),
            (PointerInput::move_by(i32::MAX, 0), (w - 1, 0)),
        ] {
            p.push(input.with_wheel(1)); // a wheel notch, so every input makes an event
            assert_eq!(p.position(), (want.0 as u32, want.1 as u32), "{input:?}");
            let ev = pop(&mut p).unwrap();
            assert_eq!((ev.x, ev.y), want, "{input:?}");
        }
    }

    #[test]
    fn relative_events() {
        let mut p = relative();
        assert!(p.push(PointerInput::move_to(100, 50)));
        assert!(p.push(PointerInput::move_to(90, 55)));
        assert!(p.push(PointerInput::move_by(3, -4)));
        assert_eq!(pop(&mut p), Some(rel(100, 50, 0)));
        assert_eq!(pop(&mut p), Some(rel(-10, 5, 0)));
        assert_eq!(pop(&mut p), Some(rel(3, -4, 0)));
        // dx/dy are two's complement halfwords.
        p.push(PointerInput::move_by(-1, -2));
        assert_eq!(r(&mut p, REG_EVENT), EVENT_VALID | EVENT_RELATIVE);
        assert_eq!(r(&mut p, REG_EVENT_XY), 0xfffe_ffff);
        // PTR_POS stays absolute.
        assert_eq!(p.position(), (92, 49));
        assert_eq!(r(&mut p, REG_POS), 92 | 49 << 16);
    }

    #[test]
    fn relative_motion_against_an_edge() {
        let mut p = relative();
        // Deltas from the host are reported as they are, even past the edge...
        assert!(p.push(PointerInput::move_by(-30, 0)));
        assert_eq!(pop(&mut p), Some(rel(-30, 0, 0)));
        assert_eq!(p.position(), (0, 0));
        // ...and saturate at 16 bits.
        p.push(PointerInput::move_by(100_000, -100_000));
        assert_eq!(pop(&mut p), Some(rel(i16::MAX, i16::MIN, 0)));
        assert_eq!(p.position(), (FB_WIDTH - 1, 0), "the position is clamped");
        // A position past the edge is clamped first, so it moves only to the edge.
        p.push(PointerInput::move_to(5000, 5000));
        let (w, h) = (FB_WIDTH as i16, FB_HEIGHT as i16);
        assert_eq!(pop(&mut p), Some(rel(0, h - 1, 0)));
        assert!(
            !p.push(PointerInput::move_to(w as i32, h as i32)),
            "no motion left: no event"
        );
    }

    #[test]
    fn mode_is_chosen_when_the_event_is_made() {
        let mut p = Pointer::default();
        p.push(PointerInput::move_to(30, 40));
        p.write(REG_CTRL, 4, CTRL_RELATIVE).unwrap();
        p.push(PointerInput::move_to(31, 38));
        p.write(REG_CTRL, 4, 0).unwrap();
        p.push(PointerInput::move_to(32, 38));
        assert_eq!(pop(&mut p), Some(abs(30, 40, 0)));
        assert_eq!(pop(&mut p), Some(rel(1, -2, 0)));
        assert_eq!(pop(&mut p), Some(abs(32, 38, 0)));
    }

    #[test]
    fn inputs_that_change_nothing_make_no_event() {
        let mut p = Pointer::default();
        assert!(!p.push(PointerInput::move_to(0, 0)));
        assert!(!p.push(PointerInput::move_by(0, 0)));
        assert!(!p.push(PointerInput::default()));
        assert!(!p.push(PointerInput::move_by(-1, -1)), "clamped: still at (0, 0)");
        assert!(!p.push(PointerInput::buttons(0xf8)), "only reserved button bits");
        p.push(PointerInput::move_to(7, 7).with_buttons(BUTTON_RIGHT));
        assert!(!p.push(PointerInput::move_to(7, 7).with_buttons(BUTTON_RIGHT)));
        assert_eq!(p.len(), 1);
        assert_eq!(p.dropped, 0, "not an overflow");
        assert_eq!(r(&mut p, REG_STATUS), STATUS_EVENT);
        // In relative mode a delta that clamps to nothing is still motion.
        let mut p = relative();
        assert!(p.push(PointerInput::move_by(-1, 0)));
        assert!(!p.push(PointerInput::move_to(0, 0)));
    }

    #[test]
    fn buttons_and_wheel_encoding() {
        let mut p = Pointer::default();
        p.push(PointerInput::buttons(BUTTON_LEFT));
        p.push(PointerInput::buttons(BUTTON_LEFT | BUTTON_RIGHT));
        p.push(PointerInput::buttons(BUTTONS_MASK | 0xf8)); // reserved bits ignored
        p.push(PointerInput::buttons(BUTTON_MIDDLE).with_wheel(1));
        p.push(PointerInput::buttons(BUTTON_MIDDLE).with_wheel(-1));
        p.push(PointerInput::buttons(BUTTON_MIDDLE).with_wheel(1000));
        p.push(PointerInput::buttons(BUTTON_MIDDLE).with_wheel(-1000));
        let words: Vec<u32> = (0..7).map(|_| r(&mut p, REG_EVENT)).collect();
        assert_eq!(
            words,
            [
                0x8000_0001,
                0x8000_0003,
                0x8000_0007,
                0x8000_0104,
                0x8000_ff04,
                0x8000_7f04,
                0x8000_8004,
            ]
        );
        assert_eq!(r(&mut p, REG_BUTTONS), BUTTON_MIDDLE as u32);
        assert_eq!([BUTTON_LEFT, BUTTON_RIGHT, BUTTON_MIDDLE], [1, 2, 4]);

        // Round trips, and what decode refuses.
        let all = PointerEvent {
            x: -300,
            y: 299,
            relative: true,
            buttons: BUTTONS_MASK,
            wheel: -128,
        };
        assert_eq!(all.encode(), 0x8000_800f);
        assert_eq!(all.encode_xy(), 0x012b_fed4);
        for ev in [all, abs(639, 479, BUTTON_RIGHT), PointerEvent::default()] {
            assert_eq!(PointerEvent::decode(ev.encode(), ev.encode_xy()), Some(ev));
        }
        assert_eq!(PointerEvent::decode(0, 0), None, "0 means empty");
        assert_eq!(PointerEvent::decode(0x0000_0001, 0), None, "no valid bit");
        assert_eq!(PointerEvent::decode(0x8000_0010, 0), None, "reserved bit 4");
        assert_eq!(PointerEvent::decode(0x8001_0000, 0), None, "reserved bit 16");
    }

    #[test]
    fn full_fifo_drops_new_events_and_flags_overflow() {
        let mut p = Pointer::default();
        for i in 0..FIFO_LEN {
            assert!(p.push(PointerInput::move_to(i as i32 + 1, 0)));
        }
        assert!(!p.push(PointerInput::move_to(200, 100)));
        assert!(!p.push(PointerInput::buttons(BUTTON_LEFT)));
        assert_eq!(p.dropped, 2);
        assert_eq!(p.len(), FIFO_LEN);
        assert_eq!(r(&mut p, REG_STATUS), STATUS_EVENT | STATUS_OVERFLOW);
        // The dropped changes still reach PTR_POS and PTR_BUTTONS, so a program can
        // catch up after an overflow.
        assert_eq!(r(&mut p, REG_POS), 200 | 100 << 16);
        assert_eq!(r(&mut p, REG_BUTTONS), BUTTON_LEFT as u32);
        // The oldest events are kept, in order.
        for i in 0..FIFO_LEN {
            assert_eq!(pop(&mut p), Some(abs(i as i16 + 1, 0, 0)));
        }
        assert_eq!(pop(&mut p), None);
        assert_eq!(r(&mut p, REG_STATUS), STATUS_OVERFLOW, "sticky");
        // Writing 0, or to bit 0, leaves it; writing 1 to bit 1 clears it.
        p.write(REG_STATUS, 4, STATUS_EVENT).unwrap();
        assert_eq!(r(&mut p, REG_STATUS), STATUS_OVERFLOW);
        p.write(REG_STATUS, 4, STATUS_OVERFLOW).unwrap();
        assert_eq!(r(&mut p, REG_STATUS), 0);
        // Room again.
        assert!(p.push(PointerInput::buttons(0)));
        assert_eq!(pop(&mut p), Some(abs(200, 100, 0)));
    }

    #[test]
    fn irq_line_follows_ctrl_and_status() {
        let mut p = Pointer::default();
        p.push(PointerInput::move_to(1, 1));
        assert!(!p.irq(), "interrupts disabled");
        p.write(REG_CTRL, 4, 0xffff_ffff).unwrap();
        assert_eq!(
            r(&mut p, REG_CTRL),
            CTRL_IRQ_ENABLE | CTRL_RELATIVE,
            "reserved bits read 0"
        );
        assert!(p.irq());
        r(&mut p, REG_EVENT_XY);
        assert!(p.irq(), "reading XY pops nothing");
        r(&mut p, REG_EVENT);
        assert!(!p.irq(), "drained");
        p.push(PointerInput::move_by(1, 0));
        assert!(p.irq());
        p.write(REG_CTRL, 4, CTRL_RELATIVE).unwrap();
        assert!(!p.irq());
        assert_eq!(r(&mut p, REG_STATUS), STATUS_EVENT, "the event is still there");
        // Overflow alone does not interrupt.
        let mut p = Pointer::default();
        p.write(REG_CTRL, 4, CTRL_IRQ_ENABLE).unwrap();
        for i in 0..=FIFO_LEN {
            p.push(PointerInput::move_to(i as i32 + 1, 0));
        }
        while r(&mut p, REG_EVENT) != 0 {}
        assert_eq!(r(&mut p, REG_STATUS), STATUS_OVERFLOW);
        assert!(!p.irq());
    }

    #[test]
    fn read_only_registers_and_unknown_offsets() {
        let mut p = Pointer::default();
        p.push(PointerInput::move_to(3, 4).with_buttons(BUTTON_RIGHT));
        for off in [REG_EVENT, REG_EVENT_XY, REG_POS, REG_BUTTONS, 0x18, 0xffc] {
            p.write(off, 4, 0xffff_ffff).unwrap();
        }
        p.write(REG_STATUS, 4, STATUS_EVENT).unwrap();
        assert_eq!(p.len(), 1, "writes do not touch the FIFO");
        assert_eq!(r(&mut p, REG_POS), 3 | 4 << 16);
        assert_eq!(r(&mut p, REG_BUTTONS), BUTTON_RIGHT as u32);
        assert_eq!(r(&mut p, 0x18), 0);
        assert_eq!(r(&mut p, 0xffc), 0);
        assert_eq!(r(&mut p, REG_CTRL), 0);
        assert_eq!(pop(&mut p), Some(abs(3, 4, BUTTON_RIGHT)));
    }

    #[test]
    fn registers_are_word_only() {
        let mut p = Pointer::default();
        p.push(PointerInput::move_to(3, 4));
        for size in [1, 2] {
            for off in [
                REG_STATUS,
                REG_EVENT,
                REG_EVENT_XY,
                REG_CTRL,
                REG_POS,
                REG_BUTTONS,
            ] {
                assert_eq!(p.read(off, size), Err(BusFault));
            }
            assert_eq!(p.write(REG_CTRL, size, CTRL_IRQ_ENABLE), Err(BusFault));
            assert_eq!(p.write(REG_STATUS, size, STATUS_OVERFLOW), Err(BusFault));
        }
        assert_eq!(p.len(), 1, "a faulting read pops nothing");
        assert_eq!(p.ctrl, 0);
    }
}
