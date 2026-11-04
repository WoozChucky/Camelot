/* objects: norelocs/start.o norelocs/data.o norelocs/more.o */
/* A first section that is not at the start of a page, without room below
 * it for the headers (0x114 bytes for ELF32, 0x1c8 for ELF64: the ELF
 * header and the 7 program headers ld.lld counts, see "The headers" in
 * merlin-ld's src/write.rs). ld.lld loads no headers and puts the section a
 * page into the file, at offset 0x1100, after the room it left for them. */
ENTRY(_start)
SECTIONS
{
    . = 0x80000100;
    .text : { *(.text.start) *(.text*) }
    .rodata : { *(.rodata*) }
    .data : { *(.data*) }
    .bss : { *(.bss*) }
}
