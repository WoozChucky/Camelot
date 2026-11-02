/* The boot ROM's disk loader (diskboot.s): at 0x1100 in the boot ROM, after
 * the stub (0x1000) and the park loop (0x1020). Code and strings only, all
 * in .text. */
OUTPUT_ARCH(riscv)
ENTRY(disk_boot)

MEMORY { ROM (rx) : ORIGIN = 0x1100, LENGTH = 0xF00 }

SECTIONS
{
    .text : { *(.text*) } > ROM
}
