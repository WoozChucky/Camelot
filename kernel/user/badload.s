# badload: loads from the kernel's first page. Every process's page table
# maps the kernel, but without U, so the load is a page fault in U-mode and
# the kernel kills the process. With an argument (`badload plic`), it loads
# from the PLIC instead, 2 MiB into one of the 4 MiB megapages the kernel
# maps it with, so the kernel's report shows a megapage's mapping.

    .text
    .globl main
main:
    li    t0, 0x80000000
    li    t1, 2
    blt   a0, t1, 1f            # argc < 2: the kernel's first page
    li    t0, 0x0c201000
1:  lw    t1, 0(t0)
    li    a0, 0                 # not reached
    call  exit
