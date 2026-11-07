/* cc1: the C compiler that runs on Camelot (/bin/cc1), merlin rewritten in
 * the C subset merlin compiles (issue #241, the C compiler on Camelot, #243).
 * For the same input it writes what the host's merlin (crates/merlin)
 * writes, byte for byte, and gives the same diagnostics: the host compiler
 * is the reference, and crates/xtask/tests/selfhost.rs compares the two.
 * It is the compiler proper, as clang's `-cc1` and GCC's `cc1` are: the
 * `cc` driver (#242) runs it, then as and ld.
 *
 * It lands in stages: the preprocessor (`cc1 -E`, for C and for assembly,
 * `.S`), then the compiler without -g (C to assembly for as, and
 * --dump-ast), which is what it is now; -g and the rest of merlin's
 * diagnostics come next. Each file mirrors a module of merlin:
 *
 *   main.c        the command line and the front end's driver: reading
 *                 the file, the order of the diagnostics, the error limit,
 *                 the output (main.rs, lib.rs)
 *   util.c        memory (the arena, the translation unit's heap), byte
 *                 buffers, formatting, maps, vectors, 128-bit integers,
 *                 sorting, UTF-8
 *   diag.c        diagnostics, printed as clang prints them (lib.rs's Error)
 *   lexer.c       text to preprocessing tokens, literals, and the parser's
 *                 tokens of them (lexer.rs)
 *   preprocess.c  directives, macros, `#if`, the -E output, the tokens it
 *                 hands the compiler (preprocess.rs)
 *   types.c       C's types, their sizes and how clang names them (types.rs)
 *   ast.c         the syntax tree, and --dump-ast's printer (ast.rs)
 *   consteval.c   constant expressions (consteval.rs)
 *   parser.c      tokens to the syntax tree (parser.rs)
 *   sema.c        names, types, conversions, warnings (sema.rs)
 *   flow.c        what of a function can run (flow.rs)
 *   sequence.c    -Wunsequenced (sequence.rs)
 *   elide.c       what a constant condition leaves out (elide.rs)
 *   abi.c         the calling convention (abi.rs)
 *   codegen.c     the checked tree to assembly (codegen.rs)
 *   tables.h      merlin's tables, generated from its sources by `cargo
 *   tables.c      xtask kernel` (crates/xtask/src/c_tables.rs): the
 *                 punctuators, the directives' names, the predefined
 *                 macros, the keywords, the parser's lists and limits, the
 *                 registers' names
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
 * The arena, for what lives as long as the compilation of one translation
 * unit (names, token texts, macros, diagnostics): a bump allocator over
 * chunks of 256 KiB, a big allocation in a block of its own, nothing in it
 * freed on its own, and all of it freed when the translation unit is done
 * (arena_free), as merlin's memory is. What grows, or has a shorter life
 * and can be large (an array, a map's table, a file's tokens while it is
 * read, the lexer's scratch, the output), is on the translation unit's
 * heap: malloc'd, freed when done with (heap_free), and what is left of it
 * freed with the arena. Only what lives for the whole run (the command
 * line's lists) is plain malloc's. Out of memory is fatal. */
void *arena_alloc(uint n);
char *arena_strdup(const char *s);
char *arena_strndup(const char *s, uint n);
/* Frees the arena and the heap, the interned names with them: nothing
 * allocated in either may be used after. */
void arena_free(void);
/* Where the arena is: arena_release gives back everything allocated there
 * since arena_mark, which nothing may use after (nor may a name have been
 * interned in between). For work whose results are thrown away: the code
 * generator's first pass over a function. */
struct arena_point {
    void *chunk;
    uint used;
    void *big;
};
void arena_mark(struct arena_point *m);
void arena_release(struct arena_point *m);
/* malloc, calloc, realloc and free on the translation unit's heap. */
void *heap_alloc(size_t n);
void *heap_calloc(size_t n, size_t size);
void *heap_realloc(void *p, size_t n);
void heap_free(void *p);
/* A growable array on the heap: grow(ptr, &cap, count + 1, size) makes
 * room. */
void *grow(void *p, uint *cap, uint need, uint size);
void die(const char *msg);

/* A byte buffer (on the heap). */
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
 * numbers, and %b for bytes that may hold a NUL (a pointer and a uint
 * length). */
void buf_fmt(struct buf *b, const char *f, ...);
/* A formatted string in the arena (merlin's format!). */
char *fmt(const char *f, ...);
/* The same, with its length in *len, for text that may hold a NUL. */
char *fmtn(uint *len, const char *f, ...);

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
/* Frees the map's tables (not its keys or values), leaving it empty. */
void map_free(struct map *m);

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
/* The bytes as UTF-8, what is ill-formed replaced by U+FFFD (Rust's
 * String::from_utf8_lossy), in the arena. */
char *from_utf8_lossy(const uchar *s, uint n, uint *out_len);

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
/* <0, 0, >0 as a is before, at or after b: by file, line, column (the
 * order Rust derives for merlin's Pos). */
int pos_cmp(struct pos a, struct pos b);

/* ---- Diagnostics (diag.c, merlin's Error) ----
 * An error, which stops the compilation; a fatal one, after which clang
 * gives up; a warning, which does not stop it; a note, which follows one
 * of the others to explain it. */
enum { SEV_ERROR, SEV_FATAL, SEV_WARNING, SEV_NOTE };

struct diag {
    /* Where the problem is: line 0 when it is tied to no place. */
    struct pos pos;
    /* The message, msglen bytes: it may hold a NUL, when it quotes the
     * source (`#error`, a header's name). */
    const char *msg;
    uint msglen;
    /* The file's name (filelen bytes) when it is not the file being
     * compiled (a header), set by diag_name_files; "" for a diagnostic of
     * no file at all. */
    const char *file;
    uint filelen;
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
/* An error, or a fatal one, whose message is len bytes that may hold a
 * NUL. */
struct diag *diag_error_n(struct pos pos, const char *msg, uint len);
struct diag *diag_fatal_n(struct pos pos, const char *msg, uint len);
/* A warning of that kind. */
struct diag *diag_warning_n(struct pos pos, const char *msg, uint len, const char *flag);
/* Adds a note at pos after the notes d has; d. */
struct diag *diag_note(struct diag *d, struct pos pos, const char *msg);
int diag_is_warning(struct diag *d);
/* merlin's name_files: sets the file of d and its notes from the files'
 * names (names[i], lens[i] bytes, by pos.file) where it is not the file
 * being compiled, and names the file of each place a message gives
 * between two FILE_MARK bytes. */
void diag_name_files(struct diag *d, const char **names, const uint *lens, uint nfiles);
/* What stands around a file's number in a message, for its name
 * (merlin's FILE_MARK, U+0001): sema's places (`struct (unnamed at
 * t.c:3:1)`) are written so, and a message that quotes the source may
 * hold one too. */
#define FILE_MARK 1
/* Prints d and its notes to stderr as clang does, `input` the name of the
 * file being compiled (merlin's Error::render), under -w and -W: a warning
 * left out, or made an error (`[-Werror,-WFLAG]`), which sets
 * diag_failed (main.c fails the translation unit then). */
void diag_print(struct diag *d, const char *input);
/* Takes -w or a -W option, in command-line order, which diag_print applies
 * as clang does (lib/warnings.c's warning_level, merlin::warnings). */
void diag_take_w(char *opt);
int diag_option_warnings(void);
/* Whether a fatal error was said (diag.c): nothing more is. */
extern int diag_fatal_seen;
/* Prints d and its notes as they are, -w and -W aside (the command line's
 * own diagnostics). */
void diag_print_as_is(struct diag *d, const char *input);
extern int diag_failed;

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
    /* The token as written, line splices removed: len bytes, and a NUL after
     * them (the source may hold a NUL too, which len counts). An
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

/* What lexing a file gives: its tokens (on the heap, `n` of them) and where
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
/* How a short text lexes, for pasting and the spacing of -E, which only ask
 * whether it is one token or two: its first tokens (up to 3), their kinds,
 * and where each one's text is in `chars`, the text after phases 1 and 2. */
struct split {
    /* It lexed (no unterminated comment) as far as it was read. */
    int ok;
    uint count;
    uchar kind[3];
    uchar punct[3];
    uint start[3];
    uint len[3];
    uchar *chars;
};
/* Lexes at most max (up to 3) tokens of text (n bytes), as C or assembly;
 * s->ok. Free it with lex_split_free. */
int lex_split(const char *text, uint n, int asm, uint max, struct split *s);
void lex_split_free(struct split *s);
/* Whether token k of s is the n bytes at text. */
int split_is(struct split *s, uint k, const char *text, uint n);

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
/* Each takes a token's text and its length (a literal may hold a NUL).
 *
 * Reads the preprocessing number `text` at pos; an error, or NULL. */
struct diag *lex_integer(const char *text, uint len, struct pos pos, struct intlit *out);
/* A character constant's value, as an int (merlin's char_constant), its
 * diagnostics added to *diags (linked by next, in order); an error, or
 * NULL. */
struct diag *lex_char_constant(const char *text, uint len, struct pos pos, struct diag **diags,
                               int64_t *value);
/* The bytes of a string literal as written, escapes decoded (merlin's
 * decode), in *out; its warnings added to *diags; an error, or NULL. */
struct diag *lex_decode(const char *text, uint len, struct pos pos, struct diag **diags, struct buf *out);

/* Appends d to the list at *list (in order). */
void diag_append(struct diag **list, struct diag *d);


/* ---- 128-bit integers (util.c) ----
 * merlin's constants are Rust's i128s, which hold every value of every
 * integer type (an unsigned long goes up to 2^64 - 1) as the number it is,
 * and what is worked out from them (a product of two of them, before it is
 * wrapped to its type) without overflowing. C has no such type, so this is
 * one: two's complement in 128 bits, `hi` the high half. */
typedef struct {
    uint64_t lo;
    uint64_t hi;
} i128;

i128 i128_of(int64_t v);
i128 i128_of_u64(uint64_t v);
int i128_is_zero(i128 a);
int i128_eq(i128 a, i128 b);
/* <0, 0, >0 as a is less than, equal to or greater than b. */
int i128_cmp(i128 a, i128 b);
int i128_lt(i128 a, i128 b);
int i128_le(i128 a, i128 b);
int i128_neg_p(i128 a);
i128 i128_add(i128 a, i128 b);
i128 i128_sub(i128 a, i128 b);
i128 i128_neg(i128 a);
i128 i128_mul(i128 a, i128 b);
/* Division truncating toward zero and its remainder, as Rust's (and C's):
 * b is not 0. */
i128 i128_div(i128 a, i128 b);
i128 i128_rem(i128 a, i128 b);
i128 i128_and(i128 a, i128 b);
i128 i128_or(i128 a, i128 b);
i128 i128_xor(i128 a, i128 b);
i128 i128_not(i128 a);
/* Shifts by n < 128 bits; shr is arithmetic, as Rust's on i128. */
i128 i128_shl(i128 a, uint n);
i128 i128_shr(i128 a, uint n);
uint i128_leading_zeros(i128 a);
/* Whether it is a value of int64_t (and of uint64_t). */
int i128_fits_i64(i128 a);
int i128_fits_u64(i128 a);
int64_t i128_i64(i128 a);
/* In decimal, as Rust's Display writes it. */
char *i128_str(i128 a);
void buf_i128(struct buf *b, i128 a);
i128 i128_min(i128 a, i128 b);
i128 i128_max(i128 a, i128 b);

/* ---- Vectors (util.c) ----
 * A growable array of pointers (on the heap), Rust's Vec of what merlin keeps
 * by value: in cc1 each element is its own object, in the arena, so moving
 * one between vectors moves the pointer. */
struct vec {
    void **v;
    uint n;
    uint cap;
};
void vec_push(struct vec *l, void *p);
/* The last element (the vector is not empty), and taking it away. */
void *vec_last(struct vec *l);
void *vec_pop(struct vec *l);
/* Rust's mem::take: *l's elements, l left empty. */
struct vec vec_take(struct vec *l);
/* Appends every element of from. */
void vec_extend(struct vec *l, struct vec *from);
/* Rust's split_off: the elements from `at` on, which l loses. */
struct vec vec_split_off(struct vec *l, uint at);
void vec_insert(struct vec *l, uint at, void *p);
void *vec_remove(struct vec *l, uint at);
void vec_truncate(struct vec *l, uint n);
/* Sorts the n pointers at v so that none comes before one it is `before`,
 * keeping the order of those neither is before (a stable sort, as Rust's
 * sort and sort_by_key are), in O(n log n). */
void sort_stable(void **v, uint n, int (*before)(void *a, void *b));

/* A map from positions to pointers, and a set of positions (a map whose
 * values are not looked at): Rust's HashMap<Pos, _> and HashSet<Pos>. */
struct posmap {
    struct pos *keys;
    void **values;
    uchar *used;
    uint cap;
    uint count;
};
void *posmap_get(struct posmap *m, struct pos key);
int posmap_has(struct posmap *m, struct pos key);
void posmap_put(struct posmap *m, struct pos key, void *value);
/* A position, kept in the arena, as a posmap's value. */
struct pos *pos_box(struct pos p);

/* A map from 64-bit numbers (expression ids, local ids) to pointers. */
struct intmap {
    uint64_t *keys;
    void **values;
    uchar *used;
    uint cap;
    uint count;
};
void *intmap_get(struct intmap *m, uint64_t key);
int intmap_has(struct intmap *m, uint64_t key);
void intmap_put(struct intmap *m, uint64_t key, void *value);

/* ---- The parser's tokens (lexer.c, merlin's Token and Tok) ---- */
enum { TK_IDENT, TK_NUM, TK_STR, TK_KW, TK_PUNCT, TK_UNKNOWN, TK_EOF };

struct token {
    uchar kind;
    /* TK_KW: which keyword (KW_, tables.h). */
    uchar kw;
    /* TK_NUM: its type. */
    uchar ik;
    /* TK_NUM: its value, in range for its type. */
    i128 value;
    /* TK_IDENT: the name (interned); TK_PUNCT: the punctuator, PUNCTS's. */
    const char *name;
    /* TK_STR: its bytes, escapes decoded, without the terminating NUL. */
    uchar *bytes;
    uint len;
    struct pos pos;
    struct pos end;
    /* As written in the source. */
    const char *text;
};

int kw_starts_decl(int k);
/* Phase 7: the parser's tokens (an array on the heap, TK_EOF last) of the
 * preprocessed ones; diagnostics added to diags; 0 for a keyword merlin
 * does not handle, the last diagnostic its error. */
int lex_finish(struct pptok *toks, uint n, struct pos end, struct token **out, uint *nout, struct vec *diags);

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
    /* Each file read has its MD5 taken, for -g's line table. */
    int checksums;
    /* C already preprocessed (-x cpp-output, a .i file): every directive
     * runs, but macros are expanded only in directives, as clang's
     * SetMacroExpansionOnlyInDirectives has it (merlin's
     * Options::preprocessed). */
    int preprocessed;
    /* The main file's name in the object, if not the one its diagnostics
     * give it (merlin's Options::main_file_name, clang's -main-file-name):
     * the .file of its STT_FILE symbol and, with -g, the compile unit's and
     * .file 0's name. "-" for standard input, <stdin> in diagnostics. */
    const char *main_file_name;
};

/* Preprocesses the file `name`, whose text is src (n bytes), for -E: the
 * text merlin -E writes, as C (merlin's spell) or as assembly (print_asm),
 * in *out, and the diagnostics, in order, in *diags (an array on the heap of
 * *ndiags). Returns 0 if one of them is an error (the text is then not to
 * be written). */
int preprocess_text(const char *name, const char *src, uint n, struct options *opts, struct buf *out,
                    struct diag ***diags, uint *ndiags);

/* What preprocessing a translation unit to compile it gives (merlin's
 * Output). */
struct pp_output {
    /* Every token, macros replaced, directives gone. */
    struct pptok *toks;
    uint ntoks;
    /* The positions of the tokens a macro's expansion made. */
    struct posmap macros;
    /* The end of the file being compiled. */
    struct pos end;
    /* The name of each file, by number, and its length (a name #line gives
     * may hold a NUL). */
    const char **files;
    uint *lens;
    uint nfiles;
    /* Each file's checksum, by number, if options.checksums asked for them
     * (merlin's Output::checksums): of its bytes as read. */
    struct checksum *sums;
    /* The diagnostics, in order, and for each where the last token that
     * came out before it is. */
    struct diag **diags;
    struct pos *after;
    uint ndiags;
};
int preprocess_unit(const char *name, const char *src, uint n, struct options *opts, struct pp_output *out);

/* ---- Types (types.c, merlin's types.rs) ---- */

/* The integer types (merlin's IntKind), in its order. */
enum {
    IK_BOOL,
    IK_CHAR,
    IK_SCHAR,
    IK_UCHAR,
    IK_SHORT,
    IK_USHORT,
    IK_INT,
    IK_UINT,
    IK_LONG,
    IK_ULONG,
    IK_LLONG,
    IK_ULLONG
};

/* The size of a pointer, and of long: 8 bytes (LP64). */
#define PTR_SIZE 8
/* The largest object merlin lays out: sizes fit a signed 32-bit offset. */
#define MAX_OBJECT_SIZE 2147483647u

uint ik_size(int k);
int ik_signed(int k);
const char *ik_name(int k);
int ik_promoted(int k);
int ik_size_t(void);
int ik_ptrdiff_t(void);
i128 ik_min(int k);
i128 ik_max(int k);
i128 ik_wrap(int k, i128 v);
/* The usual arithmetic conversions of two integer types (merlin's common). */
int ik_common(int a, int b);

/* const and volatile (merlin's Quals), as bits. */
enum { Q_CONST = 1, Q_VOLATILE = 2 };
/* "const volatile", "const", "volatile" or "". */
const char *quals_name(uint q);

enum { TY_VOID, TY_INT, TY_PTR, TY_ARRAY, TY_RECORD, TY_FUNC, TY_QUAL };

/* A C type (merlin's Type). Types are never changed once made, so they are
 * shared freely; two are the same type when ty_eq says so (structs and
 * unions by identity). */
struct type {
    uchar kind;
    /* TY_INT: which (IK_). */
    uchar ik;
    /* TY_QUAL: its qualifiers (Q_ bits, at least one). */
    uchar quals;
    /* TY_ARRAY: whether its length is known. */
    uchar has_len;
    uint len;
    /* TY_PTR: what it points to; TY_ARRAY: its elements; TY_QUAL: the type
     * qualified (never an array or another TY_QUAL). */
    struct type *to;
    struct record *rec;
    struct functype *fn;
};

/* How a declaration spelled the base of its type, where that says more than
 * the type (merlin's Spelling): through a typedef name, or as an enum. A
 * NULL `struct spelling *` is merlin's Spelled(None). */
enum { SP_TYPEDEF, SP_ENUM };
struct spelling {
    int kind;
    struct typedef_name *td;
    struct enum_type *en;
};

struct typedef_name {
    const char *name;
    struct type *ty;
    struct spelling *spelled;
    struct pos pos;
};

struct enum_const {
    const char *name;
    i128 value;
};

struct enum_type {
    /* NULL for an enum without a tag. */
    const char *tag;
    struct type *ty;
    struct enum_const *constants;
    uint nconstants;
    struct pos pos;
    /* The typedef's name that names it, if it has no tag (or NULL). */
    const char *typedef_name;
};

/* A parameter as its declaration had it (merlin's Spellings). */
struct param_spelling {
    struct type *ty;
    struct spelling *spelled;
};

struct functype {
    struct type *ret;
    struct type **params;
    uint nparams;
    uchar variadic;
    /* Declared with () (merlin's Unprototyped). */
    uchar unprototyped;
    /* Each parameter's declared type and spelling, or none (nspelled 0). */
    struct param_spelling *spelled;
    uint nspelled;
};

struct member {
    const char *name;
    struct type *ty;
    uint offset;
    struct spelling *spelled;
    struct pos pos;
    struct pos begin;
};

struct record_body {
    struct member *members;
    uint nmembers;
    uint size;
    uint align;
    struct pos pos;
};

/* A struct or union (merlin's Record): each definition (or first use of a
 * tag) makes one, which every use shares. */
struct record {
    uchar is_union;
    /* NULL for an anonymous one. */
    const char *tag;
    struct pos pos;
    const char *typedef_name;
    /* NULL while it is incomplete. */
    struct record_body *body;
};

/* A member a record is completed with (merlin's tuple of them). */
struct member_decl {
    const char *name;
    struct type *ty;
    struct spelling *spelled;
    struct pos pos;
    struct pos begin;
};

extern struct type *TY_VOID_T;
struct type *ty_int(int k);
struct type *ty_ptr(struct type *to);
/* An array of n (has_len) elements, or of unknown size. */
struct type *ty_array(struct type *of, int has_len, uint n);
struct type *ty_func(struct type *ret, struct type **params, uint n, int variadic);
struct type *ty_qualified(struct type *t, uint q);
struct type *ty_unqual(struct type *t);
int ty_eq(struct type *a, struct type *b);
int ty_similar(struct type *a, struct type *b);
struct type *ty_without_quals(struct type *t);
uint ty_quals(struct type *t);
/* The qualifiers t adds to of, or -1 if t is not of so qualified. */
int ty_quals_added_to(struct type *t, struct type *of);
/* sizeof, or -1 for none (void, a function, an incomplete type). */
int64_t ty_size(struct type *t);
uint ty_align(struct type *t);
int ty_is_integer(struct type *t);
int ty_is_ptr(struct type *t);
int ty_is_array(struct type *t);
int ty_is_func(struct type *t);
int ty_is_void(struct type *t);
struct record *ty_record(struct type *t);
int ty_is_scalar(struct type *t);
int ty_is_addressed(struct type *t);
/* The integer kind, or -1. */
int ty_int_kind(struct type *t);
struct type *ty_decay(struct type *t);
/* What adding 1 moves a pointer by, or -1. */
int64_t ty_step(struct type *t);
/* What a pointer or array points to, or NULL. */
struct type *ty_pointee(struct type *t);
int ty_is_unsigned(struct type *t);
int ty_compatible(struct type *a, struct type *b);
struct type *ty_composite(struct type *a, struct type *b);
/* The type as clang names it in a message, quoted, as spelled says. */
char *ty_named(struct type *t, struct spelling *spelled);
/* The type as a declaration of name (merlin's declare; "" for the type's
 * own name, its Display). */
char *ty_declare(struct type *t, const char *name);
char *ty_str(struct type *t);
int fn_has_prototype(struct functype *f);
int fn_compatible(struct functype *a, struct functype *b);
/* Parameter i's spelling, if the type has them (else NULL). */
struct spelling *fn_param_spelled(struct functype *f, uint i);
struct enum_type *enum_of(struct spelling *spelled, struct type *ty);
int distinct_enums(struct type *a, struct spelling *sa, struct type *b, struct spelling *sb);
uint64_t align_up(uint64_t n, uint align);

struct record *record_new(int is_union, const char *tag, struct pos pos);
const char *record_keyword(struct record *r);
/* Lays out the members and completes the record; 0 if it was complete. */
int record_complete(struct record *r, struct member_decl *members, uint n, struct pos pos);
char *record_name(struct record *r);
struct member *record_member(struct record_body *b, const char *name);
char *enum_name(struct enum_type *e);
struct type *spelling_ty(struct spelling *s);
struct spelling *spelling_typedef(struct typedef_name *t);
struct spelling *spelling_enum(struct enum_type *e);

/* clang's place of an unnamed struct, union or enum, `\1FILE\1LINE:COL`,
 * the file's number between two FILE_MARKs (above), named by
 * diag_name_files. */
char *place(struct pos pos);

/* ---- The syntax tree (ast.c, merlin's ast.rs) ---- */

enum {
    OP_ADD,
    OP_SUB,
    OP_MUL,
    OP_DIV,
    OP_REM,
    OP_SHL,
    OP_SHR,
    OP_BITAND,
    OP_BITOR,
    OP_BITXOR,
    OP_EQ,
    OP_NE,
    OP_LT,
    OP_LE,
    OP_GT,
    OP_GE,
    OP_LOGAND,
    OP_LOGOR,
    /* An assignment's: plain `=`. */
    OP_NONE
};
const char *binop_symbol(int op);
int binop_is_comparison(int op);

enum { UOP_NEG, UOP_PLUS, UOP_NOT, UOP_BITNOT };
const char *unop_symbol(int op);

/* Where a variable lives (merlin's VarRef). */
enum { VAR_NONE, VAR_LOCAL, VAR_GLOBAL };
struct var {
    uchar kind;
    /* VAR_LOCAL: its index in its function's locals. */
    uint local;
    /* VAR_GLOBAL: its symbol. */
    const char *sym;
};
int var_eq(struct var a, struct var b);

enum {
    E_NUM,
    E_ENUMERATOR,
    E_STR,
    E_VAR,
    E_ASSIGN,
    E_INCDEC,
    E_BINARY,
    E_UNARY,
    E_ADDR,
    E_DEREF,
    E_CAST,
    E_SIZEOF_TYPE,
    E_SIZEOF_EXPR,
    E_COND,
    E_COMMA,
    E_MEMBER,
    E_CALL,
    E_VA_START,
    E_VA_ARG,
    E_VA_END,
    E_VA_COPY,
    E_COMPOUND
};

/* An expression (merlin's Expr and ExprKind). Its operands are x, y and z,
 * as each kind says:
 *   E_ASSIGN   x the target, y the value; op the operator, OP_NONE for =
 *   E_INCDEC   x the target; flag: ++ (else --), prefix
 *   E_BINARY   x op y
 *   E_UNARY    op x
 *   E_ADDR, E_DEREF, E_CAST, E_SIZEOF_EXPR, E_VA_ARG, E_VA_END   x
 *   E_COND     x ? y : z
 *   E_COMMA    x, y
 *   E_MEMBER   x the base; flag: ->
 *   E_CALL     x the callee, args the arguments
 *   E_VA_START x ap, y last;  E_VA_COPY x dst, y src
 * Each operand belongs to its expression alone: what merlin clones, cc1
 * copies (expr_clone). */
struct expr {
    uchar kind;
    uchar op;
    uchar flag;
    uchar prefix;
    /* E_CAST: implicit. */
    uchar implicit;
    struct type *ty;
    struct pos pos;
    /* Its own number (merlin's ExprId): a copy keeps its original's. */
    uint64_t id;
    struct expr *x;
    struct expr *y;
    struct expr *z;
    /* E_NUM, E_ENUMERATOR. */
    i128 value;
    /* E_ENUMERATOR, E_VAR, E_MEMBER. */
    const char *name;
    /* E_ENUMERATOR: its enum; E_CAST: how an explicit one spelled its type. */
    struct spelling *spelled;
    /* E_STR: the bytes, without the terminating NUL. */
    uchar *bytes;
    uint len;
    /* E_VAR. */
    struct var var;
    /* E_MEMBER. */
    uint offset;
    /* E_MEMBER: where the name is; E_SIZEOF_EXPR: where the operand
     * begins; E_VA_ARG: where the type is; E_CALL: its `(`. */
    struct pos at;
    /* E_CALL: its `)`. */
    struct pos at2;
    /* E_SIZEOF_TYPE. */
    struct type *of;
    /* E_CALL: the arguments; the hidden local of a struct result (or -1);
     * and per argument the local it is copied to, or -1 (narg_copies of
     * them, 0 before sema). */
    struct expr **args;
    uint nargs;
    int result;
    int *arg_copies;
    uint narg_copies;
    /* E_COMPOUND: the list, and its hidden local (or -1). */
    struct init *init;
    int local;
};

struct expr *expr_new(int kind, struct pos pos);
/* A deep copy, ids kept (merlin's clone). */
struct expr *expr_clone(struct expr *e);
/* The number the next expression gets (merlin's ExprId::next). */
uint64_t expr_next_id(void);
uint64_t expr_fresh_id(void);
struct expr *expr_num(i128 v, struct type *ty, struct pos pos);
struct expr *expr_unop(int kind, struct expr *x, struct pos pos);
/* In a checked tree, the symbol a call goes straight to, or NULL. */
const char *direct_callee(struct expr *e);

/* One step of a designation. */
struct designator {
    /* A member (.name), else an index ([index]). */
    uchar member;
    const char *name;
    struct expr *index;
    /* Where its `.` or `[` is, and a member's name. */
    struct pos pos;
    struct pos at;
};

enum { I_EXPR, I_LIST, I_DESIGNATED };
struct init {
    uchar kind;
    /* I_EXPR. */
    struct expr *e;
    /* I_LIST: the items (struct init *), and where its `{` is. */
    struct vec items;
    struct pos pos;
    /* I_DESIGNATED: the designators, then what they place. */
    struct designator **ds;
    uint nds;
    struct init *inner;
};
struct pos init_pos(struct init *i);
struct init *init_clone(struct init *i);

/* The params in scope of an unevaluated expression: a name, its type,
 * where it is declared. */
struct proto_param {
    const char *name;
    struct type *ty;
    struct pos pos;
};

/* A range of indexes [start, end). */
struct range {
    uint start;
    uint end;
};

struct file_vla {
    struct pos at;
    int has_top;
    struct range top;
    struct range *inner;
    uint ninner;
};

/* An expression the parser needed no value of, for sema to check (merlin's
 * Unevaluated, whose comments explain each field). */
struct unevaluated {
    struct expr *expr;
    int has_untyped;
    struct pos untyped_pos;
    const char *untyped_msg;
    int has_non_integer;
    struct pos non_integer_pos;
    const char *non_integer_msg;
    uchar folded;
    uchar constant;
    struct diag *diagnostic;
    uint with;
    struct file_vla *vla;
    struct proto_param *params;
    uint nparams;
};
struct unevaluated *unevaluated_new(struct expr *e);

enum { ST_NONE, ST_STATIC, ST_EXTERN, ST_REGISTER };

/* One name of a declaration (merlin's Declarator). */
struct declarator {
    const char *name;
    struct type *ty;
    int storage;
    /* NULL for none. */
    struct init *init;
    struct var var;
    uchar define;
    struct pos pos;
    struct pos begin;
    struct spelling *spelled;
    /* struct unevaluated *, before its name and after its initializer. */
    struct vec sized;
    struct vec unevaluated;
};

struct param {
    const char *name;
    struct type *ty;
    struct pos pos;
    struct spelling *spelled;
    uchar register_;
    /* The array type it was declared as, or NULL. */
    struct type *array;
};

struct local {
    const char *name;
    struct type *ty;
    struct pos pos;
    uchar temp;
};

struct function {
    const char *name;
    struct type *ret;
    struct spelling *ret_spelled;
    struct param *params;
    uint nparams;
    uchar variadic;
    uchar is_static;
    uchar is_inline;
    uchar is_extern;
    uchar inline_only;
    uchar prototyped;
    /* A definition (else a prototype), and its body's statements. */
    uchar has_body;
    struct vec body;
    /* struct local *. */
    struct vec locals;
    int ret_ptr;
    struct pos pos;
    struct pos end;
    struct vec unevaluated;
};
struct type *function_ty(struct function *f);

enum {
    S_BLOCK,
    S_UNEVALUATED,
    S_DECL,
    S_EXPR,
    S_EMPTY,
    S_IF,
    S_WHILE,
    S_DOWHILE,
    S_FOR,
    S_RETURN,
    S_BREAK,
    S_CONTINUE,
    S_SWITCH,
    S_CASE,
    S_DEFAULT,
    S_LABEL,
    S_GOTO
};

/* A switch's case: its value, and its id. */
struct case_ref {
    i128 value;
    uint id;
};

/* A statement (merlin's Stmt and StmtKind):
 *   S_BLOCK        list: the statements
 *   S_UNEVALUATED  list: struct unevaluated *
 *   S_DECL         list: struct declarator *
 *   S_EXPR         e
 *   S_EMPTY        glued
 *   S_IF           e the condition, body then, els (or NULL)
 *   S_WHILE, S_DOWHILE   e, body
 *   S_FOR          init (or NULL), e the condition (or NULL), step (or
 *                  NULL), body; list: unevaluated
 *   S_RETURN       e (or NULL)
 *   S_SWITCH       e, body, cases, id the default's (-1 for none), covered
 *   S_CASE         e, value, id, body, bare
 *   S_DEFAULT      id, body
 *   S_LABEL        name, body
 *   S_GOTO         name, at */
struct stmt {
    uchar kind;
    uchar glued;
    uchar covered;
    uchar bare;
    struct pos pos;
    struct vec list;
    struct expr *e;
    struct expr *step;
    struct stmt *init;
    struct stmt *body;
    struct stmt *els;
    struct case_ref *cases;
    uint ncases;
    int id;
    i128 value;
    const char *name;
    struct pos at;
};
struct stmt *stmt_new(int kind, struct pos pos);

enum { ITEM_FUNC, ITEM_DECL };
struct item {
    int kind;
    struct function *func;
    /* ITEM_DECL: struct declarator *. */
    struct vec decls;
};

/* What clang takes a misspelt name for (merlin's Correction). */
struct correction {
    const char *name;
    struct pos pos;
    int has_enumerator;
    i128 value;
    struct spelling *of;
};

/* How the program was written, past its tree (merlin's Written): parens
 * and brackets map to struct pos *, corrections to struct correction *;
 * the rest are sets. */
struct written {
    struct posmap parens;
    struct posmap macros;
    struct posmap brackets;
    struct posmap bit_patterns;
    struct posmap int_literals;
    struct posmap char_literals;
    struct posmap corrections;
};

/* An unevaluated expression of the file's scope, after how many items. */
struct file_unevaluated {
    uint after;
    struct unevaluated *u;
};

struct program {
    /* struct item *. */
    struct vec items;
    /* struct file_unevaluated *. */
    struct vec unevaluated;
    /* struct diag *. */
    struct vec warnings;
    struct written written;
};

/* ---- Constant expressions (consteval.c, merlin's consteval.rs) ---- */

/* What an address constant points into: nothing (a plain number used as an
 * address), a global symbol, or a string literal. */
enum { BASE_NONE, BASE_SYM, BASE_STR };

/* A constant (merlin's Const): an integer of a type (v, ik), or an address,
 * base + offset bytes. */
struct cval {
    int is_addr;
    i128 v;
    int ik;
    int base;
    const char *sym;
    uchar *bytes;
    uint len;
    i128 offset;
};

/* Each returns whether there is a value, and puts it in its last argument. */
int consteval_int(struct expr *e, i128 *v);
int consteval_parsed_int(struct expr *e, i128 *v);
int consteval_parsed_unevaluated_int(struct expr *e, i128 *v);
int is_null_pointer(struct expr *e);
int consteval_eval(struct expr *e, struct cval *out);
int consteval_in_code(struct expr *e, struct cval *out);
int consteval_overflow_at(struct expr *e, int checked, int division, i128 *wrapped, int *kind);
int consteval_parsed_note(struct expr *e, struct pos *at, char **msg);
int consteval_note(struct expr *e, int checked, struct pos *at, char **msg);
int consteval_why_not(struct expr *e, struct pos *at, const char **msg);
int consteval_condition(struct expr *e, i128 *v);
int consteval_cfg_condition(struct expr *e, i128 *v);

/* ---- The parser (parser.c, merlin's parser.rs) ---- */

/* clang's typo correction of typo among n names: the index of the one it
 * takes, or -1. */
int correction(const char *typo, const char **names, uint n);
/* Parses the tokens (TK_EOF last): 1 and the program, or 0, the syntax
 * error and what came before it in *prog. */
int parse_program(struct token *toks, uint ntoks, struct program *prog, struct diag **error);

/* ---- Sema (sema.c, merlin's sema.rs) ---- */

/* merlin's check_after: 0 with the warnings added to the program's, or 1
 * with every diagnostic in *errors. earlier: where the errors before sema's
 * are reported. */
int sema_check(struct program *prog, struct pos *earlier, uint nearlier, struct vec *errors);
/* merlin's check_cut, of a program a syntax error at cut cut short: the
 * diagnostics before the error and after it. */
void sema_check_cut(struct program *prog, struct pos *earlier, uint nearlier, struct pos cut, struct vec *before,
                    struct vec *after);
/* e wrapped in an implicit conversion to `to`, in place, unless it has that
 * type. */
void coerce(struct expr *e, struct type *to);
/* The type of l op r for arithmetic, bitwise and shift operators, of
 * decayed operands; NULL if C does not allow them. */
struct type *arith_type(int op, struct type *l, struct type *r);
char *symbol_key(const char *base, uint n);
char *symbol_base(const char *key);
int same_operand(struct expr *a, struct expr *b);

/* ---- What can run (flow.c, merlin's flow.rs) ---- */

/* Walks a function's checked body: the ids of the expressions that may be
 * evaluated in *live, whether control can fall off its end, whether a
 * return can be reached. */
int flow_analyze(struct vec *body, struct intmap *live, int *falls_off, int *returns);

/* ---- Unsequenced side effects (sequence.c, merlin's sequence.rs) ---- */

/* A -Wunsequenced warning: where, its message, the expression it is of. */
struct finding {
    struct pos at;
    const char *msg;
    uint64_t of;
};
/* The warnings of a full expression, of a declaration's list: how many, a
 * array of them on the heap in *out. */
uint sequence_check(struct expr *e, struct finding **out);
uint sequence_check_init(struct init *init, struct finding **out);

/* ---- The calling convention (abi.c, merlin's abi.rs) ---- */

/* The bytes of a register, and of an argument word; the argument
 * registers, a0 to a7. */
#define WORD PTR_SIZE
#define ARG_REGS 8
/* How one argument or result travels: a scalar in a register, a small
 * struct as one or two words, a big one as the address of a copy. */
enum { PASS_SCALAR, PASS_WORDS1, PASS_WORDS2, PASS_REF };
int abi_classify(struct type *ty);
uint abi_registers(int pass);
int abi_returns_in_memory(struct type *ret);
uint abi_call_arguments(struct type *ret, struct type **args, uint n, uint *words, int *passes);
uint abi_stack_words(uint total);

/* ---- What a constant condition leaves out (elide.c, merlin's elide.rs) ---- */

/* clang's ContainsLabel, containsBreak and mightAddDeclToScope. */
int contains_label(struct stmt *s, int ignore_cases);
int contains_break(struct stmt *s);
int might_add_decl(struct stmt *s);
/* The statements switch sw on the constant value runs (clang's
 * FindCaseStatementsForValue): 1 and them in *out, or 0 if they cannot be
 * picked out. */
int case_statements(struct stmt *sw, i128 value, struct vec *out);

/* ---- MD5 (md5.c, merlin's md5.rs) ---- */

/* The MD5 digest (RFC 1321) of the n bytes at data, in digest. */
void md5(const uchar *data, uint n, uchar digest[16]);
/* The digest as 32 lowercase hex digits, the first byte first, and a NUL:
 * how clang writes it after `md5 0x`. */
void md5_hex(const uchar digest[16], char out[33]);

/* A file's MD5 checksum, if it has one (has: a name #line gives has
 * none). */
struct checksum {
    uchar has;
    uchar sum[16];
};

/* ---- Debug information (debug.c, merlin's debug.rs) ----
 * What the code generator tells it as it generates (codegen.c), for the
 * line table's directives and the DWARF compile unit it writes at the end. */

struct lines;
struct dvar;
struct dlabel;
struct dscope;

/* LLVM's sys::path::filename (POSIX style, merlin-as's dwarf::file_name)
 * of the n bytes at p: the last component, its length in *len (in p, or
 * "." for a trailing `/`). */
const char *path_file_name(const char *p, uint n, uint *len);
/* Appends the n bytes at s as a string the assembler reads, quoted as
 * LLVM's assembly printer quotes one (merlin's debug::quoted): `"` and `\`
 * escaped, printable ASCII as it is, \b \f \n \r \t by name, any other byte
 * as three octal digits. */
void quote(struct buf *b, const char *s, uint n);

/* -g's state for one translation unit (merlin's Lines::new): the
 * compilation directory (-fdebug-compilation-dir), the preprocessor's files
 * (names[i], lens[i] bytes) and their checksums, by pos.file, and the main
 * file's name for the compile unit and .file 0 if not names[0] (NULL). */
struct lines *debug_new(const char *comp_dir, const char **names, const uint *lens, struct checksum *sums,
                        uint nfiles, const char *main_name);
/* `.file 0 ...`, the first line of the output until debug_renumber puts it
 * where clang has it. */
char *debug_file0(struct lines *l);
/* The labels at the start and the end of a new block's code. */
void debug_block_labels(struct lines *l, const char **begin, const char **end);
/* The .loc that says the next instruction comes from pos; column >= 0
 * overrides the position's (0 for a function's first line). */
char *debug_loc(struct lines *l, struct pos pos, int column, int prologue_end);
/* A parameter or a variable named name, of type ty (spelled spelled),
 * declared at pos, living at symbol sym, or (sym NULL) in the frame at
 * offset from s0. */
struct dvar *debug_var(struct lines *l, const char *name, struct type *ty, struct spelling *spelled, struct pos pos,
                       const char *sym, int offset);
/* A global variable the file defines, at symbol sym: the events since the
 * last function or global are its initializer's. */
void debug_global(struct lines *l, const char *name, const char *sym, struct type *ty, struct spelling *spelled,
                  struct pos pos, int external);
/* A variable declared in the function being generated, as clang's front
 * end describes it there. */
void debug_declared(struct lines *l, struct dvar *v);
/* An explicit cast to ty (spelled spelled) in the code generated. */
void debug_cast(struct lines *l, struct type *ty, struct spelling *spelled);
/* An enumeration constant of the enum `of` spells, in the code generated. */
void debug_enumerator(struct lines *l, struct spelling *of);
/* goto label name, at pos, whose code starts at label sym. */
struct dlabel *debug_label(struct lines *l, const char *name, struct pos pos, const char *sym);
/* String literal label, a char array of len bytes (its NUL included, if it
 * has one), at pos. */
void debug_literal(struct lines *l, const char *label, uint len, struct pos pos);
/* The functions, by name, in the order of their code. */
void debug_code_order(struct lines *l, const char **names, uint n);
/* A function generated, for its DW_TAG_subprogram (merlin's
 * Lines::function's arguments). */
struct debug_function {
    const char *name;
    /* The label just past its code. */
    const char *end;
    /* Where it starts (its name) and its body ends. */
    struct pos start;
    struct pos body_end;
    int external;
    int prototyped;
    struct type *ret;
    struct spelling *ret_spelled;
    /* struct dvar *. */
    struct vec params;
    int variadic;
    /* Its body's variables and blocks. */
    struct dscope *body;
};
void debug_function(struct lines *l, struct debug_function *f);
/* A scope (merlin's Scope): new, with the labels around its code ("" for
 * a function's body); a goto label in it; a variable it declares; and a
 * block in it, once its code is generated. */
struct dscope *debug_scope(const char *begin, const char *end);
void debug_scope_label(struct dscope *s, struct dlabel *label);
void debug_scope_var(struct dscope *s, struct dvar *v);
void debug_scope_block(struct dscope *s, struct dscope *inner);
/* Numbers the files as LLVM does, and makes text from byte `from` on (the
 * code generator's output so far, after what came before it) have its .loc
 * directives renumbered and the .file directives where clang writes them.
 * Call it once the code is generated and its order known. */
void debug_renumber(struct lines *l, struct buf *text, uint from);
/* Appends the debug sections (.debug_abbrev, .debug_info, .debug_str and
 * the others), and the label at the start of .debug_line that the compile
 * unit points at; nothing if the file has no functions. */
void debug_sections(struct lines *l, struct buf *out);

/* ---- Code generation (codegen.c, merlin's codegen.rs) ---- */

/* The assembly of a checked program (merlin's generate_with): with -g
 * (debug not NULL) its debug information too. 1 and the text appended to
 * out (written there as it is put together, no copy made), or 0 and the
 * error. */
int codegen_generate(struct program *prog, struct lines *debug, struct buf *out, struct diag **error);

/* --dump-ast's text. */
void ast_dump(struct program *p, struct buf *out);
/* A string literal as C writes it. */
void ast_quote(struct buf *b, uchar *bytes, uint len);

#endif
