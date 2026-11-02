/* badptr: hands the kernel pointers it must refuse. Every call below must
 * fail with -EFAULT (or -ENOSYS, for an unknown call number) without
 * printing, reading or changing anything; then a good write must return its
 * length. Exits with status 0 if all of that holds, else with the number of
 * the first case that didn't. */
#include "user.h"

/* User space (kernel/include/vm.h): the stack is its last page, with an
 * unmapped guard page under it. */
#define USER_TOP 0x00400000u
#define PAGE 0x1000u
#define GUARD (USER_TOP - 2 * PAGE)
/* The kernel's first page: mapped in every process, but without U. */
#define KERNEL 0x80000000u

static const char msg[] = "badptr: every bad pointer refused\n";

/* The case being checked, for the exit status. */
static int failed_case;
static int number;

static void expect(int got, int want) {
    number++;
    if (got != want && failed_case == 0) {
        failed_case = number;
    }
}

int main(void) {
    char *bad[] = {
        (char *)KERNEL,           /* kernel memory */
        (char *)0,                /* page 0: never mapped */
        (char *)(USER_TOP - 2),   /* the last two bytes of the stack, then past the end */
        (char *)(GUARD + PAGE - 4), /* the guard page under the stack */
    };
    for (int i = 0; i < 4; i++) {
        expect(write(STDOUT, bad[i], 4), -EFAULT);           /* cases 1, 3, 5, 7 */
        expect(read(STDIN, bad[i], 4), -EFAULT);             /* cases 2, 4, 6, 8 */
    }
    /* A length that wraps around the address space. */
    expect(write(STDOUT, msg, 0xffffffffu), -EFAULT);        /* 9 */
    char buf[8];
    expect(read(STDIN, buf, 0xffffffffu), -EFAULT);          /* 10 */
    /* Read-only memory is no place to read into: the program's rodata and
     * its code. */
    expect(read(STDIN, (char *)msg, 4), -EFAULT);            /* 11 */
    expect(read(STDIN, (char *)main, 4), -EFAULT);           /* 12 */
    /* Past the end of the heap. */
    int brk = sbrk(0);
    expect(write(STDOUT, (char *)brk, 1), -EFAULT);          /* 13 */
    expect(read(STDIN, (char *)brk, 1), -EFAULT);            /* 14 */
    /* The other calls that take pointers. */
    int status;
    expect(wait(-1, (int *)KERNEL), -EFAULT);                /* 15 */
    expect(wait(-1, (int *)msg), -EFAULT);                   /* 16 */
    expect(spawn((char *)KERNEL, NULL), -EFAULT);            /* 17 */
    char *argv[] = {"echo", (char *)KERNEL, NULL};
    expect(spawn("echo", argv), -EFAULT);                    /* 18 */
    expect(spawn("echo", (char **)KERNEL), -EFAULT);         /* 19 */
    /* No child was started by any of that. */
    expect(wait(-1, &status), -ECHILD);                      /* 20 */
    /* A call number nobody defined. */
    expect(syscall(99, 0, 0, 0), -ENOSYS);                   /* 21 */
    /* And a good buffer. */
    uint len = strlen(msg);
    expect(write(STDOUT, msg, len), (int)len);               /* 22 */
    return failed_case;
}
