/* whoami: prints the effective user's name, from /etc/passwd, or its
 * number when it has none (docs/excalibur-spec.md, "Users"). */
#include "user.h"

int main(int argc, char **argv) {
    (void)argv;
    if (argc != 1) {
        fprintf(STDERR, "usage: whoami\n");
        return 2;
    }
    uid_t uid = geteuid();
    struct passwd *pw = getpwuid(uid);
    if (pw == NULL) {
        fprintf(STDERR, "whoami: no name for user id %u\n", uid);
        return 1;
    }
    printf("%s\n", pw->pw_name);
    return 0;
}
