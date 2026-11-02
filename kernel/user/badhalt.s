# badhalt: tries to stop the machine. xcal.halt is M-mode only (spec section
# 9.2), so in U-mode it is an illegal instruction, and the kernel kills the
# process instead.

    .section .text.start
    .globl _start
_start:
    li    a0, 0
    xcal.halt a0
    call  exit                  # not reached
