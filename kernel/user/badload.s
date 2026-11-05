# badload: loads from the kernel's first page. Every process's page table
# maps the kernel, but without U, so the load is a page fault in U-mode and
# the kernel kills the process.
#
# -0x80000000 is the kernel's first page: sign-extended, 0xffff_ffff_8000_0000,
# where the kernel image is linked, in the upper half of the address space.

    .text
    .globl main
main:
    li    t0, -0x80000000
    lw    t1, 0(t0)
    li    a0, 0                 # not reached
    call  exit
