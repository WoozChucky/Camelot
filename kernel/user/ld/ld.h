/* ld: the linker that runs on Camelot (/bin/ld), merlin-ld rewritten in the
 * C subset merlin compiles (issue #136, the second step of self-hosting,
 * #23). For the same objects and script it writes the same executable as
 * the host's merlin-ld (crates/merlin-ld), byte for byte: the host tool is
 * the reference, and crates/xtask/tests/selfhost.rs compares the two. Each
 * file here mirrors a module of merlin-ld, whose comments explain the rules
 * in more depth:
 *
 *   main.c      the command line and response files (main.rs, lib.rs)
 *   util.c      memory, buffers, messages, maps
 *   object.c    reading relocatable objects (object.rs)
 *   library.c   the library members a program needs (library.rs)
 *   script.c    the linker script subset (script.rs)
 *   layout.c    symbols, sections and addresses (layout.rs)
 *   reloc.c     one relocation's formula and field (reloc.rs)
 *   relocate.c  the sections' contents, relocated (relocate.rs)
 *   write.c     the ELF executable (write.rs)
 *   tables.h    the relocation types' numbers and names, generated from
 *   tables.c    the elf crate's (crates/xtask/src/c_tables.rs)
 *
 * The 64-bit arithmetic merlin-ld does with Rust's i64 and u64 (addresses
 * up to 4 GiB exactly, load addresses, relocation ranges, R_RISCV_64) is
 * lib/i64.c's. Not here: the link map (merlin-ld's --Map), and RV64
 * (ELF64 objects and executables, #157). */
#ifndef LD_H
#define LD_H

#include "../user.h"
#include "tables.h"

/* ---- Memory, buffers, messages (util.c) ---- */
void die(const char *msg);
void *xmalloc(uint n);
void *xcalloc(uint n);
char *xstrdup(const char *s);
char *xstrndup(const char *s, uint n);
/* A growable array: grow(ptr, &cap, count + 1, size) makes room. */
void *grow(void *p, uint *cap, uint need, uint size);
/* A formatted message, as long as it needs to be (malloc'd): %s %d %u %x
 * %c %%, and a width with 0 for %x (%08x). 64-bit numbers are passed as
 * strings (i64_dec, i64_hex). */
char *fmt(const char *f, ...);

struct buf {
    uchar *data;
    uint len;
    uint cap;
};
void buf_put(struct buf *b, const void *p, uint n);
void buf_byte(struct buf *b, uint v);
void buf_u16(struct buf *b, uint v);
void buf_u32(struct buf *b, uint v);
/* Pads with zeros to `len` bytes (if shorter). */
void buf_pad(struct buf *b, uint len);

/* The link's errors and warnings, in the order they come, as merlin-ld
 * collects them (each already says which file it concerns). */
struct messages {
    char **msgs;
    uint n;
    uint cap;
};
extern struct messages errors;
extern struct messages warnings;
void add_message(struct messages *m, char *msg);
void error(char *msg);

/* A string -> int map (open addressing); keys must stay alive. */
struct strmap {
    char **keys;
    int *values;
    uint cap;
    uint count;
};
int map_get(struct strmap *m, const char *key, int *value);
void map_put(struct strmap *m, char *key, int value);

/* A uint -> int map. */
struct uintmap {
    uint *keys;
    int *values;
    uchar *used;
    uint cap;
    uint count;
};
int umap_get(struct uintmap *m, uint key, int *value);
void umap_put(struct uintmap *m, uint key, int value);

/* ---- Objects (object.c) ---- */
/* The ELF constants (SHT_*, SHF_*, SHN_*, STB_*, STT_*, PT_*, PF_*) are
 * tables.h's, generated from the elf crate's. */

/* One Elf32_Rela. */
struct rela {
    uint offset;
    uint type;
    uint sym;
    int addend;
};

/* Where the layout put an input section. */
enum { NOT_PLACED, PLACED, PLACED_DEBUG };

struct section {
    char *name;
    uint type;
    uint flags;
    uchar *data; /* the file's bytes */
    uint datalen; /* how many: size, or 0 for NOBITS and the null section */
    uint size;
    uint align;
    uint entsize;
    struct rela *relocs;
    uint nrelocs;
    uint caprelocs;
    /* The layout's (layout.c): a script rule took it; where it is (`addr`:
     * its address, or its offset in its debug section); which output
     * section holds it (-1: none); whether /DISCARD/ matched it. */
    int assigned;
    int placed;
    uint addr;
    int out;
    int discarded;
};

struct symbol {
    char *name;
    uint value;
    uint size;
    uint bind;
    uint type;
    uint shndx;
};

struct object {
    char *name;
    struct section *sections;
    uint nsections;
    struct symbol *symbols;
    uint nsymbols;
};

/* Parses a relocatable object (its sections' data points into `file`,
 * which must stay alive). 0 with the message in *err (without the file's
 * name) if it is not one merlin-ld takes. */
int object_parse(const char *name, uchar *file, uint len, struct object *out, char **err);
/* How a symbol is named in messages: a section symbol by its section. */
const char *symbol_display_name(struct object *o, uint index);
int sym_is_local(struct symbol *s);
int sym_is_weak(struct symbol *s);
int sym_is_undefined(struct symbol *s);

/* An input file: its name (for messages) and contents. */
struct input {
    char *name;
    uchar *data;
    uint len;
};

/* ---- Libraries (library.c) ---- */
/* Parses the n files of a command line, in its order, lazy[i] saying which
 * are library members, compares their classes and picks the members the
 * others need (merlin-ld's layout::read, check_classes and library::pulled):
 * the files that are not members, in order, then the members needed, in
 * library order. `format` is the class the script's OUTPUT_FORMAT names,
 * which every file must have. Returns the objects and their count in *count,
 * or NULL after adding errors; NULL too when errors were added before (the
 * script's), once every file is parsed. */
struct object *parse_with_needed(struct input *inputs, int *lazy, uint n, int format, uint *count);

/* ---- Linker scripts (script.c) ---- */
enum { X_NUM, X_DOT, X_SYMBOL, X_ALIGN, X_ORIGIN, X_LENGTH, X_ADDR, X_ADD, X_SUB };
struct expr {
    int kind;
    i64 num;
    char *name; /* X_SYMBOL; X_ORIGIN, X_LENGTH: the region; X_ADDR: the output section */
    struct expr *a;
    struct expr *b;
};

/* `. = e`, `. += e` or `sym = e` (symbol NULL: the location counter). */
struct assign {
    char *symbol;
    int add;
    struct expr *e;
    uint line;
};

/* An item of an output section: `*(patterns)` or an assignment. */
struct item {
    int is_assign;
    char **patterns;
    uint npatterns;
    struct assign assign;
};

struct output_desc {
    char *name;
    int noload;
    struct expr *at;    /* AT(lma), the load address; NULL: none */
    struct expr *align; /* NULL: none */
    struct item *items;
    uint nitems;
    char *region; /* NULL: none */
    uint line;
};

struct command {
    int is_output;
    struct assign assign;
    struct output_desc out;
};

/* The classes OUTPUT_FORMAT names: elf32-littleriscv, elf64-littleriscv. */
enum { FORMAT_NONE, FORMAT_ELF32, FORMAT_ELF64 };

struct script {
    char *entry;
    int format; /* OUTPUT_FORMAT's first argument; FORMAT_NONE: none */
    int has_memory;
    char *region_name;
    uint origin;
    uint length;
    struct command *commands;
    uint ncommands;
};

#define DISCARD_NAME "/DISCARD/"
extern const char DEFAULT_SCRIPT[];
/* Parses a script, for ELF32 (its exact values and 4 GiB limits) or, when
 * elf64 is set, for ELF64 (64-bit values, wrapping), as merlin-ld reads it
 * first, for the errors ld.lld gives whatever the class. 0 with *line and
 * *msg set on an error. */
int script_parse(const char *src, struct script *out, uint *line, char **msg, int elf64);
/* "elf32-littleriscv" or "elf64-littleriscv". */
const char *format_name(int format);
/* A number: decimal or 0x hex, with an optional K or M. */
int parse_number(const char *w, i64 *out);
int glob_match(const char *pattern, const char *name);
int expr_uses_dot(struct expr *e);
/* Whether the value depends on a symbol (ld.lld keeps an empty section
 * whose AT or ALIGN does). */
int expr_uses_symbols(struct expr *e);
/* Whether some expression of the script names output section `name` with
 * ADDR(...) (ld.lld keeps such a section even if it is empty). */
int script_names_addr(struct script *s, const char *name);

/* ---- The layout (layout.c) ---- */
struct input_ref {
    uint object;
    uint section;
    uint addr;
};

struct output_section {
    char *name;
    uint addr;
    /* 1 for an empty section at exactly 4 GiB (addr is then 0), the one
     * address past 32 bits a section may have on ELF32: merlin-ld keeps it
     * 64-bit, and so it is above every other section, not below. */
    uint addr_hi;
    uint size;
    /* The load address (LMA, the segment's p_paddr): addr unless the script
     * says otherwise with AT. 64 bits, as merlin-ld's u64: a load range past
     * 4 GiB is an error, and the section is still placed. */
    i64 lma;
    int at; /* the section has an AT of its own: it starts a segment */
    uint align;
    uint flags;
    int nobits;
    struct input_ref *inputs;
    uint ninputs;
    uint capinputs;
};

/* A debug section of the executable: not loaded; an input's addr is its
 * offset in it. */
struct debug_section {
    char *name;
    uint size;
    uint align;
    uint flags;
    uint entsize;
    struct input_ref *inputs;
    uint ninputs;
    uint capinputs;
};

/* Where a global is defined: symbol `sym` of object `obj`, or (obj -1) by
 * the script, as `value`. */
struct global {
    char *name;
    int obj;
    uint sym;
    uint value;
};

struct layout {
    struct object *objects;
    uint nobjects;
    struct output_section *sections;
    uint nsections;
    struct debug_section *debug;
    uint ndebug;
    struct global *globals;
    uint nglobals;
    struct strmap global_index;
    uint entry;
    /* The script has a MEMORY region, which every output section is in:
     * such a section never shares a segment with the headers (write.c). */
    int memory;
    /* ld.lld would write a .riscv.attributes section, and so a
     * PT_RISCV_ATTRIBUTES program header, which write.c leaves room for. */
    int riscv_attributes;
};

/* merlin-ld's layout::build: 0 after adding errors. */
int layout_build(struct object *objects, uint nobjects, const char *script_name, const char *script_text,
                 const char *entry_opt, struct layout *l);
/* Values: 1 with *out set, or 0 (none). */
int object_symbol(struct layout *l, uint obj, uint sym, uint *out);
int global_value(struct layout *l, const char *name, uint *out);
int in_dropped_section(struct layout *l, uint obj, uint sym);
int is_undefined_weak(struct layout *l, uint obj, uint sym);
/* ld.lld makes an output section of this name and these flags RELRO. */
int is_relro(const char *name, uint flags);
/* S for a relocation in `obj` against `sym`: 1, or 0 with *err. */
int reloc_symbol(struct layout *l, uint obj, uint sym, uint *out, char **err);

/* ---- Relocations (reloc.c, relocate.c) ---- */
enum { RE_OK, RE_UNSUPPORTED, RE_ALIGN_PADDING, RE_OUT_OF_RANGE, RE_MISALIGNED, RE_RANGE_AND_ALIGN, RE_TRUNCATED,
       RE_MISSING_HI20, RE_LO12_ADDEND };

/* What a failed relocation says: the value and its bounds (OUT_OF_RANGE,
 * and RANGE_AND_ALIGN, an odd offset out of range: two errors, misaligned
 * by 2 too), the value and the alignment, in min (MISALIGNED), the bytes
 * needed and left (TRUNCATED). */
struct reloc_error {
    i64 value;
    i64 min;
    i64 max;
    uint need;
    uint have;
};

/* The PCREL_HI20 sites by place, for the PCREL_LO12s: index into his. */
struct hi_site {
    uint sym;
    int addend;
};
struct reloc_context {
    /* The relocations patch a section that is not loaded (debug
     * information), where R_RISCV_64 is sign-extended from 32 bits. */
    int debug;
    struct uintmap by_place;
    struct hi_site *sites;
    uint nsites;
    uint cap;
};

/* Patches `bytes` (`have` of them, from the place to the end of the input
 * section) for one relocation. S, `sym`, is 64 bits, as merlin-ld's u64: an
 * address, or -A for debug information of discarded code. Returns RE_OK or
 * what is wrong (nothing is written then). */
int reloc_apply(uint type, uint place, i64 sym, int addend, uchar *bytes, uint have, struct reloc_context *ctx,
                struct reloc_error *e);
/* A relocation type's psABI name (or "relocation type N"). */
const char *reloc_type_name(uint type);

/* The output sections' contents (NULL for NOBITS) and the debug sections',
 * relocated: 0 after adding errors. */
int relocate(struct layout *l, uchar ***sections, uchar ***debug);

/* ---- The executable (write.c) ---- */
/* The ELF file, without the debug sections if `strip` (the same bytes as a
 * link without them: merlin-ld's strip_debug). */
void write_elf(struct layout *l, uchar **sections, uchar **debug, int strip, struct buf *out);
/* merlin-ld's write::check: adds the errors the program headers would have. */
void write_check(struct layout *l);

#endif
