/* as: the assembler that runs on Camelot (/bin/as), merlin-as rewritten in
 * the C subset merlin compiles (issue #135, step 1 of self-hosting, #23).
 * For the same input it writes the same ELF object as the host's merlin-as
 * (crates/merlin-as), byte for byte: the host tool is the reference, and
 * crates/xtask/tests compares the two. Each file here mirrors a module of
 * merlin-as, whose comments explain the rules in more depth:
 *
 *   main.c    the command line, reading the file, the passes (main.rs, lib.rs)
 *   util.c    memory, byte buffers, 64-bit numbers, errors
 *   lexer.c   text to tokens, one statement at a time (lexer.rs)
 *   parser.c  tokens to statements (parser.rs)
 *   encode.c  one instruction to machine words (encode.rs)
 *   asm.c     sections, symbols, directives, local labels (asm.rs)
 *   elf.c     the relocatable object file (elf.rs)
 *   tables.h  the encodings, names and numbers, generated from the host
 *             tools' own tables (crates/xtask/src/as_tables.rs)
 *
 * Unlike merlin-as, which holds every token of the file at once, this one
 * reads a statement, assembles it and forgets it: a machine has 4 MiB of RAM
 * by default, and the kernel's biggest assembly file has 200 KB of text.
 *
 * Not yet here (the host's merlin-as has them): `.file N` and `.loc`, the
 * DWARF line tables (step 2 of #135). */
#ifndef AS_H
#define AS_H

#include "../user.h"

/* ---- 64-bit integers ----
 * merlin-as computes with i64 (expressions, range checks), and merlin has no
 * long long, so a value is two words, in two's complement. */
typedef struct {
    uint lo;
    uint hi;
} i64;

i64 i64_of(int v);
i64 i64_u(uint v);
i64 i64_add(i64 a, i64 b);
i64 i64_sub(i64 a, i64 b);
i64 i64_neg(i64 a);
i64 i64_not(i64 a);
i64 i64_mul(i64 a, i64 b);
/* a / b and a % b truncated toward zero, as Rust's checked_div/checked_rem:
 * 0 when there is no result (b is 0, or i64::MIN / -1). */
int i64_div(i64 a, i64 b, i64 *q);
int i64_rem(i64 a, i64 b, i64 *r);
/* Shifts by the low 6 bits of n (wrapping_shl, wrapping_shr: arithmetic). */
i64 i64_shl(i64 a, uint n);
i64 i64_sar(i64 a, uint n);
i64 i64_and(i64 a, i64 b);
i64 i64_or(i64 a, i64 b);
i64 i64_xor(i64 a, i64 b);
/* -1, 0 or 1, comparing as signed. */
int i64_cmp(i64 a, i64 b);
int i64_eq(i64 a, i64 b);
int i64_is_neg(i64 a);
/* Whether lo <= v < hi (signed). */
int i64_in(i64 v, i64 lo, i64 hi);
/* Decimal (signed), and 0x-prefixed hexadecimal of the unsigned value, as
 * Rust's {} and {:#x}: each call returns one of a few rotating buffers. */
const char *i64_dec(i64 v);
const char *i64_hex(i64 v);

/* ---- Memory ----
 * Two bump allocators: perm_alloc's memory lasts the whole run (symbols,
 * section contents), tmp_alloc's until tmp_reset(), called after each
 * statement (its tokens and syntax tree). Out of memory is fatal. */
void *perm_alloc(uint n);
void *tmp_alloc(uint n);
void tmp_reset(void);
char *perm_strdup(const char *s);
char *tmp_strndup(const char *s, uint n);
/* A growable array: grow(ptr, &cap, count + 1, size) makes room. */
void *grow(void *p, uint *cap, uint need, uint size);
void die(const char *msg);

/* A byte buffer. */
struct buf {
    uchar *data;
    uint len;
    uint cap;
};
void buf_put(struct buf *b, const void *p, uint n);
void buf_byte(struct buf *b, uint v);
void buf_u16(struct buf *b, uint v);
void buf_u32(struct buf *b, uint v);
void buf_zeros(struct buf *b, uint n);
void buf_align(struct buf *b, uint a);

/* ---- Errors ----
 * An error, by source line (0: not tied to a line), with its message. */
struct error {
    uint line;
    char *msg;
    int phase;
};
/* Which part of the assembler the errors come from: merlin-as stops at a
 * lexer error, then at syntax errors, before it assembles, so only the
 * first phase's errors are shown. */
enum { PHASE_LEX, PHASE_PARSE, PHASE_ASM };
void error_phase(int phase);
/* Collects an error of the current phase (the message is copied). */
void error_at(uint line, const char *msg);
/* How many errors of `phase` there are. */
uint error_count(int phase);
/* Sorts the errors of `phase` by line, stable (Rust's sort_by_key), and
 * prints them as `FILE:LINE: error: MESSAGE` to stderr. */
void errors_print(const char *file, int phase);

/* ---- Tokens (lexer.c) ---- */
enum {
    T_IDENT,    /* names, mnemonics, directives, registers: text */
    T_INT,      /* num */
    T_WIDE,     /* a hex number wider than 64 bits: wide[16], big-endian */
    T_STR,      /* text (len bytes, not NUL-terminated inside) */
    T_LOCALREF, /* 1b / 1f: num.lo is the label number, fwd */
    T_PUNCT,    /* punct: one of , ( ) : + - * / % ~ & | ^ = */
    T_SHIFT,    /* << (fwd 1) or >> (fwd 0) */
    T_NEWLINE
};

struct token {
    int kind;
    uint line;
    i64 num;
    char *text;
    uint len;
    int punct;
    int fwd;
    uchar wide[16];
};

/* Reads the source from `src` (len bytes) a statement at a time. */
struct lexer {
    const char *src;
    uint len;
    uint pos;
    uint line;
    int done;
};
void lexer_init(struct lexer *lx, const char *src, uint len);
/* The tokens of the next statement, ended by a T_NEWLINE (a newline, a `;`
 * or the end of the text), in tmp memory. Returns their count; 0 at the end;
 * -1 on an error, which it reports (merlin-as stops at a lexer error). */
int lex_statement(struct lexer *lx, struct token **out);

/* ---- Syntax (parser.c) ---- */
enum { E_NUM, E_SYM, E_NEG, E_NOT, E_BIN, E_MOD };
enum { B_ADD, B_SUB, B_MUL, B_DIV, B_REM, B_SHL, B_SHR, B_AND, B_OR, B_XOR };
enum { M_HI, M_LO, M_PCREL_HI, M_PCREL_LO };

struct expr {
    int kind;
    i64 num;
    char *sym;
    int op; /* E_BIN: B_...; E_MOD: M_... */
    struct expr *a;
    struct expr *b;
};

enum { OPD_REG, OPD_EXPR, OPD_MEM };
struct operand {
    int kind;
    int reg;          /* OPD_REG: the register; OPD_MEM: the base */
    struct expr *e;   /* OPD_EXPR; OPD_MEM: the offset */
    int int_token;    /* OPD_MEM: the offset was one integer token, or none */
};

enum { DA_EXPR, DA_STR, DA_WIDE };
struct dirarg {
    int kind;
    struct expr *e;
    uchar *str;
    uint len;
    uchar wide[16];
};

enum { S_LABEL, S_DIRECTIVE, S_INSN };
struct stmt {
    int kind;
    uint line;
    char *name;
    struct operand *ops;
    uint nops;
    struct dirarg *args;
    uint nargs;
};

/* The statements of one lexed statement's tokens (labels, then at most one
 * directive or instruction), in tmp memory. Returns their count, or -1 after
 * reporting a syntax error. */
int parse_statement(struct token *toks, int n, struct stmt **out);
const char *modifier_name(int m);
int reg_parse(const char *name);

/* ---- Relocations and the encoder (encode.c) ---- */
enum {
    R_ABS32,
    R_BRANCH,
    R_JAL,
    R_CALL_PLT,
    R_PCREL_HI20,
    R_PCREL_LO12_I,
    R_PCREL_LO12_S,
    R_HI20,
    R_LO12_I,
    R_LO12_S
};
uint reloc_elf_type(int kind);

/* One instruction word, with the relocation that fills in its immediate,
 * if any: against `name` + `addend`, or (`auipc` >= 0) against the label on
 * word `auipc` of the same instruction (the low half of a pc-relative pair). */
struct emit {
    uint word;
    int has_fixup;
    int kind;
    char *name;
    i64 addend;
    int auipc;
};

/* Encodes one instruction into words (at most 2). Returns how many, or 0
 * with the message in *err. */
int encode(const char *m, struct operand *ops, uint nops, struct emit *out, char **err);

/* What the encoder asks the assembler (asm.c), merlin-as's Ctx. */
int ctx_constant(struct expr *e, i64 *v);
int ctx_symbol(struct expr *e, char **name, i64 *addend);
int ctx_undefined(const char *name);
int ctx_csr(struct expr *e, uint *addr);
int csr_parse(const char *name, uint *addr);

/* The immediate scramblers, which asm.c also uses to patch resolved labels. */
uint i_imm(int imm);
uint s_imm(int imm);
uint b_imm(int imm);
uint u_imm(uint imm20);
uint j_imm(int imm);
/* value = (hi << 12) + lo, lo the sign-extended low 12 bits. */
void hi_lo(int value, uint *hi, int *lo);

/* ---- The object (asm.c) ---- */
enum { SEC_TEXT, SEC_DATA, SEC_RODATA, SEC_BSS, SEC_DEBUG };
enum { SYM_NOTYPE, SYM_FUNC, SYM_OBJECT, SYM_SECTION };

struct reloc {
    uint offset;
    int kind;
    char *symbol;
    i64 addend;
    uint line;
    /* After asm_finish: the index in the object's symbols of the first one
     * named `symbol`. */
    uint sym;
};

struct section {
    char *name;
    int kind;
    struct buf data;
    uint size;
    uint align;
    struct reloc *relocs;
    uint nrelocs;
    uint cap;
};

struct symbol {
    char *name;
    int section; /* -1: undefined or a constant */
    uint value;
    int global;
    int defined;
    int has_constant;
    i64 constant;
    int ty;
    uint size;
    int keep; /* written to the symbol table (see asm_finish) */
};

struct object {
    struct section *sections;
    uint nsections;
    struct symbol *symbols;
    uint nsymbols;
};

void asm_init(void);
void asm_statement(struct stmt *s);
/* Ends the file: checks and resolves what needs every label (merlin-as's
 * finish). Returns the object, or 0 if there were errors. */
struct object *asm_finish(void);

/* ---- The ELF file (elf.c) ---- */
void elf_write(struct object *obj, struct buf *out);

#endif
