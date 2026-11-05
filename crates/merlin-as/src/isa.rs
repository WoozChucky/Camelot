//! The ISA a source may name: `.option arch` and `.attribute arch` (or
//! `.attribute 5`), read as LLVM's `RISCVISAInfo` reads them, far enough to
//! tell whether they name the ISA merlin-as assembles for, `-march`'s.
//!
//! In clang they change the ISA: `.option arch, +zicond` turns on an
//! extension for the code after it (and a new `$x<isa>` mapping symbol
//! names it), `.attribute arch, "..."` sets the object's `Tag_RISCV_arch`
//! and the name of every `$x`. merlin-as has no other extensions than
//! `-march`'s (`rv64ima_zicsr_zifencei`), so one that names another ISA (an
//! RV32 one among them) is an error; one that names the same ISA, as clang's
//! `-S` output does (`.attribute 5, "rv64i2p1_m2p0_..."`), is accepted and
//! changes nothing.
//!
//! Two ISAs are the same when they have the same base width and the same
//! extensions at the same versions, once each has the extensions the others
//! imply as LLVM adds them: `m` implies `zmmul`, `a` implies `zaamo` and
//! `zalrsc`, and `zaamo` with `zalrsc` makes `a` (so `.option arch, -a`
//! changes nothing, as in LLVM, which writes no new `$x` for it).

use std::collections::BTreeMap;

/// An ISA: its extensions and their versions (major, minor).
type Exts = BTreeMap<String, (u32, u32)>;

/// merlin-as's extensions, at LLVM 23's versions: what [`crate::elf::ARCH`]
/// spells.
const OURS: [(&str, u32, u32); 8] = [
    ("i", 2, 1),
    ("m", 2, 0),
    ("a", 2, 1),
    ("zicsr", 2, 0),
    ("zifencei", 2, 0),
    ("zmmul", 1, 0),
    ("zaamo", 1, 0),
    ("zalrsc", 1, 0),
];

fn ours() -> Exts {
    OURS.iter().map(|&(n, a, b)| (n.to_string(), (a, b))).collect()
}

/// Adds what the extensions imply, as LLVM does.
fn imply(e: &mut Exts) {
    if e.contains_key("m") {
        e.entry("zmmul".into()).or_insert((1, 0));
    }
    if e.contains_key("a") {
        e.entry("zaamo".into()).or_insert((1, 0));
        e.entry("zalrsc".into()).or_insert((1, 0));
    }
    if e.contains_key("zaamo") && e.contains_key("zalrsc") {
        e.entry("a".into()).or_insert((2, 1));
    }
}

/// The default version of an extension, LLVM 23's; None for one merlin-as
/// does not know (which is another ISA whatever its version).
fn default_version(name: &str) -> Option<(u32, u32)> {
    OURS.iter().find(|&&(n, ..)| n == name).map(|&(_, a, b)| (a, b))
}

/// Splits a trailing version (`2p1`, `2`) off an extension's name.
fn split_version(s: &str) -> (&str, Option<(u32, u32)>) {
    let digits = s.trim_end_matches(|c: char| c.is_ascii_digit() || c == 'p');
    let tail = &s[digits.len()..];
    // `p` belongs to the version only between digits.
    let tail = tail.trim_start_matches('p');
    let name = &s[..s.len() - tail.len()];
    if tail.is_empty() {
        return (s, None);
    }
    let (major, minor) = tail.split_once('p').unwrap_or((tail, "0"));
    match (major.parse(), minor.parse()) {
        (Ok(a), Ok(b)) if !minor.contains('p') => (name, Some((a, b))),
        _ => (s, None),
    }
}

/// A full ISA string (`rv64ima_zicsr`, `rv64i2p1_m2p0_..._zalrsc1p0`): its
/// width and its extensions with what they imply. None if it is no ISA
/// string LLVM would take with merlin-as's extensions: then it names some
/// other ISA, or none.
fn parse(s: &str) -> Option<(u32, Exts)> {
    let (bits, rest) = match s.strip_prefix("rv32") {
        Some(r) => (32, r),
        None => (64, s.strip_prefix("rv64")?),
    };
    let mut exts = Exts::new();
    let mut add = |name: &str, version: Option<(u32, u32)>| -> Option<()> {
        let v = version.or_else(|| default_version(name))?;
        exts.insert(name.to_string(), v);
        Some(())
    };
    for (k, token) in rest.split('_').enumerate() {
        if token.is_empty() {
            return None;
        }
        // Multi-letter extensions (`zicsr2p0`), each a token of its own.
        if k > 0 && token.starts_with(['z', 's', 'x']) {
            let (name, version) = split_version(token);
            add(name, version)?;
            continue;
        }
        // Single letters, each with its version (`i2p1m2p0`, `ima`); the
        // base (`i`) first.
        let b = token.as_bytes();
        let mut i = 0;
        while i < b.len() {
            if !b[i].is_ascii_lowercase() || (k == 0 && i == 0 && b[i] != b'i') {
                return None;
            }
            let start = i;
            i += 1;
            while i < b.len()
                && (b[i].is_ascii_digit() || (b[i] == b'p' && i + 1 < b.len() && b[i + 1].is_ascii_digit()))
            {
                i += 1;
            }
            let (name, version) = split_version(&token[start..i]);
            add(name, version)?;
        }
    }
    imply(&mut exts);
    Some((bits, exts))
}

/// The base width of merlin-as's ISA: RV64.
const BITS: u32 = 64;

/// The error for a directive that names another ISA than `-march`'s.
fn other(directive: &str) -> String {
    format!(
        "{directive}: merlin-as assembles only -march's ISA ({}), and this names another",
        crate::elf::ARCH
    )
}

/// `.attribute arch, "isa"` (`.attribute 5, "isa"`): Ok if `isa` is
/// `-march`'s ISA.
pub fn check_attribute(isa: &str) -> Result<(), String> {
    match parse(isa) {
        Some((b, e)) if b == BITS && e == ours() => Ok(()),
        _ => Err(other(".attribute arch")),
    }
}

/// `.option arch, ...`: a full ISA string, or `+ext` and `-ext` items
/// applied to the ISA (always `-march`'s: merlin-as never changes it). Ok
/// if the result is `-march`'s ISA. LLVM's own errors come first: a version
/// after `+` (`+m2p0`) is "extension version number parsing not currently
/// implemented", and taking away an extension another one needs (`-zmmul`
/// with `m`) is "can't disable".
pub fn check_option_arch(items: &[String]) -> Result<(), String> {
    let mut exts = ours();
    match items {
        [] => return Err(".option arch: expected an ISA, or extensions after + or -".into()),
        [full] if full.starts_with("rv") => {
            return match parse(full) {
                Some((b, e)) if b == BITS && e == exts => Ok(()),
                _ => Err(other(".option arch")),
            };
        }
        _ => {}
    }
    for item in items {
        if let Some(name) = item.strip_prefix('+') {
            if split_version(name).1.is_some() {
                return Err("extension version number parsing not currently implemented".into());
            }
            let Some(v) = default_version(name) else {
                return Err(other(".option arch"));
            };
            exts.insert(name.to_string(), v);
        } else if let Some(name) = item.strip_prefix('-') {
            let needs = |by: &str| exts.contains_key(by).then(|| by.to_string());
            let needed = match name {
                "zmmul" => needs("m"),
                "zaamo" | "zalrsc" => needs("a"),
                _ => None,
            };
            if let Some(by) = needed {
                return Err(format!(
                    "can't disable {name} extension; {by} extension requires {name} extension"
                ));
            }
            exts.remove(name);
        } else {
            return Err(format!(
                ".option arch: expected an ISA, or extensions after + or -, not '{item}'"
            ));
        }
        imply(&mut exts);
    }
    if exts == ours() {
        Ok(())
    } else {
        Err(other(".option arch"))
    }
}

/// `.option rvc`: the C extension, which merlin-as does not have.
pub fn rvc() -> String {
    other(".option rvc")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items(s: &[&str]) -> Vec<String> {
        s.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn the_same_isa_however_it_is_written() {
        for isa in [
            "rv64ima_zicsr_zifencei",
            "rv64i2p1_m2p0_a2p1_zicsr2p0_zifencei2p0_zmmul1p0_zaamo1p0_zalrsc1p0",
            "rv64i2p1_m2p0_a2p1_zicsr2p0_zifencei2p0",
            "rv64ima2p1_zicsr_zifencei",
            "rv64im_zicsr_zifencei_zaamo_zalrsc",
            "rv64i_zicsr_zifencei_zaamo_zalrsc_m",
        ] {
            assert_eq!(check_attribute(isa), Ok(()), "{isa}");
            assert_eq!(check_option_arch(&items(&[isa])), Ok(()), "{isa}");
        }
        assert_eq!(check_attribute(crate::elf::ARCH), Ok(()));
        for list in [
            &["+m"][..],
            &["+zmmul"],
            &["+zaamo"],
            &["+m", "+zmmul", "-c"],
            &["-a"],
        ] {
            assert_eq!(check_option_arch(&items(list)), Ok(()), "{list:?}");
        }
    }

    #[test]
    fn another_isa_is_refused() {
        for isa in [
            "rv64i2p1_m2p0",
            "rv32ima_zicsr_zifencei",
            "rv64imac_zicsr_zifencei",
            "rv64ima_zicsr",
            "rv64gc",
            "rv64i2p0_m2p0_a2p1_zicsr2p0_zifencei2p0",
            "rv64ima_zicsr_zifencei_zicond",
            "RV64IMA_ZICSR_ZIFENCEI",
            "",
        ] {
            assert!(check_attribute(isa).is_err(), "{isa}");
        }
        for list in [&["+zicond"][..], &["-m"], &["-zicsr"], &["+c"], &["rv64imac"]] {
            assert!(check_option_arch(&items(list)).is_err(), "{list:?}");
        }
        assert_eq!(
            check_option_arch(&items(&["-zmmul"])),
            Err("can't disable zmmul extension; m extension requires zmmul extension".into())
        );
        assert_eq!(
            check_option_arch(&items(&["+m2p0"])),
            Err("extension version number parsing not currently implemented".into())
        );
    }
}
