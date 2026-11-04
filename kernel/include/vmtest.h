/* Self-tests for kalloc and paging (vmtest.c). */
#ifndef CAMELOT_VMTEST_H
#define CAMELOT_VMTEST_H

/* Runs them with paging on, prints "vm self-test: N/M passed" and returns the
 * number of failures. Leaves the free list and the kernel page table as it
 * found them, except for one second-level table it adds to the kernel's. */
int vm_selftest(void);

#if __riscv_xlen == 64
/* Runs rounds of NPROC kernel processes that end at once, on this hart
 * alone, and checks that their kernel stacks come back: those this hart
 * does not cache unmapped, their pages free again (vm.h, kstack_free()).
 * Prints "kstack self-test: N/M passed" and returns the number of
 * failures. After proc_init(), with interrupts on. */
int kstack_selftest(void);
#endif

#endif
