/* threadtest: checks the thread and futex calls do what
 * docs/camelot-syscalls.md ("Threads") says, and the library's locks on
 * them: creating, ending and joining threads and every error of each call,
 * futexes (a value that differs returns at once, a bad pointer is an
 * error, a wake wakes no more than it is asked to), running out of thread
 * slots, the library's mutex and malloc from several threads, sbrk from
 * three threads at once, a pipe write whose buffer another thread unmaps
 * meanwhile, and processes with threads that end (exit from one thread
 * while others run, kill, the last thread's thread_exit, exec, a fault).
 * Prints a line for each check that fails and, at the end,
 * "threadtest: N checks, F failed"; exits with F.
 *
 *   threadtest fs         instead checks threads using the root filesystem
 *                         while others exec, exit, are killed or close
 *                         their descriptors, and prints "threadtest fs: N
 *                         checks, F failed". It needs a disk, and leaves
 *                         it as it found it.
 *
 * It runs as its own child for the checks on whole processes:
 *   threadtest live       starts threads that sleep, wait on a futex, join
 *                         another and spin, then exits with 7 from main
 *   threadtest hang       starts threads that wait on a futex and join
 *                         another, then waits on a futex itself, until it is
 *                         killed: no thread sleeps on a timer, so only
 *                         kill's own wakeups can end it
 *   threadtest last       ends its main thread with thread_exit(0); its
 *                         other thread joins it and returns 9, which ends
 *                         the process with 9
 *   threadtest exec       starts threads that sleep, then execs
 *                         `threadtest alone PID TID...`, which exits 0 if
 *                         its pid and tid are PID and none of the TIDs is
 *                         there to join any more
 *   threadtest fault      a thread stores to address 0, which kills the
 *                         whole process
 * and, for `threadtest fs`:
 *   threadtest execread   a thread opens, reads and closes /bin/threadtest
 *                         over and over while main execs it (`alone PID`)
 *   threadtest execwrite  a thread writes WRITE_SIZE bytes at a time, more
 *                         than one transaction each, to TMP_WRITE, and main
 *                         execs (`alone PID`) once it has a few times
 *   threadtest ioexit     threads read the program and write TMP_WRITE
 *                         over and over; main exits with 7
 *   threadtest iokill     the same, main waiting on a futex until it is
 *                         killed */
#include "user.h"

/* Thread slots, for every process together (NTHREAD in
 * kernel/include/proc.h). */
#define NTHREAD 16
#define STACK_SIZE 1024
#define PAGE 4096
/* Where the heap may grow to (USER_LOAD_TOP in kernel/include/vm.h). */
#define USER_LOAD_TOP 0x0b7ff000
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

/* How many of the futex test's waiters have woken. */
static int left_word2;

static int counts_wake(void *arg) {
    int r = waits_on_word2(arg);
    atomic_add(&left_word2, 1);
    return r;
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

    /* Four waiters, woken one, then two, at a time: once all four sleep,
     * a wake of two leaves at least one asleep to show it woke no more.
     * They sleep a few instructions after arriving; the two ticks give
     * them those. */
    word2 = 0;
    int tids[4];
    for (int i = 0; i < 4; i++) {
        tids[i] = start(counts_wake, NULL, i);
    }
    while (atomic_load(&arrived) < 4) {
        yield();
    }
    sleep(2);
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
    int n = futex_wake(&word2, 2);
    check_eq(n, 2, "futex_wake(2) with three asleep wakes two");
    woken += n;
    sleep(2);
    check_eq(atomic_load(&left_word2), 3, "and only two more have woken");
    int most = 0;
    tries = 0;
    while (woken < 4 && tries < 10000) {
        n = futex_wake(&word2, 2);
        if (n > most) {
            most = n;
        }
        woken += n;
        yield();
        tries++;
    }
    check(most <= 1, "the last one is woken alone");
    check_eq(woken, 4, "every waiter woken");
    for (int i = 0; i < 4; i++) {
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

/* Grows and shrinks the heap a page at a time, while other threads do the
 * same: each sbrk is one at a time in the kernel, and each shrink follows
 * its own growth, so the end of the heap is where it started once all are
 * done. On several harts each shrink's shootdown reaches the harts the
 * others run on. */
static int sbrks(void *arg) {
    (void)arg;
    for (int i = 0; i < 20; i++) {
        if (sbrk(PAGE) < 0) {
            return 1;
        }
        /* Some time in U-mode, for the others' shootdowns to find this
         * thread's hart on the page table rather than asleep on the heap's
         * lock. Not in the new page: another thread's shrink may take it. */
        volatile int x = 0;
        for (int j = 0; j < 5000; j++) {
            x = x + j;
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
    for (int i = 0; i < 3; i++) {
        tids[i] = start(sbrks, NULL, i);
    }
    for (int i = 0; i < 3; i++) {
        check_eq(joined(tids[i]), 0, "sbrk from three threads");
    }
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

/* ---- A buffer unmapped while a call uses it ---- */

static int pipe_fds[2];
static uint pipe_src;
static int pipe_started;

/* Writes 1024 bytes from pipe_src: the first half in a page that stays,
 * the second in the heap's last page, which the main thread unmaps while
 * this one waits for room in the pipe. Returns what write returned. */
static int writes_pipe(void *arg) {
    (void)arg;
    atomic_store(&pipe_started, 1);
    return write(pipe_fds[1], (void *)pipe_src, 1024);
}

static void test_unmapped(void) {
    /* The heap's end on a page boundary, then one more page, B. The 512
     * bytes before B are 'A's, B's first 512 'B's: a write of 1024 fills the
     * pipe (512 bytes) with the 'A's and waits. */
    int end = sbrk(0);
    int pad = (PAGE - end % PAGE) % PAGE;
    if (pad > 0) {
        sbrk(pad);
    }
    char *b = (char *)sbrk(PAGE);
    check(b == (char *)(end + pad), "a page for the pipe's buffer");
    /* The page before B, the heap's or the program's data, stays. */
    pipe_src = (uint)b - 512;
    memset(b - 512, 'A', 512);
    memset(b, 'B', 512);
    check_eq(pipe(pipe_fds), 0, "pipe");
    int t = start(writes_pipe, NULL, 0);
    while (atomic_load(&pipe_started) == 0) {
        yield();
    }
    /* Two ticks: the writer has filled the pipe and sleeps in write. */
    sleep(2);
    check_eq(sbrk(-PAGE), (int)((uint)b + PAGE), "unmap the buffer's second half");
    char got[1024];
    int n = 0;
    int r = read(pipe_fds[0], got, sizeof got);
    if (r > 0) {
        n = r;
    }
    /* The write fails where its buffer went: a short write of the half
     * that was there, and nothing it couldn't copy reaches the pipe. */
    check_eq(joined(t), 512, "a write whose buffer is unmapped stops short");
    close(pipe_fds[1]);
    while ((r = read(pipe_fds[0], got + n, sizeof got - (uint)n)) > 0) {
        n += r;
    }
    close(pipe_fds[0]);
    check_eq(n, 512, "the reader gets only what was copied");
    int as = 0;
    for (int i = 0; i < n; i++) {
        if (got[i] == 'A') {
            as++;
        }
    }
    check_eq(as, 512, "all of it the first half");
    /* A buffer that is unmapped from the start is an error, not 0. */
    check_eq(pipe(pipe_fds), 0, "pipe");
    check_eq(write(pipe_fds[1], b, 16), -EFAULT, "write from an unmapped buffer");
    check_eq(write(pipe_fds[1], "x", 1), 1, "write a byte");
    check_eq(read(pipe_fds[0], b, 16), -EFAULT, "read into an unmapped buffer");
    check_eq(read(pipe_fds[0], got, 16), 1, "and the byte is still there");
    close(pipe_fds[0]);
    close(pipe_fds[1]);
    check_eq(fstat(0, (struct stat *)b), -EFAULT, "fstat to an unmapped buffer");
    sbrk(-pad);
    check_eq(sbrk(0), end, "the heap is back where it was");
}

/* ---- Threads on the filesystem (threadtest fs) ---- */

/* The program, which the root image has. */
#define PROGRAM "/bin/threadtest"
/* The scratch files, removed by the end. */
#define TMP_WRITE "/threadtest.w"
#define TMP_CLOSE "/threadtest.c"
/* More than a transaction's worth (MAXWRITE in kernel/file.c, 3 KiB): each
 * write is several, one after another. */
#define WRITE_SIZE 8192

static char wbuf[WRITE_SIZE];
static int io_rounds;
static int io_go;

/* Opens, reads a little of and closes the program, over and over. */
static int reads_program(void *arg) {
    (void)arg;
    char buf[16];
    for (;;) {
        int fd = open(PROGRAM, O_RDONLY);
        if (fd >= 0) {
            read(fd, buf, sizeof buf);
            close(fd);
        }
        atomic_add(&io_rounds, 1);
    }
    return 0;
}

/* Once io_go is set, writes WRITE_SIZE bytes to TMP_WRITE over and over,
 * from its start each time, yielding between writes: begin_op() is not
 * fair, and a writer that never lets go of the hart on one hart could
 * start every transaction before the main thread gets one. */
static int writes_file(void *arg) {
    (void)arg;
    while (atomic_load(&io_go) == 0) {
        yield();
    }
    int fd = open(TMP_WRITE, O_WRONLY | O_CREAT | O_TRUNC);
    for (;;) {
        lseek(fd, 0, SEEK_SET);
        write(fd, wbuf, WRITE_SIZE);
        atomic_add(&io_rounds, 1);
        yield();
    }
    return 0;
}

/* exec of `alone` with only the pid: exits 0 if it is the same process,
 * with one thread. */
static int exec_alone(void) {
    char pid[12];
    snprintf(pid, sizeof pid, "%d", getpid());
    char *args[] = {"threadtest", "alone", pid, NULL};
    return exec(PROGRAM, args);
}

/* The filesystem child modes, or -1 for another. */
static int fs_child(const char *mode) {
    if (strcmp(mode, "execread") == 0) {
        start(reads_program, NULL, 0);
        while (atomic_load(&io_rounds) == 0) {
            yield();
        }
        exec_alone();
        return 70;
    }
    if (strcmp(mode, "execwrite") == 0) {
        start(writes_file, NULL, 0);
        atomic_store(&io_go, 1);
        /* A few rounds first: the writer is in the middle of its writes
         * when the exec starts, and they have pushed the program out of the
         * buffer cache, so that loading it waits for the disk meanwhile. */
        while (atomic_load(&io_rounds) < 4) {
            yield();
        }
        exec_alone();
        return 71;
    }
    if (strcmp(mode, "ioexit") == 0 || strcmp(mode, "iokill") == 0) {
        start(reads_program, NULL, 0);
        start(writes_file, NULL, 1);
        atomic_store(&io_go, 1);
        while (atomic_load(&io_rounds) < 4) {
            yield();
        }
        if (strcmp(mode, "ioexit") == 0) {
            exit(7);
        }
        for (;;) {
            futex_wait(&forever, 0);
        }
    }
    return -1;
}

/* For the close checks: a descriptor another thread closes while these
 * use it. Each returns the error that stopped it. */
static int shared_fd;
static int used_fd;

static int reads_fd(void *arg) {
    (void)arg;
    char buf[64];
    for (;;) {
        int r = read(shared_fd, buf, sizeof buf);
        if (r < 0) {
            return r;
        }
        if (r == 0) {
            lseek(shared_fd, 0, SEEK_SET);
        }
        atomic_add(&used_fd, 1);
    }
    return 0;
}

static int writes_fd(void *arg) {
    (void)arg;
    for (;;) {
        int r = write(shared_fd, wbuf, WRITE_SIZE);
        if (r < 0) {
            return r;
        }
        lseek(shared_fd, 0, SEEK_SET);
        atomic_add(&used_fd, 1);
    }
    return 0;
}

static int chdirs(void *arg) {
    (void)arg;
    for (int i = 0; i < 50; i++) {
        if (chdir("/bin") != 0 || chdir("/") != 0) {
            return 1;
        }
    }
    return 0;
}

static int opens(void *arg) {
    (void)arg;
    for (int i = 0; i < 50; i++) {
        int fd = open(PROGRAM, O_RDONLY);
        if (fd < 0) {
            return 1;
        }
        close(fd);
    }
    return 0;
}

static void test_fs(void) {
    memset(wbuf, 'w', sizeof wbuf);
    /* exec waits for its other threads only once it has closed the
     * program: a thread reading the same file, or waiting for room in the
     * log, finishes its call and ends. */
    check_eq(status_of(run("execread")), 0, "exec while a thread reads the program");
    check_eq(status_of(run("execwrite")), 0, "exec while a thread writes");
    /* exit and kill while threads are in the middle of file calls. */
    check_eq(status_of(run("ioexit")), 7, "exit while threads read and write");
    int pid = run("iokill");
    sleep(3);
    check_eq(kill(pid), 0, "kill a process whose threads read and write");
    check_eq(status_of(pid), -1, "it ends with -1");
    check_eq(unlink(TMP_WRITE), 0, "remove the written file");

    /* close of a descriptor other threads read and write: their calls
     * under way finish on the file they hold; the next ones find no
     * descriptor. */
    shared_fd = open(TMP_CLOSE, O_RDWR | O_CREAT | O_TRUNC);
    check(shared_fd >= 0, "open a file to share");
    check_eq(write(shared_fd, wbuf, 100), 100, "write it");
    int r = start(reads_fd, NULL, 0);
    int w = start(writes_fd, NULL, 1);
    while (atomic_load(&used_fd) < 8) {
        yield();
    }
    check_eq(close(shared_fd), 0, "close it while two threads use it");
    check_eq(joined(r), -EBADF, "the reader's next read finds it closed");
    check_eq(joined(w), -EBADF, "the writer's next write finds it closed");
    check_eq(unlink(TMP_CLOSE), 0, "remove it");

    /* chdir in one thread while another opens. */
    r = start(chdirs, NULL, 0);
    w = start(opens, NULL, 1);
    check_eq(joined(r), 0, "chdir from one thread");
    check_eq(joined(w), 0, "while another opens");
    check_eq(chdir("/"), 0, "back to the root");
    int fd = open("bin/threadtest", O_RDONLY);
    check(fd >= 0, "a relative path from there");
    close(fd);
}

/* The child modes. */
static int child(int argc, char **argv) {
    const char *mode = argv[1];
    if (strcmp(mode, "live") == 0) {
        int a = start(waits_forever, NULL, 0);
        start(sleeps, NULL, 1);
        start(joins, (void *)(uint)a, 2);
        start(spins, NULL, 3);
        sleep(2);
        exit(7);
    }
    if (strcmp(mode, "hang") == 0) {
        /* No thread in sleep(), which wakes on every tick and would see the
         * kill by itself: kill must wake the futex and join waiters. */
        int a = start(waits_forever, NULL, 0);
        start(joins, (void *)(uint)a, 1);
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
    int r = fs_child(mode);
    return r >= 0 ? r : 60;
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
    if (argc > 1 && strcmp(argv[1], "fs") == 0) {
        test_fs();
        printf("threadtest fs: %d checks, %d failed\n", checks, failures);
        return failures;
    }
    if (argc > 1) {
        return child(argc, argv);
    }
    test_threads();
    test_futex();
    test_locks();
    test_unmapped();
    test_processes();
    printf("threadtest: %d checks, %d failed\n", checks, failures);
    return failures;
}
