/* cc: the C compiler driver (#242), as clang's driver is clang's: it runs
 * the stages that build a program, each a program of its own, and checks
 * each one's exit status.
 *
 *   cc [-c|-S|-E] [-o OUT] [-g] [-I DIR] [-D NAME[=VALUE]] [-U NAME]
 *      [-L DIR] [-l NAME] [-Wl,ARG,...] [-Wp,ARG,...] [-Wa,ARG,...]
 *      [-x LANG] [-P] [-O LEVEL] [-std=STD]
 *      [-w] [-W...] [-ffreestanding] [-v] [-###] [@FILE] FILE... [-- FILE...]
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
 * .i file C already preprocessed, a .S file assembly to preprocess first
 * (cc1 -E -x assembler-with-cpp), a .s file assembly, and anything else an
 * object or a library for the linker; -x LANG says what the files after it
 * are instead, and after `--` every argument is a file. What a stage
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
 * for one); the others go on, as in clang, cc says which failed once they
 * are done, and exits with the first failure's status. What a failed
 * stage was to make for the user (its -o file, if a regular file it may
 * write) is removed, as clang removes it.
 *
 * Camelot's exec takes at most MAXARG (16) arguments of ARG_MAX bytes
 * together, so a stage's command line that is longer is given to it in a
 * response file (`ld @/tmp/response-....txt`), written as clang's driver
 * writes one when a command line is too long for the system; cc1 and ld
 * read them. cc reads them too, as clang does (the C library's
 * expand_response_files): `@FILE` on its own command line stands for the
 * arguments of FILE, which is how to give it more than the shell's line
 * takes. */
#include "user.h"
#include "warning_groups.h"

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

static void remove_temps(void);

/* Out of memory: what cc made in /tmp goes, then cc. */
static void oom(void) {
    fprintf(STDERR, "cc: error: out of memory\n");
    remove_temps();
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

/* -w and the -W options, in command-line order: what clang's driver applies
 * to its own warnings as its compiler applies them to its (the C library's
 * warning_level, with clang's groups of the driver's flags,
 * warning_groups.h, generated from crates/merlin/src/warning_groups.txt). */
static struct strv diag_opts;

/* A warning with its flag, clang's: `[-Wunused-command-line-argument]` for
 * most of the driver's. Left out under -w or -Wno-FLAG (or a group of it);
 * under -Werror or -Werror=FLAG an error, `[-Werror,-WFLAG]`, which stops
 * the stages as any error does. */
static void warning_of(const char *flag, const char *fmt, const char *a) {
    int level = warning_level(diag_opts.v, diag_opts.n, flag, DRIVER_WARNING_GROUPS, NDRIVER_WARNING_GROUPS);
    if (level == WARNING_IGNORED) {
        return;
    }
    fprintf(STDERR, level == WARNING_ERROR ? "cc: error: " : "cc: warning: ");
    fprintf(STDERR, fmt, a);
    fprintf(STDERR, level == WARNING_ERROR ? " [-Werror,-W%s]\n" : " [-W%s]\n", flag);
    if (level == WARNING_ERROR) {
        errors++;
    }
}

static void warning(const char *fmt, const char *a) {
    warning_of("unused-command-line-argument", fmt, a);
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
 * bytes, so the prefix is cut to fit: `hello-1a2b3c.s`, `response` cut to
 * `res-1a2b3c.txt`. The file is made with O_EXCL, a new name tried while
 * the name is taken, and remembered for removal. Returns its path, or NULL
 * having said why there is none, as clang says it. */
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
    error("unable to make temporary file: %s", strerror(-err));
    return NULL;
}

static void remove_temps(void) {
    for (int i = 0; i < temps.n; i++) {
        unlink(temps.v[i]);
    }
}

/* ---- The command line ---- */

/* The last stage to run: -E, -S, -c, or the link. */
enum { PREPROCESS, COMPILE, ASSEMBLE, LINK };

/* An input file's language: what clang calls it for -x. LANG_BAD marks
 * the place of a -x whose language cc doesn't know, said with the inputs'
 * errors, in their order. LANG_CPP_OUTPUT is C already preprocessed (a .i
 * file), which cc1 compiles as C. */
enum { LANG_BAD = -2, LANG_NONE = -1, LANG_C, LANG_CPP_OUTPUT, LANG_ASM, LANG_ASM_CPP, LANG_OBJECT };

/* An input of the command line, in its order: a file (with its language),
 * or a linker input that is no file, `-lNAME` or `-Wl,ARG,...`. */
enum { IN_FILE, IN_LIB, IN_WL };

struct input {
    int kind;
    const char *arg; /* the file; -lNAME as the linker takes it; -Wl,... as clang renders it */
    int lang;
    int missing; /* a file that isn't there */
};

/* What clang warns of when nothing runs that it is for (`argument unused
 * during compilation: '-L/x'`): each option that may go unused, as clang
 * renders it, and what uses it. A command line that only links (its
 * inputs objects and libraries) uses the compiler's options without a
 * word, FOR_CC1's, FOR_CPP's, FOR_TOOLS' and FOR_CPP_TOOLS', as clang's
 * driver claims them then.
 * Never said: -std= (clang claims it whatever runs) and -w (which leaves
 * every warning out, this one too). */
enum {
    FOR_ASSEMBLE, /* -c: a last stage of assembling or linking */
    FOR_COMPILE,  /* -S: a last stage of compiling or later */
    FOR_LINK,     /* -L: the link */
    FOR_CC1,      /* -D, -P, -O, -W, -ffreestanding,
                     -fdebug-compilation-dir: a cc1 stage */
    FOR_CPP,      /* -U, -Wp,: a cc1 stage that preprocesses (not of C
                     preprocessed already, which clang gives none of them;
                     -D it claims for that one too) */
    FOR_TOOLS,    /* -g: a cc1 or an `as` stage, which clang gives it */
    FOR_CPP_TOOLS, /* -I: a cc1 stage that preprocesses, or an `as` one */
    FOR_AS_ARGS   /* -Wa,: a cc1 or an `as` stage, on a line that only
                     links too (it is not one of the compiler's options) */
};

struct claim {
    const char *rendered;
    int used_by;
};

/* What the command line asks for. */
static int phase = LINK;
static const char *output;
static int print_only;             /* -### */
static int verbose;                /* -v */
static int debug;                  /* -g */
static int no_line_markers;        /* -P */
static const char *comp_dir;       /* -fdebug-compilation-dir */
static const char *opt_level;      /* the last -O, as given */
static const char *std;            /* the last -std= */
static int freestanding;           /* -ffreestanding */
static struct strv warning_opts;   /* -W..., in order, but -Wl, -Wp, -Wa, */
static int no_warnings;            /* -w */
static struct strv cpp_opts;       /* -I DIR, -D NAME, -U NAME, in order, as "-I" "DIR" pairs */
static struct strv wp_args;        /* -Wp,'s arguments, for cc1, in order */
static struct strv wa_args;        /* -Wa,'s arguments, for `as`, in order */
static struct strv lib_dirs;       /* -L, as -LDIR */
static struct input *inputs;
static int ninputs;
static struct claim *claims;
static int nclaims;
/* Errors in the options themselves: with any, -### prints no stage. */
static int option_errors;
/* The last -x (as clang renders it) and whether an input came after it. */
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
           "  --                      Treat every argument after it as an input file\n"
           "  -c                      Only run preprocess, compile, and assemble steps\n"
           "  -D <macro>=<value>      Define <macro> to <value> (or 1 if <value> omitted)\n"
           "  -E                      Only run the preprocessor\n"
           "  -fdebug-compilation-dir=<value>\n"
           "                          The compilation directory to embed in the debug info\n"
           "  -ffreestanding          Assert that the compilation takes place in a freestanding environment\n"
           "  -g                      Generate source-level debug information\n"
           "  --help                  Display available options\n"
           "  -I <dir>                Add directory to the end of the list of include search paths\n"
           "  -L <dir>                Add directory to library search path\n"
           "  -l <name>               Link with the library lib<name>.a of the library search path\n"
           "  -O<level>               Optimization level (merlin does not optimize: none changes the code)\n"
           "  -o <file>               Write output to <file>\n"
           "  -P                      Disable linemarker output in -E mode\n"
           "  -S                      Only run preprocess and compilation steps\n"
           "  -std=<value>            Language standard to compile for (c99, merlin's)\n"
           "  -U <macro>              Undefine macro <macro>\n"
           "  -v                      Show commands to run and use verbose output\n"
           "  --version               Print version information\n"
           "  -W<warning>             Enable the specified warning\n"
           "  -Wa,<arg>               Pass the comma separated arguments in <arg> to the assembler\n"
           "  -Wl,<arg>               Pass the comma separated arguments in <arg> to the linker\n"
           "  -Wp,<arg>               Pass the comma separated arguments in <arg> to the preprocessor\n"
           "  -w                      Suppress all warnings\n"
           "  -x <language>           Treat subsequent input files as having type <language>\n"
           "  @<file>                 Read more arguments from <file>\n");
}

/* The language of a file named `name`, by its extension, as clang gives
 * it. */
static int lang_of(const char *name) {
    if (ends_with(name, ".c")) {
        return LANG_C;
    }
    if (ends_with(name, ".i")) {
        return LANG_CPP_OUTPUT;
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
    if (strcmp(name, "cpp-output") == 0) {
        return LANG_CPP_OUTPUT;
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
 * NULL, having said so, if there is none. */
static const char *value(char **args, int nargs, int *i, const char *flag) {
    const char *a = args[*i];
    size_t k = strlen(flag);
    if (a[k] != 0) {
        return a + k;
    }
    if (*i + 1 >= nargs) {
        error("argument to '%s' is missing (expected 1 value)", flag);
        option_errors++;
        return NULL;
    }
    *i += 1;
    return args[*i];
}

static void add_input(int kind, const char *arg, int lang) {
    inputs[ninputs].kind = kind;
    inputs[ninputs].arg = arg;
    inputs[ninputs].lang = lang;
    inputs[ninputs].missing = 0;
    ninputs++;
}

static void claim(const char *rendered, int used_by) {
    claims[nclaims].rendered = rendered;
    claims[nclaims].used_by = used_by;
    nclaims++;
}

/* A file input, in the language -x gave (or its name's). */
static void add_file(const char *a, int lang) {
    add_input(IN_FILE, a, lang != LANG_NONE ? lang : lang_of(a));
    input_after_x = 1;
}

/* `-Wl,a,,b` (or -Wp, or -Wa,, `a` its first four bytes) as clang renders
 * it: its values, the empty ones left out, joined by commas (`-Wl,a,b`);
 * each value pushed onto `values` if it is not NULL. */
static const char *render_comma(const char *a, struct strv *values) {
    char *joined = copy_n(a, 3);
    const char *p = a + 4;
    for (;;) {
        const char *comma = strchr(p, ',');
        size_t n = comma != NULL ? (size_t)(comma - p) : strlen(p);
        if (n > 0) {
            char *part = copy_n(p, n);
            char *next = concat(joined, ",", part);
            free(joined);
            joined = next;
            if (values != NULL) {
                push(values, part);
            } else {
                free(part);
            }
        }
        if (comma == NULL) {
            break;
        }
        p = comma + 1;
    }
    return joined;
}

/* -OLEVEL as clang's driver gives it to its -cc1: `-O` is -O1, `-O4` -O3
 * (with clang's warning: only that one, a level cc1 takes as -O3 too),
 * -Ofast is deprecated (its warning too); anything else as it is, for cc1
 * to judge (`-O9` is its warning, `-Ox` its error). */
static const char *opt_for_cc1(const char *a) {
    const char *level = a + 2;
    if (level[0] == 0) {
        return "-O1";
    }
    if (strcmp(level, "fast") == 0) {
        warning_of("deprecated-ofast",
                   "argument '%s' is deprecated; use '-O3' to enable only conforming optimizations, or "
                   "consult the documentation for the same behavior",
                   a);
        return a;
    }
    if (strcmp(level, "4") == 0) {
        warning_of("deprecated", "%s is equivalent to -O3", a);
        return "-O3";
    }
    return a;
}

/* Reads the options (args[1..nargs)). Returns 0, or 1 for --help or
 * --version, which have been answered. */
static int parse(char **args, int nargs) {
    inputs = malloc((size_t)nargs * sizeof(struct input));
    claims = malloc((size_t)nargs * sizeof(struct claim));
    if (inputs == NULL || claims == NULL) {
        oom();
    }
    int lang = LANG_NONE;
    for (int i = 1; i < nargs; i++) {
        char *a = args[i];
        const char *v;
        if (a[0] != '-' || a[1] == 0) {
            /* A file; `-` is standard input. */
            add_file(a, lang);
        } else if (strcmp(a, "--") == 0) {
            /* Every argument after it is a file, as in clang. */
            for (i++; i < nargs; i++) {
                add_file(args[i], lang);
            }
        } else if (strcmp(a, "-###") == 0) {
            print_only = 1;
        } else if (strcmp(a, "-v") == 0) {
            verbose = 1;
        } else if (strcmp(a, "-c") == 0) {
            claim(a, FOR_ASSEMBLE);
            if (phase > ASSEMBLE) {
                phase = ASSEMBLE;
            }
        } else if (strcmp(a, "-S") == 0) {
            claim(a, FOR_COMPILE);
            if (phase > COMPILE) {
                phase = COMPILE;
            }
        } else if (strcmp(a, "-E") == 0) {
            phase = PREPROCESS;
        } else if (strcmp(a, "-g") == 0) {
            claim(a, FOR_TOOLS);
            debug = 1;
        } else if (strcmp(a, "-P") == 0) {
            claim(a, FOR_CC1);
            no_line_markers = 1;
        } else if (strcmp(a, "-w") == 0) {
            no_warnings = 1;
            push(&diag_opts, a);
        } else if (strcmp(a, "-ffreestanding") == 0) {
            claim(a, FOR_CC1);
            freestanding = 1;
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
                option_errors++;
                continue;
            } else if ((v = value(args, nargs, &i, "-fdebug-compilation-dir")) != NULL) {
                comp_dir = v;
            } else {
                continue;
            }
            claim(concat("-fdebug-compilation-dir=", comp_dir, NULL), FOR_CC1);
        } else if (starts_with(a, "-std=")) {
            std = a;
        } else if (a[1] == 'O') {
            claim(a, FOR_CC1);
            opt_level = a;
        } else if (starts_with(a, "-Wl,")) {
            add_input(IN_WL, render_comma(a, NULL), LANG_NONE);
        } else if (starts_with(a, "-Wp,")) {
            /* For cc1, as options of its own (`-Wp,-DX` is -DX). */
            claim(render_comma(a, &wp_args), FOR_CPP);
        } else if (starts_with(a, "-Wa,")) {
            /* For `as`, after clang's own options for it. */
            claim(render_comma(a, &wa_args), FOR_AS_ARGS);
        } else if (a[1] == 'W' && a[2] != 0) {
            claim(a, FOR_CC1);
            push(&warning_opts, a);
            push(&diag_opts, a);
        } else if (a[1] == 'o' || a[1] == 'I' || a[1] == 'D' || a[1] == 'U' || a[1] == 'L' || a[1] == 'l' ||
                   a[1] == 'x') {
            char flag[3] = {'-', a[1], 0};
            v = value(args, nargs, &i, flag);
            if (v == NULL) {
                continue;
            }
            switch (a[1]) {
            case 'o':
                output = v;
                break;
            case 'I':
                claim(concat("-I ", v, NULL), FOR_CPP_TOOLS);
                push(&cpp_opts, "-I");
                push(&cpp_opts, v);
                break;
            case 'D':
            case 'U':
                claim(concat(flag, " ", v), a[1] == 'D' ? FOR_CC1 : FOR_CPP);
                push(&cpp_opts, a[1] == 'D' ? "-D" : "-U");
                push(&cpp_opts, v);
                break;
            case 'L':
                push(&lib_dirs, concat("-L", v, NULL));
                claim(lib_dirs.v[lib_dirs.n - 1], FOR_LINK);
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
                last_x = concat("-x ", v, NULL);
                input_after_x = 0;
                break;
            }
        } else {
            error("unknown argument: '%s'", a);
            option_errors++;
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
/* Whether a cc1 stage and an `as` stage were worked out: what the options
 * of FOR_CC1 and FOR_TOOLS are for; and a cc1 stage that preprocesses,
 * FOR_CPP's. */
static int have_cc1;
static int have_cpp_cc1;
static int have_as;

static struct job *new_job(int tool, int input) {
    struct job *j = &jobs[njobs++];
    memset(j, 0, sizeof *j);
    j->tool = tool;
    j->input = input;
    const char *name = tool == CC1 ? "cc1" : tool == AS ? "as" : "ld";
    push(&j->argv, concat(installed_dir, strcmp(installed_dir, "/") == 0 ? "" : "/", name));
    return j;
}

/* The name cc1 knows a language by (`-x`). */
static const char *cc1_lang(int lang) {
    return lang == LANG_C ? "c" : lang == LANG_CPP_OUTPUT ? "cpp-output" : "assembler-with-cpp";
}

/* cc1 on input `in` (of language `lang`), preprocessing only if
 * `preprocess`, writing `out` (`-` for standard output): the options in
 * clang's order (tests/cc/clang-stages.txt holds it): -O, the mode,
 * -ffreestanding, the debug information, -v, -P, -I, -D and -U in their
 * order, -Wp,'s arguments, /usr/include, the -W options in their order,
 * -w, -std, the output, the language. */
static void cc1_job(int k, const char *in, int lang, int preprocess, const char *out, const char *result) {
    struct job *j = new_job(CC1, k);
    have_cc1 = 1;
    /* C already preprocessed is given no -I, -D, -U, -Wp, or
     * /usr/include, as clang gives its -cc1 none for cpp-output. */
    int cpp = lang != LANG_CPP_OUTPUT;
    have_cpp_cc1 |= cpp;
    if (opt_level != NULL) {
        /* Worked out (and warned of) once, for the first cc1 stage, as
         * clang's driver does it as it makes its -cc1 job. */
        static const char *for_cc1;
        if (for_cc1 == NULL) {
            for_cc1 = opt_for_cc1(opt_level);
        }
        push(&j->argv, for_cc1);
    }
    if (preprocess) {
        push(&j->argv, "-E");
    }
    if (freestanding) {
        push(&j->argv, "-ffreestanding");
    }
    if (debug) {
        push(&j->argv, "-g");
    }
    if (comp_dir != NULL) {
        push(&j->argv, concat("-fdebug-compilation-dir=", comp_dir, NULL));
    }
    /* -v: cc1 says its version and its #include search list too, as
     * clang's -cc1 does. */
    if (verbose) {
        push(&j->argv, "-v");
    }
    if (no_line_markers) {
        push(&j->argv, "-P");
    }
    /* -I, -D and -U in their order, -Wp,'s arguments, then /usr/include,
     * where clang gives its system directories; none for C preprocessed
     * already. */
    if (cpp) {
        for (int i = 0; i < cpp_opts.n; i++) {
            push(&j->argv, cpp_opts.v[i]);
        }
        for (int i = 0; i < wp_args.n; i++) {
            push(&j->argv, wp_args.v[i]);
        }
        push(&j->argv, "-I");
        push(&j->argv, INCLUDE_DIR);
    }
    /* The -W options in their order, then -w, as clang gives them. */
    for (int i = 0; i < warning_opts.n; i++) {
        push(&j->argv, warning_opts.v[i]);
    }
    if (no_warnings) {
        push(&j->argv, "-w");
    }
    if (std != NULL) {
        push(&j->argv, std);
    }
    push(&j->argv, "-o");
    push(&j->argv, out);
    push(&j->argv, "-x");
    push(&j->argv, cc1_lang(lang));
    push(&j->argv, in);
    j->result = result;
}

static void as_job(int k, const char *in, const char *out, const char *result) {
    struct job *j = new_job(AS, k);
    have_as = 1;
    for (int i = 0; i < wa_args.n; i++) {
        push(&j->argv, wa_args.v[i]);
    }
    push(&j->argv, "-o");
    push(&j->argv, out);
    push(&j->argv, in);
    j->result = result;
}

/* The file the user gets for input `in` at the last stage, by its stem and
 * `ext` in the current directory (`hello.o` of `src/hello.c`, `-.o` of
 * standard input, as clang names them), or -o's. */
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
                /* -Wl,a,b: the words between the commas (none for a
                 * bare `-Wl,`, rendered `-Wl`). */
                const char *p = in->arg + 3;
                while (*p == ',') {
                    p++;
                    const char *comma = strchr(p, ',');
                    size_t n = comma != NULL ? (size_t)(comma - p) : strlen(p);
                    push(&objects, copy_n(p, n));
                    p += n;
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
        /* C preprocessed already has nothing to preprocess: with -E, no
         * stage, as clang's driver says. */
        if (in->lang == LANG_CPP_OUTPUT && phase == PREPROCESS) {
            warning("%s: previously preprocessed input", file);
            continue;
        }
        /* Preprocessing only: -E, or -S of assembly that is preprocessed
         * (which clang writes to standard output, as -E would). */
        if (phase == PREPROCESS || (phase == COMPILE && in->lang == LANG_ASM_CPP)) {
            cc1_job(k, file, in->lang, 1, output != NULL ? output : "-", output);
            continue;
        }
        if (phase == COMPILE) {
            const char *out = result_name(file, ".s");
            cc1_job(k, file, in->lang, 0, out, out);
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

/* Runs job `j` and returns its exit status, or NOT_STARTED if it could not
 * be started, having said why as LLVM does. A command line too long for
 * exec goes in a response file, `@FILE` (cc1 and ld read them), written
 * and shown with -v as clang's driver writes and shows its own. */
#define NOT_STARTED (-1000)

static int run(struct job *j) {
    char **argv = j->argv.v;
    char *short_argv[3];
    char *text = NULL;
    if (j->tool != AS && too_long(argv)) {
        text = response_file_text(argv + 1, j->argv.n - 1);
        if (text == NULL) {
            oom();
        }
        char *rsp = make_temp("response", "txt");
        if (rsp == NULL) {
            return NOT_STARTED;
        }
        int fd = open(rsp, O_WRONLY | O_TRUNC);
        int r = fd < 0 ? fd : write_all(fd, text, strlen(text));
        if (fd >= 0) {
            close(fd);
        }
        if (r < 0) {
            error("unable to make temporary file: %s", strerror(-r));
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
            fprintf(STDERR, " Arguments passed via response file:\n%s\n (end of response file)\n", text);
        }
    }
    int pid = spawn(argv[0], argv, environ);
    if (pid < 0) {
        /* LLVM's words (sys::Execute on a Unix). */
        if (pid == -ENOENT) {
            fprintf(STDERR, "cc: error: unable to execute command: Executable \"%s\" doesn't exist!\n", argv[0]);
        } else {
            fprintf(STDERR, "cc: error: unable to execute command: posix_spawn failed: %s\n", strerror(-pid));
        }
        return NOT_STARTED;
    }
    int status = 0;
    if (wait(pid, &status) != pid) {
        return NOT_STARTED;
    }
    return status;
}

/* Whether the user may write the file at `path` that is a regular file:
 * only such a one is removed when the stage that was to make it fails, as
 * clang's Compilation::CleanupFile leaves anything else (a device, `-o
 * /dev/null`, a socket's name) where it is. */
static int removable(const char *path) {
    struct stat st;
    if (stat(path, &st) < 0 || st.type != T_FILE) {
        return 0;
    }
    uid_t uid = geteuid();
    if (uid == 0) {
        return 1;
    }
    if (st.uid == uid) {
        return (st.mode & 0200) != 0;
    }
    if (st.gid == getegid()) {
        return (st.mode & 0020) != 0;
    }
    return (st.mode & 0002) != 0;
}

/* Runs the stages in order, but none whose input's earlier stage failed
 * (the link if any did), as clang does. Removes the file a failed stage
 * was to make, says why each stage failed once they are all done (as
 * clang does after running its jobs: a compiler that exits 1, or could not
 * start, has said why; the assembler and the linker always get cc's word),
 * and returns
 * the first failure's status (1 for one that could not start or was
 * killed), or 0. */
static int run_jobs(void) {
    int status = 0;
    int *failed = calloc((size_t)ninputs + 1, sizeof(int));
    /* Each failed stage's job and status, in order. */
    int *failures = calloc((size_t)njobs + 1, sizeof(int));
    int *statuses = calloc((size_t)njobs + 1, sizeof(int));
    int nfailures = 0;
    int any_failed = 0;
    if (failed == NULL || failures == NULL || statuses == NULL) {
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
        failures[nfailures] = i;
        statuses[nfailures] = r;
        nfailures++;
        if (j->result != NULL && removable(j->result)) {
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
    for (int f = 0; f < nfailures; f++) {
        int tool = jobs[failures[f]].tool;
        int r = statuses[f];
        if (r == NOT_STARTED) {
            /* What clang's driver says of a stage it could not start,
             * which it counts as exit code 1: nothing more for the
             * compiler, which has good diagnostics, as for its exit 1. */
            if (tool != CC1) {
                fprintf(STDERR, "cc: error: %s command failed with exit code 1 (use -v to see invocation)\n",
                        tool_name(tool));
            }
        } else if (r < 0) {
            /* Killed: Camelot's exit status for a process ended from
             * outside (or by a fault), where Linux has signals. */
            fprintf(STDERR, "cc: error: %s command failed due to signal (use -v to see invocation)\n",
                    tool_name(tool));
        } else if (tool != CC1 || r != 1) {
            fprintf(STDERR, "cc: error: %s command failed with exit code %d (use -v to see invocation)\n",
                    tool_name(tool), r);
        }
    }
    free(failed);
    free(failures);
    free(statuses);
    return status;
}

/* Clang's warnings of options nothing ran to use, in their order, if all
 * else is well. */
static void warn_unused(void) {
    /* Only a link: the compiler's options are claimed, said nothing of. */
    int link_only = phase == LINK && !have_cc1 && !have_as;
    int cpp_tools = have_cpp_cc1 || have_as;
    for (int i = 0; i < nclaims; i++) {
        int used_by = claims[i].used_by;
        int tools = have_cc1 || have_as;
        int unused = (used_by == FOR_ASSEMBLE && phase < ASSEMBLE) || (used_by == FOR_COMPILE && phase < COMPILE) ||
                     (used_by == FOR_LINK && phase != LINK) || (used_by == FOR_CC1 && !have_cc1 && !link_only) ||
                     (used_by == FOR_CPP && !have_cpp_cc1 && !link_only) ||
                     (used_by == FOR_TOOLS && !tools && !link_only) ||
                     (used_by == FOR_CPP_TOOLS && !cpp_tools && !link_only) || (used_by == FOR_AS_ARGS && !tools);
        if (unused) {
            warning("argument unused during compilation: '%s'", claims[i].rendered);
        }
    }
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

    /* @FILE, as clang reads response files: a file that can't be read (a
     * directory) or that names itself is clang's message alone. */
    char why[512];
    int nargs = 0;
    char **args = expand_response_files(argc, argv, &nargs, why, sizeof why);
    if (args == NULL) {
        fprintf(STDERR, "%s\n", why);
        return 1;
    }
    if (parse(args, nargs)) {
        return 0;
    }
    if (print_only || verbose) {
        fprintf(STDERR, "cc version %s\nTarget: %s\nThread model: posix\nInstalledDir: %s\n", VERSION, TARGET,
                installed_dir);
    }
    /* A -x after the last input says nothing; then the inputs: a language
     * -x doesn't know, standard input without -E or -x, and files that
     * aren't there. */
    if (last_x != NULL && !input_after_x) {
        warning("'%s' after last input file has no effect", last_x);
    }
    int files = 0;
    for (int k = 0; k < ninputs; k++) {
        struct input *in = &inputs[k];
        if (in->lang == LANG_BAD) {
            error("language not recognized: '%s'", in->arg);
            continue;
        }
        if (in->kind == IN_FILE && strcmp(in->arg, "-") == 0) {
            /* Standard input: C, as clang takes it, but with no -x clang
             * preprocesses it only (-E). */
            if (phase != PREPROCESS && in->lang == LANG_OBJECT) {
                error("-E or -x required when input is from standard input", NULL);
            }
            if (in->lang == LANG_OBJECT) {
                in->lang = LANG_C;
            }
        } else if (in->kind == IN_FILE) {
            struct stat st;
            if (stat(in->arg, &st) < 0) {
                error("no such file or directory: '%s'", in->arg);
                in->missing = 1;
                continue;
            }
        }
        files++;
    }
    if (files == 0) {
        if (!print_only && !verbose) {
            error("no input files", NULL);
        }
        return errors > 0 ? 1 : 0;
    }
    /* The stages, and clang's warnings of inputs that mean nothing for the
     * last one asked for; none with an error in the options themselves, as
     * clang prints none then. */
    if (option_errors == 0) {
        build_jobs();
    }
    /* -o names one file: with -c, -S or -E, only one input may make one
     * (said once the stages are worked out, as clang does). */
    if (output != NULL && phase != LINK) {
        int outputs = 0;
        for (int k = 0; k < ninputs; k++) {
            struct input *in = &inputs[k];
            if (in->kind == IN_FILE && !in->missing && in->lang != LANG_BAD && in->lang != LANG_OBJECT &&
                !(in->lang == LANG_ASM && phase < ASSEMBLE) &&
                !(in->lang == LANG_CPP_OUTPUT && phase == PREPROCESS)) {
                outputs++;
            }
        }
        if (outputs > 1) {
            error("cannot specify -o when generating multiple output files", NULL);
        }
    }
    if (errors == 0) {
        warn_unused();
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
