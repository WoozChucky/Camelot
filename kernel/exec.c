/* Running programs from a path (exec.h): the lookup, from the root
 * filesystem or among the embedded programs, then proc_create() for spawn
 * or proc_exec_load() and proc_exec_commit() for exec. */
#include "exec.h"
#include "elf.h"
#include "fs.h"
#include "kalloc.h"
#include "log.h"
#include "string.h"

/* elf_file's read() for a file: the inode is locked for the whole load. */
static bool read_inode(const struct elf_file *f, void *dst, u64 off, u32 n) {
    return readi(f->inode, dst, off, n) == (int)n;
}

/* The embedded program `path` names when there is no filesystem: `NAME`
 * in `/bin/NAME` or `bin/NAME` (any number of slashes), else NULL. */
static const struct user_program *embedded(const char *path) {
    while (*path == '/') {
        path++;
    }
    if (path[0] != 'b' || path[1] != 'i' || path[2] != 'n' || path[3] != '/') {
        return NULL;
    }
    path += 3;
    while (*path == '/') {
        path++;
    }
    for (const char *c = path; *c != 0; c++) {
        if (*c == '/') {
            return NULL;
        }
    }
    return *path == 0 ? NULL : user_program_find(path);
}

/* Finds the executable at `path` and sets `f` up to read it: from the
 * filesystem unless `from_kernel`, in a transaction and with the inode
 * locked until prog_close(); else from the kernel image. 0 or an error. */
static int prog_open(const char *path, bool from_kernel, struct elf_file *f) {
    if (from_kernel) {
        const struct user_program *prog = embedded(path);
        if (prog == NULL) {
            return -ENOENT;
        }
        elf_from_memory(f, prog->elf, prog->size);
        return 0;
    }
    begin_op();
    int err;
    struct inode *ip = namei(path, &err);
    if (ip == NULL) {
        end_op();
        return err;
    }
    ilock(ip);
    if (ip->type != T_FILE) {
        iunlockput(ip);
        end_op();
        return -EACCES;
    }
    f->size = ip->size;
    f->read = read_inode;
    f->image = NULL;
    f->inode = ip;
    return 0;
}

static void prog_close(struct elf_file *f) {
    if (f->inode != NULL) {
        iunlockput(f->inode);
        end_op();
    }
}

/* What a process running `path` is called: its last name. */
static const char *last_name(const char *path) {
    const char *name = path;
    for (const char *c = path; *c != 0; c++) {
        if (*c == '/' && c[1] != 0) {
            name = c + 1;
        }
    }
    return name;
}

int exec_spawn(const char *path, const struct proc_args *args) {
    struct elf_file f;
    int err = prog_open(path, !fs_mounted, &f);
    if (err < 0) {
        return err;
    }
    int pid = proc_create(&f, path, last_name(path), args);
    prog_close(&f);
    return pid;
}

/* Loads the program `f` for exec, closes it, and runs it as `name`. The
 * whole image is read before the file is closed, and only then does
 * proc_exec_commit() wait for the process's other threads to end: one of
 * them may be waiting for the program's inode lock (reading the same file),
 * or for room in the log, which this transaction holds until it ends. */
static int load_and_run(struct elf_file *f, const char *path, const char *name, const struct proc_args *args) {
    struct exec_image img;
    int err = proc_exec_load(f, path, args, &img);
    prog_close(f);
    if (err < 0) {
        return err;
    }
    return proc_exec_commit(&img, name);
}

int exec_replace(const char *path, const struct proc_args *args) {
    struct elf_file f;
    int err = prog_open(path, !fs_mounted, &f);
    if (err < 0) {
        return err;
    }
    return load_and_run(&f, path, last_name(path), args);
}

int exec_init(const char *path, const char *name) {
    struct elf_file f;
    int err = prog_open(path, !fs_mounted, &f);
    if (err == -ENOENT && fs_mounted) {
        /* A root filesystem without a shell still boots to one. */
        err = prog_open(path, true, &f);
    }
    if (err < 0) {
        return err;
    }
    struct proc_args *args = kzalloc();
    if (args == NULL) {
        prog_close(&f);
        return -ENOMEM;
    }
    args->argc = 1;
    args->len = strlen(name) + 1;
    memcpy(args->strings, name, args->len);
    err = load_and_run(&f, path, name, args);
    kfree(args);
    return err;
}
