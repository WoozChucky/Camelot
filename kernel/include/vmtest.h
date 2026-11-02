/* Self-tests for kalloc and paging (vmtest.c). */
#ifndef CAMELOT_VMTEST_H
#define CAMELOT_VMTEST_H

/* Runs them with paging on, prints "vm self-test: N/M passed" and returns the
 * number of failures. Leaves the free list and the kernel page table as it
 * found them, except for one second-level table it adds to the kernel's. */
int vm_selftest(void);

#endif
