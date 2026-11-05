/* as: DWARF line tables, what `.file` and `.loc` become (merlin-as's
 * dwarf.rs, which explains the format and LLVM's rules at length).
 *
 * A compiler says which source line each instruction comes from:
 *
 *     .file 0 "kernel" "main.c"           file 0: the compilation directory and main file
 *     .file 1 "kernel" "include/types.h"  other files, numbered from 1
 *     .loc  0 42 5 prologue_end           the next instruction: main.c, line 42, column 5
 *
 * asm.c records a row (a section, an offset and a struct loc) for the first
 * instruction or data after each `.loc`; at the end, dwarf_encode writes
 * them as a DWARF 5 `.debug_line`: a header with the directory and file
 * tables (their names in `.debug_line_str`), then the line number program,
 * one sequence per code section. The bytes are LLVM's for the same input
 * (MCDwarfLineAddr::encode, MCDwarfLineTable::emitOne), as merlin-as's are:
 *
 *   - Registers start at file 1, line 1, column 0, is_stmt set, ISA 0.
 *   - Before each row only what changed is set: DW_LNS_set_file, then
 *     set_column, DW_LNE_set_discriminator (whenever the row has one),
 *     set_isa, negate_stmt, then the one-row flags basic_block,
 *     prologue_end and epilogue_begin.
 *   - The row: the first of a sequence sets the address (DW_LNE_set_address,
 *     a relocation) and adds the line with no address advance; the others
 *     advance both at once, with a special opcode when the two deltas fit
 *     in one (see advance). */
#include "as.h"

/* DW_LNS_*: the standard opcodes. */
#define COPY 1
#define ADVANCE_PC 2
#define ADVANCE_LINE 3
#define SET_FILE 4
#define SET_COLUMN 5
#define NEGATE_STMT 6
#define SET_BASIC_BLOCK 7
#define CONST_ADD_PC 8
#define SET_PROLOGUE_END 10
#define SET_EPILOGUE_BEGIN 11
#define SET_ISA 12
/* DW_LNE_*: extended opcodes, after a 0 byte and their length. */
#define END_SEQUENCE 1
#define SET_ADDRESS 2
#define SET_DISCRIMINATOR 4

/* The header's parameters, LLVM's: special opcodes from 13 up, each adding
 * a line delta in -5..8 and an address delta of 0..17 bytes. */
#define LINE_BASE (-5)
#define LINE_RANGE 14
#define OPCODE_BASE 13
/* The address advance of special opcode 255, which DW_LNS_const_add_pc adds. */
#define MAX_SPECIAL_ADDR ((255 - OPCODE_BASE) / LINE_RANGE)

/* How many operands each standard opcode 1..12 takes. */
static const uchar STANDARD_LENGTHS[12] = {0, 1, 1, 1, 1, 0, 0, 0, 1, 0, 0, 1};

/* DW_LNCT_* and DW_FORM_*, for the directory and file tables' formats. */
#define LNCT_PATH 1
#define LNCT_DIRECTORY_INDEX 2
#define LNCT_MD5 5
#define FORM_LINE_STRP 0x1f
#define FORM_UDATA 0x0f
#define FORM_DATA16 0x1e

static void uleb(struct buf *out, uint v) {
    for (;;) {
        uint byte = v & 0x7f;
        v >>= 7;
        if (v == 0) {
            buf_byte(out, byte);
            return;
        }
        buf_byte(out, byte | 0x80);
    }
}

/* A signed LEB128 of a 64-bit value: a line delta is the difference of two
 * 32-bit line numbers. */
static void sleb(struct buf *out, int64_t v) {
    for (;;) {
        uint byte = (uint)v & 0x7f;
        v >>= 7; /* arithmetic: the sign comes in */
        if ((v == 0 && (byte & 0x40) == 0) || (v == -1 && (byte & 0x40) != 0)) {
            buf_byte(out, byte);
            return;
        }
        buf_byte(out, byte | 0x80);
    }
}

/* ---- The directory and file tables ---- */

/* Where LLVM's sys::path functions see the root directory of `path`
 * (root_dir_start, POSIX style): the `/` of `/x`, the separator after a
 * network name (`//net/x`), or none (-1). */
static int root_dir_start(const char *path) {
    uint len = (uint)strlen(path);
    if (len > 3 && path[0] == '/' && path[1] == '/' && path[2] != '/') {
        char *sep = strchr(path + 2, '/');
        return sep == NULL ? -1 : (int)(sep - path);
    }
    return path[0] == '/' ? 0 : -1;
}

/* Where the last component of the first `len` bytes of `path` starts
 * (LLVM's filename_pos): at a trailing `/` itself, else after the last `/`
 * (0 if there is none, or if it is the second of a leading `//`). */
static uint filename_pos(const char *path, uint len) {
    if (len > 0 && path[len - 1] == '/') {
        return len - 1;
    }
    uint i = len;
    while (i > 0 && path[i - 1] != '/') {
        i--;
    }
    if (i == 0 || (i == 2 && path[0] == '/')) {
        return 0;
    }
    return i;
}

/* LLVM's sys::path::filename (POSIX style): the last component of `path`.
 * A trailing `/` is the component `.` (`a/` is `.` in `a`), except a root
 * directory alone, which is `/`. */
static char *file_name(const char *path) {
    uint len = (uint)strlen(path);
    int root = root_dir_start(path);
    /* Skip the trailing separators, unless one is the root directory. */
    uint end = len;
    while (end > 0 && (int)(end - 1) != root && path[end - 1] == '/') {
        end--;
    }
    if (len > 0 && path[len - 1] == '/' && (root < 0 || (int)end > root + 1)) {
        return ".";
    }
    uint start = filename_pos(path, end);
    return tmp_strndup(path + start, end - start);
}

/* LLVM's sys::path::parent_path (POSIX style): `path` without its last
 * component and the separators before it, keeping a root directory (`/q.h`
 * is in `/`); empty if there is nothing before the last component. */
static char *parent_path(const char *path) {
    uint end = filename_pos(path, (uint)strlen(path));
    int filename_was_sep = path[end] == '/';
    int root = root_dir_start(path);
    while (end > 0 && (root < 0 || (int)end > root) && path[end - 1] == '/') {
        end--;
    }
    if (root >= 0 && (int)end == root && !filename_was_sep) {
        return tmp_strndup(path, (uint)root + 1);
    }
    return tmp_strndup(path, end);
}

/* The directory and file tables as LLVM builds them from the `.file`s:
 * directory 0 is the compilation directory and file 0 the main file, as
 * written in `.file 0` (the name not split). Every other file is filed as
 * MCDwarfLineTableHeader::tryGetFile files it: a directory that is the
 * compilation directory is none; a file with no directory then has its
 * name split into parent_path and file_name (`include/types.h` is
 * `types.h` in `include`, `/q.h` is `q.h` in `/`), unless the name has no
 * parent; an empty name is `<stdin>`. Then its directory is 0 if empty,
 * else found or added in the order the files name them. Fills dirs (the
 * directories), names and dir_of (per file, its name and directory). */
static uint file_tables(struct dfile *root, struct dfile *files, uint nfiles, char **dirs, char **names,
                        uint *dir_of) {
    const char *comp_dir = root->dir;
    uint ndirs = 1;
    dirs[0] = root->dir;
    names[0] = root->name;
    dir_of[0] = 0;
    for (uint i = 0; i < nfiles; i++) {
        char *dir = files[i].dir;
        char *name = files[i].name;
        if (name[0] == 0) {
            dir = "";
            name = "<stdin>";
        } else {
            if (strcmp(dir, comp_dir) == 0) {
                dir = "";
            }
            if (dir[0] == 0) {
                char *parent = parent_path(name);
                if (parent[0] != 0) {
                    dir = parent;
                    name = file_name(name);
                }
            }
        }
        uint index = 0;
        if (dir[0] != 0) {
            index = ndirs;
            for (uint d = 1; d < ndirs; d++) {
                if (strcmp(dirs[d], dir) == 0) {
                    index = d;
                    break;
                }
            }
            if (index == ndirs) {
                dirs[ndirs++] = dir;
            }
        }
        names[i + 1] = name;
        dir_of[i + 1] = index;
    }
    return ndirs;
}

/* ---- Strings and fixups ---- */

/* `.debug_line_str`'s strings, each once, in the order first added. */
struct strings {
    uint base;
    struct buf *bytes;
    char **seen;
    uint *offsets;
    uint n;
};

static uint string_add(struct strings *s, char *text) {
    for (uint i = 0; i < s->n; i++) {
        if (strcmp(s->seen[i], text) == 0) {
            return s->offsets[i];
        }
    }
    uint off = s->base + s->bytes->len;
    buf_put(s->bytes, text, (uint)strlen(text) + 1);
    s->seen[s->n] = text;
    s->offsets[s->n] = off;
    s->n++;
    return off;
}

/* A field of `size` bytes to relocate at `out`'s end (plus base): a 4-byte
 * offset `off` of `.debug_line_str` (section -1), or an address, offset
 * `off` of a section, as wide as an address (4 or 8 bytes). */
static void fixup(struct line_table *t, uint base, int section, uint off, uint size) {
    t->fixups = grow(t->fixups, &t->cap, t->nfixups + 1, sizeof(struct dfixup));
    struct dfixup *f = &t->fixups[t->nfixups++];
    f->at = base + t->line.len;
    f->section = section;
    f->off = off;
    buf_zeros(&t->line, size);
}

/* ---- The line number program ---- */

/* Adds a row `line_delta` lines and `addr_delta` bytes after the last one,
 * in as few bytes as LLVM does. A special opcode `op` (13..255) adds
 * LINE_BASE + (op - 13) % 14 lines and (op - 13) / 14 bytes and makes the
 * row; a line delta out of its range goes first with DW_LNS_advance_line,
 * an address delta up to 17 bytes too big with DW_LNS_const_add_pc (special
 * opcode 255's advance), a bigger one with DW_LNS_advance_pc. */
static void advance(struct buf *out, int64_t line_delta, uint addr_delta) {
    int temp;
    int need_copy = 0;
    if (line_delta >= LINE_BASE && line_delta < LINE_BASE + LINE_RANGE) {
        temp = (int)line_delta - LINE_BASE;
    } else {
        buf_byte(out, ADVANCE_LINE);
        sleb(out, line_delta);
        line_delta = 0;
        temp = -LINE_BASE;
        need_copy = 1;
    }
    if (line_delta == 0 && addr_delta == 0) {
        buf_byte(out, COPY);
        return;
    }
    temp += OPCODE_BASE;
    if (addr_delta < 256 + MAX_SPECIAL_ADDR) {
        uint op = (uint)temp + addr_delta * LINE_RANGE;
        if (op <= 255) {
            buf_byte(out, op);
            return;
        }
        if (addr_delta >= MAX_SPECIAL_ADDR) {
            op = (uint)temp + (addr_delta - MAX_SPECIAL_ADDR) * LINE_RANGE;
            if (op <= 255) {
                buf_byte(out, CONST_ADD_PC);
                buf_byte(out, op);
                return;
            }
        }
    }
    buf_byte(out, ADVANCE_PC);
    uleb(out, addr_delta);
    buf_byte(out, need_copy ? COPY : (uint)temp);
}

/* One sequence: the rows of section `sec`, which ends at `end`. Its
 * addresses are `address_size` bytes. */
static void program(struct line_table *t, uint base, int sec, uint end, struct row *rows, uint nrows,
                    uint address_size) {
    struct buf *out = &t->line;
    uint file = 1, line = 1, column = 0, flags = LOC_IS_STMT, isa = 0;
    int have_last = 0;
    uint last = 0;
    for (uint k = 0; k < nrows; k++) {
        if (rows[k].section != sec) {
            continue;
        }
        struct loc *loc = &rows[k].loc;
        uint offset = rows[k].offset;
        if (file != loc->file) {
            file = loc->file;
            buf_byte(out, SET_FILE);
            uleb(out, file);
        }
        if (column != loc->column) {
            column = loc->column;
            buf_byte(out, SET_COLUMN);
            uleb(out, column);
        }
        if (loc->discriminator != 0) {
            struct buf operand;
            memset(&operand, 0, sizeof operand);
            uleb(&operand, loc->discriminator);
            buf_byte(out, 0);
            uleb(out, operand.len + 1);
            buf_byte(out, SET_DISCRIMINATOR);
            buf_put(out, operand.data, operand.len);
            free(operand.data);
        }
        if (isa != loc->isa) {
            isa = loc->isa;
            buf_byte(out, SET_ISA);
            uleb(out, isa);
        }
        if (((loc->flags ^ flags) & LOC_IS_STMT) != 0) {
            flags = loc->flags;
            buf_byte(out, NEGATE_STMT);
        }
        if (loc->flags & LOC_BASIC_BLOCK) {
            buf_byte(out, SET_BASIC_BLOCK);
        }
        if (loc->flags & LOC_PROLOGUE_END) {
            buf_byte(out, SET_PROLOGUE_END);
        }
        if (loc->flags & LOC_EPILOGUE_BEGIN) {
            buf_byte(out, SET_EPILOGUE_BEGIN);
        }
        int64_t line_delta = (int64_t)loc->line - (int64_t)line;
        if (!have_last) {
            /* An extended opcode: 0, its length (the opcode and the
             * address), DW_LNE_set_address, then the address itself. */
            buf_byte(out, 0);
            buf_byte(out, 1 + address_size);
            buf_byte(out, SET_ADDRESS);
            fixup(t, base, sec, offset, address_size);
            advance(out, line_delta, 0);
        } else {
            advance(out, line_delta, offset - last);
        }
        line = loc->line;
        last = offset;
        have_last = 1;
    }
    /* The end: one past the section's last byte. */
    uint delta = end - (have_last ? last : end);
    if (delta == MAX_SPECIAL_ADDR) {
        buf_byte(out, CONST_ADD_PC);
    } else if (delta != 0) {
        buf_byte(out, ADVANCE_PC);
        uleb(out, delta);
    }
    buf_byte(out, 0);
    buf_byte(out, 1);
    buf_byte(out, END_SEQUENCE);
}

/* ---- The table ---- */

void dwarf_encode(struct dfile *root, struct dfile *files, uint nfiles, struct row *rows, uint nrows,
                  struct section *sections, uint nsections, uint base, uint str_base, uint address_size,
                  struct line_table *t) {
    memset(t, 0, sizeof *t);
    uint nentries = nfiles + 1;
    char **dirs = tmp_alloc(nentries * sizeof(char *));
    char **names = tmp_alloc(nentries * sizeof(char *));
    uint *dir_of = tmp_alloc(nentries * sizeof(uint));
    uint ndirs = file_tables(root, files, nfiles, dirs, names, dir_of);
    int md5 = root->has_md5;
    for (uint i = 0; i < nfiles; i++) {
        md5 = md5 && files[i].has_md5;
    }
    struct strings strings;
    strings.base = str_base;
    strings.bytes = &t->line_str;
    strings.seen = tmp_alloc((ndirs + nentries) * sizeof(char *));
    strings.offsets = tmp_alloc((ndirs + nentries) * sizeof(uint));
    strings.n = 0;

    struct buf *out = &t->line;
    buf_u32(out, 0); /* unit_length, filled in last */
    buf_u16(out, 5); /* version */
    buf_byte(out, address_size);
    buf_byte(out, 0); /* segment_selector_size */
    uint header_length_at = out->len;
    buf_u32(out, 0);
    buf_byte(out, 1); /* minimum_instruction_length */
    buf_byte(out, 1); /* maximum_operations_per_instruction */
    buf_byte(out, 1); /* default_is_stmt */
    buf_byte(out, (uint)LINE_BASE);
    buf_byte(out, LINE_RANGE);
    buf_byte(out, OPCODE_BASE);
    buf_put(out, STANDARD_LENGTHS, 12);

    /* The directories, then the files, each name a .debug_line_str offset. */
    buf_byte(out, 1);
    buf_byte(out, LNCT_PATH);
    buf_byte(out, FORM_LINE_STRP);
    uleb(out, ndirs);
    for (uint d = 0; d < ndirs; d++) {
        fixup(t, base, -1, string_add(&strings, dirs[d]), 4);
    }
    buf_byte(out, md5 ? 3 : 2);
    buf_byte(out, LNCT_PATH);
    buf_byte(out, FORM_LINE_STRP);
    buf_byte(out, LNCT_DIRECTORY_INDEX);
    buf_byte(out, FORM_UDATA);
    if (md5) {
        buf_byte(out, LNCT_MD5);
        buf_byte(out, FORM_DATA16);
    }
    uleb(out, nentries);
    for (uint i = 0; i < nentries; i++) {
        fixup(t, base, -1, string_add(&strings, names[i]), 4);
        uleb(out, dir_of[i]);
        if (md5) {
            buf_put(out, i == 0 ? root->md5 : files[i - 1].md5, 16);
        }
    }
    uint header_length = out->len - header_length_at - 4;
    for (uint b = 0; b < 4; b++) {
        out->data[header_length_at + b] = (uchar)(header_length >> (8 * b));
    }

    /* A sequence per section, in the order their first rows came. */
    int *order = tmp_alloc(nsections * sizeof(int));
    uint nseq = 0;
    for (uint k = 0; k < nrows; k++) {
        uint j = 0;
        while (j < nseq && order[j] != rows[k].section) {
            j++;
        }
        if (j == nseq) {
            order[nseq++] = rows[k].section;
        }
    }
    for (uint j = 0; j < nseq; j++) {
        program(t, base, order[j], sections[order[j]].size, rows, nrows, address_size);
    }
    uint unit_length = out->len - 4;
    for (uint b = 0; b < 4; b++) {
        out->data[b] = (uchar)(unit_length >> (8 * b));
    }
}
