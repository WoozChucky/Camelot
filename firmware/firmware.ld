/* Camelot's firmware image (spec section 7, "Firmware"): linked and loaded
 * at the start of RAM, 0x8000_0000, in the 2 MiB that OpenSBI's fw_jump
 * keeps for itself on QEMU virt; the next stage starts right after it, at
 * 0x8020_0000. M-mode runs untranslated, so the image runs where it is
 * linked. The entry (entry.S's _start) comes first: Grail starts the
 * firmware at its ELF entry, and a raw copy of the image would start at
 * its first byte. */
OUTPUT_ARCH(riscv)
ENTRY(_start)

MEMORY { FW (rwx) : ORIGIN = 0x80000000, LENGTH = 2M }

SECTIONS
{
    .text : {
        *(.text.entry)          /* _start must be first */
        *(.text .text.*)
    } > FW

    .rodata : {
        *(.rodata .rodata.* .srodata .srodata.*)
    } > FW

    .data : {
        *(.data .data.* .sdata .sdata.*)
    } > FW

    /* The scratch blocks and what the coldboot reads: zeroed by hart 0
     * (entry.S), not loaded. */
    .bss (NOLOAD) : ALIGN(16) {
        __bss_start = .;
        *(.sbss .sbss.* .bss .bss.* COMMON)
        . = ALIGN(8);
        __bss_end = .;
    } > FW

    __fw_end = .;

    /DISCARD/ : { *(.comment) *(.note .note.*) *(.eh_frame) }
}
