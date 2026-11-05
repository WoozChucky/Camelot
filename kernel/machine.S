# The RV32 kernel's M-mode layer, until #159 removes RV32: Camelot's SBI
# implementation, the firmware's own (firmware/sbi.S, spec section 7,
# "Firmware"), carried in the kernel image, which the boot ROM starts in
# M-mode (entry.S's _start sends hart 0 to its coldboot, the other harts to
# wait for a hart_start). The source is the firmware's, so the two never
# differ: the S-mode kernel above makes the same SBI calls on both widths.
#
# The RV64 kernel has no M-mode code: it runs in S-mode only, on the
# firmware Grail loads with --bios default (or OpenSBI), and this file is
# empty for it.

#if __riscv_xlen == 32
#include "../firmware/sbi.S"
#endif
