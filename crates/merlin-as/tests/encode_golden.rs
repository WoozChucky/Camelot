//! Compares the encoder against LLVM, line by line: tests/golden/encode.golden,
//! every line of encode.s as clang assembles it for riscv64 (regenerate it with
//! scripts/gen-encode-golden.sh). Each line is assembled on its own, so every
//! label is an undefined symbol and stays a relocation.
//!
//! And `li` on many more values than the golden has: tests/golden/li.golden
//! (scripts/gen-li-golden.sh) gives clang's sequence for each value, which
//! merlin-as must pick too.

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

/// Why a line differs from LLVM, if it does.
fn check(case: &Case) -> Result<(), String> {
    let obj = merlin_as::assemble(&case.src).map_err(|errs| format!("error: {}", errs[0].msg))?;
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
fn check_all(golden: &str, lines: usize) {
    let cases = cases(golden);
    assert_eq!(cases.len(), lines);
    let fails: Vec<String> = cases
        .iter()
        .filter_map(|c| check(c).err().map(|why| format!("  {:<28} {why}", c.src)))
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
    check_all(include_str!("golden/encode.golden"), 299);
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
/// `li_steps`, the sequence `li` emits, is as long as clang's.
fn check_li(text: &str, values: usize) {
    let corpus = li_corpus(text);
    assert_eq!(corpus.len(), values);
    let mut fails = Vec::new();
    for (value, want) in &corpus {
        let src = format!("li a0, {:#x}", *value as u64);
        let words: Vec<u32> = match merlin_as::assemble(&src) {
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
        let steps = merlin_as::encode::li_steps(*value).len();
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
fn li_matches_llvm() {
    let text = include_str!("golden/li.golden");
    check_li(text, 2246);
    // Every length LLVM's algorithm can give, 1 to 8 instructions.
    let mut lengths: Vec<usize> = li_corpus(text).iter().map(|(_, w)| w.len()).collect();
    lengths.sort();
    lengths.dedup();
    assert_eq!(lengths, [1, 2, 3, 4, 5, 6, 7, 8]);
}
