/* objects: norelocs/start.o norelocs/data.o norelocs/more.o */
/* Assignments inside output sections and a stack section made of `. +=`. */
ENTRY(_start)
SECTIONS
{
    . = 0x80001000;
    .text : { *(.text.start) . = ALIGN(64); mid = .; *(.text*) }
    . += 0x100;
    .rodata : { *(.rodata*) }
    . = ALIGN(0x1000);
    .data : { __data_start = .; *(.data*) . += 3; __data_end = .; }
    .bss : { *(.bss*) . = ALIGN(16); }
    .stack : { . += 4K; }
    __stack_top = .;
}
