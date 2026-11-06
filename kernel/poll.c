/* poll() (poll.h): every descriptor's file asked what it is ready for,
 * through its poll hook (filepoll, file.c), until one is ready for what
 * was asked, the time runs out or the process is killed.
 *
 * Each scan asks every file, and each hook names its channels in the
 * caller's poll table before it looks; when nothing is ready the thread
 * sleeps on the table (poll_sleep, proc.c), and any wakeup() of one of its
 * channels since the scan began, or from then on, has it scan again. The
 * timeout is counted in ticks, as sleep() counts them: the tick counter is
 * one of the channels, so each tick wakes the thread to look at the time.
 *
 * The files are held (fd_get) for the whole call, so a descriptor another
 * thread closes meanwhile goes on being polled, as on Linux. */
#include "poll.h"
#include "file.h"
#include "proc.h"
#include "timer.h"
#include "trap.h"
#include "vm.h"

long sys_poll(uintptr ufds, u64 nfds, long timeout) {
    if (nfds > POLL_MAX_FDS) {
        return -EINVAL;
    }
    pagetable_t pt = myproc()->pagetable;
    usize size = (usize)nfds * sizeof(struct pollfd);
    struct pollfd fds[POLL_MAX_FDS];
    /* The array is written back at the end: writable user memory. */
    if (!uvm_range_ok(pt, ufds, size, PTE_W) || copyin(pt, fds, ufds, size) != 0) {
        return -EFAULT;
    }
    struct file *files[POLL_MAX_FDS];
    for (u32 i = 0; i < nfds; i++) {
        files[i] = fds[i].fd >= 0 ? fd_get((u32)fds[i].fd) : NULL;
    }
    /* Milliseconds to ticks, rounded up; Linux's timeout is an int. */
    int ms = (int)timeout;
    u32 wait = ms > 0 ? (u32)(((u64)ms * TICK_HZ + 999) / 1000) : 0;
    u32 start = ticks;

    struct poll_table table;
    poll_begin(&table);
    long count;
    for (;;) {
        if (ms > 0) {
            poll_wait(&table, (const void *)&ticks);
        }
        count = 0;
        for (u32 i = 0; i < nfds; i++) {
            u32 mask = 0;
            if (fds[i].fd >= 0) {
                if (files[i] == NULL) {
                    mask = POLLNVAL;
                } else {
                    u32 want = (u32)(u16)fds[i].events | POLLERR | POLLHUP;
                    mask = filepoll(files[i], &table) & want;
                }
            }
            fds[i].revents = (i16)mask;
            if (mask != 0) {
                count++;
            }
        }
        if (count > 0 || ms == 0 || (ms > 0 && ticks - start >= wait)) {
            break;
        }
        if (proc_killed()) {
            count = -EINTR;
            break;
        }
        poll_sleep(&table);
    }
    poll_end();
    for (u32 i = 0; i < nfds; i++) {
        if (files[i] != NULL) {
            fileclose(files[i]);
        }
    }
    if (count >= 0 && copyout(pt, ufds, fds, size) != 0) {
        return -EFAULT;
    }
    return count;
}
