/* Terminals' line discipline (spec section 10, "Terminals"; after Linux's
 * drivers/tty/n_tty.c): what the console (console.c) and the pseudo-
 * terminals' slaves (pty.c) share, so that both edit lines, echo and take
 * termios the same way.
 *
 * A terminal is a struct tty and its driver's two functions: `echo`, where
 * what the discipline prints goes (the echo, and ^C), and `pull`, which
 * brings in input waiting elsewhere (the console's UART ring; NULL for a
 * pty, whose master pushes its bytes in). Input arrives through
 * tty_receive(), which maps it (ICRNL, INLCR, IGNCR) into a 256-byte ring
 * and acts on the interrupt characters (ISIG) at once; the rest of the
 * work, the line's editing and the echo, happens when a program reads or
 * polls, as the console always did: input typed ahead is echoed after the
 * prompt that asks for it, not in the middle of other output.
 *
 * Canonical mode (ICANON) edits a line of up to TTY_LINE_MAX - 1
 * characters and its end: printable characters (0x20-0x7E) are kept,
 * VERASE and BS erase one, VWERASE a word, VKILL the line, NL (and VEOL,
 * if set) ends it, keeping it, VEOF ends it without (so on an empty line a
 * read returns 0); a NL right after a CR that ICRNL made a NL is skipped
 * (CR LF is one line end); other characters are dropped. Non-canonical
 * mode passes every byte, with Linux's VMIN and VTIME (in tenths of a
 * second, a tick each). With ISIG, VINTR and VQUIT kill the terminal's
 * foreground process (TIOCSPGRP), discarding the input unless NOFLSH and
 * echoing "^C" and a newline; with no foreground, or one that has ended,
 * they are dropped, and so is VSUSP (Camelot has no stopped processes). OPOST with ONLCR turns
 * NL into CR NL on output.
 *
 * Locking: the functions are called holding the terminal's lock, `lock`,
 * but tty_init() (before anything else can reach the terminal) and
 * tty_read(), which takes it and lets go of it while it sleeps. Readers
 * sleep on the tty, writers waiting for room in its input on `in`. */
#ifndef CAMELOT_TTY_H
#define CAMELOT_TTY_H

#include "types.h"
#include "vm.h"

struct poll_table;
struct spinlock;

/* ioctl requests (Linux's, asm-generic/ioctls.h). */
#define TCGETS 0x5401
#define TCSETS 0x5402
/* TCSETS once the output has gone: a pty's slave waits for its master to
 * read what it wrote. */
#define TCSETSW 0x5403
#define TCSETSF 0x5404 /* TCSETS, the input discarded first */
#define TCFLSH 0x540B
#define TIOCGPGRP 0x540F
#define TIOCSPGRP 0x5410
#define TIOCGWINSZ 0x5413
#define TIOCSWINSZ 0x5414
#define FIONREAD 0x541B
#define TIOCGPTN 0x80045430  /* a pty master: its number, the N of /dev/pts/N */
#define TIOCSPTLCK 0x40045431 /* a pty master: lock (nonzero) or unlock its slave */

/* TCFLSH's argument. */
#define TCIFLUSH 0
#define TCOFLUSH 1
#define TCIOFLUSH 2

/* struct termios, Linux's for TCGETS (asm-generic/termbits.h): 36 bytes. */
#define NCCS 19
struct termios {
    u32 c_iflag;
    u32 c_oflag;
    u32 c_cflag;
    u32 c_lflag;
    u8 c_line;
    u8 c_cc[NCCS];
};

/* c_cc's characters; 0 is none. */
#define VINTR 0
#define VQUIT 1
#define VERASE 2
#define VKILL 3
#define VEOF 4
#define VTIME 5
#define VMIN 6
#define VSWTC 7
#define VSTART 8
#define VSTOP 9
#define VSUSP 10
#define VEOL 11
#define VREPRINT 12
#define VDISCARD 13
#define VWERASE 14
#define VLNEXT 15
#define VEOL2 16

/* The flags, Linux's values (octal in its headers). */

/* c_iflag: the input's mapping. Linux's other bits are kept, not acted on. */
#define INLCR 0x40
#define IGNCR 0x80
#define ICRNL 0x100
#define IXON 0x400

/* c_oflag. */
#define OPOST 0x1
#define ONLCR 0x4

/* c_cflag: kept, as a serial line's, and not acted on. */
#define B38400 0xf
#define CS8 0x30
#define CREAD 0x80
#define HUPCL 0x400

/* c_lflag. */
#define ISIG 0x1
#define ICANON 0x2
#define ECHO 0x8
#define ECHOE 0x10
#define ECHOK 0x20
#define ECHONL 0x40
#define NOFLSH 0x80
#define ECHOCTL 0x200
#define ECHOKE 0x800
#define IEXTEN 0x8000

/* struct winsize, Linux's: 8 bytes. */
struct winsize {
    u16 ws_row;
    u16 ws_col;
    u16 ws_xpixel;
    u16 ws_ypixel;
};

/* The raw input waiting, and the longest line, its newline included. */
#define TTY_IN 256
#define TTY_LINE_MAX 128

struct tty;

struct tty_ops {
    /* Prints the `n` bytes at `s`, output processing done: the echo. */
    void (*echo)(struct tty *t, const char *s, u32 n);
    /* Brings in input waiting elsewhere, with tty_receive(), while
     * tty_room() says there is room. May be NULL. */
    void (*pull)(struct tty *t);
};

struct tty {
    struct spinlock *lock;
    const struct tty_ops *ops;
    struct termios termios;
    struct winsize winsize;
    /* The foreground process, which VINTR and VQUIT kill: TIOCSPGRP's pid
     * (a process group of one until Camelot has groups, #116); 0: none. */
    int fg;
    /* The other side is gone (a pty's master): reads end, writes fail. */
    bool hung;
    /* The last byte's arrival, in ticks: VTIME's timer. */
    u32 last_rx;
    /* The input received, mapped, not yet edited or read: [in_tail,
     * in_head), the counters running on. */
    u32 in_head;
    u32 in_tail;
    char in[TTY_IN];
    /* Canonical mode's line: `line_len` bytes, `line_taken` of them read
     * (line_taken <= line_len <= TTY_LINE_MAX); once `line_done` nothing
     * is added until it has all been read, whatever the mode. In
     * non-canonical mode what a line held when the mode changed is read
     * first. */
    char line[TTY_LINE_MAX];
    u32 line_len;
    u32 line_taken;
    bool line_done;
    /* The last byte received was a CR that ICRNL made a NL. */
    bool after_cr;
};

/* Sets up `t`, guarded by `lock`, with the console's defaults (`console`:
 * as the console has always behaved, no output processing, no VKILL or
 * VWERASE) or Linux's for a new terminal (tty_std_termios). */
void tty_init(struct tty *t, struct spinlock *lock, const struct tty_ops *ops, bool console);

/* Whether the input has room for one byte more. */
bool tty_room(const struct tty *t);

/* Whether `c` is one tty_receive() acts on at once rather than keeping
 * (ISIG's VINTR, VQUIT, VSUSP): it needs no room, so a driver whose input
 * is full passes it on anyway. */
bool tty_signal_char(const struct tty *t, char c);

/* A byte typed: mapped and added to the input, or acted on (ISIG); wakes
 * the readers. False if there was no room: the caller keeps it, or drops
 * it. */
bool tty_receive(struct tty *t, char c);

/* `c` as the output gets it under `oflag` (a terminal's c_oflag: OPOST,
 * ONLCR) into out[], 1 or 2 bytes: how many. */
u32 tty_opost(u32 oflag, char c, char out[2]);

/* read() on the terminal: waits as the mode says, then copies to user
 * address `dst` up to `n` bytes: their count (a hung-up terminal's line
 * typed part way too), 0 at the end (VEOF on an empty line, or a hung-up
 * terminal with nothing left), -EFAULT, -EINTR if the process was
 * killed. */
isize tty_read(struct tty *t, pagetable_t pt, uintptr dst, usize n);

/* Whether a read would not wait, having named `t`'s readers' channel in
 * `pt`: edits and echoes as a read would. */
bool tty_poll_in(struct tty *t, struct poll_table *pt);

/* The termios, window size, foreground and input requests (TCGETS,
 * TCSETS, TCSETSW, TCSETSF, TIOCGWINSZ, TIOCSWINSZ, TIOCGPGRP, TIOCSPGRP,
 * FIONREAD, TCFLSH's input part): the result, or -ENOTTY for another. */
long tty_ioctl(struct tty *t, u32 request, uintptr arg);

/* The other side is gone: wakes every reader and writer. */
void tty_hangup(struct tty *t);

#endif
