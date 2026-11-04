//! What a section is in ELF (its type, flags and entry size), and how
//! `.section` decides it: LLVM's rules (`ELFAsmParser::parseSectionArguments`
//! and the sections `MCObjectFileInfo` makes before the first line), so that
//! merlin-as writes the section headers clang writes.
//!
//! ```text
//! .section name[, "flags"[, @type[, entsize]]]
//! ```
//!
//! * **The name's defaults.** A name LLVM knows by its prefix gets flags and
//!   a type without being told ([`name_defaults`]): `.text` and `.text.*`
//!   are `AX`, `.rodata*` `A`, `.data*` and `.bss*` `WA` (`.bss*` `NOBITS`),
//!   `.note*` is `NOTE`, `.init_array` `INIT_ARRAY`, and so on. Any other
//!   name (`.mystuff`, `.sdata`) has no flags at all: not even loaded.
//! * **The flags string** adds to the defaults, one letter a flag
//!   ([`FLAG_LETTERS`]: `a` alloc, `w` write, `x` exec, `M` merge, `S`
//!   strings, `T` TLS, `e` exclude, `R` retain), or is a number taken as it
//!   is (`"0x2"`). `.section .text.f,"aw"` is `WAX`.
//! * **The type**, `@progbits`, `%progbits` or `"progbits"` ([`type_named`]),
//!   replaces the name's. A mergeable section (`M`) must give one, and its
//!   entry size after it: `.section .rodata.str1.1,"aMS",@progbits,1`.
//! * **Sections LLVM made already** ([`PREDEFINED`]) keep their own
//!   attributes, whatever the name's defaults say: `.debug_str` and
//!   `.debug_line_str` are `MS` with entries of 1 byte, `.rodata.cst4` is
//!   `AM` with entries of 4, even when `.section` gives no flags.
//! * **Going back** to a section with `.section` and nothing after the name
//!   always works. With flags, a type or an entry size, they must be what
//!   the section has, or it is LLVM's "changed section flags for .foo,
//!   expected: 0x2" (also "type", "entsize").
//!
//! Section groups (`G`, COMDAT) and linked-to sections (`o`) need more
//! than a flag and are not supported; neither are `unique` sections or
//! Sun-style `#alloc` flags.

/// Section types (`sh_type`).
pub const SHT_PROGBITS: u32 = 1;
pub const SHT_NOTE: u32 = 7;
pub const SHT_NOBITS: u32 = 8;
pub const SHT_INIT_ARRAY: u32 = 14;
pub const SHT_FINI_ARRAY: u32 = 15;
pub const SHT_PREINIT_ARRAY: u32 = 16;
/// `.sframe`'s type (GNU's stack trace format).
pub const SHT_GNU_SFRAME: u32 = 0x6fff_fff4;
/// `.llvm.callgraph`'s type.
pub const SHT_LLVM_CALL_GRAPH: u32 = 0x6fff_4c0f;

/// Section flags (`sh_flags`).
pub const SHF_WRITE: u64 = 0x1;
pub const SHF_ALLOC: u64 = 0x2;
pub const SHF_EXECINSTR: u64 = 0x4;
pub const SHF_MERGE: u64 = 0x10;
pub const SHF_STRINGS: u64 = 0x20;
pub const SHF_LINK_ORDER: u64 = 0x80;
pub const SHF_GROUP: u64 = 0x200;
pub const SHF_TLS: u64 = 0x400;
pub const SHF_GNU_RETAIN: u64 = 0x20_0000;
pub const SHF_EXCLUDE: u64 = 0x8000_0000;

/// A section's ELF attributes: `sh_type`, `sh_flags`, `sh_entsize`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Attrs {
    pub ty: u32,
    pub flags: u64,
    pub entsize: u64,
}

/// The letters of a `.section` flags string and the flag each sets
/// (LLVM's `parseSectionFlags` for RISC-V). `?` ("the group of the
/// previous section") sets none: merlin-as has no groups, so it is always
/// the "no group" it means then. `G` and `o` are not here: they take more
/// arguments, which merlin-as does not support.
pub const FLAG_LETTERS: [(u8, u64); 9] = [
    (b'a', SHF_ALLOC),
    (b'e', SHF_EXCLUDE),
    (b'x', SHF_EXECINSTR),
    (b'w', SHF_WRITE),
    (b'M', SHF_MERGE),
    (b'S', SHF_STRINGS),
    (b'T', SHF_TLS),
    (b'R', SHF_GNU_RETAIN),
    (b'?', 0),
];

/// The type names `.section` takes (LLVM's, without its own `llvm_*` ones
/// and x86-64's `unwind`).
pub const TYPE_NAMES: [(&str, u32); 6] = [
    ("progbits", SHT_PROGBITS),
    ("nobits", SHT_NOBITS),
    ("note", SHT_NOTE),
    ("init_array", SHT_INIT_ARRAY),
    ("fini_array", SHT_FINI_ARRAY),
    ("preinit_array", SHT_PREINIT_ARRAY),
];

/// The sections LLVM's assembler has before it reads the first line
/// (`MCObjectFileInfo::initELFMCObjectFileInfo` for RISC-V), with their type,
/// flags and entry size. Most have the attributes their names would give
/// them anyway; those that do not are why this table exists: the string and
/// constant sections that are mergeable, `.debug_*.dwo`'s `SHF_EXCLUDE`,
/// and the special types.
pub const PREDEFINED: &[(&str, u32, u64, u64)] = &[
    (".bss", SHT_NOBITS, SHF_WRITE | SHF_ALLOC, 0),
    (".text", SHT_PROGBITS, SHF_EXECINSTR | SHF_ALLOC, 0),
    (".data", SHT_PROGBITS, SHF_WRITE | SHF_ALLOC, 0),
    (".rodata", SHT_PROGBITS, SHF_ALLOC, 0),
    (".tdata", SHT_PROGBITS, SHF_ALLOC | SHF_TLS | SHF_WRITE, 0),
    (".tbss", SHT_NOBITS, SHF_ALLOC | SHF_TLS | SHF_WRITE, 0),
    (".data.rel.ro", SHT_PROGBITS, SHF_ALLOC | SHF_WRITE, 0),
    (".rodata.cst4", SHT_PROGBITS, SHF_ALLOC | SHF_MERGE, 4),
    (".rodata.cst8", SHT_PROGBITS, SHF_ALLOC | SHF_MERGE, 8),
    (".rodata.cst16", SHT_PROGBITS, SHF_ALLOC | SHF_MERGE, 16),
    (".rodata.cst32", SHT_PROGBITS, SHF_ALLOC | SHF_MERGE, 32),
    (".gcc_except_table", SHT_PROGBITS, SHF_ALLOC, 0),
    // DWARF.
    (".debug_abbrev", SHT_PROGBITS, 0, 0),
    (".debug_info", SHT_PROGBITS, 0, 0),
    (".debug_line", SHT_PROGBITS, 0, 0),
    (".debug_line_str", SHT_PROGBITS, SHF_MERGE | SHF_STRINGS, 1),
    (".debug_frame", SHT_PROGBITS, 0, 0),
    (".debug_pubnames", SHT_PROGBITS, 0, 0),
    (".debug_pubtypes", SHT_PROGBITS, 0, 0),
    (".debug_gnu_pubnames", SHT_PROGBITS, 0, 0),
    (".debug_gnu_pubtypes", SHT_PROGBITS, 0, 0),
    (".debug_str", SHT_PROGBITS, SHF_MERGE | SHF_STRINGS, 1),
    (".debug_loc", SHT_PROGBITS, 0, 0),
    (".debug_aranges", SHT_PROGBITS, 0, 0),
    (".debug_ranges", SHT_PROGBITS, 0, 0),
    (".debug_macinfo", SHT_PROGBITS, 0, 0),
    (".debug_macro", SHT_PROGBITS, 0, 0),
    (".debug_names", SHT_PROGBITS, 0, 0),
    (".apple_names", SHT_PROGBITS, 0, 0),
    (".apple_objc", SHT_PROGBITS, 0, 0),
    (".apple_namespaces", SHT_PROGBITS, 0, 0),
    (".apple_types", SHT_PROGBITS, 0, 0),
    (".debug_str_offsets", SHT_PROGBITS, 0, 0),
    (".debug_addr", SHT_PROGBITS, 0, 0),
    (".debug_rnglists", SHT_PROGBITS, 0, 0),
    (".debug_loclists", SHT_PROGBITS, 0, 0),
    // DWARF of split debug information (`.dwo` files), never linked.
    (".debug_info.dwo", SHT_PROGBITS, SHF_EXCLUDE, 0),
    (".debug_types.dwo", SHT_PROGBITS, SHF_EXCLUDE, 0),
    (".debug_abbrev.dwo", SHT_PROGBITS, SHF_EXCLUDE, 0),
    (
        ".debug_str.dwo",
        SHT_PROGBITS,
        SHF_MERGE | SHF_STRINGS | SHF_EXCLUDE,
        1,
    ),
    (".debug_line.dwo", SHT_PROGBITS, SHF_EXCLUDE, 0),
    (".debug_loc.dwo", SHT_PROGBITS, SHF_EXCLUDE, 0),
    (".debug_str_offsets.dwo", SHT_PROGBITS, SHF_EXCLUDE, 0),
    (".debug_rnglists.dwo", SHT_PROGBITS, SHF_EXCLUDE, 0),
    (".debug_macinfo.dwo", SHT_PROGBITS, SHF_EXCLUDE, 0),
    (".debug_macro.dwo", SHT_PROGBITS, SHF_EXCLUDE, 0),
    (".debug_loclists.dwo", SHT_PROGBITS, SHF_EXCLUDE, 0),
    (".debug_cu_index", SHT_PROGBITS, 0, 0),
    (".debug_tu_index", SHT_PROGBITS, 0, 0),
    // LLVM's own, and unwinding.
    (".llvm_stackmaps", SHT_PROGBITS, SHF_ALLOC, 0),
    (".llvm_faultmaps", SHT_PROGBITS, SHF_ALLOC, 0),
    (".eh_frame", SHT_PROGBITS, SHF_ALLOC, 0),
    (".sframe", SHT_GNU_SFRAME, SHF_ALLOC, 0),
    (".llvm.callgraph", SHT_LLVM_CALL_GRAPH, 0, 0),
    (".stack_sizes", SHT_PROGBITS, 0, 0),
    (".pseudo_probe", SHT_PROGBITS, 0, 0),
    (".pseudo_probe_desc", SHT_PROGBITS, 0, 0),
    (".llvm_stats", SHT_PROGBITS, 0, 0),
];

/// The attributes of one of LLVM's [`PREDEFINED`] sections.
pub fn predefined(name: &str) -> Option<Attrs> {
    PREDEFINED
        .iter()
        .find(|&&(n, ..)| n == name)
        .map(|&(_, ty, flags, entsize)| Attrs { ty, flags, entsize })
}

/// LLVM's `hasPrefix`: `name` is `prefix` or starts with `prefix.` (so
/// `.text.f` has the prefix `.text`, and `.textual` does not).
fn has_prefix(name: &str, prefix: &str) -> bool {
    name.strip_prefix(prefix)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with('.'))
}

/// The type and flags LLVM gives a section from its name alone: what a
/// `.section` without flags (or type) gets, and what flags given add to.
pub fn name_defaults(name: &str) -> (u32, u64) {
    let flags = if has_prefix(name, ".rodata") || name == ".rodata1" {
        SHF_ALLOC
    } else if name == ".fini" || name == ".init" || has_prefix(name, ".text") {
        SHF_ALLOC | SHF_EXECINSTR
    } else if has_prefix(name, ".data")
        || name == ".data1"
        || has_prefix(name, ".bss")
        || has_prefix(name, ".init_array")
        || has_prefix(name, ".fini_array")
        || has_prefix(name, ".preinit_array")
    {
        SHF_ALLOC | SHF_WRITE
    } else if has_prefix(name, ".tdata") || has_prefix(name, ".tbss") {
        SHF_ALLOC | SHF_WRITE | SHF_TLS
    } else {
        0
    };
    // `.note` is a plain prefix here (`.notes` too), as in LLVM.
    let ty = if name.starts_with(".note") {
        SHT_NOTE
    } else if has_prefix(name, ".init_array") {
        SHT_INIT_ARRAY
    } else if has_prefix(name, ".bss") || has_prefix(name, ".tbss") {
        SHT_NOBITS
    } else if has_prefix(name, ".fini_array") {
        SHT_FINI_ARRAY
    } else if has_prefix(name, ".preinit_array") {
        SHT_PREINIT_ARRAY
    } else {
        SHT_PROGBITS
    };
    (ty, flags)
}

/// The attributes of a section named without a `.section` that says more:
/// LLVM's own if it is one of the [`PREDEFINED`], else its name's defaults.
/// This is what `.text`, `.data`, `.rodata` and `.bss` switch to, what a
/// `.lcomm` puts its symbol in and what the line table is written to.
pub fn by_name(name: &str) -> Attrs {
    predefined(name).unwrap_or_else(|| {
        let (ty, flags) = name_defaults(name);
        Attrs {
            ty,
            flags,
            entsize: 0,
        }
    })
}

/// A number written as LLVM's `StringRef::getAsInteger(0, ...)` reads it,
/// the whole string: `0x`/`0X` hexadecimal, `0b`/`0B` binary, `0o`/`0O` or a
/// leading `0` octal, decimal otherwise. None if it is not one or does not
/// fit in 32 bits (flags and types are 32-bit fields to LLVM's parser).
pub fn parse_number(s: &str) -> Option<u32> {
    let (digits, radix) = if let Some(d) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        (d, 16)
    } else if let Some(d) = s.strip_prefix("0b").or_else(|| s.strip_prefix("0B")) {
        (d, 2)
    } else if let Some(d) = s.strip_prefix("0o").or_else(|| s.strip_prefix("0O")) {
        (d, 8)
    } else if s.len() > 1 && s.starts_with('0') {
        (&s[1..], 8)
    } else {
        (s, 10)
    };
    // from_str_radix would also take a sign, which getAsInteger does not.
    if digits.is_empty() || !digits.bytes().all(|c| c.is_ascii_alphanumeric()) {
        return None;
    }
    u32::from_str_radix(digits, radix).ok()
}

/// The flags of a `.section` flags string: a number taken as it is, or
/// letters ([`FLAG_LETTERS`]). An unknown byte is the error, for the
/// message: `G` and `o` too, which merlin-as does not support.
pub fn parse_flags(s: &[u8]) -> Result<u64, u8> {
    if let Some(n) = std::str::from_utf8(s).ok().and_then(parse_number) {
        return Ok(n.into());
    }
    let mut flags = 0;
    for &c in s {
        match FLAG_LETTERS.iter().find(|&&(l, _)| l == c) {
            Some(&(_, f)) => flags |= f,
            None => return Err(c),
        }
    }
    Ok(flags)
}

/// The type a `.section` type name stands for: one of [`TYPE_NAMES`], or a
/// number (`@0x70000001`).
pub fn type_named(name: &str) -> Option<u32> {
    TYPE_NAMES
        .iter()
        .find(|&&(n, _)| n == name)
        .map(|&(_, t)| t)
        .or_else(|| parse_number(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_give_llvms_defaults() {
        // llvm-readelf -S of llvm-mc's objects for `.section NAME` alone.
        const WA: u64 = SHF_WRITE | SHF_ALLOC;
        const AX: u64 = SHF_ALLOC | SHF_EXECINSTR;
        for (name, ty, flags, entsize) in [
            (".foo", SHT_PROGBITS, 0, 0),
            (".text.noflags", SHT_PROGBITS, AX, 0),
            (".textual", SHT_PROGBITS, 0, 0),
            (".init", SHT_PROGBITS, AX, 0),
            (".fini", SHT_PROGBITS, AX, 0),
            (".rodata.str2", SHT_PROGBITS, SHF_ALLOC, 0),
            (".rodata.cst4", SHT_PROGBITS, SHF_ALLOC | SHF_MERGE, 4),
            (".rodata.cst16", SHT_PROGBITS, SHF_ALLOC | SHF_MERGE, 16),
            (".bss.x", SHT_NOBITS, WA, 0),
            (".data.x", SHT_PROGBITS, WA, 0),
            (".data.rel.ro", SHT_PROGBITS, WA, 0),
            (".tdata", SHT_PROGBITS, WA | SHF_TLS, 0),
            (".tbss", SHT_NOBITS, WA | SHF_TLS, 0),
            (".init_array", SHT_INIT_ARRAY, WA, 0),
            (".note.y", SHT_NOTE, 0, 0),
            (".sbss", SHT_PROGBITS, 0, 0),
            (".sdata", SHT_PROGBITS, 0, 0),
            (".srodata", SHT_PROGBITS, 0, 0),
            (".eh_frame", SHT_PROGBITS, SHF_ALLOC, 0),
            (".debug_info", SHT_PROGBITS, 0, 0),
            (".debug_str", SHT_PROGBITS, SHF_MERGE | SHF_STRINGS, 1),
            (".debug_line_str", SHT_PROGBITS, SHF_MERGE | SHF_STRINGS, 1),
            (".sframe", SHT_GNU_SFRAME, SHF_ALLOC, 0),
            (".llvm.callgraph", SHT_LLVM_CALL_GRAPH, 0, 0),
            (
                ".debug_str.dwo",
                SHT_PROGBITS,
                SHF_MERGE | SHF_STRINGS | SHF_EXCLUDE,
                1,
            ),
            (".debug_info.dwo", SHT_PROGBITS, SHF_EXCLUDE, 0),
        ] {
            assert_eq!(by_name(name), Attrs { ty, flags, entsize }, "{name}");
        }
    }

    #[test]
    fn flags_are_letters_or_a_number() {
        assert_eq!(parse_flags(b"aMS"), Ok(SHF_ALLOC | SHF_MERGE | SHF_STRINGS));
        assert_eq!(parse_flags(b"eR"), Ok(SHF_EXCLUDE | SHF_GNU_RETAIN));
        assert_eq!(parse_flags(b""), Ok(0));
        assert_eq!(parse_flags(b"a?"), Ok(SHF_ALLOC));
        assert_eq!(parse_flags(b"0x2"), Ok(SHF_ALLOC));
        assert_eq!(parse_flags(b"6"), Ok(SHF_ALLOC | SHF_EXECINSTR));
        assert_eq!(parse_flags(b"q"), Err(b'q'));
        assert_eq!(parse_flags(b"aG"), Err(b'G'));
        assert_eq!(parse_flags(b"o"), Err(b'o'));
        assert_eq!(parse_flags(b"a\xc3\xa9"), Err(0xc3));
    }

    #[test]
    fn numbers_read_as_llvm_reads_them() {
        for (s, n) in [
            ("0x30", 0x30),
            ("0X1f", 0x1f),
            ("0b101", 5),
            ("017", 15),
            ("0o17", 15),
            ("0", 0),
            ("42", 42),
        ] {
            assert_eq!(parse_number(s), Some(n), "{s}");
        }
        for s in ["", "0x", "08", "-1", "+1", "0x100000000", "1a", "a"] {
            assert_eq!(parse_number(s), None, "{s}");
        }
        assert_eq!(type_named("nobits"), Some(SHT_NOBITS));
        assert_eq!(type_named("0x70000001"), Some(0x7000_0001));
        assert_eq!(type_named("7"), Some(SHT_NOTE));
        assert_eq!(type_named("weird"), None);
    }
}
