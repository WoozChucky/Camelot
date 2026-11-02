/* objects: norelocs/start.o merge/a.o merge/b.o */
ENTRY(_start)
SECTIONS
{
    . = 0x80000000;
    .text : { *(.text.start) *(.text*) }
    .rodata : { *(.rodata*) }
    __end = .;
}
