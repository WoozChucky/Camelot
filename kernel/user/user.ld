/* Layout of a user program (spec section 10, "Kernel processes";
 * docs/camelot-syscalls.md, "Starting a program"), the same for RV32 and
 * RV64. Page 0 stays unmapped so that a null pointer faults, and the code
 * starts at 0x1000 with crt0's _start. Every output section starts on a
 * page boundary, so each page belongs to one segment and the kernel's ELF
 * loader can give it that segment's permissions. The heap (sbrk) starts on
 * the page after the last one. The stack is the kernel's to place: at the
 * top of user space (USER_TOP: 0x0C00_0000 on RV32; on RV64 the top of the
 * lower half, 2^47 in Sv48 or 2^38 in Sv39), growing down to an 8 MiB
 * limit. The program headers are in no segment (the first starts past
 * them), so the auxiliary vector's AT_PHDR is 0. */
OUTPUT_ARCH(riscv)
ENTRY(_start)

SECTIONS
{
    . = 0x1000;
    .text : { *(.text.start) *(.text .text.*) }
    . = ALIGN(4096);
    .rodata : { *(.rodata .rodata.* .srodata .srodata.*) }
    . = ALIGN(4096);
    .data : { *(.data .data.* .sdata .sdata.*) }
    .bss : { *(.sbss .sbss.* .bss .bss.*) }
}
