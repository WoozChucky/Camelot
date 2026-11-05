/* The ELF loader: puts an executable's PT_LOAD segments into a process's user
 * space. The executables come from the filesystem, or from the kernel itself
 * (userprogs.s, which `cargo xtask kernel` generates) when there is none,
 * and are checked as untrusted either way: a bad image is an error, never a
 * panic or a mapping outside user space. Everything is copied out through
 * the elf_file's read(), so an image in memory needs no particular
 * alignment, and a file is read piece by piece rather than whole. */
#include "elf.h"
#include "kalloc.h"
#include "memlayout.h"
#include "string.h"

/* What a segment's flags allow, as PTE permissions (always with U). Returns 0
 * for flags the loader refuses: a segment must be readable. */
static u32 segment_perm(u32 flags) {
    if ((flags & PF_R) == 0) {
        return 0;
    }
    u32 perm = PTE_U | PTE_R;
    if (flags & PF_W) {
        perm |= PTE_W;
    }
    if (flags & PF_X) {
        perm |= PTE_X;
    }
    return perm;
}

static bool read_memory(const struct elf_file *f, void *dst, u64 off, u32 n) {
    memcpy(dst, f->image + off, n);
    return true;
}

void elf_from_memory(struct elf_file *f, const u8 *image, u32 size) {
    f->size = size;
    f->read = read_memory;
    f->image = image;
    f->inode = NULL;
}

/* Maps the pages of segment `ph` into `pt`, filled from `f`. */
static const char *load_segment(pagetable_t pt, const struct elf_file *f, const elf_phdr *ph) {
    u32 perm = segment_perm(ph->flags);
    if (perm == 0) {
        return "segment not readable";
    }
    /* In user space below the stack's guard page. The second test can't
     * overflow once the first holds. */
    if (ph->vaddr < USER_BASE || ph->vaddr >= USER_LOAD_TOP || ph->memsz > USER_LOAD_TOP - ph->vaddr) {
        return "segment outside user space";
    }
    uintptr vaddr = (uintptr)ph->vaddr;
    uintptr end = vaddr + (uintptr)ph->memsz;
    for (uintptr page = PGROUNDDOWN(vaddr); page < end; page += PGSIZE) {
        pte_t *pte = walk(pt, page, false);
        if (pte != NULL && (*pte & PTE_V)) {
            return "segments share a page";
        }
        u8 *mem = kzalloc();
        if (mem == NULL) {
            return ELF_OUT_OF_MEMORY;
        }
        /* The part of the file's bytes [vaddr, vaddr + filesz) on this page;
         * the rest of it stays zero (.bss, and the page around the segment). */
        uintptr from = page > vaddr ? page : vaddr;
        uintptr file_end = vaddr + (uintptr)ph->filesz;
        uintptr to = page + PGSIZE < file_end ? page + PGSIZE : file_end;
        if (from < to && !f->read(f, mem + (from - page), ph->offset + (from - vaddr), (u32)(to - from))) {
            kfree(mem);
            return "segment outside the file";
        }
        if (map_pages(pt, page, pa(mem), PGSIZE, perm) != 0) {
            kfree(mem);
            return ELF_OUT_OF_MEMORY;
        }
    }
    return NULL;
}

const char *elf_load(pagetable_t pt, const struct elf_file *f, uintptr *entry, uintptr *end) {
    u64 size = f->size;
    elfhdr eh;
    if (size < sizeof eh || !f->read(f, &eh, 0, sizeof eh)) {
        return "not an ELF file";
    }
    if (eh.magic != ELF_MAGIC) {
        return "not an ELF file";
    }
    if (eh.class != ELF_CLASS || eh.data != ELFDATA2LSB || eh.machine != EM_RISCV) {
        return "not a " ELF_CLASS_BITS " little-endian RISC-V file";
    }
    if (eh.type != ET_EXEC) {
        return "not an executable";
    }
    if (eh.phentsize != sizeof(elf_phdr) || eh.phnum > ELF_MAX_PHNUM || eh.phoff > size ||
        eh.phnum * sizeof(elf_phdr) > size - eh.phoff) {
        return "bad program headers";
    }

    uintptr top = USER_BASE;
    for (u32 i = 0; i < eh.phnum; i++) {
        elf_phdr ph;
        if (!f->read(f, &ph, eh.phoff + i * sizeof ph, sizeof ph)) {
            return "bad program headers";
        }
        if (ph.type != PT_LOAD || ph.memsz == 0) {
            continue;
        }
        if (ph.filesz > ph.memsz || ph.offset > size || ph.filesz > size - ph.offset) {
            return "segment outside the file";
        }
        const char *err = load_segment(pt, f, &ph);
        if (err != NULL) {
            return err;
        }
        if (PGROUNDUP(ph.vaddr + ph.memsz) > top) {
            top = PGROUNDUP(ph.vaddr + ph.memsz);
        }
    }

    /* The entry point must be code the program can run, on an instruction
     * boundary: sepc would quietly drop its low bits (IALIGN is 32), and the
     * program start somewhere other than where its header says. */
    if (eh.entry % 4 != 0) {
        return "misaligned entry point";
    }
    u32 want = PTE_V | PTE_U | PTE_X;
    pte_t *pte = eh.entry < USER_TOP ? walk(pt, (uintptr)eh.entry, false) : NULL;
    if (pte == NULL || (*pte & want) != want) {
        return "entry point not in an executable segment";
    }
    *entry = (uintptr)eh.entry;
    *end = top;
    return NULL;
}
