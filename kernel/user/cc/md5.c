/* cc1: MD5 (RFC 1321), the checksum DWARF 5 line tables give each source
 * file (DW_LNCT_MD5), follows merlin's md5.rs. With -g, cc1 writes every
 * file's as clang does, `.file N "dir" "name" md5 0x...`, and the assembler
 * copies it into .debug_line: a debugger can tell from it whether the
 * source it shows is the one the program was compiled from. (MD5 is no
 * longer a secure hash; here it only has to notice a file that changed,
 * which it does.)
 *
 * The message is read in 64-byte blocks, each mixed into a 128-bit state
 * of four 32-bit words a, b, c, d, all little-endian:
 *
 * 1. Padding (RFC 1321, 3.1 and 3.2): a 1 bit (the byte 0x80), then zeros
 *    up to 56 bytes into the last block, then the message's length in bits
 *    as a 64-bit number. So the padded message is a whole number of
 *    blocks, and messages that differ only in trailing zeros or in length
 *    do not pad to the same bytes.
 * 2. The state starts at the four words of section 3.3.
 * 3. Each block (3.4) is 16 words x[0..16], taken in 64 steps, four rounds
 *    of 16. Every step adds to one word of the state a function of the
 *    other three, a word of the block and a constant, rotates it left and
 *    adds the next word:
 *
 *        a = b + ((a + f(b, c, d) + x[k] + T[i]) <<< s)
 *
 *    The round decides f and the order the words k are read in; T[i] and
 *    the rotations s are fixed tables. After the 64 steps, the state
 *    before the block is added back in.
 * 4. The digest (3.5) is the state's four words, little-endian: 16 bytes,
 *    written as 32 hex digits (d41d8cd98f00b204e9800998ecf8427e for the
 *    empty message). */
#include "cc.h"

/* T[i], the constant of step i: the integer part of 4294967296 *
 * abs(sin(i + 1)), i + 1 in radians (RFC 1321, 3.4). As a table, since a
 * floating-point sin need not round the same everywhere (and cc1 has no
 * floating point). */
static const uint T[64] = {
    0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee, 0xf57c0faf, 0x4787c62a, 0xa8304613, 0xfd469501,
    0x698098d8, 0x8b44f7af, 0xffff5bb1, 0x895cd7be, 0x6b901122, 0xfd987193, 0xa679438e, 0x49b40821,
    0xf61e2562, 0xc040b340, 0x265e5a51, 0xe9b6c7aa, 0xd62f105d, 0x02441453, 0xd8a1e681, 0xe7d3fbc8,
    0x21e1cde6, 0xc33707d6, 0xf4d50d87, 0x455a14ed, 0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a,
    0xfffa3942, 0x8771f681, 0x6d9d6122, 0xfde5380c, 0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70,
    0x289b7ec6, 0xeaa127fa, 0xd4ef3085, 0x04881d05, 0xd9d4d039, 0xe6db99e5, 0x1fa27cf8, 0xc4ac5665,
    0xf4292244, 0x432aff97, 0xab9423a7, 0xfc93a039, 0x655b59c3, 0x8f0ccc92, 0xffeff47d, 0x85845dd1,
    0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1, 0xf7537e82, 0xbd3af235, 0x2ad7d2bb, 0xeb86d391,
};

/* The left rotations of each round's steps, which repeat every four
 * steps. */
static const uint S[4][4] = {{7, 12, 17, 22}, {5, 9, 14, 20}, {4, 11, 16, 23}, {6, 10, 15, 21}};

/* A 32-bit word rotated left by n (0 < n < 32). The words are uints, whose
 * arithmetic wraps modulo 2^32 as Rust's wrapping_add does. */
static uint rotate_left(uint v, uint n) {
    return (v << n) | (v >> (32 - n));
}

/* 3.4: mixes one 64-byte block into the state. */
static void mix(uint state[4], const uchar *block) {
    uint x[16];
    for (uint i = 0; i < 16; i++) {
        const uchar *b = block + 4 * i;
        x[i] = (uint)b[0] | (uint)b[1] << 8 | (uint)b[2] << 16 | (uint)b[3] << 24;
    }
    uint a = state[0];
    uint b = state[1];
    uint c = state[2];
    uint d = state[3];
    for (uint i = 0; i < 64; i++) {
        uint round = i / 16;
        /* Each round's function of b, c and d, bit by bit, and the word of
         * the block it reads at step i: */
        uint f;
        uint k;
        if (round == 0) {
            /* F: if b then c else d; the words in order. */
            f = (b & c) | (~b & d);
            k = i;
        } else if (round == 1) {
            /* G: if d then b else c; from word 1, every fifth. */
            f = (b & d) | (c & ~d);
            k = (5 * i + 1) % 16;
        } else if (round == 2) {
            /* H: the parity of the three; from word 5, every third. */
            f = b ^ c ^ d;
            k = (3 * i + 5) % 16;
        } else {
            /* I: c xor (b or not d); every seventh. */
            f = c ^ (b | ~d);
            k = (7 * i) % 16;
        }
        uint sum = a + f + x[k] + T[i];
        uint rotated = b + rotate_left(sum, S[round][i % 4]);
        /* The words move one place on (a <- d <- c <- b <- the new word), so
         * that the next step updates the next word, as the RFC's steps
         * [abcd ...], [dabc ...], [cdab ...], [bcda ...] do. */
        a = d;
        d = c;
        c = b;
        b = rotated;
    }
    state[0] += a;
    state[1] += b;
    state[2] += c;
    state[3] += d;
}

void md5(const uchar *data, uint n, uchar digest[16]) {
    /* 3.3: the initial state, the words 01 23 45 67, 89 ab cd ef, fe dc ba
     * 98 and 76 54 32 10 as the RFC writes them, low byte first. */
    uint state[4] = {0x67452301, 0xefcdab89, 0x98badcfe, 0x10325476};

    /* 3.1 and 3.2: the padding and the length. Whole blocks are taken from
     * the message itself; the rest, padded, makes one or two more. */
    uint whole = n / 64 * 64;
    for (uint at = 0; at < whole; at += 64) {
        mix(state, data + at);
    }
    uchar tail[128];
    uint rest = n - whole;
    memcpy(tail, data + whole, rest);
    uint len = rest;
    tail[len++] = 0x80;
    while (len % 64 != 56) {
        tail[len++] = 0;
    }
    /* The length in bits, modulo 2^64 as the RFC says. */
    uint64_t bits = (uint64_t)n * 8;
    for (uint i = 0; i < 8; i++) {
        tail[len++] = (uchar)(bits >> (8 * i));
    }
    for (uint at = 0; at < len; at += 64) {
        mix(state, tail + at);
    }

    /* 3.5: the state's words, little-endian. */
    for (uint i = 0; i < 4; i++) {
        for (uint k = 0; k < 4; k++) {
            digest[4 * i + k] = (uchar)(state[i] >> (8 * k));
        }
    }
}

void md5_hex(const uchar digest[16], char out[33]) {
    static const char HEX[] = "0123456789abcdef";
    for (uint i = 0; i < 16; i++) {
        out[2 * i] = HEX[digest[i] >> 4];
        out[2 * i + 1] = HEX[digest[i] & 15];
    }
    out[32] = 0;
}
