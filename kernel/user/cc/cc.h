/* cc1: the C compiler that runs on Camelot (/bin/cc1), merlin rewritten in
 * the C subset merlin compiles (issue #241, the C compiler on Camelot, #243).
 * For the same input it writes what the host's merlin (crates/merlin)
 * writes, byte for byte, and gives the same diagnostics: the host compiler
 * is the reference, and crates/xtask/tests/selfhost.rs compares the two.
 * It is the compiler proper, as clang's `-cc1` and GCC's `cc1` are: the
 * `cc` driver (#242) runs it, then as and ld.
 *
 * It lands in stages. This one is the program's structure and the
 * preprocessor: `cc1 -E`, for C and for assembly (`.S`). Each file mirrors
 * a module of merlin, whose comments explain the rules in more depth:
 *
 *   main.c        the command line, reading the file (main.rs, lib.rs)
 *   util.c        memory (the arena), byte buffers, formatting, maps
 *   diag.c        diagnostics, printed as clang prints them (lib.rs's Error)
 *   lexer.c       text to preprocessing tokens, literals (lexer.rs)
 *   preprocess.c  directives, macros, `#if`, the -E output (preprocess.rs)
 *   tables.h      merlin's tables, generated from its sources by `cargo
 *   tables.c      xtask kernel` (crates/xtask/src/c_tables.rs): the
 *                 punctuators, the directives' names, the predefined macros
 *
 * Its numbers are merlin's types: uint for Rust's u32, int64_t and
 * uint64_t for i64 and u64. Text is UTF-8, as Rust's strings are, and is
 * handled a byte at a time where that comes to the same as Rust's chars,
 * a character at a time where it would not (see lexer.c). */
#ifndef CC_H
#define CC_H

#include "../user.h"
#include "tables.h"

/* ---- Memory (util.c) ----
 * The arena: one region at the end of the heap, grown with sbrk as it
 * fills, for what lives as long as the compilation (names, token texts,
 * macros, diagnostics). Nothing in it is freed; the process's exit frees
 * it all. What has a shorter life and can be large (a file's tokens while
 * it is read, the lexer's scratch, the output) is malloc'd and freed.
 * Out of memory is fatal. */
void *arena_alloc(uint n);
char *arena_strdup(const char *s);
char *arena_strndup(const char *s, uint n);
/* How many bytes the arena holds. */
uint arena_used(void);
/* A growable malloc'd array: grow(ptr, &cap, count + 1, size) makes room. */
void *grow(void *p, uint *cap, uint need, uint size);
void die(const char *msg);

/* A byte buffer (malloc'd). */
struct buf {
    char *data;
    uint len;
    uint cap;
};
void buf_put(struct buf *b, const void *p, uint n);
void buf_str(struct buf *b, const char *s);
void buf_byte(struct buf *b, uint c);
/* The buffer's bytes as a NUL-terminated string in the arena. */
char *buf_arena(struct buf *b);
void buf_free(struct buf *b);
/* Appends the formatted text: %s %d %u %c %%, %lld %llu for 64-bit
 * numbers. */
void buf_fmt(struct buf *b, const char *f, ...);
/* A formatted string in the arena (merlin's format!). */
char *fmt(const char *f, ...);

/* A map from strings to pointers (open addressing). The keys must live as
 * long as the map (arena strings). */
struct map {
    const char **keys;
    void **values;
    uint cap;
    uint count;
};
void *map_get(struct map *m, const char *key);
void map_put(struct map *m, const char *key, void *value);
/* Removes the key, if there; whether it was. */
int map_remove(struct map *m, const char *key);

/* The identifier `s` (len bytes), interned: the same text is the same
 * pointer, NUL-terminated, in the arena. */
const char *intern(const char *s, uint len);

/* ---- UTF-8 (util.c) ---- */
/* The length of the character whose first byte is c (1 for a byte that
 * starts none, which valid UTF-8 has not). */
uint utf8_len(uint c);
/* The character at s (n bytes left), its length in *len. */
uint utf8_decode(const uchar *s, uint n, uint *len);
/* Appends the character v as UTF-8. */
void utf8_put(struct buf *b, uint v);
/* Whether the bytes are valid UTF-8, as Rust's read_to_string checks. */
int valid_utf8(const uchar *s, uint n);

/* ---- Positions (merlin's Pos) ----
 * Which file (0 is the one being compiled, then the headers and the
 * built-in and command-line macros, numbered by the preprocessor in the
 * order it reads them), then the 1-based line and column (columns count
 * bytes, as clang's do). */
struct pos {
    uint file;
    uint line;
    uint col;
};
struct pos pos_at(uint file, uint line, uint col);
int pos_eq(struct pos a, struct pos b);
/* <0, 0, >0 as a is before, at or after b: by file, line, column. */
int pos_cmp(struct pos a, struct pos b);

/* ---- Diagnostics (diag.c, merlin's Error) ----
 * An error, which stops the compilation; a fatal one, after which clang
 * gives up; a warning, which does not stop it; a note, which follows one
 * of the others to explain it. */
enum { SEV_ERROR, SEV_FATAL, SEV_WARNING, SEV_NOTE };

struct diag {
    /* Where the problem is: line 0 when it is tied to no place. */
    struct pos pos;
    char *msg;
    /* The file's name when it is not the file being compiled (a header),
     * set by diag_name_files; "" for a diagnostic of no file at all. */
    const char *file;
    int severity;
    /* clang's -W flag for it, printed after the message, or NULL. */
    const char *flag;
    /* Its notes, in order, linked by `next`. */
    struct diag *notes;
    struct diag *next;
};

/* merlin's Error::new, ::warning, ::warning_unflagged, ::fatal: the
 * message is the caller's (an arena string or a constant). */
struct diag *diag_error(struct pos pos, const char *msg);
struct diag *diag_warning(struct pos pos, const char *msg, const char *flag);
struct diag *diag_warning_unflagged(struct pos pos, const char *msg);
struct diag *diag_fatal(struct pos pos, const char *msg);
/* Adds a note at pos after the notes d has; d. */
struct diag *diag_note(struct diag *d, struct pos pos, const char *msg);
int diag_is_warning(struct diag *d);
/* Sets the file of d and its notes from `files` (by pos.file), where it is
 * not the file being compiled (merlin's name_files). */
void diag_name_files(struct diag *d, const char **files, uint nfiles);
/* Prints d and its notes to stderr as clang does, `input` the name of the
 * file being compiled (merlin's Error::render). */
void diag_print(struct diag *d, const char *input);

/* ---- Preprocessing tokens (lexer.c, merlin's PpToken) ---- */
enum {
    PP_IDENT,  /* an identifier, keywords too */
    PP_NUMBER, /* a preprocessing number */
    PP_CHAR,   /* a character constant, as written */
    PP_STR,    /* a string literal, as written */
    PP_PUNCT,  /* a punctuator: punct is its index in PUNCTS */
    /* Text that is no token, with clang's warning for it in msg (or NULL). */
    PP_INVALID,
    /* The preprocessor's stand-in for an empty argument next to `##`. */
    PP_PLACEMARKER,
    /* A directive passed on, a whole line as -E writes it (`#pragma x`). */
    PP_DIRECTIVE
};

struct pptok {
    uchar kind;
    uchar punct;
    /* The first token of its line: a `#` here starts a directive. */
    uchar bol;
    /* Whitespace or a comment comes right before it. */
    uchar space;
    /* The token as written, line splices removed; NUL-terminated. An
     * identifier's is interned, a punctuator's is PUNCTS's. */
    const char *text;
    uint len;
    /* PP_INVALID: the warning clang gives for it, or NULL. */
    const char *msg;
    /* Its first character, and just past its last. */
    struct pos pos;
    struct pos end;
    /* The lexer's warnings of what comes before it (a trigraph, a splice
     * with blanks), linked by `next`: the preprocessor gives them as it
     * reads the token. */
    struct diag *lexed;
};

int tok_is_punct(const struct pptok *t, const char *p);
/* The identifier's name, or NULL if it is no identifier. */
const char *tok_ident(const struct pptok *t);

/* What lexing a file gives: its tokens (malloc'd, `n` of them) and where
 * it ends, or an error (an unterminated comment). */
struct lexed {
    struct pptok *toks;
    uint n;
    struct pos end;
    struct diag *error;
};
/* Splits src (n bytes of UTF-8) into preprocessing tokens, as C (asm 0) or
 * as assembly (asm 1: trigraphs are characters), its positions in file
 * `file` (merlin's lex_pp_in). */
void lex_pp(const char *src, uint n, uint file, int asm, struct lexed *out);
/* How `text` (n bytes) lexes, without keeping the tokens: how many tokens
 * it is (up to max, then max), whether it lexed at all (no unterminated
 * comment), and the kind and length of the first. For pasting and the
 * spacing of -E, which only ask that. */
uint lex_count(const char *text, uint n, int asm, uint max, int *ok, int *kind, uint *len);

/* An integer constant, as clang's NumericLiteralParser reads one (merlin's
 * lexer::integer). */
struct intlit {
    /* fits: it fits in 64 bits, its value in value; else low has its low
     * 64 bits. */
    int fits;
    uint64_t value;
    uint64_t low;
    int decimal;
    int is_unsigned;
    /* 1 for `l`, 2 for `ll`. */
    int lng;
};
/* Reads the preprocessing number `text` at pos; an error, or NULL. */
struct diag *lex_integer(const char *text, struct pos pos, struct intlit *out);
/* A character constant's value, as an int (merlin's char_constant), its
 * diagnostics added to *diags (linked by next, in order); an error, or
 * NULL. */
struct diag *lex_char_constant(const char *text, struct pos pos, struct diag **diags, int64_t *value);
/* The bytes of a string literal as written, escapes decoded (merlin's
 * decode), in *out; its warnings added to *diags; an error, or NULL. */
struct diag *lex_decode(const char *text, struct pos pos, struct diag **diags, struct buf *out);

/* Appends d to the list at *list (in order). */
void diag_append(struct diag **list, struct diag *d);

/* ---- The preprocessor (preprocess.c) ---- */
/* The language of the input, as clang's -x names it. */
enum { LANG_C, LANG_ASM };

/* A -D or -U, as written after it. */
struct cmd_macro {
    int undef;
    const char *text;
};

/* What the command line tells the preprocessor (merlin's Options). */
struct options {
    const char **include_dirs;
    uint ninclude_dirs;
    struct cmd_macro *macros;
    uint nmacros;
    int lang;
    /* Preprocessed assembly has line markers: -P turns them off. */
    int line_markers;
};

/* Preprocesses the file `name`, whose text is src (n bytes), for -E: the
 * text merlin -E writes, as C (merlin's spell) or as assembly (print_asm),
 * in *out, and the diagnostics, in order, in *diags (a malloc'd array of
 * *ndiags). Returns 0 if one of them is an error (the text is then not to
 * be written). */
int preprocess_text(const char *name, const char *src, uint n, struct options *opts, struct buf *out,
                    struct diag ***diags, uint *ndiags);

#endif
