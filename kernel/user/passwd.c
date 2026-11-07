/* passwd: changes a password (docs/excalibur-spec.md, "Users").
 *
 *   passwd [USER]
 *
 * Changes USER's password, the caller's own by default: root may change
 * anyone's, anyone else only their own, and must give the current one
 * first. The new one is asked twice (the terminal's echo off), hashed with
 * a new salt (pw_hash, lib/sha256.c) and written to USER's line of
 * /etc/shadow, which the rest of the file keeps as it was.
 *
 * Like su, passwd is a set-user-ID root program (mode 4755): /etc/shadow
 * is root's alone, and passwd's real uid says who is asking. It rewrites
 * the file in place (Camelot has no rename to swap a new one in), so a
 * crash while it writes may leave the file short: a teaching system's
 * shortcut. */
#include "user.h"

/* An account's name at most. */
#define FIELD_MAX 64
/* The most bytes of /etc/shadow, as the library reads it. */
#define SHADOW_MAX 4096
/* Ticks to wait after a wrong password (10 a second), as login does. */
#define FAIL_PAUSE 10
/* The salt's length: hex digits of a hash of the time and the pid. */
#define SALT_LEN 16

/* Asks `prompt` with the echo off (getpass) and copies the answer to
 * `buf`, PASS_MAX + 1 bytes. 0, or -1 at the end of the input. */
static int ask(const char *prompt, char *buf) {
    char *answer = getpass(prompt);
    if (answer == NULL) {
        return -1;
    }
    strcpy(buf, answer);
    memset(answer, 0, strlen(answer));
    return 0;
}

/* A new salt: the first SALT_LEN hex digits of a SHA-256 of what changes
 * from one run to the next (the date and the time since boot, to the
 * nanosecond, and the pid). Enough for a teaching system: two salts that
 * differ keep one hash from serving for two accounts. */
static void new_salt(char *salt) {
    struct {
        struct timespec now;
        struct timespec boot;
        int pid;
    } seed;
    memset(&seed, 0, sizeof seed);
    clock_gettime(CLOCK_REALTIME, &seed.now);
    clock_gettime(CLOCK_MONOTONIC, &seed.boot);
    seed.pid = getpid();
    uint8_t digest[SHA256_SIZE];
    sha256(&seed, sizeof seed, digest);
    char hex[2 * SHA256_SIZE + 1];
    sha256_hex(digest, hex);
    memcpy(salt, hex, SALT_LEN);
    salt[SALT_LEN] = 0;
}

/* Puts `hash` as `name`'s in /etc/shadow, its other lines as they were (a
 * new line at the end if `name` has none). 0, or an error: -EFBIG for a
 * file longer than SHADOW_MAX, which it would cut short. */
static int write_shadow(const char *name, const char *hash) {
    /* A byte more than the file may have, to tell a longer one, and a NUL. */
    static char old[SHADOW_MAX + 2];
    static char out[SHADOW_MAX + PW_HASH_MAX + FIELD_MAX + 8];
    int fd = open(SHADOW_FILE, O_RDONLY);
    if (fd < 0) {
        return fd;
    }
    int len = 0;
    for (;;) {
        int n = read(fd, old + len, SHADOW_MAX + 1 - len);
        if (n <= 0) {
            break;
        }
        len += n;
    }
    close(fd);
    if (len > SHADOW_MAX) {
        return -EFBIG;
    }
    old[len] = 0;
    size_t nlen = strlen(name);
    int at = 0;
    int found = 0;
    char *line = old;
    while (*line != 0) {
        char *end = strchr(line, '\n');
        char *next = end != NULL ? end + 1 : line + strlen(line);
        if (!found && strncmp(line, name, nlen) == 0 && line[nlen] == ':') {
            /* NAME:HASH, then the line's other fields as they were. */
            char *rest = strchr(line + nlen + 1, ':');
            int keep = rest != NULL ? (int)(next - rest) : (end != NULL ? 1 : 0);
            at += snprintf(out + at, sizeof out - at, "%s:%s", name, hash);
            if (rest != NULL) {
                memcpy(out + at, rest, keep);
                at += keep;
            } else if (end != NULL) {
                out[at++] = '\n';
            }
            found = 1;
        } else {
            memcpy(out + at, line, next - line);
            at += (int)(next - line);
        }
        line = next;
    }
    if (!found) {
        if (at > 0 && out[at - 1] != '\n') {
            out[at++] = '\n';
        }
        at += snprintf(out + at, sizeof out - at, "%s:%s:\n", name, hash);
    }
    fd = open(SHADOW_FILE, O_WRONLY | O_TRUNC);
    if (fd < 0) {
        return fd;
    }
    int w = write(fd, out, at);
    close(fd);
    return w == at ? 0 : (w < 0 ? w : -EIO);
}

int main(int argc, char **argv) {
    if (argc > 2) {
        fprintf(STDERR, "usage: passwd [user]\n");
        return 2;
    }
    if (geteuid() != 0) {
        fprintf(STDERR, "passwd: not set-user-ID root (chmod 4755 /bin/passwd as root)\n");
        return 1;
    }
    uid_t caller = getuid();
    char name[FIELD_MAX + 1];
    if (argc == 2) {
        snprintf(name, sizeof name, "%s", argv[1]);
    } else {
        struct passwd *me = getpwuid(caller);
        if (me == NULL) {
            fprintf(STDERR, "passwd: no account for user id %u\n", caller);
            return 1;
        }
        snprintf(name, sizeof name, "%s", me->pw_name);
    }
    struct passwd *pw = getpwnam(name);
    if (pw == NULL) {
        fprintf(STDERR, "passwd: user %s does not exist\n", name);
        return 1;
    }
    if (caller != 0 && pw->pw_uid != caller) {
        fprintf(STDERR, "passwd: you may not change the password of %s\n", name);
        return 1;
    }
    char current[PASS_MAX + 1];
    char fresh[PASS_MAX + 1];
    char again[PASS_MAX + 1];
    printf("Changing password for %s\n", name);
    if (caller != 0) {
        if (ask("Current password: ", current) < 0) {
            return 1;
        }
        struct spwd *sp = getspnam(name);
        int ok = sp != NULL && pw_check(current, sp->sp_pwdp);
        memset(current, 0, sizeof current);
        if (!ok) {
            sleep(FAIL_PAUSE);
            fprintf(STDERR, "passwd: Authentication failure\n");
            return 1;
        }
    }
    if (ask("New password: ", fresh) < 0 || ask("Retype new password: ", again) < 0) {
        return 1;
    }
    if (strcmp(fresh, again) != 0) {
        fprintf(STDERR, "passwd: the passwords do not match\n");
        return 1;
    }
    if (fresh[0] == 0) {
        fprintf(STDERR, "passwd: an empty password is not allowed\n");
        return 1;
    }
    char salt[SALT_LEN + 1];
    new_salt(salt);
    char hash[PW_HASH_MAX + 1];
    pw_hash(fresh, salt, hash, sizeof hash);
    memset(fresh, 0, sizeof fresh);
    memset(again, 0, sizeof again);
    int err = write_shadow(name, hash);
    if (err < 0) {
        fprintf(STDERR, "passwd: cannot write %s: %s\n", SHADOW_FILE, strerror(-err));
        return 1;
    }
    printf("passwd: password changed\n");
    return 0;
}
