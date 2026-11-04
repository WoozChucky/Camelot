/* objects: at/at.o */
/* Which empty output sections count, as ld.lld decides (adjustOutputSections
 * and isDiscardable): one dropped for being empty evaluates nothing, so the
 * AT of .dropped moves no load address; one an expression needs is kept:
 * .kept, whose AT uses a symbol, sets the difference the rest of the image
 * keeps (1 MiB), .named because an ADDR names it, .aligned because its
 * ALIGN uses a symbol. */
OUTPUT_ARCH(riscv)
ENTRY(_start)

SECTIONS
{
    load = 0x80100000;
    big = 64;
    . = 0x80000000;
    .dropped : AT(0x80200000) { *(.nothing) }
    .kept : AT(load) { *(.nothing) }
    .text : { *(.text.start) *(.text*) }
    .named : { *(.nothing) }
    .aligned : ALIGN(big) { *(.nothing) }
    .rodata : { *(.rodata*) }
    .data : { __data_start = .; __data_load = .; *(.data*) __data_end = .; }
    .bss : { __bss_start = .; *(.bss*) __bss_end = .; }
    __named = ADDR(.named);
}
