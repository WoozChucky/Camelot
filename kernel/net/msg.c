/* sendmsg() and recvmsg()'s pieces (msg.h): user buffers walked as an
 * iov_iter is on Linux, and SCM_RIGHTS's descriptors taken and given. */
#include "msg.h"
#include "file.h"
#include "proc.h"
#include "string.h"
#include "trap.h"

int uio_iovec(struct uio *u, pagetable_t pt, uintptr uiov, usize n, bool out) {
    if (n > UIO_MAXIOV) {
        return -EMSGSIZE;
    }
    if (n > 0 && copyin(pt, u->iov, uiov, n * sizeof(struct iovec)) < 0) {
        return -EFAULT;
    }
    u->pt = pt;
    u->cnt = (u32)n;
    u->idx = 0;
    u->off = 0;
    u64 total = 0;
    for (u32 i = 0; i < u->cnt; i++) {
        if (!uvm_range_ok(pt, u->iov[i].iov_base, u->iov[i].iov_len, out ? PTE_W : PTE_R)) {
            return -EFAULT;
        }
        /* Checked, so each is a range of user memory: the sum of 16 of
         * them can't wrap. */
        total += u->iov[i].iov_len;
    }
    u->resid = total > MSG_MAX_LEN ? MSG_MAX_LEN : (u32)total;
    return 0;
}

void uio_single(struct uio *u, pagetable_t pt, uintptr buf, usize n) {
    u->pt = pt;
    u->iov[0].iov_base = buf;
    u->iov[0].iov_len = n;
    u->cnt = 1;
    u->idx = 0;
    u->off = 0;
    u->resid = n > MSG_MAX_LEN ? MSG_MAX_LEN : (u32)n;
}

int uio_move(struct uio *u, void *k, u32 n, bool out) {
    /* On copies, so that a failed copy leaves the uio as it was. */
    u32 idx = u->idx;
    usize off = u->off;
    u32 done = 0;
    while (done < n) {
        /* `resid` counts bytes in these buffers, so one has some left. */
        const struct iovec *v = &u->iov[idx];
        usize left = v->iov_len - off;
        if (left == 0) {
            idx++;
            off = 0;
            continue;
        }
        u32 m = n - done < left ? n - done : (u32)left;
        int r = out ? copyout(u->pt, v->iov_base + off, (u8 *)k + done, m)
                    : copyin(u->pt, (u8 *)k + done, v->iov_base + off, m);
        if (r < 0) {
            return -EFAULT;
        }
        off += m;
        done += m;
    }
    u->idx = idx;
    u->off = off;
    u->resid -= n;
    return 0;
}

int msg_get(pagetable_t pt, uintptr umsg, struct msghdr *m) {
    return copyin(pt, m, umsg, sizeof *m) < 0 ? -EFAULT : 0;
}

int msg_check_out(pagetable_t pt, uintptr umsg, const struct msghdr *m, usize addr_size) {
    if (!uvm_range_ok(pt, umsg, sizeof *m, PTE_W)) {
        return -EFAULT;
    }
    if (m->msg_name != 0) {
        usize n = m->msg_namelen < addr_size ? m->msg_namelen : addr_size;
        if (!uvm_range_ok(pt, m->msg_name, n, PTE_W)) {
            return -EFAULT;
        }
    }
    usize most = CMSG_SPACE(SCM_MAX_FD * sizeof(int));
    usize n = m->msg_controllen < most ? m->msg_controllen : most;
    if (m->msg_control != 0 && !uvm_range_ok(pt, m->msg_control, n, PTE_W)) {
        return -EFAULT;
    }
    return 0;
}

void scm_drop(struct file **files, u32 n) {
    for (u32 i = 0; i < n; i++) {
        fileclose(files[i]);
    }
}

int scm_get(pagetable_t pt, const struct msghdr *m, bool rights, struct file **files, u32 *n) {
    *n = 0;
    usize len = m->msg_controllen;
    usize off = 0;
    int err = 0;
    /* Each message whole in the buffer, the next at the next multiple of
     * 8 (Linux's CMSG_NXTHDR); a tail too short for a header is ignored. */
    while (err == 0 && len - off >= sizeof(struct cmsghdr)) {
        struct cmsghdr h;
        uintptr at = m->msg_control + off;
        if (copyin(pt, &h, at, sizeof h) < 0) {
            err = -EFAULT;
            break;
        }
        if (h.cmsg_len < sizeof h || h.cmsg_len > len - off) {
            err = -EINVAL;
            break;
        }
        if (h.cmsg_level == SOL_SOCKET) {
            if (!rights || h.cmsg_type != SCM_RIGHTS) {
                err = -EINVAL;
                break;
            }
            usize count = (h.cmsg_len - sizeof h) / sizeof(int);
            if (count > SCM_MAX_FD - *n) {
                err = -EINVAL;
                break;
            }
            for (usize i = 0; i < count; i++) {
                int fd;
                if (copyin(pt, &fd, at + sizeof h + i * sizeof(int), sizeof fd) < 0) {
                    err = -EFAULT;
                    break;
                }
                struct file *f = fd >= 0 ? fd_get((u32)fd) : NULL;
                if (f == NULL) {
                    err = -EBADF;
                    break;
                }
                files[(*n)++] = f;
            }
        }
        usize step = CMSG_ALIGN(h.cmsg_len);
        if (step > len - off) {
            break;
        }
        off += step;
    }
    if (err < 0) {
        scm_drop(files, *n);
        *n = 0;
    }
    return err;
}

usize scm_install(pagetable_t pt, const struct msghdr *m, struct file **files, u32 n, bool cloexec, u32 *flags) {
    if (n == 0) {
        return 0;
    }
    /* As many as the buffer holds after the header (Linux's
     * scm_detach_fds). */
    u32 room = 0;
    if (m->msg_control != 0 && m->msg_controllen > sizeof(struct cmsghdr)) {
        usize fit = (m->msg_controllen - sizeof(struct cmsghdr)) / sizeof(int);
        room = fit < n ? (u32)fit : n;
    }
    int fds[SCM_MAX_FD];
    u32 given = 0;
    while (given < room) {
        int fd = fd_install_from(files[given], 0, cloexec);
        if (fd < 0) {
            break;
        }
        fds[given++] = fd;
    }
    if (given < n) {
        *flags |= MSG_CTRUNC;
        scm_drop(files + given, n - given);
    }
    if (given == 0) {
        return 0;
    }
    struct cmsghdr h;
    h.cmsg_len = CMSG_LEN(given * sizeof(int));
    h.cmsg_level = SOL_SOCKET;
    h.cmsg_type = SCM_RIGHTS;
    /* Checked (msg_check_out): another thread may have unmapped it since,
     * and then the descriptors are open but the program doesn't know them,
     * as on Linux when the copy fails after they were given. */
    (void)copyout(pt, m->msg_control, &h, sizeof h);
    (void)copyout(pt, m->msg_control + sizeof h, fds, given * sizeof(int));
    usize used = CMSG_SPACE(given * sizeof(int));
    return used < m->msg_controllen ? used : m->msg_controllen;
}

int msg_put(pagetable_t pt, uintptr umsg, u32 namelen, usize controllen, u32 flags) {
    int fl = (int)flags;
    if (copyout(pt, umsg + offsetof(struct msghdr, msg_namelen), &namelen, sizeof namelen) < 0 ||
        copyout(pt, umsg + offsetof(struct msghdr, msg_controllen), &controllen, sizeof controllen) < 0 ||
        copyout(pt, umsg + offsetof(struct msghdr, msg_flags), &fl, sizeof fl) < 0) {
        return -EFAULT;
    }
    return 0;
}
