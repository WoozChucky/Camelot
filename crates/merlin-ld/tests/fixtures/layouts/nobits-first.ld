/* objects: norelocs/start.o norelocs/data.o norelocs/more.o */
/* .bss before .data: ld.lld keeps one RW PT_LOAD and writes zeros for .bss. */
SECTIONS
{
    . = 0x80000000;
    .text : { *(.text*) }
    .bss : { *(.bss*) }
    .data : { *(.data*) }
    .rodata : { *(.rodata*) }
}
