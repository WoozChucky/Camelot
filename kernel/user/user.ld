/* Layout of a user program (spec section 10, "Kernel processes"). User space
 * is the low 4 MiB: page 0 stays unmapped so that a null pointer faults, the
 * code starts at 0x1000, and the kernel puts the stack in the last page,
 * below 0x0040_0000. Every output section starts on a page boundary, so each
 * page belongs to one segment and the kernel's ELF loader can give it that
 * segment's permissions. */
OUTPUT_ARCH(riscv)
ENTRY(_start)

SECTIONS
{
    . = 0x1000;
    .text : { *(.text.start) *(.text .text.*) }
    . = ALIGN(4096);
    .rodata : { *(.rodata .rodata.*) }
    . = ALIGN(4096);
    .data : { *(.data .data.*) }
    .bss : { *(.bss .bss.*) }
}
