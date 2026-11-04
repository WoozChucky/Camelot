/* objects: norelocs/start.o norelocs/data.o norelocs/more.o layouts/edge.o layouts/attrs.o */
/* Output section attributes as kernel/kernel.ld uses them, with MEMORY:
 * ALIGN(n) after the colon (above and below the inputs' alignment), NOLOAD
 * on sections of NOBITS inputs, of PROGBITS inputs and of assignments only,
 * and /DISCARD/ after a top-level `. = ALIGN(...)`. */
OUTPUT_ARCH(riscv)
ENTRY(_start)

MEMORY { RAM (rwx) : ORIGIN = 0x80000000, LENGTH = 16M }

SECTIONS
{
    .text : { *(.text.start) *(.text*) } > RAM
    .rodata : ALIGN(4096) { __rodata_start = .; *(.rodata*) } > RAM
    .data : ALIGN(2) { *(.data*) } > RAM            /* inputs want 16 */
    .bss (NOLOAD) : ALIGN(64) { __bss_start = .; *(.bss*) } > RAM
    .noload (NOLOAD) : { *(.noload*) } > RAM
    .stack (NOLOAD) : ALIGN(16) { . += 0x100; __stack_top = .; } > RAM
    . = ALIGN(4096);
    __end = .;
    /DISCARD/ : { *(.discard*) }
}
