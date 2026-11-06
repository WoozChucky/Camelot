/* The Wayland wire format (gui.h, "The Wayland wire format"): one end of a
 * connection, messages built into `out` and sent with their descriptors in
 * one sendmsg, received into `in` with theirs, and read an argument at a
 * time. Nothing a peer sends is trusted: a size is checked against what
 * came, a string must end with its NUL, an fd must have come. */
#include "../gui.h"

/* A message's header: the object, then the size and the opcode. */
#define HEADER 8

static uint get32(const uchar *p) {
    return (uint)p[0] | ((uint)p[1] << 8) | ((uint)p[2] << 16) | ((uint)p[3] << 24);
}

static void put32(uchar *p, uint v) {
    p[0] = (uchar)v;
    p[1] = (uchar)(v >> 8);
    p[2] = (uchar)(v >> 16);
    p[3] = (uchar)(v >> 24);
}

void gui_wire_init(struct gui_wire *w, int fd) {
    memset(w, 0, sizeof(*w));
    w->fd = fd;
    w->start = -1;
}

void gui_wire_release(struct gui_wire *w) {
    for (int i = 0; i < w->nin_fds; i++) {
        close(w->in_fds[i]);
    }
    w->nin_fds = 0;
}

/* ---- Sending ---- */

void gui_wire_begin(struct gui_wire *w, uint object, uint opcode) {
    if (w->error < 0) {
        return;
    }
    /* Room for a message of the largest size, and its descriptors. */
    if (w->out_len > (int)sizeof(w->out) - GUI_WIRE_MAX || w->nout_fds > GUI_WIRE_FDS / 2) {
        gui_wire_flush(w);
        if (w->error < 0) {
            return;
        }
    }
    w->start = w->out_len;
    put32(w->out + w->out_len, object);
    put32(w->out + w->out_len + 4, opcode);
    w->out_len += HEADER;
}

/* Room for `n` more bytes in the message being built. */
static int room(struct gui_wire *w, uint n) {
    if (w->error < 0 || w->start < 0) {
        return 0;
    }
    if ((uint)(w->out_len - w->start) + n > GUI_WIRE_MAX) {
        w->error = -EMSGSIZE;
        return 0;
    }
    return 1;
}

void gui_wire_uint(struct gui_wire *w, uint v) {
    if (room(w, 4)) {
        put32(w->out + w->out_len, v);
        w->out_len += 4;
    }
}

void gui_wire_int(struct gui_wire *w, int v) {
    gui_wire_uint(w, (uint)v);
}

void gui_wire_fixed(struct gui_wire *w, int v) {
    gui_wire_uint(w, (uint)v);
}

/* `len` bytes and the zeros that make them a multiple of 4. */
static void put_bytes(struct gui_wire *w, const void *data, uint len) {
    uint padded = (len + 3) & ~3u;
    if (room(w, padded)) {
        memcpy(w->out + w->out_len, data, len);
        memset(w->out + w->out_len + len, 0, padded - len);
        w->out_len += (int)padded;
    }
}

void gui_wire_string(struct gui_wire *w, const char *s) {
    if (s == NULL) {
        gui_wire_uint(w, 0);
        return;
    }
    uint len = (uint)strlen(s) + 1;
    if (room(w, 4 + ((len + 3) & ~3u))) {
        gui_wire_uint(w, len);
        put_bytes(w, s, len);
    }
}

void gui_wire_array(struct gui_wire *w, const void *data, uint len) {
    if (room(w, 4 + ((len + 3) & ~3u))) {
        gui_wire_uint(w, len);
        put_bytes(w, data, len);
    }
}

void gui_wire_fd(struct gui_wire *w, int fd) {
    if (w->error < 0) {
        return;
    }
    if (w->nout_fds == GUI_WIRE_FDS) {
        w->error = -EMSGSIZE;
        return;
    }
    w->out_fds[w->nout_fds++] = fd;
}

int gui_wire_end(struct gui_wire *w) {
    if (w->error < 0 || w->start < 0) {
        w->start = -1;
        return w->error;
    }
    uint size = (uint)(w->out_len - w->start);
    uint opcode = get32(w->out + w->start + 4);
    put32(w->out + w->start + 4, (size << 16) | (opcode & 0xffff));
    w->start = -1;
    return 0;
}

int gui_wire_flush(struct gui_wire *w) {
    if (w->error < 0) {
        return w->error;
    }
    int sent = 0;
    while (sent < w->out_len || w->nout_fds > 0) {
        struct iovec iov;
        iov.iov_base = w->out + sent;
        iov.iov_len = (size_t)(w->out_len - sent);
        struct msghdr m;
        memset(&m, 0, sizeof(m));
        m.msg_iov = &iov;
        m.msg_iovlen = 1;
        /* The descriptors go with the first bytes. */
        uchar control[CMSG_SPACE(GUI_WIRE_FDS * sizeof(int))];
        if (w->nout_fds > 0) {
            m.msg_control = control;
            m.msg_controllen = CMSG_SPACE((size_t)w->nout_fds * sizeof(int));
            struct cmsghdr *h = CMSG_FIRSTHDR(&m);
            h->cmsg_len = CMSG_LEN((size_t)w->nout_fds * sizeof(int));
            h->cmsg_level = SOL_SOCKET;
            h->cmsg_type = SCM_RIGHTS;
            memcpy(CMSG_DATA(h), w->out_fds, (size_t)w->nout_fds * sizeof(int));
        }
        ssize_t n = sendmsg(w->fd, &m, 0);
        if (n < 0) {
            w->error = (int)n;
            return w->error;
        }
        w->nout_fds = 0;
        sent += (int)n;
        if (n == 0 && sent < w->out_len) {
            w->error = -EPIPE;
            return w->error;
        }
    }
    w->out_len = 0;
    return 0;
}

/* ---- Receiving ---- */

int gui_wire_read(struct gui_wire *w, int nowait) {
    if (w->in_pos > 0) {
        memmove(w->in, w->in + w->in_pos, (size_t)(w->in_len - w->in_pos));
        w->in_len -= w->in_pos;
        w->in_pos = 0;
    }
    if (w->in_len == (int)sizeof(w->in)) {
        return -EMSGSIZE; /* can't be: a message is at most half of it */
    }
    struct iovec iov;
    iov.iov_base = w->in + w->in_len;
    iov.iov_len = sizeof(w->in) - (size_t)w->in_len;
    uchar control[CMSG_SPACE(GUI_WIRE_FDS * sizeof(int))];
    struct msghdr m;
    memset(&m, 0, sizeof(m));
    m.msg_iov = &iov;
    m.msg_iovlen = 1;
    m.msg_control = control;
    m.msg_controllen = sizeof(control);
    ssize_t n = recvmsg(w->fd, &m, MSG_CMSG_CLOEXEC | (nowait ? MSG_DONTWAIT : 0));
    if (n < 0) {
        return (int)n;
    }
    for (struct cmsghdr *h = CMSG_FIRSTHDR(&m); h != NULL; h = CMSG_NXTHDR(&m, h)) {
        if (h->cmsg_level != SOL_SOCKET || h->cmsg_type != SCM_RIGHTS) {
            continue;
        }
        int count = (int)((h->cmsg_len - CMSG_LEN(0)) / sizeof(int));
        int fds[GUI_WIRE_FDS];
        if (count > GUI_WIRE_FDS) {
            count = GUI_WIRE_FDS;
        }
        memcpy(fds, CMSG_DATA(h), (size_t)count * sizeof(int));
        for (int i = 0; i < count; i++) {
            if (w->nin_fds < GUI_WIRE_FDS * 2) {
                w->in_fds[w->nin_fds++] = fds[i];
            } else {
                close(fds[i]); /* more than any messages could take */
                w->error = -EPROTO;
            }
        }
    }
    w->in_len += (int)n;
    return (int)n;
}

int gui_wire_next(struct gui_wire *w, struct gui_message *m) {
    int avail = w->in_len - w->in_pos;
    if (avail < HEADER) {
        return 0;
    }
    const uchar *p = w->in + w->in_pos;
    uint word = get32(p + 4);
    uint size = word >> 16;
    if (size < HEADER || size % 4 != 0 || size > GUI_WIRE_MAX) {
        return -EPROTO;
    }
    if ((int)size > avail) {
        return 0;
    }
    m->object = get32(p);
    m->opcode = word & 0xffff;
    m->size = size;
    m->args = p + HEADER;
    m->pos = 0;
    m->bad = 0;
    m->wire = w;
    w->in_pos += (int)size;
    return 1;
}

/* The arguments' bytes. */
static uint arg_bytes(const struct gui_message *m) {
    return m->size - HEADER;
}

uint gui_msg_uint(struct gui_message *m) {
    if (m->pos + 4 > arg_bytes(m)) {
        m->bad = 1;
        return 0;
    }
    uint v = get32(m->args + m->pos);
    m->pos += 4;
    return v;
}

int gui_msg_int(struct gui_message *m) {
    return (int)gui_msg_uint(m);
}

int gui_msg_fixed(struct gui_message *m) {
    return (int)gui_msg_uint(m);
}

/* `len` bytes and their padding: where they start, or NULL and `bad`. */
static const uchar *take(struct gui_message *m, uint len) {
    uint padded = (len + 3) & ~3u;
    if (padded < len || m->pos + padded > arg_bytes(m)) {
        m->bad = 1;
        return NULL;
    }
    const uchar *p = m->args + m->pos;
    m->pos += padded;
    return p;
}

const char *gui_msg_string(struct gui_message *m) {
    uint len = gui_msg_uint(m);
    if (m->bad || len == 0) {
        return NULL;
    }
    const uchar *p = take(m, len);
    if (p == NULL) {
        return NULL;
    }
    /* Ended by its NUL, and no NUL before it. */
    if (p[len - 1] != 0 || strlen((const char *)p) != len - 1) {
        m->bad = 1;
        return NULL;
    }
    return (const char *)p;
}

const void *gui_msg_array(struct gui_message *m, uint *len) {
    *len = gui_msg_uint(m);
    if (m->bad) {
        *len = 0;
        return NULL;
    }
    const uchar *p = take(m, *len);
    if (p == NULL) {
        *len = 0;
    }
    return p;
}

int gui_msg_fd(struct gui_message *m) {
    struct gui_wire *w = m->wire;
    if (w->nin_fds == 0) {
        m->bad = 1;
        return -1;
    }
    int fd = w->in_fds[0];
    w->nin_fds--;
    memmove(w->in_fds, w->in_fds + 1, (size_t)w->nin_fds * sizeof(int));
    return fd;
}
