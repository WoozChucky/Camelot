/* Camelot kernel layout: everything in RAM from 0x8000_0000, where the boot
 * ROM jumps (spec section 7). Grail loads the ELF segments directly. */
OUTPUT_ARCH(riscv)
ENTRY(_start)

MEMORY { RAM (rwx) : ORIGIN = 0x80000000, LENGTH = 128M }

SECTIONS
{
    .text : {
        *(.text.start)          /* _start must be first */
        *(.text .text.*)
    } > RAM

    .rodata : ALIGN(16) {
        *(.rodata .rodata.* .srodata .srodata.*)
    } > RAM

    .data : ALIGN(16) {
        *(.data .data.* .sdata .sdata.*)
    } > RAM

    .bss (NOLOAD) : ALIGN(16) {
        __bss_start = .;
        *(.sbss .sbss.* .bss .bss.* COMMON)
        . = ALIGN(4);
        __bss_end = .;
    } > RAM

    .stack (NOLOAD) : ALIGN(16) {
        __stack_bottom = .;
        . += 16K;
        __stack_top = .;
    } > RAM

    /* First free byte after the kernel image: the page allocator starts here. */
    . = ALIGN(4096);
    __kernel_end = .;

    /DISCARD/ : { *(.comment) *(.note .note.*) *(.eh_frame) }
}
