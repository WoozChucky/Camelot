//! The diagnostic cases of `codegen` (generated; curated by hand).

use super::{case, Case};

#[rustfmt::skip]
pub fn cases() -> Vec<Case> {
    vec![
        // frame_size_limit
        case("int main() { char a[2147483640]; char b[100]; return 0; }", &[]),
        // register_names_cannot_be_symbols
        case("int sp() { return 0; } int main() { return 0; }", &[]),
        case("int sp; int main() { return 0; }", &[]),
        case("int a0() { return 0; } int main() { return 0; }", &[]),
        case("int a0; int main() { return 0; }", &[]),
        case("int x5() { return 0; } int main() { return 0; }", &[]),
        case("int x5; int main() { return 0; }", &[]),
        case("int fp() { return 0; } int main() { return 0; }", &[]),
        case("int fp; int main() { return 0; }", &[]),
        case("int zero() { return 0; } int main() { return 0; }", &[]),
        case("int zero; int main() { return 0; }", &[]),
    ]
}
