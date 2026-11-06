/* The kernel console (spec section 10, "Kernel console"): the read and write
 * system calls on file descriptors 0-2, on top of the interrupt-driven UART,
 * a terminal through the line discipline the pseudo-terminals share
 * (tty.h): in canonical mode, a read returns once a whole line has been
 * typed. */
#ifndef CAMELOT_CONSOLE_H
#define CAMELOT_CONSOLE_H

#include "types.h"
#include "vm.h"

struct file;

/* Sets up the console's lock and its terminal. kmain calls it before
 * interrupts are on. */
void console_init(void);

/* The UART's interrupt: moves received bytes into its ring buffer
 * (uart_intr) and from there into the terminal's input as it has room,
 * which wakes the processes waiting in console_read(), and acts on Ctrl-C
 * (tty.h's ISIG) at once. */
void console_intr(void);

/* read(0, dst, n) for the current process, whose page table is `pt`:
 * tty_read() on the console's terminal. With the console's defaults, a
 * read waits, sleeping, until a line is complete, then copies up to `n`
 * bytes of it to user address `dst` and returns how many. What a read
 * leaves of a line goes to the next one, so a line longer than `n` takes
 * several reads.
 *
 * Lines are edited as they are read, with the bytes echoed: printable
 * characters (0x20-0x7E) are kept, up to TTY_LINE_MAX - 1 of them; BS
 * (0x08) and DEL (0x7F) erase one, echoing BS, space, BS; CR, LF or CR LF
 * ends the line, which then ends with a LF (a LF right after a CR is
 * skipped, so raw consoles and piped text both work); Ctrl-D (0x04) ends it
 * without a newline, so on an empty line the read returns 0, the end of the
 * input. Other control characters are ignored, and Ctrl-C too unless a
 * foreground process is set (TIOCSPGRP), which it kills.
 *
 * Returns -EFAULT, having taken nothing, if [dst, dst + n) is not writable
 * user memory. */
isize console_read(struct file *f, pagetable_t pt, uintptr dst, usize n);

/* A character the keyboard typed (keyboard.c, while no process reads the
 * keyboard): it goes into the terminal's input, as the UART's bytes do, or
 * is dropped if the input is full. Any context; takes the console's lock. */
void console_input(char c);

/* write(1 or 2, src, n): prints the `n` bytes at user address `src`
 * (through the output processing, none by default) and returns n, or
 * returns -EFAULT, printing nothing, if any of them is not readable user
 * memory. */
isize console_write(struct file *f, pagetable_t pt, uintptr src, usize n);

struct poll_table;

/* poll() on the console: always ready to write; ready to read once a read
 * would not wait (a line complete, in canonical mode, or Ctrl-D ending the
 * input), editing as a read would, echoing what was typed ahead, on a file
 * open for reading. */
u32 console_poll(struct file *f, struct poll_table *pt);

/* ioctl() on the console: tty_ioctl(), the termios, window size and
 * foreground requests. */
long console_ioctl(struct file *f, u32 request, uintptr arg);

#endif
