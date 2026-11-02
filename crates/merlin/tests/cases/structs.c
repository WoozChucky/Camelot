// expect: 16
// Structs: members through `.` and `->`, nesting, arrays of structs, struct
// assignment and initialization.

struct point {
    int x;
    int y;
};

struct rect {
    struct point min, max;
    char tag;
};

struct point origin;
struct rect unit = {{0, 0}, {1, 1}, 'u'};

int area(struct rect *r) {
    return (r->max.x - r->min.x) * (r->max.y - r->min.y);
}

void grow(struct rect *r, int by) {
    r->max.x += by;
    r->max.y = r->max.y + by;
}

int main(void) {
    int ok = 0;
    struct rect r;
    struct point pts[3];
    struct point *p = pts;
    int i;
    r.min.x = 1;
    r.min.y = 2;
    r.max.x = 4;
    r.max.y = 6;
    r.tag = 'r';
    ok += area(&r) == 12;
    grow(&r, 2);
    ok += area(&r) == 30;
    ok += r.tag == 'r';
    for (i = 0; i < 3; i++) {
        pts[i].x = i;
        pts[i].y = i * i;
    }
    ok += pts[2].y == 4;
    ok += (p + 1)->x == 1;
    ok += (*p).y == 0;
    p++;
    ok += p->y == 1;
    r.min = pts[2];
    ok += r.min.x == 2 && r.min.y == 4;
    unit.max = r.min;
    ok += unit.max.y == 4;
    ok += unit.tag == 'u';
    ok += origin.x == 0 && origin.y == 0;
    {
        struct rect copy = r;
        copy.max.x = 100;
        ok += r.max.x == 6 && copy.max.x == 100;
        ok += copy.tag == 'r';
    }
    ok += sizeof(struct rect) == 20;
    ok += sizeof pts == 24;
    // Assignment is an expression: its value is the assigned struct.
    ok += (pts[0] = pts[1] = r.max).y == 8 && pts[0].x == 6;
    return ok;
}
