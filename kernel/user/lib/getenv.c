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

/* The array setenv last made, from malloc: while environ is still it,
 * setenv can grow it with realloc; once a program has set environ to an
 * array of its own (or before setenv made one: crt0's, on the stack), it
 * makes a new one, as glibc's setenv does with its last_environ. */
static char **owned;

/* How many entries environ has. */
static int count(void) {
    int n = 0;
    while (environ != NULL && environ[n] != NULL) {
        n++;
    }
    return n;
}

int setenv(const char *name, const char *value, int overwrite) {
    int len = name_length(name);
    if (len < 0) {
        return -EINVAL;
    }
    int n = count();
    int at = 0;
    while (at < n && !is_named(environ[at], name, len)) {
        at++;
    }
    if (at < n && !overwrite) {
        return 0;
    }
    /* The new entry, NAME=value. The one it replaces is not freed: a
     * pointer getenv returned into it stays good (glibc keeps them too). */
    size_t vlen = strlen(value);
    char *entry = malloc((size_t)len + 1 + vlen + 1);
    if (entry == NULL) {
        return -ENOMEM;
    }
    memcpy(entry, name, (size_t)len);
    entry[len] = '=';
    memcpy(entry + len + 1, value, vlen + 1);
    if (at < n) {
        environ[at] = entry;
        return 0;
    }
    /* A new variable goes at the end: an array one longer, which is ours
     * to grow if environ is the one we made last time. */
    char **grown;
    if (environ != NULL && environ == owned) {
        grown = realloc(owned, sizeof(char *) * (size_t)(n + 2));
    } else {
        grown = malloc(sizeof(char *) * (size_t)(n + 2));
        if (grown != NULL) {
            for (int i = 0; i < n; i++) {
                grown[i] = environ[i];
            }
        }
    }
    if (grown == NULL) {
        free(entry);
        return -ENOMEM;
    }
    grown[n] = entry;
    grown[n + 1] = NULL;
    owned = grown;
    environ = grown;
    return 0;
}

int unsetenv(const char *name) {
    int len = name_length(name);
    if (len < 0) {
        return -EINVAL;
    }
    if (environ == NULL) {
        return 0;
    }
    /* Every entry called `name` goes, the others move down in their order:
     * in place, whoever's array it is, as glibc's unsetenv does. */
    char **to = environ;
    for (char **from = environ; *from != NULL; from++) {
        if (!is_named(*from, name, len)) {
            *to = *from;
            to++;
        }
    }
    *to = NULL;
    return 0;
}
