/* merlin-objdump fixture: a compiled C file with debug info. The compiler's
 * unnamed temporary labels end up in the symbol table as `.L0 `, which
 * llvm-objdump does not use as code labels, and the .debug_* sections show
 * as DEBUG in -h.
 * Built by scripts/gen-objdump-golden.sh:
 *   clang --target=riscv32 -march=rv32im_zicsr_zifencei -mabi=ilp32 -O1 -g -c debug.c -o debug.o
 */
extern int g(int);

int f(int x)
{
    return g(x) + g(x + 1);
}
