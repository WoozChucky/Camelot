/* badptr: hands the kernel pointers it must refuse. Every call below must
 * fail with -EFAULT (or -ENOSYS, for an unknown call number) without
 * printing, reading or changing anything; then a good write, of "badptr:
 * every bad pointer refused", must return its length. Exits with status 0 if
 * all of that holds, else prints "badptr: case N failed" and exits with N,
 * the number of the first case that didn't. A bad pointer is refused before
 * anything else is looked at, so the calls on paths fail the same way with
 * or without a filesystem. */
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
    expect(spawn("/bin/echo", argv), -EFAULT);               /* 18 */
    expect(spawn("/bin/echo", (char **)KERNEL), -EFAULT);    /* 19 */
    /* Program names that run into the end of the heap: a short one, and
     * one longer than any program's, which spawn still reads to its end. */
    char *end = (char *)sbrk(PAGE) + PAGE;
    memset(end - 200, 'a', 200);
    expect(spawn(end - 10, NULL), -EFAULT);                  /* 20 */
    expect(spawn(end - 30, NULL), -EFAULT);                  /* 21 */
    /* No child was started by any of that. */
    expect(wait(-1, &status), -ECHILD);                      /* 22 */
    /* A call number nobody defined. */
    expect(syscall(99, 0, 0, 0), -ENOSYS);                   /* 23 */
    /* The file calls' paths, a short one and a long one running off the
     * heap among them. */
    expect(open((char *)KERNEL, O_RDONLY), -EFAULT);         /* 24 */
    expect(open(end - 10, O_RDONLY), -EFAULT);               /* 25 */
    expect(open(end - 200, O_RDONLY | O_CREAT), -EFAULT);    /* 26: past MAXPATH */
    expect(mkdir((char *)KERNEL), -EFAULT);                  /* 27 */
    expect(unlink((char *)0), -EFAULT);                      /* 28 */
    expect(chdir((char *)KERNEL), -EFAULT);                  /* 29 */
    expect(link((char *)KERNEL, "/x"), -EFAULT);             /* 30 */
    expect(link("/x", end - 10), -EFAULT);                   /* 31 */
    expect(exec((char *)KERNEL, NULL), -EFAULT);             /* 32 */
    expect(exec("/bin/echo", argv), -EFAULT);                /* 33 */
    expect(exec("/bin/echo", (char **)KERNEL), -EFAULT);     /* 34 */
    /* Where fstat and pipe store: nowhere, or read-only memory. */
    struct stat st;
    expect(fstat(STDOUT, (struct stat *)KERNEL), -EFAULT);   /* 35 */
    expect(fstat(STDOUT, (struct stat *)msg), -EFAULT);      /* 36 */
    expect(fstat(STDOUT, (struct stat *)(end - 8)), -EFAULT); /* 37 */
    expect(pipe((int *)KERNEL), -EFAULT);                    /* 38 */
    expect(pipe((int *)msg), -EFAULT);                       /* 39 */
    expect(pipe((int *)(end - 4)), -EFAULT);                 /* 40 */
    /* None of that opened a descriptor: 3 is the lowest free one. */
    expect(dup(STDOUT), 3);                                  /* 41 */
    expect(close(3), 0);                                     /* 42 */
    /* A pipe's buffers: the failed calls move nothing. */
    int p[2];
    expect(pipe(p), 0);                                      /* 43 */
    expect(write(p[1], (char *)KERNEL, 4), -EFAULT);         /* 44 */
    expect(write(p[1], "ok", 2), 2);                         /* 45 */
    expect(read(p[0], (char *)msg, 2), -EFAULT);             /* 46 */
    expect(read(p[0], end - 1, 2), -EFAULT);                 /* 47 */
    expect(fstat(p[0], &st) == 0 && st.size == 2, 1);        /* 48 */
    expect(read(p[0], buf, sizeof buf), 2);                  /* 49 */
    expect(buf[0] == 'o' && buf[1] == 'k', 1);               /* 50 */
    close(p[0]);
    close(p[1]);
    /* And a good buffer. */
    if (failed_case == 0) {
        uint len = strlen(msg);
        expect(write(STDOUT, msg, len), (int)len);           /* 24 */
    } else {
        printf("badptr: case %d failed\n", failed_case);
    }
    return failed_case;
}
