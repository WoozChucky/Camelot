// expect: 9
// Global structs and unions with initializers: members, padding, pointers to
// other globals and to members.

struct node {
    int v;
    struct node *next;
};

struct node n3 = {3, 0};
struct node n2 = {2, &n3};
struct node n1 = {1, &n2};
struct node *head = &n1;

struct mixed {
    char c;
    short s;
    int i;
    char tail[3];
} mx = {'a', -2, 100000, "xy"};

union num {
    int i;
    char c;
} un = {0x41424344};

static struct node *last = &n3;
int *pi = &mx.i;

struct {
    int a[3];
} arr_in_struct = {{1, 2, 3}};

int main(void) {
    int ok = 0, sum = 0;
    struct node *p;
    static struct node local_static = {7, &n1};
    for (p = head; p; p = p->next)
        sum += p->v;
    ok += sum == 6;
    ok += mx.c == 'a' && mx.s == -2 && mx.i == 100000;
    ok += mx.tail[1] == 'y' && mx.tail[2] == 0;
    ok += un.c == 0x44;
    ok += last->v == 3;
    ok += *pi == 100000;
    ok += arr_in_struct.a[2] == 3;
    ok += sizeof mx == 12;
    ok += local_static.next->next->v == 2;
    return ok;
}
