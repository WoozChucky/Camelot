//! `-g`: the debug information merlin writes (`merlin::debug`), assembled by
//! merlin-as and read back with elf32. It must not change the code: every test
//! case compiles to the same loaded bytes, relocations and symbols with and
//! without it.

use merlin::{DebugInfo, Options};
use std::path::{Path, PathBuf};

fn debug_opts(comp_dir: &str) -> Options {
    Options {
        debug: Some(DebugInfo {
            comp_dir: comp_dir.into(),
        }),
        ..Options::default()
    }
}

/// The parts of an object that end up loaded: each allocated section's name,
/// bytes and relocations (offset, type, symbol name), and the symbols other
/// than section symbols.
type Loaded = (
    Vec<(String, Vec<u8>, Vec<(u32, u32, String)>)>,
    Vec<(String, u32, u32, u8)>,
);

fn loaded(obj: &[u8]) -> Loaded {
    let elf = elf32::Elf::parse(obj).unwrap();
    let symbols = elf.symbols().unwrap();
    let mut sections = Vec::new();
    for (i, s) in elf.sections.iter().enumerate() {
        if !s.is_alloc() {
            continue;
        }
        let mut relocs = Vec::new();
        for rel in elf
            .sections
            .iter()
            .filter(|r| r.ty == elf32::SHT_RELA && r.info as usize == i)
        {
            for r in elf.relocations(rel).unwrap() {
                relocs.push((r.offset, r.ty, symbols[r.sym as usize].name.clone()));
            }
        }
        sections.push((s.name.clone(), elf.section_data(s).unwrap().to_vec(), relocs));
    }
    let symbols = symbols
        .iter()
        .filter(|s| s.ty() != elf32::STT_SECTION)
        .map(|s| (s.name.clone(), s.value, s.size, s.info))
        .collect();
    (sections, symbols)
}

fn cases() -> Vec<PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/cases");
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "c"))
        .collect();
    files.sort();
    files
}

#[test]
fn debug_information_changes_no_code() {
    let cases = cases();
    assert!(cases.len() > 50);
    for path in cases {
        let src = std::fs::read_to_string(&path).unwrap();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let plain = merlin::compile_with(&name, &src, &Options::default(), &merlin::no_files).unwrap();
        let debug = merlin::compile_with(&name, &src, &debug_opts("tests/cases"), &merlin::no_files).unwrap();
        assert!(debug.contains(".loc 0 "), "{name}: no line information");
        let plain = merlin_as::assemble_to_elf(&plain).unwrap();
        let debug = merlin_as::assemble_to_elf(&debug).unwrap_or_else(|e| panic!("{name}: {e:?}"));
        assert_eq!(loaded(&plain), loaded(&debug), "{name}");
    }
}

#[test]
fn the_line_table_follows_the_statements() {
    let src = "\
int g;
int f(int n) {
    int s = 0;
    for (int i = 0; i < n; i++)
        s += i;
    while (s > 10)
        s--;
    if (s)
        g = s;
    return s;
}
void empty(void) {
}
";
    let asm = merlin::compile_with("loop.c", src, &debug_opts("dir"), &merlin::no_files).unwrap();
    let obj = merlin_as::assemble_to_elf(&asm).unwrap();
    let elf = elf32::Elf::parse(&obj).unwrap();
    let tables = elf.line_tables().unwrap();
    assert_eq!(tables.len(), 1);
    let t = &tables[0];
    assert_eq!(t.version, 5);
    assert_eq!(t.files, ["dir/loop.c"]);
    let seq = &t.sequences[0];
    let lines: Vec<(u32, u32, bool)> = seq
        .rows
        .iter()
        .filter(|r| !r.end_sequence)
        .map(|r| (r.line, r.column, r.prologue_end))
        .collect();
    assert_eq!(
        lines,
        [
            (2, 0, false), // f's prologue
            (3, 9, true),  // s = 0: the prologue ends
            (4, 14, false),
            (4, 23, false), // i < n (at its operator), each time round
            (5, 9, false),
            (4, 29, false), // i++
            (6, 14, false),
            (7, 9, false),
            (8, 5, false),
            (9, 9, false),
            (10, 5, false),
            (11, 1, false), // the closing brace: the epilogue
            (12, 0, false), // empty's prologue
            (13, 1, true),  // its closing brace ends it
        ]
    );
    // Every row is at an instruction, in order, in .text.
    let text = elf.sections.iter().position(|s| s.name == ".text").unwrap();
    assert_eq!(seq.section, Some(text));
    assert!(seq.rows.windows(2).all(|w| w[0].addr < w[1].addr));
    assert_eq!(seq.rows.last().unwrap().addr, elf.sections[text].size);

    // The compile unit's length covers .debug_info exactly; it names the
    // file, the directory and the functions.
    let info = elf.sections.iter().find(|s| s.name == ".debug_info").unwrap();
    let data = elf.section_data(info).unwrap();
    let length = u32::from_le_bytes(data[..4].try_into().unwrap());
    assert_eq!(length as usize + 4, data.len());
    let text_of = |b: &[u8]| String::from_utf8_lossy(b).into_owned();
    let strings = text_of(data);
    for s in ["merlin\0", "loop.c\0dir\0", "f\0", "empty\0"] {
        assert!(strings.contains(s), "{s:?} in {strings:?}");
    }
}

#[test]
fn headers_get_their_own_file_numbers() {
    let src = "#include \"inc/h.h\"\nint main(void) {\n    return twice(2);\n}\n";
    let load = |p: &Path| {
        (p == Path::new("inc/h.h")).then(|| "static int twice(int x) {\n    return 2 * x;\n}\n".to_string())
    };
    let asm = merlin::compile_with("m.c", src, &debug_opts("src"), &load).unwrap();
    assert!(asm.starts_with("    .file 0 \"src\" \"m.c\"\n"), "{asm}");
    assert!(
        asm.contains("    .file 1 \"src\" \"inc/h.h\"\n    .loc 1 1 0\n"),
        "{asm}"
    );
    let obj = merlin_as::assemble_to_elf(&asm).unwrap();
    let tables = elf32::Elf::parse(&obj).unwrap().line_tables().unwrap();
    assert_eq!(tables[0].files, ["src/m.c", "src/inc/h.h"]);
    // A file without functions has nothing to describe.
    let asm = merlin::compile_with("d.c", "int x = 1;\n", &debug_opts("src"), &merlin::no_files).unwrap();
    assert!(!asm.contains(".debug_info"), "{asm}");
    assert!(merlin_as::assemble_to_elf(&asm).is_ok());
}

#[test]
fn the_command_line_takes_g() {
    let dir = std::env::temp_dir().join(format!("merlin-debug-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.c"), "int f(void) { return 1; }\n").unwrap();
    let run = |args: &[&str]| {
        let out = std::process::Command::new(env!("CARGO_BIN_EXE_merlin"))
            .args(args)
            .arg("a.c")
            .arg("-o")
            .arg("-")
            .current_dir(&dir)
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        std::fs::read_to_string(dir.join("-")).unwrap()
    };
    assert!(run(&["-g", "-fdebug-compilation-dir=here"]).starts_with("    .file 0 \"here\" \"a.c\"\n"));
    assert!(run(&["-g", "-fdebug-compilation-dir", "there"]).starts_with("    .file 0 \"there\" \"a.c\"\n"));
    // By default the directory merlin runs in.
    let cwd = dir.canonicalize().unwrap();
    let asm = run(&["-g"]);
    let first = asm.lines().next().unwrap();
    assert!(first.starts_with("    .file 0 \""), "{first}");
    let named = first.split('"').nth(1).unwrap().replace("\\\\", "\\");
    assert_eq!(Path::new(&named).canonicalize().unwrap(), cwd);
    assert!(!run(&[]).contains(".file"));
    let _ = std::fs::remove_dir_all(&dir);
}
