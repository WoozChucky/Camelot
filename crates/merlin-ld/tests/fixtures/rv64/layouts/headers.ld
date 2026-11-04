/* objects: norelocs/start.o norelocs/data.o norelocs/more.o */
/* Room for the headers below the first section, in its page: ld.lld loads
 * them at the start of the page and of the file, in a read-only PT_LOAD of
 * their own (the code after them is executable, and in a MEMORY region),
 * with a PT_PHDR for the program header table. */
ENTRY(_start)
MEMORY { RAM (rwx) : ORIGIN = 0x80000400, LENGTH = 16M }
SECTIONS
{
    .text : { *(.text.start) *(.text*) } > RAM
    .rodata : { *(.rodata*) } > RAM
    .data : { *(.data*) } > RAM
    .bss : { *(.bss*) } > RAM
}
