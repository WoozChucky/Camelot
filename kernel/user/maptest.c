/* maptest: checks memory mappings (docs/camelot-syscalls.md, "Memory
 * mappings"; kernel/include/mmap.h, shm.h): mmap, munmap and mprotect of
 * anonymous memory, their errors, demand paging, the kernel's pointer
 * checks over mapped pages, and shared-memory objects (memfd_create,
 * ftruncate) mapped twice in one process and in two. Prints a line for
 * each check that fails and, at the end, "maptest: N checks, F failed";
 * exits with F.
 *
 *   maptest              the checks, which fit in a machine's 8 MiB
 *   maptest none         touches a PROT_NONE page: killed
 *   maptest unmapped     touches a page after munmap: killed
 *   maptest readonly     writes a read-only page: killed
 *   maptest past         touches a page past a shared object's end: killed
 *   maptest fb           draws into /dev/fb0's memory (a root image's)
 *   maptest threads      threads map and unmap memory at once
 *   maptest child        (shared_checks()'s child) */
#include "user.h"

#define PAGE 4096ul
#define MIB (1024ul * 1024ul)

static int checks;
static int failed;

static void check(int ok, const char *what) {
    checks++;
    if (!ok) {
        failed++;
        printf("maptest: FAILED: %s\n", what);
    }
}

static void check_eq(long got, long want, const char *what) {
    checks++;
    if (got != want) {
        failed++;
        printf("maptest: FAILED: %s: got %ld, want %ld\n", what, got, want);
    }
}

/* mmap's result as a number: an address, or minus an error number. */
static long map(void *addr, size_t len, int prot, int flags, int fd, off_t offset) {
    return (long)mmap(addr, len, prot, flags, fd, offset);
}

/* An anonymous private mapping of `len` bytes, readable and writable,
 * wherever the kernel puts it: its address, or minus an error. */
static long anon(size_t len) {
    return map(0, len, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
}

/* The pipe the pointer checks write into and read from. */
static int fds[2];

/* What write() of `len` bytes at `addr` into the pipe returns; whatever it
 * wrote is read back out, so the pipe never fills. */
static long write_at(uintptr_t addr, size_t len) {
    long r = write(fds[1], (const void *)addr, len);
    char sink[64];
    if (r > 0) {
        read(fds[0], sink, (size_t)r);
    }
    return r;
}

/* What read() of `len` bytes (16 at most) into `addr` from the pipe
 * returns, with "0123..." waiting in it; what a failed read left there is
 * taken out, so the pipe is empty again. */
static long read_at(uintptr_t addr, size_t len) {
    write(fds[1], "0123456789abcdef", len);
    long r = read(fds[0], (void *)addr, len);
    if (r < 0) {
        char sink[16];
        read(fds[0], sink, len);
    }
    return r;
}

static void anonymous_checks(void) {
    long a = anon(3 * PAGE);
    check(a > 0 && a % (long)PAGE == 0, "mmap gives a page-aligned address");
    if (a <= 0) {
        return;
    }
    char *p = (char *)a;
    char here;
    check((uintptr_t)a > (uintptr_t)sbrk(0) && (uintptr_t)a < (uintptr_t)&here,
          "between the heap and the stack");
    check(p[0] == 0 && p[PAGE] == 0 && p[3 * PAGE - 1] == 0, "the pages read zero");
    p[0] = 'a';
    p[PAGE] = 'b';
    p[2 * PAGE + 7] = 'c';
    check(p[0] == 'a' && p[PAGE] == 'b' && p[2 * PAGE + 7] == 'c', "and keep what is written");

    /* The next mapping goes below, from the top down. */
    long b = anon(PAGE);
    check(b > 0 && b < a, "the next mapping is below");

    /* The kernel's copies reach pages not touched yet: it maps them as a
     * touch would. */
    long c = anon(2 * PAGE);
    check(c > 0, "map two pages");
    check_eq(write_at((uintptr_t)c + PAGE - 4, 8), 8, "write() from pages never touched");
    check_eq(read_at((uintptr_t)c + PAGE + 100, 16), 16, "read() into a page never touched");
    check(memcmp((char *)c + PAGE + 100, "0123456789abcdef", 16) == 0, "what read() put there");

    /* munmap of the middle page: the others stay, with what they hold. */
    check_eq(munmap(p + PAGE, PAGE), 0, "munmap the middle page");
    check(p[0] == 'a' && p[2 * PAGE + 7] == 'c', "the pages around it keep their bytes");
    check_eq(write_at((uintptr_t)p + PAGE, 1), -EFAULT, "the hole is no user memory");
    check_eq(write_at((uintptr_t)p + PAGE - 1, 2), -EFAULT, "nor a range across it");
    check_eq(map(p + PAGE, PAGE, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED_NOREPLACE,
                 -1, 0),
             a + (long)PAGE, "MAP_FIXED_NOREPLACE fills the hole");
    check(p[PAGE] == 0, "with a new zeroed page");
    check_eq(map(p, PAGE, PROT_READ, MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED_NOREPLACE, -1, 0), -EEXIST,
             "MAP_FIXED_NOREPLACE over a mapping");
    check_eq(map(p, PAGE, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED, -1, 0), a,
             "MAP_FIXED over a mapping");
    check(p[0] == 0 && p[2 * PAGE + 7] == 'c', "replaces it, and only it");

    /* mprotect: read-only, then nothing, then back; the bytes stay. */
    p[2 * PAGE] = 'd';
    check_eq(mprotect(p + 2 * PAGE, PAGE, PROT_READ), 0, "mprotect read-only");
    check(p[2 * PAGE] == 'd', "still readable");
    check_eq(read_at((uintptr_t)p + 2 * PAGE, 4), -EFAULT, "read() into it");
    check_eq(write_at((uintptr_t)p + 2 * PAGE, 4), 4, "write() from it");
    check_eq(mprotect(p + 2 * PAGE, PAGE, PROT_NONE), 0, "mprotect PROT_NONE");
    check_eq(write_at((uintptr_t)p + 2 * PAGE, 4), -EFAULT, "write() from a PROT_NONE page");
    check_eq(mprotect(p, 3 * PAGE, PROT_READ | PROT_WRITE), 0, "mprotect the three back");
    check(p[2 * PAGE] == 'd' && p[2 * PAGE + 7] == 'c', "the page kept its bytes");
    p[2 * PAGE] = 'e';
    check(p[2 * PAGE] == 'e', "writable again");
    check_eq(mprotect(p, 4 * PAGE, PROT_READ), -ENOMEM, "mprotect past the mapping");
    check_eq(mprotect(p + 1, PAGE, PROT_READ), -EINVAL, "mprotect an unaligned address");
    check_eq(mprotect(p, PAGE, 0x10), -EINVAL, "mprotect with a protection that doesn't exist");
    check_eq(mprotect(p, PAGE, PROT_READ | PROT_WRITE | PROT_SEM), 0, "PROT_SEM, which means nothing");
    check_eq(mprotect(p, 0, PROT_READ), 0, "mprotect of nothing");

    /* A hint is taken where there is room. */
    check_eq(munmap(p, 3 * PAGE), 0, "munmap the three");
    check_eq(map(p + PAGE, PAGE, PROT_READ, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0), a + (long)PAGE,
             "a hint where there is room");
    check_eq(munmap(p + PAGE, PAGE), 0, "munmap it");
    check_eq(munmap(p, 3 * PAGE), 0, "munmap where nothing is mapped");
    check_eq(write_at((uintptr_t)p, 1), -EFAULT, "unmapped");
    check_eq(munmap((void *)b, PAGE), 0, "munmap the second");
    check_eq(munmap((void *)c, 2 * PAGE), 0, "munmap the third");
}

static void error_checks(void) {
    check_eq(anon(0), -EINVAL, "mmap of nothing");
    long odd = map(0, PAGE, PROT_READ | PROT_WRITE | 0x10, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    check(odd > 0, "a protection bit mmap doesn't know is ignored");
    if (odd > 0) {
        *(char *)odd = 1;
        munmap((void *)odd, PAGE);
    }
    check_eq(map(0, PAGE, PROT_READ, MAP_ANONYMOUS, -1, 0), -EINVAL, "neither shared nor private");
    check_eq(map(0, PAGE, PROT_READ, 0x04 | MAP_ANONYMOUS, -1, 0), -EINVAL, "a type that doesn't exist");
    check_eq(map((void *)(PAGE + 1), PAGE, PROT_READ, MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED, -1, 0),
             -EINVAL, "MAP_FIXED at an unaligned address");
    check_eq(map(0, PAGE, PROT_READ, MAP_PRIVATE, 15, 0), -EBADF, "a descriptor that isn't open");
    check_eq(map(0, PAGE, PROT_READ, MAP_SHARED, fds[0], 0), -ENODEV, "a pipe");
    check_eq(map(0, PAGE, PROT_READ, MAP_SHARED, 0, 0), -ENODEV, "the console");
    check_eq(map(0, PAGE, PROT_READ, MAP_SHARED, fds[1], 0), -EACCES, "a descriptor not open for reading");
    check_eq(map(0, PAGE, PROT_READ, MAP_SHARED, fds[0], 1), -EINVAL, "an unaligned offset");
    check_eq(anon(64 * 1024 * MIB), -ENOMEM, "more than the RAM");
    check_eq(map((void *)PAGE, PAGE, PROT_READ, MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED, -1, 0), -ENOMEM,
             "MAP_FIXED over the program");
    check_eq(munmap((void *)(PAGE + 1), PAGE), -EINVAL, "munmap an unaligned address");
    check_eq(munmap((void *)PAGE, 0), -EINVAL, "munmap nothing");
    check_eq(munmap((void *)PAGE, ~0ul - PAGE), -EINVAL, "munmap past user space");
}

/* The heap can't grow into a mapping, and comes back once it's gone. */
static void heap_checks(void) {
    uintptr_t end = ((uintptr_t)sbrk(0) + PAGE - 1) & ~(PAGE - 1);
    long m = map((void *)(end + PAGE), PAGE, PROT_READ, MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED, -1, 0);
    check_eq(m, (long)(end + PAGE), "a mapping just above the heap");
    long before = (long)sbrk(0);
    check_eq((long)sbrk((long)(2 * PAGE)), -ENOMEM, "sbrk into the mapping");
    check_eq((long)sbrk(0), before, "changes nothing");
    check_eq(munmap((void *)m, PAGE), 0, "munmap it");
    check_eq((long)sbrk((long)(2 * PAGE)), before, "then sbrk grows there");
    check_eq((long)sbrk(-(long)(2 * PAGE)), before + (long)(2 * PAGE), "and shrinks back");
}

/* Pages are mapped on their first touch: 200 mappings of a megabyte, much
 * more than a test machine's RAM, each touched once, cost a page each (and
 * their page tables). */
static void demand_checks(void) {
    long first = 0;
    int mapped = 0;
    for (int i = 0; i < 200; i++) {
        long m = anon(MIB);
        if (m <= 0) {
            break;
        }
        if (first == 0) {
            first = m;
        }
        ((char *)m)[i * 8] = (char)i;
        mapped++;
    }
    check_eq(mapped, 200, "200 MiB of mappings, a page of each touched");
    /* They went one below the other, from `first` down, one area. */
    char *low = (char *)(first - 199 * (long)MIB);
    int bad = 0;
    for (int i = 0; i < 200; i++) {
        if (low[(long)(199 - i) * (long)MIB + i * 8] != (char)i) {
            bad++;
        }
    }
    check_eq(bad, 0, "each kept its byte");
    check_eq(munmap(low, 200 * MIB), 0, "munmap them all at once");
}

/* memfd_create and ftruncate, and two mappings of one object in one
 * process. */
static void memfd_checks(void) {
    int fd = memfd_create("maptest", 0);
    check(fd >= 0, "memfd_create");
    if (fd < 0) {
        return;
    }
    struct stat st;
    check(fstat(fd, &st) == 0 && st.type == T_FILE && st.size == 0, "an empty regular file to fstat");
    check_eq(fcntl(fd, F_GETFD, 0), 0, "not closed on exec");
    check_eq(ftruncate(fd, (off_t)(2 * PAGE)), 0, "ftruncate to two pages");
    check(fstat(fd, &st) == 0 && st.size == (off_t)(2 * PAGE), "fstat gives the size");

    long a = map(0, 2 * PAGE, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
    long b = map(0, PAGE, PROT_READ | PROT_WRITE, MAP_SHARED, fd, (off_t)PAGE);
    check(a > 0 && b > 0, "map the object twice, the second from its second page");
    if (a <= 0 || b <= 0) {
        close(fd);
        return;
    }
    char *x = (char *)a;
    char *y = (char *)b;
    check(x[0] == 0 && x[PAGE] == 0, "a new object reads zero");
    x[PAGE + 5] = 'x';
    check(y[5] == 'x', "a write through one mapping shows in the other");
    y[6] = 'y';
    check(x[PAGE + 6] == 'y', "and back");
    check_eq(write_at((uintptr_t)y + 5, 2), 2, "write() from the object's pages");
    check_eq(read_at((uintptr_t)x + 100, 4), 4, "read() into a page never touched");
    check(memcmp(x + 100, "0123", 4) == 0, "what read() put there");

    check_eq(map(0, PAGE, PROT_READ, MAP_PRIVATE, fd, 0), -EINVAL, "a private mapping of the object");
    check_eq(map(0, PAGE, PROT_READ, MAP_SHARED, fd, 1), -EINVAL, "an unaligned offset");
    long v = map(0, PAGE, PROT_READ, MAP_SHARED_VALIDATE, fd, (off_t)PAGE);
    check(v > 0 && ((char *)v)[5] == 'x', "MAP_SHARED_VALIDATE is MAP_SHARED");
    if (v > 0) {
        munmap((void *)v, PAGE);
    }
    check_eq(map(0, PAGE, PROT_READ, MAP_SHARED_VALIDATE | 0x80, fd, 0), -EOPNOTSUPP,
             "that refuses a flag Linux doesn't know");
    check_eq(map(0, 2 * PAGE, PROT_READ, MAP_SHARED, fd, (off_t)(1024 * MIB - PAGE)), -EOVERFLOW,
             "a mapping past the largest object");
    char buf[4];
    check_eq(read(fd, buf, 4), -EINVAL, "read() of the object");
    check_eq(write(fd, buf, 4), -EINVAL, "write() to it");
    check_eq(lseek(fd, 0, SEEK_SET), -ESPIPE, "lseek() on it");
    check_eq(ftruncate(fd, -1), -EINVAL, "ftruncate to a negative size");
    check_eq(ftruncate(fd, (off_t)(1024 * MIB + 1)), -EFBIG, "ftruncate past 1 GiB");
    check_eq(ftruncate(fds[1], 0), -EINVAL, "ftruncate of a pipe");
    check_eq(ftruncate(15, 0), -EBADF, "ftruncate of a closed descriptor");

    /* A shrink keeps the pages a mapping has touched, but they read zero
     * once the object grows back over them. */
    check_eq(ftruncate(fd, (off_t)PAGE), 0, "shrink to one page");
    check(y[5] == 'x', "a page a mapping touched stays mapped there");
    check_eq(ftruncate(fd, (off_t)(2 * PAGE)), 0, "grow back to two");
    check(y[5] == 0 && x[PAGE + 6] == 0, "the second page reads zero again");

    /* The object outlives its descriptor while it is mapped. */
    check_eq(close(fd), 0, "close the descriptor");
    x[7] = 'z';
    check(x[7] == 'z' && munmap(x, 2 * PAGE) == 0 && munmap(y, PAGE) == 0, "the mappings still work");

    int cloexec = memfd_create("cloexec", MFD_CLOEXEC | MFD_ALLOW_SEALING);
    check_eq(fcntl(cloexec, F_GETFD, 0), FD_CLOEXEC, "MFD_CLOEXEC");
    close(cloexec);
    check_eq(memfd_create("maptest", 4), -EINVAL, "a flag memfd_create doesn't know");
    check_eq(memfd_create((const char *)8, 0), -EFAULT, "a name that isn't user memory");
    char name[300];
    memset(name, 'n', sizeof name - 1);
    name[sizeof name - 1] = 0;
    check_eq(memfd_create(name, 0), -EINVAL, "a name longer than 249 bytes");
    name[249] = 0;
    int longest = memfd_create(name, 0);
    check(longest >= 0, "a name of 249 bytes");
    close(longest);
}

/* The rounds of shared_checks()' exchange: the parent stores an odd turn
 * in the object's first word, the child answers with the next, even one. */
#define TURNS 200

/* Waits for the word at `w` to be `turn`: spinning, so that on several harts
 * both processes run at once, and yielding now and then, so that on one the
 * other gets to run. */
static void await_turn(int *w, int turn) {
    for (int spins = 1; atomic_load(w) != turn; spins++) {
        if (spins % 256 == 0) {
            yield();
        }
    }
}

/* `maptest child`: the other end of shared_checks(), with the object at
 * descriptor 0. Waits for each of the parent's turns, checks the word it
 * wrote with it, and answers with its own. */
static int child(void) {
    int *w = (int *)map(0, PAGE, PROT_READ | PROT_WRITE, MAP_SHARED, 0, 0);
    if ((long)w < 0) {
        return 2;
    }
    for (int turn = 1; turn < TURNS; turn += 2) {
        await_turn(&w[0], turn);
        if (w[1] != 0x1234 + turn) {
            return 3;
        }
        w[2] = 0x5678 + turn;
        atomic_store(&w[0], turn + 1);
    }
    return 0;
}

/* Two processes see each other's writes to one object: this one, and a
 * child that maps it from its descriptor 0 (spawn passes descriptors 0 to
 * 2 on). On several harts they run at once. */
static void shared_checks(void) {
    int fd = memfd_create("shared", 0);
    check_eq(ftruncate(fd, (off_t)PAGE), 0, "an object of a page");
    int *w = (int *)map(0, PAGE, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
    check((long)w > 0, "mapped");
    if ((long)w <= 0) {
        return;
    }
    int console = dup(0);
    close(0);
    check_eq(dup(fd), 0, "the object at descriptor 0");
    int pid = spawn("/bin/maptest", (char *[]){"maptest", "child", NULL}, environ);
    close(0);
    dup(console);
    close(console);
    close(fd);
    check(pid > 0, "spawn the child");
    int seen = 0;
    for (int turn = 1; pid > 0 && turn < TURNS; turn += 2) {
        w[1] = 0x1234 + turn;
        atomic_store(&w[0], turn);
        await_turn(&w[0], turn + 1);
        seen += w[2] == 0x5678 + turn;
    }
    check_eq(seen, TURNS / 2, "the child's writes");
    int status = -1;
    check_eq(wait(pid, &status), pid, "the child ends");
    check_eq(status, 0, "having seen the parent's write");
    check_eq(munmap(w, PAGE), 0, "munmap the object, its last reference");
}

/* The framebuffer's VRAM: 640x480 pixels of 32 bits (spec section 9.3). */
#define FB_WIDTH 640
#define FB_HEIGHT 480
#define FB_SIZE (FB_WIDTH * FB_HEIGHT * 4ul)

/* The pixel maptest fb draws at (x, y): red the column, green the row. */
static uint fb_pixel(uint x, uint y) {
    return (x & 0xff) << 16 | (y & 0xff) << 8 | 0x5a;
}

/* `maptest fb`: draws into the framebuffer's memory through a mapping of
 * /dev/fb0 (a device mapping, the device's own pages), on a root image
 * that has the node. */
static void fb_checks(void) {
    int fd = open("/dev/fb0", O_RDWR);
    check(fd >= 0, "open /dev/fb0");
    if (fd < 0) {
        return;
    }
    long m = map(0, FB_SIZE, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
    check(m > 0, "map its memory");
    if (m <= 0) {
        close(fd);
        return;
    }
    uint *px = (uint *)m;
    for (uint y = 0; y < FB_HEIGHT; y++) {
        for (uint x = 0; x < FB_WIDTH; x++) {
            px[y * FB_WIDTH + x] = fb_pixel(x, y);
        }
    }
    check(px[FB_WIDTH * FB_HEIGHT - 1] == fb_pixel(FB_WIDTH - 1, FB_HEIGHT - 1), "the last pixel");
    long second = map(0, PAGE, PROT_READ, MAP_PRIVATE, fd, (off_t)PAGE);
    check(second > 0 && ((uint *)second)[0] == px[PAGE / 4],
          "a private mapping from its second page shows the same memory");
    /* The kernel copies only to and from RAM: a system call's buffer in a
     * device's memory is refused. */
    check_eq(write_at((uintptr_t)px + 8, 4), -EFAULT, "write() from the framebuffer");
    check_eq(map(0, FB_SIZE + PAGE, PROT_READ, MAP_SHARED, fd, 0), -EINVAL, "a mapping past its memory");
    check_eq(map(0, PAGE, PROT_READ, MAP_SHARED, fd, (off_t)FB_SIZE), -EINVAL, "an offset at its end");
    char buf[4];
    check_eq(read(fd, buf, 4), -EINVAL, "read() of the device");
    check_eq(munmap((void *)second, PAGE), 0, "munmap the second");
    check_eq(munmap(px, FB_SIZE), 0, "munmap the first");
    close(fd);

    int ro = open("/dev/fb0", O_RDONLY);
    check_eq(map(0, PAGE, PROT_READ | PROT_WRITE, MAP_SHARED, ro, 0), -EACCES,
             "a writable mapping of a descriptor open for reading");
    check_eq(map(0, PAGE, PROT_READ | PROT_WRITE, MAP_PRIVATE, ro, 0), -EACCES,
             "a private one too: the device's memory is the device's");
    long r = map(0, PAGE, PROT_READ, MAP_SHARED, ro, 0);
    check(r > 0 && ((uint *)r)[1] == fb_pixel(1, 0), "a read-only one");
    check_eq(mprotect((void *)r, PAGE, PROT_READ | PROT_WRITE), -EACCES, "made writable");
    check_eq(munmap((void *)r, PAGE), 0, "munmap it");
    close(ro);
}

/* `maptest threads`: the main thread and MAPPERS - 1 more each, ROUNDS
 * times, map AREA_PAGES pages, fill them, make them read-only and unmap
 * them, at once (on several harts, side by side). The addresses one thread
 * unmaps are the next mapping's, often another thread's, on another hart:
 * munmap must have shot the old translations down on every hart running
 * the process before the pages are reused. A hart that kept one would read
 * the old pages through the new mapping, or write into pages that have gone
 * to another: so a new mapping must read zero, and what a thread wrote must
 * stay until it unmaps it. Each round also adds one to a counter in a shared
 * object's page, mapped twice. */
#define MAPPERS 4
#define ROUNDS 40
#define AREA_PAGES 4
#define MAPPER_STACK 4096

static char mapper_stacks[MAPPERS][MAPPER_STACK];
/* Per thread, what went wrong: no mapping, a page that didn't read zero, a
 * mark that changed, munmap or mprotect failing. */
static int mapper_bad[MAPPERS];
static int *counter;

/* Whether every page of the area at `w` holds `mark`, first word and last. */
static int marked(uint *w, uint mark) {
    for (size_t i = 0; i < AREA_PAGES; i++) {
        uint *page = w + i * PAGE / 4;
        if (page[0] != mark || page[PAGE / 4 - 1] != mark) {
            return 0;
        }
    }
    return 1;
}

static int mapper(void *arg) {
    int me = (int)(uintptr_t)arg;
    for (int r = 0; r < ROUNDS; r++) {
        long m = anon(AREA_PAGES * PAGE);
        if (m <= 0) {
            mapper_bad[me]++;
            continue;
        }
        uint *w = (uint *)m;
        if (!marked(w, 0)) {
            mapper_bad[me]++;
        }
        uint mark = (uint)(me + 1) << 16 | (uint)(r + 1);
        for (size_t i = 0; i < AREA_PAGES; i++) {
            w[i * PAGE / 4] = mark;
            w[(i + 1) * PAGE / 4 - 1] = mark;
        }
        if (r % 8 == 0) {
            yield();
        }
        if (mprotect(w, AREA_PAGES * PAGE, PROT_READ) != 0 || !marked(w, mark)) {
            mapper_bad[me]++;
        }
        if (munmap(w, AREA_PAGES * PAGE) != 0) {
            mapper_bad[me]++;
        }
        atomic_add(counter, 1);
    }
    return me + 100;
}

static void threads_checks(void) {
    int fd = memfd_create("counter", 0);
    check_eq(ftruncate(fd, (off_t)PAGE), 0, "the counter's object");
    long a = map(0, PAGE, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
    long b = map(0, PAGE, PROT_READ, MAP_SHARED, fd, 0);
    close(fd);
    check(a > 0 && b > 0, "mapped twice");
    if (a <= 0 || b <= 0) {
        return;
    }
    counter = (int *)a;
    int tids[MAPPERS];
    for (int i = 1; i < MAPPERS; i++) {
        tids[i] = thread_start(mapper, (void *)(uintptr_t)i, mapper_stacks[i], MAPPER_STACK);
        check(tids[i] > 0, "start a thread");
    }
    mapper((void *)0);
    for (int i = 1; i < MAPPERS; i++) {
        int status = -1;
        check(tids[i] > 0 && thread_join(tids[i], &status) == 0 && status == i + 100, "join it");
    }
    for (int i = 0; i < MAPPERS; i++) {
        check_eq(mapper_bad[i], 0, "a thread's mappings");
    }
    check_eq(*(int *)b, MAPPERS * ROUNDS, "the counter, through the other mapping");
    check_eq(munmap((void *)a, PAGE), 0, "munmap the counter");
    check_eq(munmap((void *)b, PAGE), 0, "and its other mapping");
}

int main(int argc, char **argv) {
    if (argc == 2 && strcmp(argv[1], "child") == 0) {
        return child();
    }
    if (argc == 2 && strcmp(argv[1], "past") == 0) {
        /* A page of a mapping past the object's end. */
        int fd = memfd_create("past", 0);
        ftruncate(fd, 1);
        char *p = (char *)map(0, 2 * PAGE, PROT_READ, MAP_SHARED, fd, 0);
        return p[0] + p[PAGE];
    }
    if (argc == 2) {
        long m = anon(PAGE);
        char *p = (char *)m;
        if (strcmp(argv[1], "none") == 0) {
            mprotect(p, PAGE, PROT_NONE);
            return p[0];
        }
        if (strcmp(argv[1], "unmapped") == 0) {
            p[0] = 1;
            munmap(p, PAGE);
            return p[0];
        }
        if (strcmp(argv[1], "readonly") == 0) {
            mprotect(p, PAGE, PROT_READ);
            p[0] = 1;
            return 0;
        }
    }
    int fb = argc == 2 && strcmp(argv[1], "fb") == 0;
    int threads = argc == 2 && strcmp(argv[1], "threads") == 0;
    if (argc != 1 && !fb && !threads) {
        printf("usage: maptest [none | unmapped | readonly | past | fb | threads]\n");
        return 1;
    }
    if (pipe(fds) != 0) {
        printf("maptest: no pipe\n");
        return 1;
    }
    if (fb) {
        fb_checks();
    } else if (threads) {
        threads_checks();
    } else {
        anonymous_checks();
        error_checks();
        heap_checks();
        demand_checks();
        memfd_checks();
        shared_checks();
    }
    printf("maptest: %d checks, %d failed\n", checks, failed);
    return failed;
}
