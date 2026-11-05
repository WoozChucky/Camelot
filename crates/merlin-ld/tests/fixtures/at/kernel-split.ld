/* objects: at/kernel.o */
/* The higher-half program with `AT(. - OFFSET)` on every section that
 * starts a segment, `.` being the section's start once aligned: .text2
 * follows .text with the same permissions but has an AT, so it starts a
 * segment of its own (two RX segments). */
OUTPUT_ARCH(riscv)
ENTRY(_start)

SECTIONS
{
    . = 0xffffffff80000000;
    .text : AT(. - 0xffffffff00000000) { *(.text.start) }
    .text2 : AT(. - 0xffffffff00000000) ALIGN(64) { *(.text .text.*) }
    .rodata : { *(.rodata .rodata.*) }
    .data : AT(. - 0xffffffff00000000) { *(.data .data.*) }
    .bss (NOLOAD) : { __bss_start = .; *(.bss .bss.*) __bss_end = .; }
    /DISCARD/ : { *(.comment) }
}
