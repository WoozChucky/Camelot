/* Local sockets (docs/camelot-syscalls.md, "Local sockets"; spec section
 * 10, "Local sockets"): AF_UNIX stream sockets, Linux's, between the
 * processes of this machine, which find each other by a path on the
 * filesystem, a socket inode, or are made a connected pair at once
 * (socketpair). A message may carry open files, SCM_RIGHTS (msg.h): the
 * descriptors a process sends arrive as new descriptors of the receiver
 * for the same open files.
 *
 * A local socket is an open file of type FD_UNIX (file.h), so read(),
 * write(), poll(), fstat(), dup() and close() work on it; the socket calls
 * of net/socket.c come here for one (syscall.c), without the network's
 * lock, since binding and connecting look up paths on the disk. */
#ifndef CAMELOT_UNIX_H
#define CAMELOT_UNIX_H

#include "types.h"
#include "vm.h"

struct poll_table;
struct stat;
struct unix_sock;

/* Sets up the sockets' lock. kmain calls it at boot. */
void unix_init(void);

/* Whether descriptor `fd` of the current process is a local socket, for
 * syscall.c to send a socket call here rather than to the network. Another
 * thread may close it and open something else there before the call takes
 * it: the call then fails with -ENOTSOCK or -EBADF, as it may whenever a
 * descriptor is closed under it. */
bool unix_fd(u32 fd);

/* What file.c does to a local socket's file: read() and write() are recv()
 * and send() without a control message (descriptors that come with what a
 * read() takes are closed), poll(), fstat() (T_SOCKET, the bytes waiting
 * to be read, and nlink 1, or 0 once nothing more will come), and its last
 * close. */
isize unix_read(struct unix_sock *u, pagetable_t pt, uintptr dst, usize n);
isize unix_write(struct unix_sock *u, pagetable_t pt, uintptr src, usize n);
u32 unix_poll(struct unix_sock *u, struct poll_table *pt);
void unix_stat(struct unix_sock *u, struct stat *st);
void unix_close(struct unix_sock *u);

/* The socket calls on a local socket (docs/camelot-syscalls.md, "Local
 * sockets"); socket(AF_UNIX, ...) is unix_sys_socket, and socketpair()
 * every domain's. */
int unix_sys_socket(u32 type, u32 protocol);
int unix_sys_socketpair(u32 domain, u32 type, u32 protocol, uintptr ufds);
int unix_sys_bind(u32 fd, uintptr uaddr);
int unix_sys_listen(u32 fd, u32 backlog);
int unix_sys_accept(u32 fd, uintptr uaddr);
int unix_sys_connect(u32 fd, uintptr uaddr);
isize unix_sys_sendto(u32 fd, uintptr ubuf, usize len, uintptr uaddr);
isize unix_sys_recvfrom(u32 fd, uintptr ubuf, usize len, uintptr uaddr);
int unix_sys_shutdown(u32 fd, u32 how);
int unix_sys_name(u32 fd, uintptr uaddr, bool peer);
int unix_sys_setsockopt(u32 fd, u32 level, u32 option, uintptr uvalue, u32 len);
int unix_sys_getsockopt(u32 fd, u32 level, u32 option, uintptr uvalue, uintptr ulen);
isize unix_sys_sendmsg(u32 fd, uintptr umsg, u32 flags);
isize unix_sys_recvmsg(u32 fd, uintptr umsg, u32 flags);

#endif
