/* The boot ROM's disk loader (diskboot.s, and diskboot-rv64.s for RV64
 * machines): at 0x1100 in the boot ROM, after the stub (0x1000) and the park
 * loop (0x1020). Code and strings only, all in .text. The ELF and program
 * headers fit below it in its page, so the linker loads them at 0x1000 in a
 * segment of their own, as ld.lld does; Grail takes only what is at 0x1100
 * and after (src/boot.rs). */
OUTPUT_ARCH(riscv)
ENTRY(disk_boot)

MEMORY { ROM (rx) : ORIGIN = 0x1100, LENGTH = 0xF00 }

SECTIONS
{
    .text : { *(.text*) } > ROM
}
