/* init: the first process (docs/excalibur-spec.md, "Init and the shell").
 * The kernel starts it as pid 1 (falling back on /bin/sh when there is
 * none), and it starts the machine's services, as /etc/rc lists them:
 *
 *   # comment
 *   once     /bin/sh
 *   respawn  /bin/shd -p 23
 *
 * one service a line: what init does when it ends (`once`: nothing,
 * `respawn`: start it again), the program's path, and its arguments,
 * separated by blanks. Every service gets init's descriptors (the console)
 * and environment, and argv[0] is its program's name, the path's last part.
 *
 * - A respawn service that ends is started again. One that exits with
 *   EX_CONFIG (78, sysexits.h's) less than a second after it started can't
 *   run with this machine's configuration (shd with no network card): it
 *   said why, and init leaves it (a longer run that ends with 78, a shell's
 *   `exit 78`, is any other end). One that ends less than a second after it started, five times in a
 *   row, is left too, with a line that says so, rather than started over
 *   and over, as sysvinit stops a service that respawns too fast; a longer
 *   run starts the count again, so a shell ended at once now and then (by
 *   Ctrl-D) is not lost.
 * - Once every `once` service has ended (if /etc/rc has any), init shuts
 *   the machine down: it kills every other process, collects them, and
 *   exits with the status of the last one to end. So the server's console
 *   shell, a `once` service, stops the machine with `exit`, as it did when
 *   it was the first process itself. An /etc/rc of respawn services only
 *   runs until the machine is stopped from outside.
 * - The kernel gives init every process whose parent ends (an orphan), and
 *   init collects them as they end, as Linux's init does.
 *
 * Without /etc/rc (or with no service in it), init becomes the shell: it
 * execs /bin/sh, which is then pid 1, as with no init at all. Run by
 * anything but the kernel, init says it isn't the first process and exits
 * 1. */
#include "user.h"

/* Services at most: a process slot each (of the 32, NPROC,
 * docs/camelot-syscalls.md), leaving the rest for what they start. */
#define MAX_SERVICES 7
/* Arguments a service has at most, its program's name included. */
#define MAX_ARGS 8
/* The longest /etc/rc. */
#define RC_SIZE 2048
/* A run of a respawn service shorter than this, in ticks (10 a second),
 * is a quick exit... */
#define RESPAWN_MIN_TICKS 10
/* ...and after this many in a row it is not started again. */
#define RESPAWN_MAX_QUICK 5

#define RC "/etc/rc"
#define SHELL "/bin/sh"

struct service {
    int respawn; /* start it again when it ends */
    char *path;
    char *argv[MAX_ARGS + 1];
    int pid;     /* while it runs, else 0 */
    uint started; /* uptime() when it was last started */
    int quick;   /* quick exits in a row */
};

static struct service services[MAX_SERVICES];
static int nservices;
/* One byte more than an /etc/rc may have, to tell a longer one, and the NUL. */
static char rc[RC_SIZE + 2];

/* The last part of `path`, after its last '/'. */
static char *last_part(char *path) {
    char *name = path;
    for (char *p = path; *p != 0; p++) {
        if (*p == '/') {
            name = p + 1;
        }
    }
    return name;
}

/* Splits `line` into its words at blanks, in place: at most `max` of them
 * in `words`. Returns how many there were, or max + 1 if there were more. */
static int split(char *line, char **words, int max) {
    int n = 0;
    char *p = line;
    for (;;) {
        while (*p == ' ' || *p == '\t' || *p == '\r') {
            p++;
        }
        if (*p == 0) {
            return n;
        }
        if (n == max) {
            return max + 1;
        }
        words[n++] = p;
        while (*p != 0 && *p != ' ' && *p != '\t' && *p != '\r') {
            p++;
        }
        if (*p != 0) {
            *p++ = 0;
        }
    }
}

/* Reads /etc/rc into `services`, saying what is wrong with a line and
 * leaving it out. Returns false if there is no /etc/rc. */
static int read_rc(void) {
    int fd = open(RC, O_RDONLY);
    if (fd < 0) {
        return 0;
    }
    int len = 0;
    for (;;) {
        int n = read(fd, rc + len, RC_SIZE + 1 - len);
        if (n <= 0) {
            break;
        }
        len += n;
    }
    close(fd);
    if (len > RC_SIZE) {
        /* The rest is left out, and the line it cuts in two too. */
        printf("init: %s: longer than %d bytes, the rest left out\n", RC, RC_SIZE);
        len = RC_SIZE;
        while (len > 0 && rc[len - 1] != '\n') {
            len--;
        }
    }
    rc[len] = 0;
    char *line = rc;
    for (int number = 1; *line != 0; number++) {
        char *end = strchr(line, '\n');
        char *next = end != NULL ? end + 1 : line + strlen(line);
        if (end != NULL) {
            *end = 0;
        }
        char *hash = strchr(line, '#');
        if (hash != NULL) {
            *hash = 0;
        }
        char *words[MAX_ARGS + 2];
        int n = split(line, words, MAX_ARGS + 1);
        if (n == 0) {
            line = next;
            continue;
        }
        if (strcmp(words[0], "once") != 0 && strcmp(words[0], "respawn") != 0) {
            printf("init: %s:%d: %s: not once or respawn\n", RC, number, words[0]);
        } else if (n == 1 || words[1][0] != '/') {
            printf("init: %s:%d: no program's path\n", RC, number);
        } else if (n > MAX_ARGS + 1) {
            printf("init: %s:%d: more than %d arguments\n", RC, number, MAX_ARGS - 1);
        } else if (nservices == MAX_SERVICES) {
            printf("init: %s:%d: more than %d services\n", RC, number, MAX_SERVICES);
        } else {
            struct service *s = &services[nservices];
            s->respawn = words[0][0] == 'r';
            s->path = words[1];
            s->argv[0] = last_part(words[1]);
            for (int i = 2; i < n; i++) {
                s->argv[i - 1] = words[i];
            }
            s->argv[n - 1] = NULL;
            nservices++;
        }
        line = next;
    }
    return 1;
}

/* The status of a service that couldn't start, as the shell's for a
 * program that can't be run. */
#define NOT_STARTED 127

/* Starts service `s`: 0, or NOT_STARTED, having said why. */
static int start(struct service *s) {
    s->started = uptime();
    int pid = spawn(s->path, s->argv, environ);
    if (pid < 0) {
        printf("init: cannot start %s: %s\n", s->path, strerror(-pid));
        s->pid = 0;
        return NOT_STARTED;
    }
    s->pid = pid;
    return 0;
}

/* Whether a `once` service is listed, and whether one still runs. */
static int has_once(void) {
    for (int i = 0; i < nservices; i++) {
        if (!services[i].respawn) {
            return 1;
        }
    }
    return 0;
}

static int once_running(void) {
    for (int i = 0; i < nservices; i++) {
        if (!services[i].respawn && services[i].pid != 0) {
            return 1;
        }
    }
    return 0;
}

/* The machine's end: every other process killed and collected, then init's
 * own, with `status`. Killed again after each one collected: a process
 * started after a kill_descendants() snapshot (a session shd started for a
 * connection just then) is killed by the next, its parent having been
 * killed by the last. */
static int shut_down(int status) {
    do {
        kill_descendants(getpid());
    } while (wait(-1, NULL) > 0);
    return status;
}

int main(void) {
    if (getpid() != 1) {
        fprintf(STDERR, "init: not the first process\n");
        return 1;
    }
    if (!read_rc() || nservices == 0) {
        char *argv[] = {"sh", NULL};
        int err = exec(SHELL, argv, environ);
        printf("init: cannot run %s: %s\n", SHELL, strerror(-err));
        return 1;
    }
    int last = 0; /* the last `once` service's status */
    for (int i = 0; i < nservices; i++) {
        if (start(&services[i]) != 0 && !services[i].respawn) {
            last = NOT_STARTED;
        }
    }
    for (;;) {
        if (has_once() && !once_running()) {
            return shut_down(last);
        }
        int status;
        int pid = wait(-1, &status);
        if (pid < 0) {
            /* No process left at all: every service failed to start, or
             * ended for good. */
            return last;
        }
        for (int i = 0; i < nservices; i++) {
            struct service *s = &services[i];
            if (s->pid != pid) {
                continue;
            }
            s->pid = 0;
            if (!s->respawn) {
                last = status;
                break;
            }
            int quick = uptime() - s->started < RESPAWN_MIN_TICKS;
            if (quick && status == EX_CONFIG) {
                break; /* it can't run here, and said why */
            }
            s->quick = quick ? s->quick + 1 : 0;
            if (s->quick == RESPAWN_MAX_QUICK) {
                printf("init: %s ended at once %d times, exit %d: not started again\n", s->path,
                       RESPAWN_MAX_QUICK, status);
            } else {
                start(s); /* one that can't start any more is left */
            }
            break;
        }
        /* Any other pid was an orphan's: collected. */
    }
}
