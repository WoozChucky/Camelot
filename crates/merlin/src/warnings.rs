//! `-w` and the `-W` options, as clang applies them to its warnings: what
//! merlin's command line (and cc1's, kernel/user/cc/diag.c, and the `cc`
//! driver's for its own warnings, kernel/user/cc.c) makes of a warning.
//!
//! A `-W` option names a warning flag or a group of them, clang's groups:
//! `-Wno-tautological-compare` turns off
//! `-Wtautological-constant-out-of-range-compare`, which the group holds.
//! The groups that hold each flag merlin and the driver give a warning of
//! are [`GROUPS`], generated from clang's `diagtool tree` by
//! `scripts/gen-warning-groups.sh`; `everything` holds every warning, an
//! unflagged one too. The options apply in command-line order, as clang's
//! `ProcessWarningOptions` applies them ([`level`]).

/// `FLAG: GROUP...` lines, the groups that hold each flag (but `FLAG` itself
/// and `everything`), from clang's `diagtool tree`.
pub const GROUPS: &str = include_str!("warning_groups.txt");

/// Each flag of [`GROUPS`], with the groups that hold it, in the file's
/// order (by flag).
pub fn table() -> Vec<(&'static str, Vec<&'static str>)> {
    GROUPS
        .lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
        .filter_map(|l| l.split_once(':'))
        .map(|(flag, groups)| (flag.trim(), groups.split_whitespace().collect()))
        .collect()
}

/// Whether option `name` (what follows `-W` or `-Wno-`) names `flag` or a
/// group that holds it; `flag` is `None` for an unflagged warning, which
/// only `everything` holds.
pub fn names(name: &str, flag: Option<&str>) -> bool {
    if name == "everything" {
        return true;
    }
    let Some(flag) = flag else {
        return false;
    };
    name == flag
        || table()
            .iter()
            .any(|(f, groups)| *f == flag && groups.contains(&name))
}

/// What a warning becomes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    /// Left out.
    Ignored,
    Warning,
    /// Made an error: `error: ... [-Werror,-WFLAG]`, which fails what gave it.
    Error,
}

/// Whether `a` is an option this module takes: `-w`, or `-W` and a name (not
/// `-Wl,`, `-Wp,` or `-Wa,`, the driver's, which hand arguments on: no
/// warning's name has a comma).
pub fn is_option(a: &str) -> bool {
    a == "-w" || (a.starts_with("-W") && a.len() > 2 && !a.contains(','))
}

/// What a warning of `flag` (`None`: unflagged) becomes under `options`, the
/// `-w` and `-W` options in command-line order, as clang decides it: every
/// warning merlin gives is on by default; `-WX` turns X's on, `-Wno-X` off;
/// `-Werror=X` turns them on and makes them errors, `-Wno-error=X` takes
/// that back; `-Werror` and `-Wno-error` decide for the warnings no
/// `-W[no-]error=` named; and `-w`, wherever it is, leaves every warning
/// out.
pub fn level(options: &[String], flag: Option<&str>) -> Level {
    let mut on = true;
    let mut error_of: Option<bool> = None;
    let mut all_errors = false;
    for o in options {
        if o == "-w" {
            return Level::Ignored;
        }
        let Some(w) = o.strip_prefix("-W") else {
            continue;
        };
        if w == "error" {
            all_errors = true;
        } else if w == "no-error" {
            all_errors = false;
        } else if let Some(x) = w.strip_prefix("error=") {
            if names(x, flag) {
                on = true;
                error_of = Some(true);
            }
        } else if let Some(x) = w.strip_prefix("no-error=") {
            if names(x, flag) {
                error_of = Some(false);
            }
        } else if let Some(x) = w.strip_prefix("no-") {
            if names(x, flag) {
                on = false;
            }
        } else if names(w, flag) {
            on = true;
        }
    }
    if !on {
        Level::Ignored
    } else if error_of.unwrap_or(all_errors) {
        Level::Error
    } else {
        Level::Warning
    }
}

/// `diags` under `options` ([`level`]): warnings left out, or made errors
/// (`[-Werror,-WFLAG]`; an unflagged one just an error). Errors and notes
/// are as they were: an error clang makes of a warning by default
/// (`-Wint-conversion`) stays one, which merlin can't compile otherwise.
/// Also whether a warning was made an error, which fails what gave it.
pub fn apply(options: &[String], diags: Vec<crate::Error>) -> (Vec<crate::Error>, bool) {
    let mut failed = false;
    let mut out = Vec::new();
    for mut d in diags {
        if d.severity == crate::Severity::Warning {
            match level(options, d.flag) {
                Level::Ignored => continue,
                Level::Warning => {}
                Level::Error => {
                    d.severity = crate::Severity::Error;
                    // A flag is a 'static str, as merlin's are: this one
                    // lives as long as the process, as it would anyway.
                    if let Some(flag) = d.flag {
                        d.flag = Some(Box::leak(format!("error,-W{flag}").into_boxed_str()));
                    }
                    failed = true;
                }
            }
        }
        out.push(d);
    }
    (out, failed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(o: &[&str]) -> Vec<String> {
        o.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn groups_and_order_are_clangs() {
        let f = Some("tautological-constant-out-of-range-compare");
        assert_eq!(level(&opts(&[]), f), Level::Warning);
        assert_eq!(level(&opts(&["-Wno-tautological-compare"]), f), Level::Ignored);
        assert_eq!(level(&opts(&["-Werror=tautological-compare"]), f), Level::Error);
        assert_eq!(level(&opts(&["-Wno-everything"]), f), Level::Ignored);
        assert_eq!(level(&opts(&["-Wno-everything"]), None), Level::Ignored);
        assert_eq!(level(&opts(&["-Wno-all"]), Some("return-type")), Level::Ignored);
        let r = Some("return-type");
        assert_eq!(
            level(&opts(&["-Wno-return-type", "-Werror=return-type"]), r),
            Level::Error
        );
        assert_eq!(
            level(&opts(&["-Werror=return-type", "-Wno-return-type"]), r),
            Level::Ignored
        );
        assert_eq!(
            level(&opts(&["-Werror", "-Wno-error=return-type"]), r),
            Level::Warning
        );
        assert_eq!(
            level(&opts(&["-Wno-error=return-type", "-Werror"]), r),
            Level::Warning
        );
        assert_eq!(level(&opts(&["-Werror", "-w"]), r), Level::Ignored);
        assert_eq!(level(&opts(&["-Werror"]), None), Level::Error);
        assert!(!is_option("-Wp,-DX") && !is_option("-Wl,-S") && !is_option("-W"));
        assert!(table().iter().any(|(f, _)| *f == "unused-command-line-argument"));
    }
}
