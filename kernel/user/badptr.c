/* badptr: hands the kernel pointers it must refuse. Every call below must
 * fail with -EFAULT (or -ENOSYS, for an unknown call number) without
 * printing, reading or changing anything; then a good write, of "badptr:
 * every bad pointer refused", must return its length. Exits with status 0 if
 * all of that holds, else prints "badptr: case N failed" and exits with N,
 * the number of the first case that didn't. A bad pointer is refused before
 * anything else is looked at, so the calls on paths fail the same way with
 * or without a filesystem. */
#include "user.h"

/* User space (kernel/include/vm.h): the stack grows down from USER_TOP to
 * at most USER_STACK_LIMIT, 8 MiB below, with an unmapped guard page under
 * that. USER_TOP is 0x0c000000 on RV32; on RV64 it depends on the paging
 * mode (2^47 in Sv48, 2^38 in Sv39), so main() finds it: the arguments are
 * in the stack's last page. */
static uintptr_t user_top;
#define USER_TOP user_top
#define PAGE 0x1000ul
#define USER_STACK_LIMIT (USER_TOP - (8ul << 20))
#define GUARD (USER_STACK_LIMIT - PAGE)
/* Kernel memory, mapped in every process but without U: on RV32 the
 * kernel's first page, on RV64 the kernel image at its link address, in
 * the upper half of the address space (vm.h). */
#ifdef _LP64
#define KERNEL 0xffffffff80000000ul
#else
#define KERNEL 0x80000000ul
#endif

static const char msg[] = "badptr: every bad pointer refused\n";

/* The case being checked, for the exit status. */
static int failed_case;
static int number;

static void expect(long got, long want) {
    number++;
    if (got != want && failed_case == 0) {
        failed_case = number;
    }
}

int main(int argc, char **args) {
    (void)argc;
    user_top = ((uintptr_t)args[0] + PAGE - 1) & ~(PAGE - 1);
    char *bad[] = {
        (char *)KERNEL,           /* kernel memory */
        (char *)0,                /* page 0: never mapped */
        (char *)(USER_TOP - 2),   /* the last two bytes of the stack, then past the end */
        (char *)(GUARD + PAGE - 4), /* the guard page under the stack's limit */
    };
    for (int i = 0; i < 4; i++) {
        expect(write(STDOUT, bad[i], 4), -EFAULT);           /* cases 1, 3, 5, 7 */
        expect(read(STDIN, bad[i], 4), -EFAULT);             /* cases 2, 4, 6, 8 */
    }
    /* A length that wraps around the address space. */
    expect(write(STDOUT, msg, (size_t)-1), -EFAULT);         /* 9 */
    char buf[8];
    expect(read(STDIN, buf, (size_t)-1), -EFAULT);           /* 10 */
    /* Read-only memory is no place to read into: the program's rodata and
     * its code. */
    expect(read(STDIN, (char *)msg, 4), -EFAULT);            /* 11 */
    expect(read(STDIN, (char *)main, 4), -EFAULT);           /* 12 */
    /* Past the end of the heap. */
    char *brk = sbrk(0);
    expect(write(STDOUT, brk, 1), -EFAULT);                  /* 13 */
    expect(read(STDIN, brk, 1), -EFAULT);                    /* 14 */
    /* The other calls that take pointers. */
    int status;
    expect(wait(-1, (int *)KERNEL), -EFAULT);                /* 15 */
    expect(wait(-1, (int *)msg), -EFAULT);                   /* 16 */
    expect(spawn((char *)KERNEL, NULL, environ), -EFAULT);   /* 17 */
    char *argv[] = {"echo", (char *)KERNEL, NULL};
    expect(spawn("/bin/echo", argv, environ), -EFAULT);      /* 18 */
    expect(spawn("/bin/echo", (char **)KERNEL, environ), -EFAULT); /* 19 */
    /* Program names that run into the end of the heap: a short one, and
     * one longer than any program's, which spawn still reads to its end. */
    char *end = (char *)sbrk(PAGE) + PAGE;
    memset(end - 200, 'a', 200);
    expect(spawn(end - 10, NULL, environ), -EFAULT);         /* 20 */
    expect(spawn(end - 30, NULL, environ), -EFAULT);         /* 21 */
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
    expect(exec((char *)KERNEL, NULL, environ), -EFAULT);    /* 32 */
    expect(exec("/bin/echo", argv, environ), -EFAULT);       /* 33 */
    expect(exec("/bin/echo", (char **)KERNEL, environ), -EFAULT); /* 34 */
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
    /* A bad pointer comes before a path that is empty or too long, in
     * whichever argument it is. */
    char long_path[MAXPATH + 2];
    memset(long_path, 'a', sizeof long_path - 1);
    long_path[sizeof long_path - 1] = 0;
    char **bad_argv = (char **)4;
    expect(link("", (char *)KERNEL), -EFAULT);               /* 43 */
    expect(link(long_path, (char *)0), -EFAULT);             /* 44 */
    expect(spawn("", bad_argv, environ), -EFAULT);           /* 45 */
    expect(spawn(long_path, argv, environ), -EFAULT);        /* 46 */
    expect(exec("", bad_argv, environ), -EFAULT);            /* 47 */
    expect(exec(long_path, argv, environ), -EFAULT);         /* 48 */
    expect(wait(-1, &status), -ECHILD);                      /* 49 */
    /* A pipe's buffers: the failed calls move nothing. */
    int p[2];
    expect(pipe(p), 0);                                      /* 50 */
    expect(write(p[1], (char *)KERNEL, 4), -EFAULT);         /* 51 */
    expect(write(p[1], "ok", 2), 2);                         /* 52 */
    expect(read(p[0], (char *)msg, 2), -EFAULT);             /* 53 */
    expect(read(p[0], end - 1, 2), -EFAULT);                 /* 54 */
    expect(fstat(p[0], &st) == 0 && st.size == 2, 1);        /* 55 */
    expect(read(p[0], buf, sizeof buf), 2);                  /* 56 */
    expect(buf[0] == 'o' && buf[1] == 'k', 1);               /* 57 */
    close(p[0]);
    close(p[1]);
    /* The environment, which spawn and exec read as they read argv: a
     * bad array, a bad string in it, an array that runs off the end of
     * the heap; and a bad one comes before an empty path too. */
    char *echo[] = {"echo", NULL};
    char *bad_env[] = {"A=1", (char *)KERNEL, NULL};
    char **tail = (char **)end - 1;
    *tail = "A=1";
    expect(spawn("/bin/echo", echo, (char **)KERNEL), -EFAULT); /* 58 */
    expect(spawn("/bin/echo", echo, bad_env), -EFAULT);      /* 59 */
    expect(exec("/bin/echo", echo, (char **)KERNEL), -EFAULT); /* 60 */
    expect(exec("/bin/echo", echo, bad_env), -EFAULT);       /* 61 */
    expect(spawn("/bin/echo", echo, tail), -EFAULT);         /* 62 */
    expect(spawn("", NULL, bad_argv), -EFAULT);              /* 63 */
    expect(exec("", NULL, bad_argv), -EFAULT);               /* 64 */
    expect(wait(-1, &status), -ECHILD);                      /* 65 */
#ifdef _LP64
    /* RV64: every address from USER_TOP up is the kernel's, or not an
     * address at all (between the two halves): the first one past user
     * space, the lowest of the upper half, the direct map's start in Sv48
     * and in Sv39, the vmalloc area's, the very top; and a range from the
     * last user page into them. Never truncated to a user address: 2^32 +
     * 0x1000 is not 0x1000. */
    char *kernel_half[] = {
        (char *)USER_TOP,
        (char *)0xffff800000000000ul,
        (char *)0xffffaf8000000000ul,
        (char *)0xffffffd600000000ul,
        (char *)0xffff8f8000000000ul,
        (char *)0xfffffffffffff000ul,
    };
    for (int i = 0; i < 6; i++) {
        expect(write(STDOUT, kernel_half[i], 1), -EFAULT);   /* 66, 69, ... */
        expect(read(STDIN, kernel_half[i], 1), -EFAULT);     /* 67, 70, ... */
        expect(fstat(STDOUT, (struct stat *)kernel_half[i]), -EFAULT); /* 68, 71, ... */
    }
    expect(write(STDOUT, (char *)(USER_TOP - 1), 2), -EFAULT); /* 84 */
    expect(open((char *)KERNEL, O_RDONLY), -EFAULT);         /* 85 */
    expect(wait(-1, (int *)0xffffffd600000000ul), -EFAULT);  /* 86 */
    expect(write(STDOUT, (char *)0x100000000ul + 0x1000, 1), -EFAULT); /* 87 */
#endif
    /* And a good buffer. */
    if (failed_case == 0) {
        size_t len = strlen(msg);
        expect(write(STDOUT, msg, len), (long)len);          /* 66, or 88 on RV64 */
    } else {
        printf("badptr: case %d failed\n", failed_case);
    }
    return failed_case;
}
