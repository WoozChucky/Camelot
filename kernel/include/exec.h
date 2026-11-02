/* Running programs (spec section 10, "Kernel files"): spawn and exec find
 * the executable a path names and hand it to the ELF loader (elf.c) through
 * proc_create() or proc_exec_load() (proc.c).
 *
 * With a root filesystem, a path is looked up like any other (from the
 * working directory unless it starts with `/`) and must name a regular
 * file, which is read from the disk as it loads; the transaction it is
 * read in keeps it from being freed meanwhile. Without one (no disk, or
 * none the kernel could mount), the programs embedded in the kernel image
 * (kernel/userprogs.s) stand in for /bin: `/bin/NAME` (or `bin/NAME`, the
 * working directory being the root) names the program NAME, and nothing
 * else exists. */
#ifndef CAMELOT_EXEC_H
#define CAMELOT_EXEC_H

#include "proc.h"
#include "types.h"

/* spawn(): starts the program at `path` with `args` as a child of the
 * current process. Returns its pid, or -ENOENT/-ENOTDIR/-ENAMETOOLONG (the
 * path), -EACCES (not a regular file), or proc_create()'s errors. */
int exec_spawn(const char *path, const struct proc_args *args);

/* exec(): replaces the current process's program with the one at `path`.
 * Returns argc for the new program's a0, or an error as exec_spawn() (but
 * -EAGAIN), the old program still running. */
int exec_replace(const char *path, const struct proc_args *args);

/* The first process's program: the one at `path`, with argv = { name }.
 * From the root filesystem if there is one, falling back to the embedded
 * programs if it has no such file. Returns argc or an error. */
int exec_init(const char *path, const char *name);

#endif
