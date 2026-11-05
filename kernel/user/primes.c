/* primes: counts the primes below a limit with several processes at once,
 * each counting one slice of the range by trial division: work that only
 * takes CPU time, to keep every hart busy (spec section 10, "Kernel SMP").
 *
 *   primes [WORKERS [LIMIT]]      (default 4 workers, below 20000)
 *
 * starts WORKERS copies of itself (`primes worker FROM TO`, at most 6, so
 * that with the shell and primes there are 8 processes, NPROC), the range
 * [0, LIMIT) cut into equal slices (the last one also takes what is left
 * over). Each counts the primes in its slice, prints `primes [FROM, TO): N`
 * and exits with N. primes waits for all of them, adds their counts up and
 * prints `primes: N below LIMIT, W workers` (exit 0), or what went wrong
 * (exit 1). */
#include "user.h"

#define MAX_WORKERS 6

static int is_prime(uint n) {
    if (n < 2) {
        return 0;
    }
    if (n % 2 == 0) {
        return n == 2;
    }
    for (uint d = 3; d * d <= n; d += 2) {
        if (n % d == 0) {
            return 0;
        }
    }
    return 1;
}

static int worker(uint from, uint to) {
    uint count = 0;
    for (uint n = from; n < to; n++) {
        count += (uint)is_prime(n);
    }
    printf("primes [%u, %u): %u\n", from, to, count);
    return (int)count;
}

int main(int argc, char **argv) {
    if (argc == 4 && strcmp(argv[1], "worker") == 0) {
        return worker((uint)atoi(argv[2]), (uint)atoi(argv[3]));
    }
    int workers = argc > 1 ? atoi(argv[1]) : 4;
    int limit = argc > 2 ? atoi(argv[2]) : 20000;
    if (argc > 3 || workers < 1 || workers > MAX_WORKERS || limit < workers || limit > 1000000) {
        fprintf(STDERR, "usage: primes [WORKERS [LIMIT]] (1 to %d workers, a limit up to 1000000)\n",
                MAX_WORKERS);
        return 1;
    }
    uint slice = (uint)limit / (uint)workers;
    int pids[MAX_WORKERS];
    for (int w = 0; w < workers; w++) {
        char from[12];
        char to[12];
        snprintf(from, sizeof from, "%u", (uint)w * slice);
        snprintf(to, sizeof to, "%u", w == workers - 1 ? (uint)limit : (uint)(w + 1) * slice);
        char *args[] = {"primes", "worker", from, to, NULL};
        pids[w] = spawn("/bin/primes", args, environ);
        if (pids[w] < 0) {
            fprintf(STDERR, "primes: cannot start worker %d: %s\n", w + 1, strerror(-pids[w]));
            /* Collect the ones already running before giving up. */
            for (int i = 0; i < w; i++) {
                wait(pids[i], NULL);
            }
            return 1;
        }
    }
    int total = 0;
    int failed = 0;
    for (int w = 0; w < workers; w++) {
        int status = 0;
        if (wait(pids[w], &status) != pids[w] || status < 0) {
            fprintf(STDERR, "primes: worker %d: exit %d\n", w + 1, status);
            failed = 1;
        } else {
            total += status;
        }
    }
    if (failed) {
        return 1;
    }
    printf("primes: %d below %d, %d workers\n", total, limit, workers);
    return 0;
}
