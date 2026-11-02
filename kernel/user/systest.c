/* systest: checks the system calls do what docs/camelot-syscalls.md says,
 * from user space. Prints a line for each check that fails and, at the end,
 * "systest: N checks, F failed"; exits with F.
 *
 * It also runs as its own child, to have one that does something known:
 *   systest exit N        exits with status N
 *   systest sleep N       sleeps N ticks, then exits with status 0
 *   systest args A B ...  exits 0 if its arguments are exactly "args A B ..."
 *                         (the parent passes them), else 1
 *   systest sbrk N        grows its heap by N bytes (see grow below) */
#include "user.h"

/* Process slots (NPROC in kernel/include/proc.h). */
#define NPROC 8
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

/* What a child run of systest does (see the top of the file). */
static int child(int argc, char **argv) {
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
    int pid = spawn("systest", argv);
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
    check_eq(syscall(11, 0, 0, 0), -ENOSYS, "call 11");
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
    check_eq(spawn("nope", NULL), -ENOENT, "spawn of an unknown program");
    check_eq(spawn("", NULL), -ENOENT, "spawn of an empty name");
    check_eq(spawn("a-name-far-too-long-for-any-program", NULL), -ENOENT, "spawn of a long name");

    check_eq(run_child("exit", "0"), 0, "exit status 0");
    check_eq(run_child("exit", "42"), 42, "exit status 42");
    check_eq(run_child("exit", "-3"), -3, "exit status -3");
    char *args[] = {"systest", "args", "", "two words", "x", NULL};
    int pid = spawn("systest", args);
    check(pid > 0, "spawn with arguments");
    check_eq(wait(pid, &status), pid, "wait for it");
    check_eq(status, 0, "the child got its arguments");

    /* wait(pid) takes that child only; wait(-1) any; a NULL status is fine. */
    int a = spawn("systest", (char *[]){"systest", "exit", "1", NULL});
    int b = spawn("systest", (char *[]){"systest", "exit", "2", NULL});
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
        int p = spawn("systest", (char *[]){"systest", "sleep", "2", NULL});
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
    pid = spawn("systest", (char *[]){"systest", "exit", "5", NULL});
    check(pid > 0, "a slot is free again");
    check_eq(wait(pid, &status), pid, "and its child ends");

    /* Argument limits. */
    char *many[MAXARG + 2];
    for (int i = 0; i <= MAXARG; i++) {
        many[i] = "x";
    }
    many[MAXARG + 1] = NULL;
    check_eq(spawn("echo", many), -E2BIG, "too many arguments");
    many[MAXARG] = NULL;
    pid = spawn("systest", many);
    check(pid > 0, "MAXARG arguments");
    check_eq(wait(pid, &status), pid, "wait for it");
    check_eq(status, 100, "and it ran");
    char big[ARG_MAX];
    memset(big, 'y', sizeof big - 1);
    big[sizeof big - 1] = 0;
    char *huge[] = {"echo", big, NULL};
    check_eq(spawn("echo", huge), -E2BIG, "arguments over ARG_MAX bytes");
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

int main(int argc, char **argv) {
    if (argc > 1) {
        return child(argc, argv);
    }
    test_simple_calls();
    test_files();
    test_spawn_and_wait();
    test_sbrk();
    printf("systest: %d checks, %d failed\n", checks, failures);
    return failures;
}
