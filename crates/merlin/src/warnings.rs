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
//!
//! An option that names no group of clang's ([`OPTIONS`], every name
//! `diagtool tree` lists) is clang's warning `unknown warning option '-Wfoo'
//! [-Wunknown-warning-option]`, a warning like any other under `-w` and the
//! `-W` options ([`option_warnings`]).

/// `FLAG: GROUP...` lines, the groups that hold each flag (but `FLAG` itself
/// and `everything`), from clang's `diagtool tree`, after `//` comments (a
/// flag may start with `#`: `#warnings`).
pub const GROUPS: &str = include_str!("warning_groups.txt");

/// Every warning group clang has, a name a line after `//` comments, from
/// clang's `diagtool tree`: the names a `-W` option may give.
pub const OPTIONS: &str = include_str!("warning_options.txt");

/// Whether `name` is one of clang's warning groups ([`OPTIONS`]); the empty
/// name, `-W` alone, is the root of them all.
pub fn known(name: &str) -> bool {
    name.is_empty() || OPTIONS.lines().any(|l| l == name)
}

/// The warnings clang's `-cc1` gives of its `-W` options as it starts, in
/// their order, before any other diagnostic (`ProcessWarningOptions`'
/// report pass): a name that is no group, `unknown warning option
/// '-WNAME'` (`-Wno-NAME`, and `-Werror=NAME` for `-Werror=` and
/// `-Wno-error=` alike, as clang names it; `everything` among them, which
/// is no group), and a `-Werror` or `-Wfatal-errors` followed by what is no
/// `=NAME`, `unknown -Werror warning specifier: '-Werrorfoo'`. Each is of
/// the flag `unknown-warning-option`, to be applied as any warning
/// ([`apply`]). No `did you mean` follows the name, where clang may suggest
/// one.
pub fn option_warnings(options: &[String]) -> Vec<crate::Error> {
    let mut out = Vec::new();
    let mut say = |msg: String| {
        let mut e = crate::Error::new(crate::Pos::default(), msg).unplaced();
        e.severity = crate::Severity::Warning;
        e.flag = Some("unknown-warning-option");
        out.push(e);
    };
    for o in options {
        let Some(w) = o.strip_prefix("-W") else {
            continue; // -w
        };
        let w = if w == "format=0" { "no-format" } else { w };
        let (positive, opt) = match w.strip_prefix("no-") {
            Some(rest) => (false, rest),
            None => (true, w),
        };
        if opt == "system-headers" || opt == "everything" {
            continue;
        }
        if let Some(rest) = opt.strip_prefix("error") {
            if rest.is_empty() {
                continue;
            }
            if !rest.starts_with('=') && rest != "-implicit-function-declaration" {
                say(format!("unknown -Werror warning specifier: '{o}'"));
                continue;
            }
            let name = &rest[1..];
            if !name.is_empty() && !known(name) {
                say(format!("unknown warning option '-Werror={name}'"));
            }
            continue;
        }
        if let Some(rest) = opt.strip_prefix("fatal-errors") {
            if rest.is_empty() {
                continue;
            }
            if !(rest.starts_with('=') || rest.starts_with('-')) || rest.len() == 1 {
                say(format!("unknown -Wfatal-errors warning specifier: '{o}'"));
                continue;
            }
            let name = &rest[1..];
            if !known(name) {
                say(format!("unknown warning option '-Wfatal-errors={name}'"));
            }
            continue;
        }
        if !known(opt) {
            say(format!(
                "unknown warning option '{}{opt}'",
                if positive { "-W" } else { "-Wno-" }
            ));
        }
    }
    out
}

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

/// Whether `a` is an option this module takes: `-w`, or `-W` and a name,
/// none (`-W`) too, as clang's `-cc1` takes it (not `-Wl,`, `-Wp,` or
/// `-Wa,`, the driver's, which hand arguments on: no warning's name has a
/// comma).
pub fn is_option(a: &str) -> bool {
    a == "-w" || (a.starts_with("-W") && !a.contains(','))
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
        // clang's other spellings: -Wformat=0 is -Wno-format, and
        // -Werror-implicit-function-declaration GCC's -Werror=...
        let w = match w {
            "format=0" => "no-format",
            "error-implicit-function-declaration" => "error=implicit-function-declaration",
            "no-error-implicit-function-declaration" => "no-error=implicit-function-declaration",
            w => w,
        };
        match w {
            // `-Werror=` with no name is `-Werror`, as in clang.
            "error" | "error=" => werror = true,
            "no-error" | "no-error=" => werror = false,
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
        assert!(!is_option("-Wp,-DX") && !is_option("-Wl,-S") && is_option("-W"));
        assert!(table().iter().any(|(f, _)| *f == "unused-command-line-argument"));
        assert_eq!(level(&opts(&["-Werror="]), r), Level::Error);
        assert_eq!(level(&opts(&["-Werror", "-Wno-error="]), r), Level::Warning);
    }

    #[test]
    fn unknown_options_are_clangs() {
        let said =
            |o: &[&str]| -> Vec<String> { option_warnings(&opts(o)).into_iter().map(|e| e.msg).collect() };
        assert!(said(&[
            "-Wall",
            "-Wno-CL4",
            "-W",
            "-Wno-",
            "-Werror=",
            "-Werror=return-type"
        ])
        .is_empty());
        assert!(said(&[
            "-Weverything",
            "-Wno-everything",
            "-Wsystem-headers",
            "-Wformat=0",
            "-w"
        ])
        .is_empty());
        assert!(said(&[
            "-Wfatal-errors",
            "-Werror-implicit-function-declaration",
            "-W#warnings"
        ])
        .is_empty());
        assert_eq!(
            said(&["-Wzz", "-Wno-zz", "-Werror=zz", "-Wno-error=zz", "-Wcl4"]),
            [
                "unknown warning option '-Wzz'",
                "unknown warning option '-Wno-zz'",
                "unknown warning option '-Werror=zz'",
                "unknown warning option '-Werror=zz'",
                "unknown warning option '-Wcl4'",
            ]
        );
        assert_eq!(
            said(&["-Werror=everything", "-Wno-error=everything"]),
            [
                "unknown warning option '-Werror=everything'",
                "unknown warning option '-Werror=everything'"
            ]
        );
        assert_eq!(
            said(&[
                "-Werrorzz",
                "-Wno-error-zz",
                "-Werror-",
                "-Wfatal-errorsx",
                "-Wfatal-errors=",
                "-Wfatal-errors-zz"
            ]),
            [
                "unknown -Werror warning specifier: '-Werrorzz'",
                "unknown -Werror warning specifier: '-Wno-error-zz'",
                "unknown -Werror warning specifier: '-Werror-'",
                "unknown -Wfatal-errors warning specifier: '-Wfatal-errorsx'",
                "unknown -Wfatal-errors warning specifier: '-Wfatal-errors='",
                "unknown warning option '-Wfatal-errors=zz'",
            ]
        );
        let w = option_warnings(&opts(&["-Wzz"]));
        assert_eq!(w[0].flag, Some("unknown-warning-option"));
        assert_eq!(
            level(&opts(&["-Wzz", "-Wno-unknown-warning-option"]), w[0].flag),
            Level::Ignored
        );
    }
}
