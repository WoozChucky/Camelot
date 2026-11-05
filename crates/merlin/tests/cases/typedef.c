// expect: 11
// typedef names for every kind of type, scoped like variables. The sizes
// checked last: a list is a pointer, 8 bytes, and a node an int padded to a
// pointer's alignment and a pointer.

typedef int myint;
typedef unsigned char byte;
typedef struct node {
    int value;
    struct node *next;
} node;
typedef node *list;
typedef int (*binop)(int, int);
typedef int vec3[3];
typedef struct {
    short a, b;
} anon;

int add(int a, int b) {
    return a + b;
}

int length(list l) {
    int n = 0;
    while (l) {
        n++;
        l = l->next;
    }
    return n;
}

int main(void) {
    int ok = 0;
    myint x = 5;
    byte b = 300 & 0xff;
    node n3 = {3, 0}, n2 = {2, &n3}, n1 = {1, &n2};
    list l = &n1;
    binop op = add;
    vec3 v = {1, 2, 3};
    anon an = {7, 8};
    ok += x == 5;
    ok += b == 44;
    ok += length(l) == 3;
    ok += l->next->next->value == 3;
    ok += op(2, 3) == 5;
    ok += sizeof(vec3) == 12 && v[2] == 3;
    ok += an.a + an.b == 15;
    ok += sizeof(byte) == 1;
    {
        typedef char myint; /* shadows the outer one */
        myint c = 255;
        ok += c == 255 && sizeof(myint) == 1;
    }
    ok += sizeof(myint) == 4;
    ok += sizeof(list) == sizeof(void *) && sizeof(list) == __SIZEOF_POINTER__ &&
          sizeof(node) == 2 * sizeof(list);
    return ok;
}
