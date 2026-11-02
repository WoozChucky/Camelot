// expect: 8
// Initializer lists with inner braces left out: the values fill each
// subobject in turn.

struct pt {
    int x, y;
};

struct pt line[2] = {1, 2, 3, 4};
int m[2][3] = {1, 2, 3, 4};
struct {
    char name[4];
    int v;
} items[] = {"ab", 1, "cd", 2};

struct wrap {
    struct pt p;
    int extra;
};

int main(void) {
    int ok = 0;
    struct pt local[2] = {5, 6, 7};
    int grid[2][2] = {{1}, 2, 3};
    struct wrap w = {line[1], 9};
    struct wrap w2 = {8, 9, 10};
    ok += line[1].x == 3 && line[1].y == 4;
    ok += m[1][0] == 4 && m[1][1] == 0;
    ok += sizeof items == 16;
    ok += items[1].name[1] == 'd' && items[1].v == 2;
    ok += local[1].x == 7 && local[1].y == 0;
    ok += grid[0][0] == 1 && grid[0][1] == 0 && grid[1][0] == 2 && grid[1][1] == 3;
    ok += w.p.y == 4 && w.extra == 9;
    ok += w2.p.x == 8 && w2.p.y == 9 && w2.extra == 10;
    return ok;
}
