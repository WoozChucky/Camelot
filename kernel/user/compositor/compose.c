/* Compositing: the screen is /dev/fb0's buffer at double height, two
 * frames (docs/camelot-syscalls.md, "The framebuffer"). The server draws
 * into the one not shown and pans to it, which the display does at the
 * next vertical blank, so no frame is seen half drawn; poll tells it when
 * the flip is done (POLLOUT), and only then is the next frame drawn, at
 * most one a vertical blank.
 *
 * Only what changed is drawn again: the rectangles damaged since the last
 * frame (gui_damage), and those the last frame drew, which the frame drawn
 * now, two frames old, lacks. Each is drawn from the bottom up: the
 * background, the layer surfaces under the windows, each window in
 * stacking order (its border, title bar and close button, drawn with the
 * GUI library, and its content, copied, or blended for an ARGB8888
 * surface), the layer surfaces over them, the pointer's arrow on top. */
#include "compositor.h"

#define BACKGROUND GUI_RGB(0x2e, 0x4a, 0x62)
#define FRAME_EDGE GUI_RGB(0x20, 0x20, 0x28)
#define TITLE_ACTIVE GUI_RGB(0x30, 0x70, 0xd0)
#define TITLE_INACTIVE GUI_RGB(0x80, 0x80, 0x88)
#define TITLE_TEXT GUI_RGB(0xff, 0xff, 0xff)
#define CLOSE_FILL GUI_RGB(0xc0, 0x40, 0x40)
#define CLOSE_MARK GUI_RGB(0xff, 0xff, 0xff)

struct gui_rect window_content(const struct window *w) {
    const struct surface *s = w->surface;
    int width = s != NULL && s->has_content ? s->content.width : 0;
    int height = s != NULL && s->has_content ? s->content.height : 0;
    return gui_rect(w->x, w->y, width, height);
}

struct gui_rect window_title(const struct window *w) {
    struct gui_rect c = window_content(w);
    if (w->layer >= 0) {
        return gui_rect(c.x, c.y, 0, 0); /* a layer surface has no decorations */
    }
    return gui_rect(c.x, c.y - TITLE_H, c.w, TITLE_H);
}

struct gui_rect window_frame(const struct window *w) {
    struct gui_rect c = window_content(w);
    if (w->layer >= 0) {
        return c;
    }
    return gui_rect(c.x - BORDER, c.y - TITLE_H - BORDER, c.w + 2 * BORDER, c.h + TITLE_H + 2 * BORDER);
}

struct gui_rect window_close_button(const struct window *w) {
    struct gui_rect t = window_title(w);
    if (w->layer >= 0) {
        return t;
    }
    int margin = (TITLE_H - CLOSE_SIZE) / 2;
    return gui_rect(t.x + t.w - margin - CLOSE_SIZE, t.y + margin, CLOSE_SIZE, CLOSE_SIZE);
}

struct gui_rect cursor_rect(void) {
    if (srv.have_cursor) {
        return gui_rect(srv.px, srv.py, srv.cursor.width, srv.cursor.height);
    }
    return gui_rect(srv.px, srv.py, 6, 6);
}

void damage(struct gui_rect r) {
    r = gui_rect_intersect(r, gui_rect(0, 0, srv.width, srv.height));
    if (!gui_rect_empty(r)) {
        gui_damage_add(&srv.damage, r);
        srv.repaint = 1;
    }
}

int screen_init(void) {
    srv.fb_fd = open("/dev/fb0", O_RDWR);
    if (srv.fb_fd < 0) {
        fprintf(2, "compositor: /dev/fb0: %s\n", strerror(-srv.fb_fd));
        return -1;
    }
    struct fb_var_screeninfo var;
    struct fb_fix_screeninfo fix;
    if (ioctl(srv.fb_fd, FBIOGET_VSCREENINFO, &var) < 0 || ioctl(srv.fb_fd, FBIOGET_FSCREENINFO, &fix) < 0) {
        fprintf(2, "compositor: /dev/fb0 is no framebuffer\n");
        return -1;
    }
    /* Two frames, the second on a page's start (ypanstep's rows). */
    int step = fix.ypanstep > 0 ? fix.ypanstep : 1;
    srv.rows = ((int)var.yres + step - 1) / step * step;
    var.yres_virtual = (uint)srv.rows + var.yres;
    var.yoffset = 0;
    var.bits_per_pixel = 32;
    var.activate = FB_ACTIVATE_NOW;
    srv.double_buffered = ioctl(srv.fb_fd, FBIOPUT_VSCREENINFO, &var) == 0;
    if (!srv.double_buffered) {
        fprintf(2, "compositor: one frame only: the picture may tear\n");
    }
    if (ioctl(srv.fb_fd, FBIOGET_VSCREENINFO, &var) < 0 || ioctl(srv.fb_fd, FBIOGET_FSCREENINFO, &fix) < 0) {
        return -1;
    }
    srv.width = (int)var.xres;
    srv.height = (int)var.yres;
    void *mem = mmap(NULL, fix.smem_len, PROT_READ | PROT_WRITE, MAP_SHARED, srv.fb_fd, 0);
    if ((long)mem < 0) {
        fprintf(2, "compositor: can't map /dev/fb0: %s\n", strerror((int)-(long)mem));
        return -1;
    }
    int stride = (int)fix.line_length / 4;
    gui_buffer_init(&srv.frames[0], mem, srv.width, srv.height, stride);
    gui_buffer_init(&srv.frames[1], (uint *)mem + (srv.double_buffered ? srv.rows * stride : 0), srv.width,
                    srv.height, stride);
    srv.have_font = gui_font_load(&srv.font, FONT_PATH) == 0;
    srv.have_cursor = gui_image_load(&srv.cursor, CURSOR_PATH) == 0;
    srv.px = srv.width / 2;
    srv.py = srv.height / 2;
    srv.work = gui_rect(0, 0, srv.width, srv.height);
    /* The second frame drawn whole and flipped to; the first is drawn whole
     * once that flip is done (it is the last frame's damage then). */
    gui_damage_clear(&srv.damage);
    gui_damage_clear(&srv.prev);
    gui_damage_add(&srv.damage, gui_rect(0, 0, srv.width, srv.height));
    srv.back = 1;
    repaint();
    srv.repaint = 1;
    return 0;
}

/* A surface's content, inside the clip. */
static void draw_content(struct gui_buffer *fb, struct window *w, struct gui_rect clip) {
    struct gui_rect content = gui_rect_intersect(window_content(w), clip);
    if (gui_rect_empty(content)) {
        return;
    }
    gui_clip(fb, content);
    struct gui_buffer *src = &w->surface->content;
    struct gui_rect from = gui_rect(0, 0, src->width, src->height);
    if (w->surface->argb) {
        gui_blend(fb, w->x, w->y, src, from);
    } else {
        gui_blit(fb, w->x, w->y, src, from);
    }
}

/* A window's decorations and content, inside the frame's clip. */
static void draw_window(struct gui_buffer *fb, struct window *w, struct gui_rect clip) {
    struct gui_rect frame = window_frame(w);
    struct gui_rect area = gui_rect_intersect(frame, clip);
    if (gui_rect_empty(area)) {
        return;
    }
    if (w->layer >= 0) {
        draw_content(fb, w, clip);
        return;
    }
    gui_clip(fb, area);
    gui_outline(fb, frame, FRAME_EDGE);
    struct gui_rect title = window_title(w);
    gui_fill(fb, title, w == srv.focus ? TITLE_ACTIVE : TITLE_INACTIVE);
    struct gui_rect button = window_close_button(w);
    if (srv.have_font) {
        /* The title, up to the close button. */
        struct gui_rect text = gui_rect(title.x, title.y, button.x - title.x - 2, TITLE_H);
        struct gui_rect text_clip = gui_rect_intersect(text, area);
        if (!gui_rect_empty(text_clip)) {
            gui_clip(fb, text_clip);
            gui_text(fb, &srv.font, title.x + 4, title.y + (TITLE_H - srv.font.height) / 2, w->title, TITLE_TEXT, 0);
            gui_clip(fb, area);
        }
    }
    gui_fill(fb, button, CLOSE_FILL);
    gui_line(fb, button.x + 3, button.y + 3, button.x + button.w - 4, button.y + button.h - 4, CLOSE_MARK);
    gui_line(fb, button.x + button.w - 4, button.y + 3, button.x + 3, button.y + button.h - 4, CLOSE_MARK);
    draw_content(fb, w, clip);
}

static void draw_cursor(struct gui_buffer *fb, struct gui_rect clip) {
    struct gui_rect r = gui_rect_intersect(cursor_rect(), clip);
    if (gui_rect_empty(r)) {
        return;
    }
    gui_clip(fb, r);
    if (srv.have_cursor) {
        gui_blend(fb, srv.px, srv.py, &srv.cursor, gui_rect(0, 0, srv.cursor.width, srv.cursor.height));
    } else {
        gui_fill(fb, cursor_rect(), GUI_RGB(0xff, 0xff, 0xff));
        gui_outline(fb, cursor_rect(), GUI_RGB(0, 0, 0));
    }
}

static void draw(struct gui_buffer *fb, struct gui_rect r) {
    gui_clip(fb, r);
    gui_fill(fb, r, BACKGROUND);
    for (int i = 0; i < srv.nlayers && srv.layers[i]->layer < ZWLR_LAYER_SHELL_V1_LAYER_TOP; i++) {
        draw_window(fb, srv.layers[i], r);
    }
    for (int i = 0; i < srv.nstack; i++) {
        draw_window(fb, srv.stack[i], r);
    }
    for (int i = 0; i < srv.nlayers; i++) {
        if (srv.layers[i]->layer >= ZWLR_LAYER_SHELL_V1_LAYER_TOP) {
            draw_window(fb, srv.layers[i], r);
        }
    }
    draw_cursor(fb, r);
    gui_unclip(fb);
}

void repaint(void) {
    struct gui_damage region = srv.damage;
    for (int i = 0; i < srv.prev.count; i++) {
        gui_damage_add(&region, srv.prev.rects[i]);
    }
    struct gui_buffer *fb = &srv.frames[srv.double_buffered ? srv.back : 0];
    for (int i = 0; i < region.count; i++) {
        draw(fb, region.rects[i]);
    }
    srv.prev = srv.damage;
    gui_damage_clear(&srv.damage);
    srv.repaint = 0;
    /* The frame callbacks of what was committed go with this frame. */
    for (int i = 0; i < MAX_CLIENTS; i++) {
        if (srv.clients[i] != NULL) {
            frames_shown(srv.clients[i]);
        }
    }
    if (srv.double_buffered) {
        struct fb_var_screeninfo var;
        ioctl(srv.fb_fd, FBIOGET_VSCREENINFO, &var);
        var.xoffset = 0;
        var.yoffset = srv.back ? (uint)srv.rows : 0;
        var.activate = FB_ACTIVATE_NOW;
        ioctl(srv.fb_fd, FBIOPAN_DISPLAY, &var);
        srv.back ^= 1;
    }
    srv.flip_pending = 1;
}

void flip_done(void) {
    srv.flip_pending = 0;
    uint time = now_ms();
    for (int i = 0; i < MAX_CLIENTS; i++) {
        if (srv.clients[i] != NULL) {
            frames_done(srv.clients[i], time);
        }
    }
}
