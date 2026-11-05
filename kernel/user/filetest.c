/* filetest: checks the file system calls do what docs/camelot-syscalls.md
 * says, on a writable root filesystem: open's flags, reading, writing and
 * seeking, shared and separate offsets, directories, links, removing files
 * still open and directories still in use, working directories, running
 * programs from files, and every error a bad path or a bad descriptor
 * gives. It works in /ft, which it makes and removes again. Prints a line
 * for each check that fails and, at the end, "filetest: N checks, F
 * failed"; exits with F.
 *
 * Other modes:
 *   filetest ops      a fixed sequence of the calls that change the disk,
 *                     for the crash tests (see ops() below); prints
 *                     "filetest: ops done" and exits 0, or says what failed
 *                     and exits 1
 *   filetest ro       on a read-only root: every call that would write
 *                     fails with -EROFS and reading still works; prints
 *                     "filetest: N checks, F failed" too
 *   filetest incwd    a child: exits 0 if its working directory holds a
 *                     file `c` (its parent's working directory: inherited)
 *   filetest hold T   a child: holds files open (see hold()) */
#include "user.h"

/* Descriptors per process (NOFILE in kernel/include/file.h). */
#define NOFILE 16
/* A big file: past the largest of the filesystem's version 1 (268 KiB),
 * which version 2's extent trees have no limit like. */
#define BIG_FILE_SIZE (300 * 1024)
/* The console's device number (CONSOLE in kernel/include/file.h). */
#define CONSOLE 1

static int checks;
static int failures;

static void check(int ok, const char *what) {
    checks++;
    if (!ok) {
        failures++;
        printf("filetest: FAILED: %s\n", what);
    }
}

static void check_eq(long got, long want, const char *what) {
    checks++;
    if (got != want) {
        failures++;
        printf("filetest: FAILED: %s: got %ld, want %ld\n", what, got, want);
    }
}

/* Byte i of a test file with `seed` (as kernel/fstest.c's, so the host can
 * check one too). */
static char pattern(uint i, uint seed) {
    return (char)(i * 7 + seed + i / 251);
}

static char buf[4096];

/* Writes `n` bytes of the pattern with `seed` to `fd`, a buffer at a time;
 * returns what the last write returned, or n. */
static int write_pattern(int fd, uint n, uint seed) {
    for (uint done = 0; done < n;) {
        uint m = n - done < sizeof buf ? n - done : sizeof buf;
        for (uint i = 0; i < m; i++) {
            buf[i] = pattern(done + i, seed);
        }
        int r = write(fd, buf, m);
        if (r != (int)m) {
            return r;
        }
        done += m;
    }
    return (int)n;
}

/* Whether `fd` holds, from its offset on, exactly `n` bytes of the pattern
 * with `seed`, read in pieces of `piece` bytes. */
static int holds_pattern(int fd, uint n, uint seed, uint piece) {
    uint total = 0;
    int r;
    while ((r = read(fd, buf, piece)) > 0) {
        for (int i = 0; i < r; i++) {
            if (buf[i] != pattern(total + (uint)i, seed)) {
                return 0;
            }
        }
        total += (uint)r;
    }
    return r == 0 && total == n;
}

/* The size of the file at `path`, or the error. */
static int size_of(const char *path) {
    struct stat st;
    int r = stat(path, &st);
    return r < 0 ? r : (int)st.size;
}

/* The link count of the file at `path`, or the error. */
static int links_of(const char *path) {
    struct stat st;
    int r = stat(path, &st);
    return r < 0 ? r : (int)st.nlink;
}

/* Waits for child `pid`; returns its status, or -1000 if it isn't one. */
static int status_of(int pid) {
    int status;
    return pid > 0 && wait(pid, &status) == pid ? status : -1000;
}

static void test_open_errors(void) {
    check_eq(open("nofile", O_RDONLY), -ENOENT, "open a missing file");
    check_eq(open("/ft/nodir/x", O_WRONLY | O_CREAT), -ENOENT, "create in a missing directory");
    check_eq(open("", O_RDONLY), -ENOENT, "open the empty path");
    check_eq(open("abcdefghijklmno", O_RDONLY), -ENAMETOOLONG, "a name of 15 bytes");
    check_eq(open("abcdefghijklmno", O_WRONLY | O_CREAT), -ENAMETOOLONG, "create a name of 15 bytes");
    char path[MAXPATH + 2];
    memset(path, 'a', sizeof path - 1);
    path[sizeof path - 1] = 0;
    check_eq(open(path, O_RDONLY), -ENAMETOOLONG, "a path over MAXPATH bytes");
    for (int i = 1; i < MAXPATH; i += 2) {
        path[i] = '/';
    }
    path[MAXPATH] = 0;
    check_eq(open(path, O_RDONLY), -ENOENT, "a path of MAXPATH bytes is looked up");
    check_eq(open("/ft", O_WRONLY), -EISDIR, "a directory for writing");
    check_eq(open("/ft", O_RDWR | O_CREAT), -EISDIR, "a directory for reading and writing");
    check_eq(open(".", O_RDWR | O_TRUNC), -EISDIR, "a directory to empty");
    check_eq(open("newf/", O_WRONLY | O_CREAT), -EISDIR, "create a name ending in /: a directory's");
    check_eq(unlink("newf"), -ENOENT, "which makes nothing");
}

static void test_read_write_seek(void) {
    struct stat st;
    int fd = open("a", O_RDWR | O_CREAT);
    check_eq(fd, 3, "create a file: the lowest free descriptor");
    check_eq(open("a/x", O_RDONLY), -ENOTDIR, "a path through a file");
    check_eq(mkdir("a/x"), -ENOTDIR, "mkdir through a file");
    /* More than a transaction takes: several of them. */
    check_eq(write_pattern(fd, 5000, 1), 5000, "write 5000 bytes");
    check_eq(fstat(fd, &st), 0, "fstat on a file");
    check(st.type == T_FILE && st.size == 5000 && st.nlink == 1 && st.ino > 1, "a file of 5000 bytes");
    check_eq(lseek(fd, 0, SEEK_CUR), 5000, "the offset is past the bytes written");
    check_eq(read(fd, buf, 10), 0, "nothing to read at the end");
    check_eq(lseek(fd, 0, SEEK_SET), 0, "back to the start");
    check(holds_pattern(fd, 5000, 1, 333), "read it back in pieces");
    check_eq(lseek(fd, 1, SEEK_END), -EINVAL, "no seeking past the end");
    check_eq(lseek(fd, -1, SEEK_SET), -EINVAL, "no seeking before the start");
    check_eq(lseek(fd, 0, 3), -EINVAL, "an unknown whence");
    check_eq(lseek(fd, 0x7fffffff, SEEK_CUR), -EINVAL, "an offset that would overflow");
    check_eq(lseek(fd, -10, SEEK_END), 4990, "seek from the end");
    check_eq(read(fd, buf, 20), 10, "read the last 10 bytes");
#ifdef _LP64
    /* RV64: an off_t is 64 bits, all of them the offset. Past the end is
     * past the largest file too (2^32 blocks of 1 KiB, 2^42 bytes), and
     * fails the same way, never as the offset's low 32 bits would: 2^32 +
     * 100 is not 100. */
    check(sizeof(off_t) == 8 && sizeof(st.size) == 8 && sizeof(struct stat) == 24, "a 64-bit off_t");
    check_eq(lseek(fd, 0x100000000l + 100, SEEK_SET), -EINVAL, "2^32 + 100 is past the end, not 100");
    check_eq(lseek(fd, 0x40000000000l + 1, SEEK_SET), -EINVAL, "past the largest file");
    check_eq(lseek(fd, LONG_MAX, SEEK_SET), -EINVAL, "LONG_MAX");
    check_eq(lseek(fd, LONG_MAX, SEEK_END), -EINVAL, "LONG_MAX from the end");
    check_eq(lseek(fd, LONG_MIN, SEEK_END), -EINVAL, "LONG_MIN from the end");
    check_eq(lseek(fd, -0x100000000l + 4990, SEEK_END), -EINVAL, "2^32 - 10 bytes before the start");
    check_eq(lseek(fd, 0, SEEK_CUR), 5000, "and the offset is where it was");
#endif
    check_eq(lseek(fd, 100, SEEK_SET), 100, "seek into the middle");
    check_eq(write(fd, "XYZ", 3), 3, "write in the middle");
    check_eq(lseek(fd, -3, SEEK_CUR), 100, "seek back over it");
    check_eq(read(fd, buf, 4), 4, "read it");
    check(memcmp(buf, "XYZ", 3) == 0 && buf[3] == pattern(103, 1), "the middle changed, nothing else");
    check_eq(size_of("a"), 5000, "the size didn't");
    check_eq(close(fd), 0, "close it");
    check_eq(read(fd, buf, 1), -EBADF, "a closed descriptor");

    /* Access modes. */
    fd = open("a", O_RDONLY);
    check_eq(write(fd, "x", 1), -EBADF, "no writing a file opened for reading");
    close(fd);
    fd = open("a", O_WRONLY);
    check_eq(read(fd, buf, 1), -EBADF, "no reading a file opened for writing");
    check_eq(fcntl(fd, F_GETFD, 0), 0, "a file opened without O_CLOEXEC stays open on exec");
    close(fd);
    /* O_CLOEXEC: the descriptor closes on exec (systest checks exec closes
     * it), and the file opens as ever. */
    fd = open("a", O_RDONLY | O_CLOEXEC);
    check_eq(fcntl(fd, F_GETFD, 0), FD_CLOEXEC, "O_CLOEXEC sets FD_CLOEXEC");
    check_eq(read(fd, buf, 1), 1, "and the file reads");
    close(fd);
    /* O_CREAT on a file that exists opens it; O_TRUNC empties it, if it is
     * opened for writing. */
    fd = open("a", O_WRONLY | O_CREAT);
    check(fd >= 0 && size_of("a") == 5000, "O_CREAT opens a file that exists, as it is");
    close(fd);
    fd = open("a", O_RDONLY | O_TRUNC);
    check(fd >= 0 && size_of("a") == 5000, "O_TRUNC does nothing for reading");
    close(fd);
    fd = open("a", O_RDWR | O_TRUNC);
    check(fd >= 0 && size_of("a") == 0, "O_TRUNC empties a file");
    check_eq(read(fd, buf, 1), 0, "an empty file");

    /* dup shares the offset; open again doesn't. */
    int fd2 = dup(fd);
    check_eq(write(fd, "ab", 2), 2, "write through one descriptor");
    check_eq(write(fd2, "cd", 2), 2, "then through its dup");
    close(fd2);
    int r1 = open("a", O_RDONLY);
    int r2 = open("a", O_RDONLY);
    char c1 = 0;
    char c2 = 0;
    read(r1, &c1, 1);
    read(r2, &c2, 1);
    check(c1 == 'a' && c2 == 'a', "two opens, two offsets");
    check_eq(read(r1, buf, 10), 3, "a dup shares the offset: abcd");
    check(memcmp(buf, "bcd", 3) == 0, "abcd it is");
    close(r1);
    close(r2);
    close(fd);

    /* Every descriptor taken. */
    int n = 0;
    while (open("a", O_RDONLY) >= 0) {
        n++;
    }
    check_eq(n, NOFILE - 3, "files until the table is full");
    check_eq(open("a", O_RDONLY), -EMFILE, "open with every descriptor taken");
    check_eq(open("new", O_WRONLY | O_CREAT), -EMFILE, "nor creating one");
    for (int i = 3; i < NOFILE; i++) {
        close(i);
    }
    check_eq(size_of("new"), -ENOENT, "and that one wasn't made");
}

/* Whether directory `path` has an entry `name` for inode `inum` (any
 * inode if 0), and its first two entries are `.` and `..`. */
static int dir_has(const char *path, const char *name, uint inum) {
    int fd = open(path, O_RDONLY);
    if (fd < 0) {
        return 0;
    }
    struct dirent de;
    int found = 0;
    int i = 0;
    int dots = 1;
    while (read(fd, &de, sizeof de) == sizeof de) {
        if (i == 0) {
            dots &= strcmp(de.name, ".") == 0;
        } else if (i == 1) {
            dots &= strcmp(de.name, "..") == 0;
        }
        i++;
        char n[DIRSIZ + 1];
        memcpy(n, de.name, DIRSIZ);
        n[DIRSIZ] = 0;
        if (de.inum != 0 && strcmp(n, name) == 0 && (inum == 0 || de.inum == inum)) {
            found = 1;
        }
    }
    close(fd);
    return found && dots;
}

static void test_directories(void) {
    struct stat st;
    check_eq(mkdir("d"), 0, "mkdir");
    check_eq(mkdir("d"), -EEXIST, "mkdir of a name taken");
    check_eq(mkdir("a"), -EEXIST, "mkdir of a file's name");
    check_eq(mkdir("/"), -EEXIST, "mkdir /");
    check_eq(mkdir("/ft/nodir/d"), -ENOENT, "mkdir in a missing directory");
    check_eq(mkdir("abcdefghijklmnop"), -ENAMETOOLONG, "mkdir of a name too long");
    check_eq(stat("d", &st), 0, "stat a directory");
    check(st.type == T_DIR && st.nlink == 1 && st.size == 32, "an empty directory: . and ..");
    check_eq(links_of("."), 2, "its parent gains a link, for its ..");
    check(dir_has(".", "d", st.ino), "reading a directory gives its entries");
    check(dir_has("d", "..", 0), "and a new one has . and ..");
    int fd = open("d", O_RDONLY);
    check_eq(read(fd, buf, 7), 7, "a directory reads like a file");
    check_eq(lseek(fd, 0, SEEK_END), 32, "and seeks like one");
    close(fd);

    /* Links. */
    check_eq(link("a", "d/b"), 0, "link");
    check_eq(links_of("a"), 2, "two names");
    check_eq(links_of("d/b"), 2, "for one file");
    struct stat sa;
    stat("a", &sa);
    stat("d/b", &st);
    check_eq((int)st.ino, (int)sa.ino, "the same inode");
    check_eq(link("a", "d/b"), -EEXIST, "link to a name taken");
    check_eq(link("d", "e"), -EPERM, "no links to a directory");
    check_eq(link("nofile", "x"), -ENOENT, "link a missing file");
    check_eq(link("a", "nodir/x"), -ENOENT, "link into a missing directory");
    check_eq(link("a", "a/x"), -ENOTDIR, "link through a file");
    check_eq(link("a", "abcdefghijklmnop"), -ENAMETOOLONG, "link to a name too long");
    check_eq(link("a", "/"), -EEXIST, "link to /");
    check_eq(links_of("a"), 2, "failed links change nothing");

    /* Removing. */
    check_eq(unlink("d"), -ENOTEMPTY, "unlink a directory with entries");
    check_eq(unlink("."), -EINVAL, "unlink .");
    check_eq(unlink("d/.."), -EINVAL, "unlink ..");
    check_eq(unlink("/"), -EINVAL, "unlink /");
    check_eq(unlink("nofile"), -ENOENT, "unlink a missing file");
    check_eq(unlink("a/x"), -ENOTDIR, "unlink through a file");
    check_eq(unlink("abcdefghijklmnop"), -ENAMETOOLONG, "unlink a name too long");
    check_eq(unlink("d/b"), 0, "unlink a link");
    check_eq(links_of("a"), 1, "one name left");

    /* A file removed while open: it stays until it is closed. */
    fd = open("a", O_RDWR);
    check_eq(unlink("a"), 0, "unlink an open file");
    check_eq(size_of("a"), -ENOENT, "its name is gone");
    check_eq(fstat(fd, &st), 0, "fstat it");
    check(st.nlink == 0 && st.size == 4, "it has no links, and its bytes");
    check_eq(read(fd, buf, 10), 4, "read it");
    check_eq(write(fd, "efgh", 4), 4, "write it");
    check_eq(close(fd), 0, "close it: it goes");
}

static void test_working_directory(void) {
    struct stat st;
    check_eq(chdir("d"), 0, "chdir");
    int fd = open("c", O_WRONLY | O_CREAT);
    check(fd >= 0, "create a file there");
    close(fd);
    check_eq(stat("/ft/d/c", &st), 0, "it is in that directory");
    int pid = spawn("/bin/filetest", (char *[]){"filetest", "incwd", NULL}, environ);
    check_eq(status_of(pid), 0, "a child starts in its parent's working directory");
    check_eq(chdir(".."), 0, "chdir ..");
    check_eq(stat("d/c", &st), 0, "back in /ft");
    check_eq(chdir("nofile"), -ENOENT, "chdir to a missing directory");
    check_eq(chdir("d/c"), -ENOTDIR, "chdir to a file");
    check_eq(chdir("/ft/d/c/x"), -ENOTDIR, "chdir through a file");
    check_eq(chdir(""), -ENOENT, "chdir to the empty path");
    check_eq(unlink("d/c"), 0, "remove the file");
    check_eq(unlink("d"), 0, "remove the empty directory");
    check_eq(links_of("."), 1, "and the parent's link for its ..");

    /* A working directory removed while in use: nothing more goes in. */
    check_eq(mkdir("gone"), 0, "mkdir gone");
    check_eq(chdir("gone"), 0, "chdir gone");
    check_eq(unlink("/ft/gone"), 0, "remove it while it is the working directory");
    check_eq(open("x", O_WRONLY | O_CREAT), -ENOENT, "no file can be made in it");
    check_eq(mkdir("y"), -ENOENT, "nor a directory");
    check_eq(link("/bin/sh", "z"), -ENOENT, "nor a link");
    check_eq(stat(".", &st), 0, "it can still be looked at");
    check(st.nlink == 0 && st.type == T_DIR, "with no links");
    check_eq(chdir("/ft"), 0, "chdir away: it goes");

    /* Its .. leads nowhere, even once the parent is gone too: the parent's
     * inode is freed (and may be reused), so following it would panic or
     * land in some other directory. */
    check_eq(mkdir("pa"), 0, "mkdir pa");
    check_eq(mkdir("pa/b"), 0, "mkdir pa/b");
    check_eq(chdir("pa/b"), 0, "chdir pa/b");
    check_eq(unlink("/ft/pa/b"), 0, "remove the working directory");
    check_eq(chdir(".."), -ENOENT, "its .. is not found while the parent is there");
    check_eq(unlink("/ft/pa"), 0, "remove its parent: it goes");
    check_eq(chdir(".."), -ENOENT, "chdir .. after that");
    check_eq(open("..", O_RDONLY), -ENOENT, "open .. after that");
    check_eq(mkdir("/ft/pc"), 0, "a new directory, maybe in the parent's old inode");
    check_eq(chdir("../pc"), -ENOENT, "still not reached through ..");
    check_eq(stat(".", &st), 0, "the working directory is unchanged");
    check(st.nlink == 0 && st.type == T_DIR, "still the removed one");
    check_eq(chdir("/ft"), 0, "chdir away");
    check_eq(unlink("pc"), 0, "remove pc");
}

/* Copies the file at `from` to a new file at `to`. Returns 0 or an error. */
static int copy_file(const char *from, const char *to) {
    int in = open(from, O_RDONLY);
    if (in < 0) {
        return in;
    }
    int out = open(to, O_WRONLY | O_CREAT | O_TRUNC);
    if (out < 0) {
        close(in);
        return out;
    }
    int n;
    while ((n = read(in, buf, sizeof buf)) > 0) {
        if (write(out, buf, (uint)n) != n) {
            n = -ENOSPC;
            break;
        }
    }
    close(in);
    close(out);
    return n;
}

/* Runs `argv` from `path` with its output to a pipe and returns what it
 * printed (in `out`, `size` bytes) and its status in *status. */
static int run_captured(const char *path, char **argv, char *out, int size, int *status) {
    int p[2];
    if (pipe(p) < 0) {
        return -1;
    }
    int console = dup(STDOUT);
    close(STDOUT);
    dup(p[1]);
    close(p[1]);
    int pid = spawn(path, argv, environ);
    close(STDOUT);
    dup(console);
    close(console);
    int total = 0;
    int n;
    while (total < size - 1 && (n = read(p[0], out + total, (uint)(size - 1 - total))) > 0) {
        total += n;
    }
    out[total] = 0;
    close(p[0]);
    *status = status_of(pid);
    return pid;
}

static void test_programs(void) {
    char out[64];
    int status;
    check_eq(copy_file("/bin/echo", "myecho"), 0, "copy /bin/echo");
    check(run_captured("/ft/myecho", (char *[]){"myecho", "from", "a", "file", NULL}, out, sizeof out,
                       &status) > 0,
          "spawn a program written by a program");
    check(strcmp(out, "from a file\n") == 0 && status == 0, "and it runs");
    check(run_captured("myecho", (char *[]){"myecho", "relative", NULL}, out, sizeof out, &status) > 0,
          "spawn by a relative path");
    check(strcmp(out, "relative\n") == 0, "and it runs too");
    int fd = open("notelf", O_WRONLY | O_CREAT);
    write(fd, "#!/bin/sh\n", 10);
    close(fd);
    /* The kernel prints why: "spawn: /ft/notelf: not an ELF file". */
    check_eq(spawn("/ft/notelf", NULL, environ), -ENOEXEC, "a file that isn't a program");
    /* exec fails as late as it can, reading the file: this program carries
     * on, the same process, its memory as it was. */
    int pid = getpid();
    char mark[] = "still here";
    check_eq(exec("/ft/notelf", (char *[]){"notelf", NULL}, environ), -ENOEXEC, "exec a file that isn't a program");
    check(getpid() == pid && strcmp(mark, "still here") == 0, "and the old program carries on");
    check_eq(spawn("/ft", NULL, environ), -EACCES, "a directory isn't a program");
    check_eq(spawn("/dev/console", NULL, environ), -EACCES, "nor is a device");
    check_eq(exec("/ft", NULL, environ), -EACCES, "exec a directory");
    check_eq(spawn("/ft/notelf/x", NULL, environ), -ENOTDIR, "a path through a file");
    check_eq(unlink("myecho"), 0, "remove the copy");
    check_eq(unlink("notelf"), 0, "remove the script");
}

static void test_devices(void) {
    struct stat st;
    int fd = open("/dev/console", O_WRONLY);
    check(fd >= 0, "open the console by its path");
    check_eq(write(fd, "", 0), 0, "write nothing to it");
    check_eq(fstat(fd, &st), 0, "fstat it");
    check(st.type == T_DEVICE && st.major == CONSOLE && st.ino > 1 && st.size == 0, "a device node");
    check_eq(lseek(fd, 0, SEEK_SET), -ESPIPE, "no seeking on a device");
    check_eq(read(fd, buf, 1), -EBADF, "opened for writing only");
    close(fd);
}

/* `filetest hold T`, a child of test_open_file_table(), in /ft with a pipe
 * at 0 and 1: opens new files hT00, hT01... until open fails, writes 'M' to
 * 1 if that was -EMFILE (its descriptors ran out), 'N' if -ENFILE (the
 * kernel's open files did) and the file was not made, 'X' otherwise; then
 * holds them until 0 reaches its end, removes them, and exits with how many
 * it opened. */
static int hold(char tag) {
    char name[] = "hT00";
    name[1] = tag;
    int opened = 0;
    int r;
    for (;;) {
        name[2] = (char)('0' + opened / 10);
        name[3] = (char)('0' + opened % 10);
        r = open(name, O_RDWR | O_CREAT);
        if (r < 0) {
            break;
        }
        opened++;
    }
    char c = r == -EMFILE ? 'M' : r == -ENFILE && unlink(name) == -ENOENT ? 'N' : 'X';
    write(STDOUT, &c, 1);
    while (read(STDIN, &c, 1) > 0) {
    }
    for (int i = 0; i < opened; i++) {
        name[2] = (char)('0' + i / 10);
        name[3] = (char)('0' + i % 10);
        unlink(name);
    }
    return opened;
}

/* Children hold files open until the kernel's table of open files is full:
 * open and pipe then fail with -ENFILE, having made nothing (not a panic:
 * the inode cache has room for every open file). */
static void test_open_file_table(void) {
    int hold_pipe[2], ready[2];
    check_eq(pipe(hold_pipe), 0, "a pipe to hold the children");
    check_eq(pipe(ready), 0, "a pipe they say they are ready on");
    int in = dup(STDIN);
    int out = dup(STDOUT);
    close(STDIN);
    dup(hold_pipe[0]);
    close(STDOUT);
    dup(ready[1]);
    /* At most 6: with the shell and filetest, NPROC (8) processes. */
    int pids[6];
    int children = 0;
    char c = 'M';
    char tag[2] = "a";
    while (c == 'M' && children < 6) {
        tag[0] = (char)('a' + children);
        int pid = spawn("/bin/filetest", (char *[]){"filetest", "hold", tag, NULL}, environ);
        if (pid < 0) {
            break;
        }
        pids[children++] = pid;
        if (read(ready[0], &c, 1) != 1) {
            c = '?';
        }
    }
    close(STDIN);
    dup(in);
    close(in);
    close(STDOUT);
    dup(out);
    close(out);
    check(c == 'N', "a child's open fails with -ENFILE, making nothing");
    check_eq(open("full", O_WRONLY | O_CREAT), -ENFILE, "so does the parent's");
    check_eq(unlink("full"), -ENOENT, "which makes nothing either");
    int p[2];
    check_eq(pipe(p), -ENFILE, "and pipe");
    close(hold_pipe[1]);
    for (int i = 0; i < children; i++) {
        int status = status_of(pids[i]);
        if (i < children - 1) {
            check_eq(status, NOFILE - 3, "a child opens a file for each of its free descriptors");
        } else {
            check(status >= 0 && status < NOFILE - 3, "the last child opens fewer");
        }
    }
    close(hold_pipe[0]);
    close(ready[0]);
    close(ready[1]);
    int fd = open("full", O_WRONLY | O_CREAT);
    check(fd >= 0, "once they are closed, open works again");
    close(fd);
    check_eq(unlink("full"), 0, "remove it");
}

static void test_big_file(void) {
    int fd = open("big", O_WRONLY | O_CREAT);
    check_eq(write_pattern(fd, BIG_FILE_SIZE, 2), BIG_FILE_SIZE, "write a file past 268 KiB");
    check_eq(size_of("big"), BIG_FILE_SIZE, "its size");
    close(fd);
    fd = open("big", O_RDONLY);
    check(holds_pattern(fd, BIG_FILE_SIZE, 2, sizeof buf), "it reads back");
    close(fd);
    fd = open("big", O_WRONLY | O_TRUNC);
    check_eq(size_of("big"), 0, "O_TRUNC empties it");
    close(fd);
    check_eq(unlink("big"), 0, "remove it");
}

/* The sequence the crash tests stop after every sector of. Each step is a
 * transaction or a few (the 5000-byte write is two), and the disk must hold
 * all of each or none of it:
 *
 *   mkdir /ops; create /ops/a and write 5000 bytes (seed 11); link /ops/b
 *   to it; mkdir /ops/d; in it, create c and write 100 bytes (seed 12);
 *   truncate /ops/b (O_TRUNC) and write 10 bytes (seed 13); open /ops/a,
 *   unlink /ops/a and /ops/b (the file has no links but is open: an orphan
 *   until it is closed), close it (it goes); unlink /ops/d/c and /ops/d;
 *   create /ops/e, empty.
 *
 * What is left: /ops and /ops/e. */
static int ops(void) {
    int fd;
#define STEP(call, what)                                                                                     \
    if ((call) < 0) {                                                                                        \
        printf("filetest: ops: %s failed\n", what);                                                          \
        return 1;                                                                                            \
    }
    STEP(mkdir("/ops"), "mkdir /ops");
    STEP(fd = open("/ops/a", O_WRONLY | O_CREAT), "create /ops/a");
    STEP(write_pattern(fd, 5000, 11) - 5000, "write /ops/a");
    close(fd);
    STEP(link("/ops/a", "/ops/b"), "link /ops/b");
    STEP(mkdir("/ops/d"), "mkdir /ops/d");
    STEP(chdir("/ops/d"), "chdir /ops/d");
    STEP(fd = open("c", O_WRONLY | O_CREAT), "create c");
    STEP(write_pattern(fd, 100, 12) - 100, "write c");
    close(fd);
    STEP(chdir("/"), "chdir /");
    STEP(fd = open("/ops/b", O_WRONLY | O_TRUNC), "truncate /ops/b");
    STEP(write_pattern(fd, 10, 13) - 10, "write /ops/b");
    close(fd);
    STEP(fd = open("/ops/a", O_RDONLY), "open /ops/a");
    STEP(unlink("/ops/a"), "unlink /ops/a");
    STEP(unlink("/ops/b"), "unlink /ops/b");
    STEP(close(fd), "close the orphan");
    STEP(unlink("/ops/d/c"), "unlink /ops/d/c");
    STEP(unlink("/ops/d"), "unlink /ops/d");
    STEP(fd = open("/ops/e", O_WRONLY | O_CREAT), "create /ops/e");
    close(fd);
#undef STEP
    printf("filetest: ops done\n");
    return 0;
}

/* On a read-only root (the tests give it one with /README). */
static int read_only(void) {
    struct stat st;
    check_eq(open("/README", O_WRONLY), -EROFS, "no writing a file");
    check_eq(open("/README", O_RDWR | O_TRUNC), -EROFS, "nor emptying it");
    check_eq(open("/new", O_WRONLY | O_CREAT), -EROFS, "nor making one");
    check_eq(mkdir("/new"), -EROFS, "nor a directory");
    check_eq(unlink("/README"), -EROFS, "nor removing one");
    check_eq(link("/README", "/new"), -EROFS, "nor linking one");
    check_eq(open("/nodir/x", O_RDONLY), -ENOENT, "a missing file is still missing");
    check_eq(open("/dev/nodriver", O_RDONLY), -ENODEV, "a device node with no driver");
    int fd = open("/README", O_RDONLY | O_CREAT);
    check(fd >= 0, "reading still works, O_CREAT on a file that exists too");
    check_eq(fstat(fd, &st), 0, "fstat it");
    check(st.size > 0 && read(fd, buf, sizeof buf) == (int)st.size, "read it whole");
    close(fd);
    fd = open("/dev/console", O_RDWR);
    check(fd >= 0, "a device is opened for writing even so");
    close(fd);
    check_eq(chdir("/bin"), 0, "chdir needs no writing");
    check_eq(stat("sh", &st), 0, "nor does a relative path");
    printf("filetest: %d checks, %d failed\n", checks, failures);
    return failures;
}

int main(int argc, char **argv) {
    if (argc > 1 && strcmp(argv[1], "ops") == 0) {
        return ops();
    }
    if (argc > 1 && strcmp(argv[1], "ro") == 0) {
        return read_only();
    }
    if (argc > 1 && strcmp(argv[1], "incwd") == 0) {
        struct stat st;
        return stat("c", &st) == 0 ? 0 : 1;
    }
    if (argc > 2 && strcmp(argv[1], "hold") == 0) {
        return hold(argv[2][0]);
    }
    int r = mkdir("/ft");
    if (r < 0 || chdir("/ft") < 0) {
        printf("filetest: cannot make /ft: %s\n", strerror(-r));
        return 1;
    }
    test_open_errors();
    test_read_write_seek();
    test_directories();
    test_working_directory();
    test_programs();
    test_devices();
    test_open_file_table();
    test_big_file();
    check_eq(chdir("/"), 0, "chdir /");
    check_eq(unlink("/ft"), 0, "remove /ft: it is empty");
    printf("filetest: %d checks, %d failed\n", checks, failures);
    return failures;
}
