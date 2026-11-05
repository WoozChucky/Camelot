/* printenv [NAME...]: prints the value of each variable NAME of its
 * environment (getenv), a line each, or without a NAME every entry of the
 * environment (environ), a line each, as coreutils' printenv does. Exits
 * 0 if every NAME has a value, 1 if one has none (a NAME with a '=' in it
 * names none), 2 if the output can't be written. */
#include "user.h"

int main(int argc, char **argv) {
    int status = 0;
    int r = 0;
    if (argc == 1) {
        for (char **e = environ; *e != NULL && r >= 0; e++) {
            r = printf("%s\n", *e);
        }
    }
    for (int i = 1; i < argc && r >= 0; i++) {
        const char *value = getenv(argv[i]);
        if (value == NULL) {
            status = 1;
        } else {
            r = printf("%s\n", value);
        }
    }
    if (r < 0) {
        fprintf(STDERR, "printenv: %s\n", strerror(-r));
        return 2;
    }
    return status;
}
