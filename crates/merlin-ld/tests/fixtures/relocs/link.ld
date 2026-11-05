/* objects: relocs/main.o relocs/lib.o relocs/relax.o */
/* The RV64 twin of ../../relocs/link.ld, with the same addresses: main.S's
   JAL edge checks rely on .low at 0x80000000, _start (first in .text) at
   0x80100000 and .high at 0x80200000. .text.branch goes last so the rest of
   .text stays within branch range of `fail`.

   On RV64, lui reaches a sign-extended 32-bit window around 0, not RAM at
   0x80000000: `lui %hi(x)` takes -2^31 - 0x800 <= x < 2^31 - 0x800, and so
   do auipc and a call for the distance from the instruction. The absolute
   symbols at the end are what main.S aims lui at; the pc_* and call_*
   symbols sit at exactly the largest and the smallest distance from the
   instruction that names them. None of them is ever dereferenced or
   called: main.S compares the addresses it gets with the right ones. */
OUTPUT_ARCH(riscv)
OUTPUT_FORMAT("elf64-littleriscv")
ENTRY(_start)

SECTIONS
{
    . = 0x80000000;
    .low : { *(.low) }
    . = 0x80100000;
    .text : { *(.text.start) *(.text) *(.text.*) }
    .rodata : { *(.rodata*) }
    .data : { *(.data*) }
    .bss : { *(.bss*) }
    . = ALIGN(16);
    . += 4K;
    __stack_top = .;
    . = 0x80200000;
    .high : { *(.high) }

    uart = 0x10000000;                      /* lui %hi + sb %lo: the UART */
    hi_round = 0x12345800;                  /* %hi rounds up, %lo is -0x800 */
    hi_max = 0x7ffff7ff;                    /* the largest lui %hi reaches */
    hi_min = 0xffffffff7ffff800;            /* the smallest: -2^31 - 0x800 */
    kernel_base = 0xffffffff80000000;       /* a higher-half address */
    far64 = 0x123456789abcdef0;             /* only a .dword holds this */
    top64 = 0xfffffffffffffff0;             /* .dword top64 + 0x20 wraps */
    pc_max = pc_max_site + 0x7ffff7ff;
    pc_min = pc_min_site - 0x80000800;
    call_max = call_max_site + 0x7ffff7ff;
    call_min = call_min_site - 0x80000800;
}
