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
 *   tables.c    elf32's (crates/xtask/src/c_tables.rs)
 *
 * The 64-bit arithmetic merlin-ld does with Rust's i64 and u64 (addresses
 * up to 4 GiB exactly, relocation ranges) is lib/i64.c's. Not here: the
 * link map (merlin-ld's --Map). */
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
#define SHT_NULL 0
#define SHT_PROGBITS 1
#define SHT_SYMTAB 2
#define SHT_STRTAB 3
#define SHT_RELA 4
#define SHT_NOBITS 8
#define SHT_REL 9
#define SHT_GROUP 17
#define SHT_SYMTAB_SHNDX 18

#define SHF_WRITE 0x1
#define SHF_ALLOC 0x2
#define SHF_EXECINSTR 0x4
#define SHF_MERGE 0x10
#define SHF_STRINGS 0x20
#define SHF_GROUP 0x200
#define SHF_TLS 0x400

#define SHN_UNDEF 0
#define SHN_LORESERVE 0xff00
#define SHN_ABS 0xfff1
#define SHN_COMMON 0xfff2

#define STB_LOCAL 0
#define STB_GLOBAL 1
#define STB_WEAK 2

#define STT_NOTYPE 0
#define STT_SECTION 3
#define STT_FILE 4
#define STT_TLS 6

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
/* Parses `inputs` and the members of `library` they need (merlin-ld's
 * with_needed), which follow them in library order. Returns the objects
 * and their count in *n, or NULL after adding errors. */
struct object *parse_with_needed(struct input *inputs, uint ninputs, struct input *library, uint nlibrary,
                                 uint *n);

/* ---- Linker scripts (script.c) ---- */
enum { X_NUM, X_DOT, X_SYMBOL, X_ALIGN, X_ORIGIN, X_LENGTH, X_ADD, X_SUB };
struct expr {
    int kind;
    i64 num;
    char *name; /* X_SYMBOL; X_ORIGIN, X_LENGTH: the region */
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

struct script {
    char *entry;
    int has_memory;
    char *region_name;
    uint origin;
    uint length;
    struct command *commands;
    uint ncommands;
};

#define DISCARD_NAME "/DISCARD/"
extern const char DEFAULT_SCRIPT[];
/* Parses a script. 0 with *line and *msg set on an error. */
int script_parse(const char *src, struct script *out, uint *line, char **msg);
/* A number: decimal or 0x hex, with an optional K or M. */
int parse_number(const char *w, i64 *out);
int glob_match(const char *pattern, const char *name);
int expr_uses_dot(struct expr *e);

/* ---- The layout (layout.c) ---- */
struct input_ref {
    uint object;
    uint section;
    uint addr;
};

struct output_section {
    char *name;
    uint addr;
    uint size;
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
};

/* merlin-ld's layout::build: 0 after adding errors. */
int layout_build(struct object *objects, uint nobjects, const char *script_name, const char *script_text,
                 const char *entry_opt, struct layout *l);
/* Values: 1 with *out set, or 0 (none). */
int object_symbol(struct layout *l, uint obj, uint sym, uint *out);
int global_value(struct layout *l, const char *name, uint *out);
int in_dropped_section(struct layout *l, uint obj, uint sym);
int is_undefined_weak(struct layout *l, uint obj, uint sym);
/* S for a relocation in `obj` against `sym`: 1, or 0 with *err. */
int reloc_symbol(struct layout *l, uint obj, uint sym, uint *out, char **err);

/* ---- Relocations (reloc.c, relocate.c) ---- */
enum { RE_OK, RE_UNSUPPORTED, RE_NEEDS_RELAXATION, RE_OUT_OF_RANGE, RE_MISALIGNED, RE_TRUNCATED, RE_MISSING_HI20,
       RE_LO12_ADDEND };

/* What a failed relocation says: the value and its bounds (OUT_OF_RANGE),
 * the value and the alignment, in min (MISALIGNED), the bytes needed and
 * left (TRUNCATED). */
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
    struct uintmap by_place;
    struct hi_site *sites;
    uint nsites;
    uint cap;
};

/* Patches `bytes` (`have` of them, from the place to the end of the input
 * section) for one relocation. Returns RE_OK or what is wrong (nothing is
 * written then). */
int reloc_apply(uint type, uint place, uint sym, int addend, uchar *bytes, uint have, struct reloc_context *ctx,
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

#endif
