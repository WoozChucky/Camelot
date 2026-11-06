/* fbtest: checks the framebuffer device, /dev/fb0 (docs/camelot-syscalls.md,
 * "The framebuffer"; kernel/include/fb.h): Linux's fbdev requests, the
 * modes, a double-height buffer in RAM flipped with FBIOPAN_DISPLAY, the
 * vertical blank, and the errors. Prints a line for each check that fails
 * and, at the end, "fbtest: N checks, F failed"; exits with F. On a root
 * image, which has the node.
 *
 *   fbtest        the checks
 *   fbtest show   800x600 from a double-height buffer, its second frame
 *                 (from row 608, the first on a page) shown by a flip: prints "fbtest: shown" and waits for a
 *                 line before it lets go of the device, for a test to look
 *                 at the display meanwhile */
#include "user.h"

#define PAGE 4096ul

static int checks;
static int failed;

static void check(int ok, const char *what) {
    checks++;
    if (!ok) {
        failed++;
        printf("fbtest: FAILED: %s\n", what);
    }
}

static void check_eq(long got, long want, const char *what) {
    checks++;
    if (got != want) {
        failed++;
        printf("fbtest: FAILED: %s: got %ld, want %ld\n", what, got, want);
    }
}

/* The pixel fbtest draws at (x, y) of frame `frame`: the frame in blue, the
 * column's low bits in red, the row's in green. */
static uint pixel(uint frame, uint x, uint y) {
    return (x & 0xff) << 16 | (y & 0xff) << 8 | (0x40 + frame);
}

/* Draws frame `frame` into `buf` from row `row0`. */
static void draw(uint *buf, uint width, uint height, uint row0, uint frame) {
    uint *row = buf + (size_t)row0 * width;
    for (uint y = 0; y < height; y++) {
        for (uint x = 0; x < width; x++) {
            row[(size_t)y * width + x] = pixel(frame, x, y);
        }
    }
}

static long map(size_t len, int fd) {
    return (long)mmap(0, len, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
}

/* A mode's var, as a program asks for one. */
static void want_mode(struct fb_var_screeninfo *v, uint w, uint h, uint vh) {
    memset(v, 0, sizeof *v);
    v->xres = w;
    v->yres = h;
    v->xres_virtual = w;
    v->yres_virtual = vh;
    v->bits_per_pixel = 32;
}

static void info_checks(int fd) {
    struct fb_var_screeninfo v;
    struct fb_fix_screeninfo f;
    check_eq(ioctl(fd, FBIOGET_VSCREENINFO, &v), 0, "FBIOGET_VSCREENINFO");
    check(v.xres == 640 && v.yres == 480 && v.xres_virtual == 640 && v.yres_virtual == 480,
          "640x480, one frame");
    check(v.xoffset == 0 && v.yoffset == 0 && v.bits_per_pixel == 32, "from row 0, 32 bits a pixel");
    check(v.red.offset == 16 && v.red.length == 8 && v.green.offset == 8 && v.green.length == 8 &&
              v.blue.offset == 0 && v.blue.length == 8 && v.transp.length == 0,
          "XRGB8888");
    check_eq(ioctl(fd, FBIOGET_FSCREENINFO, &f), 0, "FBIOGET_FSCREENINFO");
    check(strcmp(f.id, "grail-fb") == 0, "its id");
    check(f.smem_start == 0x31000000ul && f.smem_len == 640 * 480 * 4, "VRAM");
    check(f.line_length == 2560 && f.type == FB_TYPE_PACKED_PIXELS && f.visual == FB_VISUAL_TRUECOLOR,
          "packed truecolor rows of 2560 bytes");
    check_eq(f.ypanstep, 8, "a pan moves by 8 rows, 5 pages");

    /* The errors: a request no device knows, a bad pointer, a closed
     * descriptor; the request is an unsigned int, as Linux takes it. */
    check_eq(ioctl(fd, 0x4680, &v), -ENOTTY, "an unknown request");
    check_eq(ioctl(fd, 0x5401, &v), -ENOTTY, "a terminal's request");
    check_eq(ioctl(fd, FBIOGET_VSCREENINFO, (void *)8), -EFAULT, "GET into page 0");
    check_eq(ioctl(fd, FBIOGET_FSCREENINFO, (void *)0xfffffffffffff000ul), -EFAULT, "GET into the kernel");
    check_eq(syscall(53, fd, 0x100000000l | FBIOGET_VSCREENINFO, (long)&v), 0,
             "the request's upper half ignored");
}

static void mode_checks(int fd) {
    struct fb_var_screeninfo v;
    struct fb_fix_screeninfo f;
    /* What FB_ACTIVATE_TEST says, changing nothing. */
    want_mode(&v, 800, 600, 0);
    v.bits_per_pixel = 0;
    v.activate = FB_ACTIVATE_TEST;
    check_eq(ioctl(fd, FBIOPUT_VSCREENINFO, &v), 0, "FB_ACTIVATE_TEST 800x600");
    check(v.xres_virtual == 800 && v.yres_virtual == 600 && v.bits_per_pixel == 32 && v.red.offset == 16,
          "filled in as it would be set");
    check_eq(ioctl(fd, FBIOGET_VSCREENINFO, &v), 0, "GET after the test");
    check_eq(v.xres, 640, "the mode not set");

    /* What no mode is. */
    want_mode(&v, 700, 500, 500);
    check_eq(ioctl(fd, FBIOPUT_VSCREENINFO, &v), -EINVAL, "700x500");
    want_mode(&v, 640, 480, 480);
    v.bits_per_pixel = 16;
    check_eq(ioctl(fd, FBIOPUT_VSCREENINFO, &v), -EINVAL, "16 bits a pixel");
    want_mode(&v, 640, 480, 3 * 480);
    check_eq(ioctl(fd, FBIOPUT_VSCREENINFO, &v), -EINVAL, "three frames");
    want_mode(&v, 640, 480, 961);
    v.activate = FB_ACTIVATE_TEST;
    check_eq(ioctl(fd, FBIOPUT_VSCREENINFO, &v), -EINVAL, "a row more than two frames");
    /* 800x600's frame is no whole number of pages: its second frame starts
     * at row 608, the first on a page. */
    want_mode(&v, 800, 600, 1208);
    v.activate = FB_ACTIVATE_TEST;
    check_eq(ioctl(fd, FBIOPUT_VSCREENINFO, &v), 0, "800x600 with room for a frame from row 608");
    want_mode(&v, 800, 600, 1209);
    v.activate = FB_ACTIVATE_TEST;
    check_eq(ioctl(fd, FBIOPUT_VSCREENINFO, &v), -EINVAL, "and not a row more");
    want_mode(&v, 640, 480, 960);
    v.xres_virtual = 1280;
    check_eq(ioctl(fd, FBIOPUT_VSCREENINFO, &v), -EINVAL, "wider than the mode");
    want_mode(&v, 640, 480, 960);
    v.yoffset = 4;
    check_eq(ioctl(fd, FBIOPUT_VSCREENINFO, &v), -EINVAL, "from a row off a page");
    want_mode(&v, 640, 480, 960);
    v.yoffset = 488;
    check_eq(ioctl(fd, FBIOPUT_VSCREENINFO, &v), -EINVAL, "from a row past the second frame's");
    check_eq(ioctl(fd, FBIOPUT_VSCREENINFO, (void *)8), -EFAULT, "PUT from page 0");

    /* 800x600 in RAM: the display shows it from there. */
    want_mode(&v, 800, 600, 600);
    check_eq(ioctl(fd, FBIOPUT_VSCREENINFO, &v), 0, "800x600");
    check_eq(ioctl(fd, FBIOGET_FSCREENINFO, &f), 0, "its FSCREENINFO");
    check(f.smem_start != 0x31000000ul && f.smem_start % PAGE == 0, "a buffer on a page of RAM");
    check(f.smem_len == 800 * 600 * 4 && f.line_length == 3200, "800x600x4, rows of 3200");
    check_eq(f.ypanstep, 32, "a pan of 32 rows, 25 pages");
    long m = map(f.smem_len, fd);
    check(m > 0, "map it");
    if (m > 0) {
        uint *px = (uint *)m;
        check(px[0] == 0 && px[800 * 600 - 1] == 0, "cleared");
        draw(px, 800, 600, 0, 0);
        /* The buffer is mapped: no mode that needs another. */
        want_mode(&v, 640, 480, 480);
        check_eq(ioctl(fd, FBIOPUT_VSCREENINFO, &v), -EBUSY, "640x480 while mapped");
        want_mode(&v, 800, 600, 600);
        check_eq(ioctl(fd, FBIOPUT_VSCREENINFO, &v), 0, "the same mode again while mapped");
        check_eq(munmap(px, f.smem_len), 0, "munmap it");
    }
    want_mode(&v, 640, 480, 480);
    check_eq(ioctl(fd, FBIOPUT_VSCREENINFO, &v), 0, "back to 640x480");
    check_eq(ioctl(fd, FBIOGET_FSCREENINFO, &f), 0, "FSCREENINFO");
    check(f.smem_start == 0x31000000ul, "in VRAM again");
}

static void flip_checks(int fd) {
    struct fb_var_screeninfo v;
    struct fb_fix_screeninfo f;
    want_mode(&v, 640, 480, 960);
    check_eq(ioctl(fd, FBIOPUT_VSCREENINFO, &v), 0, "640x480, two frames");
    check_eq(ioctl(fd, FBIOGET_FSCREENINFO, &f), 0, "its FSCREENINFO");
    check(f.smem_len == 640 * 960 * 4 && f.smem_start != 0x31000000ul, "two frames in RAM");
    long m = map(f.smem_len, fd);
    check(m > 0, "map both");
    if (m <= 0) {
        return;
    }
    uint *px = (uint *)m;
    draw(px, 640, 480, 0, 0);
    draw(px, 640, 480, 480, 1);
    check_eq(ioctl(fd, FBIOGET_VSCREENINFO, &v), 0, "GET");
    check_eq(v.yoffset, 0, "the first frame shown");
    v.yoffset = 480;
    v.activate = FB_ACTIVATE_VBL;
    check_eq(ioctl(fd, FBIOPAN_DISPLAY, &v), 0, "flip to the second, waiting for it");
    check_eq(ioctl(fd, FBIOGET_VSCREENINFO, &v), 0, "GET");
    check_eq(v.yoffset, 480, "the second frame shown");
    v.yoffset = 0;
    v.activate = 0;
    check_eq(ioctl(fd, FBIOPAN_DISPLAY, &v), 0, "flip back, not waiting");
    uint crtc = 0;
    check_eq(ioctl(fd, FBIO_WAITFORVSYNC, &crtc), 0, "FBIO_WAITFORVSYNC");
    check_eq(ioctl(fd, FBIO_WAITFORVSYNC, &crtc), 0, "and the next");
    crtc = 1;
    check_eq(ioctl(fd, FBIO_WAITFORVSYNC, &crtc), -ENODEV, "a second display");
    check_eq(ioctl(fd, FBIO_WAITFORVSYNC, (void *)8), -EFAULT, "FBIO_WAITFORVSYNC from page 0");
    v.yoffset = 4;
    check_eq(ioctl(fd, FBIOPAN_DISPLAY, &v), -EINVAL, "a pan off a page");
    v.yoffset = 488;
    check_eq(ioctl(fd, FBIOPAN_DISPLAY, &v), -EINVAL, "a pan past the buffer");
    v.yoffset = 0;
    v.xoffset = 8;
    check_eq(ioctl(fd, FBIOPAN_DISPLAY, &v), -EINVAL, "a pan sideways");
    check_eq((long)mmap(0, f.smem_len + PAGE, PROT_READ, MAP_SHARED, fd, 0), -EINVAL,
             "a mapping past the buffer");
    check_eq(ioctl(fd, FBIOBLANK, (void *)1), 0, "FBIOBLANK");
    check_eq(ioctl(fd, FBIOBLANK, (void *)5), -EINVAL, "a blank level past FB_BLANK_POWERDOWN");
    check_eq(ioctl(fd, FBIOBLANK, (void *)FB_BLANK_UNBLANK), 0, "unblank");
    check_eq(munmap(px, f.smem_len), 0, "munmap them");
}

/* `fbtest show`: 800x600, two frames, the second shown by a flip. */
static int show(int fd) {
    struct fb_var_screeninfo v;
    struct fb_fix_screeninfo f;
    want_mode(&v, 800, 600, 1208);
    if (ioctl(fd, FBIOPUT_VSCREENINFO, &v) != 0 || ioctl(fd, FBIOGET_FSCREENINFO, &f) != 0) {
        printf("fbtest: no 800x600 with two frames\n");
        return 1;
    }
    long m = map(f.smem_len, fd);
    if (m <= 0) {
        printf("fbtest: no mapping: %ld\n", m);
        return 1;
    }
    draw((uint *)m, 800, 600, 0, 0);
    draw((uint *)m, 800, 600, 608, 1);
    v.yoffset = 608;
    v.activate = FB_ACTIVATE_VBL;
    if (ioctl(fd, FBIOPAN_DISPLAY, &v) != 0) {
        printf("fbtest: no flip\n");
        return 1;
    }
    printf("fbtest: shown\n");
    char c;
    while (read(0, &c, 1) == 1 && c != '\n') {
    }
    return 0;
}

int main(int argc, char **argv) {
    int fd = open("/dev/fb0", O_RDWR);
    if (fd < 0) {
        printf("fbtest: no /dev/fb0: %d\n", fd);
        return 1;
    }
    if (argc == 2 && strcmp(argv[1], "show") == 0) {
        return show(fd);
    }
    info_checks(fd);
    mode_checks(fd);
    flip_checks(fd);
    close(fd);
    printf("fbtest: %d checks, %d failed\n", checks, failed);
    return failed;
}
