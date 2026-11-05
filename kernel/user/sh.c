/* sh: the Camelot shell, the first process the kernel starts.
 *
 * Reads a line and runs it. The line is a list of jobs separated by '&':
 * each but the last runs in the background, the last in the foreground,
 * and the shell waits for it before the next prompt. A job is a pipeline,
 * commands separated by '|', each one's output going to the next one's
 * input. A command is words separated by blanks, with optional
 * redirections among them: `< FILE` reads the input from FILE, `> FILE`
 * writes the output to FILE (made, or emptied, first). '&', '|', '<' and
 * '>' need no blanks around them. So `ls /bin | wc & cat < a > b` counts
 * the programs in the background and copies a to b in the foreground.
 *
 * The first word names a built-in or a program: `NAME` is the program
 * /bin/NAME, and a word with a '/' in it is the program at that path
 * (relative to the working directory unless it starts with '/'). The
 * program gets the words as its argv.
 *
 * There is no fork: the kernel's spawn starts a program as a child, which
 * inherits the shell's descriptors 0, 1 and 2. So to start a program with
 * other ones, the shell puts them at 0 and 1 itself (close, then dup, which
 * takes the lowest free descriptor), spawns it, and puts its own console
 * back from copies it keeps above 2, which no child inherits.
 *
 * What it prints besides the programs' own output:
 *   [PID]                for each program of a background job
 *   NAME: exit S         when a foreground program ends with a status S != 0
 *   [PID] NAME: exit S   when `wait` collects a background program
 *   sh: NAME: REASON     when a program can't be started, a file can't be
 *                        opened, or a line makes no sense
 *
 * Built-ins: help, exit [N], wait, cd [DIR], export, unset and exec [PROG
 * [ARG...]] (see below). All but exec run in the shell itself, with its
 * redirections, but can't be part of a pipeline. The shell ends with `exit`, or at the end
 * of the input (Ctrl-D on an empty line), with the status N or else the last
 * foreground program's; when it is the first process, the kernel then stops
 * the machine with it.
 *
 * `exec PROG [ARG...]` is POSIX's: the program replaces the shell in its
 * process (the kernel's exec), with the same pid, parent, descriptors and
 * children, so the shell's parent waits for the program instead, and the
 * program's status is the session's. The command's redirections are the
 * program's: the shell keeps its own input and output in copies from
 * descriptor 10 up that close on exec (fcntl's F_DUPFD_CLOEXEC, as bash
 * keeps them), puts the files at 0 and 1 and execs, and the kernel closes
 * the copies as the program starts, so it gets nothing else. If the program
 * can't be started, the shell puts its own back from the copies and says
 * so. Then, as POSIX has it, an interactive shell carries on, and any other
 * ends: with 127 if there is no such program, 126 if it is there but can't
 * be run, and 1 if a redirection can't be made. The shell is interactive if
 * its input was the console or a connection when it started (`sh < FILE`
 * and `CMD | sh` are not). Without a program, `exec`'s redirections stay
 * the shell's: `exec < FILE` reads the next lines from FILE. In a pipeline
 * or a background job, which POSIX runs in a subshell, `exec` replaces the
 * subshell, which here is the command alone: `exec NAME ...` runs the
 * program NAME, never a built-in, and `exec > FILE` only makes FILE (or
 * empties it), with status 0.
 *
 * Variables and the environment, as POSIX's sh has them (there is no
 * `$NAME` expansion: the variables are for the programs). The shell starts
 * with a variable for each NAME=value of its own environment (what crt0
 * put in environ), all of them exported; NAME is a letter or '_', then
 * letters, digits and '_'. Words NAME=value before a command's first other
 * word are assignments:
 *   NAME=value ...        alone, they set shell variables, which stay
 *                         exported if they were (else they are the
 *                         shell's own, until `export NAME`);
 *   NAME=value PROG ...   before a program, they are in its environment
 *                         only (and the shell's stays as it was);
 *   NAME=value exec PROG  before `exec`, `exit`, `export` or `unset` (POSIX's
 *                         special built-ins), they set the shell's
 *                         variables first, and `exec` exports them, as
 *                         dash and bash do, so PROG has them;
 *   NAME=value cd ...     before another built-in, they are for it alone:
 *                         `HOME=/bin cd` goes to /bin.
 * `export NAME[=value]...` exports variables (setting them first if given
 * a value: `export NAME` alone exports one that may get its value later);
 * `export` or `export -p` lists the exported ones as `export NAME=value`
 * (or `export NAME`, without a value yet); `unset NAME...` removes them.
 * `cd` without a directory goes to HOME, as POSIX's does (/ if there is
 * none). Every program the shell starts, through spawn or `exec`, gets the
 * exported variables that have a value as its environment, its
 * assignments added (or put in place of the variable of the same name).
 *
 * Its input is usually the console, whose line discipline edits and echoes
 * a line and hands it over whole, one per read. Any other input (a network
 * connection from shd, a pipe, a file) has no line discipline: the shell
 * reads it a byte at a time up to each newline, so that nothing after the
 * line is taken from the programs it starts, which read the same input.
 * There the other end edits and echoes lines itself; a '\r' before the
 * newline (a telnet client's line end) is dropped.
 *
 * On a connection, the end of the input is a hang-up: the client is gone
 * (or said it is done), and nobody is left to see what runs. So the shell
 * kills every program it started, background ones included, and theirs,
 * before it ends; `exit` leaves them running. shd, which started it, could
 * not do it afterwards: once the shell has ended, its orphans have no
 * parent, and nothing tells whose they were. */
#include "user.h"

#define PROMPT "$ "
/* The console's longest line (CONSOLE_LINE_MAX in kernel/include/console.h),
 * its newline included, and a NUL. */
#define LINE_SIZE 129
/* Background programs not collected yet that the shell remembers the names
 * of: one per process slot is plenty. */
#define MAX_JOBS 8
#define NAME_SIZE 16
/* Words and operators in a line: a line of 128 bytes can't have more. */
#define MAX_TOKENS 128
/* Commands in a pipeline: there are only 8 process slots. */
#define MAX_PIPE 8

struct job {
    int pid; /* 0: a free entry */
    char name[NAME_SIZE];
};

static struct job jobs[MAX_JOBS];

/* The exit status of the last foreground program. */
static int last_status;

/* Whether the input is the console (a device) or a network connection: see
 * the top. */
static int console_input;
static int connection_input;

/* Whether the shell is interactive, as POSIX means it: its input was the
 * console or a connection when it started (see the top; a later `exec <
 * FILE` doesn't change it, as in bash). */
static int interactive;

/* Where the shell keeps copies of its descriptors meanwhile: 10 and up, as
 * bash does (POSIX leaves descriptors 0 to 9 to the user). */
#define SAVED_FD 10

/* A token of a line: a word, or an operator ('&', '|', '<', '>'). */
struct token {
    char op; /* 0 for a word */
    char *word;
};

/* A command: its assignments (NAME=value words, see the top), its words
 * (NULL-terminated) and where its input comes from and its output goes, if
 * not from the shell or the pipeline. */
struct cmd {
    char *assigns[MAXARG];
    int nassigns;
    char *argv[MAXARG + 1];
    int argc;
    char *in;
    char *out;
    /* Set for `exec NAME ...` apart from the shell, once the `exec` is
     * dropped: NAME is a program, never a built-in (see run_job). */
    int program_only;
};

/* The shell's variables (see the top): NAME=value, or NAME alone for one
 * exported before it has a value, each string from malloc. */
#define MAX_VARS 32

struct var {
    char *entry; /* NULL: a free slot */
    int exported;
};

static struct var vars[MAX_VARS];

/* The length of the name NAME=value `s` starts with, if it is an
 * assignment, else 0. A name is a letter or '_', then letters, digits and
 * '_' (POSIX's). With `whole`, `s` must be a name and nothing else. */
static int name_length(const char *s, int whole) {
    int n = 0;
    while ((s[n] >= 'a' && s[n] <= 'z') || (s[n] >= 'A' && s[n] <= 'Z') || s[n] == '_' ||
           (n > 0 && s[n] >= '0' && s[n] <= '9')) {
        n++;
    }
    if (n == 0) {
        return 0;
    }
    return s[n] == (whole ? 0 : '=') ? n : 0;
}

/* Whether entry `e` (NAME=value or NAME) is the variable `name`, `len`
 * bytes. */
static int is_named(const char *e, const char *name, int len) {
    return strncmp(e, name, (size_t)len) == 0 && (e[len] == '=' || e[len] == 0);
}

/* The variable `name` (`len` bytes), or NULL. */
static struct var *find_var(const char *name, int len) {
    for (int i = 0; i < MAX_VARS; i++) {
        if (vars[i].entry != NULL && is_named(vars[i].entry, name, len)) {
            return &vars[i];
        }
    }
    return NULL;
}

/* Sets the variable `entry` names to it: NAME=value, or NAME alone (which
 * leaves the value of one that has one). It stays exported if it was, and
 * is if `export`. Returns 0, or -1 having said why it can't. */
static int set_var(const char *entry, int len, int export) {
    struct var *v = find_var(entry, len);
    if (v != NULL && entry[len] == 0) {
        v->exported |= export;
        return 0;
    }
    if (v == NULL) {
        for (int i = 0; i < MAX_VARS && v == NULL; i++) {
            if (vars[i].entry == NULL) {
                v = &vars[i];
                v->exported = 0;
            }
        }
        if (v == NULL) {
            fprintf(STDERR, "sh: %s: too many variables\n", entry);
            return -1;
        }
    }
    size_t size = strlen(entry) + 1;
    char *copy = malloc(size);
    if (copy == NULL) {
        fprintf(STDERR, "sh: %s: out of memory\n", entry);
        return -1;
    }
    memcpy(copy, entry, size);
    free(v->entry);
    v->entry = copy;
    v->exported |= export;
    return 0;
}

/* Sets the shell's variables to command `c`'s assignments, exporting them
 * if `export`. */
static void assign_all(const struct cmd *c, int export) {
    for (int i = 0; i < c->nassigns; i++) {
        set_var(c->assigns[i], name_length(c->assigns[i], 0), export);
    }
}

/* The value of `name` for command `c`: its own assignment, or the shell's
 * variable, or NULL if neither has one. */
static const char *lookup(const struct cmd *c, const char *name) {
    int len = (int)strlen(name);
    for (int i = c->nassigns - 1; i >= 0; i--) {
        if (is_named(c->assigns[i], name, len)) {
            return c->assigns[i] + len + 1;
        }
    }
    struct var *v = find_var(name, len);
    return v != NULL && v->entry[len] == '=' ? v->entry + len + 1 : NULL;
}

/* The environment a program run with the `n` assignments `assigns` gets,
 * in `envp` (MAX_VARS + MAXARG + 1 entries), NULL-terminated: every
 * exported variable with a value, in the order of the shell's table, each
 * one assigned in its place with its new value, then the assigned ones
 * that are no variable. A name assigned twice takes the last value. */
static void build_env(char **assigns, int nassigns, char **envp) {
    int n = 0;
    for (int i = 0; i < MAX_VARS; i++) {
        const char *e = vars[i].entry;
        if (e != NULL && vars[i].exported && strchr(e, '=') != NULL) {
            envp[n] = vars[i].entry;
            n++;
        }
    }
    for (int i = 0; i < nassigns; i++) {
        char *a = assigns[i];
        int len = name_length(a, 0);
        int at = 0;
        while (at < n && !is_named(envp[at], a, len)) {
            at++;
        }
        envp[at] = a;
        if (at == n) {
            n++;
        }
    }
    envp[n] = NULL;
}

/* The shell's first variables: its environment's entries with a good name,
 * exported. */
static void import_env(void) {
    for (char **e = environ; e != NULL && *e != NULL; e++) {
        int len = name_length(*e, 0);
        if (len > 0) {
            set_var(*e, len, 1);
        }
    }
}

/* `export [-p] [NAME[=value]...]` (see the top). */
static void export_builtin(struct cmd *c) {
    if (c->argc == 1 || (c->argc == 2 && strcmp(c->argv[1], "-p") == 0)) {
        for (int i = 0; i < MAX_VARS; i++) {
            if (vars[i].entry != NULL && vars[i].exported) {
                printf("export %s\n", vars[i].entry);
            }
        }
        return;
    }
    for (int i = 1; i < c->argc; i++) {
        const char *a = c->argv[i];
        int len = name_length(a, 0);
        if (len == 0) {
            len = name_length(a, 1);
        }
        if (len == 0) {
            fprintf(STDERR, "sh: export: %s: bad variable name\n", a);
            continue;
        }
        set_var(a, len, 1);
    }
}

/* `unset NAME...`: the variables go, exported or not. */
static void unset_builtin(struct cmd *c) {
    for (int i = 1; i < c->argc; i++) {
        const char *name = c->argv[i];
        int len = name_length(name, 1);
        if (len == 0) {
            fprintf(STDERR, "sh: unset: %s: bad variable name\n", name);
            continue;
        }
        struct var *v = find_var(name, len);
        if (v != NULL) {
            free(v->entry);
            v->entry = NULL;
            v->exported = 0;
        }
    }
}

static int is_blank(char c) {
    return c == ' ' || c == '\t';
}

static int is_op(char c) {
    return c == '&' || c == '|' || c == '<' || c == '>';
}

/* Splits `s` into tokens in place: each word is NUL-terminated where it
 * ends (an operator right after a word is recorded before its byte is
 * overwritten). Returns how many, or -1 if there are more than `max`. */
static int tokenize(char *s, struct token *t, int max) {
    int n = 0;
    for (;;) {
        while (is_blank(*s)) {
            *s = 0;
            s++;
        }
        if (*s == 0) {
            return n;
        }
        if (n == max) {
            return -1;
        }
        if (is_op(*s)) {
            t[n].op = *s;
            t[n].word = NULL;
            *s = 0;
            s++;
        } else {
            t[n].op = 0;
            t[n].word = s;
            while (*s != 0 && !is_blank(*s) && !is_op(*s)) {
                s++;
            }
        }
        n++;
    }
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
    printf("cd [DIR]        change the working directory, to HOME (or /) without DIR\n");
    printf("exec [PROG ...] replace the shell with PROG; without one, keep its redirections\n");
    printf("export [NAME..] pass variables (NAME or NAME=VALUE) to programs; alone, list them\n");
    printf("unset NAME...   remove variables\n");
    printf("NAME=VALUE      set a variable; before a command, for that command alone\n");
    printf("PROG [ARG...]   run /bin/PROG (e.g. ls, cat, echo, wc, sh), or the program at PROG if it has a /\n");
    printf("CMD < FILE      read the input from FILE\n");
    printf("CMD > FILE      write the output to FILE\n");
    printf("CMD | CMD       send the first command's output to the second one\n");
    printf("CMD & CMD       run the first command in the background\n");
}

/* Whether command `c` is one of the built-ins builtin() runs. `exec` is
 * not: run_job deals with it before anything else. */
static int is_builtin(const struct cmd *c) {
    if (c->argc == 0 || c->program_only) {
        return 0;
    }
    const char *name = c->argv[0];
    return strcmp(name, "exit") == 0 || strcmp(name, "help") == 0 || strcmp(name, "wait") == 0 ||
           strcmp(name, "cd") == 0 || strcmp(name, "export") == 0 || strcmp(name, "unset") == 0;
}

/* Runs built-in `c`. Returns 1 if the shell should exit, with the status in
 * *exit_status. */
static int builtin(struct cmd *c, int *exit_status) {
    const char *name = c->argv[0];
    /* POSIX's special built-ins keep the assignments before them; the
     * others see them (lookup()) but leave the shell's variables be. */
    if (strcmp(name, "exit") == 0 || strcmp(name, "export") == 0 || strcmp(name, "unset") == 0) {
        assign_all(c, 0);
    }
    if (strcmp(name, "exit") == 0) {
        *exit_status = c->argv[1] != NULL ? atoi(c->argv[1]) : last_status;
        return 1;
    }
    if (strcmp(name, "help") == 0) {
        help();
    } else if (strcmp(name, "wait") == 0) {
        wait_all();
    } else if (strcmp(name, "export") == 0) {
        export_builtin(c);
    } else if (strcmp(name, "unset") == 0) {
        unset_builtin(c);
    } else if (strcmp(name, "cd") == 0) {
        /* Without a directory, HOME (POSIX's), or the root if it has none. */
        const char *home = lookup(c, "HOME");
        const char *dir = c->argv[1] != NULL ? c->argv[1] : home != NULL && home[0] != 0 ? home : "/";
        int r = chdir(dir);
        if (r < 0) {
            fprintf(STDERR, "sh: cd: %s: %s\n", dir, strerror(-r));
        }
    }
    return 0;
}

/* The tokens t[0..n) as a command in *c. Returns 0, or -1 (having said why)
 * if they make no sense. */
static int parse_cmd(struct token *t, int n, struct cmd *c) {
    c->nassigns = 0;
    c->argc = 0;
    c->in = NULL;
    c->out = NULL;
    c->program_only = 0;
    for (int i = 0; i < n; i++) {
        if (t[i].op == 0 && c->argc == 0 && name_length(t[i].word, 0) > 0) {
            /* An assignment: a NAME=value before the first other word. */
            if (c->nassigns == MAXARG) {
                fprintf(STDERR, "sh: too many assignments\n");
                return -1;
            }
            c->assigns[c->nassigns] = t[i].word;
            c->nassigns++;
            continue;
        }
        if (t[i].op == 0) {
            if (c->argc == MAXARG) {
                fprintf(STDERR, "sh: too many arguments\n");
                return -1;
            }
            c->argv[c->argc] = t[i].word;
            c->argc++;
            continue;
        }
        /* '<' or '>': the next token is the file. */
        if (i + 1 == n || t[i + 1].op != 0) {
            fprintf(STDERR, "sh: %c with no file\n", t[i].op);
            return -1;
        }
        if (t[i].op == '<') {
            c->in = t[i + 1].word;
        } else {
            c->out = t[i + 1].word;
        }
        i++;
    }
    c->argv[c->argc] = NULL;
    if (c->argc == 0 && c->nassigns == 0) {
        fprintf(STDERR, "sh: a command with no program\n");
        return -1;
    }
    return 0;
}

/* The path program `name` runs: /bin/NAME, or NAME itself if it has a '/'.
 * `buf` holds MAXPATH + 2 bytes: a name too long for a path still makes one
 * too long, which spawn refuses. */
static const char *program_path(const char *name, char *buf) {
    if (strchr(name, '/') != NULL) {
        return name;
    }
    const char *bin = "/bin/";
    int n = 0;
    while (bin[n] != 0) {
        buf[n] = bin[n];
        n++;
    }
    for (int i = 0; name[i] != 0 && n <= MAXPATH; i++) {
        buf[n] = name[i];
        n++;
    }
    buf[n] = 0;
    return buf;
}

/* Makes descriptor `fd` (0 or 1) a copy of `from`. */
static void put_at(int fd, int from) {
    close(fd);
    /* The lowest free descriptor: `fd`, just closed. */
    dup(from);
}

/* Opens command `c`'s redirections: *in and *out are the descriptors, or
 * left alone if it has none. Returns 0, or -1 having said why. */
static int open_files(struct cmd *c, int *in, int *out) {
    if (c->in != NULL) {
        int fd = open(c->in, O_RDONLY);
        if (fd < 0) {
            fprintf(STDERR, "sh: %s: %s\n", c->in, strerror(-fd));
            return -1;
        }
        *in = fd;
    }
    if (c->out != NULL) {
        int fd = open(c->out, O_WRONLY | O_CREAT | O_TRUNC);
        if (fd < 0) {
            fprintf(STDERR, "sh: %s: %s\n", c->out, strerror(-fd));
            if (c->in != NULL) {
                close(*in);
            }
            return -1;
        }
        *out = fd;
    }
    return 0;
}

static void close_if(int fd) {
    if (fd >= 0) {
        close(fd);
    }
}

/* Whether spawn's or exec's error `err` means there is no program `name`:
 * POSIX's status 127, where 126 is for one that is there but can't be run.
 * A NAME is looked up in /bin, as bash looks a command up along PATH, where
 * any lookup that fails finds nothing: -ENOENT, -ENOTDIR (/bin is not a
 * directory) and -ENAMETOOLONG (a NAME longer than a file name can be). A
 * path is run as it is, as bash runs one, and is missing only for -ENOENT:
 * -ENOTDIR (a name on the way is a file) is 126, bash's "Not a directory". */
static int not_found(const char *name, int err) {
    if (strchr(name, '/') != NULL) {
        return err == -ENOENT;
    }
    return err == -ENOENT || err == -ENOTDIR || err == -ENAMETOOLONG;
}

/* Says why program `name` can't be started: spawn's or exec's error `err`.
 * A NAME that isn't in /bin is the usual mistake: "no such program". */
static void cannot_start(const char *name, int err) {
    int bare = strchr(name, '/') == NULL;
    const char *why = bare && not_found(name, err) ? "no such program" : strerror(-err);
    fprintf(STDERR, "sh: %s: %s\n", name, why);
}

/* Looks at what the input is (see the top). */
static void look_at_input(void) {
    struct stat st;
    int got_stat = fstat(STDIN, &st) == 0;
    console_input = got_stat && st.type == T_DEVICE;
    connection_input = got_stat && st.type == T_SOCKET;
}

/* `exec` failed with `status`: the last status, and, unless the shell is
 * interactive, its end (see the top). Returns 1 if the shell should exit,
 * with the status in *exit_status. */
static int exec_failed(int status, int *exit_status) {
    last_status = status;
    if (interactive) {
        return 0;
    }
    *exit_status = status;
    return 1;
}

/* A copy of descriptor `fd` from SAVED_FD up that closes on exec, or -1
 * having said why there is none. */
static int save_fd(int fd) {
    int copy = fcntl(fd, F_DUPFD_CLOEXEC, SAVED_FD);
    if (copy < 0) {
        fprintf(STDERR, "sh: exec: cannot keep descriptor %d: %s\n", fd, strerror(-copy));
    }
    return copy;
}

/* Puts descriptor `fd` back from save_fd()'s `copy`, if there is one, and
 * closes the copy. */
static void restore_fd(int fd, int copy) {
    if (copy >= 0) {
        put_at(fd, copy);
        close(copy);
    }
}

/* `exec [PROG [ARG...]]`, alone in the foreground (see the top). Returns 1
 * if the shell should exit, with the status in *exit_status: a shell that
 * is not interactive, where PROG could not be started or a redirection
 * could not be made. */
static int exec_builtin(struct cmd *c, int *exit_status) {
    /* A special built-in: the assignments are the shell's from now on, and
     * exported for the program, as dash and bash have them. */
    assign_all(c, c->argc > 1);
    int in = -1;
    int out = -1;
    if (open_files(c, &in, &out) < 0) {
        return exec_failed(1, exit_status);
    }
    if (c->argc == 1) {
        /* No program: the redirections become the shell's own. */
        if (in >= 0) {
            put_at(STDIN, in);
            close(in);
        }
        if (out >= 0) {
            put_at(STDOUT, out);
            close(out);
        }
        look_at_input();
        return 0;
    }
    /* The shell's own, kept where the program won't get them. */
    int saved_in = in >= 0 ? save_fd(STDIN) : -1;
    int saved_out = out >= 0 ? save_fd(STDOUT) : -1;
    if ((in >= 0 && saved_in < 0) || (out >= 0 && saved_out < 0)) {
        close_if(in);
        close_if(out);
        close_if(saved_in);
        close_if(saved_out);
        return exec_failed(1, exit_status);
    }
    if (in >= 0) {
        put_at(STDIN, in);
        close(in);
    }
    if (out >= 0) {
        put_at(STDOUT, out);
        close(out);
    }
    char path[MAXPATH + 2];
    /* The shell's environment, the assignments in it already. */
    char *envp[MAX_VARS + MAXARG + 1];
    build_env(c->assigns, 0, envp);
    int err = exec(program_path(c->argv[1], path), &c->argv[1], envp);
    /* Still here: the program couldn't start, and the copies are open. */
    restore_fd(STDIN, saved_in);
    restore_fd(STDOUT, saved_out);
    cannot_start(c->argv[1], err);
    return exec_failed(not_found(c->argv[1], err) ? 127 : 126, exit_status);
}

/* Runs the pipeline cmds[0..n), in the background or not. Returns 1 if the
 * shell should exit (the `exit` built-in, or a failed `exec`), with the
 * status in *exit_status. */
static int run_job(struct cmd *cmds, int n, int background, int *exit_status) {
    if (n == 1 && !background && cmds[0].argc == 0) {
        /* Assignments alone: the shell's variables (see the top), and the
         * status 0, as POSIX has it. Their redirections are made below, as
         * for `exec` alone (in a pipeline or in the background, a subshell
         * would make the assignments, and they go with it). */
        assign_all(&cmds[0], 0);
        if (cmds[0].in == NULL && cmds[0].out == NULL) {
            last_status = 0;
            return 0;
        }
    }
    if (n == 1 && !background && cmds[0].argc > 0 && strcmp(cmds[0].argv[0], "exec") == 0) {
        return exec_builtin(&cmds[0], exit_status);
    }
    /* In a pipeline or a background job, which POSIX runs in subshells,
     * `exec` replaces the command's subshell, which here is the command
     * itself. So one `exec` goes: `exec NAME ...` runs the program NAME and
     * never a built-in (`exec cd /bin &` is /bin/cd, `exec exec echo &` is
     * /bin/exec), and `exec` alone is left with no words, a command that
     * only makes its redirections and ends with 0, as its subshell would. */
    for (int i = 0; i < n; i++) {
        struct cmd *c = &cmds[i];
        if (c->argc > 0 && strcmp(c->argv[0], "exec") == 0) {
            /* argv[argc] is the NULL, which moves down too. */
            for (int k = 0; k < c->argc; k++) {
                c->argv[k] = c->argv[k + 1];
            }
            c->argc--;
            c->program_only = 1;
        }
    }
    if (n == 1 && is_builtin(&cmds[0]) && cmds[0].in == NULL && cmds[0].out == NULL) {
        return builtin(&cmds[0], exit_status);
    }
    for (int i = 0; i < n; i++) {
        if (n > 1 && is_builtin(&cmds[i])) {
            fprintf(STDERR, "sh: %s: a built-in can't be in a pipeline\n", cmds[i].argv[0]);
            return 0;
        }
    }
    /* The shell's own input and output, kept above 2 meanwhile. */
    int console_in = dup(STDIN);
    int console_out = dup(STDOUT);
    int pids[MAX_PIPE];
    const char *names[MAX_PIPE];
    int started = 0;
    /* Whether the last command is an `exec` with no program, or
     * assignments alone, whose status, 0, is the pipeline's. */
    int ends_with_exec = 0;
    int quit = 0;
    /* The read end of the pipe from the previous command, for this one. */
    int from_prev = -1;
    for (int i = 0; i < n; i++) {
        int in = from_prev;
        int out = -1;
        from_prev = -1;
        if (i + 1 < n) {
            int p[2];
            int r = pipe(p);
            if (r < 0) {
                fprintf(STDERR, "sh: pipe: %s\n", strerror(-r));
                close_if(in);
                break;
            }
            out = p[1];
            from_prev = p[0];
        }
        /* A redirection wins over the pipe: then the pipe's end goes
         * unused (and the next command reads nothing from it). */
        int file_in = -1;
        int file_out = -1;
        if (open_files(&cmds[i], &file_in, &file_out) < 0) {
            close_if(in);
            close_if(out);
            break;
        }
        if (file_in >= 0) {
            close_if(in);
            in = file_in;
        }
        if (file_out >= 0) {
            close_if(out);
            out = file_out;
        }
        if (cmds[i].argc == 0) {
            /* `exec` alone, or assignments: its files are made, and that
             * is all. Nothing reads or writes the pipe ends it had. */
            close_if(in);
            close_if(out);
            ends_with_exec = i + 1 == n;
            continue;
        }
        put_at(STDIN, in >= 0 ? in : console_in);
        put_at(STDOUT, out >= 0 ? out : console_out);
        close_if(in);
        close_if(out);
        if (is_builtin(&cmds[i])) {
            /* Alone, with redirections (see above). */
            quit = builtin(&cmds[i], exit_status);
            break;
        }
        char path[MAXPATH + 2];
        char *envp[MAX_VARS + MAXARG + 1];
        build_env(cmds[i].assigns, cmds[i].nassigns, envp);
        int pid = spawn(program_path(cmds[i].argv[0], path), cmds[i].argv, envp);
        if (pid < 0) {
            cannot_start(cmds[i].argv[0], pid);
            break;
        }
        pids[started] = pid;
        names[started] = cmds[i].argv[0];
        started++;
    }
    close_if(from_prev);
    put_at(STDIN, console_in);
    put_at(STDOUT, console_out);
    close(console_in);
    close(console_out);

    for (int i = 0; i < started; i++) {
        const char *name = names[i];
        if (background) {
            printf("[%d]\n", pids[i]);
            remember(pids[i], name);
            continue;
        }
        int status;
        if (wait(pids[i], &status) == pids[i]) {
            last_status = status;
            if (status != 0) {
                printf("%s: exit %d\n", name, status);
            }
        }
    }
    if (ends_with_exec && !background) {
        last_status = 0;
    }
    return quit;
}

/* Runs a line: its '&'-separated jobs, in order. Returns 1 if the shell
 * should exit. */
static int run_line(char *line, int *exit_status) {
    struct token t[MAX_TOKENS];
    int n = tokenize(line, t, MAX_TOKENS);
    if (n < 0) {
        fprintf(STDERR, "sh: too many words\n");
        return 0;
    }
    int start = 0;
    while (start < n) {
        /* The job: up to the next '&' or the end. */
        int end = start;
        while (end < n && t[end].op != '&') {
            end++;
        }
        struct cmd cmds[MAX_PIPE];
        int ncmds = 0;
        int ok = end > start;
        if (!ok) {
            fprintf(STDERR, "sh: & with no command\n");
        }
        for (int c = start; ok && c < end;) {
            int stop = c;
            while (stop < end && t[stop].op != '|') {
                stop++;
            }
            if (ncmds == MAX_PIPE) {
                fprintf(STDERR, "sh: too many commands in a pipeline\n");
                ok = 0;
            } else if (parse_cmd(&t[c], stop - c, &cmds[ncmds]) < 0) {
                ok = 0;
            } else {
                ncmds++;
                if (stop < end && stop + 1 == end) {
                    fprintf(STDERR, "sh: | with no command\n");
                    ok = 0;
                }
            }
            c = stop + 1;
        }
        if (!ok) {
            return 0;
        }
        if (run_job(cmds, ncmds, end < n, exit_status)) {
            return 1;
        }
        start = end + 1;
    }
    return 0;
}

/* Reads a line of input that isn't the console's into `buf` (LINE_SIZE
 * bytes), a byte at a time, without its newline (and a '\r' before it).
 * A longer line is read to its end, said to be too long, and left out.
 * Returns its length, or -1 at the end of the input (or an error, a reset
 * connection) before any byte of a line. */
static int read_line(char *buf) {
    int n = 0;
    int too_long = 0;
    for (;;) {
        char c;
        if (read(STDIN, &c, 1) != 1) {
            if (n == 0 && !too_long) {
                return -1;
            }
            break; /* the last line, without its newline */
        }
        if (c == '\n') {
            break;
        }
        if (n == LINE_SIZE - 1) {
            too_long = 1;
        } else {
            buf[n] = c;
            n++;
        }
    }
    if (too_long) {
        fprintf(STDERR, "sh: line too long\n");
        n = 0;
    }
    if (n > 0 && buf[n - 1] == '\r') {
        n--;
    }
    buf[n] = 0;
    return n;
}

int main(void) {
    char line[LINE_SIZE];
    import_env();
    look_at_input();
    interactive = console_input || connection_input;
    printf("Type help for the list of commands.\n");
    for (;;) {
        printf(PROMPT);
        int n = console_input ? getline(line, sizeof line) : read_line(line);
        if (n < 0) {
            if (connection_input) {
                kill_descendants(getpid()); /* the hang-up: see the top */
            }
            return last_status;
        }
        int status;
        if (run_line(line, &status)) {
            return status;
        }
    }
}
