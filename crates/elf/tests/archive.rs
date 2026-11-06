//! The archive writer against llvm-ar: tests/archive holds objects clang
//! made, a text member, and the archives llvm-ar made of them
//! (scripts/gen-archive-golden.sh). Written again from the same members,
//! each is the same bytes; read back, each gives its members and the
//! symbol table llvm-nm --print-armap prints.

use elf::archive::{self, NewMember};
use std::path::{Path, PathBuf};

fn dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/archive")
}

fn read(name: &str) -> Vec<u8> {
    std::fs::read(dir().join(name)).unwrap_or_else(|e| panic!("{name}: {e}"))
}

/// The archive llvm-ar made of `files` (paths under tests/archive), and
/// the elf crate's of the same: they must be equal.
fn check(golden: &str, files: &[&str]) {
    check_with(golden, files, true)
}

/// [`check`], the archive written with a symbol table if `symbol_table`.
fn check_with(golden: &str, files: &[&str], symbol_table: bool) {
    let data: Vec<Vec<u8>> = files.iter().map(|f| read(f)).collect();
    let members: Vec<NewMember> = files
        .iter()
        .zip(&data)
        .map(|(f, d)| NewMember {
            name: archive::member_name_of(f),
            data: d,
        })
        .collect();
    let want = read(golden);
    let got = archive::write_with(&members, symbol_table).unwrap();
    assert!(
        got == want,
        "{golden}: {} bytes, llvm-ar's {} (first difference at {:?})",
        got.len(),
        want.len(),
        got.iter().zip(&want).position(|(a, b)| a != b)
    );
    let parsed = archive::parse(&want).unwrap();
    let names: Vec<&str> = parsed.members.iter().map(|m| m.name.as_str()).collect();
    let want_names: Vec<&str> = members.iter().map(|m| m.name).collect();
    assert_eq!(names, want_names, "{golden}");
    for (m, d) in parsed.members.iter().zip(&data) {
        assert_eq!(m.data, &d[..], "{golden}: {}", m.name);
    }
}

#[test]
fn the_writer_writes_what_llvm_ar_writes() {
    check(
        "objs.a",
        &["a.o", "nosyms.o", "b.o", "a_very_long_member_name.o", "notes.txt"],
    );
    check("nosyms.a", &["nosyms.o"]);
    check("short.a", &["a.o", "nosyms.o", "b.o", "c.o", "notes.txt"]);
    check(
        "longnames.a",
        &[
            "abcdefghijklm.o",
            "abcdefghijklmn.o",
            "a_very_long_member_name.o",
            "a_very_long_member_name.o",
        ],
    );
    check("empty.a", &[]);
    check("text.a", &["notes.txt", "more.txt"]);
    assert!(!archive::parse(&read("text.a")).unwrap().has_symbol_table);
    let no_table = read("nosymtab.a");
    assert!(!archive::parse(&no_table).unwrap().has_symbol_table);
    check_with(
        "nosymtab.a",
        &[
            "abcdefghijklm.o",
            "abcdefghijklmn.o",
            "a_very_long_member_name.o",
            "a_very_long_member_name.o",
        ],
        false,
    );
}

#[test]
fn the_symbol_table_is_llvm_ars() {
    let objs = read("objs.a");
    let a = archive::parse(&objs).unwrap();
    assert!(a.has_symbol_table);
    let got: Vec<(&str, &str)> = a
        .symbols
        .iter()
        .map(|(s, m)| (s.as_str(), a.members[*m].name.as_str()))
        .collect();
    // llvm-nm --print-armap objs.a
    assert_eq!(
        got,
        [
            ("alpha", "a.o"),
            ("beta", "a.o"),
            ("gamma", "a.o"),
            ("common_thing", "a.o"),
            ("absolute_sym", "a.o"),
            ("hid", "a.o"),
            ("delta", "b.o"),
            ("delta", "a_very_long_member_name.o"),
        ]
    );
    assert_eq!(archive::symbols(&read("nosyms.o")).unwrap(), Vec::<String>::new());
    assert_eq!(
        archive::symbols(&read("notes.txt")).unwrap(),
        Vec::<String>::new()
    );
    assert!(archive::parse(&read("nosyms.a")).unwrap().symbols.is_empty());
}

#[test]
fn replacing_members_is_llvm_ars_r() {
    let objs = read("objs.a");
    let old = archive::parse(&objs).unwrap();
    let old: Vec<NewMember> = old
        .members
        .iter()
        .map(|m| NewMember {
            name: &m.name,
            data: m.data,
        })
        .collect();
    let (new_a, c) = (read("new/a.o"), read("c.o"));
    let files = [
        NewMember {
            name: "a.o",
            data: &new_a,
        },
        NewMember {
            name: "c.o",
            data: &c,
        },
    ];
    let members = archive::replace(&old, &files);
    assert_eq!(archive::write(&members).unwrap(), read("replaced.a"));
}
