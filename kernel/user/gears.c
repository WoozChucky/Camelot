/* gears: three gears turning, the desktop's demo (#185,
 * docs/excalibur-spec.md section 10, "The desktop shell"), after Brian
 * Paul's glxgears: the same gears (teeth, radii, widths, colours and
 * places), turning at 70 degrees a second, seen from the same angle and
 * lit by the same light. With no floating point (merlin compiles none) and
 * no GPU, everything is fixed point, Q16 (16 fractional bits): a sine table
 * computed from its Taylor series at the start, each gear's triangles
 * turned, lit by the angle of their face to the light, projected in
 * perspective and filled into the window's buffer through a depth buffer,
 * the faces turned away from the eye left out.
 *
 *   gears
 *
 * A frame is drawn each time the display server says it is a good time
 * (the frame callback), so the gears turn as fast as the machine draws
 * them; every 5 seconds the rate is printed, as glxgears does ("N frames
 * in 5.0 seconds = X FPS"). Escape or closing the window ends it (exit 0;
 * 1 if it can't start). */
#include "gui.h"

#define WIDTH 300
#define HEIGHT 300

/* Fixed point: ONE is 1.0. */
#define SHIFT 16
#define ONE (1 << SHIFT)
/* Angles in 1/TURN of a turn. */
#define TURN 4096
#define QUARTER (TURN / 4)
#define DEG(d) ((d) * TURN / 360)

/* The most triangles a gear has: 20 teeth of 22 each. */
#define MAX_TRIS 480

#define REPORT_MS 5000

struct vec {
    long x, y, z;
};

struct tri {
    struct vec v[3];
    struct vec n; /* the face's normal, ONE long, pointing out */
};

struct gear {
    struct tri tris[MAX_TRIS];
    int ntris;
    uint color; /* 0xRRGGBB */
    long x, y;  /* where it is */
    int phase;  /* its angle against the first gear's: angle * speed + phase */
    int speed;
};

static struct gear gears[3];
static struct gui_window *win;
static int *depth;
static int depth_w;
static int depth_h;
static int frame_ready = 1;
static int quit;
/* sin of 0 to a quarter turn, Q30. */
static long sine[QUARTER + 1];

/* ---- Fixed-point arithmetic ---- */

/* sin(x) = x - x^3/3! + x^5/5! - ...: up to x^13, which for x up to pi/2
 * is within 2^-30 of it. pi in Q30 is 3373259426. */
static void make_sine(void) {
    const long pi_q30 = 3373259426L;
    for (int k = 0; k <= QUARTER; k++) {
        long x = pi_q30 / 2 * k / QUARTER;
        long term = x;
        long sum = x;
        for (int n = 1; n <= 6; n++) {
            term = -((term * x) >> 30);
            term = (term * x) >> 30;
            term /= (2 * n) * (2 * n + 1);
            sum += term;
        }
        sine[k] = sum;
    }
}

/* sin and cos of angle a (in 1/TURN turns), Q16. */
static long isin(int a) {
    a %= TURN;
    if (a < 0) {
        a += TURN;
    }
    long s;
    if (a <= QUARTER) {
        s = sine[a];
    } else if (a <= 2 * QUARTER) {
        s = sine[2 * QUARTER - a];
    } else if (a <= 3 * QUARTER) {
        s = -sine[a - 2 * QUARTER];
    } else {
        s = -sine[TURN - a];
    }
    return s >> (30 - SHIFT);
}

static long icos(int a) {
    return isin(a + QUARTER);
}

static long mul(long a, long b) {
    return (a * b) >> SHIFT;
}

static long isqrt(long n) {
    if (n <= 0) {
        return 0;
    }
    long x = n;
    long y = (x + 1) / 2;
    while (y < x) {
        x = y;
        y = (x + n / x) / 2;
    }
    return x;
}

static struct vec normalize(struct vec v) {
    long len = isqrt(v.x * v.x + v.y * v.y + v.z * v.z);
    if (len == 0) {
        return v;
    }
    struct vec n = {v.x * ONE / len, v.y * ONE / len, v.z * ONE / len};
    return n;
}

/* ---- The gears' shapes (glxgears' gear()) ---- */

static struct gear *building;

static struct vec at(long r, int a, long z) {
    struct vec v = {mul(r, icos(a)), mul(r, isin(a)), z};
    return v;
}

static void add_tri(struct vec a, struct vec b, struct vec c, struct vec n) {
    if (building->ntris == MAX_TRIS) {
        return;
    }
    struct tri *t = &building->tris[building->ntris++];
    t->v[0] = a;
    t->v[1] = b;
    t->v[2] = c;
    t->n = n;
}

static void add_quad(struct vec a, struct vec b, struct vec c, struct vec d, struct vec n) {
    add_tri(a, b, c, n);
    add_tri(a, c, d, n);
}

/* A quad of the rim between two points on it (front and back), its
 * normal pointing out, at right angles to the step from p to q. */
static void rim(struct vec p, struct vec q, long half) {
    struct vec n = {q.y - p.y, p.x - q.x, 0};
    struct vec pf = {p.x, p.y, half};
    struct vec pb = {p.x, p.y, -half};
    struct vec qf = {q.x, q.y, half};
    struct vec qb = {q.x, q.y, -half};
    add_quad(pf, pb, qb, qf, normalize(n));
}

/* A gear of `teeth` teeth round a hole: radii and widths in Q16. */
static void make_gear(struct gear *g, long inner, long outer, long width, int teeth, long tooth_depth) {
    building = g;
    g->ntris = 0;
    long r0 = inner;
    long r1 = outer - tooth_depth / 2;
    long r2 = outer + tooth_depth / 2;
    long half = width / 2;
    struct vec front = {0, 0, ONE};
    struct vec back = {0, 0, -ONE};
    for (int i = 0; i < teeth; i++) {
        int a = i * TURN / teeth;
        int da = TURN / teeth / 4;
        int next = (i + 1) * TURN / teeth;
        /* The faces: the ring between the hole and the teeth's roots, then
         * the tooth, front and back. */
        add_quad(at(r0, a, half), at(r1, a, half), at(r1, next, half), at(r0, next, half), front);
        add_quad(at(r1, a, half), at(r2, a + da, half), at(r2, a + 2 * da, half), at(r1, a + 3 * da, half),
                 front);
        add_quad(at(r0, a, -half), at(r0, next, -half), at(r1, next, -half), at(r1, a, -half), back);
        add_quad(at(r1, a, -half), at(r1, a + 3 * da, -half), at(r2, a + 2 * da, -half), at(r2, a + da, -half),
                 back);
        /* The rim: up the tooth, its top, down, and the root to the next. */
        rim(at(r1, a, 0), at(r2, a + da, 0), half);
        rim(at(r2, a + da, 0), at(r2, a + 2 * da, 0), half);
        rim(at(r2, a + 2 * da, 0), at(r1, a + 3 * da, 0), half);
        rim(at(r1, a + 3 * da, 0), at(r1, next, 0), half);
        /* The hole's wall, facing in. */
        struct vec hole = {-icos(a + 2 * da), -isin(a + 2 * da), 0};
        add_quad(at(r0, a, half), at(r0, next, half), at(r0, next, -half), at(r0, a, -half), hole);
    }
}

/* ---- Drawing ---- */

/* The view: turned 20 degrees about x and 30 about y, 40 units away. */
#define VIEW_X DEG(20)
#define VIEW_Y DEG(30)
#define DISTANCE (40 * ONE)

/* The turns of a frame: the gear's (its cosine and sine), and the view's,
 * which are the same every frame. */
struct turn {
    long c, s;
    long cy, sy, cx, sx;
    long x, y; /* the gear's place */
};

/* A point of the gear in the eye's space: the gear's turn, its place
 * (unless it is a normal), then the view's turns. */
static struct vec to_eye(struct vec p, const struct turn *t, int translate) {
    struct vec q = {mul(p.x, t->c) - mul(p.y, t->s), mul(p.x, t->s) + mul(p.y, t->c), p.z};
    if (translate) {
        q.x += t->x;
        q.y += t->y;
    }
    struct vec r = {mul(q.x, t->cy) + mul(q.z, t->sy), q.y, mul(q.z, t->cy) - mul(q.x, t->sy)};
    struct vec e = {r.x, mul(r.y, t->cx) - mul(r.z, t->sx), mul(r.y, t->sx) + mul(r.z, t->cx)};
    if (translate) {
        e.z -= DISTANCE;
    }
    return e;
}

/* A point on the screen: x and y in 1/16 pixels, its depth (positive, Q8). */
struct spot {
    long x, y;
    long z;
};

static struct spot project(struct vec e, int w, int h) {
    /* glFrustum(-1, 1, -h/w, h/w, 5, 60): 5 units away is the window's
     * half width. */
    long dist = -e.z;
    long scale = 5L * w / 2;
    struct spot s = {
        (long)w * 8 + e.x * scale * 16 / dist,
        (long)h * 8 - e.y * scale * 16 / dist,
        dist >> 8,
    };
    return s;
}

static long edge(struct spot a, struct spot b, long px, long py) {
    return (px - a.x) * (b.y - a.y) - (py - a.y) * (b.x - a.x);
}

static void fill(struct gui_buffer *b, struct spot v0, struct spot v1, struct spot v2, uint color) {
    long area = edge(v0, v1, v2.x, v2.y);
    if (area == 0) {
        return;
    }
    long sign = area > 0 ? 1 : -1;
    area *= sign;
    long min_x = v0.x < v1.x ? v0.x : v1.x;
    min_x = v2.x < min_x ? v2.x : min_x;
    long max_x = v0.x > v1.x ? v0.x : v1.x;
    max_x = v2.x > max_x ? v2.x : max_x;
    long min_y = v0.y < v1.y ? v0.y : v1.y;
    min_y = v2.y < min_y ? v2.y : min_y;
    long max_y = v0.y > v1.y ? v0.y : v1.y;
    max_y = v2.y > max_y ? v2.y : max_y;
    int x0 = (int)(min_x >> 4);
    int x1 = (int)(max_x >> 4);
    int y0 = (int)(min_y >> 4);
    int y1 = (int)(max_y >> 4);
    x0 = x0 < 0 ? 0 : x0;
    y0 = y0 < 0 ? 0 : y0;
    x1 = x1 >= b->width ? b->width - 1 : x1;
    y1 = y1 >= b->height ? b->height - 1 : y1;
    if (x0 > x1 || y0 > y1) {
        return;
    }
    /* The edge functions at the first pixel's centre, and their steps. */
    long px = (long)x0 * 16 + 8;
    long py = (long)y0 * 16 + 8;
    long w0 = edge(v1, v2, px, py) * sign;
    long w1 = edge(v2, v0, px, py) * sign;
    long w2 = edge(v0, v1, px, py) * sign;
    long a0 = (v2.y - v1.y) * 16 * sign;
    long a1 = (v0.y - v2.y) * 16 * sign;
    long a2 = (v1.y - v0.y) * 16 * sign;
    long b0 = -(v2.x - v1.x) * 16 * sign;
    long b1 = -(v0.x - v2.x) * 16 * sign;
    long b2 = -(v1.x - v0.x) * 16 * sign;
    /* The depth, Q16 over the Q8 depths, and its steps. */
    long z = ((w0 * v0.z + w1 * v1.z + w2 * v2.z) << 8) / area;
    long dzx = ((a0 * v0.z + a1 * v1.z + a2 * v2.z) << 8) / area;
    long dzy = ((b0 * v0.z + b1 * v1.z + b2 * v2.z) << 8) / area;
    uint pixel = 0xff000000u | color;
    for (int y = y0; y <= y1; y++) {
        long e0 = w0;
        long e1 = w1;
        long e2 = w2;
        long zz = z;
        uint *row = b->pixels + (long)y * b->stride;
        int *drow = depth + (long)y * depth_w;
        for (int x = x0; x <= x1; x++) {
            if ((e0 | e1 | e2) >= 0) {
                int d = (int)(zz >> 8);
                if (d < drow[x]) { /* nearer */
                    drow[x] = d;
                    row[x] = pixel;
                }
            }
            e0 += a0;
            e1 += a1;
            e2 += a2;
            zz += dzx;
        }
        w0 += b0;
        w1 += b1;
        w2 += b2;
        z += dzy;
    }
}

/* The light, glxgears' (5, 5, 10), as a unit vector in the eye's space. */
static struct vec light;

static uint shade(uint color, struct vec n) {
    long d = mul(n.x, light.x) + mul(n.y, light.y) + mul(n.z, light.z);
    d = d < 0 ? 0 : d;
    /* The ambient light, 0.2, and the rest by the angle. */
    long k = ONE / 5 + d;
    k = k > ONE ? ONE : k;
    uint r = (uint)(((color >> 16) & 0xff) * k >> SHIFT);
    uint g = (uint)(((color >> 8) & 0xff) * k >> SHIFT);
    uint b = (uint)((color & 0xff) * k >> SHIFT);
    return r << 16 | g << 8 | b;
}

static void draw_gear(struct gui_buffer *b, const struct gear *g, int angle) {
    int a = angle * g->speed + g->phase;
    struct turn turn = {icos(a), isin(a), icos(VIEW_Y), isin(VIEW_Y), icos(VIEW_X), isin(VIEW_X), g->x, g->y};
    for (int i = 0; i < g->ntris; i++) {
        const struct tri *t = &g->tris[i];
        struct vec e0 = to_eye(t->v[0], &turn, 1);
        struct vec n = to_eye(t->n, &turn, 0);
        /* Facing away: the normal points the way the eye looks. */
        if (mul(n.x, e0.x) + mul(n.y, e0.y) + mul(n.z, e0.z) >= 0) {
            continue;
        }
        struct vec e1 = to_eye(t->v[1], &turn, 1);
        struct vec e2 = to_eye(t->v[2], &turn, 1);
        fill(b, project(e0, b->width, b->height), project(e1, b->width, b->height),
             project(e2, b->width, b->height), shade(g->color, n));
    }
}

static int angle;

static void draw(void) {
    struct gui_buffer *b = gui_window_buffer(win);
    if (b == NULL) {
        quit = 1;
        return;
    }
    if (depth == NULL || depth_w != b->width || depth_h != b->height) {
        free(depth);
        depth_w = b->width;
        depth_h = b->height;
        depth = malloc(sizeof(int) * (size_t)depth_w * (size_t)depth_h);
        if (depth == NULL) {
            quit = 1;
            return;
        }
    }
    /* As far as can be (the C library's memset goes a byte at a time). */
    int *end = depth + (long)depth_w * depth_h;
    for (int *p = depth; p < end; p++) {
        *p = 0x7fffffff;
    }
    gui_fill(b, gui_rect(0, 0, b->width, b->height), GUI_RGB(0, 0, 0));
    for (int i = 0; i < 3; i++) {
        draw_gear(b, &gears[i], angle);
    }
    gui_window_damage(win, gui_rect(0, 0, b->width, b->height));
    if (gui_window_commit(win) < 0) {
        quit = 1;
    }
    frame_ready = 0;
}

static void on_close(struct gui_window *w) {
    (void)w;
    quit = 1;
}

static void on_key(struct gui_window *w, uint code, int pressed) {
    (void)w;
    if (pressed && code == KEY_ESC) {
        quit = 1;
    }
}

static void on_frame(struct gui_window *w) {
    (void)w;
    frame_ready = 1;
}

static const struct gui_window_handlers handlers = {
    .close = on_close,
    .key = on_key,
    .frame = on_frame,
};

static long now_ms(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return ts.tv_sec * 1000 + ts.tv_nsec / 1000000;
}

int main(void) {
    make_sine();
    struct vec l = {5 * ONE, 5 * ONE, 10 * ONE};
    light = normalize(l);
    /* glxgears' three: (inner, outer, width, teeth, tooth depth), colour,
     * place, and how they turn against the first. */
    make_gear(&gears[0], ONE, 4 * ONE, ONE, 20, ONE * 7 / 10);
    gears[0].color = 0xcc1a00;
    gears[0].x = -3 * ONE;
    gears[0].y = -2 * ONE;
    gears[0].speed = 1;
    gears[0].phase = 0;
    make_gear(&gears[1], ONE / 2, 2 * ONE, 2 * ONE, 10, ONE * 7 / 10);
    gears[1].color = 0x00cc33;
    gears[1].x = ONE * 31 / 10;
    gears[1].y = -2 * ONE;
    gears[1].speed = -2;
    gears[1].phase = DEG(-9);
    make_gear(&gears[2], ONE * 13 / 10, 2 * ONE, ONE / 2, 10, ONE * 7 / 10);
    gears[2].color = 0x3333ff;
    gears[2].x = -ONE * 31 / 10;
    gears[2].y = ONE * 42 / 10;
    gears[2].speed = -2;
    gears[2].phase = DEG(-25);

    int err;
    struct gui_display *d = gui_connect(NULL, &err);
    if (d == NULL) {
        fprintf(2, "gears: can't connect to the display server: %s\n", strerror(-err));
        return 1;
    }
    win = gui_window_open(d, "Gears", WIDTH, HEIGHT, &handlers, NULL);
    if (win == NULL) {
        fprintf(2, "gears: can't open a window\n");
        return 1;
    }
    long start = now_ms();
    long report = start;
    int frames = 0;
    while (!quit) {
        if (frame_ready) {
            /* 70 degrees a second. */
            angle = (int)((now_ms() - start) * DEG(70) / 1000 % TURN);
            draw();
            frames++;
        }
        long t = now_ms();
        if (t - report >= REPORT_MS) {
            long tenths = frames * 10000L / (t - report);
            printf("%d frames in %ld.%ld seconds = %ld.%ld FPS\n", frames, (t - report) / 1000,
                   (t - report) / 100 % 10, tenths / 10, tenths % 10);
            frames = 0;
            report = t;
        }
        if (gui_dispatch(d, REPORT_MS) < 0) {
            break;
        }
    }
    free(depth);
    gui_window_destroy(win);
    gui_disconnect(d);
    return 0;
}
