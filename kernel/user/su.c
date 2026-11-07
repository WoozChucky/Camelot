/* su: runs a shell as another user (docs/excalibur-spec.md, "Users").
 *
 *   su [USER]
 *
 * USER is root by default. su asks for USER's password (the terminal's
 * echo off, as login asks), unless whoever runs it is root already, then
 * becomes USER as login does (setgid, then setuid; HOME, USER, LOGNAME and
 * SHELL set) and runs USER's shell; the working directory stays where it
 * was. A wrong password says `su: Authentication failure` and exits 1.
 *
 * Changing to another user needs root's power, which an ordinary user's
 * process hasn't: su is a set-user-ID program, owned by root with mode
 * 4755 (`cargo xtask fs` installs it so), so it runs with an effective uid
 * of 0 whoever starts it, and its real uid says who that was. It trusts
 * nothing else of its caller's. */
#include "user.h"

/* An account's name at most, as login reads it. */
#define NAME_LEN 64
/* Ticks to wait after a wrong password (10 a second), as login does. */
#define FAIL_PAUSE 10

int main(int argc, char **argv) {
    if (argc > 2) {
        fprintf(STDERR, "usage: su [user]\n");
        return 2;
    }
    const char *name = argc == 2 ? argv[1] : "root";
    if (geteuid() != 0) {
        fprintf(STDERR, "su: not set-user-ID root (chmod 4755 /bin/su as root)\n");
        return 1;
    }
    struct passwd *pw = getpwnam(name);
    if (pw == NULL) {
        fprintf(STDERR, "su: user %s does not exist\n", name);
        return 1;
    }
    /* Copied: getspnam and the next getpwnam reuse their buffers. */
    uid_t uid = pw->pw_uid;
    gid_t gid = pw->pw_gid;
    char home[128];
    char shell[128];
    char user[NAME_LEN + 1];
    snprintf(home, sizeof home, "%s", pw->pw_dir);
    snprintf(shell, sizeof shell, "%s", pw->pw_shell);
    snprintf(user, sizeof user, "%s", pw->pw_name);
    if (getuid() != 0) {
        char *password = getpass("Password: ");
        if (password == NULL) {
            return 1;
        }
        struct spwd *sp = getspnam(user);
        int ok = sp != NULL && pw_check(password, sp->sp_pwdp);
        memset(password, 0, strlen(password));
        if (!ok) {
            sleep(FAIL_PAUSE);
            fprintf(STDERR, "su: Authentication failure\n");
            return 1;
        }
    }
    if (setgid(gid) < 0 || setuid(uid) < 0) {
        fprintf(STDERR, "su: cannot become %s\n", user);
        return 1;
    }
    setenv("HOME", home, 1);
    setenv("USER", user, 1);
    setenv("LOGNAME", user, 1);
    setenv("SHELL", shell, 1);
    char *slash = strrchr(shell, '/');
    char *args[] = {slash != NULL ? slash + 1 : shell, NULL};
    int err = exec(shell, args, environ);
    fprintf(STDERR, "su: cannot run %s: %s\n", shell, strerror(-err));
    return 1;
}
