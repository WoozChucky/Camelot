//! merlin-as's mapping symbols against clang's (#192): `$x<isa>` where code
//! starts and `$d` where data starts, in every section, which is how
//! llvm-objdump 23 and merlin-objdump tell data among instructions from
//! instructions.
//!
//! * `tests/mapping/*.s`, made for this (the code and data switches,
//!   padding, empty data, `.insn`, `.option`, `.comm`, every section kind),
//!   against clang's objects next to them, `*.o`
//!   (scripts/gen-mapping-golden.sh): the whole symbol table, every field,
//!   in clang's order, and every section's header and contents (the
//!   padding of `align.s`'s alignments with a fill and a maximum). And
//!   `merlin-objdump -d -r` of ours against `llvm-objdump -d -r` of
//!   clang's (`*.d.txt`), the listing the mapping symbols decide.
//! * The line table corpus, `tests/debug_line` (clang's objects too, with
//!   line tables): the same symbols, but for the labels clang makes for its
//!   line table where merlin-as uses section symbols (`.L0 `,
//!   `.Lline_table_start0`).
//! * The encoder's corpus, `tests/golden/encode.s`,
//!   instructions only: one `$x` at the start, as clang writes for it.

use std::path::{Path, PathBuf};

const STT_SECTION: u8 = 3;

/// A symbol as `llvm-objdump -t` shows it: name, value, size, `st_info`,
/// `st_other`, and where it is (a section's name, `ABS` or `UND`). A
/// section symbol is named after its section.
type Sym = (String, u64, u64, u8, u8, String);

fn symbols(file: &[u8]) -> Vec<Sym> {
    let elf = elf::Elf::parse(file).unwrap();
    elf.symbols().unwrap()[1..]
        .iter()
        .map(|s| {
            let place = match s.shndx {
                elf::SHN_ABS => "ABS".to_string(),
                elf::SHN_UNDEF => "UND".to_string(),
                i => elf.sections[i as usize].name.clone(),
            };
            let name = if s.ty() == STT_SECTION {
                place.clone()
            } else {
                s.name.clone()
            };
            (name, s.value, s.size, s.info, s.other, place)
        })
        .collect()
}

/// A section: name, type, flags, size, alignment, entry size, contents.
type Sect = (String, u32, u64, u64, u64, u64, Vec<u8>);

/// The sections of an object but its symbol, string and relocation tables
/// (which clang orders otherwise): name, type, flags, size, alignment,
/// entry size and contents, in their order. The padding's bytes are in the
/// contents, and the alignment a padding left out still raises.
fn sections(file: &[u8]) -> Vec<Sect> {
    let elf = elf::Elf::parse(file).unwrap();
    elf.sections[1..]
        .iter()
        .filter(|s| ![elf::SHT_SYMTAB, elf::SHT_STRTAB, elf::SHT_RELA].contains(&s.ty))
        .map(|s| {
            let data = elf.section_data(s).unwrap().to_vec();
            (s.name.clone(), s.ty, s.flags, s.size, s.align, s.entsize, data)
        })
        .collect()
}

fn is_mapping(s: &Sym) -> bool {
    s.0 == "$d" || s.0.starts_with("$x")
}

/// clang's name for each mapping symbol of code, per width: `$x` and the
/// ISA of `-march=rv64ima_zicsr_zifencei`, as `.riscv.attributes` spells it.
fn code() -> String {
    format!("$x{}", merlin_as::elf::ARCH)
}

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests")
}

/// Where clang's objects are: `tests/<set>`.
fn objects(set: &str) -> PathBuf {
    root().join(set)
}

/// The source of object `name` in `set`.
fn source(set: &str, name: &str) -> String {
    std::fs::read_to_string(objects(set).join(format!("{name}.s"))).unwrap()
}

fn assemble(src: &str) -> Vec<u8> {
    merlin_as::assemble_to_elf(src).unwrap_or_else(|e| panic!("{e:?}"))
}

/// The objects of `set`, by name.
fn names(set: &str) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(objects(set))
        .unwrap()
        .filter_map(|e| {
            let name = e.unwrap().file_name().into_string().unwrap();
            name.strip_suffix(".o").map(String::from)
        })
        .collect();
    names.sort();
    names
}

fn listing(syms: &[Sym]) -> String {
    syms.iter()
        .map(|s| format!("{:#x} {:#x} {:#04x} {} {:<18} {}", s.1, s.2, s.3, s.4, s.5, s.0))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The labels clang makes for `.` (`dot.s`) are all `.L0 `; merlin-as's
/// are `.Ldot$0`, `.Ldot$1`, ... (a name per label, since it finds them by
/// name).
fn clang_names(mut syms: Vec<Sym>) -> Vec<Sym> {
    for s in &mut syms {
        if s.0.starts_with(".Ldot$") {
            s.0 = ".L0 ".into();
        }
    }
    syms
}

#[test]
fn mapping_symbols_are_clangs() {
    let names = names("mapping");
    assert_eq!(names, ["align", "dot", "mapping", "order"]);
    for name in &names {
        let ours = assemble(&source("mapping", name));
        let clang = std::fs::read(objects("mapping").join(format!("{name}.o"))).unwrap();
        let (o, c) = (clang_names(symbols(&ours)), symbols(&clang));
        assert!(o.iter().any(|s| s.0 == code()), "{name}: no {}", code());
        assert_eq!(listing(&o), listing(&c), "{name}.s: symbols");
        assert_eq!(sections(&ours), sections(&clang), "{name}.s: sections");

        // The listing: the same data among the same instructions, labels
        // and relocations, and the padding's bytes.
        let txt = objects("mapping").join(format!("{name}.d.txt"));
        if name == "dot" {
            assert!(
                !txt.exists(),
                "dot.s has no listing (scripts/gen-mapping-golden.sh)"
            );
            continue;
        }
        let opts = merlin_objdump::Options {
            disassemble: true,
            relocs: true,
            ..Default::default()
        };
        let ours = merlin_objdump::dump(&format!("{name}.o"), &ours, &opts).unwrap();
        let want = std::fs::read_to_string(&txt).unwrap().replace("\r\n", "\n");
        assert_eq!(ours, want, "{name}.s: merlin-objdump -d -r");
    }
}

/// The line table corpus: every symbol but the labels and section symbols
/// that differ (module docs), the mapping symbols of the line table's own
/// sections included, in clang's order.
#[test]
fn line_table_objects_have_clangs_mapping_symbols() {
    let names = names("debug_line");
    assert!(names.len() >= 4, "{names:?}");
    let comparable = |syms: Vec<Sym>| -> Vec<Sym> {
        syms.into_iter()
            .filter(|s| !s.0.starts_with(".L") && s.3 & 0xf != STT_SECTION)
            .collect()
    };
    for name in &names {
        let ours = assemble(&source("debug_line", name));
        let clang = std::fs::read(objects("debug_line").join(format!("{name}.o"))).unwrap();
        let (o, c) = (comparable(symbols(&ours)), comparable(symbols(&clang)));
        assert!(o.iter().any(is_mapping), "{name}");
        assert_eq!(listing(&o), listing(&c), "{name}.s");
    }
}

/// The encoder's corpus is instructions only: clang's object of it (with
/// -mno-relax) has one `$x` at the start of `.text`, and the `$d` of
/// `.riscv.attributes`.
#[test]
fn instructions_alone_have_one_mapping_symbol() {
    let src = std::fs::read_to_string(root().join("golden/encode.s")).unwrap();
    {
        let mapping: Vec<(String, u64, String)> = symbols(&assemble(&src))
            .into_iter()
            .filter(is_mapping)
            .map(|s| (s.0, s.1, s.5))
            .collect();
        let want = [
            (code(), 0, ".text".to_string()),
            ("$d".to_string(), 0, ".riscv.attributes".to_string()),
        ];
        assert_eq!(mapping, want);
    }
}
