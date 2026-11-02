# merlin-objdump fixture: a linked executable laid out by linked.ld to hit
# the corners of labelling code in an executable:
#   - .text has no symbol at its start (it is labelled <.text>), and the
#     linker script defines `before` below the section start;
#   - `mid` (from linked.ld) sits in the middle of the last instruction of
#     .text, which is still decoded as a whole word;
#   - calls and branches into another section (.text.far) and to an absolute
#     symbol outside every section (abs_fn);
#   - .text.hi ends exactly at 2^32;
#   - every section name is shorter than 13 characters (.riscv.attributes is
#     discarded), so -h pads the name column to its minimum width.
# Built by scripts/gen-objdump-golden.sh:
#   clang --target=riscv32 -march=rv32im_zicsr_zifencei -mabi=ilp32 -c -mno-relax linked.S -o linked.o
#   ld.lld -T linked.ld linked.o -o linked.elf

    .text
    nop
    nop
    .globl _start
_start:
    call far
    jal far
    jal abs_fn
    auipc a3, 0
    jalr ra, 0(a3)
    j _start
    ret

    .section .text.far, "ax"
    .globl far
far:
    tail _start

    .section .text.hi, "ax"
    .globl hi
hi:
    nop
    nop
    j hi
    ret
