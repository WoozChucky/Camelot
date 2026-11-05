//! MD5 (RFC 1321), the checksum DWARF 5 line tables give each source file
//! (`DW_LNCT_MD5`): with `-g`, merlin writes every file's as clang does,
//! `.file N "dir" "name" md5 0x...`, and merlin-as copies it into
//! `.debug_line`. A debugger can tell from it whether the source it shows is
//! the one the program was compiled from. (MD5 is no longer a secure hash;
//! here it only has to notice a file that changed, which it does.)
//!
//! The message is read in 64-byte blocks, each mixed into a 128-bit state
//! of four 32-bit words `a`, `b`, `c`, `d`, all little-endian:
//!
//! 1. *Padding* (RFC 1321, 3.1 and 3.2): a 1 bit (the byte `0x80`), then
//!    zeros up to 56 bytes into the last block, then the message's length
//!    in bits as a 64-bit number. So the padded message is a whole number
//!    of blocks, and messages that differ only in trailing zeros or in
//!    length do not pad to the same bytes.
//! 2. *The state* starts at the four words of section 3.3.
//! 3. *Each block* (3.4) is 16 words `x[0..16]`, taken in 64 steps, four
//!    rounds of 16. Every step adds to one word of the state a function of
//!    the other three, a word of the block and a constant, rotates it left
//!    and adds the next word:
//!
//!    ```text
//!    a = b + ((a + f(b, c, d) + x[k] + T[i]) <<< s)
//!    ```
//!
//!    The round decides `f` and the order the words `k` are read in; `T[i]`
//!    and the rotations `s` are fixed tables. After the 64 steps, the state
//!    before the block is added back in.
//! 4. *The digest* (3.5) is the state's four words, little-endian: 16 bytes,
//!    written as 32 hex digits (`d41d8cd98f00b204e9800998ecf8427e` for the
//!    empty message).

/// `T[i]`, the constant of step `i`: the integer part of
/// `4294967296 * abs(sin(i + 1))`, `i + 1` in radians (RFC 1321, 3.4). As a
/// table, since a floating-point `sin` need not round the same everywhere.
const T: [u32; 64] = [
    0xd76a_a478,
    0xe8c7_b756,
    0x2420_70db,
    0xc1bd_ceee,
    0xf57c_0faf,
    0x4787_c62a,
    0xa830_4613,
    0xfd46_9501,
    0x6980_98d8,
    0x8b44_f7af,
    0xffff_5bb1,
    0x895c_d7be,
    0x6b90_1122,
    0xfd98_7193,
    0xa679_438e,
    0x49b4_0821,
    0xf61e_2562,
    0xc040_b340,
    0x265e_5a51,
    0xe9b6_c7aa,
    0xd62f_105d,
    0x0244_1453,
    0xd8a1_e681,
    0xe7d3_fbc8,
    0x21e1_cde6,
    0xc337_07d6,
    0xf4d5_0d87,
    0x455a_14ed,
    0xa9e3_e905,
    0xfcef_a3f8,
    0x676f_02d9,
    0x8d2a_4c8a,
    0xfffa_3942,
    0x8771_f681,
    0x6d9d_6122,
    0xfde5_380c,
    0xa4be_ea44,
    0x4bde_cfa9,
    0xf6bb_4b60,
    0xbebf_bc70,
    0x289b_7ec6,
    0xeaa1_27fa,
    0xd4ef_3085,
    0x0488_1d05,
    0xd9d4_d039,
    0xe6db_99e5,
    0x1fa2_7cf8,
    0xc4ac_5665,
    0xf429_2244,
    0x432a_ff97,
    0xab94_23a7,
    0xfc93_a039,
    0x655b_59c3,
    0x8f0c_cc92,
    0xffef_f47d,
    0x8584_5dd1,
    0x6fa8_7e4f,
    0xfe2c_e6e0,
    0xa301_4314,
    0x4e08_11a1,
    0xf753_7e82,
    0xbd3a_f235,
    0x2ad7_d2bb,
    0xeb86_d391,
];

/// The left rotations of each round's steps, which repeat every four steps.
const S: [[u32; 4]; 4] = [[7, 12, 17, 22], [5, 9, 14, 20], [4, 11, 16, 23], [6, 10, 15, 21]];

/// The MD5 digest of `data`: 16 bytes, in the order they are written.
pub fn md5(data: &[u8]) -> [u8; 16] {
    // 3.3: the initial state, the words 01 23 45 67, 89 ab cd ef, fe dc ba
    // 98 and 76 54 32 10 as the RFC writes them, low byte first.
    let mut state: [u32; 4] = [0x6745_2301, 0xefcd_ab89, 0x98ba_dcfe, 0x1032_5476];

    // 3.1 and 3.2: the padding and the length. Whole blocks are taken from
    // the message itself; the rest, padded, makes one or two more.
    let (blocks, rest) = data.as_chunks::<64>();
    for block in blocks {
        mix(&mut state, block);
    }
    let mut tail = rest.to_vec();
    tail.push(0x80);
    while tail.len() % 64 != 56 {
        tail.push(0);
    }
    // The length in bits, modulo 2^64 as the RFC says.
    tail.extend_from_slice(&(data.len() as u64).wrapping_mul(8).to_le_bytes());
    for block in tail.as_chunks::<64>().0 {
        mix(&mut state, block);
    }

    // 3.5: the state's words, little-endian.
    let mut digest = [0; 16];
    for (out, word) in digest.as_chunks_mut::<4>().0.iter_mut().zip(state) {
        *out = word.to_le_bytes();
    }
    digest
}

/// 3.4: mixes one 64-byte block into the state.
fn mix(state: &mut [u32; 4], block: &[u8; 64]) {
    let mut x = [0u32; 16];
    for (w, b) in x.iter_mut().zip(block.as_chunks::<4>().0) {
        *w = u32::from_le_bytes(*b);
    }
    let [mut a, mut b, mut c, mut d] = *state;
    for i in 0..64 {
        let round = i / 16;
        // Each round's function of b, c and d, bit by bit, and the word of
        // the block it reads at step i:
        let (f, k) = match round {
            // F: if b then c else d; the words in order.
            0 => ((b & c) | (!b & d), i),
            // G: if d then b else c; from word 1, every fifth.
            1 => ((b & d) | (c & !d), (5 * i + 1) % 16),
            // H: the parity of the three; from word 5, every third.
            2 => (b ^ c ^ d, (3 * i + 5) % 16),
            // I: c xor (b or not d); every seventh.
            _ => (c ^ (b | !d), (7 * i) % 16),
        };
        let sum = a.wrapping_add(f).wrapping_add(x[k]).wrapping_add(T[i]);
        let rotated = b.wrapping_add(sum.rotate_left(S[round][i % 4]));
        // The words move one place on (a <- d <- c <- b <- the new word), so
        // that the next step updates the next word, as the RFC's steps
        // `[abcd ...]`, `[dabc ...]`, `[cdab ...]`, `[bcda ...]` do.
        (a, b, c, d) = (d, rotated, b, c);
    }
    for (s, v) in state.iter_mut().zip([a, b, c, d]) {
        *s = s.wrapping_add(v);
    }
}

/// `digest` as 32 lowercase hex digits, the first byte first: how clang
/// writes it after `md5 0x` (LLVM's `MD5Result::digest`).
pub fn hex(digest: &[u8; 16]) -> String {
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sum(data: &[u8]) -> String {
        hex(&md5(data))
    }

    /// The test suite of RFC 1321, appendix A.5.
    #[test]
    fn rfc_1321_test_suite() {
        assert_eq!(sum(b""), "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(sum(b"a"), "0cc175b9c0f1b6a831c399e269772661");
        assert_eq!(sum(b"abc"), "900150983cd24fb0d6963f7d28e17f72");
        assert_eq!(sum(b"message digest"), "f96b697d7cb7938d525a2f31aaf161d0");
        assert_eq!(
            sum(b"abcdefghijklmnopqrstuvwxyz"),
            "c3fcd3d76192e4007dfb496cca67e13b"
        );
        assert_eq!(
            sum(b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789"),
            "d174ab98d277d9f5a5611c2c9f419d9f"
        );
        assert_eq!(
            sum(b"12345678901234567890123456789012345678901234567890123456789012345678901234567890"),
            "57edf4a22be3c955ac49da2e2107b67a"
        );
    }

    /// Lengths around the block and the padding: 55 bytes pad within their
    /// block, 56 need another; 64 is a whole block, padded with a block of
    /// its own. The digests are .NET's `MD5.ComputeHash` of as many `a`s.
    #[test]
    fn padding_at_the_block_boundaries() {
        for (n, want) in [
            (55, "ef1772b6dff9a122358552954ad0df65"),
            (56, "3b0c8ac703f828b04c6c197006d17218"),
            (63, "b06521f39153d618550606be297466d5"),
            (64, "014842d480b571495a4a0363793f7367"),
            (65, "c743a45e0d2e6a95cb859adae0248435"),
            (119, "8a7bd0732ed6a28ce75f6dabc90e1613"),
            (120, "5f61c0ccad4cac44c75ff505e1f1e537"),
            (1000, "cabe45dcc9ae5b66ba86600cca6b8ba8"),
        ] {
            assert_eq!(sum(&vec![b'a'; n]), want, "{n} bytes");
        }
    }
}
