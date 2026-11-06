/* The file system calls (sysfile.c; docs/camelot-syscalls.md): descriptors,
 * pipes and paths. Each takes the trap frame of the calling process and
 * returns the call's result, as syscall() dispatches them. */
#ifndef CAMELOT_SYSFILE_H
#define CAMELOT_SYSFILE_H

#include "trap.h"
#include "types.h"
#include "vm.h"

long sys_read(struct trap_frame *f);
long sys_write(struct trap_frame *f);
long sys_open(struct trap_frame *f);
long sys_close(struct trap_frame *f);
long sys_lseek(struct trap_frame *f);
long sys_fstat(struct trap_frame *f);
long sys_stat(struct trap_frame *f);
long sys_getcwd(struct trap_frame *f);
long sys_mkdir(struct trap_frame *f);
long sys_unlink(struct trap_frame *f);
long sys_link(struct trap_frame *f);
long sys_chdir(struct trap_frame *f);
long sys_dup(struct trap_frame *f);
long sys_fcntl(struct trap_frame *f);
long sys_pipe(struct trap_frame *f);
long sys_memfd_create(struct trap_frame *f);
long sys_ftruncate(struct trap_frame *f);
long sys_ioctl(struct trap_frame *f);

/* Copies the path at user address `va` in `pt` to `buf` (MAXPATH + 1
 * bytes). Returns 0; -EFAULT if a byte up to its NUL is not readable user
 * memory (a path too long is still read to its end, so a bad pointer is
 * reported as one); -ENAMETOOLONG if it is longer than MAXPATH; -ENOENT if
 * it is empty. */
int fetch_path(pagetable_t pt, uintptr va, char *buf);

#endif
