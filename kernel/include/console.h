/* The kernel console (spec section 10, "Kernel console"): the read and write
 * system calls on file descriptors 0-2, on top of the interrupt-driven UART.
 * Input is line-buffered and edited here, like a terminal in canonical mode:
 * a read returns once a whole line has been typed. */
#ifndef CAMELOT_CONSOLE_H
#define CAMELOT_CONSOLE_H

#include "types.h"
#include "vm.h"

struct file;

/* Longest input line in bytes, its newline included. */
#define CONSOLE_LINE_MAX 128

/* Sets up the console's lock. kmain calls it before interrupts are on. */
void console_init(void);

/* The UART's interrupt: moves received bytes into its ring buffer
 * (uart_intr) and wakes the processes waiting in console_read(). */
void console_intr(void);

/* read(0, dst, n) for the current process, whose page table is `pt`: waits,
 * sleeping, until a line is complete, then copies up to `n` bytes of it to
 * user address `dst` and returns how many. What a read leaves of a line goes
 * to the next one, so a line longer than `n` takes several reads.
 *
 * Lines are edited as they are typed, with the bytes echoed: printable
 * characters (0x20-0x7E) are kept, up to CONSOLE_LINE_MAX - 1 of them; BS
 * (0x08) and DEL (0x7F) erase one, echoing BS, space, BS; CR, LF or CR LF
 * ends the line, which then ends with a LF (a LF right after a CR is
 * skipped, so raw consoles and piped text both work); Ctrl-D (0x04) ends it
 * without a newline, so on an empty line the read returns 0, the end of the
 * input. Other control characters are ignored.
 *
 * Returns -EFAULT, having taken nothing, if [dst, dst + n) is not writable
 * user memory. */
/* A character the keyboard typed (keyboard.c, while no process reads the
 * keyboard): it waits, as the UART's bytes do, for console_read(). Any
 * context; takes the console's lock. */
void console_input(char c);

isize console_read(struct file *f, pagetable_t pt, uintptr dst, usize n);

/* write(1 or 2, src, n): prints the `n` bytes at user address `src` and
 * returns n, or returns -EFAULT, printing nothing, if any of them is not
 * readable user memory. */
isize console_write(struct file *f, pagetable_t pt, uintptr src, usize n);

#endif
