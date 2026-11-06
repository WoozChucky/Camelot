/* Images (gui.h, "Images"): QOI, the Quite OK Image format (qoiformat.org,
 * the specification of 2022), and reading whole files.
 *
 * A QOI file is a 14-byte header, "qoif", the width and the height
 * (big-endian u32s), the channels (3, RGB, or 4, RGBA) and the colour
 * space, then the pixels in rows as chunks, then seven 0 bytes and a 1.
 * The decoder keeps the previous pixel (at first 0, 0, 0, 255) and 64
 * pixels seen, each at (r * 3 + g * 5 + b * 7 + a * 11) % 64. A chunk is,
 * by its first byte:
 *
 *   0xfe          RGB: r, g, b follow; alpha as before
 *   0xff          RGBA: r, g, b, a follow
 *   00iiiiii      INDEX: the pixel seen at i
 *   01rrggbb      DIFF: each of r, g, b plus (2 bits) - 2, wrapping
 *   10gggggg      LUMA: g plus (6 bits) - 32; the next byte, rrrrbbbb,
 *                 r and b plus that much and (4 bits) - 8 more
 *   11nnnnnn      RUN: the previous pixel n + 1 times (n up to 61)
 *
 * and every pixel decoded goes into the 64 at its hash. */
#include "../gui.h"

#define QOI_HEADER 14
#define QOI_END 8
#define QOI_OP_INDEX 0x00
#define QOI_OP_DIFF 0x40
#define QOI_OP_LUMA 0x80
#define QOI_OP_RUN 0xc0
#define QOI_OP_RGB 0xfe
#define QOI_OP_RGBA 0xff
#define QOI_MASK 0xc0
/* The most pixels an image may have: 4096 x 4096. */
#define MAX_PIXELS (4096 * 4096)
/* The largest file gui_read_file reads. */
#define MAX_FILE (16 * 1024 * 1024)

static uint be32(const uchar *p) {
    return ((uint)p[0] << 24) | ((uint)p[1] << 16) | ((uint)p[2] << 8) | (uint)p[3];
}

static uint argb(uint r, uint g, uint b, uint a) {
    return ((a & 0xff) << 24) | ((r & 0xff) << 16) | ((g & 0xff) << 8) | (b & 0xff);
}

int gui_qoi_decode(struct gui_buffer *img, const void *data, size_t len) {
    const uchar *p = data;
    if (len < QOI_HEADER + QOI_END || p[0] != 'q' || p[1] != 'o' || p[2] != 'i' || p[3] != 'f') {
        return -EINVAL;
    }
    uint width = be32(p + 4);
    uint height = be32(p + 8);
    uint channels = p[12];
    if (width == 0 || height == 0 || width > 4096 || height > 4096 || width * height > MAX_PIXELS ||
        (channels != 3 && channels != 4) || p[13] > 1) {
        return -EINVAL;
    }
    int r = gui_buffer_alloc(img, (int)width, (int)height);
    if (r < 0) {
        return r;
    }
    uint seen[64];
    memset(seen, 0, sizeof(seen));
    uint pr = 0, pg = 0, pb = 0, pa = 255;
    size_t at = QOI_HEADER;
    size_t end = len - QOI_END;
    uint run = 0;
    uint total = width * height;
    for (uint i = 0; i < total; i++) {
        if (run > 0) {
            run--;
        } else {
            if (at >= end) {
                gui_buffer_free(img);
                return -EINVAL;
            }
            uint b = p[at++];
            if (b == QOI_OP_RGB || b == QOI_OP_RGBA) {
                size_t need = b == QOI_OP_RGB ? 3 : 4;
                if (at + need > end) {
                    gui_buffer_free(img);
                    return -EINVAL;
                }
                pr = p[at];
                pg = p[at + 1];
                pb = p[at + 2];
                if (b == QOI_OP_RGBA) {
                    pa = p[at + 3];
                }
                at += need;
            } else if ((b & QOI_MASK) == QOI_OP_INDEX) {
                uint px = seen[b];
                pa = px >> 24;
                pr = (px >> 16) & 0xff;
                pg = (px >> 8) & 0xff;
                pb = px & 0xff;
            } else if ((b & QOI_MASK) == QOI_OP_DIFF) {
                pr = (pr + ((b >> 4) & 3) - 2) & 0xff;
                pg = (pg + ((b >> 2) & 3) - 2) & 0xff;
                pb = (pb + (b & 3) - 2) & 0xff;
            } else if ((b & QOI_MASK) == QOI_OP_LUMA) {
                if (at >= end) {
                    gui_buffer_free(img);
                    return -EINVAL;
                }
                uint b2 = p[at++];
                uint dg = (b & 0x3f) - 32;
                pr = (pr + dg + ((b2 >> 4) & 0x0f) - 8) & 0xff;
                pg = (pg + dg) & 0xff;
                pb = (pb + dg + (b2 & 0x0f) - 8) & 0xff;
            } else {
                run = b & 0x3f; /* this pixel, then `run` more */
            }
            seen[(pr * 3 + pg * 5 + pb * 7 + pa * 11) % 64] = argb(pr, pg, pb, pa);
        }
        img->pixels[i] = argb(pr, pg, pb, pa);
    }
    return 0;
}

int gui_read_file(const char *path, uchar **data, size_t *len) {
    struct stat st;
    int fd = open(path, O_RDONLY);
    if (fd < 0) {
        return fd;
    }
    int r = fstat(fd, &st);
    if (r < 0) {
        close(fd);
        return r;
    }
    if (st.size <= 0 || st.size > MAX_FILE) {
        close(fd);
        return -EFBIG;
    }
    uchar *buf = malloc((size_t)st.size);
    if (buf == NULL) {
        close(fd);
        return -ENOMEM;
    }
    off_t got = 0;
    while (got < st.size) {
        ssize_t n = read(fd, buf + got, (size_t)(st.size - got));
        if (n <= 0) {
            close(fd);
            free(buf);
            return n < 0 ? (int)n : -EIO;
        }
        got += n;
    }
    close(fd);
    *data = buf;
    *len = (size_t)st.size;
    return 0;
}

int gui_image_load(struct gui_buffer *img, const char *path) {
    uchar *data;
    size_t len;
    int r = gui_read_file(path, &data, &len);
    if (r < 0) {
        return r;
    }
    r = gui_qoi_decode(img, data, len);
    free(data);
    return r;
}
