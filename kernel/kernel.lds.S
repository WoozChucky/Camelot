/* The Camelot kernel's layout, for both widths: one script that goes through
 * the C preprocessor first, as Linux's arch/riscv/kernel/vmlinux.lds.S does.
 * cargo xtask kernel preprocesses it for each width (merlin -E -P, no line
 * markers: a linker script is no assembly) and links with what comes out;
 * #if __riscv_xlen picks the layout.
 *
 * RV32: everything in RAM from 0x8000_0000, where the boot ROM jumps (spec
 * section 7), identity mapped once paging is on. Grail loads the ELF
 * segments where they are linked.
 *
 * RV64 (#154): a higher-half kernel, as Linux's riscv64 vmlinux.lds lays one
 * out. The image is linked at KERNEL_LINK_ADDR, 0xffff_ffff_8000_0000, the
 * top 2 GiB of the address space, where paging maps it (include/vm.h), and
 * loaded at the start of RAM, 0x8000_0000, where the boot ROM jumps: the AT
 * on the first section gives every segment's p_paddr, which Grail and the
 * bootloader load at, while p_vaddr and every symbol are the high addresses.
 * entry.S starts at the physical address, with paging off, and reaches the
 * high ones once it has turned Sv48 (or Sv39) on.
 *
 * Text, read-only data and the rest each start on a page boundary, so the
 * kernel page table (vm.c) maps them with different permissions: text R+X,
 * rodata R, everything from .data on R+W. */
OUTPUT_ARCH(riscv)
ENTRY(_start)

#if __riscv_xlen == 32
MEMORY { RAM (rwx) : ORIGIN = 0x80000000, LENGTH = 128M }
/* Every section goes in RAM, where it is loaded too. */
#define IN_RAM > RAM
#else
#define IN_RAM
#endif

SECTIONS
{
#if __riscv_xlen == 64
    . = 0xffffffff80000000;

    .text : AT(0x80000000) {
#else
    .text : {
#endif
        __text_start = .;
        *(.text.start)          /* _start must be first */
        *(.text .text.*)
    } IN_RAM

    .rodata : ALIGN(4096) {
        __rodata_start = .;     /* also the end of the text pages */
        *(.rodata .rodata.* .srodata .srodata.*)
    } IN_RAM

    .data : ALIGN(4096) {
        __data_start = .;       /* also the end of the rodata pages */
        *(.data .data.* .sdata .sdata.*)
    } IN_RAM

    .bss (NOLOAD) : ALIGN(16) {
        __bss_start = .;
        *(.sbss .sbss.* .bss .bss.* COMMON)
#if __riscv_xlen == 64
        . = ALIGN(8);
#else
        . = ALIGN(4);
#endif
        __bss_end = .;
    } IN_RAM

    /* Hart 0's boot stack: 16 KiB. On RV64 it is KSTACK_SIZE
     * (include/vm.h), aligned to twice that, as every kernel stack is, so
     * that kernel_vec's one-bit overflow check (kernelvec.S) holds for it
     * too, with a page of its own below it, after .bss, which the kernel
     * page table leaves unmapped (vm.c, kvminit()): its guard page, as the
     * other kernel stacks have one. */
#if __riscv_xlen == 64
    . += 4K;
    .stack (NOLOAD) : ALIGN(32K) {
#else
    .stack (NOLOAD) : ALIGN(16) {
#endif
        __stack_bottom = .;
        . += 16K;
        __stack_top = .;
    } IN_RAM

    /* First byte after the kernel image: the page allocator starts here. */
    . = ALIGN(4096);
    __kernel_end = .;

    /DISCARD/ : { *(.comment) *(.note .note.*) *(.eh_frame) }
}
