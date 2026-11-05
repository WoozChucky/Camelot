//! merlin-as's debug information against LLVM's: every `.s` in tests/debug_line
//! is assembled by merlin-as and compared with the object clang made of it, the
//! `.o` next to it (regenerate with scripts/gen-debug-line-golden.sh). Every
//! `.debug_*` section must hold the same bytes, with the same relocations: a
//! relocation is compared by what it points at (a section and an offset in it),
//! since clang names labels where merlin-as uses section symbols. A line
//! table's addresses are 8 bytes, with R_RISCV_64.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const STT_SECTION: u8 = 3;

/// A relocation: offset, type, and the place it points at (section name, offset).
type Reloc = (u64, u32, String, i64);

/// The `.debug_*` sections of an object: contents and relocations, by name.
fn debug_sections(file: &[u8]) -> BTreeMap<String, (Vec<u8>, Vec<Reloc>)> {
    let elf = elf::Elf::parse(file).unwrap();
    let symbols = elf.symbols().unwrap();
    let mut out = BTreeMap::new();
    for s in &elf.sections {
        if s.name.starts_with(".debug_") {
            out.insert(
                s.name.clone(),
                (elf.section_data(s).unwrap().to_vec(), Vec::new()),
            );
        }
    }
    for rel in elf.sections.iter().filter(|s| s.ty == 4) {
        let target = &elf.sections[rel.info as usize];
        let Some((_, relocs)) = out.get_mut(&target.name) else {
            continue;
        };
        for r in elf.relocations(rel).unwrap() {
            let sym = &symbols[r.sym as usize];
            let (place, value) = match sym.section() {
                Some(sec) if sym.ty() == STT_SECTION => (elf.sections[sec].name.clone(), 0),
                Some(sec) => (elf.sections[sec].name.clone(), sym.value as i64),
                None => (sym.name.clone(), 0),
            };
            relocs.push((r.offset, r.ty, place, value + r.addend));
        }
        relocs.sort();
    }
    out
}

fn hex(b: &[u8]) -> String {
    b.chunks(16)
        .enumerate()
        .map(|(i, c)| {
            let bytes: Vec<String> = c.iter().map(|b| format!("{b:02x}")).collect();
            format!("{:04x}  {}", i * 16, bytes.join(" "))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/debug_line")
}

/// The source of `name`'s object.
fn source(name: &str) -> String {
    std::fs::read_to_string(dir().join(format!("{name}.s"))).unwrap()
}

fn assemble(name: &str) -> Result<Vec<u8>, String> {
    merlin_as::assemble_to_elf(&source(name)).map_err(|e| format!("{e:?}"))
}

/// Why `name`'s output differs from clang's, if it does.
fn check(name: &str) -> Result<(), String> {
    let ours = assemble(name)?;
    let theirs = std::fs::read(dir().join(format!("{name}.o"))).unwrap();
    let (ours, theirs) = (debug_sections(&ours), debug_sections(&theirs));
    let names = |m: &BTreeMap<String, _>| m.keys().cloned().collect::<Vec<_>>();
    if !theirs.contains_key(".debug_line") {
        return Err("clang made no .debug_line".into());
    }
    if names(&ours) != names(&theirs) {
        return Err(format!(
            "sections: {:?}, clang's {:?}",
            names(&ours),
            names(&theirs)
        ));
    }
    for (sec, (data, relocs)) in &ours {
        let (want, want_relocs) = &theirs[sec];
        if data != want {
            return Err(format!("{sec}:\n{}\nclang's:\n{}", hex(data), hex(want)));
        }
        if relocs != want_relocs {
            return Err(format!(
                "{sec} relocations:\n{relocs:?}\nclang's:\n{want_relocs:?}"
            ));
        }
    }
    Ok(())
}

/// The names of the files in `dir` with extension `ext`, sorted.
fn names(dir: &Path, ext: &str) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| {
            let name = e.unwrap().file_name().into_string().unwrap();
            name.strip_suffix(ext).map(String::from)
        })
        .collect();
    names.sort();
    names
}

#[test]
fn debug_sections_match_llvm() {
    // Every source has its object.
    assert_eq!(names(&dir(), ".s"), names(&dir(), ".o"));
    let names = names(&dir(), ".o");
    assert!(names.len() >= 4, "{names:?}");
    let fails: Vec<String> = names
        .iter()
        .filter_map(|n| check(n).err().map(|e| format!("{n}.s: {e}")))
        .collect();
    assert!(fails.is_empty(), "{}", fails.join("\n\n"));
}

/// The rows the elf crate reads from an object, with their sections' names:
/// (section, [(address, path, line, column, is_stmt, prologue_end,
/// epilogue_begin, end)]).
type Rows = Vec<(String, Vec<(u64, String, u32, u32, bool, bool, bool, bool)>)>;

fn rows(file: &[u8]) -> Rows {
    let elf = elf::Elf::parse(file).unwrap();
    let mut out = Vec::new();
    for t in elf.line_tables().unwrap() {
        for seq in &t.sequences {
            let section = elf.sections[seq.section.expect("an object's sequence has a section")]
                .name
                .clone();
            let rows = seq
                .rows
                .iter()
                .map(|r| {
                    let path = t.path(r.file).to_string();
                    (
                        r.addr,
                        path,
                        r.line,
                        r.column,
                        r.is_stmt,
                        r.prologue_end,
                        r.epilogue_begin,
                        r.end_sequence,
                    )
                })
                .collect();
            out.push((section, rows));
        }
    }
    out
}

/// The rows read back from our objects are clang's.
#[test]
fn line_tables_read_back() {
    for name in ["sections", "directories", "opcodes"] {
        let ours = rows(&assemble(name).unwrap());
        let theirs = rows(&std::fs::read(dir().join(format!("{name}.o"))).unwrap());
        assert_eq!(ours, theirs, "{name}");
    }
    let src = source("sections");
    let (c, h) = ("/src/kernel/t.c".to_string(), "/src/kernel/t.h".to_string());
    assert_eq!(
        rows(&merlin_as::assemble_to_elf(&src).unwrap()),
        [
            (
                ".text".to_string(),
                vec![
                    (0, c.clone(), 2, 0, true, false, false, false),
                    (4, c.clone(), 3, 13, true, true, false, false),
                    (8, h, 40, 2, true, false, false, false),
                    (12, c.clone(), 5, 5, false, false, true, false),
                    (20, c.clone(), 5, 5, false, false, false, true),
                ]
            ),
            (
                ".text.other".to_string(),
                vec![
                    (0, c.clone(), 9, 1, false, false, false, false),
                    (4, c.clone(), 1000, 1, false, false, false, false),
                    (12, c, 1000, 1, false, false, false, true),
                ]
            ),
        ]
    );
}
