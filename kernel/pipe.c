/* Pipes (pipe.h), after xv6's pipe.c, without its spinlock: kernel code is
 * never preempted, so everything between two sleeps runs alone, and a
 * reader's check that the pipe is empty and its sleep can't miss a writer's
 * wakeup.
 *
 * The buffer is a ring: `nread` and `nwrite` count the bytes ever read and
 * written, so the pipe holds nwrite - nread bytes, from data[nread %
 * PIPESIZE] on. They wrap at 2^32, a multiple of PIPESIZE, and the
 * differences stay right. Readers sleep on &nread, writers on &nwrite. */
#include "pipe.h"
#include "kalloc.h"
#include "memlayout.h"
#include "proc.h"

struct pipe {
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
            (*rf)->ref = 0; /* nothing else set: just give the slot back */
        }
        kfree(p);
        return -ENFILE;
    }
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
    if (writable) {
        p->writeopen = false;
        wakeup(&p->nread); /* readers now see the end */
    } else {
        p->readopen = false;
        wakeup(&p->nwrite); /* writers now fail */
    }
    if (!p->readopen && !p->writeopen) {
        kfree(p);
    }
}

int pipewrite(struct pipe *p, pagetable_t pt, u32 src, u32 n) {
    u32 done = 0;
    while (done < n) {
        if (!p->readopen) {
            /* Nobody will ever read the rest. */
            return done > 0 ? (int)done : -EPIPE;
        }
        u32 used = p->nwrite - p->nread;
        if (used == PIPESIZE) {
            /* Full: let the readers at what is there, and wait for room. */
            wakeup(&p->nread);
            sleep(&p->nwrite);
            continue;
        }
        /* As much as fits before the end of the ring. */
        u32 at = p->nwrite % PIPESIZE;
        u32 m = MIN(MIN(n - done, PIPESIZE - used), PIPESIZE - at);
        copyin(pt, p->data + at, src + done, m);
        p->nwrite += m;
        done += m;
    }
    wakeup(&p->nread);
    return (int)done;
}

int piperead(struct pipe *p, pagetable_t pt, u32 dst, u32 n) {
    while (p->nread == p->nwrite && p->writeopen) {
        sleep(&p->nread);
    }
    u32 count = 0;
    while (count < n && p->nread != p->nwrite) {
        u32 at = p->nread % PIPESIZE;
        u32 m = MIN(MIN(n - count, p->nwrite - p->nread), PIPESIZE - at);
        copyout(pt, dst + count, p->data + at, m);
        p->nread += m;
        count += m;
    }
    /* Room for the writers. */
    wakeup(&p->nwrite);
    return (int)count;
}

u32 pipe_bytes(const struct pipe *p) {
    return p->nwrite - p->nread;
}
