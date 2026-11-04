/* PLIC setup and the claim/complete handshake for hart 0's S-mode context,
 * which takes every device interrupt (plic.h). */
#include "plic.h"
#include "mmio.h"
#include "vm.h"

/* The kernel's mapping of the registers, set by plic_init(). */
static uintptr plic;

void plic_init(void) {
    plic = ioremap(PLIC_BASE, PLIC_SIZE);
    mmio_write32(plic + PLIC_THRESHOLD(PLIC_CTX_S), 0);
    plic_enable(PLIC_IRQ_UART);
}

void plic_enable(u32 irq) {
    /* Any non-zero priority beats threshold 0; 0 would mean "never". */
    mmio_write32(plic + PLIC_PRIORITY(irq), 1);
    u32 enabled = mmio_read32(plic + PLIC_ENABLE(PLIC_CTX_S));
    mmio_write32(plic + PLIC_ENABLE(PLIC_CTX_S), enabled | (1u << irq));
}

u32 plic_claim(void) {
    return mmio_read32(plic + PLIC_CLAIM(PLIC_CTX_S));
}

void plic_complete(u32 irq) {
    mmio_write32(plic + PLIC_CLAIM(PLIC_CTX_S), irq);
}
