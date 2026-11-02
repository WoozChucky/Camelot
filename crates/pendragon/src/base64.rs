//! Base64 (RFC 4648, the standard alphabet with `=` padding): how bytes travel inside
//! the API's JSON, a program image going up and console output coming down.
//! The decoder is strict: no whitespace, no missing or extra padding, and no stray
//! bits in the last character, so every text has one meaning.

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, &b)| n | (b as u32) << (16 - 8 * i));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

pub fn decode(text: &str) -> Result<Vec<u8>, String> {
    let bad = |why: &str| format!("not base64: {why}");
    let bytes = text.as_bytes();
    if !bytes.len().is_multiple_of(4) {
        return Err(bad("the length is not a multiple of 4"));
    }
    let value = |c: u8| ALPHABET.iter().position(|&a| a == c).map(|v| v as u32);
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    let quads = bytes.len() / 4;
    for (q, quad) in bytes.chunks(4).enumerate() {
        let pad = quad.iter().rev().take_while(|&&c| c == b'=').count();
        if pad > 2 || (pad > 0 && q + 1 != quads) {
            return Err(bad("'=' before the end"));
        }
        let mut n = 0u32;
        for (i, &c) in quad[..4 - pad].iter().enumerate() {
            let v = value(c).ok_or_else(|| bad("a character outside the alphabet"))?;
            n |= v << (18 - 6 * i);
        }
        let keep = 3 - pad;
        // The bits past the last whole byte must be zero.
        if n & (0xff_ffff >> (8 * keep)) != 0 {
            return Err(bad("stray bits in the last character"));
        }
        out.extend_from_slice(&n.to_be_bytes()[1..1 + keep]);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc_4648_vectors() {
        for (plain, coded) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(encode(plain.as_bytes()), coded);
            assert_eq!(decode(coded).unwrap(), plain.as_bytes());
        }
    }

    #[test]
    fn every_byte_round_trips() {
        let all: Vec<u8> = (0..=255).collect();
        for len in 0..all.len() {
            assert_eq!(decode(&encode(&all[..len])).unwrap(), &all[..len]);
        }
        assert_eq!(encode(&[0xfb, 0xff]), "+/8=");
    }

    #[test]
    fn strict() {
        for bad in [
            "Z", "Zg", "Zg=", "Zg===", "Z===", "====", "Zm9v\n", " Zm9v", "Zm9v====", "Zg==Zm9v", "Zm=v",
            "Zh==", "Zm9=", "Zm-v", "Zm_v", "Zm9vé",
        ] {
            assert!(decode(bad).is_err(), "{bad:?}");
        }
    }
}
