//! Xcal keyboard (spec section 9.4): a FIFO of key events behind three registers,
//! with an interrupt line to PLIC source 2.
//!
//! The device reports keys in one of two modes, which `KBD_CTRL` bit 1 selects:
//!
//! - **ASCII mode** (the default): the character the host's layout typed, with Shift and
//!   Ctrl as bits on it (the [`code`] table). Enough for a shell.
//! - **Raw mode**: the physical key, as the USB HID usage ID a real USB keyboard sends
//!   ([`usage`], the Keyboard/Keypad page of the HID Usage Tables), one press and one
//!   release per key, modifiers included. A kernel maps them to its own key codes and
//!   user space applies a layout, as Linux's HID driver and evdev do.
//!
//! The device only holds state. Key events come from whoever owns the machine through
//! [`Keyboard::push`] (or [`crate::Machine::key_event`]): the framebuffer window on
//! Windows, a remote viewer, a test, or any other host-side producer. Without one, the
//! FIFO stays empty. A producer does not know which mode the program chose, so each
//! [`KeyEvent`] carries both forms, and the device queues the one its mode reports.

use crate::bus::BusFault;
use std::collections::VecDeque;

// Register offsets from the start of the keyboard page.
pub const REG_STATUS: u32 = 0x0;
pub const REG_DATA: u32 = 0x4;
pub const REG_CTRL: u32 = 0x8;

/// `KBD_STATUS` bit 0: at least one event is waiting in `KBD_DATA`.
pub const STATUS_EVENT: u32 = 1 << 0;
/// `KBD_CTRL` bit 0: raise PLIC source 2 while an event is waiting.
pub const CTRL_IRQ_ENABLE: u32 = 1 << 0;
/// `KBD_CTRL` bit 1: raw mode (physical keys as HID usages); clear = ASCII mode.
/// Changing it empties the FIFO. Other bits read as 0.
pub const CTRL_RAW: u32 = 1 << 1;

/// ASCII mode, `KBD_DATA` bit 8: the event is a release (0 = press).
pub const DATA_RELEASE: u32 = 1 << 8;
/// ASCII mode, `KBD_DATA` bit 9: Shift was held.
pub const DATA_SHIFT: u32 = 1 << 9;
/// ASCII mode, `KBD_DATA` bit 10: Ctrl was held.
pub const DATA_CTRL: u32 = 1 << 10;

/// Raw mode, `KBD_DATA` bits 15:0: the key's HID usage ID.
pub const RAW_USAGE: u32 = 0xffff;
/// Raw mode, `KBD_DATA` bit 16: the event is a release (0 = press).
pub const RAW_RELEASE: u32 = 1 << 16;

/// Events the FIFO holds. Events that arrive while it is full are dropped.
pub const FIFO_LEN: usize = 64;

/// ASCII mode's key codes that are not printable ASCII (spec section 9.4).
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

    /// Whether `c` is in the spec's key code table, so a keyboard can produce it.
    pub fn is_valid(c: u8) -> bool {
        matches!(c, BACKSPACE | TAB | ENTER | ESC | 0x20..=0x7e | UP..=RIGHT | F1..=F12)
    }
}

/// Raw mode's key codes: usage IDs of the Keyboard/Keypad page (0x07) of the USB HID
/// Usage Tables (version 1.4, chapter 10), the codes a USB keyboard sends. A usage
/// names a key's position, not what it types: [`A`] is the key labelled A on a US
/// keyboard, Q on a French one.
///
/// [`KEYS`] is the table hosts use to turn their own key codes into usages: the scan
/// code Windows reports and the browser's `KeyboardEvent.code`, after Chromium's
/// `ui/events/keycodes/dom/dom_code_data.inc`, which lists the same correspondence.
pub mod usage {
    /// Keys A to Z are 0x04 to 0x1d, in alphabetical order.
    pub const A: u16 = 0x04;
    pub const Z: u16 = 0x1d;
    /// Keys 1 to 9 are 0x1e to 0x26, then 0 is 0x27.
    pub const DIGIT1: u16 = 0x1e;
    pub const DIGIT0: u16 = 0x27;
    pub const ENTER: u16 = 0x28;
    pub const ESCAPE: u16 = 0x29;
    pub const BACKSPACE: u16 = 0x2a;
    pub const TAB: u16 = 0x2b;
    pub const SPACE: u16 = 0x2c;
    pub const CAPS_LOCK: u16 = 0x39;
    /// F1 to F12 are 0x3a to 0x45.
    pub const F1: u16 = 0x3a;
    pub const F12: u16 = 0x45;
    pub const PRINT_SCREEN: u16 = 0x46;
    pub const SCROLL_LOCK: u16 = 0x47;
    pub const PAUSE: u16 = 0x48;
    pub const INSERT: u16 = 0x49;
    pub const HOME: u16 = 0x4a;
    pub const PAGE_UP: u16 = 0x4b;
    pub const DELETE: u16 = 0x4c;
    pub const END: u16 = 0x4d;
    pub const PAGE_DOWN: u16 = 0x4e;
    pub const RIGHT: u16 = 0x4f;
    pub const LEFT: u16 = 0x50;
    pub const DOWN: u16 = 0x51;
    pub const UP: u16 = 0x52;
    pub const NUM_LOCK: u16 = 0x53;
    pub const KP_ENTER: u16 = 0x58;
    /// Keypad 1 to 9 are 0x59 to 0x61, keypad 0 is 0x62.
    pub const KP1: u16 = 0x59;
    pub const KP0: u16 = 0x62;
    /// The key between left Shift and Z on ISO keyboards ("Non-US \ and |").
    pub const NON_US_BACKSLASH: u16 = 0x64;
    /// The Menu key ("Keyboard Application").
    pub const APPLICATION: u16 = 0x65;
    /// Hangul/English toggle on a Korean keyboard ("Keyboard LANG1").
    pub const LANG1: u16 = 0x90;
    /// Hanja conversion on a Korean keyboard ("Keyboard LANG2").
    pub const LANG2: u16 = 0x91;
    // The modifiers, 0xe0 to 0xe7: the eight bits of a boot keyboard report's first
    // byte, in the same order.
    pub const LEFT_CTRL: u16 = 0xe0;
    pub const LEFT_SHIFT: u16 = 0xe1;
    pub const LEFT_ALT: u16 = 0xe2;
    /// Left Windows/Super/Meta ("Left GUI").
    pub const LEFT_GUI: u16 = 0xe3;
    pub const RIGHT_CTRL: u16 = 0xe4;
    pub const RIGHT_SHIFT: u16 = 0xe5;
    /// Right Alt, AltGr on many layouts: a key of its own here, never Ctrl+Alt.
    pub const RIGHT_ALT: u16 = 0xe6;
    pub const RIGHT_GUI: u16 = 0xe7;

    /// Whether `u` is a key of the Keyboard/Keypad page, so a keyboard can report it:
    /// 0x04 to 0xa4, 0xb0 to 0xdd and the modifiers 0xe0 to 0xe7. 0 means "no event"
    /// and 0x01 to 0x03 are error states, not keys; the rest is reserved.
    pub fn is_valid(u: u16) -> bool {
        matches!(u, 0x04..=0xa4 | 0xb0..=0xdd | 0xe0..=0xe7)
    }

    /// One row of [`KEYS`].
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct Key {
        /// The HID usage ID.
        pub usage: u16,
        /// The scan code Windows reports for the key in a key message's `lParam`: bits
        /// 16-23, with 0xe000 added when the extended-key flag (bit 24) is set. These
        /// are the PC's scan code set 1 codes, except for two keys Windows swaps: Num
        /// Lock is 0xe045 and Pause 0x45 (set 1 sends 0x45 for Num Lock and E1 1D 45
        /// for Pause).
        pub win: u16,
        /// The browser's `KeyboardEvent.code` for the key (W3C, "UI Events
        /// KeyboardEvent code Values").
        pub dom: &'static str,
    }

    const fn k(usage: u16, win: u16, dom: &'static str) -> Key {
        Key { usage, win, dom }
    }

    /// The keys hosts can report, in usage order: those of a 104- or 105-key PC
    /// keyboard, F13 to F24, the Japanese and Korean keys (Lang1 to Lang4), Power and
    /// the volume keys. A few keys have other names too ([`WIN_ALIASES`],
    /// [`DOM_ALIASES`]).
    /// Usage 0x32 (the ISO "#~" key, next to Enter) is missing on purpose: Windows and
    /// browsers report it as the US backslash key, 0x31, which is at the same place,
    /// and Linux's HID driver gives the two the same key code.
    pub const KEYS: &[Key] = &[
        k(0x04, 0x1e, "KeyA"),
        k(0x05, 0x30, "KeyB"),
        k(0x06, 0x2e, "KeyC"),
        k(0x07, 0x20, "KeyD"),
        k(0x08, 0x12, "KeyE"),
        k(0x09, 0x21, "KeyF"),
        k(0x0a, 0x22, "KeyG"),
        k(0x0b, 0x23, "KeyH"),
        k(0x0c, 0x17, "KeyI"),
        k(0x0d, 0x24, "KeyJ"),
        k(0x0e, 0x25, "KeyK"),
        k(0x0f, 0x26, "KeyL"),
        k(0x10, 0x32, "KeyM"),
        k(0x11, 0x31, "KeyN"),
        k(0x12, 0x18, "KeyO"),
        k(0x13, 0x19, "KeyP"),
        k(0x14, 0x10, "KeyQ"),
        k(0x15, 0x13, "KeyR"),
        k(0x16, 0x1f, "KeyS"),
        k(0x17, 0x14, "KeyT"),
        k(0x18, 0x16, "KeyU"),
        k(0x19, 0x2f, "KeyV"),
        k(0x1a, 0x11, "KeyW"),
        k(0x1b, 0x2d, "KeyX"),
        k(0x1c, 0x15, "KeyY"),
        k(0x1d, 0x2c, "KeyZ"),
        k(0x1e, 0x02, "Digit1"),
        k(0x1f, 0x03, "Digit2"),
        k(0x20, 0x04, "Digit3"),
        k(0x21, 0x05, "Digit4"),
        k(0x22, 0x06, "Digit5"),
        k(0x23, 0x07, "Digit6"),
        k(0x24, 0x08, "Digit7"),
        k(0x25, 0x09, "Digit8"),
        k(0x26, 0x0a, "Digit9"),
        k(0x27, 0x0b, "Digit0"),
        k(0x28, 0x1c, "Enter"),
        k(0x29, 0x01, "Escape"),
        k(0x2a, 0x0e, "Backspace"),
        k(0x2b, 0x0f, "Tab"),
        k(0x2c, 0x39, "Space"),
        k(0x2d, 0x0c, "Minus"),
        k(0x2e, 0x0d, "Equal"),
        k(0x2f, 0x1a, "BracketLeft"),
        k(0x30, 0x1b, "BracketRight"),
        k(0x31, 0x2b, "Backslash"),
        k(0x33, 0x27, "Semicolon"),
        k(0x34, 0x28, "Quote"),
        k(0x35, 0x29, "Backquote"),
        k(0x36, 0x33, "Comma"),
        k(0x37, 0x34, "Period"),
        k(0x38, 0x35, "Slash"),
        k(0x39, 0x3a, "CapsLock"),
        k(0x3a, 0x3b, "F1"),
        k(0x3b, 0x3c, "F2"),
        k(0x3c, 0x3d, "F3"),
        k(0x3d, 0x3e, "F4"),
        k(0x3e, 0x3f, "F5"),
        k(0x3f, 0x40, "F6"),
        k(0x40, 0x41, "F7"),
        k(0x41, 0x42, "F8"),
        k(0x42, 0x43, "F9"),
        k(0x43, 0x44, "F10"),
        k(0x44, 0x57, "F11"),
        k(0x45, 0x58, "F12"),
        k(0x46, 0xe037, "PrintScreen"),
        k(0x47, 0x46, "ScrollLock"),
        k(0x48, 0x45, "Pause"),
        k(0x49, 0xe052, "Insert"),
        k(0x4a, 0xe047, "Home"),
        k(0x4b, 0xe049, "PageUp"),
        k(0x4c, 0xe053, "Delete"),
        k(0x4d, 0xe04f, "End"),
        k(0x4e, 0xe051, "PageDown"),
        k(0x4f, 0xe04d, "ArrowRight"),
        k(0x50, 0xe04b, "ArrowLeft"),
        k(0x51, 0xe050, "ArrowDown"),
        k(0x52, 0xe048, "ArrowUp"),
        k(0x53, 0xe045, "NumLock"),
        k(0x54, 0xe035, "NumpadDivide"),
        k(0x55, 0x37, "NumpadMultiply"),
        k(0x56, 0x4a, "NumpadSubtract"),
        k(0x57, 0x4e, "NumpadAdd"),
        k(0x58, 0xe01c, "NumpadEnter"),
        k(0x59, 0x4f, "Numpad1"),
        k(0x5a, 0x50, "Numpad2"),
        k(0x5b, 0x51, "Numpad3"),
        k(0x5c, 0x4b, "Numpad4"),
        k(0x5d, 0x4c, "Numpad5"),
        k(0x5e, 0x4d, "Numpad6"),
        k(0x5f, 0x47, "Numpad7"),
        k(0x60, 0x48, "Numpad8"),
        k(0x61, 0x49, "Numpad9"),
        k(0x62, 0x52, "Numpad0"),
        k(0x63, 0x53, "NumpadDecimal"),
        k(0x64, 0x56, "IntlBackslash"),
        k(0x65, 0xe05d, "ContextMenu"),
        k(0x66, 0xe05e, "Power"),
        k(0x67, 0x59, "NumpadEqual"),
        k(0x68, 0x64, "F13"),
        k(0x69, 0x65, "F14"),
        k(0x6a, 0x66, "F15"),
        k(0x6b, 0x67, "F16"),
        k(0x6c, 0x68, "F17"),
        k(0x6d, 0x69, "F18"),
        k(0x6e, 0x6a, "F19"),
        k(0x6f, 0x6b, "F20"),
        k(0x70, 0x6c, "F21"),
        k(0x71, 0x6d, "F22"),
        k(0x72, 0x6e, "F23"),
        k(0x73, 0x76, "F24"),
        k(0x7f, 0xe020, "AudioVolumeMute"),
        k(0x80, 0xe030, "AudioVolumeUp"),
        k(0x81, 0xe02e, "AudioVolumeDown"),
        k(0x85, 0x7e, "NumpadComma"),
        k(0x87, 0x73, "IntlRo"),
        k(0x88, 0x70, "KanaMode"),
        k(0x89, 0x7d, "IntlYen"),
        k(0x8a, 0x79, "Convert"),
        k(0x8b, 0x7b, "NonConvert"),
        k(0x90, 0x72, "Lang1"),
        k(0x91, 0x71, "Lang2"),
        k(0x92, 0x78, "Lang3"),
        k(0x93, 0x77, "Lang4"),
        k(0xe0, 0x1d, "ControlLeft"),
        k(0xe1, 0x2a, "ShiftLeft"),
        k(0xe2, 0x38, "AltLeft"),
        k(0xe3, 0xe05b, "MetaLeft"),
        k(0xe4, 0xe01d, "ControlRight"),
        k(0xe5, 0x36, "ShiftRight"),
        k(0xe6, 0xe038, "AltRight"),
        k(0xe7, 0xe05c, "MetaRight"),
    ];

    /// Scan codes Windows reports for a key besides its own ([`Key::win`]), with the
    /// key's usage. A PC keyboard sends other codes for two keys under a modifier, and
    /// Windows' HID driver makes them up from the same USB usage: Pause with Ctrl held
    /// (Break) is E0 46, Print Screen with Alt held (SysRq) is 54. The key is the
    /// same one, as Linux's `atkbd` maps them (KEY_PAUSE and KEY_SYSRQ).
    pub const WIN_ALIASES: &[(u16, u16)] = &[(0xe046, PAUSE), (0x54, PRINT_SCREEN)];

    /// `KeyboardEvent.code` names that older browsers gave a key, with its usage: the
    /// Windows keys were `OSLeft` and `OSRight` (Firefox before 118), and the volume
    /// keys `VolumeMute`, `VolumeUp` and `VolumeDown`.
    pub const DOM_ALIASES: &[(&str, u16)] = &[
        ("OSLeft", LEFT_GUI),
        ("OSRight", RIGHT_GUI),
        ("VolumeMute", 0x7f),
        ("VolumeUp", 0x80),
        ("VolumeDown", 0x81),
    ];

    /// The usage of the key Windows reports as scan code `win` ([`Key::win`], or one
    /// of [`WIN_ALIASES`]), if it is one of [`KEYS`].
    pub fn from_win_scan(win: u16) -> Option<u16> {
        let alias = || WIN_ALIASES.iter().find(|a| a.0 == win).map(|a| a.1);
        KEYS.iter().find(|k| k.win == win).map(|k| k.usage).or_else(alias)
    }

    /// The usage of the key a browser reports as `KeyboardEvent.code` `dom` (or one of
    /// [`DOM_ALIASES`]), if it is one of [`KEYS`].
    pub fn from_dom_code(dom: &str) -> Option<u16> {
        let alias = || DOM_ALIASES.iter().find(|a| a.0 == dom).map(|a| a.1);
        KEYS.iter().find(|k| k.dom == dom).map(|k| k.usage).or_else(alias)
    }
}

/// One key press or release, in both of the device's forms: the ASCII-mode code and
/// the raw-mode usage. The device queues the form its mode reports, so a host that
/// knows only one of them leaves the other 0, and the key is then invisible in the
/// other mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyEvent {
    /// ASCII mode's key code, see [`code`]; 0 if the key has none (it types nothing:
    /// Shift alone, Home, an accented letter).
    pub code: u8,
    /// Raw mode's HID usage of the physical key, see [`usage`]; 0 if unknown.
    pub usage: u16,
    pub release: bool,
    /// A press of a key that is already down: the host's auto-repeat. ASCII mode
    /// reports it as another press; raw mode drops it, as a USB keyboard repeats
    /// nothing (repeating is software's job, as in Linux's input layer).
    pub repeat: bool,
    /// Shift was held (ASCII mode only: in raw mode Shift is a key of its own).
    pub shift: bool,
    /// Ctrl was held (ASCII mode only).
    pub ctrl: bool,
}

impl KeyEvent {
    /// A press with ASCII-mode code `code` and no usage.
    pub fn press(code: u8) -> Self {
        KeyEvent {
            code,
            usage: 0,
            release: false,
            repeat: false,
            shift: false,
            ctrl: false,
        }
    }

    /// A release with ASCII-mode code `code` and no usage.
    pub fn release(code: u8) -> Self {
        KeyEvent {
            release: true,
            ..Self::press(code)
        }
    }

    /// A press of the physical key `usage`, with no ASCII-mode code.
    pub fn key_press(usage: u16) -> Self {
        Self::press(0).with_usage(usage)
    }

    /// A release of the physical key `usage`, with no ASCII-mode code.
    pub fn key_release(usage: u16) -> Self {
        Self::release(0).with_usage(usage)
    }

    pub fn with_usage(self, usage: u16) -> Self {
        KeyEvent { usage, ..self }
    }

    pub fn with_repeat(self, repeat: bool) -> Self {
        KeyEvent { repeat, ..self }
    }

    pub fn with_shift(self, shift: bool) -> Self {
        KeyEvent { shift, ..self }
    }

    pub fn with_ctrl(self, ctrl: bool) -> Self {
        KeyEvent { ctrl, ..self }
    }

    /// The ASCII-mode `KBD_DATA` word: bits 7:0 code, bit 8 release, bit 9 shift, bit
    /// 10 ctrl.
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
            release: word & DATA_RELEASE != 0,
            shift: word & DATA_SHIFT != 0,
            ctrl: word & DATA_CTRL != 0,
            ..Self::press(code)
        })
    }

    /// The raw-mode `KBD_DATA` word: bits 15:0 usage, bit 16 release.
    pub fn encode_raw(self) -> u32 {
        self.usage as u32 | if self.release { RAW_RELEASE } else { 0 }
    }

    /// Inverse of [`KeyEvent::encode_raw`]; `None` for 0 (empty) and reserved bits.
    pub fn decode_raw(word: u32) -> Option<Self> {
        let usage = (word & RAW_USAGE) as u16;
        if usage == 0 || word & !(RAW_USAGE | RAW_RELEASE) != 0 {
            return None;
        }
        Some(KeyEvent {
            release: word & RAW_RELEASE != 0,
            ..Self::key_press(usage)
        })
    }
}

#[derive(Default)]
pub struct Keyboard {
    /// `KBD_DATA` words, encoded in the mode that was on when each event came.
    fifo: VecDeque<u32>,
    ctrl: u32,
    /// Events dropped because the FIFO was full.
    pub dropped: u64,
}

impl Keyboard {
    /// Queues an event for the program, in the form the current mode reports. Returns
    /// false and drops it if it has no valid form in this mode: in ASCII mode a code
    /// in the spec's table ([`code::is_valid`]; this excludes 0, which `KBD_DATA`
    /// reserves for "empty"), in raw mode a usage of the HID page ([`usage::is_valid`])
    /// on a press that is not an auto-repeat. Also false if the FIFO is full, which
    /// counts it in `dropped`.
    pub fn push(&mut self, ev: KeyEvent) -> bool {
        let word = if self.raw() {
            if !usage::is_valid(ev.usage) || (ev.repeat && !ev.release) {
                return false;
            }
            ev.encode_raw()
        } else {
            if !code::is_valid(ev.code) {
                return false;
            }
            ev.encode()
        };
        if self.fifo.len() >= FIFO_LEN {
            self.dropped += 1;
            return false;
        }
        self.fifo.push_back(word);
        true
    }

    /// Whether the program chose raw mode (`KBD_CTRL` bit 1).
    pub fn raw(&self) -> bool {
        self.ctrl & CTRL_RAW != 0
    }

    /// `KBD_CTRL` as the program last wrote it (reserved bits clear).
    pub fn ctrl(&self) -> u32 {
        self.ctrl
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
            REG_DATA => self.fifo.pop_front().unwrap_or(0),
            REG_CTRL => self.ctrl,
            _ => 0,
        })
    }

    /// Only `KBD_CTRL` is writable; writes elsewhere are ignored. A write that changes
    /// the mode empties the FIFO, so the program never reads an event of the other
    /// mode (a PS/2 keyboard likewise clears its output buffer when told to change scan
    /// code sets).
    pub fn write(&mut self, off: u32, size: u32, val: u32) -> Result<(), BusFault> {
        if size != 4 {
            return Err(BusFault);
        }
        if off == REG_CTRL {
            let val = val & (CTRL_IRQ_ENABLE | CTRL_RAW);
            if (val ^ self.ctrl) & CTRL_RAW != 0 {
                self.fifo.clear();
            }
            self.ctrl = val;
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

    fn raw_keyboard() -> Keyboard {
        let mut k = Keyboard::default();
        k.write(REG_CTRL, 4, CTRL_RAW).unwrap();
        k
    }

    #[test]
    fn empty_keyboard_reads_zero() {
        let mut k = Keyboard::default();
        assert_eq!(r(&mut k, REG_STATUS), 0);
        assert_eq!(r(&mut k, REG_DATA), 0);
        assert_eq!(r(&mut k, REG_DATA), 0, "popping an empty FIFO changes nothing");
        assert_eq!(r(&mut k, REG_CTRL), 0);
        assert!(!k.irq());
        assert!(!k.raw(), "ASCII mode at reset");
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
        // The usage is not part of the ASCII-mode word.
        assert_eq!(all.with_usage(usage::RIGHT).encode(), 0x783);
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
    fn raw_encoding_matches_the_spec() {
        assert_eq!(KeyEvent::key_press(usage::A).encode_raw(), 0x0_0004);
        assert_eq!(KeyEvent::key_release(usage::A).encode_raw(), 0x1_0004);
        assert_eq!(KeyEvent::key_press(usage::RIGHT_GUI).encode_raw(), 0x0_00e7);
        // Shift and Ctrl are keys of their own in raw mode, not bits; the code is not
        // part of the word.
        let ev = KeyEvent::release(b'a')
            .with_usage(usage::A)
            .with_shift(true)
            .with_ctrl(true);
        assert_eq!(ev.encode_raw(), 0x1_0004);
        for ev in [
            KeyEvent::key_press(usage::LEFT_SHIFT),
            KeyEvent::key_release(0xffff),
        ] {
            assert_eq!(KeyEvent::decode_raw(ev.encode_raw()), Some(ev));
        }
        assert_eq!(KeyEvent::decode_raw(0), None, "0 means empty");
        assert_eq!(KeyEvent::decode_raw(0x1_0000), None, "a release without a usage");
        assert_eq!(KeyEvent::decode_raw(0x2_0004), None, "reserved bit 17");
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
        assert!(
            !k.push(KeyEvent::key_press(usage::LEFT_SHIFT)),
            "no code: nothing in ASCII mode"
        );
        assert!(k.is_empty());
        assert_eq!(k.dropped, 0);
    }

    #[test]
    fn only_codes_from_the_table_are_queued() {
        let valid: Vec<u8> = (0..=255).filter(|&c| code::is_valid(c)).collect();
        let want: Vec<u8> = [0x08, 0x09, 0x0a, 0x1b]
            .into_iter()
            .chain(0x20..=0x7e)
            .chain(0x80..=0x83)
            .chain(0x90..=0x9b)
            .collect();
        assert_eq!(valid, want);
        let mut k = Keyboard::default();
        for c in 0..=255u8 {
            let ev = KeyEvent::release(c).with_shift(true);
            assert_eq!(k.push(ev), code::is_valid(c), "code {c:#x}");
            if k.len() == FIFO_LEN {
                while k.read(REG_DATA, 4).unwrap() != 0 {}
            }
        }
        assert_eq!(k.dropped, 0, "an invalid code is not a FIFO overflow");
    }

    #[test]
    fn only_usages_of_the_keyboard_page_are_queued() {
        let valid: Vec<u16> = (0..=0xffff).filter(|&u| usage::is_valid(u)).collect();
        let want: Vec<u16> = (0x04..=0xa4).chain(0xb0..=0xdd).chain(0xe0..=0xe7).collect();
        assert_eq!(valid, want);
        let mut k = raw_keyboard();
        for u in 0..=0xffffu16 {
            assert_eq!(
                k.push(KeyEvent::key_release(u)),
                usage::is_valid(u),
                "usage {u:#x}"
            );
            if k.len() == FIFO_LEN {
                while k.read(REG_DATA, 4).unwrap() != 0 {}
            }
        }
        assert_eq!(k.dropped, 0, "an invalid usage is not a FIFO overflow");
        // A key with a code but no usage is invisible in raw mode.
        assert!(!k.push(KeyEvent::press(b'a')));
    }

    #[test]
    fn raw_mode_reports_physical_keys() {
        let mut k = raw_keyboard();
        assert_eq!(r(&mut k, REG_CTRL), CTRL_RAW);
        assert!(k.raw());
        // Shift+A as the window sends it: the code with Shift's bit, and the usages.
        let shift = KeyEvent::key_press(usage::LEFT_SHIFT);
        let a = KeyEvent::press(b'A').with_usage(usage::A).with_shift(true);
        for ev in [
            shift,
            a,
            KeyEvent { release: true, ..a },
            KeyEvent {
                release: true,
                ..shift
            },
        ] {
            assert!(k.push(ev));
        }
        let words: Vec<u32> = (0..5).map(|_| r(&mut k, REG_DATA)).collect();
        assert_eq!(words, [0xe1, 0x04, 0x1_0004, 0x1_00e1, 0]);
    }

    #[test]
    fn modifiers_alone_and_left_and_right() {
        let mut k = raw_keyboard();
        let mods = [
            usage::LEFT_CTRL,
            usage::LEFT_SHIFT,
            usage::LEFT_ALT,
            usage::LEFT_GUI,
            usage::RIGHT_CTRL,
            usage::RIGHT_SHIFT,
            usage::RIGHT_ALT,
            usage::RIGHT_GUI,
        ];
        assert_eq!(mods, [0xe0, 0xe1, 0xe2, 0xe3, 0xe4, 0xe5, 0xe6, 0xe7]);
        for &m in &mods {
            assert!(k.push(KeyEvent::key_press(m)));
        }
        for &m in mods.iter().rev() {
            assert!(k.push(KeyEvent::key_release(m)));
        }
        let want: Vec<u32> = mods
            .iter()
            .map(|&m| m as u32)
            .chain(mods.iter().rev().map(|&m| m as u32 | RAW_RELEASE))
            .collect();
        let got: Vec<u32> = (0..16).map(|_| r(&mut k, REG_DATA)).collect();
        assert_eq!(got, want);
        assert!(k.is_empty());
    }

    #[test]
    fn auto_repeat_is_a_press_in_ascii_mode_only() {
        let mut k = Keyboard::default();
        let down = KeyEvent::press(code::LEFT).with_usage(usage::LEFT);
        for ev in [
            down,
            down.with_repeat(true),
            down.with_repeat(true),
            KeyEvent {
                release: true,
                ..down
            },
        ] {
            assert!(k.push(ev));
        }
        let ascii: Vec<u32> = (0..4).map(|_| r(&mut k, REG_DATA)).collect();
        assert_eq!(ascii, [0x82, 0x82, 0x82, 0x182]);

        let mut k = raw_keyboard();
        assert!(k.push(down));
        assert!(!k.push(down.with_repeat(true)), "a USB keyboard does not repeat");
        assert!(k.push(KeyEvent {
            release: true,
            ..down
        }));
        assert_eq!(k.dropped, 0);
        let raw: Vec<u32> = (0..3).map(|_| r(&mut k, REG_DATA)).collect();
        assert_eq!(raw, [usage::LEFT as u32, usage::LEFT as u32 | RAW_RELEASE, 0]);
    }

    #[test]
    fn changing_the_mode_empties_the_fifo() {
        let mut k = Keyboard::default();
        k.push(KeyEvent::press(b'a').with_usage(usage::A));
        k.write(REG_CTRL, 4, CTRL_IRQ_ENABLE).unwrap();
        assert_eq!(k.len(), 1, "the interrupt enable alone keeps the events");
        k.write(REG_CTRL, 4, CTRL_IRQ_ENABLE | CTRL_RAW).unwrap();
        assert!(k.is_empty(), "no ASCII event is read in raw mode");
        assert!(!k.irq());
        k.push(KeyEvent::release(b'a').with_usage(usage::A));
        k.write(REG_CTRL, 4, CTRL_RAW).unwrap();
        assert_eq!(r(&mut k, REG_DATA), 0x1_0004, "same mode, events kept");
        k.push(KeyEvent::key_press(usage::ESCAPE));
        k.write(REG_CTRL, 4, 0).unwrap();
        assert!(k.is_empty());
        assert_eq!(k.dropped, 0, "a flush is not an overflow");
    }

    #[test]
    fn usage_table() {
        use usage::*;
        // Ordered and unique by usage; every usage valid, every host code unique.
        assert!(KEYS.windows(2).all(|w| w[0].usage < w[1].usage));
        for (i, key) in KEYS.iter().enumerate() {
            assert!(is_valid(key.usage), "{key:?}");
            assert_ne!(key.win, 0, "{key:?}");
            assert!(
                KEYS[i + 1..].iter().all(|o| o.win != key.win && o.dom != key.dom),
                "{key:?}"
            );
            assert_eq!(from_win_scan(key.win), Some(key.usage));
            assert_eq!(from_dom_code(key.dom), Some(key.usage));
        }
        // The page's runs: letters, digits, F1-F12, the keypad digits, the modifiers.
        let at = |u: u16| KEYS.iter().find(|k| k.usage == u).unwrap().dom;
        for (i, c) in ('A'..='Z').enumerate() {
            assert_eq!(at(A + i as u16), format!("Key{c}"));
        }
        assert_eq!(Z - A, 25);
        for n in 1..=9 {
            assert_eq!(at(DIGIT1 + n - 1), format!("Digit{n}"));
            assert_eq!(at(KP1 + n - 1), format!("Numpad{n}"));
        }
        assert_eq!((at(DIGIT0), at(KP0)), ("Digit0", "Numpad0"));
        for n in 1..=12 {
            assert_eq!(at(F1 + n - 1), format!("F{n}"));
        }
        assert_eq!(F12, 0x45);
        let named = [
            (ENTER, "Enter"),
            (ESCAPE, "Escape"),
            (BACKSPACE, "Backspace"),
            (TAB, "Tab"),
            (SPACE, "Space"),
            (CAPS_LOCK, "CapsLock"),
            (PRINT_SCREEN, "PrintScreen"),
            (SCROLL_LOCK, "ScrollLock"),
            (PAUSE, "Pause"),
            (INSERT, "Insert"),
            (HOME, "Home"),
            (PAGE_UP, "PageUp"),
            (DELETE, "Delete"),
            (END, "End"),
            (PAGE_DOWN, "PageDown"),
            (RIGHT, "ArrowRight"),
            (LEFT, "ArrowLeft"),
            (DOWN, "ArrowDown"),
            (UP, "ArrowUp"),
            (NUM_LOCK, "NumLock"),
            (KP_ENTER, "NumpadEnter"),
            (NON_US_BACKSLASH, "IntlBackslash"),
            (APPLICATION, "ContextMenu"),
            (LEFT_CTRL, "ControlLeft"),
            (LEFT_SHIFT, "ShiftLeft"),
            (LEFT_ALT, "AltLeft"),
            (LEFT_GUI, "MetaLeft"),
            (RIGHT_CTRL, "ControlRight"),
            (RIGHT_SHIFT, "ShiftRight"),
            (RIGHT_ALT, "AltRight"),
            (RIGHT_GUI, "MetaRight"),
        ];
        for (u, dom) in named {
            assert_eq!(at(u), dom, "usage {u:#x}");
        }
        assert_eq!((at(LANG1), at(LANG2)), ("Lang1", "Lang2"));
        assert_eq!((at(0x92), at(0x93)), ("Lang3", "Lang4"));
    }

    #[test]
    fn aliases_name_keys_of_the_table() {
        use usage::*;
        // An alias is no key's own code, and names a key of the table.
        for &(win, u) in WIN_ALIASES {
            assert!(KEYS.iter().all(|k| k.win != win), "{win:#x}");
            assert!(KEYS.iter().any(|k| k.usage == u), "{win:#x}");
            assert_eq!(from_win_scan(win), Some(u));
        }
        for &(dom, u) in DOM_ALIASES {
            assert!(KEYS.iter().all(|k| k.dom != dom), "{dom}");
            assert!(KEYS.iter().any(|k| k.usage == u), "{dom}");
            assert_eq!(from_dom_code(dom), Some(u));
        }
        // Ctrl+Break is Pause, Alt+Print Screen (SysRq) is Print Screen.
        assert_eq!(from_win_scan(0xe046), Some(PAUSE));
        assert_eq!(from_win_scan(0x54), Some(PRINT_SCREEN));
        assert_eq!(from_win_scan(0x46), Some(SCROLL_LOCK), "without E0, Scroll Lock");
        assert_eq!(from_dom_code("OSLeft"), from_dom_code("MetaLeft"));
        assert_eq!(from_dom_code("OSRight"), from_dom_code("MetaRight"));
        assert_eq!(from_dom_code("VolumeMute"), from_dom_code("AudioVolumeMute"));
        assert_eq!(from_dom_code("VolumeUp"), from_dom_code("AudioVolumeUp"));
        assert_eq!(from_dom_code("VolumeDown"), from_dom_code("AudioVolumeDown"));
    }

    #[test]
    fn windows_scan_codes() {
        use usage::*;
        // Extended keys (bit 24 of lParam, 0xe0 here) are the right-hand modifiers and
        // the grey keys; without it the same scan code is the left modifier or the
        // keypad key.
        for (plain, ext, left, right) in [
            (0x1d, 0xe01d, LEFT_CTRL, RIGHT_CTRL),
            (0x38, 0xe038, LEFT_ALT, RIGHT_ALT),
            (0x1c, 0xe01c, ENTER, KP_ENTER),
            (0x47, 0xe047, KP1 + 6, HOME),
            (0x48, 0xe048, KP1 + 7, UP),
            (0x53, 0xe053, KP1 + 10, DELETE),
            (0x35, 0xe035, 0x38, 0x54),
        ] {
            assert_eq!(from_win_scan(plain), Some(left), "{plain:#x}");
            assert_eq!(from_win_scan(ext), Some(right), "{ext:#x}");
        }
        // Right Shift is not an extended key; the Windows keys are.
        assert_eq!(from_win_scan(0x2a), Some(LEFT_SHIFT));
        assert_eq!(from_win_scan(0x36), Some(RIGHT_SHIFT));
        assert_eq!(from_win_scan(0xe05b), Some(LEFT_GUI));
        assert_eq!(from_win_scan(0xe05c), Some(RIGHT_GUI));
        // Windows' swap of Num Lock and Pause.
        assert_eq!(from_win_scan(0xe045), Some(NUM_LOCK));
        assert_eq!(from_win_scan(0x45), Some(PAUSE));
        // Nothing for 0 (a synthesized message with no scan code) or unknown codes.
        assert_eq!(from_win_scan(0), None);
        assert_eq!(from_win_scan(0xe0ff), None);
        assert_eq!(from_dom_code(""), None);
        assert_eq!(from_dom_code("keya"), None, "codes are case-sensitive");
    }

    #[test]
    fn irq_line_follows_ctrl_and_status() {
        let mut k = Keyboard::default();
        k.push(KeyEvent::press(b'a'));
        assert!(!k.irq(), "interrupts disabled");
        k.write(REG_CTRL, 4, !CTRL_RAW).unwrap();
        assert_eq!(r(&mut k, REG_CTRL), CTRL_IRQ_ENABLE, "reserved bits read 0");
        assert!(k.irq());
        r(&mut k, REG_DATA);
        assert!(!k.irq(), "drained");
        k.push(KeyEvent::release(b'a'));
        assert!(k.irq());
        k.write(REG_CTRL, 4, 0).unwrap();
        assert!(!k.irq());
        assert_eq!(r(&mut k, REG_STATUS), STATUS_EVENT, "the event is still there");
        k.write(REG_CTRL, 4, 0xffff_ffff).unwrap();
        assert_eq!(
            r(&mut k, REG_CTRL),
            CTRL_IRQ_ENABLE | CTRL_RAW,
            "bits 1:0 are kept"
        );
        k.push(KeyEvent::key_press(usage::A));
        assert!(k.irq(), "raw mode interrupts the same way");
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
        assert_eq!(k.read(REG_DATA, 8), Err(BusFault));
        assert_eq!(k.len(), 1, "a faulting read pops nothing");
        assert_eq!(k.write(REG_CTRL, 1, 3), Err(BusFault));
        assert_eq!(k.write(REG_CTRL, 2, 3), Err(BusFault));
        assert_eq!(k.write(REG_CTRL, 8, 3), Err(BusFault));
        assert_eq!(k.ctrl(), 0);
        assert_eq!(k.len(), 1, "a faulting write changes no mode");
    }
}
