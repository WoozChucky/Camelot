/* The boot ROM's disk loader (diskboot.S): at 0x1100 in the boot ROM, after
 * the stub (0x1000) and the park loop (0x1020). Code and strings only, all in
 * .text. The ELF64 file's ELF and program headers are too big for the 0x100
 * bytes below it in its page, and neither ld.lld nor merlin-ld loads them.
 * Grail takes only what is at 0x1100 and after (src/boot.rs). */
OUTPUT_ARCH(riscv)
ENTRY(disk_boot)

MEMORY { ROM (rx) : ORIGIN = 0x1100, LENGTH = 0xF00 }

SECTIONS
{
    .text : { *(.text*) } > ROM
}
