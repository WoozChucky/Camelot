//! A stand-in for GNU `as`, for clang's driver with `-fno-integrated-as`
//! in the ignored test that holds Camelot's `cc` to it
//! (`crates/xtask/tests/selfhost.rs`, `clangs_driver_says_what_cc_says`):
//! the shape `cc` follows, cc1 writing assembly and `as` assembling it, with
//! no GNU toolchain on the host. It takes what clang gives GNU `as`
//! (`-mabi ABI -march ISA [-I DIR] [-g] [-gdwarf-5] -o OUT IN`, and
//! `-march=ISA` from `-Wa,`), assembles IN
//! with merlin-as as `/bin/as` does, and says merlin-as's errors
//! (`IN:LINE: error: MESSAGE`) with exit status 1, as `/bin/as` does: so
//! clang's word on a failed assembly can be compared with cc's.

use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let mut output = None;
    let mut input = None;
    while let Some(a) = args.next() {
        match a.as_str() {
            // What clang passes on that merlin-as has no use for.
            "-mabi" | "-march" | "-I" => {
                args.next();
            }
            "-g" | "-gdwarf-5" => {}
            // `-Wa,-march=ISA`, which `/bin/as` takes: merlin-as's ISA is
            // the only one.
            _ if a.starts_with("-march=") => {}
            "-o" => output = args.next(),
            _ => input = Some(a),
        }
    }
    let (Some(output), Some(input)) = (output, input) else {
        eprintln!("gnu_as: usage: gnu_as [-mabi ABI] [-march ISA] -o OUT IN");
        return ExitCode::from(2);
    };
    let text = match std::fs::read_to_string(&input) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("gnu_as: cannot read {input}: {e}");
            return ExitCode::from(1);
        }
    };
    match xtask::toolchain::assemble(&input, &text) {
        Ok(object) => match std::fs::write(&output, object) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("gnu_as: cannot write {output}: {e}");
                ExitCode::from(1)
            }
        },
        Err(errors) => {
            eprintln!("{errors}");
            ExitCode::from(1)
        }
    }
}
