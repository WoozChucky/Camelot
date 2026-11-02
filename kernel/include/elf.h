/* The ELF32 executables the kernel loads into user space (elf.c). Only what
 * the loader reads: the file header and the program headers. */
#ifndef CAMELOT_ELF_H
#define CAMELOT_ELF_H

#include "types.h"
#include "vm.h"

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

/* Loads the executable `image` (`size` bytes) into the user part of `pt`:
 * every PT_LOAD segment goes into fresh zeroed pages, mapped with U and the
 * segment's permissions. Returns NULL and sets *entry, or returns what is
 * wrong with the image. Segments must lie in [USER_BASE, USER_LOAD_TOP) and
 * not share pages, and the entry point must be in an executable one. On an
 * error, the pages mapped so far stay in `pt` for uvm_free() to release. */
const char *elf_load(pagetable_t pt, const u8 *image, u32 size, u32 *entry);

#endif
