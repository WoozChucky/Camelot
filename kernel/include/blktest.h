/* Self-test for the disk driver and the buffer cache (blktest.c). */
#ifndef CAMELOT_BLKTEST_H
#define CAMELOT_BLKTEST_H

/* With a disk attached, runs the tests on disk 0 in kernel processes,
 * prints "blk self-test: N/M passed" and returns the number of failures.
 * Leaves the disk as it found it. Without a disk, does nothing and returns
 * 0. Needs blk_init(), binit() and interrupts on. */
int blk_selftest(void);

#endif
