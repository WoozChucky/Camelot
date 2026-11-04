//! Libraries: objects that are linked only when a program needs them, the
//! way ld links the members of an archive (`libc.a`) and ld.lld the objects
//! between `--start-lib` and `--end-lib`.
//!
//! A program's own objects are always linked. A library member is linked
//! only if it defines a global symbol that is still undefined: one that the
//! objects linked so far refer to and none of them defines. Pulling a member
//! in can leave new symbols undefined (the object with `printf` calls
//! `write`, which another member may define), so the search goes round the
//! members again until a whole pass pulls in nothing more. A program that
//! never calls `malloc` gets neither its code nor its static data.
//!
//! Two details follow ld:
//!
//! - A weak undefined reference pulls nothing in: it is 0 unless something
//!   linked anyway defines it.
//! - Only what members define counts, not what they refer to: a member that
//!   nobody needs is left out even if it would resolve itself.
//!
//! The members that are pulled in are linked after the program's objects, in
//! library order, whatever order they were found in, so the output depends
//! only on which members are needed.
//!
//! Every member must still be of the executable's class, needed or not, as
//! ld.lld checks its lazy objects, in command-line order with the other
//! files ([`object::check_classes`]).

use crate::object::{self, Object};
use crate::{Error, Input};
use std::collections::BTreeSet;

/// The global symbols `obj` defines.
fn defines(obj: &Object) -> impl Iterator<Item = &str> {
    obj.symbols
        .iter()
        .skip(1)
        .filter(|s| !s.is_local() && !s.is_undefined() && !s.name.is_empty())
        .map(|s| s.name.as_str())
}

/// The global symbols `obj` refers to without defining them, weak ones left
/// out.
fn needs(obj: &Object) -> impl Iterator<Item = &str> {
    obj.symbols
        .iter()
        .skip(1)
        .filter(|s| !s.is_local() && s.is_undefined() && !s.is_weak() && !s.name.is_empty())
        .map(|s| s.name.as_str())
}

fn parse_all(inputs: &[Input]) -> Result<Vec<Object>, Vec<Error>> {
    let mut objects = Vec::new();
    let mut errors = Vec::new();
    for i in inputs {
        match object::parse(&i.name, &i.data) {
            Ok(o) => objects.push(o),
            Err(e) => errors.push(Error(format!("{}: {e}", i.name))),
        }
    }
    if errors.is_empty() {
        Ok(objects)
    } else {
        Err(errors)
    }
}

/// Which members of `library` linking `inputs` needs: their indexes, in
/// library order, for a link without a script (`cargo xtask kernel` links
/// each user program with the C library this way). The files are checked
/// as [`crate::link_with_library`] checks them: the objects, then the
/// members, each of the first object's class.
pub fn needed(inputs: &[Input], library: &[Input]) -> Result<Vec<usize>, Vec<Error>> {
    let objects = parse_all(inputs)?;
    let members = parse_all(library)?;
    if let Some(first) = objects.first() {
        let files = objects
            .iter()
            .map(|o| (o, false))
            .chain(members.iter().map(|m| (m, true)));
        let wrong = object::check_classes(first.class, None, files);
        if !wrong.is_empty() {
            return Err(wrong);
        }
    }
    let all: Vec<&Object> = objects.iter().chain(&members).collect();
    let lazy: Vec<bool> = (0..all.len()).map(|i| i >= objects.len()).collect();
    let pulled = pulled(&all, &lazy);
    Ok((0..members.len())
        .filter(|&i| pulled[objects.len() + i])
        .collect())
}

/// Which of a link's files are library members it pulls in: `files` in
/// command-line order, `lazy` saying which are members. The others are
/// always linked.
pub(crate) fn pulled(files: &[&Object], lazy: &[bool]) -> Vec<bool> {
    let program = files.iter().zip(lazy).filter(|&(_, &l)| !l).map(|(o, _)| *o);
    let mut defined: BTreeSet<&str> = program.clone().flat_map(defines).collect();
    let mut undefined: BTreeSet<&str> = program.flat_map(needs).filter(|n| !defined.contains(n)).collect();
    let mut pulled = vec![false; files.len()];
    loop {
        let mut progress = false;
        for (i, m) in files.iter().enumerate() {
            if !lazy[i] || pulled[i] || !defines(m).any(|n| undefined.contains(n)) {
                continue;
            }
            pulled[i] = true;
            progress = true;
            for n in defines(m) {
                defined.insert(n);
                undefined.remove(n);
            }
            for n in needs(m) {
                if !defined.contains(n) {
                    undefined.insert(n);
                }
            }
        }
        if !progress {
            break;
        }
    }
    pulled
}

/// `inputs` followed by the members of `library` they need ([`needed`]):
/// what to hand the linker.
pub fn with_needed(inputs: &[Input], library: &[Input]) -> Result<Vec<Input>, Vec<Error>> {
    let needed = needed(inputs, library)?;
    let mut all = inputs.to_vec();
    all.extend(needed.into_iter().map(|i| library[i].clone()));
    Ok(all)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::object::Class;

    /// Makes global symbol `name` of the object `elf` weak (merlin-as has no
    /// `.weak`): its binding in `.symtab` becomes `STB_WEAK`.
    fn make_weak(elf: &mut [u8], name: &str) {
        let info_at = {
            let parsed = elf::Elf::parse(elf).unwrap();
            let symtab = &parsed.sections[parsed.symtab_index().unwrap()];
            let syms = parsed.symbols_in(symtab).unwrap();
            let i = syms
                .iter()
                .position(|s| s.name == name)
                .unwrap_or_else(|| panic!("no symbol {name}"));
            let entsize = (symtab.entsize as usize).max(parsed.class.sym_size());
            // st_info is byte 12 of an ELF32 entry, after st_name, st_value
            // and st_size; ELF64 moves it up to byte 4, before the 8-byte
            // st_value and st_size.
            let st_info = match parsed.class {
                Class::Elf32 => 12,
                Class::Elf64 => 4,
            };
            symtab.offset as usize + i * entsize + st_info
        };
        elf[info_at] = (elf::STB_WEAK << 4) | (elf[info_at] & 0xf);
    }

    /// An RV32 object from merlin-as: `defs` defined as globals in `.text`,
    /// `refs` referred to from `.data`, `weak` as weak references.
    fn obj(name: &str, defs: &[&str], refs: &[&str], weak: &[&str]) -> Input {
        obj_of(Class::Elf32, name, defs, refs, weak)
    }

    /// [`obj`] of either class: an RV64 object for `Class::Elf64`.
    fn obj_of(class: Class, name: &str, defs: &[&str], refs: &[&str], weak: &[&str]) -> Input {
        let mut src = String::from("    .text\n");
        for d in defs {
            src.push_str(&format!("    .globl {d}\n{d}:\n    nop\n"));
        }
        src.push_str("    .data\n");
        for r in refs.iter().chain(weak) {
            src.push_str(&format!("    .word {r}\n"));
        }
        let xlen = match class {
            Class::Elf32 => excalibur::Xlen::Rv32,
            Class::Elf64 => excalibur::Xlen::Rv64,
        };
        let mut data = merlin_as::assemble_to_elf_with(&src, &merlin_as::Options { xlen })
            .unwrap_or_else(|e| panic!("{e:?}\n{src}"));
        for w in weak {
            make_weak(&mut data, w);
        }
        Input {
            name: name.into(),
            data,
        }
    }

    #[test]
    fn the_test_objects_define_and_need() {
        let o = object::parse("a.o", &obj("a.o", &["f"], &["g"], &["h"]).data).unwrap();
        assert_eq!(defines(&o).collect::<Vec<_>>(), ["f"]);
        assert_eq!(needs(&o).collect::<Vec<_>>(), ["g"]);
    }

    #[test]
    fn pulls_in_only_what_is_needed_transitively() {
        let program = [obj("main.o", &["main"], &["printf", "exit"], &[])];
        let library = [
            obj("malloc.o", &["malloc", "free"], &["sbrk"], &[]),
            obj("string.o", &["strlen", "memcpy"], &[], &[]),
            obj("printf.o", &["printf", "fprintf"], &["write", "strlen"], &[]),
            obj("usys.o", &["write", "exit", "sbrk"], &[], &[]),
        ];
        // printf (from main) needs write and strlen; usys also brings sbrk,
        // but nobody needs malloc.
        assert_eq!(needed(&program, &library).unwrap(), [1, 2, 3]);
        let linked = with_needed(&program, &library).unwrap();
        let names: Vec<&str> = linked.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, ["main.o", "string.o", "printf.o", "usys.o"]);
        // A program that needs nothing gets nothing.
        let alone = [obj("main.o", &["main"], &[], &[])];
        assert_eq!(needed(&alone, &library).unwrap(), Vec::<usize>::new());
    }

    #[test]
    fn a_member_found_late_still_resolves_earlier_members() {
        // b.o needs a.o, which comes before it: a second pass finds it.
        let program = [obj("main.o", &["main"], &["b"], &[])];
        let library = [obj("a.o", &["a"], &[], &[]), obj("b.o", &["b"], &["a"], &[])];
        assert_eq!(needed(&program, &library).unwrap(), [0, 1]);
    }

    #[test]
    fn defined_symbols_and_weak_references_pull_nothing_in() {
        // The program defines strlen itself and only weakly wants malloc.
        let program = [obj("main.o", &["main", "strlen"], &["strlen"], &["malloc"])];
        let library = [
            obj("string.o", &["strlen"], &[], &[]),
            obj("malloc.o", &["malloc"], &[], &[]),
        ];
        assert_eq!(needed(&program, &library).unwrap(), Vec::<usize>::new());
    }

    #[test]
    fn bad_objects_are_errors_named_by_file() {
        let program = [obj("main.o", &["main"], &[], &[])];
        let bad = [Input {
            name: "junk.o".into(),
            data: b"not an object".to_vec(),
        }];
        let errs = needed(&program, &bad).unwrap_err();
        assert_eq!(errs.len(), 1);
        assert!(errs[0].0.starts_with("junk.o: "), "{:?}", errs);
    }

    fn messages(r: Result<Vec<usize>, Vec<Error>>) -> Vec<String> {
        r.unwrap_err().into_iter().map(|e| e.0).collect()
    }

    #[test]
    fn rv64_members_are_pulled_in_as_rv32_ones() {
        let rv64 =
            |name, defs: &[&str], refs: &[&str], weak: &[&str]| obj_of(Class::Elf64, name, defs, refs, weak);
        let o = object::parse("a.o", &rv64("a.o", &["f"], &["g"], &["h"]).data).unwrap();
        assert_eq!(o.class, Class::Elf64);
        // make_weak found st_info in the 24-byte entry: h is not needed.
        assert_eq!(needs(&o).collect::<Vec<_>>(), ["g"]);
        let program = [rv64("main.o", &["main"], &["printf"], &["malloc"])];
        let library = [
            rv64("malloc.o", &["malloc"], &[], &[]),
            rv64("printf.o", &["printf"], &["write"], &[]),
            rv64("usys.o", &["write"], &[], &[]),
        ];
        assert_eq!(needed(&program, &library).unwrap(), [1, 2]);
    }

    #[test]
    fn every_member_must_have_the_programs_class() {
        let program = [
            obj_of(Class::Elf64, "main.o", &["main"], &["f"], &[]),
            obj_of(Class::Elf64, "more.o", &[], &[], &[]),
        ];
        // Members of the other class are errors even when nothing needs
        // them, as ld.lld checks its lazy objects: all of them, in order,
        // compared with the first program object.
        let library = [
            obj("a32.o", &["a"], &[], &[]),
            obj_of(Class::Elf64, "f.o", &["f"], &[], &[]),
            obj("b32.o", &["b"], &[], &[]),
        ];
        assert_eq!(
            messages(needed(&program, &library)),
            [
                "a32.o is incompatible with main.o",
                "b32.o is incompatible with main.o"
            ]
        );
        // A program object of the other class comes first, as it does on
        // ld.lld's command line.
        let mixed = [program[0].clone(), obj("c32.o", &[], &[], &[])];
        assert_eq!(
            messages(needed(&mixed, &library)),
            [
                "c32.o is incompatible with main.o",
                "a32.o is incompatible with main.o",
                "b32.o is incompatible with main.o"
            ]
        );
    }

    #[test]
    fn classes_are_checked_in_command_line_order() {
        let parse = |i: Input| object::parse(&i.name.clone(), &i.data).unwrap();
        let a64 = parse(obj_of(Class::Elf64, "a64.o", &[], &[], &[]));
        let b64 = parse(obj_of(Class::Elf64, "b64.o", &[], &[], &[]));
        let b32 = parse(obj("b32.o", &[], &[], &[]));
        let c32 = parse(obj("c32.o", &[], &[], &[]));
        let check = |class, format, files: &[(&Object, bool)]| -> Vec<String> {
            object::check_classes(class, format, files.iter().copied())
                .into_iter()
                .map(|e| e.0)
                .collect()
        };
        // `a64.o --start-lib b32.o --end-lib c32.o`: in that order, as ld.lld.
        assert_eq!(
            check(Class::Elf64, None, &[(&a64, false), (&b32, true), (&c32, false)]),
            [
                "b32.o is incompatible with a64.o",
                "c32.o is incompatible with a64.o"
            ]
        );
        // `--start-lib b32.o --end-lib a64.o b64.o`: the class is the library
        // member's, and before any other file has it ld.lld names nothing.
        assert_eq!(
            check(Class::Elf32, None, &[(&b32, true), (&a64, false), (&b64, false)]),
            ["a64.o is incompatible", "b64.o is incompatible"]
        );
        // `--start-lib b32.o --end-lib c32.o a64.o`: then c32.o has it.
        assert_eq!(
            check(Class::Elf32, None, &[(&b32, true), (&c32, false), (&a64, false)]),
            ["a64.o is incompatible with c32.o"]
        );
        // With an OUTPUT_FORMAT, every file is compared with it.
        assert_eq!(
            check(
                Class::Elf32,
                Some(Class::Elf32),
                &[(&a64, false), (&b32, false), (&b64, true)]
            ),
            [
                "a64.o is incompatible with elf32-littleriscv",
                "b64.o is incompatible with elf32-littleriscv"
            ]
        );
        assert!(check(Class::Elf64, Some(Class::Elf64), &[(&a64, false), (&b64, true)]).is_empty());
    }
}
