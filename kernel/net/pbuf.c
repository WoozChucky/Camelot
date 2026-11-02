/* The packet buffer pool (docs/camelot-net.md, "Buffers"): NPBUF buffers of
 * PBUF_SIZE bytes, two to a page from kalloc, taken when the card is set
 * up. Every frame the stack receives or sends lives in one, from the card's
 * DMA to the socket that reads it or the card that sends it, so nothing is
 * ever copied between layers on the way down, and a fixed pool means the
 * network can never eat the rest of memory: when it is empty, frames are
 * dropped instead (counted in no_buffer). */
#include "kalloc.h"
#include "memlayout.h"
#include "net.h"

static struct pbuf pbufs[NPBUF];
/* The free buffers, linked through `next`. */
static struct pbuf *free_list;
static u32 free_count;

bool pbuf_init(void) {
    u32 per_page = PGSIZE / PBUF_SIZE;
    for (u32 i = 0; i < NPBUF; i += per_page) {
        u8 *page = kalloc();
        if (page == NULL) {
            return false;
        }
        for (u32 j = 0; j < per_page; j++) {
            pbufs[i + j].data = page + j * PBUF_SIZE;
            pbuf_free(&pbufs[i + j]);
        }
    }
    return true;
}

struct pbuf *pbuf_alloc(void) {
    struct pbuf *p = free_list;
    if (p == NULL) {
        net_stats.no_buffer++;
        return NULL;
    }
    free_list = p->next;
    free_count--;
    p->next = NULL;
    p->len = 0;
    p->off = 0;
    p->addr = 0;
    p->port = 0;
    return p;
}

void pbuf_free(struct pbuf *p) {
    p->next = free_list;
    free_list = p;
    free_count++;
}

u32 pbuf_available(void) {
    return free_count;
}

void pbuf_push(struct pbuf_queue *q, struct pbuf *p) {
    p->next = NULL;
    if (q->tail == NULL) {
        q->head = p;
    } else {
        q->tail->next = p;
    }
    q->tail = p;
    q->len++;
}

struct pbuf *pbuf_pop(struct pbuf_queue *q) {
    struct pbuf *p = q->head;
    if (p == NULL) {
        return NULL;
    }
    q->head = p->next;
    if (q->head == NULL) {
        q->tail = NULL;
    }
    q->len--;
    p->next = NULL;
    return p;
}

void pbuf_drain(struct pbuf_queue *q) {
    for (;;) {
        struct pbuf *p = pbuf_pop(q);
        if (p == NULL) {
            return;
        }
        pbuf_free(p);
    }
}
