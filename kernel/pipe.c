/* Pipes (pipe.h), after xv6's pipe.c.
 *
 * The buffer is a ring: `nread` and `nwrite` count the bytes ever read and
 * written, so the pipe holds nwrite - nread bytes, from data[nread %
 * PIPESIZE] on. They wrap at 2^32, a multiple of PIPESIZE, and the
 * differences stay right. Readers sleep on &nread, writers on &nwrite.
 *
 * Locking: each pipe's spinlock protects all of it. Readers and writers
 * check the ring and sleep with it, so a reader's check that the pipe is
 * empty and its sleep can't miss a writer's wakeup. Copying from and to
 * user memory happens under it: copyin() and copyout() walk the page table
 * and never sleep. */
#include "pipe.h"
#include "kalloc.h"
#include "memlayout.h"
#include "poll.h"
#include "proc.h"
#include "spinlock.h"

struct pipe {
    struct spinlock lock;
    char data[PIPESIZE];
    u32 nread;
    u32 nwrite;
    bool readopen;  /* the read end's file is open */
    bool writeopen; /* the write end's file is open */
};

/* The struct takes a page of its own. */
_Static_assert(sizeof(struct pipe) <= PGSIZE, "a pipe fits a page");

#define MIN(a, b) ((a) < (b) ? (a) : (b))

int pipealloc(struct file **rf, struct file **wf) {
    struct pipe *p = kalloc();
    if (p == NULL) {
        return -ENOMEM;
    }
    *rf = filealloc();
    *wf = *rf != NULL ? filealloc() : NULL;
    if (*wf == NULL) {
        if (*rf != NULL) {
            fileclose(*rf); /* nothing else set: just gives the slot back */
        }
        kfree(p);
        return -ENFILE;
    }
    initlock(&p->lock, "pipe");
    p->nread = 0;
    p->nwrite = 0;
    p->readopen = true;
    p->writeopen = true;
    (*rf)->type = FD_PIPE;
    (*rf)->readable = true;
    (*rf)->pipe = p;
    (*wf)->type = FD_PIPE;
    (*wf)->writable = true;
    (*wf)->pipe = p;
    return 0;
}

void pipeclose(struct pipe *p, bool writable) {
    acquire(&p->lock);
    if (writable) {
        p->writeopen = false;
        wakeup(&p->nread); /* readers now see the end */
    } else {
        p->readopen = false;
        wakeup(&p->nwrite); /* writers now fail */
    }
    bool gone = !p->readopen && !p->writeopen;
    release(&p->lock);
    if (gone) {
        kfree(p);
    }
}

isize pipewrite(struct pipe *p, pagetable_t pt, uintptr src, usize n) {
    usize done = 0;
    acquire(&p->lock);
    while (done < n) {
        if (!p->readopen) {
            /* Nobody will ever read the rest. */
            release(&p->lock);
            return done > 0 ? (isize)done : -EPIPE;
        }
        u32 used = p->nwrite - p->nread;
        if (used == PIPESIZE) {
            if (proc_killed()) {
                break; /* the caller ends on its way out */
            }
            /* Full: let the readers at what is there, and wait for room. */
            wakeup(&p->nread);
            sleep(&p->nwrite, &p->lock);
            continue;
        }
        /* As much as fits before the end of the ring. */
        u32 at = p->nwrite % PIPESIZE;
        u32 m = (u32)MIN(MIN(n - done, PIPESIZE - used), PIPESIZE - at);
        /* The caller checked the buffer, but another thread of the process
         * may have unmapped part of it since (sbrk): what is left of the
         * write fails, and the reader never sees what wasn't copied. */
        if (copyin(pt, p->data + at, src + done, m) != 0) {
            wakeup(&p->nread);
            release(&p->lock);
            return done > 0 ? (isize)done : -EFAULT;
        }
        p->nwrite += m;
        done += m;
    }
    wakeup(&p->nread);
    release(&p->lock);
    return (isize)done;
}

isize piperead(struct pipe *p, pagetable_t pt, uintptr dst, usize n) {
    acquire(&p->lock);
    while (p->nread == p->nwrite && p->writeopen && !proc_killed()) {
        sleep(&p->nread, &p->lock);
    }
    usize count = 0;
    while (count < n && p->nread != p->nwrite) {
        u32 at = p->nread % PIPESIZE;
        u32 m = (u32)MIN(MIN(n - count, p->nwrite - p->nread), PIPESIZE - at);
        /* As in pipewrite: bytes that couldn't be copied stay in the pipe. */
        if (copyout(pt, dst + count, p->data + at, m) != 0) {
            if (count == 0) {
                release(&p->lock);
                return -EFAULT;
            }
            break;
        }
        p->nread += m;
        count += m;
    }
    /* Room for the writers. */
    wakeup(&p->nwrite);
    release(&p->lock);
    return (isize)count;
}

u32 pipe_poll(struct pipe *p, bool writable, struct poll_table *pt) {
    /* Readers sleep on &nread, which writes and the write end's close
     * wake; writers on &nwrite, which reads and the read end's close
     * wake. */
    poll_wait(pt, writable ? (const void *)&p->nwrite : (const void *)&p->nread);
    acquire(&p->lock);
    u32 mask = 0;
    if (writable) {
        if (p->nwrite - p->nread < PIPESIZE) {
            mask |= POLL_READY_OUT;
        }
        if (!p->readopen) {
            mask |= POLLERR;
        }
    } else {
        if (p->nwrite != p->nread) {
            mask |= POLL_READY_IN;
        }
        if (!p->writeopen) {
            mask |= POLLHUP;
        }
    }
    release(&p->lock);
    return mask;
}

u32 pipe_bytes(struct pipe *p) {
    acquire(&p->lock);
    u32 n = p->nwrite - p->nread;
    release(&p->lock);
    return n;
}
