/* Camelot's bootloader (boot.s): one boot block, 1 KiB at most, of which the
 * last 16 bytes are mkfs's (camelot-fs installs the kernel and fills them in).
 * Linked at 0, but it runs where the firmware loads it (0x8020_0000) and
 * then where it moves itself (the 64 KiB below the device tree): every
 * address it uses is pc-relative. Code and strings only, all
 * in .text, so the image is the one segment mkfs copies. */
OUTPUT_ARCH(riscv)
ENTRY(_start)

MEMORY { BOOT (rx) : ORIGIN = 0, LENGTH = 1008 }

SECTIONS
{
    .text : { *(.text*) } > BOOT
}
