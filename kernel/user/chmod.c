/* chmod: changes files' permission bits (docs/camelot-syscalls.md, "Users
 * and permissions").
 *
 *   chmod MODE FILE...
 *
 * MODE is octal, as `chmod 644 notes` or `chmod 4755 /bin/su`: the
 * permission bits, at most 7777 (no symbolic modes such as `u+x`). Only a
 * file's owner or root may change them. Says which files it could not
 * change and exits 1 if there was one. */
#include "user.h"

int main(int argc, char **argv) {
    if (argc < 3) {
        fprintf(STDERR, "usage: chmod MODE FILE...\n");
        return 2;
    }
    uint mode = 0;
    const char *p = argv[1];
    for (; *p >= '0' && *p <= '7' && mode <= 07777; p++) {
        mode = mode * 8 + (uint)(*p - '0');
    }
    if (*p != 0 || p == argv[1] || mode > 07777) {
        fprintf(STDERR, "chmod: %s: not an octal mode of at most 7777\n", argv[1]);
        return 2;
    }
    int status = 0;
    for (int i = 2; i < argc; i++) {
        int err = chmod(argv[i], mode);
        if (err < 0) {
            fprintf(STDERR, "chmod: %s: %s\n", argv[i], strerror(-err));
            status = 1;
        }
    }
    return status;
}
