/* sh: the Camelot shell, the first process the kernel starts.
 *
 * Reads a line, splits it into commands at '&' and each command into words
 * at blanks, and runs them: a built-in command itself, anything else by
 * spawning the program the first word names, with the words as its argv. A
 * command followed by '&' runs in the background; the last one runs in the
 * foreground, and the shell waits for it before the next prompt. So
 * `ping & pong` runs both side by side and comes back when pong is done; the
 * `wait` built-in collects the background ones.
 *
 * What it prints besides the programs' own output:
 *   [PID]                when a background program starts
 *   NAME: exit S         when a foreground program ends with a status S != 0
 *   [PID] NAME: exit S   when `wait` collects a background program
 *   sh: NAME: REASON     when a program can't be started
 *
 * Built-ins: help, exit [N], wait. The shell ends with `exit`, or at the end
 * of the input (Ctrl-D on an empty line), with the status N or else the last
 * foreground program's; the kernel then stops the machine with it. */
#include "user.h"

#define PROMPT "$ "
/* The console's longest line (CONSOLE_LINE_MAX in kernel/include/console.h),
 * its newline included, and a NUL. */
#define LINE_SIZE 129
/* Background programs not collected yet that the shell remembers the names
 * of: one per process slot is plenty. */
#define MAX_JOBS 8
#define NAME_SIZE 16

struct job {
    int pid; /* 0: a free entry */
    char name[NAME_SIZE];
};

static struct job jobs[MAX_JOBS];

/* The exit status of the last foreground program. */
static int last_status;

static int is_blank(char c) {
    return c == ' ' || c == '\t';
}

/* Splits `s` into blank-separated words in place: NUL-terminates each and
 * puts a pointer to it in `words`, then a NULL. Returns how many, or -1 if
 * there are more than `max`. */
static int split(char *s, char **words, int max) {
    int n = 0;
    for (;;) {
        while (is_blank(*s)) {
            s++;
        }
        if (*s == 0) {
            break;
        }
        if (n == max) {
            return -1;
        }
        words[n] = s;
        n++;
        while (*s != 0 && !is_blank(*s)) {
            s++;
        }
        if (*s != 0) {
            *s = 0;
            s++;
        }
    }
    words[n] = NULL;
    return n;
}

static void copy_name(char *dst, const char *src) {
    int i = 0;
    while (src[i] != 0 && i < NAME_SIZE - 1) {
        dst[i] = src[i];
        i++;
    }
    dst[i] = 0;
}

static void remember(int pid, const char *name) {
    for (int i = 0; i < MAX_JOBS; i++) {
        if (jobs[i].pid == 0) {
            jobs[i].pid = pid;
            copy_name(jobs[i].name, name);
            return;
        }
    }
}

/* The background job `pid` was collected: says so and forgets it. */
static void collected(int pid, int status) {
    for (int i = 0; i < MAX_JOBS; i++) {
        if (jobs[i].pid == pid) {
            printf("[%d] %s: exit %d\n", pid, jobs[i].name, status);
            jobs[i].pid = 0;
            return;
        }
    }
    printf("[%d] exit %d\n", pid, status);
}

/* `wait`: collects every child until none is left. */
static void wait_all(void) {
    for (;;) {
        int status;
        int pid = wait(-1, &status);
        if (pid < 0) {
            return;
        }
        collected(pid, status);
    }
}

static void help(void) {
    printf("help            this list\n");
    printf("exit [N]        leave the shell, with status N or the last program's\n");
    printf("wait            wait for every background program\n");
    printf("PROG [ARG...]   run a program, e.g. hello, echo, ticks, ping, pong, sh\n");
    printf("CMD & CMD       run the first command in the background\n");
}

/* Runs one command. A built-in runs in the shell (whether or not
 * `background` is asked for); a program is spawned and, unless in the
 * background, waited for. Returns 1 if the shell should exit, with the
 * status in *exit_status. */
static int run(char **words, int background, int *exit_status) {
    const char *name = words[0];
    if (strcmp(name, "exit") == 0) {
        *exit_status = words[1] != NULL ? atoi(words[1]) : last_status;
        return 1;
    }
    if (strcmp(name, "help") == 0) {
        help();
        return 0;
    }
    if (strcmp(name, "wait") == 0) {
        wait_all();
        return 0;
    }
    int pid = spawn(name, words);
    if (pid < 0) {
        fprintf(STDERR, "sh: %s: %s\n", name, strerror(-pid));
        return 0;
    }
    if (background) {
        printf("[%d]\n", pid);
        remember(pid, name);
        return 0;
    }
    int status;
    if (wait(pid, &status) == pid) {
        last_status = status;
        if (status != 0) {
            printf("%s: exit %d\n", name, status);
        }
    }
    return 0;
}

/* Runs a line: its '&'-separated commands, in order. Returns 1 if the shell
 * should exit. */
static int run_line(char *line, int *exit_status) {
    char *cmd = line;
    for (;;) {
        char *amp = strchr(cmd, '&');
        if (amp != NULL) {
            *amp = 0;
        }
        char *words[MAXARG + 1];
        int n = split(cmd, words, MAXARG);
        if (n < 0) {
            fprintf(STDERR, "sh: too many arguments\n");
            return 0;
        }
        if (n > 0 && run(words, amp != NULL, exit_status)) {
            return 1;
        }
        if (amp == NULL) {
            return 0;
        }
        cmd = amp + 1;
    }
}

int main(void) {
    char line[LINE_SIZE];
    printf("Type help for the list of commands.\n");
    for (;;) {
        printf(PROMPT);
        if (getline(line, sizeof line) < 0) {
            return last_status;
        }
        int status;
        if (run_line(line, &status)) {
            return status;
        }
    }
}
