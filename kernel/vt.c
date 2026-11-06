/* The VT100/ANSI parser (include/vt.h): Paul Williams' state machine,
 * with the states Camelot's terminals need. In every state a C0 control is
 * executed where it comes (a sequence goes on after it), CAN and SUB cancel
 * a sequence, and ESC starts a new one; DEL is ignored. */
#include "include/vt.h"

#define ST_GROUND VT_GROUND
#define ST_ESCAPE 1
#define ST_ESCAPE_INTERMEDIATE 2
#define ST_CSI_ENTRY 3
#define ST_CSI_PARAM 4
#define ST_CSI_INTERMEDIATE 5
#define ST_CSI_IGNORE 6
#define ST_OSC 7
#define ST_OSC_ESC 8 /* ESC in an OSC string: `\` ends it (ST) */

void vt_init(struct vt_parser *p) {
    p->state = ST_GROUND;
    p->nparams = 0;
    p->marker = 0;
    p->intermediate = 0;
    p->ch = 0;
}

static void clear(struct vt_parser *p) {
    p->nparams = 0;
    p->marker = 0;
    p->intermediate = 0;
    for (int i = 0; i < VT_MAX_PARAMS; i++) {
        p->params[i] = 0;
    }
}

unsigned int vt_param(const struct vt_parser *p, int i, unsigned int dflt) {
    return i < p->nparams && p->params[i] != 0 ? p->params[i] : dflt;
}

/* A digit or a ';' of a CSI's parameters. */
static void param_byte(struct vt_parser *p, unsigned char ch) {
    if (p->nparams == 0) {
        p->nparams = 1;
    }
    if (ch == ';') {
        if (p->nparams < VT_MAX_PARAMS) {
            p->nparams++;
        }
        return;
    }
    unsigned int *v = &p->params[p->nparams - 1];
    *v = *v * 10 + (unsigned int)(ch - '0');
    if (*v > VT_PARAM_MAX) {
        *v = VT_PARAM_MAX;
    }
}

int vt_parse(struct vt_parser *p, unsigned char ch) {
    p->ch = ch;
    /* Anywhere: CAN and SUB end a sequence, ESC starts one. In an OSC
     * string an ESC may be the start of its ST. */
    if (ch == 0x18 || ch == 0x1a) {
        p->state = ST_GROUND;
        return VT_NONE;
    }
    if (ch == 0x1b) {
        p->state = p->state == ST_OSC ? ST_OSC_ESC : ST_ESCAPE;
        clear(p);
        return VT_NONE;
    }
    if (p->state == ST_OSC || p->state == ST_OSC_ESC) {
        /* BEL or ESC \ ends it; ESC and anything else starts a sequence. */
        if (ch == 0x07 || (p->state == ST_OSC_ESC && ch == '\\')) {
            p->state = ST_GROUND;
            return VT_NONE;
        }
        if (p->state == ST_OSC_ESC) {
            p->state = ST_ESCAPE;
            return vt_parse(p, ch);
        }
        return VT_NONE;
    }
    if (ch < 0x20) {
        return VT_EXECUTE; /* a C0 control, in any state but a string */
    }
    if (ch == 0x7f) {
        return VT_NONE;
    }
    if (ch >= 0x80) {
        /* Past ASCII (UTF-8's bytes) is text: a sequence it comes in is
         * not one after all. */
        p->state = ST_GROUND;
        return VT_PRINT;
    }
    switch (p->state) {
    case ST_GROUND:
        return VT_PRINT;
    case ST_ESCAPE:
        if (ch == '[') {
            p->state = ST_CSI_ENTRY;
        } else if (ch == ']') {
            p->state = ST_OSC;
        } else if (ch < 0x30) {
            p->intermediate = ch;
            p->state = ST_ESCAPE_INTERMEDIATE;
        } else {
            p->state = ST_GROUND;
            return VT_ESC;
        }
        return VT_NONE;
    case ST_ESCAPE_INTERMEDIATE:
        if (ch < 0x30) {
            return VT_NONE; /* more than one: the first is kept */
        }
        p->state = ST_GROUND;
        return VT_ESC;
    case ST_CSI_ENTRY:
    case ST_CSI_PARAM:
        if ((ch >= '0' && ch <= '9') || ch == ';') {
            p->state = ST_CSI_PARAM;
            param_byte(p, ch);
            return VT_NONE;
        }
        if (ch >= 0x3c && ch <= 0x3f) {
            /* A marker belongs first: anywhere else it spoils the sequence. */
            if (p->state == ST_CSI_ENTRY) {
                p->marker = ch;
                p->state = ST_CSI_PARAM;
            } else {
                p->state = ST_CSI_IGNORE;
            }
            return VT_NONE;
        }
        if (ch == ':') {
            p->state = ST_CSI_IGNORE; /* sub-parameters: none taken */
            return VT_NONE;
        }
        if (ch < 0x30) {
            p->intermediate = ch;
            p->state = ST_CSI_INTERMEDIATE;
            return VT_NONE;
        }
        p->state = ST_GROUND;
        return ch <= 0x7e ? VT_CSI : VT_NONE;
    case ST_CSI_INTERMEDIATE:
        if (ch < 0x30) {
            return VT_NONE;
        }
        if (ch < 0x40) {
            p->state = ST_CSI_IGNORE; /* a parameter after an intermediate */
            return VT_NONE;
        }
        p->state = ST_GROUND;
        return ch <= 0x7e ? VT_CSI : VT_NONE;
    case ST_CSI_IGNORE:
        if (ch >= 0x40 && ch <= 0x7e) {
            p->state = ST_GROUND;
        }
        return VT_NONE;
    }
    p->state = ST_GROUND;
    return VT_NONE;
}
