/* Fixed-width types for the kernel. No libc: these replace <stdint.h>,
 * <stddef.h> and <stdbool.h>. The kernel builds for both widths while RV32
 * and RV64 exist side by side (#141): RV32 is ILP32 (int, long and pointers
 * are 32 bits), RV64 LP64 (int is 32 bits, long and pointers 64). u64 and
 * i64 are 64 bits on both, as Linux's are: `long long`, which merlin has
 * since #152 (on RV32 in register pairs, with libgcc.c's helpers). */
#ifndef CAMELOT_TYPES_H
#define CAMELOT_TYPES_H

typedef unsigned char u8;
typedef unsigned short u16;
typedef unsigned int u32;
typedef unsigned long long u64;
typedef signed char i8;
typedef signed short i16;
typedef signed int i32;
typedef signed long long i64;

/* Pointer-sized integers and object sizes: a register's width. */
typedef unsigned long usize;
typedef signed long isize;
typedef unsigned long uintptr;

/* The system call ABI's types, Linux's (docs/camelot-syscalls.md): a file
 * offset (off_t) and a time in seconds (time_t) are a `long`, so 64 bits on
 * RV64 (LP64) and 32 on RV32 (ILP32, where an offset past 2^31 - 1 has no
 * value, as on 32-bit Linux without O_LARGEFILE). A user address is a
 * uintptr, a byte count a usize and a count or an error an isize (size_t
 * and ssize_t). */
typedef long off_t;
typedef long time_t;

/* A physical address, as page-table entries, satp and the devices' DMA
 * registers take them; a kernel pointer is a virtual address, and pa() and
 * va() (vm.h) convert between the two, as Linux's phys_addr_t, __pa and
 * __va. On RV32 the kernel's mappings are the identity, and the two are the
 * same number. */
typedef uintptr phys_addr_t;

typedef int bool;
#define true 1
#define false 0

#define NULL ((void *)0)

/* The offset of `field` in `type`, as <stddef.h>'s: for the _Static_asserts
 * that tie a structure to the assembly (and the tests) reading it. */
#define offsetof(type, field) ((usize) &((type *)0)->field)

#endif
