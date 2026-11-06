/* objects: -Larchive -lmain -lcrt -ldemo */
/* Archives alone, the program in one of its own (libmain.a): nothing is
 * linked until the entry symbol extracts crt0.o, whose main extracts
 * main.o, whose puts extracts puts.o, strlen.o and putc.o, and then
 * crt0.o's exit extracts exit.o. */
OUTPUT_ARCH(riscv)
ENTRY(_start)

MEMORY { RAM (rwx) : ORIGIN = 0x80000000, LENGTH = 4M }

SECTIONS
{
    .text : { *(.text.start) *(.text*) } > RAM
    .rodata : { *(.rodata*) } > RAM
    .data : { *(.data*) } > RAM
    .bss : { *(.bss*) *(COMMON) } > RAM
    . = ALIGN(16);
    . += 4K;
    __stack_top = .;
}
