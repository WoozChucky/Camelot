/* objects: at/kernel.o */
/* A higher-half kernel's layout in the form of Linux's riscv vmlinux.lds
 * (AT(ADDR(.x) - LOAD_OFFSET)), with Camelot's offset: linked at
 * 0xffffffff80000000, loaded at 0x80000000. .rodata has no AT and keeps
 * .text's difference; .data has its own, the same; the NOLOAD .bss keeps
 * it too. Three segments, each loaded 0xffffffff00000000 below where it
 * runs. */
OUTPUT_ARCH(riscv)
ENTRY(_start)

MEMORY { RAM (rwx) : ORIGIN = 0xffffffff80000000, LENGTH = 128M }

SECTIONS
{
    .text : AT(ADDR(.text) - 0xffffffff00000000) { *(.text.start) *(.text .text.*) } > RAM
    .rodata : ALIGN(4096) { *(.rodata .rodata.*) } > RAM
    .data : AT(ADDR(.data) - 0xffffffff00000000) ALIGN(4096) { *(.data .data.*) } > RAM
    .bss (NOLOAD) : ALIGN(16) { __bss_start = .; *(.bss .bss.*) __bss_end = .; } > RAM
    /DISCARD/ : { *(.comment) }
}
