/* ld: `ld [-T script] [-e entry] [-o out] [-S] [-L dir] input.o ...
 * [lib.a] [-lname] [--start-lib lib.o ... --end-lib] [@file]`, merlin-ld's
 * command line (main.rs, lib.rs). The link: the objects, the archives'
 * members (archive.c) and the library members they need (library.c), the
 * layout (layout.c), the relocated contents (relocate.c), the file
 * (write.c). Errors are reported as merlin-ld reports them; a link with
 * errors writes nothing. */
#include "ld.h"

static const char USAGE[] =
    "usage: ld [options] input.o ... [lib.a] [-lNAME]\n"
    "\n"
    "  -o FILE        output executable (default: a.out)\n"
    "  -T FILE        linker script (default: .text at 0x80000000, then .rodata, .data, .bss)\n"
    "  -e SYMBOL      entry point symbol or address (overrides ENTRY in the script)\n"
    "  -S, --strip-debug\n"
    "                 leave out the debug information (.debug_* sections)\n"
    "  -L DIR         look for -l libraries in DIR (every -L, in order)\n"
    "  -l NAME        the archive libNAME.a of the first -L directory that has it\n"
    "                 (-l:FILE: FILE itself); an archive's members, as the objects\n"
    "                 of --start-lib, are linked only if needed\n"
    "  --start-lib    the objects up to --end-lib are a library: each is linked\n"
    "                 only if it defines a symbol the others need (like ld.lld's)\n"
    "  --end-lib      ends a library\n"
    "  @FILE          more arguments, read from FILE (separated by white space)";

/* The whole file, or NULL with *err set (a negative error number). */
static uchar *read_file(const char *path, uint *len, int *err) {
    int fd = open(path, O_RDONLY);
    if (fd < 0) {
        *err = fd;
        return NULL;
    }
    struct stat st;
    int r = fstat(fd, &st);
    if (r < 0) {
        close(fd);
        *err = r;
        return NULL;
    }
    /* The size, an off_t: the linker keeps a file in memory, whose sizes
     * are uints, so a file is at most 4 GiB - 1 bytes here (no Camelot
     * process has that much memory). */
    uint size = (uint)st.size;
    uchar *data = xmalloc(size + 1);
    uint done = 0;
    while (done < size) {
        int n = (int)read(fd, data + done, size - done);
        if (n < 0) {
            close(fd);
            *err = n;
            return NULL;
        }
        if (n == 0) {
            break;
        }
        done += (uint)n;
    }
    close(fd);
    data[done] = 0;
    *len = done;
    return data;
}

static int write_file(const char *path, const uchar *data, uint len) {
    int fd = open(path, O_WRONLY | O_CREAT | O_TRUNC, 0755);
    if (fd < 0) {
        return fd;
    }
    uint done = 0;
    while (done < len) {
        uint n = len - done > 4096 ? 4096 : len - done;
        int r = write(fd, data + done, n);
        if (r <= 0) {
            close(fd);
            return r < 0 ? r : -ENOSPC;
        }
        done += (uint)r;
    }
    return close(fd);
}

static int is_space(int c) {
    return c == ' ' || c == '\t' || c == '\n' || c == '\r' || c == '\v' || c == '\f';
}

/* The arguments, each @FILE replaced by the words of FILE, as GNU ld and
 * ld.lld read response files (without their quoting): how a link line
 * longer than the shell takes is given. 0 after a message. */
static int expand(int argc, char **argv, char ***out, uint *n) {
    uint cap = 0;
    *out = NULL;
    *n = 0;
    for (int i = 1; i < argc; i++) {
        char *a = argv[i];
        if (a[0] != '@' || a[1] == 0) {
            *out = grow(*out, &cap, *n + 1, sizeof(char *));
            (*out)[(*n)++] = a;
            continue;
        }
        char *file = a + 1;
        uint len;
        int err;
        char *text = (char *)read_file(file, &len, &err);
        if (text == NULL) {
            fprintf(2, "ld: cannot read %s: %s\n", file, strerror(-err));
            return 0;
        }
        char *p = text;
        for (;;) {
            while (*p && is_space((uchar)*p)) {
                p++;
            }
            if (*p == 0) {
                break;
            }
            char *w = p;
            while (*p && !is_space((uchar)*p)) {
                p++;
            }
            if (*p) {
                *p++ = 0;
            }
            if (w[0] == '@') {
                fprintf(2, "ld: %s: a response file cannot name another (%s)\n", file, w);
                return 0;
            }
            *out = grow(*out, &cap, *n + 1, sizeof(char *));
            (*out)[(*n)++] = w;
        }
    }
    return 1;
}

static void print_errors(void) {
    for (uint i = 0; i < errors.n; i++) {
        fprintf(2, "ld: error: %s\n", errors.msgs[i]);
    }
}

/* An input file of the command line, or the library -l names, and
 * whether it is a library member. */
struct arg_input {
    char *name;
    int lib;
    int search;
};

/* Whether `path` can be opened: a file -l may find. */
static int exists(const char *path) {
    int fd = open(path, O_RDONLY);
    if (fd < 0) {
        return 0;
    }
    struct stat st;
    int r = fstat(fd, &st);
    close(fd);
    return r == 0 && st.type == T_FILE;
}

/* Where -lNAME is (merlin-ld's search_library): libNAME.a, or FILE for
 * -l:FILE, in the first of the n directories that has it. NULL if none. */
static char *search_library(const char *name, char **dirs, uint n) {
    char *file = name[0] == ':' ? xstrdup(name + 1) : fmt("lib%s.a", name);
    for (uint i = 0; i < n; i++) {
        uint len = (uint)strlen(dirs[i]);
        char *path = len > 0 && dirs[i][len - 1] == '/' ? fmt("%s%s", dirs[i], file) : fmt("%s/%s", dirs[i], file);
        if (exists(path)) {
            return path;
        }
    }
    return NULL;
}

int main(int argc, char **argv) {
    char **args;
    uint nargs;
    if (!expand(argc, argv, &args, &nargs)) {
        return 2;
    }
    struct arg_input *names = xcalloc((nargs + 1) * sizeof(struct arg_input));
    uint nnames = 0;
    char **dirs = xcalloc((nargs + 1) * sizeof(char *));
    uint ndirs = 0;
    int in_lib = 0, strip = 0;
    char *output = "a.out";
    char *script = NULL;
    char *entry = NULL;
    for (uint i = 0; i < nargs; i++) {
        char *a = args[i];
        /* `-o=x` and the like, and the separate-argument forms. */
        char *flag = a;
        char *inline_value = NULL;
        char *eq = strchr(a, '=');
        if (eq != NULL && a[0] == '-') {
            flag = xstrndup(a, (uint)(eq - a));
            inline_value = eq + 1;
        }
        char **target = NULL;
        const char *name = NULL;
        /* -L DIR and -l NAME take their value as -o does; -LDIR and -lNAME
         * joined. */
        char *dir = NULL;
        char *library = NULL;
        if (strcmp(flag, "-o") == 0) {
            target = &output;
            name = "-o";
        } else if (strcmp(flag, "-T") == 0) {
            target = &script;
            name = "-T";
        } else if (strcmp(flag, "-e") == 0 || strcmp(flag, "--entry") == 0) {
            target = &entry;
            name = "-e";
        } else if (strcmp(flag, "--Map") == 0 || strcmp(flag, "-Map") == 0) {
            fprintf(2, "ld: --Map is not supported here (the host's merlin-ld writes link maps)\n");
            return 2;
        } else if (strcmp(flag, "-S") == 0 || strcmp(flag, "--strip-debug") == 0) {
            strip = 1;
        } else if (strcmp(flag, "--start-lib") == 0) {
            if (in_lib) {
                fprintf(2, "ld: nested --start-lib\n");
                return 2;
            }
            in_lib = 1;
        } else if (strcmp(flag, "--end-lib") == 0) {
            if (!in_lib) {
                fprintf(2, "ld: --end-lib without --start-lib\n");
                return 2;
            }
            in_lib = 0;
        } else if (strcmp(flag, "-h") == 0 || strcmp(flag, "--help") == 0) {
            printf("%s\n", USAGE);
            return 0;
        } else if (strcmp(flag, "-L") == 0 || strcmp(flag, "--library-path") == 0) {
            target = &dir;
            name = "-L";
        } else if (strcmp(flag, "-l") == 0 || strcmp(flag, "--library") == 0) {
            target = &library;
            name = "-l";
        } else if (flag[0] == '-' && flag[1] == 'L') {
            dir = a + 2;
        } else if (flag[0] == '-' && flag[1] == 'l') {
            library = a + 2;
        } else if (flag[0] == '-' && flag[1] != 0) {
            fprintf(2, "ld: unknown option %s\n%s\n", flag, USAGE);
            return 2;
        } else {
            names[nnames].name = a;
            names[nnames].lib = in_lib;
            nnames++;
        }
        if (target != NULL) {
            if (inline_value != NULL) {
                *target = inline_value;
            } else if (i + 1 < nargs) {
                *target = args[++i];
            } else {
                fprintf(2, "ld: %s needs an argument\n", name);
                return 2;
            }
        }
        if (dir != NULL) {
            dirs[ndirs++] = dir;
        }
        if (library != NULL) {
            names[nnames].name = library;
            names[nnames].lib = in_lib;
            names[nnames].search = 1;
            nnames++;
        }
    }
    if (in_lib) {
        fprintf(2, "ld: --start-lib without --end-lib\n");
        return 2;
    }
    if (nnames == 0) {
        fprintf(2, "ld: no input files\n%s\n", USAGE);
        return 2;
    }

    /* Every -L counts for every -l, as in ld: the directories are all
     * known before a library is looked for. */
    int missing = 0;
    for (uint i = 0; i < nnames; i++) {
        if (names[i].search) {
            char *path = search_library(names[i].name, dirs, ndirs);
            if (path == NULL) {
                fprintf(2, "ld: error: unable to find library -l%s\n", names[i].name);
                missing = 1;
            }
            names[i].name = path;
        }
    }
    if (missing) {
        return 1;
    }

    /* In command-line order, which ld.lld reads them in, library members
     * too; an archive stands for its members, each a library member
     * (merlin-ld's library::expand_archives), named lib.a(m.o). */
    uint cap = 0, n = 0;
    struct input *files = NULL;
    int *lazy = NULL;
    uint lazy_cap = 0;
    for (uint i = 0; i < nnames; i++) {
        struct input in;
        int err;
        in.name = names[i].name;
        in.data = read_file(in.name, &in.len, &err);
        if (in.data == NULL) {
            fprintf(2, "ld: cannot read %s: %s\n", in.name, strerror(-err));
            return 2;
        }
        if (is_thin_archive(in.data, in.len)) {
            error(fmt("%s: thin archives are not supported", in.name));
            continue;
        }
        if (!is_archive(in.data, in.len)) {
            files = grow(files, &cap, n + 1, sizeof(struct input));
            lazy = grow(lazy, &lazy_cap, n + 1, sizeof(int));
            files[n] = in;
            lazy[n++] = names[i].lib;
            continue;
        }
        struct ar_member *members;
        uint count;
        char *msg;
        if (!archive_parse(in.data, in.len, &members, &count, &msg)) {
            error(fmt("%s: %s", in.name, msg));
            continue;
        }
        for (uint k = 0; k < count; k++) {
            /* What a link takes from an archive: relocatable ELF objects
             * (ET_REL, e_type 1 at byte 16), as ld.lld. */
            uchar *d = members[k].data;
            if (members[k].len < 18 || memcmp(d, "\177ELF", 4) != 0 || d[16] != 1 || d[17] != 0) {
                add_message(&warnings, fmt("%s: archive member '%s' is neither ET_REL nor LLVM bitcode",
                                           in.name, members[k].name));
                continue;
            }
            files = grow(files, &cap, n + 1, sizeof(struct input));
            lazy = grow(lazy, &lazy_cap, n + 1, sizeof(int));
            files[n].name = fmt("%s(%s)", in.name, members[k].name);
            files[n].data = members[k].data;
            files[n].len = members[k].len;
            lazy[n++] = 1;
        }
    }
    if (errors.n != 0) {
        print_errors();
        return 1;
    }
    char *script_text = NULL;
    if (script != NULL) {
        uint len;
        int err;
        script_text = (char *)read_file(script, &len, &err);
        if (script_text == NULL) {
            fprintf(2, "ld: cannot read %s: %s\n", script, strerror(-err));
            return 2;
        }
    }

    /* ld.lld's order (merlin-ld's layout::read): the script first, so its
     * errors come first; then every file, in command-line order.
     * layout_build reads the script again for the layout. */
    struct script read;
    uint line;
    char *msg;
    if (!script_parse(script_text != NULL ? script_text : DEFAULT_SCRIPT, &read, &line, &msg)) {
        error(fmt("%s:%u: %s", script != NULL ? script : "<default layout>", line, msg));
    }
    /* The entry symbol, which extracts the member that defines it: -e,
     * else ENTRY(...), else _start, unless it is an address. */
    const char *entry_name = entry != NULL ? entry : read.entry != NULL ? read.entry : "_start";
    uint64_t entry_addr;
    if (errors.n != 0 || parse_number(entry_name, &entry_addr)) {
        entry_name = NULL;
    }
    uint nobjects;
    struct object *objects =
        parse_with_needed(files, lazy, n, entry_name, errors.n == 0 ? &read : NULL, &nobjects);
    struct layout layout;
    uchar **images, **debug;
    if (objects == NULL || !layout_build(objects, nobjects, script, script_text, entry, &layout) ||
        !relocate(&layout, &images, &debug)) {
        print_errors();
        return 1;
    }
    for (uint i = 0; i < warnings.n; i++) {
        fprintf(2, "ld: warning: %s\n", warnings.msgs[i]);
    }
    struct buf elf;
    write_elf(&layout, images, debug, strip, &elf);
    int r = write_file(output, elf.data, elf.len);
    if (r < 0) {
        fprintf(2, "ld: cannot write %s: %s\n", output, strerror(-r));
        return 2;
    }
    return 0;
}
