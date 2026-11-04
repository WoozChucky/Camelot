// expect: 77
// expect-lp64: 81
// Compound literals at file scope are objects with static storage that the
// globals' initializers point into; `sizeof expr` sizes arrays (pointers
// are 8 bytes on LP64).

struct point {
    int x, y;
};

int *primes = (int[]){2, 3, 5, 7};
struct point *origin = &(struct point){.y = 4};
const char *const *names = (const char *const[]){"ab", "cde", 0};
struct point **path = (struct point *[]){&(struct point){1, 2}, &(struct point){3, 4}, 0};

int table[] = {10, 20, 30};
char copy[sizeof table / sizeof table[0]];
char name_buf[sizeof "hello"];
char point_bytes[sizeof origin->x + sizeof *path];

int main(void) {
    int sum = 0;
    for (int i = 0; i < 4; i++) {
        sum += primes[i]; // 17
    }
    primes[0] = 1; // writable, and lives on
    sum += primes[0]; // 18
    sum += origin->x + origin->y; // 22
    sum += names[1][2] - 'a'; // 'e' - 'a' = 4: 26
    for (struct point **p = path; *p; p++) {
        sum += (*p)->x * (*p)->y; // 2 + 12: 40
    }
    sum += sizeof copy; // 3: 43
    sum += sizeof name_buf; // 6: 49
    sum += sizeof point_bytes; // 8: 57
    // Local compound literals still live on the stack, one per evaluation.
    int *local = (int[]){20};
    return sum + *local; // 77
}
