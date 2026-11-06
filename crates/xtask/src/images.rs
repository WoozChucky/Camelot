//! The desktop's images (#183): drawn by hand as XPM files in
//! [`IMAGES_DIR`] (`kernel/images/NAME.xpm`), text that reads as the
//! picture and diffs line by line, and converted by `cargo xtask fs` into
//! QOI, the format the GUI library decodes (`kernel/user/gui/image.c`), for
//! the desktop image's `/usr/share/images/NAME.qoi`.
//!
//! XPM (X PixMap, version 3) is C: an array of strings. The first is
//! `"WIDTH HEIGHT COLOURS CHARS"` (and a hotspot, which this ignores), then a
//! string a colour, `CHARS` characters naming it and `c` and its value, a
//! `#RRGGBB` or `None` (transparent), then a string a row, `CHARS`
//! characters a pixel. Only the `c` key is read.
//!
//! QOI (qoiformat.org) is encoded as its reference encoder does: the same
//! chunks, byte for byte. Its header says 4 channels when some pixel is not
//! opaque, else 3.

use std::path::Path;

/// The images' sources, relative to the repository root.
pub const IMAGES_DIR: &str = "kernel/images";

/// An image: pixels as 0xAARRGGBB, in rows, alpha not premultiplied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u32>,
}

/// The string literals of C source `text`, in order, comments skipped.
fn strings(text: &str) -> Result<Vec<String>, String> {
    let b = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i..].starts_with(b"/*") {
            let end = text[i + 2..].find("*/").ok_or("a comment not ended")?;
            i += end + 4;
        } else if b[i] == b'"' {
            let mut s = String::new();
            i += 1;
            while i < b.len() && b[i] != b'"' {
                if b[i] == b'\\' || b[i] == b'\n' {
                    return Err("an escape or a newline in a string".into());
                }
                s.push(b[i] as char);
                i += 1;
            }
            if i == b.len() {
                return Err("a string not ended".into());
            }
            i += 1;
            out.push(s);
        } else {
            i += 1;
        }
    }
    Ok(out)
}

/// Reads an XPM image.
pub fn parse_xpm(text: &str) -> Result<Image, String> {
    let s = strings(text)?;
    let head: Vec<u32> = s
        .first()
        .ok_or("no strings")?
        .split_whitespace()
        .map(|w| w.parse().map_err(|_| format!("a bad number in the header: {w}")))
        .collect::<Result<_, _>>()?;
    if head.len() != 4 && head.len() != 6 {
        return Err("the header is width, height, colours, characters a pixel (and a hotspot)".into());
    }
    let (width, height, colours, cpp) = (head[0], head[1], head[2] as usize, head[3] as usize);
    if width == 0 || height == 0 || cpp == 0 || colours == 0 {
        return Err("an empty image".into());
    }
    if s.len() != 1 + colours + height as usize {
        return Err(format!(
            "{} strings, not 1 + {colours} colours + {height} rows",
            s.len()
        ));
    }
    let mut palette: Vec<(String, u32)> = Vec::new();
    for line in &s[1..1 + colours] {
        if line.len() < cpp {
            return Err(format!("colour line too short: {line:?}"));
        }
        let (key, rest) = line.split_at(cpp);
        let words: Vec<&str> = rest.split_whitespace().collect();
        let at = words
            .iter()
            .position(|w| *w == "c")
            .ok_or_else(|| format!("no `c` colour: {line:?}"))?;
        let value = words.get(at + 1).ok_or_else(|| format!("no value: {line:?}"))?;
        let argb = if value.eq_ignore_ascii_case("none") {
            0
        } else {
            let hex = value
                .strip_prefix('#')
                .filter(|h| h.len() == 6)
                .ok_or_else(|| format!("not #RRGGBB or None: {value}"))?;
            0xff00_0000 | u32::from_str_radix(hex, 16).map_err(|_| format!("not hex: {value}"))?
        };
        if palette.iter().any(|(k, _)| k == key) {
            return Err(format!("colour {key:?} twice"));
        }
        palette.push((key.to_string(), argb));
    }
    let mut pixels = Vec::with_capacity((width * height) as usize);
    for (y, row) in s[1 + colours..].iter().enumerate() {
        if row.len() != width as usize * cpp {
            return Err(format!(
                "row {y} is {} characters, not {}",
                row.len(),
                width as usize * cpp
            ));
        }
        for x in 0..width as usize {
            let key = &row[x * cpp..(x + 1) * cpp];
            let (_, argb) = palette
                .iter()
                .find(|(k, _)| k == key)
                .ok_or_else(|| format!("row {y}: no colour {key:?}"))?;
            pixels.push(*argb);
        }
    }
    Ok(Image {
        width,
        height,
        pixels,
    })
}

/// QOI's hash of a pixel: where its encoder and decoder keep it.
fn qoi_hash(p: u32) -> usize {
    let (a, r, g, b) = (p >> 24, (p >> 16) & 0xff, (p >> 8) & 0xff, p & 0xff);
    ((r * 3 + g * 5 + b * 7 + a * 11) % 64) as usize
}

/// `img` as QOI, as the reference encoder (qoi.h's `qoi_encode`) writes it.
pub fn encode_qoi(img: &Image) -> Vec<u8> {
    let channels: u8 = if img.pixels.iter().all(|p| p >> 24 == 0xff) {
        3
    } else {
        4
    };
    let mut out = b"qoif".to_vec();
    out.extend(img.width.to_be_bytes());
    out.extend(img.height.to_be_bytes());
    out.extend([channels, 0]);
    let mut index = [0u32; 64];
    let mut prev: u32 = 0xff00_0000;
    let mut run = 0u8;
    let last = img.pixels.len() - 1;
    for (i, &px) in img.pixels.iter().enumerate() {
        if px == prev {
            run += 1;
            if run == 62 || i == last {
                out.push(0xc0 | (run - 1));
                run = 0;
            }
            continue;
        }
        if run > 0 {
            out.push(0xc0 | (run - 1));
            run = 0;
        }
        let h = qoi_hash(px);
        if index[h] == px {
            out.push(h as u8);
        } else {
            index[h] = px;
            let ch = |p: u32, s: u32| ((p >> s) & 0xff) as u8;
            if px >> 24 == prev >> 24 {
                let vr = ch(px, 16).wrapping_sub(ch(prev, 16)) as i8;
                let vg = ch(px, 8).wrapping_sub(ch(prev, 8)) as i8;
                let vb = ch(px, 0).wrapping_sub(ch(prev, 0)) as i8;
                let vg_r = vr.wrapping_sub(vg);
                let vg_b = vb.wrapping_sub(vg);
                if (-2..=1).contains(&vr) && (-2..=1).contains(&vg) && (-2..=1).contains(&vb) {
                    out.push(0x40 | (((vr + 2) as u8) << 4) | (((vg + 2) as u8) << 2) | (vb + 2) as u8);
                } else if (-8..=7).contains(&vg_r) && (-32..=31).contains(&vg) && (-8..=7).contains(&vg_b) {
                    out.push(0x80 | (vg + 32) as u8);
                    out.push((((vg_r + 8) as u8) << 4) | (vg_b + 8) as u8);
                } else {
                    out.extend([0xfe, ch(px, 16), ch(px, 8), ch(px, 0)]);
                }
            } else {
                out.extend([0xff, ch(px, 16), ch(px, 8), ch(px, 0), ch(px, 24)]);
            }
        }
        prev = px;
    }
    out.extend([0, 0, 0, 0, 0, 0, 0, 1]);
    out
}

/// Reads QOI, as the GUI library does: for the tests.
pub fn decode_qoi(b: &[u8]) -> Result<Image, String> {
    if b.len() < 22 || &b[..4] != b"qoif" {
        return Err("not QOI".into());
    }
    let width = u32::from_be_bytes(b[4..8].try_into().unwrap());
    let height = u32::from_be_bytes(b[8..12].try_into().unwrap());
    let total = (width * height) as usize;
    let mut pixels = Vec::with_capacity(total);
    let mut index = [0u32; 64];
    let mut px: u32 = 0xff00_0000;
    let mut at = 14;
    let end = b.len() - 8;
    let mut run = 0;
    while pixels.len() < total {
        if run > 0 {
            run -= 1;
        } else {
            let op = *b.get(at).filter(|_| at < end).ok_or("cut short")?;
            at += 1;
            let ch = |p: u32, s: u32| (p >> s) & 0xff;
            let set = |a: u32, r: u32, g: u32, b: u32| {
                (a << 24) | ((r & 0xff) << 16) | ((g & 0xff) << 8) | (b & 0xff)
            };
            match op {
                0xfe => {
                    px = set(px >> 24, b[at] as u32, b[at + 1] as u32, b[at + 2] as u32);
                    at += 3;
                }
                0xff => {
                    px = set(b[at + 3] as u32, b[at] as u32, b[at + 1] as u32, b[at + 2] as u32);
                    at += 4;
                }
                _ => match op >> 6 {
                    0 => px = index[op as usize],
                    1 => {
                        let d = |v: u8| (v as u32 & 3).wrapping_sub(2);
                        px = set(
                            px >> 24,
                            ch(px, 16).wrapping_add(d(op >> 4)),
                            ch(px, 8).wrapping_add(d(op >> 2)),
                            ch(px, 0).wrapping_add(d(op)),
                        );
                    }
                    2 => {
                        let b2 = b[at] as u32;
                        at += 1;
                        let dg = (op as u32 & 0x3f).wrapping_sub(32);
                        px = set(
                            px >> 24,
                            ch(px, 16)
                                .wrapping_add(dg)
                                .wrapping_add((b2 >> 4).wrapping_sub(8)),
                            ch(px, 8).wrapping_add(dg),
                            ch(px, 0)
                                .wrapping_add(dg)
                                .wrapping_add((b2 & 0xf).wrapping_sub(8)),
                        );
                    }
                    _ => run = (op & 0x3f) as usize,
                },
            }
            index[qoi_hash(px)] = px;
        }
        pixels.push(px);
    }
    Ok(Image {
        width,
        height,
        pixels,
    })
}

/// Every image in [`IMAGES_DIR`] under `root`, by name: `NAME.qoi` and its
/// bytes.
pub fn build(root: &Path) -> Result<Vec<(String, Vec<u8>)>, String> {
    let dir = root.join(IMAGES_DIR);
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .map_err(|e| format!("cannot read {}: {e}", dir.display()))?
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter(|n| n.ends_with(".xpm"))
        .collect();
    names.sort();
    names
        .iter()
        .map(|n| {
            let path = dir.join(n);
            let text =
                std::fs::read_to_string(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
            let img = parse_xpm(&text).map_err(|e| format!("{}: {e}", path.display()))?;
            Ok((format!("{}.qoi", n.trim_end_matches(".xpm")), encode_qoi(&img)))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xpm_is_read_with_its_palette_and_transparency() {
        let img = parse_xpm(
            "/* XPM */\nstatic char *x[] = {\n/* w h c cpp */\n\"3 2 2 1\",\n\". c None\",\n\"X c #102030\",\n\"X.X\",\n\".X.\"\n};\n",
        )
        .unwrap();
        assert_eq!((img.width, img.height), (3, 2));
        assert_eq!(img.pixels, [0xff10_2030, 0, 0xff10_2030, 0, 0xff10_2030, 0]);
        assert!(parse_xpm("\"2 1 1 1\", \". c red\", \"..\"").is_err());
        assert!(parse_xpm("\"2 1 1 1\", \". c None\", \"...\"").is_err());
        assert!(parse_xpm("\"2 1 1 1\", \". c None\", \".x\"").is_err());
    }

    /// Every chunk the encoder writes, read back as it was: a run past 62,
    /// small and middle differences, a pixel seen before, alpha changing.
    #[test]
    fn qoi_round_trips() {
        let mut pixels = vec![0xff00_0000; 70];
        pixels.extend([
            0xff01_0203,
            0xff01_0103,
            0xff11_1513,
            0xff80_4020,
            0xff01_0103,
            0x8001_0103,
            0,
        ]);
        pixels.extend((0..50).map(|i| 0xff00_0000 | i * 0x0103_07));
        let img = Image {
            width: pixels.len() as u32,
            height: 1,
            pixels,
        };
        let qoi = encode_qoi(&img);
        assert_eq!(qoi[12], 4);
        assert_eq!(decode_qoi(&qoi).unwrap(), img);
    }

    #[test]
    fn the_images_build() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let images = build(&root).unwrap();
        assert!(images.iter().any(|(n, _)| n == "cursor.qoi"), "{images:?}");
    }
}
