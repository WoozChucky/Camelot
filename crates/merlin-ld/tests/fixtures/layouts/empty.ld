/* objects: norelocs/start.o layouts/edge.o */
/* Output sections that are empty or hold only assignments, with MEMORY. */
OUTPUT_ARCH(riscv)
ENTRY(_start)

MEMORY { RAM (rwx) : ORIGIN = 0x80000000, LENGTH = 16M }

SECTIONS
{
    __image_start = .;      /* before any section: 0, not the region origin */
    .text : { *(.text.start) *(.text*) } > RAM
    .rodata : { *(.rodata*) } > RAM     /* only an empty, 8-aligned input */
    .data : { *(.data) } > RAM          /* only empty inputs, one 16-aligned */
    .data2 : { *(.data.*) } > RAM
    .sbss : { __sbss_start = .; *(.sbss*) __sbss_end = .; } > RAM   /* no inputs */
    .bss : { *(.bss*) } > RAM
    .nothing : { *(.nothing*) } > RAM   /* no inputs, no assignments: dropped */
    .stack : { . += 0x100; } > RAM      /* only `. +=` */
    __stack_top = .;
}
