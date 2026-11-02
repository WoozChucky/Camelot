//! Xcal framebuffer (spec section 9.3): a few control registers plus 640x480 XRGB8888 VRAM.
//!
//! The device only holds state. Showing it is the host's job: `display` blits it into a
//! window or dumps it as PPM files.

use crate::bus::BusFault;
use std::io;
use std::path::{Path, PathBuf};

pub const FB_WIDTH: u32 = 640;
pub const FB_HEIGHT: u32 = 480;
pub const FB_STRIDE: u32 = FB_WIDTH * 4;
/// 0 = XRGB8888: each pixel is a little-endian word 0x00RRGGBB.
pub const FB_FORMAT_XRGB8888: u32 = 0;
pub const VRAM_SIZE: u32 = FB_STRIDE * FB_HEIGHT;

// Register offsets from the start of the control block.
pub const REG_WIDTH: u32 = 0x00;
pub const REG_HEIGHT: u32 = 0x04;
pub const REG_STRIDE: u32 = 0x08;
pub const REG_FORMAT: u32 = 0x0c;
pub const REG_CTRL: u32 = 0x10;
pub const REG_PRESENT: u32 = 0x14;

/// `FB_CTRL` bit 0: display enable. The other bits are reserved and read as 0.
pub const CTRL_ENABLE: u32 = 1 << 0;

pub struct Framebuffer {
    /// Pixel memory, `FB_STRIDE` bytes per row, top row first. Byte order B, G, R, X,
    /// which is also what a 32-bpp Windows DIB expects, so it can be blitted as is.
    pub vram: Vec<u8>,
    pub ctrl: u32,
    /// Set by a write to `FB_PRESENT`, cleared by the host with `take_present`.
    present: bool,
    /// How many times the program wrote `FB_PRESENT`.
    pub presents: u64,
}

impl Default for Framebuffer {
    fn default() -> Self {
        Framebuffer {
            vram: vec![0; VRAM_SIZE as usize],
            ctrl: 0,
            present: false,
            presents: 0,
        }
    }
}

impl Framebuffer {
    /// Control registers are 32-bit only. Unknown offsets read as 0.
    pub fn read(&mut self, off: u32, size: u32) -> Result<u32, BusFault> {
        if size != 4 {
            return Err(BusFault);
        }
        Ok(match off {
            REG_WIDTH => FB_WIDTH,
            REG_HEIGHT => FB_HEIGHT,
            REG_STRIDE => FB_STRIDE,
            REG_FORMAT => FB_FORMAT_XRGB8888,
            REG_CTRL => self.ctrl,
            _ => 0, // FB_PRESENT is write-only
        })
    }

    /// Writes to read-only registers and unknown offsets are ignored.
    pub fn write(&mut self, off: u32, size: u32, val: u32) -> Result<(), BusFault> {
        if size != 4 {
            return Err(BusFault);
        }
        match off {
            REG_CTRL => self.ctrl = val & CTRL_ENABLE,
            REG_PRESENT => {
                self.present = true;
                self.presents += 1;
            }
            _ => {}
        }
        Ok(())
    }

    pub fn enabled(&self) -> bool {
        self.ctrl & CTRL_ENABLE != 0
    }

    /// Whether a `FB_PRESENT` write is waiting for the host.
    pub fn present_pending(&self) -> bool {
        self.present
    }

    /// Returns whether the program wrote `FB_PRESENT` since the last call.
    pub fn take_present(&mut self) -> bool {
        std::mem::take(&mut self.present)
    }

    /// Pixel at (x, y) as 0x00RRGGBB.
    pub fn pixel(&self, x: u32, y: u32) -> u32 {
        let off = (y * FB_STRIDE + x * 4) as usize;
        u32::from_le_bytes([
            self.vram[off],
            self.vram[off + 1],
            self.vram[off + 2],
            self.vram[off + 3],
        ]) & 0x00ff_ffff
    }

    /// The image the display shows, as a binary PPM (P6). A disabled display is black.
    pub fn to_ppm(&self) -> Vec<u8> {
        let header = format!("P6\n{FB_WIDTH} {FB_HEIGHT}\n255\n");
        let len = header.len() + (FB_WIDTH * FB_HEIGHT * 3) as usize;
        let mut out = Vec::with_capacity(len);
        out.extend_from_slice(header.as_bytes());
        if !self.enabled() {
            out.resize(len, 0);
            return out;
        }
        // Rows are packed (stride = width * 4), so VRAM is just a run of pixels.
        for &[b, g, r, _] in self.vram.as_chunks::<4>().0 {
            out.extend_from_slice(&[r, g, b]);
        }
        out
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

    pub fn dump(&mut self, fb: &Framebuffer) -> io::Result<PathBuf> {
        let path = self.dir.join(format!("frame-{:04}.ppm", self.next));
        std::fs::write(&path, fb.to_ppm())?;
        self.next += 1;
        Ok(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn info_registers() {
        let mut fb = Framebuffer::default();
        assert_eq!(fb.read(REG_WIDTH, 4), Ok(640));
        assert_eq!(fb.read(REG_HEIGHT, 4), Ok(480));
        assert_eq!(fb.read(REG_STRIDE, 4), Ok(2560));
        assert_eq!(fb.read(REG_FORMAT, 4), Ok(0));
        assert_eq!(fb.read(0x18, 4), Ok(0));
        assert_eq!(fb.read(0xffc, 4), Ok(0));
        assert_eq!(VRAM_SIZE, 0x12_c000);
    }

    #[test]
    fn read_only_registers_ignore_writes() {
        let mut fb = Framebuffer::default();
        for reg in [REG_WIDTH, REG_HEIGHT, REG_STRIDE, REG_FORMAT] {
            fb.write(reg, 4, 0xdead_beef).unwrap();
        }
        assert_eq!(fb.read(REG_WIDTH, 4), Ok(640));
        assert_eq!(fb.read(REG_HEIGHT, 4), Ok(480));
        assert_eq!(fb.read(REG_STRIDE, 4), Ok(2560));
        assert_eq!(fb.read(REG_FORMAT, 4), Ok(0));
    }

    #[test]
    fn ctrl_keeps_only_enable_bit() {
        let mut fb = Framebuffer::default();
        assert!(!fb.enabled());
        fb.write(REG_CTRL, 4, 0xffff_ffff).unwrap();
        assert_eq!(fb.read(REG_CTRL, 4), Ok(1));
        assert!(fb.enabled());
        fb.write(REG_CTRL, 4, 0xffff_fffe).unwrap();
        assert_eq!(fb.read(REG_CTRL, 4), Ok(0));
        assert!(!fb.enabled());
    }

    #[test]
    fn present_is_write_only_and_latched() {
        let mut fb = Framebuffer::default();
        assert!(!fb.take_present());
        fb.write(REG_PRESENT, 4, 0).unwrap();
        assert!(fb.present_pending());
        fb.write(REG_PRESENT, 4, 123).unwrap();
        assert_eq!(fb.read(REG_PRESENT, 4), Ok(0));
        assert_eq!(fb.presents, 2);
        assert!(fb.take_present());
        assert!(!fb.present_pending());
        assert!(!fb.take_present());
    }

    #[test]
    fn registers_are_word_only() {
        let mut fb = Framebuffer::default();
        assert_eq!(fb.read(REG_WIDTH, 1), Err(BusFault));
        assert_eq!(fb.read(REG_WIDTH, 2), Err(BusFault));
        assert_eq!(fb.write(REG_CTRL, 1, 1), Err(BusFault));
        assert_eq!(fb.write(REG_PRESENT, 2, 1), Err(BusFault));
        assert!(!fb.enabled());
        assert!(!fb.take_present());
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
        let ppm = fb.to_ppm();
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
        let ppm = fb.to_ppm();
        let header = b"P6\n640 480\n255\n";
        assert_eq!(ppm.len(), header.len() + 640 * 480 * 3);
        assert!(ppm[header.len()..].iter().all(|&b| b == 0));
    }

    #[test]
    fn dumper_numbers_frames() {
        let dir = std::env::temp_dir().join(format!("grail-fb-unit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut d = FrameDumper::new(dir.join("nested")).unwrap();
        let fb = Framebuffer::default();
        let a = d.dump(&fb).unwrap();
        let b = d.dump(&fb).unwrap();
        assert_eq!(a.file_name().unwrap(), "frame-0000.ppm");
        assert_eq!(b.file_name().unwrap(), "frame-0001.ppm");
        assert_eq!(std::fs::read(&b).unwrap(), fb.to_ppm());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
