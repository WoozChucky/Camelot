/* objects: at/at.o */
/* AT in the middle, as for a program in ROM: .data is loaded right after
 * .rodata (at __data_load, a symbol) and runs a page later, where _start
 * copies it. The NOLOAD .bss after it has no AT and keeps .data's
 * difference, which only moves where the loader would zero it. */
OUTPUT_ARCH(riscv)
ENTRY(_start)

MEMORY { RAM (rwx) : ORIGIN = 0x80000000, LENGTH = 4M }

SECTIONS
{
    .text : { *(.text.start) *(.text*) } > RAM
    .rodata : { *(.rodata*) __data_load = .; } > RAM
    .data : AT(__data_load) ALIGN(4096) { __data_start = .; *(.data*) __data_end = .; } > RAM
    .bss (NOLOAD) : { __bss_start = .; *(.bss*) __bss_end = .; } > RAM
}
