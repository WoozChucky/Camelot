/* permtest: checks the kernel's users and permissions
 * (docs/camelot-syscalls.md, "Users and permissions") on a writable root
 * filesystem. Run by root, it makes /pt and files of every mode in it,
 * checks root's bypass, then runs itself as an ordinary user (uid and gid
 * 1000, no account needed: the kernel knows numbers only) to check every
 * refusal, and removes /pt again. Each part prints a line for each check
 * that fails and, at its end, "permtest: N checks, F failed" (the user
 * part "permtest user: ..."), and exits with F (the root part with its
 * own and the user part's).
 *
 * Other modes (the parts it runs):
 *   permtest user PID becomes uid and gid 1000 and checks what they may do;
 *                     PID is a root process it may not kill
 *   permtest true     exits 0 (copies of permtest are the programs run)
 *   permtest euid     exits 0 if it runs with an effective uid of 0 and a
 *                     real one of 1000 (a set-user-ID root copy of permtest,
 *                     run by the user part)
 */
#include "user.h"

#define USER 1000
#define GROUP 1000
/* Another user, whose files the user part may not touch. */
#define OTHER 1001

static int checks;
static int failures;
static const char *part = "permtest";
/* The local sockets the root part listens on while the user part runs:
 * one whose name is 0700 (connecting needs write on it), one 0777. */
#define SHUT_SOCKET "/pt/sockshut"
#define OPEN_SOCKET "/pt/sockopen"
static int listeners[2] = {-1, -1};

static void check(int ok, const char *what) {
    checks++;
    if (!ok) {
        failures++;
        printf("%s: FAILED: %s\n", part, what);
    }
}

static void check_eq(long got, long want, const char *what) {
    checks++;
    if (got != want) {
        failures++;
        printf("%s: FAILED: %s: got %ld, want %ld\n", part, what, got, want);
    }
}

/* Makes `path` holding `text`, with `mode`; 0 or an error. */
static int make(const char *path, const char *text, mode_t mode) {
    int fd = open(path, O_WRONLY | O_CREAT | O_TRUNC, mode);
    if (fd < 0) {
        return fd;
    }
    int n = (int)strlen(text);
    int w = write(fd, text, (uint)n);
    close(fd);
    return w == n ? 0 : -EIO;
}

/* Copies the file at `from` to a new file `to` with `mode`. */
static int copy(const char *from, const char *to, mode_t mode) {
    static char buf[1024];
    int in = open(from, O_RDONLY);
    if (in < 0) {
        return in;
    }
    int out = open(to, O_WRONLY | O_CREAT | O_TRUNC, mode);
    if (out < 0) {
        close(in);
        return out;
    }
    int n;
    while ((n = read(in, buf, sizeof buf)) > 0) {
        if (write(out, buf, (uint)n) != n) {
            n = -EIO;
            break;
        }
    }
    close(in);
    close(out);
    return n < 0 ? n : 0;
}

/* The exit status of child `pid`, or -1000 if it can't be collected. */
static int status_of(int pid) {
    int status;
    if (pid < 0 || wait(pid, &status) != pid) {
        return -1000;
    }
    return status;
}

/* Opens `path` as `flags` asks and closes it: 0 or open's error. */
static int try_open(const char *path, int flags) {
    int fd = open(path, flags);
    if (fd < 0) {
        return fd;
    }
    close(fd);
    return 0;
}

/* What `path`'s stat says of its owner and mode, packed for comparing:
 * uid, gid and mode, or the error. */
static long owner_of(const char *path) {
    struct stat st;
    int r = stat(path, &st);
    if (r < 0) {
        return r;
    }
    return (long)st.uid << 32 | (long)st.gid << 16 | st.mode;
}

static long owner(uint uid, uint gid, uint mode) {
    return (long)uid << 32 | (long)gid << 16 | mode;
}

/* ---- The user part: uid and gid 1000 ---- */

static void as_user(int root_pid) {
    part = "permtest user";
    check_eq(setgid(GROUP), 0, "root sets the gid");
    check_eq(setuid(USER), 0, "root sets the uid");
    check_eq(getuid(), USER, "getuid after setuid");
    check_eq(geteuid(), USER, "geteuid after setuid");
    check_eq(getgid(), GROUP, "getgid after setgid");
    check_eq(getegid(), GROUP, "getegid after setgid");
    /* Root's power is gone for good. */
    check_eq(setuid(0), -EPERM, "setuid to root");
    check_eq(setuid(OTHER), -EPERM, "setuid to another user");
    check_eq(setgid(0), -EPERM, "setgid to root's group");
    check_eq(setuid(USER), 0, "setuid to its own uid");
    check_eq(setgid(GROUP), 0, "setgid to its own gid");

    /* Reading and writing: the owner's, the group's or the others' bits. */
    check_eq(try_open("/pt/secret", O_RDONLY), -EACCES, "read root's 0600 file");
    check_eq(try_open("/pt/secret", O_WRONLY), -EACCES, "write root's 0600 file");
    check_eq(try_open("/pt/other", O_RDONLY), -EACCES, "read another user's 0600 file");
    check_eq(try_open("/pt/public", O_RDONLY), 0, "read a 0644 file");
    check_eq(try_open("/pt/public", O_WRONLY), -EACCES, "write a 0644 file not its own");
    check_eq(try_open("/pt/public", O_RDWR), -EACCES, "read and write it");
    check_eq(try_open("/pt/public", O_WRONLY | O_TRUNC), -EACCES, "empty it");
    check_eq(try_open("/pt/group", O_RDONLY), 0, "read a 0640 file of its group");
    check_eq(try_open("/pt/group", O_WRONLY), -EACCES, "but not write it");
    /* The owner's bits count for the owner, even when the others' allow
     * more. */
    check_eq(try_open("/pt/mine", O_RDONLY), -EACCES, "read its own 0044 file");
    check_eq(try_open("/pt/rootdir", O_RDONLY), 0, "list a 0755 directory");
    check_eq(try_open("/pt/closed", O_RDONLY), -EACCES, "list a 0700 directory");

    /* Searching: execute on every directory of a path. */
    struct stat st;
    check_eq(stat("/pt/closed/f", &st), -EACCES, "stat through a 0700 directory");
    check_eq(try_open("/pt/closed/f", O_RDONLY), -EACCES, "open through it");
    check_eq(chdir("/pt/closed"), -EACCES, "chdir into it");
    check_eq(stat("/pt/listonly/f", &st), -EACCES, "stat through a 0744 directory");
    check_eq(stat("/pt/searchonly/f", &st), 0, "stat through a 0711 directory");
    check_eq(try_open("/pt/searchonly", O_RDONLY), -EACCES, "but not list it");
    check_eq(spawn("/pt/closed/prog", (char *[]){"prog", "true", NULL}, environ), -EACCES,
             "spawn through a 0700 directory");

    /* Making and removing: write and search on the directory. */
    check_eq(make("/pt/rootdir/new", "x", 0644), -EACCES, "create in root's 0755 directory");
    check_eq(make("/bin/new", "x", 0644), -EACCES, "create in /bin");
    check_eq(try_open("/bin/sh", O_WRONLY), -EACCES, "write /bin/sh");
    check_eq(mkdir("/pt/rootdir/d", 0755), -EACCES, "mkdir in it");
    check_eq(unlink("/pt/rootdir/f"), -EACCES, "unlink in it");
    check_eq(link("/pt/public", "/pt/rootdir/l"), -EACCES, "link into it");
    check_eq(make("/pt/home/x", "mine", 0600), 0, "create in its own directory");
    check_eq(owner_of("/pt/home/x"), owner(USER, GROUP, 0600), "the file is its own, with the mode given");
    check_eq(link("/pt/home/x", "/pt/rootdir/l"), -EACCES, "link its own file into it");
    int fd = open("/pt/home/ro", O_WRONLY | O_CREAT, 0444);
    check(fd >= 0, "create a 0444 file for writing");
    check_eq(write(fd, "w", 1), 1, "and write it: the new file is open as asked");
    close(fd);
    check_eq(try_open("/pt/home/ro", O_WRONLY), -EACCES, "but not open it for writing again");
    check_eq(mkdir("/pt/home/d", 0700), 0, "mkdir in it");
    check_eq(owner_of("/pt/home/d"), owner(USER, GROUP, 0700), "the directory is its own");
    check_eq(link("/pt/home/x", "/pt/home/d/y"), 0, "link in it");
    check_eq(unlink("/pt/home/d/y"), 0, "unlink in it");
    check_eq(unlink("/pt/home/d"), 0, "rmdir in it");

    /* A sticky directory: anyone makes files, only their owners remove
     * them. */
    check_eq(make("/pt/tmp/u", "u", 0644), 0, "create in a 1777 directory");
    check_eq(unlink("/pt/tmp/u"), 0, "remove its own file there");
    check_eq(unlink("/pt/tmp/r"), -EPERM, "remove root's file there");
    check_eq(unlink("/pt/open/r"), 0, "remove root's file in a 0777 directory");

    /* chmod: the owner's; chown: root's. */
    check_eq(chmod("/pt/home/x", 0640), 0, "chmod its own file");
    check_eq(owner_of("/pt/home/x"), owner(USER, GROUP, 0640), "the new mode");
    check_eq(chmod("/pt/home/x", 02640), 0, "set-group-ID of its own group");
    check_eq(owner_of("/pt/home/x"), owner(USER, GROUP, 02640), "is kept");
    check_eq(chmod("/pt/public", 0666), -EPERM, "chmod root's file");
    check_eq(chown("/pt/home/x", OTHER, -1), -EPERM, "chown its own file away");
    check_eq(chown("/pt/home/x", USER, GROUP), -EPERM, "chown, even to itself");
    check_eq(chmod("/pt/closed/f", 0777), -EACCES, "chmod through a 0700 directory");

    /* Running programs: execute, and a set-user-ID program runs as its
     * owner. */
    check_eq(spawn("/pt/noexec", (char *[]){"noexec", "true", NULL}, environ), -EACCES, "spawn a 0644 program");
    check_eq(spawn("/pt/rootonly", (char *[]){"rootonly", "true", NULL}, environ), -EACCES,
             "spawn a 0700 program of root's");
    int pid = spawn("/pt/suid", (char *[]){"suid", "euid", NULL}, environ);
    check_eq(status_of(pid), 0, "a set-user-ID root program runs as root");
    pid = spawn("/pt/cleared", (char *[]){"cleared", "euid", NULL}, environ);
    check_eq(status_of(pid), 1, "chown cleared a program's set-user-ID bit");

    /* Other users' processes. */
    check_eq(kill(root_pid), -EPERM, "kill a root process");

    /* A local socket's name: connecting needs write on it. */
    struct sockaddr_un un;
    memset(&un, 0, sizeof un);
    un.sun_family = AF_UNIX;
    strcpy(un.sun_path, SHUT_SOCKET);
    int c = socket(AF_UNIX, SOCK_STREAM, 0);
    check_eq(connect(c, &un), -EACCES, "connect to root's 0700 socket");
    strcpy(un.sun_path, OPEN_SOCKET);
    check_eq(connect(c, &un), 0, "connect to a 0777 one");
    close(c);

    /* The devices: the console, the screen and the input are root's, so
     * that nobody reads another's keys (a session keeps the console it was
     * given open). */
    check_eq(try_open("/dev/console", O_RDONLY), -EACCES, "open the console");
    check_eq(try_open("/dev/input/event0", O_RDONLY), -EACCES, "open the keyboard");
    check_eq(try_open("/dev/fb0", O_RDWR), -EACCES, "open the framebuffer");
    check_eq(try_open("/dev/pts/0", O_RDWR), -EACCES, "open a terminal's slave");

    /* The machine's address is root's, network card or not. */
    check_eq(netconf(0x0a00020f, 0xffffff00, 0), -EPERM, "netconf");

    /* The privileged ports, if there is a network to bind on. */
    int s = socket(AF_INET, SOCK_DGRAM, 0);
    if (s >= 0) {
        struct sockaddr_in a;
        sockaddr_set(&a, INADDR_ANY, 80);
        check_eq(bind(s, &a), -EACCES, "bind port 80");
        sockaddr_set(&a, INADDR_ANY, 1024);
        check_eq(bind(s, &a), 0, "bind port 1024");
        close(s);
    }

    /* What it made goes; root removes the rest. */
    unlink("/pt/home/x");
    unlink("/pt/home/ro");
}

/* ---- The root part ---- */

/* Makes /pt: files and directories of every mode the user part tries. */
static void set_up(void) {
    check_eq(mkdir("/pt", 0755), 0, "mkdir /pt");
    check_eq(make("/pt/secret", "root's\n", 0600), 0, "make /pt/secret");
    check_eq(make("/pt/other", "another's\n", 0600), 0, "make /pt/other");
    check_eq(chown("/pt/other", OTHER, OTHER), 0, "give it to another user");
    check_eq(make("/pt/public", "everyone's\n", 0644), 0, "make /pt/public");
    check_eq(make("/pt/group", "the group's\n", 0640), 0, "make /pt/group");
    check_eq(chown("/pt/group", -1, GROUP), 0, "give it to the user's group");
    check_eq(owner_of("/pt/group"), owner(0, GROUP, 0640), "chown -1 keeps the uid");
    check_eq(make("/pt/mine", "the user's\n", 0044), 0, "make /pt/mine");
    check_eq(chown("/pt/mine", USER, -1), 0, "give it to the user");
    check_eq(mkdir("/pt/rootdir", 0755), 0, "mkdir /pt/rootdir");
    check_eq(make("/pt/rootdir/f", "", 0644), 0, "a file in it");
    check_eq(mkdir("/pt/closed", 0700), 0, "mkdir /pt/closed");
    check_eq(make("/pt/closed/f", "", 0644), 0, "a file in it");
    check_eq(copy("/bin/permtest", "/pt/closed/prog", 0755), 0, "a program in it");
    check_eq(mkdir("/pt/listonly", 0744), 0, "mkdir /pt/listonly");
    check_eq(make("/pt/listonly/f", "", 0644), 0, "a file in it");
    check_eq(mkdir("/pt/searchonly", 0711), 0, "mkdir /pt/searchonly");
    check_eq(make("/pt/searchonly/f", "", 0644), 0, "a file in it");
    check_eq(mkdir("/pt/home", 0755), 0, "mkdir /pt/home");
    check_eq(chown("/pt/home", USER, GROUP), 0, "give it to the user");
    check_eq(mkdir("/pt/tmp", 01777), 0, "mkdir /pt/tmp, sticky");
    check_eq(make("/pt/tmp/r", "", 0644), 0, "root's file in it");
    check_eq(mkdir("/pt/open", 0777), 0, "mkdir /pt/open");
    check_eq(make("/pt/open/r", "", 0644), 0, "root's file in it");
    check_eq(copy("/bin/permtest", "/pt/noexec", 0644), 0, "a program without x");
    check_eq(copy("/bin/permtest", "/pt/rootonly", 0700), 0, "a program only root runs");
    check_eq(copy("/bin/permtest", "/pt/suid", 04755), 0, "a set-user-ID copy of permtest");
    check_eq(owner_of("/pt/suid"), owner(0, 0, 04755), "its mode");
    check_eq(copy("/bin/permtest", "/pt/cleared", 04755), 0, "another");
    check_eq(chown("/pt/cleared", 0, 0), 0, "chown it");
    check_eq(owner_of("/pt/cleared"), owner(0, 0, 0755), "chown clears set-user-ID");
    /* A directory keeps them. */
    check_eq(chmod("/pt/tmp", 03777), 0, "chmod a directory set-group-ID");
    check_eq(chown("/pt/tmp", 0, 0), 0, "chown it");
    check_eq(owner_of("/pt/tmp"), owner(0, 0, 03777), "a directory keeps its bits");
    check_eq(chmod("/pt/tmp", 01777), 0, "back to 1777");
    check_eq(chmod("/pt/nothing", 0644), -ENOENT, "chmod a missing file");
    check_eq(chmod("/pt/secret", 0170600), 0, "chmod ignores bits past 07777");
    check_eq(owner_of("/pt/secret"), owner(0, 0, 0600), "they are dropped");
    /* Two listening local sockets, for the user part to connect to. */
    const char *names[2] = {SHUT_SOCKET, OPEN_SOCKET};
    for (int i = 0; i < 2; i++) {
        struct sockaddr_un un;
        memset(&un, 0, sizeof un);
        un.sun_family = AF_UNIX;
        strcpy(un.sun_path, names[i]);
        listeners[i] = socket(AF_UNIX, SOCK_STREAM, 0);
        check_eq(bind(listeners[i], &un), 0, "bind a local socket");
        check_eq(listen(listeners[i], 1), 0, "listen on it");
    }
    check_eq(chmod(SHUT_SOCKET, 0700), 0, "a 0700 socket");
}

/* Root reads, writes and searches anything; it runs only what has an x. */
static void root_bypass(void) {
    check_eq(make("/pt/none", "nobody's\n", 0), 0, "make a 0000 file");
    check_eq(try_open("/pt/none", O_RDWR), 0, "root reads and writes a 0000 file");
    check_eq(mkdir("/pt/shut", 0), 0, "mkdir a 0000 directory");
    check_eq(make("/pt/shut/f", "", 0644), 0, "root makes a file in it");
    check_eq(try_open("/pt/shut/f", O_RDONLY), 0, "and searches it");
    check_eq(unlink("/pt/shut/f"), 0, "and removes from it");
    check_eq(unlink("/pt/shut"), 0, "rmdir it");
    check_eq(chdir("/pt/closed"), 0, "chdir into a 0700 directory");
    check_eq(chdir("/"), 0, "and out");
    check_eq(spawn("/pt/noexec", (char *[]){"noexec", "true", NULL}, environ), -EACCES,
             "root can't run a program no one may");
    check_eq(copy("/bin/permtest", "/pt/userx", 0100), 0, "a program only its owner runs");
    check_eq(status_of(spawn("/pt/userx", (char *[]){"userx", "true", NULL}, environ)), 0,
             "root runs it: one x is enough");
    check_eq(status_of(spawn("/pt/rootonly", (char *[]){"rootonly", "true", NULL}, environ)), 0,
             "root runs its 0700 program");
    check_eq(chmod("/pt/other", 0644), 0, "root chmods another user's file");
    check_eq(chmod("/pt/other", 0600), 0, "and back");
    check_eq(try_open("/pt/other", O_RDONLY), 0, "root reads another user's 0600 file");
    /* A pipe is its maker's. */
    int p[2];
    check_eq(pipe(p), 0, "pipe");
    struct stat st;
    check_eq(fstat(p[0], &st), 0, "fstat a pipe");
    check(st.mode == 0600 && st.uid == 0 && st.gid == 0, "a pipe is its maker's, 0600");
    close(p[0]);
    close(p[1]);
}

static const char *const made[] = {
    "/pt/secret", "/pt/other", "/pt/public", "/pt/group", "/pt/mine", "/pt/rootdir/f", "/pt/rootdir",
    "/pt/closed/f", "/pt/closed/prog", "/pt/closed", "/pt/listonly/f", "/pt/listonly", "/pt/searchonly/f",
    "/pt/searchonly", "/pt/home", "/pt/tmp/r", "/pt/tmp", "/pt/open", "/pt/noexec", "/pt/rootonly",
    "/pt/suid", "/pt/cleared", "/pt/none", "/pt/userx", SHUT_SOCKET, OPEN_SOCKET, "/pt",
};

int main(int argc, char **argv) {
    if (argc == 3 && strcmp(argv[1], "user") == 0) {
        as_user(atoi(argv[2]));
        printf("%s: %d checks, %d failed\n", part, checks, failures);
        return failures;
    }
    if (argc == 2 && strcmp(argv[1], "true") == 0) {
        return 0;
    }
    if (argc == 2 && strcmp(argv[1], "euid") == 0) {
        return geteuid() == 0 && getuid() == USER ? 0 : 1;
    }
    if (argc != 1) {
        fprintf(STDERR, "usage: permtest\n");
        return 2;
    }
    if (geteuid() != 0) {
        printf("permtest: must be run as root\n");
        return 1;
    }
    struct stat root;
    if (stat("/", &root) < 0) {
        printf("permtest: no disk\n");
        return 1;
    }
    set_up();
    root_bypass();
    char pid[16];
    snprintf(pid, sizeof pid, "%d", getpid());
    int user_failures = status_of(spawn("/bin/permtest", (char *[]){"permtest", "user", pid, NULL}, environ));
    check_eq(user_failures, 0, "the user part");
    close(listeners[0]);
    close(listeners[1]);
    for (uint i = 0; i < sizeof made / sizeof made[0]; i++) {
        int r = unlink(made[i]);
        if (r < 0) {
            printf("permtest: FAILED: unlink %s: %s\n", made[i], strerror(-r));
            failures++;
        }
    }
    printf("permtest: %d checks, %d failed\n", checks, failures);
    return failures;
}
