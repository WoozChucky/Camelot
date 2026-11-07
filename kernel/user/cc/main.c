/* cc1: `cc1 [-march=ISA] [-mabi=ABI] [-o out.s] [-I dir] [-D name[=value]]
 * [-U name] [-g [-fdebug-compilation-dir=DIR]] [-x LANG] [-O LEVEL]
 * [-std=c99] [-v] [-w] [-W...] [-ffreestanding] [-E [-P] | --dump-ast] [@file]
 * input.c...`, merlin's command line (follows merlin's main.rs
 * and lib.rs): the preprocessor (-E, of C and of assembly: `.S`, `-x
 * assembler-with-cpp`), the syntax tree (--dump-ast) and the compiler, C to
 * assembly, with -g its debugging information (debug.c). */
#include "cc.h"

static const char USAGE[] = "usage: cc1 [options] input.c...\n"
                            "\n"
                            "  -march=ISA        rv64ima_zicsr_zifencei, the only one (and the default)\n"
                            "  -mabi=ABI         lp64, the only one (merlin has no floating point)\n"
                            "  -o FILE           output file (default: each input with a .s extension;\n"
                            "                    with -E, standard output; `-` is standard output); needs\n"
                            "                    a single input\n"
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
                            "  -x LANG           what the inputs after it are: c (or cpp-output, C\n"
                            "                    already preprocessed: a .i file), or assembler-with-cpp\n"
                            "                    (assembly for merlin-as, only preprocessed: -E); without\n"
                            "                    it, .S files are assembler-with-cpp, .i files C, and the\n"
                            "                    rest C\n"
                            "  -O LEVEL          -O0, -O1, -O2, -O3, -Os, -Oz, -Og or -Ofast, as clang\n"
                            "                    takes them; merlin does not optimize, so every level\n"
                            "                    compiles the same code\n"
                            "  -std=STD          the language: c99 (or gnu99, iso9899:1999), the only one\n"
                            "  -v                say the version and the #include search list, as clang\n"
                            "                    -cc1 -v does\n"
                            "  -w                no warnings\n"
                            "  -Wno-FLAG         no warnings of -WFLAG; -WFLAG (and -Wall...) changes nothing,\n"
                            "                    every warning merlin gives being clang's default\n"
                            "  -Werror[=FLAG]    warnings (of FLAG) are errors; -Wno-error[=FLAG] undoes it\n"
                            "  -ffreestanding    changes nothing: merlin compiles for a freestanding\n"
                            "                    implementation (__STDC_HOSTED__ is 0)\n"
                            "  --dump-ast        print the checked syntax tree to stdout instead of compiling\n"
                            "  @FILE             more arguments, read from FILE (quoted as clang reads them:\n"
                            "                    excalibur::response)\n"
                            "\n"
                            "Each input is a translation unit, compiled to its own assembly file. A .S\n"
                            "file is preprocessed (-E) as clang -E -x assembler-with-cpp does, with\n"
                            "__ASSEMBLER__ and the target's __riscv_xlen defined, for merlin-as. An input\n"
                            "`-` is standard input, named <stdin>.";

/* The ISA merlin compiles for, as clang spells it for -march= (MARCH),
 * and the ABI, as -mabi= names it (ABI). */
static const char MARCH[] = "rv64ima_zicsr_zifencei";
static const char ABI[] = "lp64";

enum { MODE_COMPILE, MODE_PREPROCESS, MODE_DUMP_AST };

/* An input file and its language; standard input (`-`) is named
 * <stdin>, as clang names it. */
struct input {
    const char *name;
    int lang;
    int from_stdin;
    /* C already preprocessed: -x cpp-output, a .i file. */
    int preprocessed;
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

/* All that descriptor `fd` gives until its end, NUL-terminated, or NULL
 * with *err set: a file's text, or standard input's (`-`). */
static char *read_fd(int fd, uint *len, int *err) {
    struct buf b;
    memset(&b, 0, sizeof b);
    char chunk[4096];
    for (;;) {
        int n = read(fd, chunk, sizeof chunk);
        if (n < 0) {
            buf_free(&b);
            *err = n;
            return NULL;
        }
        if (n == 0) {
            break;
        }
        buf_put(&b, chunk, (uint)n);
    }
    buf_put(&b, "", 0);
    *len = b.len;
    return b.data;
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
    char *text = read_fd(fd, len, err);
    close(fd);
    return text;
}

/* cc1's version, merlin's (the workspace's: a test checks they agree). */
#define VERSION "0.1.0"

/* Whether -v was given. */
static int verbose;

/* What -v says of the #include search list before a translation unit, as
 * clang's -cc1 -v says it (merlin's search_list): the -I directories that
 * are not there and those given twice (the same inode) left out, and said
 * so, then the list. cc1 searches a quoted name in the including file's
 * directory first, which clang's list does not show either, then the -I
 * directories in order, as it searches an angled one: they are both
 * lists'. What it leaves out changes no search. */
static void print_search_list(struct options *opts) {
    struct buf missing;
    struct buf twice;
    struct buf list;
    memset(&missing, 0, sizeof missing);
    memset(&twice, 0, sizeof twice);
    memset(&list, 0, sizeof list);
    uint *inodes = malloc((opts->ninclude_dirs + 1) * sizeof(uint));
    if (inodes == NULL) {
        die("out of memory");
    }
    uint nkept = 0;
    for (uint i = 0; i < opts->ninclude_dirs; i++) {
        const char *d = opts->include_dirs[i];
        struct stat st;
        if (stat(d, &st) < 0 || st.type != T_DIR) {
            buf_fmt(&missing, "ignoring nonexistent directory \"%s\"\n", d);
            continue;
        }
        int seen = 0;
        for (uint k = 0; k < nkept; k++) {
            seen |= inodes[k] == st.ino;
        }
        if (seen) {
            buf_fmt(&twice, "ignoring duplicate directory \"%s\"\n", d);
            continue;
        }
        inodes[nkept++] = st.ino;
        buf_fmt(&list, " %s\n", d);
    }
    /* All of it in one write: the missing, the twice given, the list. */
    buf_put(&missing, twice.data, twice.len);
    buf_str(&missing, "#include \"...\" search starts here:\n#include <...> search starts here:\n");
    buf_put(&missing, list.data, list.len);
    buf_str(&missing, "End of search list.\n");
    write(2, missing.data, missing.len);
    buf_free(&missing);
    buf_free(&twice);
    buf_free(&list);
    free(inodes);
}

/* What -O and -std= said (main), clang's diagnostics of no file, and
 * whether one is an error. */
static struct diag *said[8];
static uint nsaid;
static int said_error;

static void say(struct diag *d) {
    d->file = "";
    d->filelen = 0;
    if (nsaid < 8) {
        said[nsaid++] = d;
    }
    said_error |= d->severity == SEV_ERROR;
}

/* -OLEVEL, as clang's -cc1 takes it (merlin's check_opt): s, z, g, fast, a
 * level of 0 to 3 (00, 03 too) or 4 itself (clang's -O4 option, its -O3)
 * say nothing; another level, a 32-bit decimal number (a '-' but no '+'
 * before it; 04 too), is clang's warning that it is not supported;
 * anything else its error. merlin does not optimize: no level changes the
 * code. */
static void check_opt(const char *level) {
    static const char *const NAMES[] = {"", "s", "z", "g", "fast", "4"};
    for (uint i = 0; i < sizeof NAMES / sizeof NAMES[0]; i++) {
        if (strcmp(level, NAMES[i]) == 0) {
            return;
        }
    }
    struct pos none;
    memset(&none, 0, sizeof none);
    const char *p = level[0] == '-' ? level + 1 : level;
    /* The number, as Rust's i32 parse takes it: in range, or none. */
    int number = *p != 0;
    long long value = 0;
    for (; *p != 0 && number; p++) {
        number = *p >= '0' && *p <= '9';
        value = value * 10 + (*p - '0');
        if (value > 2147483648LL) {
            number = 0;
        }
    }
    if (number && level[0] == '-') {
        value = -value;
    }
    if (!number || value > 2147483647LL) {
        say(diag_error(none, fmt("invalid integral value '%s' in '-O%s'", level, level)));
    } else if (value < 0 || value > 3) {
        say(diag_warning_unflagged(none, fmt("optimization level '-O%s' is not supported; using '-O3' instead", level)));
    }
}

/* -std=VALUE, as clang's -cc1 takes it for C (merlin's check_std): C99's
 * names say nothing; merlin refuses the other C standards; another
 * language's (C++'s, OpenCL's, HLSL's) is clang's `not allowed with 'C'`,
 * and what is none clang's `invalid value`, with its notes naming the C
 * standards. */
static void check_std(const char *value) {
    static const char *const C99[] = {"c99", "c9x", "iso9899:1999", "iso9899:199x", "gnu99", "gnu9x"};
    static const char *const OTHER_C[] = {
        "c89", "c90", "iso9899:1990", "iso9899:199409", "gnu89", "gnu90", "c11", "c1x", "iso9899:2011",
        "iso9899:201x", "gnu11", "gnu1x", "c17", "iso9899:2017", "c18", "iso9899:2018", "gnu17", "gnu18", "c23",
        "c2x", "iso9899:2024", "gnu23", "gnu2x", "c2y", "gnu2y"};
    static const char *const OTHER_LANGUAGES[] = {
        "c++98", "c++03", "gnu++98", "gnu++03", "c++0x", "c++11", "gnu++0x", "gnu++11", "c++1y", "c++14", "gnu++1y", "gnu++14", "c++1z", "c++17", "gnu++1z", "gnu++17", "c++2a", "c++20", "gnu++2a", "gnu++20", "c++2b", "c++23", "gnu++2b", "gnu++23", "c++2c", "c++26", "gnu++2c", "gnu++26",
        "cl", "CL", "cl1.0", "cl1.1", "CL1.1", "cl1.2", "CL1.2", "cl2.0", "CL2.0", "cl3.0", "CL3.0",
        "clc++", "CLC++", "clc++1.0", "CLC++1.0", "clc++2021", "CLC++2021", "hlsl", "hlsl2015", "hlsl2016",
        "hlsl2017", "hlsl2018", "hlsl2021", "hlsl202x", "hlsl202y"};
    static const char *const NOTES[] = {
        "use 'c89', 'c90', or 'iso9899:1990' for 'ISO C 1990' standard",
        "use 'iso9899:199409' for 'ISO C 1990 with amendment 1' standard",
        "use 'gnu89' or 'gnu90' for 'ISO C 1990 with GNU extensions' standard",
        "use 'c99' or 'iso9899:1999' for 'ISO C 1999' standard",
        "use 'gnu99' for 'ISO C 1999 with GNU extensions' standard",
        "use 'c11' or 'iso9899:2011' for 'ISO C 2011' standard",
        "use 'gnu11' for 'ISO C 2011 with GNU extensions' standard",
        "use 'c17', 'iso9899:2017', 'c18', or 'iso9899:2018' for 'ISO C 2017' standard",
        "use 'gnu17' or 'gnu18' for 'ISO C 2017 with GNU extensions' standard",
        "use 'c23' or 'iso9899:2024' for 'ISO C 2023' standard",
        "use 'gnu23' for 'ISO C 2023 with GNU extensions' standard",
        "use 'c2y' for 'Working Draft for ISO C2y' standard",
        "use 'gnu2y' for 'Working Draft for ISO C2y with GNU extensions' standard"};
    for (uint i = 0; i < sizeof C99 / sizeof C99[0]; i++) {
        if (strcmp(value, C99[i]) == 0) {
            return;
        }
    }
    struct pos none;
    memset(&none, 0, sizeof none);
    for (uint i = 0; i < sizeof OTHER_C / sizeof OTHER_C[0]; i++) {
        if (strcmp(value, OTHER_C[i]) == 0) {
            say(diag_error(none, fmt("merlin compiles C99: '-std=%s' is not supported", value)));
            return;
        }
    }
    int other = 0;
    for (uint i = 0; i < sizeof OTHER_LANGUAGES / sizeof OTHER_LANGUAGES[0]; i++) {
        other |= strcmp(value, OTHER_LANGUAGES[i]) == 0;
    }
    if (other) {
        say(diag_error(none, fmt("invalid argument '-std=%s' not allowed with 'C'", value)));
        return;
    }
    struct diag *e = diag_error(none, fmt("invalid value '%s' in '-std=%s'", value, value));
    for (uint i = 0; i < sizeof NOTES / sizeof NOTES[0]; i++) {
        diag_note(e, none, NOTES[i]);
    }
    say(e);
}

static int write_file(const char *path, const char *data, uint len) {
    int fd = open(path, O_WRONLY | O_CREAT | O_TRUNC, 0644);
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
    /* `.file "NAME"`, the first line of the assembly, as clang writes it with
     * or without -g: the base name of the file being compiled (merlin's
     * file_directive), whatever #line says; the code generator's text after
     * it. The assembler makes it the object's STT_FILE symbol; it has
     * nothing to do with the numbered .files of a line table. */
    const char *main_name = opts->main_file_name != NULL ? opts->main_file_name : name;
    uint base_len;
    const char *base = path_file_name(main_name, (uint)strlen(main_name), &base_len);
    buf_str(text, "    .file ");
    quote(text, base, base_len);
    buf_byte(text, '\n');
    /* -g: the files the preprocessor read, what the .file directives say
     * of them. */
    struct lines *debug = NULL;
    if (comp_dir != NULL) {
        debug = debug_new(comp_dir, pp.files, pp.lens, pp.sums, pp.nfiles, opts->main_file_name);
    }
    struct diag *error;
    if (!codegen_generate(&prog, debug, text, &error)) {
        diag_name_files(error, pp.files, pp.lens, pp.nfiles);
        print_all(&prog.warnings, name);
        diag_print(error, name);
        return 0;
    }
    print_all(&prog.warnings, name);
    return 1;
}

/* The input file's text, NUL-terminated (its length in *len), or NULL
 * with what stops it said: a file it cannot read, or one that is not UTF-8
 * (Rust's read_to_string reads only that). */
static char *read_input(struct input *in, uint *len) {
    int err = 0;
    char *src = in->from_stdin ? read_fd(0, len, &err) : read_file(in->name, len, &err);
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
    if (output == NULL || strcmp(output, "-") == 0) {
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
    opts->preprocessed = in->preprocessed;
    /* Standard input is `-` in the object, as clang's driver gives its
     * -cc1 `-main-file-name -`; <stdin> in diagnostics. */
    opts->main_file_name = in->from_stdin ? "-" : NULL;
    struct buf text;
    memset(&text, 0, sizeof text);
    int status = 1;
    /* A warning -Werror makes an error fails it too (diag.c). */
    diag_failed = 0;
    if (compile_unit(in->name, src, len, opts, dump, comp_dir, &text) && !diag_failed) {
        /* Without -o: the input with .s for its .c or .i (or added), as
         * clang's -cc1 names it; for standard input, and for --dump-ast,
         * standard output. */
        const char *out = output;
        if (out == NULL && !dump && !in->from_stdin) {
            uint n = (uint)strlen(in->name);
            out = ends_with(in->name, ".c") || ends_with(in->name, ".i")
                      ? fmt("%s.s", arena_strndup(in->name, n - 2))
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
    opts->preprocessed = in->preprocessed;
    struct buf text;
    memset(&text, 0, sizeof text);
    struct diag **diags = NULL;
    uint ndiags = 0;
    diag_failed = 0;
    int ok = preprocess_text(in->name, src, len, opts, &text, &diags, &ndiags);
    /* Diagnostics as clang prints them, on stderr: the warnings of a
     * preprocessing that succeeds, or every diagnostic of one that fails;
     * only errors fail. */
    for (uint i = 0; i < ndiags; i++) {
        diag_print(diags[i], in->name);
    }
    int status = ok && !diag_failed ? write_output(output, &text) : 1;
    /* The translation unit is done: its memory goes, the arena and the
     * heap (its source, the text and the diagnostics). */
    arena_free();
    return status;
}

/* The directory cc1 runs in, as merlin's std::env::current_dir finds it:
 * the getcwd system call, with a buffer that grows until the path fits (a
 * page is the kernel's limit). NULL if it fails (the directory removed,
 * say): the caller then uses `.`, as merlin does. For the whole run: plain
 * malloc, not the arena, which each translation unit frees. */
static char *current_dir(void) {
    for (uint size = 256; size <= 4096; size *= 2) {
        char *buf = malloc(size);
        if (buf == NULL) {
            die("out of memory");
        }
        long n = getcwd(buf, size);
        if (n > 0) {
            return buf;
        }
        free(buf);
        if (n != -ERANGE) {
            break;
        }
    }
    return NULL;
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
    /* @FILE: how the `cc` driver gives cc1 a command line longer than exec
     * takes (MAXARG arguments), as clang's driver gives its -cc1 one when a
     * command line is too long for the system; read as clang reads them
     * (the C library's expand_response_files, merlin's
     * excalibur::response). For the whole run: plain malloc. */
    char error[512];
    int nargs = 0;
    char **args = expand_response_files(argc, argv, &nargs, error, sizeof error);
    if (args == NULL) {
        /* LLVM's tools say it alone, and exit 1. */
        fprintf(2, "%s\n", error);
        return 1;
    }
    argc = nargs;
    argv = args;
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
    /* The last -O's level and the last -std='s value: the ones judged. */
    const char *opt = NULL;
    const char *std_value = NULL;
    /* -x LANG applies to the inputs after it, as in clang; -1 goes by each
     * file's name. */
    int lang = -1;
    /* Whether -x cpp-output gave the language. */
    int preprocessed = 0;
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
        } else if (strcmp(a, "-v") == 0) {
            verbose = 1;
        } else if (strcmp(a, "-ffreestanding") == 0) {
            /* merlin is freestanding (__STDC_HOSTED__ 0) already. */
        } else if (a[1] == 'O') {
            opt = a + 2;
        } else if (strncmp(a, "-std=", 5) == 0) {
            std_value = a + 5;
        } else if (is_warning_option(a)) {
            /* -w, and -W but for -Wl, -Wp, and -Wa, (the driver's, which
             * cc1, as clang's -cc1, does not take). */
            diag_take_w(a);
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
            preprocessed = 0;
            if (strcmp(l, "none") == 0) {
                lang = -1;
            } else if (strcmp(l, "c") == 0) {
                lang = LANG_C;
            } else if (strcmp(l, "cpp-output") == 0) {
                /* C already preprocessed (a .i file): macros expanded in
                 * directives alone, as clang's -cc1 takes it. */
                lang = LANG_C;
                preprocessed = 1;
            } else if (strcmp(l, "assembler-with-cpp") == 0) {
                lang = LANG_ASM;
            } else {
                return usage_error(fmt("language not recognized: '%s'", l));
            }
        } else if (a[0] == '-' && a[1] != 0) {
            return usage_error(fmt("unknown option %s", a));
        } else {
            int from_stdin = strcmp(a, "-") == 0;
            inputs[ninputs].name = from_stdin ? "<stdin>" : a;
            inputs[ninputs].lang = lang >= 0 ? lang : lang_of_file(a);
            inputs[ninputs].from_stdin = from_stdin;
            inputs[ninputs].preprocessed = lang >= 0 ? preprocessed : ends_with(a, ".i");
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
    /* The last -std='s and the last -O's diagnostics, in that order, before
     * anything else, as clang's -cc1 gives them as it reads its command line
     * (and as they are: -w and -W are not applied yet); an error stops it
     * there. */
    if (std_value != NULL) {
        check_std(std_value);
    }
    if (opt != NULL) {
        check_opt(opt);
    }
    for (uint i = 0; i < nsaid; i++) {
        diag_print_as_is(said[i], "");
    }
    if (said_error) {
        return 1;
    }
    /* -v: the version, as clang's -cc1 says it once it starts. */
    if (verbose) {
        fprintf(2, "cc1 version %s default target riscv64\n", VERSION);
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
            comp_dir = cwd != NULL ? cwd : ".";
        }
        opts.checksums = 1;
    } else {
        comp_dir = NULL;
    }
    int status = 0;
    for (uint i = 0; i < ninputs; i++) {
        if (verbose) {
            print_search_list(&opts);
        }
        int r = mode == MODE_PREPROCESS ? translate(&inputs[i], output, &opts)
                                        : compile_file(&inputs[i], output, &opts, mode == MODE_DUMP_AST, comp_dir);
        if (r != 0) {
            status = r;
        }
    }
    return status;
}
