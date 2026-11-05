/* getenv and its kin (user.h): getenv, setenv and unsetenv, over
 * environ (environ.c), as POSIX describes them and glibc's stdlib/getenv.c
 * and stdlib/setenv.c do them, but for errno: the errors come back as
 * negative numbers, as everywhere in this library. */
#include "../user.h"

/* The length of `name` if it can name a variable, else -1: POSIX's setenv
 * refuses an empty name and one with a '=' in it (the '=' would end the
 * name in the entry), and getenv finds nothing for one. */
static int name_length(const char *name) {
    if (name == NULL || name[0] == 0) {
        return -1;
    }
    int n = 0;
    while (name[n] != 0) {
        if (name[n] == '=') {
            return -1;
        }
        n++;
    }
    return n;
}

/* Whether environment entry `entry` is the variable `name`, `len` bytes:
 * it starts with the name and a '='. */
static int is_named(const char *entry, const char *name, int len) {
    return strncmp(entry, name, (size_t)len) == 0 && entry[len] == '=';
}

char *getenv(const char *name) {
    int len = name_length(name);
    if (len < 0 || environ == NULL) {
        return NULL;
    }
    for (char **e = environ; *e != NULL; e++) {
        if (is_named(*e, name, len)) {
            return *e + len + 1;
        }
    }
    return NULL;
}

/* setenv and unsetenv change environ, and threads share it: as glibc's
 * setenv.c holds its envlock, they hold this mutex across the whole of
 * what they do, so that two of them at once can't both grow the same
 * array, or compact one another is copying. getenv takes no lock, as
 * glibc's doesn't: a program that reads the environment in one thread
 * while another changes it keeps them apart itself (setenv may move the
 * array getenv is reading), as POSIX leaves it, where neither getenv nor
 * setenv need be thread-safe. */
static struct mutex envlock;

/* The array setenv last made, from malloc: glibc's last_environ. setenv
 * grows it with realloc whether or not environ is still it. When a
 * program has set environ to an array of its own since (or setenv hasn't
 * made one yet: environ is crt0's, on the stack), setenv copies environ's
 * pointers into the grown array, and the array it had made before is
 * reused rather than lost, as glibc's __add_to_environ does. */
static char **last_environ;

int setenv(const char *name, const char *value, int overwrite) {
    int len = name_length(name);
    if (len < 0) {
        return -EINVAL;
    }
    mutex_lock(&envlock);
    /* environ is read once the lock is held: another thread's setenv may
     * have just made a new one. */
    int size = 0;
    char **ep = environ;
    if (ep != NULL) {
        while (*ep != NULL && !is_named(*ep, name, len)) {
            ep++;
            size++;
        }
    }
    if (ep == NULL || *ep == NULL) {
        /* A new variable, at the end: an array one longer (with room for
         * its NULL), grown from the one setenv made last. realloc keeps
         * that one if it fails, and so does environ. */
        char **grown = realloc(last_environ, sizeof(char *) * (size_t)(size + 2));
        if (grown == NULL) {
            mutex_unlock(&envlock);
            return -ENOMEM;
        }
        if (environ != last_environ) {
            memcpy(grown, environ, sizeof(char *) * (size_t)size);
        }
        grown[size] = NULL;
        grown[size + 1] = NULL;
        ep = grown + size;
        last_environ = grown;
        environ = grown;
    }
    if (*ep == NULL || overwrite) {
        /* The entry, NAME=value. The one it replaces is not freed: a
         * pointer getenv returned into it stays good (glibc keeps them
         * too). If there is no memory for it, the array grown above stays,
         * still ending where it did. */
        size_t vlen = strlen(value);
        char *entry = malloc((size_t)len + 1 + vlen + 1);
        if (entry == NULL) {
            mutex_unlock(&envlock);
            return -ENOMEM;
        }
        memcpy(entry, name, (size_t)len);
        entry[len] = '=';
        memcpy(entry + len + 1, value, vlen + 1);
        *ep = entry;
    }
    mutex_unlock(&envlock);
    return 0;
}

int unsetenv(const char *name) {
    int len = name_length(name);
    if (len < 0) {
        return -EINVAL;
    }
    mutex_lock(&envlock);
    /* Every entry called `name` goes, the others move down in their order:
     * in place, whoever's array it is, as glibc's unsetenv does. */
    if (environ != NULL) {
        char **to = environ;
        for (char **from = environ; *from != NULL; from++) {
            if (!is_named(*from, name, len)) {
                *to = *from;
                to++;
            }
        }
        *to = NULL;
    }
    mutex_unlock(&envlock);
    return 0;
}
