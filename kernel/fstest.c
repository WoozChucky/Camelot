/* Mounting the root filesystem at boot, and its self-test (spec section 10,
 * "Kernel filesystem"). Disk requests sleep, so it all runs in a kernel
 * process, on disk 0:
 *
 * - mount: the superblock is checked, a committed transaction left in the
 *   log by a crash is replayed ("fs: replayed N blocks from the log"), and
 *   inodes a crash left with no links are freed; then one line with the
 *   sizes and what is free, and a listing of `/`;
 * - read checks, on any filesystem: the root is inode 1 and its own `.` and
 *   `..`; spellings of `/` (`//`, `/.`, `/..`, ...) all find it; every entry
 *   of `/` is found by its path; missing names, names that are too long and
 *   paths through a file fail with the right error; nameiparent() splits off
 *   the last name; the first file of `/` reads back whole in odd-sized
 *   pieces; and each file in `/bin` named after one of the kernel's
 *   embedded programs is byte for byte that program (`cargo xtask fs` puts
 *   them there), or, for the disk kernel, which embeds none, that /bin
 *   holds programs;
 * - with `fs_test` = FS_TEST_WRITE (patched in by the tests) and a writable
 *   disk, transactions: make /fstest and /fstest/a (2500 bytes) in one, then
 *   /fstest/b (700 bytes), remove a, make /fstest/x (a block), write
 *   /fstest/big (13 KiB + 100 bytes) 3 KiB per transaction, removing x
 *   while holding a reference to it (an inode with no links until the next
 *   transaction frees it) part way, after which big goes on growing next to
 *   its last block rather than in x's freed one; make and remove /fstest/d;
 *   have two kernel processes make /fstest/p1 and p2 at once (their
 *   operations share one transaction: group commit), then more processes
 *   make begin_op() wait for room in the log and for a commit; read
 *   everything back, remove big, check the errors, the free counts and the
 *   number of commits, and list /fstest. The tests stop the machine after
 *   every disk write of this and check that the disk, replayed, holds all
 *   of each transaction or none of it;
 * - with `fs_test` = FS_TEST_FULL, a file written until the disk is full,
 *   which first finds no block for the tree to grow a level;
 * - with `fs_test` = FS_TEST_TREE, the trees of /t1 and /t2, two files the
 *   tests made with their blocks interleaved: read, grown, and freed in
 *   several transactions;
 * - with `fs_test` = FS_TEST_GROW, two files written a block at a time in
 *   turn, so that their trees grow through every kind of split, each
 *   append logging what the format document counts for it. */
#include "fstest.h"
#include "blk.h"
#include "fs.h"
#include "kprintf.h"
#include "log.h"
#include "proc.h"
#include "string.h"

volatile u32 fs_test = FS_TEST_READ;

static int passed;
static int failed;
/* A filesystem was mounted and the checks ran. */
static bool tested;

static u8 data[4 * BSIZE];
static char path[64];

static void check(bool ok, const char *what) {
    if (ok) {
        passed++;
    } else {
        failed++;
        kprintf("fs self-test FAILED: %s\n", what);
    }
}

/* Inode `inum` as the disk (or the cache, if a transaction changed it)
 * has it, without locking it. */
static void dinode(u32 inum, struct dinode *out) {
    struct buf *b = fs_bread(fs_dev, IBLOCK(inum, fs_sb));
    memcpy(out, (const struct dinode *)b->data + inum % IPB, sizeof *out);
    brelse(b);
}

/* `prefix` + `name` in `path`. */
static const char *join(const char *prefix, const char *name) {
    ksnprintf(path, sizeof path, "%s%s", prefix, name);
    return path;
}

/* Calls `fn` for each used entry of the directory `dir`, with its name
 * (NUL-terminated) and inode. */
static void for_each_entry(const char *dir, void (*fn)(const char *name, u32 inum)) {
    int err;
    struct inode *dp = namei(dir, &err);
    if (dp == NULL) {
        return;
    }
    ilock(dp);
    struct dirent de;
    char name[DIRSIZ + 1];
    for (u32 off = 0; off < dp->size; off += sizeof de) {
        if (readi(dp, &de, off, sizeof de) != sizeof de) {
            break;
        }
        if (de.inum == 0) {
            continue;
        }
        memcpy(name, de.name, DIRSIZ);
        name[DIRSIZ] = 0;
        /* `fn` may look paths up, which locks directories: not this one. */
        iunlock(dp);
        fn(name, de.inum);
        ilock(dp);
    }
    iunlock(dp);
    iput(dp);
}

/* ---- Listing ---- */

static void ls_entry(const char *name, u32 inum) {
    struct dinode d;
    dinode(inum, &d);
    char type = d.type == T_DIR ? 'd' : d.type == T_FILE ? 'f' : d.type == T_DEVICE ? 'c' : '?';
    kprintf("%c %4u %2u %7u %s\n", type, inum, d.nlink, d.size, name);
}

/* Lists directory `dir`: type (d, f, c), inode, links, size, name. */
static void ls(const char *dir) {
    kprintf("fs: ls %s\n", dir);
    begin_op();
    for_each_entry(dir, ls_entry);
    end_op();
}

/* ---- Read checks ---- */

/* The root's entries other than `.` and `..` that namei() didn't find as
 * themselves, and the first regular file there. */
static u32 entries_wrong;
static char first_file[DIRSIZ + 1];

static void check_entry(const char *name, u32 inum) {
    if (namecmp(name, ".") == 0 || namecmp(name, "..") == 0) {
        return;
    }
    int err;
    struct inode *ip = namei(join("/", name), &err);
    if (ip == NULL || ip->inum != inum) {
        entries_wrong++;
    }
    if (ip != NULL) {
        iput(ip);
    }
    struct dinode d;
    dinode(inum, &d);
    if (d.type == T_FILE && first_file[0] == 0) {
        memcpy(first_file, name, DIRSIZ + 1);
    }
}

/* Reads all of file `name` in pieces of `piece` bytes; returns the bytes
 * read and whether a read at the end returned 0. */
static u32 read_in_pieces(struct inode *ip, u32 piece, bool *end_ok) {
    u32 total = 0;
    for (;;) {
        int n = readi(ip, data, total, piece);
        if (n <= 0) {
            break;
        }
        total += (u32)n;
    }
    *end_ok = readi(ip, data, ip->size, 1) == 0 && readi(ip, data, ip->size + 5, 1) == 0;
    return total;
}

/* Whether file `p` holds exactly `len` bytes equal to `want`. */
static bool holds(const char *p, const u8 *want, u32 len) {
    int err;
    struct inode *ip = namei(p, &err);
    if (ip == NULL) {
        return false;
    }
    ilock(ip);
    bool ok = ip->type == T_FILE && ip->size == len;
    for (u32 off = 0; ok && off < len; off += sizeof data) {
        u32 n = len - off < sizeof data ? len - off : sizeof data;
        ok = readi(ip, data, off, n) == (int)n && memcmp(data, want + off, n) == 0;
    }
    iunlockput(ip);
    return ok;
}

/* The regular files of /bin, counted by count_file(). */
static u32 bin_files;

static void count_file(const char *name, u32 inum) {
    (void)name;
    struct dinode d;
    dinode(inum, &d);
    if (d.type == T_FILE) {
        bin_files++;
    }
}

/* Each embedded program that /bin has. A kernel with none (the disk kernel,
 * kernel/kernel-disk.elf) runs every program from /bin: then the check is
 * that /bin has some. */
static void check_programs(void) {
    int err;
    struct inode *bin = namei("/bin", &err);
    if (bin == NULL) {
        return;
    }
    iput(bin);
    if (user_program_count == 0) {
        bin_files = 0;
        for_each_entry("/bin", count_file);
        kprintf("fs: /bin holds %u programs, the kernel none\n", bin_files);
        check(bin_files > 0, "/bin holds the programs");
        return;
    }
    u32 found = 0;
    u32 same = 0;
    for (u32 i = 0; i < user_program_count; i++) {
        const struct user_program *prog = &user_programs[i];
        struct inode *ip = namei(join("/bin/", prog->name), &err);
        if (ip == NULL) {
            continue;
        }
        iput(ip);
        found++;
        same += holds(join("/bin/", prog->name), prog->elf, prog->size);
    }
    kprintf("fs: /bin holds %u of the kernel's %u programs\n", found, user_program_count);
    check(same == found, "the programs in /bin are the kernel's");
}

static void read_checks(void) {
    int err;
    begin_op();
    struct inode *root = namei("/", &err);
    check(root != NULL && root->inum == ROOTINO, "/ is inode 1");
    if (root == NULL) {
        end_op();
        return;
    }
    ilock(root);
    check(root->type == T_DIR, "the root is a directory");
    struct inode *dot = dirlookup(root, ".", NULL);
    struct inode *dotdot = dirlookup(root, "..", NULL);
    check(dot == root && dotdot == root, "the root's . and .. are itself");
    iunlock(root);
    if (dot != NULL) {
        iput(dot);
    }
    if (dotdot != NULL) {
        iput(dotdot);
    }

    static const char *const roots[] = {"", "/", "//", "/.", "/..", "/./../", ".", "/../.."};
    bool all = true;
    for (u32 i = 0; i < sizeof roots / sizeof roots[0]; i++) {
        struct inode *ip = namei(roots[i], &err);
        all = all && ip == root;
        if (ip != NULL) {
            iput(ip);
        }
    }
    check(all, "every spelling of / finds the root");

    entries_wrong = 0;
    first_file[0] = 0;
    for_each_entry("/", check_entry);
    check(entries_wrong == 0, "every entry of / is found by its path");

    struct inode *ip = namei("/no such file", &err);
    check(ip == NULL && err == -ENOENT, "a missing name is not found");
    ip = namei("/abcdefghijklmno", &err);
    check(ip == NULL && err == -ENAMETOOLONG, "a name of 15 bytes is too long");
    char name[DIRSIZ + 1];
    ip = nameiparent("//new/", name, &err);
    check(ip == root && strcmp(name, "new") == 0, "nameiparent splits off the last name");
    if (ip != NULL) {
        iput(ip);
    }
    ip = nameiparent("/", name, &err);
    check(ip == NULL && err == -EINVAL, "/ has no parent");

    if (first_file[0] != 0) {
        char file[DIRSIZ + 2];
        ksnprintf(file, sizeof file, "/%s", first_file);
        ip = namei(join(file, "/x"), &err);
        check(ip == NULL && err == -ENOTDIR, "a path through a file fails");
        ip = namei(file, &err);
        if (ip != NULL) {
            ilock(ip);
            bool end_small;
            bool end_big;
            u32 small = read_in_pieces(ip, 333, &end_small);
            u32 big = read_in_pieces(ip, sizeof data, &end_big);
            check(small == ip->size && big == ip->size && end_small && end_big,
                  "a file reads back whole, and nothing past its end");
            iunlockput(ip);
        }
    }
    check_programs();
    iput(root);
    end_op();
}

/* ---- Transactions (FS_TEST_WRITE) ---- */

#define A_LEN 2500
#define B_LEN 700
#define X_LEN BSIZE
#define BIG_LEN (13 * BSIZE + 100)
#define CHUNK (3 * BSIZE)
#define BIG_CHUNKS ((BIG_LEN + CHUNK - 1) / CHUNK)
#define P_LEN 1500
/* /fstest, b, p1 and p2 (2 blocks each) are left; the files of the
 * begin_op() tests (r1, r2, j2, j3, c1, c2) are empty. */
#define BLOCKS_LEFT (1 + 1 + 2 + 2)
#define INODES_LEFT 10
/* T1 (/fstest and a), b, removing a and then freeing it (a file's blocks
 * are freed in transactions of their own, after the unlink's: itrunc),
 * making x, making big, its chunks, removing x and freeing it, making d,
 * removing d and freeing it, the two writers' operations together, the
 * begin_op() tests (2 + 1 + 2), removing big and freeing it. */
#define COMMITS (4 + 1 + 1 + BIG_CHUNKS + 2 + 3 + 1 + 5 + 2)

/* Where /fstest/x goes, part way through writing /fstest/big: at the
 * start of a chunk. */
#define BIG_SPLIT (12 * BSIZE)
_Static_assert(BIG_SPLIT % CHUNK == 0, "big's chunks");

/* The contents of the test files: byte i of the file with `seed` is
 * i * 7 + seed + i / 251 (so no two blocks are alike). */
static u8 pattern_byte(u32 i, u32 seed) {
    return (u8)(i * 7 + seed + i / 251);
}

static void fill(u8 *p, u32 off, u32 n, u32 seed) {
    for (u32 i = 0; i < n; i++) {
        p[i] = pattern_byte(off + i, seed);
    }
}

/* Whether file `p` holds `len` bytes of the pattern with `seed`. Its own
 * transaction (iput may write). */
static bool holds_pattern(const char *p, u32 len, u32 seed) {
    int err;
    begin_op();
    struct inode *ip = namei(p, &err);
    if (ip == NULL) {
        end_op();
        return false;
    }
    ilock(ip);
    bool ok = ip->type == T_FILE && ip->size == len;
    for (u32 off = 0; ok && off < len; off += sizeof data) {
        u32 n = len - off < sizeof data ? len - off : sizeof data;
        ok = readi(ip, data, off, n) == (int)n;
        for (u32 i = 0; ok && i < n; i++) {
            ok = data[i] == pattern_byte(off + i, seed);
        }
    }
    iunlockput(ip);
    end_op();
    return ok;
}

/* Makes file `p` holding `len` bytes of the pattern with `seed`, filled
 * in `buf`, in the caller's operation. */
static void make_file(const char *p, u32 len, u32 seed, u8 *buf) {
    int err;
    struct inode *ip = fs_create(p, T_FILE, 0, 0, &err);
    check(ip != NULL, "create a file");
    if (ip == NULL) {
        return;
    }
    fill(buf, 0, len, seed);
    check(writei(ip, buf, 0, len) == (int)len, "write a file");
    iunlockput(ip);
}

/* Writes `n` bytes of the pattern with `seed` at `off` of `ip` (unlocked,
 * with a reference), in a transaction of its own; returns what writei()
 * did. */
static int write_chunk(struct inode *ip, u32 off, u32 n, u32 seed) {
    begin_op();
    ilock(ip);
    fill(data, off, n, seed);
    int r = writei(ip, data, off, n);
    iunlock(ip);
    end_op();
    return r;
}

/* ---- Kernel processes at once ---- */

/* Starts a kernel process for each of `fns` and sleeps until each has
 * called proc_done(). */
static u32 procs_left;
static struct spinlock procs_left_lock;

static void proc_done(void) {
    acquire(&procs_left_lock);
    procs_left--;
    wakeup(&procs_left);
    release(&procs_left_lock);
}

static bool run_procs(void (*const *fns)(void), u32 n) {
    initlock(&procs_left_lock, "fstest");
    acquire(&procs_left_lock);
    procs_left = 0;
    for (u32 i = 0; i < n; i++) {
        const char *why;
        if (kproc_create("fstest", fns[i], &why) == NULL) {
            kprintf("fs self-test: cannot start a process: %s\n", why);
            break;
        }
        procs_left++;
    }
    while (procs_left > 0) {
        sleep(&procs_left, &procs_left_lock);
    }
    bool ok = procs_left == 0 && n > 0;
    release(&procs_left_lock);
    return ok;
}

/* How long a process waits for another to get somewhere: it yields up to
 * PATIENCE times, so a broken log makes a check fail instead of the test
 * hanging. The others need a few rounds at most. */
#define PATIENCE 1000

static bool yield_until(bool (*cond)(void)) {
    for (u32 i = 0; i < PATIENCE && !cond(); i++) {
        yield();
    }
    return cond();
}

/* Group commit: two kernel processes, each making a file in an operation of
 * its own. Each starts its operation and yields, so both are in progress
 * before either writes (once the transaction holds a block, begin_op()
 * would hold a second operation back until the commit: the log must have
 * room for MAXOPBLOCKS more per operation, as the tests below check). Then
 * the last to end commits both: one transaction. Each has its own buffer,
 * since they run at the same time. */
static u8 writer_data[2][P_LEN];

static void writer(u32 n) {
    begin_op();
    yield();
    make_file(n == 0 ? "/fstest/p1" : "/fstest/p2", P_LEN, 4 + n, writer_data[n]);
    end_op();
    proc_done();
}

static void writer1(void) {
    writer(0);
}

static void writer2(void) {
    writer(1);
}

/* begin_op()'s waits (log.c), each made to happen: the room tests, where
 * an operation must wait because those in progress could fill the log, and
 * the commit test, where one must wait for a commit sleeping on the disk.
 * The files are empty: making one changes /fstest's block, the inode block
 * and nothing else. */
static u32 room_waits0;
static bool r1_logged;
static bool j1_in;
static bool j2_in;
static bool j3_in;

static bool someone_waits_for_room(void) {
    return log_room_waits > room_waits0;
}

static bool r1_has_logged(void) {
    return r1_logged;
}

static bool j1_j2_in(void) {
    return j1_in && j2_in;
}

static bool j3_started(void) {
    return j3_in;
}

static bool committing(void) {
    return log_committing();
}

/* Room, 1: r1's operation has written blocks when r2 begins one. Both
 * reserving MAXOPBLOCKS could overflow the log (lh.n + 2 * 22 > 44), so r2
 * waits, r1 commits alone, then r2 runs: two transactions. */
static void room1(void) {
    begin_op();
    make_file("/fstest/r1", 0, 0, writer_data[0]);
    r1_logged = true;
    check(yield_until(someone_waits_for_room), "an operation waits while one that wrote is in progress");
    end_op();
    proc_done();
}

static void room2(void) {
    yield_until(r1_has_logged);
    begin_op();
    make_file("/fstest/r2", 0, 0, writer_data[1]);
    end_op();
    proc_done();
}

/* Room, 2: j1 and j2 are in operations, neither has written, and j3 must
 * wait (3 * 22 > 44). j1 ends without writing: its end_op() wakes j3, which
 * now fits beside j2 (2 * 22) and joins its transaction before j2 writes
 * anything. One commit for both files. */
static void join1(void) {
    begin_op();
    j1_in = true;
    check(yield_until(someone_waits_for_room), "a third operation waits for room");
    end_op();
    proc_done();
}

static void join2(void) {
    begin_op();
    j2_in = true;
    check(yield_until(j3_started), "an operation ending makes room for a waiting one");
    make_file("/fstest/j2", 0, 0, writer_data[0]);
    end_op();
    proc_done();
}

static void join3(void) {
    yield_until(j1_j2_in);
    begin_op();
    j3_in = true;
    make_file("/fstest/j3", 0, 0, writer_data[1]);
    end_op();
    proc_done();
}

/* Commit: c1's end_op() commits, sleeping on the disk; c2 begins an
 * operation meanwhile, and must wait for the commit to finish: its blocks
 * go in the next transaction. */
static void commit1(void) {
    begin_op();
    make_file("/fstest/c1", 0, 0, writer_data[0]);
    end_op();
    proc_done();
}

static void commit2(void) {
    check(yield_until(committing), "a commit sleeps on the disk");
    begin_op();
    make_file("/fstest/c2", 0, 0, writer_data[1]);
    end_op();
    proc_done();
}

static void admission_test(void) {
    static void (*const room[])(void) = {room1, room2};
    static void (*const join[])(void) = {join1, join2, join3};
    static void (*const commit[])(void) = {commit1, commit2};

    u32 commits = log_commits;
    room_waits0 = log_room_waits;
    bool ran = run_procs(room, 2);
    check(ran && log_commits == commits + 2, "a waiting operation goes in the next transaction");

    commits = log_commits;
    room_waits0 = log_room_waits;
    ran = run_procs(join, 3);
    check(ran && log_commits == commits + 1, "a woken operation shares the transaction");

    commits = log_commits;
    u32 commit_waits = log_commit_waits;
    ran = run_procs(commit, 2);
    check(ran && log_commit_waits > commit_waits && log_commits == commits + 2,
          "an operation waits for a commit under way");
}

/* The link count of the directory or file at `p`, in a transaction of its
 * own. */
static u32 nlink(const char *p) {
    int err;
    u32 n = 0;
    begin_op();
    struct inode *ip = namei(p, &err);
    if (ip != NULL) {
        ilock(ip);
        n = ip->nlink;
        iunlockput(ip);
    }
    end_op();
    return n;
}

static void write_test(void) {
    u32 blocks0;
    u32 inodes0;
    fs_count_free(&blocks0, &inodes0);
    u32 commits0 = log_commits;
    int err;
    struct dinode d;

    /* T1: a directory and a file in it. */
    begin_op();
    struct inode *ip = fs_create("/fstest", T_DIR, 0, 0, &err);
    check(ip != NULL, "make /fstest");
    if (ip != NULL) {
        iunlockput(ip);
    }
    make_file("/fstest/a", A_LEN, 1, data);
    end_op();

    begin_op();
    make_file("/fstest/b", B_LEN, 2, data);
    end_op();

    begin_op();
    check(fs_unlink("/fstest/a") == 0, "remove /fstest/a");
    end_op();

    /* x: one block, in the first one a's removal freed. It is removed
     * while big grows, which must go on next to its last block (the goal)
     * rather than take x's, the first free one. */
    begin_op();
    make_file("/fstest/x", X_LEN, 6, data);
    end_op();

    /* A file bigger than one transaction may write: made empty, then
     * written CHUNK bytes at a time, each in its own transaction (as the
     * write system call will). The reference outlives the transactions. */
    begin_op();
    struct inode *big = fs_create("/fstest/big", T_FILE, 0, 0, &err);
    check(big != NULL, "make /fstest/big");
    if (big != NULL) {
        iunlock(big);
    }
    end_op();
    bool wrote = big != NULL;
    u32 off;
    for (off = 0; wrote && off < BIG_SPLIT; off += CHUNK) {
        wrote = write_chunk(big, off, CHUNK, 3) == CHUNK;
    }

    /* x removed while still in use (as an open file can be): the
     * unlink commits its last link, and the inode stays allocated until the
     * last iput(), in the next transaction. A crash in between leaves it
     * for the next mount to free. */
    begin_op();
    struct inode *x = namei("/fstest/x", &err);
    check(x != NULL && fs_unlink("/fstest/x") == 0, "remove /fstest/x while it is in use");
    end_op();
    u32 x_block = 0;
    if (x != NULL) {
        u32 x_inum = x->inum;
        dinode(x_inum, &d);
        x_block = d.root.h.entries == 1 ? d.root.e[0].start_lo : 0;
        check(d.type == T_FILE && d.nlink == 0 && x_block != 0, "a file in use keeps its inode and blocks");
        begin_op();
        iput(x);
        end_op();
        dinode(x_inum, &d);
        check(d.type == T_FREE && d.root.h.entries == 0, "the last user frees it");
    }

    for (; wrote && off < BIG_LEN; off += CHUNK) {
        u32 n = BIG_LEN - off < CHUNK ? BIG_LEN - off : CHUNK;
        wrote = write_chunk(big, off, n, 3) == (int)n;
    }
    check(wrote, "write /fstest/big a piece per transaction");
    if (big != NULL) {
        /* Its first blocks went where a's were freed, around x; the rest
         * after b, and on from there: two extents, none in x's block. */
        dinode(big->inum, &d);
        bool apart = true;
        for (u32 i = 0; i < d.root.h.entries; i++) {
            apart = apart && (x_block < d.root.e[i].start_lo || x_block >= d.root.e[i].start_lo + d.root.e[i].len);
        }
        check(d.root.h.depth == 0 && d.root.h.entries == 2 && apart,
              "big grows next to its last block, not in x's freed one");
        begin_op();
        iput(big);
        end_op();
    }

    /* An empty directory, made and removed: /fstest's link count follows. */
    begin_op();
    ip = fs_create("/fstest/d", T_DIR, 0, 0, &err);
    check(ip != NULL, "make /fstest/d");
    if (ip != NULL) {
        iunlockput(ip);
    }
    end_op();
    check(nlink("/fstest") == 2 && nlink("/fstest/d") == 1, "a subdirectory links to its parent");
    begin_op();
    check(fs_unlink("/fstest/d") == 0, "remove the empty /fstest/d");
    end_op();
    check(nlink("/fstest") == 1, "and the link goes with it");

    /* Group commit. */
    static void (*const writers[])(void) = {writer1, writer2};
    u32 commits = log_commits;
    check(run_procs(writers, 2), "start two writers");
    check(log_commits == commits + 1, "two writers' operations commit together");

    admission_test();

    /* Read it all back. */
    check(holds_pattern("/fstest/b", B_LEN, 2), "/fstest/b reads back");
    check(holds_pattern("/fstest/big", BIG_LEN, 3), "/fstest/big reads back, from both its extents");
    check(holds_pattern("/fstest/p1", P_LEN, 4) && holds_pattern("/fstest/p2", P_LEN, 5),
          "the two writers' files read back");
    static const char *const empty[] = {"/fstest/r1", "/fstest/r2", "/fstest/j2",
                                        "/fstest/j3", "/fstest/c1", "/fstest/c2"};
    bool all = true;
    for (u32 i = 0; i < sizeof empty / sizeof empty[0]; i++) {
        all = all && holds_pattern(empty[i], 0, 0);
    }
    check(all, "the begin_op() tests' files are there");

    /* big goes, with every block of both its extents. */
    begin_op();
    check(fs_unlink("/fstest/big") == 0, "remove /fstest/big");
    end_op();

    /* Errors, which change nothing. */
    begin_op();
    ip = namei("/fstest/a", &err);
    check(ip == NULL && err == -ENOENT, "/fstest/a is gone");
    check(fs_create("/fstest/b", T_FILE, 0, 0, &err) == NULL && err == -EEXIST, "create: the name is taken");
    check(fs_create("/nowhere/x", T_FILE, 0, 0, &err) == NULL && err == -ENOENT, "create: no such directory");
    check(fs_create("/fstest/b/x", T_FILE, 0, 0, &err) == NULL && err == -ENOTDIR,
          "create: a file is not a directory");
    check(fs_create("/fstest/abcdefghijklmno", T_FILE, 0, 0, &err) == NULL && err == -ENAMETOOLONG,
          "create: a name too long");
    check(fs_unlink("/fstest") == -ENOTEMPTY, "unlink: a directory with entries");
    check(fs_unlink("/fstest/..") == -EINVAL && fs_unlink("/") == -EINVAL, "unlink: . and .. and /");
    check(fs_unlink("/fstest/a") == -ENOENT, "unlink: no such file");
    ip = namei("/fstest/b", &err);
    if (ip != NULL) {
        ilock(ip);
        check(writei(ip, data, B_LEN + 1, 1) == -EINVAL, "write: no holes");
        check(readi(ip, data, B_LEN, 1) == 0, "read: nothing at the end");
        check(ip->size == B_LEN, "and the size is unchanged");
        iunlockput(ip);
    }
    end_op();
    /* Files past what the kernel's offsets reach (on RV32, 2^31 - 1 bytes,
     * which only mkfs or an RV64 kernel could make; on RV64 the format's
     * limit): open() refuses them, and a write at the largest file's end
     * goes no further. An inode in memory only: its size is all writei()
     * looks at before it refuses. */
    struct inode big_one;
    memset(&big_one, 0, sizeof big_one);
    big_one.size = MAXFILE_BYTES;
    bool fits = !inode_too_big(&big_one);
    big_one.size = MAXFILE_BYTES + 1;
    bool past = inode_too_big(&big_one);
    big_one.size = (u64)1 << 32;
    bool high = inode_too_big(&big_one) == (MAXFILE_BYTES < ((u64)1 << 32));
    big_one.size = MAXFILE_BYTES - 1;
    bool efbig = writei(&big_one, data, MAXFILE_BYTES - 1, 2) == -EFBIG;
    check(fits && past && high && efbig, "a file past MAXFILE_BYTES is too big to open or write");

    u32 blocks;
    u32 inodes;
    fs_count_free(&blocks, &inodes);
    check(blocks == blocks0 - BLOCKS_LEFT && inodes == inodes0 - INODES_LEFT,
          "what was removed gave its blocks and inode back");
    check(log_commits - commits0 == COMMITS, "one commit per transaction that wrote");
    ls("/fstest");
}

/* ---- A full disk (FS_TEST_FULL) ---- */

/* The tests give it a disk with 13 free blocks. /full and /pad take a block
 * each in turn, 4 times, so /full is 4 extents (its root is full), and /pad
 * then takes all but one of the rest. Adding a block to /full needs that
 * block and a tree block for the root's entries to move to: there is none,
 * so the write fails with -ENOSPC and changes nothing, the block it took
 * given back. With /pad gone, /full fills the disk, its tree a level
 * deeper. */
static void full_test(void) {
    int err;
    begin_op();
    struct inode *full = fs_create("/full", T_FILE, 0, 0, &err);
    if (full != NULL) {
        iunlock(full);
    }
    struct inode *pad = fs_create("/pad", T_FILE, 0, 0, &err);
    if (pad != NULL) {
        iunlock(pad);
    }
    end_op();
    check(full != NULL && pad != NULL, "make /full and /pad");
    if (full == NULL || pad == NULL) {
        return;
    }
    u32 off = 0;
    u32 pad_off = 0;
    bool ok = true;
    for (u32 i = 0; i < EXT_ROOT_MAX; i++) {
        ok = ok && write_chunk(full, off, BSIZE, 7) == BSIZE;
        off += BSIZE;
        ok = ok && write_chunk(pad, pad_off, BSIZE, 8) == BSIZE;
        pad_off += BSIZE;
    }
    u32 blocks;
    u32 inodes;
    fs_count_free(&blocks, &inodes);
    while (ok && blocks > 1) {
        ok = write_chunk(pad, pad_off, BSIZE, 8) == BSIZE;
        pad_off += BSIZE;
        fs_count_free(&blocks, &inodes);
    }
    struct dinode before;
    dinode(full->inum, &before);
    check(ok && before.root.h.depth == 0 && before.root.h.entries == EXT_ROOT_MAX && blocks == 1,
          "/full is 4 extents, and one block is left");
    check(write_chunk(full, off, BSIZE, 7) == -ENOSPC, "a block that needs a new tree block fails with ENOSPC");
    struct dinode d;
    dinode(full->inum, &d);
    fs_count_free(&blocks, &inodes);
    check(memcmp(&d, &before, sizeof d) == 0 && blocks == 1, "and changes nothing: the block it took went back");
    begin_op();
    check(fs_unlink("/pad") == 0, "remove /pad");
    iput(pad);
    end_op();
    int r;
    while ((r = write_chunk(full, off, CHUNK, 7)) > 0) {
        off += (u32)r;
    }
    check(r == -ENOSPC, "writing to a full disk fails with ENOSPC");
    dinode(full->inum, &d);
    fs_count_free(&blocks, &inodes);
    check(d.size == off && d.root.h.depth == 1 && blocks == 0, "/full grew a level, and no block is lost");
    begin_op();
    iput(full);
    end_op();
    check(holds_pattern("/full", off, 7), "/full reads back");
    kprintf("fs: the disk filled up at %u bytes\n", off);
}

/* ---- Extent trees (FS_TEST_TREE) ---- */

/* The tests give it a disk holding /t1 and /t2, T_LEN bytes each of the
 * pattern with seeds 11 and 12, their blocks interleaved (a block of t1, a
 * block of t2, ...), made by the host's mkfs code: every block an extent of
 * its own, so each tree is 2 deep with a dozen leaves. Their blocks read
 * back; a block added to /t1 reads back too; removing /t1 frees its blocks
 * in more than one transaction (a leaf emptied takes a log slot); and
 * emptying /t2 as open's O_TRUNC does, under its write lock, leaves it
 * empty. */
#define T_LEN (1200 * BSIZE)

static void tree_test(void) {
    u32 blocks0;
    u32 inodes0;
    fs_count_free(&blocks0, &inodes0);
    check(holds_pattern("/t1", T_LEN, 11) && holds_pattern("/t2", T_LEN, 12), "the host's trees read back");
    int err;
    begin_op();
    struct inode *t1 = namei("/t1", &err);
    struct inode *t2 = namei("/t2", &err);
    end_op();
    if (t1 == NULL || t2 == NULL) {
        check(false, "find /t1 and /t2");
        return;
    }
    struct dinode d1;
    struct dinode d2;
    dinode(t1->inum, &d1);
    dinode(t2->inum, &d2);
    check(d1.root.h.depth == 2 && d2.root.h.depth == 2, "both trees are 2 deep");
    check(write_chunk(t1, T_LEN, BSIZE, 11) == BSIZE && holds_pattern("/t1", T_LEN + BSIZE, 11),
          "a block added to a deep tree reads back");
    u32 commits = log_commits;
    begin_op();
    bool removed = fs_unlink("/t1") == 0;
    iput(t1);
    end_op();
    check(removed && log_commits - commits >= 2, "a big file's blocks are freed in several transactions");
    acquiresleep(&t2->wlock);
    begin_op();
    ilock(t2);
    itrunc(t2);
    bool empty = t2->size == 0 && t2->flags == 0 && t2->root.h.depth == 0 && t2->root.h.entries == 0;
    iunlock(t2);
    end_op();
    releasesleep(&t2->wlock);
    begin_op();
    iput(t2);
    end_op();
    check(empty && holds_pattern("/t2", 0, 12), "O_TRUNC's truncation empties /t2");
    u32 blocks;
    u32 inodes;
    fs_count_free(&blocks, &inodes);
    check(blocks > blocks0 + 2 * (T_LEN / BSIZE) && inodes == inodes0 + 1, "every block came back");
}

/* ---- Growing trees (FS_TEST_GROW) ---- */

/* The tests give it a disk of one bitmap block with a README. /g1 and /g2
 * take a block each in turn, a transaction per block, until each holds
 * G_BLOCKS: every block an extent of its own, so each tree grows every way
 * there is (docs/camelot-fs.md, "Growing a file"). At g1's 5th extent the
 * root's 4 move to a new leaf, the tree a level deeper; at its 85th that
 * leaf is full and a new one goes under the root; at its 337th the 4 leaves
 * and the root are full, and the tree grows to depth 2: the root's 4 index
 * entries move to a new index block, which takes a new leaf too. Each
 * append logs what the document counts: the data block, the bitmap block
 * (the only one), the new tree blocks and the inode, whose block holds the
 * root: 4, 4 and 5 blocks. Before g1's 337th, /pad fills the disk to 2 free
 * blocks, one fewer than that append needs: it takes the data block and the
 * old root's copy, finds none for the new leaf, gives both back and fails
 * with -ENOSPC, the tree as it was. */
#define G_BLOCKS 340
#define G_SPLIT 84      /* g1's 85th extent: the first leaf is full */
#define G_DEEPER 336    /* its 337th: every leaf and the root are full */
#define APPEND_WORST 14 /* docs/camelot-fs.md, "Limits on transactions" */

/* Adds block `k` (the pattern with `seed`) to `ip` in a transaction of its
 * own; *logged is how many blocks it logged. */
static int append_logged(struct inode *ip, u32 k, u32 seed, u32 *logged) {
    begin_op();
    ilock(ip);
    fill(data, k * BSIZE, BSIZE, seed);
    u32 before = log_blocks();
    int r = writei(ip, data, k * BSIZE, BSIZE);
    *logged = log_blocks() - before;
    iunlock(ip);
    end_op();
    return r;
}

static void grow_test(void) {
    int err;
    struct inode *g[2];
    begin_op();
    g[0] = fs_create("/g1", T_FILE, 0, 0, &err);
    if (g[0] != NULL) {
        iunlock(g[0]);
    }
    g[1] = fs_create("/g2", T_FILE, 0, 0, &err);
    if (g[1] != NULL) {
        iunlock(g[1]);
    }
    struct inode *pad = fs_create("/pad", T_FILE, 0, 0, &err);
    if (pad != NULL) {
        iunlock(pad);
    }
    end_op();
    check(g[0] != NULL && g[1] != NULL && pad != NULL, "make /g1, /g2 and /pad");
    if (g[0] == NULL || g[1] == NULL || pad == NULL) {
        return;
    }
    bool ok = true;
    u32 worst = 0;
    struct dinode d;
    for (u32 k = 0; ok && k < G_BLOCKS; k++) {
        for (u32 f = 0; ok && f < 2; f++) {
            if (f == 0 && k == G_DEEPER) {
                u32 blocks;
                u32 inodes;
                fs_count_free(&blocks, &inodes);
                u32 pad_off = 0;
                while (ok && blocks > 2) {
                    ok = write_chunk(pad, pad_off, BSIZE, 8) == BSIZE;
                    pad_off += BSIZE;
                    fs_count_free(&blocks, &inodes);
                }
                struct dinode before;
                dinode(g[0]->inum, &before);
                u32 logged;
                int r = append_logged(g[0], k, 21, &logged);
                dinode(g[0]->inum, &d);
                fs_count_free(&blocks, &inodes);
                check(ok && r == -ENOSPC && memcmp(&d, &before, sizeof d) == 0 && blocks == 2,
                      "with 2 blocks free, the 337th extent fails with ENOSPC and changes nothing");
                begin_op();
                ok = fs_unlink("/pad") == 0;
                iput(pad);
                end_op();
            }
            u32 logged = 0;
            ok = ok && append_logged(g[f], k, 21 + f, &logged) == BSIZE;
            worst = logged > worst ? logged : worst;
            if (f != 0 || !ok) {
                continue;
            }
            dinode(g[0]->inum, &d);
            if (k == EXT_ROOT_MAX) {
                check(d.root.h.depth == 1 && d.root.h.entries == 1 && logged == 4,
                      "the 5th extent grows the tree to depth 1: data, bitmap, a tree block, the inode");
            } else if (k == G_SPLIT) {
                check(d.root.h.depth == 1 && d.root.h.entries == 2 && logged == 4,
                      "the 85th splits the full leaf: a new leaf under the root");
            } else if (k == G_DEEPER) {
                check(d.root.h.depth == 2 && d.root.h.entries == 1 && logged == 5,
                      "the 337th grows the tree to depth 2: the old root's copy and a new leaf");
            }
        }
    }
    check(ok && worst <= APPEND_WORST, "every append fits the document's worst case");
    begin_op();
    iput(g[0]);
    iput(g[1]);
    end_op();
    check(holds_pattern("/g1", G_BLOCKS * BSIZE, 21) && holds_pattern("/g2", G_BLOCKS * BSIZE, 22),
          "/g1 and /g2 read back");
}

/* ---- Boot ---- */

static void fs_main(void) {
    const char *why = "";
    int err = fs_mount(0, &why);
    if (err == -ENOENT) {
        kprintf("fs: no filesystem on disk 0\n");
        return;
    }
    if (err < 0) {
        kprintf("fs: disk 0 not mounted: %s\n", why);
        return;
    }
    if (log_recovered > 0) {
        kprintf("fs: replayed %u blocks from the log\n", log_recovered);
    }
    if (fs_truncations > 0) {
        kprintf("fs: finished %u truncations\n", fs_truncations);
    }
    if (fs_orphans > 0) {
        kprintf("fs: freed %u inodes that no directory named\n", fs_orphans);
    }
    u32 blocks;
    u32 inodes;
    fs_count_free(&blocks, &inodes);
    kprintf("fs: disk 0 mounted: %u blocks (%u free), %u inodes (%u free), %u log blocks%s\n", fs_sb.size,
            blocks, fs_sb.ninodes, inodes, fs_sb.nlog,
            fs_log_too_small ? ", read-only: the log is too small to write" : fs_readonly ? ", read-only" : "");
    ls("/");
    tested = true;
    read_checks();
    if (fs_test == FS_TEST_WRITE || fs_test == FS_TEST_FULL || fs_test == FS_TEST_TREE ||
        fs_test == FS_TEST_GROW) {
        if (fs_readonly) {
            kprintf("fs: write test skipped: the %s\n",
                    fs_log_too_small ? "log is too small to write" : "disk is read-only");
        } else if (fs_test == FS_TEST_WRITE) {
            write_test();
        } else if (fs_test == FS_TEST_FULL) {
            full_test();
        } else if (fs_test == FS_TEST_TREE) {
            tree_test();
        } else {
            grow_test();
        }
    }
}

int fs_boot(void) {
    if (blk_count() == 0) {
        return 0;
    }
    const char *err;
    if (kproc_create("fs", fs_main, &err) == NULL) {
        kprintf("fs: cannot start: %s\n", err);
        return 1;
    }
    scheduler();
    if (!tested) {
        return 0;
    }
    kprintf("fs self-test: %d/%d passed\n", passed, passed + failed);
    return failed;
}
