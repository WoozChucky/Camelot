/* objects: norelocs/start.o norelocs/data.o norelocs/more.o */
/* Output sections without inputs take SHF_ALLOC and SHF_WRITE from the
 * previous section that had some, never SHF_EXECINSTR (ld.lld's
 * adjustOutputSections): `.marker`, after the code, is read-only and starts
 * the R segment with .rodata; `.tag`, after the data, is RW. */
OUTPUT_ARCH(riscv)
ENTRY(_start)

SECTIONS
{
    . = 0x80000000;
    .text : { *(.text.start) *(.text*) }
    .marker : { . += 4; }
    .rodata : { *(.rodata*) }
    .data : { *(.data*) }
    .tag : { __tag = .; . += 4; }
    .bss : { *(.bss*) }
}
