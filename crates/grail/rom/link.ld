/* The boot ROM's disk loader (diskboot.S, built for RV32 and for RV64
 * machines): at 0x1100 in the boot ROM, after the stub (0x1000) and the park
 * loop (0x1020). Code and strings only, all in .text. An ELF32 file's ELF and
 * program headers fit in the 0x100 bytes below it in its page, so the linker
 * loads them at 0x1000 in a segment of their own, as ld.lld does; an ELF64
 * file's (diskboot-rv64.elf) are too big for them, and neither ld.lld nor
 * merlin-ld loads them. Grail takes only what is at 0x1100 and after
 * (src/boot.rs). */
OUTPUT_ARCH(riscv)
ENTRY(disk_boot)

MEMORY { ROM (rx) : ORIGIN = 0x1100, LENGTH = 0xF00 }

SECTIONS
{
    .text : { *(.text*) } > ROM
}
