/* The Camelot kernel's layout: a script that goes through the C preprocessor
 * first, as Linux's arch/riscv/kernel/vmlinux.lds.S does. cargo xtask kernel
 * preprocesses it (merlin -E -P, no line markers: a linker script is no
 * assembly) and links with what comes out.
 *
 * A higher-half kernel, as Linux's riscv64 vmlinux.lds lays one out (#154).
 * The image is linked at KERNEL_LINK_ADDR, 0xffff_ffff_8000_0000, the
 * top 2 GiB of the address space, where paging maps it (include/vm.h), and
 * loaded at 0x8020_0000, where the firmware starts its next stage (OpenSBI's
 * FW_JUMP_ADDR on QEMU virt, and Camelot's firmware's: spec section 7,
 * "Firmware"), right after the 2 MiB the firmware keeps at the start of
 * RAM. Linux's riscv64 Image loads there too. The AT on the first section
 * gives every segment's p_paddr, which Grail and the bootloader load at,
 * while p_vaddr and every symbol are the high addresses. entry.S starts at
 * the physical address, in S-mode with paging off, and reaches the high
 * ones once it has turned Sv48 (or Sv39) on.
 *
 * Text, read-only data and the rest each start on a page boundary, so the
 * kernel page table (vm.c) maps them with different permissions: text R+X,
 * rodata R, everything from .data on R+W. */
OUTPUT_ARCH(riscv)
ENTRY(_start)

SECTIONS
{
    . = 0xffffffff80000000;

    .text : AT(0x80200000) {
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

    /* Hart 0's boot stack: 16 KiB, KSTACK_SIZE
     * (include/vm.h), aligned to twice that, as every kernel stack is, so
     * that kernel_vec's one-bit overflow check (kernelvec.S) holds for it
     * too, with a page of its own below it, after .bss, which the kernel
     * page table leaves unmapped (vm.c, kvminit()): its guard page, as the
     * other kernel stacks have one. */
    . += 4K;
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
