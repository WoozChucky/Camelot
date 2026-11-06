/* stress: many processes at once in every part of the kernel that they
 * share, under timer preemption, for the locking tests (spec section 10,
 * "Kernel locking").
 *
 *   stress [WORKERS [ROUNDS]]      (default 4 workers, 4 rounds)
 *
 * starts WORKERS copies of itself (`stress worker ID ROUNDS`, at most 6, so
 * that with the shell and stress there are 8 processes, NPROC; 5 on a root
 * image, whose init takes a slot too), each of
 * which, in each round:
 *
 *   - grows its heap by PAGES pages (sbrk: the page allocator), fills every
 *     word with a pattern of its own, spins for a while (so timer ticks
 *     preempt it and the others run in between), checks the pattern and
 *     shrinks the heap back (freeing the pages);
 *   - prints `stress W: round R` on the console, one write;
 *   - sends RECORD bytes down a pipe shared by all the workers, which stress
 *     reads at the same time (the pipe fills, and writers and the reader
 *     sleep on each other);
 *   - with a filesystem, writes a file of its own (stress.W in the working
 *     directory), reads it back, checks it and removes it (the log, the
 *     inode and block caches, the disk).
 *
 * A write to a pipe that is fuller than the write is long goes in pieces, so
 * the workers' bytes may interleave. Each byte says whose it is and where it
 * stands: the worker's id in the top three bits, its count of bytes sent so
 * far, mod 32, in the low five. stress checks every byte, waits for the
 * workers and prints `stress: W workers, R rounds, N bytes: ok` (exit 0),
 * or what went wrong (exit 1). A worker exits with the number of its first
 * failed check, 0 if none failed. */
#include "user.h"

#define MAX_WORKERS 6
#define PAGES 4
#define PAGE 4096
/* Spin iterations per round: about half a tick (a spin loop is five
 * instructions, a tick a million). */
#define SPIN 100000
/* The bytes each worker sends down the pipe per round. */
#define RECORD 200
/* The file each worker writes: a little over one block, so it takes two. */
#define FILE_BYTES 1500

static uint pattern(uint id, uint round, uint i) {
    return (id << 24) ^ (round << 16) ^ (i * 2654435761u);
}

/* Byte `n` of the ones worker `id` sends. */
static uchar pipe_byte(uint id, uint n) {
    return (uchar)((id << 5) | (n & 31));
}

/* The heap round: 1 if the pages kept what was written. */
static int heap_round(uint id, uint round) {
    char *old = sbrk(PAGES * PAGE);
    if ((long)old < 0) {
        return 0;
    }
    uint *words = (uint *)old;
    uint n = PAGES * PAGE / 4;
    for (uint i = 0; i < n; i++) {
        words[i] = pattern(id, round, i);
    }
    spin(SPIN);
    int ok = 1;
    for (uint i = 0; i < n; i++) {
        if (words[i] != pattern(id, round, i)) {
            ok = 0;
            break;
        }
    }
    if ((char *)sbrk(-(PAGES * PAGE)) != old + PAGES * PAGE) {
        ok = 0;
    }
    return ok;
}

/* The file round: 1 if the file read back as written, or if there is no
 * filesystem (open fails with -ENOENT); 0 otherwise. */
static int file_round(uint id, uint round) {
    static uchar data[FILE_BYTES];
    static uchar back[FILE_BYTES];
    char name[16];
    snprintf(name, sizeof name, "stress.%u", id);
    int fd = open(name, O_CREAT | O_TRUNC | O_RDWR);
    if (fd == -ENOENT) {
        return 1;
    }
    if (fd < 0) {
        return 0;
    }
    for (uint i = 0; i < FILE_BYTES; i++) {
        data[i] = (uchar)(id + round * 3 + i);
    }
    int ok = write(fd, data, FILE_BYTES) == FILE_BYTES && lseek(fd, 0, SEEK_SET) == 0 &&
             read(fd, back, FILE_BYTES) == FILE_BYTES && memcmp(data, back, FILE_BYTES) == 0;
    close(fd);
    if (unlink(name) != 0) {
        ok = 0;
    }
    return ok;
}

static int worker(uint id, uint rounds) {
    uchar record[RECORD];
    for (uint r = 1; r <= rounds; r++) {
        if (!heap_round(id, r)) {
            return 1;
        }
        fprintf(STDERR, "stress %u: round %u\n", id, r);
        for (uint i = 0; i < RECORD; i++) {
            record[i] = pipe_byte(id, (r - 1) * RECORD + i);
        }
        if (write(STDOUT, record, RECORD) != RECORD) {
            return 2;
        }
        if (!file_round(id, r)) {
            return 3;
        }
    }
    return 0;
}

int main(int argc, char **argv) {
    if (argc == 4 && strcmp(argv[1], "worker") == 0) {
        return worker((uint)atoi(argv[2]), (uint)atoi(argv[3]));
    }
    uint workers = argc > 1 ? (uint)atoi(argv[1]) : 4;
    uint rounds = argc > 2 ? (uint)atoi(argv[2]) : 4;
    if (argc > 3 || workers < 1 || workers > MAX_WORKERS || rounds < 1 || rounds > 1000) {
        fprintf(STDERR, "usage: stress [WORKERS [ROUNDS]] (1 to %d workers, 1 to 1000 rounds)\n",
                MAX_WORKERS);
        return 1;
    }
    /* The workers' output (descriptor 1) is the pipe; they print to the
     * console on 2. */
    int p[2];
    if (pipe(p) != 0) {
        fprintf(STDERR, "stress: pipe failed\n");
        return 1;
    }
    int out = dup(STDOUT);
    close(STDOUT);
    dup(p[1]);
    close(p[1]);
    int pids[MAX_WORKERS];
    char id[4];
    char count[8];
    snprintf(count, sizeof count, "%u", rounds);
    uint started = 0;
    for (uint w = 0; w < workers; w++) {
        snprintf(id, sizeof id, "%u", w + 1);
        char *args[] = {"stress", "worker", id, count, NULL};
        pids[w] = spawn("/bin/stress", args, environ);
        if (pids[w] < 0) {
            break;
        }
        started++;
    }
    /* Only the workers hold the write end now: the pipe ends once they
     * have all exited. */
    close(STDOUT);
    dup(out);
    close(out);

    uchar buf[64];
    uint seen[MAX_WORKERS + 1];
    memset(seen, 0, sizeof seen);
    uint total = 0;
    int bad = 0;
    int n;
    while (!bad && (n = read(p[0], buf, sizeof buf)) > 0) {
        for (int i = 0; i < n; i++) {
            uint w = buf[i] >> 5;
            if (w < 1 || w > started || buf[i] != pipe_byte(w, seen[w])) {
                fprintf(STDERR, "stress: bad byte %x after %u\n", buf[i], total);
                bad = 1;
                break;
            }
            seen[w]++;
            total++;
        }
    }
    close(p[0]);
    int failed = bad || started < workers;
    for (uint w = 0; w < started; w++) {
        int status;
        if (wait(pids[w], &status) != pids[w] || status != 0) {
            fprintf(STDERR, "stress: worker %u: exit %d\n", w + 1, status);
            failed = 1;
        } else if (seen[w + 1] != rounds * RECORD) {
            fprintf(STDERR, "stress: worker %u: %u bytes, want %u\n", w + 1, seen[w + 1], rounds * RECORD);
            failed = 1;
        }
    }
    if (failed) {
        return 1;
    }
    printf("stress: %u workers, %u rounds, %u bytes: ok\n", workers, rounds, total);
    return 0;
}
