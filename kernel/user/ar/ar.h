/* ar: the archiver that runs on Camelot (/bin/ar), the elf crate's
 * archive.rs in the C subset merlin compiles (#240). It makes archives
 * (`ar rcs libx.a a.o b.o`) byte for byte as llvm-ar makes them for the
 * same members, in its default deterministic mode, lists them (`ar t`) and
 * takes members out (`ar x`). The format, the System V one with GNU's long
 * names, is described in crates/elf/src/archive.rs:
 *
 *   main.c     the command line: the operation and its modifiers
 *   archive.c  reading and writing archives, ar r's replacing
 *   symbols.c  the symbols an ELF object defines, for the symbol table
 *
 * crates/xtask/tests/selfhost.rs checks its archives against llvm-ar's
 * (the elf crate's fixtures) and against the elf crate's writer. */
#ifndef AR_H
#define AR_H

#include "../user.h"

void die(const char *msg);
void *xmalloc(uint n);
char *xstrndup(const char *s, uint n);
/* A growable array: grow(ptr, &cap, count + 1, size) makes room. */
void *grow(void *p, uint *cap, uint need, uint size);

struct buf {
    uchar *data;
    uint len;
    uint cap;
};
void buf_put(struct buf *b, const void *p, uint n);
void buf_byte(struct buf *b, uint v);

/* A member: its name in the archive and its bytes. */
struct member {
    char *name;
    uchar *data;
    uint len;
};

/* Whether `data` is an archive ("!<arch>\n"). */
int is_archive(const uchar *data, uint len);
/* An archive's members, in order (the elf crate's archive::parse). 0 with
 * *err set if it is corrupt. The members' bytes point into `data`. */
int archive_read(uchar *data, uint len, struct member **out, uint *count, char **err);
/* The archive of the n members, in order, with a symbol table if
 * `symtab` (the elf crate's archive::write_with). 0 with *err set if a
 * member is an ELF file that does not parse. */
int archive_write(struct member *m, uint n, int symtab, struct buf *out, char **err);
/* ar r: the members an archive that had `old` (nold of them) holds once
 * the nfiles `files` are put in it (the elf crate's archive::replace): an
 * old member a file of its name still unused replaces in its place, the
 * files left over at the end. */
struct member *archive_replace(struct member *old, uint nold, struct member *files, uint nfiles, uint *count);

/* The names of the global or weak symbols an ELF object (ELF32 or ELF64)
 * defines, in its symbol table's order, each followed by a NUL, appended
 * to `names`; their count added to *count. Nothing for a file that is not
 * ELF. 0 with *err set for an ELF file that does not parse. */
int defined_symbols(const uchar *data, uint len, struct buf *names, uint *count, char **err);

#endif
