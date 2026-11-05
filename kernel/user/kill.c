/* kill PID...: ends each process (the kill system call): it exits with
 * status -1 before it runs any more of its own code, even from the middle
 * of a sleep or a read. The shell started it in the background with `&`
 * and printed its pid; `ps` lists them all. Exits 1 if a pid is not a
 * number (one too big for a pid included), names no process or one that
 * can't be killed, the shell (it says so and goes on), else 0. */
#include "user.h"

int main(int argc, char **argv) {
    if (argc < 2) {
        fprintf(STDERR, "usage: kill PID...\n");
        return 1;
    }
    int status = 0;
    for (int i = 1; i < argc; i++) {
        char *end;
        long pid = strtol(argv[i], &end, 10);
        /* A pid is an int: a number too big for one is not a pid, and
         * must not end up naming some other process by its low 32 bits:
         * a long has 64 bits, and anything past INT_MAX is refused (strtol
         * gives LONG_MAX for a number past a long, which is past it too). */
        if (end == argv[i] || *end != 0 || pid <= 0 || pid > INT_MAX) {
            fprintf(STDERR, "kill: %s: not a process id\n", argv[i]);
            status = 1;
            continue;
        }
        int r = kill((int)pid);
        if (r < 0) {
            fprintf(STDERR, "kill: %s: %s\n", argv[i], strerror(-r));
            status = 1;
        }
    }
    return status;
}
