/* objects: norelocs/start.o norelocs/data.o norelocs/more.o */
/* A gap of a page between two RW sections: still one PT_LOAD in ld.lld. */
SECTIONS
{
    . = 0x80000000;
    .text : { *(.text*) }
    .rodata : { *(.rodata*) }
    .data : { *(.data) }
    . += 0x1000;
    .data2 : { *(.data.*) }
    .bss : { *(.bss*) }
}
