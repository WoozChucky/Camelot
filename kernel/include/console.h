/* The kernel console: line input on top of the interrupt-driven UART. */
#ifndef CAMELOT_CONSOLE_H
#define CAMELOT_CONSOLE_H

#include "types.h"

/* Returns the next byte typed on the console, sleeping in wfi until there is
 * one. Needs interrupts on (trap_init). */
char console_getc(void);

/* Reads one line into `buf` (`size` bytes, at least 1) and returns its length.
 * The line is NUL-terminated, without the newline. What is typed is echoed.
 * Backspace (BS or DEL) erases the last character. The line ends at CR, LF or
 * CR LF: a LF right after a CR is skipped, so terminals that send either work.
 * Other control characters are ignored, and characters beyond `size` - 1 are
 * dropped. */
int console_readline(char *buf, int size);

#endif
