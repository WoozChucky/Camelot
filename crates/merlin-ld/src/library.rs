//! Libraries: objects that are linked only when a program needs them, the
//! members of an archive (`libc.a`, given by name or found with `-l`) and
//! the objects between `--start-lib` and `--end-lib`, as ld.lld links both.
//!
//! A program's own objects are always linked. A library member is linked
//! (*extracted*) only if it defines a global symbol that is still
//! undefined: one that the objects linked so far refer to and none of them
//! defines. Extracting a member can leave new symbols undefined (the object
//! with `printf` calls `write`, which another member may define), and those
//! extract more. A program that never calls `malloc` gets neither its code
//! nor its static data.
//!
//! ## ld.lld's order
//!
//! Which members are linked, and in what order they follow the program's
//! objects, is what ld.lld does ([`order`]), so that a link here and
//! ld.lld's of the same files give the same bytes. ld.lld reads the files
//! in command-line order, keeping one symbol table:
//!
//! - An object that is linked goes at the end of the list of linked files,
//!   then its global symbols are resolved: the ones it defines first, then
//!   the ones it refers to, in its symbol table's order. A reference to a
//!   symbol that a library member read earlier defines extracts that member
//!   at once (it goes on the list, and its own symbols are resolved, which
//!   may extract more, depth first) before the next reference is looked at.
//! - A library member is only noted: each global it defines becomes lazy,
//!   "defined by this member if anyone needs it", unless something already
//!   defines it (or an earlier member does lazily). If the program already
//!   refers to one of them, the member is extracted there and then.
//! - After the last file, a lazy entry symbol (`-e`, `ENTRY(...)`, or
//!   `_start`) extracts its member: "if an entry symbol is in a static
//!   archive, pull out that file now".
//!
//! So `crt0.o prog.o -lc` links crt0.o, prog.o, then each member of libc.a
//! as it is found needed: in archive order for what the program calls, and
//! a member that a member needs right after the one that needs it, if it
//! comes earlier in the archive (`printf.o`, then `strlen.o`). Unlike GNU
//! ld, a library also resolves references from files after it on the
//! command line (`-lc prog.o` works), as ld.lld's do: the order of a
//! library among the objects only changes the order of the output.
//!
//! Two more details follow ld:
//!
//! - A weak undefined reference extracts nothing: it is 0 unless something
//!   linked anyway defines it (a strong reference later still extracts).
//! - Only what members define counts, not what they refer to: a member that
//!   nobody needs is left out even if it would resolve itself.
//!
//! Every member must still be an object merlin-ld links, needed or not, as
//! ld.lld reads its lazy objects: an ELF32 (RV32) member is an error
//! ([`object::parse`]). A member of an archive that is not an ELF file (a
//! text file) is left out with a warning, as ld.lld leaves it.

use crate::object::{self, Object};
use crate::{Error, Input};
use std::collections::HashMap;

/// The global symbols `obj` defines.
fn defines(obj: &Object) -> impl Iterator<Item = &str> {
    obj.symbols
        .iter()
        .skip(1)
        .filter(|s| !s.is_local() && !s.is_undefined() && !s.name.is_empty())
        .map(|s| s.name.as_str())
}

/// The global symbols `obj` refers to without defining them, and whether
/// each reference is weak.
fn refers(obj: &Object) -> impl Iterator<Item = (&str, bool)> {
    obj.symbols
        .iter()
        .skip(1)
        .filter(|s| !s.is_local() && s.is_undefined() && !s.name.is_empty())
        .map(|s| (s.name.as_str(), s.is_weak()))
}

/// What a name is in ld.lld's symbol table while the files are read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Sym {
    /// Referred to, by nothing but weak references if `weak`.
    Undefined { weak: bool },
    /// Defined by a linked file.
    Defined,
    /// Defined by library member `file`, not linked (yet); `weak` if only
    /// weak references want it, which extract nothing.
    Lazy { file: usize, weak: bool },
}

/// The resolution: the files, which are lazy, the symbol table, and the
/// linked files in their order.
struct Resolver<'a> {
    files: &'a [&'a Object],
    lazy: Vec<bool>,
    symbols: HashMap<&'a str, Sym>,
    linked: Vec<usize>,
}

impl<'a> Resolver<'a> {
    /// Links file `i`: on the list, then its definitions, then its
    /// references, each of which may extract a member.
    fn link(&mut self, i: usize) {
        self.lazy[i] = false;
        self.linked.push(i);
        let obj = self.files[i];
        for name in defines(obj) {
            self.symbols.insert(name, Sym::Defined);
        }
        for (name, weak) in refers(obj) {
            match self.symbols.get(name).copied() {
                None => {
                    self.symbols.insert(name, Sym::Undefined { weak });
                }
                Some(Sym::Undefined { weak: true }) if !weak => {
                    self.symbols.insert(name, Sym::Undefined { weak: false });
                }
                Some(Sym::Lazy { file, .. }) if weak => {
                    self.symbols.insert(name, Sym::Lazy { file, weak: true });
                }
                Some(Sym::Lazy { file, .. }) => self.extract(file),
                Some(Sym::Undefined { .. } | Sym::Defined) => {}
            }
        }
    }

    /// Notes library member `i`'s definitions as lazy, or extracts it if
    /// the program already needs one of them.
    fn note(&mut self, i: usize) {
        for name in defines(self.files[i]) {
            match self.symbols.get(name).copied() {
                None => {
                    self.symbols.insert(name, Sym::Lazy { file: i, weak: false });
                }
                Some(Sym::Undefined { weak: true }) => {
                    self.symbols.insert(name, Sym::Lazy { file: i, weak: true });
                }
                Some(Sym::Undefined { weak: false }) => {
                    self.extract(i);
                    return;
                }
                Some(Sym::Defined | Sym::Lazy { .. }) => {}
            }
        }
    }

    fn extract(&mut self, i: usize) {
        if self.lazy[i] {
            self.link(i);
        }
    }
}

/// The files a link of `files` links, in ld.lld's order ([the module's
/// description](self)): indexes into `files`, which are in command-line
/// order, `lazy` saying which are library members. `entry` is the entry
/// symbol's name, if the entry is not an address.
pub(crate) fn order(files: &[&Object], lazy: &[bool], entry: Option<&str>) -> Vec<usize> {
    let mut r = Resolver {
        files,
        lazy: lazy.to_vec(),
        symbols: HashMap::new(),
        linked: Vec::new(),
    };
    for (i, &l) in lazy.iter().enumerate() {
        if l {
            r.note(i);
        } else {
            r.link(i);
        }
    }
    if let Some(Sym::Lazy { file, .. }) = entry.and_then(|e| r.symbols.get(e).copied()) {
        r.extract(file);
    }
    r.linked
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

/// Which members of `library` linking `inputs` and then `library` needs,
/// with `entry` the entry symbol: their indexes, in the order the link puts
/// them after `inputs` ([`order`]). The files are checked as
/// [`crate::link_with_library`] checks them: the objects, then the members.
pub fn needed(inputs: &[Input], library: &[Input], entry: Option<&str>) -> Result<Vec<usize>, Vec<Error>> {
    let objects = parse_all(inputs)?;
    let members = parse_all(library)?;
    let all: Vec<&Object> = objects.iter().chain(&members).collect();
    let lazy: Vec<bool> = (0..all.len()).map(|i| i >= objects.len()).collect();
    Ok(order(&all, &lazy, entry)
        .into_iter()
        .filter_map(|i| i.checked_sub(objects.len()))
        .collect())
}

/// `inputs` followed by the members of `library` they need ([`needed`]),
/// in the order a link puts them: what to hand the linker.
pub fn with_needed(inputs: &[Input], library: &[Input]) -> Result<Vec<Input>, Vec<Error>> {
    let needed = needed(inputs, library, None)?;
    let mut all = inputs.to_vec();
    all.extend(needed.into_iter().map(|i| library[i].clone()));
    Ok(all)
}

/// A command line's files, its archives replaced by their members
/// ([`expand_archives`]).
#[derive(Debug)]
pub struct Expanded {
    /// Each file, and whether it is a library member.
    pub files: Vec<(Input, bool)>,
    /// One for each archive member that is not an object.
    pub warnings: Vec<String>,
}

/// The files of a command line with each archive replaced by its members,
/// all of them library members, named `lib.a(member.o)` as ld.lld names
/// them; and a warning for each member that is not an ELF file, which is
/// left out. An archive that does not parse is an error.
pub fn expand_archives(files: &[(&Input, bool)]) -> Result<Expanded, Vec<Error>> {
    let mut out = Vec::new();
    let mut warnings = Vec::new();
    let mut errors = Vec::new();
    for &(input, lazy) in files {
        if !elf::archive::is_archive(&input.data) {
            out.push((input.clone(), lazy));
            continue;
        }
        let archive = match elf::archive::parse(&input.data) {
            Ok(a) => a,
            Err(e) => {
                errors.push(Error(format!("{}: {}", input.name, e.0)));
                continue;
            }
        };
        for m in &archive.members {
            if !m.data.starts_with(b"\x7fELF") {
                warnings.push(format!(
                    "{}: archive member '{}' is neither ET_REL nor LLVM bitcode",
                    input.name, m.name
                ));
                continue;
            }
            let member = Input {
                name: format!("{}({})", input.name, m.name),
                data: m.data.to_vec(),
            };
            out.push((member, true));
        }
    }
    if errors.is_empty() {
        Ok(Expanded { files: out, warnings })
    } else {
        Err(errors)
    }
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
            let entsize = (symtab.entsize as usize).max(parsed.class.sym_size());
            // st_info is byte 4 of an ELF64 entry, after st_name and before
            // the 8-byte st_value and st_size.
            symtab.offset as usize + i * entsize + 4
        };
        elf[info_at] = (elf::STB_WEAK << 4) | (elf[info_at] & 0xf);
    }

    /// An object from merlin-as: `defs` defined as globals in `.text`, `refs`
    /// referred to from `.data`, `weak` as weak references.
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
    fn the_test_objects_define_and_refer() {
        let o = object::parse("a.o", &obj("a.o", &["f"], &["g"], &["h"]).data).unwrap();
        assert_eq!(defines(&o).collect::<Vec<_>>(), ["f"]);
        assert_eq!(refers(&o).collect::<Vec<_>>(), [("g", false), ("h", true)]);
    }

    #[test]
    fn pulls_in_only_what_is_needed_depth_first() {
        let program = [obj("main.o", &["main"], &["printf", "exit"], &[])];
        let library = [
            obj("malloc.o", &["malloc", "free"], &["sbrk"], &[]),
            obj("string.o", &["strlen", "memcpy"], &[], &[]),
            obj("printf.o", &["printf", "fprintf"], &["write", "strlen"], &[]),
            obj("usys.o", &["write", "exit", "sbrk"], &[], &[]),
        ];
        // printf (from main) is extracted when it is read, and its strlen,
        // already lazy in string.o, at once after it; usys.o when it is read
        // (write and exit). Nobody needs malloc, though usys.o brings sbrk.
        assert_eq!(needed(&program, &library, None).unwrap(), [2, 1, 3]);
        // A program that needs nothing gets nothing.
        let alone = [obj("main.o", &["main"], &[], &[])];
        assert_eq!(needed(&alone, &library, None).unwrap(), Vec::<usize>::new());
    }

    #[test]
    fn a_member_found_late_still_resolves_earlier_members() {
        // b.o needs a.o, which comes before it: extracted right after b.o.
        let program = [obj("main.o", &["main"], &["b"], &[])];
        let library = [obj("a.o", &["a"], &[], &[]), obj("b.o", &["b"], &["a"], &[])];
        assert_eq!(needed(&program, &library, None).unwrap(), [1, 0]);
    }

    #[test]
    fn a_library_before_the_program_resolves_it_too() {
        let a = object::parse("a.o", &obj("a.o", &["a"], &["b"], &[]).data).unwrap();
        let b = object::parse("b.o", &obj("b.o", &["b"], &[], &[]).data).unwrap();
        let main = object::parse("main.o", &obj("main.o", &["main"], &["a"], &[]).data).unwrap();
        let unused = object::parse("u.o", &obj("u.o", &["u"], &[], &[]).data).unwrap();
        // `--start-lib b.o u.o --end-lib main.o --start-lib a.o --end-lib`:
        // main.o extracts nothing when it is read (a is not lazy yet), a.o
        // is extracted when it is read, and b.o, lazy since the start, by
        // a.o's reference.
        let files = [&b, &unused, &main, &a];
        assert_eq!(order(&files, &[true, true, false, true], None), [2, 3, 0]);
    }

    #[test]
    fn defined_symbols_and_weak_references_pull_nothing_in() {
        // The program defines strlen itself and only weakly wants malloc.
        let program = [obj("main.o", &["main", "strlen"], &["strlen"], &["malloc"])];
        let library = [
            obj("string.o", &["strlen"], &[], &[]),
            obj("malloc.o", &["malloc"], &[], &[]),
        ];
        assert_eq!(needed(&program, &library, None).unwrap(), Vec::<usize>::new());
        // A strong reference after the weak one extracts it.
        let more = [program[0].clone(), obj("more.o", &[], &["malloc"], &[])];
        assert_eq!(needed(&more, &library, None).unwrap(), [1]);
    }

    #[test]
    fn a_lazy_entry_symbol_extracts_its_member() {
        let program = [obj("main.o", &["main"], &[], &[])];
        let library = [
            obj("crt.o", &["_start"], &["main"], &[]),
            obj("other.o", &["other"], &[], &[]),
        ];
        assert_eq!(needed(&program, &library, None).unwrap(), Vec::<usize>::new());
        assert_eq!(needed(&program, &library, Some("_start")).unwrap(), [0]);
        assert_eq!(
            needed(&program, &library, Some("nowhere")).unwrap(),
            Vec::<usize>::new()
        );
    }

    #[test]
    fn bad_objects_are_errors_named_by_file() {
        let program = [obj("main.o", &["main"], &[], &[])];
        let bad = [Input {
            name: "junk.o".into(),
            data: b"not an object".to_vec(),
        }];
        let errs = needed(&program, &bad, None).unwrap_err();
        assert_eq!(errs.len(), 1);
        assert!(errs[0].0.starts_with("junk.o: "), "{:?}", errs);
    }

    fn messages(r: Result<Vec<usize>, Vec<Error>>) -> Vec<String> {
        r.unwrap_err().into_iter().map(|e| e.0).collect()
    }

    /// clang's RV32 object of crates/elf/tests/llvm: an ELF32 file.
    fn rv32_object(name: &str) -> Input {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../elf/tests/llvm/rv32/prog.o");
        Input {
            name: name.into(),
            data: std::fs::read(path).unwrap(),
        }
    }

    #[test]
    fn every_member_must_be_an_elf64_object() {
        let program = [
            obj("main.o", &["main"], &["f"], &[]),
            obj("more.o", &[], &[], &[]),
        ];
        // ELF32 members are errors even when nothing needs them, as ld.lld
        // reads its lazy objects: all of them, in order.
        let library = [
            rv32_object("a32.o"),
            obj("f.o", &["f"], &[], &[]),
            rv32_object("b32.o"),
        ];
        let elf32 = "an ELF32 (RV32) object: merlin-ld links RV64 (ELF64) objects only";
        assert_eq!(
            messages(needed(&program, &library, None)),
            [format!("a32.o: {elf32}"), format!("b32.o: {elf32}")]
        );
        // A program object too.
        let mixed = [program[0].clone(), rv32_object("c32.o")];
        assert_eq!(
            messages(needed(&mixed, &library, None)),
            [format!("c32.o: {elf32}")]
        );
    }

    #[test]
    fn archives_become_their_members() {
        let f = obj("f.o", &["f"], &[], &[]);
        let g = obj("g.o", &["g"], &[], &[]);
        let members = [
            elf::archive::NewMember {
                name: "f.o",
                data: &f.data,
            },
            elf::archive::NewMember {
                name: "notes.txt",
                data: b"text",
            },
            elf::archive::NewMember {
                name: "g.o",
                data: &g.data,
            },
        ];
        let lib = Input {
            name: "/usr/lib/libx.a".into(),
            data: elf::archive::write(&members).unwrap(),
        };
        let main = obj("main.o", &["main"], &["g"], &[]);
        let Expanded { files, warnings } = expand_archives(&[(&main, false), (&lib, false)]).unwrap();
        let names: Vec<(&str, bool)> = files.iter().map(|(i, l)| (i.name.as_str(), *l)).collect();
        assert_eq!(
            names,
            [
                ("main.o", false),
                ("/usr/lib/libx.a(f.o)", true),
                ("/usr/lib/libx.a(g.o)", true)
            ]
        );
        assert_eq!(files[2].0.data, g.data);
        assert_eq!(
            warnings,
            ["/usr/lib/libx.a: archive member 'notes.txt' is neither ET_REL nor LLVM bitcode"]
        );
        let mut bad = lib.clone();
        bad.data.truncate(70);
        let errs = expand_archives(&[(&bad, false)]).unwrap_err();
        assert!(errs[0].0.starts_with("/usr/lib/libx.a: "), "{errs:?}");
    }
}
