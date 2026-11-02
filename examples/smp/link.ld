OUTPUT_ARCH(riscv)
ENTRY(_start)

MEMORY { RAM (rwx) : ORIGIN = 0x80000000, LENGTH = 128M }

SECTIONS
{
    .text : { *(.text.start) *(.text*) } > RAM
    .rodata : { *(.rodata*) } > RAM
    .data : { *(.data*) } > RAM
    .bss : { *(.bss*) *(COMMON) } > RAM
    . = ALIGN(16);
    . += 64K;
    __stack_top = .;
}
