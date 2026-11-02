/* Physical memory: pages, and where the kernel image sits in RAM (kernel.ld).
 * RAM itself is discovered at boot from sysinfo (kalloc.c). */
#ifndef CAMELOT_MEMLAYOUT_H
#define CAMELOT_MEMLAYOUT_H

#include "types.h"

#define PGSIZE 4096u
#define PGSHIFT 12

#define PGROUNDUP(a) (((a) + PGSIZE - 1) & ~(PGSIZE - 1))
#define PGROUNDDOWN(a) ((a) & ~(PGSIZE - 1))

/* Linker symbols (kernel.ld), all page aligned. Only their addresses mean
 * anything: text from __text_start (= _start = 0x8000_0000), read-only data
 * from __rodata_start, writable data, .bss and the boot stack from
 * __data_start, and free RAM from __kernel_end. */
extern char __text_start[];
extern char __rodata_start[];
extern char __data_start[];
extern char __kernel_end[];

#endif
