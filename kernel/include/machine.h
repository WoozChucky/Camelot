/* The M-mode layer (spec section 10, "Kernel traps"): Camelot's SBI
 * implementation on Grail (machine.S), the firmware the S-mode kernel
 * calls through include/sbi.h. Only machine.S and entry.S's M-mode code
 * use what is here: no S-mode code touches the CLINT, an M-mode CSR or a
 * scratch block. */
#ifndef CAMELOT_MACHINE_H
#define CAMELOT_MACHINE_H

/* Constants only, for machine.S: the scratch blocks' layout. */
/* Each hart's block of machine_scratch, which its mscratch points at: slots
 * a register wide (XLEN bits). machine.S uses these slot numbers, times
 * SZREG.
 *   - Slots 0-5 hold t0-t5 while machine_vec runs (it swaps t6 with
 *     mscratch, so the SBI's arguments in a0-a7 stay where they are).
 *   - The hart's HSM state (SBI_HSM_STATE_*, include/sbi.h), a u32 in the
 *     slot's first 4 bytes, which other harts read (hart_get_status) and
 *     change (hart_start, with lr.w/sc.w).
 *   - A start request for a stopped hart: the address to start at, 0 for
 *     none (RAM, where every start address is, starts at 0x8000_0000), and
 *     the opaque value for a1. hart_start writes the opaque value first and
 *     the address last; the stopped hart waits for the address. */
#define MSCRATCH_SAVE 0        /* slots 0-5: t0-t5 */
#define MSCRATCH_HSM_STATE 6   /* SBI_HSM_STATE_*, a u32 */
#define MSCRATCH_START_ADDR 7  /* the physical address hart_start asked for, or 0 */
#define MSCRATCH_START_ARG 8   /* hart_start's opaque, which the hart gets in a1 */
#define MSCRATCH_WORDS 16      /* a power of two: a block is at hart << MSCRATCH_SHIFT */
#if __riscv_xlen == 64
#define MSCRATCH_SHIFT 7       /* log2(16 slots of 8 bytes) */
#else
#define MSCRATCH_SHIFT 6       /* log2(16 slots of 4 bytes) */
#endif

/* Nothing for C: machine.S defines machine_scratch (NCPU blocks, hart i's
 * at i << MSCRATCH_SHIFT bytes, at its physical address in mscratch, as
 * M-mode does not translate), sbi_hart_count, sbi_ram_base and
 * sbi_ram_size (what the coldboot reads from sysinfo), and machine_vec, the
 * M-mode trap vector, and the S-mode kernel uses none of them. */

#endif
