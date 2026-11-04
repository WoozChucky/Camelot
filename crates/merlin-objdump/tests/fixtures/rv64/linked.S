# merlin-objdump fixture, RV64: linked.S for RV64, laid out by
# rv64/linked.ld, with one more section high in the address space:
#   - .text has no symbol at its start (it is labelled <.text>), and the
#     linker script defines `before` below the section start;
#   - `mid` (from rv64/linked.ld) sits in the middle of the last
#     instruction of .text, which is still decoded as a whole word;
#   - calls and branches into another section (.text.far) and to an absolute
#     symbol outside every section (abs_fn);
#   - .text.hi ends exactly at 2^32, which on RV64 is an address like any;
#   - .text.top is at 0xffffffff80000000, where the kernel image goes: its
#     addresses take 16 digits, and an auipc+jalr pair there wraps nowhere.
# Built by scripts/gen-objdump-golden.sh:
#   clang --target=riscv64 -march=rv64ima_zicsr_zifencei -mabi=lp64 -c -mno-relax rv64/linked.S -o rv64/linked.o
#   ld.lld -T rv64/linked.ld rv64/linked.o -o rv64/linked.elf

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

    .section .text.top, "ax"
    .globl top
top:
    auipc a3, 0
    jalr ra, 12(a3)
    j top
    beqz a0, top
    ret
