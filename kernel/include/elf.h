/* The ELF32 executables the kernel loads into user space (elf.c), from a
 * file or from the kernel image. Only what the loader reads: the file header
 * and the program headers. */
#ifndef CAMELOT_ELF_H
#define CAMELOT_ELF_H

#include "types.h"
#include "vm.h"

struct inode;

#define ELF_MAGIC 0x464c457fu /* "\x7fELF", read as a little-endian word */
#define ELFCLASS32 1
#define ELFDATA2LSB 1
#define ET_EXEC 2
#define EM_RISCV 243

#define PT_LOAD 1
#define PF_X 1
#define PF_W 2
#define PF_R 4

/* Most program headers the loader looks at. */
#define ELF_MAX_PHNUM 16

struct elf32_ehdr {
    u32 magic;
    u8 class;
    u8 data;
    u8 version;
    u8 ident_rest[9];
    u16 type;
    u16 machine;
    u32 version2;
    u32 entry;
    u32 phoff;
    u32 shoff;
    u32 flags;
    u16 ehsize;
    u16 phentsize;
    u16 phnum;
    u16 shentsize;
    u16 shnum;
    u16 shstrndx;
};

struct elf32_phdr {
    u32 type;
    u32 offset;
    u32 vaddr;
    u32 paddr;
    u32 filesz;
    u32 memsz;
    u32 flags;
    u32 align;
};

/* What elf_load() returns when it runs out of pages (any other reason is
 * something wrong with the image). */
#define ELF_OUT_OF_MEMORY "out of memory"

/* An executable to load: `size` bytes that `read` copies out, from an image
 * in memory (an embedded program) or a file (exec.c). */
struct elf_file {
    u32 size;
    /* Copies the `n` bytes at `off` (off + n <= size) to `dst`; false if
     * it can't. */
    bool (*read)(const struct elf_file *f, void *dst, u32 off, u32 n);
    const u8 *image;     /* an image in memory */
    struct inode *inode; /* a file, locked */
};

/* Loads the executable `f` into the user part of `pt`: every PT_LOAD
 * segment goes into fresh zeroed pages, mapped with U and the segment's
 * permissions. Returns NULL and sets *entry and *end (the page boundary
 * after the highest segment, where the heap can start), or returns what is
 * wrong with the image. Segments must lie in [USER_BASE, USER_LOAD_TOP) and
 * not share pages, and the entry point must be 4-byte aligned and in an
 * executable one. On an error, the pages mapped so far stay in `pt` for
 * uvm_free() to release. */
const char *elf_load(pagetable_t pt, const struct elf_file *f, u32 *entry, u32 *end);

/* An elf_file for the `size`-byte image at `image`. */
void elf_from_memory(struct elf_file *f, const u8 *image, u32 size);

#endif
