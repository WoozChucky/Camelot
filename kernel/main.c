/* Camelot kernel: C entry point. entry.s sets up the stack and clears .bss,
 * start() drops to S-mode, then supervisor_entry calls kmain(); its return
 * value becomes the exit code. After booting, kmain runs the user-space shell
 * as the first process, and returns when it exits. */
#include "bio.h"
#include "blk.h"
#include "blktest.h"
#include "fstest.h"
#include "kalloc.h"
#include "kprintf.h"
#include "memlayout.h"
#include "mmio.h"
#include "proc.h"
#include "riscv.h"
#include "string.h"
#include "timer.h"
#include "trap.h"
#include "types.h"
#include "uart.h"
#include "vm.h"
#include "vmtest.h"
#include "xcal.h"

#define CAMELOT_VERSION "0.1.0"

/* Timer ticks to wait for before starting the shell. */
#define BOOT_TICKS 10

/* The first process: the user-space shell. */
#define SHELL "sh"

/* Deliberate faults, to exercise the panic path. The image ships with
 * BOOT_FAULT_NONE; tests patch `boot_fault` in it to pick another one. The
 * value is non-zero so the variable lives in .data, which is part of the
 * image, rather than in .bss, which entry.s clears. */
#define BOOT_FAULT_NONE 1
#define BOOT_FAULT_ILLEGAL 2 /* illegal_instruction(): unimp, registers marked */
#define BOOT_FAULT_LOAD 3        /* a load from UNMAPPED_ADDR: load page fault */
#define BOOT_FAULT_STORE_TEXT 4  /* a store to kmain's code: store page fault */
/* Misuses of the memory API, each caught by a check that panics. */
#define BOOT_FAULT_REMAP 5          /* map_pages over the kernel text */
#define BOOT_FAULT_UNMAP 6          /* unmap_pages of unmapped page 0 */
#define BOOT_FAULT_W_ONLY 7         /* map_pages with W but not R */
#define BOOT_FAULT_MEGAPAGE 8       /* walk into a 4 MiB leaf in the root */
#define BOOT_FAULT_KFREE_UNALIGNED 9 /* kfree of a pointer inside a page */
#define BOOT_FAULT_KFREE_KERNEL 10  /* kfree of the kernel's first page */
#define BOOT_FAULT_KFREE_PAST_RAM 11 /* kfree of the page after the end of RAM */

/* Nothing is mapped below the boot ROM (spec section 7), so the kernel page
 * table doesn't map it either. */
#define UNMAPPED_ADDR 0x00000100

/* A 4 MiB region with nothing in it (spec section 7), for BOOT_FAULT_MEGAPAGE. */
#define EMPTY_REGION 0x50000000

volatile u32 boot_fault = BOOT_FAULT_NONE;

int kmain(void);

static char buf[64];
static int passed;
static int failed;

/* Compares the last ksnprintf result (in `buf`, length `len`) with `want`. */
static void expect(int len, const char *want) {
    if (strcmp(buf, want) == 0 && len == (int)strlen(want)) {
        passed++;
    } else {
        failed++;
        kprintf("kprintf self-test FAILED: got \"%s\" (%d), want \"%s\"\n", buf, len, want);
    }
}

static void kprintf_selftest(void) {
    expect(ksnprintf(buf, sizeof buf, "%d", 0), "0");
    expect(ksnprintf(buf, sizeof buf, "%d", 42), "42");
    expect(ksnprintf(buf, sizeof buf, "%d", -42), "-42");
    expect(ksnprintf(buf, sizeof buf, "%d", 2147483647), "2147483647");
    expect(ksnprintf(buf, sizeof buf, "%d", (i32)0x80000000u), "-2147483648");
    expect(ksnprintf(buf, sizeof buf, "%u", 0u), "0");
    expect(ksnprintf(buf, sizeof buf, "%u", 4294967295u), "4294967295");
    expect(ksnprintf(buf, sizeof buf, "%x", 0u), "0");
    expect(ksnprintf(buf, sizeof buf, "%x", 0xdeadbeefu), "deadbeef");
    expect(ksnprintf(buf, sizeof buf, "%08x", 0x2au), "0000002a");
    expect(ksnprintf(buf, sizeof buf, "%5d", -42), "  -42");
    expect(ksnprintf(buf, sizeof buf, "%05d", -42), "-0042");
    expect(ksnprintf(buf, sizeof buf, "%2u", 12345u), "12345");
    expect(ksnprintf(buf, sizeof buf, "%lu", 7u), "7");
    expect(ksnprintf(buf, sizeof buf, "%p", (void *)0x80000000u), "0x80000000");
    expect(ksnprintf(buf, sizeof buf, "%p", NULL), "0x00000000");
    expect(ksnprintf(buf, sizeof buf, "%s", "excalibur"), "excalibur");
    expect(ksnprintf(buf, sizeof buf, "%4s|", "ab"), "  ab|");
    expect(ksnprintf(buf, sizeof buf, "%s", (const char *)NULL), "(null)");
    expect(ksnprintf(buf, sizeof buf, "%c%c", 'o', 'k'), "ok");
    expect(ksnprintf(buf, sizeof buf, "100%%"), "100%");
    expect(ksnprintf(buf, sizeof buf, "%q %5y"), "%q %5y");
    expect(ksnprintf(buf, sizeof buf, "end %"), "end %");
    expect(ksnprintf(buf, sizeof buf, "%s=%d", "x", -1), "x=-1");
    /* More arguments than a0-a7 hold: the rest come from the stack. */
    expect(ksnprintf(buf, sizeof buf, "%d %d %d %d %d %d %d %d %d %d", 1, 2, 3, 4, 5, 6, 7, 8, 9, 10),
           "1 2 3 4 5 6 7 8 9 10");

    /* Truncation: returns the full length, keeps size-1 chars and a NUL. */
    int n = ksnprintf(buf, 4, "%s", "abcdef");
    if (n == 6 && strcmp(buf, "abc") == 0) {
        passed++;
    } else {
        failed++;
        kprintf("kprintf self-test FAILED: truncation gave \"%s\" (%d)\n", buf, n);
    }
}

/* Idles in wfi until `count` timer ticks have passed, printing "tick N" for
 * each one. The lines count the ticks rather than sample `ticks`, so none is
 * skipped when several arrive while a line is printed. Needs interrupts on
 * (trap_init). */
static void wait_ticks(u32 count) {
    u32 printed = 0;
    while (printed < count) {
        /* Interrupts off from the check to wfi: a tick taken in between would
         * leave wfi asleep for a whole interval. wfi still wakes on the
         * pending tick, which is taken as soon as intr_on() runs. */
        intr_off();
        if (ticks == printed) {
            wfi();
        }
        intr_on();
        if (ticks != printed) {
            printed++;
            kprintf("tick %u\n", printed);
        }
    }
}

/* The BOOT_FAULT_ values from 5 up: each call should panic. */
static void memory_misuse(void) {
    u32 text = (u32)(uintptr)__text_start;
    if (boot_fault == BOOT_FAULT_REMAP) {
        map_pages(kernel_pagetable, text, text, PGSIZE, PTE_R);
    } else if (boot_fault == BOOT_FAULT_UNMAP) {
        /* Page 0 shares its 4 MiB region with the test device, so this finds
         * an invalid PTE in an existing second-level table. */
        unmap_pages(kernel_pagetable, 0, PGSIZE, false);
    } else if (boot_fault == BOOT_FAULT_W_ONLY) {
        map_pages(kernel_pagetable, 0, 0, PGSIZE, PTE_W);
    } else if (boot_fault == BOOT_FAULT_MEGAPAGE) {
        kernel_pagetable[VPN(EMPTY_REGION, 1)] = PA2PTE(EMPTY_REGION) | PTE_R | PTE_V;
        walk(kernel_pagetable, EMPTY_REGION, false);
    } else if (boot_fault == BOOT_FAULT_KFREE_UNALIGNED) {
        kfree((char *)kalloc() + 4);
    } else if (boot_fault == BOOT_FAULT_KFREE_KERNEL) {
        kfree(__text_start);
    } else if (boot_fault == BOOT_FAULT_KFREE_PAST_RAM) {
        kfree((void *)(uintptr)(ram_base + ram_size));
    } else {
        return;
    }
    kprintf("memory misuse %u didn't panic\n", boot_fault);
    failed++;
}

static void deliberate_fault(void) {
    if (boot_fault == BOOT_FAULT_ILLEGAL) {
        illegal_instruction();
    } else if (boot_fault == BOOT_FAULT_LOAD) {
        u32 v = mmio_read32(UNMAPPED_ADDR);
        kprintf("load from %p returned %x instead of faulting\n", (void *)UNMAPPED_ADDR, v);
        failed++;
    } else if (boot_fault == BOOT_FAULT_STORE_TEXT) {
        mmio_write32((uintptr)kmain, 0);
        kprintf("store to kernel text at %p didn't fault\n", (void *)(uintptr)kmain);
        failed++;
    } else {
        memory_misuse();
    }
}

/* Starts the user-space shell (kernel/user/sh.c) as the first process and
 * runs processes until it and everything it left running have ended.
 * Returns its exit status, or -1 if it couldn't start. */
static int run_shell(void) {
    const struct user_program *sh = user_program_find(SHELL);
    if (sh == NULL) {
        kprintf("no program called %s\n", SHELL);
        return -1;
    }
    int status;
    if (!proc_run_init(sh, &status)) {
        return -1;
    }
    return status;
}

/* One line per disk blk_init() found, or says there is none. */
static void print_disks(void) {
    u32 n = blk_count();
    if (n == 0) {
        kprintf("disk: none\n");
        return;
    }
    for (u32 i = 0; i < n; i++) {
        u32 sectors = blk_capacity(i);
        kprintf("disk %u: %u sectors (%u KiB), irq %u%s\n", i, sectors, sectors / 2, blk_irq(i),
                blk_read_only(i) ? ", read-only" : "");
    }
}

int kmain(void) {
    uart_init();
    kprintf("Camelot v%s booting...\n", CAMELOT_VERSION);

    kprintf("kprintf: d=%d u=%u x=%x p=%p s=%s c=%c %%=%%\n", -42, 4294967295u, 0xcafef00du,
            (void *)0x80000000u, "excalibur", 'K');
    kprintf_selftest();
    kprintf("kprintf self-test: %d/%d passed\n", passed, passed + failed);

    /* The debug console works without any driver: show the failure count. */
    xcal_dbg((u32)failed);

    /* Memory: every free page goes on the free list, then paging goes on.
     * Everything from here runs with Sv32 translation. */
    kinit();
    kprintf("ram: %u MiB at %p\n", ram_size >> 20, (void *)(uintptr)ram_base);
    kvminit();
    kvminithart();
    kprintf("paging: on (Sv32)\n");
    failed += vm_selftest();

    trap_init();
    kprintf("timer: %u Hz tick, %u Hz timebase\n", TICK_HZ, timebase_hz());
    blk_init();
    binit();
    print_disks();
    wait_ticks(BOOT_TICKS);

    deliberate_fault();
    failed += blk_selftest();
    failed += fs_boot();
    int status = run_shell();
    /* Failed self-tests are what the exit code reports first. */
    return failed != 0 ? failed : status;
}
