/* Layout for linked.S (see there). One segment per section keeps the file small. */
ENTRY(_start)
abs_fn = 0x7fff0000;
PHDRS
{
    text PT_LOAD;
    far PT_LOAD;
    hi PT_LOAD;
}
SECTIONS
{
    . = 0x80000000;
    .text : { before = . - 16; *(.text) mid = . - 6; } :text
    . = 0x80001000;
    .text.far : { *(.text.far) } :far
    . = 0xfffffff0;
    .text.hi : { *(.text.hi) } :hi
    /DISCARD/ : { *(.riscv.attributes) }
}
