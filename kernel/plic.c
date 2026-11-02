/* PLIC setup and the claim/complete handshake for the S-mode context. */
#include "plic.h"
#include "mmio.h"

void plic_init(void) {
    mmio_write32(PLIC_THRESHOLD(PLIC_CTX_S), 0);
    plic_enable(PLIC_IRQ_UART);
}

void plic_enable(u32 irq) {
    /* Any non-zero priority beats threshold 0; 0 would mean "never". */
    mmio_write32(PLIC_PRIORITY(irq), 1);
    u32 enabled = mmio_read32(PLIC_ENABLE(PLIC_CTX_S));
    mmio_write32(PLIC_ENABLE(PLIC_CTX_S), enabled | (1u << irq));
}

u32 plic_claim(void) {
    return mmio_read32(PLIC_CLAIM(PLIC_CTX_S));
}

void plic_complete(u32 irq) {
    mmio_write32(PLIC_CLAIM(PLIC_CTX_S), irq);
}
