//! A remote screen as both ends of a screen attachment keep it (protocol doc, section
//! 6.4): an image of XRGB8888 pixels that the server updates from the VM's framebuffer
//! and turns into `ScreenFrame` rectangles, and that a viewer rebuilds from those
//! rectangles.
//!
//! The server keeps one [`Screen`] per VM, holding exactly what its viewers were last
//! sent. [`Screen::update`] compares the framebuffer with it in tiles of [`TILE`] ×
//! [`TILE`] pixels, takes in what changed, and returns the changed area as a few
//! rectangles: consecutive changed tiles of a tile row make one rectangle, and a
//! rectangle grows downwards while the tile rows below change over the same columns
//! (so a whole new screen is one rectangle, and a moved cursor one or two small ones).
//! A screen that did not change gives no rectangles, and so no frames. A viewer starts
//! from black and applies each frame with [`Screen::apply`]; after the same frames,
//! both ends hold the same pixels.
//!
//! Nothing here knows about sockets or windows, so the tests check it pixel for pixel.

use crate::Message;

/// Bytes per pixel in format 0 (XRGB8888).
pub const BYTES_PER_PIXEL: usize = 4;

/// Side of the square tiles [`Screen::update`] compares, in pixels. Smaller tiles find
/// smaller changes but cost more rectangles; 32 × 32 pixels is 4 KiB.
pub const TILE: u32 = 32;

/// A rectangle of the screen, in pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl Rect {
    /// Number of pixels in the rectangle.
    pub fn area(&self) -> u64 {
        self.width as u64 * self.height as u64
    }
}

/// An image of `width` × `height` XRGB8888 pixels, rows top to bottom with no
/// padding: the layout of Grail's VRAM and of a `ScreenFrame`'s pixels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Screen {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

impl Screen {
    /// A black screen. Panics if it is empty or too large to address (more than 4 GiB
    /// of pixels), which no `ScreenAttached` a viewer accepts can ask for.
    pub fn new(width: u32, height: u32) -> Screen {
        let len = Self::byte_len(width, height).expect("a non-empty screen that fits in memory");
        Screen {
            width,
            height,
            pixels: vec![0; len],
        }
    }

    /// Bytes of a `width` × `height` image, or `None` if it is empty or would not fit
    /// in a frame's `bytes` field.
    pub fn byte_len(width: u32, height: u32) -> Option<usize> {
        let len = width as u64 * height as u64 * BYTES_PER_PIXEL as u64;
        (len > 0 && len <= u32::MAX as u64).then_some(len as usize)
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    /// The whole screen as a rectangle.
    pub fn bounds(&self) -> Rect {
        Rect {
            x: 0,
            y: 0,
            width: self.width,
            height: self.height,
        }
    }

    /// The pixels, `width` × 4 bytes per row, top row first.
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    /// The pixel at (`x`, `y`) as `0x00RRGGBB` (the X byte is ignored).
    pub fn pixel(&self, x: u32, y: u32) -> u32 {
        let off = self.offset(x, y);
        let p = &self.pixels[off..off + BYTES_PER_PIXEL];
        u32::from_le_bytes([p[0], p[1], p[2], p[3]]) & 0x00ff_ffff
    }

    fn stride(&self) -> usize {
        self.width as usize * BYTES_PER_PIXEL
    }

    fn offset(&self, x: u32, y: u32) -> usize {
        y as usize * self.stride() + x as usize * BYTES_PER_PIXEL
    }

    /// The bytes of row `y` from column `x`, `width` pixels long.
    fn span(&self, x: u32, y: u32, width: u32) -> std::ops::Range<usize> {
        let start = self.offset(x, y);
        start..start + width as usize * BYTES_PER_PIXEL
    }

    /// Viewer side: copies a `ScreenFrame`'s rectangle in. Fails, changing nothing,
    /// if the rectangle does not lie inside the screen or `pixels` is not its size.
    pub fn apply(&mut self, rect: Rect, pixels: &[u8]) -> Result<(), String> {
        let fits = |pos: u32, len: u32, max: u32| pos as u64 + len as u64 <= max as u64;
        if rect.width == 0
            || rect.height == 0
            || !fits(rect.x, rect.width, self.width)
            || !fits(rect.y, rect.height, self.height)
        {
            return Err(format!(
                "a {}x{} rectangle at ({}, {}) is not inside the {}x{} screen",
                rect.width, rect.height, rect.x, rect.y, self.width, self.height
            ));
        }
        let row = rect.width as usize * BYTES_PER_PIXEL;
        if pixels.len() as u64 != rect.area() * BYTES_PER_PIXEL as u64 {
            return Err(format!(
                "a {}x{} rectangle needs {} pixel bytes, not {}",
                rect.width,
                rect.height,
                rect.area() * BYTES_PER_PIXEL as u64,
                pixels.len()
            ));
        }
        for (i, src) in pixels.chunks_exact(row).enumerate() {
            let dst = self.span(rect.x, rect.y + i as u32, rect.width);
            self.pixels[dst].copy_from_slice(src);
        }
        Ok(())
    }

    /// Viewer side: applies a `ScreenFrame` message; see [`Screen::apply`]. Any other
    /// message is an error.
    pub fn apply_frame(&mut self, msg: &Message) -> Result<Rect, String> {
        match msg {
            Message::ScreenFrame {
                x,
                y,
                width,
                height,
                pixels,
                ..
            } => {
                let rect = Rect {
                    x: *x,
                    y: *y,
                    width: *width,
                    height: *height,
                };
                self.apply(rect, pixels)?;
                Ok(rect)
            }
            other => Err(format!("{} is not a screen frame", other.name())),
        }
    }

    /// The pixels of `rect` (which must lie inside the screen), rows top to bottom.
    pub fn region(&self, rect: Rect) -> Vec<u8> {
        let mut out = Vec::with_capacity(rect.area() as usize * BYTES_PER_PIXEL);
        for y in rect.y..rect.y + rect.height {
            out.extend_from_slice(&self.pixels[self.span(rect.x, y, rect.width)]);
        }
        out
    }

    /// Server side: the `ScreenFrame` for VM `vm` carrying `rect` of this screen.
    pub fn frame(&self, vm: u32, rect: Rect) -> Message {
        Message::ScreenFrame {
            vm,
            x: rect.x,
            y: rect.y,
            width: rect.width,
            height: rect.height,
            pixels: self.region(rect),
        }
    }

    /// Server side: makes this screen `current` (an image of the same size; `None` is
    /// black, a display that is off) and returns the rectangles that changed, top to
    /// bottom. Empty when nothing did.
    ///
    /// Each row is compared whole first, which is all an unchanged row costs; only a
    /// row that changed is compared tile by tile, to find which tiles of its tile row
    /// to send.
    pub fn update(&mut self, current: Option<&[u8]>) -> Vec<Rect> {
        let stride = self.stride();
        if let Some(cur) = current {
            assert_eq!(cur.len(), self.pixels.len(), "an image of the screen's size");
        }
        let black = vec![0u8; if current.is_none() { stride } else { 0 }];
        let row_of = |y: u32| -> &[u8] {
            match current {
                Some(cur) => &cur[y as usize * stride..(y as usize + 1) * stride],
                None => &black,
            }
        };
        let tiles_x = self.width.div_ceil(TILE);
        let mut rects: Vec<Rect> = Vec::new();
        // Rectangles that reach the bottom of the previous tile row: they may grow down.
        let mut open: Vec<usize> = Vec::new();
        let mut dirty = vec![false; tiles_x as usize];
        for band in (0..self.height).step_by(TILE as usize) {
            let band_height = TILE.min(self.height - band);
            dirty.fill(false);
            for y in band..band + band_height {
                let new = row_of(y);
                let old = &mut self.pixels[y as usize * stride..(y as usize + 1) * stride];
                if old == new {
                    continue;
                }
                let tile_bytes = TILE as usize * BYTES_PER_PIXEL;
                for (t, flag) in dirty.iter_mut().enumerate() {
                    let start = t * tile_bytes;
                    let end = (start + tile_bytes).min(stride);
                    if !*flag && old[start..end] != new[start..end] {
                        *flag = true;
                    }
                }
                // Copying the whole row is the same as copying its changed tiles: the
                // others are equal already.
                old.copy_from_slice(new);
            }
            // Runs of changed tiles, each joined to a rectangle of the band above that
            // spans the same columns, or a new rectangle.
            let mut still_open = Vec::new();
            let mut t = 0;
            while t < tiles_x as usize {
                if !dirty[t] {
                    t += 1;
                    continue;
                }
                let first = t;
                while t < tiles_x as usize && dirty[t] {
                    t += 1;
                }
                let x = first as u32 * TILE;
                let width = (t as u32 * TILE).min(self.width) - x;
                let above = open
                    .iter()
                    .copied()
                    .find(|&i| rects[i].x == x && rects[i].width == width);
                match above {
                    Some(i) => {
                        rects[i].height += band_height;
                        still_open.push(i);
                    }
                    None => {
                        still_open.push(rects.len());
                        rects.push(Rect {
                            x,
                            y: band,
                            width,
                            height: band_height,
                        });
                    }
                }
            }
            open = still_open;
        }
        rects
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An image whose every pixel is `f(x, y)`.
    fn image(width: u32, height: u32, f: impl Fn(u32, u32) -> u32) -> Vec<u8> {
        let mut v = Vec::with_capacity((width * height) as usize * 4);
        for y in 0..height {
            for x in 0..width {
                v.extend_from_slice(&f(x, y).to_le_bytes());
            }
        }
        v
    }

    fn rect(x: u32, y: u32, width: u32, height: u32) -> Rect {
        Rect { x, y, width, height }
    }

    #[test]
    fn an_unchanged_screen_has_no_rectangles() {
        let mut s = Screen::new(640, 480);
        assert!(s.update(None).is_empty(), "black is black");
        assert!(s.update(Some(&vec![0; 640 * 480 * 4])).is_empty());
        let img = image(640, 480, |x, y| x ^ y << 8);
        assert_eq!(s.update(Some(&img)), [s.bounds()]);
        assert!(s.update(Some(&img)).is_empty());
        assert_eq!(s.pixels(), img);
    }

    #[test]
    fn changes_become_tile_rectangles() {
        let mut s = Screen::new(640, 480);
        let mut img = vec![0; 640 * 480 * 4];
        // One pixel: its tile.
        img[s.offset(100, 50)] = 1;
        assert_eq!(s.update(Some(&img)), [rect(96, 32, 32, 32)]);
        // Two pixels in neighbouring tiles of one row: one rectangle.
        img[s.offset(127, 70)] = 2;
        img[s.offset(128, 70)] = 2;
        assert_eq!(s.update(Some(&img)), [rect(96, 64, 64, 32)]);
        // A column of pixels down three tile rows: one tall rectangle; another change
        // apart from it its own.
        for y in 0..96 {
            let off = s.offset(5, y);
            img[off] = 3;
        }
        img[s.offset(600, 470)] = 4;
        assert_eq!(s.update(Some(&img)), [rect(0, 0, 32, 96), rect(576, 448, 32, 32)]);
        // Off again: everything that is not black. The tiles at x 96-159 do not make
        // one rectangle: the tile row at y 32 changed one tile there, the next two.
        let rects = s.update(None);
        assert_eq!(
            rects,
            [
                rect(0, 0, 32, 96),
                rect(96, 32, 32, 32),
                rect(96, 64, 64, 32),
                rect(576, 448, 32, 32)
            ]
        );
        assert!(s.pixels().iter().all(|&b| b == 0));
    }

    #[test]
    fn rectangles_only_grow_down_over_the_same_columns() {
        let mut s = Screen::new(128, 96);
        let mut img = vec![0; 128 * 96 * 4];
        let mut set = |x: u32, y: u32| img[(y * 128 + x) as usize * 4] = 9;
        // Row 0: tiles 0-1; row 1: tiles 0-1 and 3; row 2: tile 1.
        set(0, 0);
        set(40, 0);
        set(0, 40);
        set(40, 40);
        set(100, 40);
        set(40, 70);
        assert_eq!(
            s.update(Some(&img)),
            [rect(0, 0, 64, 64), rect(96, 32, 32, 32), rect(32, 64, 32, 32)]
        );
    }

    #[test]
    fn partial_tiles_at_the_edges() {
        // 70 x 40: tiles of 32, 32 and 6 pixels across, 32 and 8 down.
        let mut s = Screen::new(70, 40);
        let img = image(70, 40, |x, y| if x == 69 && y == 39 { 0xff } else { 0 });
        assert_eq!(s.update(Some(&img)), [rect(64, 32, 6, 8)]);
        let img = image(70, 40, |_, _| 7);
        assert_eq!(s.update(Some(&img)), [rect(0, 0, 70, 40)]);
    }

    /// The viewer's side: whatever changes, the rectangles `update` returns, applied
    /// to a copy of the old screen, give the new one.
    #[test]
    fn frames_rebuild_the_screen() {
        let (w, h) = (200, 150);
        let mut server = Screen::new(w, h);
        let mut viewer = Screen::new(w, h);
        let mut img = vec![0u8; (w * h * 4) as usize];
        let mut seed = 0x1234_5678u32;
        let mut rand = move || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed
        };
        for round in 0..200 {
            // A few random boxes of random colours.
            for _ in 0..(rand() % 4) {
                let (x, y) = (rand() % w, rand() % h);
                let (bw, bh) = (1 + rand() % (w - x), 1 + rand() % (h - y).min(40));
                let colour = rand() & 0xffffff;
                for yy in y..y + bh {
                    for xx in x..x + bw {
                        let off = ((yy * w + xx) * 4) as usize;
                        img[off..off + 4].copy_from_slice(&colour.to_le_bytes());
                    }
                }
            }
            let current = (round % 17 != 16).then_some(&img[..]);
            for r in server.update(current) {
                let Message::ScreenFrame { vm, .. } = server.frame(3, r) else {
                    unreachable!()
                };
                assert_eq!(vm, 3);
                assert_eq!(viewer.apply_frame(&server.frame(3, r)), Ok(r));
            }
            assert_eq!(viewer, server, "round {round}");
            let want: &[u8] = current.unwrap_or(&[]);
            if current.is_some() {
                assert_eq!(viewer.pixels(), want);
            } else {
                assert!(viewer.pixels().iter().all(|&b| b == 0));
            }
        }
    }

    #[test]
    fn frames_outside_the_screen_are_refused() {
        let mut s = Screen::new(10, 10);
        let before = s.clone();
        for (r, len) in [
            (rect(9, 0, 2, 1), 8),
            (rect(0, 10, 1, 1), 4),
            (rect(0, 0, 0, 5), 0),
            (rect(u32::MAX, 0, 2, 1), 8),
            (rect(0, u32::MAX - 1, 1, 4), 16),
            (rect(0, 0, 2, 2), 12),
        ] {
            assert!(s.apply(r, &vec![0xff; len]).is_err(), "{r:?}");
        }
        assert_eq!(s, before);
        assert!(s.apply_frame(&Message::Ok).is_err());
        s.apply(rect(8, 9, 2, 1), &[1, 2, 3, 4, 5, 6, 7, 8]).unwrap();
        assert_eq!(s.pixel(8, 9), 0x030201);
        assert_eq!(s.pixel(9, 9), 0x070605);
        assert_eq!(s.region(rect(8, 9, 2, 1)), [1, 2, 3, 4, 5, 6, 7, 8]);
    }

    #[test]
    fn sizes() {
        assert_eq!(Screen::byte_len(640, 480), Some(1_228_800));
        assert_eq!(Screen::byte_len(0, 480), None);
        assert_eq!(Screen::byte_len(65536, 65536), None);
        assert_eq!(Screen::new(3, 2).pixels().len(), 24);
    }
}
