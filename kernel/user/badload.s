# badload: loads from the kernel's first page. Every process's page table
# maps the kernel, but without U, so the load is a page fault in U-mode and
# the kernel kills the process.

    .text
    .globl main
main:
    li    t0, 0x80000000
    lw    t1, 0(t0)
    li    a0, 0                 # not reached
    call  exit
