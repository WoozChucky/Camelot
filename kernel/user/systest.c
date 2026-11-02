/* systest: checks the system calls do what docs/camelot-syscalls.md says,
 * from user space: the process calls, and the descriptor calls on the
 * console and on pipes, which need no filesystem (filetest checks the
 * ones on files). Prints a line for each check that fails and, at the end,
 * "systest: N checks, F failed"; exits with F.
 *
 * It also runs as its own child, to have one that does something known:
 *   systest exit N        exits with status N
 *   systest sleep N       sleeps N ticks, then exits with status 0
 *   systest args A B ...  exits 0 if its arguments are exactly "args A B ..."
 *                         (the parent passes them), else 1
 *   systest sbrk N        grows its heap by N bytes (see grow below)
 *   systest pipewrite N   writes N bytes of a pattern to its output, 100 at
 *                         a time
 *   systest pipefill      writes 2000 bytes to its output, then 1, and exits
 *                         0 if the first write stopped part way and the
 *                         second failed with -EPIPE (its reader goes away)
 *   systest count         reads its input to the end; exits with the bytes
 *   systest fds           exits 0 if it has descriptors 0, 1 and 2 and no
 *                         other
 *   systest exec          dup()s its output to descriptor 3 and execs
 *                         `systest afterexec PID`, PID being its pid, which
 *                         exits 0 if its pid is PID and descriptor 3 is
 *                         still open
 *   systest execfail      execs paths that fail, then exits 0 if each failed
 *                         as it should and the program is still its own */
#include "user.h"

/* Process slots (NPROC in kernel/include/proc.h). */
#define NPROC 8
/* Descriptors per process (NOFILE in kernel/include/file.h). */
#define NOFILE 16
/* A pipe's buffer (PIPESIZE in kernel/include/pipe.h). */
#define PIPESIZE 512
/* The console's device number (CONSOLE in kernel/include/file.h). */
#define CONSOLE 1
#define PAGE 4096
/* Where the heap may grow to (USER_LOAD_TOP in kernel/include/vm.h). */
#define USER_LOAD_TOP 0x003fe000

static int checks;
static int failures;

static void check(int ok, const char *what) {
    checks++;
    if (!ok) {
        failures++;
        printf("systest: FAILED: %s\n", what);
    }
}

static void check_eq(int got, int want, const char *what) {
    checks++;
    if (got != want) {
        failures++;
        printf("systest: FAILED: %s: got %d, want %d\n", what, got, want);
    }
}

/* systest sbrk N: grows the heap by N bytes, and prints
 * "systest: sbrk N: R", R being 0 or the error. A test runs it on a machine
 * with too little memory, so that the kernel runs out of pages partway
 * through: then the call must have changed nothing, the pages it had mapped
 * already given back. Exits 0 if the heap is as it should be afterwards. */
static int grow(int n) {
    int base = sbrk(0);
    int got = sbrk(n);
    printf("systest: sbrk %d: %d\n", n, got < 0 ? got : 0);
    if (got >= 0) {
        return got == base && sbrk(0) == base + n ? 0 : 1;
    }
    if (sbrk(0) != base) {
        return 2;
    }
    /* Nothing is mapped past the end: wait checks that it can store the
     * status before anything else. */
    if (wait(-1, (int *)base) != -EFAULT) {
        return 3;
    }
    /* And the heap grows again from there. */
    if (sbrk(PAGE) != base) {
        return 4;
    }
    *(int *)base = 1;
    return 0;
}

/* Byte i of what `systest pipewrite` writes. */
static char pattern(int i) {
    return (char)('a' + i % 23);
}

static int pipe_write(int n) {
    char buf[100];
    for (int done = 0; done < n;) {
        int m = n - done < 100 ? n - done : 100;
        for (int i = 0; i < m; i++) {
            buf[i] = pattern(done + i);
        }
        if (write(STDOUT, buf, (uint)m) != m) {
            return 1;
        }
        done += m;
    }
    return 0;
}

static int pipe_fill(void) {
    static char buf[2000];
    int n = write(STDOUT, buf, sizeof buf);
    int m = write(STDOUT, buf, 1);
    return m == -EPIPE && n > 0 && n < (int)sizeof buf ? 0 : 1;
}

static int count_input(void) {
    char buf[64];
    int total = 0;
    int n;
    while ((n = read(STDIN, buf, sizeof buf)) > 0) {
        total += n;
    }
    return n == 0 ? total : 100;
}

static int only_standard_fds(void) {
    struct stat st;
    for (int fd = 0; fd < NOFILE; fd++) {
        if ((fstat(fd, &st) == 0) != (fd <= STDERR)) {
            return 1;
        }
    }
    return 0;
}

/* `n` in decimal, in `buf` (12 bytes). */
static char *decimal(int n, char *buf) {
    char *p = buf + 11;
    *p = 0;
    do {
        p--;
        *p = (char)('0' + n % 10);
        n /= 10;
    } while (n > 0);
    return p;
}

static int exec_self(void) {
    if (dup(STDOUT) != 3) {
        return 1;
    }
    char pid[12];
    char *argv[] = {"systest", "afterexec", decimal(getpid(), pid), NULL};
    exec("/bin/systest", argv);
    return 2; /* exec returned: it failed */
}

static int after_exec(const char *pid) {
    struct stat st;
    if (getpid() != atoi(pid)) {
        return 1;
    }
    return fstat(3, &st) == 0 && st.type == T_DEVICE ? 0 : 2;
}

/* Exec calls that must fail, leaving this program running. */
static int exec_fail(void) {
    char *many[MAXARG + 2];
    for (int i = 0; i <= MAXARG; i++) {
        many[i] = "x";
    }
    many[MAXARG + 1] = NULL;
    if (exec("/bin/nope", NULL) != -ENOENT || exec("", NULL) != -ENOENT ||
        exec("/bin/systest", many) != -E2BIG || exec("/bin/systest", (char **)0x80000000u) != -EFAULT) {
        return 1;
    }
    return 0;
}

/* What a child run of systest does (see the top of the file). */
static int child(int argc, char **argv) {
    if (strcmp(argv[1], "pipewrite") == 0 && argc == 3) {
        return pipe_write(atoi(argv[2]));
    }
    if (strcmp(argv[1], "pipefill") == 0) {
        return pipe_fill();
    }
    if (strcmp(argv[1], "count") == 0) {
        return count_input();
    }
    if (strcmp(argv[1], "fds") == 0) {
        return only_standard_fds();
    }
    if (strcmp(argv[1], "exec") == 0) {
        return exec_self();
    }
    if (strcmp(argv[1], "afterexec") == 0 && argc == 3) {
        return after_exec(argv[2]);
    }
    if (strcmp(argv[1], "execfail") == 0) {
        return exec_fail();
    }
    if (strcmp(argv[1], "sbrk") == 0 && argc == 3) {
        return grow(atoi(argv[2]));
    }
    if (strcmp(argv[1], "exit") == 0 && argc == 3) {
        return atoi(argv[2]);
    }
    if (strcmp(argv[1], "sleep") == 0 && argc == 3) {
        sleep((uint)atoi(argv[2]));
        return 0;
    }
    if (strcmp(argv[1], "args") == 0) {
        const char *want[] = {"systest", "args", "", "two words", "x"};
        if (argc != 5 || argv[argc] != NULL) {
            return 1;
        }
        for (int i = 0; i < argc; i++) {
            if (strcmp(argv[i], want[i]) != 0) {
                return 1;
            }
        }
        /* The strings and the array are on the stack, above sp. */
        int here;
        if ((uint)argv < (uint)&here || (uint)argv[0] < (uint)argv) {
            return 1;
        }
        return 0;
    }
    return 100;
}

/* Spawns systest with the arguments `a` and `b` and waits for it; returns
 * its exit status, or the error. */
static int run_child(char *a, char *b) {
    char *argv[] = {"systest", a, b, NULL};
    int pid = spawn("/bin/systest", argv);
    if (pid < 0) {
        return pid;
    }
    int status = -1000;
    int got = wait(pid, &status);
    return got == pid ? status : -1000 + got;
}

static void test_simple_calls(void) {
    int pid = getpid();
    check(pid > 1, "getpid is above the shell's");
    check_eq(getpid(), pid, "getpid again");
    check(uptime() >= 10, "uptime counts the boot ticks");
    check_eq(yield(), 0, "yield");
    uint before = uptime();
    check_eq(sleep(3), 0, "sleep");
    check(uptime() - before >= 3, "sleep lasts its ticks");
    check_eq(sleep(0), 0, "sleep 0");
    check_eq(syscall(0, 0, 0, 0), -ENOSYS, "call 0");
    check_eq(syscall(22, 0, 0, 0), -ENOSYS, "call 22");
    check_eq(syscall(-1, 0, 0, 0), -ENOSYS, "call -1");
}

static void test_files(void) {
    char c;
    check_eq(write(STDIN, "x", 1), -EBADF, "write to fd 0");
    check_eq(write(3, "x", 1), -EBADF, "write to fd 3");
    check_eq(write(-1, "x", 1), -EBADF, "write to fd -1");
    check_eq(read(STDOUT, &c, 1), -EBADF, "read from fd 1");
    check_eq(read(7, &c, 1), -EBADF, "read from fd 7");
    check_eq(write(STDOUT, "", 0), 0, "an empty write");
    check_eq(read(STDIN, &c, 0), 0, "an empty read");
    check_eq(write(STDERR, "", 0), 0, "an empty write to stderr");
}

static void test_spawn_and_wait(void) {
    int status = 7;
    check_eq(wait(-1, &status), -ECHILD, "wait without children");
    check_eq(status, 7, "a failed wait leaves the status alone");
    check_eq(wait(1, &status), -ECHILD, "wait for the shell, not a child");
    check_eq(spawn("/bin/nope", NULL), -ENOENT, "spawn of an unknown program");
    check_eq(spawn("", NULL), -ENOENT, "spawn of an empty path");
    check_eq(spawn("/bin/a-name-far-too-long-for-any-program", NULL), -ENOENT, "spawn of a long name");

    check_eq(run_child("exit", "0"), 0, "exit status 0");
    check_eq(run_child("exit", "42"), 42, "exit status 42");
    check_eq(run_child("exit", "-3"), -3, "exit status -3");
    char *args[] = {"systest", "args", "", "two words", "x", NULL};
    int pid = spawn("/bin/systest", args);
    check(pid > 0, "spawn with arguments");
    check_eq(wait(pid, &status), pid, "wait for it");
    check_eq(status, 0, "the child got its arguments");

    /* wait(pid) takes that child only; wait(-1) any; a NULL status is fine. */
    int a = spawn("/bin/systest", (char *[]){"systest", "exit", "1", NULL});
    int b = spawn("/bin/systest", (char *[]){"systest", "exit", "2", NULL});
    check(a > 0 && b > a, "two children");
    check_eq(wait(b, &status), b, "wait for the second");
    check_eq(status, 2, "its status");
    check_eq(wait(b, &status), -ECHILD, "it is gone");
    check_eq(wait(-1, NULL), a, "wait for any, no status");
    check_eq(wait(-1, &status), -ECHILD, "none left");

    /* Fill every process slot: the shell, this and six children. */
    int pids[NPROC];
    int n = 0;
    for (;;) {
        int p = spawn("/bin/systest", (char *[]){"systest", "sleep", "2", NULL});
        if (p < 0) {
            check_eq(p, -EAGAIN, "spawn with every slot taken");
            break;
        }
        pids[n] = p;
        n++;
        if (n == NPROC) {
            break;
        }
    }
    check_eq(n, NPROC - 2, "children until the slots run out");
    /* wait(-1) collects them in no particular order, each once. */
    for (int i = 0; i < n; i++) {
        int got = wait(-1, &status);
        int found = 0;
        for (int j = 0; j < n; j++) {
            if (pids[j] == got) {
                found = 1;
                pids[j] = 0;
            }
        }
        check(found, "collect a sleeper");
        check_eq(status, 0, "a sleeper's status");
    }
    check_eq(wait(-1, &status), -ECHILD, "every sleeper collected");
    pid = spawn("/bin/systest", (char *[]){"systest", "exit", "5", NULL});
    check(pid > 0, "a slot is free again");
    check_eq(wait(pid, &status), pid, "and its child ends");

    /* Argument limits. */
    char *many[MAXARG + 2];
    for (int i = 0; i <= MAXARG; i++) {
        many[i] = "x";
    }
    many[MAXARG + 1] = NULL;
    check_eq(spawn("/bin/echo", many), -E2BIG, "too many arguments");
    many[MAXARG] = NULL;
    pid = spawn("/bin/systest", many);
    check(pid > 0, "MAXARG arguments");
    check_eq(wait(pid, &status), pid, "wait for it");
    check_eq(status, 100, "and it ran");
    char big[ARG_MAX];
    memset(big, 'y', sizeof big - 1);
    big[sizeof big - 1] = 0;
    char *huge[] = {"echo", big, NULL};
    check_eq(spawn("/bin/echo", huge), -E2BIG, "arguments over ARG_MAX bytes");
    /* A path is at most MAXPATH bytes. */
    char path[MAXPATH + 2];
    memset(path, 'p', sizeof path - 1);
    path[0] = '/';
    path[sizeof path - 1] = 0;
    check_eq(spawn(path, NULL), -ENAMETOOLONG, "a path over MAXPATH bytes");
}

static void test_sbrk(void) {
    int base = sbrk(0);
    check(base > 0 && base % PAGE == 0, "the heap starts on a page boundary");
    check(base > (int)(uint)test_sbrk, "above the program");
    check_eq(sbrk(3 * PAGE + 10), base, "grow by three pages and a bit");
    check_eq(sbrk(0), base + 3 * PAGE + 10, "the new end");
    char *heap = (char *)base;
    int zero = 1;
    for (int i = 0; i < 3 * PAGE + 10; i++) {
        zero &= heap[i] == 0;
        heap[i] = (char)i;
    }
    check(zero, "new heap memory reads zero");
    check_eq(heap[PAGE + 5], (char)(PAGE + 5), "and takes stores");
    /* The pages beyond the end are not mapped: a write from there fails. */
    check_eq(write(STDOUT, heap + 4 * PAGE, 1), -EFAULT, "nothing past the last page");
    check_eq(sbrk(-(2 * PAGE + 20)), base + 3 * PAGE + 10, "shrink");
    check_eq(write(STDOUT, heap + 2 * PAGE, 1), -EFAULT, "the freed page is gone");
    check_eq(heap[PAGE - 11], (char)(PAGE - 11), "the kept bytes are still there");
    check_eq(sbrk(PAGE), base + PAGE - 10, "grow again");
    zero = 1;
    for (int i = PAGE - 10; i < 2 * PAGE - 10; i++) {
        zero &= heap[i] == 0;
    }
    check(zero, "regrown memory reads zero, the kept page's tail too");
    check_eq(heap[100], 100, "and the rest is kept");
    int end = base + 2 * PAGE - 10;
    check_eq(sbrk(-(1 << 30)), -EINVAL, "shrink below the start");
    check_eq(sbrk(base - end - 1), -EINVAL, "shrink one byte below the start");
    check_eq(sbrk(USER_LOAD_TOP - end + 1), -ENOMEM, "grow one byte into the guard page");
    check_eq(sbrk(0x7fffffff), -ENOMEM, "grow past the end of user space");
    check_eq(sbrk(0), end, "failed calls change nothing");
    check_eq(sbrk(base - end), end, "back to empty");
    check_eq(sbrk(0), base, "empty");
    check_eq(write(STDOUT, heap, 1), -EFAULT, "with no page left");
}

/* Spawns `argv` with `in` and `out` (if not -1) as its descriptors 0 and
 * 1, as the shell does: put them there, spawn, put the console back. */
static int spawn_with(int in, int out, char **argv) {
    int console_in = dup(STDIN);
    int console_out = dup(STDOUT);
    if (in >= 0) {
        close(STDIN);
        dup(in);
    }
    if (out >= 0) {
        close(STDOUT);
        dup(out);
    }
    int pid = spawn("/bin/systest", argv);
    close(STDIN);
    dup(console_in);
    close(STDOUT);
    dup(console_out);
    close(console_in);
    close(console_out);
    return pid;
}

/* Waits for child `pid`; returns its status, or -1000 if it isn't one. */
static int status_of(int pid) {
    int status;
    return pid > 0 && wait(pid, &status) == pid ? status : -1000;
}

static void test_descriptors(void) {
    struct stat st;
    check_eq(fstat(STDOUT, &st), 0, "fstat on the console");
    check(st.type == T_DEVICE && st.major == CONSOLE && st.ino == 0 && st.size == 0,
          "the console is a device, opened by the kernel");
    check_eq(fstat(3, &st), -EBADF, "fstat on a closed descriptor");
    check_eq(fstat(NOFILE, &st), -EBADF, "fstat past the table");
    check_eq(lseek(STDIN, 0, SEEK_SET), -ESPIPE, "lseek on the console");
    check_eq(lseek(5, 0, SEEK_SET), -EBADF, "lseek on a closed descriptor");
    check_eq(close(-1), -EBADF, "close -1");
    check_eq(close(NOFILE), -EBADF, "close past the table");
    check_eq(close(9), -EBADF, "close a closed descriptor");
    check_eq(dup(9), -EBADF, "dup a closed descriptor");
    check_eq(open("/x", O_WRONLY | O_RDWR), -EINVAL, "open with two access modes");
    check_eq(open("/x", 0x1000), -EINVAL, "open with an unknown flag");

    /* dup takes the lowest free descriptor, and shares the open file. */
    int fd = dup(STDOUT);
    check_eq(fd, 3, "dup gives the lowest free descriptor");
    check_eq(write(fd, "", 0), 0, "a write to the copy");
    check_eq(read(fd, &st, 1), -EBADF, "the copy is write-only too");
    check_eq(close(fd), 0, "close the copy");
    check_eq(close(fd), -EBADF, "close it twice");
    check_eq(write(STDOUT, "", 0), 0, "the original stays open");

    /* Every descriptor taken. */
    int n = 0;
    while (dup(STDERR) >= 0) {
        n++;
    }
    check_eq(n, NOFILE - 3, "descriptors until the table is full");
    check_eq(dup(STDERR), -EMFILE, "dup with every descriptor taken");
    int p[2] = {-7, -7};
    check_eq(pipe(p), -EMFILE, "pipe with every descriptor taken");
    check(p[0] == -7 && p[1] == -7, "a failed pipe stores nothing");
    for (int i = 3; i < NOFILE; i++) {
        close(i);
    }
    check_eq(dup(STDERR), 3, "and they are free again");
    close(3);
}

static void test_pipes(void) {
    int p[2];
    char buf[PIPESIZE + 1];
    struct stat st;
    check_eq(pipe(p), 0, "pipe");
    check(p[0] == 3 && p[1] == 4, "the lowest two descriptors");
    check_eq(write(p[1], "hello", 5), 5, "write to a pipe");
    check_eq(fstat(p[0], &st), 0, "fstat on a pipe");
    check(st.type == T_PIPE && st.size == 5 && st.ino == 0, "a pipe holding 5 bytes");
    check_eq(read(p[0], buf, 3), 3, "read part of it");
    check_eq(read(p[0], buf + 3, sizeof buf - 3), 2, "read the rest: no more than there is");
    check(memcmp(buf, "hello", 5) == 0, "and it is what went in");
    check_eq(read(p[1], buf, 1), -EBADF, "the write end is not for reading");
    check_eq(write(p[0], "x", 1), -EBADF, "the read end is not for writing");
    check_eq(lseek(p[0], 0, SEEK_SET), -ESPIPE, "lseek on a pipe");
    check_eq(read(p[0], buf, 0), 0, "an empty read of an empty pipe");
    /* PIPESIZE bytes fit without a reader. */
    memset(buf, 'z', sizeof buf);
    check_eq(write(p[1], buf, PIPESIZE), PIPESIZE, "fill the pipe");
    check_eq(read(p[0], buf, sizeof buf), PIPESIZE, "and empty it");
    check_eq(write(p[1], "end", 3), 3, "write, then close the write end");
    check_eq(close(p[1]), 0, "close the write end");
    check_eq(read(p[0], buf, sizeof buf), 3, "what was written is still there");
    check_eq(read(p[0], buf, sizeof buf), 0, "then the end of the input");
    check_eq(close(p[0]), 0, "close the read end");

    /* No reader. */
    check_eq(pipe(p), 0, "another pipe");
    close(p[0]);
    check_eq(write(p[1], "x", 1), -EPIPE, "write with no reader");
    check_eq(write(p[1], "", 0), 0, "an empty write with no reader");
    close(p[1]);

    /* A child writing more than the pipe holds: it sleeps while the pipe
     * is full, and the end of the input comes when it exits. */
    check_eq(pipe(p), 0, "a pipe for a child");
    int pid = spawn_with(-1, p[1], (char *[]){"systest", "pipewrite", "3000", NULL});
    check(pid > 0, "spawn a writer");
    close(p[1]);
    int total = 0;
    int same = 1;
    int n;
    while ((n = read(p[0], buf, 77)) > 0) {
        for (int i = 0; i < n; i++) {
            same &= buf[i] == pattern(total + i);
        }
        total += n;
    }
    check_eq(n, 0, "the end of the input when the writer is gone");
    check_eq(total, 3000, "every byte arrives");
    check(same, "in order");
    check_eq(status_of(pid), 0, "the writer's status");
    close(p[0]);

    /* The reader goes away while the writer waits for room. */
    check_eq(pipe(p), 0, "a pipe for a writer that gets no reader");
    pid = spawn_with(-1, p[1], (char *[]){"systest", "pipefill", NULL});
    close(p[1]);
    check_eq(read(p[0], buf, 10), 10, "read a little");
    close(p[0]);
    check_eq(status_of(pid), 0, "the writer saw its write cut short, then -EPIPE");

    /* A child reading a pipe as its input. */
    check_eq(pipe(p), 0, "a pipe for a reader");
    pid = spawn_with(p[0], -1, (char *[]){"systest", "count", NULL});
    close(p[0]);
    check_eq(write(p[1], "abcdefg", 7), 7, "write to the reader");
    close(p[1]);
    check_eq(status_of(pid), 7, "the reader read it all, then the end");

    /* The reader goes away while the writer is already asleep on a full
     * pipe: closing the read end wakes it. The kernel is never preempted,
     * so once the pipe is seen full the writer is asleep (it fills it and
     * sleeps in one go). */
    check_eq(pipe(p), 0, "a pipe for a writer asleep when its reader goes");
    pid = spawn_with(-1, p[1], (char *[]){"systest", "pipefill", NULL});
    close(p[1]);
    while (fstat(p[0], &st) == 0 && st.size < PIPESIZE) {
        yield();
    }
    check_eq(st.size, PIPESIZE, "the writer filled the pipe");
    close(p[0]);
    check_eq(status_of(pid), 0, "closing the read end wakes the writer: cut short, then -EPIPE");

    /* The last writer goes away while the reader is asleep on an empty
     * pipe: closing the write end wakes it with the end of the input. */
    check_eq(pipe(p), 0, "a pipe for a reader asleep when its writer goes");
    pid = spawn_with(p[0], -1, (char *[]){"systest", "count", NULL});
    close(p[0]);
    check_eq(write(p[1], "x", 1), 1, "write a byte to the reader");
    while (fstat(p[1], &st) == 0 && st.size > 0) {
        yield();
    }
    /* A few more turns, for the reader to go back to sleep on the pipe. */
    for (int i = 0; i < 4; i++) {
        yield();
    }
    close(p[1]);
    check_eq(status_of(pid), 1, "closing the write end wakes the reader: the end of the input");

    /* Only descriptors 0, 1 and 2 are inherited. */
    check_eq(pipe(p), 0, "a pipe the child must not get");
    check_eq(status_of(spawn_with(-1, -1, (char *[]){"systest", "fds", NULL})), 0,
             "a child gets descriptors 0, 1 and 2 only");
    close(p[0]);
    close(p[1]);
}

static void test_exec(void) {
    check_eq(status_of(spawn("/bin/systest", (char *[]){"systest", "exec", NULL})), 0,
             "exec keeps the pid and the descriptors");
    check_eq(status_of(spawn("/bin/systest", (char *[]){"systest", "execfail", NULL})), 0,
             "a failed exec returns its error");
}

int main(int argc, char **argv) {
    if (argc > 1) {
        return child(argc, argv);
    }
    test_simple_calls();
    test_files();
    test_spawn_and_wait();
    test_sbrk();
    test_descriptors();
    test_pipes();
    test_exec();
    printf("systest: %d checks, %d failed\n", checks, failures);
    return failures;
}
