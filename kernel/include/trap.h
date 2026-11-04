/* S-mode trap handling (spec section 10, "Kernel traps"). */
#ifndef CAMELOT_TRAP_H
#define CAMELOT_TRAP_H

/* Assembly includes this file too, for the trap frame's layout (kernelvec.S,
 * uservec.S) and the exit codes (entry.S): the C is under
 * #ifndef __ASSEMBLER__ (const.h). */
#ifndef __ASSEMBLER__
#include "spinlock.h"
#include "types.h"

/* What kernel_vec (kernelvec.S) saves on the stack, and user_vec (uservec.S)
 * in a process's trap frame: every slot a register wide (XLEN bits). The
 * offsets are a contract with both, in slots of SZREG bytes (4 on RV32, 8 on
 * RV64): regs[i] at slot i, then sepc (32), scause (33), stval (34), sstatus
 * (35) and kernel_tp (36), the FRAME_ offsets below. Both write sepc and
 * sstatus back before sret, so a handler may change where execution
 * resumes. */
struct trap_frame {
    uintptr regs[32]; /* x0-x31 as they were at the trap; regs[0] is always 0 */
    uintptr sepc;
    uintptr scause;
    uintptr stval;
    uintptr sstatus;
    /* The hart's id, which user_vec puts in tp (the program's tp is in
     * regs[4]): user_trap_return() sets it before each return to U-mode, so
     * a process that moves to another hart takes that hart's id with it.
     * Not used in kernel_vec's frames, where tp is the kernel's already. */
    uintptr kernel_tp;
    /* To a multiple of 16 bytes, for the stack below: 37 slots make 148
     * bytes on RV32 and 296 on RV64. */
#if __riscv_xlen == 64
    uintptr pad[1];
#else
    uintptr pad[3];
#endif
};

#endif

/* The frame's layout, which kernelvec.S and uservec.S use: its size, a
 * multiple of 16, and the CSRs' offsets, after the 32 registers, in slots a
 * register wide (FRAME_SLOT). */
#if __riscv_xlen == 64
#define TRAP_FRAME_SIZE 304
#define FRAME_SLOT 8
#else
#define TRAP_FRAME_SIZE 160
#define FRAME_SLOT 4
#endif
#define FRAME_SEPC (32 * FRAME_SLOT)
#define FRAME_SCAUSE (33 * FRAME_SLOT)
#define FRAME_STVAL (34 * FRAME_SLOT)
#define FRAME_SSTATUS (35 * FRAME_SLOT)
#define FRAME_KERNEL_TP (36 * FRAME_SLOT)

#ifndef __ASSEMBLER__
_Static_assert(sizeof(struct trap_frame) == TRAP_FRAME_SIZE, "kernelvec.S and uservec.S use TRAP_FRAME_SIZE");
_Static_assert(sizeof(uintptr) == FRAME_SLOT, "a frame slot is a register wide");
_Static_assert(offsetof(struct trap_frame, sepc) == FRAME_SEPC, "kernelvec.S and uservec.S use FRAME_SEPC");
_Static_assert(offsetof(struct trap_frame, scause) == FRAME_SCAUSE, "kernelvec.S and uservec.S use FRAME_SCAUSE");
_Static_assert(offsetof(struct trap_frame, stval) == FRAME_STVAL, "kernelvec.S and uservec.S use FRAME_STVAL");
_Static_assert(offsetof(struct trap_frame, sstatus) == FRAME_SSTATUS, "kernelvec.S and uservec.S use FRAME_SSTATUS");
_Static_assert(offsetof(struct trap_frame, kernel_tp) == FRAME_KERNEL_TP, "uservec.S uses FRAME_KERNEL_TP");
#endif

/* Exit code after a kernel panic, and after an unexpected M-mode trap or
 * an S-mode one before kernel_vec (entry.S's early_trap, early_s_trap). */
#define PANIC_EXIT_CODE 0xfe
#define EXIT_UNEXPECTED_TRAP 0xff

#ifndef __ASSEMBLER__

/* Timer ticks since boot, counted by kernel_trap under `tickslock` (which
 * the sleep system call sleeps with). A single aligned word, so reading it
 * needs no lock. */
extern volatile u32 ticks;
extern struct spinlock tickslock;

/* Set once the kernel panics: kprintf() stops taking the UART's lock, which
 * the code that panicked may hold. panic_hart is the hart that panicked, or
 * PANIC_NO_HART, no hart's id, until it has stored its own (just after
 * setting `panicking`). */
extern volatile bool panicking;
extern volatile u32 panic_hart;
#define PANIC_NO_HART 0xffffffffu

/* Stops this hart for good, with interrupts off: what a hart does when
 * another one is panicking. */
void panic_freeze(void);

/* panic_freeze() if another hart is panicking, else nothing. Every byte
 * written to the UART goes through it (uart_putc), so once a hart starts a
 * panic, whatever the others print, kprintf(), a process's write or the
 * console's echo, stops them before it gets out, and the report comes out
 * whole. */
void panic_stop_others(void);

/* S-mode trap vector; entry.S points stvec at it once the kernel runs at
 * its link addresses. */
void kernel_vec(void);

/* Called by kernel_vec with the saved registers. */
void kernel_trap(struct trap_frame *f);

#if __riscv_xlen == 64
/* Called by kernel_vec, on this hart's overflow stack, when a trap's frame
 * would not fit on the kernel stack it came on (the stack ran into its guard
 * page, or is about to): reports a kernel stack overflow, with the thread's
 * name and the registers of the trap, and panics. Never returns. */
void kstack_overflow(struct trap_frame *f);
#endif

/* Register numbers in struct trap_frame's regs[]. */
#define REG_SP 2
#define REG_A0 10
#define REG_A1 11
#define REG_A7 17

/* The U-mode trap vector (uservec.S): stvec points at it while a process
 * runs in U-mode, and sscratch at the process's trap frame. Saves the user
 * registers there, switches to the process's kernel stack (just below the
 * frame) and calls user_trap(). */
void user_vec(void);

/* Called by user_vec, on the process's kernel stack: a timer tick yields the
 * CPU, an external interrupt is serviced, an ecall goes to syscall(), and any
 * other exception kills the process. Then returns to U-mode through
 * user_trap_return(). Never returns. */
void user_trap(struct trap_frame *f);

/* Back to U-mode with the registers in `f`, the current process's frame:
 * interrupts off, stvec = user_vec, sstatus.SPP = U and SPIE set, then
 * user_return(). Never returns. */
void user_trap_return(struct trap_frame *f);

/* uservec.S: sscratch = f, sepc and sstatus from f, every register from f,
 * sret. Never returns. */
void user_return(struct trap_frame *f);

/* Whether `scause` is a page fault (instruction, load or store). */
bool is_page_fault(uintptr scause);

/* Sets up the PLIC and enables the interrupts the kernel handles (the
 * forwarded timer tick and external interrupts), then turns interrupts on in
 * S-mode. */
void trap_init(void);

/* Name of a trap cause, e.g. "illegal instruction". `cause` is a raw scause
 * value, interrupt bit included. */
const char *trap_cause_name(uintptr cause);

/* Prints "panic: <msg>" and stops the machine with PANIC_EXIT_CODE. */
void panic(const char *msg);

/* Executes an illegal instruction (unimp) after setting every register but sp
 * to its own number in each byte (x5 = 0x05050505 on RV32,
 * 0x0505050505050505 on RV64), so a test can check the panic's register
 * dump. Never returns. */
void illegal_instruction(void);
#endif

#endif
