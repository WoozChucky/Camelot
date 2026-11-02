/* threadtest: checks the thread and futex calls do what
 * docs/camelot-syscalls.md ("Threads") says, and the library's locks on
 * them: creating, ending and joining threads and every error of each call,
 * futexes (a value that differs returns at once, a bad pointer is an
 * error, a wake wakes no more than it is asked to), running out of thread
 * slots, the library's mutex and malloc from several threads, sbrk from
 * two threads at once, and processes with threads that end (exit from one
 * thread while others run, kill, the last thread's thread_exit, exec, a
 * fault). Prints a line for each check that fails and, at the end,
 * "threadtest: N checks, F failed"; exits with F.
 *
 * It runs as its own child for the checks on whole processes:
 *   threadtest live       starts threads that sleep, wait on a futex, join
 *                         another and spin, then exits with 7 from main
 *   threadtest hang       starts threads that sleep, wait on a futex and
 *                         join another, then waits on a futex itself, until
 *                         it is killed
 *   threadtest last       ends its main thread with thread_exit(0); its
 *                         other thread joins it and returns 9, which ends
 *                         the process with 9
 *   threadtest exec       starts threads that sleep, then execs
 *                         `threadtest alone PID TID...`, which exits 0 if
 *                         its pid and tid are PID and none of the TIDs is
 *                         there to join any more
 *   threadtest fault      a thread stores to address 0, which kills the
 *                         whole process */
#include "user.h"

/* Thread slots, for every process together (NTHREAD in
 * kernel/include/proc.h). */
#define NTHREAD 16
#define STACK_SIZE 1024
#define PAGE 4096
/* Where the heap may grow to (USER_LOAD_TOP in kernel/include/vm.h). */
#define USER_LOAD_TOP 0x003fe000
/* Process slots (NPROC in kernel/include/proc.h). */
#define NPROC 8

/* One stack for each thread there can be at once. */
static char stacks[NTHREAD][STACK_SIZE];

static int checks;
static int failures;

static void check(int ok, const char *what) {
    checks++;
    if (!ok) {
        failures++;
        printf("threadtest: FAILED: %s\n", what);
    }
}

static void check_eq(int got, int want, const char *what) {
    checks++;
    if (got != want) {
        failures++;
        printf("threadtest: FAILED: %s: got %d, want %d\n", what, got, want);
    }
}

/* Starts fn(arg) on stack `i`. */
static int start(int (*fn)(void *), void *arg, int i) {
    return thread_start(fn, arg, stacks[i], STACK_SIZE);
}

/* Joins `tid` and returns its status, or the error, as -1000 + it, so a
 * failed join can't pass for a status. */
static int joined(int tid) {
    int status = 0;
    int r = thread_join(tid, &status);
    return r == 0 ? status : -1000 + r;
}

/* ---- Creating, ending and joining ---- */

static int returns_arg(void *arg) {
    return (int)(uint)arg;
}

/* thread_exit from a few calls down: the innermost call ends the thread. */
static void deep_exit(int depth) {
    if (depth > 0) {
        deep_exit(depth - 1);
    }
    thread_exit(7);
}

static int exits_deep(void *arg) {
    (void)arg;
    deep_exit(5);
    return 99; /* not reached */
}

static int seen_tid;

static int stores_tid(void *arg) {
    (void)arg;
    atomic_store(&seen_tid, gettid());
    return 0;
}

static int ended_flag;

static int sets_flag(void *arg) {
    (void)arg;
    atomic_store(&ended_flag, 1);
    return 5;
}

/* thread_create's raw entry: a0 = the argument, sp = the stack, ra = 0, so
 * it must end with thread_exit. */
static int raw_arg;
static int raw_sp_ok;

static void raw_entry(void *arg) {
    char here;
    raw_arg = (int)(uint)arg;
    /* Its frame is just below the stack it was given. */
    uint sp = (uint)&here;
    raw_sp_ok = sp < (uint)(stacks[1] + STACK_SIZE) && sp > (uint)stacks[1];
    thread_exit(3);
}

/* The parent's pid, from procs: its main thread's tid, which is another
 * process's thread. */
static int parent_pid(void) {
    struct procinfo info[NPROC];
    int n = procs(info, NPROC);
    for (int i = 0; i < n && i < NPROC; i++) {
        if ((int)info[i].pid == getpid()) {
            return (int)info[i].ppid;
        }
    }
    return 0;
}

static void test_threads(void) {
    int pid = getpid();
    check_eq(gettid(), pid, "the main thread's tid is the pid");

    int t = start(returns_arg, (void *)42, 0);
    check(t > pid, "a new thread's tid is above the pid");
    check_eq(joined(t), 42, "join gets what the function returned");
    check_eq(thread_join(t, NULL), -ESRCH, "a thread is joined once");

    t = start(exits_deep, NULL, 0);
    check_eq(joined(t), 7, "thread_exit from deep in a thread");

    t = start(stores_tid, NULL, 0);
    check_eq(joined(t), 0, "join");
    check_eq(seen_tid, t, "gettid in a thread is its tid");

    /* Join one that has certainly ended: it set the flag on its way out,
     * and two ticks are more than it needs to finish. */
    t = start(sets_flag, NULL, 0);
    while (atomic_load(&ended_flag) == 0) {
        yield();
    }
    sleep(2);
    check_eq(joined(t), 5, "join a thread that has ended");

    /* A bad status pointer fails before anything, and the thread can still
     * be joined. */
    t = start(returns_arg, (void *)8, 0);
    check_eq(thread_join(t, (int *)0x80000000), -EFAULT, "join with a kernel pointer");
    check_eq(thread_join(t, (int *)USER_LOAD_TOP), -EFAULT, "join with the guard page");
    check_eq(joined(t), 8, "joinable after a bad pointer");
    check_eq(thread_join(t, NULL), -ESRCH, "and only once");

    t = start(returns_arg, (void *)9, 0);
    check_eq(thread_join(t, NULL), 0, "join without a status");
    check_eq(thread_join(gettid(), NULL), -EINVAL, "join yourself");
    check_eq(thread_join(0, NULL), -ESRCH, "join tid 0");
    check_eq(thread_join(-1, NULL), -ESRCH, "join tid -1");
    check_eq(thread_join(99999, NULL), -ESRCH, "join a tid nobody has");
    int ppid = parent_pid();
    check(ppid > 0, "a parent");
    check_eq(thread_join(ppid, NULL), -ESRCH, "join another process's thread");

    /* The raw call. */
    t = thread_create(raw_entry, (void *)77, stacks[1] + STACK_SIZE);
    check(t > 0, "thread_create");
    check_eq(joined(t), 3, "a raw thread ends with thread_exit");
    check_eq(raw_arg, 77, "a raw thread gets its argument in a0");
    check(raw_sp_ok, "a raw thread runs on the stack it was given");

    /* Its errors: the pointers first, then the alignment. */
    char *top = stacks[1] + STACK_SIZE;
    check_eq(thread_create((void (*)(void *))0, NULL, top), -EFAULT, "entry NULL");
    check_eq(thread_create((void (*)(void *))0x80000000, NULL, top), -EFAULT, "entry in the kernel");
    check_eq(thread_create((void (*)(void *))(uint)&checks, NULL, top), -EFAULT, "entry in data");
    check_eq(thread_create(raw_entry, NULL, NULL), -EFAULT, "stack NULL");
    check_eq(thread_create(raw_entry, NULL, (void *)0x80001000), -EFAULT, "stack in the kernel");
    check_eq(thread_create(raw_entry, NULL, (void *)USER_LOAD_TOP), -EFAULT, "stack over the guard page");
    check_eq(thread_create(raw_entry, NULL, (void *)0x2000), -EFAULT, "stack in the program's code");
    check_eq(thread_create(raw_entry, NULL, top - 8), -EINVAL, "stack not 16-byte aligned");
    check_eq(thread_create((void (*)(void *))((uint)raw_entry + 2), NULL, top), -EINVAL, "entry not aligned");
}

/* ---- Futexes ---- */

static int word;
static int woke_with;

static int waits_on_word(void *arg) {
    (void)arg;
    woke_with = futex_wait(&word, 0);
    return 0;
}

static int word2;
static int arrived;

static int waits_on_word2(void *arg) {
    (void)arg;
    atomic_add(&arrived, 1);
    /* Woken by futex_wake (0), or too late to sleep (-EAGAIN, once the
     * test has changed the word). */
    int r = futex_wait(&word2, 0);
    return r == 0 || r == -EAGAIN ? 0 : r;
}

static void test_futex(void) {
    word = 5;
    check_eq(futex_wait(&word, 4), -EAGAIN, "futex_wait on another value returns at once");
    check_eq(futex_wait(NULL, 0), -EFAULT, "futex_wait on NULL");
    check_eq(futex_wait((int *)0x80000000, 0), -EFAULT, "futex_wait on the kernel");
    check_eq(futex_wait((int *)USER_LOAD_TOP, 0), -EFAULT, "futex_wait on the guard page");
    check_eq(futex_wait((int *)((char *)&word + 2), 5), -EINVAL, "futex_wait misaligned");
    check_eq(futex_wake(NULL, 1), -EFAULT, "futex_wake on NULL");
    check_eq(futex_wake((int *)0x80000000, 1), -EFAULT, "futex_wake on the kernel");
    check_eq(futex_wake((int *)((char *)&word + 1), 1), -EINVAL, "futex_wake misaligned");
    check_eq(futex_wake(&word, 1), 0, "futex_wake with nobody waiting");
    /* Code is readable, so a futex there is fine, if useless. */
    check_eq(futex_wake((int *)0x1000, 1), 0, "futex_wake on code");

    /* One waiter: the wake returns 1 only once it sleeps. */
    word = 0;
    int t = start(waits_on_word, NULL, 0);
    int tries = 0;
    while (futex_wake(&word, 1) != 1 && tries < 10000) {
        yield();
        tries++;
    }
    check(tries < 10000, "futex_wake woke the waiter");
    check_eq(joined(t), 0, "join the waiter");
    check_eq(woke_with, 0, "futex_wait returns 0 when woken");

    /* Three waiters, woken at most two at a time. */
    word2 = 0;
    int tids[3];
    for (int i = 0; i < 3; i++) {
        tids[i] = start(waits_on_word2, NULL, i);
    }
    while (atomic_load(&arrived) < 3) {
        yield();
    }
    int woken = 0;
    tries = 0;
    while (woken == 0 && tries < 10000) {
        woken = futex_wake(&word2, 1);
        yield();
        tries++;
    }
    check_eq(woken, 1, "futex_wake(1) wakes one");
    check_eq(futex_wake(&word2, 0), 0, "futex_wake(0) wakes none");
    check_eq(futex_wake(&word2, -1), 0, "futex_wake(-1) wakes none");
    int most = 0;
    tries = 0;
    while (woken < 3 && tries < 10000) {
        int n = futex_wake(&word2, 2);
        if (n > most) {
            most = n;
        }
        woken += n;
        yield();
        tries++;
    }
    check(most <= 2, "futex_wake(2) wakes at most two");
    check_eq(woken, 3, "every waiter woken");
    for (int i = 0; i < 3; i++) {
        check_eq(joined(tids[i]), 0, "join a waiter");
    }
}

/* ---- The library's locks, and running out of slots ---- */

static struct mutex lock;
static struct spinlock slock;
static int counter;
static int spun;

static int adds(void *arg) {
    int n = (int)(uint)arg;
    for (int i = 0; i < n; i++) {
        mutex_lock(&lock);
        counter = counter + 1;
        mutex_unlock(&lock);
        spin_lock(&slock);
        spun = spun + 1;
        spin_unlock(&slock);
    }
    return 0;
}

static int gate;

static int waits_at_gate(void *arg) {
    (void)arg;
    while (atomic_load(&gate) == 0) {
        futex_wait(&gate, 0);
    }
    return 1;
}

/* Allocates, fills, checks and frees blocks of sizes that differ from one
 * thread to another, all at once: malloc's list holds up only if its lock
 * keeps the threads out of each other's way. 0 if every block held its
 * bytes. */
static int mallocs(void *arg) {
    int me = (int)(uint)arg;
    for (int round = 0; round < 20; round++) {
        uchar *blocks[4];
        for (int i = 0; i < 4; i++) {
            uint n = 16 + 24 * (uint)(me + i + round % 3);
            blocks[i] = malloc(n);
            if (blocks[i] == NULL) {
                return 1;
            }
            memset(blocks[i], me * 16 + i, n);
        }
        for (int i = 0; i < 4; i++) {
            uint n = 16 + 24 * (uint)(me + i + round % 3);
            for (uint j = 0; j < n; j++) {
                if (blocks[i][j] != (uchar)(me * 16 + i)) {
                    return 2;
                }
            }
            free(blocks[i]);
        }
    }
    return 0;
}

/* Grows and shrinks the heap a page at a time, while another thread does
 * the same: each sbrk is one at a time in the kernel, so the end of the
 * heap is where it started once both are done. */
static int sbrks(void *arg) {
    (void)arg;
    for (int i = 0; i < 20; i++) {
        if (sbrk(PAGE) < 0) {
            return 1;
        }
        if (sbrk(-PAGE) < 0) {
            return 2;
        }
    }
    return 0;
}

static void test_locks(void) {
    struct mutex m = {0};
    check_eq(mutex_trylock(&m), 1, "trylock a free mutex");
    check_eq(mutex_trylock(&m), 0, "trylock a held one");
    mutex_unlock(&m);
    check_eq(mutex_trylock(&m), 1, "trylock it again once free");
    mutex_unlock(&m);
    check_eq(m.state, 0, "an unlocked mutex is 0");

    int tids[3];
    for (int i = 0; i < 3; i++) {
        tids[i] = start(adds, (void *)200, i);
    }
    for (int i = 0; i < 3; i++) {
        check_eq(joined(tids[i]), 0, "join an adder");
    }
    check_eq(counter, 600, "the mutex keeps every addition");
    check_eq(spun, 600, "the spinlock keeps every addition");

    for (int i = 0; i < 3; i++) {
        tids[i] = start(mallocs, (void *)(uint)i, i);
    }
    for (int i = 0; i < 3; i++) {
        check_eq(joined(tids[i]), 0, "malloc from three threads at once");
    }

    int end = sbrk(0);
    tids[0] = start(sbrks, NULL, 0);
    tids[1] = start(sbrks, NULL, 1);
    check_eq(joined(tids[0]), 0, "sbrk from two threads");
    check_eq(joined(tids[1]), 0, "sbrk from two threads");
    check_eq(sbrk(0), end, "the heap is back where it was");

    /* Every free slot, until thread_start says there are none. */
    int all[NTHREAD];
    int n = 0;
    int err = 0;
    while (n < NTHREAD) {
        int t = start(waits_at_gate, NULL, n);
        if (t < 0) {
            err = t;
            break;
        }
        all[n] = t;
        n++;
    }
    check(n > 0 && n < NTHREAD, "threads until the slots run out");
    check_eq(err, -EAGAIN, "no slot left");
    atomic_store(&gate, 1);
    futex_wake(&gate, NTHREAD);
    int ok = 1;
    for (int i = 0; i < n; i++) {
        ok = ok && joined(all[i]) == 1;
    }
    check(ok, "join every one of them");
    int t = start(returns_arg, (void *)4, 0);
    check_eq(joined(t), 4, "a slot again once they are joined");
}

/* ---- Processes with threads ---- */

static int forever;

static int sleeps(void *arg) {
    (void)arg;
    sleep(100000);
    return 0;
}

static int waits_forever(void *arg) {
    (void)arg;
    for (;;) {
        futex_wait(&forever, 0);
    }
    return 0;
}

static int joins(void *arg) {
    return joined((int)(uint)arg);
}

static int spins(void *arg) {
    (void)arg;
    volatile int x = 0;
    for (;;) {
        x = x + 1;
    }
    return 0;
}

static int faults(void *arg) {
    (void)arg;
    *(volatile int *)0 = 1;
    return 0;
}

static int ends_last(void *arg) {
    int main_tid = (int)(uint)arg;
    /* The main thread has ended (or will): joining it gets its status. */
    int status = joined(main_tid);
    return status == 0 ? 9 : 100;
}

/* The child modes. */
static int child(int argc, char **argv) {
    const char *mode = argv[1];
    if (strcmp(mode, "live") == 0 || strcmp(mode, "hang") == 0) {
        int a = start(waits_forever, NULL, 0);
        start(sleeps, NULL, 1);
        start(joins, (void *)(uint)a, 2);
        if (strcmp(mode, "live") == 0) {
            start(spins, NULL, 3);
            sleep(2);
            exit(7);
        }
        for (;;) {
            futex_wait(&forever, 0);
        }
    }
    if (strcmp(mode, "last") == 0) {
        start(ends_last, (void *)(uint)gettid(), 0);
        thread_exit(0);
    }
    if (strcmp(mode, "exec") == 0) {
        char tids[3][12];
        for (int i = 0; i < 3; i++) {
            snprintf(tids[i], sizeof tids[i], "%d", start(sleeps, NULL, i));
        }
        char pid[12];
        snprintf(pid, sizeof pid, "%d", getpid());
        char *args[] = {"threadtest", "alone", pid, tids[0], tids[1], tids[2], NULL};
        exec("/bin/threadtest", args);
        return 50;
    }
    if (strcmp(mode, "alone") == 0) {
        int pid = atoi(argv[2]);
        if (getpid() != pid || gettid() != pid) {
            return 51;
        }
        for (int i = 3; i < argc; i++) {
            if (thread_join(atoi(argv[i]), NULL) != -ESRCH) {
                return 52;
            }
        }
        return 0;
    }
    if (strcmp(mode, "fault") == 0) {
        start(sleeps, NULL, 0);
        start(faults, NULL, 1);
        for (;;) {
            futex_wait(&forever, 0);
        }
    }
    return 60;
}

static int run(const char *mode) {
    char *args[] = {"threadtest", (char *)mode, NULL};
    return spawn("/bin/threadtest", args);
}

static int status_of(int pid) {
    int status = -1000;
    if (pid < 0 || wait(pid, &status) != pid) {
        return -2000;
    }
    return status;
}

/* The state procs() gives for `pid`, or -1 if it isn't there. */
static int state_of(int pid) {
    struct procinfo info[NPROC];
    int n = procs(info, NPROC);
    for (int i = 0; i < n && i < NPROC; i++) {
        if ((int)info[i].pid == pid) {
            return (int)info[i].state;
        }
    }
    return -1;
}

static void test_processes(void) {
    check_eq(status_of(run("live")), 7, "exit from main with threads running");
    check_eq(status_of(run("last")), 9, "the last thread's thread_exit ends the process");
    check_eq(status_of(run("exec")), 0, "exec ends the other threads");
    check_eq(status_of(run("fault")), -1, "a fault in a thread kills the process");

    /* kill, once every thread of the child sleeps. */
    int pid = run("hang");
    int tries = 0;
    while (state_of(pid) != PROC_SLEEPING && tries < 10000) {
        yield();
        tries++;
    }
    check(tries < 10000, "the child's threads all sleep");
    check_eq(kill(pid), 0, "kill a process with threads");
    check_eq(status_of(pid), -1, "it ends with -1");
}

int main(int argc, char **argv) {
    if (argc > 1) {
        return child(argc, argv);
    }
    test_threads();
    test_futex();
    test_locks();
    test_processes();
    printf("threadtest: %d checks, %d failed\n", checks, failures);
    return failures;
}
