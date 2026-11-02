//! Compares the encoder against llvm-mc, line by line (tests/golden/encode.golden,
//! regenerate with scripts/gen-encode-golden.sh). Each line is assembled on
//! its own, so every label is an undefined symbol and stays a relocation.

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

/// Why a line differs from llvm-mc, if it does.
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
    let fmt = |w: &[u32]| w.iter().map(|w| format!("{w:08x}")).collect::<Vec<_>>().join(" ");
    if words != case.words {
        return Err(format!(
            "words: got [{}], want [{}]",
            fmt(&words),
            fmt(&case.words)
        ));
    }
    if relocs != case.relocs {
        return Err(format!("relocs: got {relocs:?}, want {:?}", case.relocs));
    }
    Ok(())
}

#[test]
fn encode_golden() {
    let cases = cases();
    assert_eq!(cases.len(), 118);
    let fails: Vec<String> = cases
        .iter()
        .filter_map(|c| check(c).err().map(|why| format!("  {:<28} {why}", c.src)))
        .collect();
    assert!(
        fails.is_empty(),
        "{} of {} lines encode differently from llvm-mc:\n{}",
        fails.len(),
        cases.len(),
        fails.join("\n")
    );
}
