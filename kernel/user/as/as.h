/* as: the assembler that runs on Camelot (/bin/as), merlin-as rewritten in
 * the C subset merlin compiles (issue #135, step 1 of self-hosting, #23).
 * For the same input it writes the same ELF object as the host's merlin-as
 * (crates/merlin-as), byte for byte: the host tool is the reference, and
 * crates/xtask/tests compares the two. Each file here mirrors a module of
 * merlin-as, whose comments explain the rules in more depth:
 *
 *   main.c    the command line, reading the file, the passes (main.rs, lib.rs)
 *   util.c    memory, byte buffers, errors (64-bit numbers: lib/i64.c)
 *   lexer.c   text to tokens, one statement at a time (lexer.rs)
 *   parser.c  tokens to statements (parser.rs)
 *   encode.c  one instruction to machine words (encode.rs)
 *   asm.c     sections, symbols, directives, local labels (asm.rs)
 *   section.c a section's ELF type, flags and entry size (section.rs)
 *   dwarf.c   the DWARF line table of `.file` and `.loc` (dwarf.rs)
 *   elf.c     the relocatable object file (elf.rs)
 *   tables.h  the encodings, names and numbers, generated from the host
 *   tables.c  tools' own tables (crates/xtask/src/c_tables.rs): the header
 *             declares the tables and defines the numbers, tables.c holds
 *             the tables, once for the program
 *
 * Unlike merlin-as, which holds every token of the file at once, this one
 * reads a statement, assembles it and forgets it: a machine has 4 MiB of RAM
 * by default, and the kernel's biggest assembly file has 200 KB of text. */
#ifndef AS_H
#define AS_H

#include "../user.h"

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
/* A formatted message in tmp memory, as long as it needs to be (merlin-as's
 * messages hold whole names): %s %d %u %x %X (upper case) %c %%, and a
 * width with 0 for %x (%08x). */
char *fmt(const char *f, ...);

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

/* A string -> int map (open addressing). */
struct strmap {
    char **keys;
    int *values;
    uint cap;
    uint count;
};
int map_get(struct strmap *m, const char *key, int *value);
/* Sets key to value; `key` must stay alive (perm memory). */
void map_put(struct strmap *m, char *key, int value);

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
/* After parse_statement failed: whether its error belongs to the next
 * statement, which merlin-as then skips (a `%` that ends a statement: the
 * modifier's name it reads is the statement's end). *line is the line to
 * report it on if there is no next statement. */
int parse_takes_next(uint *line);
/* Reports that error, on `line`. */
void parse_report_next(uint line);
const char *modifier_name(int m);
int reg_parse(const char *name);

/* ---- Relocations and the encoder (encode.c) ---- */
/* One instruction word, with the relocation that fills in its immediate,
 * if any: against `name` + `addend`, or (`auipc` >= 0) against the label on
 * word `auipc` of the same instruction (the low half of a pc-relative pair).
 * `kind` is the relocation's psABI type, R_RISCV_* from tables.h, as every
 * fixup's and relocation's kind is. */
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

/* ---- Sections' ELF attributes (section.c) ---- */
/* A section's sh_type, sh_flags and sh_entsize (SHT_* and SHF_* are in
 * tables.h). */
struct sec_attrs {
    uint ty;
    uint flags;
    uint entsize;
};
/* The type and flags LLVM gives a section from its name alone. */
void section_name_defaults(const char *name, uint *ty, uint *flags);
/* Whether `name` is one of the sections LLVM makes before the first line,
 * and if so, its attributes in *out. */
int section_predefined(const char *name, struct sec_attrs *out);
/* The attributes of a section named without a `.section` that says more:
 * LLVM's own for a predefined one, else its name's defaults. */
void section_by_name(const char *name, struct sec_attrs *out);
/* The `len` bytes at s as a number, read as LLVM's getAsInteger(0) reads
 * a whole string (0x, 0b, 0o or 0 for octal, else decimal), in *out.
 * Returns 0 if they are none or the number passes 32 bits. */
int section_number(const uchar *s, uint len, uint *out);
/* The flags of a `.section` flags string, a number or letters, in *out.
 * Returns 0 at the first byte that is no flag letter (G and o too), which
 * goes in *bad. */
int section_flags(const uchar *s, uint len, uint *out, uint *bad);
/* The type a `.section` type name or number stands for, in *out; 0 if none. */
int section_type(const uchar *s, uint len, uint *out);

/* ---- The object (asm.c) ---- */
/* SYM_MAPPING: a mapping symbol ($x<isa>, $d), which nothing refers to. */
enum { SYM_NOTYPE, SYM_FUNC, SYM_OBJECT, SYM_SECTION, SYM_MAPPING };

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
    struct sec_attrs attrs;
    struct buf data;
    uint size;
    uint align;
    struct reloc *relocs;
    uint nrelocs;
    uint cap;
    /* What it holds at its end, for its mapping symbols: 0 (nothing yet),
     * MAP_CODE or MAP_DATA (asm.c's mapping_symbol). */
    int mapping;
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

/* ---- Line tables (dwarf.c) ---- */
/* `.loc` flags. */
#define LOC_IS_STMT 1
#define LOC_BASIC_BLOCK 2
#define LOC_PROLOGUE_END 4
#define LOC_EPILOGUE_BEGIN 8

/* A `.file`: a directory ("" for the compilation directory), a name, and
 * maybe an MD5 checksum. */
struct dfile {
    char *dir;
    char *name;
    int has_md5;
    uchar md5[16];
};

/* Where a row says its instruction comes from: a `.loc`. */
struct loc {
    uint file;
    uint line;
    uint column;
    uint flags;
    uint isa;
    uint discriminator;
};

/* A row of the line table: offset `offset` of section `section`. */
struct row {
    int section;
    uint offset;
    struct loc loc;
};

/* A 4-byte field of `.debug_line` (at `at`, from the section's start) for
 * the linker to fill in: offset `off` of `.debug_line_str` (section -1) or
 * of a code section. */
struct dfixup {
    uint at;
    int section;
    uint off;
};

/* An encoded line table: `.debug_line`'s bytes, the strings it adds to
 * `.debug_line_str`, and the fields to relocate. */
struct line_table {
    struct buf line;
    struct buf line_str;
    struct dfixup *fixups;
    uint nfixups;
    uint cap;
};

/* Encodes the rows (file 0 is `root`, files 1.. are `files`) as LLVM does,
 * one sequence per section in the order their first rows came, each ending
 * at the section's size. `base` is where the table goes in `.debug_line`
 * and `str_base` where its strings go in `.debug_line_str`. */
void dwarf_encode(struct dfile *root, struct dfile *files, uint nfiles, struct row *rows, uint nrows,
                  struct section *sections, uint nsections, uint base, uint str_base, struct line_table *t);

/* ---- The ELF file (elf.c) ---- */
void elf_write(struct object *obj, struct buf *out);
/* The contents of `.riscv.attributes` (merlin-as's elf::riscv_attributes):
 * Tag_RISCV_arch = RISCV_ARCH. */
void riscv_attributes(struct buf *out);

#endif
