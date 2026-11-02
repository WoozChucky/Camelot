/* Fixed-width types for the kernel. No libc: these replace <stdint.h>,
 * <stddef.h> and <stdbool.h>. Excalibur is ILP32: int, long and pointers are
 * 32 bits. 64-bit types are left out until Merlin grows `long long` (#15). */
#ifndef CAMELOT_TYPES_H
#define CAMELOT_TYPES_H

typedef unsigned char u8;
typedef unsigned short u16;
typedef unsigned int u32;
typedef signed char i8;
typedef signed short i16;
typedef signed int i32;

/* Pointer-sized integers and object sizes. */
typedef unsigned long usize;
typedef signed long isize;
typedef unsigned long uintptr;

typedef int bool;
#define true 1
#define false 0

#define NULL ((void *)0)

#endif
