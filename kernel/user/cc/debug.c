/* cc1: debug information (-g), follows merlin's debug.rs and its order.rs:
 * where each instruction comes from in the source, and what the program's
 * variables and types are, for debuggers (merlin-dbg, lldb) and
 * merlin-objdump -l. The spec's section 10 ("Debug information") says what
 * is described and how; debug.rs's comments explain clang's and LLVM's
 * rules in full, and this file says where each piece follows them.
 *
 * Like merlin, cc1 writes it as assembly, the way clang does, and leaves
 * the encoding of the line table to the assembler:
 *
 *   - `.file N "dir" "name" md5 0x...` numbers the source files: 0 is the
 *     file being compiled, with the compilation directory
 *     (-fdebug-compilation-dir, the directory the names are relative to);
 *     then each header that has code or declares a described entry,
 *     numbered as LLVM numbers them (debug_renumber). Each names its file
 *     as clang does, split into a directory and a name ("File names"
 *     below), with the MD5 of its contents (md5.c).
 *   - `.loc FILE LINE COLUMN` goes before the code of every statement
 *     (codegen.c): a function's first line (column 0) before its prologue,
 *     each statement's start, a loop's condition and step each time they
 *     run, the closing brace before the epilogue. The first after the
 *     prologue says prologue_end.
 *
 * The rest is DWARF 5 debugging information entries (DIEs), in
 * .debug_info, with their abbreviations in .debug_abbrev: a compile unit,
 * a subprogram per function with its parameters, locals, lexical blocks and
 * labels, a variable per global and per string literal, and the types they
 * use, each described once. The tree is built (struct die), laid out to know
 * where each entry starts (a type is referred to by its offset in the
 * unit, DW_FORM_ref4), and written byte by byte with .byte, .half, .word,
 * .quad and .asciz. The entries are in clang's order, which order() works
 * out from what the code generator saw (struct event). None of it changes
 * the code: labels and directives assemble to no bytes in .text.
 *
 * File names: a .file names its file the way clang 23's -S does on Linux
 * (and so on Camelot, a POSIX system): clang makes the file's DIFile
 * (clang_file: a relative name goes with the compilation directory, an
 * absolute one is split where it leaves it), then LLVM's assembler files it
 * (file_entry: the compilation directory is no directory, a file named as
 * the main file with the same checksum is file 0, and a name with no
 * directory left is split at its last `/`).
 *
 * Text: a file's name is bytes and a length (struct str), as the
 * preprocessor gives it (a name #line gives may hold a NUL); the other
 * names (identifiers, labels, the compilation directory) are C strings. */
#include "cc.h"

/* ---- Text with a length ---- */

struct str {
    const char *s;
    uint n;
};

static struct str str_n(const char *s, uint n) {
    struct str r;
    r.s = s;
    r.n = n;
    return r;
}

static struct str str_c(const char *s) {
    return str_n(s, (uint)strlen(s));
}

static int str_eq(struct str a, struct str b) {
    return a.n == b.n && (a.n == 0 || memcmp(a.s, b.s, a.n) == 0);
}

/* The buffer's bytes, as text in the arena; the buffer is freed. */
static struct str str_of_buf(struct buf *b) {
    struct str r = str_n(buf_arena(b), b->len);
    buf_free(b);
    return r;
}

/* ---- Paths: LLVM's sys::path, POSIX style (merlin-as's dwarf.rs) ---- */

/* Where LLVM's sys::path functions see the root directory of p
 * (root_dir_start): the `/` of /x, the separator after a network name
 * (//net/x), or none (-1). */
static int root_dir_start(struct str p) {
    if (p.n > 3 && p.s[0] == '/' && p.s[1] == '/' && p.s[2] != '/') {
        for (uint i = 2; i < p.n; i++) {
            if (p.s[i] == '/') {
                return (int)i;
            }
        }
        return -1;
    }
    return p.n > 0 && p.s[0] == '/' ? 0 : -1;
}

/* Where the last component of the n bytes at s starts (filename_pos): at a
 * trailing `/` itself, else after the last `/` (0 if there is none, or if
 * it is the second of a leading //). */
static uint filename_pos(const char *s, uint n) {
    if (n > 0 && s[n - 1] == '/') {
        return n - 1;
    }
    int last = -1;
    for (uint i = 0; i < n; i++) {
        if (s[i] == '/') {
            last = (int)i;
        }
    }
    if (last < 0 || (last == 1 && s[0] == '/')) {
        return 0;
    }
    return (uint)last + 1;
}

/* sys::path::filename: the last component of p. A trailing `/` is the
 * component `.` (a/ is . in a), except a root directory alone, which is
 * `/`. */
static struct str file_name(struct str p) {
    int root = root_dir_start(p);
    /* Skip the trailing separators, unless one is the root directory. */
    uint end = p.n;
    while (end > 0 && (root < 0 || (int)end - 1 != root) && p.s[end - 1] == '/') {
        end--;
    }
    if (p.n > 0 && p.s[p.n - 1] == '/' && (root < 0 || (int)end > root + 1)) {
        return str_c(".");
    }
    uint start = filename_pos(p.s, end);
    return str_n(p.s + start, end - start);
}

const char *path_file_name(const char *p, uint n, uint *len) {
    struct str f = file_name(str_n(p, n));
    *len = f.n;
    return f.s;
}

/* sys::path::parent_path: p without its last component and the separators
 * before it, keeping a root directory (/q.h is in /); empty if there is
 * nothing before the last component. */
static struct str parent_path(struct str p) {
    uint end = filename_pos(p.s, p.n);
    int filename_was_sep = end < p.n && p.s[end] == '/';
    int root = root_dir_start(p);
    while (end > 0 && (root < 0 || (int)end > root) && p.s[end - 1] == '/') {
        end--;
    }
    if (root >= 0 && (int)end == root && !filename_was_sep) {
        return str_n(p.s, (uint)root + 1);
    }
    return str_n(p.s, end);
}

/* The directory and the name LLVM's assembler files a `.file N "dir"
 * "name"` under (MCDwarfLineTableHeader::tryGetFile, merlin-as's
 * file_entry): a directory that is the compilation directory is none; a
 * file with no directory then has its name split into parent_path and
 * file_name (include/types.h is types.h in include), unless the name has no
 * parent. An empty name is <stdin>, in no directory. */
static void file_entry(struct str comp_dir, struct str dir, struct str name, struct str *out_dir,
                       struct str *out_name) {
    if (name.n == 0) {
        *out_dir = str_c("");
        *out_name = str_c("<stdin>");
        return;
    }
    if (str_eq(dir, comp_dir)) {
        dir = str_c("");
    }
    if (dir.n == 0) {
        struct str parent = parent_path(name);
        if (parent.n > 0) {
            *out_dir = parent;
            *out_name = file_name(name);
            return;
        }
    }
    *out_dir = dir;
    *out_name = name;
}

/* A path separator: names use `/` (lines_name). */
static int is_sep(uint c) {
    return c == '/';
}

/* Whether a component is a root name: a network name (//net). (A drive,
 * C:, is one on Windows only: not on Camelot, as not for clang on Linux.) */
static int is_root_name(struct str c) {
    return c.n > 2 && is_sep((uchar)c.s[0]) && c.s[1] == c.s[0] && !is_sep((uchar)c.s[2]);
}

/* Whether every byte of c is a separator. */
static int all_seps(struct str c) {
    for (uint i = 0; i < c.n; i++) {
        if (!is_sep((uchar)c.s[i])) {
            return 0;
        }
    }
    return 1;
}

/* Where the next separator at or after `from` is in p (its end if none). */
static uint next_sep(struct str p, uint from) {
    for (uint i = from; i < p.n; i++) {
        if (is_sep((uchar)p.s[i])) {
            return i;
        }
    }
    return p.n;
}

/* The components of p as LLVM's sys::path::begin iterates them (merlin's
 * debug::components): a root name, a root directory `/`, then the names
 * between separators (several in a row are one), and `.` for a trailing
 * separator. They go in out (a heap array of *n, freed by the caller). */
static struct str *components(struct str p, uint *count) {
    struct str *out = NULL;
    uint cap = 0;
    uint n = 0;
    if (p.n == 0) {
        *count = 0;
        return NULL;
    }
    uint first;
    if (p.n > 2 && is_sep((uchar)p.s[0]) && p.s[1] == p.s[0] && !is_sep((uchar)p.s[2])) {
        first = next_sep(p, 2);
    } else if (is_sep((uchar)p.s[0])) {
        first = 1;
    } else {
        first = next_sep(p, 0);
    }
    uint at = 0;
    struct str component = str_n(p.s, first);
    for (;;) {
        out = grow(out, &cap, n + 1, sizeof(struct str));
        out[n++] = component;
        at += component.n;
        if (at == p.n) {
            *count = n;
            return out;
        }
        if (is_sep((uchar)p.s[at])) {
            /* The root directory after a root name. */
            if (is_root_name(component)) {
                component = str_n(p.s + at, 1);
                continue;
            }
            while (at < p.n && is_sep((uchar)p.s[at])) {
                at++;
            }
            if (at == p.n && !str_eq(component, str_c("/"))) {
                at--;
                component = str_c(".");
                continue;
            }
        }
        component = str_n(p.s + at, next_sep(p, at) - at);
    }
}

/* LLVM's sys::path::append of one component, joining with `/`: after a
 * path that ends with a separator, the component without its leading ones;
 * else a separator first, unless the path is empty or the component starts
 * with a separator or is a root name. */
static void append(struct buf *path, struct str c) {
    if (path->len > 0 && is_sep((uchar)path->data[path->len - 1])) {
        uint k = 0;
        while (k < c.n && c.s[k] == '/') {
            k++;
        }
        buf_put(path, c.s + k, c.n - k);
        return;
    }
    if (!(path->len == 0 || (c.n > 0 && is_sep((uchar)c.s[0])) || is_root_name(c))) {
        buf_byte(path, '/');
    }
    buf_put(path, c.s, c.n);
}

/* The directory and the name of the DIFile clang makes for a file named
 * `name` (CGDebugInfo::createFile), comp_dir the compilation directory: a
 * relative name is in comp_dir. An absolute one loses the components it
 * has in common with comp_dir, which are its directory (/src/inc/h.h with
 * /src is inc/h.h in /src), unless they are only a root (/): then it keeps
 * them, and has no directory. */
static void clang_file(struct str comp_dir, struct str name, struct str *dir, struct str *rest) {
    if (!(name.n > 0 && name.s[0] == '/')) {
        *dir = comp_dir;
        *rest = name;
        return;
    }
    uint nfile;
    uint ncur;
    struct str *file = components(name, &nfile);
    struct str *cur = components(comp_dir, &ncur);
    uint common = 0;
    while (common < nfile && common < ncur && str_eq(file[common], cur[common])) {
        common++;
    }
    /* The root's components: a root name, a root directory, or both. */
    uint root = 0;
    if (nfile > 0 && is_root_name(file[0])) {
        root = 1 + (nfile > 1 && all_seps(file[1]) ? 1 : 0);
    } else if (nfile > 0 && all_seps(file[0])) {
        root = 1;
    }
    if (common <= root) {
        *dir = str_c("");
        *rest = name;
    } else {
        struct buf d;
        struct buf r;
        memset(&d, 0, sizeof d);
        memset(&r, 0, sizeof r);
        for (uint i = 0; i < common; i++) {
            append(&d, file[i]);
        }
        for (uint i = common; i < nfile; i++) {
            append(&r, file[i]);
        }
        *dir = str_of_buf(&d);
        *rest = str_of_buf(&r);
    }
    heap_free(file);
    heap_free(cur);
}

/* The name of the main file as clang gives it to the compile unit and to
 * `.file 0` (CGDebugInfo::CreateCompileUnit): its directory (`.` if it has
 * none) and its base name joined, without the ./s that start it. m.c and
 * ./m.c are m.c, sub/../m.c stays itself. */
static struct str root_name(struct str name) {
    struct str dir = parent_path(name);
    if (dir.n == 0) {
        dir = str_c(".");
    }
    struct buf path;
    memset(&path, 0, sizeof path);
    buf_put(&path, dir.s, dir.n);
    append(&path, file_name(name));
    struct str p = str_of_buf(&path);
    /* sys::path::remove_leading_dotslash. */
    while (p.n > 2 && p.s[0] == '.' && p.s[1] == '/') {
        p = str_n(p.s + 2, p.n - 2);
        while (p.n > 0 && p.s[0] == '/') {
            p = str_n(p.s + 1, p.n - 1);
        }
    }
    return p;
}

void quote(struct buf *b, const char *s, uint n) {
    buf_byte(b, '"');
    for (uint i = 0; i < n; i++) {
        uint c = (uchar)s[i];
        if (c == '"') {
            buf_str(b, "\\\"");
        } else if (c == '\\') {
            buf_str(b, "\\\\");
        } else if (c >= 0x20 && c <= 0x7e) {
            buf_byte(b, c);
        } else if (c == 8) {
            buf_str(b, "\\b");
        } else if (c == 12) {
            buf_str(b, "\\f");
        } else if (c == '\n') {
            buf_str(b, "\\n");
        } else if (c == '\r') {
            buf_str(b, "\\r");
        } else if (c == '\t') {
            buf_str(b, "\\t");
        } else {
            /* Any other byte, control or not ASCII, as three octal digits:
             * é, UTF-8 c3 a9, is \303\251. */
            buf_byte(b, '\\');
            buf_byte(b, '0' + (c >> 6));
            buf_byte(b, '0' + (c >> 3 & 7));
            buf_byte(b, '0' + (c & 7));
        }
    }
    buf_byte(b, '"');
}

/* Whether two checksums are the same: both none, or the same bytes. */
static int sum_eq(struct checksum a, struct checksum b) {
    return a.has == b.has && (!a.has || memcmp(a.sum, b.sum, 16) == 0);
}

/* `.file N "dir" "name" md5 0x...`, as clang's assembly printer writes it
 * (printDwarfFileDirective): no directory if it is empty, no checksum if
 * there is none. */
static char *file_line(uint n, struct str dir, struct str name, struct checksum md5sum) {
    struct buf b;
    memset(&b, 0, sizeof b);
    buf_fmt(&b, ".file %u ", n);
    if (dir.n > 0) {
        quote(&b, dir.s, dir.n);
        buf_byte(&b, ' ');
    }
    quote(&b, name.s, name.n);
    if (md5sum.has) {
        char hex[33];
        md5_hex(md5sum.sum, hex);
        buf_fmt(&b, " md5 0x%s", hex);
    }
    char *r = buf_arena(&b);
    buf_free(&b);
    return r;
}

/* ---- DWARF's numbers (DWARF 5, section 7) ---- */

#define TAG_ARRAY_TYPE 0x01
#define TAG_ENUMERATION_TYPE 0x04
#define TAG_FORMAL_PARAMETER 0x05
#define TAG_LABEL 0x0a
#define TAG_LEXICAL_BLOCK 0x0b
#define TAG_MEMBER 0x0d
#define TAG_POINTER_TYPE 0x0f
#define TAG_COMPILE_UNIT 0x11
#define TAG_STRUCTURE_TYPE 0x13
#define TAG_SUBROUTINE_TYPE 0x15
#define TAG_TYPEDEF 0x16
#define TAG_UNION_TYPE 0x17
#define TAG_UNSPECIFIED_PARAMETERS 0x18
#define TAG_SUBRANGE_TYPE 0x21
#define TAG_BASE_TYPE 0x24
#define TAG_CONST_TYPE 0x26
#define TAG_ENUMERATOR 0x28
#define TAG_SUBPROGRAM 0x2e
#define TAG_VARIABLE 0x34
#define TAG_VOLATILE_TYPE 0x35

#define AT_LOCATION 0x02
#define AT_NAME 0x03
#define AT_BYTE_SIZE 0x0b
#define AT_STMT_LIST 0x10
#define AT_LOW_PC 0x11
#define AT_HIGH_PC 0x12
#define AT_LANGUAGE 0x13
#define AT_COMP_DIR 0x1b
#define AT_CONST_VALUE 0x1c
#define AT_PRODUCER 0x25
#define AT_PROTOTYPED 0x27
#define AT_COUNT 0x37
#define AT_DATA_MEMBER_LOCATION 0x38
#define AT_DECL_COLUMN 0x39
#define AT_DECL_FILE 0x3a
#define AT_DECL_LINE 0x3b
#define AT_DECLARATION 0x3c
#define AT_ENCODING 0x3e
#define AT_EXTERNAL 0x3f
#define AT_FRAME_BASE 0x40
#define AT_TYPE 0x49
#define AT_STR_OFFSETS_BASE 0x72
#define AT_ADDR_BASE 0x73

#define FORM_DATA2 0x05
#define FORM_DATA4 0x06
#define FORM_DATA8 0x07
#define FORM_DATA1 0x0b
#define FORM_SDATA 0x0d
#define FORM_UDATA 0x0f
#define FORM_REF4 0x13
#define FORM_SEC_OFFSET 0x17
#define FORM_EXPRLOC 0x18
#define FORM_FLAG_PRESENT 0x19
#define FORM_ADDRX 0x1b
#define FORM_STRX1 0x25
#define FORM_STRX2 0x26
#define FORM_STRX3 0x27
#define FORM_STRX4 0x28

#define ATE_BOOLEAN 0x02
#define ATE_SIGNED 0x05
#define ATE_SIGNED_CHAR 0x06
#define ATE_UNSIGNED 0x07
#define ATE_UNSIGNED_CHAR 0x08

#define OP_ADDRX 0xa1
/* DW_OP_reg0 + 8: s0, the frame pointer. */
#define OP_REG8 0x58
#define OP_FBREG 0x91

#define LANG_C99 0x0c
#define UT_COMPILE 1

/* v in unsigned LEB128, in out (at most 10 bytes); how many bytes. */
static uint uleb(uint64_t v, uchar *out) {
    uint n = 0;
    for (;;) {
        uchar b = (uchar)(v & 0x7f);
        v >>= 7;
        if (v == 0) {
            out[n++] = b;
            return n;
        }
        out[n++] = b | 0x80;
    }
}

/* v in signed LEB128, in out (at most 10 bytes); how many bytes. */
static uint sleb(int64_t v, uchar *out) {
    uint n = 0;
    for (;;) {
        uchar b = (uchar)(v & 0x7f);
        /* An arithmetic shift, as Rust's >> of an i64: what C leaves to
         * the compiler for a negative value, done by hand. */
        v = v < 0 ? ~(~v >> 7) : v >> 7;
        if ((v == 0 && (b & 0x40) == 0) || (v == -1 && (b & 0x40) != 0)) {
            out[n++] = b;
            return n;
        }
        out[n++] = b | 0x80;
    }
}

static uint uleb_len(uint64_t v) {
    uchar b[10];
    return uleb(v, b);
}

/* ---- What the code generator records ---- */

/* A parameter or a variable: its name, where it is declared, its type (an
 * index in the type table, -1 for void) and where it lives: in the frame
 * at an offset from s0, or at a symbol (a global, a static local). */
struct dvar {
    const char *name;
    uint file;
    uint line;
    int ty;
    /* NULL: in the frame, at offset. */
    const char *sym;
    int offset;
};

/* A goto label: its name, where it is and its code's label. */
struct dlabel {
    const char *name;
    uint file;
    uint line;
    uint col;
    const char *sym;
};

/* A block's variables in the frame and the blocks inside it that declare
 * some, its static variables and those of the blocks inside it, and the
 * labels around its code (merlin's Scope). */
struct dscope {
    const char *begin;
    const char *end;
    /* struct dvar *: in the frame. */
    struct vec vars;
    /* struct dlabel *: the goto labels in it, after its variables, as clang
     * has them. */
    struct vec labels;
    /* struct dscope *. */
    struct vec blocks;
    /* No lexical block of its own: a block without variables in the frame
     * whose labels still are, in the block around it, where it is. */
    int transparent;
    /* struct dvar *: clang describes every static local of a function in
     * the function's entry, after its variables in the frame and its
     * blocks, in the order they are declared, whatever block declares
     * them: a block's statics go up to the block around it as the block
     * ends, and from there to the function. */
    struct vec statics;
};

/* Something the code of a function or the initializer of a global does
 * that clang's front end does too as it emits them, in the order it does
 * it, for order() to work out the order of the unit's entries as clang's
 * (merlin's Event). */
enum {
    /* A local variable is described: its type, and the .file number of its
     * declaration. */
    EV_LOCAL,
    /* A static local, at its symbol, of its type. */
    EV_STATIC,
    /* An explicit cast to this type, which clang keeps in the unit
     * (DIBuilder::retainType). */
    EV_CAST,
    /* String literal `index` of the unit's. */
    EV_LITERAL,
    /* An enumeration constant of this enum, which clang describes as its
     * code uses the constant. */
    EV_ENUM
};

struct event {
    int kind;
    int ty;
    uint file;
    const char *sym;
    uint index;
};

/* A function, for its DW_TAG_subprogram. */
struct dfunc {
    const char *name;
    /* The label just past its code. */
    const char *end;
    /* Its .file number and line. */
    uint file;
    uint line;
    /* Where it starts (its name) and its body ends. */
    struct pos start;
    struct pos body_end;
    int external;
    /* Declared with a prototype (not `()`). */
    int prototyped;
    int ret;
    /* struct event *: what its code did, in order. */
    struct vec events;
    /* struct dvar *. */
    struct vec params;
    int variadic;
    /* The variables of its body, and its blocks. */
    struct dscope *body;
};

/* A global variable the file defines. */
struct dglobal {
    struct dvar *var;
    int external;
    /* struct event *: what its initializer did (the string literals it
     * holds, the casts it keeps). */
    struct vec events;
};

/* A string literal, which clang describes as a variable without a name:
 * its label, its type (char [N], its NUL counted) and where it is. */
struct dliteral {
    const char *label;
    int ty;
    uint file;
    uint line;
};

/* A type in the table (merlin's TypeNode): what its entry says. Each is
 * described once, and variables, members and other types refer to it by
 * its index. */
enum {
    DT_BASE,
    /* The type the counts of array subranges have, as clang names it
     * (__ARRAY_SIZE_TYPE__). */
    DT_ARRAY_SIZE,
    DT_PTR,
    DT_ARRAY,
    DT_RECORD,
    DT_QUAL,
    DT_FUNC,
    DT_TYPEDEF,
    DT_ENUM
};

/* A member of a record's entry: its name, type, offset, and .file number
 * and line. */
struct dmember {
    const char *name;
    int ty;
    uint offset;
    uint file;
    uint line;
};

struct dtype {
    int kind;
    /* DT_BASE: which integer type. */
    int ik;
    /* DT_PTR: what it points to; DT_QUAL: what it qualifies; DT_ARRAY: the
     * element type; DT_FUNC: what it returns; DT_TYPEDEF: what it stands
     * for; DT_ENUM: its integer type. -1 for void. */
    int to;
    /* DT_QUAL: const (else volatile). */
    int konst;
    /* DT_ARRAY: each dimension's count (-1: of unknown size), outermost
     * first. */
    int64_t *dims;
    uint ndims;
    /* DT_RECORD: the record, its members (filled in after the entry is in
     * the table, as a member may point back to the record). */
    struct record *rec;
    struct dmember *members;
    uint nmembers;
    /* DT_RECORD, DT_TYPEDEF, DT_ENUM: the .file number and the line (pos,
     * for a typedef and an enum). */
    uint file;
    uint line;
    struct pos pos;
    /* DT_FUNC: what it takes, whether `...` follows, and whether it has a
     * prototype (int () has none). */
    int *params;
    uint nparams;
    int variadic;
    int prototyped;
    /* DT_TYPEDEF: its name. */
    const char *name;
    /* DT_ENUM: the enum (its tag and constants). */
    struct enum_type *en;
};

/* A file of the line table as its .file names it (merlin's FileEntry): the
 * directory (none if empty) and the name LLVM's assembler files it under,
 * its MD5, and the directory and name tryGetFile looks it up by (its
 * SourceIdMap): files with the same are one file of the table. */
struct dfile {
    struct str dir;
    struct str name;
    struct checksum md5;
    struct str key_dir;
    struct str key_name;
};

/* A function or a global, in the order clang's front end emits them. */
struct item_ref {
    int func;
    uint index;
};

/* An entry of the unit (order.rs's Child). */
enum { CH_TYPE, CH_FUNCTION, CH_GLOBAL, CH_LITERAL };
struct child {
    int kind;
    uint index;
};

/* An entry of a function clang makes after its variables and blocks: a
 * type it defines, or a static local, by symbol (order.rs's Local). */
struct local_entry {
    int is_static;
    int ty;
    const char *sym;
};

/* A growable array of ints. */
struct ints {
    int *v;
    uint n;
    uint cap;
};

static void ints_push(struct ints *l, int x) {
    l->v = grow(l->v, &l->cap, l->n + 1, sizeof(int));
    l->v[l->n++] = x;
}

static int ints_has(struct ints *l, int x) {
    for (uint i = 0; i < l->n; i++) {
        if (l->v[i] == x) {
            return 1;
        }
    }
    return 0;
}

/* Where each entry goes, and the files they name in the order LLVM numbers
 * them (order.rs's Order). */
struct order {
    /* The unit's children. */
    struct child *unit;
    uint nunit;
    uint capunit;
    /* Each function's types and static locals, after its blocks. */
    struct local_entry **locals;
    uint *nlocals;
    uint *caplocals;
    /* Each function's types made as the module begins (phase 1), its
     * first children, before its parameters. */
    struct ints *first;
    /* Whether the module begins by describing something (phase 1 has
     * entries): LLVM makes the unit then. */
    int begins;
    /* The files the entries of phase 1 are declared in, in the order LLVM
     * needs them (numbers given as the code was generated; some more than
     * once, file 0 among them). */
    struct ints begin_files;
    /* The same, for the entries LLVM makes as the module ends (phases 3 to
     * 6). */
    struct ints end_files;
};

/* The anonymous types records describe: each one's record and member
 * (nested_types). */
struct nested {
    int ty;
    int record;
    uint member;
};

/* The debug information of one translation unit as it is generated
 * (merlin's Lines). */
struct lines {
    /* The compilation directory as given, which `.file 0` writes, and with
     * `/` separators, for working out the files' directories. */
    const char *comp_dir_given;
    struct str comp_dir;
    /* The preprocessor's file names, with `/` separators, and their MD5s,
     * by pos.file. */
    struct str *names;
    struct checksum *md5;
    uint nfiles;
    /* The main file's name as `.file 0` gives it (root_name). */
    struct str root;
    /* pos.file -> .file number, for the files given one so far (-1 for
     * none yet); the last one is for a file past the preprocessor's. */
    int *numbers;
    /* The files numbered so far, file n at n - 1. */
    struct dfile *entries;
    uint nentries;
    uint capentries;
    /* The final .file numbers of the numbers given as the code was
     * generated (debug_renumber), -1 for none; nrenumbered of them. */
    int *renumbered;
    uint nrenumbered;
    /* char *: the .file directives of the files only the entries LLVM
     * makes as the module ends name, written with the debug sections. */
    struct vec late_files;
    /* The order of the entries, worked out by debug_renumber, which needs
     * it to number the files: with the anonymous types records describe
     * and the types functions define. */
    struct order *order;
    struct nested *nested;
    uint nnested;
    /* How many blocks have labels. */
    uint blocks;
    /* struct dfunc *, struct dglobal *, struct dliteral *. */
    struct vec functions;
    struct vec globals;
    struct vec literals;
    /* The functions and the globals, in the order clang's front end emits
     * them, which code generation follows. */
    struct item_ref *items;
    uint nitems;
    uint capitems;
    /* The functions in the order of their code, which LLVM describes them
     * in (indices in functions). */
    uint *code;
    uint ncode;
    /* struct event *: those of the function or global being generated. */
    struct vec events;
    /* struct dtype *, and each one's key (type_key) -> its index + 1. */
    struct vec types;
    struct map type_ids;
};

struct lines *debug_new(const char *comp_dir, const char **names, const uint *lens, struct checksum *sums,
                        uint nfiles) {
    struct lines *l = arena_alloc(sizeof *l);
    memset(l, 0, sizeof *l);
    l->comp_dir_given = comp_dir;
    char *slashed = arena_strdup(comp_dir);
    for (char *p = slashed; *p; p++) {
        if (*p == '\\') {
            *p = '/';
        }
    }
    l->comp_dir = str_c(slashed);
    /* The file's name with `/` separators. */
    l->names = arena_alloc((nfiles + 1) * sizeof(struct str));
    l->md5 = arena_alloc((nfiles + 1) * sizeof(struct checksum));
    for (uint i = 0; i < nfiles; i++) {
        char *n = arena_strndup(names[i], lens[i]);
        for (uint k = 0; k < lens[i]; k++) {
            if (n[k] == '\\') {
                n[k] = '/';
            }
        }
        l->names[i] = str_n(n, lens[i]);
        l->md5[i] = sums[i];
    }
    l->nfiles = nfiles;
    l->numbers = arena_alloc((nfiles + 1) * sizeof(int));
    for (uint i = 0; i <= nfiles; i++) {
        l->numbers[i] = -1;
    }
    l->root = root_name(nfiles > 0 ? l->names[0] : str_c("<unknown>"));
    return l;
}

/* The file's name with `/` separators. */
static struct str lines_name(struct lines *l, uint file) {
    return file < l->nfiles ? l->names[file] : str_c("<unknown>");
}

/* The file's MD5 checksum. */
static struct checksum lines_checksum(struct lines *l, uint file) {
    struct checksum none;
    memset(&none, 0, sizeof none);
    return file < l->nfiles ? l->md5[file] : none;
}

char *debug_file0(struct lines *l) {
    return file_line(0, str_c(l->comp_dir_given), l->root, lines_checksum(l, 0));
}

/* Whether a file of this name and checksum is the main file, as `.file 0`
 * names it (LLVM's isRootFile). */
static int is_root(struct lines *l, struct str name, struct checksum sum) {
    return str_eq(name, l->root) && sum_eq(sum, lines_checksum(l, 0));
}

/* The .file number of file, given the first time it is asked for:
 * debug_renumber gives the final ones (merlin's Lines::number). 0 for the
 * main file: one named as `.file 0` names it, with its checksum, which the
 * file being compiled is unless its name starts with ./ or otherwise
 * differs from root_name's (clang then gives its code a file of its own). */
static uint number(struct lines *l, uint file) {
    uint slot = file < l->nfiles ? file : l->nfiles;
    if (l->numbers[slot] >= 0) {
        return (uint)l->numbers[slot];
    }
    /* The DIFile's directory and name, then the ones tryGetFile looks up:
     * the compilation directory is no directory, and a file with no name is
     * <stdin>. */
    struct str di_dir;
    struct str di_name;
    clang_file(l->comp_dir, lines_name(l, file), &di_dir, &di_name);
    struct str dir = di_dir;
    struct str name = di_name;
    if (di_name.n == 0) {
        dir = str_c("");
        name = str_c("<stdin>");
    } else if (str_eq(di_dir, l->comp_dir)) {
        dir = str_c("");
    }
    struct checksum sum = lines_checksum(l, file);
    if (is_root(l, name, sum)) {
        l->numbers[slot] = 0;
        return 0;
    }
    /* A file already numbered, by its directory and name as LLVM's
     * assembler looks them up and its checksum: clang's DIFile, of which a
     * header read twice has one. Two that differ only in their checksums
     * (a header, and a #line naming it, which has none) have numbers of
     * their own here, made one by debug_renumber. */
    for (uint i = 0; i < l->nentries; i++) {
        struct dfile *f = &l->entries[i];
        if (str_eq(f->key_dir, dir) && str_eq(f->key_name, name) && sum_eq(f->md5, sum)) {
            l->numbers[slot] = (int)i + 1;
            return i + 1;
        }
    }
    /* The directory and name tryGetFile files it under, which clang's
     * .file writes. Unless the assembler, reading them, would file them
     * differently: the split lost something (clang's own -S output is wrong
     * there), and the .file names the file as its DIFile does, which the
     * assembler files as clang -c files it. ./m.c is m.c in `.`, which an
     * assembler would take for the main file m.c; with the compilation
     * directory `.`, ./h.h is h.h in `.`, which an assembler would file in
     * the compilation directory itself. */
    struct str d;
    struct str f;
    file_entry(l->comp_dir, dir, name, &d, &f);
    struct str d2;
    struct str f2;
    file_entry(l->comp_dir, d, f, &d2, &f2);
    if (is_root(l, f, sum) || !str_eq(d2, d) || !str_eq(f2, f)) {
        d = str_eq(di_dir, l->comp_dir) ? str_c(l->comp_dir_given) : di_dir;
        f = di_name;
    }
    l->entries = grow(l->entries, &l->capentries, l->nentries + 1, sizeof(struct dfile));
    struct dfile *e = &l->entries[l->nentries++];
    e->dir = d;
    e->name = f;
    e->md5 = sum;
    e->key_dir = dir;
    e->key_name = name;
    l->numbers[slot] = (int)l->nentries;
    return l->nentries;
}

/* The .file number of a declaration's file. */
static uint decl_file(struct lines *l, struct pos pos) {
    return number(l, pos.file);
}

/* The .file directive of file n (as the code was generated) under its final
 * number `to`. */
static char *file_directive(struct lines *l, uint n, uint to) {
    struct dfile *f = &l->entries[n - 1];
    return file_line(to, f->dir, f->name, f->md5);
}

void debug_block_labels(struct lines *l, const char **begin, const char **end) {
    l->blocks++;
    *begin = fmt(".Ldbg.block.%u", l->blocks);
    *end = fmt(".Ldbg.block_end.%u", l->blocks);
}

char *debug_loc(struct lines *l, struct pos pos, int column, int prologue_end) {
    uint n = number(l, pos.file);
    uint col = column >= 0 ? (uint)column : pos.col;
    return fmt(".loc %u %u %u%s", n, pos.line, col, prologue_end ? " prologue_end" : "");
}

/* ---- Types ---- */

static int type_of(struct lines *l, struct type *ty, struct spelling *spelled);

/* The index of the type whose key is key (type_ids), or -1. */
static int find_type(struct lines *l, const char *key) {
    void *v = map_get(&l->type_ids, key);
    return v == NULL ? -1 : (int)((uintptr_t)v - 1);
}

/* The index of the type whose key is key, added to the table if it is new
 * (node, which the caller made). */
static int add_type(struct lines *l, const char *key, struct dtype *node) {
    int id = find_type(l, key);
    if (id >= 0) {
        return id;
    }
    vec_push(&l->types, node);
    map_put(&l->type_ids, key, (void *)(uintptr_t)l->types.n);
    return (int)l->types.n - 1;
}

static struct dtype *new_type(int kind) {
    struct dtype *t = arena_alloc(sizeof *t);
    memset(t, 0, sizeof *t);
    t->kind = kind;
    t->to = -1;
    return t;
}

/* What makes two types the same, to describe each once (merlin's TypeKey),
 * as a string: records, typedefs and enums are themselves (their address),
 * the rest their parts. */
static int base(struct lines *l, int k) {
    const char *key = fmt("B%d", k);
    int id = find_type(l, key);
    if (id >= 0) {
        return id;
    }
    struct dtype *t = new_type(DT_BASE);
    t->ik = k;
    return add_type(l, key, t);
}

static int pointer(struct lines *l, int to) {
    const char *key = fmt("P%d", to);
    int id = find_type(l, key);
    if (id >= 0) {
        return id;
    }
    struct dtype *t = new_type(DT_PTR);
    t->to = to;
    return add_type(l, key, t);
}

static int array_size(struct lines *l) {
    int id = find_type(l, "S");
    if (id >= 0) {
        return id;
    }
    return add_type(l, "S", new_type(DT_ARRAY_SIZE));
}

/* Type id with qualifiers q: const volatile T is const of volatile of T,
 * as clang has it. */
static int qualify(struct lines *l, int id, uint q) {
    if (q & Q_VOLATILE) {
        const char *key = fmt("Q0,%d", id);
        if (find_type(l, key) < 0) {
            struct dtype *t = new_type(DT_QUAL);
            t->to = id;
            add_type(l, key, t);
        }
        id = find_type(l, key);
    }
    if (q & Q_CONST) {
        const char *key = fmt("Q1,%d", id);
        if (find_type(l, key) < 0) {
            struct dtype *t = new_type(DT_QUAL);
            t->konst = 1;
            t->to = id;
            add_type(l, key, t);
        }
        id = find_type(l, key);
    }
    return id;
}

/* A struct or union: in the table before its members are, so that a member
 * that points back to it finds it. */
static int record(struct lines *l, struct record *r) {
    const char *key = fmt("R%llu", (uint64_t)(uintptr_t)r);
    int id = find_type(l, key);
    if (id >= 0) {
        return id;
    }
    /* Where it is defined, as clang has it, which may come after where it
     * was first declared (`struct s;` in a header). */
    struct pos pos = r->body != NULL ? r->body->pos : r->pos;
    uint file = decl_file(l, pos);
    struct dtype *t = new_type(DT_RECORD);
    t->rec = r;
    t->file = file;
    t->line = pos.line;
    id = add_type(l, key, t);
    if (r->body != NULL) {
        uint n = r->body->nmembers;
        struct dmember *ms = arena_alloc((n + 1) * sizeof(struct dmember));
        for (uint i = 0; i < n; i++) {
            struct member *m = &r->body->members[i];
            ms[i].name = m->name;
            ms[i].ty = type_of(l, m->ty, m->spelled);
            ms[i].offset = m->offset;
            ms[i].file = decl_file(l, m->pos);
            ms[i].line = m->pos.line;
        }
        t->members = ms;
        t->nmembers = n;
    }
    return id;
}

/* A typedef name or an enum. */
static int spelling(struct lines *l, struct spelling *s) {
    const void *at = s->kind == SP_TYPEDEF ? (void *)s->td : (void *)s->en;
    const char *key = fmt("T%llu", (uint64_t)(uintptr_t)at);
    int id = find_type(l, key);
    if (id >= 0) {
        return id;
    }
    if (s->kind == SP_TYPEDEF) {
        struct typedef_name *td = s->td;
        int of = type_of(l, td->ty, td->spelled);
        uint file = decl_file(l, td->pos);
        struct dtype *t = new_type(DT_TYPEDEF);
        t->name = td->name;
        t->to = of;
        t->file = file;
        t->pos = td->pos;
        return add_type(l, key, t);
    }
    struct enum_type *e = s->en;
    int of = base(l, ty_int_kind(e->ty));
    uint file = decl_file(l, e->pos);
    struct dtype *t = new_type(DT_ENUM);
    t->en = e;
    t->to = of;
    t->file = file;
    t->pos = e->pos;
    return add_type(l, key, t);
}

/* Whether spelled spells an array typedef and t is that array, qualified or
 * not. */
static int from_array_typedef(struct spelling *spelled, struct type *t) {
    if (spelled == NULL) {
        return 0;
    }
    struct type *st = spelling_ty(spelled);
    return ty_is_array(st) && ty_quals_added_to(t, st) >= 0;
}

/* The index in the type table of ty, as spelled spelled its base (-1 for
 * void). The spelled type is found again inside ty: where what is left of
 * ty is the spelled type, the typedef or the enum stands for it (Node *p is
 * a pointer to the typedef Node). */
static int type_of(struct lines *l, struct type *ty, struct spelling *spelled) {
    if (spelled != NULL) {
        struct type *st = spelling_ty(spelled);
        if (ty_eq(ty, st)) {
            return spelling(l, spelled);
        }
        /* A typedef of an array, qualified (const row): cc1's type has the
         * qualifiers on the elements, clang's describes the typedef
         * qualified, const of row. */
        if (ty_is_array(st)) {
            int q = ty_quals_added_to(ty, st);
            if (q >= 0) {
                int id = spelling(l, spelled);
                return qualify(l, id, (uint)q);
            }
        }
    }
    switch (ty->kind) {
    case TY_VOID:
        return -1;
    case TY_INT:
        return base(l, ty->ik);
    case TY_PTR:
        return pointer(l, type_of(l, ty->to, spelled));
    case TY_ARRAY: {
        /* int m[2][3] is one array type with two subranges, as clang
         * describes it, unless an inner array is a typedef's. */
        uint ndims = 0;
        struct type *t = ty;
        while (t->kind == TY_ARRAY && !from_array_typedef(spelled, t)) {
            ndims++;
            t = t->to;
        }
        /* Each dimension's count, -1 for none. */
        int64_t *dims = arena_alloc((ndims + 1) * sizeof(int64_t));
        t = ty;
        for (uint i = 0; i < ndims; i++) {
            dims[i] = t->has_len ? (int64_t)t->len : -1;
            t = t->to;
        }
        int elem = type_of(l, t, spelled);
        if (elem < 0) {
            return -1;
        }
        struct buf key;
        memset(&key, 0, sizeof key);
        buf_fmt(&key, "A%d", elem);
        for (uint i = 0; i < ndims; i++) {
            if (dims[i] < 0) {
                buf_str(&key, ",-");
            } else {
                buf_fmt(&key, ",%llu", (uint64_t)dims[i]);
            }
        }
        char *k = buf_arena(&key);
        buf_free(&key);
        int id = find_type(l, k);
        if (id < 0) {
            struct dtype *a = new_type(DT_ARRAY);
            a->to = elem;
            a->dims = dims;
            a->ndims = ndims;
            id = add_type(l, k, a);
        }
        return id;
    }
    case TY_RECORD:
        return record(l, ty->rec);
    case TY_QUAL:
        return qualify(l, type_of(l, ty->to, spelled), ty->quals);
    default: {
        struct functype *f = ty->fn;
        int ret = type_of(l, f->ret, spelled);
        /* Each parameter as its declaration has it (int (*read)(const
         * pagetable_t)), as clang describes them. */
        int *params = arena_alloc((f->nparams + 1) * sizeof(int));
        for (uint i = 0; i < f->nparams; i++) {
            if (i < f->nspelled) {
                params[i] = type_of(l, f->spelled[i].ty, f->spelled[i].spelled);
            } else {
                params[i] = type_of(l, f->params[i], NULL);
            }
        }
        /* int () says nothing of its parameters, which clang describes
         * apart from int (void). */
        int prototyped = !(f->unprototyped && f->nparams == 0 && !f->variadic);
        struct buf key;
        memset(&key, 0, sizeof key);
        buf_fmt(&key, "F%d/%d/%d", ret, f->variadic ? 1 : 0, prototyped);
        for (uint i = 0; i < f->nparams; i++) {
            buf_fmt(&key, ",%d", params[i]);
        }
        char *k = buf_arena(&key);
        buf_free(&key);
        int id = find_type(l, k);
        if (id < 0) {
            struct dtype *t = new_type(DT_FUNC);
            t->to = ret;
            t->params = params;
            t->nparams = f->nparams;
            t->variadic = f->variadic ? 1 : 0;
            t->prototyped = prototyped;
            id = add_type(l, k, t);
        }
        return id;
    }
    }
}

/* ---- Variables and functions ---- */

struct dvar *debug_var(struct lines *l, const char *name, struct type *ty, struct spelling *spelled, struct pos pos,
                       const char *sym, int offset) {
    int t = type_of(l, ty, spelled);
    uint file = decl_file(l, pos);
    struct dvar *v = arena_alloc(sizeof *v);
    v->name = name;
    v->file = file;
    v->line = pos.line;
    v->ty = t;
    v->sym = sym;
    v->offset = offset;
    return v;
}

static struct event *event(struct lines *l, int kind) {
    struct event *e = arena_alloc(sizeof *e);
    memset(e, 0, sizeof *e);
    e->kind = kind;
    e->ty = -1;
    vec_push(&l->events, e);
    return e;
}

void debug_global(struct lines *l, const char *name, const char *sym, struct type *ty, struct spelling *spelled,
                  struct pos pos, int external) {
    struct dglobal *g = arena_alloc(sizeof *g);
    g->var = debug_var(l, name, ty, spelled, pos, sym, 0);
    g->external = external;
    g->events = vec_take(&l->events);
    l->items = grow(l->items, &l->capitems, l->nitems + 1, sizeof(struct item_ref));
    l->items[l->nitems].func = 0;
    l->items[l->nitems].index = l->globals.n;
    l->nitems++;
    vec_push(&l->globals, g);
}

void debug_declared(struct lines *l, struct dvar *v) {
    if (v->sym == NULL) {
        struct event *e = event(l, EV_LOCAL);
        e->ty = v->ty;
        e->file = v->file;
    } else {
        struct event *e = event(l, EV_STATIC);
        e->sym = v->sym;
        e->ty = v->ty;
    }
}

void debug_cast(struct lines *l, struct type *ty, struct spelling *spelled) {
    int t = type_of(l, ty, spelled);
    /* A cast to void has no type to keep. */
    if (t >= 0) {
        event(l, EV_CAST)->ty = t;
    }
}

void debug_enumerator(struct lines *l, struct spelling *of) {
    if (of != NULL) {
        int t = spelling(l, of);
        event(l, EV_ENUM)->ty = t;
    }
}

struct dlabel *debug_label(struct lines *l, const char *name, struct pos pos, const char *sym) {
    struct dlabel *d = arena_alloc(sizeof *d);
    d->file = decl_file(l, pos);
    d->name = name;
    d->line = pos.line;
    d->col = pos.col;
    d->sym = sym;
    return d;
}

void debug_literal(struct lines *l, const char *label, uint len, struct pos pos) {
    struct type *array = ty_array(ty_int(IK_CHAR), 1, len);
    int ty = type_of(l, array, NULL);
    uint file = decl_file(l, pos);
    event(l, EV_LITERAL)->index = l->literals.n;
    struct dliteral *lit = arena_alloc(sizeof *lit);
    lit->label = label;
    lit->ty = ty;
    lit->file = file;
    lit->line = pos.line;
    vec_push(&l->literals, lit);
}

void debug_code_order(struct lines *l, const char **names, uint n) {
    struct map by_name;
    memset(&by_name, 0, sizeof by_name);
    for (uint i = l->functions.n; i > 0; i--) {
        struct dfunc *f = l->functions.v[i - 1];
        map_put(&by_name, f->name, (void *)(uintptr_t)i);
    }
    l->code = arena_alloc((n + 1) * sizeof(uint));
    l->ncode = 0;
    for (uint i = 0; i < n; i++) {
        void *at = map_get(&by_name, names[i]);
        if (at != NULL) {
            l->code[l->ncode++] = (uint)((uintptr_t)at - 1);
        }
    }
    heap_free(by_name.keys);
    heap_free(by_name.values);
}

void debug_function(struct lines *l, struct debug_function *df) {
    struct dfunc *f = arena_alloc(sizeof *f);
    f->file = number(l, df->start.file);
    f->ret = type_of(l, df->ret, df->ret_spelled);
    f->name = df->name;
    f->end = df->end;
    f->line = df->start.line;
    f->start = df->start;
    f->body_end = df->body_end;
    f->external = df->external;
    f->prototyped = df->prototyped;
    f->events = vec_take(&l->events);
    f->params = df->params;
    f->variadic = df->variadic;
    f->body = df->body;
    l->items = grow(l->items, &l->capitems, l->nitems + 1, sizeof(struct item_ref));
    l->items[l->nitems].func = 1;
    l->items[l->nitems].index = l->functions.n;
    l->nitems++;
    vec_push(&l->functions, f);
}

/* ---- Scopes ---- */

struct dscope *debug_scope(const char *begin, const char *end) {
    struct dscope *s = arena_alloc(sizeof *s);
    memset(s, 0, sizeof *s);
    s->begin = begin;
    s->end = end;
    return s;
}

void debug_scope_label(struct dscope *s, struct dlabel *label) {
    vec_push(&s->labels, label);
}

void debug_scope_var(struct dscope *s, struct dvar *v) {
    vec_push(v->sym == NULL ? &s->vars : &s->statics, v);
}

void debug_scope_block(struct dscope *s, struct dscope *inner) {
    vec_extend(&s->statics, &inner->statics);
    inner->statics.n = 0;
    if (inner->vars.n == 0 && inner->labels.n == 0) {
        /* An entry with nothing in it says nothing: its blocks come here. */
        vec_extend(&s->blocks, &inner->blocks);
    } else {
        inner->transparent = inner->vars.n == 0;
        vec_push(&s->blocks, inner);
    }
}

/* The files of the scope's goto labels and its blocks', in order. */
static void label_files(struct dscope *s, struct ints *out) {
    for (uint i = 0; i < s->labels.n; i++) {
        ints_push(out, (int)((struct dlabel *)s->labels.v[i])->file);
    }
    for (uint i = 0; i < s->blocks.n; i++) {
        label_files(s->blocks.v[i], out);
    }
}

/* ---- The types records and functions define ---- */

/* The anonymous structs, unions and enums that a record's member is of, or
 * points to or is an array of: each one's record and member (the first that
 * reaches it), which clang describes it in, after the member. A C type with
 * no name can only be used where it is defined. */
static void nested_types(struct lines *l) {
    uint cap = 0;
    for (uint id = 0; id < l->types.n; id++) {
        struct dtype *t = l->types.v[id];
        if (t->kind != DT_RECORD) {
            continue;
        }
        for (uint i = 0; i < t->nmembers; i++) {
            int b = t->members[i].ty;
            /* Through pointers, arrays and qualifiers (typedefs have
             * names). */
            while (b >= 0) {
                struct dtype *bt = l->types.v[b];
                if (bt->kind != DT_PTR && bt->kind != DT_QUAL && bt->kind != DT_ARRAY) {
                    break;
                }
                b = bt->to;
            }
            if (b < 0) {
                continue;
            }
            struct dtype *bt = l->types.v[b];
            int anonymous = (bt->kind == DT_RECORD && bt->rec->tag == NULL) || (bt->kind == DT_ENUM && bt->en->tag == NULL);
            int seen = 0;
            for (uint k = 0; k < l->nnested; k++) {
                seen |= l->nested[k].ty == b;
            }
            if (anonymous && b != (int)id && !seen) {
                l->nested = grow(l->nested, &cap, l->nnested + 1, sizeof(struct nested));
                l->nested[l->nnested].ty = b;
                l->nested[l->nnested].record = (int)id;
                l->nested[l->nnested].member = i;
                l->nnested++;
            }
        }
    }
}

/* Whether position p lies in [start, end] of start's file. */
static int inside(struct pos p, struct pos start, struct pos end) {
    if (p.file != start.file) {
        return 0;
    }
    int after_start = start.line < p.line || (start.line == p.line && start.col <= p.col);
    int before_end = p.line < end.line || (p.line == end.line && p.col <= end.col);
    return after_start && before_end;
}

/* The structs, unions, enums and typedefs defined inside a function's body
 * (where they are declared lies inside its code's), but those nested_types
 * puts in records: each one's type, in local_ty, and function, in local_fn
 * (-1 for none), by type. clang describes them in the function's entry. */
static void local_types(struct lines *l, int *local_fn, struct ints *local) {
    for (uint id = 0; id < l->types.n; id++) {
        local_fn[id] = -1;
        struct dtype *t = l->types.v[id];
        struct pos pos;
        if (t->kind == DT_RECORD) {
            pos = t->rec->body != NULL ? t->rec->body->pos : t->rec->pos;
        } else if (t->kind == DT_TYPEDEF || t->kind == DT_ENUM) {
            pos = t->pos;
        } else {
            continue;
        }
        for (uint f = 0; f < l->functions.n; f++) {
            struct dfunc *fn = l->functions.v[f];
            if (inside(pos, fn->start, fn->body_end)) {
                int nested = 0;
                for (uint k = 0; k < l->nnested; k++) {
                    nested |= l->nested[k].ty == (int)id;
                }
                if (!nested) {
                    local_fn[id] = (int)f;
                    ints_push(local, (int)id);
                }
                break;
            }
        }
    }
}

/* ---- The order of the entries (merlin's debug/order.rs) ----
 *
 * LLVM writes a unit's entries in the order it makes them, and it makes
 * them in phases:
 *
 * 1. as the module begins, the enums (the unit's `enums` list) and then the
 *    retained types (the types of explicit casts, which clang keeps); but
 *    not a type a function defines, which clang keeps in the function's
 *    retainedNodes instead, made in phase 4. A retained type of the unit
 *    that refers to a function's type makes that type here, and with it,
 *    in its place among the unit's children, the function's entry, which
 *    is the type's context: an entry without attributes yet, whose first
 *    children are the types made so;
 * 2. as each function's code is done, its DW_TAG_subprogram (unless phase
 *    1 made it), in the order of the code, with its variables and blocks
 *    but none of their types;
 * 3. as the module ends, the unit's globals (string literals among them);
 * 4. then each function's static locals and the types it defines, inside
 *    its entry;
 * 5. then each function's return type;
 * 6. then the types of each function's parameters and locals, in the order
 *    they are declared.
 *
 * Phases 4 to 6 go through the functions in the order of their code.
 * Making an entry makes the types it refers to that are not made yet,
 * depth first, each one added before those it refers to (a pointer before
 * what it points to, a struct before its members' types, an array before
 * its element type and then __ARRAY_SIZE_TYPE__). A type is added to the
 * unit unless it belongs elsewhere: an anonymous type a member is of to its
 * record, a type a function defines to the function.
 *
 * The lists of phases 1, 3 and 4 are clang's front end's, in the order it
 * makes them: the order the code generator emitted the file's functions and
 * globals in (`items`), each one's events in order. The order LLVM makes
 * the entries in is also the order it numbers the files they are declared
 * in, each the first time an entry needs it: an entry's file is given once
 * the types it refers to are made (a typedef's file after the type it
 * stands for's, a struct's after its members'), but a variable's before its
 * type's, and a function's before its return type's. A struct only
 * declared has no file, and neither has __builtin_va_list. */

/* The types a type's entry refers to, in the order LLVM makes them
 * (__ARRAY_SIZE_TYPE__ after an array's element type). */
static void referred(struct lines *l, uint t, struct ints *out) {
    struct dtype *d = l->types.v[t];
    out->n = 0;
    switch (d->kind) {
    case DT_PTR:
    case DT_QUAL:
    case DT_TYPEDEF:
        if (d->to >= 0) {
            ints_push(out, d->to);
        }
        return;
    case DT_ARRAY: {
        ints_push(out, d->to);
        int size = find_type(l, "S");
        if (size >= 0) {
            ints_push(out, size);
        }
        return;
    }
    case DT_RECORD:
        for (uint i = 0; i < d->nmembers; i++) {
            if (d->members[i].ty >= 0) {
                ints_push(out, d->members[i].ty);
            }
        }
        return;
    case DT_FUNC:
        if (d->to >= 0) {
            ints_push(out, d->to);
        }
        for (uint i = 0; i < d->nparams; i++) {
            if (d->params[i] >= 0) {
                ints_push(out, d->params[i]);
            }
        }
        return;
    case DT_ENUM:
        ints_push(out, d->to);
        return;
    default:
        return;
    }
}

static void push_local(struct order *o, uint f, int is_static, int ty, const char *sym) {
    o->locals[f] = grow(o->locals[f], &o->caplocals[f], o->nlocals[f] + 1, sizeof(struct local_entry));
    struct local_entry *e = &o->locals[f][o->nlocals[f]++];
    e->is_static = is_static;
    e->ty = ty;
    e->sym = sym;
}

static int has_local(struct order *o, uint f, int is_static, int ty, const char *sym) {
    for (uint i = 0; i < o->nlocals[f]; i++) {
        struct local_entry *e = &o->locals[f][i];
        if (e->is_static == is_static && (is_static ? strcmp(e->sym, sym) == 0 : e->ty == ty)) {
            return 1;
        }
    }
    return 0;
}

static void push_child(struct order *o, int kind, uint index) {
    o->unit = grow(o->unit, &o->capunit, o->nunit + 1, sizeof(struct child));
    o->unit[o->nunit].kind = kind;
    o->unit[o->nunit].index = index;
    o->nunit++;
}

/* What the front end does as it emits: the state of the first walk. */
struct front {
    struct lines *l;
    struct order *o;
    uchar *made;
    int *local_fn;
    /* The enums and the retained types, in the order they come. */
    struct ints enums;
    struct ints retained;
    uchar *is_retained;
};

/* The front end makes type t, if not made yet: an enum goes to its list,
 * a function's type to the function's, and what it refers to is made
 * after it. */
static void front_make(struct front *fr, int t) {
    if (t < 0 || fr->made[t]) {
        return;
    }
    fr->made[t] = 1;
    struct lines *l = fr->l;
    if (fr->local_fn[t] >= 0) {
        push_local(fr->o, (uint)fr->local_fn[t], 0, t, NULL);
    } else if (((struct dtype *)l->types.v[t])->kind == DT_ENUM) {
        ints_push(&fr->enums, t);
    }
    struct ints refs;
    memset(&refs, 0, sizeof refs);
    referred(l, (uint)t, &refs);
    for (uint i = 0; i < refs.n; i++) {
        /* __ARRAY_SIZE_TYPE__ is LLVM's, not the front end's. */
        if (((struct dtype *)l->types.v[refs.v[i]])->kind != DT_ARRAY_SIZE) {
            front_make(fr, refs.v[i]);
        }
    }
    heap_free(refs.v);
}

/* A type the unit retains, once. */
static void retain(struct front *fr, int t) {
    if (t >= 0 && !fr->is_retained[t]) {
        fr->is_retained[t] = 1;
        ints_push(&fr->retained, t);
    }
}

/* LLVM: what makes the entries, in the order it makes them (order.rs's
 * Maker). */
struct maker {
    struct lines *l;
    struct order *o;
    uchar *made;
    int *record_of;
    int *local_fn;
    /* The files the entries made so far name, in order. */
    struct ints *files;
    /* Whether phase 1 is over. */
    int begun;
    /* Which functions' entries are in the unit. */
    uchar *opened;
};

/* Makes type t's entry if it is not made yet: the types it refers to, and
 * the file it is declared in after theirs. */
static void make_type(struct maker *m, int t) {
    if (t < 0 || m->made[t]) {
        return;
    }
    struct lines *l = m->l;
    /* An anonymous type is made with its record. */
    int r = m->record_of[t];
    if (r >= 0 && !m->made[r]) {
        make_type(m, r);
        if (m->made[t]) {
            return;
        }
    }
    m->made[t] = 1;
    if (r < 0 && m->local_fn[t] < 0) {
        push_child(m->o, CH_TYPE, (uint)t);
    }
    /* A function's type made as the module begins makes the function's
     * entry, its context, where it is. */
    int f = m->local_fn[t];
    if (f >= 0 && !m->begun) {
        if (!m->opened[f]) {
            m->opened[f] = 1;
            push_child(m->o, CH_FUNCTION, (uint)f);
        }
        ints_push(&m->o->first[f], t);
    }
    struct dtype *d = l->types.v[t];
    if (d->kind == DT_RECORD) {
        /* Each member's type, then its file; the record's own last, if it
         * is defined. */
        for (uint i = 0; i < d->nmembers; i++) {
            make_type(m, d->members[i].ty);
            ints_push(m->files, (int)d->members[i].file);
        }
        if (d->rec->body != NULL) {
            ints_push(m->files, (int)d->file);
        }
        return;
    }
    struct ints refs;
    memset(&refs, 0, sizeof refs);
    referred(l, (uint)t, &refs);
    for (uint i = 0; i < refs.n; i++) {
        make_type(m, refs.v[i]);
    }
    heap_free(refs.v);
    if ((d->kind == DT_TYPEDEF || d->kind == DT_ENUM) && d->pos.line != 0) {
        ints_push(m->files, (int)d->file);
    }
}

/* The static local of symbol sym, among every function's. */
static struct dvar *find_static(struct lines *l, const char *sym) {
    for (uint i = 0; i < l->functions.n; i++) {
        struct dfunc *f = l->functions.v[i];
        for (uint k = 0; k < f->body->statics.n; k++) {
            struct dvar *v = f->body->statics.v[k];
            if (strcmp(v->sym, sym) == 0) {
                return v;
            }
        }
    }
    return NULL;
}

/* The order of the unit's entries, and of each function's types and static
 * locals, as clang's (see above). local_fn says which types are a
 * function's, local lists them; nested_types has said which are a
 * record's. */
static struct order *order(struct lines *l, int *local_fn, struct ints *local) {
    uint ntypes = l->types.n;
    uint nfuncs = l->functions.n;
    struct order *o = arena_alloc(sizeof *o);
    memset(o, 0, sizeof *o);
    o->locals = arena_alloc((nfuncs + 1) * sizeof(struct local_entry *));
    o->nlocals = arena_alloc((nfuncs + 1) * sizeof(uint));
    o->caplocals = arena_alloc((nfuncs + 1) * sizeof(uint));
    o->first = arena_alloc((nfuncs + 1) * sizeof(struct ints));
    memset(o->locals, 0, (nfuncs + 1) * sizeof(struct local_entry *));
    memset(o->nlocals, 0, (nfuncs + 1) * sizeof(uint));
    memset(o->caplocals, 0, (nfuncs + 1) * sizeof(uint));
    memset(o->first, 0, (nfuncs + 1) * sizeof(struct ints));
    int *record_of = arena_alloc((ntypes + 1) * sizeof(int));
    for (uint t = 0; t < ntypes; t++) {
        record_of[t] = -1;
    }
    for (uint k = 0; k < l->nnested; k++) {
        record_of[l->nested[k].ty] = l->nested[k].record;
    }

    /* What the front end makes as it emits them: the enums, the kept types,
     * the globals and each function's types and static locals. */
    struct front fr;
    memset(&fr, 0, sizeof fr);
    fr.l = l;
    fr.o = o;
    fr.made = heap_calloc(ntypes + 1, 1);
    fr.local_fn = local_fn;
    fr.is_retained = heap_calloc(ntypes + 1, 1);
    struct child *globals = NULL;
    uint nglobals = 0;
    uint capglobals = 0;
    for (uint k = 0; k < l->nitems; k++) {
        uint i = l->items[k].index;
        if (l->items[k].func) {
            struct dfunc *f = l->functions.v[i];
            front_make(&fr, f->ret);
            for (uint p = 0; p < f->params.n; p++) {
                front_make(&fr, ((struct dvar *)f->params.v[p])->ty);
            }
            for (uint e = 0; e < f->events.n; e++) {
                struct event *ev = f->events.v[e];
                if (ev->kind == EV_LOCAL) {
                    front_make(&fr, ev->ty);
                } else if (ev->kind == EV_STATIC) {
                    front_make(&fr, ev->ty);
                    push_local(o, i, 1, -1, ev->sym);
                } else if (ev->kind == EV_CAST) {
                    front_make(&fr, ev->ty);
                    /* The unit retains it, unless it is a type the function
                     * defines, which the function does. */
                    if (ev->ty >= 0 && local_fn[ev->ty] < 0) {
                        retain(&fr, ev->ty);
                    }
                } else if (ev->kind == EV_LITERAL) {
                    front_make(&fr, ((struct dliteral *)l->literals.v[ev->index])->ty);
                    globals = grow(globals, &capglobals, nglobals + 1, sizeof(struct child));
                    globals[nglobals].kind = CH_LITERAL;
                    globals[nglobals].index = ev->index;
                    nglobals++;
                } else {
                    front_make(&fr, ev->ty);
                }
            }
        } else {
            struct dglobal *g = l->globals.v[i];
            for (uint e = 0; e < g->events.n; e++) {
                struct event *ev = g->events.v[e];
                if (ev->kind == EV_LITERAL) {
                    front_make(&fr, ((struct dliteral *)l->literals.v[ev->index])->ty);
                    globals = grow(globals, &capglobals, nglobals + 1, sizeof(struct child));
                    globals[nglobals].kind = CH_LITERAL;
                    globals[nglobals].index = ev->index;
                    nglobals++;
                } else if (ev->kind == EV_CAST) {
                    front_make(&fr, ev->ty);
                    retain(&fr, ev->ty);
                }
            }
            front_make(&fr, g->var->ty);
            globals = grow(globals, &capglobals, nglobals + 1, sizeof(struct child));
            globals[nglobals].kind = CH_GLOBAL;
            globals[nglobals].index = i;
            nglobals++;
        }
    }
    /* A function's statics its code never declared (none should be). */
    for (uint i = 0; i < nfuncs; i++) {
        struct dfunc *f = l->functions.v[i];
        for (uint k = 0; k < f->body->statics.n; k++) {
            struct dvar *v = f->body->statics.v[k];
            if (!has_local(o, i, 1, -1, v->sym)) {
                push_local(o, i, 1, -1, v->sym);
            }
        }
    }
    /* And a function's types the front end did not reach. */
    for (uint k = 0; k < local->n; k++) {
        int t = local->v[k];
        uint f = (uint)local_fn[t];
        if (!has_local(o, f, 0, t, NULL)) {
            push_local(o, f, 0, t, NULL);
        }
    }

    /* ---- LLVM: the entries, in the order it makes them ---- */
    struct maker m;
    memset(&m, 0, sizeof m);
    m.l = l;
    m.o = o;
    m.made = heap_calloc(ntypes + 1, 1);
    m.record_of = record_of;
    m.local_fn = local_fn;
    m.files = &o->begin_files;
    m.opened = heap_calloc(nfuncs + 1, 1);
    /* 1. The enums, then the kept types. */
    for (uint k = 0; k < fr.enums.n; k++) {
        make_type(&m, fr.enums.v[k]);
    }
    for (uint k = 0; k < fr.retained.n; k++) {
        make_type(&m, fr.retained.v[k]);
    }
    o->begins = fr.enums.n > 0 || fr.retained.n > 0;
    m.files = &o->end_files;
    m.begun = 1;
    /* 2. The functions, in the order of their code, but those phase 1 made
     * already. */
    for (uint k = 0; k < l->ncode; k++) {
        uint i = l->code[k];
        if (!m.opened[i]) {
            m.opened[i] = 1;
            push_child(o, CH_FUNCTION, i);
        }
    }
    /* The types phase 1 made are no longer the function's to make. */
    for (uint f = 0; f < nfuncs; f++) {
        uint kept = 0;
        for (uint k = 0; k < o->nlocals[f]; k++) {
            struct local_entry *e = &o->locals[f][k];
            if (!e->is_static && ints_has(&o->first[f], e->ty)) {
                continue;
            }
            o->locals[f][kept++] = *e;
        }
        o->nlocals[f] = kept;
    }
    /* 3. The globals: each one's type, then its file. */
    for (uint k = 0; k < nglobals; k++) {
        push_child(o, globals[k].kind, globals[k].index);
        if (globals[k].kind == CH_GLOBAL) {
            struct dglobal *g = l->globals.v[globals[k].index];
            make_type(&m, g->var->ty);
            ints_push(m.files, (int)g->var->file);
        } else {
            struct dliteral *lit = l->literals.v[globals[k].index];
            make_type(&m, lit->ty);
            ints_push(m.files, (int)lit->file);
        }
    }
    /* 4. Each function's types and static locals, the functions in the
     * order of their code (LLVM's list of subprograms), as 5 and 6. */
    for (uint k = 0; k < l->ncode; k++) {
        uint i = l->code[k];
        for (uint x = 0; x < o->nlocals[i]; x++) {
            struct local_entry *e = &o->locals[i][x];
            if (!e->is_static) {
                make_type(&m, e->ty);
            } else {
                struct dvar *v = find_static(l, e->sym);
                if (v != NULL) {
                    make_type(&m, v->ty);
                    ints_push(m.files, (int)v->file);
                }
            }
        }
    }
    /* 5. Each function's return type, after its own file. */
    for (uint k = 0; k < l->ncode; k++) {
        struct dfunc *f = l->functions.v[l->code[k]];
        ints_push(m.files, (int)f->file);
        make_type(&m, f->ret);
    }
    /* 6. Each function's parameters' and locals' types, each after the
     * variable's file, and its labels' files. */
    for (uint k = 0; k < l->ncode; k++) {
        struct dfunc *f = l->functions.v[l->code[k]];
        for (uint p = 0; p < f->params.n; p++) {
            struct dvar *v = f->params.v[p];
            ints_push(m.files, (int)v->file);
            make_type(&m, v->ty);
        }
        for (uint e = 0; e < f->events.n; e++) {
            struct event *ev = f->events.v[e];
            if (ev->kind == EV_LOCAL) {
                ints_push(m.files, (int)ev->file);
                make_type(&m, ev->ty);
            }
        }
        label_files(f->body, m.files);
    }
    /* Anything else described (none should be left). */
    for (uint t = 0; t < ntypes; t++) {
        make_type(&m, (int)t);
    }
    heap_free(fr.made);
    heap_free(fr.is_retained);
    heap_free(fr.enums.v);
    heap_free(fr.retained.v);
    heap_free(globals);
    heap_free(m.made);
    heap_free(m.opened);
    return o;
}

/* ---- Numbering the files ---- */

/* The number in a directive line: what follows `name` at its start (after
 * the indentation), up to the next blank, if it is a number (merlin's
 * renumber's `directive`); -1 if the line is no such directive. */
static int64_t directive_number(const char *line, uint n, const char *name) {
    uint i = 0;
    while (i < n && (line[i] == ' ' || line[i] == '\t' || line[i] == '\n' || line[i] == '\r')) {
        i++;
    }
    uint k = (uint)strlen(name);
    if (n - i < k || memcmp(line + i, name, k) != 0) {
        return -1;
    }
    i += k;
    while (i < n && (line[i] == ' ' || line[i] == '\t' || line[i] == '\n' || line[i] == '\r')) {
        i++;
    }
    uint start = i;
    uint64_t v = 0;
    while (i < n && line[i] >= '0' && line[i] <= '9') {
        v = v * 10 + (uint64_t)(line[i] - '0');
        if (v > 4294967295ull) {
            return -1;
        }
        i++;
    }
    /* The whole word must be the number (a u32). */
    if (i == start || (i < n && !(line[i] == ' ' || line[i] == '\t' || line[i] == '\n' || line[i] == '\r'))) {
        return -1;
    }
    return (int64_t)v;
}

/* The number given to file n (as the code was generated) in the final
 * numbering, which `give` hands out (merlin's renumber's closure). */
struct giving {
    struct lines *l;
    /* by number as generated: the final one, or -1. */
    int *map;
    /* The keys given a final number so far, in order: number k + 1 is
     * keys[k]. */
    uint *keys;
    uint nkeys;
};

/* Gives file n its final number if it has none: that of the file LLVM
 * looks up under the same directory and name, or the next. The new number,
 * or -1 if it had one or took another's. */
static int give(struct giving *g, uint n) {
    if (g->map[n] >= 0) {
        return -1;
    }
    struct dfile *f = &g->l->entries[n - 1];
    for (uint k = 0; k < g->nkeys; k++) {
        struct dfile *other = &g->l->entries[g->keys[k] - 1];
        if (str_eq(other->key_dir, f->key_dir) && str_eq(other->key_name, f->key_name)) {
            g->map[n] = (int)k + 1;
            return -1;
        }
    }
    g->keys[g->nkeys++] = n;
    g->map[n] = (int)g->nkeys;
    return (int)g->nkeys;
}

void debug_renumber(struct lines *l, struct buf *text, uint from) {
    /* Arrays count their elements in the type clang names
     * __ARRAY_SIZE_TYPE__. */
    for (uint i = 0; i < l->types.n; i++) {
        if (((struct dtype *)l->types.v[i])->kind == DT_ARRAY) {
            array_size(l);
            break;
        }
    }
    nested_types(l);
    int *local_fn = arena_alloc((l->types.n + 1) * sizeof(int));
    struct ints local;
    memset(&local, 0, sizeof local);
    local_types(l, local_fn, &local);
    struct order *o = order(l, local_fn, &local);
    heap_free(local.v);

    /* The final numbers, from 1 as LLVM needs them. Files that LLVM's
     * assembler looks up under the same directory and name (two DIFiles
     * that differ in their checksums) are one: the first one LLVM needs
     * makes the file, with its checksum and its .file, whatever one the
     * code generation numbered first. */
    struct giving g;
    g.l = l;
    g.map = arena_alloc((l->nentries + 1) * sizeof(int));
    for (uint i = 0; i <= l->nentries; i++) {
        g.map[i] = -1;
    }
    g.map[0] = 0;
    g.keys = arena_alloc((l->nentries + 1) * sizeof(uint));
    g.nkeys = 0;
    /* 1. The files of the entries made as the module begins. */
    struct buf first;
    memset(&first, 0, sizeof first);
    for (uint k = 0; k < o->begin_files.n; k++) {
        uint n = (uint)o->begin_files.v[k];
        int to = give(&g, n);
        if (to >= 0) {
            buf_fmt(&first, "    %s\n", file_directive(l, n, (uint)to));
        }
    }
    int unit_first = o->begins || l->globals.n > 0 || l->literals.n > 0;
    /* 2. The files of the code, as the output needs them. The only .file
     * in the text is .file 0. */
    /* What came before the code generator's output stays as it is. */
    struct buf out;
    memset(&out, 0, sizeof out);
    buf_put(&out, text->data, from);
    const char *file0 = NULL;
    uint file0_len = 0;
    uint at = from;
    while (at < text->len) {
        uint end = at;
        while (end < text->len && text->data[end] != '\n') {
            end++;
        }
        if (end < text->len) {
            end++;
        }
        const char *line = text->data + at;
        uint len = end - at;
        at = end;
        if (directive_number(line, len, ".file ") >= 0) {
            if (unit_first) {
                buf_put(&out, line, len);
                buf_put(&out, first.data, first.len);
            } else {
                file0 = line;
                file0_len = len;
            }
            continue;
        }
        int64_t n = directive_number(line, len, ".loc ");
        if (n < 0) {
            buf_put(&out, line, len);
            continue;
        }
        if (file0 != NULL) {
            buf_put(&out, file0, file0_len);
            file0 = NULL;
        }
        int to = give(&g, (uint)n);
        if (to >= 0) {
            buf_fmt(&out, "    %s\n", file_directive(l, (uint)n, (uint)to));
        }
        /* The line with its number renumbered: `    .loc N ...` becomes
         * `    .loc TO ...`. */
        uint dot = 0;
        while (line[dot] != '.') {
            dot++;
        }
        uint space = dot;
        while (line[space] != ' ') {
            space++;
        }
        char *from = fmt("%u", (uint)n);
        buf_put(&out, line, space + 1);
        buf_fmt(&out, "%u", (uint)g.map[n]);
        uint rest = space + 1 + (uint)strlen(from);
        buf_put(&out, line + rest, len - rest);
    }
    if (file0 != NULL) {
        /* No .loc came: .file 0 goes first. */
        struct buf with;
        memset(&with, 0, sizeof with);
        buf_put(&with, out.data, from);
        buf_put(&with, file0, file0_len);
        buf_put(&with, out.data + from, out.len - from);
        buf_free(&out);
        out = with;
    }
    /* 3. The files of the entries made as the module ends. */
    for (uint k = 0; k < o->end_files.n; k++) {
        uint n = (uint)o->end_files.v[k];
        int to = give(&g, n);
        if (to >= 0) {
            vec_push(&l->late_files, file_directive(l, n, (uint)to));
        }
    }
    l->renumbered = g.map;
    l->nrenumbered = l->nentries + 1;
    l->order = o;
    buf_free(&first);
    buf_free(text);
    *text = out;
}

/* The final number of file n. */
static uint final_file(struct lines *l, uint n) {
    if (n < l->nrenumbered && l->renumbered[n] >= 0) {
        return (uint)l->renumbered[n];
    }
    return n;
}

/* ---- The entries ---- */

/* An attribute's value (merlin's Val), which decides its form. Constants
 * take the smallest data form that holds them, strings and addresses are
 * indices (in .debug_str_offsets and .debug_addr), as clang writes them for
 * DWARF 5. */
enum {
    /* A string (s), DW_FORM_strx1 to strx4 once pooled: V_STRX. */
    V_STR,
    V_STRX,
    /* DW_FORM_data1, whatever the value (an encoding). */
    V_DATA1,
    /* DW_FORM_data2, whatever the value (a language). */
    V_DATA2,
    /* A constant: the first data form that holds it (LLVM's
     * DIEInteger::BestForm). */
    V_CONST,
    V_SDATA,
    V_UDATA,
    /* DW_FORM_flag_present: the attribute is there, and takes no bytes. */
    V_FLAG,
    /* A label's address (s), DW_FORM_addrx once pooled: V_ADDRX. */
    V_ADDR,
    V_ADDRX,
    /* DW_FORM_data4: the distance between two labels (s - s2: high pc). */
    V_LEN,
    /* DW_FORM_ref4: a type's entry, by its index in the table (u). */
    V_REF,
    /* DW_FORM_exprloc. */
    V_EXPR,
    /* DW_FORM_sec_offset: a label in another section (s). */
    V_SEC_OFFSET
};

/* Part of a location expression: bytes, or a symbol's address (sym), which
 * pooling makes DW_OP_addrx and its index in .debug_addr. */
struct epart {
    const char *sym;
    uchar bytes[12];
    uint len;
};

struct val {
    int kind;
    uint64_t u;
    int64_t i;
    const char *s;
    const char *s2;
    struct epart *parts;
    uint nparts;
};

struct attr {
    uint at;
    struct val v;
};

/* A debugging information entry as it is built. */
struct die {
    uint tag;
    /* What the entry is, for the comment before it. */
    const char *what;
    struct attr *attrs;
    uint nattrs;
    uint capattrs;
    /* struct die *. */
    struct vec children;
    /* Its abbreviation's code, once numbered. */
    uint64_t code;
    /* The type it describes, if it is one (else -1). */
    int type_id;
};

static struct die *new_die(uint tag, const char *what) {
    struct die *d = arena_alloc(sizeof *d);
    memset(d, 0, sizeof *d);
    d->tag = tag;
    d->what = what;
    d->type_id = -1;
    return d;
}

static struct val *attr(struct die *d, uint at, int kind) {
    d->attrs = grow(d->attrs, &d->capattrs, d->nattrs + 1, sizeof(struct attr));
    struct attr *a = &d->attrs[d->nattrs++];
    memset(a, 0, sizeof *a);
    a->at = at;
    a->v.kind = kind;
    return &a->v;
}

static void attr_str(struct die *d, uint at, const char *s) {
    attr(d, at, V_STR)->s = s;
}

static void attr_const(struct die *d, uint at, uint64_t v) {
    attr(d, at, V_CONST)->u = v;
}

static void attr_data1(struct die *d, uint at, uint v) {
    attr(d, at, V_DATA1)->u = v;
}

static void attr_flag(struct die *d, uint at) {
    attr(d, at, V_FLAG);
}

static void attr_addr(struct die *d, uint at, const char *label) {
    attr(d, at, V_ADDR)->s = label;
}

static void attr_len(struct die *d, uint at, const char *end, const char *start) {
    struct val *v = attr(d, at, V_LEN);
    v->s = end;
    v->s2 = start;
}

static void attr_ref(struct die *d, uint at, int t) {
    attr(d, at, V_REF)->u = (uint64_t)t;
}

/* A type attribute, if the type is not void. */
static void typed(struct die *d, int t) {
    if (t >= 0) {
        attr_ref(d, AT_TYPE, t);
    }
}

static void attr_sec_offset(struct die *d, uint at, const char *label) {
    attr(d, at, V_SEC_OFFSET)->s = label;
}

/* An expression of one part: bytes, or (sym) a symbol's address. */
static void attr_expr(struct die *d, uint at, const uchar *bytes, uint n, const char *sym) {
    struct val *v = attr(d, at, V_EXPR);
    v->parts = arena_alloc(sizeof(struct epart));
    memset(v->parts, 0, sizeof(struct epart));
    v->nparts = 1;
    v->parts[0].sym = sym;
    if (sym == NULL) {
        memcpy(v->parts[0].bytes, bytes, n);
        v->parts[0].len = n;
    }
}

/* A DW_AT_decl_file of file n. */
static void attr_file(struct lines *l, struct die *d, uint n) {
    attr_const(d, AT_DECL_FILE, final_file(l, n));
}

/* The entry of a parameter or variable. */
static struct die *var_die(struct lines *l, uint tag, struct dvar *v) {
    struct die *d = new_die(tag, v->name);
    if (v->sym == NULL) {
        uchar b[12];
        b[0] = OP_FBREG;
        uint n = 1 + sleb(v->offset, b + 1);
        attr_expr(d, AT_LOCATION, b, n, NULL);
    } else {
        attr_expr(d, AT_LOCATION, NULL, 0, v->sym);
    }
    /* An unnamed parameter (int f(int) { ... }) has none. */
    if (v->name[0] != 0) {
        attr_str(d, AT_NAME, v->name);
    }
    attr_file(l, d, v->file);
    attr_const(d, AT_DECL_LINE, v->line);
    typed(d, v->ty);
    return d;
}

/* The entry of a global (or a static local: external 0), in the order clang
 * writes its attributes (name, type, file, line, location). */
static struct die *global_die(struct lines *l, struct dvar *v, int external) {
    struct die *d = new_die(TAG_VARIABLE, v->name);
    attr_str(d, AT_NAME, v->name);
    typed(d, v->ty);
    if (external) {
        attr_flag(d, AT_EXTERNAL);
    }
    attr_file(l, d, v->file);
    attr_const(d, AT_DECL_LINE, v->line);
    attr_expr(d, AT_LOCATION, NULL, 0, v->sym);
    return d;
}

/* The entries of a scope's variables in the frame, its labels and its
 * blocks. */
static void scope_children(struct lines *l, struct dscope *s, struct vec *out) {
    for (uint i = 0; i < s->vars.n; i++) {
        vec_push(out, var_die(l, TAG_VARIABLE, s->vars.v[i]));
    }
    for (uint i = 0; i < s->labels.n; i++) {
        struct dlabel *lb = s->labels.v[i];
        struct die *d = new_die(TAG_LABEL, lb->name);
        attr_str(d, AT_NAME, lb->name);
        attr_file(l, d, lb->file);
        attr_const(d, AT_DECL_LINE, lb->line);
        attr_const(d, AT_DECL_COLUMN, lb->col);
        attr_addr(d, AT_LOW_PC, lb->sym);
        vec_push(out, d);
    }
    for (uint i = 0; i < s->blocks.n; i++) {
        struct dscope *b = s->blocks.v[i];
        if (b->transparent) {
            scope_children(l, b, out);
            continue;
        }
        struct die *d = new_die(TAG_LEXICAL_BLOCK, "a block");
        attr_addr(d, AT_LOW_PC, b->begin);
        attr_len(d, AT_HIGH_PC, b->end, b->begin);
        scope_children(l, b, &d->children);
        vec_push(out, d);
    }
}

static struct die *type_die(struct lines *l, int id);

/* The entry of function i, with its static locals and the types it defines
 * in the order the order has them, after the types it defines that LLVM
 * makes as the module begins. */
static struct die *function_die(struct lines *l, uint i) {
    struct dfunc *f = l->functions.v[i];
    struct order *o = l->order;
    struct die *d = new_die(TAG_SUBPROGRAM, f->name);
    attr_addr(d, AT_LOW_PC, f->name);
    attr_len(d, AT_HIGH_PC, f->end, f->name);
    uchar reg8 = OP_REG8;
    attr_expr(d, AT_FRAME_BASE, &reg8, 1, NULL);
    attr_str(d, AT_NAME, f->name);
    attr_file(l, d, f->file);
    attr_const(d, AT_DECL_LINE, f->line);
    /* int f() has no prototype; clang says so by leaving this out. */
    if (f->prototyped) {
        attr_flag(d, AT_PROTOTYPED);
    }
    typed(d, f->ret);
    if (f->external) {
        attr_flag(d, AT_EXTERNAL);
    }
    for (uint k = 0; k < o->first[i].n; k++) {
        vec_push(&d->children, type_die(l, o->first[i].v[k]));
    }
    for (uint k = 0; k < f->params.n; k++) {
        vec_push(&d->children, var_die(l, TAG_FORMAL_PARAMETER, f->params.v[k]));
    }
    /* The variables in the frame, the labels and the blocks, then `...`,
     * then every static local of the function and the types it defines,
     * whatever block declares them, as clang has them. */
    scope_children(l, f->body, &d->children);
    if (f->variadic) {
        vec_push(&d->children, new_die(TAG_UNSPECIFIED_PARAMETERS, "the variadic arguments"));
    }
    for (uint k = 0; k < o->nlocals[i]; k++) {
        struct local_entry *e = &o->locals[i][k];
        if (!e->is_static) {
            vec_push(&d->children, type_die(l, e->ty));
            continue;
        }
        for (uint s = 0; s < f->body->statics.n; s++) {
            struct dvar *v = f->body->statics.v[s];
            if (strcmp(v->sym, e->sym) == 0) {
                vec_push(&d->children, global_die(l, v, 0));
                break;
            }
        }
    }
    return d;
}

/* The entry of a string literal: a variable without a name, as clang
 * describes one. */
static struct die *literal_die(struct lines *l, struct dliteral *lit) {
    struct die *d = new_die(TAG_VARIABLE, fmt("the string literal %s", lit->label));
    typed(d, lit->ty);
    attr_file(l, d, lit->file);
    attr_const(d, AT_DECL_LINE, lit->line);
    attr_expr(d, AT_LOCATION, NULL, 0, lit->label);
    return d;
}

/* The entry of type id, with the anonymous types nested_types puts in
 * it. */
static struct die *type_die(struct lines *l, int id) {
    struct dtype *t = l->types.v[id];
    struct die *d;
    switch (t->kind) {
    case DT_BASE: {
        int k = t->ik;
        d = new_die(TAG_BASE_TYPE, ik_name(k));
        uint encoding;
        if (k == IK_BOOL) {
            encoding = ATE_BOOLEAN;
        } else if (k == IK_CHAR || k == IK_UCHAR) {
            /* Plain char is unsigned on RISC-V. */
            encoding = ATE_UNSIGNED_CHAR;
        } else if (k == IK_SCHAR) {
            encoding = ATE_SIGNED_CHAR;
        } else if (ik_signed(k)) {
            encoding = ATE_SIGNED;
        } else {
            encoding = ATE_UNSIGNED;
        }
        attr_str(d, AT_NAME, ik_name(k));
        attr_data1(d, AT_ENCODING, encoding);
        attr_const(d, AT_BYTE_SIZE, ik_size(k));
        break;
    }
    case DT_ARRAY_SIZE:
        d = new_die(TAG_BASE_TYPE, "__ARRAY_SIZE_TYPE__");
        attr_str(d, AT_NAME, "__ARRAY_SIZE_TYPE__");
        attr_const(d, AT_BYTE_SIZE, 8);
        attr_data1(d, AT_ENCODING, ATE_UNSIGNED);
        break;
    case DT_PTR:
        d = new_die(TAG_POINTER_TYPE, "a pointer");
        typed(d, t->to);
        break;
    case DT_ARRAY: {
        d = new_die(TAG_ARRAY_TYPE, "an array");
        attr_ref(d, AT_TYPE, t->to);
        int size_type = find_type(l, "S");
        for (uint i = 0; i < t->ndims; i++) {
            struct die *s = new_die(TAG_SUBRANGE_TYPE, "a dimension");
            attr_ref(s, AT_TYPE, size_type);
            if (t->dims[i] >= 0) {
                attr_const(s, AT_COUNT, (uint64_t)t->dims[i]);
            }
            vec_push(&d->children, s);
        }
        break;
    }
    case DT_RECORD: {
        struct record *r = t->rec;
        const char *what = r->tag != NULL ? fmt("%s %s", record_keyword(r), r->tag)
                                          : fmt("%s (anonymous at %u:%u)", record_keyword(r), r->pos.line, r->pos.col);
        d = new_die(r->is_union ? TAG_UNION_TYPE : TAG_STRUCTURE_TYPE, what);
        if (r->tag != NULL) {
            attr_str(d, AT_NAME, r->tag);
        }
        /* One only declared has no file and line (clang's
         * isForwardDecl). */
        if (r->body != NULL) {
            attr_const(d, AT_BYTE_SIZE, r->body->size);
            attr_file(l, d, t->file);
            attr_const(d, AT_DECL_LINE, t->line);
        } else {
            attr_flag(d, AT_DECLARATION);
        }
        for (uint i = 0; i < t->nmembers; i++) {
            struct dmember *mb = &t->members[i];
            struct die *m = new_die(TAG_MEMBER, mb->name);
            attr_str(m, AT_NAME, mb->name);
            typed(m, mb->ty);
            attr_file(l, m, mb->file);
            attr_const(m, AT_DECL_LINE, mb->line);
            /* A union's members too, at 0, as clang writes them. */
            attr_const(m, AT_DATA_MEMBER_LOCATION, mb->offset);
            vec_push(&d->children, m);
            /* The anonymous types this member's type is made of, defined
             * here: inside the record, after the member, as clang has
             * them. */
            for (uint k = 0; k < l->nnested; k++) {
                if (l->nested[k].record == id && l->nested[k].member == i) {
                    vec_push(&d->children, type_die(l, l->nested[k].ty));
                }
            }
        }
        break;
    }
    case DT_QUAL:
        d = new_die(t->konst ? TAG_CONST_TYPE : TAG_VOLATILE_TYPE, t->konst ? "const" : "volatile");
        typed(d, t->to);
        break;
    case DT_FUNC:
        d = new_die(TAG_SUBROUTINE_TYPE, "a function type");
        /* The return type first, then the flag, as LLVM's constructTypeDIE
         * adds them: the abbreviation lists the attributes in this order.
         * A type without a prototype has no flag, and its unknown
         * parameters are `...`'s entry. */
        typed(d, t->to);
        if (t->prototyped) {
            attr_flag(d, AT_PROTOTYPED);
        }
        for (uint i = 0; i < t->nparams; i++) {
            struct die *p = new_die(TAG_FORMAL_PARAMETER, "a parameter");
            typed(p, t->params[i]);
            vec_push(&d->children, p);
        }
        if (t->variadic || !t->prototyped) {
            vec_push(&d->children, new_die(TAG_UNSPECIFIED_PARAMETERS, "the variadic arguments"));
        }
        break;
    case DT_TYPEDEF:
        d = new_die(TAG_TYPEDEF, t->name);
        typed(d, t->to);
        attr_str(d, AT_NAME, t->name);
        /* A builtin one (__builtin_va_list) is nowhere in the file. */
        if (t->pos.line != 0) {
            attr_file(l, d, t->file);
            attr_const(d, AT_DECL_LINE, t->pos.line);
        }
        break;
    default: {
        struct enum_type *e = t->en;
        d = new_die(TAG_ENUMERATION_TYPE, fmt("enum %s", e->tag != NULL ? e->tag : "(anonymous)"));
        attr_ref(d, AT_TYPE, t->to);
        if (e->tag != NULL) {
            attr_str(d, AT_NAME, e->tag);
        }
        attr_const(d, AT_BYTE_SIZE, 4);
        attr_file(l, d, t->file);
        attr_const(d, AT_DECL_LINE, t->pos.line);
        struct dtype *of = l->types.v[t->to];
        int is_signed = of->kind == DT_BASE && ik_signed(of->ik);
        for (uint i = 0; i < e->nconstants; i++) {
            struct die *c = new_die(TAG_ENUMERATOR, e->constants[i].name);
            attr_str(c, AT_NAME, e->constants[i].name);
            /* The constant's form says how to read it: signed for an enum
             * of int, unsigned for one of unsigned int, as clang writes
             * them (the value's low 64 bits, as Rust's `as` takes them). */
            if (is_signed) {
                attr(c, AT_CONST_VALUE, V_SDATA)->i = (int64_t)e->constants[i].value.lo;
            } else {
                attr(c, AT_CONST_VALUE, V_UDATA)->u = e->constants[i].value.lo;
            }
            vec_push(&d->children, c);
        }
        break;
    }
    }
    d->type_id = id;
    return d;
}

/* ---- Forms, sizes and the pools ---- */

/* How many bytes an index takes as DW_FORM_strx1 to strx4, and the form. */
static uint strx_size(uint64_t i, uint *form) {
    if (i <= 0xff) {
        *form = FORM_STRX1;
        return 1;
    }
    if (i <= 0xffff) {
        *form = FORM_STRX2;
        return 2;
    }
    if (i <= 0xffffff) {
        *form = FORM_STRX3;
        return 3;
    }
    *form = FORM_STRX4;
    return 4;
}

/* How many bytes a constant takes in the smallest data form, and the
 * form. */
static uint data_size(uint64_t v, uint *form) {
    if (v <= 0xff) {
        *form = FORM_DATA1;
        return 1;
    }
    if (v <= 0xffff) {
        *form = FORM_DATA2;
        return 2;
    }
    if (v <= 0xffffffffull) {
        *form = FORM_DATA4;
        return 4;
    }
    *form = FORM_DATA8;
    return 8;
}

static uint form_of(struct val *v) {
    uint form = 0;
    switch (v->kind) {
    case V_STRX:
        strx_size(v->u, &form);
        return form;
    case V_DATA1:
        return FORM_DATA1;
    case V_DATA2:
        return FORM_DATA2;
    case V_CONST:
        data_size(v->u, &form);
        return form;
    case V_LEN:
        return FORM_DATA4;
    case V_SDATA:
        return FORM_SDATA;
    case V_UDATA:
        return FORM_UDATA;
    case V_FLAG:
        return FORM_FLAG_PRESENT;
    case V_ADDRX:
        return FORM_ADDRX;
    case V_REF:
        return FORM_REF4;
    case V_EXPR:
        return FORM_EXPRLOC;
    case V_SEC_OFFSET:
        return FORM_SEC_OFFSET;
    default:
        die("debug: a string or an address not pooled");
        return 0;
    }
}

/* The bytes of a (pooled) expression. */
static uint expr_len(struct val *v) {
    uint n = 0;
    for (uint i = 0; i < v->nparts; i++) {
        n += v->parts[i].len;
    }
    return n;
}

static uint val_size(struct val *v) {
    uint form;
    uchar b[12];
    switch (v->kind) {
    case V_STRX:
        return strx_size(v->u, &form);
    case V_DATA1:
        return 1;
    case V_DATA2:
        return 2;
    case V_CONST:
        return data_size(v->u, &form);
    case V_LEN:
    case V_REF:
    case V_SEC_OFFSET:
        return 4;
    case V_SDATA:
        return sleb(v->i, b);
    case V_UDATA:
    case V_ADDRX:
        return uleb(v->u, b);
    case V_FLAG:
        return 0;
    case V_EXPR: {
        uint n = expr_len(v);
        return uleb_len(n) + n;
    }
    default:
        die("debug: a string or an address not pooled");
        return 0;
    }
}

/* The strings or the addresses, each once, by their index in
 * .debug_str_offsets or .debug_addr, in the order they come. */
struct pool {
    struct vec items;
    struct map index;
};

/* The index of item in the pool, added at the end if it is new. */
static uint index_in(struct pool *p, const char *item) {
    void *at = map_get(&p->index, item);
    if (at != NULL) {
        return (uint)((uintptr_t)at - 1);
    }
    vec_push(&p->items, (void *)item);
    map_put(&p->index, item, (void *)(uintptr_t)p->items.n);
    return p->items.n - 1;
}

/* Turns the strings and addresses of this entry and those below it into
 * indices in the pools, adding those not seen yet: a string becomes a
 * V_STRX, an address a V_ADDRX, and a symbol's address in an expression
 * DW_OP_addrx and its index. */
static void pool(struct die *d, struct pool *strings, struct pool *addrs) {
    for (uint i = 0; i < d->nattrs; i++) {
        struct val *v = &d->attrs[i].v;
        if (v->kind == V_STR) {
            v->kind = V_STRX;
            v->u = index_in(strings, v->s);
        } else if (v->kind == V_ADDR) {
            v->kind = V_ADDRX;
            v->u = index_in(addrs, v->s);
        } else if (v->kind == V_EXPR) {
            for (uint k = 0; k < v->nparts; k++) {
                struct epart *p = &v->parts[k];
                if (p->sym != NULL) {
                    p->bytes[0] = OP_ADDRX;
                    p->len = 1 + uleb(index_in(addrs, p->sym), p->bytes + 1);
                    p->sym = NULL;
                }
            }
        }
    }
    for (uint i = 0; i < d->children.n; i++) {
        pool(d->children.v[i], strings, addrs);
    }
}

/* An abbreviation: a tag, whether children follow, and each attribute and
 * its form. */
struct abbrev {
    uint tag;
    int children;
    uchar *pairs;
    uint npairs;
};

struct abbrevs {
    struct vec list;
    /* The shape, as text -> its code. */
    struct map codes;
};

/* Gives this entry and those below it their abbreviations, adding the
 * shapes not seen yet, numbered from 1 in the order they come. */
static void number_abbrevs(struct die *d, struct abbrevs *abbrevs) {
    struct buf shape;
    memset(&shape, 0, sizeof shape);
    buf_fmt(&shape, "%u,%u", d->tag, d->children.n > 0 ? 1 : 0);
    for (uint i = 0; i < d->nattrs; i++) {
        buf_fmt(&shape, ",%u:%u", d->attrs[i].at, form_of(&d->attrs[i].v));
    }
    char *key = buf_arena(&shape);
    buf_free(&shape);
    void *at = map_get(&abbrevs->codes, key);
    if (at != NULL) {
        d->code = (uintptr_t)at;
    } else {
        struct abbrev *a = arena_alloc(sizeof *a);
        a->tag = d->tag;
        a->children = d->children.n > 0;
        a->npairs = d->nattrs;
        a->pairs = arena_alloc(2 * d->nattrs + 1);
        for (uint i = 0; i < d->nattrs; i++) {
            a->pairs[2 * i] = (uchar)d->attrs[i].at;
            a->pairs[2 * i + 1] = (uchar)form_of(&d->attrs[i].v);
        }
        vec_push(&abbrevs->list, a);
        map_put(&abbrevs->codes, key, (void *)(uintptr_t)abbrevs->list.n);
        d->code = abbrevs->list.n;
    }
    for (uint i = 0; i < d->children.n; i++) {
        number_abbrevs(d->children.v[i], abbrevs);
    }
}

/* Lays the entry out at `at` (from the unit's start), noting where each
 * type's entry is; returns the offset just past it and its children. */
static uint layout_die(struct die *d, uint at, uint *type_offsets) {
    if (d->type_id >= 0) {
        type_offsets[d->type_id] = at;
    }
    uint next = at + uleb_len(d->code);
    for (uint i = 0; i < d->nattrs; i++) {
        next += val_size(&d->attrs[i].v);
    }
    if (d->children.n > 0) {
        for (uint i = 0; i < d->children.n; i++) {
            next = layout_die(d->children.v[i], next, type_offsets);
        }
        /* The end of the children. */
        next++;
    }
    return next;
}

/* `.byte` of some bytes, in hex: `.byte 0x0b, 0x91`. */
static void bytes_line(struct buf *s, const uchar *b, uint n) {
    static const char HEX[] = "0123456789abcdef";
    buf_str(s, ".byte ");
    for (uint i = 0; i < n; i++) {
        if (i > 0) {
            buf_str(s, ", ");
        }
        buf_str(s, "0x");
        buf_byte(s, HEX[b[i] >> 4]);
        buf_byte(s, HEX[b[i] & 15]);
    }
}

/* A tag's name, for the comments. */
static const char *tag_name(uint tag) {
    switch (tag) {
    case TAG_ARRAY_TYPE:
        return "DW_TAG_array_type";
    case TAG_ENUMERATION_TYPE:
        return "DW_TAG_enumeration_type";
    case TAG_FORMAL_PARAMETER:
        return "DW_TAG_formal_parameter";
    case TAG_LABEL:
        return "DW_TAG_label";
    case TAG_LEXICAL_BLOCK:
        return "DW_TAG_lexical_block";
    case TAG_MEMBER:
        return "DW_TAG_member";
    case TAG_POINTER_TYPE:
        return "DW_TAG_pointer_type";
    case TAG_COMPILE_UNIT:
        return "DW_TAG_compile_unit";
    case TAG_STRUCTURE_TYPE:
        return "DW_TAG_structure_type";
    case TAG_SUBROUTINE_TYPE:
        return "DW_TAG_subroutine_type";
    case TAG_TYPEDEF:
        return "DW_TAG_typedef";
    case TAG_UNION_TYPE:
        return "DW_TAG_union_type";
    case TAG_UNSPECIFIED_PARAMETERS:
        return "DW_TAG_unspecified_parameters";
    case TAG_SUBRANGE_TYPE:
        return "DW_TAG_subrange_type";
    case TAG_BASE_TYPE:
        return "DW_TAG_base_type";
    case TAG_CONST_TYPE:
        return "DW_TAG_const_type";
    case TAG_ENUMERATOR:
        return "DW_TAG_enumerator";
    case TAG_SUBPROGRAM:
        return "DW_TAG_subprogram";
    case TAG_VARIABLE:
        return "DW_TAG_variable";
    case TAG_VOLATILE_TYPE:
        return "DW_TAG_volatile_type";
    default:
        return "?";
    }
}

/* Writes the entry and its children. */
static void emit_die(struct die *d, struct buf *s, uint *type_offsets, uint depth) {
    uchar b[12];
    buf_str(s, "    # ");
    for (uint i = 0; i < depth; i++) {
        buf_str(s, "  ");
    }
    buf_fmt(s, "%s: %s\n", tag_name(d->tag), d->what);
    buf_str(s, "    ");
    bytes_line(s, b, uleb(d->code, b));
    buf_byte(s, '\n');
    for (uint i = 0; i < d->nattrs; i++) {
        struct val *v = &d->attrs[i].v;
        uint form;
        if (v->kind == V_FLAG) {
            continue;
        }
        buf_str(s, "    ");
        switch (v->kind) {
        case V_STRX: {
            uint n = strx_size(v->u, &form);
            for (uint k = 0; k < n; k++) {
                b[k] = (uchar)(v->u >> (8 * k));
            }
            bytes_line(s, b, n);
            break;
        }
        case V_DATA1:
            buf_fmt(s, ".byte %u", (uint)v->u);
            break;
        case V_DATA2:
            buf_fmt(s, ".half %u", (uint)v->u);
            break;
        case V_CONST: {
            uint n = data_size(v->u, &form);
            const char *directive = n == 1 ? ".byte" : n == 2 ? ".half" : n == 4 ? ".word" : ".quad";
            buf_fmt(s, "%s %llu", directive, v->u);
            break;
        }
        case V_SDATA:
            bytes_line(s, b, sleb(v->i, b));
            break;
        case V_UDATA:
        case V_ADDRX:
            bytes_line(s, b, uleb(v->u, b));
            break;
        case V_LEN:
            buf_fmt(s, ".word %s - %s", v->s, v->s2);
            break;
        case V_REF:
            buf_fmt(s, ".word %u", type_offsets[v->u]);
            break;
        case V_SEC_OFFSET:
            buf_fmt(s, ".word %s", v->s);
            break;
        case V_EXPR: {
            uint n = expr_len(v);
            bytes_line(s, b, uleb(n, b));
            buf_fmt(s, "    # an expression of %u %s", n, n == 1 ? "byte" : "bytes");
            for (uint k = 0; k < v->nparts; k++) {
                buf_str(s, "\n    ");
                bytes_line(s, v->parts[k].bytes, v->parts[k].len);
            }
            break;
        }
        default:
            die("debug: a string or an address not pooled");
        }
        buf_byte(s, '\n');
    }
    if (d->children.n > 0) {
        for (uint i = 0; i < d->children.n; i++) {
            emit_die(d->children.v[i], s, type_offsets, depth + 1);
        }
        buf_str(s, "    .byte 0    # the end of the children\n");
    }
}

void debug_sections(struct lines *l, struct buf *s) {
    if (l->functions.n == 0) {
        return;
    }
    struct order *o = l->order;
    const char *first = ((struct dfunc *)l->functions.v[l->code[0]])->name;
    const char *last_end = ((struct dfunc *)l->functions.v[l->code[l->ncode - 1]])->end;

    /* The attributes in clang's order. */
    struct die *cu = new_die(TAG_COMPILE_UNIT, "the compile unit");
    attr_str(cu, AT_PRODUCER, "merlin");
    attr(cu, AT_LANGUAGE, V_DATA2)->u = LANG_C99;
    attr_str(cu, AT_NAME, arena_strndup(l->root.s, l->root.n));
    attr_sec_offset(cu, AT_STR_OFFSETS_BASE, ".Lstr_offsets_base0");
    attr_sec_offset(cu, AT_STMT_LIST, ".Lline_table_start0");
    attr_str(cu, AT_COMP_DIR, l->comp_dir_given);
    attr_addr(cu, AT_LOW_PC, first);
    attr_len(cu, AT_HIGH_PC, last_end, first);
    attr_sec_offset(cu, AT_ADDR_BASE, ".Laddr_table_base0");
    /* The entries in clang's order (see order). */
    for (uint k = 0; k < o->nunit; k++) {
        struct child *c = &o->unit[k];
        struct die *d;
        if (c->kind == CH_FUNCTION) {
            d = function_die(l, c->index);
        } else if (c->kind == CH_GLOBAL) {
            struct dglobal *g = l->globals.v[c->index];
            d = global_die(l, g->var, g->external);
        } else if (c->kind == CH_LITERAL) {
            d = literal_die(l, l->literals.v[c->index]);
        } else {
            d = type_die(l, (int)c->index);
        }
        vec_push(&cu->children, d);
    }

    /* The strings and the addresses, each once, by their index in
     * .debug_str_offsets and .debug_addr, in the order they come. */
    struct pool strings;
    struct pool addrs;
    memset(&strings, 0, sizeof strings);
    memset(&addrs, 0, sizeof addrs);
    pool(cu, &strings, &addrs);

    /* The abbreviations, one per shape of entry (tag, children, the
     * attributes' forms), numbered from 1 in the order they come. */
    struct abbrevs abbrevs;
    memset(&abbrevs, 0, sizeof abbrevs);
    number_abbrevs(cu, &abbrevs);

    /* Where each entry starts, from the unit's start: its header is 12
     * bytes (length, version, unit type, address size, abbreviations). */
    uint *type_offsets = heap_calloc(l->types.n + 1, sizeof(uint));
    uint end = layout_die(cu, 12, type_offsets);

    for (uint i = 0; i < l->late_files.n; i++) {
        buf_fmt(s, "    %s\n", (char *)l->late_files.v[i]);
    }
    buf_str(s, "\n    .section .debug_abbrev,\"\",@progbits\n");
    buf_str(s, ".Ldebug_abbrev0:\n");
    for (uint i = 0; i < abbrevs.list.n; i++) {
        struct abbrev *a = abbrevs.list.v[i];
        uchar b[14];
        uint n = uleb(i + 1, b);
        b[n++] = (uchar)a->tag;
        b[n++] = (uchar)a->children;
        buf_str(s, "    ");
        bytes_line(s, b, n);
        buf_fmt(s, "    # %u: %s\n", i + 1, tag_name(a->tag));
        for (uint k = 0; k < a->npairs; k++) {
            buf_str(s, "    ");
            bytes_line(s, a->pairs + 2 * k, 2);
            buf_byte(s, '\n');
        }
        buf_str(s, "    .byte 0, 0\n");
    }
    buf_str(s, "    .byte 0    # the end of the abbreviations\n");

    buf_str(s, "\n    .section .debug_info,\"\",@progbits\n");
    buf_fmt(s, "    .word %u    # unit length\n", end - 4);
    buf_str(s, "    .half 5    # DWARF version\n");
    buf_fmt(s, "    .byte %u    # a compile unit\n", UT_COMPILE);
    buf_fmt(s, "    .byte %u    # address size\n", PTR_SIZE);
    buf_str(s, "    .word .Ldebug_abbrev0\n");
    emit_die(cu, s, type_offsets, 0);

    /* The strings: their offsets in .debug_str (a header of 8 bytes, then
     * a word each, as clang writes them), then the strings. */
    buf_str(s, "\n    .section .debug_str_offsets,\"\",@progbits\n");
    buf_fmt(s, "    .word %u    # length\n", 4 + 4 * strings.items.n);
    buf_str(s, "    .half 5    # DWARF version\n");
    buf_str(s, "    .half 0    # padding\n");
    buf_str(s, ".Lstr_offsets_base0:\n");
    for (uint i = 0; i < strings.items.n; i++) {
        buf_fmt(s, "    .word .Linfo_string%u\n", i);
    }
    buf_str(s, "\n    .section .debug_str,\"MS\",@progbits,1\n");
    for (uint i = 0; i < strings.items.n; i++) {
        const char *t = strings.items.v[i];
        buf_fmt(s, ".Linfo_string%u:\n", i);
        buf_str(s, "    .asciz ");
        quote(s, t, (uint)strlen(t));
        buf_byte(s, '\n');
    }
    /* The addresses: a header of 8 bytes, then one each, written .quad as
     * clang writes them. */
    buf_str(s, "\n    .section .debug_addr,\"\",@progbits\n");
    buf_fmt(s, "    .word %u    # length\n", 4 + PTR_SIZE * addrs.items.n);
    buf_str(s, "    .half 5    # DWARF version\n");
    buf_fmt(s, "    .byte %u    # address size\n", PTR_SIZE);
    buf_str(s, "    .byte 0    # segment selector size\n");
    buf_str(s, ".Laddr_table_base0:\n");
    for (uint i = 0; i < addrs.items.n; i++) {
        buf_fmt(s, "    .quad %s\n", (char *)addrs.items.v[i]);
    }

    buf_str(s, "\n    .section .debug_line,\"\",@progbits\n");
    buf_str(s, ".Lline_table_start0:\n");
    heap_free(type_offsets);
}
