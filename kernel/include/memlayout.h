/* Physical memory: pages, and where the kernel image sits in RAM (kernel.ld).
 * RAM itself is discovered at boot from sysinfo (kalloc.c). */
#ifndef CAMELOT_MEMLAYOUT_H
#define CAMELOT_MEMLAYOUT_H

#include "types.h"

/* An unsigned long, a register wide: ~(PGSIZE - 1) must keep the upper bits
 * of a 64-bit address. */
#define PGSIZE 4096ul
#define PGSHIFT 12

#define PGROUNDUP(a) (((a) + PGSIZE - 1) & ~(PGSIZE - 1))
#define PGROUNDDOWN(a) ((a) & ~(PGSIZE - 1))

/* Linker symbols (kernel.ld, kernel64.ld), all page aligned. Only their
 * addresses mean anything: text from __text_start (= _start, loaded at
 * 0x8000_0000: on RV32 also its address, on RV64 linked at
 * 0xffff_ffff_8000_0000), read-only data from __rodata_start, writable data,
 * .bss and the boot stack from __data_start, and free RAM from
 * __kernel_end. Hart 0's boot stack is [__stack_bottom, __stack_top). */
extern char __text_start[];
extern char __rodata_start[];
extern char __data_start[];
extern char __stack_bottom[];
extern char __stack_top[];
extern char __kernel_end[];

#endif
