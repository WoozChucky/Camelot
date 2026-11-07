/* kill_descendants: ends what a process started, and what that started. */
#include "../user.h"

/* Processes the snapshot holds: as many as the kernel has (NPROC, 32). */
#define MAX_PROCS 32

/* One snapshot for every caller: a program kills its tree from one thread
 * at a time. Static, so it costs the caller's one-page stack nothing. */
static struct procinfo snapshot[MAX_PROCS];

int kill_descendants(int pid) {
    int n = procs(snapshot, MAX_PROCS);
    if (n > MAX_PROCS) {
        n = MAX_PROCS;
    }
    /* `pid` and every process found to descend from it. Pids only grow and
     * procs() lists them in order, so a parent always comes before its
     * children: one pass over the list finds the whole tree. */
    int tree[MAX_PROCS + 1];
    int count = 1;
    tree[0] = pid;
    for (int i = 0; i < n; i++) {
        for (int j = 0; j < count; j++) {
            if ((int)snapshot[i].ppid == tree[j]) {
                tree[count] = (int)snapshot[i].pid;
                count++;
                break;
            }
        }
    }
    /* The youngest first: a parent killed before its children would leave
     * them orphans, with no parent left to find them by. The ended ones
     * (zombies) need nothing. */
    int killed = 0;
    for (int j = count - 1; j > 0; j--) {
        int state = 0;
        for (int i = 0; i < n; i++) {
            if ((int)snapshot[i].pid == tree[j]) {
                state = (int)snapshot[i].state;
            }
        }
        if (state != PROC_ZOMBIE && kill(tree[j]) == 0) {
            killed++;
        }
    }
    return killed;
}
