/* environ: the program's environment (user.h), a member of its own, as
 * glibc's posix/environ.c, so that crt0, which sets it to main's envp before
 * calling main, pulls in this variable and nothing else: the functions
 * that look at it are getenv.c's. Zero until crt0 sets it, a NULL-terminated
 * array of NAME=value strings after. */
#include "../user.h"

char **environ;
