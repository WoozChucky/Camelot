//! Register names.

/// ABI names, indexed by register number.
pub const ABI_NAMES: [&str; 32] = [
    "zero", "ra", "sp", "gp", "tp", "t0", "t1", "t2", "s0", "s1", "a0", "a1", "a2", "a3", "a4", "a5", "a6",
    "a7", "s2", "s3", "s4", "s5", "s6", "s7", "s8", "s9", "s10", "s11", "t3", "t4", "t5", "t6",
];

pub fn abi_name(r: u8) -> &'static str {
    ABI_NAMES[r as usize & 31]
}

/// Parses `x0`..`x31`, ABI names, and `fp` (alias for s0).
pub fn parse(name: &str) -> Option<u8> {
    if let Some(n) = name.strip_prefix('x') {
        if let Ok(n) = n.parse::<u8>() {
            return (n < 32).then_some(n);
        }
    }
    if name == "fp" {
        return Some(8);
    }
    ABI_NAMES.iter().position(|&n| n == name).map(|i| i as u8)
}
