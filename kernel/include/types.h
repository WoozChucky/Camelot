/* Fixed-width types for the kernel. No libc: these replace <stdint.h>,
 * <stddef.h> and <stdbool.h>. The kernel is LP64, as Linux's riscv64 is:
 * int is 32 bits, long and pointers 64. u64 and i64 are `long long`, as
 * Linux's are. */
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

/* The system call ABI's types (docs/camelot-syscalls.md): a file offset
 * (off_t) and a time in seconds (time_t) are a `long`, 64 bits, as Linux's.
 * A user address is a uintptr, a byte count a usize and a count or an error
 * an isize (size_t and ssize_t). */
typedef long off_t;
typedef long time_t;

/* A physical address, as page-table entries, satp and the devices' DMA
 * registers take them; a kernel pointer is a virtual address, and pa() and
 * va() (vm.h) convert between the two, as Linux's phys_addr_t, __pa and
 * __va. */
typedef uintptr phys_addr_t;

typedef int bool;
#define true 1
#define false 0

#define NULL ((void *)0)

/* The offset of `field` in `type`, as <stddef.h>'s: for the _Static_asserts
 * that tie a structure to the assembly (and the tests) reading it. */
#define offsetof(type, field) ((usize) &((type *)0)->field)

#endif
