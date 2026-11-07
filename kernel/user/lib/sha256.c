/* SHA-256 (FIPS 180-4) and the password hash Camelot's accounts use
 * (docs/excalibur-spec.md, "Users"). This is a teaching system's: a salted
 * SHA-256 iterated PW_ROUNDS times, easy to read and to check by hand, not
 * hardened cryptography. A real system uses a slow, memory-hard function
 * (yescrypt, bcrypt, Argon2) and compares hashes in constant time.
 *
 * SHA-256 works on 64-byte blocks: the message, a 1 bit, zeros, and the
 * message's length in bits as a big-endian 64-bit number, padded to a whole
 * number of blocks. Each block goes through 64 rounds that mix it into
 * eight 32-bit words of state, which start as the fractional parts of the
 * square roots of the first eight primes; the round constants K are those
 * of the cube roots of the first 64. The digest is the final state, big
 * endian. */
#include "../user.h"

/* The first 32 bits of the fractional parts of the cube roots of the first
 * 64 primes. */
static const uint32_t K[64] = {
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
};

static uint32_t ror(uint32_t x, int n) {
    return (x >> n) | (x << (32 - n));
}

/* Mixes the 64-byte block at s->buf into the state. */
static void compress(struct sha256 *s) {
    /* The message schedule: the block's 16 big-endian words, then 48 more
     * made from them. */
    uint32_t w[64];
    for (int i = 0; i < 16; i++) {
        const uint8_t *p = s->buf + 4 * i;
        w[i] = (uint32_t)p[0] << 24 | (uint32_t)p[1] << 16 | (uint32_t)p[2] << 8 | p[3];
    }
    for (int i = 16; i < 64; i++) {
        uint32_t s0 = ror(w[i - 15], 7) ^ ror(w[i - 15], 18) ^ (w[i - 15] >> 3);
        uint32_t s1 = ror(w[i - 2], 17) ^ ror(w[i - 2], 19) ^ (w[i - 2] >> 10);
        w[i] = w[i - 16] + s0 + w[i - 7] + s1;
    }
    uint32_t a = s->h[0];
    uint32_t b = s->h[1];
    uint32_t c = s->h[2];
    uint32_t d = s->h[3];
    uint32_t e = s->h[4];
    uint32_t f = s->h[5];
    uint32_t g = s->h[6];
    uint32_t h = s->h[7];
    for (int i = 0; i < 64; i++) {
        uint32_t S1 = ror(e, 6) ^ ror(e, 11) ^ ror(e, 25);
        uint32_t ch = (e & f) ^ (~e & g);
        uint32_t t1 = h + S1 + ch + K[i] + w[i];
        uint32_t S0 = ror(a, 2) ^ ror(a, 13) ^ ror(a, 22);
        uint32_t maj = (a & b) ^ (a & c) ^ (b & c);
        uint32_t t2 = S0 + maj;
        h = g;
        g = f;
        f = e;
        e = d + t1;
        d = c;
        c = b;
        b = a;
        a = t1 + t2;
    }
    s->h[0] += a;
    s->h[1] += b;
    s->h[2] += c;
    s->h[3] += d;
    s->h[4] += e;
    s->h[5] += f;
    s->h[6] += g;
    s->h[7] += h;
}

void sha256_init(struct sha256 *s) {
    /* The fractional parts of the square roots of the first 8 primes. */
    s->h[0] = 0x6a09e667;
    s->h[1] = 0xbb67ae85;
    s->h[2] = 0x3c6ef372;
    s->h[3] = 0xa54ff53a;
    s->h[4] = 0x510e527f;
    s->h[5] = 0x9b05688c;
    s->h[6] = 0x1f83d9ab;
    s->h[7] = 0x5be0cd19;
    s->len = 0;
    s->n = 0;
}

void sha256_update(struct sha256 *s, const void *data, size_t len) {
    const uint8_t *p = data;
    s->len += len;
    while (len > 0) {
        size_t take = 64 - s->n;
        if (take > len) {
            take = len;
        }
        memcpy(s->buf + s->n, p, take);
        s->n += (uint32_t)take;
        p += take;
        len -= take;
        if (s->n == 64) {
            compress(s);
            s->n = 0;
        }
    }
}

void sha256_final(struct sha256 *s, uint8_t digest[SHA256_SIZE]) {
    uint64_t bits = s->len * 8;
    /* The 1 bit, then zeros up to 8 bytes short of a block's end (a block
     * more if the length doesn't fit after the 1), then the length. */
    s->buf[s->n++] = 0x80;
    if (s->n > 56) {
        memset(s->buf + s->n, 0, 64 - s->n);
        compress(s);
        s->n = 0;
    }
    memset(s->buf + s->n, 0, 56 - s->n);
    for (int i = 0; i < 8; i++) {
        s->buf[56 + i] = (uint8_t)(bits >> (56 - 8 * i));
    }
    compress(s);
    for (int i = 0; i < 8; i++) {
        digest[4 * i] = (uint8_t)(s->h[i] >> 24);
        digest[4 * i + 1] = (uint8_t)(s->h[i] >> 16);
        digest[4 * i + 2] = (uint8_t)(s->h[i] >> 8);
        digest[4 * i + 3] = (uint8_t)s->h[i];
    }
}

void sha256(const void *data, size_t len, uint8_t digest[SHA256_SIZE]) {
    struct sha256 s;
    sha256_init(&s);
    sha256_update(&s, data, len);
    sha256_final(&s, digest);
}

void sha256_hex(const uint8_t digest[SHA256_SIZE], char hex[2 * SHA256_SIZE + 1]) {
    static const char digits[] = "0123456789abcdef";
    for (int i = 0; i < SHA256_SIZE; i++) {
        hex[2 * i] = digits[digest[i] >> 4];
        hex[2 * i + 1] = digits[digest[i] & 15];
    }
    hex[2 * SHA256_SIZE] = 0;
}

/* Whether `salt` can be a hash's salt: 1 to PW_SALT_MAX bytes, none of
 * them a `$` (which ends it), `:` (which ends /etc/shadow's field) or a
 * newline. */
static int salt_ok(const char *salt, size_t len) {
    if (len == 0 || len > PW_SALT_MAX) {
        return 0;
    }
    for (size_t i = 0; i < len; i++) {
        if (salt[i] == '$' || salt[i] == ':' || salt[i] == '\n') {
            return 0;
        }
    }
    return 1;
}

/* The hash of `password` with the `slen` bytes of `salt`, `rounds` times,
 * written to `out` (PW_HASH_MAX + 1 bytes). */
static void hash_rounds(const char *password, const char *salt, size_t slen, int rounds, char *out) {
    size_t plen = strlen(password);
    /* The first round hashes the salt and the password; each of the others
     * the last digest, the salt and the password again, so that no round
     * can be skipped: each needs the one before it. */
    uint8_t digest[SHA256_SIZE];
    struct sha256 s;
    sha256_init(&s);
    sha256_update(&s, salt, slen);
    sha256_update(&s, password, plen);
    sha256_final(&s, digest);
    for (int i = 1; i < rounds; i++) {
        sha256_init(&s);
        sha256_update(&s, digest, sizeof digest);
        sha256_update(&s, salt, slen);
        sha256_update(&s, password, plen);
        sha256_final(&s, digest);
    }
    char hex[2 * SHA256_SIZE + 1];
    sha256_hex(digest, hex);
    char salt_copy[PW_SALT_MAX + 1];
    memcpy(salt_copy, salt, slen);
    salt_copy[slen] = 0;
    snprintf(out, PW_HASH_MAX + 1, "$sha256$%d$%s$%s", rounds, salt_copy, hex);
}

int pw_hash(const char *password, const char *salt, char *out, size_t size) {
    if (!salt_ok(salt, strlen(salt))) {
        return -EINVAL;
    }
    if (size < PW_HASH_MAX + 1) {
        return -ERANGE;
    }
    hash_rounds(password, salt, strlen(salt), PW_ROUNDS, out);
    return 0;
}

int pw_check(const char *password, const char *hash) {
    /* "$sha256$" ROUNDS "$" SALT "$" HEX: the rounds and the salt are the
     * hash's own, so a hash made with other rounds still checks. */
    const char *prefix = "$sha256$";
    size_t n = strlen(prefix);
    if (strncmp(hash, prefix, n) != 0) {
        return 0;
    }
    const char *p = hash + n;
    int rounds = 0;
    while (*p >= '0' && *p <= '9' && rounds <= PW_ROUNDS_MAX) {
        rounds = rounds * 10 + (*p++ - '0');
    }
    if (*p != '$' || rounds < 1 || rounds > PW_ROUNDS_MAX) {
        return 0;
    }
    const char *salt = p + 1;
    const char *end = strchr(salt, '$');
    if (end == NULL || !salt_ok(salt, (size_t)(end - salt))) {
        return 0;
    }
    char made[PW_HASH_MAX + 1];
    hash_rounds(password, salt, (size_t)(end - salt), rounds, made);
    return strcmp(made, hash) == 0;
}
