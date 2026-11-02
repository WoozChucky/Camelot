/* Camelot's bootloader (boot.s): one boot block, 1 KiB at most, of which the
 * last 16 bytes are mkfs's (camelot-fs installs the kernel and fills them in).
 * Linked at 0, but it runs wherever the boot ROM loads it (the last 64 KiB
 * of RAM): every address it uses is pc-relative. Code and strings only, all
 * in .text, so the image is the one segment mkfs copies. */
OUTPUT_ARCH(riscv)
ENTRY(_start)

MEMORY { BOOT (rx) : ORIGIN = 0, LENGTH = 1008 }

SECTIONS
{
    .text : { *(.text*) } > BOOT
}
