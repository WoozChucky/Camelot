/* threads: several threads of one process adding to shared counters under
 * locks (spec section 10, "Kernel threads"; issue #64's first criterion).
 *
 *   threads [THREADS [ROUNDS]]      (default 4 threads, 500 rounds)
 *
 * starts THREADS threads (1 to 8) with thread_start, on stacks in .bss.
 * Each, ROUNDS times, takes the mutex and adds one to `counter` by reading
 * it, working for a while and writing it back, which loses updates unless
 * the mutex keeps the others out meanwhile; then it does the same to
 * `spun` under a spinlock. Each also counts its own rounds, and returns its
 * number + 100. The main thread joins them all, checks every status and
 * that both counters hold THREADS * ROUNDS, and prints
 *
 *     threads: T threads, R rounds: counter N, spun N: ok
 *
 * (exit 0), or what went wrong (exit 1). On several harts the threads run
 * at once, on one hart they take turns on the timer tick: the counts must
 * come out the same either way. */
#include "user.h"

#define MAX_THREADS 8
#define STACK_SIZE 2048

static char stacks[MAX_THREADS][STACK_SIZE];

static struct mutex lock;
static struct spinlock slock;
static int counter;
static int spun;
static int rounds;
static int done[MAX_THREADS];

/* Some work between the read and the write: the window in which another
 * thread, without the lock, would read the same value. */
static int work(int x) {
    volatile int v = x;
    for (int i = 0; i < 20; i++) {
        v = v + i - i;
    }
    return v;
}

static int worker(void *arg) {
    int me = (int)(uint)arg;
    for (int r = 0; r < rounds; r++) {
        mutex_lock(&lock);
        int c = counter;
        counter = work(c) + 1;
        mutex_unlock(&lock);

        spin_lock(&slock);
        int s = spun;
        spun = work(s) + 1;
        spin_unlock(&slock);

        done[me] = done[me] + 1;
    }
    return me + 100;
}

int main(int argc, char **argv) {
    int n = argc > 1 ? atoi(argv[1]) : 4;
    rounds = argc > 2 ? atoi(argv[2]) : 500;
    if (argc > 3 || n < 1 || n > MAX_THREADS || rounds < 1) {
        fprintf(STDERR, "usage: threads [THREADS [ROUNDS]] (1 to %d threads)\n", MAX_THREADS);
        return 1;
    }
    int tids[MAX_THREADS];
    for (int i = 0; i < n; i++) {
        tids[i] = thread_start(worker, (void *)(uint)i, stacks[i], STACK_SIZE);
        if (tids[i] < 0) {
            fprintf(STDERR, "threads: thread_start: %s\n", strerror(-tids[i]));
            return 1;
        }
    }
    int failed = 0;
    for (int i = 0; i < n; i++) {
        int status = -1;
        int r = thread_join(tids[i], &status);
        if (r != 0) {
            fprintf(STDERR, "threads: thread_join %d: %s\n", tids[i], strerror(-r));
            failed = 1;
        } else if (status != i + 100) {
            fprintf(STDERR, "threads: thread %d ended with %d, not %d\n", i, status, i + 100);
            failed = 1;
        } else if (done[i] != rounds) {
            fprintf(STDERR, "threads: thread %d did %d rounds\n", i, done[i]);
            failed = 1;
        }
    }
    int want = n * rounds;
    if (counter != want || spun != want) {
        fprintf(STDERR, "threads: counter %d, spun %d, want %d\n", counter, spun, want);
        failed = 1;
    }
    if (failed) {
        return 1;
    }
    printf("threads: %d threads, %d rounds: counter %d, spun %d: ok\n", n, rounds, counter, spun);
    return 0;
}
