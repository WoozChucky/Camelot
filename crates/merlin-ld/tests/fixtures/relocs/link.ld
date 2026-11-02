/* objects: relocs/main.o relocs/lib.o relocs/relax.o */
/* main.S's JAL edge checks rely on these addresses: .low at 0x80000000,
   _start (first in .text) at 0x80100000, .high at 0x80200000. .text.branch
   goes last so the rest of .text stays within branch range of `fail`. */
OUTPUT_ARCH(riscv)
ENTRY(_start)

SECTIONS
{
    . = 0x80000000;
    .low : { *(.low) }
    . = 0x80100000;
    .text : { *(.text.start) *(.text) *(.text.*) }
    .rodata : { *(.rodata*) }
    .data : { *(.data*) }
    .bss : { *(.bss*) }
    . = ALIGN(16);
    . += 4K;
    __stack_top = .;
    . = 0x80200000;
    .high : { *(.high) }
}
