/* libtest: checks the C library (lib/, lib/README.md) from a program:
 * strings and memory (memset, memcpy, memmove and memcmp at every
 * alignment), numbers, snprintf's formats, malloc and free (and
 * how free merges neighbours), and the readers on a pipe. Needs no
 * filesystem. Prints a line for each check that fails and, at the end,
 * "libtest: N checks, F failed"; exits with F.
 *
 * `libtest feed N` is a child it runs: writes N numbered lines to its
 * output, in writes that cut lines in two. `libtest nap` sleeps for long,
 * and `libtest family` starts a `libtest nap` and sleeps too: the
 * processes kill_descendants ends. `libtest getenv NAME VALUE` exits 0 if
 * getenv gives it NAME's value VALUE (none for "-"): the environment a
 * child gets. */
#include "user.h"

static int checks;
static int failures;

static void check(int ok, const char *what) {
    checks++;
    if (!ok) {
        failures++;
        printf("libtest: FAILED: %s\n", what);
    }
}

static void check_eq(long got, long want, const char *what) {
    checks++;
    if (got != want) {
        failures++;
        printf("libtest: FAILED: %s: got %ld, want %ld\n", what, got, want);
    }
}

static void check_str(const char *got, const char *want, const char *what) {
    checks++;
    if (strcmp(got, want) != 0) {
        failures++;
        printf("libtest: FAILED: %s: got \"%s\", want \"%s\"\n", what, got, want);
    }
}

/* user.h's types: the fixed-width ones are as wide as their names say, and
 * the ABI's (size_t, ssize_t, off_t, time_t, intptr_t) a long's width, a
 * register's. */
static void test_types(void) {
    check(sizeof(int8_t) == 1 && sizeof(uint8_t) == 1, "int8_t and uint8_t are a byte");
    check(sizeof(int16_t) == 2 && sizeof(uint16_t) == 2, "int16_t and uint16_t are 2 bytes");
    check(sizeof(int32_t) == 4 && sizeof(uint32_t) == 4, "int32_t and uint32_t are 4 bytes");
    check(sizeof(int64_t) == 8 && sizeof(uint64_t) == 8, "int64_t and uint64_t are 8 bytes");
    check((int8_t)0xff < 0 && (uint8_t)0xff > 0 && (int16_t)0xffff < 0, "int8_t and int16_t are signed");
    check(sizeof(size_t) == sizeof(long) && sizeof(ssize_t) == sizeof(long), "size_t is a long's width");
    check(sizeof(off_t) == sizeof(long) && sizeof(time_t) == sizeof(long), "off_t and time_t too");
    check(sizeof(intptr_t) == sizeof(void *) && sizeof(uintptr_t) == sizeof(void *), "intptr_t holds a pointer");
    check(sizeof(long) == sizeof(void *), "a long is a pointer's width");
    check(sizeof(long) == 8, "LP64: a long is 8 bytes");
    check(LONG_MAX == (long)(ULONG_MAX >> 1) && LONG_MIN == -LONG_MAX - 1, "LONG_MAX and LONG_MIN");
    check(INT64_MAX == 0x7fffffffffffffffll && INT64_MIN + INT64_MAX == -1 && UINT64_MAX + 1 == 0,
          "the 64-bit limits");
    check(SIZE_MAX == (size_t)-1 && UINT_MAX == (uint)-1 && UINT32_MAX == 0xffffffffu, "the unsigned limits");
    check(INT_MAX == 2147483647 && INT_MIN == (int)0x80000000u, "INT_MAX and INT_MIN");
}

static void test_strings(void) {
    char buf[32];
    check_eq((int)strlen(""), 0, "strlen of nothing");
    check_eq((int)strlen("camelot"), 7, "strlen");
    check(strcmp("abc", "abc") == 0, "strcmp equal");
    check(strcmp("abc", "abd") < 0 && strcmp("abd", "abc") > 0, "strcmp order");
    check(strcmp("ab", "abc") < 0, "strcmp prefix first");
    check(strcmp("\xff", "a") > 0, "strcmp compares unsigned bytes");
    check(strncmp("abcx", "abcy", 3) == 0 && strncmp("abcx", "abcy", 4) < 0, "strncmp");
    check(strncmp("ab", "ab", 10) == 0 && strncmp("a", "b", 0) == 0, "strncmp stops at the NUL and at n");
    check_str(strcpy(buf, "round"), "round", "strcpy");
    check_str(strcat(buf, " table"), "round table", "strcat");
    const char *s = "excalibur";
    check(strchr(s, 'c') == s + 2 && strchr(s, 'z') == NULL && strchr(s, 0) == s + 9, "strchr");
    check(strrchr(s, 'r') == s + 8 && strrchr(s, 'z') == NULL && strrchr(s, 0) == s + 9, "strrchr");
    check(strstr(s, "lib") == s + 4 && strstr(s, "libx") == NULL, "strstr");
    check(strstr(s, "") == s && strstr("", "") != NULL && strstr("", "a") == NULL, "strstr empty");
    check(strstr("aab", "ab") != NULL, "strstr after a false start");

    memset(buf, 'x', 8);
    buf[8] = 0;
    check_str(buf, "xxxxxxxx", "memset");
    memcpy(buf, "12345678", 8);
    check(memcmp(buf, "12345678", 8) == 0, "memcpy");
    memmove(buf + 2, buf, 6);
    check(memcmp(buf, "12123456", 8) == 0, "memmove up");
    memmove(buf, buf + 2, 6);
    check(memcmp(buf, "12345656", 8) == 0, "memmove down");
    check(memcmp("a\x80", "a\x01", 2) > 0 && memcmp("ab", "ac", 1) == 0, "memcmp");
}

/* ---- The memory functions at every alignment ----
 *
 * memset, memcpy, memmove and memcmp (lib/mem*.S, the kernel's) work a
 * register at a time where their pointers are on a register boundary, and
 * a byte at a time around that: checked at every alignment of each pointer
 * within a register (0 to 7) and every length to 64, and at a few long
 * lengths for alignments 0 and 5 of each, so co-aligned and not, with
 * guard bytes either side that must not change. One check for each
 * function and pair of alignments; the first wrong case is printed.
 * (kernel.rs's kernel_memory_functions_at_every_alignment checks the same
 * code, the kernel's, at more lengths: merlin's C makes the checking slow
 * here.)
 *
 * The buffers are arrays of uint64_t, so that each starts on a register
 * boundary (merlin aligns a uchar array to a byte) and an offset from its
 * start, MEM_GUARD (8) included, is an alignment. */

#define MEM_GUARD 8
#define MEM_LONGEST 4101
#define MEM_SIZE ((MEM_LONGEST + 8 + 2 * MEM_GUARD + 7) / 8 * 8)
static uint64_t mem_src_words[MEM_SIZE / 8];
static uint64_t mem_dst_words[MEM_SIZE / 8];
#define mem_src ((uchar *)mem_src_words)
#define mem_dst ((uchar *)mem_dst_words)

/* The byte at offset `i` of the patterns: different from its neighbours. */
#define MEM_BYTE(i) ((uchar)((i) * 7 + 3) | 1)

/* The lengths past 64: around the loops' larger steps (memcpy's 128-byte
 * rounds, memset's 256) and a page and more. */
static const size_t mem_long[] = {127, 128, 129, 255, 256, 257, 1024, MEM_LONGEST};

/* Length `i` of the ones checked: 0 to 64, then mem_long's if `longer`;
 * (size_t)-1 past the last. */
static size_t mem_length(size_t i, int longer) {
    if (i <= 64) {
        return i;
    }
    if (!longer || i - 65 >= sizeof mem_long / sizeof mem_long[0]) {
        return (size_t)-1;
    }
    return mem_long[i - 65];
}

/* Whether the `n` bytes at `at` (in mem_dst) are `want`'s, or all `c` if
 * `want` is NULL, and the MEM_GUARD bytes either side still 0xee; puts 0xee
 * back in the n bytes for the next call. */
static int mem_check(uchar *at, size_t n, const uchar *want, uchar c) {
    int ok = 1;
    for (uchar *g = at - MEM_GUARD; g < at; g++) {
        ok = ok && *g == 0xee;
    }
    for (size_t k = 0; k < n; k++) {
        ok = ok && at[k] == (want ? want[k] : c);
        at[k] = 0xee;
    }
    for (uchar *g = at + n; g < at + n + MEM_GUARD; g++) {
        ok = ok && *g == 0xee;
    }
    return ok;
}

static void mem_report(const char *f, size_t sa, size_t da, size_t n) {
    printf("libtest: FAILED: %s from alignment %lu to %lu, %lu bytes\n", f, (unsigned long)sa,
           (unsigned long)da, (unsigned long)n);
}

static void test_memory_alignments(void) {
    for (size_t i = 0; i < MEM_SIZE; i++) {
        mem_src[i] = MEM_BYTE(i);
        mem_dst[i] = 0xee;
    }
    for (size_t da = 0; da < 8; da++) {
        int ok = 1;
        size_t n;
        for (size_t i = 0; ok && (n = mem_length(i, 1)) != (size_t)-1; i++) {
            uchar *at = mem_dst + MEM_GUARD + da;
            ok = memset(at, 0x1ab, n) == at && mem_check(at, n, NULL, 0xab);
            if (!ok) {
                mem_report("memset", 0, da, n);
            }
        }
        check(ok, "memset at every alignment");
    }
    for (size_t sa = 0; sa < 8; sa++) {
        for (size_t da = 0; da < 8; da++) {
            int longer = (sa == 0 || sa == 5) && (da == 0 || da == 5);
            for (int f = 0; f < 2; f++) {
                int ok = 1;
                size_t n;
                for (size_t i = 0; ok && (n = mem_length(i, longer)) != (size_t)-1; i++) {
                    uchar *at = mem_dst + MEM_GUARD + da;
                    const uchar *from = mem_src + MEM_GUARD + sa;
                    void *r = f == 0 ? memcpy(at, from, n) : memmove(at, from, n);
                    ok = r == at && mem_check(at, n, from, 0);
                    if (!ok) {
                        mem_report(f == 0 ? "memcpy" : "memmove", sa, da, n);
                    }
                }
                check(ok, f == 0 ? "memcpy at every alignment" : "memmove at every alignment");
            }
        }
    }
}

/* memmove within one buffer, overlapping both ways: from offset s to d,
 * every pair to 16 apart (every alignment of each, dst below src and
 * above), at the lengths around a register's and two's. */
static void test_memmove_overlap(void) {
    static const size_t lengths[] = {0, 1, 7, 8, 9, 15, 16, 17, 24, 33, 64};
    int ok = 1;
    for (size_t s = 0; s < 16 && ok; s++) {
        for (size_t d = 0; d < 16 && ok; d++) {
            for (size_t l = 0; l < sizeof lengths / sizeof lengths[0] && ok; l++) {
                size_t n = lengths[l];
                for (size_t k = 0; k < n + 16; k++) {
                    mem_dst[k] = MEM_BYTE(k);
                }
                ok = memmove(mem_dst + d, mem_dst + s, n) == mem_dst + d;
                for (size_t k = 0; k < n + 16 && ok; k++) {
                    size_t from = k >= d && k < d + n ? k - d + s : k;
                    ok = mem_dst[k] == MEM_BYTE(from);
                }
                if (!ok) {
                    printf("libtest: FAILED: memmove within a buffer from %lu to %lu, %lu bytes\n",
                           (unsigned long)s, (unsigned long)d, (unsigned long)n);
                }
            }
        }
    }
    check(ok, "memmove overlapping, both ways");
}

/* memcmp: 0 for equal bytes, whatever follows them; else the sign of the
 * first byte that differs, as unsigned bytes (0x80 above 0x01), wherever
 * it is: before a register boundary, at any byte of a whole register, or
 * after the last one. */
static void test_memcmp_alignments(void) {
    for (size_t sa = 0; sa < 8; sa++) {
        for (size_t da = 0; da < 8; da++) {
            int ok = 1;
            for (size_t n = 0; n <= 24 && ok; n++) {
                uchar *x = mem_src + MEM_GUARD + sa;
                uchar *y = mem_dst + MEM_GUARD + da;
                for (size_t k = 0; k < n; k++) {
                    x[k] = MEM_BYTE(k);
                    y[k] = MEM_BYTE(k);
                }
                x[n] = 1;
                y[n] = 2;
                ok = memcmp(x, y, n) == 0;
                for (size_t p = 0; p < n && ok; p++) {
                    x[p] = 0x80;
                    y[p] = 0x01;
                    ok = memcmp(x, y, n) == 1 && memcmp(y, x, n) == -1;
                    /* A later difference the other way does not count. */
                    if (ok && p + 1 < n) {
                        y[p + 1] = 0xff;
                        ok = memcmp(x, y, n) == 1;
                        y[p + 1] = MEM_BYTE(p + 1);
                    }
                    x[p] = MEM_BYTE(p);
                    y[p] = MEM_BYTE(p);
                }
                if (!ok) {
                    mem_report("memcmp", sa, da, n);
                }
            }
            check(ok, "memcmp at every alignment");
        }
    }
}

static void test_numbers(void) {
    char *end;
    check_eq(atoi("42"), 42, "atoi");
    check_eq(atoi("-17x"), -17, "atoi stops at a non-digit");
    check_eq(atoi("-2147483648"), (int)0x80000000u, "atoi of INT_MIN");
    check_eq((int)strtol("123", &end, 10), 123, "strtol");
    check(*end == 0, "strtol ends at the end");
    check_eq((int)strtol("  -0x1F!", &end, 0), -31, "strtol hex with a sign");
    check(*end == '!', "strtol ends after the digits");
    check_eq((int)strtol("0755", NULL, 0), 0755, "strtol octal");
    check_eq((int)strtol("z", &end, 36), 35, "strtol base 36");
    const char *bad = "pid";
    check_eq((int)strtol(bad, &end, 10), 0, "strtol of no digits");
    check(end == bad, "no digits: end is the start");
    check_eq((int)strtol("0xg", &end, 16), 0, "strtol 0x without a hex digit");
    check(*end == 'x', "it stops at the x");
    /* Out of range: the limit, as in C, not a wrapped-around value. A long
     * is 64 bits, and so are its limits. */
    check_eq(strtol("9223372036854775807", NULL, 10), LONG_MAX, "strtol of LONG_MAX");
    check_eq(strtol("-9223372036854775808", NULL, 10), LONG_MIN, "strtol of LONG_MIN");
    check_eq(strtol("9223372036854775808", &end, 10), LONG_MAX, "strtol just past LONG_MAX");
    check_eq(strtol("-9223372036854775809", NULL, 10), LONG_MIN, "strtol just past LONG_MIN");
    check_eq(strtol("18446744073709551622x", &end, 10), LONG_MAX, "strtol of 2^64 + 6 does not wrap to 6");
    check(*end == 'x', "and it ends after all the digits");
    check_eq(strtol("0x10000000000000000", NULL, 0), LONG_MAX, "strtol of a hex number out of range");
    check_eq(strtol("4294967302", NULL, 10), 4294967302l, "strtol of 2^32 + 6, a long");
}

/* A shift count the compiler cannot fold: the shifts below are the
 * arithmetic /bin/as and /bin/ld compute with. */
static int shift_counts[] = {0, 3, 32, 40, 63};

/* The value in hexadecimal, as merlin-as prints a u64 with {:#x}. */
static const char *hex64(uint64_t v) {
    static char buf[24];
    snprintf(buf, sizeof buf, "0x%llx", v);
    return buf;
}

/* int64_t and uint64_t, `long long`: 64 bits. */
static void test_long_long(void) {
    uint64_t big = (uint64_t)0xffffffffu + 1;
    check(big == (uint64_t)1 << 32, "uint64_t addition carries into the high word");
    int64_t five = 5, seven = 7;
    check(five - seven == -2, "int64_t subtraction below zero");
    check_str(hex64((uint64_t)(int64_t)-16), "0xfffffffffffffff0", "a negative int64_t as a uint64_t");
    check(((uint64_t)0x80000000u * 6) == 12884901888ull, "a product past 32 bits");
    int64_t m7 = -7, two = 2;
    check(m7 / two == -3, "int64_t division truncates toward zero");
    check(m7 % two == -1, "int64_t remainder has the dividend's sign");
    uint64_t n = 0xfffffffffffffff0ull, three = 3, thousand = 1000;
    check(n / three == 0x5555555555555550ull, "uint64_t division past 32 bits");
    check(n / thousand == 18446744073709551ull && n % thousand == 600, "uint64_t division and remainder");
    check_str(hex64((uint64_t)1 << shift_counts[3]), "0x10000000000", "a left shift past 32 bits");
    int64_t m64 = -64;
    check(m64 >> shift_counts[1] == -8, "an int64_t right shift keeps the sign");
    check_str(hex64((uint64_t)m64 >> shift_counts[1]), "0x1ffffffffffffff8",
              "a uint64_t right shift shifts zeros in");
    uint64_t ones = ~(uint64_t)0;
    check_str(hex64(ones >> shift_counts[3]), "0xffffff", "a right shift past 32 bits");
    /* The edges of a register pair: no shift, the high word alone, the
     * top bit alone. */
    check_str(hex64(ones >> shift_counts[0]), "0xffffffffffffffff", "a right shift by 0");
    check_str(hex64(ones >> shift_counts[2]), "0xffffffff", "a right shift by 32");
    check_str(hex64(ones >> shift_counts[4]), "0x1", "a right shift by 63");
    check(ones << shift_counts[4] == (uint64_t)1 << 63, "a left shift by 63");
    int64_t m1 = -1, one = 1;
    check(m1 < one && (uint64_t)m1 > (uint64_t)one, "int64_t compares signed, uint64_t unsigned");
    check(INT64_MIN < INT64_MAX && (int64_t)0x80000000u > 0, "the 64-bit comparisons see the high word");
}

static void test_snprintf(void) {
    char buf[64];
    snprintf(buf, sizeof buf, "%d %u %x %c %s %%", -42, 4294967295u, 0xcafef00du, 'K', "excalibur");
    check_str(buf, "-42 4294967295 cafef00d K excalibur %", "the conversions");
    snprintf(buf, sizeof buf, "[%5d|%-5d|%05d|%-05d]", 42, 42, -42, 7);
    check_str(buf, "[   42|42   |-0042|7    ]", "widths and flags");
    snprintf(buf, sizeof buf, "[%6s|%-6s|%3s]", "ab", "ab", "toolong");
    check_str(buf, "[    ab|ab    |toolong]", "string widths");
    snprintf(buf, sizeof buf, "%d %ld %lu", (int)0x80000000u, 7, 8u);
    check_str(buf, "-2147483648 7 8", "INT_MIN and l");
    snprintf(buf, sizeof buf, "%p %s", (void *)0x1000, (char *)NULL);
    check_str(buf, "0x0000000000001000 (null)", "%p and a null string");
    /* A long's digits: all 64 bits of it. */
    snprintf(buf, sizeof buf, "%ld %lu %lx", LONG_MIN, ~0ul, 0x123456789abcdefl);
    check_str(buf, "-9223372036854775808 18446744073709551615 123456789abcdef", "64-bit longs");
    /* A long long is 64 bits too. */
    snprintf(buf, sizeof buf, "%lld %llu %llx", LLONG_MIN, ULLONG_MAX, 0x123456789abcdefll);
    check_str(buf, "-9223372036854775808 18446744073709551615 123456789abcdef", "%ll: 64-bit long longs");
    snprintf(buf, sizeof buf, "%d|%lld|%d", 1, -2ll, 3);
    check_str(buf, "1|-2|3", "%lld between ints");
    snprintf(buf, sizeof buf, "[%20llu|%-5lld|%05llx]", 4294967296ull, -1ll, 0xabcull);
    check_str(buf, "[          4294967296|-1   |00abc]", "%ll with widths and flags");
    int64_t i64max = INT64_MAX;
    uint64_t u64 = (uint64_t)1 << 40;
    snprintf(buf, sizeof buf, "%lld %llx", i64max, u64);
    check_str(buf, "9223372036854775807 10000000000", "int64_t and uint64_t");
    /* z: a size_t (or ssize_t), a long's width. */
    snprintf(buf, sizeof buf, "%zu %zd %zx", sizeof(struct stat), (ssize_t)-5, (size_t)0xff);
    check_str(buf, "24 -5 ff", "%z: size_t and ssize_t");
    snprintf(buf, sizeof buf, "%zu", SIZE_MAX);
    check_str(buf, "18446744073709551615", "%zu of SIZE_MAX");
    snprintf(buf, sizeof buf, "%q %");
    check_str(buf, "%q %", "unknown conversions are printed as written");
    check_eq(snprintf(buf, 6, "%s", "camelot"), 7, "snprintf returns the whole length");
    check_str(buf, "camel", "and cuts it to fit, with a NUL");
    buf[0] = 'z';
    check_eq(snprintf(buf, 0, "abc"), 3, "snprintf with no room");
    check_eq(buf[0], 'z', "writes nothing then");
    check_eq(snprintf(buf, 1, "abc"), 3, "snprintf with room for the NUL only");
    check_eq(buf[0], 0, "writes the NUL");
}

/* malloc's header (lib/malloc.c): a pointer and a size, 16 bytes, which is
 * also every block's alignment. */
#define HDR (2 * (int)sizeof(void *))

/* A block of this many bytes is two pages with its header: more
 * than any free block before test_merge (it runs before anything else
 * uses the heap) and at least the page malloc grows the heap by, so each
 * comes from the top of the heap, sbrk asked for exactly it. */
#define MERGE_BYTES (2 * 4096 - HDR)

/* Three neighbouring blocks of MERGE_BYTES, lowest first, from fresh heap
 * at its top; 0 if they aren't that. */
static int three_neighbours(char **blk) {
    char *top = sbrk(0);
    for (int i = 0; i < 3; i++) {
        blk[i] = malloc(MERGE_BYTES);
    }
    return blk[0] == top + HDR && blk[1] == blk[0] + MERGE_BYTES + HDR &&
           blk[2] == blk[1] + MERGE_BYTES + HDR && (char *)sbrk(0) == top + 3 * (MERGE_BYTES + HDR);
}

/* Frees three neighbours in the order `a`, `b`, `c` (0 the lowest), then
 * asks for one block exactly as big as the three with their headers: it
 * fits, without growing the heap, only if free merged all three into one,
 * and then it is where they were. Returns it, to be freed at the end. */
static char *merge_case(int a, int b, int c, const char *what) {
    char *blk[3];
    check(three_neighbours(blk), "three neighbouring blocks from the top of the heap");
    free(blk[a]);
    free(blk[b]);
    free(blk[c]);
    char *top = sbrk(0);
    char *p = malloc(3 * MERGE_BYTES + 2 * HDR);
    check(p == blk[0] && sbrk(0) == top, what);
    return p;
}

/* free merges a block with the free one above it and with the one below:
 * freeing three neighbours from the bottom up takes merges below, from the
 * top down merges above, and the outer two first, then the middle one,
 * both at once. The merged blocks stay taken until the end, so that each
 * case starts from fresh heap. */
static void test_merge(void) {
    char *up = merge_case(0, 1, 2, "freed upwards, three neighbours merge into one");
    char *down = merge_case(2, 1, 0, "freed downwards, three neighbours merge into one");
    char *outer = merge_case(0, 2, 1, "the middle one freed last merges with both");
    free(up);
    free(down);
    free(outer);
}

static void test_malloc(void) {
    check(malloc(0) == NULL, "malloc(0)");
    char *a = malloc(10);
    char *b = malloc(100);
    char *c = malloc(3000);
    check(a != NULL && b != NULL && c != NULL, "malloc");
    uintptr_t align = (uintptr_t)HDR - 1;
    check(((uintptr_t)a & align) == 0 && ((uintptr_t)b & align) == 0 && ((uintptr_t)c & align) == 0,
          "aligned to the header's size");
    check(b >= a + 10 || a >= b + 100, "blocks don't overlap");
    memset(a, 'a', 10);
    memset(b, 'b', 100);
    memset(c, 'c', 3000);
    check(a[9] == 'a' && b[0] == 'b' && b[99] == 'b' && c[0] == 'c', "blocks hold their bytes");
    /* A freed block is reused for a request that fits. */
    free(b);
    char *d = malloc(100);
    check(d == b, "a freed block is reused");
    free(a);
    free(c);
    free(d);
    /* What was freed is used again, without growing the heap (test_merge
     * checks that neighbours merge). */
    char *top = sbrk(0);
    char *e = malloc(3000);
    check(e != NULL && sbrk(0) == top, "freed blocks are used again");
    free(e);
    /* Grow past a page: sbrk is asked for more. */
    char *big = malloc(100000);
    check(big != NULL && (char *)sbrk(0) > top, "a big block grows the heap");
    big[99999] = 1;
    free(big);
    free(NULL);
    /* calloc zeroes, realloc keeps the bytes. */
    int *z = calloc(50, sizeof(int));
    int zero = 1;
    for (int i = 0; i < 50; i++) {
        zero &= z[i] == 0;
        z[i] = i;
    }
    check(zero, "calloc zeroes");
    z = realloc(z, 4000);
    check(z != NULL && z[0] == 0 && z[49] == 49, "realloc keeps the bytes");
    free(z);
    check(calloc(SIZE_MAX / 2, 3) == NULL && calloc(3, SIZE_MAX / 2) == NULL, "calloc overflow");
    check(malloc(0x7fffffff) == NULL, "malloc of more than there is");
    check(malloc(SIZE_MAX) == NULL && malloc(SIZE_MAX - 8) == NULL, "malloc of SIZE_MAX does not wrap around");
    /* A size_t of 64 bits, whole: 2^32 + 16 is not 16. */
    check(malloc(0x100000010ul) == NULL, "malloc of 4 GiB and 16 bytes is more than there is");
    /* Many small blocks, freed in an odd order, then one big one. */
    char *many[40];
    for (int i = 0; i < 40; i++) {
        many[i] = malloc((size_t)(i % 7 + 1) * 16);
        many[i][0] = (char)i;
    }
    int intact = 1;
    for (int i = 0; i < 40; i += 2) {
        free(many[i]);
    }
    for (int i = 1; i < 40; i += 2) {
        intact &= many[i][0] == (char)i;
        free(many[i]);
    }
    check(intact, "blocks survive their neighbours being freed");
    top = sbrk(0);
    char *all = malloc(3000);
    check(all != NULL && sbrk(0) == top, "and their room is used again");
    free(all);
}

/* A program that moves the end of the heap itself, by an odd number of
 * bytes: the blocks malloc then takes from fresh heap are still aligned to
 * its header's size, and so is the end of the heap after them. Runs first,
 * while nothing is free, and returns its block, to be freed once the other
 * heap tests are done: the request is smaller than a block that gets a
 * mapping of its own, and once free it would be the block the next ones
 * come from, where test_merge wants them from the top of the heap. */
static char *test_odd_break(void) {
    uintptr_t align = (uintptr_t)HDR - 1;
    char *top = sbrk(3);
    check((long)top >= 0 && ((uintptr_t)sbrk(0) & align) != 0, "sbrk by 3 bytes leaves the end unaligned");
    char *p = malloc(120000);
    check(p != NULL && p > top, "a block from above the odd end");
    check(((uintptr_t)p & align) == 0 && ((uintptr_t)sbrk(0) & align) == 0,
          "is aligned to the header's size, and so is the new end");
    p[119999] = 1;
    return p;
}

/* A block of 128 KiB or more has a mapping of its own (mmap), past the
 * heap, which doesn't grow; free gives it back (munmap), so the same
 * request gets the same pages again, zeroed. realloc keeps its bytes. */
static void test_mapped(void) {
    char *top = sbrk(0);
    char *p = malloc(128 * 1024);
    check(p != NULL && p > top && sbrk(0) == top, "a block of 128 KiB is mapped, past the heap");
    check(((uintptr_t)p & 4095) == (uintptr_t)HDR, "after its header, at the start of a page");
    p[0] = 1;
    p[128 * 1024 - 1] = 2;
    p = realloc(p, 200 * 1024);
    check(p != NULL && p[0] == 1 && p[128 * 1024 - 1] == 2, "realloc keeps its bytes");
    p[200 * 1024 - 1] = 3;
    char *q = malloc(300 * 1024);
    check(q != NULL && q != p && sbrk(0) == top, "another");
    free(q);
    char *again = malloc(300 * 1024);
    check(again == q && again[0] == 0, "free gives it back: the next gets its address, zeroed");
    free(again);
    free(p);
    int *zeroed = calloc(64 * 1024, sizeof(int));
    check(zeroed != NULL && zeroed[0] == 0 && zeroed[64 * 1024 - 1] == 0, "calloc of a mapped block reads zero");
    free(zeroed);
    char *small = malloc(1000);
    check(small != NULL && small < top, "a small block still comes from the heap");
    free(small);
}

/* libtest feed N: N lines "line K", written in pieces of 7 bytes. */
static int feed(int n) {
    static char text[1024];
    int len = 0;
    for (int k = 1; k <= n; k++) {
        len += snprintf(text + len, sizeof text - (size_t)len, "line %d\n", k);
    }
    for (int at = 0; at < len; at += 7) {
        int m = len - at < 7 ? len - at : 7;
        if (write(STDOUT, text + at, (size_t)m) != m) {
            return 1;
        }
    }
    return 0;
}

static void test_reader(void) {
    /* A child writes 100 lines into a pipe in odd pieces; the reader gives
     * them back whole. */
    int p[2];
    check_eq(pipe(p), 0, "a pipe");
    int out = dup(STDOUT);
    close(STDOUT);
    dup(p[1]);
    int pid = spawn("/bin/libtest", (char *[]){"libtest", "feed", "100", NULL}, environ);
    close(STDOUT);
    dup(out);
    close(out);
    close(p[1]);
    check(pid > 0, "spawn the feeder");
    struct reader r;
    reader_init(&r, p[0]);
    char line[16];
    char want[16];
    int k = 0;
    int ok = 1;
    int n;
    while ((n = reader_getline(&r, line, sizeof line)) > 0) {
        k++;
        snprintf(want, sizeof want, "line %d\n", k);
        ok &= strcmp(line, want) == 0 && n == (int)strlen(want);
    }
    check(ok, "every line whole, with its newline");
    check_eq(k, 100, "all the lines");
    check_eq(n, 0, "then the end of the input");
    check_eq(reader_getline(&r, line, sizeof line), 0, "and again");
    int status = -1;
    check_eq(wait(pid, &status), pid, "the feeder ends");
    check_eq(status, 0, "having written it all");
    close(p[0]);

    /* A long line comes in pieces, a last line may lack its newline, and
     * a read error is passed on. */
    check_eq(pipe(p), 0, "another pipe");
    write(p[1], "abcdefghij\nxy", 13);
    close(p[1]);
    reader_init(&r, p[0]);
    char small[5];
    check_eq(reader_getline(&r, small, sizeof small), 4, "a piece of a long line");
    check_str(small, "abcd", "its first bytes");
    check_eq(reader_getline(&r, small, sizeof small), 4, "the next piece");
    check_eq(reader_getline(&r, small, sizeof small), 3, "its end");
    check_str(small, "ij\n", "with the newline");
    check_eq(reader_getline(&r, small, sizeof small), 2, "a last line without a newline");
    check_str(small, "xy", "as it is");
    check_eq(reader_getline(&r, small, sizeof small), 0, "the end");
    close(p[0]);
    reader_init(&r, STDOUT);
    check_eq(reader_getline(&r, small, sizeof small), -EBADF, "an error");
}

/* lib/inet.c and the part of lib/resolve.c that needs no network. */
static void test_inet(void) {
    check_eq(htons(0x1234), 0x3412, "htons");
    check_eq(ntohs(htons(8080)), 8080, "ntohs undoes htons");
    check(htonl(0x0a00020fu) == 0x0f02000au, "htonl");
    check(ntohl(htonl(0xc0a80101u)) == 0xc0a80101u, "ntohl undoes htonl");
    struct in_addr a;
    check(inet_aton("10.0.2.15", &a) && a.s_addr == htonl(0x0a00020fu), "inet_aton");
    check(inet_aton("0.0.0.0", &a) && a.s_addr == 0, "inet_aton of 0.0.0.0");
    const char *bad[] = {"", "1.2.3", "1.2.3.4.5", "256.1.1.1", "1.2.3.4 ", "1..2.3", "0001.2.3.4", "a.b.c.d", "-1.2.3.4"};
    for (uint i = 0; i < sizeof bad / sizeof bad[0]; i++) {
        check(!inet_aton(bad[i], &a), bad[i]);
    }
    check(inet_addr("127.0.0.1") == htonl(0x7f000001u), "inet_addr");
    check(inet_addr("x") == INADDR_NONE, "inet_addr of a name");
    a.s_addr = htonl(0xc0a801ffu);
    check_str(inet_ntoa(a), "192.168.1.255", "inet_ntoa");
    struct sockaddr_in sa;
    memset(&sa, 0xff, sizeof sa);
    sockaddr_set(&sa, 0x0a000202u, 8080);
    const uchar *b = (const uchar *)&sa;
    check(sizeof sa == 16 && sa.sin_family == AF_INET, "sockaddr_set: 16 bytes, AF_INET");
    check(b[2] == 0x1f && b[3] == 0x90 && b[4] == 10 && b[5] == 0 && b[6] == 2 && b[7] == 2,
          "sockaddr_set: the port and the address big-endian");
    int zeros = 1;
    for (int i = 8; i < 16; i++) {
        zeros = zeros && b[i] == 0;
    }
    check(zeros, "sockaddr_set: the rest zeroed");
    check(resolve("10.1.2.3", &a) == 0 && a.s_addr == htonl(0x0a010203u), "resolve of an address");
    check_eq(resolve("", &a), -EINVAL, "resolve of nothing");
    check_eq(resolve("a..b", &a), -EINVAL, "resolve of an empty label");
    check_eq(resolve("a b", &a), -EINVAL, "resolve of a name with a bad character");
    check_str(resolve_error(ENOENT), "unknown host", "resolve_error of ENOENT");
    check_str(resolve_error(ETIMEDOUT), strerror(ETIMEDOUT), "resolve_error of anything else");
}

/* Sleeps for long: until it is killed. */
static int nap(void) {
    sleep(1000000);
    return 0;
}

static int family(void) {
    if (spawn("/bin/libtest", (char *[]){"libtest", "nap", NULL}, environ) < 0) {
        return 1;
    }
    return nap();
}

/* The pid of a process whose parent is `ppid` (the first), or 0. */
static int child_of(int ppid) {
    struct procinfo info[16];
    int n = procs(info, 16);
    for (int i = 0; i < n && i < 16; i++) {
        if ((int)info[i].ppid == ppid) {
            return (int)info[i].pid;
        }
    }
    return 0;
}

/* Whether process `pid` is in the table. */
static int exists(int pid) {
    struct procinfo info[16];
    int n = procs(info, 16);
    for (int i = 0; i < n && i < 16; i++) {
        if ((int)info[i].pid == pid) {
            return 1;
        }
    }
    return 0;
}

static void test_kill_descendants(void) {
    check_eq(kill_descendants(getpid()), 0, "kill_descendants with no children");
    /* Two children, one with a child of its own. */
    int fam = spawn("/bin/libtest", (char *[]){"libtest", "family", NULL}, environ);
    int napper = spawn("/bin/libtest", (char *[]){"libtest", "nap", NULL}, environ);
    check(fam > 0 && napper > 0, "spawn a family and a napper");
    int grandchild = 0;
    for (int tries = 0; tries < 1000 && grandchild == 0; tries++) {
        grandchild = child_of(fam);
        if (grandchild == 0) {
            sleep(1);
        }
    }
    check(grandchild > 0, "the family's own child starts");
    check_eq(kill_descendants(getpid()), 3, "kill_descendants kills the children and the grandchild");
    int status = 0;
    check_eq(wait(fam, &status), fam, "the family ends");
    check_eq(status, -1, "killed");
    check_eq(wait(napper, &status), napper, "the napper ends");
    check_eq(status, -1, "killed too");
    /* The grandchild, an orphan once its parent ended, is freed when it
     * ends: it was killed, so that is soon. */
    int tries = 0;
    while (exists(grandchild) && tries < 1000) {
        sleep(1);
        tries++;
    }
    check(!exists(grandchild), "the grandchild ends");
    check_eq(kill_descendants(getpid()), 0, "nothing left to kill");
}

/* `libtest getenv NAME VALUE`: exits 0 if its environment gives NAME the
 * value VALUE, or none for VALUE "-", else 1. */
static int getenv_child(const char *name, const char *want) {
    const char *got = getenv(name);
    if (strcmp(want, "-") == 0) {
        return got == NULL ? 0 : 1;
    }
    return got != NULL && strcmp(got, want) == 0 ? 0 : 1;
}

/* Runs `libtest getenv NAME VALUE` with the environment `envp` and returns
 * its exit status, or the error. */
static int run_getenv(const char *name, const char *want, char **envp) {
    char *argv[] = {"libtest", "getenv", (char *)name, (char *)want, NULL};
    int pid = spawn("/bin/libtest", argv, envp);
    if (pid < 0) {
        return pid;
    }
    int status = -1000;
    return wait(pid, &status) == pid ? status : -1000;
}

/* How many entries environ has. */
static int entries(void) {
    int n = 0;
    while (environ[n] != NULL) {
        n++;
    }
    return n;
}

static void test_env(void) {
    /* The environment the shell passed, as crt0 left it in environ. */
    char **start = environ;
    check(start != NULL, "crt0 sets environ");
    int n = entries();
    check(getenv("") == NULL, "getenv of an empty name");
    check(getenv("LIBTEST_A") == NULL, "getenv of a variable that isn't there");
    check_eq(setenv("LIBTEST_A", "one", 0), 0, "setenv adds a variable");
    check_str(getenv("LIBTEST_A"), "one", "getenv finds it");
    check(environ != start, "setenv makes an array of its own");
    check_eq(entries(), n + 1, "one more entry");
    check_str(environ[n], "LIBTEST_A=one", "at the end");
    check(environ[0] == start[0], "the others stay");
    char *kept = getenv("LIBTEST_A");
    check_eq(setenv("LIBTEST_A", "two", 0), 0, "setenv without overwrite");
    check_str(getenv("LIBTEST_A"), "one", "leaves the value");
    check_eq(setenv("LIBTEST_A", "two", 1), 0, "setenv with overwrite");
    check_str(getenv("LIBTEST_A"), "two", "replaces it");
    check_str(kept, "one", "and what getenv returned before stays good");
    check_eq(entries(), n + 1, "in its place");
    check_eq(setenv("LIBTEST_B", "", 1), 0, "setenv to an empty value");
    check(getenv("LIBTEST_B") != NULL && getenv("LIBTEST_B")[0] == 0, "an empty value is a value");
    check_eq(setenv("", "x", 1), -EINVAL, "setenv of an empty name");
    check_eq(setenv("A=B", "x", 1), -EINVAL, "setenv of a name with a =");
    check(getenv("A=B") == NULL, "getenv of a name with a =");
    check_eq(unsetenv(""), -EINVAL, "unsetenv of an empty name");
    check_eq(unsetenv("A=B"), -EINVAL, "unsetenv of a name with a =");
    /* A child started with environ has what setenv put there, and not what
     * unsetenv took out. */
    check_eq(run_getenv("LIBTEST_A", "two", environ), 0, "a child gets the variable");
    check_eq(unsetenv("LIBTEST_A"), 0, "unsetenv");
    check(getenv("LIBTEST_A") == NULL, "removes it");
    check_eq(entries(), n + 1, "and its entry");
    check_eq(unsetenv("LIBTEST_A"), 0, "unsetenv of a variable that isn't there");
    check_eq(run_getenv("LIBTEST_A", "-", environ), 0, "a child doesn't get it");
    check_eq(run_getenv("LIBTEST_B", "", environ), 0, "a child gets an empty value");
    check_eq(run_getenv("HOME", "-", NULL), 0, "a NULL environment is an empty one");

    /* An array of the program's own: getenv takes the first entry of a
     * name, unsetenv removes them all, setenv makes a new array. */
    char *own[] = {"D=1", "LIBTEST_D=first", "X", "LIBTEST_D=second", "LIBTEST_DD=3", NULL};
    environ = own;
    check_str(getenv("LIBTEST_D"), "first", "getenv takes the first");
    check_str(getenv("D"), "1", "a name is all of it, not a part");
    check(getenv("X") == NULL, "an entry without a = has no value");
    check_eq(unsetenv("LIBTEST_D"), 0, "unsetenv of a name with two entries");
    check(environ == own && own[1] != NULL && strcmp(own[1], "X") == 0 && strcmp(own[2], "LIBTEST_DD=3") == 0 &&
              own[3] == NULL,
          "removes both, in place, the others in order");
    check_eq(setenv("E", "5", 1), 0, "setenv on the program's array");
    check(environ != own && strcmp(environ[3], "E=5") == 0 && environ[4] == NULL, "copies it, one longer");
    check_eq(run_getenv("E", "5", environ), 0, "a child gets that one");
    environ = NULL;
    check(getenv("E") == NULL, "getenv with no environment");
    check_eq(setenv("F", "6", 1), 0, "setenv with no environment");
    check(environ != NULL && strcmp(environ[0], "F=6") == 0 && environ[1] == NULL, "makes one");
    /* The array setenv made last is the one it grows, even when environ
     * has become another since: it copies that one's pointers in rather
     * than leave its own unused (glibc's __add_to_environ). realloc keeps
     * a block that is big enough, and this one is: it had room for E's five. */
    char **made = environ;
    char *other[] = {"G=7", NULL};
    environ = other;
    check_eq(setenv("H", "8", 1), 0, "setenv on another array again");
    check(environ == made && strcmp(environ[0], "G=7") == 0 && strcmp(environ[1], "H=8") == 0 && environ[2] == NULL,
          "grows the array it made before, which it has not lost");
    environ = start;
}

int main(int argc, char **argv) {
    if (argc == 4 && strcmp(argv[1], "getenv") == 0) {
        return getenv_child(argv[2], argv[3]);
    }
    if (argc == 3 && strcmp(argv[1], "feed") == 0) {
        return feed(atoi(argv[2]));
    }
    if (argc == 2 && strcmp(argv[1], "nap") == 0) {
        return nap();
    }
    if (argc == 2 && strcmp(argv[1], "family") == 0) {
        return family();
    }
    test_inet();
    test_types();
    test_strings();
    test_memory_alignments();
    test_memmove_overlap();
    test_memcmp_alignments();
    test_numbers();
    test_long_long();
    test_snprintf();
    /* First, while the heap is still empty. */
    char *odd = test_odd_break();
    test_merge();
    test_malloc();
    free(odd);
    test_mapped();
    test_reader();
    test_kill_descendants();
    test_env();
    printf("libtest: %d checks, %d failed\n", checks, failures);
    return failures;
}
