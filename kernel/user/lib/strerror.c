/* What each error number means: the messages programs print after a name,
 * as in "cat: /missing: no such file or directory". */
#include "../user.h"

const char *strerror(int err) {
    switch (err) {
    case EPERM:
        return "operation not permitted";
    case ENOENT:
        return "no such file or directory";
    case ESRCH:
        return "no such process";
    case E2BIG:
        return "argument list too long";
    case ENOEXEC:
        return "not an executable";
    case EBADF:
        return "bad file descriptor";
    case ECHILD:
        return "no such child";
    case EAGAIN:
        return "no free process slot";
    case ENOMEM:
        return "out of memory";
    case EACCES:
        return "not a regular file";
    case EFAULT:
        return "bad address";
    case EEXIST:
        return "file exists";
    case ENODEV:
        return "no such device";
    case ENOTDIR:
        return "not a directory";
    case EISDIR:
        return "is a directory";
    case EINVAL:
        return "invalid argument";
    case ENFILE:
        return "too many open files in the system";
    case EMFILE:
        return "too many open files";
    case EFBIG:
        return "file too large";
    case ENOSPC:
        return "no space left on the disk";
    case ESPIPE:
        return "not seekable";
    case EROFS:
        return "read-only filesystem";
    case EPIPE:
        return "broken pipe";
    case ENAMETOOLONG:
        return "name too long";
    case ENOSYS:
        return "no such system call";
    case ENOTEMPTY:
        return "directory not empty";
    default:
        return "unknown error";
    }
}
