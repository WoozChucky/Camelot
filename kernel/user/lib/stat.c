/* stat: what fstat says about the file at a path. */
#include "../user.h"

int stat(const char *path, struct stat *st) {
    int fd = open(path, O_RDONLY);
    if (fd < 0) {
        return fd;
    }
    int r = fstat(fd, st);
    close(fd);
    return r;
}
