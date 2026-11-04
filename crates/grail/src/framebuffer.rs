//! Xcal framebuffer (spec section 9.3): a small display controller. It has a few modes
//! (640x480 to 1920x1080), shows ("scans out") either its own 640x480 VRAM or any
//! page-aligned buffer in RAM, latches a new buffer at vertical blank (a page flip), and
//! raises an interrupt there, on PLIC source [`crate::devices::FB_IRQ`].
//!
//! This is the model of Linux's KMS, and of the display controllers it drives: a mode
//! (the CRTC's timing), a plane that points at a framebuffer in memory, and a page flip
//! that takes effect at the next vertical blank, with a vblank event.
//!
//! - Writing `FB_MODE` is a modeset (`drmModeSetCrtc`): the new mode and the buffer in
//!   `FB_SCANOUT` take effect together, at once, if the buffer holds a frame of the mode.
//! - Writing `FB_FLIP` queues `FB_SCANOUT` for the next vertical blank
//!   (`drmModePageFlip`), so a program that draws into one buffer while the other is
//!   shown never shows a half-drawn frame.
//! - Every vertical blank sets `FB_IRQ`'s pending bit, which interrupts while enabled.
//! - A **cursor plane** (KMS's cursor plane, `DRM_IOCTL_MODE_CURSOR`): a 64x64 ARGB8888
//!   image in RAM, premultiplied, which the host blends over the frame where the guest
//!   puts it, so that moving the pointer redraws nothing. Its registers are
//!   double-buffered: what the guest writes takes effect at the next vertical blank, as a
//!   flip does.
//!
//! The vertical blank is guest time: [`Framebuffer::advance`] counts the machine's ticks
//! (one per step, as `mtime`), and a frame is a sixtieth of a second of them
//! ([`REFRESH_HZ`]). A run without `--clock` stays reproducible, and a paced one has 60
//! frames per wall-clock second.
//!
//! The device only holds state. Showing it is the host's job: `display` blits the frame
//! ([`Framebuffer::frame`]) into a window or dumps it as PPM files, and Avalon streams it
//! to viewers.

use crate::bus::{BusFault, RAM_BASE};
use crate::mmio::{self, Field};
use std::io;
use std::path::{Path, PathBuf};

/// The size of mode 0, the default: what the display shows at reset, and what the
/// built-in VRAM holds.
pub const FB_WIDTH: u32 = 640;
pub const FB_HEIGHT: u32 = 480;
pub const FB_STRIDE: u32 = FB_WIDTH * 4;
/// 0 = XRGB8888: each pixel is a little-endian word 0x00RRGGBB.
pub const FB_FORMAT_XRGB8888: u32 = 0;
/// The built-in VRAM: one 640x480 frame. Bigger modes scan out of RAM.
pub const VRAM_SIZE: u32 = FB_STRIDE * FB_HEIGHT;
/// `FB_VERSION`: 2 is the display controller of spec section 9.3 with its cursor plane;
/// 1 was the same without it (no cursor registers: they read as 0). The framebuffer
/// before that had nothing at this offset, which read as 0.
pub const FB_VERSION: u32 = 2;
/// Vertical blanks per second of guest time.
pub const REFRESH_HZ: u32 = 60;
/// A scanout buffer in RAM starts on a page.
pub const SCANOUT_ALIGN: u64 = 4096;
/// The cursor image's width and height in pixels: 64x64, the size KMS drivers commonly
/// give (`DRM_CAP_CURSOR_WIDTH` and `DRM_CAP_CURSOR_HEIGHT`), and the only one this
/// cursor plane has. `FB_CURSOR_SIZE` says so.
pub const CURSOR_SIZE: u32 = 64;
/// Bytes of the cursor image: 64 rows of 64 pixels of 4 bytes, packed, 16 KiB. It starts
/// on a page in RAM, as a scanout buffer does.
pub const CURSOR_BYTES: u64 = (CURSOR_SIZE * CURSOR_SIZE * 4) as u64;

/// A display mode. Rows are packed: the stride is `width * 4` bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mode {
    pub width: u32,
    pub height: u32,
}

impl Mode {
    /// Bytes per row.
    pub const fn stride(&self) -> u32 {
        self.width * 4
    }

    /// Bytes of a frame in this mode.
    pub const fn frame_size(&self) -> u64 {
        self.stride() as u64 * self.height as u64
    }
}

/// The modes `FB_MODE` selects, by index: 640x480 (the default), 800x600, 1024x768,
/// 1280x720, 1280x1024 and 1920x1080, all at [`REFRESH_HZ`]. Only mode 0 fits in VRAM.
pub const MODES: [Mode; 6] = [
    Mode {
        width: 640,
        height: 480,
    },
    Mode {
        width: 800,
        height: 600,
    },
    Mode {
        width: 1024,
        height: 768,
    },
    Mode {
        width: 1280,
        height: 720,
    },
    Mode {
        width: 1280,
        height: 1024,
    },
    Mode {
        width: 1920,
        height: 1080,
    },
];

/// The largest mode's frame, in bytes: what a host showing any mode must be able to hold.
pub const MAX_FRAME_SIZE: u64 = MODES[MODES.len() - 1].frame_size();

/// The index in [`MODES`] of the mode `width` × `height`, if there is one.
pub fn mode_index(width: u32, height: u32) -> Option<u32> {
    MODES
        .iter()
        .position(|m| (m.width, m.height) == (width, height))
        .map(|i| i as u32)
}

// Register offsets from the start of the control block.
pub const REG_WIDTH: u32 = 0x00;
pub const REG_HEIGHT: u32 = 0x04;
pub const REG_STRIDE: u32 = 0x08;
pub const REG_FORMAT: u32 = 0x0c;
pub const REG_CTRL: u32 = 0x10;
pub const REG_PRESENT: u32 = 0x14;
pub const REG_VERSION: u32 = 0x18;
pub const REG_MODES: u32 = 0x1c;
pub const REG_MODE: u32 = 0x20;
pub const REG_MODE_SEL: u32 = 0x24;
pub const REG_MODE_WIDTH: u32 = 0x28;
pub const REG_MODE_HEIGHT: u32 = 0x2c;
pub const REG_MODE_STRIDE: u32 = 0x30;
pub const REG_STATUS: u32 = 0x34;
/// 64-bit: 0x38 is its low half, 0x3c its high half.
pub const REG_SCANOUT: u32 = 0x38;
pub const REG_FLIP: u32 = 0x40;
pub const REG_IRQ: u32 = 0x44;
pub const REG_VBLANKS: u32 = 0x48;
pub const REG_CURSOR_CTRL: u32 = 0x4c;
/// 64-bit: 0x50 is its low half, 0x54 its high half.
pub const REG_CURSOR_ADDR: u32 = 0x50;
pub const REG_CURSOR_POS: u32 = 0x58;
pub const REG_CURSOR_HOT: u32 = 0x5c;
pub const REG_CURSOR_SIZE: u32 = 0x60;

/// `FB_CTRL` bit 0: display enable. The other bits are reserved and read as 0.
pub const CTRL_ENABLE: u32 = 1 << 0;

/// `FB_STATUS` bit 0 (read-only): a flip waits for the next vertical blank.
pub const STATUS_FLIP_PENDING: u32 = 1 << 0;
/// `FB_STATUS` bit 1 (write 1 to clear): a write to `FB_MODE` named no mode.
pub const STATUS_BAD_MODE: u32 = 1 << 1;
/// `FB_STATUS` bit 2 (write 1 to clear): a modeset or a flip was refused because
/// `FB_SCANOUT` named no buffer for the mode.
pub const STATUS_BAD_SCANOUT: u32 = 1 << 2;
/// `FB_STATUS` bit 3 (read-only): cursor registers were written since the last vertical
/// blank, which has yet to take them.
pub const STATUS_CURSOR_PENDING: u32 = 1 << 3;
/// `FB_STATUS` bit 4 (write 1 to clear): a vertical blank took an enabled cursor whose
/// `FB_CURSOR_ADDR` named no cursor image, and showed no cursor.
pub const STATUS_BAD_CURSOR: u32 = 1 << 4;
const STATUS_ERRORS: u32 = STATUS_BAD_MODE | STATUS_BAD_SCANOUT | STATUS_BAD_CURSOR;

/// `FB_CURSOR_CTRL` bit 0: the cursor plane is on. The other bits are reserved.
pub const CURSOR_CTRL_ENABLE: u32 = 1 << 0;
/// `FB_CURSOR_HOT`'s bits: the hotspot's x in bits 5:0, its y in bits 21:16, each 0 to 63.
const CURSOR_HOT_MASK: u32 = 0x003f_003f;

/// `FB_IRQ` bit 0: vertical-blank interrupt enable.
pub const IRQ_VBLANK_ENABLE: u32 = 1 << 0;
/// `FB_IRQ` bit 1 (write 1 to clear): a vertical blank happened since it was cleared.
pub const IRQ_VBLANK: u32 = 1 << 1;

/// The control block's registers, as [`mmio::Field`] names them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reg {
    Width,
    Height,
    Stride,
    Format,
    Ctrl,
    Present,
    Version,
    Modes,
    Mode,
    ModeSel,
    ModeWidth,
    ModeHeight,
    ModeStride,
    Status,
    /// 64-bit.
    Scanout,
    Flip,
    Irq,
    Vblanks,
    CursorCtrl,
    /// 64-bit.
    CursorAddr,
    CursorPos,
    CursorHot,
    CursorSize,
}

/// The cursor plane as a vertical blank latched it: what the display shows.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CursorState {
    /// Shown: `FB_CURSOR_CTRL` enabled it and `FB_CURSOR_ADDR` named an image.
    pub on: bool,
    /// The image's physical address in RAM, page-aligned (meaningful while `on`).
    pub addr: u64,
    /// Where the image's top left pixel is on the screen: negative, or past the mode's
    /// size, for an image that hangs off an edge (KMS's `CRTC_X` and `CRTC_Y`).
    pub x: i32,
    pub y: i32,
    /// The hotspot, the pixel of the image that points. The display does not use it (the
    /// image is placed by its top left corner); a viewer that draws the cursor at its
    /// own pointer does (KMS's `HOTSPOT_X` and `HOTSPOT_Y`, for virtual hardware).
    pub hot_x: u32,
    pub hot_y: u32,
}

/// The cursor plane over a [`Frame`]: an image of `width` × `height` pixels in ARGB8888
/// with premultiplied alpha (little-endian words 0xAARRGGBB, so bytes B, G, R, A, each
/// colour already multiplied by A), its top left pixel at (`x`, `y`) of the frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CursorPlane<'a> {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    /// The hotspot, for a viewer that draws the cursor at its own pointer; blending does
    /// not use it.
    pub hot_x: u32,
    pub hot_y: u32,
    /// `width * 4` bytes per row, `height` rows, top row first.
    pub pixels: &'a [u8],
}

/// `a` × `b` / 255, rounded to the nearest, for 8-bit values: pixman's `MUL_UN8`.
fn mul_un8(a: u8, b: u8) -> u8 {
    let t = u32::from(a) * u32::from(b) + 0x80;
    (((t >> 8) + t) >> 8) as u8
}

/// Blends the premultiplied ARGB8888 pixel `src` over the XRGB8888 pixel `dst`, with
/// Porter and Duff's OVER: each colour becomes `src + dst × (255 - alpha) / 255`,
/// saturated, as pixman computes it. `dst`'s X byte is kept.
pub fn blend_pixel(dst: &mut [u8], src: &[u8]) {
    let alpha = src[3];
    for c in 0..3 {
        dst[c] = src[c].saturating_add(mul_un8(dst[c], 255 - alpha));
    }
}

impl CursorPlane<'_> {
    /// The pixel of the image at frame position (`fx`, `fy`), if the image covers it.
    fn source(&self, fx: u32, fy: u32) -> Option<&[u8]> {
        let sx = i64::from(fx) - i64::from(self.x);
        let sy = i64::from(fy) - i64::from(self.y);
        if !(0..i64::from(self.width)).contains(&sx) || !(0..i64::from(self.height)).contains(&sy) {
            return None;
        }
        let at = (sy as usize * self.width as usize + sx as usize) * 4;
        Some(&self.pixels[at..at + 4])
    }

    /// Blends the cursor over `dst`, a `width` × `height` XRGB8888 frame (rows packed,
    /// top row first), with OVER on premultiplied alpha ([`blend_pixel`]), as KMS's
    /// default blend mode ("Pre-multiplied") and pixman do. What of the image is off the
    /// frame is left out.
    pub fn blend_into(&self, dst: &mut [u8], width: u32, height: u32) {
        let left = i64::from(self.x).clamp(0, i64::from(width)) as u32;
        let right = (i64::from(self.x) + i64::from(self.width)).clamp(0, i64::from(width)) as u32;
        let top = i64::from(self.y).clamp(0, i64::from(height)) as u32;
        let bottom = (i64::from(self.y) + i64::from(self.height)).clamp(0, i64::from(height)) as u32;
        for fy in top..bottom {
            for fx in left..right {
                let src = self.source(fx, fy).expect("inside the image");
                let at = (fy as usize * width as usize + fx as usize) * 4;
                blend_pixel(&mut dst[at..at + 4], src);
            }
        }
    }
}

/// What the display shows: the current mode's size and, while the display is enabled,
/// its pixels, and the cursor plane over them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Frame<'a> {
    pub width: u32,
    pub height: u32,
    /// `width * 4` bytes per row, `height` rows, top row first, XRGB8888 (bytes B, G, R,
    /// X: a 32-bpp Windows DIB as well). `None` while the display is disabled: black.
    /// This is the plane that scans out the frame, without the cursor.
    pub pixels: Option<&'a [u8]>,
    /// The cursor plane over it, while one is shown (never while the display is
    /// disabled).
    pub cursor: Option<CursorPlane<'a>>,
}

impl Frame<'_> {
    /// Pixel at (x, y) as 0x00RRGGBB, with the cursor blended over it where it covers
    /// it; black while the display is disabled.
    pub fn pixel(&self, x: u32, y: u32) -> u32 {
        let Some(p) = self.pixels else {
            return 0;
        };
        let off = (y as usize * self.width as usize + x as usize) * 4;
        let mut px = [p[off], p[off + 1], p[off + 2], p[off + 3]];
        if let Some(src) = self.cursor.as_ref().and_then(|c| c.source(x, y)) {
            blend_pixel(&mut px, src);
        }
        u32::from_le_bytes(px) & 0x00ff_ffff
    }

    /// What the display shows, `width * 4` bytes per row: the pixels with the cursor
    /// blended over them (a copy only while there is a cursor). `None` while the display
    /// is disabled.
    pub fn composited(&self) -> Option<std::borrow::Cow<'_, [u8]>> {
        let pixels = self.pixels?;
        Some(match &self.cursor {
            None => std::borrow::Cow::Borrowed(pixels),
            Some(cursor) => {
                let mut out = pixels.to_vec();
                cursor.blend_into(&mut out, self.width, self.height);
                std::borrow::Cow::Owned(out)
            }
        })
    }

    /// The frame as a binary PPM (P6), the cursor blended over it. A disabled display is
    /// black.
    pub fn to_ppm(&self) -> Vec<u8> {
        let header = format!("P6\n{} {}\n255\n", self.width, self.height);
        let len = header.len() + self.width as usize * self.height as usize * 3;
        let mut out = Vec::with_capacity(len);
        out.extend_from_slice(header.as_bytes());
        let Some(pixels) = self.composited() else {
            out.resize(len, 0);
            return out;
        };
        // Rows are packed (stride = width * 4), so a frame is just a run of pixels.
        for &[b, g, r, _] in pixels.as_chunks::<4>().0 {
            out.extend_from_slice(&[r, g, b]);
        }
        out
    }
}

pub struct Framebuffer {
    /// The built-in VRAM, `FB_STRIDE` bytes per row, top row first. Byte order B, G, R,
    /// X, which is also what a 32-bpp Windows DIB expects, so it can be blitted as is.
    pub vram: Vec<u8>,
    pub ctrl: u32,
    /// Set by a write to `FB_PRESENT`, by a flip taking effect and by a modeset; cleared
    /// by the host with `take_present`.
    present: bool,
    /// How many times the program wrote `FB_PRESENT`.
    pub presents: u64,
    /// How many flips took effect at a vertical blank.
    pub flips: u64,
    /// How many modesets (writes to `FB_MODE` that took effect) there were.
    pub modesets: u64,
    /// Vertical blanks since reset (`FB_VBLANKS` shows the low 32 bits).
    pub vblanks: u64,
    /// Index of the current mode in [`MODES`].
    mode: u32,
    mode_sel: u32,
    /// `FB_SCANOUT` as written: what the next modeset or flip shows.
    scanout_reg: u64,
    /// The buffer shown now: 0 (VRAM) or a RAM address checked against the mode.
    live: u64,
    /// The buffer a flip latches at the next vertical blank, checked when it was queued.
    pending: Option<u64>,
    /// `FB_STATUS`'s error bits.
    errors: u32,
    /// `FB_IRQ`.
    irq: u32,
    /// Ticks per frame.
    period: u64,
    /// Ticks until the next vertical blank, 1 to `period`.
    to_vblank: u64,
    /// The cursor registers as written (`FB_CURSOR_CTRL`, `FB_CURSOR_ADDR`,
    /// `FB_CURSOR_POS`, `FB_CURSOR_HOT`): what the next vertical blank takes.
    cursor_ctrl: u32,
    cursor_addr: u64,
    cursor_pos: u32,
    cursor_hot: u32,
    /// A cursor register was written since the last vertical blank (`FB_STATUS` bit 3).
    cursor_armed: bool,
    /// The cursor the display shows, as the last vertical blank that took the registers
    /// latched it.
    cursor: CursorState,
    /// How many vertical blanks changed the cursor shown (`cursor`): it moved, appeared,
    /// went, or took another image or hotspot.
    pub cursor_updates: u64,
    /// Set when a vertical blank changes the cursor shown; cleared by the host with
    /// `take_cursor_update`.
    cursor_update: bool,
    /// The size of RAM, as the last register write gave it: a vertical blank checks the
    /// cursor's address against it (only a write arms the cursor registers).
    ram_len: u64,
}

impl Default for Framebuffer {
    /// The framebuffer of a machine on the default nominal clock.
    fn default() -> Self {
        Framebuffer::new(crate::config::DEFAULT_NOMINAL_HZ)
    }
}

impl Framebuffer {
    /// The framebuffer at reset of a machine whose clock ticks `hz` times a second: mode
    /// 0, VRAM scanned out, the display off, interrupts off, the first vertical blank a
    /// frame away.
    pub fn new(hz: u32) -> Self {
        let period = u64::from((hz / REFRESH_HZ).max(1));
        Framebuffer {
            vram: vec![0; VRAM_SIZE as usize],
            ctrl: 0,
            present: false,
            presents: 0,
            flips: 0,
            modesets: 0,
            vblanks: 0,
            mode: 0,
            mode_sel: 0,
            scanout_reg: 0,
            live: 0,
            pending: None,
            errors: 0,
            irq: 0,
            period,
            to_vblank: period,
            cursor_ctrl: 0,
            cursor_addr: 0,
            cursor_pos: 0,
            cursor_hot: 0,
            cursor_armed: false,
            cursor: CursorState::default(),
            cursor_updates: 0,
            cursor_update: false,
            ram_len: 0,
        }
    }

    /// The cursor the display shows: what the last vertical blank latched.
    pub fn cursor(&self) -> CursorState {
        self.cursor
    }

    /// The current mode.
    pub fn mode(&self) -> Mode {
        MODES[self.mode as usize]
    }

    /// The buffer shown now: 0 for VRAM, else its physical address in RAM.
    pub fn scanout(&self) -> u64 {
        self.live
    }

    /// Ticks per frame: the machine's clock over [`REFRESH_HZ`].
    pub fn frame_ticks(&self) -> u64 {
        self.period
    }

    /// What the 4-byte-aligned offset `off` holds: a 32-bit register, a half of the
    /// 64-bit `FB_SCANOUT`, or nothing.
    fn field(off: u32) -> Field<Reg> {
        Field::Word(match off {
            REG_WIDTH => Reg::Width,
            REG_HEIGHT => Reg::Height,
            REG_STRIDE => Reg::Stride,
            REG_FORMAT => Reg::Format,
            REG_CTRL => Reg::Ctrl,
            REG_PRESENT => Reg::Present,
            REG_VERSION => Reg::Version,
            REG_MODES => Reg::Modes,
            REG_MODE => Reg::Mode,
            REG_MODE_SEL => Reg::ModeSel,
            REG_MODE_WIDTH => Reg::ModeWidth,
            REG_MODE_HEIGHT => Reg::ModeHeight,
            REG_MODE_STRIDE => Reg::ModeStride,
            REG_STATUS => Reg::Status,
            REG_SCANOUT => return Field::Low(Reg::Scanout),
            o if o == REG_SCANOUT + 4 => return Field::High(Reg::Scanout),
            REG_FLIP => Reg::Flip,
            REG_IRQ => Reg::Irq,
            REG_VBLANKS => Reg::Vblanks,
            REG_CURSOR_CTRL => Reg::CursorCtrl,
            REG_CURSOR_ADDR => return Field::Low(Reg::CursorAddr),
            o if o == REG_CURSOR_ADDR + 4 => return Field::High(Reg::CursorAddr),
            REG_CURSOR_POS => Reg::CursorPos,
            REG_CURSOR_HOT => Reg::CursorHot,
            REG_CURSOR_SIZE => Reg::CursorSize,
            _ => return Field::Unused,
        })
    }

    /// The value of register `reg`.
    fn value(&self, reg: Reg) -> u64 {
        let mode = self.mode();
        let sel = MODES.get(self.mode_sel as usize);
        let flip = if self.pending.is_some() {
            STATUS_FLIP_PENDING
        } else {
            0
        };
        let cursor = if self.cursor_armed {
            STATUS_CURSOR_PENDING
        } else {
            0
        };
        u64::from(match reg {
            Reg::Width => mode.width,
            Reg::Height => mode.height,
            Reg::Stride => mode.stride(),
            Reg::Format => FB_FORMAT_XRGB8888,
            Reg::Ctrl => self.ctrl,
            Reg::Version => FB_VERSION,
            Reg::Modes => MODES.len() as u32,
            Reg::Mode => self.mode,
            Reg::ModeSel => self.mode_sel,
            Reg::ModeWidth => sel.map_or(0, |m| m.width),
            Reg::ModeHeight => sel.map_or(0, |m| m.height),
            Reg::ModeStride => sel.map_or(0, |m| m.stride()),
            Reg::Status => self.errors | flip | cursor,
            Reg::Scanout => return self.scanout_reg,
            Reg::Irq => self.irq,
            Reg::Vblanks => self.vblanks as u32,
            Reg::CursorCtrl => self.cursor_ctrl,
            Reg::CursorAddr => return self.cursor_addr,
            Reg::CursorPos => self.cursor_pos,
            Reg::CursorHot => self.cursor_hot,
            Reg::CursorSize => CURSOR_SIZE | CURSOR_SIZE << 16,
            Reg::Present | Reg::Flip => 0, // write-only
        })
    }

    /// Reads a register, at the widths of spec section 9 ([`mmio::access`]): 32-bit
    /// registers take 4-byte accesses, `FB_SCANOUT` and `FB_CURSOR_ADDR` an 8-byte one or
    /// either 4-byte half, and unused offsets read as 0 at 4 and 8 bytes. Those two are
    /// 64-bit on an RV32 machine too (they have no version 1 layout), so the device takes
    /// 8-byte accesses at both widths.
    pub fn read(&mut self, off: u32, size: u32) -> Result<u64, BusFault> {
        let access = mmio::access(off, size, true, Self::field)?;
        Ok(access.read(|reg| self.value(reg)))
    }

    /// Writes a register, at the same widths as [`Framebuffer::read`]. Writes to
    /// read-only registers and unused offsets are ignored. `ram_len` is the size of RAM,
    /// which a scanout buffer must lie in.
    pub fn write(&mut self, off: u32, size: u32, val: u64, ram_len: u64) -> Result<(), BusFault> {
        let access = mmio::access(off, size, true, Self::field)?;
        // For the vertical blank, which checks the cursor's address against RAM.
        self.ram_len = ram_len;
        let Some(reg) = access.reg() else {
            return Ok(());
        };
        let word = val as u32;
        match reg {
            // A half keeps the other: only a modeset or a flip acts on the value.
            Reg::Scanout => self.scanout_reg = access.merge(self.scanout_reg, val),
            Reg::Ctrl => self.ctrl = word & CTRL_ENABLE,
            Reg::Present => {
                self.present = true;
                self.presents += 1;
            }
            Reg::Mode => self.set_mode(word, ram_len),
            Reg::ModeSel => self.mode_sel = word,
            Reg::Status => self.errors &= !(word & STATUS_ERRORS),
            Reg::Flip => self.flip(ram_len),
            Reg::Irq => {
                // Bit 0 takes the value written; writing 1 to bit 1 clears it.
                let pending = self.irq & IRQ_VBLANK & !(word & IRQ_VBLANK);
                self.irq = pending | word & IRQ_VBLANK_ENABLE;
            }
            // The cursor's registers are double-buffered: a write, of any of them, is
            // taken at the next vertical blank, with whatever the others hold then.
            Reg::CursorCtrl => {
                self.cursor_ctrl = word & CURSOR_CTRL_ENABLE;
                self.cursor_armed = true;
            }
            Reg::CursorAddr => {
                self.cursor_addr = access.merge(self.cursor_addr, val);
                self.cursor_armed = true;
            }
            Reg::CursorPos => {
                self.cursor_pos = word;
                self.cursor_armed = true;
            }
            Reg::CursorHot => {
                self.cursor_hot = word & CURSOR_HOT_MASK;
                self.cursor_armed = true;
            }
            Reg::Width
            | Reg::Height
            | Reg::Stride
            | Reg::Format
            | Reg::Version
            | Reg::Modes
            | Reg::ModeWidth
            | Reg::ModeHeight
            | Reg::ModeStride
            | Reg::Vblanks
            | Reg::CursorSize => {} // read-only
        }
        Ok(())
    }

    /// A modeset: mode `index` and the buffer `FB_SCANOUT` names take effect at once, or
    /// neither does (an error bit says why). A flip still pending is dropped: the
    /// modeset already shows `FB_SCANOUT`.
    fn set_mode(&mut self, index: u32, ram_len: u64) {
        let Some(&mode) = MODES.get(index as usize) else {
            self.errors |= STATUS_BAD_MODE;
            return;
        };
        if !Self::fits(self.scanout_reg, mode, ram_len) {
            self.errors |= STATUS_BAD_SCANOUT;
            return;
        }
        self.mode = index;
        self.live = self.scanout_reg;
        self.pending = None;
        self.modesets += 1;
        self.present = true;
    }

    /// A page flip: `FB_SCANOUT` is shown from the next vertical blank, if it holds a
    /// frame of the current mode. A flip queued while another is pending replaces it,
    /// as a later write to a double-buffered register would.
    fn flip(&mut self, ram_len: u64) {
        if Self::fits(self.scanout_reg, self.mode(), ram_len) {
            self.pending = Some(self.scanout_reg);
        } else {
            self.errors |= STATUS_BAD_SCANOUT;
        }
    }

    /// Whether `addr` names a buffer that can show `mode`: 0 (VRAM) for a mode that
    /// fits in VRAM, or a page-aligned address with a whole frame of RAM from it.
    fn fits(addr: u64, mode: Mode, ram_len: u64) -> bool {
        if addr == 0 {
            return mode.frame_size() <= u64::from(VRAM_SIZE);
        }
        let Some(off) = addr.checked_sub(RAM_BASE) else {
            return false;
        };
        addr.is_multiple_of(SCANOUT_ALIGN)
            && off
                .checked_add(mode.frame_size())
                .is_some_and(|end| end <= ram_len)
    }

    /// Ticks until the next vertical blank, at least 1.
    pub fn ticks_to_vblank(&self) -> u64 {
        self.to_vblank
    }

    /// Whether the vertical-blank interrupt is enabled, so that the next vertical blank
    /// may wake a hart waiting for it.
    pub fn vblank_irq_enabled(&self) -> bool {
        self.irq & IRQ_VBLANK_ENABLE != 0
    }

    /// Lets `ticks` ticks of guest time pass. At a vertical blank a pending flip takes
    /// effect and `FB_IRQ`'s pending bit is set. Several frames passing at once (a
    /// machine idle in `wfi` skipping ahead) are one vertical blank for the flip and the
    /// interrupt, as a guest asleep through them could not tell the difference, and
    /// `FB_VBLANKS` counts every one of them.
    pub fn advance(&mut self, ticks: u64) {
        if ticks < self.to_vblank {
            self.to_vblank -= ticks;
            return;
        }
        let past = ticks - self.to_vblank;
        self.vblanks += 1 + past / self.period;
        self.to_vblank = self.period - past % self.period;
        self.irq |= IRQ_VBLANK;
        if let Some(addr) = self.pending.take() {
            self.live = addr;
            self.flips += 1;
            self.present = true;
        }
        if std::mem::take(&mut self.cursor_armed) {
            self.latch_cursor();
        }
    }

    /// The vertical blank takes the cursor registers: the cursor is shown from now on
    /// as they say, if they enable it and `FB_CURSOR_ADDR` names an image (a page with
    /// [`CURSOR_BYTES`] of RAM from it); an enabled cursor with no image is not shown,
    /// and sets BAD_CURSOR.
    fn latch_cursor(&mut self) {
        let enabled = self.cursor_ctrl & CURSOR_CTRL_ENABLE != 0;
        let fits = Self::fits_cursor(self.cursor_addr, self.ram_len);
        if enabled && !fits {
            self.errors |= STATUS_BAD_CURSOR;
        }
        // Signed 16-bit halves, so that an image can hang off the left and top edges.
        let next = CursorState {
            on: enabled && fits,
            addr: self.cursor_addr,
            x: i32::from(self.cursor_pos as u16 as i16),
            y: i32::from((self.cursor_pos >> 16) as u16 as i16),
            hot_x: self.cursor_hot & 0xffff,
            hot_y: self.cursor_hot >> 16,
        };
        // What a hidden cursor holds does not show: hiding or keeping it hidden is
        // one change at most.
        let shown = |c: CursorState| c.on.then_some(c);
        if shown(next) != shown(self.cursor) {
            self.cursor_updates += 1;
            self.cursor_update = true;
        }
        self.cursor = next;
    }

    /// Whether `addr` names a cursor image: page-aligned, with [`CURSOR_BYTES`] of RAM
    /// from it.
    fn fits_cursor(addr: u64, ram_len: u64) -> bool {
        addr.checked_sub(RAM_BASE).is_some_and(|off| {
            addr.is_multiple_of(SCANOUT_ALIGN)
                && off.checked_add(CURSOR_BYTES).is_some_and(|end| end <= ram_len)
        })
    }

    /// Whether a vertical blank changed the cursor shown since the last call: the host
    /// then shows the frame again, with the cursor where it is now.
    pub fn take_cursor_update(&mut self) -> bool {
        std::mem::take(&mut self.cursor_update)
    }

    /// Level of the framebuffer's PLIC line: high while the vertical-blank interrupt is
    /// enabled and pending, so writing 1 to `FB_IRQ` bit 1 lowers it.
    pub fn irq(&self) -> bool {
        self.irq & IRQ_VBLANK_ENABLE != 0 && self.irq & IRQ_VBLANK != 0
    }

    /// What the display shows now: VRAM, or a buffer in `ram` (all of RAM, from
    /// `RAM_BASE`), and the cursor's image in `ram` over it. The image is read as it is
    /// now, as the frame is: a program that changes the cursor's look draws the new one
    /// into another image and points `FB_CURSOR_ADDR` at it.
    pub fn frame<'a>(&'a self, ram: &'a [u8]) -> Frame<'a> {
        let mode = self.mode();
        let len = mode.frame_size() as usize;
        let pixels = self.enabled().then(|| match self.live {
            0 => &self.vram[..len],
            addr => {
                let off = (addr - RAM_BASE) as usize;
                &ram[off..off + len]
            }
        });
        let c = self.cursor;
        let cursor = (pixels.is_some() && c.on).then(|| {
            let off = (c.addr - RAM_BASE) as usize;
            CursorPlane {
                x: c.x,
                y: c.y,
                width: CURSOR_SIZE,
                height: CURSOR_SIZE,
                hot_x: c.hot_x,
                hot_y: c.hot_y,
                pixels: &ram[off..off + CURSOR_BYTES as usize],
            }
        });
        Frame {
            width: mode.width,
            height: mode.height,
            pixels,
            cursor,
        }
    }

    /// Whether `FB_CTRL` enables the display. Only the picture depends on it (black while
    /// disabled): the frame clock, flips, modesets and the interrupt run either way.
    pub fn enabled(&self) -> bool {
        self.ctrl & CTRL_ENABLE != 0
    }

    /// Whether a new frame waits for the host to show it: `FB_PRESENT` was written, a
    /// flip took effect or a modeset happened.
    pub fn present_pending(&self) -> bool {
        self.present
    }

    /// Returns whether there was a present ([`Framebuffer::present_pending`]) since the
    /// last call.
    pub fn take_present(&mut self) -> bool {
        std::mem::take(&mut self.present)
    }

    /// How many new frames the program has shown: `FB_PRESENT` writes, flips that took
    /// effect and modesets. [`crate::Machine::run_for`] stops when it changes.
    pub fn shown(&self) -> u64 {
        self.presents + self.flips + self.modesets
    }

    /// How many times what the display shows changed by the program's doing: the new
    /// frames of [`Framebuffer::shown`], and the vertical blanks that moved, showed or
    /// hid the cursor, which change the picture without a new frame.
    /// [`crate::Machine::run_for`] stops when it changes.
    pub fn changes(&self) -> u64 {
        self.shown() + self.cursor_updates
    }

    /// Pixel at (x, y) of the built-in VRAM, as 0x00RRGGBB.
    pub fn pixel(&self, x: u32, y: u32) -> u32 {
        let off = (y * FB_STRIDE + x * 4) as usize;
        u32::from_le_bytes([
            self.vram[off],
            self.vram[off + 1],
            self.vram[off + 2],
            self.vram[off + 3],
        ]) & 0x00ff_ffff
    }
}

/// Writes `frame-0000.ppm`, `frame-0001.ppm`, ... into a directory.
pub struct FrameDumper {
    dir: PathBuf,
    next: u32,
}

impl FrameDumper {
    /// Creates the directory if it does not exist yet.
    pub fn new(dir: impl AsRef<Path>) -> io::Result<Self> {
        let dir = dir.as_ref().to_path_buf();
        std::fs::create_dir_all(&dir)?;
        Ok(FrameDumper { dir, next: 0 })
    }

    pub fn dump(&mut self, frame: &Frame) -> io::Result<PathBuf> {
        let path = self.dir.join(format!("frame-{:04}.ppm", self.next));
        std::fs::write(&path, frame.to_ppm())?;
        self.next += 1;
        Ok(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RAM of the tests: 8 MiB.
    const RAM: u64 = 8 << 20;

    fn r(fb: &mut Framebuffer, off: u32) -> u64 {
        fb.read(off, 4).unwrap()
    }

    fn w(fb: &mut Framebuffer, off: u32, val: u64) {
        fb.write(off, 4, val, RAM).unwrap();
    }

    #[test]
    fn info_registers() {
        let mut fb = Framebuffer::default();
        assert_eq!(fb.read(REG_WIDTH, 4), Ok(640));
        assert_eq!(fb.read(REG_HEIGHT, 4), Ok(480));
        assert_eq!(fb.read(REG_STRIDE, 4), Ok(2560));
        assert_eq!(fb.read(REG_FORMAT, 4), Ok(0));
        assert_eq!(fb.read(REG_VERSION, 4), Ok(2));
        assert_eq!(fb.read(REG_MODES, 4), Ok(6));
        assert_eq!(fb.read(REG_MODE, 4), Ok(0));
        assert_eq!(fb.read(REG_CURSOR_SIZE, 4), Ok(64 | 64 << 16));
        assert_eq!(fb.read(0x64, 4), Ok(0));
        assert_eq!(fb.read(0xffc, 4), Ok(0));
        assert_eq!(VRAM_SIZE, 0x12_c000);
        assert_eq!(MAX_FRAME_SIZE, 1920 * 1080 * 4);
    }

    #[test]
    fn read_only_registers_ignore_writes() {
        let mut fb = Framebuffer::default();
        let ro = [
            REG_WIDTH,
            REG_HEIGHT,
            REG_STRIDE,
            REG_FORMAT,
            REG_VERSION,
            REG_MODES,
            REG_MODE_WIDTH,
            REG_MODE_HEIGHT,
            REG_MODE_STRIDE,
            REG_VBLANKS,
            REG_CURSOR_SIZE,
        ];
        let before: Vec<u64> = ro.iter().map(|&reg| r(&mut fb, reg)).collect();
        for reg in ro {
            w(&mut fb, reg, 0xdead_beef);
        }
        let after: Vec<u64> = ro.iter().map(|&reg| r(&mut fb, reg)).collect();
        assert_eq!(before, after);
        assert_eq!(fb.mode(), MODES[0]);
    }

    #[test]
    fn ctrl_keeps_only_enable_bit() {
        let mut fb = Framebuffer::default();
        assert!(!fb.enabled());
        w(&mut fb, REG_CTRL, 0xffff_ffff);
        assert_eq!(fb.read(REG_CTRL, 4), Ok(1));
        assert!(fb.enabled());
        w(&mut fb, REG_CTRL, 0xffff_fffe);
        assert_eq!(fb.read(REG_CTRL, 4), Ok(0));
        assert!(!fb.enabled());
    }

    #[test]
    fn present_is_write_only_and_latched() {
        let mut fb = Framebuffer::default();
        assert!(!fb.take_present());
        w(&mut fb, REG_PRESENT, 0);
        assert!(fb.present_pending());
        w(&mut fb, REG_PRESENT, 123);
        assert_eq!(fb.read(REG_PRESENT, 4), Ok(0));
        assert_eq!(fb.presents, 2);
        assert!(fb.take_present());
        assert!(!fb.present_pending());
        assert!(!fb.take_present());
    }

    #[test]
    fn registers_are_word_only_but_the_scanout() {
        let mut fb = Framebuffer::default();
        assert_eq!(fb.read(REG_WIDTH, 1), Err(BusFault));
        assert_eq!(fb.read(REG_WIDTH, 2), Err(BusFault));
        assert_eq!(fb.read(REG_WIDTH, 8), Err(BusFault), "two 32-bit registers");
        assert_eq!(fb.write(REG_CTRL, 1, 1, RAM), Err(BusFault));
        assert_eq!(fb.write(REG_PRESENT, 2, 1, RAM), Err(BusFault));
        assert_eq!(fb.write(REG_MODE, 8, 1, RAM), Err(BusFault));
        assert_eq!(
            fb.read(REG_VBLANKS, 8),
            Err(BusFault),
            "a 32-bit register and unused space"
        );
        assert_eq!(fb.read(REG_SCANOUT, 2), Err(BusFault));
        assert_eq!(
            fb.read(REG_SCANOUT + 4, 8),
            Err(BusFault),
            "the high half is 4 bytes"
        );
        assert_eq!(fb.write(REG_SCANOUT, 1, 1, RAM), Err(BusFault));
        assert!(!fb.enabled());
        assert!(!fb.take_present());
        assert_eq!(fb.read(REG_SCANOUT, 8), Ok(0));
    }

    /// Spec section 9: unused offsets read as 0 and ignore writes at every width the
    /// device takes, 8 bytes included, since `FB_SCANOUT` takes them; the bytes inside
    /// `FB_SCANOUT` that start no half (0x39 to 0x3b) are unused too. Other widths
    /// fault there as anywhere.
    #[test]
    fn unused_offsets_read_as_zero_at_four_and_eight_bytes() {
        let mut fb = Framebuffer::default();
        fb.write(REG_SCANOUT, 8, 0x1234_5678_9abc_d000, RAM).unwrap();
        for off in [0x68, 0x100, 0xff8] {
            assert_eq!(fb.read(off, 8), Ok(0), "{off:#x}");
            assert_eq!(fb.read(off, 4), Ok(0), "{off:#x}");
            assert_eq!(fb.write(off, 8, !0, RAM), Ok(()), "{off:#x}");
            assert_eq!(fb.write(off, 4, !0, RAM), Ok(()), "{off:#x}");
            assert_eq!(fb.read(off, 1), Err(BusFault), "{off:#x}");
            assert_eq!(fb.write(off, 2, 0, RAM), Err(BusFault), "{off:#x}");
        }
        assert_eq!(fb.read(0xffc, 4), Ok(0));
        for off in [REG_SCANOUT + 1, REG_SCANOUT + 2, REG_SCANOUT + 3] {
            assert_eq!(fb.read(off, 4), Ok(0), "{off:#x}");
            assert_eq!(fb.write(off, 4, 0, RAM), Ok(()), "{off:#x}");
        }
        assert_eq!(fb.read(REG_SCANOUT + 2, 8), Err(BusFault), "misaligned");
        // Nothing changed: not the scanout, not the display, no present.
        assert_eq!(fb.read(REG_SCANOUT, 8), Ok(0x1234_5678_9abc_d000));
        assert_eq!((fb.ctrl, fb.mode(), fb.presents), (0, MODES[0], 0));
        assert!(!fb.present_pending());
    }

    #[test]
    fn scanout_is_64_bit_whole_or_by_halves() {
        let mut fb = Framebuffer::default();
        fb.write(REG_SCANOUT, 8, 0x1122_3344_5566_7788, RAM).unwrap();
        assert_eq!(fb.read(REG_SCANOUT, 8), Ok(0x1122_3344_5566_7788));
        assert_eq!(fb.read(REG_SCANOUT, 4), Ok(0x5566_7788));
        assert_eq!(fb.read(REG_SCANOUT + 4, 4), Ok(0x1122_3344));
        fb.write(REG_SCANOUT, 4, 0x8000_0000, RAM).unwrap();
        assert_eq!(fb.read(REG_SCANOUT, 8), Ok(0x1122_3344_8000_0000));
        fb.write(REG_SCANOUT + 4, 4, 0, RAM).unwrap();
        assert_eq!(fb.read(REG_SCANOUT, 8), Ok(0x8000_0000));
        // Writing the register shows nothing yet: only a modeset or a flip does.
        assert_eq!(fb.scanout(), 0);
        assert!(!fb.present_pending());
    }

    #[test]
    fn modes_are_listed_through_the_selector() {
        let mut fb = Framebuffer::default();
        let mut listed = Vec::new();
        for i in 0..r(&mut fb, REG_MODES) {
            w(&mut fb, REG_MODE_SEL, i);
            assert_eq!(r(&mut fb, REG_MODE_SEL), i);
            let (width, height) = (r(&mut fb, REG_MODE_WIDTH), r(&mut fb, REG_MODE_HEIGHT));
            assert_eq!(r(&mut fb, REG_MODE_STRIDE), width * 4);
            listed.push((width, height));
        }
        assert_eq!(
            listed,
            [
                (640, 480),
                (800, 600),
                (1024, 768),
                (1280, 720),
                (1280, 1024),
                (1920, 1080)
            ]
        );
        // A selector past the last mode reads zeros.
        w(&mut fb, REG_MODE_SEL, 6);
        assert_eq!(r(&mut fb, REG_MODE_WIDTH), 0);
        assert_eq!(r(&mut fb, REG_MODE_HEIGHT), 0);
        assert_eq!(r(&mut fb, REG_MODE_STRIDE), 0);
        assert_eq!(mode_index(1024, 768), Some(2));
        assert_eq!(mode_index(1024, 769), None);
    }

    #[test]
    fn a_modeset_takes_the_mode_and_the_scanout_together() {
        let mut fb = Framebuffer::default();
        // 800x600 does not fit in VRAM: refused, nothing changes.
        w(&mut fb, REG_MODE, 1);
        assert_eq!(r(&mut fb, REG_STATUS), u64::from(STATUS_BAD_SCANOUT));
        assert_eq!((fb.mode(), fb.modesets), (MODES[0], 0));
        w(&mut fb, REG_STATUS, u64::from(STATUS_BAD_SCANOUT));
        assert_eq!(r(&mut fb, REG_STATUS), 0, "write 1 to clear");

        // With a buffer in RAM it is taken at once, and the host is asked to redraw.
        fb.write(REG_SCANOUT, 8, RAM_BASE + 0x10_0000, RAM).unwrap();
        w(&mut fb, REG_MODE, 1);
        assert_eq!(r(&mut fb, REG_STATUS), 0);
        assert_eq!(fb.mode(), MODES[1]);
        assert_eq!(fb.scanout(), RAM_BASE + 0x10_0000);
        assert_eq!(
            (
                r(&mut fb, REG_MODE),
                r(&mut fb, REG_WIDTH),
                r(&mut fb, REG_HEIGHT)
            ),
            (1, 800, 600)
        );
        assert_eq!(r(&mut fb, REG_STRIDE), 3200);
        assert!(fb.take_present());
        assert_eq!(fb.modesets, 1);

        // A mode that does not exist.
        w(&mut fb, REG_MODE, 6);
        assert_eq!(r(&mut fb, REG_STATUS), u64::from(STATUS_BAD_MODE));
        assert_eq!(fb.mode(), MODES[1]);
        // Back to 640x480 in VRAM.
        fb.write(REG_SCANOUT, 8, 0, RAM).unwrap();
        w(&mut fb, REG_MODE, 0);
        assert_eq!((fb.mode(), fb.scanout()), (MODES[0], 0));
        assert_eq!(
            r(&mut fb, REG_STATUS),
            u64::from(STATUS_BAD_MODE),
            "errors stay until cleared"
        );
    }

    #[test]
    fn a_bad_scanout_is_refused() {
        let mut fb = Framebuffer::default();
        let big = MODES[5];
        let last_page = RAM_BASE + RAM - SCANOUT_ALIGN;
        for (addr, mode, why) in [
            (RAM_BASE + 0x800, 0, "not page-aligned"),
            (0x3100_0000, 0, "VRAM is 0, not its address"),
            (0x1000, 0, "the boot ROM"),
            (RAM_BASE - SCANOUT_ALIGN, 0, "below RAM"),
            (last_page, 0, "runs past the end of RAM"),
            (
                RAM_BASE + RAM - big.frame_size() + SCANOUT_ALIGN,
                5,
                "a page short",
            ),
            (!0xfff, 0, "overflows"),
            (0, 5, "VRAM is too small for 1920x1080"),
        ] {
            fb.write(REG_SCANOUT, 8, addr, RAM).unwrap();
            w(&mut fb, REG_MODE, mode);
            assert_eq!(
                r(&mut fb, REG_STATUS),
                u64::from(STATUS_BAD_SCANOUT),
                "modeset: {why}"
            );
            w(&mut fb, REG_STATUS, 0xffff_ffff);
            if mode == 0 {
                w(&mut fb, REG_FLIP, 1);
                assert_eq!(
                    r(&mut fb, REG_STATUS),
                    u64::from(STATUS_BAD_SCANOUT),
                    "flip: {why}"
                );
                w(&mut fb, REG_STATUS, 0xffff_ffff);
            }
            assert_eq!((fb.mode(), fb.scanout()), (MODES[0], 0), "{why}");
        }
        // A frame that ends exactly at the end of RAM is fine.
        fb.write(REG_SCANOUT, 8, RAM_BASE + RAM - MODES[0].frame_size(), RAM)
            .unwrap();
        w(&mut fb, REG_FLIP, 1);
        assert_eq!(r(&mut fb, REG_STATUS), u64::from(STATUS_FLIP_PENDING));
    }

    #[test]
    fn a_flip_is_latched_at_the_vertical_blank() {
        let mut fb = Framebuffer::new(6000); // 100 ticks a frame
        assert_eq!(fb.frame_ticks(), 100);
        let buf = RAM_BASE + 0x20_0000;
        fb.write(REG_SCANOUT, 8, buf, RAM).unwrap();
        w(&mut fb, REG_FLIP, 1);
        assert_eq!(r(&mut fb, REG_STATUS), u64::from(STATUS_FLIP_PENDING));
        assert_eq!(fb.scanout(), 0, "not yet");
        // Changing FB_SCANOUT after the flip does not change what it latches.
        fb.write(REG_SCANOUT, 8, 0, RAM).unwrap();
        fb.advance(99);
        assert_eq!((fb.scanout(), fb.vblanks), (0, 0));
        assert!(!fb.present_pending());
        fb.advance(1);
        assert_eq!((fb.scanout(), fb.vblanks, fb.flips), (buf, 1, 1));
        assert_eq!(r(&mut fb, REG_STATUS), 0);
        assert_eq!(r(&mut fb, REG_VBLANKS), 1);
        assert!(fb.take_present(), "the host shows the new frame");
        // The next vertical blank, a frame later, has nothing to flip.
        fb.advance(100);
        assert_eq!((fb.scanout(), fb.vblanks, fb.flips), (buf, 2, 1));
        assert!(!fb.present_pending());
    }

    #[test]
    fn the_latest_flip_wins_and_a_modeset_drops_a_pending_one() {
        let mut fb = Framebuffer::new(6000);
        let (a, b) = (RAM_BASE + 0x20_0000, RAM_BASE + 0x40_0000);
        fb.write(REG_SCANOUT, 8, a, RAM).unwrap();
        w(&mut fb, REG_FLIP, 1);
        fb.write(REG_SCANOUT, 8, b, RAM).unwrap();
        w(&mut fb, REG_FLIP, 1);
        // A refused flip leaves the pending one alone.
        fb.write(REG_SCANOUT, 8, 1, RAM).unwrap();
        w(&mut fb, REG_FLIP, 1);
        assert_eq!(
            r(&mut fb, REG_STATUS),
            u64::from(STATUS_FLIP_PENDING | STATUS_BAD_SCANOUT)
        );
        fb.advance(100);
        assert_eq!((fb.scanout(), fb.flips), (b, 1));

        fb.write(REG_SCANOUT, 8, a, RAM).unwrap();
        w(&mut fb, REG_FLIP, 1);
        fb.write(REG_SCANOUT, 8, b, RAM).unwrap();
        w(&mut fb, REG_MODE, 1);
        assert_eq!((fb.mode(), fb.scanout()), (MODES[1], b));
        assert_eq!(r(&mut fb, REG_STATUS) & u64::from(STATUS_FLIP_PENDING), 0);
        fb.advance(100);
        assert_eq!((fb.scanout(), fb.flips), (b, 1), "the dropped flip never happens");
    }

    #[test]
    fn the_vertical_blank_interrupts_while_enabled_until_cleared() {
        let mut fb = Framebuffer::new(6000);
        fb.advance(100);
        assert_eq!(
            r(&mut fb, REG_IRQ),
            u64::from(IRQ_VBLANK),
            "pending, even while disabled"
        );
        assert!(!fb.irq());
        w(&mut fb, REG_IRQ, u64::from(IRQ_VBLANK_ENABLE));
        assert!(fb.irq(), "enabling with one pending raises the line");
        assert_eq!(r(&mut fb, REG_IRQ), u64::from(IRQ_VBLANK_ENABLE | IRQ_VBLANK));
        w(&mut fb, REG_IRQ, u64::from(IRQ_VBLANK_ENABLE | IRQ_VBLANK));
        assert!(!fb.irq(), "write 1 to clear");
        assert_eq!(r(&mut fb, REG_IRQ), u64::from(IRQ_VBLANK_ENABLE));
        fb.advance(50);
        assert!(!fb.irq());
        fb.advance(50);
        assert!(fb.irq());
        // Reserved bits read 0; disabling keeps the pending bit.
        w(&mut fb, REG_IRQ, 0xffff_fffc);
        assert_eq!(r(&mut fb, REG_IRQ), u64::from(IRQ_VBLANK));
        assert!(!fb.irq());
    }

    #[test]
    fn skipped_frames_count_and_the_phase_is_kept() {
        let mut fb = Framebuffer::new(6000);
        fb.advance(30);
        assert_eq!(fb.ticks_to_vblank(), 70);
        // 70 to the first vertical blank, then 3 more frames and 20 ticks.
        fb.advance(70 + 300 + 20);
        assert_eq!(fb.vblanks, 4);
        assert_eq!(fb.ticks_to_vblank(), 80);
        fb.advance(80);
        assert_eq!(fb.vblanks, 5);
        assert_eq!(fb.ticks_to_vblank(), 100);
        // A clock slower than 60 Hz still has a vertical blank every tick.
        let mut slow = Framebuffer::new(5);
        assert_eq!(slow.frame_ticks(), 1);
        slow.advance(1);
        assert_eq!(slow.vblanks, 1);
    }

    #[test]
    fn frame_comes_from_vram_or_ram() {
        let mut ram = vec![0u8; RAM as usize];
        let mut fb = Framebuffer::default();
        fb.vram[0..4].copy_from_slice(&0x0012_3456u32.to_le_bytes());
        let f = fb.frame(&ram);
        assert_eq!((f.width, f.height, f.pixels), (640, 480, None), "disabled");
        assert_eq!(f.pixel(0, 0), 0);
        fb.ctrl = CTRL_ENABLE;
        assert_eq!(fb.frame(&ram).pixel(0, 0), 0x12_3456);
        assert_eq!(fb.frame(&ram).pixels.unwrap().len(), VRAM_SIZE as usize);

        let off = 0x10_0000;
        let last = off + (800 * 600 - 1) * 4;
        ram[last..last + 4].copy_from_slice(&0x00ab_cdefu32.to_le_bytes());
        fb.write(REG_SCANOUT, 8, RAM_BASE + off as u64, RAM).unwrap();
        fb.write(REG_MODE, 4, 1, RAM).unwrap();
        let f = fb.frame(&ram);
        assert_eq!((f.width, f.height), (800, 600));
        assert_eq!(f.pixels.unwrap().len(), 800 * 600 * 4);
        assert_eq!(f.pixel(799, 599), 0xab_cdef);
        assert_eq!(f.pixel(0, 0), 0);
    }

    #[test]
    fn ppm_converts_xrgb_to_rgb() {
        let mut fb = Framebuffer::default();
        fb.vram[0..4].copy_from_slice(&0x0012_3456u32.to_le_bytes());
        let last = (VRAM_SIZE - 4) as usize;
        fb.vram[last..].copy_from_slice(&0xff_abcdefu32.to_le_bytes());
        assert_eq!(fb.pixel(0, 0), 0x12_3456);
        assert_eq!(fb.pixel(639, 479), 0xab_cdef); // X byte ignored

        fb.ctrl = CTRL_ENABLE;
        let ppm = fb.frame(&[]).to_ppm();
        let header = b"P6\n640 480\n255\n";
        assert_eq!(&ppm[..header.len()], header);
        assert_eq!(ppm.len(), header.len() + 640 * 480 * 3);
        assert_eq!(&ppm[header.len()..header.len() + 3], &[0x12, 0x34, 0x56]);
        assert_eq!(&ppm[ppm.len() - 3..], &[0xab, 0xcd, 0xef]);
    }

    #[test]
    fn disabled_display_is_black() {
        let mut fb = Framebuffer::default();
        fb.vram.fill(0xff);
        let ppm = fb.frame(&[]).to_ppm();
        let header = b"P6\n640 480\n255\n";
        assert_eq!(ppm.len(), header.len() + 640 * 480 * 3);
        assert!(ppm[header.len()..].iter().all(|&b| b == 0));
    }

    /// Disabling the display blanks the picture only, as VGA's screen-off bit does: the
    /// modeset, the flip at the vertical blank, `FB_VBLANKS` and the interrupt go on.
    #[test]
    fn a_disabled_display_keeps_its_timing() {
        let mut fb = Framebuffer::new(6000); // 100 ticks a frame
        w(&mut fb, REG_IRQ, u64::from(IRQ_VBLANK_ENABLE));
        let (a, b) = (RAM_BASE + 0x10_0000, RAM_BASE + 0x40_0000);
        fb.write(REG_SCANOUT, 8, a, RAM).unwrap();
        w(&mut fb, REG_MODE, 1);
        assert_eq!((fb.mode(), fb.scanout(), fb.modesets), (MODES[1], a, 1));
        fb.write(REG_SCANOUT, 8, b, RAM).unwrap();
        w(&mut fb, REG_FLIP, 1);
        fb.advance(100);
        assert_eq!((fb.scanout(), fb.flips, fb.vblanks), (b, 1, 1));
        assert_eq!(r(&mut fb, REG_STATUS), 0, "the flip took effect");
        assert!(fb.irq(), "the vertical blank interrupts");
        let frame = fb.frame(&[]);
        assert_eq!((frame.width, frame.height, frame.pixels), (800, 600, None));
    }

    /// `FB_CURSOR_POS` for an image whose top left pixel is at (`x`, `y`).
    fn pos(x: i16, y: i16) -> u64 {
        u64::from(x as u16) | u64::from(y as u16) << 16
    }

    #[test]
    fn cursor_registers_hold_what_is_written() {
        let mut fb = Framebuffer::default();
        assert_eq!(r(&mut fb, REG_CURSOR_SIZE), 64 | 64 << 16);
        for reg in [REG_CURSOR_CTRL, REG_CURSOR_POS, REG_CURSOR_HOT] {
            assert_eq!(r(&mut fb, reg), 0, "{reg:#x} at reset");
        }
        assert_eq!(fb.read(REG_CURSOR_ADDR, 8), Ok(0));
        assert_eq!(fb.cursor(), CursorState::default());
        // Reserved bits read as 0; the position is a whole word, the hotspot 6 bits a half.
        w(&mut fb, REG_CURSOR_CTRL, 0xffff_ffff);
        assert_eq!(r(&mut fb, REG_CURSOR_CTRL), 1);
        w(&mut fb, REG_CURSOR_POS, 0xfff6_8001);
        assert_eq!(r(&mut fb, REG_CURSOR_POS), 0xfff6_8001);
        w(&mut fb, REG_CURSOR_HOT, 0xffff_ffff);
        assert_eq!(r(&mut fb, REG_CURSOR_HOT), 0x003f_003f);
        // FB_CURSOR_ADDR is 64-bit: whole, or by its halves, which keep each other.
        fb.write(REG_CURSOR_ADDR, 8, 0x1122_3344_5566_7000, RAM).unwrap();
        assert_eq!(fb.read(REG_CURSOR_ADDR, 4), Ok(0x5566_7000));
        assert_eq!(fb.read(REG_CURSOR_ADDR + 4, 4), Ok(0x1122_3344));
        fb.write(REG_CURSOR_ADDR + 4, 4, 0, RAM).unwrap();
        assert_eq!(fb.read(REG_CURSOR_ADDR, 8), Ok(0x5566_7000));
        // The other registers are 32-bit, and 2-byte accesses fault as anywhere.
        assert_eq!(fb.read(REG_CURSOR_POS, 8), Err(BusFault));
        assert_eq!(
            fb.write(REG_CURSOR_CTRL, 8, 1, RAM),
            Err(BusFault),
            "a register and the next"
        );
        assert_eq!(
            fb.read(REG_CURSOR_ADDR + 4, 8),
            Err(BusFault),
            "the high half is 4 bytes"
        );
        assert_eq!(fb.write(REG_CURSOR_HOT, 2, 0, RAM), Err(BusFault));
        // Nothing is shown until a vertical blank takes it.
        assert_eq!(r(&mut fb, REG_STATUS), u64::from(STATUS_CURSOR_PENDING));
        assert_eq!(fb.cursor(), CursorState::default());
        assert!(!fb.present_pending());
    }

    /// The cursor's registers are double-buffered: written any time, all taken together
    /// at the next vertical blank, which shows the cursor as they say from then on.
    /// Moving the cursor is no new frame: no present, nothing for `shown`.
    #[test]
    fn the_cursor_is_latched_at_the_vertical_blank() {
        let mut fb = Framebuffer::new(6000); // 100 ticks a frame
        let image = RAM_BASE + 0x1000;
        fb.write(REG_CURSOR_ADDR, 8, image, RAM).unwrap();
        w(&mut fb, REG_CURSOR_POS, pos(-10, 20));
        w(&mut fb, REG_CURSOR_HOT, 3 | 5 << 16);
        w(&mut fb, REG_CURSOR_CTRL, 1);
        fb.advance(99);
        assert_eq!(fb.cursor(), CursorState::default(), "not yet");
        assert!(!fb.take_cursor_update());
        fb.advance(1);
        let shown = CursorState {
            on: true,
            addr: image,
            x: -10,
            y: 20,
            hot_x: 3,
            hot_y: 5,
        };
        assert_eq!(fb.cursor(), shown);
        assert_eq!(r(&mut fb, REG_STATUS), 0, "taken");
        assert!(fb.take_cursor_update(), "the host shows it");
        assert!(!fb.take_cursor_update());
        assert_eq!((fb.cursor_updates, fb.shown(), fb.changes()), (1, 0, 1));
        assert!(!fb.present_pending(), "no new frame");

        // A move written in two steps within a frame is one change, at the blank.
        w(&mut fb, REG_CURSOR_POS, pos(300, 20));
        w(&mut fb, REG_CURSOR_POS, pos(300, 200));
        fb.advance(50);
        assert_eq!((fb.cursor().x, fb.cursor().y), (-10, 20));
        fb.advance(50);
        assert_eq!((fb.cursor().x, fb.cursor().y), (300, 200));
        assert_eq!(fb.cursor_updates, 2);
        // A write of what is shown already changes nothing, nor does a blank with none.
        w(&mut fb, REG_CURSOR_POS, pos(300, 200));
        fb.advance(100);
        fb.advance(100);
        assert_eq!(fb.cursor_updates, 2);
        assert!(fb.take_cursor_update());
        assert!(!fb.take_cursor_update());

        // Hiding it is a change; moving it while hidden is not, until it shows again.
        w(&mut fb, REG_CURSOR_CTRL, 0);
        fb.advance(100);
        assert!(!fb.cursor().on);
        assert_eq!(fb.cursor_updates, 3);
        w(&mut fb, REG_CURSOR_POS, pos(1, 2));
        fb.advance(100);
        assert_eq!(fb.cursor_updates, 3);
        w(&mut fb, REG_CURSOR_CTRL, 1);
        fb.advance(100);
        assert_eq!((fb.cursor().on, fb.cursor().x, fb.cursor().y), (true, 1, 2));
        assert_eq!(fb.cursor_updates, 4);
        // Frames skipped at once (an idle machine) are one blank for the cursor too.
        w(&mut fb, REG_CURSOR_POS, pos(7, 7));
        fb.advance(1000);
        assert_eq!((fb.cursor().x, fb.cursor_updates), (7, 5));
        // A modeset keeps the cursor where it is, as a CRTC keeps its cursor plane.
        fb.write(REG_SCANOUT, 8, RAM_BASE + 0x10_0000, RAM).unwrap();
        w(&mut fb, REG_MODE, 1);
        assert_eq!(fb.cursor().x, 7);
        assert_eq!(fb.flips, 0);
    }

    #[test]
    fn a_cursor_with_no_image_is_refused_at_the_vertical_blank() {
        let mut fb = Framebuffer::new(6000);
        for (addr, why) in [
            (0, "VRAM holds no cursor"),
            (RAM_BASE + 0x800, "not page-aligned"),
            (RAM_BASE - SCANOUT_ALIGN, "below RAM"),
            (RAM_BASE + RAM - SCANOUT_ALIGN, "runs past the end of RAM"),
            (!0xfff, "overflows"),
        ] {
            fb.write(REG_CURSOR_ADDR, 8, addr, RAM).unwrap();
            w(&mut fb, REG_CURSOR_CTRL, 1);
            assert_eq!(r(&mut fb, REG_STATUS), u64::from(STATUS_CURSOR_PENDING), "{why}");
            fb.advance(100);
            assert_eq!(r(&mut fb, REG_STATUS), u64::from(STATUS_BAD_CURSOR), "{why}");
            assert!(!fb.cursor().on, "{why}");
            w(&mut fb, REG_STATUS, u64::from(STATUS_BAD_CURSOR));
            assert_eq!(r(&mut fb, REG_STATUS), 0, "write 1 to clear");
        }
        assert_eq!(fb.cursor_updates, 0, "never shown");
        // A disabled cursor needs no image; one that ends at the end of RAM is fine.
        fb.write(REG_CURSOR_ADDR, 8, 0, RAM).unwrap();
        w(&mut fb, REG_CURSOR_CTRL, 0);
        fb.advance(100);
        assert_eq!(r(&mut fb, REG_STATUS), 0);
        fb.write(REG_CURSOR_ADDR, 8, RAM_BASE + RAM - CURSOR_BYTES, RAM)
            .unwrap();
        w(&mut fb, REG_CURSOR_CTRL, 1);
        fb.advance(100);
        assert_eq!(r(&mut fb, REG_STATUS), 0);
        assert!(fb.cursor().on);
    }

    #[test]
    fn mul_un8_rounds_to_the_nearest() {
        for a in 0..=255u8 {
            for b in 0..=255u8 {
                let exact = f64::from(a) * f64::from(b) / 255.0;
                assert_eq!(mul_un8(a, b), exact.round() as u8, "{a} x {b}");
            }
        }
    }

    /// The display shows the cursor blended over the frame with OVER on premultiplied
    /// alpha: an opaque pixel replaces the frame's, a clear one keeps it, one half
    /// covered adds its colour to half the frame's; the image hangs off the edges.
    #[test]
    fn the_frame_shows_the_cursor_blended_over_it() {
        let mut ram = vec![0u8; RAM as usize];
        let mut fb = Framebuffer::new(6000);
        fb.vram.fill(0x80); // every pixel 0x80 in each byte: grey
        fb.ctrl = CTRL_ENABLE;
        // The image at RAM + 0x2000: pixel (0, 0) opaque red, (1, 0) half-covered green
        // (premultiplied: 0x80 of alpha, 0x80 of green), (2, 0) clear, (63, 63) white.
        let image = 0x2000;
        let px = |x: usize, y: usize| image + (y * 64 + x) * 4;
        ram[px(0, 0)..px(0, 0) + 4].copy_from_slice(&0xffff_0000u32.to_le_bytes());
        ram[px(1, 0)..px(1, 0) + 4].copy_from_slice(&0x8000_8000u32.to_le_bytes());
        ram[px(63, 63)..px(63, 63) + 4].copy_from_slice(&0xffff_ffffu32.to_le_bytes());
        fb.write(REG_CURSOR_ADDR, 8, RAM_BASE + image as u64, RAM)
            .unwrap();
        w(&mut fb, REG_CURSOR_POS, pos(10, 20));
        w(&mut fb, REG_CURSOR_CTRL, 1);
        assert_eq!(fb.frame(&ram).cursor, None, "until the vertical blank");
        fb.advance(100);
        let frame = fb.frame(&ram);
        let cursor = frame.cursor.unwrap();
        assert_eq!(
            (cursor.x, cursor.y, cursor.width, cursor.height),
            (10, 20, 64, 64)
        );
        assert_eq!(cursor.pixels.len(), CURSOR_BYTES as usize);
        assert_eq!(frame.pixel(10, 20), 0xff_0000, "opaque");
        // Green: 0x80 + 0x80 * 0x7f / 255 = 0x80 + 0x40; red and blue 0x80 * 0x7f / 255.
        assert_eq!(frame.pixel(11, 20), 0x40_c040, "half covered");
        assert_eq!(frame.pixel(12, 20), 0x80_8080, "clear");
        assert_eq!(frame.pixel(73, 83), 0xff_ffff);
        assert_eq!(frame.pixel(9, 20), 0x80_8080, "outside");
        assert_eq!(frame.pixel(74, 84), 0x80_8080, "outside");
        // The composited frame is the same pixels; the scanout plane is untouched.
        let all = frame.composited().unwrap();
        for (x, y) in [(10, 20), (11, 20), (12, 20), (73, 83), (0, 0)] {
            let at = (y * 640 + x) * 4;
            let word = u32::from_le_bytes(all[at..at + 4].try_into().unwrap()) & 0xff_ffff;
            assert_eq!(word, frame.pixel(x as u32, y as u32), "({x}, {y})");
        }
        assert!(fb.vram.iter().all(|&b| b == 0x80));
        let ppm = frame.to_ppm();
        let at = b"P6\n640 480\n255\n".len() + (20 * 640 + 10) * 3;
        assert_eq!(&ppm[at..at + 6], &[0xff, 0, 0, 0x40, 0xc0, 0x40]);

        // Off the top left corner only (63, 63) is left, at (0, 0); off the bottom
        // right only (0, 0), at (639, 479); wholly off, nothing.
        for (x, y, at, want) in [
            (-63, -63, (0, 0), 0xff_ffff),
            (639, 479, (639, 479), 0xff_0000),
            (-64, 0, (0, 0), 0x80_8080),
            (640, 0, (639, 0), 0x80_8080),
            (i16::MIN, i16::MAX, (0, 479), 0x80_8080),
        ] {
            w(&mut fb, REG_CURSOR_POS, pos(x, y));
            fb.advance(100);
            let frame = fb.frame(&ram);
            assert_eq!(frame.pixel(at.0, at.1), want, "at ({x}, {y})");
            assert!(frame.composited().unwrap() == frame.composited().unwrap());
            frame.to_ppm(); // clipped, not out of bounds
        }
        // A display that is off shows nothing, the cursor included.
        fb.ctrl = 0;
        let frame = fb.frame(&ram);
        assert_eq!((frame.pixels, frame.cursor), (None, None));
    }

    /// Saturation: a pixel that is not validly premultiplied (colour above alpha) adds
    /// up to 255 at most, as pixman's saturating add does.
    #[test]
    fn blending_saturates() {
        let mut dst = [0xf0, 0x10, 0x00, 0x00];
        blend_pixel(&mut dst, &[0x80, 0x80, 0x80, 0x00]);
        assert_eq!(dst, [0xff, 0x90, 0x80, 0x00]);
    }

    #[test]
    fn dumper_numbers_frames() {
        let dir = std::env::temp_dir().join(format!("grail-fb-unit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut d = FrameDumper::new(dir.join("nested")).unwrap();
        let fb = Framebuffer::default();
        let a = d.dump(&fb.frame(&[])).unwrap();
        let b = d.dump(&fb.frame(&[])).unwrap();
        assert_eq!(a.file_name().unwrap(), "frame-0000.ppm");
        assert_eq!(b.file_name().unwrap(), "frame-0001.ppm");
        assert_eq!(std::fs::read(&b).unwrap(), fb.frame(&[]).to_ppm());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
