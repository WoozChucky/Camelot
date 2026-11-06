/* objects: archive/crt0.o archive/main.o -Larchive -ldemo */
/* link.ld's link, with the script naming `hook`, which only a weak
 * reference of main.o wanted: ld.lld refers to each symbol the script's
 * expressions name once every file is read, so hook.o is extracted, last,
 * and main.o's hook_ptr is its address: the exit code is 23. */
OUTPUT_ARCH(riscv)
ENTRY(_start)

MEMORY { RAM (rwx) : ORIGIN = 0x80000000, LENGTH = 4M }

SECTIONS
{
    .text : { *(.text.start) *(.text*) } > RAM
    .rodata : { *(.rodata*) } > RAM
    .data : { *(.data*) } > RAM
    .bss : { *(.bss*) *(COMMON) } > RAM
    . = ALIGN(16);
    . += 4K;
    __stack_top = .;
    __hook = hook;
}
