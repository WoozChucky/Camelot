/* The kernel's error numbers, as Linux's include/linux/errno.h: every one a
 * system call returns (syscall.h, Linux's uapi numbers, which user space
 * shares), and the few the kernel uses that no system call returns. Those
 * stay out of syscall.h, whose numbers are docs/camelot-syscalls.md's, and
 * have Linux's numbers too: the uapi ones (asm-generic/errno-base.h) where
 * Linux has one, and ENOTSUPP Linux's kernel-internal 524. */
#ifndef CAMELOT_ERRNO_H
#define CAMELOT_ERRNO_H

#include "syscall.h"

/* fs_mount()'s (fs.h), and sbi_err_map_linux_errno()'s (sbi.h):
 * SBI_ERR_IO. */
#define EIO 5 /* the disk failed, or an SBI call's input or output */

/* (ERANGE, sbi_err_map_linux_errno()'s SBI_ERR_BAD_RANGE, is getcwd's
 * too: syscall.h has it.) */

/* Linux's include/linux/errno.h: "Operation is not supported", the errno
 * Linux maps SBI_ERR_NOT_SUPPORTED and SBI_ERR_FAILED to (and any SBI error
 * it doesn't know). A kernel-internal number: not EOPNOTSUPP (95), which
 * user space sees. */
#define ENOTSUPP 524
#endif
