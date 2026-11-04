/* The RV64 Camelot kernel's layout (#154): a higher-half kernel, as Linux's
 * riscv64 vmlinux.lds lays one out. The image is linked at
 * KERNEL_LINK_ADDR, 0xffff_ffff_8000_0000, the top 2 GiB of the address
 * space, where paging maps it (include/vm.h), and loaded at the start of RAM,
 * 0x8000_0000, where the boot ROM jumps (spec section 7): the AT on the
 * first section gives every segment's p_paddr, which Grail and the
 * bootloader load at, while p_vaddr and every symbol are the high addresses.
 * entry.S starts at the physical address, with paging off, and reaches the
 * high ones once it has turned Sv48 (or Sv39) on.
 *
 * The RV32 kernel's layout is kernel.ld, identity mapped at 0x8000_0000.
 * Linux keeps one script for every configuration by preprocessing it
 * (vmlinux.lds.S); Camelot keeps two until RV32 goes (#159).
 *
 * As in kernel.ld, text, read-only data and the rest each start on a page
 * boundary, so the kernel page table (vm.c) maps them with different
 * permissions: text R+X, rodata R, everything from .data on R+W. */
OUTPUT_ARCH(riscv)
ENTRY(_start)

SECTIONS
{
    . = 0xffffffff80000000;

    .text : AT(0x80000000) {
        __text_start = .;
        *(.text.start)          /* _start must be first */
        *(.text .text.*)
    }

    .rodata : ALIGN(4096) {
        __rodata_start = .;     /* also the end of the text pages */
        *(.rodata .rodata.* .srodata .srodata.*)
    }

    .data : ALIGN(4096) {
        __data_start = .;       /* also the end of the rodata pages */
        *(.data .data.* .sdata .sdata.*)
    }

    .bss (NOLOAD) : ALIGN(16) {
        __bss_start = .;
        *(.sbss .sbss.* .bss .bss.* COMMON)
        . = ALIGN(8);
        __bss_end = .;
    }

    /* Hart 0's boot stack: KSTACK_SIZE (16 KiB, include/vm.h), aligned to
     * twice that, as every kernel stack is, so that kernel_vec's one-bit
     * overflow check (kernelvec.S) holds for it too. */
    .stack (NOLOAD) : ALIGN(32K) {
        __stack_bottom = .;
        . += 16K;
        __stack_top = .;
    }

    /* First byte after the kernel image: the page allocator starts here. */
    . = ALIGN(4096);
    __kernel_end = .;

    /DISCARD/ : { *(.comment) *(.note .note.*) *(.eh_frame) }
}
