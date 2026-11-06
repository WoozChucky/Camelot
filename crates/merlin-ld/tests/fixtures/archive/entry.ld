/* objects: archive/main.o -Larchive -lcrt -l:libdemo.a */
/* The start code in an archive of its own, libcrt.a: nothing refers to
 * _start, but it is the entry symbol, so ld.lld extracts crt0.o once every
 * file is read, and crt0.o's exit then extracts exit.o, last. -l:FILE names
 * the archive's file itself. */
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
