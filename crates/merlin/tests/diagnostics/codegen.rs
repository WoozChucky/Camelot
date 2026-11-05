//! The code generator's diagnostics: merlin's own, for what clang takes.

use super::{case, Case};

#[rustfmt::skip]
pub fn cases() -> Vec<Case> {
    // A register's name as a symbol's: merlin-as reads `call sp` as a
    // register, where clang, which assembles what it compiles, has no text
    // to read.
    let register = |src: &'static str, name: &str| {
        let line = format!("1:5: error: '{name}' is a register name, which cannot be used as a symbol name");
        case(src, &[line.as_str()])
            .merlin("a register's name as a symbol's")
    };
    vec![
        // A frame past what an `addi` and `lui` reach (2 GiB).
        case("int main() { char a[2147483640]; char b[100]; return 0; }", &["1:5: error: the locals of 'main' need a stack frame larger than 2 GiB"]).merlin("a frame over 2 GiB"),
        register("int sp() { return 0; } int main() { return 0; }", "sp"),
        register("int sp; int main() { return 0; }", "sp"),
        register("int a0() { return 0; } int main() { return 0; }", "a0"),
        register("int a0; int main() { return 0; }", "a0"),
        register("int x5() { return 0; } int main() { return 0; }", "x5"),
        register("int x5; int main() { return 0; }", "x5"),
        register("int fp() { return 0; } int main() { return 0; }", "fp"),
        register("int fp; int main() { return 0; }", "fp"),
        register("int zero() { return 0; } int main() { return 0; }", "zero"),
        register("int zero; int main() { return 0; }", "zero"),
    ]
}
