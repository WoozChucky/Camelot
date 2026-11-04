/* objects: norelocs/start.o norelocs/data.o norelocs/more.o */
/* The headers loaded with a read-only first section and no MEMORY: they
 * share its PT_LOAD, which starts at the page below the section, and which
 * its AT moves to a load address of its own (with everything after it). */
ENTRY(_start)
SECTIONS
{
    . = 0x80100400;
    .rodata : AT(0x80000400) { *(.rodata*) }
    .text : { *(.text.start) *(.text*) }
    .data : { *(.data*) }
    .bss : { *(.bss*) }
}
