/* libtest: checks the C library (lib/, lib/README.md) from a program:
 * strings and memory, numbers, snprintf's formats, malloc and free (and
 * how free merges neighbours), and the readers on a pipe. Needs no
 * filesystem. Prints a line for each check that fails and, at the end,
 * "libtest: N checks, F failed"; exits with F.
 *
 * `libtest feed N` is the child it runs: writes N numbered lines to its
 * output, in writes that cut lines in two. */
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

static void check_eq(int got, int want, const char *what) {
    checks++;
    if (got != want) {
        failures++;
        printf("libtest: FAILED: %s: got %d, want %d\n", what, got, want);
    }
}

static void check_str(const char *got, const char *want, const char *what) {
    checks++;
    if (strcmp(got, want) != 0) {
        failures++;
        printf("libtest: FAILED: %s: got \"%s\", want \"%s\"\n", what, got, want);
    }
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
    /* Out of range: the limit, as in C, not a wrapped-around value. */
    check_eq((int)strtol("2147483647", NULL, 10), LONG_MAX, "strtol of LONG_MAX");
    check_eq((int)strtol("-2147483648", NULL, 10), LONG_MIN, "strtol of LONG_MIN");
    check_eq((int)strtol("2147483648", &end, 10), LONG_MAX, "strtol just past LONG_MAX");
    check_eq((int)strtol("-2147483649", NULL, 10), LONG_MIN, "strtol just past LONG_MIN");
    check_eq((int)strtol("4294967302x", &end, 10), LONG_MAX, "strtol of 2^32 + 6 does not wrap to 6");
    check(*end == 'x', "and it ends after all the digits");
    check_eq((int)strtol("0x100000000", NULL, 0), LONG_MAX, "strtol of a hex number out of range");
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
    check_str(buf, "0x00001000 (null)", "%p and a null string");
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

/* A block of this many bytes is two pages with its 8-byte header: more
 * than any free block before test_merge (it runs before anything else
 * uses the heap) and at least the page malloc grows the heap by, so each
 * comes from the top of the heap, sbrk asked for exactly it. */
#define MERGE_BYTES (2 * 4096 - 8)

/* Three neighbouring blocks of MERGE_BYTES, lowest first, from fresh heap
 * at its top; 0 if they aren't that. */
static int three_neighbours(char **blk) {
    int top = sbrk(0);
    for (int i = 0; i < 3; i++) {
        blk[i] = malloc(MERGE_BYTES);
    }
    return blk[0] == (char *)top + 8 && blk[1] == blk[0] + MERGE_BYTES + 8 &&
           blk[2] == blk[1] + MERGE_BYTES + 8 && sbrk(0) == top + 3 * (MERGE_BYTES + 8);
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
    int top = sbrk(0);
    char *p = malloc(3 * MERGE_BYTES + 16);
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
    check(((uint)a & 7) == 0 && ((uint)b & 7) == 0 && ((uint)c & 7) == 0, "8-byte aligned");
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
    int top = sbrk(0);
    char *e = malloc(3000);
    check(e != NULL && sbrk(0) == top, "freed blocks are used again");
    free(e);
    /* Grow past a page: sbrk is asked for more. */
    char *big = malloc(100000);
    check(big != NULL && sbrk(0) > top, "a big block grows the heap");
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
    check(calloc(0x10000, 0x10000) == NULL, "calloc overflow");
    check(malloc(0x7fffffff) == NULL, "malloc of more than there is");
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

/* libtest feed N: N lines "line K", written in pieces of 7 bytes. */
static int feed(int n) {
    static char text[1024];
    int len = 0;
    for (int k = 1; k <= n; k++) {
        len += snprintf(text + len, sizeof text - (size_t)len, "line %d\n", k);
    }
    for (int at = 0; at < len; at += 7) {
        int m = len - at < 7 ? len - at : 7;
        if (write(STDOUT, text + at, (uint)m) != m) {
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
    int pid = spawn("/bin/libtest", (char *[]){"libtest", "feed", "100", NULL});
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

int main(int argc, char **argv) {
    if (argc == 3 && strcmp(argv[1], "feed") == 0) {
        return feed(atoi(argv[2]));
    }
    test_inet();
    test_strings();
    test_numbers();
    test_snprintf();
    /* First, while the heap is still empty. */
    test_merge();
    test_malloc();
    test_reader();
    printf("libtest: %d checks, %d failed\n", checks, failures);
    return failures;
}
