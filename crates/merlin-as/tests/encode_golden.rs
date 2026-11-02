//! Compares the encoder against llvm-mc, line by line (tests/golden/encode.golden,
//! regenerate with scripts/gen-encode-golden.sh).
//!
//! `encode_golden_progress` passes as long as nothing implemented is wrong, and
//! prints how far along the encoder is. `encode_golden_complete` (ignored by
//! default) additionally requires every line to be implemented:
//!
//!     cargo test -p merlin-as --test encode_golden -- --nocapture
//!     cargo test -p merlin-as --test encode_golden -- --ignored

use merlin_as::encode::UNIMPLEMENTED;

struct Case {
    src: String,
    words: Vec<u32>,
    /// "offset:R_RISCV_TYPE:symbol[+hexaddend]" with `<auipc>` for pcrel_lo targets.
    relocs: Vec<String>,
}

fn cases() -> Vec<Case> {
    let text = include_str!("golden/encode.golden");
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

enum Outcome {
    Pass,
    Unimplemented,
    Fail(String),
}

fn check(case: &Case) -> Outcome {
    let obj = match merlin_as::assemble(&case.src) {
        Ok(o) => o,
        Err(errs) if errs.iter().all(|e| e.msg.starts_with(UNIMPLEMENTED)) => return Outcome::Unimplemented,
        Err(errs) => return Outcome::Fail(format!("error: {}", errs[0].msg)),
    };
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
    let fmt = |w: &[u32]| w.iter().map(|w| format!("{w:08x}")).collect::<Vec<_>>().join(" ");
    if words != case.words {
        return Outcome::Fail(format!(
            "words: got [{}], want [{}]",
            fmt(&words),
            fmt(&case.words)
        ));
    }
    if relocs != case.relocs {
        return Outcome::Fail(format!("relocs: got {relocs:?}, want {:?}", case.relocs));
    }
    Outcome::Pass
}

fn run(require_all: bool) {
    let cases = cases();
    let (mut pass, mut todo, mut fails) = (0, Vec::new(), Vec::new());
    for c in &cases {
        match check(c) {
            Outcome::Pass => pass += 1,
            Outcome::Unimplemented => todo.push(c.src.clone()),
            Outcome::Fail(why) => fails.push(format!("  {:<28} {why}", c.src)),
        }
    }
    println!(
        "encoder: {pass}/{} lines match llvm-mc, {} not implemented yet",
        cases.len(),
        todo.len()
    );
    if !todo.is_empty() {
        let mut mnemonics: Vec<&str> = todo
            .iter()
            .map(|s| s.split_whitespace().next().unwrap())
            .collect();
        mnemonics.sort();
        mnemonics.dedup();
        println!("still to do: {}", mnemonics.join(" "));
    }
    assert!(
        fails.is_empty(),
        "{} lines encode differently from llvm-mc:\n{}",
        fails.len(),
        fails.join("\n")
    );
    if require_all {
        assert!(todo.is_empty(), "{} lines not implemented yet", todo.len());
    }
}

#[test]
fn encode_golden_progress() {
    run(false);
}

#[test]
#[ignore = "passes once the encoder is complete"]
fn encode_golden_complete() {
    run(true);
}
