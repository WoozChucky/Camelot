/* Camelot kernel: C entry point. entry.S sets up the stack and clears .bss,
 * start() drops to S-mode, then supervisor_entry calls kmain() on hart 0
 * (on RV64 with paging already on, at the kernel's link addresses, through
 * the early page table); its return value becomes the exit code. After
 * booting, kmain starts the other harts, if the machine has any (smp.c),
 * runs the user-space shell as the first process, and returns when it
 * exits: on RV64 a 64-bit program in the lower half of the address space
 * (vm.h), with the LP64 system call ABI (syscall.h). */
#include "bio.h"
#include "blk.h"
#include "blktest.h"
#include "console.h"
#include "file.h"
#include "fstest.h"
#include "kalloc.h"
#include "kprintf.h"
#include "memlayout.h"
#include "net.h"
#include "mmio.h"
#include "proc.h"
#include "riscv.h"
#include "sbi.h"
#include "smp.h"
#include "spinlock.h"
#include "string.h"
#include "sysinfo.h"
#include "timer.h"
#include "trap.h"
#include "types.h"
#include "uart.h"
#include "vm.h"
#include "vmtest.h"
#include "xcal.h"

#define CAMELOT_VERSION "0.1.0"

/* The banner printed first at boot, before the version line. Plain ASCII, so it
 * shows the same on a serial console, in Herald and in Grail's window. */
static const char *const banner[] = {
    "\n",
    "    ____      _      __  __   _____   _        ___    _____\n",
    "   / ___|    / \\    |  \\/  | | ____| | |      / _ \\  |_   _|\n",
    "  | |       / _ \\   | |\\/| | |  _|   | |     | | | |   | |\n",
    "  | |___   / ___ \\  | |  | | | |___  | |___  | |_| |   | |\n",
    "   \\____| /_/   \\_\\ |_|  |_| |_____| |_____|  \\___/    |_|\n",
    "\n",
};

/* Timer ticks to wait for before starting the shell. */
#define BOOT_TICKS 10

/* The first process: the user-space shell, its path and its name. */
#define SHELL_PATH "/bin/sh"
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
/* Misuses of a spinlock or of interrupt nesting, each caught by acquire(),
 * release(), sched() or pop_off(). */
#define BOOT_FAULT_DOUBLE_ACQUIRE 12 /* acquire of a lock this hart holds */
#define BOOT_FAULT_RELEASE_UNHELD 13 /* release of a lock nobody holds */
#define BOOT_FAULT_YIELD_HOLDING 14  /* a kernel process yields holding a lock */
#define BOOT_FAULT_POP_UNMATCHED 15  /* pop_off with no push_off */
#define BOOT_FAULT_POP_INTR_ON 16    /* pop_off with interrupts back on */
/* RV64: a kernel process recurses until its stack runs into the guard page
 * below it, which kernel_vec reports as a kernel stack overflow. */
#define BOOT_FAULT_KSTACK_OVERFLOW 17
/* RV64: pa() of an ioremap() address, which has no physical address to
 * compute (kernel_pa() panics). */
#define BOOT_FAULT_PA_VMALLOC 18
/* RV64: kmain recurses on hart 0's boot stack until it runs into the guard
 * page below it, reported like any kernel stack's overflow. */
#define BOOT_FAULT_BOOT_STACK_OVERFLOW 19

/* Nothing is mapped below the boot ROM (spec section 7), so the kernel page
 * table doesn't map it either (on RV64 it maps nothing in the lower half). */
#define UNMAPPED_ADDR 0x00000100

/* A region with nothing in it, for BOOT_FAULT_MEGAPAGE: on RV32 a 4 MiB hole
 * of the memory map (spec section 7), on RV64 what one root entry maps just
 * past the direct map's room (Linux's KASAN shadow, unused here). */
#if __riscv_xlen == 64
#define EMPTY_REGION (PAGE_OFFSET + direct_map_size)
#define EMPTY_REGION_PA 0
#else
#define EMPTY_REGION 0x50000000
#define EMPTY_REGION_PA EMPTY_REGION
#endif

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
    expect(ksnprintf(buf, sizeof buf, "%lu", 7ul), "7");
#if __riscv_xlen == 64
    /* The 64-bit conversions (the RV32 kernel's count of checks stays what
     * its tests expect). */
    expect(ksnprintf(buf, sizeof buf, "%ld", -7l), "-7");
    expect(ksnprintf(buf, sizeof buf, "%lx", 0xfffful), "ffff");
    expect(ksnprintf(buf, sizeof buf, "%llx", 0x123456789abcdefull), "123456789abcdef");
    expect(ksnprintf(buf, sizeof buf, "%llu", 18446744073709551615ull), "18446744073709551615");
    expect(ksnprintf(buf, sizeof buf, "%lld", -9223372036854775807ll - 1), "-9223372036854775808");
    expect(ksnprintf(buf, sizeof buf, "%016llx", 0x2aull), "000000000000002a");
    expect(ksnprintf(buf, sizeof buf, "%lx", 0xffffffff80000000ul), "ffffffff80000000");
    expect(ksnprintf(buf, sizeof buf, "%p", (void *)0xffffffff80000000ul), "0xffffffff80000000");
    expect(ksnprintf(buf, sizeof buf, "%p", NULL), "0x0000000000000000");
#else
    expect(ksnprintf(buf, sizeof buf, "%p", (void *)0x80000000u), "0x80000000");
    expect(ksnprintf(buf, sizeof buf, "%p", NULL), "0x00000000");
#endif
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
    uintptr text = (uintptr)__text_start;
    if (boot_fault == BOOT_FAULT_REMAP) {
        map_pages(kernel_pagetable, text, pa(text), PGSIZE, PTE_R);
    } else if (boot_fault == BOOT_FAULT_UNMAP) {
        /* Page 0 shares its 4 MiB region with the test device, so this finds
         * an invalid PTE in an existing second-level table. */
        unmap_pages(kernel_pagetable, 0, PGSIZE, false);
    } else if (boot_fault == BOOT_FAULT_W_ONLY) {
        map_pages(kernel_pagetable, 0, 0, PGSIZE, PTE_W);
    } else if (boot_fault == BOOT_FAULT_MEGAPAGE) {
        /* A leaf in the root, mapping the region onto itself on RV32 and onto
         * physical address 0 on RV64: walk() must not go through it. */
        kernel_pagetable[VPN(EMPTY_REGION, PT_LEVELS - 1)] = PA2PTE(EMPTY_REGION_PA) | PTE_R | PTE_V;
        walk(kernel_pagetable, EMPTY_REGION, false);
    } else if (boot_fault == BOOT_FAULT_KFREE_UNALIGNED) {
        kfree((char *)kalloc() + 4);
    } else if (boot_fault == BOOT_FAULT_KFREE_KERNEL) {
        kfree(__text_start);
    } else if (boot_fault == BOOT_FAULT_KFREE_PAST_RAM) {
        kfree(va(ram_base + ram_size));
    } else {
        return;
    }
    kprintf("memory misuse %u didn't panic\n", boot_fault);
    failed++;
}

#if __riscv_xlen == 64
/* BOOT_FAULT_KSTACK_OVERFLOW's kernel process: each call takes a few hundred
 * bytes more of its stack, until the stack runs into its guard page. The
 * volatile accesses keep the compiler from folding the recursion away, and
 * the volatile `deeper` from seeing that it never ends. */
static volatile bool deeper = true;

static u32 recurse(u32 depth) {
    volatile u8 frame[256];
    frame[0] = (u8)depth;
    return (deeper ? recurse(depth + 1) : 0) + frame[0];
}

static void overflow_stack(void) {
    recurse(0);
}
#endif

/* The lock the BOOT_FAULT_ values from 12 up misuse. */
static struct spinlock fault_lock;

/* BOOT_FAULT_YIELD_HOLDING's kernel process: gives up the CPU while it
 * holds a spinlock, which sched() refuses (with interrupts off, a thread
 * that then wanted the lock would spin on it forever). */
static void yield_holding(void) {
    acquire(&fault_lock);
    yield();
}

/* The BOOT_FAULT_ values from 12 up: each should panic. */
static void lock_misuse(void) {
    initlock(&fault_lock, "boot_fault");
    if (boot_fault == BOOT_FAULT_DOUBLE_ACQUIRE) {
        acquire(&fault_lock);
        acquire(&fault_lock);
    } else if (boot_fault == BOOT_FAULT_RELEASE_UNHELD) {
        release(&fault_lock);
    } else if (boot_fault == BOOT_FAULT_YIELD_HOLDING) {
        const char *err;
        if (kproc_create("yield_holding", yield_holding, &err) == NULL) {
            kprintf("lock misuse %u: cannot start a process: %s\n", boot_fault, err);
            failed++;
            return;
        }
        scheduler();
    } else if (boot_fault == BOOT_FAULT_POP_UNMATCHED) {
        intr_off();
        pop_off();
    } else if (boot_fault == BOOT_FAULT_POP_INTR_ON) {
        push_off();
        intr_on();
        pop_off();
    } else {
        return;
    }
    kprintf("lock misuse %u didn't panic\n", boot_fault);
    failed++;
}

static void deliberate_fault(void) {
#if __riscv_xlen == 64
    if (boot_fault == BOOT_FAULT_KSTACK_OVERFLOW) {
        const char *err;
        if (kproc_create("overflow", overflow_stack, &err) == NULL) {
            kprintf("stack overflow: cannot start a process: %s\n", err);
            failed++;
            return;
        }
        scheduler();
    }
    if (boot_fault == BOOT_FAULT_BOOT_STACK_OVERFLOW) {
        recurse(0);
    }
    if (boot_fault == BOOT_FAULT_PA_VMALLOC) {
        phys_addr_t p = pa(uart_regs);
        kprintf("pa(%p) gave %lx instead of panicking\n", (void *)uart_regs, p);
        failed++;
    }
#endif
    if (boot_fault >= BOOT_FAULT_DOUBLE_ACQUIRE && boot_fault <= BOOT_FAULT_POP_INTR_ON) {
        lock_misuse();
    } else if (boot_fault == BOOT_FAULT_ILLEGAL) {
        illegal_instruction();
    } else if (boot_fault == BOOT_FAULT_LOAD) {
        u32 v = mmio_read32(UNMAPPED_ADDR);
        kprintf("load from %p returned %x instead of faulting\n", (void *)UNMAPPED_ADDR, v);
        failed++;
    } else if (boot_fault == BOOT_FAULT_STORE_TEXT) {
        mmio_write32((uintptr)kmain, 0);
        kprintf("store to kernel text at %p didn't fault\n", (void *)kmain);
        failed++;
    } else {
        memory_misuse();
    }
}

/* Starts the user-space shell (kernel/user/sh.c) as the first process and
 * runs processes until it and everything it left running have ended: from
 * the root filesystem's /bin, or the kernel's own copy (exec.h). Returns its
 * exit status, or -1 if it couldn't start. */
static int run_shell(void) {
    int status;
    if (!proc_run_init(SHELL_PATH, SHELL, &status)) {
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
    /* This hart's struct cpu, which every lock uses: first of all. */
    cpu_init();
    /* The UART where the boot reaches it: on RV64 the fixmap (vm.h). */
    uart_init();
    for (u32 i = 0; i < sizeof banner / sizeof banner[0]; i++)
        kprintf("%s", banner[i]);
    kprintf("Camelot v%s booting...\n", CAMELOT_VERSION);
    /* What the SBI under the kernel has, before anything needs it: the
     * timer, IPIs, starting harts and stopping the machine. */
    sbi_init();

    kprintf("kprintf: d=%d u=%u x=%x p=%p s=%s c=%c %%=%%\n", -42, 4294967295u, 0xcafef00du,
            (void *)0x80000000u, "excalibur", 'K');
    kprintf_selftest();
    kprintf("kprintf self-test: %d/%d passed\n", passed, passed + failed);

    /* The debug console works without any driver: show the failure count. */
    xcal_dbg((u32)failed);

    /* Memory: every free page goes on the free list, then paging goes on
     * with the kernel page table. Everything from here runs with Sv32
     * translation (on RV64 Sv48 or Sv39, which was on already, with the
     * early page table: kvminithart() drops it). */
    kinit();
    kprintf("ram: %lu MiB at 0x%lx\n", ram_size >> 20, ram_base);
    kvminit();
    kvminithart();
    /* From here on the devices through ioremap() (vm.h). */
    sysinfo_map();
    uart_map();
#if __riscv_xlen == 64
    kprintf("paging: on (%s)\n", pt_levels == 4 ? "Sv48" : "Sv39");
#else
    kprintf("paging: on (Sv32)\n");
#endif
    failed += vm_selftest();

    /* The locks of what interrupt handlers and processes share, before
     * either can run. */
    proc_init();
    console_init();
    file_init();
    trap_init();
    kprintf("timer: %u Hz tick, %u Hz timebase\n", TICK_HZ, timebase_hz());
    blk_init();
    binit();
    fs_init();
    print_disks();
    net_init();
    wait_ticks(BOOT_TICKS);

    deliberate_fault();
    failed += blk_selftest();
    failed += fs_boot();
    failed += net_boot();
#if __riscv_xlen == 64
    failed += kstack_selftest();
#endif
    /* The other harts, last: the self-tests above run their kernel
     * processes on hart 0 alone. */
    failed += smp_boot();
#if __riscv_xlen == 64
    /* How deep the kernel stacks have gone, against their KSTACK_SIZE: a
     * quarter of it must be left. */
    usize used = kstack_max_used();
    kprintf("kstack: %lu of %lu bytes used at most\n", used, KSTACK_SIZE);
    if (used > KSTACK_SIZE / 4 * 3) {
        kprintf("kstack: less than a quarter of the stack left\n");
        failed++;
    }
#endif
    int status = run_shell();
    /* Failed self-tests are what the exit code reports first. */
    return failed != 0 ? failed : status;
}
