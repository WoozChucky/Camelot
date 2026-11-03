//! End-to-end tests against clang-built objects and ld.lld's output for the
//! same inputs (`tests/fixtures`, regenerate with `cargo xtask ld-fixtures`).
//! Each `name.ld` there lists its objects in its first comment;
//! `name.lld.elf` is ld.lld's link, `name.expected.txt` is
//! `llvm-readelf -S -l -s` of it and `name.objdump.txt` is `llvm-objdump -d`
//! of it.
//!
//! Linked by merlin-ld, the PT_LOAD contents must equal ld.lld's byte for
//! byte, the section headers and symbols must match, merlin-objdump's
//! disassembly must match llvm-objdump's, and the program must run in Grail:
//!
//! * `fixtures/norelocs`: three relocation-free objects.
//! * `fixtures/layouts`: layout edge cases (empty and assignment-only output
//!   sections, gaps, NOBITS before PROGBITS, `. +=` sections).
//! * `fixtures/hello`: `examples/hello/hello.S` assembled by clang.
//! * `fixtures/multiobj`: `examples/multiobj`, two objects that call and
//!   point into each other.
//! * `fixtures/relocs`: every relocation type, across objects, at the edges
//!   of their ranges; one object built with relaxation (`R_RISCV_RELAX`).
//! * `fixtures/merge`: mergeable strings, where merlin-ld differs from ld.lld
//!   on purpose (spec §10); the test pins down the difference.
//! * `fixtures/rejects`, `fixtures/symbols`: objects for error paths.

use grail::{Machine, RunResult};
use merlin_ld::{Input, Options, ScriptSource};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn input(path: &Path) -> Input {
    Input {
        name: path.file_name().unwrap().to_string_lossy().into_owned(),
        data: std::fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display())),
    }
}

fn inputs(dir: &str, names: &[&str]) -> Vec<Input> {
    names
        .iter()
        .map(|n| input(&fixtures().join(dir).join(n)))
        .collect()
}

fn script_file(path: &Path) -> Option<ScriptSource> {
    Some(ScriptSource {
        name: path.file_name().unwrap().to_string_lossy().into_owned(),
        text: std::fs::read_to_string(path).unwrap(),
    })
}

fn script(text: &str) -> Options {
    Options {
        script: Some(ScriptSource {
            name: "test.ld".into(),
            text: text.into(),
        }),
        entry: None,
    }
}

fn errors(r: Result<impl std::fmt::Debug, Vec<merlin_ld::Error>>) -> Vec<String> {
    r.expect_err("link should fail")
        .into_iter()
        .map(|e| e.0)
        .collect()
}

fn norelocs() -> Vec<Input> {
    inputs("norelocs", &["start.o", "data.o", "more.o"])
}

fn norelocs_opts() -> Options {
    Options {
        script: script_file(&fixtures().join("norelocs/link.ld")),
        entry: None,
    }
}

/// The objects a fixture script names in its `/* objects: ... */` comment
/// (paths relative to `fixtures/`), and the script itself.
fn case(script: &str) -> (Vec<Input>, Options) {
    let path = fixtures().join(script);
    let opts = Options {
        script: script_file(&path),
        entry: None,
    };
    let text = &opts.script.as_ref().unwrap().text;
    let list = text
        .split("*/")
        .next()
        .and_then(|c| c.trim().strip_prefix("/*"))
        .and_then(|c| c.trim().strip_prefix("objects:"));
    let objs = match list {
        Some(list) => list
            .split_whitespace()
            .map(|o| input(&fixtures().join(o)))
            .collect(),
        // Without the comment: every object in the script's directory, in
        // name order (as `cargo xtask ld-fixtures` links them).
        None => {
            let mut paths: Vec<PathBuf> = std::fs::read_dir(path.parent().unwrap())
                .unwrap()
                .map(|e| e.unwrap().path())
                .filter(|p| p.extension().is_some_and(|e| e == "o"))
                .collect();
            paths.sort();
            assert!(!paths.is_empty(), "{script}: no objects");
            paths.iter().map(|p| input(p)).collect()
        }
    };
    (objs, opts)
}

// ---------------------------------------------------------------------------
// Reading `llvm-readelf -S -l -s` output (the committed expectations).

#[derive(Debug, Clone, PartialEq, Eq)]
struct Sect {
    name: String,
    ty: String,
    addr: u32,
    size: u32,
    flags: String,
    align: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Load {
    offset: u32,
    vaddr: u32,
    paddr: u32,
    filesz: u32,
    memsz: u32,
    flags: String,
    align: u32,
}

#[derive(Debug, Default)]
struct Readelf {
    entry: u32,
    /// Allocated sections only.
    sections: Vec<Sect>,
    loads: Vec<Load>,
    /// Global and weak symbols.
    globals: HashMap<String, u32>,
    /// Local symbols (only read from our own output).
    locals: Vec<(String, u32)>,
}

fn hex(s: &str) -> u32 {
    u32::from_str_radix(s.trim_start_matches("0x"), 16).unwrap()
}

fn parse_readelf(text: &str) -> Readelf {
    let mut r = Readelf::default();
    for line in text.lines().map(|l| l.trim_end_matches('\r')) {
        let t: Vec<&str> = line.split_whitespace().collect();
        if let Some(e) = line.strip_prefix("Entry point ") {
            r.entry = hex(e.trim());
        } else if line.trim_start().starts_with('[') && !line.contains("[Nr]") {
            let rest: Vec<&str> = line.split(']').nth(1).unwrap().split_whitespace().collect();
            // name type addr off size es [flg] lk inf al
            if rest.len() == 10 && rest[6].contains('A') {
                r.sections.push(Sect {
                    name: rest[0].into(),
                    ty: rest[1].into(),
                    addr: hex(rest[2]),
                    size: hex(rest[4]),
                    flags: rest[6].into(),
                    align: rest[9].parse().unwrap(),
                });
            }
        } else if t.first() == Some(&"LOAD") {
            r.loads.push(Load {
                offset: hex(t[1]),
                vaddr: hex(t[2]),
                paddr: hex(t[3]),
                filesz: hex(t[4]),
                memsz: hex(t[5]),
                flags: t[6..t.len() - 1].concat(),
                align: hex(t[t.len() - 1]),
            });
        } else if t.len() == 8 && (t[4] == "GLOBAL" || t[4] == "WEAK") && t[6] != "UND" {
            // (ld.lld lists undefined weak symbols, merlin-ld leaves them out.)
            r.globals.insert(t[7].into(), hex(t[1]));
        }
    }
    r
}

/// `fixtures/<name>.expected.txt`, e.g. `expected("norelocs/link")`.
fn expected(name: &str) -> Readelf {
    let path = fixtures().join(format!("{name}.expected.txt"));
    parse_readelf(&std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display())))
}

// ---------------------------------------------------------------------------
// Reading our own ELF output the same way, straight from the bytes.

fn u16_at(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
fn cstr(b: &[u8], o: usize) -> String {
    let end = b[o..].iter().position(|&c| c == 0).unwrap();
    String::from_utf8(b[o..o + end].to_vec()).unwrap()
}

/// File offset of the section header named `name`.
fn shdr(elf: &[u8], name: &str) -> usize {
    let shoff = u32_at(elf, 32) as usize;
    let (shnum, shstrndx) = (u16_at(elf, 48) as usize, u16_at(elf, 50) as usize);
    let shstr = u32_at(elf, shoff + 40 * shstrndx + 16) as usize;
    (0..shnum)
        .map(|i| shoff + 40 * i)
        .find(|&h| cstr(elf, shstr + u32_at(elf, h) as usize) == name)
        .unwrap_or_else(|| panic!("no section {name}"))
}

fn section_index(elf: &[u8], name: &str) -> u32 {
    ((shdr(elf, name) - u32_at(elf, 32) as usize) / 40) as u32
}

/// File offset of the `.symtab` entry named `name`.
fn symbol_entry(elf: &[u8], name: &str) -> usize {
    let symtab = shdr(elf, ".symtab");
    let (off, size) = (
        u32_at(elf, symtab + 16) as usize,
        u32_at(elf, symtab + 20) as usize,
    );
    let shoff = u32_at(elf, 32) as usize;
    let strtab = u32_at(elf, shoff + 40 * u32_at(elf, symtab + 24) as usize + 16) as usize;
    (off..off + size)
        .step_by(16)
        .find(|&s| cstr(elf, strtab + u32_at(elf, s) as usize) == name)
        .unwrap_or_else(|| panic!("no symbol {name}"))
}

fn read_ours(elf: &[u8]) -> Readelf {
    assert_eq!(&elf[..4], b"\x7fELF");
    assert_eq!(u16_at(elf, 16), 2, "ET_EXEC");
    assert_eq!(u16_at(elf, 18), 243, "EM_RISCV");
    let mut r = Readelf {
        entry: u32_at(elf, 24),
        ..Default::default()
    };
    let (phoff, shoff) = (u32_at(elf, 28) as usize, u32_at(elf, 32) as usize);
    let (phnum, shnum, shstrndx) = (
        u16_at(elf, 44) as usize,
        u16_at(elf, 48) as usize,
        u16_at(elf, 50) as usize,
    );
    for i in 0..phnum {
        let p = phoff + 32 * i;
        assert_eq!(u32_at(elf, p), 1, "only PT_LOAD");
        let f = u32_at(elf, p + 24);
        let flags = [(4, "R"), (2, "W"), (1, "E")]
            .iter()
            .filter(|(b, _)| f & b != 0)
            .map(|(_, s)| *s)
            .collect();
        r.loads.push(Load {
            offset: u32_at(elf, p + 4),
            vaddr: u32_at(elf, p + 8),
            paddr: u32_at(elf, p + 12),
            filesz: u32_at(elf, p + 16),
            memsz: u32_at(elf, p + 20),
            flags,
            align: u32_at(elf, p + 28),
        });
    }
    let sh = |i: usize, f: usize| u32_at(elf, shoff + 40 * i + f);
    let shstr = sh(shstrndx, 16) as usize;
    for i in 1..shnum {
        let ty = sh(i, 4);
        let flags = sh(i, 8);
        if ty == 2 {
            // .symtab: collect globals.
            let (off, size, strtab) = (
                sh(i, 16) as usize,
                sh(i, 20) as usize,
                sh(sh(i, 24) as usize, 16) as usize,
            );
            for s in (off + 16..off + size).step_by(16) {
                let (name, value) = (cstr(elf, strtab + u32_at(elf, s) as usize), u32_at(elf, s + 4));
                if elf[s + 12] >> 4 != 0 {
                    r.globals.insert(name, value);
                } else {
                    r.locals.push((name, value));
                }
            }
        }
        if flags & 2 == 0 {
            continue;
        }
        let flag_str = [(1, "W"), (2, "A"), (4, "X")]
            .iter()
            .filter(|(b, _)| flags & b != 0)
            .map(|(_, s)| *s)
            .collect();
        r.sections.push(Sect {
            name: cstr(elf, shstr + sh(i, 0) as usize),
            ty: match ty {
                1 => "PROGBITS".into(),
                8 => "NOBITS".into(),
                t => format!("type {t}"),
            },
            addr: sh(i, 12),
            size: sh(i, 20),
            flags: flag_str,
            align: sh(i, 32),
        });
    }
    r
}

// ---------------------------------------------------------------------------

/// Acceptance: relocation-free clang objects link to the same PT_LOADs,
/// sections and symbol addresses as ld.lld.
#[test]
fn norelocs_matches_ld_lld() {
    assert_eq!(
        expected("norelocs/link").loads.len(),
        3,
        "fixture sanity: RX, R and RW segments"
    );
    assert_matches_ld_lld("norelocs/link", "OK\n");
}

/// `(vaddr, memsz, file bytes)` of every PT_LOAD of an executable.
fn load_images(elf: &[u8]) -> Vec<(u32, u32, Vec<u8>)> {
    let (phoff, phnum) = (u32_at(elf, 28) as usize, u16_at(elf, 44) as usize);
    (0..phnum)
        .map(|i| phoff + 32 * i)
        .filter(|&p| u32_at(elf, p) == 1)
        .map(|p| {
            let (off, filesz) = (u32_at(elf, p + 4) as usize, u32_at(elf, p + 16) as usize);
            (
                u32_at(elf, p + 8),
                u32_at(elf, p + 20),
                elf[off..off + filesz].to_vec(),
            )
        })
        .collect()
}

/// `llvm-objdump -d` text without its first line (which names the file),
/// line endings normalized.
fn disassembly_body(text: &str) -> Vec<String> {
    text.lines()
        .map(|l| l.trim_end_matches('\r').to_string())
        .skip_while(|l| !l.contains("file format"))
        .skip(1)
        .collect()
}

/// merlin-objdump's `-d` of our link against llvm-objdump's of ld.lld's
/// (`name.objdump.txt`). Two deliberate differences (spec §10) are allowed:
///
/// * where LLVM prints `<unknown>` (the Xcal instructions), merlin-objdump
///   names the instruction; the address and the word must still agree;
/// * a symbol assigned by the script is `SHN_ABS` in merlin-ld's output, so
///   when it shares its address with a symbol of the section, a label line
///   may name the other one.
fn assert_same_disassembly(name: &str, linked: &merlin_ld::Linked) {
    let elf = &linked.elf;
    let script_symbol = |n: &str| {
        linked
            .layout
            .globals
            .iter()
            .any(|(g, d)| g == n && matches!(d, merlin_ld::layout::Def::Script(_)))
    };
    let opts = merlin_objdump::Options {
        disassemble: true,
        ..Default::default()
    };
    let ours = disassembly_body(&merlin_objdump::dump("ours.elf", elf, &opts).unwrap());
    let path = fixtures().join(format!("{name}.objdump.txt"));
    let lld = disassembly_body(&std::fs::read_to_string(&path).unwrap());
    assert!(lld.len() > 3, "{name}: empty llvm-objdump listing");
    assert_eq!(ours.len(), lld.len(), "{name}: disassembly length");
    // `80001040 <mid>:` -> ("80001040", "mid")
    let label = |l: &str| {
        let (addr, rest) = l.split_once(" <")?;
        Some((addr.to_string(), rest.strip_suffix(">:")?.to_string()))
    };
    for (o, l) in ours.iter().zip(&lld) {
        if let Some(word) = l.strip_suffix("<unknown>") {
            assert!(o.starts_with(word), "{name}:\n ours: {o}\n llvm: {l}");
        } else if let (Some((oa, _)), Some((la, ln))) = (label(o), label(l)) {
            assert!(
                o == l || (oa == la && script_symbol(&ln)),
                "{name}:\n ours: {o}\n llvm: {l}"
            );
        } else {
            assert_eq!(o, l, "{name}: disassembly");
        }
    }
}

/// Links fixture script `name.ld` with the objects it lists, checks the result
/// against ld.lld's (`name.expected.txt`, `name.lld.elf`, `name.objdump.txt`)
/// and runs it in Grail, which must print `output` and exit with 0.
fn assert_matches_ld_lld(name: &str, output: &str) -> merlin_ld::Linked {
    assert_matches_ld_lld_exit(name, output, 0)
}

/// [`assert_matches_ld_lld`] for a program that exits with `exit`.
fn assert_matches_ld_lld_exit(name: &str, output: &str, exit: u32) -> merlin_ld::Linked {
    let (objs, opts) = case(&format!("{name}.ld"));
    let linked = merlin_ld::link(&objs, &opts).unwrap_or_else(|e| panic!("{name}: {e:?}"));
    assert!(
        linked.layout.warnings.is_empty(),
        "{name}: {:?}",
        linked.layout.warnings
    );
    let ours = read_ours(&linked.elf);
    let lld = expected(name);
    assert_eq!(ours.entry, lld.entry, "{name}: entry");
    assert_eq!(ours.sections, lld.sections, "{name}: sections");
    assert_eq!(ours.loads, lld.loads, "{name}: PT_LOADs");
    assert_eq!(ours.globals, lld.globals, "{name}: global symbols");

    // Byte for byte: every loaded byte, relocated code and data included.
    let lld_elf = std::fs::read(fixtures().join(format!("{name}.lld.elf"))).unwrap();
    let (ours_img, lld_img) = (load_images(&linked.elf), load_images(&lld_elf));
    assert_eq!(ours_img.len(), lld_img.len(), "{name}: PT_LOAD count");
    for ((va, msz, a), (vb, mszb, b)) in ours_img.iter().zip(&lld_img) {
        assert_eq!(
            (va, msz, a.len()),
            (vb, mszb, b.len()),
            "{name}: segment at {va:#x}"
        );
        if let Some(i) = (0..a.len()).find(|&i| a[i] != b[i]) {
            panic!(
                "{name}: first difference at {:#x}: ours {:02x?}, ld.lld {:02x?}",
                va + i as u32,
                &a[i..(i + 8).min(a.len())],
                &b[i..(i + 8).min(b.len())]
            );
        }
    }
    assert_same_disassembly(name, &linked);

    let (_, r, out) = run(&linked.elf);
    assert_eq!(
        (r, out.as_str()),
        (RunResult::Exited(exit), output),
        "{name}: run in Grail"
    );
    linked
}

/// Layout edge cases against ld.lld: output sections with only empty inputs
/// (they still align), with only assignments (kept, PROGBITS, flags of the
/// previous section), with only `. +=` (PROGBITS with file contents), with
/// nothing (dropped), a symbol before the first section with MEMORY (0), a
/// gap and NOBITS before PROGBITS inside one PT_LOAD, an empty R section
/// between RX and RW (no PT_LOAD of its own), a weak definition.
#[test]
fn layout_edge_cases_match_ld_lld() {
    let mut names: Vec<String> = std::fs::read_dir(fixtures().join("layouts"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "ld"))
        .map(|p| format!("layouts/{}", p.file_stem().unwrap().to_string_lossy()))
        .collect();
    names.sort();
    assert_eq!(
        names,
        [
            "layouts/attrs",
            "layouts/empty",
            "layouts/gap",
            "layouts/inside",
            "layouts/nobits-first",
            "layouts/page-gap"
        ]
    );
    for name in &names {
        assert_matches_ld_lld(name, "OK\n");
    }
    // Spot checks, so a broken fixture can't make both sides agree on nonsense.
    let lld = expected("layouts/empty");
    assert_eq!(lld.globals["__image_start"], 0);
    assert_eq!(lld.globals["wk"], 0x8000_0024, "weak definition");
    let data = lld.sections.iter().find(|s| s.name == ".data").unwrap();
    assert_eq!((data.size, data.align), (0, 16));
    assert!(lld.sections.iter().all(|s| s.name != ".nothing"));
    let stack = lld.sections.iter().find(|s| s.name == ".stack").unwrap();
    assert_eq!((stack.ty.as_str(), stack.flags.as_str()), ("PROGBITS", "WA"));
    assert_eq!(expected("layouts/gap").loads.len(), 2);
    assert_eq!(expected("layouts/nobits-first").loads.len(), 3);
    // Output section attributes (kernel/kernel.ld's): ALIGN(n) raises the
    // alignment and never lowers it, NOLOAD makes even PROGBITS input and
    // an assignment-only section NOBITS, /DISCARD/ drops a section and its
    // global.
    let attrs = expected("layouts/attrs");
    let sec = |name: &str| attrs.sections.iter().find(|s| s.name == name).unwrap();
    assert_eq!((sec(".rodata").addr, sec(".rodata").align), (0x8000_1000, 4096));
    assert_eq!(sec(".data").align, 16);
    assert_eq!((sec(".bss").ty.as_str(), sec(".bss").align), ("NOBITS", 64));
    assert_eq!(sec(".noload").ty, "NOBITS");
    assert_eq!(sec(".stack").ty, "NOBITS");
    assert!(attrs.sections.iter().all(|s| !s.name.starts_with(".discard")));
    assert!(!attrs.globals.contains_key("dropped"));
    assert_eq!(attrs.globals["__end"], 0x8000_2000);
    assert_eq!(attrs.loads.len(), 3);
}

/// What /DISCARD/ throws away cannot be referred to: ld.lld's "relocation
/// refers to a discarded section" is merlin-ld's "was discarded by
/// /DISCARD/". Like ld.lld, an `la` reports it once, at its PCREL_HI20: the
/// PCREL_LO12 paired with it adds no second error.
#[test]
fn a_reference_to_a_discarded_section_is_an_error() {
    let (mut inputs, opts) = case("layouts/attrs.ld");
    inputs.push(merlin_ld::Input {
        name: "use.o".into(),
        data: merlin_as::assemble_to_elf("    .data\n    .word dropped\n").unwrap(),
    });
    let errs = errors(merlin_ld::link(&inputs, &opts));
    assert_eq!(errs.len(), 1, "{errs:?}");
    assert!(
        errs[0].contains(
            "use.o: R_RISCV_32 at .data+0x0 refers to 'dropped', whose section was discarded by /DISCARD/"
        ),
        "{errs:?}"
    );

    let (mut inputs, opts) = case("layouts/attrs.ld");
    inputs.push(merlin_ld::Input {
        name: "la.o".into(),
        data: merlin_as::assemble_to_elf("    .text\n    la a0, dropped\n").unwrap(),
    });
    let errs = errors(merlin_ld::link(&inputs, &opts));
    assert_eq!(errs.len(), 1, "{errs:?}");
    assert!(
        errs[0].contains(
            "la.o: R_RISCV_PCREL_HI20 at .text+0x0 refers to 'dropped', whose section was discarded"
        ),
        "{errs:?}"
    );
}

/// Spec §10: merlin-ld does not merge SHF_MERGE strings. clang puts every C
/// string literal in `.rodata.str1.1`; with the same literal in two objects
/// ld.lld keeps one copy, merlin-ld keeps both and warns.
#[test]
fn mergeable_strings_are_not_merged() {
    let (objs, opts) = case("merge/link.ld");
    let linked = merlin_ld::link(&objs, &opts).unwrap();
    let l = &linked.layout;
    assert_eq!(l.warnings.len(), 1, "{:?}", l.warnings);
    assert!(
        l.warnings[0].starts_with(".rodata: mergeable (SHF_MERGE) input sections hold 6 bytes of duplicate"),
        "{}",
        l.warnings[0]
    );
    let lld = expected("merge/link");
    let ours = read_ours(&linked.elf);
    assert_eq!(ours.sections[0], lld.sections[0], ".text is unaffected");
    let (o, t) = (&ours.sections[1], &lld.sections[1]);
    assert_eq!((o.name.as_str(), o.addr), (".rodata", t.addr));
    assert_eq!(o.size, t.size + 6, "one extra \"hello\\0\"");
    assert_eq!(lld.globals["s1"], lld.globals["s2"], "ld.lld merged them");
    assert_eq!(ours.globals["s1"], lld.globals["s1"]);
    assert_eq!(
        ours.globals["s2"],
        ours.globals["after_a"] + 1,
        "input order, no merging"
    );
    assert_eq!(ours.globals["__end"], lld.globals["__end"] + 6);
    // No duplicates, no warning.
    let one = merlin_ld::link(&objs[..2], &opts).unwrap();
    assert!(one.layout.warnings.is_empty(), "{:?}", one.layout.warnings);
}

/// PT_LOAD contents line up with the input sections and the file is
/// consistent: every segment's bytes are where its offset says.
#[test]
fn norelocs_segment_contents() {
    let linked = merlin_ld::link(&norelocs(), &norelocs_opts()).unwrap();
    let elf = grail::elf::parse(&linked.elf).unwrap();
    let data = norelocs();
    let start = merlin_ld::object::parse("start.o", &data[0].data).unwrap();
    let text_start = start.sections.iter().find(|s| s.name == ".text.start").unwrap();
    let text = &elf.segments[0];
    assert_eq!(text.paddr, 0x8000_0000);
    assert_eq!(&text.data[..text_start.data.len()], &text_start.data[..]);
    let rw = &elf.segments[2];
    assert_eq!(rw.data.len(), 0x24);
    assert_eq!(rw.mem_size, 0x148);
}

fn run(elf: &[u8]) -> (Machine, RunResult, String) {
    let mut m = Machine::with_ram(grail::DEFAULT_RAM_SIZE);
    // Garbage in RAM, so .bss zeroing by the loader is actually checked.
    m.cpu.bus.ram.fill(0xa5);
    m.cpu.bus.uart.capture = true;
    m.load_elf(elf).unwrap();
    let r = m.run(Some(10_000), |_| {});
    let out = String::from_utf8(std::mem::take(&mut m.cpu.bus.uart.captured)).unwrap();
    (m, r, out)
}

fn ram(m: &Machine, addr: u32, len: usize) -> &[u8] {
    let off = (addr - grail::bus::RAM_BASE) as usize;
    &m.cpu.bus.ram[off..off + len]
}

fn check_norelocs_memory(m: &Machine, layout: &merlin_ld::Layout) {
    let sym = |n: &str| layout.symbol(n).unwrap_or_else(|| panic!("{n} undefined"));
    let words: Vec<u8> = [1u32, 2, 3, 4].iter().flat_map(|w| w.to_le_bytes()).collect();
    assert_eq!(ram(m, sym("table"), 16), &words[..]);
    assert_eq!(ram(m, sym("greeting"), 8), b"Camelot\0");
    assert_eq!(
        ram(m, sym("counter"), 8),
        &[0x78, 0x56, 0x34, 0x12, 0xf0, 0xde, 0xbc, 0x9a]
    );
    assert_eq!(ram(m, sym("flag"), 1), &[0x5a]);
    assert_eq!(ram(m, sym("wide"), 4), &0xcafef00du32.to_le_bytes());
    assert!(
        ram(m, sym("buffer"), 256).iter().all(|&b| b == 0),
        ".bss not zeroed"
    );
    assert!(
        ram(m, sym("scratch"), 24).iter().all(|&b| b == 0),
        ".bss not zeroed"
    );
    assert_eq!(sym("wide") % 16, 0);
    assert_eq!(sym("counter") % 8, 0);
    assert_eq!(sym("buffer") % 16, 0);
    assert_eq!(sym("scratch") % 8, 0);
}

/// Acceptance: Grail loads (and runs) what merlin-ld wrote.
#[test]
fn norelocs_runs_in_grail() {
    let linked = merlin_ld::link(&norelocs(), &norelocs_opts()).unwrap();
    let (m, r, out) = run(&linked.elf);
    assert_eq!(r, RunResult::Exited(0));
    assert_eq!(out, "OK\n");
    check_norelocs_memory(&m, &linked.layout);
}

/// Without -T: .text at 0x8000_0000, then .rodata, .data, .bss.
#[test]
fn default_layout() {
    let linked = merlin_ld::link(&norelocs(), &Options::default()).unwrap();
    let l = &linked.layout;
    let names: Vec<&str> = l.sections.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, [".text", ".rodata", ".data", ".bss"]);
    assert_eq!(l.sections[0].addr, 0x8000_0000);
    assert_eq!(l.entry, 0x8000_0000);
    assert_eq!(l.entry_symbol.as_deref(), Some("_start"));
    for w in l.sections.windows(2) {
        assert!(w[1].addr >= w[0].addr + w[0].size);
        assert_eq!(w[1].addr % w[1].align, 0);
        assert!(w[1].addr - (w[0].addr + w[0].size) < w[1].align);
    }
    assert!(l.sections[3].nobits);
    let (m, r, out) = run(&linked.elf);
    assert_eq!(r, RunResult::Exited(0));
    assert_eq!(out, "OK\n");
    check_norelocs_memory(&m, l);
}

/// Input order decides placement within an output section.
#[test]
fn input_order() {
    let a = merlin_ld::layout(
        &inputs("norelocs", &["start.o", "data.o", "more.o"]),
        &norelocs_opts(),
    )
    .unwrap();
    let b = merlin_ld::layout(
        &inputs("norelocs", &["start.o", "more.o", "data.o"]),
        &norelocs_opts(),
    )
    .unwrap();
    assert!(a.symbol("counter").unwrap() < a.symbol("flag").unwrap());
    assert!(b.symbol("flag").unwrap() < b.symbol("counter").unwrap());
    // .text.start is first either way: `*(.text.start)` comes before `*(.text*)`.
    let c = merlin_ld::layout(&inputs("norelocs", &["data.o", "start.o"]), &norelocs_opts()).unwrap();
    assert_eq!(c.symbol("_start"), Some(0x8000_0000));
}

/// Acceptance: examples/hello under its link.ld gets ld.lld's section addresses.
#[test]
fn hello_layout_matches_ld_lld() {
    let objs = inputs("hello", &["hello.o"]);
    let opts = Options {
        script: script_file(&repo_root().join("examples/hello/link.ld")),
        entry: None,
    };
    let layout = merlin_ld::layout(&objs, &opts).unwrap();
    let lld = expected("hello/link");
    let ours: Vec<(String, u32, u32, u32)> = layout
        .sections
        .iter()
        .map(|s| (s.name.clone(), s.addr, s.size, s.align))
        .collect();
    let theirs: Vec<(String, u32, u32, u32)> = lld
        .sections
        .iter()
        .map(|s| (s.name.clone(), s.addr, s.size, s.align))
        .collect();
    assert_eq!(ours, theirs);
    assert_eq!(layout.entry, lld.entry);
    for (name, value) in &lld.globals {
        assert_eq!(layout.symbol(name), Some(*value), "{name}");
    }
    assert_eq!(layout.symbol("__stack_top"), Some(0x8001_0070));
}

/// `fixtures/hello` and `fixtures/multiobj` are built from copies of the
/// examples' sources and scripts; if an example changes, its fixture (the
/// objects and their ld.lld listings) is stale until `cargo xtask ld-fixtures`
/// runs again.
#[test]
fn example_fixtures_are_current() {
    let copies = [
        ("hello", &["hello.S", "link.ld"][..]),
        ("multiobj", &["main.S", "print.S", "link.ld"]),
    ];
    for (dir, files) in copies {
        for f in files {
            let read = |p: PathBuf| std::fs::read_to_string(&p).unwrap().replace("\r\n", "\n");
            assert!(
                read(repo_root().join("examples").join(dir).join(f)) == read(fixtures().join(dir).join(f)),
                "examples/{dir}/{f} changed: run `cargo xtask ld-fixtures`"
            );
        }
    }
}

/// The PCREL_HI20 sites handed to the relocation hook are keyed by
/// each auipc's own address, with its symbol's final address.
#[test]
fn relocation_context() {
    let opts = Options {
        script: script_file(&repo_root().join("examples/hello/link.ld")),
        entry: None,
    };
    let layout = merlin_ld::layout(&inputs("hello", &["hello.o"]), &opts).unwrap();
    let (ctx, failed) = merlin_ld::relocate::context(&layout);
    assert!(failed.is_empty());
    let site = |name: &str| {
        let i = layout.objects[0]
            .symbols
            .iter()
            .position(|s| s.name == name)
            .unwrap();
        merlin_ld::reloc::HiSite {
            sym: layout.reloc_symbol(0, i).unwrap(),
            addend: 0,
        }
    };
    let text = layout.section(".text").unwrap().addr;
    let rodata = layout.section(".rodata").unwrap();
    assert!((rodata.addr..rodata.addr + rodata.size).contains(&site("message").sym));
    let mut sites: Vec<_> = ctx.pcrel_hi20.iter().map(|(&k, &v)| (k, v)).collect();
    sites.sort_by_key(|&(k, _)| k);
    assert_eq!(sites, [(text, site("__stack_top")), (text + 8, site("message"))]);
}

fn hello_opts() -> Options {
    Options {
        script: script_file(&repo_root().join("examples/hello/link.ld")),
        entry: None,
    }
}

/// Acceptance: examples/hello assembled by clang and linked by merlin-ld is
/// ld.lld's executable byte for byte (llvm-objdump -d of both is identical),
/// and runs in Grail.
#[test]
fn hello_matches_ld_lld_and_runs() {
    // xcal.dbg shows its own pc: 0x80000018, after the 8-byte `call puts`.
    // (Built with linker relaxation, ld.lld would shrink that call to a
    // single `jal` and print 0x80000014.)
    assert_matches_ld_lld(
        "hello/link",
        "Hello from Camelot!\n[dbg] pc=80000018 a0 = 0x00000014 (20)\n",
    );
}

/// examples/multiobj, two clang objects: calls, pc-relative addresses and
/// `.word` pointers from one into the other, `.data` written at run time and
/// `.bss` (which `run` checks is zeroed: RAM starts as garbage).
#[test]
fn multiobj_matches_ld_lld_and_runs() {
    for (obj, ty) in [
        ("multiobj/main.o", "R_RISCV_CALL_PLT"),
        ("multiobj/main.o", "R_RISCV_32"),
        ("multiobj/main.o", "R_RISCV_PCREL_HI20"),
        ("multiobj/print.o", "R_RISCV_PCREL_LO12_I"),
    ] {
        assert!(reloc_types(obj).contains_key(ty), "{obj} has no {ty}");
    }
    let linked = assert_matches_ld_lld_exit(
        "multiobj/link",
        "Hello from two objects!\nputs lives in print.S.\n[dbg] pc=80000034 a0 = 0x0000002f (47)\n",
        42,
    );
    let l = &linked.layout;
    let bss = l.section(".bss").unwrap();
    assert_eq!(l.symbol("bytes_written"), Some(bss.addr));
    assert!(l.symbol("puts").unwrap() > l.symbol("_start").unwrap());
}

/// The relocation types each fixture object carries, by name.
fn reloc_types(path: &str) -> HashMap<String, usize> {
    let data = std::fs::read(fixtures().join(path)).unwrap();
    let obj = merlin_ld::object::parse(path, &data).unwrap();
    let mut n = HashMap::new();
    for s in &obj.sections {
        for r in &s.relocs {
            *n.entry(merlin_ld::reloc::name(r.ty)).or_insert(0) += 1;
        }
    }
    n
}

/// Acceptance: every relocation type, in both directions between objects,
/// at the edges of the BRANCH and JAL ranges, at the %hi/%lo rounding edge,
/// with addends and against an undefined weak symbol, gives ld.lld's bytes.
/// The program also checks its own relocated values at run time.
#[test]
fn relocs_match_ld_lld() {
    // Fixture sanity: the objects really carry what the test is about.
    let main = reloc_types("relocs/main.o");
    for ty in [
        "R_RISCV_32",
        "R_RISCV_BRANCH",
        "R_RISCV_JAL",
        "R_RISCV_CALL",
        "R_RISCV_CALL_PLT",
        "R_RISCV_PCREL_HI20",
        "R_RISCV_PCREL_LO12_I",
        "R_RISCV_PCREL_LO12_S",
        "R_RISCV_HI20",
        "R_RISCV_LO12_I",
        "R_RISCV_LO12_S",
    ] {
        assert!(main.contains_key(ty), "main.o has no {ty}: {main:?}");
    }
    assert!(
        !main.contains_key("R_RISCV_RELAX"),
        "main.o is built with -mno-relax"
    );
    let relax = reloc_types("relocs/relax.o");
    assert_eq!(relax.get("R_RISCV_RELAX"), Some(&5), "{relax:?}");
    assert_eq!(reloc_types("rejects/align.o").get("R_RISCV_ALIGN"), Some(&1));

    let linked = assert_matches_ld_lld("relocs/link", "OK\n");
    let l = &linked.layout;
    // The edges main.S aims at, so a changed fixture can't silently lose them.
    let start = l.symbol("_start").unwrap();
    assert_eq!(start, 0x8010_0000);
    assert_eq!(l.symbol("high_target").unwrap() - (start + 4), 0xf_fffc);
    assert_eq!((start + 8) - l.symbol("low_target").unwrap(), 0x10_0000);
    let edges = l.symbol("branch_edges").unwrap();
    assert_eq!(edges - l.symbol("branch_back").unwrap(), 4096);
    assert_eq!(l.symbol("branch_fwd").unwrap() - (edges + 4), 4092);
}

/// A jump or call to an undefined weak symbol aims at itself (as in ld.lld):
/// 0 is out of a branch's reach. Data references to it see 0.
#[test]
fn undefined_weak_jumps_target_themselves() {
    let (objs, opts) = case("relocs/link.ld");
    let linked = merlin_ld::link(&objs, &opts).unwrap();
    let elf = grail::elf::parse(&linked.elf).unwrap();
    let text = elf.segments.iter().find(|s| s.paddr == 0x8010_0000).unwrap();
    let words: Vec<u32> = text
        .data
        .chunks(4)
        .map(|c| u32::from_le_bytes(c.try_into().unwrap()))
        .collect();
    // `call missing` (auipc ra, 0; jalr ra, 0(ra)), `jal missing`, `beq a0, a1, missing`.
    let at = words
        .windows(4)
        .position(|w| w == [0x0000_0097, 0x0000_80e7, 0x0000_00ef, 0x00b5_0063])
        .expect("the self-targeting jumps");
    assert!(at > 0);
}

/// A weak reference that another object defines is that definition, not an
/// undefined weak: main.o's `.weak lib_inc` call, jump and branch reach
/// lib.o's `lib_inc` (an undefined weak would aim them at themselves).
#[test]
fn weak_reference_defined_elsewhere_reaches_the_definition() {
    use excalibur::Inst;
    let (objs, opts) = case("relocs/link.ld");
    assert_eq!(objs[0].name, "main.o");
    let linked = merlin_ld::link(&objs, &opts).unwrap();
    let l = &linked.layout;
    let main = &l.objects[0];
    let weak = main.symbols.iter().position(|s| s.name == "lib_inc").unwrap();
    assert!(main.symbols[weak].is_weak() && main.symbols[weak].is_undefined());
    assert!(!l.is_undefined_weak(0, weak));
    let target = l.symbol("lib_inc").unwrap();

    let elf = grail::elf::parse(&linked.elf).unwrap();
    let word = |addr: u32| {
        let seg = elf
            .segments
            .iter()
            .find(|s| addr >= s.paddr && addr < s.paddr + s.data.len() as u32)
            .unwrap();
        u32_at(&seg.data, (addr - seg.paddr) as usize)
    };
    let sec = main
        .sections
        .iter()
        .position(|s| s.name == ".text.start")
        .unwrap();
    let base = l.input_addr(0, sec).unwrap();
    let mut kinds = Vec::new();
    for r in main.sections[sec]
        .relocs
        .iter()
        .filter(|r| r.sym as usize == weak)
    {
        let p = base + r.offset;
        let to = match excalibur::decode(word(p), excalibur::Xlen::Rv32) {
            Inst::Jal { imm, .. } | Inst::Branch { imm, .. } => p.wrapping_add(imm as u32),
            Inst::Auipc { imm: hi, .. } => match excalibur::decode(word(p + 4), excalibur::Xlen::Rv32) {
                Inst::Jalr { imm: lo, .. } => p.wrapping_add(hi as u32).wrapping_add(lo as u32),
                other => panic!("no jalr after the auipc: {other:?}"),
            },
            other => panic!("unexpected {other:?}"),
        };
        let ty = merlin_ld::reloc::name(r.ty);
        assert_eq!(to, target, "{ty} at {p:#x} goes to {to:#x}, not lib_inc");
        kinds.push(ty);
    }
    assert_eq!(kinds, ["R_RISCV_CALL_PLT", "R_RISCV_JAL", "R_RISCV_BRANCH"]);
}

fn patch_hello(edit: impl Fn(&mut Vec<u8>, usize)) -> Vec<String> {
    let mut b = std::fs::read(fixtures().join("hello/hello.o")).unwrap();
    let rela = u32_at(&b, shdr(&b, ".rela.text.start") + 16) as usize;
    edit(&mut b, rela);
    errors(merlin_ld::link(
        &[Input {
            name: "hello.o".into(),
            data: b,
        }],
        &hello_opts(),
    ))
}

/// The symbol table index of `name` in a fixture object.
fn symbol_index(path: &str, name: &str) -> u32 {
    let data = std::fs::read(fixtures().join(path)).unwrap();
    let obj = merlin_ld::object::parse(path, &data).unwrap();
    obj.symbols.iter().position(|s| s.name == name).unwrap() as u32
}

/// hello.o's relocations, in order: PCREL_HI20 __stack_top (+0x0),
/// PCREL_LO12_I .Lpcrel_hi0 (+0x4), PCREL_HI20 message (+0x8),
/// PCREL_LO12_I .Lpcrel_hi1 (+0xc). Each entry is 12 bytes: r_offset,
/// r_info (symbol << 8 | type), r_addend.
fn set_rela(b: &mut [u8], rela: usize, n: usize, field: usize, v: u32) {
    let at = rela + 12 * n + field;
    b[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

/// Relocations that can't be applied are reported with the file, section,
/// offset and symbol, all of them in one run.
#[test]
fn relocation_errors() {
    // Sanity: unpatched, it links.
    assert!(merlin_ld::link(&inputs("hello", &["hello.o"]), &hello_opts()).is_ok());

    // The HI20 moved past the end of .text.start (0x50 bytes): its LO12 then
    // has no HI20 at its auipc label.
    let errs = patch_hello(|b, rela| set_rela(b, rela, 0, 0, 0x1000));
    assert_eq!(
        errs,
        [
            "hello.o: R_RISCV_PCREL_HI20 at .text.start+0x1000 is outside section .text.start (size 0x50)",
            "hello.o: R_RISCV_PCREL_LO12_I at .text.start+0x4 refers to '.Lpcrel_hi0', which is not the \
             place of an R_RISCV_PCREL_HI20",
        ]
    );

    // A 4-byte field in the last 2 bytes of the section.
    let errs = patch_hello(|b, rela| set_rela(b, rela, 2, 0, 0x4e));
    assert_eq!(
        errs[0],
        "hello.o: relocation R_RISCV_PCREL_HI20 against 'message' at .text.start+0x4e patches 4 bytes, but \
         section .text.start ends 2 bytes after it"
    );

    // A type merlin-ld doesn't handle (GOT_HI20, for -fPIC code).
    let stack_top = symbol_index("hello/hello.o", "__stack_top");
    let errs = patch_hello(|b, rela| set_rela(b, rela, 0, 4, (stack_top << 8) | 20));
    assert_eq!(
        errs,
        [
            "hello.o: unsupported relocation R_RISCV_GOT_HI20 against '__stack_top' at .text.start+0x0",
            "hello.o: R_RISCV_PCREL_LO12_I at .text.start+0x4 refers to '.Lpcrel_hi0', which is not the \
             place of an R_RISCV_PCREL_HI20",
        ]
    );

    // A PCREL_LO12 pointing at the data symbol instead of the auipc label,
    // at an undefined symbol, or with an addend of its own.
    let message = symbol_index("hello/hello.o", "message");
    let errs = patch_hello(|b, rela| set_rela(b, rela, 1, 4, (message << 8) | 24));
    assert_eq!(
        errs,
        ["hello.o: R_RISCV_PCREL_LO12_I at .text.start+0x4 refers to 'message' in section .rodata, not to \
          the label of an auipc in .text.start"]
    );
    let errs = patch_hello(|b, rela| set_rela(b, rela, 1, 4, (stack_top << 8) | 25));
    assert_eq!(
        errs,
        ["hello.o: R_RISCV_PCREL_LO12_S at .text.start+0x4 refers to '__stack_top', not to the label of an \
          auipc in .text.start"]
    );
    let errs = patch_hello(|b, rela| set_rela(b, rela, 3, 8, 4));
    assert_eq!(
        errs,
        ["hello.o: R_RISCV_PCREL_LO12_I against '.Lpcrel_hi1' at .text.start+0xc has addend 4; a %pcrel_lo \
          takes the offset of its R_RISCV_PCREL_HI20 and must not have its own"]
    );
    // A PCREL_LO12 whose label is an auipc without a PCREL_HI20 on it.
    let errs = patch_hello(|b, rela| set_rela(b, rela, 2, 4, (message << 8) | 26));
    assert_eq!(
        errs,
        ["hello.o: R_RISCV_PCREL_LO12_I at .text.start+0xc refers to '.Lpcrel_hi1', which is not the place \
          of an R_RISCV_PCREL_HI20"]
    );

    // The relocations retargeted at .rodata, which is made NOBITS.
    let errs = patch_hello(|b, _| {
        let rodata = shdr(b, ".rodata");
        b[rodata + 4..rodata + 8].copy_from_slice(&8u32.to_le_bytes());
        let index = section_index(b, ".rodata");
        let rela = shdr(b, ".rela.text.start");
        b[rela + 28..rela + 32].copy_from_slice(&index.to_le_bytes());
    });
    assert_eq!(errs.len(), 4, "{errs:?}");
    assert_eq!(
        errs[0],
        "hello.o: R_RISCV_PCREL_HI20 at .rodata+0x0 patches a NOBITS section"
    );
}

/// Out of range and misaligned branches and jumps, from the relocs fixture
/// laid out differently.
#[test]
fn branch_and_jump_range_errors() {
    let objs = inputs("relocs", &["main.o", "lib.o", "relax.o"]);
    // .high 4 bytes too far, .low 4 bytes too low, and .text.branch (8 KiB)
    // between main.o's checks and lib.o.
    let errs = errors(merlin_ld::link(
        &objs,
        &script(
            "ENTRY(_start) SECTIONS { . = 0x7ffffffc; .low : { *(.low) } . = 0x80100000; \
             .text : { *(.text.start) *(.text.branch) *(.text*) } .rodata : { *(.rodata*) } \
             .data : { *(.data*) } .bss : { *(.bss*) } __stack_top = . + 4K; \
             . = 0x80200004; .high : { *(.high) } }",
        ),
    ));
    // lib_check's `bne a0, t0, fail` is 0x3c into lib.o's .text, 8280 bytes
    // after `fail` once .text.branch is in between; main.o's branch to the
    // weak (but defined) lib_inc is about as far the other way. Were it taken for
    // an undefined weak, it would silently target itself instead.
    assert_eq!(
        errs,
        [
            "main.o: relocation R_RISCV_JAL out of range for 'high_target' at .text.start+0x4: 1048576 is not \
             in [-1048576, 1048574]",
            "main.o: relocation R_RISCV_JAL out of range for 'low_target' at .text.start+0x8: -1048580 is not \
             in [-1048576, 1048574]",
            "main.o: relocation R_RISCV_BRANCH out of range for 'lib_inc' at .text.start+0x268: 8444 is not \
             in [-4096, 4094]",
            "lib.o: relocation R_RISCV_BRANCH out of range for 'fail' at .text+0x3c: -8280 is not in [-4096, \
             4094]",
        ]
    );

    // A jump to an odd offset (the addend made odd by hand).
    let mut main = std::fs::read(fixtures().join("relocs/main.o")).unwrap();
    let rela = u32_at(&main, shdr(&main, ".rela.text.start") + 16) as usize;
    // The first relocation of .text.start is `jal high_target` at +0x4.
    assert_eq!(u32_at(&main, rela), 4);
    main[rela + 8..rela + 12].copy_from_slice(&1u32.to_le_bytes());
    let objs = [
        Input {
            name: "main.o".into(),
            data: main,
        },
        input(&fixtures().join("relocs/lib.o")),
        input(&fixtures().join("relocs/relax.o")),
    ];
    let (_, opts) = case("relocs/link.ld");
    assert_eq!(
        errors(merlin_ld::link(&objs, &opts)),
        [
            "main.o: relocation R_RISCV_JAL against 'high_target' at .text.start+0x4 is misaligned: offset \
          1048573 is not a multiple of 2"
        ]
    );
}

/// A relocation patches only its own input section, even when another input
/// section follows it in the same output section (link.ld puts main.o's
/// .text right after its .text.start).
#[test]
fn relocation_stops_at_its_input_section() {
    let mut main = std::fs::read(fixtures().join("relocs/main.o")).unwrap();
    let size = u32_at(&main, shdr(&main, ".text.start") + 20);
    let rela = u32_at(&main, shdr(&main, ".rela.text.start") + 16) as usize;
    // `jal high_target` moved to the last 2 bytes of .text.start.
    main[rela..rela + 4].copy_from_slice(&(size - 2).to_le_bytes());
    let objs = [
        Input {
            name: "main.o".into(),
            data: main,
        },
        input(&fixtures().join("relocs/lib.o")),
        input(&fixtures().join("relocs/relax.o")),
    ];
    let (_, opts) = case("relocs/link.ld");
    let layout = merlin_ld::layout(&objs, &opts).unwrap();
    let text = layout.section(".text").unwrap();
    let next = text.inputs.iter().position(|i| i.addr == text.addr + size);
    assert!(next.is_some(), "another input section follows .text.start");
    assert_eq!(
        errors(merlin_ld::link(&objs, &opts)),
        [format!(
            "main.o: relocation R_RISCV_JAL against 'high_target' at .text.start+{:#x} patches 4 bytes, but \
             section .text.start ends 2 bytes after it",
            size - 2
        )]
    );
}

/// R_RISCV_32 must fit 32 bits; merlin-as objects carry it for `.word sym`.
#[test]
fn data_word_relocations() {
    let obj = |src: &str| Input {
        name: "w.o".into(),
        data: merlin_as::assemble_to_elf(src).unwrap(),
    };
    let src = ".globl _start\n.text\n_start:\n.word 0x0000100b\n.data\n\
               .word _start\n.word _start + 8\n.word _start - 0x80000000\n.word here\nhere:\n";
    let linked = merlin_ld::link(&[obj(src)], &Options::default()).unwrap();
    let data = linked.layout.section(".data").unwrap();
    let elf = grail::elf::parse(&linked.elf).unwrap();
    let seg = elf.segments.iter().find(|s| s.paddr == data.addr).unwrap();
    let words: Vec<u32> = seg.data[..16]
        .chunks(4)
        .map(|c| u32::from_le_bytes(c.try_into().unwrap()))
        .collect();
    assert_eq!(words, [0x8000_0000, 0x8000_0008, 0, data.addr + 16]);

    // _start is at 0x80000004: one more than 0xffffffff.
    let errs = errors(merlin_ld::link(
        &[obj(
            ".globl _start\n.text\n.word 0\n_start:\n.word 0x0000100b\n.data\n.word 0\n.word _start + 0x7ffffffc\n",
        )],
        &Options::default(),
    ));
    assert_eq!(
        errs,
        [
            "w.o: relocation R_RISCV_32 out of range for '_start' at .data+0x4: 4294967296 is not in \
          [-2147483648, 4294967295]"
        ]
    );
}

/// R_RISCV_ALIGN only makes sense with relaxation; R_RISCV_RELAX is ignored
/// (relocs/relax.o, above).
#[test]
fn align_needs_relaxation() {
    let errs = errors(merlin_ld::link(
        &inputs("rejects", &["align.o"]),
        &Options::default(),
    ));
    assert_eq!(
        errs,
        [
            "align.o: R_RISCV_ALIGN at .text+0x4 needs linker relaxation, which merlin-ld does not do; \
          assemble with -mno-relax"
        ]
    );
}

// ---------------------------------------------------------------------------
// Symbol errors.

#[test]
fn undefined_reference() {
    // hello.o needs __stack_top, which only its linker script defines.
    let errs = errors(merlin_ld::link(
        &inputs("hello", &["hello.o"]),
        &Options::default(),
    ));
    assert_eq!(errs, ["hello.o: undefined reference to '__stack_top'"]);
}

#[test]
fn undefined_references_from_several_objects() {
    let a = merlin_as::assemble_to_elf(".data\n.word foo\n.word bar\n.word foo\n").unwrap();
    let b = merlin_as::assemble_to_elf(".data\n.word bar\n").unwrap();
    let objs = [
        Input {
            name: "a.o".into(),
            data: a,
        },
        Input {
            name: "b.o".into(),
            data: b,
        },
    ];
    let errs = errors(merlin_ld::link(&objs, &Options::default()));
    assert_eq!(
        errs,
        [
            "a.o: undefined reference to 'foo'",
            "a.o: undefined reference to 'bar'",
            "b.o: undefined reference to 'bar'",
        ]
    );
}

#[test]
fn duplicate_symbol() {
    let objs = [
        input(&fixtures().join("norelocs/start.o")),
        Input {
            name: "again.o".into(),
            data: std::fs::read(fixtures().join("norelocs/start.o")).unwrap(),
        },
    ];
    let errs = errors(merlin_ld::link(&objs, &Options::default()));
    assert_eq!(
        errs,
        ["again.o: duplicate symbol '_start' (first defined in start.o)"]
    );
}

#[test]
fn duplicate_symbols_from_merlin_as_objects() {
    let src = ".globl shared\n.globl _start\n.text\n_start:\n.word 0x0000100b\n.data\nshared: .word 7\nlocal: .word 8\n";
    let objs: Vec<Input> = ["x.o", "y.o", "z.o"]
        .iter()
        .map(|n| Input {
            name: n.to_string(),
            data: merlin_as::assemble_to_elf(src).unwrap(),
        })
        .collect();
    let errs = errors(merlin_ld::link(&objs, &Options::default()));
    assert_eq!(
        errs,
        [
            "y.o: duplicate symbol 'shared' (first defined in x.o)",
            "y.o: duplicate symbol '_start' (first defined in x.o)",
            "z.o: duplicate symbol 'shared' (first defined in x.o)",
            "z.o: duplicate symbol '_start' (first defined in x.o)",
        ]
    );
}

/// Locals with the same name in different objects don't clash.
#[test]
fn locals_are_per_object() {
    let src = |v: u32| format!(".data\ncount: .word {v}\n");
    let mut objs: Vec<Input> = [1, 2]
        .iter()
        .map(|&v| Input {
            name: format!("{v}.o"),
            data: merlin_as::assemble_to_elf(&src(v)).unwrap(),
        })
        .collect();
    objs.push(input(&fixtures().join("norelocs/start.o")));
    let linked = merlin_ld::link(&objs, &Options::default()).unwrap();
    assert_eq!(linked.layout.symbol("count"), None, "locals are not global");
    let (m, r, _) = run(&linked.elf);
    assert_eq!(r, RunResult::Exited(0));
    let data = linked.layout.section(".data").unwrap();
    assert_eq!(ram(&m, data.addr, 8), &[1, 0, 0, 0, 2, 0, 0, 0]);
    // Both are in the output .symtab, as locals, at their own addresses.
    let ours = read_ours(&linked.elf);
    let counts: Vec<u32> = ours
        .locals
        .iter()
        .filter(|(n, _)| n == "count")
        .map(|&(_, v)| v)
        .collect();
    assert_eq!(counts, [data.addr, data.addr + 4]);
    assert!(!ours.globals.contains_key("count"));
}

#[test]
fn weak_symbols() {
    for order in [["data.o", "weak.o"], ["weak.o", "data.o"]] {
        let objs: Vec<Input> = order
            .iter()
            .map(|n| match *n {
                "weak.o" => input(&fixtures().join("symbols/weak.o")),
                n => input(&fixtures().join("norelocs").join(n)),
            })
            .collect();
        let l = merlin_ld::layout(&objs, &Options::default()).unwrap();
        let text = l.section(".text").unwrap();
        // data.o's strong `helper` (code) beats weak.o's weak one (data).
        assert!(
            (text.addr..text.addr + text.size).contains(&l.symbol("helper").unwrap()),
            "{order:?}"
        );
        let data = l.section(".data").unwrap();
        assert!((data.addr..data.addr + data.size).contains(&l.symbol("weak_only").unwrap()));
        // Weak undefined: no error, no definition, and relocations against it
        // see 0.
        assert_eq!(l.symbol("missing"), None);
        let w = l.objects.iter().position(|o| o.name == "weak.o").unwrap();
        let missing = l.objects[w]
            .symbols
            .iter()
            .position(|s| s.name == "missing")
            .unwrap();
        assert_eq!(l.reloc_symbol(w, missing), Ok(0));
        let helper = l.objects[w]
            .symbols
            .iter()
            .position(|s| s.name == "helper")
            .unwrap();
        assert_eq!(
            l.reloc_symbol(w, helper),
            Ok(l.symbol("helper").unwrap()),
            "the strong one"
        );
        // No _start among these: warn and fall back to .text.
        assert_eq!(l.entry, text.addr);
        assert_eq!(l.warnings.len(), 1);
    }
}

/// Symbols assigned by the script override definitions in objects (ld.lld's
/// behavior) and satisfy undefined references.
#[test]
fn script_symbols() {
    let objs = inputs("norelocs", &["start.o", "data.o"]);
    let l = merlin_ld::layout(
        &objs,
        &script(
            "SECTIONS { . = 0x80000000; .text : { *(.text*) } .rodata : { *(.rodata*) } \
             .data : { *(.data*) } .bss : { *(.bss*) } \
             helper = 0x1234; top = . + 16; end_of_text = ADDR_UNUSED; }",
        ),
    );
    let errs: Vec<String> = l.unwrap_err().into_iter().map(|e| e.0).collect();
    assert_eq!(errs, ["test.ld:1: undefined symbol 'ADDR_UNUSED' in expression"]);

    let l = merlin_ld::layout(
        &objs,
        &script(
            "SECTIONS { . = 0x80000000; .text : { *(.text*) } t_end = .; .rodata : { *(.rodata*) } \
             .data : { *(.data*) } .bss : { *(.bss*) } \
             helper = 0x1234; top = . + 16; alias = table + 4; }",
        ),
    )
    .unwrap();
    assert_eq!(l.symbol("helper"), Some(0x1234));
    let bss = l.section(".bss").unwrap();
    assert_eq!(l.symbol("top"), Some(bss.addr + bss.size + 16));
    assert_eq!(l.symbol("alias"), Some(l.symbol("table").unwrap() + 4));
    let text = l.section(".text").unwrap();
    assert_eq!(l.symbol("t_end"), Some(text.addr + text.size));
}

// ---------------------------------------------------------------------------
// Linker-script layout.

#[test]
fn script_assignments_inside_sections() {
    let l = merlin_ld::layout(
        &norelocs(),
        &script(
            "ENTRY(helper)
             SECTIONS {
               . = 0x80001000;
               .text : { *(.text.start) . = ALIGN(64); mid = .; *(.text*) }
               . += 0x100;
               .rodata : { *(.rodata*) }
               . = ALIGN(0x1000);
               .data : { __data_start = .; *(.data*) . += 3; __data_end = .; }
               .bss : { *(.bss*) . = ALIGN(16); }
               .stack : { . += 4K; }
               __stack_top = .;
             }",
        ),
    )
    .unwrap();
    let s = |n: &str| l.section(n).unwrap();
    assert_eq!(s(".text").addr, 0x8000_1000);
    assert_eq!(l.symbol("mid"), Some(0x8000_1040));
    assert_eq!(l.symbol("helper"), Some(0x8000_1040));
    assert_eq!(l.entry, 0x8000_1040);
    assert_eq!(l.entry_symbol.as_deref(), Some("helper"));
    assert_eq!(s(".rodata").addr, s(".text").addr + s(".text").size + 0x100);
    assert_eq!(s(".data").addr, 0x8000_2000);
    assert_eq!(l.symbol("__data_start"), Some(0x8000_2000));
    assert_eq!(l.symbol("__data_end"), Some(0x8000_2000 + 0x24 + 3));
    assert_eq!(s(".data").size, 0x27);
    assert_eq!(s(".bss").addr, 0x8000_2030);
    assert_eq!(s(".bss").size, 0x120);
    // Only `. +=`: PROGBITS with the previous section's flags, as in ld.lld
    // (fixtures/layouts/inside.ld checks this script against ld.lld).
    assert!(!s(".stack").nobits);
    assert_eq!(s(".stack").flags, s(".bss").flags);
    assert_eq!(s(".stack").size, 4096);
    assert_eq!(l.symbol("__stack_top"), Some(s(".stack").addr + 4096));
    let linked = merlin_ld::link(
        &norelocs(),
        &script("SECTIONS { . = 0x80000000; .text : { *(.text*) } .x : { *(.rodata*) *(.data*) *(.bss*) } }"),
    )
    .unwrap();
    let x = linked.layout.section(".x").unwrap();
    assert!(
        !x.nobits,
        "mixed PROGBITS/NOBITS output section has file contents"
    );
    let (_, r, out) = run(&linked.elf);
    assert_eq!((r, out.as_str()), (RunResult::Exited(0), "OK\n"));
}

/// ld.lld's rule: an output section is dropped only if nothing matched it and
/// it has no assignments. (fixtures/layouts/empty.ld checks the same cases
/// against ld.lld.)
#[test]
fn empty_output_sections() {
    let l = merlin_ld::layout(
        &inputs("norelocs", &["start.o"]),
        &script(
            "SECTIONS { . = 0x80000000; .text : { *(.text*) } .data : { d = .; *(.data*) } \
             .none : { *(.none) } .sym : { e = .; } .bss : { *(.bss*) } }",
        ),
    )
    .unwrap();
    let names: Vec<&str> = l.sections.iter().map(|s| s.name.as_str()).collect();
    // start.o has no .data or .bss at all: .data stays for its assignment, .bss goes.
    assert_eq!(names, [".text", ".data", ".sym"]);
    for s in &l.sections[1..] {
        assert_eq!((s.addr, s.size), (0x8000_0020, 0), "{}", s.name);
    }
    assert_eq!(l.symbol("d"), Some(0x8000_0020));
    assert_eq!(l.symbol("e"), Some(0x8000_0020));
    let sym = l.section(".sym").unwrap();
    assert!(!sym.nobits);
    assert_eq!(sym.flags, l.section(".data").unwrap().flags);
    // Empty sections don't get PT_LOADs of their own.
    let linked = merlin_ld::link(
        &inputs("norelocs", &["start.o"]),
        &script("SECTIONS { . = 0x80000000; .text : { *(.text*) } .data : { d = .; } }"),
    )
    .unwrap();
    assert_eq!(read_ours(&linked.elf).loads.len(), 1);
}

#[test]
fn entry_option() {
    let mut opts = norelocs_opts();
    opts.entry = Some("helper".into());
    let l = merlin_ld::layout(&norelocs(), &opts).unwrap();
    assert_eq!(l.entry, l.symbol("helper").unwrap());
    opts.entry = Some("0x80000010".into());
    assert_eq!(merlin_ld::layout(&norelocs(), &opts).unwrap().entry, 0x8000_0010);
    opts.entry = Some("nowhere".into());
    assert_eq!(
        errors(merlin_ld::layout(&norelocs(), &opts)),
        ["entry symbol 'nowhere' is not defined"]
    );
}

#[test]
fn layout_errors() {
    let cases: &[(&str, &str)] = &[
        (
            "SECTIONS { .text : { *(.text*) } .data : { *(.data*) } .bss : { *(.bss*) } }",
            "data.o: section '.rodata' is not placed by linker script test.ld",
        ),
        (
            "MEMORY { RAM : ORIGIN = 0x80000000, LENGTH = 0x40 } SECTIONS { .text : { *(.text*) } > RAM \
             .rodata : { *(.rodata*) } > RAM .data : { *(.data*) } > RAM .bss : { *(.bss*) } > RAM }",
            "test.ld:1: section .data overflows MEMORY region RAM by 36 bytes",
        ),
        (
            "MEMORY { RAM : ORIGIN = 0x80000000, LENGTH = 1M } SECTIONS { .text : { *(.text*) } \
             . = ALIGN(0x100); .rodata : { *(.rodata*) } .data : { *(.data*) } .bss : { *(.bss*) } }",
            "test.ld:1: '.' is moved before output section .rodata, which is placed at the next free address of \
             MEMORY region RAM instead; move the assignment inside the section",
        ),
        (
            "SECTIONS { . = 0x80000000; . = 0x1000; }",
            "test.ld:1: cannot move the location counter backward (from 0x80000000 to 0x1000)",
        ),
        (
            "SECTIONS { . = 0xfffffff0; .text : { *(.text*) } }",
            "test.ld:1: section .text extends past the 4 GiB address space",
        ),
        (
            // Exactly 4 GiB long: must not wrap to an empty section.
            "SECTIONS { . = 0; .s : { . += 0x100000000; } }",
            "test.ld:1: section .s extends past the 4 GiB address space",
        ),
        (
            "SECTIONS { . = 0x80000000; . += 0x7fffffffffffffff; }",
            "test.ld:1: arithmetic overflow",
        ),
        ("SECTIONS { x = 0x100000000; }", "test.ld:1: value 0x100000000 of 'x' does not fit in 32 bits"),
        ("SECTIONS { x = ORIGIN(RAM); }", "test.ld:1: unknown memory region 'RAM'"),
        ("SECTIONS { . = ALIGN(0); }", "test.ld:1: ALIGN(0): alignment must be positive"),
        ("SECTIONS { .text : { KEEP(*(.text)) } }", "test.ld:1: only '*(...)' input section lists are supported"),
    ];
    for (src, want) in cases {
        let errs = errors(merlin_ld::layout(&norelocs(), &script(src)));
        assert!(
            errs.iter().any(|e| e.starts_with(want)),
            "{src}\n got {errs:?}\nwant {want}"
        );
    }
}

#[test]
fn default_layout_rejects_unknown_sections() {
    let o = merlin_as::assemble_to_elf(".section .mystuff\n.word 1\n").unwrap();
    let errs = errors(merlin_ld::link(
        &[Input {
            name: "m.o".into(),
            data: o,
        }],
        &Options::default(),
    ));
    assert_eq!(
        errs,
        ["m.o: section '.mystuff' is not placed by the default layout (use a linker script with -T)"]
    );
    // Empty, but a global lives there: it would have no address.
    let o = merlin_as::assemble_to_elf(".globl marker\n.section .mystuff\nmarker:\n").unwrap();
    let errs = errors(merlin_ld::link(
        &[Input {
            name: "m.o".into(),
            data: o.clone(),
        }],
        &Options::default(),
    ));
    assert_eq!(
        errs,
        ["m.o: section '.mystuff' is not placed by the default layout (use a linker script with -T)"]
    );
    // An empty section with only locals (or nothing) is just left out.
    let o = merlin_as::assemble_to_elf(".section .mystuff\nlocal_marker:\n").unwrap();
    let mut objs = vec![Input {
        name: "m.o".into(),
        data: o,
    }];
    objs.push(input(&fixtures().join("norelocs/start.o")));
    merlin_ld::link(&objs, &Options::default()).unwrap();
}

// ---------------------------------------------------------------------------
// Bad inputs.

#[test]
fn rejects_bad_objects() {
    let good = std::fs::read(fixtures().join("norelocs/start.o")).unwrap();
    let patched = |off: usize, bytes: &[u8]| {
        let mut b = good.clone();
        b[off..off + bytes.len()].copy_from_slice(bytes);
        b
    };
    let reject = |name: &str| std::fs::read(fixtures().join("rejects").join(name)).unwrap();
    let cases: Vec<(Vec<u8>, &str)> = vec![
        (b"hello".to_vec(), "x.o: not an ELF file"),
        (patched(4, &[2]), "x.o: not a 32-bit ELF file"),
        (patched(5, &[2]), "x.o: not a little-endian ELF file"),
        (patched(16, &[2, 0]), "x.o: an executable (e_type 2)"),
        (patched(18, &[62, 0]), "x.o: e_machine is 62, not RISC-V"),
        (
            patched(36, &[1, 0, 0, 0]),
            "x.o: built with compressed instructions",
        ),
        (patched(36, &[4, 0, 0, 0]), "x.o: e_flags 0x4 not supported"),
        (good[..good.len() - 8].to_vec(), "x.o: truncated file"),
        (
            std::fs::read(repo_root().join("examples/hello/hello.elf")).unwrap(),
            "x.o: an executable (e_type 2), expected a relocatable object (ET_REL)",
        ),
        (
            reject("common.o"),
            "x.o: common symbol 'foo' is not supported (define it in .bss, or compile with -fno-common)",
        ),
        (
            reject("comdat.o"),
            "x.o: section group '.group' (COMDAT) is not supported",
        ),
        (
            // A member of a group whose SHT_GROUP section is gone.
            {
                let mut b = reject("comdat.o");
                let g = shdr(&b, ".group");
                b[g + 4..g + 8].copy_from_slice(&1u32.to_le_bytes());
                b
            },
            "x.o: section '.text.f' is in a section group (COMDAT), not supported",
        ),
        (
            reject("tls.o"),
            "x.o: thread-local section '.tdata' is not supported",
        ),
        (
            // STT_TLS on a symbol in an ordinary section.
            {
                let mut b = good.clone();
                let s = symbol_entry(&b, "_start");
                b[s + 12] = (1 << 4) | 6;
                b
            },
            "x.o: thread-local symbol '_start' is not supported",
        ),
        (
            {
                let mut b = std::fs::read(fixtures().join("hello/hello.o")).unwrap();
                let r = shdr(&b, ".rela.text.start");
                b[r + 4..r + 8].copy_from_slice(&9u32.to_le_bytes());
                b
            },
            "x.o: section '.rela.text.start' is SHT_REL; RISC-V objects use SHT_RELA",
        ),
        (
            reject("nonalloc.o"),
            "x.o: global symbol 'g' is defined in section '.note.camelot', which is not loaded",
        ),
    ];
    for (data, want) in cases {
        let errs = errors(merlin_ld::link(
            &[Input {
                name: "x.o".into(),
                data,
            }],
            &Options::default(),
        ));
        assert_eq!(errs.len(), 1);
        assert!(errs[0].starts_with(want), "got {:?}, want {want:?}", errs[0]);
    }
    assert_eq!(
        errors(merlin_ld::link(&[], &Options::default())),
        ["no input files"]
    );
}

#[test]
fn map_file() {
    let linked = merlin_ld::link(&norelocs(), &norelocs_opts()).unwrap();
    let map = merlin_ld::map::map(&linked.layout);
    for want in [
        "Entry point: 0x80000000 (_start)",
        ".text                0x80000000 0x00000028 4",
        "    0x80000020 0x00000008 data.o(.text.helper)",
        "        0x80000020 helper",
        ".bss                 0x80000070 0x00000118 16  (nobits)",
        "    0x80001190 __stack_top",
    ] {
        assert!(map.contains(want), "map lacks {want:?}:\n{map}");
    }
    assert!(!map.contains("$x"), "mapping symbols are noise");
}

// ---------------------------------------------------------------------------
// Segments.

fn loads(script_text: &str) -> Vec<Load> {
    let linked = merlin_ld::link(&norelocs(), &script(script_text)).unwrap();
    // Grail must accept every shape.
    let (_, r, out) = run(&linked.elf);
    assert_eq!((r, out.as_str()), (RunResult::Exited(0), "OK\n"));
    read_ours(&linked.elf).loads
}

/// One PT_LOAD per run of sections with the same permissions; gaps and NOBITS
/// sections inside a run are zeros in the file. (Gaps of a page and NOBITS
/// before PROGBITS are checked against ld.lld in fixtures/layouts.)
#[test]
fn segments_group_by_permissions() {
    let l = loads(
        "SECTIONS { . = 0x80000000; .text : { *(.text*) } .rodata : { *(.rodata*) } \
         .data : { *(.data) } . += 0x10; .data2 : { *(.data.*) } .bss : { *(.bss*) } }",
    );
    assert_eq!(l.len(), 3);
    let rw = &l[2];
    assert_eq!(rw.flags, "RW");
    // .data at 0x40 (8 bytes), 0x10 gap, .data.more aligned to 16 at 0x60 (0x14 bytes).
    assert_eq!((rw.vaddr, rw.filesz), (0x8000_0040, 0x34));
    // Same permissions again after a different run: a new segment.
    let l = loads(
        "SECTIONS { . = 0x80000000; .text : { *(.text*) } .data : { *(.data) } \
         .rodata : { *(.rodata*) } .data2 : { *(.data.* .bss*) } }",
    );
    let flags: Vec<&str> = l.iter().map(|s| s.flags.as_str()).collect();
    assert_eq!(flags, ["RE", "RW", "R", "RW"]);
    for s in &l {
        assert_eq!(s.offset % 0x1000, s.vaddr % 0x1000, "offset congruent to address");
    }
    // The NOBITS bytes inside a segment read as zeros once loaded.
    let linked = merlin_ld::link(
        &norelocs(),
        &script(
            "SECTIONS { . = 0x80000000; .text : { *(.text*) } .bss : { *(.bss*) } \
             .data : { *(.data*) } .rodata : { *(.rodata*) } }",
        ),
    )
    .unwrap();
    let (m, _, _) = run(&linked.elf);
    check_norelocs_memory(&m, &linked.layout);
}

#[test]
fn library_members_are_linked_only_when_needed() {
    // multiobj's main.o is the program; its library holds a second main.o
    // (whose _start would be a duplicate), norelocs' more.o (which nothing
    // needs) and print.o (which main.o calls): only print.o is linked, and
    // the result is the plain link of main.o and print.o.
    let main = input(&fixtures().join("multiobj/main.o"));
    let print = input(&fixtures().join("multiobj/print.o"));
    let library = [
        Input {
            name: "again.o".into(),
            data: main.data.clone(),
        },
        input(&fixtures().join("norelocs/more.o")),
        print.clone(),
    ];
    let program = [main.clone()];
    assert_eq!(merlin_ld::library::needed(&program, &library).unwrap(), [2]);
    let opts = script(&std::fs::read_to_string(fixtures().join("multiobj/link.ld")).unwrap());
    let linked = merlin_ld::link_with_library(&program, &library, &opts).unwrap();
    let plain = merlin_ld::link(&[main, print], &opts).unwrap();
    assert_eq!(linked.elf, plain.elf);
    // Without the member, the reference is undefined.
    let errs = errors(merlin_ld::link_with_library(&program, &library[..2], &opts));
    assert!(
        errs.iter().any(|e| e.contains("undefined reference to 'puts'")),
        "{errs:?}"
    );
}

// ---------------------------------------------------------------------------
// Command line.

#[test]
fn command_line() {
    let exe = env!("CARGO_BIN_EXE_merlin-ld");
    let dir = std::env::temp_dir().join(format!("merlin-ld-cli-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let out = dir.join("out.elf");
    let map = dir.join("out.map");
    let fx = fixtures().join("norelocs");
    let status = std::process::Command::new(exe)
        .arg(fx.join("start.o"))
        .arg(fx.join("data.o"))
        .arg(fx.join("more.o"))
        .arg("-T")
        .arg(fx.join("link.ld"))
        .arg("-o")
        .arg(&out)
        .arg(format!("--Map={}", map.display()))
        .arg("-e")
        .arg("_start")
        .status()
        .unwrap();
    assert!(status.success());
    let elf = std::fs::read(&out).unwrap();
    assert_eq!(read_ours(&elf).loads, expected("norelocs/link").loads);
    assert!(std::fs::read_to_string(&map).unwrap().contains("__stack_top"));

    let o = std::process::Command::new(exe)
        .arg(fixtures().join("hello/hello.o"))
        .arg("-o")
        .arg(&out)
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&o.stderr);
    assert!(
        stderr.starts_with("merlin-ld: error: ")
            && stderr.contains("hello.o: undefined reference to '__stack_top'"),
        "{stderr}"
    );

    // A library member nobody needs is left out: a second start.o would be
    // a duplicate _start, but between --start-lib and --end-lib it is only
    // linked if something needs a symbol it defines.
    let status = std::process::Command::new(exe)
        .arg(fx.join("start.o"))
        .arg("--start-lib")
        .arg(fx.join("start.o"))
        .arg("--end-lib")
        .arg(fx.join("data.o"))
        .arg(fx.join("more.o"))
        .arg("-T")
        .arg(fx.join("link.ld"))
        .arg("-o")
        .arg(&out)
        .status()
        .unwrap();
    assert!(status.success());
    assert_eq!(
        read_ours(&std::fs::read(&out).unwrap()).loads,
        expected("norelocs/link").loads
    );
    for bad in [
        &["--end-lib"][..],
        &["--start-lib", "--start-lib"],
        &["--start-lib", "x.o"],
        &["--start-lib", "x.o", "--end-lib"],
    ] {
        let o = std::process::Command::new(exe).args(bad).output().unwrap();
        assert_eq!(o.status.code(), Some(2), "{bad:?}");
    }

    // A response file: the same link, its arguments read from a file (any
    // white space between them), mixed with ones on the command line.
    let args = dir.join("link.args");
    let words = [
        fx.join("start.o"),
        fx.join("data.o"),
        "-T".into(),
        fx.join("link.ld"),
    ];
    let words: Vec<String> = words.iter().map(|w| w.display().to_string()).collect();
    std::fs::write(
        &args,
        format!("{}\n\t{}  {}\n", words[0], words[1], words[2..].join(" ")),
    )
    .unwrap();
    let out2 = dir.join("out2.elf");
    let status = std::process::Command::new(exe)
        .arg(format!("@{}", args.display()))
        .arg(fx.join("more.o"))
        .arg("-o")
        .arg(&out2)
        .status()
        .unwrap();
    assert!(status.success());
    let first = std::process::Command::new(exe)
        .args([fx.join("start.o"), fx.join("data.o"), fx.join("more.o")])
        .arg("-T")
        .arg(fx.join("link.ld"))
        .arg("-o")
        .arg(&out)
        .status()
        .unwrap();
    assert!(first.success());
    assert_eq!(std::fs::read(&out2).unwrap(), std::fs::read(&out).unwrap());
    std::fs::write(&args, "@other.args\n").unwrap();
    let o = std::process::Command::new(exe)
        .arg(format!("@{}", args.display()))
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&o.stderr).contains("cannot name another"));
    let o = std::process::Command::new(exe)
        .arg("@no-such-file")
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(2));

    let o = std::process::Command::new(exe).arg("--bogus").output().unwrap();
    assert_eq!(o.status.code(), Some(2));
    let o = std::process::Command::new(exe).output().unwrap();
    assert_eq!(o.status.code(), Some(2));
    let _ = std::fs::remove_dir_all(&dir);
}

/// Two objects with line tables (`.file`/`.loc`), as `merlin -g` makes them,
/// and the same two without.
fn with_and_without_lines() -> (Vec<Input>, Vec<Input>) {
    let a = "\
        .file 0 \"src\" \"a.c\"
        .text
        .globl _start
_start:
        .loc 0 3 0
        call f
        .loc 0 4 5 prologue_end
        j _start
        .data
        .word 7
";
    let b = "\
        .file 0 \"src\" \"b.c\"
        .file 1 \"src\" \"include/b.h\"
        .text
        nop
        .globl f
f:
        .loc 0 10 0
        addi a0, a0, 1
        .loc 1 2 0
        ret
";
    let strip = |s: &str| -> String {
        s.lines()
            .filter(|l| !l.trim_start().starts_with(".file") && !l.trim_start().starts_with(".loc"))
            .map(|l| format!("{l}\n"))
            .collect()
    };
    let objs = |srcs: [&str; 2]| -> Vec<Input> {
        srcs.iter()
            .zip(["a.o", "b.o"])
            .map(|(s, n)| Input {
                name: n.into(),
                data: merlin_as::assemble_to_elf(s).unwrap(),
            })
            .collect()
    };
    (objs([a, b]), objs([&strip(a), &strip(b)]))
}

#[test]
fn line_tables_are_kept_and_relocated() {
    let (debug, plain) = with_and_without_lines();
    let opts = Options::default();
    let linked = merlin_ld::link(&debug, &opts).unwrap();
    let without = merlin_ld::link(&plain, &opts).unwrap();
    let elf = elf32::Elf::parse(&linked.elf).unwrap();
    let names: Vec<&str> = elf.sections.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "",
            ".text",
            ".data",
            ".symtab",
            ".strtab",
            ".debug_line",
            ".debug_line_str",
            ".shstrtab"
        ]
    );
    // Not loaded: the segments are the same as without the line tables.
    let segments = |file: &[u8]| -> Vec<(u32, Vec<u8>)> {
        let e = elf32::Elf::parse(file).unwrap();
        e.segments
            .iter()
            .map(|p| (p.vaddr, e.segment_data(p).unwrap().to_vec()))
            .collect()
    };
    assert_eq!(segments(&linked.elf), segments(&without.elf));
    // One table per object, at the code's final addresses; the files' paths
    // made from each table's own strings (the line_strp offsets relocated).
    let text = elf.sections.iter().find(|s| s.name == ".text").unwrap().addr;
    let tables = elf.line_tables().unwrap();
    let rows: Vec<Vec<(u32, &str, u32, bool)>> = tables
        .iter()
        .map(|t| {
            assert_eq!(t.sequences.len(), 1);
            t.sequences[0]
                .rows
                .iter()
                .map(|r| (r.addr - text, t.path(r.file), r.line, r.end_sequence))
                .collect()
        })
        .collect();
    assert_eq!(
        rows,
        [
            vec![
                (0, "src/a.c", 3, false),
                (8, "src/a.c", 4, false),
                (12, "src/a.c", 4, true)
            ],
            vec![
                (16, "src/b.c", 10, false),
                (20, "src/include/b.h", 2, false),
                (24, "src/include/b.h", 2, true)
            ],
        ]
    );
    // Stripped, it is the link without them, byte for byte; stripping that
    // changes nothing.
    assert_eq!(merlin_ld::strip_debug(&linked.elf).unwrap(), without.elf);
    assert_eq!(merlin_ld::strip_debug(&without.elf).unwrap(), without.elf);
}

#[test]
fn debug_information_of_discarded_code_points_nowhere() {
    // The code of `gone` is thrown away: its sequence's address becomes 0.
    let src = "        .file 0 \"src\" \"a.c\"
        .text
        .globl _start
_start:
        .loc 0 1 0
        nop
        .section .text.gone
gone:
        .loc 0 7 0
        nop
";
    let obj = Input {
        name: "a.o".into(),
        data: merlin_as::assemble_to_elf(src).unwrap(),
    };
    let opts = script("SECTIONS { . = 0x1000; .text : { *(.text) } /DISCARD/ : { *(.text.gone) } }");
    let linked = merlin_ld::link(&[obj], &opts).unwrap();
    let elf = elf32::Elf::parse(&linked.elf).unwrap();
    let seqs = &elf.line_tables().unwrap()[0].sequences;
    let starts: Vec<(u32, u32)> = seqs.iter().map(|s| (s.rows[0].addr, s.rows[0].line)).collect();
    assert_eq!(starts, [(0x1000, 1), (0, 7)]);
}
