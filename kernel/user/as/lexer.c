/* as: text to tokens, one statement at a time (merlin-as's lexer.rs).
 *
 * Comments: `#` and `//` to the end of the line, and blocks between slash-star
 * and star-slash (which may span lines). A newline or a `;` ends a
 * statement. */
#include "as.h"

static int ident_start(int c) {
    /* `@` starts GNU type names like `@function` and `@progbits`. */
    return (c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z') || c == '_' || c == '.' || c == '$' || c == '@';
}

static int ident_char(int c) {
    return (c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z') || (c >= '0' && c <= '9') || c == '_' || c == '.' ||
           c == '$';
}

void lexer_init(struct lexer *lx, const char *src, uint len) {
    lx->src = src;
    lx->len = len;
    lx->pos = 0;
    lx->line = 1;
    lx->done = 0;
}

/* The statement being lexed: one array, reused for every statement (the
 * tokens' text is in tmp memory). */
static struct token *toks;
static uint ntoks;
static uint captoks;

static struct token *push(int kind, uint line) {
    toks = grow(toks, &captoks, ntoks + 1, sizeof(struct token));
    struct token *t = &toks[ntoks++];
    /* Field by field: clearing the whole token (its 16 bytes for a wide
     * number too) for every token costs more than lexing it. */
    t->kind = kind;
    t->line = line;
    t->num.lo = 0;
    t->num.hi = 0;
    t->text = NULL;
    t->len = 0;
    t->punct = 0;
    t->fwd = 0;
    return t;
}

static int digit_value(int c) {
    if (c >= '0' && c <= '9') {
        return c - '0';
    }
    if (c >= 'a' && c <= 'f') {
        return c - 'a' + 10;
    }
    return 99;
}

/* x * r for a small r (at most 16), as a 64-bit high and low word. */
static void mul_small(uint x, uint r, uint *hi, uint *lo) {
    uint low = (x & 0xffff) * r;
    uint high = (x >> 16) * r; /* < 2^20 */
    *lo = low + (high << 16);
    *hi = (high >> 16) + (*lo < low ? 1 : 0);
}

/* Rust's u64::from_str_radix: digits of `radix`, at least one, no overflow. */
static int parse_u64(const char *s, uint n, uint radix, i64 *out) {
    if (n == 0) {
        return 0;
    }
    uint hi = 0, lo = 0;
    for (uint i = 0; i < n; i++) {
        uint d = (uint)digit_value(s[i]);
        if (d >= radix) {
            return 0;
        }
        /* (hi:lo) * radix + d, which must stay below 2^64. */
        uint lh, ll, hh, hl;
        mul_small(lo, radix, &lh, &ll);
        mul_small(hi, radix, &hh, &hl);
        uint new_hi = hl + lh;
        if (hh != 0 || new_hi < hl) {
            return 0;
        }
        uint new_lo = ll + d;
        if (new_lo < ll) {
            new_hi++;
            if (new_hi == 0) {
                return 0;
            }
        }
        hi = new_hi;
        lo = new_lo;
    }
    out->hi = hi;
    out->lo = lo;
    return 1;
}

/* A hex number of up to 32 digits, for `.file`'s MD5: big-endian bytes. */
static int parse_u128_hex(const char *s, uint n, uchar *out) {
    if (n == 0) {
        return 0;
    }
    /* Leading zeros don't count against the 32 digits (Rust's u128 parse). */
    while (n > 1 && s[0] == '0') {
        s++;
        n--;
    }
    if (n > 32) {
        return 0;
    }
    memset(out, 0, 16);
    for (uint i = 0; i < n; i++) {
        uint d = (uint)digit_value(s[i]);
        if (d >= 16) {
            return 0;
        }
        /* Shift the 16 bytes left by 4 bits, then add the digit. */
        for (uint k = 0; k < 15; k++) {
            out[k] = (uchar)((out[k] << 4) | (out[k + 1] >> 4));
        }
        out[15] = (uchar)((out[15] << 4) | d);
    }
    return 1;
}

static void error_text(uint line, const char *a, const char *text, uint n, const char *b) {
    error_at(line, fmt("%s%s%s", a, tmp_strndup(text, n), b));
}

/* A number token (merlin-as's lex_number): `1b`/`1f` label references,
 * then 0x, 0b, 0o and decimal, with `_` separators. */
static int lex_number(const char *text, uint n, uint line) {
    /* `1b` / `1f`: a numeric label reference, unless it parses as binary. */
    if (n >= 2 && (text[n - 1] == 'b' || text[n - 1] == 'f')) {
        int digits = 1;
        for (uint i = 0; i + 1 < n; i++) {
            if (text[i] < '0' || text[i] > '9') {
                digits = 0;
            }
        }
        if (digits) {
            i64 v;
            if (!parse_u64(text, n - 1, 10, &v) || v.hi != 0) {
                error_at(line, "label number too large");
                return 0;
            }
            struct token *t = push(T_LOCALREF, line);
            t->num = v;
            t->fwd = text[n - 1] == 'f';
            return 1;
        }
    }
    char *lower = tmp_alloc(n + 1);
    uint m = 0;
    for (uint i = 0; i < n; i++) {
        char c = text[i];
        if (c >= 'A' && c <= 'Z') {
            c = (char)(c - 'A' + 'a');
        }
        if (c != '_') {
            lower[m++] = c;
        }
    }
    i64 v;
    int ok;
    int hex = m >= 2 && lower[0] == '0' && lower[1] == 'x';
    if (hex) {
        ok = parse_u64(lower + 2, m - 2, 16, &v);
    } else if (m >= 2 && lower[0] == '0' && lower[1] == 'b') {
        ok = parse_u64(lower + 2, m - 2, 2, &v);
    } else if (m >= 2 && lower[0] == '0' && lower[1] == 'o') {
        ok = parse_u64(lower + 2, m - 2, 8, &v);
    } else {
        ok = parse_u64(lower, m, 10, &v);
    }
    if (ok) {
        struct token *t = push(T_INT, line);
        t->num = v;
        return 1;
    }
    if (hex) {
        uchar wide[16];
        if (parse_u128_hex(lower + 2, m - 2, wide)) {
            struct token *t = push(T_WIDE, line);
            memcpy(t->wide, wide, 16);
            return 1;
        }
    }
    error_text(line, "invalid number '", text, n, "'");
    return 0;
}

/* A byte as Rust's `b as char` prints it: Latin-1, in UTF-8. */
static void char_text(uint b, char *out) {
    if (b < 0x80) {
        out[0] = (char)b;
        out[1] = 0;
    } else {
        out[0] = (char)(0xc0 | (b >> 6));
        out[1] = (char)(0x80 | (b & 0x3f));
        out[2] = 0;
    }
}

/* A quoted string or character literal from s[i] on (after the opening
 * quote): its bytes into *out, the index after the closing quote into
 * *next. 0 after an error. */
static int lex_quoted(const char *s, uint len, uint i, int quote, uint line, uchar **out, uint *outlen,
                      uint *next) {
    /* A string ends on its line: room for the rest of the line. */
    uint end = i;
    while (end < len && s[end] != '\n') {
        end++;
    }
    uchar *bytes = tmp_alloc(end - i + 1);
    uint n = 0;
    for (;;) {
        if (i >= len || s[i] == '\n') {
            error_at(line, "unterminated string");
            return 0;
        }
        uint c = (uchar)s[i];
        if (c == (uint)quote) {
            *out = bytes;
            *outlen = n;
            *next = i + 1;
            return 1;
        }
        if (c != '\\') {
            bytes[n++] = (uchar)c;
            i++;
            continue;
        }
        if (i + 1 >= len) {
            error_at(line, "unterminated string");
            return 0;
        }
        uint e = (uchar)s[i + 1];
        i += 2;
        if (e == 'n') {
            bytes[n++] = '\n';
        } else if (e == 't') {
            bytes[n++] = '\t';
        } else if (e == 'r') {
            bytes[n++] = '\r';
        } else if (e >= '0' && e <= '7') {
            /* Octal, one to three digits: `\0`, `\12`, `\377`. */
            uint value = e - '0';
            int digits = 1;
            while (digits < 3 && i < len && s[i] >= '0' && s[i] <= '7') {
                value = value * 8 + (uint)(s[i] - '0');
                digits++;
                i++;
            }
            if (value > 255) {
                error_at(line, "octal escape out of range");
                return 0;
            }
            bytes[n++] = (uchar)value;
        } else if (e == '\\' || e == '"' || e == '\'') {
            bytes[n++] = (uchar)e;
        } else if (e == 'x') {
            /* Exactly two characters, as Rust's u8::from_str_radix(.., 16)
             * reads them (which takes a leading '+'). */
            if (i + 2 > len) {
                error_at(line, "bad \\x escape");
                return 0;
            }
            int a = s[i], b = s[i + 1];
            int ha = digit_value(a >= 'A' && a <= 'F' ? a - 'A' + 'a' : a);
            int hb = digit_value(b >= 'A' && b <= 'F' ? b - 'A' + 'a' : b);
            uint v;
            if (a == '+' && hb < 16) {
                v = (uint)hb;
            } else if (ha < 16 && hb < 16) {
                v = (uint)(ha * 16 + hb);
            } else {
                error_at(line, "bad \\x escape");
                return 0;
            }
            i += 2;
            bytes[n++] = (uchar)v;
        } else {
            char ch[4];
            char_text(e, ch);
            error_at(line, fmt("unknown escape '\\%s'", ch));
            return 0;
        }
    }
}

int lex_statement(struct lexer *lx, struct token **out) {
    if (lx->done) {
        return 0;
    }
    ntoks = 0;
    const char *s = lx->src;
    uint len = lx->len;
    uint i = lx->pos;
    for (;;) {
        if (i >= len) {
            /* The end: one last statement end, as merlin-as has. */
            push(T_NEWLINE, lx->line);
            lx->done = 1;
            break;
        }
        int c = (uchar)s[i];
        if (c == '\n') {
            push(T_NEWLINE, lx->line);
            lx->line++;
            i++;
            break;
        }
        if (c == ';') {
            push(T_NEWLINE, lx->line);
            i++;
            break;
        }
        if (c == ' ' || c == '\t' || c == '\r') {
            i++;
        } else if (c == '#' || (c == '/' && i + 1 < len && s[i + 1] == '/')) {
            while (i < len && s[i] != '\n') {
                i++;
            }
        } else if (c == '/' && i + 1 < len && s[i + 1] == '*') {
            uint start = lx->line;
            i += 2;
            for (;;) {
                if (i + 1 >= len) {
                    error_at(start, "unterminated /* comment");
                    return -1;
                }
                if (s[i] == '*' && s[i + 1] == '/') {
                    i += 2;
                    break;
                }
                if (s[i] == '\n') {
                    lx->line++;
                }
                i++;
            }
        } else if (c == '"') {
            uchar *bytes;
            uint n, next;
            if (!lex_quoted(s, len, i + 1, '"', lx->line, &bytes, &n, &next)) {
                return -1;
            }
            struct token *t = push(T_STR, lx->line);
            t->text = (char *)bytes;
            t->len = n;
            i = next;
        } else if (c == '\'') {
            uchar *bytes;
            uint n, next;
            if (!lex_quoted(s, len, i + 1, '\'', lx->line, &bytes, &n, &next)) {
                return -1;
            }
            if (n != 1) {
                error_at(lx->line, "character literal must hold one character");
                return -1;
            }
            struct token *t = push(T_INT, lx->line);
            t->num = i64_u(bytes[0]);
            i = next;
        } else if (c >= '0' && c <= '9') {
            uint start = i;
            while (i < len && ident_char((uchar)s[i])) {
                i++;
            }
            if (!lex_number(s + start, i - start, lx->line)) {
                return -1;
            }
        } else if (ident_start(c)) {
            uint start = i;
            i++;
            while (i < len && ident_char((uchar)s[i])) {
                i++;
            }
            struct token *t = push(T_IDENT, lx->line);
            t->text = tmp_strndup(s + start, i - start);
            t->len = i - start;
        } else if (c == '<' && i + 1 < len && s[i + 1] == '<') {
            push(T_SHIFT, lx->line)->fwd = 1;
            i += 2;
        } else if (c == '>' && i + 1 < len && s[i + 1] == '>') {
            push(T_SHIFT, lx->line)->fwd = 0;
            i += 2;
        } else if (c == ',' || c == '(' || c == ')' || c == ':' || c == '+' || c == '-' || c == '*' || c == '/' ||
                   c == '%' || c == '~' || c == '&' || c == '|' || c == '^' || c == '=') {
            push(T_PUNCT, lx->line)->punct = c;
            i++;
        } else {
            /* The whole character, as Rust shows it: its UTF-8 bytes. */
            uint n = c >= 0xf0 ? 4 : c >= 0xe0 ? 3 : c >= 0xc0 ? 2 : 1;
            if (i + n > len) {
                n = len - i;
            }
            error_text(lx->line, "unexpected character '", s + i, n, "'");
            return -1;
        }
    }
    lx->pos = i;
    *out = toks;
    return (int)ntoks;
}
