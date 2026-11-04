# A string literal the way clang emits one for C ("hello" in a mergeable
# .rodata.str1.1). b.S has the same string: ld.lld keeps one copy, merlin-ld
# keeps both (a documented difference). Built by `cargo xtask ld-fixtures`.
    .section .rodata.str1.1, "aMS", @progbits, 1
    .globl s1
s1:
    .asciz "hello"

    .section .rodata.a, "a"
    .globl after_a
after_a:
    .byte 1
