/* sendmsg() and recvmsg()'s pieces (docs/camelot-syscalls.md, "Messages"):
 * the user buffers a call gathers from or scatters into, and the
 * descriptors an SCM_RIGHTS control message passes. Every kind of socket
 * uses them: local ones (net/unix.c) and TCP and UDP ones (net/socket.c).
 *
 * The structures are Linux's (syscall.h): a struct msghdr names an
 * address, an array of struct iovec, and a buffer of control messages,
 * each a struct cmsghdr and its data, 8-byte aligned. */
#ifndef CAMELOT_MSG_H
#define CAMELOT_MSG_H

#include "syscall.h"
#include "types.h"
#include "vm.h"

struct file;

/* The most buffers one call takes: -EMSGSIZE above (Linux's UIO_MAXIOV is
 * 1024). */
#define UIO_MAXIOV 16

/* The most descriptors one message passes, a process's worth: -EINVAL
 * above (Linux's SCM_MAX_FD is 253). */
#define SCM_MAX_FD 16

/* The most one call moves: INT_MAX bytes, as send and recv clamp `len`
 * (net/socket.c), so that the result fits an int. */
#define MSG_MAX_LEN 0x7fffffffu

/* A control message's header and its data, aligned (Linux's CMSG_LEN and
 * CMSG_SPACE). */
#define CMSG_ALIGN(n) (((n) + 7) & ~(usize)7)
#define CMSG_LEN(n) (sizeof(struct cmsghdr) + (n))
#define CMSG_SPACE(n) (sizeof(struct cmsghdr) + CMSG_ALIGN(n))

/* User buffers, walked in order as bytes go into or out of them (Linux's
 * iov_iter). */
struct uio {
    pagetable_t pt;
    struct iovec iov[UIO_MAXIOV];
    u32 cnt;
    u32 idx;   /* the buffer the next byte goes to or comes from */
    usize off; /* how far into it */
    u32 resid; /* the bytes left in all of them, at most MSG_MAX_LEN */
};

/* A uio of the `n` struct iovec at user address `uiov`, in `pt`, each
 * buffer readable (sendmsg gathers from them) or, if `out`, writable
 * (recvmsg scatters into them). Their lengths add up to `resid`, clamped
 * to MSG_MAX_LEN. 0, or -EMSGSIZE (`n` over UIO_MAXIOV), -EFAULT. */
int uio_iovec(struct uio *u, pagetable_t pt, uintptr uiov, usize n, bool out);

/* A uio of the one buffer of `n` bytes at `buf` (read, write, send, recv),
 * which the caller has checked. */
void uio_single(struct uio *u, pagetable_t pt, uintptr buf, usize n);

/* Copies `n` bytes (at most `resid`) between kernel memory `k` and the
 * buffers: into them if `out`, else from them, and moves on past them. 0,
 * or -EFAULT if another thread unmapped one since it was checked: then
 * the uio is where it was. */
int uio_move(struct uio *u, void *k, u32 n, bool out);

/* Reads the struct msghdr at user address `umsg`: 0 or -EFAULT. */
int msg_get(pagetable_t pt, uintptr umsg, struct msghdr *m);

/* recvmsg's checks on what it writes: the struct msghdr at `umsg`, the
 * address buffer (as much of it as an address of `addr_size` bytes takes)
 * and the control buffer (as much as SCM_MAX_FD descriptors take). 0 or
 * -EFAULT. */
int msg_check_out(pagetable_t pt, uintptr umsg, const struct msghdr *m, usize addr_size);

/* The descriptors of `m`'s SCM_RIGHTS control messages, a reference to
 * each open file in files[] (as many as SCM_MAX_FD) and their number in
 * *n. Control messages of another level than SOL_SOCKET are skipped, as
 * on Linux; a SOL_SOCKET one must be SCM_RIGHTS, and only if `rights` (a
 * local socket). 0, or -EINVAL (a control message's length out of its
 * buffer, another SOL_SOCKET type, more than SCM_MAX_FD descriptors, or
 * any when not `rights`), -EBADF (not an open descriptor), -EFAULT: then
 * no reference is held. */
int scm_get(pagetable_t pt, const struct msghdr *m, bool rights, struct file **files, u32 *n);

/* Lets go of the `n` files in files[]: descriptors that never arrived. */
void scm_drop(struct file **files, u32 n);

/* recvmsg's descriptors: gives the `n` files of files[] the lowest free
 * descriptors (closing on exec if `cloexec`), their references passing to
 * them, and stores them as one SCM_RIGHTS message at the start of `m`'s
 * control buffer, which the caller checked (msg_check_out). Files that
 * don't fit in the buffer, or find no free descriptor, are closed, and
 * MSG_CTRUNC is set in *flags, as on Linux. Returns the bytes of the
 * buffer used: 0 without descriptors. */
usize scm_install(pagetable_t pt, const struct msghdr *m, struct file **files, u32 n, bool cloexec, u32 *flags);

/* recvmsg's results, into the struct msghdr at `umsg`: msg_namelen,
 * msg_controllen and msg_flags. 0 or -EFAULT. */
int msg_put(pagetable_t pt, uintptr umsg, u32 namelen, usize controllen, u32 flags);

#endif
