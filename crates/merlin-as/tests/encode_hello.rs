//! Acceptance criterion of #4: `examples/hello/hello.S` assembles with merlin-as
//! and its code matches what clang's assembler makes of the same file
//! (`tests/golden/hello.ref`, regenerate with `scripts/gen-hello-ref.sh`).
//! Running the result in Grail still needs `ld.lld` and is checked by hand.
//!
//! Ignored until the encoder is complete:
//!
//!     cargo test -p merlin-as --test encode_hello -- --ignored
//!
//! Two things stand between a finished encoder and this test passing. Both
//! are described in `ENCODER.md`:
//!
//! * hello.S writes the Xcal instructions as `.insn i 0x0b, ...` so that clang
//!   can build it. merlin-as rejects `.insn` in the parser today, before the
//!   encoder ever runs.
//! * merlin-as is single pass and turns every label reference into a
//!   relocation. clang resolves branches, jumps and calls to labels in the same
//!   section instead. Where clang resolved a word and merlin-as relocated it,
//!   this test checks that every bit outside the immediate matches and that the
//!   relocation's label is exactly where clang's resolved offset points. If
//!   merlin-as ever resolves these itself, the words must simply be equal.

use excalibur::{decode, Inst};
use merlin_as::reloc::RelocKind;
use merlin_as::Object;

const SOURCE: &str = include_str!("../../../examples/hello/hello.S");
const SECTION: &str = ".text.start";

/// One word of clang's `.text.start`.
struct RefWord {
    word: u32,
    /// (R_RISCV_ name, symbol or `<auipc>`, addend)
    reloc: Option<(String, String, i64)>,
}

fn reference() -> Vec<RefWord> {
    let mut out = Vec::new();
    for line in include_str!("golden/hello.ref").lines() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        let f: Vec<&str> = line.split_whitespace().collect();
        let off = u32::from_str_radix(f[0], 16).unwrap();
        assert_eq!(
            off as usize,
            4 * out.len(),
            "hello.ref: words must be consecutive"
        );
        let reloc = f.get(2).map(|kind| {
            let (sym, addend) = match f[3].split_once('+') {
                Some((s, a)) => (s, i64::from_str_radix(a, 16).unwrap()),
                None => (f[3], 0),
            };
            (kind.to_string(), sym.to_string(), addend)
        });
        out.push(RefWord {
            word: u32::from_str_radix(f[1], 16).unwrap(),
            reloc,
        });
    }
    out
}

/// Bits outside the immediate that a relocation of this kind fills in.
fn keep_mask(kind: RelocKind) -> Option<u32> {
    match kind {
        RelocKind::Branch => Some(0x01ff_f07f),
        RelocKind::Jal | RelocKind::CallPlt => Some(0x0000_0fff),
        _ => None,
    }
}

/// The pc-relative offset clang resolved into the word(s) at `n`.
fn resolved_offset(kind: RelocKind, refs: &[RefWord], n: usize) -> Option<i64> {
    match (kind, decode(refs[n].word)) {
        (RelocKind::Branch, Inst::Branch { imm, .. }) => Some(imm as i64),
        (RelocKind::Jal, Inst::Jal { imm, .. }) => Some(imm as i64),
        (RelocKind::CallPlt, Inst::Auipc { imm: hi, .. }) => match decode(refs.get(n + 1)?.word) {
            Inst::Jalr { imm: lo, .. } => Some(hi as i32 as i64 + lo as i64),
            _ => None,
        },
        _ => None,
    }
}

/// Every difference between our `.text.start` and clang's.
fn differences(obj: &Object, refs: &[RefWord]) -> Vec<String> {
    let Some(sec) = obj.sections.iter().position(|s| s.name == SECTION) else {
        return vec![format!("no {SECTION} section")];
    };
    let text = &obj.sections[sec];
    let words: Vec<u32> = text
        .data
        .chunks(4)
        .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect();
    if words.len() != refs.len() {
        return vec![format!("{} words, clang has {}", words.len(), refs.len())];
    }
    // Where a label of this section is, if `name` is one.
    let label_at = |name: &str| {
        obj.symbols
            .iter()
            .find(|s| s.name == name && s.defined && s.section == Some(sec))
            .map(|s| s.value)
    };
    let clang_hi_at = |off: u32| {
        refs.get(off as usize / 4)
            .and_then(|r| r.reloc.as_ref())
            .is_some_and(|r| r.0 == "R_RISCV_PCREL_HI20")
    };

    let mut diffs = Vec::new();
    // Bits of the next word to compare (the jalr after a call clang resolved).
    let mut next_mask = !0u32;
    for (n, (&ours, r)) in words.iter().zip(refs).enumerate() {
        let off = 4 * n as u32;
        let at = format!("{off:#06x}: {ours:08x} (clang {:08x})", r.word);
        let mask = std::mem::replace(&mut next_mask, !0);
        let our_reloc = text.relocs.iter().find(|x| x.offset == off);
        match (our_reloc, &r.reloc) {
            (None, None) => {
                if ours & mask != r.word & mask {
                    diffs.push(format!("{at}: words differ"));
                }
            }
            (Some(o), Some((kind, sym, addend))) => {
                if ours != r.word {
                    diffs.push(format!("{at}: words differ"));
                }
                let same_target = if sym == "<auipc>" {
                    label_at(&o.symbol).is_some_and(clang_hi_at)
                } else {
                    o.symbol == *sym && o.addend == *addend
                };
                if o.kind.name() != kind || !same_target {
                    diffs.push(format!(
                        "{at}: relocation {} {}+{:#x}, clang has {kind} {sym}+{addend:#x}",
                        o.kind.name(),
                        o.symbol,
                        o.addend
                    ));
                }
            }
            (Some(o), None) => {
                // clang resolved this one: it must be a label in this section.
                let what = format!("{at}: {} {}", o.kind.name(), o.symbol);
                let (Some(target), Some(keep), Some(want)) = (
                    label_at(&o.symbol),
                    keep_mask(o.kind),
                    resolved_offset(o.kind, refs, n),
                ) else {
                    diffs.push(format!("{what}: clang has no relocation here"));
                    continue;
                };
                if ours & keep != r.word & keep {
                    diffs.push(format!("{what}: bits outside the immediate differ"));
                }
                let got = target as i64 + o.addend - off as i64;
                if got != want {
                    diffs.push(format!(
                        "{what}: label is at offset {got:+}, clang jumps {want:+}"
                    ));
                }
                if o.kind == RelocKind::CallPlt {
                    next_mask = 0x000f_ffff; // the jalr's immediate is part of the pair
                }
            }
            (None, Some((kind, sym, _))) => {
                diffs.push(format!("{at}: no relocation, clang has {kind} {sym}"))
            }
        }
    }
    diffs
}

#[test]
fn reference_is_well_formed() {
    let refs = reference();
    assert_eq!(refs.len(), 20);
    // hello.S's first instruction is `la sp, __stack_top`.
    let first = refs[0].reloc.as_ref().unwrap();
    assert_eq!(
        (first.0.as_str(), first.1.as_str()),
        ("R_RISCV_PCREL_HI20", "__stack_top")
    );
}

#[test]
#[ignore = "passes once the encoder is complete and hello.S assembles (see ENCODER.md)"]
fn hello_matches_clang() {
    let obj = match merlin_as::assemble(SOURCE) {
        Ok(o) => o,
        Err(errs) => {
            let lines: Vec<&str> = SOURCE.lines().collect();
            let shown: Vec<String> = errs
                .iter()
                .map(|e| {
                    let src = e.line.checked_sub(1).and_then(|i| lines.get(i)).unwrap_or(&"");
                    format!("  {e}\n      {}", src.trim())
                })
                .collect();
            panic!("hello.S does not assemble:\n{}", shown.join("\n"));
        }
    };
    let diffs = differences(&obj, &reference());
    assert!(
        diffs.is_empty(),
        "{SECTION} differs from clang:\n  {}",
        diffs.join("\n  ")
    );
}
