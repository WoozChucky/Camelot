/* Mounting the root filesystem at boot, and its self-test (fstest.c). */
#ifndef CAMELOT_FSTEST_H
#define CAMELOT_FSTEST_H

#include "types.h"

/* What the self-test does, from `fs_test` in fstest.c: the image ships with
 * FS_TEST_READ; tests patch it to FS_TEST_WRITE for the transactions (on a
 * scratch disk: they leave /fstest behind). */
#define FS_TEST_READ 1
#define FS_TEST_WRITE 2

extern volatile u32 fs_test;

/* With a disk attached, mounts disk 0 as the root filesystem in a kernel
 * process (replaying its log if needed), lists `/`, runs the self-test and
 * prints "fs self-test: N/M passed"; returns the number of failures. A disk
 * without a filesystem, or one that can't be mounted, is reported and left
 * alone (0 failures). Needs blk_init(), binit() and interrupts on. */
int fs_boot(void);

#endif
