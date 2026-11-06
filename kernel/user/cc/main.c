/* cc1: `cc1 [-march=ISA] [-mabi=ABI] [-o out.s] [-I dir] [-D name[=value]]
 * [-U name] [-g [-fdebug-compilation-dir=DIR]] [-x LANG] [-E [-P] |
 * --dump-ast] input.c...`, merlin's command line (follows merlin's main.rs
 * and lib.rs): the preprocessor (-E, of C and of assembly: `.S`, `-x
 * assembler-with-cpp`), the syntax tree (--dump-ast) and the compiler, C to
 * assembly, with -g its debugging information (debug.c). */
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
 * status 2, its status for a file it cannot write (-o, as merlin's); there
 * merlin's print! panics instead, exit status 101, which is Rust's and no
 * compiler's. */
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

/* ---- The front end (merlin's lib.rs) ---- */

/* How many errors clang gives before it stops (-ferror-limit=20): the one
 * that would be the 20th is its fatal error instead. */
#define ERROR_LIMIT 20

/* The diagnostics up to clang's error limit, and its fatal error there if
 * they reach it (merlin's error_limit). */
static struct vec error_limit(struct vec ds) {
    struct vec out;
    memset(&out, 0, sizeof out);
    uint errors = 0;
    for (uint i = 0; i < ds.n; i++) {
        struct diag *d = ds.v[i];
        if (!diag_is_warning(d)) {
            errors++;
            if (errors == ERROR_LIMIT) {
                struct diag *f = diag_fatal(pos_at(0, 0, 0), "too many errors emitted, stopping now [-ferror-limit=]");
                f->file = "";
                vec_push(&out, f);
                break;
            }
        }
        vec_push(&out, d);
    }
    return out;
}

/* The diagnostics of the lexer and those of the parser and sema, in the
 * order clang gives them (merlin's in_clangs_order): clang lexes as it
 * parses, so a token's diagnostic comes among those of the code around it,
 * by its position, while the parser's and sema's keep their order. */
static struct vec in_clangs_order(struct vec lexed, struct vec checked) {
    struct vec out;
    memset(&out, 0, sizeof out);
    uint l = 0;
    for (uint i = 0; i < checked.n; i++) {
        struct diag *d = checked.v[i];
        while (l < lexed.n && pos_cmp(((struct diag *)lexed.v[l])->pos, d->pos) <= 0) {
            vec_push(&out, lexed.v[l++]);
        }
        vec_push(&out, d);
    }
    while (l < lexed.n) {
        vec_push(&out, lexed.v[l++]);
    }
    return out;
}

/* The preprocessor's diagnostics and those of what reads its tokens (later,
 * in clang's order among themselves), in the order clang gives them
 * (merlin's Output::merge): each of the preprocessor's comes before what
 * is reported at or after the last token that came out before it. */
static struct vec merge(struct pp_output *pp, struct vec later) {
    struct vec out;
    memset(&out, 0, sizeof out);
    uint k = 0;
    for (uint i = 0; i < pp->ndiags; i++) {
        while (k < later.n && pos_cmp(((struct diag *)later.v[k])->pos, pp->after[i]) < 0) {
            vec_push(&out, later.v[k++]);
        }
        vec_push(&out, pp->diags[i]);
    }
    while (k < later.n) {
        vec_push(&out, later.v[k++]);
    }
    return out;
}

/* Names the file of every diagnostic not in the file being compiled. */
static void name_files(struct pp_output *pp, struct vec *ds) {
    for (uint i = 0; i < ds->n; i++) {
        diag_name_files(ds->v[i], pp->files, pp->lens, pp->nfiles);
    }
}

/* The front end after the preprocessor: tokens, syntax tree, checks
 * (merlin's check). 1 and the program, its warnings (the preprocessor's
 * among them) in prog->warnings; or 0 and every diagnostic in *errors. */
static int check(struct pp_output *pp, struct program *prog, struct vec *errors) {
    /* The lexer's diagnostics: clang goes on after an error in a literal,
     * or a token that is none, and so does merlin. */
    struct token *toks;
    uint ntoks;
    struct vec lexed;
    memset(&lexed, 0, sizeof lexed);
    if (!lex_finish(pp->toks, pp->ntoks, pp->end, &toks, &ntoks, &lexed)) {
        *errors = merge(pp, lexed);
        return 0;
    }
    /* Where the errors before sema's are, which leave out clang's analysis
     * of the functions done after them. */
    struct pos *earlier = heap_alloc((pp->ndiags + lexed.n + 1) * sizeof(struct pos));
    uint nearlier = 0;
    uint capearlier = pp->ndiags + lexed.n + 1;
    for (uint i = 0; i < pp->ndiags; i++) {
        if (!diag_is_warning(pp->diags[i])) {
            earlier[nearlier++] = pp->after[i];
        }
    }
    for (uint i = 0; i < lexed.n; i++) {
        if (!diag_is_warning(lexed.v[i])) {
            earlier[nearlier++] = ((struct diag *)lexed.v[i])->pos;
        }
    }
    struct diag *syntax;
    int parsed_ok = parse_program(toks, ntoks, prog, &syntax);
    prog->written.macros = pp->macros;
    /* The parser's warnings (it stops at its first error) come as it reads,
     * as the lexer's do; sema's after them. The parser's errors it goes on
     * after count too. */
    struct vec parsed = vec_take(&prog->warnings);
    for (uint i = 0; i < parsed.n; i++) {
        if (!diag_is_warning(parsed.v[i])) {
            earlier = grow(earlier, &capearlier, nearlier + 1, sizeof(struct pos));
            earlier[nearlier++] = ((struct diag *)parsed.v[i])->pos;
        }
    }
    struct vec checked;
    if (parsed_ok) {
        struct vec all;
        if (sema_check(prog, earlier, nearlier, &all)) {
            checked = all;
        } else {
            checked = vec_take(&prog->warnings);
        }
    } else {
        /* What came before a syntax error is checked as clang checks it as
         * it parses, errors too; the end of the function it is in, and of
         * the file, come after it. */
        struct vec after;
        sema_check_cut(prog, earlier, nearlier, syntax->pos, &checked, &after);
        vec_push(&checked, syntax);
        vec_extend(&checked, &after);
    }
    heap_free(earlier);
    struct vec read = in_clangs_order(lexed, parsed);
    struct vec diagnostics = error_limit(merge(pp, in_clangs_order(read, checked)));
    int all_warnings = 1;
    for (uint i = 0; i < diagnostics.n; i++) {
        if (!diag_is_warning(diagnostics.v[i])) {
            all_warnings = 0;
        }
    }
    if (parsed_ok && all_warnings) {
        prog->warnings = diagnostics;
        return 1;
    }
    *errors = diagnostics;
    return 0;
}

/* Prints a list of diagnostics, as clang does. */
static void print_all(struct vec *ds, const char *input) {
    for (uint i = 0; i < ds->n; i++) {
        diag_print(ds->v[i], input);
    }
}

/* Compiles (or dumps) one translation unit (merlin's compile_warned and
 * parse_with): 1 and the text in *text with its warnings printed, or 0
 * with every diagnostic printed. comp_dir: with -g, the compilation
 * directory; else NULL. */
static int compile_unit(const char *name, const char *src, uint n, struct options *opts, int dump,
                        const char *comp_dir, struct buf *text) {
    struct pp_output pp;
    if (!preprocess_unit(name, src, n, opts, &pp)) {
        for (uint i = 0; i < pp.ndiags; i++) {
            diag_print(pp.diags[i], name);
        }
        return 0;
    }
    struct program prog;
    struct vec errors;
    if (!check(&pp, &prog, &errors)) {
        name_files(&pp, &errors);
        print_all(&errors, name);
        return 0;
    }
    name_files(&pp, &prog.warnings);
    if (dump) {
        ast_dump(&prog, text);
        print_all(&prog.warnings, name);
        return 1;
    }
    struct diag *error = NULL;
    struct buf asm_text;
    memset(&asm_text, 0, sizeof asm_text);
    /* -g: the files the preprocessor read, what the .file directives say
     * of them. */
    struct lines *debug = NULL;
    if (comp_dir != NULL) {
        debug = debug_new(comp_dir, pp.files, pp.lens, pp.sums, pp.nfiles);
    }
    if (!codegen_generate(&prog, debug, &asm_text, &error)) {
        diag_name_files(error, pp.files, pp.lens, pp.nfiles);
        print_all(&prog.warnings, name);
        diag_print(error, name);
        buf_free(&asm_text);
        return 0;
    }
    /* `.file "NAME"`, the first line of the assembly, as clang writes it with
     * or without -g: the base name of the file being compiled (merlin's
     * file_directive), whatever #line says. The assembler makes it the
     * object's STT_FILE symbol; it has nothing to do with the numbered
     * .files of a line table. */
    uint base_len;
    const char *base = path_file_name(name, (uint)strlen(name), &base_len);
    buf_str(text, "    .file ");
    quote(text, base, base_len);
    buf_byte(text, '\n');
    buf_put(text, asm_text.data, asm_text.len);
    buf_free(&asm_text);
    print_all(&prog.warnings, name);
    return 1;
}

/* The input file's text, NUL-terminated (its length in *len), or NULL
 * with what stops it said: a file it cannot read, or one that is not UTF-8
 * (Rust's read_to_string reads only that). */
static char *read_input(struct input *in, uint *len) {
    int err = 0;
    char *src = read_file(in->name, len, &err);
    if (src == NULL) {
        fprintf(2, "cc1: cannot read %s: %s\n", in->name, strerror(-err));
        return NULL;
    }
    if (!valid_utf8((uchar *)src, *len)) {
        fprintf(2, "cc1: cannot read %s: stream did not contain valid UTF-8\n", in->name);
        heap_free(src);
        return NULL;
    }
    return src;
}

/* Writes a translation unit's output: to `output`, or standard output if
 * NULL. 0, or 2 for a file it cannot write. */
static int write_output(const char *output, struct buf *text) {
    if (output == NULL) {
        write_out(text->data, text->len);
        return 0;
    }
    int r = write_file(output, text->data, text->len);
    if (r < 0) {
        fprintf(2, "cc1: cannot write %s: %s\n", output, strerror(-r));
        return 2;
    }
    return 0;
}

/* Compiles or dumps one input file (merlin's translate): 0, or the exit
 * status of a failure. */
static int compile_file(struct input *in, const char *output, struct options *opts, int dump, const char *comp_dir) {
    uint len = 0;
    char *src = read_input(in, &len);
    if (src == NULL) {
        return 2;
    }
    opts->lang = in->lang;
    struct buf text;
    memset(&text, 0, sizeof text);
    int status = 1;
    if (compile_unit(in->name, src, len, opts, dump, comp_dir, &text)) {
        /* Without -o: the input with .s for .c (or added), or for
         * --dump-ast standard output. */
        const char *out = output;
        if (out == NULL && !dump) {
            out = ends_with(in->name, ".c") ? fmt("%s.s", arena_strndup(in->name, (uint)strlen(in->name) - 2))
                                             : fmt("%s.s", in->name);
        }
        status = write_output(out, &text);
    }
    /* The translation unit is done: its memory goes, the arena and the
     * heap (its source, tokens, tree, assembly and diagnostics). */
    arena_free();
    return status;
}

/* Preprocesses one input file (merlin's translate, for -E): 0, or the exit
 * status of a failure. */
static int translate(struct input *in, const char *output, struct options *opts) {
    uint len = 0;
    char *src = read_input(in, &len);
    if (src == NULL) {
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
    int status = ok ? write_output(output, &text) : 1;
    /* The translation unit is done: its memory goes, the arena and the
     * heap (its source, the text and the diagnostics). */
    arena_free();
    return status;
}

/* The current directory's path, found as Unix found it before getcwd was a
 * system call (Camelot has none): from `.` up, each directory's name is the
 * entry of its parent (`..`) with its inode, until the root, which is its
 * own parent. NULL if a step fails (a directory removed while in use, say),
 * which merlin's std::env::current_dir reports as an error too. */
static char *current_dir(void) {
    /* The names from the current directory up, and the path of the
     * directory being looked at: ".", "..", "../..", ... */
    struct vec names;
    memset(&names, 0, sizeof names);
    char *here = ".";
    char *result = NULL;
    for (;;) {
        struct stat st;
        struct stat up;
        char *parent = strcmp(here, ".") == 0 ? ".." : fmt("%s/..", here);
        if (stat(here, &st) < 0 || stat(parent, &up) < 0) {
            break;
        }
        if (st.ino == up.ino) {
            /* The root: the names, outermost first, each after a `/`. */
            struct buf path;
            memset(&path, 0, sizeof path);
            for (uint i = names.n; i > 0; i--) {
                buf_byte(&path, '/');
                buf_str(&path, names.v[i - 1]);
            }
            if (path.len == 0) {
                buf_byte(&path, '/');
            }
            result = buf_arena(&path);
            buf_free(&path);
            break;
        }
        int fd = open(parent, O_RDONLY);
        if (fd < 0) {
            break;
        }
        struct dirent de;
        char *name = NULL;
        while (read(fd, &de, sizeof de) == sizeof de) {
            if (de.inum != st.ino) {
                continue;
            }
            /* A name of DIRSIZ bytes has no NUL. */
            uint n = 0;
            while (n < DIRSIZ && de.name[n] != 0) {
                n++;
            }
            if ((n == 1 && de.name[0] == '.') || (n == 2 && de.name[0] == '.' && de.name[1] == '.')) {
                continue;
            }
            name = arena_strndup(de.name, n);
            break;
        }
        close(fd);
        if (name == NULL) {
            break;
        }
        vec_push(&names, name);
        here = parent;
    }
    heap_free(names.v);
    return result;
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
    uint ninputs = 0;
    const char *output = NULL;
    int mode = MODE_COMPILE;
    struct options opts;
    memset(&opts, 0, sizeof opts);
    opts.line_markers = 1;
    /* The command line's lists, at most an entry per argument, live for the
     * whole run: plain malloc, not a translation unit's heap, which goes when
     * the unit is done. */
    struct input *inputs = malloc((uint)argc * sizeof(struct input));
    opts.include_dirs = malloc((uint)argc * sizeof(char *));
    opts.macros = malloc((uint)argc * sizeof(struct cmd_macro));
    if (inputs == NULL || opts.include_dirs == NULL || opts.macros == NULL) {
        die("out of memory");
    }
    const char *abi = NULL;
    int debug = 0;
    const char *comp_dir = NULL;
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
            debug = 1;
        } else if (strncmp(a, "-fdebug-compilation-dir", 23) == 0) {
            char *v = a + 23;
            if (*v == 0) {
                if (i + 1 >= argc) {
                    return usage_error("-fdebug-compilation-dir needs a directory");
                }
                i++;
                comp_dir = argv[i];
            } else if (*v != '=') {
                return usage_error(fmt("unknown option %s", a));
            } else {
                comp_dir = v + 1;
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
            opts.include_dirs[opts.ninclude_dirs++] = d;
        } else if (strncmp(a, "-D", 2) == 0 || strncmp(a, "-U", 2) == 0) {
            int undef = a[1] == 'U';
            char *m = value(argv, argc, &i, 2);
            if (m == NULL || m[0] == 0 || (!undef && m[0] == '=')) {
                return usage_error(undef ? "-U needs a macro name" : "-D needs a macro name");
            }
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
    /* -g: the compilation directory, by default the directory cc1 runs in
     * (or `.` if it cannot be found). Each file read has its MD5 taken, for
     * the line table. (It changes nothing -E or --dump-ast writes.) */
    if (debug) {
        if (comp_dir == NULL) {
            char *cwd = current_dir();
            comp_dir = ".";
            if (cwd != NULL) {
                /* For the whole run: not in the arena, which each
                 * translation unit frees. */
                uint n = (uint)strlen(cwd);
                char *kept = malloc(n + 1);
                if (kept == NULL) {
                    die("out of memory");
                }
                memcpy(kept, cwd, n + 1);
                comp_dir = kept;
            }
        }
        opts.checksums = 1;
    } else {
        comp_dir = NULL;
    }
    int status = 0;
    for (uint i = 0; i < ninputs; i++) {
        int r = mode == MODE_PREPROCESS ? translate(&inputs[i], output, &opts)
                                        : compile_file(&inputs[i], output, &opts, mode == MODE_DUMP_AST, comp_dir);
        if (r != 0) {
            status = r;
        }
    }
    return status;
}
