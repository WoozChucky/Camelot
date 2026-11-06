/* objects: -Larchive -ldemo archive/crt0.o archive/main.o */
/* The archive before the objects: ld.lld notes its members first, so
 * crt0.o's exit extracts exit.o as soon as crt0.o is read, before main.o,
 * whose puts then extracts puts.o, strlen.o and putc.o. */
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
