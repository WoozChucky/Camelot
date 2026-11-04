// merlin-objdump fixture for -l: a compile unit whose code is in two
// sections, so its ranges are a range list (DW_AT_ranges), and functions at
// the same address in each, which LLVM's symbolizer looks up whatever their
// section. Built by scripts/gen-objdump-golden.sh three ways: DWARF 5
// (.debug_rnglists), DWARF 4 (.debug_ranges, and a compilation directory the
// paths go after) and with .debug_aranges, whose relaxed lengths LLVM reads
// as the end of the list.

int f(int a) {
    return a + 1;
}

__attribute__((section(".text.b"))) int g(int a) {
    int s = 0;
    for (int i = 0; i < a; i++)
        s += i;
    return s;
}
