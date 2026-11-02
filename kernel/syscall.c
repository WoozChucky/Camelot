/* Calls from U-mode (syscall.h). For now only the provisional ecalls that
 * let the first user programs print and exit. */
#include "syscall.h"
#include "proc.h"
#include "uart.h"
#include "vm.h"

/* Bytes copied out of user memory at a time. */
#define WRITE_CHUNK 64

/* ECALL_DEBUG_WRITE: prints the `len` bytes at user address `buf` on the
 * console. All of them must be readable user memory, or nothing is printed
 * and the result is -1. */
static int debug_write(u32 buf, u32 len) {
    pagetable_t pt = myproc()->pagetable;
    if (!uvm_range_ok(pt, buf, len, PTE_R)) {
        return -1;
    }
    char chunk[WRITE_CHUNK];
    for (u32 done = 0; done < len;) {
        u32 n = len - done < WRITE_CHUNK ? len - done : WRITE_CHUNK;
        copyin(pt, chunk, buf + done, n);
        for (u32 i = 0; i < n; i++) {
            uart_putc(chunk[i]);
        }
        done += n;
    }
    return (int)len;
}

void syscall(struct trap_frame *f) {
    u32 num = f->regs[REG_A7];
    u32 a0 = f->regs[REG_A0];
    u32 a1 = f->regs[REG_A1];
    int ret;
    if (num == ECALL_EXIT) {
        proc_exit((int)a0);
        ret = -1; /* not reached */
    } else if (num == ECALL_DEBUG_WRITE) {
        ret = debug_write(a0, a1);
    } else {
        ret = -1;
    }
    f->regs[REG_A0] = (u32)ret;
}
