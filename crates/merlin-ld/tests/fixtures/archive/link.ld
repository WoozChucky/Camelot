/* objects: archive/crt0.o archive/main.o -Larchive -ldemo */
/* The archive after the objects, as a C program links its library: ld.lld
 * links crt0.o, main.o, then puts.o (main.o's puts), strlen.o (puts.o's
 * strlen, which came before it in libdemo.a), putc.o and exit.o, each as
 * it reads them; unused.o and hook.o never. */
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
