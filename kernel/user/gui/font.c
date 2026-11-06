/* Text (gui.h, "Text"): PSF2 fonts and UTF-8.
 *
 * A PSF2 file (the kbd project's psf.h; crates/xtask/src/font.rs writes
 * one) is a 32-byte header of little-endian words, then the glyphs, then,
 * when flag bit 0 is set, the Unicode table:
 *
 *   magic 0x864ab572, version 0, the header's size, flags, the number of
 *   glyphs, the bytes a glyph, its height, its width
 *
 * A glyph is `height` rows of (width + 7) / 8 bytes, the leftmost pixel the
 * top bit of a row's first byte. The table has, for each glyph in order,
 * the code points it shows in UTF-8, then 0xff; 0xfe starts sequences of
 * combining characters (up to the 0xff), which this renderer has no use
 * for. Neither 0xfe nor 0xff can be a byte of UTF-8. */
#include "../gui.h"

#define PSF2_MAGIC 0x864ab572u
#define PSF2_HAS_UNICODE_TABLE 1u
#define PSF2_SEPARATOR 0xff
#define PSF2_STARTSEQ 0xfe
#define HEADER_SIZE 32
/* The largest glyphs and font this reads. */
#define MAX_SIDE 64
#define MAX_GLYPHS 65536

static int lookup(const struct gui_font *f, uint code);

static uint le32(const uchar *p) {
    return (uint)p[0] | ((uint)p[1] << 8) | ((uint)p[2] << 16) | ((uint)p[3] << 24);
}

/* Decodes the UTF-8 sequence at p (bytes up to end): stores its code
 * point (U+FFFD for a malformed one) and returns the bytes it took, 1 for a
 * malformed one. It reads no further than the first byte that does not
 * fit, so a NUL ends it. */
static int utf8_decode(const uchar *p, const uchar *end, uint *code) {
    uint b = p[0];
    int n;
    uint c;
    uint min;
    if (b < 0x80) {
        *code = b;
        return 1;
    } else if (b >= 0xc2 && b <= 0xdf) {
        n = 1;
        c = b & 0x1f;
        min = 0x80;
    } else if (b >= 0xe0 && b <= 0xef) {
        n = 2;
        c = b & 0x0f;
        min = 0x800;
    } else if (b >= 0xf0 && b <= 0xf4) {
        n = 3;
        c = b & 0x07;
        min = 0x10000;
    } else {
        *code = 0xfffd;
        return 1;
    }
    for (int i = 1; i <= n; i++) {
        if (p + i >= end || (p[i] & 0xc0) != 0x80) {
            *code = 0xfffd;
            return 1;
        }
        c = (c << 6) | (p[i] & 0x3f);
    }
    if (c < min || c > 0x10ffff || (c >= 0xd800 && c <= 0xdfff)) {
        *code = 0xfffd;
        return 1;
    }
    *code = c;
    return n + 1;
}

uint gui_utf8_next(const char **s) {
    const uchar *p = (const uchar *)*s;
    if (*p == 0) {
        return 0;
    }
    uint code;
    /* A sequence is 4 bytes at most; the NUL stops the decoder sooner. */
    *s += utf8_decode(p, p + 4, &code);
    return code;
}

int gui_utf8_put(char *out, uint code) {
    if (code > 0x10ffff || (code >= 0xd800 && code <= 0xdfff)) {
        code = 0xfffd;
    }
    if (code < 0x80) {
        out[0] = (char)code;
        return 1;
    }
    if (code < 0x800) {
        out[0] = (char)(0xc0 | (code >> 6));
        out[1] = (char)(0x80 | (code & 0x3f));
        return 2;
    }
    if (code < 0x10000) {
        out[0] = (char)(0xe0 | (code >> 12));
        out[1] = (char)(0x80 | ((code >> 6) & 0x3f));
        out[2] = (char)(0x80 | (code & 0x3f));
        return 3;
    }
    out[0] = (char)(0xf0 | (code >> 18));
    out[1] = (char)(0x80 | ((code >> 12) & 0x3f));
    out[2] = (char)(0x80 | ((code >> 6) & 0x3f));
    out[3] = (char)(0x80 | (code & 0x3f));
    return 4;
}

/* Sorts the map by code point, a code's first entry first (an insertion
 * sort: stable, and the table mostly comes in order). */
static void sort_map(struct gui_glyph_map *m, int n) {
    for (int i = 1; i < n; i++) {
        struct gui_glyph_map e = m[i];
        int j = i;
        while (j > 0 && m[j - 1].code > e.code) {
            m[j] = m[j - 1];
            j--;
        }
        m[j] = e;
    }
}

/* Reads the Unicode table at p (to end) into f: the code points below 256
 * into f->low, the others into f->map. Counts them first (map NULL), then
 * fills the map. Returns how many go in the map, or -EINVAL. */
static int read_table(struct gui_font *f, const uchar *p, const uchar *end, struct gui_glyph_map *map) {
    int n = 0;
    for (int g = 0; g < f->glyphs; g++) {
        int in_sequence = 0;
        for (;;) {
            if (p >= end) {
                return -EINVAL; /* a glyph's entry not ended */
            }
            if (*p == PSF2_SEPARATOR) {
                p++;
                break;
            }
            if (*p == PSF2_STARTSEQ) {
                in_sequence = 1;
                p++;
                continue;
            }
            uint code;
            p += utf8_decode(p, end, &code);
            if (in_sequence) {
                continue;
            }
            if (code < 256) {
                if (map == NULL && f->low[code] < 0) {
                    f->low[code] = (short)g;
                }
            } else {
                if (map != NULL) {
                    map[n].code = code;
                    map[n].glyph = (uint)g;
                }
                n++;
            }
        }
    }
    return n;
}

int gui_font_parse(struct gui_font *f, const void *data, size_t len) {
    const uchar *b = data;
    memset(f, 0, sizeof(*f));
    for (int i = 0; i < 256; i++) {
        f->low[i] = -1;
    }
    if (len < HEADER_SIZE || le32(b) != PSF2_MAGIC || le32(b + 4) != 0) {
        return -EINVAL;
    }
    uint header = le32(b + 8);
    uint flags = le32(b + 12);
    uint glyphs = le32(b + 16);
    uint size = le32(b + 20);
    uint height = le32(b + 24);
    uint width = le32(b + 28);
    if (header < HEADER_SIZE || width == 0 || height == 0 || width > MAX_SIDE || height > MAX_SIDE ||
        glyphs == 0 || glyphs > MAX_GLYPHS || size != height * ((width + 7) / 8)) {
        return -EINVAL;
    }
    size_t end_of_glyphs = (size_t)header + (size_t)glyphs * size;
    if (end_of_glyphs > len) {
        return -EINVAL;
    }
    f->width = (int)width;
    f->height = (int)height;
    f->glyphs = (int)glyphs;
    f->bytes_per_glyph = (int)size;
    f->bitmaps = b + header;
    if (flags & PSF2_HAS_UNICODE_TABLE) {
        const uchar *table = b + end_of_glyphs;
        const uchar *end = b + len;
        int n = read_table(f, table, end, NULL);
        if (n < 0) {
            return n;
        }
        if (n > 0) {
            f->map = malloc((size_t)n * sizeof(struct gui_glyph_map));
            if (f->map == NULL) {
                return -ENOMEM;
            }
            read_table(f, table, end, f->map);
            sort_map(f->map, n);
        }
        f->nmap = n;
    } else {
        /* No table: glyph n is code point n. */
        for (uint i = 0; i < 256 && i < glyphs; i++) {
            f->low[i] = (short)i;
        }
        if (glyphs > 256) {
            f->nmap = (int)glyphs - 256;
            f->map = malloc((size_t)f->nmap * sizeof(struct gui_glyph_map));
            if (f->map == NULL) {
                return -ENOMEM;
            }
            for (int i = 0; i < f->nmap; i++) {
                f->map[i].code = (uint)(256 + i);
                f->map[i].glyph = (uint)(256 + i);
            }
        }
    }
    f->fallback = lookup(f, 0xfffd);
    if (f->fallback < 0) {
        f->fallback = f->low['?'] >= 0 ? f->low['?'] : 0;
    }
    return 0;
}

int gui_font_load(struct gui_font *f, const char *path) {
    uchar *data;
    size_t len;
    int r = gui_read_file(path, &data, &len);
    if (r < 0) {
        return r;
    }
    r = gui_font_parse(f, data, len);
    if (r < 0) {
        free(f->map);
        f->map = NULL;
        free(data);
        return r;
    }
    f->file = data;
    return 0;
}

void gui_font_free(struct gui_font *f) {
    free(f->map);
    free(f->file);
    f->map = NULL;
    f->file = NULL;
    f->nmap = 0;
}

/* The glyph of `code`, or -1 if the font has none: the low table, or a
 * binary search of the map. */
static int lookup(const struct gui_font *f, uint code) {
    if (code < 256) {
        return f->low[code];
    }
    int lo = 0;
    int hi = f->nmap;
    while (lo < hi) {
        int mid = lo + (hi - lo) / 2;
        if (f->map[mid].code < code) {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    if (lo < f->nmap && f->map[lo].code == code) {
        return (int)f->map[lo].glyph;
    }
    return -1;
}

int gui_font_glyph(const struct gui_font *f, uint code) {
    int g = lookup(f, code);
    return g < 0 ? f->fallback : g;
}

/* Draws glyph g with its cell's top left at x, y, each pixel a `scale`
 * square. */
static void draw_glyph(struct gui_buffer *b, const struct gui_font *f, int g, int x, int y, uint color,
                       int scale) {
    int row_bytes = (f->width + 7) / 8;
    const uchar *rows = f->bitmaps + (long)g * f->bytes_per_glyph;
    struct gui_rect cell = gui_rect(x, y, f->width * scale, f->height * scale);
    if (gui_rect_empty(gui_rect_intersect(cell, b->clip))) {
        return;
    }
    for (int r = 0; r < f->height; r++) {
        const uchar *row = rows + r * row_bytes;
        for (int c = 0; c < f->width; c++) {
            if (row[c / 8] & (0x80 >> (c % 8))) {
                if (scale == 1) {
                    gui_plot(b, x + c, y + r, color);
                } else {
                    gui_fill(b, gui_rect(x + c * scale, y + r * scale, scale, scale), color);
                }
            }
        }
    }
}

static int scale_of(int style) {
    return (style & GUI_LARGE) ? 2 : 1;
}

int gui_text(struct gui_buffer *b, const struct gui_font *f, int x, int y, const char *s, uint color, int style) {
    int scale = scale_of(style);
    uint code;
    while ((code = gui_utf8_next(&s)) != 0) {
        int g = gui_font_glyph(f, code);
        draw_glyph(b, f, g, x, y, color, scale);
        if (style & GUI_BOLD) {
            draw_glyph(b, f, g, x + scale, y, color, scale);
        }
        x += f->width * scale;
    }
    return x;
}

int gui_text_width(const struct gui_font *f, const char *s, int style) {
    int n = 0;
    while (gui_utf8_next(&s) != 0) {
        n++;
    }
    return n * f->width * scale_of(style);
}

int gui_text_height(const struct gui_font *f, int style) {
    return f->height * scale_of(style);
}
