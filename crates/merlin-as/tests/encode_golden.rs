//! Compares the encoder against LLVM, line by line, for both widths:
//! tests/golden/encode.golden (RV32) and tests/golden/rv64/encode.golden
//! (RV64: every line of encode.s again, then rv64/encode.s), regenerate both
//! with scripts/gen-encode-golden.sh. Each line is assembled on its own, so
//! every label is an undefined symbol and stays a relocation.
//!
//! And `li` on many more values than the goldens have: tests/golden/li.golden
//! and tests/golden/rv64/li.golden (scripts/gen-li-golden.sh) give clang's
//! sequence for each value, which merlin-as must pick too.

use merlin_as::{Options, Xlen};

struct Case {
    src: String,
    words: Vec<u32>,
    /// "offset:R_RISCV_TYPE:symbol[+hexaddend]" with `<auipc>` for pcrel_lo targets.
    relocs: Vec<String>,
}

fn cases(text: &str) -> Vec<Case> {
    text.lines()
        .map(|l| {
            let mut parts = l.split(" | ");
            let src = parts.next().unwrap().trim().to_string();
            let words = parts
                .next()
                .unwrap()
                .split_whitespace()
                .map(|w| u32::from_str_radix(w, 16).unwrap())
                .collect();
            let relocs = parts
                .next()
                .unwrap_or("")
                .split_whitespace()
                .map(String::from)
                .collect();
            Case { src, words, relocs }
        })
        .collect()
}

fn hex_words(w: &[u32]) -> String {
    w.iter().map(|w| format!("{w:08x}")).collect::<Vec<_>>().join(" ")
}

/// Why a line differs from LLVM on `xlen`, if it does.
fn check(case: &Case, xlen: Xlen) -> Result<(), String> {
    let obj = merlin_as::assemble_with(&case.src, &Options { xlen })
        .map_err(|errs| format!("error: {}", errs[0].msg))?;
    let text = &obj.sections[0];
    let words: Vec<u32> = text
        .data
        .chunks(4)
        .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect();
    let relocs: Vec<String> = text
        .relocs
        .iter()
        .map(|r| {
            let sym = if r.symbol.starts_with(".Lpcrel_hi") {
                "<auipc>".to_string()
            } else {
                r.symbol.clone()
            };
            let add = if r.addend == 0 {
                String::new()
            } else {
                format!("+{:x}", r.addend)
            };
            format!("{:x}:{}:{sym}{add}", r.offset, r.kind.name())
        })
        .collect();
    if words != case.words {
        return Err(format!(
            "words: got [{}], want [{}]",
            hex_words(&words),
            hex_words(&case.words)
        ));
    }
    if relocs != case.relocs {
        return Err(format!("relocs: got {relocs:?}, want {:?}", case.relocs));
    }
    Ok(())
}

/// Every line of `golden` must encode as LLVM encoded it.
fn check_all(golden: &str, xlen: Xlen, lines: usize) {
    let cases = cases(golden);
    assert_eq!(cases.len(), lines);
    let fails: Vec<String> = cases
        .iter()
        .filter_map(|c| check(c, xlen).err().map(|why| format!("  {:<28} {why}", c.src)))
        .collect();
    assert!(
        fails.is_empty(),
        "{} of {} lines encode differently from LLVM:\n{}",
        fails.len(),
        cases.len(),
        fails.join("\n")
    );
}

#[test]
fn encode_golden() {
    check_all(include_str!("golden/encode.golden"), Xlen::Rv32, 168);
}

#[test]
fn encode_golden_rv64() {
    let golden = include_str!("golden/rv64/encode.golden");
    check_all(golden, Xlen::Rv64, 299);
    // Its first 168 lines are encode.s's, and only the ISA may make them
    // differ from RV32's: `li` of a value that is negative as a 32-bit word
    // but not as a 64-bit one takes more than a `lui` on RV64.
    let rv32 = cases(include_str!("golden/encode.golden"));
    let rv64 = cases(golden);
    let differ: Vec<&str> = rv32
        .iter()
        .zip(&rv64)
        .filter(|(a, b)| {
            assert_eq!(a.src, b.src, "rv64/encode.golden does not start with encode.s");
            (&a.words, &a.relocs) != (&b.words, &b.relocs)
        })
        .map(|(a, _)| a.src.as_str())
        .collect();
    assert_eq!(differ, ["li a0, 0x80000000"]);
}

/// The values of a `li` corpus, each with clang's words for `li a0, value`.
fn li_corpus(text: &str) -> Vec<(i64, Vec<u32>)> {
    text.lines()
        .filter(|l| !l.starts_with('#'))
        .map(|l| {
            let (value, words) = l.split_once(" | ").unwrap();
            let value = u64::from_str_radix(value.strip_prefix("0x").unwrap(), 16).unwrap();
            let words = words
                .split_whitespace()
                .map(|w| u32::from_str_radix(w, 16).unwrap())
                .collect();
            (value as i64, words)
        })
        .collect()
}

/// `li a0, value` gives clang's words for every value of the corpus, and
/// `li_steps`, the sequence `li` emits, is as long as clang's. RV32 values
/// are written as 32-bit words (`0xffffffff` is -1), as RV32 code writes them.
fn check_li(text: &str, xlen: Xlen, values: usize) {
    let corpus = li_corpus(text);
    assert_eq!(corpus.len(), values);
    let mut fails = Vec::new();
    for (value, want) in &corpus {
        let src = match xlen {
            Xlen::Rv32 => format!("li a0, {:#x}", *value as u32),
            Xlen::Rv64 => format!("li a0, {:#x}", *value as u64),
        };
        let words: Vec<u32> = match merlin_as::assemble_with(&src, &Options { xlen }) {
            Ok(obj) => obj.sections[0]
                .data
                .chunks(4)
                .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                .collect(),
            Err(errs) => {
                fails.push(format!("  {src:<32} error: {}", errs[0].msg));
                continue;
            }
        };
        let value = match xlen {
            Xlen::Rv32 => *value as i32 as i64,
            Xlen::Rv64 => *value,
        };
        let steps = merlin_as::encode::li_steps(value, xlen).len();
        if &words != want || steps != want.len() {
            fails.push(format!(
                "  {src:<32} got [{}] ({steps} steps), want [{}]",
                hex_words(&words),
                hex_words(want)
            ));
        }
    }
    assert!(
        fails.is_empty(),
        "{} of {} values load differently from LLVM:\n{}",
        fails.len(),
        corpus.len(),
        fails.join("\n")
    );
}

#[test]
fn li_matches_llvm_on_rv32() {
    check_li(include_str!("golden/li.golden"), Xlen::Rv32, 425);
}

#[test]
fn li_matches_llvm_on_rv64() {
    let text = include_str!("golden/rv64/li.golden");
    check_li(text, Xlen::Rv64, 2246);
    // Every length LLVM's algorithm can give, 1 to 8 instructions.
    let mut lengths: Vec<usize> = li_corpus(text).iter().map(|(_, w)| w.len()).collect();
    lengths.sort();
    lengths.dedup();
    assert_eq!(lengths, [1, 2, 3, 4, 5, 6, 7, 8]);
}
