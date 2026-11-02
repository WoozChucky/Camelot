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
 * Built-ins: help, exit [N], wait, cd [DIR], and for the network (until
 * #85 makes them programs) ifconfig [ADDR [MASK [GATEWAY]]] and ping ADDR
 * [N]: `ping` with no address runs /bin/ping. They run in the shell itself,
 * with its redirections, but can't be part of a pipeline. The shell ends
 * with `exit`, or at the end of the input (Ctrl-D on an empty line), with
 * the status N or else the last foreground program's; the kernel then stops
 * the machine with it. */
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

/* A token of a line: a word, or an operator ('&', '|', '<', '>'). */
struct token {
    char op; /* 0 for a word */
    char *word;
};

/* A command: its words (NULL-terminated) and where its input comes from
 * and its output goes, if not from the shell or the pipeline. */
struct cmd {
    char *argv[MAXARG + 1];
    int argc;
    char *in;
    char *out;
};

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
    printf("cd [DIR]        change the working directory, to / without DIR\n");
    printf("ifconfig        show the network interface; ifconfig ADDR [MASK [GATEWAY]] sets its address\n");
    printf("ping ADDR [N]   send N (4) echo requests to ADDR, a.b.c.d, and wait for the replies\n");
    printf("PROG [ARG...]   run /bin/PROG (e.g. ls, cat, echo, wc, sh), or the program at PROG if it has a /\n");
    printf("CMD < FILE      read the input from FILE\n");
    printf("CMD > FILE      write the output to FILE\n");
    printf("CMD | CMD       send the first command's output to the second one\n");
    printf("CMD & CMD       run the first command in the background\n");
}

/* Whether command `c` is a built-in. `ping` is one only with an address:
 * alone, it is still /bin/ping, the program that takes turns with pong. */
static int is_builtin(const struct cmd *c) {
    const char *name = c->argv[0];
    return strcmp(name, "exit") == 0 || strcmp(name, "help") == 0 || strcmp(name, "wait") == 0 ||
           strcmp(name, "cd") == 0 || strcmp(name, "ifconfig") == 0 ||
           (strcmp(name, "ping") == 0 && c->argc > 1);
}

/* ---- the network built-ins, until #85 makes them programs ---- */

/* Reads "a.b.c.d" into *addr. Returns 0, or -1 if `s` is not an address. */
static int parse_addr(const char *s, uint *addr) {
    uint a = 0;
    for (int part = 0; part < 4; part++) {
        uint n = 0;
        int digits = 0;
        while (*s >= '0' && *s <= '9' && digits < 4) {
            n = n * 10 + (uint)(*s - '0');
            s++;
            digits++;
        }
        if (digits == 0 || digits > 3 || n > 255) {
            return -1;
        }
        a = (a << 8) | n;
        if (part < 3) {
            if (*s != '.') {
                return -1;
            }
            s++;
        }
    }
    if (*s != 0) {
        return -1;
    }
    *addr = a;
    return 0;
}

static void print_addr(uint a) {
    printf("%u.%u.%u.%u", a >> 24, (a >> 16) & 255, (a >> 8) & 255, a & 255);
}

/* What a network call's error means to the user. */
static const char *net_error(int err) {
    return err == ENODEV ? "no network card" : strerror(err);
}

/* `ifconfig ADDR [MASK [GATEWAY]]`: a static address (MASK 255.255.255.0
 * and no gateway if left out). */
static void ifconfig_set(struct cmd *c) {
    uint addr = 0;
    uint mask = 0xffffff00;
    uint gateway = 0;
    for (int i = 1; i < c->argc && i <= 3; i++) {
        uint *dst = i == 1 ? &addr : i == 2 ? &mask : &gateway;
        if (parse_addr(c->argv[i], dst) < 0) {
            fprintf(STDERR, "sh: ifconfig: %s: not an address\n", c->argv[i]);
            return;
        }
    }
    int r = netconf(addr, mask, gateway);
    if (r < 0) {
        fprintf(STDERR, "sh: ifconfig: %s\n", net_error(-r));
    }
}

/* `ifconfig`: the interface, its counters, and loopback. */
static void ifconfig(struct cmd *c) {
    if (c->argc > 1) {
        ifconfig_set(c);
        return;
    }
    struct netinfo n;
    int r = netinfo(&n);
    if (r < 0) {
        fprintf(STDERR, "sh: ifconfig: %s\n", net_error(-r));
        return;
    }
    printf("net0: mac %02x:%02x:%02x:%02x:%02x:%02x, link %s\n", n.mac[0], n.mac[1], n.mac[2], n.mac[3], n.mac[4],
           n.mac[5], (n.flags & NETINFO_LINK) ? "up" : "down");
    if (n.flags & NETINFO_UP) {
        printf("  inet ");
        print_addr(n.addr);
        printf(" netmask ");
        print_addr(n.mask);
        if (n.gateway != 0) {
            printf(" gateway ");
            print_addr(n.gateway);
        }
        if (n.dns != 0) {
            printf(" dns ");
            print_addr(n.dns);
        }
        printf(" (%s)\n", (n.flags & NETINFO_DHCP) ? "dhcp" : "static");
    } else {
        printf("  no address\n");
    }
    struct net_stats *s = &n.stats;
    printf("  rx %u frames, %u bytes; tx %u frames, %u bytes\n", s->rx_frames, s->rx_bytes, s->tx_frames,
           s->tx_bytes);
    printf("  rx dropped: card %u, filtered %u, ether %u, arp %u, ip %u, ip-checksum %u, fragment %u, protocol %u\n",
           s->rx_card_dropped, s->rx_filtered, s->rx_bad_ether, s->rx_bad_arp, s->rx_bad_ip, s->rx_bad_ip_sum,
           s->rx_fragment, s->rx_bad_proto);
    printf("  rx dropped: icmp %u, udp %u, udp-checksum %u, no-port %u, queue-full %u\n", s->rx_bad_icmp,
           s->rx_bad_udp, s->rx_bad_udp_sum, s->rx_no_port, s->rx_queue_full);
    printf("  other: no-buffer %u, arp-failed %u, tx-error %u\n", s->no_buffer, s->arp_failed, s->tx_errors);
    printf("lo: inet 127.0.0.1 netmask 255.0.0.0\n");
}

/* `ping ADDR [N]`: N echo requests a second apart (4 if N is left out),
 * each waiting a second for its reply, then a summary. */
static void ping_cmd(struct cmd *c) {
    uint addr;
    if (parse_addr(c->argv[1], &addr) < 0) {
        fprintf(STDERR, "sh: ping: %s: not an address\n", c->argv[1]);
        return;
    }
    int count = c->argc > 2 ? atoi(c->argv[2]) : 4;
    if (count < 1) {
        count = 1;
    }
    printf("PING %s: 56 data bytes\n", c->argv[1]);
    int received = 0;
    for (int seq = 1; seq <= count; seq++) {
        struct ping_reply r;
        int e = ping(addr, (uint)seq, &r);
        if (e == 0) {
            received++;
            printf("%u bytes from ", r.len);
            print_addr(r.from);
            printf(": seq=%d ttl=%u time=%u.%03u ms\n", seq, r.ttl, r.rtt_us / 1000, r.rtt_us % 1000);
        } else if (e == -ENODEV || e == -ENETUNREACH || e == -EINVAL) {
            /* Nothing will change by trying again. */
            fprintf(STDERR, "sh: ping: %s\n", net_error(-e));
            return;
        } else {
            printf("seq=%d: %s\n", seq, strerror(-e));
        }
        if (seq < count && e == 0) {
            sleep(10);
        }
    }
    printf("%d sent, %d received\n", count, received);
}

/* Runs built-in `c`. Returns 1 if the shell should exit, with the status in
 * *exit_status. */
static int builtin(struct cmd *c, int *exit_status) {
    const char *name = c->argv[0];
    if (strcmp(name, "exit") == 0) {
        *exit_status = c->argv[1] != NULL ? atoi(c->argv[1]) : last_status;
        return 1;
    }
    if (strcmp(name, "help") == 0) {
        help();
    } else if (strcmp(name, "wait") == 0) {
        wait_all();
    } else if (strcmp(name, "ifconfig") == 0) {
        ifconfig(c);
    } else if (strcmp(name, "ping") == 0) {
        ping_cmd(c);
    } else {
        const char *dir = c->argv[1] != NULL ? c->argv[1] : "/";
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
    c->argc = 0;
    c->in = NULL;
    c->out = NULL;
    for (int i = 0; i < n; i++) {
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
    if (c->argc == 0) {
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

/* Runs the pipeline cmds[0..n), in the background or not. Returns 1 if the
 * shell should exit (the `exit` built-in), with the status in
 * *exit_status. */
static int run_job(struct cmd *cmds, int n, int background, int *exit_status) {
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
    int started = 0;
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
        int pid = spawn(program_path(cmds[i].argv[0], path), cmds[i].argv);
        if (pid < 0) {
            /* A name that isn't in /bin is the usual mistake. */
            int bare = strchr(cmds[i].argv[0], '/') == NULL;
            int missing = pid == -ENOENT || pid == -ENAMETOOLONG;
            const char *why = bare && missing ? "no such program" : strerror(-pid);
            fprintf(STDERR, "sh: %s: %s\n", cmds[i].argv[0], why);
            break;
        }
        pids[started] = pid;
        started++;
    }
    close_if(from_prev);
    put_at(STDIN, console_in);
    put_at(STDOUT, console_out);
    close(console_in);
    close(console_out);

    for (int i = 0; i < started; i++) {
        /* The programs are cmds[0..started), in order. */
        const char *name = cmds[i].argv[0];
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
