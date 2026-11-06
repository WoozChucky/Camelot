//! The framebuffer console's font (`kernel/fbcon.c`): Spleen 8x16, by
//! Frederic Cambus (BSD-2-Clause, `kernel/fonts/LICENSE.spleen`), as its
//! release ships it for the Linux console, a PSF1 file
//! (`kernel/fonts/spleen-8x16.psfu`, kept as it came). The kernel embeds it
//! as PSF2, the format Linux's console fonts have had since 1999 and the
//! one a desktop's text renderer reads too, so this module converts it:
//! [`psf1_to_psf2`], the same glyphs with a PSF2 header, and the Unicode
//! table in PSF2's UTF-8 form.
//!
//! `cargo xtask kernel` writes the converted font into [`FONT_C`], a C
//! array, before it builds the kernel ([`write`]); a test checks the
//! committed file is what [`files`] gives.
//!
//! The two formats (Linux's `include/linux/kd.h`, `psf.h` of the kbd
//! project):
//!
//! - PSF1: magic 0x36 0x04, a mode byte (bit 0: 512 glyphs rather than 256,
//!   bit 1: a Unicode table follows), the glyphs' height (each row a byte,
//!   8 pixels wide), then the glyphs. Its Unicode table has, for each
//!   glyph, little-endian u16 code points ended by 0xFFFF; 0xFFFE starts a
//!   sequence of combining ones.
//! - PSF2: a 32-byte header of little-endian u32s: magic 0x864ab572,
//!   version 0, the header's size, flags (bit 0: a Unicode table), the
//!   number of glyphs, the bytes a glyph, its height and its width; then
//!   the glyphs, each row in whole bytes, the leftmost pixel the top bit.
//!   Its Unicode table has each code point in UTF-8, a glyph's ended by
//!   0xFF, 0xFE starting a sequence.

use std::fmt::Write;
use std::path::Path;

/// The font as Spleen's release has it, relative to the repository root.
pub const SOURCE: &str = "kernel/fonts/spleen-8x16.psfu";
/// The C file the kernel embeds it from.
pub const FONT_C: &str = "kernel/font.c";

/// PSF2's magic, as a little-endian u32.
pub const PSF2_MAGIC: u32 = 0x864a_b572;
/// PSF2's flag for a Unicode table.
pub const PSF2_HAS_UNICODE_TABLE: u32 = 1;
const PSF2_HEADER_SIZE: u32 = 32;

const PSF1_MAGIC: [u8; 2] = [0x36, 0x04];
const PSF1_MODE512: u8 = 0x01;
const PSF1_MODEHASTAB: u8 = 0x02;
const PSF1_SEPARATOR: u16 = 0xffff;
const PSF1_STARTSEQ: u16 = 0xfffe;
const PSF2_SEPARATOR: u8 = 0xff;
const PSF2_STARTSEQ: u8 = 0xfe;

/// A PSF2 font, as read by [`parse_psf2`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Psf2<'a> {
    pub glyphs: u32,
    pub bytes_per_glyph: u32,
    pub height: u32,
    pub width: u32,
    /// The glyphs, `bytes_per_glyph` each.
    pub data: &'a [u8],
}

impl Psf2<'_> {
    /// Glyph `n`'s rows, `bytes_per_glyph` bytes.
    pub fn glyph(&self, n: u32) -> &[u8] {
        let size = self.bytes_per_glyph as usize;
        &self.data[n as usize * size..(n as usize + 1) * size]
    }
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}

/// Reads a PSF2 font's header and glyphs.
pub fn parse_psf2(b: &[u8]) -> Result<Psf2<'_>, String> {
    if b.len() < PSF2_HEADER_SIZE as usize || u32_at(b, 0) != PSF2_MAGIC {
        return Err("not a PSF2 font".into());
    }
    let header = u32_at(b, 8) as usize;
    let glyphs = u32_at(b, 16);
    let bytes_per_glyph = u32_at(b, 20);
    let end = header + glyphs as usize * bytes_per_glyph as usize;
    if end > b.len() {
        return Err("a PSF2 font cut short".into());
    }
    Ok(Psf2 {
        glyphs,
        bytes_per_glyph,
        height: u32_at(b, 24),
        width: u32_at(b, 28),
        data: &b[header..end],
    })
}

/// `psf1`, a PSF1 font with a Unicode table, as PSF2: the same glyphs and
/// table. Checks that each printable ASCII character is the glyph of its own
/// number, which the kernel's console relies on.
pub fn psf1_to_psf2(psf1: &[u8]) -> Result<Vec<u8>, String> {
    if psf1.len() < 4 || psf1[..2] != PSF1_MAGIC {
        return Err("not a PSF1 font".into());
    }
    let mode = psf1[2];
    let height = u32::from(psf1[3]);
    let glyphs: u32 = if mode & PSF1_MODE512 != 0 { 512 } else { 256 };
    if mode & PSF1_MODEHASTAB == 0 {
        return Err("a PSF1 font without a Unicode table".into());
    }
    let glyph_end = 4 + (glyphs * height) as usize;
    if psf1.len() < glyph_end {
        return Err("a PSF1 font cut short".into());
    }
    let mut out = Vec::new();
    for v in [
        PSF2_MAGIC,
        0,
        PSF2_HEADER_SIZE,
        PSF2_HAS_UNICODE_TABLE,
        glyphs,
        height,
        height,
        8,
    ] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.extend_from_slice(&psf1[4..glyph_end]);
    // The table: u16s to UTF-8, glyph by glyph.
    let table = &psf1[glyph_end..];
    if !table.len().is_multiple_of(2) {
        return Err("a PSF1 Unicode table of an odd size".into());
    }
    let mut glyph = 0u32;
    let mut first = Vec::new(); // each glyph's code points before any sequence
    let mut in_seq = false;
    for pair in table.chunks(2) {
        let u = u16::from_le_bytes([pair[0], pair[1]]);
        match u {
            PSF1_SEPARATOR => {
                out.push(PSF2_SEPARATOR);
                glyph += 1;
                in_seq = false;
            }
            PSF1_STARTSEQ => {
                out.push(PSF2_STARTSEQ);
                in_seq = true;
            }
            _ => {
                let c = char::from_u32(u32::from(u))
                    .ok_or_else(|| format!("glyph {glyph}: U+{u:04X} is no character"))?;
                let mut buf = [0; 4];
                out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
                if !in_seq {
                    first.push((glyph, u32::from(u)));
                }
            }
        }
    }
    if glyph != glyphs {
        return Err(format!("a Unicode table of {glyph} glyphs, not {glyphs}"));
    }
    for c in 0x20..=0x7e {
        if !first.contains(&(c, c)) {
            return Err(format!("{:?} is not glyph {c}", char::from_u32(c).unwrap()));
        }
    }
    Ok(out)
}

/// [`FONT_C`]'s text for `psf2`.
pub fn generate_c(psf2: &[u8]) -> String {
    let mut s = String::new();
    for line in [
        "/* Generated by `cargo xtask kernel` (crates/xtask/src/font.rs): do not edit.",
        " *",
        " * Spleen 8x16 by Frederic Cambus, BSD-2-Clause (fonts/LICENSE.spleen),",
        " * from fonts/spleen-8x16.psfu converted to PSF2: the font the framebuffer",
        " * console draws with (fbcon.c). */",
        "#include \"fbcon.h\"",
        "",
    ] {
        s.push_str(line);
        s.push('\n');
    }
    writeln!(s, "const usize font_psf2_size = {};", psf2.len()).unwrap();
    writeln!(s, "const u8 font_psf2[{}] = {{", psf2.len()).unwrap();
    for row in psf2.chunks(16) {
        s.push_str("   ");
        for b in row {
            write!(s, " 0x{b:02x},").unwrap();
        }
        s.push('\n');
    }
    s.push_str("};\n");
    s
}

/// The font, converted: [`psf1_to_psf2`] of [`SOURCE`].
pub fn psf2(root: &Path) -> Result<Vec<u8>, String> {
    let path = root.join(SOURCE);
    let psf1 = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    psf1_to_psf2(&psf1).map_err(|e| format!("{}: {e}", path.display()))
}

/// The generated files and their text.
pub fn files(root: &Path) -> Result<Vec<(&'static str, String)>, String> {
    Ok(vec![(FONT_C, generate_c(&psf2(root)?))])
}

/// Writes [`files`] where they differ from what is there; returns those it
/// wrote.
pub fn write(root: &Path) -> Result<Vec<&'static str>, String> {
    let mut wrote = Vec::new();
    for (file, text) in files(root)? {
        let path = root.join(file);
        if std::fs::read_to_string(&path).is_ok_and(|old| old.replace('\r', "") == text) {
            continue;
        }
        std::fs::write(&path, text).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
        wrote.push(file);
    }
    Ok(wrote)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_psf1_font_becomes_psf2_with_its_glyphs_and_table() {
        // Two glyphs of height 2... but ASCII must be there: a font of 256
        // glyphs, each its own code point.
        let mut psf1 = vec![0x36, 0x04, PSF1_MODEHASTAB, 2];
        for g in 0..256u32 {
            psf1.extend_from_slice(&[g as u8, !g as u8]);
        }
        for g in 0..256u16 {
            psf1.extend_from_slice(&g.to_le_bytes());
            if g == 0x41 {
                // A sequence: A and a combining ring, U+030A.
                psf1.extend_from_slice(&PSF1_STARTSEQ.to_le_bytes());
                psf1.extend_from_slice(&0x41u16.to_le_bytes());
                psf1.extend_from_slice(&0x30au16.to_le_bytes());
            }
            psf1.extend_from_slice(&PSF1_SEPARATOR.to_le_bytes());
        }
        let psf2 = psf1_to_psf2(&psf1).unwrap();
        let f = parse_psf2(&psf2).unwrap();
        assert_eq!((f.glyphs, f.bytes_per_glyph, f.height, f.width), (256, 2, 2, 8));
        assert_eq!(f.glyph(0x41), [0x41, 0xbe]);
        assert_eq!(u32_at(&psf2, 12), PSF2_HAS_UNICODE_TABLE);
        let table = &psf2[32 + 512..];
        // Glyph 0x41's entry: 'A', the sequence 'A' U+030A in UTF-8, the end.
        let at = table
            .iter()
            .enumerate()
            .filter(|(_, &b)| b == 0xff)
            .nth(0x40)
            .unwrap()
            .0
            + 1;
        assert_eq!(&table[at..at + 6], [0x41, 0xfe, 0x41, 0xcc, 0x8a, 0xff]);
        // ASCII where it isn't: refused.
        let mut bad = psf1.clone();
        let t = 4 + 512;
        bad[t + 4 * 0x20] = 0x21; // glyph 0x20 is '!' (each entry a code point and the end)
        assert!(psf1_to_psf2(&bad).unwrap_err().contains("' ' is not glyph 32"));
    }

    #[test]
    fn spleen_converts() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let psf2 = psf2(&root).unwrap();
        let f = parse_psf2(&psf2).unwrap();
        assert_eq!((f.glyphs, f.bytes_per_glyph, f.height, f.width), (512, 16, 16, 8));
        // 'A': the rows of its glyph have pixels; ' ' has none.
        assert!(f.glyph(u32::from(b'A')).iter().any(|&r| r != 0));
        assert!(f.glyph(u32::from(b' ')).iter().all(|&r| r == 0));
    }

    #[test]
    fn the_committed_font_c_is_current() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        for (file, want) in files(&root).unwrap() {
            let path = root.join(file);
            let committed = std::fs::read_to_string(&path)
                .unwrap_or_default()
                .replace('\r', "");
            assert!(
                committed == want,
                "{} is out of date: run `cargo xtask kernel` and commit it",
                path.display()
            );
        }
    }
}
