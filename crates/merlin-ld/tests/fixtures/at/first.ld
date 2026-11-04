/* objects: at/at.o */
/* AT on the first section only: every section after it keeps its
 * difference (LMA - VMA = 0x100000), so the whole image is loaded 1 MiB
 * above where it is linked to run, in the same segments as without AT.
 * .data runs where it is loaded (relative to the code), so _start's copy
 * moves nothing. */
OUTPUT_ARCH(riscv)
ENTRY(_start)

SECTIONS
{
    . = 0x80000000;
    .text : AT(0x80100000) { *(.text.start) *(.text*) }
    .rodata : { *(.rodata*) }
    .data : { __data_start = .; __data_load = .; *(.data*) __data_end = .; }
    .bss : { __bss_start = .; *(.bss*) __bss_end = .; }
}
