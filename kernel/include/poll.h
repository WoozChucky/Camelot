/* poll() (docs/camelot-syscalls.md, "Polling"; spec section 10, "Waiting on
 * several channels"): a thread waits for any of several open files to be
 * ready, after Linux's poll_table.
 *
 * Each kind of file has a poll hook (filepoll(), file.c), which says what
 * the file is ready for now, the POLL bits below, and names the channels
 * whose wakeup() means that may have changed: the ones its readers and
 * writers sleep on. poll_wait() adds a channel to the caller's table, and
 * from then on a wakeup() of that channel finds the thread (proc.c's
 * wakeup_locked): asleep in poll, it runs again; still checking its files,
 * it is marked `woken`, so it checks them all again rather than sleeping.
 * Since a hook names its channels before it looks at the file, under the
 * lock the file's changes are made under, a change either comes before the
 * look, which sees it, or wakes the table: none is lost, as none is for
 * sleep(). */
#ifndef CAMELOT_POLL_H
#define CAMELOT_POLL_H

#include "types.h"

/* The events (Linux's values): what `events` asks for and `revents` says.
 * POLLERR, POLLHUP and POLLNVAL are reported whether asked for or not. */
#define POLLIN 0x001     /* a read would not wait */
#define POLLPRI 0x002    /* never set */
#define POLLOUT 0x004    /* a write would not wait */
#define POLLERR 0x008    /* an error is waiting, or the reading end is gone */
#define POLLHUP 0x010    /* the other end is gone: nothing more will come */
#define POLLNVAL 0x020   /* not an open descriptor */
#define POLLRDNORM 0x040 /* as POLLIN */
#define POLLRDBAND 0x080 /* never set */
#define POLLWRNORM 0x100 /* as POLLOUT */
#define POLLWRBAND 0x200 /* never set */
#define POLLRDHUP 0x2000 /* a stream socket's peer will send no more */

/* What the hooks say, as Linux's do: the normal bits with the plain ones. */
#define POLL_READY_IN (POLLIN | POLLRDNORM)
#define POLL_READY_OUT (POLLOUT | POLLWRNORM)

/* Linux's struct pollfd: 8 bytes. A negative `fd` is skipped (its
 * `revents` 0). */
struct pollfd {
    int fd;
    i16 events;
    i16 revents;
};

_Static_assert(sizeof(struct pollfd) == 8, "Linux's struct pollfd");

/* The most descriptors one poll() takes: Linux's limit is the process's
 * RLIMIT_NOFILE; here a descriptor may appear several times, but no more
 * than this in all (-EINVAL). */
#define POLL_MAX_FDS 64

/* A hook names two channels at most (a stream socket: its own and its
 * peer's), and the timeout one more: the tick counter. */
#define POLL_CHANS (2 * POLL_MAX_FDS + 1)

/* A thread's poll table, on its kernel stack while it is in poll() (the
 * thread's `poll`, proc.h). Its fields are changed and read under
 * proc_lock. */
struct poll_table {
    u32 n;     /* channels in chans[] */
    bool woken; /* a wakeup() of one of them came since the scan began */
    const void *chans[POLL_CHANS];
};

/* Adds `chan` to `pt` (once): a hook calls it before it looks at its file.
 * Takes proc_lock, so the caller may hold any lock a sleeper holds. */
void poll_wait(struct poll_table *pt, const void *chan);

/* poll(fds, nfds, timeout): `timeout` milliseconds (rounded up to ticks;
 * negative: no limit, 0: no wait). The number of descriptors with events,
 * 0 if the time ran out, or -EINVAL (nfds over POLL_MAX_FDS), -EFAULT,
 * -EINTR (the process was killed while it waited). */
long sys_poll(uintptr ufds, u64 nfds, long timeout);

#endif
