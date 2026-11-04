/* objects: at/at.o */
/* A section with an AT starts a segment, even right after a section with
 * the same permissions and even loaded where it runs (`AT(.)`, `.` being
 * its aligned start): two RX segments, .text and .text2. */
OUTPUT_ARCH(riscv)
ENTRY(_start)

SECTIONS
{
    . = 0x80000000;
    .text : { *(.text.start) }
    .text2 : AT(.) ALIGN(16) { *(.text*) }
    .rodata : { *(.rodata*) }
    .data : { __data_start = .; __data_load = .; *(.data*) __data_end = .; }
    .bss : { __bss_start = .; *(.bss*) __bss_end = .; }
}
