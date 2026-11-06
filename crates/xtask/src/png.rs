//! PNG files of RGB images, for the GUI library's expected images
//! (`crates/xtask/tests/gui/*.png`, #183): a format every viewer shows, a
//! pull request's diff included. [`encode`] is deterministic, so a test
//! compares an image by encoding it and comparing the bytes with the
//! committed file; [`decode`] reads back what it writes.
//!
//! A PNG (RFC 2083, the W3C's PNG specification) is a signature, then
//! chunks, each its length, its type, its data and a CRC-32 of type and
//! data: IHDR (the size, 8 bits a sample, colour type 2: RGB), IDAT (the
//! rows, each after a filter byte, 0 here, compressed with zlib) and IEND.
//! The zlib stream (RFC 1950) is a two-byte header, deflate (RFC 1951) and
//! the Adler-32 of the data. The deflate here is one block with the fixed
//! Huffman codes, and LZ77 matches found through a hash of the next three
//! bytes: the images are mostly flat colour, which that shrinks a
//! hundredfold.

/// The 8 bytes every PNG starts with.
const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n'];

/// CRC-32 (the polynomial 0xedb88320, reflected), as PNG and zip use it.
fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xedb8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for &x in data {
        a = (a + x as u32) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}

/// Writes bits least significant first, as deflate packs them.
struct Bits {
    out: Vec<u8>,
    acc: u64,
    n: u32,
}

impl Bits {
    fn put(&mut self, value: u32, count: u32) {
        self.acc |= (value as u64) << self.n;
        self.n += count;
        while self.n >= 8 {
            self.out.push(self.acc as u8);
            self.acc >>= 8;
            self.n -= 8;
        }
    }

    /// A Huffman code, which deflate stores most significant bit first.
    fn code(&mut self, code: u32, len: u32) {
        let reversed = code.reverse_bits() >> (32 - len);
        self.put(reversed, len);
    }

    /// A literal or length symbol's fixed code (RFC 1951, 3.2.6).
    fn symbol(&mut self, sym: u32) {
        match sym {
            0..=143 => self.code(0x30 + sym, 8),
            144..=255 => self.code(0x190 + sym - 144, 9),
            256..=279 => self.code(sym - 256, 7),
            _ => self.code(0xc0 + sym - 280, 8),
        }
    }

    fn finish(mut self) -> Vec<u8> {
        if self.n > 0 {
            self.out.push(self.acc as u8);
        }
        self.out
    }
}

/// Deflate's length codes: the first length of each, and its extra bits.
const LENGTHS: [(u32, u32); 29] = [
    (3, 0),
    (4, 0),
    (5, 0),
    (6, 0),
    (7, 0),
    (8, 0),
    (9, 0),
    (10, 0),
    (11, 1),
    (13, 1),
    (15, 1),
    (17, 1),
    (19, 2),
    (23, 2),
    (27, 2),
    (31, 2),
    (35, 3),
    (43, 3),
    (51, 3),
    (59, 3),
    (67, 4),
    (83, 4),
    (99, 4),
    (115, 4),
    (131, 5),
    (163, 5),
    (195, 5),
    (227, 5),
    (258, 0),
];
/// Its distance codes, the same way.
const DISTANCES: [(u32, u32); 30] = [
    (1, 0),
    (2, 0),
    (3, 0),
    (4, 0),
    (5, 1),
    (7, 1),
    (9, 2),
    (13, 2),
    (17, 3),
    (25, 3),
    (33, 4),
    (49, 4),
    (65, 5),
    (97, 5),
    (129, 6),
    (193, 6),
    (257, 7),
    (385, 7),
    (513, 8),
    (769, 8),
    (1025, 9),
    (1537, 9),
    (2049, 10),
    (3073, 10),
    (4097, 11),
    (6145, 11),
    (8193, 12),
    (12289, 12),
    (16385, 13),
    (24577, 13),
];

/// The code of `value` in `table` and its extra bits' value.
fn find(table: &[(u32, u32)], value: u32) -> (usize, u32, u32) {
    let i = table.iter().rposition(|&(base, _)| base <= value).unwrap();
    (i, value - table[i].0, table[i].1)
}

/// `data` deflated: one final block, fixed codes.
fn deflate(data: &[u8]) -> Vec<u8> {
    const WINDOW: usize = 32768;
    const MAX_MATCH: usize = 258;
    let mut bits = Bits {
        out: Vec::new(),
        acc: 0,
        n: 0,
    };
    bits.put(1, 1); // BFINAL
    bits.put(1, 2); // BTYPE 01: fixed Huffman codes
    let mut last: std::collections::HashMap<[u8; 3], usize> = std::collections::HashMap::new();
    let mut i = 0;
    while i < data.len() {
        let mut best = (0, 0);
        if i + 3 <= data.len() {
            let key = [data[i], data[i + 1], data[i + 2]];
            if let Some(&at) = last.get(&key).filter(|&&at| i - at <= WINDOW) {
                let len = data[i..]
                    .iter()
                    .zip(&data[at..])
                    .take(MAX_MATCH)
                    .take_while(|(a, b)| a == b)
                    .count();
                best = (len, i - at);
            }
            last.insert(key, i);
        }
        if best.0 >= 3 {
            let (len, dist) = best;
            let (code, extra, nbits) = find(&LENGTHS, len as u32);
            bits.symbol(257 + code as u32);
            bits.put(extra, nbits);
            let (code, extra, nbits) = find(&DISTANCES, dist as u32);
            bits.code(code as u32, 5);
            bits.put(extra, nbits);
            for j in i + 1..(i + len).min(data.len().saturating_sub(2)) {
                last.insert([data[j], data[j + 1], data[j + 2]], j);
            }
            i += len;
        } else {
            bits.symbol(data[i] as u32);
            i += 1;
        }
    }
    bits.symbol(256);
    bits.finish()
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend((data.len() as u32).to_be_bytes());
    let start = out.len();
    out.extend(kind);
    out.extend(data);
    let crc = crc32(&out[start..]);
    out.extend(crc.to_be_bytes());
}

/// A PNG of `rgb`, `width` x `height` pixels of three bytes.
pub fn encode(width: u32, height: u32, rgb: &[u8]) -> Vec<u8> {
    assert_eq!(rgb.len(), (width * height * 3) as usize);
    let mut raw = Vec::with_capacity(rgb.len() + height as usize);
    for row in rgb.chunks(width as usize * 3) {
        raw.push(0); // filter: none
        raw.extend(row);
    }
    let mut z = vec![0x78, 0x01]; // deflate, a 32 KiB window, the lowest level
    z.extend(deflate(&raw));
    z.extend(adler32(&raw).to_be_bytes());
    let mut ihdr = Vec::new();
    ihdr.extend(width.to_be_bytes());
    ihdr.extend(height.to_be_bytes());
    ihdr.extend([8, 2, 0, 0, 0]);
    let mut out = SIGNATURE.to_vec();
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &z);
    chunk(&mut out, b"IEND", &[]);
    out
}

/// Reads bits least significant first.
struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl Reader<'_> {
    fn bit(&mut self) -> Result<u32, String> {
        let byte = *self.data.get(self.pos / 8).ok_or("deflate data cut short")?;
        let b = (byte >> (self.pos % 8)) & 1;
        self.pos += 1;
        Ok(b as u32)
    }

    fn bits(&mut self, n: u32) -> Result<u32, String> {
        let mut v = 0;
        for i in 0..n {
            v |= self.bit()? << i;
        }
        Ok(v)
    }

    /// A fixed literal or length symbol.
    fn symbol(&mut self) -> Result<u32, String> {
        let mut code = 0;
        for len in 1..=9 {
            code = (code << 1) | self.bit()?;
            match (len, code) {
                (7, 0..=23) => return Ok(code + 256),
                (8, 0x30..=0xbf) => return Ok(code - 0x30),
                (8, 0xc0..=0xc7) => return Ok(code - 0xc0 + 280),
                (9, 0x190..=0x1ff) => return Ok(code - 0x190 + 144),
                _ => {}
            }
        }
        Err("a bad code".into())
    }
}

/// What [`deflate`] wrote, inflated (fixed-code blocks only).
fn inflate(data: &[u8]) -> Result<Vec<u8>, String> {
    let mut r = Reader { data, pos: 0 };
    let mut out: Vec<u8> = Vec::new();
    loop {
        let last = r.bit()?;
        if r.bits(2)? != 1 {
            return Err("not a fixed-code block".into());
        }
        loop {
            let sym = r.symbol()?;
            match sym {
                0..=255 => out.push(sym as u8),
                256 => break,
                _ => {
                    let (base, n) = *LENGTHS.get(sym as usize - 257).ok_or("a bad length")?;
                    let len = base + r.bits(n)?;
                    let mut code = 0;
                    for _ in 0..5 {
                        code = (code << 1) | r.bit()?;
                    }
                    let (base, n) = *DISTANCES.get(code as usize).ok_or("a bad distance")?;
                    let dist = (base + r.bits(n)?) as usize;
                    if dist > out.len() {
                        return Err("a distance too far back".into());
                    }
                    for _ in 0..len {
                        out.push(out[out.len() - dist]);
                    }
                }
            }
        }
        if last == 1 {
            return Ok(out);
        }
    }
}

/// Reads a PNG [`encode`] wrote: the width, the height and the RGB bytes.
pub fn decode(png: &[u8]) -> Result<(u32, u32, Vec<u8>), String> {
    if png.len() < 8 || png[..8] != SIGNATURE {
        return Err("not a PNG".into());
    }
    let (mut at, mut size, mut idat) = (8, None, Vec::new());
    while at + 12 <= png.len() {
        let len = u32::from_be_bytes(png[at..at + 4].try_into().unwrap()) as usize;
        let kind = &png[at + 4..at + 8];
        let data = png.get(at + 8..at + 8 + len).ok_or("a chunk cut short")?;
        match kind {
            b"IHDR" => {
                if data.len() != 13 || data[8..] != [8, 2, 0, 0, 0] {
                    return Err("not 8-bit RGB".into());
                }
                size = Some((
                    u32::from_be_bytes(data[..4].try_into().unwrap()),
                    u32::from_be_bytes(data[4..8].try_into().unwrap()),
                ));
            }
            b"IDAT" => idat.extend(data),
            _ => {}
        }
        at += 12 + len;
    }
    let (w, h) = size.ok_or("no IHDR")?;
    let raw = inflate(idat.get(2..idat.len().saturating_sub(4)).ok_or("no IDAT")?)?;
    let stride = w as usize * 3 + 1;
    if raw.len() != stride * h as usize {
        return Err("the wrong size".into());
    }
    let mut rgb = Vec::with_capacity(stride * h as usize);
    for row in raw.chunks(stride) {
        if row[0] != 0 {
            return Err("a filter other than none".into());
        }
        rgb.extend(&row[1..]);
    }
    Ok((w, h, rgb))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksums_are_the_standard_ones() {
        assert_eq!(crc32(b"IEND"), 0xae42_6082);
        assert_eq!(adler32(b"Wikipedia"), 0x11e6_0398);
    }

    #[test]
    fn an_image_reads_back_as_it_was_written() {
        let (w, h) = (37, 23);
        let rgb: Vec<u8> = (0..w * h * 3)
            .map(|i| if i % 97 < 60 { 7 } else { (i * 31 % 251) as u8 })
            .collect();
        let png = encode(w, h, &rgb);
        assert_eq!(decode(&png).unwrap(), (w, h, rgb.clone()));
        // Flat colour shrinks.
        let flat = vec![0x40; (w * h * 3) as usize];
        assert!(encode(w, h, &flat).len() < flat.len() / 20);
    }
}
