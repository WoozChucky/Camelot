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
/// library order.
pub fn needed(inputs: &[Input], library: &[Input]) -> Result<Vec<usize>, Vec<Error>> {
    let objects = parse_all(inputs)?;
    let members = parse_all(library)?;
    let mut defined: BTreeSet<&str> = objects.iter().flat_map(defines).collect();
    let mut undefined: BTreeSet<&str> = objects
        .iter()
        .flat_map(needs)
        .filter(|n| !defined.contains(n))
        .collect();
    let mut pulled = vec![false; members.len()];
    loop {
        let mut progress = false;
        for (i, m) in members.iter().enumerate() {
            if pulled[i] || !defines(m).any(|n| undefined.contains(n)) {
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
    Ok((0..members.len()).filter(|&i| pulled[i]).collect())
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
            let entsize = (symtab.entsize as usize).max(elf::SYM_SIZE);
            // st_info is byte 12 of the entry.
            symtab.offset as usize + i * entsize + 12
        };
        elf[info_at] = (elf::STB_WEAK << 4) | (elf[info_at] & 0xf);
    }

    /// An object from merlin-as: `defs` defined as globals in `.text`,
    /// `refs` referred to from `.data`, `weak` as weak references.
    fn obj(name: &str, defs: &[&str], refs: &[&str], weak: &[&str]) -> Input {
        let mut src = String::from("    .text\n");
        for d in defs {
            src.push_str(&format!("    .globl {d}\n{d}:\n    nop\n"));
        }
        src.push_str("    .data\n");
        for r in refs.iter().chain(weak) {
            src.push_str(&format!("    .word {r}\n"));
        }
        let mut data = merlin_as::assemble_to_elf(&src).unwrap_or_else(|e| panic!("{e:?}\n{src}"));
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
}
