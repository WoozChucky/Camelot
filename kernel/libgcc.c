/* The routines compilers call for 64-bit division on RV32 (__divdi3,
 * __udivdi3, __moddi3, __umoddi3, and __muldi3): the C library's, so the
 * kernel has the same ones. See user/lib/libgcc.c. */
#include "user/lib/libgcc.c"
