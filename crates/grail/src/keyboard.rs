//! Xcal keyboard (spec section 9.4): a FIFO of key events behind three registers,
//! with an interrupt line to PLIC source 2.
//!
//! The device only holds state. Key events come from whoever owns the machine through
//! [`Keyboard::push`] (or [`crate::Machine::key_event`]): the framebuffer window on
//! Windows, a test, or any other host-side producer. Without one, the FIFO stays empty.

use crate::bus::BusFault;
use std::collections::VecDeque;

// Register offsets from the start of the keyboard page.
pub const REG_STATUS: u32 = 0x0;
pub const REG_DATA: u32 = 0x4;
pub const REG_CTRL: u32 = 0x8;

/// `KBD_STATUS` bit 0: at least one event is waiting in `KBD_DATA`.
pub const STATUS_EVENT: u32 = 1 << 0;
/// `KBD_CTRL` bit 0: raise PLIC source 2 while an event is waiting. Other bits read as 0.
pub const CTRL_IRQ_ENABLE: u32 = 1 << 0;

/// `KBD_DATA` bit 8: the event is a release (0 = press).
pub const DATA_RELEASE: u32 = 1 << 8;
/// `KBD_DATA` bit 9: Shift was held.
pub const DATA_SHIFT: u32 = 1 << 9;
/// `KBD_DATA` bit 10: Ctrl was held.
pub const DATA_CTRL: u32 = 1 << 10;

/// Events the FIFO holds. Events that arrive while it is full are dropped.
pub const FIFO_LEN: usize = 64;

/// Key codes that are not printable ASCII (spec section 9.4).
pub mod code {
    pub const BACKSPACE: u8 = 0x08;
    pub const TAB: u8 = 0x09;
    pub const ENTER: u8 = 0x0a;
    pub const ESC: u8 = 0x1b;
    pub const UP: u8 = 0x80;
    pub const DOWN: u8 = 0x81;
    pub const LEFT: u8 = 0x82;
    pub const RIGHT: u8 = 0x83;
    /// F1; F2-F12 follow (`F1 + n - 1`, up to 0x9b).
    pub const F1: u8 = 0x90;
    pub const F12: u8 = 0x9b;
}

/// One key press or release, as the program sees it in `KBD_DATA`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyEvent {
    /// Key code, see [`code`]. Never 0, so an empty `KBD_DATA` (0) cannot be mistaken
    /// for an event.
    pub code: u8,
    pub release: bool,
    pub shift: bool,
    pub ctrl: bool,
}

impl KeyEvent {
    pub fn press(code: u8) -> Self {
        KeyEvent {
            code,
            release: false,
            shift: false,
            ctrl: false,
        }
    }

    pub fn release(code: u8) -> Self {
        KeyEvent {
            release: true,
            ..Self::press(code)
        }
    }

    pub fn with_shift(self, shift: bool) -> Self {
        KeyEvent { shift, ..self }
    }

    pub fn with_ctrl(self, ctrl: bool) -> Self {
        KeyEvent { ctrl, ..self }
    }

    /// The `KBD_DATA` word: bits 7:0 code, bit 8 release, bit 9 shift, bit 10 ctrl.
    pub fn encode(self) -> u32 {
        let flag = |on: bool, bit: u32| if on { bit } else { 0 };
        self.code as u32
            | flag(self.release, DATA_RELEASE)
            | flag(self.shift, DATA_SHIFT)
            | flag(self.ctrl, DATA_CTRL)
    }

    /// Inverse of [`KeyEvent::encode`]; `None` for 0 (empty) and reserved bits.
    pub fn decode(word: u32) -> Option<Self> {
        let code = word as u8;
        if code == 0 || word & !(0xff | DATA_RELEASE | DATA_SHIFT | DATA_CTRL) != 0 {
            return None;
        }
        Some(KeyEvent {
            code,
            release: word & DATA_RELEASE != 0,
            shift: word & DATA_SHIFT != 0,
            ctrl: word & DATA_CTRL != 0,
        })
    }
}

#[derive(Default)]
pub struct Keyboard {
    fifo: VecDeque<KeyEvent>,
    pub ctrl: u32,
    /// Events dropped because the FIFO was full.
    pub dropped: u64,
}

impl Keyboard {
    /// Queues an event for the program. Returns false (and drops it) if the FIFO is full,
    /// or if its code is 0, which `KBD_DATA` reserves for "empty".
    pub fn push(&mut self, ev: KeyEvent) -> bool {
        if ev.code == 0 {
            return false;
        }
        if self.fifo.len() >= FIFO_LEN {
            self.dropped += 1;
            return false;
        }
        self.fifo.push_back(ev);
        true
    }

    /// Events waiting to be read.
    pub fn len(&self) -> usize {
        self.fifo.len()
    }

    pub fn is_empty(&self) -> bool {
        self.fifo.is_empty()
    }

    /// Level of the keyboard's PLIC line: high while `KBD_CTRL.0` is set and an event is
    /// waiting, so reading `KBD_DATA` until `KBD_STATUS.0` clears lowers it.
    pub fn irq(&self) -> bool {
        self.ctrl & CTRL_IRQ_ENABLE != 0 && !self.fifo.is_empty()
    }

    /// Registers are 32-bit only. Reading `KBD_DATA` pops the oldest event (0 if none).
    /// Unknown offsets read as 0.
    pub fn read(&mut self, off: u32, size: u32) -> Result<u32, BusFault> {
        if size != 4 {
            return Err(BusFault);
        }
        Ok(match off {
            REG_STATUS => (!self.fifo.is_empty()) as u32 * STATUS_EVENT,
            REG_DATA => self.fifo.pop_front().map_or(0, KeyEvent::encode),
            REG_CTRL => self.ctrl,
            _ => 0,
        })
    }

    /// Only `KBD_CTRL` is writable; writes elsewhere are ignored.
    pub fn write(&mut self, off: u32, size: u32, val: u32) -> Result<(), BusFault> {
        if size != 4 {
            return Err(BusFault);
        }
        if off == REG_CTRL {
            self.ctrl = val & CTRL_IRQ_ENABLE;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(k: &mut Keyboard, off: u32) -> u32 {
        k.read(off, 4).unwrap()
    }

    #[test]
    fn empty_keyboard_reads_zero() {
        let mut k = Keyboard::default();
        assert_eq!(r(&mut k, REG_STATUS), 0);
        assert_eq!(r(&mut k, REG_DATA), 0);
        assert_eq!(r(&mut k, REG_DATA), 0, "popping an empty FIFO changes nothing");
        assert_eq!(r(&mut k, REG_CTRL), 0);
        assert!(!k.irq());
    }

    #[test]
    fn data_pops_events_in_order() {
        let mut k = Keyboard::default();
        assert!(k.push(KeyEvent::press(b'h')));
        assert!(k.push(KeyEvent::release(b'h')));
        assert!(k.push(KeyEvent::press(code::F1)));
        assert_eq!(k.len(), 3);
        assert_eq!(r(&mut k, REG_STATUS), STATUS_EVENT);
        assert_eq!(r(&mut k, REG_STATUS), STATUS_EVENT, "status does not pop");
        assert_eq!(r(&mut k, REG_DATA), b'h' as u32);
        assert_eq!(r(&mut k, REG_DATA), b'h' as u32 | DATA_RELEASE);
        assert_eq!(r(&mut k, REG_STATUS), STATUS_EVENT);
        assert_eq!(r(&mut k, REG_DATA), 0x90);
        assert_eq!(r(&mut k, REG_STATUS), 0);
        assert_eq!(r(&mut k, REG_DATA), 0);
        assert!(k.is_empty());
    }

    #[test]
    fn event_encoding_matches_the_spec() {
        assert_eq!(KeyEvent::press(b'A').encode(), 0x041);
        assert_eq!(KeyEvent::release(b'A').encode(), 0x141);
        assert_eq!(KeyEvent::press(b'A').with_shift(true).encode(), 0x241);
        assert_eq!(KeyEvent::press(b'c').with_ctrl(true).encode(), 0x463);
        let all = KeyEvent::release(code::RIGHT).with_shift(true).with_ctrl(true);
        assert_eq!(all.encode(), 0x783);
        for ev in [
            KeyEvent::press(code::ENTER),
            KeyEvent::release(code::ESC).with_ctrl(true),
            all,
        ] {
            assert_eq!(KeyEvent::decode(ev.encode()), Some(ev));
        }
        assert_eq!(KeyEvent::decode(0), None, "0 means empty");
        assert_eq!(KeyEvent::decode(0x100), None, "a release without a code");
        assert_eq!(KeyEvent::decode(0x841), None, "reserved bit 11");
    }

    #[test]
    fn code_table() {
        assert_eq!(code::F12 - code::F1, 11);
        assert_eq!(
            [code::UP, code::DOWN, code::LEFT, code::RIGHT],
            [0x80, 0x81, 0x82, 0x83]
        );
        assert_eq!([code::BACKSPACE, code::ENTER, code::ESC], [0x08, 0x0a, 0x1b]);
    }

    #[test]
    fn full_fifo_drops_new_events() {
        let mut k = Keyboard::default();
        for i in 0..FIFO_LEN {
            assert!(k.push(KeyEvent::press(b' ' + i as u8)));
        }
        assert!(!k.push(KeyEvent::press(b'!')));
        assert!(!k.push(KeyEvent::release(b'!')));
        assert_eq!(k.dropped, 2);
        assert_eq!(k.len(), FIFO_LEN);
        // The oldest events are kept, in order.
        for i in 0..FIFO_LEN {
            assert_eq!(r(&mut k, REG_DATA), (b' ' + i as u8) as u32);
        }
        assert_eq!(r(&mut k, REG_DATA), 0);
        // Room again after reading.
        assert!(k.push(KeyEvent::press(b'x')));
        assert_eq!(r(&mut k, REG_DATA), b'x' as u32);
    }

    #[test]
    fn code_zero_is_never_queued() {
        let mut k = Keyboard::default();
        assert!(!k.push(KeyEvent::press(0)));
        assert!(k.is_empty());
        assert_eq!(k.dropped, 0);
    }

    #[test]
    fn irq_line_follows_ctrl_and_status() {
        let mut k = Keyboard::default();
        k.push(KeyEvent::press(b'a'));
        assert!(!k.irq(), "interrupts disabled");
        k.write(REG_CTRL, 4, 0xffff_ffff).unwrap();
        assert_eq!(r(&mut k, REG_CTRL), CTRL_IRQ_ENABLE, "reserved bits read 0");
        assert!(k.irq());
        r(&mut k, REG_DATA);
        assert!(!k.irq(), "drained");
        k.push(KeyEvent::release(b'a'));
        assert!(k.irq());
        k.write(REG_CTRL, 4, 0).unwrap();
        assert!(!k.irq());
        assert_eq!(r(&mut k, REG_STATUS), STATUS_EVENT, "the event is still there");
    }

    #[test]
    fn read_only_registers_and_unknown_offsets() {
        let mut k = Keyboard::default();
        k.push(KeyEvent::press(b'q'));
        k.write(REG_STATUS, 4, 0).unwrap();
        k.write(REG_DATA, 4, 0).unwrap();
        k.write(0xffc, 4, 0xffff_ffff).unwrap();
        assert_eq!(k.len(), 1, "writes do not touch the FIFO");
        assert_eq!(r(&mut k, 0xc), 0);
        assert_eq!(r(&mut k, 0xffc), 0);
        assert_eq!(r(&mut k, REG_CTRL), 0);
        assert_eq!(r(&mut k, REG_DATA), b'q' as u32);
    }

    #[test]
    fn registers_are_word_only() {
        let mut k = Keyboard::default();
        k.push(KeyEvent::press(b'q'));
        assert_eq!(k.read(REG_DATA, 1), Err(BusFault));
        assert_eq!(k.read(REG_DATA, 2), Err(BusFault));
        assert_eq!(k.len(), 1, "a faulting read pops nothing");
        assert_eq!(k.write(REG_CTRL, 1, 1), Err(BusFault));
        assert_eq!(k.write(REG_CTRL, 2, 1), Err(BusFault));
        assert_eq!(k.ctrl, 0);
    }
}
