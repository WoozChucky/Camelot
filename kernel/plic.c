/* PLIC setup and the claim/complete handshake for the S-mode context. */
#include "plic.h"
#include "mmio.h"

void plic_init(void) {
    /* Any non-zero priority beats threshold 0; 0 would mean "never". */
    mmio_write32(PLIC_PRIORITY(PLIC_IRQ_UART), 1);
    mmio_write32(PLIC_ENABLE(PLIC_CTX_S), 1u << PLIC_IRQ_UART);
    mmio_write32(PLIC_THRESHOLD(PLIC_CTX_S), 0);
}

u32 plic_claim(void) {
    return mmio_read32(PLIC_CLAIM(PLIC_CTX_S));
}

void plic_complete(u32 irq) {
    mmio_write32(PLIC_CLAIM(PLIC_CTX_S), irq);
}
