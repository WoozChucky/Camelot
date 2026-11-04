/* as: the assembler driver (merlin-as's asm.rs, which explains the rules at
 * length): sections and symbols, the directives, instructions placed and
 * their fixups turned into relocations, and at the end the pc-relative
 * references to local labels of the same section resolved on the spot, as
 * clang does with -mno-relax.
 *
 * Symbols are kept in the order they are first named (the symbol table's
 * order, after locals and globals are split): every place that names one
 * does it where merlin-as does. `.L` labels and section symbols go into the
 * symbol table only if a relocation still refers to them. Mapping symbols
 * ($x<isa>, $d) mark where code and data start, as clang marks them
 * (mapping_symbol). */
#include "as.h"
#include "tables.h"

/* ---- The assembler's state ---- */

static struct section *sections;
static uint nsections, capsections;
static int current;
static struct symbol *symbols;
static uint nsymbols, capsymbols;
static struct strmap by_name;
/* How many times each numeric label (`1:`) has been defined. */
static uint *local_numbers;
static uint *local_counts;
static uint nlocal, caplocal;
static uint hidden_labels;
static uint dot_labels;
/* `%pcrel_lo(label)` references, checked once every label is known. */
struct pcrel_lo {
    int section;
    char *name;
    uint line;
};
static struct pcrel_lo *pcrel_los;
static uint npcrel_lo, cappcrel_lo;
/* Names given to `.local`. */
static struct strmap locals;
static uint line;
/* Line information: `.file 0` (the compilation directory and the main
 * file) and the other `.file`s by number (files[n].name NULL: no .file n). */
static int have_file0;
static struct dfile file0;
static struct dfile *files;
static uint nfiles, capfiles;
/* The last `.loc`, until an instruction (or data) takes it. */
static int have_loc;
static struct loc pending_loc;
/* `.loc`'s is_stmt, which carries over from one to the next. */
static int is_stmt = 1;
/* The line table's rows, and the source line of the first `.loc` (for an
 * error about the table). */
static struct row *rows;
static uint nrows, caprows;
static uint first_loc_line;

#define DOT "."
#define DOT_LABEL ".Ldot$"

static void error(const char *msg) {
    error_at(line, msg);
}

static int starts_with(const char *s, const char *prefix) {
    return strncmp(s, prefix, strlen(prefix)) == 0;
}

/* How a label is named in errors: `.L1$0` -> `1`, `.Ldot$0` -> `.`,
 * `.Lfoo` -> `foo`, other names as they are. */
static const char *shown_name(const char *name) {
    if (starts_with(name, DOT_LABEL)) {
        return DOT;
    }
    if (starts_with(name, ".L")) {
        const char *rest = name + 2;
        const char *dollar = strchr(rest, '$');
        if (dollar == NULL) {
            return rest;
        }
        return tmp_strndup(rest, (uint)(dollar - rest));
    }
    return name;
}

/* ---- Symbols ---- */

static uint local_count(uint n) {
    for (uint i = 0; i < nlocal; i++) {
        if (local_numbers[i] == n) {
            return local_counts[i];
        }
    }
    return 0;
}

/* Counts one more definition of numeric label n; returns the count before. */
static uint local_bump(uint n) {
    for (uint i = 0; i < nlocal; i++) {
        if (local_numbers[i] == n) {
            return local_counts[i]++;
        }
    }
    uint c1 = caplocal;
    local_numbers = grow(local_numbers, &caplocal, nlocal + 1, sizeof(uint));
    local_counts = grow(local_counts, &c1, nlocal + 1, sizeof(uint));
    local_numbers[nlocal] = n;
    local_counts[nlocal] = 1;
    nlocal++;
    return 0;
}

/* `1b`/`1f` to the internal name of that numeric label's instance. */
static char *resolve_name(const char *name) {
    uint n = (uint)strlen(name);
    if (n >= 2 && (name[n - 1] == 'b' || name[n - 1] == 'f')) {
        /* The digits before must parse as a u32. */
        uint v = 0;
        int ok = 1;
        for (uint i = 0; i + 1 < n; i++) {
            if (name[i] < '0' || name[i] > '9') {
                ok = 0;
                break;
            }
            uint d = (uint)(name[i] - '0');
            if (v > (0xffffffffu - d) / 10) {
                ok = 0;
                break;
            }
            v = v * 10 + d;
        }
        if (ok) {
            uint count = local_count(v);
            uint k = name[n - 1] == 'b' ? count - 1 : count;
            char buf[32];
            snprintf(buf, sizeof buf, ".L%u$%u", v, k);
            return tmp_strndup(buf, (uint)strlen(buf));
        }
    }
    return (char *)name;
}

static int find_symbol(const char *name) {
    int i;
    return map_get(&by_name, name, &i) ? i : -1;
}

static int symbol_index(const char *name) {
    int i = find_symbol(name);
    if (i >= 0) {
        return i;
    }
    symbols = grow(symbols, &capsymbols, nsymbols + 1, sizeof(struct symbol));
    struct symbol *s = &symbols[nsymbols];
    memset(s, 0, sizeof *s);
    s->name = perm_strdup(name);
    s->section = -1;
    map_put(&by_name, s->name, (int)nsymbols);
    return (int)nsymbols++;
}

static void define(const char *name, int section, uint value) {
    int i = symbol_index(name);
    if (symbols[i].defined) {
        char *msg;
        msg = fmt("symbol '%s' is already defined", name);
        error(msg);
        return;
    }
    symbols[i].section = section;
    symbols[i].value = value;
    symbols[i].defined = 1;
}

/* A hidden label at `offset` in the current section for a use of `.`. */
static char *dot_label(uint offset) {
    char buf[32];
    snprintf(buf, sizeof buf, "%s%u", DOT_LABEL, dot_labels++);
    char *name = tmp_strndup(buf, (uint)strlen(buf));
    define(name, current, offset);
    return name;
}

static void label(const char *name) {
    const char *internal = name;
    /* A numeric label (`1:`): its digits parse as a u32. */
    uint v = 0;
    int numeric = name[0] != 0;
    for (const char *p = name; *p; p++) {
        uint d = (uint)(*p - '0');
        if (*p < '0' || *p > '9' || v > (0xffffffffu - d) / 10) {
            numeric = 0;
            break;
        }
        v = v * 10 + d;
    }
    if (numeric) {
        char buf[32];
        snprintf(buf, sizeof buf, ".L%u$%u", v, local_bump(v));
        internal = tmp_strndup(buf, (uint)strlen(buf));
    }
    define(internal, current, sections[current].size);
}

/* ---- Expressions ---- */

static int eval(struct expr *e, i64 *out) {
    i64 a, b;
    int i;
    switch (e->kind) {
    case E_NUM:
        *out = e->num;
        return 1;
    case E_SYM:
        i = find_symbol(e->sym);
        if (i < 0 || !symbols[i].has_constant) {
            return 0;
        }
        *out = symbols[i].constant;
        return 1;
    case E_NEG:
        if (!eval(e->a, &a)) {
            return 0;
        }
        *out = i64_neg(a);
        return 1;
    case E_NOT:
        if (!eval(e->a, &a)) {
            return 0;
        }
        *out = i64_not(a);
        return 1;
    case E_BIN:
        if (!eval(e->a, &a) || !eval(e->b, &b)) {
            return 0;
        }
        switch (e->op) {
        case B_ADD:
            *out = i64_add(a, b);
            return 1;
        case B_SUB:
            *out = i64_sub(a, b);
            return 1;
        case B_MUL:
            *out = i64_mul(a, b);
            return 1;
        case B_DIV:
            return i64_div(a, b, out);
        case B_REM:
            return i64_rem(a, b, out);
        case B_SHL:
            *out = i64_shl(a, b.lo);
            return 1;
        case B_SHR:
            /* Logical, as in LLVM (and merlin-as): -16 >> 2 is
             * 0x3ffffffffffffffc. */
            *out = i64_shr(a, b.lo);
            return 1;
        case B_AND:
            *out = i64_and(a, b);
            return 1;
        case B_OR:
            *out = i64_or(a, b);
            return 1;
        default:
            *out = i64_xor(a, b);
            return 1;
        }
    default:
        return 0;
    }
}

/* eval_relative's offsets: for each section, how many times its labels'
 * offsets were added (+1) or subtracted (-1). */
static int term_sections[64];
static int term_counts[64];
static int nterms;

static int add_term(int section, int sign) {
    for (int i = 0; i < nterms; i++) {
        if (term_sections[i] == section) {
            term_counts[i] += sign;
            return 1;
        }
    }
    if (nterms == 64) {
        return 0;
    }
    term_sections[nterms] = section;
    term_counts[nterms] = sign;
    nterms++;
    return 1;
}

static i64 times(int sign, i64 v) {
    return sign < 0 ? i64_neg(v) : v;
}

static int relative_terms(struct expr *e, int sign, i64 *out) {
    i64 a, b;
    if (e->kind == E_SYM && strcmp(e->sym, DOT) == 0) {
        *out = times(sign, i64_u(sections[current].size));
        return add_term(current, sign);
    }
    if (e->kind == E_SYM) {
        int i = find_symbol(resolve_name(e->sym));
        if (i < 0) {
            return 0;
        }
        struct symbol *s = &symbols[i];
        if (s->has_constant) {
            *out = times(sign, s->constant);
            return 1;
        }
        if (s->section >= 0 && s->defined) {
            *out = times(sign, i64_u(s->value));
            return add_term(s->section, sign);
        }
        return 0;
    }
    if (e->kind == E_BIN && (e->op == B_ADD || e->op == B_SUB)) {
        if (!relative_terms(e->a, sign, &a) || !relative_terms(e->b, e->op == B_ADD ? sign : -sign, &b)) {
            return 0;
        }
        *out = i64_add(a, b);
        return 1;
    }
    if (e->kind == E_NEG) {
        return relative_terms(e->a, -sign, out);
    }
    if (!eval(e, &a)) {
        return 0;
    }
    *out = times(sign, a);
    return 1;
}

/* A number from labels whose offsets cancel out (see asm.rs). */
static int eval_relative(struct expr *e, i64 *out) {
    nterms = 0;
    if (!relative_terms(e, 1, out)) {
        return 0;
    }
    for (int i = 0; i < nterms; i++) {
        if (term_counts[i] != 0) {
            return 0;
        }
    }
    return 1;
}

int ctx_constant(struct expr *e, i64 *v) {
    return eval(e, v);
}

int ctx_symbol(struct expr *e, char **name, i64 *addend) {
    i64 c, v;
    if (e->kind == E_SYM && !eval(e, &v)) {
        *name = resolve_name(e->sym);
        *addend = i64_u(0);
        return 1;
    }
    /* `c + sym` */
    if (e->kind == E_BIN && e->op == B_ADD && eval(e->a, &c)) {
        if (!ctx_symbol(e->b, name, addend)) {
            return 0;
        }
        *addend = i64_add(*addend, c);
        return 1;
    }
    /* `sym + c`, `sym - c` */
    if (e->kind == E_BIN && (e->op == B_ADD || e->op == B_SUB)) {
        if (!ctx_symbol(e->a, name, addend) || !eval(e->b, &c)) {
            return 0;
        }
        *addend = e->op == B_ADD ? i64_add(*addend, c) : i64_sub(*addend, c);
        return 1;
    }
    return 0;
}

int ctx_undefined(const char *name) {
    if (strcmp(name, DOT) == 0) {
        return 0;
    }
    int i = find_symbol(name);
    return i < 0 || !symbols[i].defined;
}

int ctx_csr(struct expr *e, uint *addr) {
    if (e->kind == E_SYM && csr_parse(e->sym, addr)) {
        return 1;
    }
    i64 v;
    if (!eval(e, &v)) {
        return 0;
    }
    if (i64_in(v, i64_u(0), i64_u(4096))) {
        *addr = v.lo;
        return 1;
    }
    return 0;
}

/* ---- Sections and emission ---- */

/* SHT_NOBITS: room that takes no bytes in the file, zeros only. */
static int is_nobits(struct section *s) {
    return s->attrs.ty == SHT_NOBITS;
}

/* SHF_EXECINSTR: code, which `.align` pads with `nop`s. */
static int is_code(struct section *s) {
    return (s->attrs.flags & SHF_EXECINSTR) != 0;
}

/* Debug information: a `.debug_*` section, data only. */
static int is_debug(struct section *s) {
    return starts_with(s->name, ".debug_");
}

/* Makes the section named `name` the current one if there is one. */
static int find_section(const char *name) {
    for (uint i = 0; i < nsections; i++) {
        if (strcmp(sections[i].name, name) == 0) {
            current = (int)i;
            return 1;
        }
    }
    return 0;
}

static char *section_symbol(int sec);

/* Adds a section, which becomes the current one. Its alignment starts at
 * 1, except for `.text`'s, 4: LLVM makes `.text` before the first line with
 * an instruction's alignment, and no other section, code or not, has one
 * until `.align` (or `.comm`) asks. Its section symbol is made here, where
 * LLVM registers it (the first switch to the section), and kept only if a
 * relocation uses it. */
static void new_section(const char *name, struct sec_attrs attrs) {
    sections = grow(sections, &capsections, nsections + 1, sizeof(struct section));
    struct section *s = &sections[nsections];
    memset(s, 0, sizeof *s);
    s->name = perm_strdup(name);
    s->attrs = attrs;
    s->align = strcmp(name, ".text") == 0 ? 4 : 1;
    current = (int)nsections++;
    section_symbol(current);
}

#define MAP_CODE 1
#define MAP_DATA 2

/* Marks what comes next in the current section, code or data, with a
 * mapping symbol where that changes (merlin-as's Assembler::mapping_symbol,
 * clang's RISCVELFStreamer): $x<isa> before a run of instructions, $d before
 * a run of data, even data of no bytes. A local symbol of its own, which no
 * name finds (not in by_name). */
static void mapping_symbol(int kind) {
    struct section *sec = &sections[current];
    if (sec->mapping == kind) {
        return;
    }
    sec->mapping = kind;
    symbols = grow(symbols, &capsymbols, nsymbols + 1, sizeof(struct symbol));
    struct symbol *s = &symbols[nsymbols++];
    memset(s, 0, sizeof *s);
    s->name = kind == MAP_CODE ? "$x" RISCV_ARCH : "$d";
    s->section = current;
    s->value = sec->size;
    s->defined = 1;
    s->ty = SYM_MAPPING;
}

/* Makes the section named `name` the current one, made with the attributes
 * its name gives it if there is none yet. */
static void switch_section(const char *name) {
    if (!find_section(name)) {
        struct sec_attrs a;
        section_by_name(name, &a);
        new_section(name, a);
    }
}

/* The error for bytes that would take the current section to 4 GiB. */
static void no_room(void) {
    error(fmt("section %s would be 4 GiB or larger", sections[current].name));
}

/* Whether n more bytes fit in the current section, which reports an error
 * if they do not. A section's size and offsets are 32-bit (all ELF32 has
 * room for), so a section stops short of 4 GiB: whatever would take it
 * there is an error, never a size that wraps around. */
static int has_room(uint n) {
    if (n <= 0xffffffffu - sections[current].size) {
        return 1;
    }
    no_room();
    return 0;
}

static void emit_bytes(const uchar *bytes, uint n) {
    if (!has_room(n)) {
        return;
    }
    struct section *s = &sections[current];
    if (is_nobits(s)) {
        for (uint i = 0; i < n; i++) {
            if (bytes[i] != 0) {
                char *msg;
                msg = fmt("non-zero data in %s", s->name);
                error(msg);
                return;
            }
        }
    } else {
        buf_put(&s->data, bytes, n);
    }
    s->size += n;
}

static void emit_zeros(uint n) {
    if (!has_room(n)) {
        return;
    }
    struct section *s = &sections[current];
    if (!is_nobits(s)) {
        buf_zeros(&s->data, n);
    }
    s->size += n;
}

static void add_reloc(uint offset, int kind, char *symbol, i64 addend) {
    symbol_index(symbol);
    struct section *s = &sections[current];
    s->relocs = grow(s->relocs, &s->cap, s->nrelocs + 1, sizeof(struct reloc));
    struct reloc *r = &s->relocs[s->nrelocs++];
    r->offset = offset;
    r->kind = kind;
    r->symbol = perm_strdup(symbol);
    r->addend = addend;
    r->line = line;
}

/* Pads the current section to a multiple of `align` and raises its
 * alignment (merlin-as's align_with): with the byte `fill` if has_fill (in
 * code too), else nops in code and zeros elsewhere; nothing if the padding
 * would be longer than `max` (when has_max). Padding makes no mapping
 * symbol. */
static void align_with(uint align, int has_fill, uint fill, int has_max, uint max) {
    if (align <= 1) {
        return;
    }
    struct section *s = &sections[current];
    if (align > s->align) {
        s->align = align;
    }
    uint pad = (align - s->size % align) % align;
    if (has_max && pad > max) {
        return;
    }
    if (has_fill && fill != 0) {
        uchar *bytes = tmp_alloc(pad + 1);
        memset(bytes, (int)fill, pad);
        emit_bytes(bytes, pad);
        return;
    }
    if (has_fill || !is_code(s)) {
        emit_zeros(pad);
        return;
    }
    /* Code pads as LLVM's writeNopData pads it: a zero byte to an even
     * offset, a c.nop (0x0001) to a multiple of 4, then nops. */
    if (pad % 2 == 1) {
        emit_zeros(1);
        pad--;
    }
    if (pad % 4 == 2) {
        static const uchar c_nop[2] = {0x01, 0};
        emit_bytes(c_nop, 2);
        pad -= 2;
    }
    static const uchar nop[4] = {0x13, 0, 0, 0};
    for (uint i = 0; i < pad / 4; i++) {
        emit_bytes(nop, 4);
    }
}

static void align_to(uint align) {
    align_with(align, 0, 0, 0, 0);
}

/* Gives the pending `.loc`, if any, a row at the current offset: the
 * instruction or data about to be emitted is where it points. */
static void take_loc(void) {
    if (!have_loc) {
        return;
    }
    have_loc = 0;
    rows = grow(rows, &caprows, nrows + 1, sizeof(struct row));
    rows[nrows].section = current;
    rows[nrows].offset = sections[current].size;
    rows[nrows].loc = pending_loc;
    nrows++;
}

/* Appends the encoder's words and turns their fixups into relocations. */
static void place(const char *mnemonic, struct emit *emits, int n) {
    struct section *s = &sections[current];
    char *msg;
    /* Zeros only in a NOBITS section, data only in debug information. */
    if (is_nobits(s) || is_debug(s)) {
        error(fmt("instructions are not allowed in %s", s->name));
        return;
    }
    if (s->size % 4 != 0) {
        error("instruction is not 4-byte aligned (add .align 2)");
        return;
    }
    take_loc();
    uint base = s->size;
    /* A label for `.` in an operand, made first, as clang makes it when it
     * reads the operand. */
    char *dot = NULL;
    for (int i = 0; i < n; i++) {
        if (emits[i].has_fixup && emits[i].auipc < 0 && strcmp(emits[i].name, DOT) == 0 && dot == NULL) {
            dot = dot_label(base);
        }
    }
    /* The hidden labels on auipcs a later %pcrel_lo refers to. */
    char *auipc_labels[2] = {NULL, NULL};
    for (int i = 0; i < n; i++) {
        int j = emits[i].auipc;
        if (emits[i].has_fixup && j >= 0) {
            if (j >= i) {
                msg = fmt("internal encoder error: '%s' refers to a later auipc", mnemonic);
                error(msg);
                return;
            }
            if (auipc_labels[j] == NULL) {
                char buf[32];
                snprintf(buf, sizeof buf, ".Lpcrel_hi%u", hidden_labels++);
                auipc_labels[j] = tmp_strndup(buf, (uint)strlen(buf));
                define(auipc_labels[j], current, base + 4 * (uint)j);
            }
        }
    }
    /* After the labels, before the symbols the operands name. */
    mapping_symbol(MAP_CODE);
    for (int i = 0; i < n; i++) {
        uint offset = base + 4 * (uint)i;
        struct emit *e = &emits[i];
        if (e->has_fixup) {
            char *symbol;
            i64 addend;
            if (e->auipc >= 0) {
                symbol = auipc_labels[e->auipc];
                addend = i64_u(0);
            } else {
                symbol = e->name;
                addend = e->addend;
                if (strcmp(symbol, DOT) == 0) {
                    symbol = dot;
                }
                if (e->kind == R_RISCV_PCREL_LO12_I || e->kind == R_RISCV_PCREL_LO12_S) {
                    pcrel_los = grow(pcrel_los, &cappcrel_lo, npcrel_lo + 1, sizeof(struct pcrel_lo));
                    pcrel_los[npcrel_lo].section = current;
                    pcrel_los[npcrel_lo].name = perm_strdup(symbol);
                    pcrel_los[npcrel_lo].line = line;
                    npcrel_lo++;
                }
            }
            add_reloc(offset, e->kind, symbol, addend);
        }
        uchar w[4];
        w[0] = (uchar)e->word;
        w[1] = (uchar)(e->word >> 8);
        w[2] = (uchar)(e->word >> 16);
        w[3] = (uchar)(e->word >> 24);
        emit_bytes(w, 4);
    }
}

/* ---- Directives ---- */

static int arg_const(struct dirarg *args, uint nargs, uint i, const char *what, i64 *out) {
    char *msg;
    if (i < nargs && args[i].kind == DA_EXPR) {
        if (eval(args[i].e, out)) {
            return 1;
        }
        msg = fmt("%s: expected a constant expression", what);
    } else {
        msg = fmt("%s: missing argument", what);
    }
    error(msg);
    return 0;
}

static char *arg_name(struct dirarg *args, uint nargs, uint i, const char *what) {
    if (i < nargs && args[i].kind == DA_EXPR && args[i].e->kind == E_SYM) {
        return args[i].e->sym;
    }
    if (i < nargs && args[i].kind == DA_STR) {
        return tmp_strndup((char *)args[i].str, args[i].len);
    }
    char *msg;
    msg = fmt("%s: expected a name", what);
    error(msg);
    return NULL;
}

/* Whether v can be written in size bytes, as a signed or an unsigned value
 * (merlin-as's fits_in_bytes, LLVM's isIntN || isUIntN). */
static int fits_in_bytes(i64 v, uint size) {
    if (size >= 8) {
        return 1;
    }
    i64 top = i64_shl(i64_u(1), 8 * size);
    i64 bottom = i64_neg(i64_shl(i64_u(1), 8 * size - 1));
    return i64_in(v, bottom, top);
}

/* .byte, .half, .word and .dword: merlin-as's Assembler::data. A number
 * must fit; a symbol's address is R_RISCV_32 in a .word and R_RISCV_64 in
 * a .dword. */
static void data(struct dirarg *args, uint nargs, uint size) {
    if (nargs > 0) {
        take_loc();
    }
    for (uint k = 0; k < nargs; k++) {
        if (args[k].kind != DA_EXPR) {
            error("expected a number, not a string");
            continue;
        }
        struct expr *e = args[k].e;
        i64 v;
        if (eval(e, &v) || eval_relative(e, &v)) {
            if (!fits_in_bytes(v, size)) {
                const char *unit = size == 1 ? "byte" : "bytes";
                error(fmt("data value %s does not fit in %u %s", i64_dec(v), size, unit));
                continue;
            }
            uchar bytes[8];
            for (uint i = 0; i < 4; i++) {
                bytes[i] = (uchar)(v.lo >> (8 * i));
                bytes[4 + i] = (uchar)(v.hi >> (8 * i));
            }
            mapping_symbol(MAP_DATA);
            emit_bytes(bytes, size);
            continue;
        }
        if (size != 4 && size != 8) {
            error("only .word and .dword can hold a symbol address");
            continue;
        }
        char *name;
        i64 addend;
        if (!ctx_symbol(e, &name, &addend)) {
            error("expected a constant or symbol(+offset)");
            continue;
        }
        uint off = sections[current].size;
        if (strcmp(name, DOT) == 0) {
            name = dot_label(off);
        }
        /* After the label for `.`, before the symbol it names. */
        mapping_symbol(MAP_DATA);
        add_reloc(off, size == 8 ? R_RISCV_64 : R_RISCV_32, name, addend);
        static const uchar zero[8] = {0, 0, 0, 0, 0, 0, 0, 0};
        emit_bytes(zero, size);
    }
}

/* `.set name, label + offset`: another label at that address. */
static void alias(const char *directive, const char *name, struct expr *e) {
    char *msg;
    char *target;
    i64 addend;
    if (!ctx_symbol(e, &target, &addend)) {
        msg = fmt("%s: expected a constant or label(+offset)", directive);
        error(msg);
        return;
    }
    int i = symbol_index(target);
    struct symbol *t = &symbols[i];
    if (t->section < 0 || !t->defined || t->has_constant) {
        msg = fmt("%s: '%s' must be a label defined before this", directive, shown_name(target));
        error(msg);
        return;
    }
    i64 value = i64_add(i64_u(t->value), addend);
    if (value.hi != 0) {
        msg = fmt("%s: '%s' would be outside its section", directive, name);
        error(msg);
        return;
    }
    define(name, t->section, value.lo);
}

/* `.section name[, "flags"[, @type[, entsize]]]` (merlin-as's
 * section_directive): switches to the section, made with the attributes
 * LLVM gives it (section.c): the name's defaults, plus the flags, the type
 * instead of the name's, or LLVM's own for a section it predefines. The
 * parser has made the type a string, however it was written. */
static void section_directive(struct dirarg *args, uint nargs) {
    char *name = arg_name(args, nargs, 0, ".section");
    if (name == NULL) {
        return;
    }
    struct sec_attrs want;
    section_name_defaults(name, &want.ty, &want.flags);
    want.entsize = 0;
    /* What the line asks for, which a section that exists must have. */
    uint given_flags = 0;
    int given_type = 0;
    if (nargs > 1) {
        if (args[1].kind != DA_STR) {
            error(".section: expected a string of flags");
            return;
        }
        uint bad;
        if (!section_flags(args[1].str, args[1].len, &given_flags, &bad)) {
            if (bad == 'G') {
                error(".section: flag 'G': section groups are not supported");
            } else if (bad == 'o') {
                error(".section: flag 'o': linked-to sections are not supported");
            } else if (bad > ' ' && bad < 127) {
                error(fmt(".section: unknown flag '%c'", (int)bad));
            } else {
                /* A byte that is no printable character, by number. */
                error(fmt(".section: unknown flag 0x%02x", bad));
            }
            return;
        }
        want.flags |= given_flags;
        if (nargs > 2) {
            if (args[2].kind != DA_STR) {
                error(".section: expected '@<type>', '%<type>' or \"<type>\"");
                return;
            }
            if (!section_type(args[2].str, args[2].len, &want.ty)) {
                char *t = tmp_strndup((char *)args[2].str, args[2].len);
                error(fmt(".section: unknown section type '%s'", t));
                return;
            }
            given_type = 1;
        }
        /* A mergeable section's entries are all one size, given after the
         * type. */
        uint used = 3;
        if (want.flags & SHF_MERGE) {
            if (!given_type) {
                error(".section: a mergeable section (M) must give its type");
                return;
            }
            if (nargs < 4) {
                error(".section: a mergeable section (M) must give its entry size");
                return;
            }
            i64 n;
            if (!arg_const(args, nargs, 3, ".section", &n)) {
                return;
            }
            if (i64_is_neg(n) || (n.hi == 0 && n.lo == 0)) {
                error(".section: the entry size must be positive");
                return;
            }
            /* sh_entsize is 32 bits in ELF32, and LLVM keeps an entry size
             * in 32 bits (a larger one fails its own check). */
            if (n.hi != 0) {
                error(fmt(".section: entry size %s does not fit in 32 bits", i64_dec(n)));
                return;
            }
            want.entsize = n.lo;
            used = 4;
        }
        if (nargs > used) {
            error(".section: unexpected argument (unique sections are not supported)");
            return;
        }
    }
    if (!find_section(name)) {
        /* LLVM's own sections exist before the first line: theirs win. */
        struct sec_attrs a;
        if (!section_predefined(name, &a)) {
            a = want;
        }
        new_section(name, a);
    }
    /* As LLVM (and GNU as) do, a `.section` with nothing after the name
     * takes the section as it is; with flags, a type or an entry size it
     * must not change it. The messages are LLVM's. */
    struct sec_attrs have = sections[current].attrs;
    if (given_type && have.ty != want.ty) {
        error(fmt("changed section type for %s, expected: 0x%X", name, have.ty));
    }
    if (given_flags != 0 || want.entsize != 0 || given_type) {
        if (have.flags != want.flags) {
            error(fmt("changed section flags for %s, expected: 0x%X", name, have.flags));
        }
        if (have.entsize != want.entsize) {
            error(fmt("changed section entsize for %s, expected: %u", name, have.entsize));
        }
    }
}

static int is_pow2(i64 v) {
    if (i64_is_neg(v) || (v.lo == 0 && v.hi == 0)) {
        return 0;
    }
    i64 m = i64_and(v, i64_sub(v, i64_u(1)));
    return m.lo == 0 && m.hi == 0;
}

/* `.p2align n[, fill[, max]]` (`.align` too) and `.balign bytes[, fill[,
 * max]]` (merlin-as's align_directive): the fill's low byte, may be left
 * out (`,,`); a max below 1 is LLVM's error, one of at least the alignment
 * changes nothing. */
static void align_directive(const char *name, struct dirarg *args, uint nargs) {
    i64 v;
    if (!arg_const(args, nargs, 0, name, &v)) {
        return;
    }
    uint align;
    if (strcmp(name, ".balign") == 0) {
        if (!is_pow2(v) || i64_cmp(v, i64_u(1u << 16)) > 0) {
            error(".balign: alignment must be a power of two, at most 65536");
            return;
        }
        align = v.lo;
    } else {
        if (!i64_in(v, i64_u(0), i64_u(17))) {
            error(fmt("%s: power of two must be 0..16", name));
            return;
        }
        align = 1u << v.lo;
    }
    if (nargs > 3) {
        error(fmt("%s: expected at most an alignment, a fill and a maximum", name));
        return;
    }
    int has_fill = 0;
    uint fill = 0;
    if (nargs > 1 && args[1].kind != DA_EMPTY) {
        i64 f;
        if (!arg_const(args, nargs, 1, name, &f)) {
            return;
        }
        has_fill = 1;
        fill = f.lo & 0xff;
    }
    int has_max = 0;
    uint max = 0;
    if (nargs > 2) {
        i64 m;
        if (!arg_const(args, nargs, 2, name, &m)) {
            return;
        }
        if (i64_cmp(m, i64_u(1)) < 0) {
            error("alignment directive can never be satisfied in this many bytes, ignoring maximum bytes expression");
            return;
        }
        if (i64_cmp(m, i64_u(align)) < 0) {
            has_max = 1;
            max = m.lo;
        }
    }
    align_with(align, has_fill, fill, has_max, max);
}

/* `.comm sym, size[, align]` for a .local symbol, or `.lcomm sym, size`:
 * zero bytes in .bss, at that alignment (`.lcomm`: 1). */
static void common(const char *directive, struct dirarg *args, uint nargs) {
    char *sym = arg_name(args, nargs, 0, directive);
    if (sym == NULL) {
        return;
    }
    int dummy;
    if (strcmp(directive, ".comm") == 0 && !map_get(&locals, sym, &dummy)) {
        error(fmt(".comm: '%s' is not .local; common symbols are not supported (use -fno-common)", sym));
        return;
    }
    i64 size, align;
    if (!arg_const(args, nargs, 1, directive, &size)) {
        return;
    }
    /* LLVM's ELF `.lcomm` takes no alignment. */
    if (strcmp(directive, ".lcomm") == 0 && nargs > 2) {
        error(".lcomm: alignment not supported on this target");
        return;
    }
    if (nargs > 2) {
        if (!arg_const(args, nargs, 2, directive, &align)) {
            return;
        }
    } else {
        align = i64_u(1);
    }
    /* LLVM's messages; a size too big for the section is the 4 GiB error
     * below, as for `.space`. */
    if (i64_is_neg(size)) {
        error(fmt("%s: size must be non-negative", directive));
        return;
    }
    if (!is_pow2(align)) {
        error(fmt("%s: alignment must be a power of 2", directive));
        return;
    }
    /* merlin-as's own limit, `.p2align`'s too. */
    if (i64_cmp(align, i64_u(1u << 16)) > 0) {
        error(fmt("%s: alignment must be at most 65536", directive));
        return;
    }
    int back = current;
    switch_section(".bss");
    align_to(align.lo);
    uint off = sections[current].size;
    define(sym, current, off);
    int i = symbol_index(sym);
    symbols[i].size = size.lo;
    if (symbols[i].ty == SYM_NOTYPE) {
        symbols[i].ty = SYM_OBJECT;
    }
    mapping_symbol(MAP_DATA);
    if (size.hi != 0) {
        no_room();
    } else {
        emit_zeros(size.lo);
    }
    current = back;
}

static int is_one_of(const char *name, const char *const *names) {
    for (; *names; names++) {
        if (strcmp(name, *names) == 0) {
            return 1;
        }
    }
    return 0;
}

/* ---- Line information ---- */

static int is_word(struct dirarg *a, const char *word) {
    return a->kind == DA_EXPR && a->e->kind == E_SYM && strcmp(a->e->sym, word) == 0;
}

static int same_file(struct dfile *a, struct dfile *b) {
    if (strcmp(a->dir, b->dir) != 0 || strcmp(a->name, b->name) != 0 || a->has_md5 != b->has_md5) {
        return 0;
    }
    return !a->has_md5 || memcmp(a->md5, b->md5, 16) == 0;
}

/* `.file "name"` (the file's name, which merlin-as has no use for) or
 * `.file N ["dir"] "name" [md5 0x...]`: file number N of the line table.
 * `.file 0` gives the compilation directory and the main file. */
static void file_directive(struct dirarg *args, uint nargs) {
    if (nargs == 1 && args[0].kind == DA_STR) {
        return;
    }
    if (nargs == 0 || args[0].kind != DA_EXPR) {
        error(".file: expected a file number or a name");
        return;
    }
    i64 v;
    if (!eval(args[0].e, &v) || v.hi != 0) {
        error(".file: expected a file number");
        return;
    }
    uint n = v.lo;
    char *strings[2];
    uint nstrings = 0;
    uint i = 1;
    for (; i < nargs && args[i].kind == DA_STR; i++) {
        if (nstrings < 2) {
            strings[nstrings] = tmp_strndup((char *)args[i].str, args[i].len);
        }
        nstrings++;
    }
    struct dfile file;
    memset(&file, 0, sizeof file);
    if (nstrings == 1) {
        file.dir = "";
        file.name = strings[0];
    } else if (nstrings == 2) {
        file.dir = strings[0];
        file.name = strings[1];
    } else {
        error(".file: expected [\"directory\"] \"name\" after the number");
        return;
    }
    uint rest = nargs - i;
    if (rest == 2 && is_word(&args[i], "md5")) {
        struct dirarg *sum = &args[i + 1];
        if (sum->kind == DA_WIDE) {
            memcpy(file.md5, sum->wide, 16);
        } else if (sum->kind == DA_EXPR && sum->e->kind == E_NUM) {
            /* A number that fits 64 bits: the low half, big-endian. */
            for (uint b = 0; b < 4; b++) {
                file.md5[8 + b] = (uchar)(sum->e->num.hi >> (24 - 8 * b));
                file.md5[12 + b] = (uchar)(sum->e->num.lo >> (24 - 8 * b));
            }
        } else {
            error(".file: md5 needs a 128-bit number");
            return;
        }
        file.has_md5 = 1;
    } else if (rest > 0 && is_word(&args[i], "source")) {
        error(".file: embedded source is not supported");
        return;
    } else if (rest > 0) {
        error(".file: unexpected text after the name");
        return;
    }
    file.dir = perm_strdup(file.dir);
    file.name = perm_strdup(file.name);
    if (n == 0) {
        file0 = file;
        have_file0 = 1;
        return;
    }
    if (n >= 1u << 24) {
        die("out of memory");
    }
    if (nfiles <= n) {
        files = grow(files, &capfiles, n + 1, sizeof(struct dfile));
        memset(&files[nfiles], 0, (n + 1 - nfiles) * sizeof(struct dfile));
        nfiles = n + 1;
    }
    if (files[n].name != NULL && !same_file(&files[n], &file)) {
        error(fmt("file number %u is already defined", n));
        return;
    }
    files[n] = file;
}

/* `.loc FILE [LINE [COLUMN]] [basic_block] [prologue_end]
 * [epilogue_begin] [is_stmt 0|1] [isa N] [discriminator N]`: the next
 * instruction comes from this place in the source. */
static void loc_directive(struct dirarg *args, uint nargs) {
    /* Two `.loc`s in a row: the first still gets a row, here (LLVM's rule). */
    take_loc();
    uint nums[3];
    uint nnums = 0;
    uint i = 0;
    while (nnums < 3 && i < nargs && args[i].kind == DA_EXPR && args[i].e->kind != E_SYM) {
        i64 v;
        if (!eval(args[i].e, &v) || v.hi != 0) {
            error(".loc: expected a number");
            return;
        }
        nums[nnums++] = v.lo;
        i++;
    }
    if (nnums == 0) {
        error(".loc: expected a file number");
        return;
    }
    uint file = nums[0];
    int defined = file == 0 ? have_file0 : file < nfiles && files[file].name != NULL;
    if (!defined) {
        error(fmt(".loc: file number %u has no .file", file));
        return;
    }
    struct loc loc;
    memset(&loc, 0, sizeof loc);
    loc.file = file;
    loc.line = nnums > 1 ? nums[1] : 0;
    loc.column = nnums > 2 ? nums[2] : 0;
    int stmt = is_stmt;
    while (i < nargs) {
        if (args[i].kind != DA_EXPR || args[i].e->kind != E_SYM) {
            error(".loc: expected an option such as prologue_end");
            return;
        }
        char *opt = args[i].e->sym;
        i++;
        uint flag;
        if (strcmp(opt, "basic_block") == 0) {
            flag = LOC_BASIC_BLOCK;
        } else if (strcmp(opt, "prologue_end") == 0) {
            flag = LOC_PROLOGUE_END;
        } else if (strcmp(opt, "epilogue_begin") == 0) {
            flag = LOC_EPILOGUE_BEGIN;
        } else if (strcmp(opt, "is_stmt") == 0 || strcmp(opt, "isa") == 0 || strcmp(opt, "discriminator") == 0) {
            flag = 0;
        } else {
            error(fmt(".loc: unknown option '%s'", opt));
            return;
        }
        if (flag != 0) {
            loc.flags |= flag;
            continue;
        }
        /* An option with a number after it. */
        i64 v;
        if (!(i < nargs && args[i].kind == DA_EXPR && eval(args[i].e, &v) && v.hi == 0)) {
            error(fmt(".loc: %s needs a number", opt));
            return;
        }
        i++;
        if (strcmp(opt, "is_stmt") == 0) {
            if (v.lo > 1) {
                error(".loc: is_stmt must be 0 or 1");
                return;
            }
            stmt = v.lo == 1;
        } else if (strcmp(opt, "isa") == 0) {
            loc.isa = v.lo;
        } else {
            loc.discriminator = v.lo;
        }
    }
    is_stmt = stmt;
    if (stmt) {
        loc.flags |= LOC_IS_STMT;
    }
    if (first_loc_line == 0) {
        first_loc_line = line;
    }
    pending_loc = loc;
    have_loc = 1;
}

/* ---- The ISA a source may name (merlin-as's isa.rs) ----
 *
 * `.option arch` and `.attribute arch` change the ISA in clang; this one has
 * only -march's, so one that names another is an error and one that names
 * the same is accepted. The same: the same width and the same extensions at
 * the same versions, with what they imply as LLVM adds it (m: zmmul; a:
 * zaamo and zalrsc; zaamo with zalrsc: a). */

#define NEXT 8
static const char *const EXT_NAME[NEXT] = {"i", "m", "a", "zicsr", "zifencei", "zmmul", "zaamo", "zalrsc"};
static const uint EXT_MAJOR[NEXT] = {2, 2, 2, 2, 2, 1, 1, 1};
static const uint EXT_MINOR[NEXT] = {1, 0, 1, 0, 0, 0, 0, 0};
enum { X_I, X_M, X_A, X_ZICSR, X_ZIFENCEI, X_ZMMUL, X_ZAAMO, X_ZALRSC };

struct isa {
    int has[NEXT];
    uint major[NEXT];
    uint minor[NEXT];
};

static void isa_ours(struct isa *x) {
    for (int k = 0; k < NEXT; k++) {
        x->has[k] = 1;
        x->major[k] = EXT_MAJOR[k];
        x->minor[k] = EXT_MINOR[k];
    }
}

static int isa_same(struct isa *a, struct isa *b) {
    for (int k = 0; k < NEXT; k++) {
        if (a->has[k] != b->has[k]) {
            return 0;
        }
        if (a->has[k] && (a->major[k] != b->major[k] || a->minor[k] != b->minor[k])) {
            return 0;
        }
    }
    return 1;
}

static void isa_set(struct isa *x, int k, uint major, uint minor) {
    x->has[k] = 1;
    x->major[k] = major;
    x->minor[k] = minor;
}

static void isa_imply(struct isa *x) {
    if (x->has[X_M] && !x->has[X_ZMMUL]) {
        isa_set(x, X_ZMMUL, 1, 0);
    }
    if (x->has[X_A]) {
        if (!x->has[X_ZAAMO]) {
            isa_set(x, X_ZAAMO, 1, 0);
        }
        if (!x->has[X_ZALRSC]) {
            isa_set(x, X_ZALRSC, 1, 0);
        }
    }
    if (x->has[X_ZAAMO] && x->has[X_ZALRSC] && !x->has[X_A]) {
        isa_set(x, X_A, 2, 1);
    }
}

/* The extension named by the n bytes at p; -1 if none of ours. */
static int ext_index(const char *p, uint n) {
    for (int k = 0; k < NEXT; k++) {
        if (strlen(EXT_NAME[k]) == n && strncmp(EXT_NAME[k], p, n) == 0) {
            return k;
        }
    }
    return -1;
}

static int is_digit(char c) {
    return c >= '0' && c <= '9';
}

/* Splits a trailing version (`2p1`, `2`) off the n bytes at p: the name's
 * length in *name_len, and 1 with the version, or 0 without one. */
static int split_version(const char *p, uint n, uint *name_len, uint *major, uint *minor) {
    uint e = n;
    while (e > 0 && (is_digit(p[e - 1]) || p[e - 1] == 'p')) {
        e--;
    }
    while (e < n && p[e] == 'p') {
        e++;
    }
    *name_len = n;
    if (e == n) {
        return 0;
    }
    uint a = 0, b = 0, i = e;
    int ps = 0;
    for (; i < n && is_digit(p[i]); i++) {
        a = a * 10 + (uint)(p[i] - '0');
    }
    if (i < n) {
        if (p[i] != 'p' || i + 1 >= n) {
            return 0;
        }
        i++;
        ps = 1;
        for (; i < n && is_digit(p[i]); i++) {
            b = b * 10 + (uint)(p[i] - '0');
        }
        if (i < n) {
            return 0;
        }
    }
    (void)ps;
    *name_len = e;
    *major = a;
    *minor = b;
    return 1;
}

/* Adds the extension of the n bytes at p (with its version or the default)
 * to x; 0 if it is none of ours. */
static int isa_add(struct isa *x, const char *p, uint n) {
    uint len, major, minor;
    int versioned = split_version(p, n, &len, &major, &minor);
    int k = ext_index(p, len);
    if (k < 0) {
        return 0;
    }
    if (!versioned) {
        major = EXT_MAJOR[k];
        minor = EXT_MINOR[k];
    }
    isa_set(x, k, major, minor);
    return 1;
}

/* A full ISA string into *x, and its width in *bits; 0 if it is none LLVM
 * would take with these extensions. */
static int isa_parse(const char *s, uint *bits, struct isa *x) {
    memset(x, 0, sizeof *x);
    if (strncmp(s, "rv32", 4) == 0) {
        *bits = 32;
    } else if (strncmp(s, "rv64", 4) == 0) {
        *bits = 64;
    } else {
        return 0;
    }
    const char *p = s + 4;
    for (uint k = 0;; k++) {
        uint n = 0;
        while (p[n] != 0 && p[n] != '_') {
            n++;
        }
        if (n == 0) {
            return 0;
        }
        if (k > 0 && (p[0] == 'z' || p[0] == 's' || p[0] == 'x')) {
            if (!isa_add(x, p, n)) {
                return 0;
            }
        } else {
            for (uint i = 0; i < n;) {
                if (p[i] < 'a' || p[i] > 'z' || (k == 0 && i == 0 && p[i] != 'i')) {
                    return 0;
                }
                uint start = i++;
                while (i < n && (is_digit(p[i]) || (p[i] == 'p' && i + 1 < n && is_digit(p[i + 1])))) {
                    i++;
                }
                if (!isa_add(x, p + start, i - start)) {
                    return 0;
                }
            }
        }
        if (p[n] == 0) {
            break;
        }
        p += n + 1;
    }
    isa_imply(x);
    return 1;
}

static uint isa_bits(void) {
    return strncmp(RISCV_ARCH, "rv64", 4) == 0 ? 64 : 32;
}

static void isa_other(const char *directive) {
    error(fmt("%s: merlin-as assembles only -march's ISA (%s), and this names another", directive, RISCV_ARCH));
}

/* `.attribute arch, "isa"`. */
static void check_attribute(const char *isa) {
    struct isa x, ours;
    uint bits;
    isa_ours(&ours);
    if (!isa_parse(isa, &bits, &x) || bits != isa_bits() || !isa_same(&x, &ours)) {
        isa_other(".attribute arch");
    }
}

/* `.option arch, ...`: a full ISA string, or +ext and -ext items. */
static void check_option_arch(struct dirarg *items, uint n) {
    struct isa x, ours;
    isa_ours(&ours);
    isa_ours(&x);
    if (n == 0) {
        error(".option arch: expected an ISA, or extensions after + or -");
        return;
    }
    char *first = tmp_strndup((char *)items[0].str, items[0].len);
    if (n == 1 && strncmp(first, "rv", 2) == 0) {
        uint bits;
        if (!isa_parse(first, &bits, &x) || bits != isa_bits() || !isa_same(&x, &ours)) {
            isa_other(".option arch");
        }
        return;
    }
    for (uint i = 0; i < n; i++) {
        char *item = tmp_strndup((char *)items[i].str, items[i].len);
        const char *name = item + 1;
        uint len = (uint)strlen(name);
        if (item[0] == '+') {
            uint nl, major, minor;
            if (split_version(name, len, &nl, &major, &minor)) {
                error("extension version number parsing not currently implemented");
                return;
            }
            int k = ext_index(name, len);
            if (k < 0) {
                isa_other(".option arch");
                return;
            }
            isa_set(&x, k, EXT_MAJOR[k], EXT_MINOR[k]);
        } else if (item[0] == '-') {
            int k = ext_index(name, len);
            const char *by = NULL;
            if (k == X_ZMMUL && x.has[X_M]) {
                by = "m";
            } else if ((k == X_ZAAMO || k == X_ZALRSC) && x.has[X_A]) {
                by = "a";
            }
            if (by != NULL) {
                error(fmt("can't disable %s extension; %s extension requires %s extension", name, by, name));
                return;
            }
            if (k >= 0) {
                x.has[k] = 0;
            }
        } else {
            error(fmt(".option arch: expected an ISA, or extensions after + or -, not '%s'", item));
            return;
        }
        isa_imply(&x);
    }
    if (!isa_same(&x, &ours)) {
        isa_other(".option arch");
    }
}

/* `.option NAME[, ...]` (merlin-as's Assembler::option): arch and rvc must
 * not change the ISA; the others are ignored. */
static void option_directive(struct dirarg *args, uint nargs) {
    if (nargs == 0) {
        error(".option: expected an option such as push, pop or arch");
        return;
    }
    char *what = tmp_strndup((char *)args[0].str, args[0].len);
    if (strcmp(what, "arch") == 0) {
        check_option_arch(args + 1, nargs - 1);
    } else if (strcmp(what, "rvc") == 0) {
        isa_other(".option rvc");
    }
}

/* `.attribute TAG, value`: arch (5) must name -march's ISA; the others are
 * ignored. */
static void attribute_directive(struct dirarg *args, uint nargs) {
    i64 v;
    int arch = 0;
    if (nargs > 0 && args[0].kind == DA_EXPR) {
        if (args[0].e->kind == E_SYM) {
            arch = strcmp(args[0].e->sym, "arch") == 0;
        } else {
            arch = eval(args[0].e, &v) && v.hi == 0 && v.lo == 5;
        }
    }
    if (!arch) {
        return;
    }
    if (nargs < 2 || args[1].kind != DA_STR) {
        error(".attribute arch: expected a string");
        return;
    }
    check_attribute(tmp_strndup((char *)args[1].str, args[1].len));
}

static const char *const IGNORED[] = {".ident", ".addrsig", ".addrsig_sym", ".cfi_startproc", ".cfi_endproc", NULL};

static void directive(const char *name, struct dirarg *args, uint nargs) {
    char *msg;
    i64 v;
    if (strcmp(name, ".text") == 0 || strcmp(name, ".data") == 0 || strcmp(name, ".rodata") == 0 ||
        strcmp(name, ".bss") == 0) {
        switch_section(name);
    } else if (strcmp(name, ".section") == 0) {
        section_directive(args, nargs);
    } else if (strcmp(name, ".globl") == 0 || strcmp(name, ".global") == 0) {
        for (uint i = 0; i < nargs; i++) {
            char *n = arg_name(args, nargs, i, name);
            if (n != NULL) {
                /* The index first: symbol_index may move the array, and
                 * merlin reads `symbols` before it calls. */
                int k = symbol_index(n);
                symbols[k].global = 1;
            }
        }
    } else if (strcmp(name, ".local") == 0) {
        for (uint i = 0; i < nargs; i++) {
            char *n = arg_name(args, nargs, i, name);
            if (n != NULL) {
                map_put(&locals, perm_strdup(n), 1);
            }
        }
    } else if (strcmp(name, ".comm") == 0 || strcmp(name, ".lcomm") == 0) {
        common(name, args, nargs);
    } else if (strcmp(name, ".equ") == 0 || strcmp(name, ".set") == 0) {
        char *n = arg_name(args, nargs, 0, name);
        if (n == NULL) {
            return;
        }
        if (nargs > 1 && args[1].kind == DA_EXPR && !eval(args[1].e, &v)) {
            alias(name, n, args[1].e);
            return;
        }
        if (!arg_const(args, nargs, 1, name, &v)) {
            return;
        }
        int k = symbol_index(n);
        struct symbol *s = &symbols[k];
        if (s->defined && !s->has_constant) {
            msg = fmt("'%s' is already a label", n);
            error(msg);
            return;
        }
        s->defined = 1;
        s->has_constant = 1;
        s->constant = v;
    } else if (strcmp(name, ".byte") == 0) {
        data(args, nargs, 1);
    } else if (strcmp(name, ".half") == 0 || strcmp(name, ".short") == 0 || strcmp(name, ".2byte") == 0) {
        data(args, nargs, 2);
    } else if (strcmp(name, ".word") == 0 || strcmp(name, ".long") == 0 || strcmp(name, ".4byte") == 0) {
        data(args, nargs, 4);
    } else if (strcmp(name, ".dword") == 0 || strcmp(name, ".quad") == 0 || strcmp(name, ".8byte") == 0) {
        data(args, nargs, 8);
    } else if (strcmp(name, ".ascii") == 0 || strcmp(name, ".asciz") == 0 || strcmp(name, ".string") == 0) {
        if (nargs > 0) {
            take_loc();
        }
        for (uint i = 0; i < nargs; i++) {
            if (args[i].kind == DA_STR) {
                mapping_symbol(MAP_DATA);
                emit_bytes(args[i].str, args[i].len);
                if (strcmp(name, ".ascii") != 0) {
                    static const uchar nul[1] = {0};
                    emit_bytes(nul, 1);
                }
            } else {
                msg = fmt("%s: expected a string", name);
                error(msg);
            }
        }
    } else if (strcmp(name, ".zero") == 0 || strcmp(name, ".space") == 0 || strcmp(name, ".skip") == 0) {
        /* `.space n[, fill]`: n bytes of fill's low byte (zero without one).
         * A negative n is LLVM's "invalid number of bytes"; n is never cut
         * to fit: one that would take the section to 4 GiB is an error. */
        if (!arg_const(args, nargs, 0, name, &v)) {
            return;
        }
        i64 fill = i64_u(0);
        if (nargs > 1 && !arg_const(args, nargs, 1, name, &fill)) {
            return;
        }
        if (i64_is_neg(v)) {
            error("invalid number of bytes");
            return;
        }
        /* Checked before the bytes are made, so that a huge count is an
         * error rather than a huge allocation. */
        if (v.hi != 0) {
            no_room();
            return;
        }
        if (!has_room(v.lo)) {
            return;
        }
        mapping_symbol(MAP_DATA);
        uint byte = fill.lo & 0xff;
        if (byte == 0 || v.lo == 0) {
            emit_zeros(v.lo);
        } else {
            uchar *bytes = tmp_alloc(v.lo);
            memset(bytes, (int)byte, v.lo);
            emit_bytes(bytes, v.lo);
        }
    } else if (strcmp(name, ".align") == 0 || strcmp(name, ".p2align") == 0 || strcmp(name, ".balign") == 0) {
        align_directive(name, args, nargs);
    } else if (strcmp(name, ".type") == 0) {
        char *n = arg_name(args, nargs, 0, ".type");
        if (n != NULL) {
            int ty = SYM_NOTYPE;
            if (nargs > 1 && args[1].kind == DA_EXPR && args[1].e->kind == E_SYM) {
                if (strstr(args[1].e->sym, "function") != NULL) {
                    ty = SYM_FUNC;
                } else if (strstr(args[1].e->sym, "object") != NULL) {
                    ty = SYM_OBJECT;
                }
            }
            int k = symbol_index(n);
            symbols[k].ty = ty;
        }
    } else if (strcmp(name, ".size") == 0) {
        /* `.size f, . - f` (merlin's), `.size f, .Lfunc_end0 - f` (clang's)
         * or a constant. Any value is taken, as LLVM takes it, and an
         * ELF32 st_size keeps its low 32 bits (clang riscv32 writes 0 for
         * 0x100000000 and 0xfffffffc for -4). */
        char *n = arg_name(args, nargs, 0, ".size");
        if (n != NULL) {
            int ok = nargs > 1 && args[1].kind == DA_EXPR && eval_relative(args[1].e, &v);
            if (ok) {
                int k = symbol_index(n);
                symbols[k].size = v.lo;
            } else {
                error(".size: expected a constant, or a difference of '.' and labels defined earlier in this "
                      "section");
            }
        }
    } else if (strcmp(name, ".file") == 0) {
        file_directive(args, nargs);
    } else if (strcmp(name, ".loc") == 0) {
        loc_directive(args, nargs);
    } else if (strcmp(name, ".option") == 0) {
        option_directive(args, nargs);
    } else if (strcmp(name, ".attribute") == 0) {
        attribute_directive(args, nargs);
    } else if (is_one_of(name, IGNORED)) {
        /* Metadata that does not change the output. */
    } else {
        msg = fmt("unknown directive '%s'", name);
        error(msg);
    }
}

/* ---- Statements ---- */

void asm_init(void) {
    switch_section(".text");
}

void asm_statement(struct stmt *s) {
    line = s->line;
    if (s->kind == S_LABEL) {
        label(s->name);
    } else if (s->kind == S_DIRECTIVE) {
        directive(s->name, s->args, s->nargs);
    } else {
        struct emit emits[2];
        char *err = NULL;
        int n = encode(s->name, s->ops, s->nops, emits, &err);
        if (n > 0) {
            place(s->name, emits, n);
        } else {
            error(err);
        }
    }
}

/* ---- The end of the file ---- */

/* The first relocation of `s` at or after `offset`: they are in offset
 * order, the order the code and data were emitted in. */
static uint first_reloc_at(struct section *s, uint offset) {
    uint lo = 0, hi = s->nrelocs;
    while (lo < hi) {
        uint mid = (lo + hi) / 2;
        if (s->relocs[mid].offset < offset) {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    return lo;
}

/* A %pcrel_lo's label must be on an auipc with %pcrel_hi, in its section. */
static void check_pcrel_lo(void) {
    for (uint k = 0; k < npcrel_lo; k++) {
        struct pcrel_lo *p = &pcrel_los[k];
        int i = find_symbol(p->name);
        if (i >= 0 && !symbols[i].defined && starts_with(symbols[i].name, ".L")) {
            continue; /* reported as an undefined local label */
        }
        int paired = 0;
        if (i >= 0 && symbols[i].defined && symbols[i].section == p->section) {
            struct section *sec = &sections[p->section];
            uint at = symbols[i].value;
            for (uint r = first_reloc_at(sec, at); r < sec->nrelocs && sec->relocs[r].offset == at; r++) {
                if (sec->relocs[r].kind == R_RISCV_PCREL_HI20) {
                    paired = 1;
                    break;
                }
            }
        }
        if (!paired) {
            char *msg;
            msg = fmt("%%pcrel_lo(%s): no auipc with %%pcrel_hi at that label in this section",
                     shown_name(p->name));
            error_at(p->line, msg);
        }
    }
}

/* The distance from `offset` in `section` to a fixup's target, if that is
 * a local label defined in the same section. */
static int local_distance(int section, uint offset, const char *symbol, i64 addend, i64 *out) {
    int i = find_symbol(symbol);
    if (i < 0) {
        return 0;
    }
    struct symbol *s = &symbols[i];
    if (!s->defined || s->global || s->has_constant || s->section != section) {
        return 0;
    }
    *out = i64_sub(i64_add(i64_u(s->value), addend), i64_u(offset));
    return 1;
}

/* `{:+}`: a number with its sign. */
static const char *signed_text(i64 v) {
    if (i64_is_neg(v)) {
        return i64_dec(v);
    }
    const char *d = i64_dec(v);
    char *r = tmp_alloc((uint)strlen(d) + 2);
    r[0] = '+';
    strcpy(r + 1, d);
    return r;
}

static int check_jump(i64 distance, uint bits, const char *what, char **msg) {
    i64 half = i64_shl(i64_u(1), bits - 1);
    if (!i64_in(distance, i64_neg(half), half)) {
        *msg = fmt("%s target out of range: %s bytes (limit %s..=+%s)", what, signed_text(distance),
                 i64_dec(i64_neg(half)), i64_dec(i64_sub(half, i64_u(2))));
        return 0;
    }
    if (distance.lo & 1) {
        *msg = fmt("%s target at an odd distance (%s bytes)", what, signed_text(distance));
        return 0;
    }
    return 1;
}

static void or_word(struct section *s, uint at, uint bits) {
    uchar *p = s->data.data + at;
    uint w = (uint)p[0] | (uint)p[1] << 8 | (uint)p[2] << 16 | (uint)p[3] << 24;
    w |= bits;
    p[0] = (uchar)w;
    p[1] = (uchar)(w >> 8);
    p[2] = (uchar)(w >> 16);
    p[3] = (uchar)(w >> 24);
}

/* Writes a resolved distance into the word(s) at `offset`, as a linker would. */
static int patch(int section, uint offset, int kind, i64 distance, char **msg) {
    uint hi;
    int lo;
    struct section *s = &sections[section];
    hi_lo((int)distance.lo, &hi, &lo);
    switch (kind) {
    case R_RISCV_BRANCH:
        if (!check_jump(distance, 13, "branch", msg)) {
            return 0;
        }
        or_word(s, offset, b_imm((int)distance.lo));
        return 1;
    case R_RISCV_JAL:
        if (!check_jump(distance, 21, "jump", msg)) {
            return 0;
        }
        or_word(s, offset, j_imm((int)distance.lo));
        return 1;
    case R_RISCV_PCREL_HI20:
        or_word(s, offset, u_imm(hi));
        return 1;
    case R_RISCV_PCREL_LO12_I:
        or_word(s, offset, i_imm(lo));
        return 1;
    case R_RISCV_PCREL_LO12_S:
        or_word(s, offset, s_imm(lo));
        return 1;
    default: /* R_RISCV_CALL_PLT: auipc + jalr */
        or_word(s, offset, u_imm(hi));
        or_word(s, offset + 4, i_imm(lo));
        return 1;
    }
}

/* (section, offset of a resolved auipc) -> its distance. */
struct auipc_distance {
    int section;
    uint offset;
    i64 distance;
};

static void resolve_local_fixups(void) {
    struct auipc_distance *ad = NULL;
    uint nad = 0, capad = 0;
    for (uint sec = 0; sec < nsections; sec++) {
        struct section *s = &sections[sec];
        uint kept = 0;
        for (uint k = 0; k < s->nrelocs; k++) {
            struct reloc r = s->relocs[k];
            i64 distance;
            int resolved = (r.kind == R_RISCV_BRANCH || r.kind == R_RISCV_JAL || r.kind == R_RISCV_CALL_PLT || r.kind == R_RISCV_PCREL_HI20) &&
                           local_distance((int)sec, r.offset, r.symbol, r.addend, &distance);
            if (!resolved) {
                s->relocs[kept++] = r;
                continue;
            }
            if (r.kind == R_RISCV_PCREL_HI20) {
                ad = grow(ad, &capad, nad + 1, sizeof(struct auipc_distance));
                ad[nad].section = (int)sec;
                ad[nad].offset = r.offset;
                ad[nad].distance = distance;
                nad++;
            }
            char *msg;
            if (!patch((int)sec, r.offset, r.kind, distance, &msg)) {
                error_at(r.line, fmt("'%s': %s", shown_name(r.symbol), msg));
            }
        }
        s->nrelocs = kept;
    }
    for (uint sec = 0; sec < nsections; sec++) {
        struct section *s = &sections[sec];
        uint kept = 0;
        for (uint k = 0; k < s->nrelocs; k++) {
            struct reloc r = s->relocs[k];
            int done = 0;
            if (r.kind == R_RISCV_PCREL_LO12_I || r.kind == R_RISCV_PCREL_LO12_S) {
                int i = find_symbol(r.symbol);
                if (i >= 0 && symbols[i].defined && symbols[i].section == (int)sec) {
                    /* `ad` is in (section, offset) order: pass 1 made it so. */
                    uint lo = 0, hi = nad, at = symbols[i].value;
                    while (lo < hi) {
                        uint mid = (lo + hi) / 2;
                        if (ad[mid].section < (int)sec || (ad[mid].section == (int)sec && ad[mid].offset < at)) {
                            lo = mid + 1;
                        } else {
                            hi = mid;
                        }
                    }
                    if (lo < nad && ad[lo].section == (int)sec && ad[lo].offset == at) {
                        char *msg;
                        patch((int)sec, r.offset, r.kind, ad[lo].distance, &msg);
                        done = 1;
                    }
                }
            }
            if (!done) {
                s->relocs[kept++] = r;
            }
        }
        s->nrelocs = kept;
    }
    free(ad);
}

/* The symbol of section `sec`: a name no source can write. */
static char *section_symbol(int sec) {
    char buf[32];
    snprintf(buf, sizeof buf, "\001section%d", sec);
    int i = symbol_index(buf);
    symbols[i].section = sec;
    symbols[i].defined = 1;
    symbols[i].ty = SYM_SECTION;
    return symbols[i].name;
}

static uint section_size(const char *name) {
    for (uint i = 0; i < nsections; i++) {
        if (strcmp(sections[i].name, name) == 0) {
            return sections[i].size;
        }
    }
    return 0;
}

/* Appends `bytes` to the section named `name` (made if there is none, as
 * debug sections are). Returns the section. */
static int append(const char *name, struct buf *bytes) {
    int back = current;
    switch_section(name);
    mapping_symbol(MAP_DATA);
    int sec = current;
    buf_put(&sections[sec].data, bytes->data, bytes->len);
    sections[sec].size += bytes->len;
    current = back;
    return sec;
}

/* Writes the rows as `.debug_line` (and its strings in `.debug_line_str`),
 * with relocations for the strings and the sections' addresses. */
static void line_table(void) {
    if (nrows == 0) {
        return;
    }
    if (!have_file0) {
        error_at(first_loc_line, "line information (.loc) needs .file 0, the compilation directory and the main file");
        return;
    }
    for (uint n = 1; n < nfiles; n++) {
        if (files[n].name == NULL) {
            error_at(first_loc_line, fmt("the line table's files must be numbered 1, 2, ...: no .file %u", n));
            return;
        }
    }
    uint base = section_size(".debug_line");
    uint str_base = section_size(".debug_line_str");
    struct line_table t;
    dwarf_encode(&file0, files + 1, nfiles > 0 ? nfiles - 1 : 0, rows, nrows, sections, nsections, base, str_base,
                 &t);
    int line_sec = append(".debug_line", &t.line);
    int str_sec = append(".debug_line_str", &t.line_str);
    for (uint k = 0; k < t.nfixups; k++) {
        struct dfixup *f = &t.fixups[k];
        char *symbol = section_symbol(f->section < 0 ? str_sec : f->section);
        struct section *s = &sections[line_sec];
        s->relocs = grow(s->relocs, &s->cap, s->nrelocs + 1, sizeof(struct reloc));
        struct reloc *r = &s->relocs[s->nrelocs++];
        memset(r, 0, sizeof *r);
        r->offset = f->at;
        r->kind = R_RISCV_32;
        r->symbol = symbol;
        r->addend = i64_u(f->off);
        r->line = first_loc_line;
    }
    free(t.line.data);
    free(t.line_str.data);
    free(t.fixups);
}

/* In debug sections, a relocation against a `.L` label is one against its
 * section, plus its offset. */
static void debug_relocs_to_sections(void) {
    for (uint sec = 0; sec < nsections; sec++) {
        if (!is_debug(&sections[sec])) {
            continue;
        }
        for (uint k = 0; k < sections[sec].nrelocs; k++) {
            struct reloc *r = &sections[sec].relocs[k];
            if (!starts_with(r->symbol, ".L")) {
                continue;
            }
            int i = find_symbol(r->symbol);
            if (i < 0 || symbols[i].section < 0 || !symbols[i].defined || symbols[i].has_constant) {
                continue;
            }
            uint value = symbols[i].value;
            r->symbol = section_symbol(symbols[i].section);
            r->addend = i64_add(r->addend, i64_u(value));
        }
    }
}

/* `.riscv.attributes`, which clang writes at the end: a section of the
 * object, data with its $d, after the source's sections and before the line
 * table's. */
static void attributes_section(void) {
    int back = current;
    struct sec_attrs a;
    a.ty = SHT_RISCV_ATTRIBUTES;
    a.flags = 0;
    a.entsize = 0;
    new_section(".riscv.attributes", a);
    mapping_symbol(MAP_DATA);
    struct buf b;
    memset(&b, 0, sizeof b);
    riscv_attributes(&b);
    emit_bytes(b.data, b.len);
    free(b.data);
    current = back;
}

struct object *asm_finish(void) {
    check_pcrel_lo();
    for (uint i = 0; i < nsymbols; i++) {
        if (!symbols[i].defined && starts_with(symbols[i].name, ".L")) {
            char *msg;
            msg = fmt("undefined local label '%s'", shown_name(symbols[i].name));
            error_at(0, msg);
        }
    }
    resolve_local_fixups();
    /* What clang writes when the source has been read, in its order: the
     * attributes, then the line table. */
    attributes_section();
    line_table();
    debug_relocs_to_sections();
    if (error_count(PHASE_ASM) > 0) {
        return NULL;
    }
    /* `.L` labels are assembler-internal, and a section symbol stands in for
     * labels: keep only those a relocation needs. A relocation names the
     * first symbol of its name that is no mapping symbol. */
    struct strmap referenced;
    memset(&referenced, 0, sizeof referenced);
    for (uint sec = 0; sec < nsections; sec++) {
        for (uint k = 0; k < sections[sec].nrelocs; k++) {
            map_put(&referenced, sections[sec].relocs[k].symbol, 1);
        }
    }
    struct object *obj = perm_alloc(sizeof *obj);
    obj->sections = sections;
    obj->nsections = nsections;
    obj->symbols = malloc((nsymbols + 1) * sizeof(struct symbol));
    if (obj->symbols == NULL) {
        die("out of memory");
    }
    struct strmap first;
    memset(&first, 0, sizeof first);
    for (uint i = 0; i < nsymbols; i++) {
        int dummy;
        int internal = starts_with(symbols[i].name, ".L") || symbols[i].ty == SYM_SECTION;
        if (!internal || map_get(&referenced, symbols[i].name, &dummy)) {
            if (symbols[i].ty != SYM_MAPPING && !map_get(&first, symbols[i].name, &dummy)) {
                map_put(&first, symbols[i].name, (int)obj->nsymbols);
            }
            obj->symbols[obj->nsymbols++] = symbols[i];
        }
    }
    for (uint sec = 0; sec < nsections; sec++) {
        for (uint k = 0; k < sections[sec].nrelocs; k++) {
            int i = 0;
            map_get(&first, sections[sec].relocs[k].symbol, &i);
            sections[sec].relocs[k].sym = (uint)i;
        }
    }
    return obj;
}
