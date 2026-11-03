/* ld: `ld [-T script] [-e entry] [-o out] [-S] input.o ... [--start-lib
 * lib.o ... --end-lib] [@file]`, merlin-ld's command line (main.rs, lib.rs).
 * The link: the objects and the library members they need (library.c),
 * the layout (layout.c), the relocated contents (relocate.c), the file
 * (write.c). Errors are reported as merlin-ld reports them; a link with
 * errors writes nothing. */
#include "ld.h"

static const char USAGE[] =
    "usage: ld [options] input.o ...\n"
    "\n"
    "  -o FILE        output executable (default: a.out)\n"
    "  -T FILE        linker script (default: .text at 0x80000000, then .rodata, .data, .bss)\n"
    "  -e SYMBOL      entry point symbol or address (overrides ENTRY in the script)\n"
    "  -S, --strip-debug\n"
    "                 leave out the debug information (.debug_* sections)\n"
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
    uchar *data = xmalloc(st.size + 1);
    uint done = 0;
    while (done < st.size) {
        int n = read(fd, data + done, st.size - done);
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
    int fd = open(path, O_WRONLY | O_CREAT | O_TRUNC);
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
            fprintf(2, "ld: cannot read %s: %s\n", file, strerror(err));
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

/* An input file of the command line, and whether it is a library member. */
struct arg_input {
    char *name;
    int lib;
};

int main(int argc, char **argv) {
    char **args;
    uint nargs;
    if (!expand(argc, argv, &args, &nargs)) {
        return 2;
    }
    struct arg_input *names = xcalloc((nargs + 1) * sizeof(struct arg_input));
    uint nnames = 0;
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
    }
    if (in_lib) {
        fprintf(2, "ld: --start-lib without --end-lib\n");
        return 2;
    }
    uint nfiles = 0;
    for (uint i = 0; i < nnames; i++) {
        nfiles += !names[i].lib;
    }
    if (nfiles == 0) {
        fprintf(2, "ld: no input files\n%s\n", USAGE);
        return 2;
    }

    struct input *files = xcalloc((nnames + 1) * sizeof(struct input));
    struct input *library = xcalloc((nnames + 1) * sizeof(struct input));
    uint nlibrary = 0;
    nfiles = 0;
    for (uint i = 0; i < nnames; i++) {
        struct input *in = names[i].lib ? &library[nlibrary++] : &files[nfiles++];
        int err;
        in->name = names[i].name;
        in->data = read_file(in->name, &in->len, &err);
        if (in->data == NULL) {
            fprintf(2, "ld: cannot read %s: %s\n", in->name, strerror(err));
            return 2;
        }
    }
    char *script_text = NULL;
    if (script != NULL) {
        uint len;
        int err;
        script_text = (char *)read_file(script, &len, &err);
        if (script_text == NULL) {
            fprintf(2, "ld: cannot read %s: %s\n", script, strerror(err));
            return 2;
        }
    }

    uint nobjects;
    struct object *objects = parse_with_needed(files, nfiles, library, nlibrary, &nobjects);
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
        fprintf(2, "ld: cannot write %s: %s\n", output, strerror(r));
        return 2;
    }
    return 0;
}
