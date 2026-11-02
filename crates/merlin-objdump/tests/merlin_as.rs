//! merlin-objdump on objects made by merlin-as, the files it exists to debug.
//! Only directives and `add` (the encoder's worked example) are used, so this
//! keeps working while the encoder is being written.

use merlin_objdump::{dump, Options};

const SRC: &str = "
    .text
    .word 0x00000013
start:
    add a0, a1, a2
    .word 0x0005000b
    .word 0xffffffff
    .byte 1, 2, 3

    .data
    .globl table
table:
    .word start
    .word ext + 4

    .bss
buf:
    .zero 16
";

fn object() -> Vec<u8> {
    merlin_as::assemble_to_elf(SRC).unwrap()
}

#[test]
fn disassembly() {
    let opts = Options {
        disassemble: true,
        relocs: true,
        ..Options::default()
    };
    let out = dump("t.o", &object(), &opts).unwrap();
    // No symbol at the start of .text: the section name labels it. Words that
    // do not decode are .word, the trailing odd bytes are .byte.
    let expected = "
t.o:\tfile format elf32-littleriscv

Disassembly of section .text:

00000000 <.text>:
       0: 00000013     \tnop

00000004 <start>:
       4: 00c58533     \tadd\ta0, a1, a2
       8: 0005000b     \txcal.dbg\ta0
       c: ffffffff     \t.word\t0xffffffff
      10: 01           \t.byte\t0x01
      11: 02           \t.byte\t0x02
      12: 03           \t.byte\t0x03
";
    assert_eq!(out, expected);
}

#[test]
fn data_relocations_and_symbols() {
    let opts = Options {
        relocs: true,
        symbols: true,
        section_headers: true,
        ..Options::default()
    };
    let out = dump("t.o", &object(), &opts).unwrap();
    assert!(out.contains("\nRELOCATION RECORDS FOR [.data]:\nOFFSET   TYPE                     VALUE\n"));
    assert!(out.contains("00000000 R_RISCV_32               start\n"), "{out}");
    assert!(
        out.contains("00000004 R_RISCV_32               ext+0x4\n"),
        "{out}"
    );
    assert!(out.contains("00000004 l       .text\t00000000 start\n"), "{out}");
    assert!(out.contains("00000000 g       .data\t00000000 table\n"), "{out}");
    assert!(out.contains("00000000         *UND*\t00000000 ext\n"), "{out}");
    let bss = out.lines().find(|l| l.contains(".bss ")).unwrap();
    assert!(bss.ends_with("00000010 00000000 BSS"), "{bss}");
}

/// Like llvm-objdump, a run of at least 8 zero bytes is printed as `...`,
/// but never past a relocation shown with `-r` (clang's object for the same
/// source gives the same lines, except that LLVM prints each zero halfword
/// as `0000 <unknown>`). A run whose length is not a multiple of 4 is folded
/// only up to the last whole word: 9 zero bytes, the 9th being the first byte
/// of `0x00001300`, give `...` for 8 and that word at its own address (LLVM
/// prints it as the halfwords `1300` and `0000`, from the same address).
#[test]
fn zero_runs() {
    let src = "
    .text
    .word 0x13
    .word 0, 0, 0
    .word 0x13
    .word 0
    .word 0x13
    .word 0, 0
    .word ext
    .word 0x13
    .word 0, 0
    .word 0x00001300
    .word 0x13
";
    let opts = Options {
        disassemble: true,
        relocs: true,
        ..Options::default()
    };
    let out = dump("z.o", &merlin_as::assemble_to_elf(src).unwrap(), &opts).unwrap();
    let expected = "
z.o:\tfile format elf32-littleriscv

Disassembly of section .text:

00000000 <.text>:
       0: 00000013     \tnop
\t\t...
      10: 00000013     \tnop
      14: 00000000     \t.word\t0x00000000
      18: 00000013     \tnop
\t\t...
      24: 00000000     \t.word\t0x00000000
\t\t\t00000024:  R_RISCV_32\text
      28: 00000013     \tnop
\t\t...
      34: 00001300     \t.word\t0x00001300
      38: 00000013     \tnop
";
    assert_eq!(out, expected);
}
