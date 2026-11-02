/* Camelot kernel: C entry point. entry.s sets up the stack and clears .bss,
 * start() drops to S-mode, then supervisor_entry calls kmain(); its return
 * value becomes the exit code. */
#include "kprintf.h"
#include "mmio.h"
#include "riscv.h"
#include "string.h"
#include "timer.h"
#include "trap.h"
#include "types.h"
#include "uart.h"
#include "xcal.h"

#define CAMELOT_VERSION "0.1.0"

/* Timer ticks to wait for before halting. */
#define BOOT_TICKS 10

/* Deliberate faults, to exercise the panic path. The image ships with
 * BOOT_FAULT_NONE; tests patch `boot_fault` in it to pick another one. The
 * value is non-zero so the variable lives in .data, which is part of the
 * image, rather than in .bss, which entry.s clears. */
#define BOOT_FAULT_NONE 1
#define BOOT_FAULT_ILLEGAL 2 /* illegal_instruction(): unimp, registers marked */
#define BOOT_FAULT_LOAD 3    /* a load from UNMAPPED_ADDR */

/* Nothing is mapped below the boot ROM (spec section 7). */
#define UNMAPPED_ADDR 0x00000100

volatile u32 boot_fault = BOOT_FAULT_NONE;

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

static void deliberate_fault(void) {
    if (boot_fault == BOOT_FAULT_ILLEGAL) {
        illegal_instruction();
    } else if (boot_fault == BOOT_FAULT_LOAD) {
        u32 v = mmio_read32(UNMAPPED_ADDR);
        kprintf("load from %p returned %x instead of faulting\n", (void *)UNMAPPED_ADDR, v);
        failed++;
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

    trap_init();
    kprintf("timer: %u Hz tick, %u Hz timebase\n", TICK_HZ, timebase_hz());
    wait_ticks(BOOT_TICKS);

    deliberate_fault();
    return failed;
}
