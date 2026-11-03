/* The program the elf crate's LLVM fixtures are made of, for RV32 and RV64
 * (scripts/gen-elf-golden.sh; tests/llvm.rs reads them back). Small, but with
 * a symbol of every kind, data, read-only data and zeroed data, calls and
 * address relocations, and code in two sections for the line tables. */

int counter;                    /* .bss */
int table[4] = {1, 2, 3, 4};    /* .data */
const char message[] = "elf";   /* .rodata */
int *where = &table[2];         /* .data, an address: R_RISCV_32 or R_RISCV_64 */

static int twice(int x) {
    return x * 2;
}

int sum(const int *p, int n) {
    int s = 0;
    for (int i = 0; i < n; i++)
        s += twice(p[i]);
    return s;
}

__attribute__((section(".text.other"))) int other(void) {
    return counter + message[0] + *where;
}

void _start(void) {
    counter = sum(table, 4) + other();
    for (;;) {
    }
}
