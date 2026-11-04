//! excalibur against llvm-objdump, on both widths: every instruction word of
//! `tests/golden/rv32.s` and `rv64.s`, as llvm-objdump printed it with and
//! without aliases (`scripts/gen-excalibur-golden.sh` makes the text files;
//! LLVM is not needed to run this). Each word must decode, for its width, to
//! an instruction that formats exactly as llvm-objdump did, at the same
//! address; a word llvm-objdump calls `<unknown>` must be illegal.

use excalibur::{decode, Inst, Syntax, Xlen};

/// One instruction line of llvm-objdump's output.
struct Line {
    addr: u64,
    word: u32,
    /// `mnemonic operands`, with llvm-objdump's tab turned into a space and
    /// the `<symbol+offset>` it adds after a target dropped.
    text: String,
}

/// The instruction lines of a golden file.
fn lines(name: &str) -> Vec<Line> {
    let path = format!("{}/tests/golden/{name}", env!("CARGO_MANIFEST_DIR"));
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let mut out = Vec::new();
    for l in text.lines() {
        // "      98: 00456583     \tlwu\ta1, 0x4(a0)"
        let Some((addr, rest)) = l.trim_start().split_once(": ") else {
            continue;
        };
        let Ok(addr) = u64::from_str_radix(addr, 16) else {
            continue;
        };
        let rest = rest.trim_start();
        let (word, insn) = rest
            .split_once(char::is_whitespace)
            .expect("a word and an instruction");
        let word = u32::from_str_radix(word, 16).unwrap_or_else(|_| panic!("{path}: {l}"));
        let insn = insn.trim().replacen('\t', " ", 1);
        let insn = match insn.find(" <") {
            Some(i) => insn[..i].to_string(),
            None => insn,
        };
        // The corpus is one section from address 0, and every instruction
        // is a word: a line the parser skipped would show as a gap.
        let expected = 4 * out.len() as u64;
        assert_eq!(addr, expected, "{path}: {l}: expected address {expected:#x}");
        out.push(Line {
            addr,
            word,
            text: insn,
        });
    }
    assert!(out.len() > 20, "{path}: only {} instructions", out.len());
    out
}

fn check(name: &str, xlen: Xlen, aliases: bool) {
    let syntax = Syntax {
        aliases,
        hex: true,
        xlen,
        zicbop: true,
    };
    let mut wrong = Vec::new();
    for l in lines(name) {
        let inst = decode(l.word, xlen);
        let ok = if l.text == "<unknown>" {
            inst == Inst::Illegal(l.word)
        } else {
            inst.format(Some(l.addr), syntax).to_string() == l.text
        };
        if !ok {
            wrong.push(format!(
                "{:#x}: {:08x}: llvm-objdump `{}`, excalibur `{}` ({inst:?})",
                l.addr,
                l.word,
                l.text,
                inst.format(Some(l.addr), syntax)
            ));
        }
    }
    assert!(wrong.is_empty(), "{name}:\n{}", wrong.join("\n"));
}

/// Both files of a width are one object disassembled twice: the same words
/// at the same addresses.
fn same_words(xlen: u32) {
    let words = |name: String| lines(&name).iter().map(|l| (l.addr, l.word)).collect::<Vec<_>>();
    assert_eq!(
        words(format!("rv{xlen}.objdump.txt")),
        words(format!("rv{xlen}.no-aliases.txt"))
    );
}

/// Every CSR excalibur has a name for, on either width, is in both corpora,
/// so llvm-objdump has checked each name, and that the RV32-only ones are
/// numbers on RV64.
#[test]
fn every_named_csr_is_in_both_corpora() {
    for (name, xlen) in [("rv32.objdump.txt", Xlen::Rv32), ("rv64.objdump.txt", Xlen::Rv64)] {
        let used: Vec<u16> = lines(name)
            .iter()
            .filter_map(|l| match decode(l.word, xlen) {
                Inst::Csr { csr, .. } => Some(csr),
                _ => None,
            })
            .collect();
        let missing: Vec<String> = (0..4096u16)
            .filter(|&a| excalibur::csr::name(a, Xlen::Rv32).is_some() && !used.contains(&a))
            .map(|a| format!("{a:#x}"))
            .collect();
        assert!(
            missing.is_empty(),
            "{name}: no instruction uses CSRs {}",
            missing.join(", ")
        );
    }
}

#[test]
fn both_rv64_files_hold_the_same_words() {
    same_words(64);
}

#[test]
fn both_rv32_files_hold_the_same_words() {
    same_words(32);
}

#[test]
fn rv64_matches_llvm_objdump() {
    check("rv64.objdump.txt", Xlen::Rv64, true);
}

#[test]
fn rv64_without_aliases_matches_llvm_objdump() {
    check("rv64.no-aliases.txt", Xlen::Rv64, false);
}

#[test]
fn rv32_matches_llvm_objdump() {
    check("rv32.objdump.txt", Xlen::Rv32, true);
}

#[test]
fn rv32_without_aliases_matches_llvm_objdump() {
    check("rv32.no-aliases.txt", Xlen::Rv32, false);
}
