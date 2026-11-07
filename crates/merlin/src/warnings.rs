//! `-w` and the `-W` options, as clang applies them to its warnings: what
//! merlin's command line (and cc1's, kernel/user/cc/diag.c, and the `cc`
//! driver's for its own warnings, kernel/user/cc.c) makes of a warning.
//!
//! A `-W` option names a warning flag or a group of them, clang's groups:
//! `-Wno-tautological-compare` turns off
//! `-Wtautological-constant-out-of-range-compare`, which the group holds.
//! The groups that hold each flag merlin and the driver give a warning of
//! are [`GROUPS`], generated from clang's `diagtool tree` by
//! `scripts/gen-warning-groups.sh`. `-Wno-everything` turns off every
//! warning, an unflagged one too, and `-Weverything` turns on those that
//! are off by default (none of merlin's); `everything` is no group for
//! `-Werror=` and `-Wno-error=`, which, as clang's, take it for an unknown
//! one. The options apply in command-line order, as clang's
//! `ProcessWarningOptions` applies them, to each warning's state ([`level`]).

/// `FLAG: GROUP...` lines, the groups that hold each flag (but `FLAG` itself
/// and `everything`), from clang's `diagtool tree`, after `//` comments (a
/// flag may start with `#`: `#warnings`).
pub const GROUPS: &str = include_str!("warning_groups.txt");

/// Each flag of [`GROUPS`], with the groups that hold it, in the file's
/// order (by flag).
pub fn table() -> Vec<(&'static str, Vec<&'static str>)> {
    GROUPS
        .lines()
        .filter(|l| !l.starts_with("//") && !l.trim().is_empty())
        .filter_map(|l| l.split_once(':'))
        .map(|(flag, groups)| (flag.trim(), groups.split_whitespace().collect()))
        .collect()
}

/// Whether option `name` (what follows `-W`, `-Wno-`, `-Werror=`...) names
/// `flag` or a group that holds it; `flag` is `None` for an unflagged
/// warning, which no name names (`-Wno-everything` is [`level`]'s).
pub fn names(name: &str, flag: Option<&str>) -> bool {
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
/// `-w` and `-W` options in command-line order, as clang's
/// `DiagnosticsEngine` decides it: the warning has a severity, a warning at
/// first (every warning merlin gives is on by default), and a "no -Werror"
/// mark. `-WX` makes it a warning, unless it is an error (a warning never
/// overrides an error); `-Wno-X` leaves it out; `-Werror=X` makes it an
/// error; `-Wno-error=X` makes an error a warning again and marks it;
/// `-Wno-everything` leaves out every warning, and `-Weverything` turns on
/// only those off by default. At the end, under the last of `-Werror` and
/// `-Wno-error`, a warning not marked is an error; and `-w`, wherever it
/// is, leaves every warning out.
pub fn level(options: &[String], flag: Option<&str>) -> Level {
    let mut severity = Level::Warning;
    let mut no_werror = false;
    let mut werror = false;
    for o in options {
        if o == "-w" {
            return Level::Ignored;
        }
        let Some(w) = o.strip_prefix("-W") else {
            continue;
        };
        match w {
            "error" => werror = true,
            "no-error" => werror = false,
            "everything" => {}
            "no-everything" => severity = Level::Ignored,
            _ => {
                if let Some(x) = w.strip_prefix("error=") {
                    if names(x, flag) {
                        severity = Level::Error;
                    }
                } else if let Some(x) = w.strip_prefix("no-error=") {
                    if names(x, flag) {
                        if severity == Level::Error {
                            severity = Level::Warning;
                        }
                        no_werror = true;
                    }
                } else if let Some(x) = w.strip_prefix("no-") {
                    if names(x, flag) {
                        severity = Level::Ignored;
                    }
                } else if names(w, flag) && severity != Level::Error {
                    severity = Level::Warning;
                }
            }
        }
    }
    if severity == Level::Warning && werror && !no_werror {
        Level::Error
    } else {
        severity
    }
}

/// `diags` under `options` ([`level`]): warnings left out, or made errors
/// (`[-Werror,-WFLAG]`, an unflagged one `[-Werror]`, as clang marks them). Errors and notes
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
                    d.flag = Some(match d.flag {
                        Some(flag) => Box::leak(format!("error,-W{flag}").into_boxed_str()),
                        None => "error",
                    });
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
        // A warning never overrides an error, but -Wno-X does, and -WX
        // then gives a warning again; -Weverything turns on nothing that
        // is on by default, and undoes no -Wno-.
        assert_eq!(
            level(&opts(&["-Werror=return-type", "-Wreturn-type"]), r),
            Level::Error
        );
        assert_eq!(
            level(
                &opts(&["-Werror=return-type", "-Wno-return-type", "-Wreturn-type"]),
                r
            ),
            Level::Warning
        );
        assert_eq!(
            level(
                &opts(&[
                    "-Wno-error=return-type",
                    "-Werror=return-type",
                    "-Wno-return-type",
                    "-Wreturn-type",
                    "-Werror"
                ]),
                r
            ),
            Level::Warning
        );
        assert_eq!(
            level(&opts(&["-Wno-everything", "-Weverything"]), r),
            Level::Ignored
        );
        assert_eq!(
            level(&opts(&["-Wno-return-type", "-Weverything"]), r),
            Level::Ignored
        );
        assert_eq!(
            level(&opts(&["-Wno-everything", "-Wreturn-type"]), r),
            Level::Warning
        );
        // `everything` is no group for -Werror= and -Wno-error=.
        assert_eq!(level(&opts(&["-Werror=everything"]), r), Level::Warning);
        assert_eq!(
            level(&opts(&["-Werror", "-Wno-error=everything"]), r),
            Level::Error
        );
        assert!(table()
            .iter()
            .any(|(f, groups)| *f == "#warnings" && groups.contains(&"cpp")));
        assert_eq!(level(&opts(&["-Werror"]), None), Level::Error);
        assert!(!is_option("-Wp,-DX") && !is_option("-Wl,-S") && !is_option("-W"));
        assert!(table().iter().any(|(f, _)| *f == "unused-command-line-argument"));
    }
}
