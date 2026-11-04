//! Helpers shared by the end-to-end tests of both fixture sets: `link.rs`
//! (RV32, `tests/fixtures`) and `rv64.rs` (RV64, `tests/fixtures/rv64`),
//! both made by `cargo xtask ld-fixtures` (crates/xtask/src/ld_fixtures.rs).
//!
//! A set is named by its directory under `tests/fixtures` ([`RV32`],
//! [`RV64`]); a fixture by its path in the set, `norelocs/link`. Our
//! executables are read with the elf crate, which reads both classes, and
//! ld.lld's through its `llvm-readelf -S -l -s` listing (`.expected.txt`),
//! whose layout is the same for both but for the width of the numbers.

// Each test crate uses its own part of these.
#![allow(dead_code)]

use grail::{Machine, RunResult};
use merlin_ld::{Input, Options, ScriptSource};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// The RV32 fixture set: `tests/fixtures` itself.
pub const RV32: &str = "";
/// The RV64 fixture set: `tests/fixtures/rv64`.
pub const RV64: &str = "rv64";

pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The directory of a fixture set.
pub fn fixtures(set: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(set)
}

pub fn input(path: &Path) -> Input {
    Input {
        name: path.file_name().unwrap().to_string_lossy().into_owned(),
        data: std::fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display())),
    }
}

/// The objects `names` of a set's fixture directory `dir`.
pub fn inputs(set: &str, dir: &str, names: &[&str]) -> Vec<Input> {
    names
        .iter()
        .map(|n| input(&fixtures(set).join(dir).join(n)))
        .collect()
}

pub fn script_file(path: &Path) -> Option<ScriptSource> {
    Some(ScriptSource {
        name: path.file_name().unwrap().to_string_lossy().into_owned(),
        text: std::fs::read_to_string(path).unwrap(),
    })
}

pub fn script(text: &str) -> Options {
    Options {
        script: Some(ScriptSource {
            name: "test.ld".into(),
            text: text.into(),
        }),
        entry: None,
    }
}

pub fn errors(r: Result<impl std::fmt::Debug, Vec<merlin_ld::Error>>) -> Vec<String> {
    r.expect_err("link should fail")
        .into_iter()
        .map(|e| e.0)
        .collect()
}

/// The objects a fixture script names in its `/* objects: ... */` comment
/// (paths relative to the set's directory), and the script itself.
pub fn case(set: &str, script: &str) -> (Vec<Input>, Options) {
    let path = fixtures(set).join(script);
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
            .map(|o| input(&fixtures(set).join(o)))
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

/// The relocation types a fixture object carries, by name.
pub fn reloc_types(set: &str, path: &str) -> HashMap<String, usize> {
    let data = std::fs::read(fixtures(set).join(path)).unwrap();
    let obj = merlin_ld::object::parse(path, &data).unwrap();
    let mut n = HashMap::new();
    for s in &obj.sections {
        for r in &s.relocs {
            *n.entry(merlin_ld::reloc::name(r.ty)).or_insert(0) += 1;
        }
    }
    n
}

// ---------------------------------------------------------------------------
// Reading `llvm-readelf -S -l -s` output (the committed expectations).

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sect {
    pub name: String,
    pub ty: String,
    pub addr: u64,
    pub size: u64,
    pub flags: String,
    pub align: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Load {
    pub offset: u64,
    pub vaddr: u64,
    pub paddr: u64,
    pub filesz: u64,
    pub memsz: u64,
    pub flags: String,
    pub align: u64,
}

#[derive(Debug, Default)]
pub struct Readelf {
    pub entry: u64,
    /// Allocated sections only.
    pub sections: Vec<Sect>,
    pub loads: Vec<Load>,
    /// The `PT_PHDR`, when the headers are loaded (write.rs, "The headers").
    pub phdr: Option<Load>,
    /// Global and weak symbols.
    pub globals: HashMap<String, u64>,
    /// Local symbols (only read from our own output).
    pub locals: Vec<(String, u64)>,
}

/// A hex number of the listing, 8 digits for ELF32 or 16 for ELF64
/// (`0xffffffff80000000`).
pub fn hex(s: &str) -> u64 {
    u64::from_str_radix(s.trim_start_matches("0x"), 16).unwrap()
}

pub fn parse_readelf(text: &str) -> Readelf {
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
        } else if t.first() == Some(&"LOAD") || t.first() == Some(&"PHDR") {
            let p = Load {
                offset: hex(t[1]),
                vaddr: hex(t[2]),
                paddr: hex(t[3]),
                filesz: hex(t[4]),
                memsz: hex(t[5]),
                flags: t[6..t.len() - 1].concat(),
                align: hex(t[t.len() - 1]),
            };
            if t[0] == "LOAD" {
                r.loads.push(p);
            } else {
                r.phdr = Some(p);
            }
        } else if t.len() == 8 && (t[4] == "GLOBAL" || t[4] == "WEAK") && t[6] != "UND" {
            // (ld.lld lists undefined weak symbols, merlin-ld leaves them out.)
            r.globals.insert(t[7].into(), hex(t[1]));
        }
    }
    r
}

/// `<set>/<name>.expected.txt`, e.g. `expected(RV32, "norelocs/link")`.
pub fn expected(set: &str, name: &str) -> Readelf {
    let path = fixtures(set).join(format!("{name}.expected.txt"));
    parse_readelf(&std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display())))
}

// ---------------------------------------------------------------------------
// Reading our own ELF output the same way, with the elf crate (either class).

/// `R E`-style permissions, as the listing prints them (without the spaces).
fn segment_flags(f: u32) -> String {
    [(elf::PF_R, "R"), (elf::PF_W, "W"), (elf::PF_X, "E")]
        .iter()
        .filter(|(b, _)| f & b != 0)
        .map(|(_, s)| *s)
        .collect()
}

/// `WAX`-style section flags, as the listing prints them.
fn section_flags(f: u64) -> String {
    [
        (elf::SHF_WRITE, "W"),
        (elf::SHF_ALLOC, "A"),
        (elf::SHF_EXECINSTR, "X"),
    ]
    .iter()
    .filter(|(b, _)| f & b != 0)
    .map(|(_, s)| *s)
    .collect()
}

pub fn read_ours(file: &[u8]) -> Readelf {
    let e = elf::Elf::parse(file).unwrap();
    assert_eq!(e.header.ty, elf::ET_EXEC, "ET_EXEC");
    assert_eq!(e.header.machine, elf::EM_RISCV, "EM_RISCV");
    let mut r = Readelf {
        entry: e.header.entry,
        ..Default::default()
    };
    for (i, p) in e.segments.iter().enumerate() {
        let load = Load {
            offset: p.offset,
            vaddr: p.vaddr,
            paddr: p.paddr,
            filesz: p.filesz,
            memsz: p.memsz,
            flags: segment_flags(p.flags),
            align: p.align,
        };
        match p.ty {
            elf::PT_LOAD => r.loads.push(load),
            // Only first, and only for loaded headers.
            elf::PT_PHDR if i == 0 => r.phdr = Some(load),
            t => panic!("program header {i} of type {t:#x}: only PT_LOAD, and PT_PHDR first"),
        }
    }
    for s in e.sections.iter().skip(1) {
        if s.ty == elf::SHT_SYMTAB {
            for sym in e.symbols_in(s).unwrap().into_iter().skip(1) {
                if sym.bind() != elf::STB_LOCAL {
                    r.globals.insert(sym.name, sym.value);
                } else {
                    r.locals.push((sym.name, sym.value));
                }
            }
        }
        if !s.is_alloc() {
            continue;
        }
        r.sections.push(Sect {
            name: s.name.clone(),
            ty: match s.ty {
                elf::SHT_PROGBITS => "PROGBITS".into(),
                elf::SHT_NOBITS => "NOBITS".into(),
                t => format!("type {t}"),
            },
            addr: s.addr,
            size: s.size,
            flags: section_flags(s.flags),
            align: s.align.try_into().unwrap(),
        });
    }
    r
}

/// `(vaddr, memsz, file bytes)` of every PT_LOAD of an executable.
pub fn load_images(file: &[u8]) -> Vec<(u64, u64, Vec<u8>)> {
    let e = elf::Elf::parse(file).unwrap();
    e.segments
        .iter()
        .filter(|p| p.ty == elf::PT_LOAD)
        .map(|p| (p.vaddr, p.memsz, e.segment_data(p).unwrap().to_vec()))
        .collect()
}

// ---------------------------------------------------------------------------
// Comparing a link with ld.lld's and running it.

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
pub fn assert_same_disassembly(set: &str, name: &str, linked: &merlin_ld::Linked) {
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
    let path = fixtures(set).join(format!("{name}.objdump.txt"));
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

/// Links fixture script `<set>/name.ld` with the objects it lists, checks
/// the result against ld.lld's (`name.expected.txt`, `name.lld.elf`,
/// `name.objdump.txt`) and runs it in Grail, which must print `output` and
/// exit with `exit`.
pub fn assert_matches_ld_lld_exit(set: &str, name: &str, output: &str, exit: u32) -> merlin_ld::Linked {
    let (objs, opts) = case(set, &format!("{name}.ld"));
    let linked = merlin_ld::link(&objs, &opts).unwrap_or_else(|e| panic!("{name}: {e:?}"));
    assert!(
        linked.layout.warnings.is_empty(),
        "{name}: {:?}",
        linked.layout.warnings
    );
    let ours = read_ours(&linked.elf);
    let lld = expected(set, name);
    assert_eq!(ours.entry, lld.entry, "{name}: entry");
    assert_eq!(ours.sections, lld.sections, "{name}: sections");
    assert_eq!(ours.loads, lld.loads, "{name}: PT_LOADs");
    assert_eq!(ours.phdr, lld.phdr, "{name}: PT_PHDR");
    assert_eq!(ours.globals, lld.globals, "{name}: global symbols");

    // Byte for byte: every loaded byte, relocated code and data included,
    // but for the headers when they are loaded (the first segment's first
    // bytes, up to the end of the program header table's room): each
    // linker's own ELF header and program headers.
    let headers = lld.phdr.as_ref().map_or(0, |p| (p.offset + p.filesz) as usize);
    let lld_elf = std::fs::read(fixtures(set).join(format!("{name}.lld.elf"))).unwrap();
    let (ours_img, lld_img) = (load_images(&linked.elf), load_images(&lld_elf));
    assert_eq!(ours_img.len(), lld_img.len(), "{name}: PT_LOAD count");
    for (k, ((va, msz, a), (vb, mszb, b))) in ours_img.iter().zip(&lld_img).enumerate() {
        assert_eq!(
            (va, msz, a.len()),
            (vb, mszb, b.len()),
            "{name}: segment at {va:#x}"
        );
        let from = if k == 0 { headers } else { 0 };
        if let Some(i) = (from..a.len()).find(|&i| a[i] != b[i]) {
            panic!(
                "{name}: first difference at {:#x}: ours {:02x?}, ld.lld {:02x?}",
                va + i as u64,
                &a[i..(i + 8).min(a.len())],
                &b[i..(i + 8).min(b.len())]
            );
        }
    }
    // merlin-objdump reads ELF32 only until #150, which compares the RV64
    // set's `.objdump.txt` files too. Until then the RV64 links are checked
    // through their bytes, above, which says more than the listing.
    if linked.layout.class == elf::Class::Elf32 {
        assert_same_disassembly(set, name, &linked);
    }

    let (_, r, out) = run(&linked.elf);
    assert_eq!(
        (r, out.as_str()),
        (RunResult::Exited(exit), output),
        "{name}: run in Grail"
    );
    linked
}

/// Loads `elf` into a Grail machine (RV32 or RV64, as the file's class says)
/// and runs it; returns the machine, how it ended and what it printed.
pub fn run(elf: &[u8]) -> (Machine, RunResult, String) {
    let mut m = Machine::with_ram(grail::DEFAULT_RAM_SIZE);
    // Garbage in RAM, so .bss zeroing by the loader is actually checked.
    m.cpu.bus.ram.fill(0xa5);
    m.cpu.bus.uart.capture = true;
    m.load_elf(elf).unwrap();
    let r = m.run(Some(10_000), |_| {});
    let out = String::from_utf8(std::mem::take(&mut m.cpu.bus.uart.captured)).unwrap();
    (m, r, out)
}

pub fn ram(m: &Machine, addr: u64, len: usize) -> &[u8] {
    let off = (addr - grail::bus::RAM_BASE) as usize;
    &m.cpu.bus.ram[off..off + len]
}
