/* ps: lists the processes, in pid order, from the procs system call: a
 * header, then a line for each one, its pid, its parent's (0 for none), its
 * state, its size in bytes (the end of its heap; 0 for a process of the
 * kernel's own) and its name:
 *
 *     PID  PPID STATE      SIZE NAME
 *       1     0 sleep     16384 sh
 *       2     1 run       12288 ps
 *
 * ps itself is the one running. Exits 1 if given an argument, else 0. */
#include "user.h"

/* Process slots (NPROC in kernel/include/proc.h): procs says if there are
 * more. */
#define MAX_PROCS 8

static const char *state_name(uint state) {
    if (state == PROC_RUNNING) {
        return "run";
    }
    if (state == PROC_RUNNABLE) {
        return "ready";
    }
    if (state == PROC_SLEEPING) {
        return "sleep";
    }
    if (state == PROC_ZOMBIE) {
        return "zombie";
    }
    return "?";
}

int main(int argc, char **argv) {
    (void)argv;
    if (argc > 1) {
        fprintf(STDERR, "usage: ps\n");
        return 1;
    }
    struct procinfo info[MAX_PROCS];
    int n = procs(info, MAX_PROCS);
    if (n < 0) {
        fprintf(STDERR, "ps: %s\n", strerror(-n));
        return 1;
    }
    printf("  PID  PPID STATE      SIZE NAME\n");
    for (int i = 0; i < n && i < MAX_PROCS; i++) {
        struct procinfo *p = &info[i];
        printf("%5u %5u %-6s %8u %s\n", p->pid, p->ppid, state_name(p->state), p->size, p->name);
    }
    if (n > MAX_PROCS) {
        printf("(and %d more)\n", n - MAX_PROCS);
    }
    return 0;
}
