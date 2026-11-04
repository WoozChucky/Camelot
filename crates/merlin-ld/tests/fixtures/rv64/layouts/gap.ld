/* objects: norelocs/start.o layouts/edge.o */
/* An empty R section between RX and RW, a gap of two pages inside the RW
   run, and PROGBITS after NOBITS. ld.lld keeps one RW PT_LOAD. */
ENTRY(_start)
SECTIONS
{
    . = 0x80000000;
    .text : { *(.text.start) *(.text*) }
    .rodata : { *(.rodata*) }
    .data : { *(.data .data.a) }
    . += 0x2000;
    .bss : { *(.bss*) }
    .data2 : { *(.data.c) }
    __end = .;
}
