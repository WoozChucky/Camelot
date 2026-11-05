# badload: loads from the kernel's first page. Every process's page table
# maps the kernel, but without U, so the load is a page fault in U-mode and
# the kernel kills the process. With an argument (`badload plic`), it loads
# from the PLIC instead, 2 MiB into one of the 4 MiB megapages the RV32
# kernel maps it with, so the kernel's report shows a megapage's mapping
# (on RV64, where the kernel maps devices in its own half, 0x0c201000 is
# just an unmapped user address).
#
# -0x80000000 is the kernel's first page on both widths: 0x80000000 on RV32,
# and on RV64, sign-extended, 0xffff_ffff_8000_0000, where the kernel image
# is linked, in the upper half of the address space.

    .text
    .globl main
main:
    li    t0, -0x80000000
    li    t1, 2
    blt   a0, t1, 1f            # argc < 2: the kernel's first page
    li    t0, 0x0c201000
1:  lw    t1, 0(t0)
    li    a0, 0                 # not reached
    call  exit
