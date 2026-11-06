/* cc1: `cc1 [-march=ISA] [-mabi=ABI] [-o out.s] [-I dir] [-D name[=value]]
 * [-U name] [-g] [-x LANG] [-E [-P] | --dump-ast] input.c...`, merlin's
 * command line (follows merlin's main.rs and lib.rs). This stage has the
 * preprocessor: -E, of C and of assembly (`.S`, `-x assembler-with-cpp`).
 * Compiling (and --dump-ast) is refused for now, with a message saying so. */
#include "cc.h"

static const char USAGE[] = "usage: cc1 [options] input.c...\n"
                            "\n"
                            "  -march=ISA        rv64ima_zicsr_zifencei, the only one (and the default)\n"
                            "  -mabi=ABI         lp64, the only one (merlin has no floating point)\n"
                            "  -o FILE           output file (default: each input with a .s extension;\n"
                            "                    with -E, standard output); needs a single input\n"
                            "  -I DIR            look for #include files in DIR (after the including\n"
                            "                    file's own directory for #include \"...\"); repeatable\n"
                            "  -D NAME[=VALUE]   define a macro, as #define NAME VALUE (VALUE is 1 if\n"
                            "                    left out); -D 'F(x)=VALUE' defines a function-like one\n"
                            "  -U NAME           undefine a macro (-D and -U apply in order)\n"
                            "  -g                debug information: which source line each instruction\n"
                            "                    comes from (.file, .loc), and a DWARF compile unit with\n"
                            "                    the functions, their parameters and locals, the globals\n"
                            "                    and the types, for debuggers (merlin-dbg, lldb)\n"
                            "  -fdebug-compilation-dir=DIR\n"
                            "                    the directory -g says file names are relative to\n"
                            "                    (default: the current directory)\n"
                            "  -E                preprocess only: print the tokens after preprocessing\n"
                            "  -P                with -E, no line markers in preprocessed assembly (for a\n"
                            "                    linker script: kernel/kernel.lds.S); C's -E has none\n"
                            "  -x LANG           what the inputs after it are: c, or assembler-with-cpp\n"
                            "                    (assembly for merlin-as, only preprocessed: -E); without\n"
                            "                    it, .S files are assembler-with-cpp and the rest C\n"
                            "  --dump-ast        print the checked syntax tree to stdout instead of compiling\n"
                            "\n"
                            "Each input is a translation unit, compiled to its own assembly file. A .S\n"
                            "file is preprocessed (-E) as clang -E -x assembler-with-cpp does, with\n"
                            "__ASSEMBLER__ and the target's __riscv_xlen defined, for merlin-as.";

/* The ISA merlin compiles for, as clang spells it for -march= (MARCH),
 * and the ABI, as -mabi= names it (ABI). */
static const char MARCH[] = "rv64ima_zicsr_zifencei";
static const char ABI[] = "lp64";

enum { MODE_COMPILE, MODE_PREPROCESS, MODE_DUMP_AST };

/* An input file and its language. */
struct input {
    const char *name;
    int lang;
};

static int usage_error(const char *msg) {
    fprintf(2, "cc1: %s\n%s\n", msg, USAGE);
    return 2;
}

static int ends_with(const char *s, const char *suffix) {
    uint n = (uint)strlen(s);
    uint k = (uint)strlen(suffix);
    return n >= k && strcmp(s + n - k, suffix) == 0;
}

/* The language clang gives a file by its name: .S (and .sx) are
 * assembler-with-cpp, anything else C (merlin's Lang::of_file). */
static int lang_of_file(const char *name) {
    return ends_with(name, ".S") || ends_with(name, ".sx") ? LANG_ASM : LANG_C;
}

/* The whole file, NUL-terminated, or NULL with *err set: a directory is
 * none (-EISDIR), as Rust's read_to_string refuses one. */
static char *read_file(const char *path, uint *len, int *err) {
    int fd = open(path, O_RDONLY);
    if (fd < 0) {
        *err = fd;
        return NULL;
    }
    struct stat st;
    int r = fstat(fd, &st);
    if (r < 0 || st.type == T_DIR) {
        close(fd);
        *err = r < 0 ? r : -EISDIR;
        return NULL;
    }
    struct buf b;
    memset(&b, 0, sizeof b);
    char chunk[4096];
    for (;;) {
        int n = read(fd, chunk, sizeof chunk);
        if (n < 0) {
            close(fd);
            buf_free(&b);
            *err = n;
            return NULL;
        }
        if (n == 0) {
            break;
        }
        buf_put(&b, chunk, (uint)n);
    }
    close(fd);
    buf_put(&b, "", 0);
    *len = b.len;
    return b.data;
}

static int write_file(const char *path, const char *data, uint len) {
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

/* Writes all of data to standard output. A failure stops cc1 with exit
 * status 2, as one stops merlin (whose print! gives up the program). */
static void write_out(const char *data, uint len) {
    uint done = 0;
    while (done < len) {
        int r = write(1, data + done, len - done);
        if (r <= 0) {
            fprintf(2, "cc1: cannot write standard output: %s\n", strerror(r < 0 ? -r : ENOSPC));
            exit(2);
        }
        done += (uint)r;
    }
}

/* Preprocesses one input file (merlin's translate, for -E): 0, or the exit
 * status of a failure. */
static int translate(struct input *in, const char *output, struct options *opts) {
    uint len = 0;
    int err = 0;
    char *src = read_file(in->name, &len, &err);
    if (src == NULL) {
        fprintf(2, "cc1: cannot read %s: %s\n", in->name, strerror(-err));
        return 2;
    }
    if (!valid_utf8((uchar *)src, len)) {
        fprintf(2, "cc1: cannot read %s: stream did not contain valid UTF-8\n", in->name);
        return 2;
    }
    opts->lang = in->lang;
    struct buf text;
    memset(&text, 0, sizeof text);
    struct diag **diags = NULL;
    uint ndiags = 0;
    int ok = preprocess_text(in->name, src, len, opts, &text, &diags, &ndiags);
    /* Diagnostics as clang prints them, on stderr: the warnings of a
     * preprocessing that succeeds, or every diagnostic of one that fails;
     * only errors fail. */
    for (uint i = 0; i < ndiags; i++) {
        diag_print(diags[i], in->name);
    }
    /* The translation unit is done: its memory goes (the text, malloc'd,
     * stays for writing it). */
    free(diags);
    arena_free();
    free(src);
    if (!ok) {
        buf_free(&text);
        return 1;
    }
    if (output == NULL) {
        write_out(text.data, text.len);
        buf_free(&text);
        return 0;
    }
    int r = write_file(output, text.data, text.len);
    buf_free(&text);
    if (r < 0) {
        fprintf(2, "cc1: cannot write %s: %s\n", output, strerror(-r));
        return 2;
    }
    return 0;
}

/* The value of an option written `-I dir` or `-Idir` (the same for -D, -U,
 * -o and -x): what follows the flag in the argument, or the next argument;
 * NULL if there is none. */
static char *value(char **argv, int argc, int *i, uint flag_len) {
    char *a = argv[*i];
    if (a[flag_len] != 0) {
        return a + flag_len;
    }
    if (*i + 1 >= argc) {
        return NULL;
    }
    *i += 1;
    return argv[*i];
}

int main(int argc, char **argv) {
    struct input *inputs = NULL;
    uint ninputs = 0;
    uint capinputs = 0;
    const char *output = NULL;
    int mode = MODE_COMPILE;
    struct options opts;
    memset(&opts, 0, sizeof opts);
    opts.line_markers = 1;
    uint capdirs = 0;
    uint capmacros = 0;
    const char *abi = NULL;
    /* -x LANG applies to the inputs after it, as in clang; -1 goes by each
     * file's name. */
    int lang = -1;
    for (int i = 1; i < argc; i++) {
        char *a = argv[i];
        if (strcmp(a, "-E") == 0) {
            mode = MODE_PREPROCESS;
        } else if (strcmp(a, "-P") == 0) {
            opts.line_markers = 0;
        } else if (strcmp(a, "-g") == 0) {
            /* Debug information changes nothing that -E writes. */
        } else if (strncmp(a, "-fdebug-compilation-dir", 23) == 0) {
            char *v = a + 23;
            if (*v == 0) {
                if (i + 1 >= argc) {
                    return usage_error("-fdebug-compilation-dir needs a directory");
                }
                i++;
            } else if (*v != '=') {
                return usage_error(fmt("unknown option %s", a));
            }
        } else if (strcmp(a, "--dump-ast") == 0) {
            mode = MODE_DUMP_AST;
        } else if (strncmp(a, "-march=", 7) == 0) {
            /* -march=ISA and -mabi=ABI only, as clang takes them. */
            if (strcmp(a + 7, MARCH) != 0) {
                return usage_error(fmt("unsupported -march '%s' (merlin takes %s)", a + 7, MARCH));
            }
        } else if (strncmp(a, "-mabi=", 6) == 0) {
            abi = a + 6;
        } else if (strcmp(a, "-h") == 0 || strcmp(a, "--help") == 0) {
            printf("%s\n", USAGE);
            return 0;
        } else if (strncmp(a, "-o", 2) == 0) {
            char *o = value(argv, argc, &i, 2);
            if (o == NULL) {
                return usage_error("-o needs a file name");
            }
            output = o;
        } else if (strncmp(a, "-I", 2) == 0) {
            char *d = value(argv, argc, &i, 2);
            if (d == NULL) {
                return usage_error("-I needs a directory");
            }
            opts.include_dirs = grow(opts.include_dirs, &capdirs, opts.ninclude_dirs + 1, sizeof(char *));
            opts.include_dirs[opts.ninclude_dirs++] = d;
        } else if (strncmp(a, "-D", 2) == 0 || strncmp(a, "-U", 2) == 0) {
            int undef = a[1] == 'U';
            char *m = value(argv, argc, &i, 2);
            if (m == NULL || m[0] == 0 || (!undef && m[0] == '=')) {
                return usage_error(undef ? "-U needs a macro name" : "-D needs a macro name");
            }
            opts.macros = grow(opts.macros, &capmacros, opts.nmacros + 1, sizeof(struct cmd_macro));
            opts.macros[opts.nmacros].undef = undef;
            opts.macros[opts.nmacros].text = m;
            opts.nmacros++;
        } else if (strncmp(a, "-x", 2) == 0) {
            char *l = value(argv, argc, &i, 2);
            if (l == NULL) {
                return usage_error("-x needs a language");
            }
            if (strcmp(l, "none") == 0) {
                lang = -1;
            } else if (strcmp(l, "c") == 0) {
                lang = LANG_C;
            } else if (strcmp(l, "assembler-with-cpp") == 0) {
                lang = LANG_ASM;
            } else {
                return usage_error(fmt("language not recognized: '%s'", l));
            }
        } else if (a[0] == '-') {
            return usage_error(fmt("unknown option %s", a));
        } else {
            inputs = grow(inputs, &capinputs, ninputs + 1, sizeof(struct input));
            inputs[ninputs].name = a;
            inputs[ninputs].lang = lang >= 0 ? lang : lang_of_file(a);
            ninputs++;
        }
    }
    /* The ABI must be the ISA's; clang calls any other an unknown one. */
    if (abi != NULL && strcmp(abi, ABI) != 0) {
        return usage_error(fmt("unknown target ABI '%s'", abi));
    }
    if (ninputs == 0) {
        fprintf(2, "%s\n", USAGE);
        return 2;
    }
    if (output != NULL && ninputs > 1) {
        return usage_error("-o needs a single input file");
    }
    /* Assembly is only preprocessed: merlin-as assembles it. */
    for (uint i = 0; i < ninputs; i++) {
        if (inputs[i].lang == LANG_ASM && mode != MODE_PREPROCESS) {
            return usage_error(fmt("%s: assembler-with-cpp input is only preprocessed (-E); merlin-as "
                                   "assembles the output",
                                   inputs[i].name));
        }
    }
    /* This stage of cc1 is merlin's preprocessor (#241). */
    if (mode != MODE_PREPROCESS) {
        fprintf(2, "cc1: only preprocessing (-E) is here yet: compiling comes with merlin's parser\n");
        return 2;
    }
    int status = 0;
    for (uint i = 0; i < ninputs; i++) {
        int r = translate(&inputs[i], output, &opts);
        if (r != 0) {
            status = r;
        }
    }
    return status;
}
