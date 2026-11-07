/* login: logs a user in (docs/excalibur-spec.md, "Users").
 *
 *   login
 *
 * Asks for a name (`login: `) and a password (`Password: `, which the
 * terminal doesn't echo), checks the password against /etc/shadow's hash
 * (pw_check, lib/sha256.c), and then becomes the user: it sets its group
 * and user ids to the account's (setgid first: once the uid is no longer
 * root's, setgid is refused), goes to the account's home directory and
 * execs its shell, with HOME, USER, LOGNAME and SHELL set. A wrong name or
 * password says `Login incorrect` and asks again, after a pause, as every
 * Unix login does; so does an account whose home or shell is missing.
 *
 * It must run as root, to read /etc/shadow and change its ids: init starts
 * it on the console (and shd for each connection). The end of the input
 * (Ctrl-D at `login: `) ends it, with 1. Without /etc/passwd there is no
 * one to log in: it execs /bin/sh at once, as root (a disk without
 * accounts, and the kernel without a disk, have a root shell as before).
 *
 * The password is read with the terminal's echo off (getpass, lib/tty.c):
 * ECHO cleared in the console's termios (tcsetattr, docs/camelot-syscalls.md
 * "Terminals"), which the line discipline obeys, and the modes put back
 * after the line. On a connection (shd's sessions, whose input is a socket)
 * there is no terminal to switch: the client echoes what it sends. */
#include "user.h"

/* The longest name read (a password's is getpass's PASS_MAX); the rest of
 * a longer line is left out. */
#define FIELD_MAX 64
/* Ticks to wait after a failed attempt (10 a second). */
#define FAIL_PAUSE 10

/* Reads a line from stdin into `buf`, a byte at a time, so that nothing
 * after it is taken from whoever reads next (the shell this becomes): its
 * newline (and a `\r` before it) left out, a longer line cut at `size` - 1
 * bytes. Returns its length, or -1 at the end of the input. */
static int read_field(char *buf, int size) {
    int n = 0;
    for (;;) {
        char c;
        int r = read(STDIN, &c, 1);
        if (r <= 0) {
            return n > 0 ? n : -1;
        }
        if (c == '\n') {
            break;
        }
        if (c != '\r' && n < size - 1) {
            buf[n++] = c;
        }
    }
    buf[n] = 0;
    return n;
}

/* Whether `password` is `name`'s. An account without a shadow line, or
 * with an empty or locked (`!`, `*`) hash, has no password that works. */
static int password_ok(const char *name, const char *password) {
    struct spwd *sp = getspnam(name);
    return sp != NULL && pw_check(password, sp->sp_pwdp);
}

/* Becomes `pw`'s user and runs its shell; returns, still root, if the
 * account's home or shell is missing, so that the caller asks again. */
static void become(struct passwd *pw) {
    struct stat st;
    if (stat(pw->pw_dir, &st) < 0 || st.type != T_DIR || stat(pw->pw_shell, &st) < 0 || st.type != T_FILE) {
        return;
    }
    if (setgid(pw->pw_gid) < 0 || setuid(pw->pw_uid) < 0) {
        printf("login: cannot become %s\n", pw->pw_name);
        return;
    }
    if (chdir(pw->pw_dir) < 0) {
        printf("login: no home directory %s\n", pw->pw_dir);
        exit(1);
    }
    setenv("HOME", pw->pw_dir, 1);
    setenv("USER", pw->pw_name, 1);
    setenv("LOGNAME", pw->pw_name, 1);
    setenv("SHELL", pw->pw_shell, 1);
    /* argv[0] is the shell's name, the path's last part. */
    char *name = strrchr(pw->pw_shell, '/');
    char *argv[] = {name != NULL ? name + 1 : pw->pw_shell, NULL};
    int err = exec(pw->pw_shell, argv, environ);
    printf("login: cannot run %s: %s\n", pw->pw_shell, strerror(-err));
    exit(1);
}

/* No accounts: the shell, as root, which login is. */
static int root_shell(void) {
    char *argv[] = {"sh", NULL};
    int err = exec("/bin/sh", argv, environ);
    printf("login: cannot run /bin/sh: %s\n", strerror(-err));
    return 1;
}

int main(void) {
    if (geteuid() != 0) {
        printf("login: must be run as root\n");
        return 1;
    }
    struct stat st;
    if (stat(PASSWD_FILE, &st) < 0) {
        return root_shell();
    }
    char name[FIELD_MAX + 1];
    for (;;) {
        printf("login: ");
        if (read_field(name, sizeof name) < 0) {
            return 1;
        }
        if (name[0] == 0) {
            continue;
        }
        /* getpass turns the echo off (lib/tty.c). */
        char *password = getpass("Password: ");
        if (password == NULL) {
            return 1;
        }
        struct passwd *pw = NULL;
        if (password_ok(name, password)) {
            pw = getpwnam(name);
        }
        memset(password, 0, strlen(password));
        if (pw != NULL) {
            become(pw);
        }
        sleep(FAIL_PAUSE);
        printf("Login incorrect\n");
    }
}
