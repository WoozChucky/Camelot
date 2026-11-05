//! SHA-1 (FIPS 180-4, section 6.1), for one thing only: the WebSocket handshake's
//! `Sec-WebSocket-Accept`, which RFC 6455 section 4.2.2 defines as the base64 of the
//! SHA-1 of the client's key and a fixed GUID. SHA-1 is broken for collisions, and
//! nothing here relies on it resisting them: the accept value only shows that the
//! server read the handshake, it secures nothing.

/// The 20-byte digest of `data`.
pub fn digest(data: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [0x6745_2301, 0xefcd_ab89, 0x98ba_dcfe, 0x1032_5476, 0xc3d2_e1f0];
    // Padding (section 5.1.1): a 1 bit, zeros up to 56 bytes past a multiple of 64,
    // then the message's length in bits as a big-endian u64.
    let mut message = data.to_vec();
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&((data.len() as u64).wrapping_mul(8)).to_be_bytes());

    for block in message.as_chunks::<64>().0 {
        // The message schedule (section 6.1.2, step 1): 16 words of the block, then
        // 64 more, each the XOR of four earlier ones rotated left by one.
        let mut w = [0u32; 80];
        for (t, word) in block.as_chunks::<4>().0.iter().enumerate() {
            w[t] = u32::from_be_bytes(*word);
        }
        for t in 16..80 {
            w[t] = (w[t - 3] ^ w[t - 8] ^ w[t - 14] ^ w[t - 16]).rotate_left(1);
        }
        let [mut a, mut b, mut c, mut d, mut e] = h;
        for (t, &wt) in w.iter().enumerate() {
            // The round function and constant change every 20 rounds (section 4.1.1
            // and 4.2.1): Ch, Parity, Maj, Parity.
            let (f, k) = match t {
                0..=19 => ((b & c) | (!b & d), 0x5a82_7999),
                20..=39 => (b ^ c ^ d, 0x6ed9_eba1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8f1b_bcdc),
                _ => (b ^ c ^ d, 0xca62_c1d6),
            };
            let temp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(wt);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = temp;
        }
        for (hi, v) in h.iter_mut().zip([a, b, c, d, e]) {
            *hi = hi.wrapping_add(v);
        }
    }
    let mut out = [0u8; 20];
    for (chunk, word) in out.as_chunks_mut::<4>().0.iter_mut().zip(h) {
        *chunk = word.to_be_bytes();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    /// The test vectors of FIPS 180-2's appendix A (and RFC 3174), the empty message,
    /// and messages around the padding's edges (55, 56 and 64 bytes need one, two and
    /// two blocks), checked against `sha1sum`.
    #[test]
    fn known_digests() {
        for (input, want) in [
            (&b"abc"[..], "a9993e364706816aba3e25717850c26c9cd0d89d"),
            (b"", "da39a3ee5e6b4b0d3255bfef95601890afd80709"),
            (
                b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq",
                "84983e441c3bd26ebaae4aa1f95129e5e54670f1",
            ),
            (&[b'a'; 55][..], "c1c8bbdc22796e28c0e15163d20899b65621d65a"),
            (&[b'a'; 56][..], "c2db330f6083854c99d4b5bfb6e8f29f201be699"),
            (&[b'a'; 64][..], "0098ba824b5c16427bd7a1122a5a442a25ec644d"),
        ] {
            assert_eq!(hex(&digest(input)), want, "{}", String::from_utf8_lossy(input));
        }
        let million = vec![b'a'; 1_000_000];
        assert_eq!(hex(&digest(&million)), "34aa973cd4c4daa4f61eeb2bdbad27316534016f");
    }
}
