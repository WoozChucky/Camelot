/* spacetest: checks the user address space (kernel/include/vm.h, spec
 * "Kernel processes"): a stack the kernel grows on demand, up to its limit,
 * a heap that can grow far past what fits in RAM by default, and the
 * kernel's pointer checks over all of it. Prints a line for each check that
 * fails and, at the end, "spacetest: N checks, F failed"; exits with F.
 *
 *   spacetest            the checks, which fit in a machine's default 4 MiB
 *   spacetest heap MIB   grows the heap by MIB MiB, writes and checks a word
 *                        on every page, checks pointers at its end, shrinks
 *                        it back; same report
 *   spacetest overflow   recurses until its stack runs out: the kernel kills
 *                        it, at the guard page under the stack's limit, or
 *                        sooner if RAM runs out first */
#include "user.h"

/* User space (USER_TOP, USER_STACK_LIMIT, USER_LOAD_TOP in vm.h). */
#define USER_TOP 0x0c000000u
#define USER_STACK_LIMIT 0x0b800000u
#define USER_LOAD_TOP 0x0b7ff000u
#define PAGE 4096u
#define MIB (1024u * 1024u)

static int checks;
static int failed;

static void check(int ok, const char *what) {
    checks++;
    if (!ok) {
        failed++;
        printf("spacetest: FAILED: %s\n", what);
    }
}

static void check_eq(int got, int want, const char *what) {
    checks++;
    if (got != want) {
        failed++;
        printf("spacetest: FAILED: %s: got %d, want %d\n", what, got, want);
    }
}

/* The pipe the pointer checks write into. */
static int fds[2];

/* What write() of `len` bytes at `addr` into the pipe returns; whatever it
 * wrote is read back out, so the pipe never fills. */
static int write_at(uint addr, uint len) {
    int r = write(fds[1], (const void *)addr, len);
    char sink[16];
    if (r > 0) {
        read(fds[0], sink, (uint)r);
    }
    return r;
}

/* Recursion with a 1000-byte frame: `n` levels deep, n + 1 frames of stack. */
static int deep(int n) {
    char frame[1000];
    frame[0] = (char)n;
    frame[999] = (char)(n + 1);
    if (n == 0) {
        return 0;
    }
    int below = deep(n - 1);
    return below + 1 + frame[999] - frame[0] - 1;
}

/* A frame of 64 KiB that is never touched before a system call writes into
 * it: the kernel must map the stack down to sp first. */
static int untouched(void) {
    char big[64 * 1024];
    write(fds[1], "abcdefgh", 8);
    int n = read(fds[0], big, 8);
    return n == 8 && memcmp(big, "abcdefgh", 8) == 0;
}

static void stack_checks(void) {
    check_eq(deep(200), 200, "200 frames of 1000 bytes: about 200 KiB of stack");
    check(untouched(), "a system call writes into a stack frame not touched yet");
    char here;
    uint sp = (uint)&here;
    check(sp >= USER_STACK_LIMIT && sp < USER_TOP, "the stack is in its range");
    /* The stack's top page holds the arguments: mapped. Past it is not user
     * space; under the stack's limit is the guard page. */
    check_eq(write_at(USER_TOP - 1, 1), 1, "the last byte of user space");
    check_eq(write_at(USER_TOP - 1, 2), -EFAULT, "across the end of user space");
    check_eq(write_at(USER_TOP, 1), -EFAULT, "the end of user space");
    check_eq(write_at(USER_STACK_LIMIT - 1, 1), -EFAULT, "the guard page under the stack");
    check_eq(write_at(USER_LOAD_TOP, 1), -EFAULT, "the guard page's first byte");
    /* Below sp, the stack has not grown yet: no memory of the program's. */
    check_eq(write_at(USER_STACK_LIMIT, 1), -EFAULT, "the stack's limit, not grown to");
    check_eq(write_at(sp - MIB, 1), -EFAULT, "a megabyte below sp");
}

static void heap_checks(void) {
    int end = sbrk(0);
    check_eq(sbrk((int)(USER_LOAD_TOP - (uint)end + 1)), -ENOMEM, "grow one byte into the guard page");
    check_eq(sbrk(64 * (int)MIB), -ENOMEM, "grow by more than the RAM");
    check_eq(sbrk(0), end, "failed calls change nothing");
    check_eq(write_at((uint)end, 1), -EFAULT, "the byte past the heap");
}

/* `spacetest heap MIB`. */
static void big_heap(int mib) {
    uint size = (uint)mib * MIB;
    int start = sbrk((int)size);
    check(start > 0, "grow the heap");
    if (start <= 0) {
        return;
    }
    uint base = (uint)start;
    if (size > 4 * MIB) {
        check(base + size > 4 * MIB, "the heap ends past the first 4 MiB");
    }
    for (uint a = base; a < base + size; a += PAGE) {
        *(uint *)(a & ~3u) = a;
    }
    int bad = 0;
    for (uint a = base; a < base + size; a += PAGE) {
        if (*(uint *)(a & ~3u) != a) {
            bad++;
        }
    }
    check_eq(bad, 0, "every page of the heap keeps what was written");
    check_eq(write_at(base + size - 1, 1), 1, "the heap's last byte");
    check_eq(write_at(base + size, 1), -EFAULT, "the byte past it");
    check_eq(sbrk(-(int)size), start + (int)size, "shrink it back");
    check_eq(sbrk(0), start, "back where it was");
}

/* Always 1, but volatile: a compiler cannot tell that forever() never
 * stops (clang -O2 would otherwise reject it, -Winfinite-recursion). */
static volatile int deeper = 1;

/* `spacetest overflow`: never returns. Every call keeps a 1000-byte frame
 * in memory (volatile, so an optimizer keeps it too) until the stack runs
 * into its guard page. */
static int forever(int n) {
    volatile char frame[1000];
    frame[0] = (char)n;
    if (!deeper) {
        return frame[0];
    }
    return forever(n + 1) + frame[0];
}

int main(int argc, char **argv) {
    if (argc == 2 && strcmp(argv[1], "overflow") == 0) {
        return forever(0);
    }
    if (pipe(fds) != 0) {
        printf("spacetest: no pipe\n");
        return 1;
    }
    if (argc == 3 && strcmp(argv[1], "heap") == 0) {
        big_heap(atoi(argv[2]));
    } else if (argc == 1) {
        stack_checks();
        heap_checks();
    } else {
        printf("usage: spacetest [heap MIB | overflow]\n");
        return 1;
    }
    printf("spacetest: %d checks, %d failed\n", checks, failed);
    return failed;
}
