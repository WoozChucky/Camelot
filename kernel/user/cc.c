/* cc: the C compiler driver (#242), as clang's driver is clang's: it runs
 * the stages that build a program, each a program of its own, and checks
 * each one's exit status.
 *
 *   cc [-c|-S|-E] [-o OUT] [-g] [-I DIR] [-D NAME[=VALUE]] [-U NAME]
 *      [-L DIR] [-l NAME] [-Wl,ARG,...] [-x LANG] [-P] [-v] [-###]
 *      [@FILE] FILE...
 *
 * The stages, in the directory cc itself was started from (/bin):
 *
 *   cc1 FILE.c      the compiler proper (kernel/user/cc/, merlin in C): C
 *                   to assembly; with -E, a .S file preprocessed too
 *   as FILE.s       the assembler: assembly to an object
 *   ld FILE.o...    the linker: objects and libraries to a program
 *
 * which is how clang's driver runs them with -fno-integrated-as: cc1
 * writes assembly, `as` assembles it, `ld` links. Each input file goes
 * through the stages its language needs, up to the last one asked for
 * (-E preprocess, -S compile, -c assemble, else link): a .c file is C, a
 * .S file assembly to preprocess first (cc1 -E -x assembler-with-cpp), a
 * .s file assembly, and anything else an object or a library for the
 * linker; -x LANG says what the files after it are instead. What a stage
 * writes for the next one is a temporary file in $TMPDIR (/tmp unless the
 * environment says otherwise), named after its input as clang names them
 * (`hello-1a2b3c.s`), made with O_EXCL so that it is nobody else's, and
 * removed when cc is done, whether the stages succeeded or not.
 *
 * A program is linked as the host links the user programs
 * (crates/xtask/src/userprogs.rs), from the root image's /usr/lib:
 *
 *   ld -T /usr/lib/user.ld /usr/lib/crt0.o [-L DIR...] -L/usr/lib
 *      FILE.o... [-lNAME...] -lc -o OUT
 *
 * so that `cc -o wc wc.c` on Camelot makes /bin/wc byte for byte. cc1 looks
 * for headers in /usr/include after the -I directories, as clang looks in
 * its system directories after them.
 *
 * Its options and messages are clang's (`cc: error: no such file or
 * directory: 'x.c'`, `cc: error: linker command failed with exit code 1
 * (use -v to see invocation)`): with -### it prints the stages' command
 * lines and runs none, with -v it prints each one as it runs it. A stage
 * that fails stops the stages after it that need its output (the link,
 * for one); the others go on, as in clang, and cc exits with the first
 * failure's status. What a failed stage was to make for the user (the -o
 * file) is removed, as clang removes it.
 *
 * Camelot's exec takes at most MAXARG (16) arguments of ARG_MAX bytes
 * together, so a stage's command line that is longer is given to it in a
 * response file (`ld @/tmp/response-....txt`, a word a line), as clang's
 * driver does when a command line is too long for the system; cc1 and ld
 * read them. cc reads them too: `@FILE` on its own command line stands for
 * the words of FILE, which is how to give it more than the shell's line
 * takes. */
#include "user.h"

/* What -v and -### say first, as clang does: its version, the target
 * (`--target=riscv64`), the thread model and the directory it runs the
 * stages from. */
#define VERSION "0.1.0"
#define TARGET "riscv64"

/* What a program is built from, on the root image (crates/xtask/src/fs.rs,
 * usr_files): the headers, the start code, the C library and the linker
 * script. */
#define INCLUDE_DIR "/usr/include"
#define LIB_DIR "/usr/lib"
#define CRT0 "/usr/lib/crt0.o"
#define LINKER_SCRIPT "/usr/lib/user.ld"

/* Where temporary files go when the environment names no directory. */
#define DEFAULT_TMPDIR "/tmp"

/* How many random hexadecimal digits a temporary file's name has
 * (`hello-1a2b3c.s`), and how many names cc tries before giving up, as
 * LLVM's createUniqueFile does. */
#define TEMP_DIGITS 6
#define TEMP_TRIES 128

/* ---- Lists of strings ---- */

/* A growing list of strings, NULL after the last (an argv). */
struct strv {
    char **v;
    int n;
    int cap;
};

static void oom(void) {
    fprintf(STDERR, "cc: error: out of memory\n");
    exit(1);
}

static void push(struct strv *s, const char *x) {
    if (s->n + 1 >= s->cap) {
        s->cap = s->cap == 0 ? 16 : 2 * s->cap;
        s->v = realloc(s->v, (size_t)s->cap * sizeof(char *));
        if (s->v == NULL) {
            oom();
        }
    }
    s->v[s->n++] = (char *)x;
    s->v[s->n] = NULL;
}

/* A new string, the concatenation of a, b and c (any of them "" or NULL). */
static char *concat(const char *a, const char *b, const char *c) {
    size_t la = a != NULL ? strlen(a) : 0;
    size_t lb = b != NULL ? strlen(b) : 0;
    size_t lc = c != NULL ? strlen(c) : 0;
    char *s = malloc(la + lb + lc + 1);
    if (s == NULL) {
        oom();
    }
    memcpy(s, a != NULL ? a : "", la);
    memcpy(s + la, b != NULL ? b : "", lb);
    memcpy(s + la + lb, c != NULL ? c : "", lc);
    s[la + lb + lc] = 0;
    return s;
}

/* A copy of the n bytes at s, NUL-terminated. */
static char *copy_n(const char *s, size_t n) {
    char *c = malloc(n + 1);
    if (c == NULL) {
        oom();
    }
    memcpy(c, s, n);
    c[n] = 0;
    return c;
}

static int starts_with(const char *s, const char *prefix) {
    return strncmp(s, prefix, strlen(prefix)) == 0;
}

static int ends_with(const char *s, const char *suffix) {
    size_t n = strlen(s);
    size_t k = strlen(suffix);
    return n >= k && strcmp(s + n - k, suffix) == 0;
}

/* ---- Diagnostics, as clang's driver prints them ---- */

/* Errors said so far: with any, cc runs nothing. */
static int errors;

static void error(const char *fmt, const char *a) {
    fprintf(STDERR, "cc: error: ");
    fprintf(STDERR, fmt, a);
    fprintf(STDERR, "\n");
    errors++;
}

static void warning(const char *fmt, const char *a) {
    fprintf(STDERR, "cc: warning: ");
    fprintf(STDERR, fmt, a);
    fprintf(STDERR, " [-Wunused-command-line-argument]\n");
}

/* ---- Paths ---- */

/* The directory cc runs in, from malloc, or NULL if it can't be found. */
static char *current_dir(void) {
    for (size_t size = 256; size <= 4096; size *= 2) {
        char *buf = malloc(size);
        if (buf == NULL) {
            oom();
        }
        if (getcwd(buf, size) > 0) {
            return buf;
        }
        free(buf);
    }
    return NULL;
}

/* `path` made absolute (from the current directory) and with its `.` and
 * `..` taken out, from malloc: what clang's driver makes of the path it
 * was started by to find its own directory. */
static char *absolute(const char *path) {
    char *full;
    if (path[0] == '/') {
        full = concat(path, NULL, NULL);
    } else {
        char *cwd = current_dir();
        full = concat(cwd != NULL ? cwd : "/", "/", path);
        free(cwd);
    }
    /* Rewritten in place: each name is copied down over what `.` and `..`
     * took out; out[0] is the root's '/'. */
    size_t len = strlen(full);
    size_t o = 0;
    for (size_t i = 0; i < len;) {
        while (i < len && full[i] == '/') {
            i++;
        }
        size_t start = i;
        while (i < len && full[i] != '/') {
            i++;
        }
        size_t n = i - start;
        if (n == 0 || (n == 1 && full[start] == '.')) {
            continue;
        }
        if (n == 2 && full[start] == '.' && full[start + 1] == '.') {
            /* Back to the '/' before the last name written (none at the
             * root, where `..` is the root). */
            while (o > 0) {
                o--;
                if (full[o] == '/') {
                    break;
                }
            }
            continue;
        }
        full[o] = '/';
        memmove(full + o + 1, full + start, n);
        o += 1 + n;
    }
    if (o == 0) {
        full[o++] = '/';
    }
    full[o] = 0;
    return full;
}

/* The directory part of absolute path `path` (`/bin` of `/bin/cc`), from
 * malloc. */
static char *dir_of(const char *path) {
    const char *slash = strrchr(path, '/');
    if (slash == path) {
        return concat("/", NULL, NULL);
    }
    return copy_n(path, (size_t)(slash - path));
}

/* The last part of `path`, after its last '/'. */
static const char *base_name(const char *path) {
    const char *slash = strrchr(path, '/');
    return slash != NULL ? slash + 1 : path;
}

/* The last part of `path` without its extension (`hello` of `src/hello.c`),
 * from malloc. */
static char *stem(const char *path) {
    const char *base = base_name(path);
    const char *dot = strrchr(base, '.');
    return copy_n(base, dot != NULL && dot != base ? (size_t)(dot - base) : strlen(base));
}

/* ---- Random names for temporary files ---- */

/* A splitmix64 generator, seeded from the 16 random bytes the kernel gives
 * every program (AT_RANDOM): different names for every run. */
static unsigned long random_state;

static unsigned long next_random(void) {
    random_state += 0x9e3779b97f4a7c15UL;
    unsigned long z = random_state;
    z = (z ^ (z >> 30)) * 0xbf58476d1ce4e5b9UL;
    z = (z ^ (z >> 27)) * 0x94d049bb133111ebUL;
    return z ^ (z >> 31);
}

/* What the auxiliary vector says (docs/camelot-syscalls.md, "Starting a
 * program"): it follows envp's NULL, pairs of a type and a value. Seeds the
 * generator from AT_RANDOM and returns AT_EXECFN, the path the program was
 * started with (NULL if there is none). */
static const char *read_auxv(char **envp) {
    char **e = envp;
    while (*e != NULL) {
        e++;
    }
    unsigned long *aux = (unsigned long *)(e + 1);
    const char *execfn = NULL;
    for (; aux[0] != AT_NULL; aux += 2) {
        if (aux[0] == AT_RANDOM) {
            const unsigned char *r = (const unsigned char *)aux[1];
            for (int i = 0; i < 16; i++) {
                random_state = random_state * 131 + r[i];
            }
        } else if (aux[0] == AT_EXECFN) {
            execfn = (const char *)aux[1];
        }
    }
    return execfn;
}

/* ---- Temporary files ---- */

/* Every temporary file cc made: removed when it is done. */
static struct strv temps;

/* The directory for temporary files: the first of TMPDIR, TMP, TEMP and
 * TEMPDIR the environment gives, as LLVM looks for one on a Unix, else
 * /tmp. */
static const char *temp_dir(void) {
    const char *names[] = {"TMPDIR", "TMP", "TEMP", "TEMPDIR"};
    for (int i = 0; i < 4; i++) {
        const char *d = getenv(names[i]);
        if (d != NULL && d[0] != 0) {
            return d;
        }
    }
    return DEFAULT_TMPDIR;
}

/* Makes a new, empty temporary file for `prefix` (an input's stem) with
 * extension `suffix`, as clang names them: `DIR/PREFIX-XXXXXX.SUFFIX`, six
 * random hexadecimal digits. A Camelot file name has at most DIRSIZ (14)
 * bytes, so the prefix is cut to fit: `hello-1a2b3c.s`, `respo..` cut to
 * `res-1a2b3c.txt`. The file is made with O_EXCL, a new name tried while
 * the name is taken, and remembered for removal. Returns its path, or NULL
 * having said why there is none. */
static char *make_temp(const char *prefix, const char *suffix) {
    const char *dir = temp_dir();
    int room = DIRSIZ - 1 - TEMP_DIGITS - 1 - (int)strlen(suffix);
    int keep = (int)strlen(prefix);
    if (keep > room) {
        keep = room > 0 ? room : 0;
    }
    int err = 0;
    for (int tries = 0; tries < TEMP_TRIES; tries++) {
        char name[DIRSIZ + 1];
        int n = 0;
        memcpy(name, prefix, (size_t)keep);
        n = keep;
        name[n++] = '-';
        unsigned long r = next_random();
        for (int i = 0; i < TEMP_DIGITS; i++) {
            name[n++] = "0123456789abcdef"[r & 15];
            r >>= 4;
        }
        name[n] = 0;
        char *path = concat(dir, "/", name);
        char *full = concat(path, ".", suffix);
        free(path);
        int fd = open(full, O_WRONLY | O_CREAT | O_EXCL, 0600);
        if (fd >= 0) {
            close(fd);
            push(&temps, full);
            return full;
        }
        free(full);
        err = fd;
        if (err != -EEXIST) {
            break;
        }
    }
    char *pattern = concat(dir, "/", prefix);
    fprintf(STDERR, "cc: error: unable to make temporary file %s: %s\n", pattern, strerror(-err));
    free(pattern);
    errors++;
    return NULL;
}

static void remove_temps(void) {
    for (int i = 0; i < temps.n; i++) {
        unlink(temps.v[i]);
    }
}

/* ---- Response files: cc's own ---- */

static int is_white(char c) {
    return c == ' ' || c == '\t' || c == '\n' || c == '\r' || c == '\f' || c == '\v';
}

/* The whole of file `path`, NUL-terminated, from malloc; NULL if it can't
 * be read. */
static char *read_whole(const char *path) {
    int fd = open(path, O_RDONLY);
    if (fd < 0) {
        return NULL;
    }
    struct stat st;
    if (fstat(fd, &st) < 0 || st.type == T_DIR) {
        close(fd);
        return NULL;
    }
    size_t cap = 1024;
    size_t len = 0;
    char *text = malloc(cap);
    if (text == NULL) {
        oom();
    }
    for (;;) {
        if (len + 1 == cap) {
            cap *= 2;
            text = realloc(text, cap);
            if (text == NULL) {
                oom();
            }
        }
        long n = read(fd, text + len, cap - 1 - len);
        if (n < 0) {
            close(fd);
            free(text);
            return NULL;
        }
        if (n == 0) {
            break;
        }
        len += (size_t)n;
    }
    close(fd);
    text[len] = 0;
    return text;
}

/* The response files being read, by absolute path, innermost last: one
 * that names itself, directly or through others, can't be expanded. */
static struct strv expanding;

/* Puts `arg` in `out`, or for `@FILE` the words of FILE, separated by white
 * space (without quoting, as Camelot's tools read a response file: there
 * is no shell quoting to give a word with a blank), each expanded in turn,
 * as clang reads its response files: a file that can't be read leaves the
 * argument as it is (an input file of that name, then), and a nested one
 * is a path from the current directory. Returns 0, or -1 for a file that
 * names itself, having said so as clang does. */
static int expand_arg(const char *arg, struct strv *out) {
    if (arg[0] != '@' || arg[1] == 0) {
        push(out, arg);
        return 0;
    }
    char *text = read_whole(arg + 1);
    if (text == NULL) {
        push(out, arg);
        return 0;
    }
    char *path = absolute(arg + 1);
    for (int i = 0; i < expanding.n; i++) {
        if (strcmp(expanding.v[i], path) == 0) {
            fprintf(STDERR, "recursive expansion of: '%s'\n", path);
            return -1;
        }
    }
    push(&expanding, path);
    char *p = text;
    for (;;) {
        while (*p != 0 && is_white(*p)) {
            p++;
        }
        if (*p == 0) {
            break;
        }
        char *w = p;
        while (*p != 0 && !is_white(*p)) {
            p++;
        }
        if (*p != 0) {
            *p++ = 0;
        }
        if (expand_arg(w, out) < 0) {
            return -1;
        }
    }
    expanding.n--;
    return 0;
}

/* ---- The command line ---- */

/* The last stage to run: -E, -S, -c, or the link. */
enum { PREPROCESS, COMPILE, ASSEMBLE, LINK };

/* An input file's language: what clang calls it for -x. LANG_BAD marks
 * the place of a -x whose language cc doesn't know, said with the inputs'
 * errors, in their order. */
enum { LANG_BAD = -2, LANG_NONE = -1, LANG_C, LANG_ASM, LANG_ASM_CPP, LANG_OBJECT };

/* An input of the command line, in its order: a file (with its language),
 * or a linker input that is no file, `-lNAME` or `-Wl,ARG,...`. */
enum { IN_FILE, IN_LIB, IN_WL };

struct input {
    int kind;
    const char *arg; /* the file; -lNAME as the linker takes it; -Wl,... as given */
    int lang;
    int missing;     /* a file that isn't there */
};

/* What the command line asks for. */
static int phase = LINK;
static const char *output;
static int print_only; /* -### */
static int verbose;    /* -v */
static int debug;      /* -g */
static int no_line_markers; /* -P */
static const char *comp_dir; /* -fdebug-compilation-dir */
static struct strv include_dirs;  /* -I, in order */
static struct strv macros;        /* -D NAME and -U NAME, in order, as "-D" "NAME" pairs */
static struct strv lib_dirs;      /* -L, as -LDIR */
static struct input *inputs;
static int ninputs;
/* -c, -S and -L as the user wrote them, if given: clang warns of those that
 * mean nothing for the last stage asked for (-c with -S, -L without the
 * link), if nothing else went wrong. */
static const char *c_arg;
static const char *s_arg;
static struct strv l_dir_args;
/* The last -x (as written) and whether an input came after it. */
static const char *last_x;
static int input_after_x;

/* The directory the stages' programs are in: cc's own. */
static const char *installed_dir;

static void usage(void) {
    printf("OVERVIEW: cc, the C compiler driver of Camelot\n"
           "\n"
           "USAGE: cc [options] file...\n"
           "\n"
           "OPTIONS:\n"
           "  -###                    Print (but do not run) the commands to run for this compilation\n"
           "  -c                      Only run preprocess, compile, and assemble steps\n"
           "  -D <macro>=<value>      Define <macro> to <value> (or 1 if <value> omitted)\n"
           "  -E                      Only run the preprocessor\n"
           "  -fdebug-compilation-dir=<value>\n"
           "                          The compilation directory to embed in the debug info\n"
           "  -g                      Generate source-level debug information\n"
           "  --help                  Display available options\n"
           "  -I <dir>                Add directory to the end of the list of include search paths\n"
           "  -L <dir>                Add directory to library search path\n"
           "  -l <name>               Link with the library lib<name>.a of the library search path\n"
           "  -o <file>               Write output to <file>\n"
           "  -P                      Disable linemarker output in -E mode\n"
           "  -S                      Only run preprocess and compilation steps\n"
           "  -U <macro>              Undefine macro <macro>\n"
           "  -v                      Show commands to run and use verbose output\n"
           "  --version               Print version information\n"
           "  -Wl,<arg>               Pass the comma separated arguments in <arg> to the linker\n"
           "  -x <language>           Treat subsequent input files as having type <language>\n"
           "  @<file>                 Read more arguments from <file>\n");
}

/* The language of a file named `name`, by its extension, as clang gives
 * it. */
static int lang_of(const char *name) {
    if (ends_with(name, ".c")) {
        return LANG_C;
    }
    if (ends_with(name, ".s")) {
        return LANG_ASM;
    }
    if (ends_with(name, ".S") || ends_with(name, ".sx")) {
        return LANG_ASM_CPP;
    }
    return LANG_OBJECT;
}

/* The language -x names, LANG_NONE for `none`, or LANG_BAD if it is none
 * cc knows. */
static int lang_named(const char *name) {
    if (strcmp(name, "c") == 0) {
        return LANG_C;
    }
    if (strcmp(name, "assembler") == 0) {
        return LANG_ASM;
    }
    if (strcmp(name, "assembler-with-cpp") == 0) {
        return LANG_ASM_CPP;
    }
    if (strcmp(name, "none") == 0) {
        return LANG_NONE;
    }
    return LANG_BAD;
}

/* The value of option `flag` (`-o`, `-I`...) at args[*i]: what follows the
 * flag in the same argument, or the next argument, which *i then moves to.
 * NULL, having said so, if there is none. `*written` is the option as the
 * user wrote it, for the warnings. */
static const char *value(char **args, int nargs, int *i, const char *flag, const char **written) {
    const char *a = args[*i];
    size_t k = strlen(flag);
    if (a[k] != 0) {
        *written = a;
        return a + k;
    }
    if (*i + 1 >= nargs) {
        error("argument to '%s' is missing (expected 1 value)", flag);
        return NULL;
    }
    *i += 1;
    *written = concat(flag, " ", args[*i]);
    return args[*i];
}

static void add_input(int kind, const char *arg, int lang) {
    inputs[ninputs].kind = kind;
    inputs[ninputs].arg = arg;
    inputs[ninputs].lang = lang;
    inputs[ninputs].missing = 0;
    ninputs++;
}

/* Reads the options (args[1..nargs)). Returns 0, or 1 for --help or
 * --version, which have been answered. */
static int parse(char **args, int nargs) {
    inputs = malloc((size_t)nargs * sizeof(struct input));
    if (inputs == NULL) {
        oom();
    }
    int lang = LANG_NONE;
    for (int i = 1; i < nargs; i++) {
        char *a = args[i];
        const char *written = a;
        const char *v;
        if (a[0] != '-' || a[1] == 0) {
            /* A file (`-` too, a name like any other here). */
            add_input(IN_FILE, a, lang != LANG_NONE ? lang : lang_of(a));
            input_after_x = 1;
        } else if (strcmp(a, "-###") == 0) {
            print_only = 1;
        } else if (strcmp(a, "-v") == 0) {
            verbose = 1;
        } else if (strcmp(a, "-c") == 0) {
            c_arg = a;
            if (phase > ASSEMBLE) {
                phase = ASSEMBLE;
            }
        } else if (strcmp(a, "-S") == 0) {
            s_arg = a;
            if (phase > COMPILE) {
                phase = COMPILE;
            }
        } else if (strcmp(a, "-E") == 0) {
            phase = PREPROCESS;
        } else if (strcmp(a, "-g") == 0) {
            debug = 1;
        } else if (strcmp(a, "-P") == 0) {
            no_line_markers = 1;
        } else if (strcmp(a, "--help") == 0 || strcmp(a, "-help") == 0) {
            usage();
            return 1;
        } else if (strcmp(a, "--version") == 0) {
            printf("cc version %s\nTarget: %s\nThread model: posix\nInstalledDir: %s\n", VERSION, TARGET,
                   installed_dir);
            return 1;
        } else if (starts_with(a, "-fdebug-compilation-dir")) {
            const char *rest = a + strlen("-fdebug-compilation-dir");
            if (rest[0] == '=') {
                comp_dir = rest + 1;
            } else if (rest[0] != 0) {
                error("unknown argument: '%s'", a);
            } else if ((v = value(args, nargs, &i, "-fdebug-compilation-dir", &written)) != NULL) {
                comp_dir = v;
            }
        } else if (starts_with(a, "-Wl,")) {
            add_input(IN_WL, a, LANG_NONE);
        } else if (a[1] == 'o' || a[1] == 'I' || a[1] == 'D' || a[1] == 'U' || a[1] == 'L' || a[1] == 'l' ||
                   a[1] == 'x') {
            char flag[3] = {'-', a[1], 0};
            v = value(args, nargs, &i, flag, &written);
            if (v == NULL) {
                continue;
            }
            switch (a[1]) {
            case 'o':
                output = v;
                break;
            case 'I':
                push(&include_dirs, v);
                break;
            case 'D':
            case 'U':
                push(&macros, a[1] == 'D' ? "-D" : "-U");
                push(&macros, v);
                break;
            case 'L':
                push(&lib_dirs, concat("-L", v, NULL));
                push(&l_dir_args, written);
                break;
            case 'l':
                add_input(IN_LIB, concat("-l", v, NULL), LANG_NONE);
                break;
            default: /* 'x' */
                lang = lang_named(v);
                if (lang == LANG_BAD) {
                    /* Said once the header is out, with the inputs'
                     * errors; the files after it are the linker's, as
                     * clang takes them. */
                    lang = LANG_OBJECT;
                    add_input(IN_FILE, v, LANG_BAD);
                }
                last_x = written;
                input_after_x = 0;
                break;
            }
        } else {
            error("unknown argument: '%s'", a);
        }
    }
    return 0;
}

/* ---- Stages ---- */

enum { CC1, AS, LD };

/* What clang calls each stage when it fails ("clang frontend", with cc for
 * clang): `X command failed with exit code N`. */
static const char *tool_name(int tool) {
    return tool == CC1 ? "cc frontend" : tool == AS ? "assembler" : "linker";
}

/* A stage to run: its command line (argv[0] the program's path), the file
 * it makes that the user asked for, removed if it fails (NULL for a
 * temporary file, or standard output), and the input it is a stage of (-1
 * for the link, which needs them all). */
struct job {
    int tool;
    struct strv argv;
    const char *result;
    int input;
};

static struct job *jobs;
static int njobs;

static struct job *new_job(int tool, int input) {
    struct job *j = &jobs[njobs++];
    memset(j, 0, sizeof *j);
    j->tool = tool;
    j->input = input;
    const char *name = tool == CC1 ? "cc1" : tool == AS ? "as" : "ld";
    push(&j->argv, concat(installed_dir, strcmp(installed_dir, "/") == 0 ? "" : "/", name));
    return j;
}

/* cc1 on input `in` (of language `lang`), preprocessing only if
 * `preprocess`, writing `out` (standard output if NULL): the options in
 * clang's order, the mode, the debug information, -I with /usr/include
 * after the user's, -D and -U in their order, the output, the language. */
static void cc1_job(int k, const char *in, int lang, int preprocess, const char *out, const char *result) {
    struct job *j = new_job(CC1, k);
    if (preprocess) {
        push(&j->argv, "-E");
    }
    if (no_line_markers) {
        push(&j->argv, "-P");
    }
    if (debug) {
        push(&j->argv, "-g");
    }
    if (comp_dir != NULL) {
        push(&j->argv, concat("-fdebug-compilation-dir=", comp_dir, NULL));
    }
    for (int i = 0; i < include_dirs.n; i++) {
        push(&j->argv, "-I");
        push(&j->argv, include_dirs.v[i]);
    }
    push(&j->argv, "-I");
    push(&j->argv, INCLUDE_DIR);
    for (int i = 0; i < macros.n; i++) {
        push(&j->argv, macros.v[i]);
    }
    if (out != NULL) {
        push(&j->argv, "-o");
        push(&j->argv, out);
    }
    push(&j->argv, "-x");
    push(&j->argv, lang == LANG_C ? "c" : "assembler-with-cpp");
    push(&j->argv, in);
    j->result = result;
}

static void as_job(int k, const char *in, const char *out, const char *result) {
    struct job *j = new_job(AS, k);
    push(&j->argv, "-o");
    push(&j->argv, out);
    push(&j->argv, in);
    j->result = result;
}

/* The file the user gets for input `in` at the last stage, by its stem and
 * `ext` in the current directory (`hello.o` of `src/hello.c`), or -o's. */
static const char *result_name(const char *in, const char *ext) {
    if (output != NULL) {
        return output;
    }
    return concat(stem(in), ext, NULL);
}

/* Works out the stages, as clang's driver does, in the order they run:
 * each input's, then the link. Says what clang says of inputs that mean
 * nothing for the last stage. */
static void build_jobs(void) {
    jobs = malloc(((size_t)ninputs * 2 + 1) * sizeof(struct job));
    if (jobs == NULL) {
        oom();
    }
    struct strv objects; /* the linker's inputs, in order */
    memset(&objects, 0, sizeof objects);
    for (int k = 0; k < ninputs; k++) {
        struct input *in = &inputs[k];
        if (in->missing || in->lang == LANG_BAD) {
            continue;
        }
        if (in->kind != IN_FILE || in->lang == LANG_OBJECT) {
            if (phase != LINK) {
                warning("%s: 'linker' input unused", in->arg);
            } else if (in->kind == IN_WL) {
                /* -Wl,a,b: the words between the commas. */
                const char *p = in->arg + 4;
                for (;;) {
                    const char *comma = strchr(p, ',');
                    size_t n = comma != NULL ? (size_t)(comma - p) : strlen(p);
                    if (n > 0) {
                        push(&objects, copy_n(p, n));
                    }
                    if (comma == NULL) {
                        break;
                    }
                    p = comma + 1;
                }
            } else {
                push(&objects, in->arg);
            }
            continue;
        }
        const char *file = in->arg;
        char *prefix = stem(file);
        if (in->lang == LANG_ASM && phase < ASSEMBLE) {
            warning("%s: 'assembler' input unused", file);
            continue;
        }
        /* Preprocessing only: -E, or -S of assembly that is preprocessed
         * (which clang writes to standard output, as -E would). */
        if (phase == PREPROCESS || (phase == COMPILE && in->lang == LANG_ASM_CPP)) {
            cc1_job(k, file, in->lang, 1, output, output);
            continue;
        }
        if (phase == COMPILE) {
            const char *out = result_name(file, ".s");
            cc1_job(k, file, LANG_C, 0, out, out);
            continue;
        }
        /* Assembled: from the file itself, or cc1's assembly in a
         * temporary file. */
        const char *asm_file = file;
        if (in->lang != LANG_ASM) {
            char *s = make_temp(prefix, "s");
            if (s == NULL) {
                return;
            }
            cc1_job(k, file, in->lang, in->lang == LANG_ASM_CPP, s, NULL);
            asm_file = s;
        }
        if (phase == ASSEMBLE) {
            const char *out = result_name(file, ".o");
            as_job(k, asm_file, out, out);
        } else {
            char *o = make_temp(prefix, "o");
            if (o == NULL) {
                return;
            }
            as_job(k, asm_file, o, NULL);
            push(&objects, o);
        }
    }
    if (phase != LINK) {
        return;
    }
    /* The link, as the host links the user programs. */
    struct job *j = new_job(LD, -1);
    push(&j->argv, "-T");
    push(&j->argv, LINKER_SCRIPT);
    push(&j->argv, CRT0);
    for (int i = 0; i < lib_dirs.n; i++) {
        push(&j->argv, lib_dirs.v[i]);
    }
    push(&j->argv, concat("-L", LIB_DIR, NULL));
    for (int i = 0; i < objects.n; i++) {
        push(&j->argv, objects.v[i]);
    }
    push(&j->argv, "-lc");
    push(&j->argv, "-o");
    const char *out = output != NULL ? output : "a.out";
    push(&j->argv, out);
    j->result = out;
}

/* ---- Running them ---- */

/* Whether `s` must be quoted to be read back as one word, and so is with
 * -v: it holds a blank, a quote, a backslash or a '$'. */
static int needs_quotes(const char *s) {
    for (; *s != 0; s++) {
        if (*s == ' ' || *s == '"' || *s == '\\' || *s == '$') {
            return 1;
        }
    }
    return 0;
}

/* Prints `s` as clang prints a command's argument: in double quotes (with a
 * backslash before a quote, a backslash or a '$') if `quote` or if it
 * needs them, else as it is. */
static void print_arg(const char *s, int quote) {
    if (!quote && !needs_quotes(s)) {
        fprintf(STDERR, "%s", s);
        return;
    }
    fprintf(STDERR, "\"");
    for (; *s != 0; s++) {
        if (*s == '"' || *s == '\\' || *s == '$') {
            fprintf(STDERR, "\\");
        }
        char c[2] = {*s, 0};
        fprintf(STDERR, "%s", c);
    }
    fprintf(STDERR, "\"");
}

/* Prints a command line as clang does: a blank first, the program always
 * quoted, the arguments too with -### (`quote`), and with -v only those
 * that need it. */
static void print_command(char **argv, int quote) {
    fprintf(STDERR, " ");
    print_arg(argv[0], 1);
    for (int i = 1; argv[i] != NULL; i++) {
        fprintf(STDERR, " ");
        print_arg(argv[i], quote);
    }
    fprintf(STDERR, "\n");
}

/* Whether `argv` is more than exec takes, with the environment that goes
 * with it: more than MAXARG strings, or more than ARG_MAX bytes. */
static int too_long(char **argv) {
    size_t bytes = 0;
    int n = 0;
    for (; argv[n] != NULL; n++) {
        bytes += strlen(argv[n]) + 1;
    }
    for (char **e = environ; e != NULL && *e != NULL; e++) {
        bytes += strlen(*e) + 1;
    }
    return n > MAXARG || bytes > ARG_MAX;
}

/* Writes all of `text` to `fd`. 0, or a negative error number. */
static int write_all(int fd, const char *text, size_t len) {
    while (len > 0) {
        long n = write(fd, text, len);
        if (n <= 0) {
            return n < 0 ? (int)n : -ENOSPC;
        }
        text += n;
        len -= (size_t)n;
    }
    return 0;
}

/* Runs job `j` and returns its exit status, or -1000 if it could not be
 * started, having said why. A command line too long for exec goes in a
 * response file, `@FILE` (cc1 and ld read them), which -v shows as clang
 * does. */
#define NOT_STARTED (-1000)

static int run(struct job *j) {
    char **argv = j->argv.v;
    char *short_argv[3];
    char *text = NULL;
    if (j->tool != AS && too_long(argv)) {
        /* A word a line, as Camelot's tools read them. */
        size_t len = 0;
        for (int i = 1; argv[i] != NULL; i++) {
            len += strlen(argv[i]) + 1;
        }
        text = malloc(len + 1);
        if (text == NULL) {
            oom();
        }
        size_t at = 0;
        for (int i = 1; argv[i] != NULL; i++) {
            size_t n = strlen(argv[i]);
            memcpy(text + at, argv[i], n);
            text[at + n] = '\n';
            at += n + 1;
        }
        text[at] = 0;
        char *rsp = make_temp("response", "txt");
        if (rsp == NULL) {
            return NOT_STARTED;
        }
        int fd = open(rsp, O_WRONLY | O_TRUNC);
        int r = fd < 0 ? fd : write_all(fd, text, len);
        if (fd >= 0) {
            close(fd);
        }
        if (r < 0) {
            fprintf(STDERR, "cc: error: unable to write response file %s: %s\n", rsp, strerror(-r));
            return NOT_STARTED;
        }
        short_argv[0] = argv[0];
        short_argv[1] = concat("@", rsp, NULL);
        short_argv[2] = NULL;
        argv = short_argv;
    }
    if (verbose) {
        print_command(argv, 0);
        if (text != NULL) {
            fprintf(STDERR, " Arguments passed via response file:\n%s (end of response file)\n", text);
        }
    }
    int pid = spawn(argv[0], argv, environ);
    if (pid < 0) {
        fprintf(STDERR, "cc: error: unable to execute command: %s\n", strerror(-pid));
        return NOT_STARTED;
    }
    int status = 0;
    if (wait(pid, &status) != pid) {
        return NOT_STARTED;
    }
    return status;
}

/* Runs the stages in order, but none whose input's earlier stage failed
 * (the link if any did), as clang does. Says why a stage failed when its
 * own messages may not (a compiler that exits 1 has said why; the
 * assembler and the linker are always followed by cc's word), removes the
 * file it was to make, and returns the first failure's status (1 for one
 * that could not start or was killed), or 0. */
static int run_jobs(void) {
    int status = 0;
    int *failed = calloc((size_t)ninputs + 1, sizeof(int));
    int any_failed = 0;
    if (failed == NULL) {
        oom();
    }
    for (int i = 0; i < njobs; i++) {
        struct job *j = &jobs[i];
        if (j->input >= 0 ? failed[j->input] : any_failed) {
            continue;
        }
        int r = run(j);
        if (r == 0) {
            continue;
        }
        if (r == NOT_STARTED) {
            fprintf(STDERR, "cc: error: %s command failed with exit code 1 (use -v to see invocation)\n",
                    tool_name(j->tool));
        } else if (r < 0) {
            /* Killed: Camelot's exit status for a process ended from
             * outside (or by a fault), where Linux has signals. */
            fprintf(STDERR, "cc: error: %s command failed due to signal (use -v to see invocation)\n",
                    tool_name(j->tool));
        } else if (j->tool != CC1 || r != 1) {
            fprintf(STDERR, "cc: error: %s command failed with exit code %d (use -v to see invocation)\n",
                    tool_name(j->tool), r);
        }
        if (j->result != NULL) {
            unlink(j->result);
        }
        if (j->input >= 0) {
            failed[j->input] = 1;
        }
        any_failed = 1;
        if (status == 0) {
            status = r > 0 ? r : 1;
        }
    }
    free(failed);
    return status;
}

/* ---- main ---- */

int main(int argc, char **argv, char **envp) {
    const char *execfn = read_auxv(envp);
    /* The stages are next to cc, as clang's are next to clang: in the
     * directory of the path it was started by (/bin, as the shell finds
     * it). */
    if (execfn == NULL || strchr(execfn, '/') == NULL) {
        execfn = "/bin/cc";
    }
    installed_dir = dir_of(absolute(execfn));

    struct strv args;
    memset(&args, 0, sizeof args);
    for (int i = 0; i < argc; i++) {
        if (expand_arg(argv[i], &args) < 0) {
            return 1;
        }
    }
    if (parse(args.v, args.n)) {
        return 0;
    }
    if (print_only || verbose) {
        fprintf(STDERR, "cc version %s\nTarget: %s\nThread model: posix\nInstalledDir: %s\n", VERSION, TARGET,
                installed_dir);
    }
    /* The inputs: a language -x doesn't know, and files that aren't
     * there. */
    int files = 0;
    for (int k = 0; k < ninputs; k++) {
        struct input *in = &inputs[k];
        if (in->lang == LANG_BAD) {
            error("language not recognized: '%s'", in->arg);
            continue;
        }
        struct stat st;
        if (in->kind == IN_FILE && stat(in->arg, &st) < 0) {
            error("no such file or directory: '%s'", in->arg);
            in->missing = 1;
            continue;
        }
        files++;
    }
    if (files == 0) {
        if (!print_only && !(verbose && ninputs == 0)) {
            error("no input files", NULL);
        }
        return errors > 0 ? 1 : 0;
    }
    if (last_x != NULL && !input_after_x) {
        warning("'%s' after last input file has no effect", last_x);
    }
    /* The stages, and clang's warnings of inputs that mean nothing for the
     * last one asked for. */
    build_jobs();
    /* -o names one file: with -c, -S or -E, only one input may make one
     * (said once the stages are worked out, as clang does). */
    if (output != NULL && phase != LINK) {
        int outputs = 0;
        for (int k = 0; k < ninputs; k++) {
            struct input *in = &inputs[k];
            if (in->kind == IN_FILE && !in->missing && in->lang != LANG_BAD && in->lang != LANG_OBJECT &&
                !(in->lang == LANG_ASM && phase < ASSEMBLE)) {
                outputs++;
            }
        }
        if (outputs > 1) {
            error("cannot specify -o when generating multiple output files", NULL);
        }
    }
    /* Arguments that do nothing for the last stage asked for, if all else
     * is well. */
    if (errors == 0) {
        if (c_arg != NULL && phase < ASSEMBLE) {
            warning("argument unused during compilation: '%s'", c_arg);
        }
        if (s_arg != NULL && phase < COMPILE) {
            warning("argument unused during compilation: '%s'", s_arg);
        }
        if (phase != LINK) {
            for (int i = 0; i < l_dir_args.n; i++) {
                warning("argument unused during compilation: '%s'", l_dir_args.v[i]);
            }
        }
    }
    int status = errors > 0 ? 1 : 0;
    if (print_only) {
        for (int i = 0; i < njobs; i++) {
            print_command(jobs[i].argv.v, 1);
        }
    } else if (errors == 0) {
        status = run_jobs();
    }
    remove_temps();
    return status;
}
