/* A VT100/ANSI escape sequence parser, shared by the kernel's framebuffer
 * console (fbcon.c) and the desktop's terminal emulator (kernel/user/term):
 * bytes in, one action out per byte, after Paul Williams' DEC-compatible
 * parser (vt100.net/emu/dec_ansi_parser), the state machine xterm's,
 * VTE's and Linux's consoles follow, cut to what Camelot's two terminals
 * act on: text, C0 controls, ESC sequences, CSI sequences with up to
 * VT_MAX_PARAMS numeric parameters and a private marker, and OSC strings,
 * which are skipped. Self-contained (no headers), so the kernel and the
 * user programs can both compile it. */
#ifndef CAMELOT_VT_H
#define CAMELOT_VT_H

#define VT_MAX_PARAMS 16
/* A parameter's largest value: bigger ones are cut to it. */
#define VT_PARAM_MAX 9999

/* What a byte did. */
#define VT_NONE 0    /* part of a sequence, or nothing */
#define VT_PRINT 1   /* a character to show: `ch` */
#define VT_EXECUTE 2 /* a C0 control (BS, HT, LF, CR...): `ch` */
#define VT_ESC 3     /* ESC and a final byte, `ch`; `intermediate` the byte between, if any */
#define VT_CSI 4     /* a control sequence: its final byte `ch`, its parameters and marker */

/* The state between sequences, `state` 0: there, a byte that is not ESC,
 * CAN, SUB or DEL and is below 0x80 is text (0x20 and up) or a C0 control
 * to execute, whatever vt_parse would make of it. A terminal that draws
 * every byte the kernel prints (fbcon.c) acts on those itself, sparing the
 * call. */
#define VT_GROUND 0

struct vt_parser {
    int state;
    unsigned int params[VT_MAX_PARAMS];
    int nparams;
    /* '?', '>', '=' or '<' after CSI (DEC's private modes), or 0. */
    unsigned char marker;
    /* The intermediate byte (0x20 to 0x2f) of an ESC or CSI sequence, or 0. */
    unsigned char intermediate;
    /* The byte of the action vt_parse returned. */
    unsigned char ch;
};

void vt_init(struct vt_parser *p);
/* Feeds a byte: the action it completes, VT_NONE if none. */
int vt_parse(struct vt_parser *p, unsigned char ch);
/* Parameter `i` of the last CSI, or `dflt` when it is missing or 0. */
unsigned int vt_param(const struct vt_parser *p, int i, unsigned int dflt);

#endif
