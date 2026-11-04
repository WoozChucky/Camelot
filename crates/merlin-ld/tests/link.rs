//! End-to-end tests against clang-built objects and ld.lld's output for the
//! same inputs (`tests/fixtures`, made by clang and LLD 23.1.4; regenerate
//! with `cargo xtask ld-fixtures`), for RV32: the RV32 set of fixtures. The
//! RV64 set (`tests/fixtures/rv64`) is `rv64.rs`'s; the helpers both use are
//! in `common/mod.rs`.
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
//!   sections, gaps, NOBITS before PROGBITS, `. +=` sections, a first
//!   section not at a page start, with and without the headers loaded).
//! * `fixtures/hello`: `examples/hello/hello.S` assembled by clang.
//! * `fixtures/multiobj`: `examples/multiobj`, two objects that call and
//!   point into each other.
//! * `fixtures/relocs`: every relocation type, across objects, at the edges
//!   of their ranges; one object built with relaxation (`R_RISCV_RELAX`).
//! * `fixtures/data64`: `.dword sym + A` (R_RISCV_64) on RV32, in `.data`
//!   and in a debug section, which ld.lld fills differently.
//! * `fixtures/at`: `AT(lma)` load addresses: on the first section only, on
//!   a section right after one with the same permissions, in the middle
//!   (`.data` copied from its load address, a NOLOAD `.bss` after it), and
//!   the empty sections ld.lld drops or keeps.
//! * `fixtures/merge`: mergeable strings, where merlin-ld differs from ld.lld
//!   on purpose (spec §10); the test pins down the difference.
//! * `fixtures/rejects`, `fixtures/symbols`: objects for error paths.

mod common;

use common::{errors, input, ram, read_ours, run, script, script_file, Load, RV32};
use grail::{Machine, RunResult};
use merlin_ld::{Input, Options};
use std::collections::HashMap;
use std::path::PathBuf;

// The RV32 set (`tests/fixtures`) of the helpers in common/mod.rs.

fn fixtures() -> PathBuf {
    common::fixtures(RV32)
}

fn repo_root() -> PathBuf {
    common::repo_root()
}

fn inputs(dir: &str, names: &[&str]) -> Vec<Input> {
    common::inputs(RV32, dir, names)
}

/// The objects a fixture script names in its `/* objects: ... */` comment
/// (paths relative to `fixtures/`), and the script itself.
fn case(script: &str) -> (Vec<Input>, Options) {
    common::case(RV32, script)
}

/// `fixtures/<name>.expected.txt`, e.g. `expected("norelocs/link")`.
fn expected(name: &str) -> common::Readelf {
    common::expected(RV32, name)
}

/// Links fixture script `name.ld` with the objects it lists, checks the result
/// against ld.lld's (`name.expected.txt`, `name.lld.elf`, `name.objdump.txt`)
/// and runs it in Grail, which must print `output` and exit with 0.
fn assert_matches_ld_lld(name: &str, output: &str) -> merlin_ld::Linked {
    common::assert_matches_ld_lld_exit(RV32, name, output, 0)
}

/// [`assert_matches_ld_lld`] for a program that exits with `exit`.
fn assert_matches_ld_lld_exit(name: &str, output: &str, exit: u32) -> merlin_ld::Linked {
    common::assert_matches_ld_lld_exit(RV32, name, output, exit)
}

/// The relocation types each fixture object carries, by name.
fn reloc_types(path: &str) -> HashMap<String, usize> {
    common::reloc_types(RV32, path)
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

// ---------------------------------------------------------------------------
// Patching RV32 objects: ELF32 offsets.

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

/// Layout edge cases against ld.lld: output sections with only empty inputs
/// (they still align), with only assignments (kept, PROGBITS, flags of the
/// previous section but never executable), with only `. +=` (PROGBITS with file contents), with
/// nothing (dropped), a symbol before the first section with MEMORY (0), a
/// gap and NOBITS before PROGBITS inside one PT_LOAD, an empty R section
/// between RX and RW (no PT_LOAD of its own), a weak definition, and where
/// the file starts when the first section is not at the start of a page:
/// a page in, or after the headers, loaded below the section (in a PT_LOAD
/// of their own, or in a read-only first section's) with a PT_PHDR.
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
            "layouts/headers",
            "layouts/headers-ro",
            "layouts/inside",
            "layouts/no-inputs",
            "layouts/nobits-first",
            "layouts/page-gap",
            "layouts/unaligned"
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
    // Sections without inputs are never executable: `.marker`, after the
    // code, is read-only and goes with .rodata.
    let none = expected("layouts/no-inputs");
    let flags: Vec<(&str, &str)> = none
        .sections
        .iter()
        .map(|s| (s.name.as_str(), s.flags.as_str()))
        .collect();
    assert_eq!(
        flags,
        [
            (".text", "AX"),
            (".marker", "A"),
            (".rodata", "A"),
            (".data", "WA"),
            (".tag", "WA"),
            (".bss", "WA")
        ]
    );
    assert_eq!(none.loads.len(), 3);
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
    // The headers (0x114 bytes: the ELF header and 7 program headers) don't
    // fit in the 0x100 bytes below .text: not loaded, and .text a page in.
    let unaligned = expected("layouts/unaligned");
    assert_eq!(unaligned.phdr, None);
    assert_eq!(
        (unaligned.loads[0].offset, unaligned.loads[0].vaddr),
        (0x1000 + 0x100, 0x8000_0100)
    );
    // They fit in 0x400: loaded at the page, in a segment of their own
    // before the code's, the program header table in it.
    let headers = expected("layouts/headers");
    let hdr = Load {
        offset: 0,
        vaddr: 0x8000_0000,
        paddr: 0x8000_0000,
        filesz: 0x114,
        memsz: 0x114,
        flags: "R".into(),
        align: 0x1000,
    };
    assert_eq!(headers.loads[0], hdr);
    assert_eq!(
        (headers.loads[1].offset, headers.loads[1].flags.as_str()),
        (0x400, "RE")
    );
    let phdr = headers.phdr.unwrap();
    assert_eq!((phdr.offset, phdr.vaddr, phdr.filesz), (52, 0x8000_0034, 0xe0));
    // Or in .rodata's, which then starts at the page, its load address
    // moved down with it.
    let ro = expected("layouts/headers-ro");
    assert_eq!(ro.loads.len(), 3);
    assert_eq!(
        (
            ro.loads[0].offset,
            ro.loads[0].vaddr,
            ro.loads[0].paddr,
            ro.loads[0].flags.as_str()
        ),
        (0, 0x8010_0000, 0x8000_0000, "R")
    );
    assert_eq!(ro.phdr.unwrap().paddr, 0x8000_0034);
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
        assert_eq!(w[1].addr % u64::from(w[1].align), 0);
        assert!(w[1].addr - (w[0].addr + w[0].size) < u64::from(w[1].align));
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
    let ours: Vec<(String, u64, u64, u32)> = layout
        .sections
        .iter()
        .map(|s| (s.name.clone(), s.addr, s.size, s.align))
        .collect();
    let theirs: Vec<(String, u64, u64, u32)> = lld
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

/// Some fixture sources are copies, which `cargo xtask ld-fixtures` makes
/// before it builds anything: `fixtures/hello` and `fixtures/multiobj` of
/// the examples' sources and scripts, and the RV64 set's directories that
/// work on both widths (`SHARED_WITH_RV64` in crates/xtask/src/ld_fixtures.rs)
/// of the RV32 set's. If an original changes, its copy (and the objects and
/// ld.lld listings made from it) is stale until the tool runs again.
#[test]
fn fixture_copies_are_current() {
    let read = |p: PathBuf| {
        std::fs::read_to_string(&p)
            .unwrap_or_else(|e| panic!("{}: {e}", p.display()))
            .replace("\r\n", "\n")
    };
    let stale = "changed: run `cargo xtask ld-fixtures`";
    let copies = [
        ("hello", &["hello.S", "link.ld"][..]),
        ("multiobj", &["main.S", "print.S", "link.ld"]),
    ];
    for (dir, files) in copies {
        for f in files {
            assert!(
                read(repo_root().join("examples").join(dir).join(f)) == read(fixtures().join(dir).join(f)),
                "examples/{dir}/{f} {stale}"
            );
        }
    }

    // The sources (.S and .ld) of a fixture directory, by name.
    let sources = |dir: PathBuf| -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(&dir)
            .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".S") || n.ends_with(".ld"))
            .collect();
        v.sort();
        v
    };
    let shared = ["hello", "layouts", "merge", "norelocs", "rejects", "symbols"];
    let rv64_only = ["at", "relocs"];
    let rv64 = common::fixtures(common::RV64);
    let mut dirs: Vec<String> = std::fs::read_dir(&rv64)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    dirs.sort();
    let mut known: Vec<&str> = shared.iter().chain(&rv64_only).copied().collect();
    known.sort();
    assert_eq!(dirs, known, "the RV64 set's directories: copies and its own");
    for dir in shared {
        let files = sources(fixtures().join(dir));
        assert!(!files.is_empty(), "{dir}");
        assert_eq!(sources(rv64.join(dir)), files, "rv64/{dir}: {stale}");
        for f in files {
            assert!(
                read(fixtures().join(dir).join(&f)) == read(rv64.join(dir).join(&f)),
                "{dir}/{f} {stale}"
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
    // A hint for each relaxable instruction: the call, both halves of the lw,
    // the lui and the sw, and the loop's j, which clang 23 leaves to the
    // linker together with the branch over it (relax.S).
    assert_eq!(relax.get("R_RISCV_RELAX"), Some(&6), "{relax:?}");
    for ty in ["R_RISCV_BRANCH", "R_RISCV_JAL"] {
        assert_eq!(relax.get(ty), Some(&1), "{ty}: {relax:?}");
    }
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

/// `.dword sym + A` (R_RISCV_64) on RV32 gives ld.lld's bytes: in `.data`,
/// S + A in 64 bits (the program checks the words at run time); in a debug
/// section, the 32-bit S + A sign-extended, as ld.lld's relocateNonAlloc
/// writes it.
#[test]
fn data64_matches_ld_lld() {
    let types = reloc_types("data64/data64.o");
    assert_eq!(types.get("R_RISCV_64"), Some(&8), "{types:?}");
    let linked = assert_matches_ld_lld("data64/link", "OK\n");

    let lld_file = std::fs::read(fixtures().join("data64/link.lld.elf")).unwrap();
    let debug_info = |file: &[u8]| -> Vec<u8> {
        let e = elf::Elf::parse(file).unwrap();
        let s = e.sections.iter().find(|s| s.name == ".debug_info").unwrap();
        e.section_data(s).unwrap().to_vec()
    };
    let ours = debug_info(&linked.elf);
    assert_eq!(ours, debug_info(&lld_file), ".debug_info");
    // What the comparison is about, spelled out: `target` (0x800000d0) and
    // `target + 0x7fffffff`, loaded and not.
    let target = linked.layout.symbol("target").unwrap();
    assert_eq!(target, 0x8000_00d0);
    let dwords = |b: &[u8]| -> Vec<u64> {
        b.chunks(8)
            .map(|c| u64::from_le_bytes(c.try_into().unwrap()))
            .collect()
    };
    assert_eq!(
        dwords(&ours),
        [
            0xffff_ffff_8000_00d0,
            0xffff_ffff_8000_00cf,
            0xcf,
            0xffff_ffff_ffff_ffff
        ]
    );
    let data = linked
        .layout
        .sections
        .iter()
        .position(|s| s.name == ".data")
        .unwrap();
    let images = merlin_ld::relocate::relocate(&linked.layout).unwrap();
    assert_eq!(
        dwords(&images.sections[data][..32]),
        [0x8000_00d0, 0x8000_00cf, 0x1_0000_00cf, 0xffff_ffff]
    );
}

/// `AT(lma)` against ld.lld (fixtures/at): the program is linked to run at
/// 0x8000_0000 and Grail loads it at its load addresses (the segments'
/// p_paddr) and starts it at its entry point's, so it runs where it is
/// loaded (all it uses is pc-relative) or copies .data to where it runs.
#[test]
fn load_addresses_match_ld_lld() {
    let mut names: Vec<String> = std::fs::read_dir(fixtures().join("at"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "ld"))
        .map(|p| format!("at/{}", p.file_stem().unwrap().to_string_lossy()))
        .collect();
    names.sort();
    assert_eq!(names, ["at/first", "at/kept", "at/middle", "at/split"]);
    let linked: HashMap<&str, merlin_ld::Linked> = names
        .iter()
        .map(|n| (n.as_str(), assert_matches_ld_lld(n, "OK\n")))
        .collect();
    // Spot checks, so a broken fixture can't make both sides agree on
    // nonsense: (flags, vaddr, paddr) of each PT_LOAD.
    let loads = |name: &str| -> Vec<(String, u64, u64)> {
        expected(name)
            .loads
            .iter()
            .map(|l| (l.flags.clone(), l.vaddr, l.paddr))
            .collect()
    };
    let load = |f: &str, vaddr, paddr| (f.to_string(), vaddr, paddr);
    // AT on .text only: the same segments as without it, each 1 MiB up; the
    // entry point is where _start runs.
    assert_eq!(
        loads("at/first"),
        [
            load("RE", 0x8000_0000, 0x8010_0000),
            load("R", 0x8000_00ec, 0x8010_00ec),
            load("RW", 0x8000_00f0, 0x8010_00f0)
        ]
    );
    assert_eq!(expected("at/first").entry, 0x8000_0000);
    let first = &linked["at/first"].layout;
    let sec = |l: &merlin_ld::Layout, name: &str| l.section(name).unwrap().clone();
    assert!(sec(first, ".text").at && !sec(first, ".rodata").at);
    assert_eq!(sec(first, ".bss").lma, sec(first, ".bss").addr + 0x10_0000);
    // AT(.) on .text2: a segment of its own, though it follows .text with the
    // same permissions and is loaded where it runs.
    assert_eq!(
        loads("at/split"),
        [
            load("RE", 0x8000_0000, 0x8000_0000),
            load("RE", 0x8000_0090, 0x8000_0090),
            load("R", 0x8000_00f4, 0x8000_00f4),
            load("RW", 0x8000_00f8, 0x8000_00f8)
        ]
    );
    // AT in the middle: .data (and the NOLOAD .bss after it) loaded right
    // after .rodata, running a page later.
    assert_eq!(
        loads("at/middle"),
        [
            load("RE", 0x8000_0000, 0x8000_0000),
            load("R", 0x8000_00ec, 0x8000_00ec),
            load("RW", 0x8000_1000, 0x8000_00f0)
        ]
    );
    assert_eq!(expected("at/middle").globals["__data_load"], 0x8000_00f0);
    let middle = &linked["at/middle"].layout;
    assert_eq!(sec(middle, ".bss").lma, 0x8000_0100);
    // The empty sections: .dropped is dropped and its AT ignored, .kept's
    // AT moves everything, .named and .aligned are kept.
    let kept = expected("at/kept");
    let names: Vec<&str> = kept.sections.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(
        names,
        [".kept", ".text", ".named", ".aligned", ".rodata", ".data", ".bss"]
    );
    assert_eq!(kept.globals["__named"], 0x8000_00ec);
    assert!(kept.loads.iter().all(|l| l.paddr == l.vaddr + 0x10_0000));
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
        let addr = u64::from(addr);
        let seg = elf
            .segments
            .iter()
            .find(|s| addr >= s.paddr && addr < s.paddr + s.data.len() as u64)
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
        let p = u32::try_from(base + r.offset).unwrap();
        let to = match excalibur::decode(word(p), excalibur::Xlen::Rv32) {
            Inst::Jal { imm, .. } | Inst::Branch { imm, .. } => p.wrapping_add(imm as u32),
            Inst::Auipc { imm: hi, .. } => match excalibur::decode(word(p + 4), excalibur::Xlen::Rv32) {
                Inst::Jalr { imm: lo, .. } => p.wrapping_add(hi as u32).wrapping_add(lo as u32),
                other => panic!("no jalr after the auipc: {other:?}"),
            },
            other => panic!("unexpected {other:?}"),
        };
        let ty = merlin_ld::reloc::name(r.ty);
        assert_eq!(
            u64::from(to),
            target,
            "{ty} at {p:#x} goes to {to:#x}, not lib_inc"
        );
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
    let far = script(
        "ENTRY(_start) SECTIONS { . = 0x7ffffffc; .low : { *(.low) } . = 0x80100000; \
         .text : { *(.text.start) *(.text.branch) *(.text*) } .rodata : { *(.rodata*) } \
         .data : { *(.data*) } .bss : { *(.bss*) } __stack_top = . + 4K; \
         . = 0x80200004; .high : { *(.high) } }",
    );
    let errs = errors(merlin_ld::link(&objs, &far));
    // lib_check's `bne a0, t0, fail` is 0x3c into lib.o's .text, 8280 bytes
    // after `fail` once .text.branch is in between; main.o's branch to the
    // weak (but defined) lib_inc is about as far the other way. Were it taken for
    // an undefined weak, it would silently target itself instead.
    assert_eq!(
        errs,
        [
            "main.o: relocation R_RISCV_JAL out of range for 'high_target' at .text.start+0x4: 1048576 is not \
             in [-1048576, 1048575]",
            "main.o: relocation R_RISCV_JAL out of range for 'low_target' at .text.start+0x8: -1048580 is not \
             in [-1048576, 1048575]",
            "main.o: relocation R_RISCV_BRANCH out of range for 'lib_inc' at .text.start+0x268: 8444 is not \
             in [-4096, 4095]",
            "lib.o: relocation R_RISCV_BRANCH out of range for 'fail' at .text+0x3c: -8280 is not in [-4096, \
             4095]",
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
    // Odd and out of range: both errors, the range first, as ld.lld
    // reports them ("out of range: 1048577 is not in [-1048576, 1048575]",
    // then "improper alignment").
    let errs = errors(merlin_ld::link(&objs, &far));
    assert_eq!(
        errs[..2],
        [
            "main.o: relocation R_RISCV_JAL out of range for 'high_target' at .text.start+0x4: 1048577 is not \
             in [-1048576, 1048575]",
            "main.o: relocation R_RISCV_JAL against 'high_target' at .text.start+0x4 is misaligned: offset \
             1048577 is not a multiple of 2",
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
    let next = text
        .inputs
        .iter()
        .position(|i| i.addr == text.addr + u64::from(size));
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
    assert_eq!(words, [0x8000_0000, 0x8000_0008, 0, data.addr as u32 + 16]);

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

/// Label differences and set values (ADD/SUB/SET, 32_PCREL) give ld.lld's
/// bytes. In debug information on RV32 each value is sign-extended from 32
/// bits first (ld.lld: `.dword gone - _start` is 0x8000_0000 there, gone
/// being 0).
#[test]
fn label_differences_and_sets() {
    let ranges = common::assert_diffs(merlin_as::assemble_to_elf(common::DIFFS).unwrap());
    assert_eq!(ranges, [0, 0, 0, 0x80, 0, 0, 0, 0, 4, 0, 0, 0]);
}

/// R_RISCV_ALIGN is refused: ld.lld deletes the padding beyond what the
/// alignment needs, with --no-relax too, and merlin-ld does not.
/// R_RISCV_RELAX is ignored (relocs/relax.o, above).
#[test]
fn align_padding_is_refused() {
    let errs = errors(merlin_ld::link(
        &inputs("rejects", &["align.o"]),
        &Options::default(),
    ));
    assert_eq!(
        errs,
        [
            "align.o: R_RISCV_ALIGN at .text+0xc: merlin-ld does not delete alignment padding as ld.lld \
             does; assemble with -mno-relax"
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
    let counts: Vec<u64> = ours
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

#[test]
fn the_location_counter_moves_backward_at_the_top_level() {
    // As in ld.lld: a top-level `.` may move backward, and the sections
    // only must not end up over each other.
    let l = merlin_ld::link(
        &norelocs(),
        &script(
            "SECTIONS { . = 0x80001000; .text : { *(.text*) } . = 0x80000000; .rodata : { *(.rodata*) } \
             .data : { *(.data*) } .bss : { *(.bss*) } }",
        ),
    )
    .unwrap();
    let ours = read_ours(&l.elf);
    assert_eq!(ours.sections[0].addr, 0x8000_1000);
    assert_eq!(ours.sections[1].addr, 0x8000_0000);
    // Going backward starts a segment, at a file offset after the one
    // before (the second page here, as ld.lld puts it).
    assert_eq!(ours.loads[1].vaddr, 0x8000_0000);
    assert!(ours.loads[1].offset > ours.loads[0].offset);
    let errs = errors(merlin_ld::layout(
        &norelocs(),
        &script(
            "SECTIONS { . = 0x80000000; .text : { *(.text*) } . = 0x80000008; .rodata : { *(.rodata*) } \
             .data : { *(.data*) } .bss : { *(.bss*) } }",
        ),
    ));
    assert!(
        errs[0]
            .starts_with("section .text virtual address range overlaps with .rodata (.text is [0x80000000, "),
        "{errs:?}"
    );
    assert!(
        errs[1].starts_with("section .text load address range overlaps with .rodata"),
        "{errs:?}"
    );
}

/// After a section past the address space the location counter goes on
/// from its end, so the sections after it are reported where ld.lld puts
/// them, not all at the failed one's start.
#[test]
fn sections_after_one_past_4_gib_go_on_from_its_end() {
    let errs = errors(merlin_ld::layout(
        &norelocs(),
        &script(
            "SECTIONS { . = 0x100000000; .text : { *(.text*) } .rodata : { *(.rodata*) } \
             .data : { *(.data*) } .bss : { *(.bss*) } }",
        ),
    ));
    assert_eq!(errs.len(), 4, "{errs:?}");
    assert!(
        errs[0].starts_with("test.ld:1: section .text at 0x100000000 of size 0x"),
        "{errs:?}"
    );
    for e in &errs[1..] {
        assert!(
            e.contains(" at 0x1000000") && !e.contains(" at 0x100000000 "),
            "{errs:?}"
        );
    }
}

#[test]
fn align_0_is_align_1() {
    // ld.lld's checkAlignment takes 0 for 1, in an expression and on a
    // section: neither moves anything (.text still goes at the alignment
    // of its inputs).
    let rest = ".rodata : { *(.rodata*) } .data : { *(.data*) } .bss : { *(.bss*) }";
    for (src, at) in [
        (
            format!("SECTIONS {{ . = 0x80000001; . = ALIGN(0); .text : {{ *(.text*) }} {rest} }}"),
            0x8000_0004,
        ),
        (
            format!("SECTIONS {{ . = 0x80000000; .text : ALIGN(0) {{ *(.text*) }} {rest} }}"),
            0x8000_0000,
        ),
    ] {
        let l = merlin_ld::layout(&norelocs(), &script(&src)).unwrap_or_else(|e| panic!("{src}: {e:?}"));
        assert_eq!(l.section(".text").unwrap().addr, at, "{src}");
    }
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
            "SECTIONS { . = 0x80000000; .text : { *(.text*) . = . - 0x1000; } }",
            "test.ld:1: cannot move the location counter backward (from 0x80000",
        ),
        // ld.lld's words for a section past the address space.
        (
            "SECTIONS { . = 0xfffffff0; .text : { *(.text*) } }",
            "test.ld:1: section .text at 0xfffffff0 of size 0x",
        ),
        (
            // Exactly 4 GiB long: must not wrap to an empty section.
            "SECTIONS { . = 0; .s : { . += 0x100000000; } }",
            "test.ld:1: section .s at 0x0 of size 0x100000000 exceeds available address space",
        ),
        (
            // A negative location counter, at the top level, where `.` may
            // move backward.
            "SECTIONS { . = 0 - 1; }",
            "test.ld:1: location counter 0xffffffffffffffff is past the 4 GiB address space",
        ),
        (
            "SECTIONS { . = 0x80000000; . += 0x7fffffffffffffff; }",
            "test.ld:1: arithmetic overflow",
        ),
        ("SECTIONS { x = 0x100000000; }", "test.ld:1: value 0x100000000 of 'x' does not fit in 32 bits"),
        ("SECTIONS { x = ORIGIN(RAM); }", "test.ld:1: unknown memory region 'RAM'"),
        // ld.lld's checkAlignment, in expressions and on sections, and
        // merlin-ld's limit of 2^31 on a section's.
        (
            "SECTIONS { . = 0x80000001; . = ALIGN(3); }",
            "test.ld:1: ALIGN(3): alignment must be power of 2",
        ),
        ("SECTIONS { . = ALIGN(0 - 16); }", "test.ld:1: ALIGN(-16): alignment must be power of 2"),
        (
            "SECTIONS { .text : ALIGN(24) { *(.text*) } }",
            "test.ld:1: ALIGN(24) of section .text: alignment must be power of 2",
        ),
        (
            "SECTIONS { .text : ALIGN(0x100000000) { *(.text*) } }",
            "test.ld:1: ALIGN(4294967296) of section .text: alignment must be at most 2^31 (ld.lld keeps 32 \
             bits of a section's alignment and would ignore this one)",
        ),
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
    // An allocated section (a name with no flags of its own, `.mystuff`, is
    // not even loaded, as in LLVM).
    let o = merlin_as::assemble_to_elf(".section .mystuff, \"aw\"\n.word 1\n").unwrap();
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
    let o = merlin_as::assemble_to_elf(".globl marker\n.section .mystuff, \"aw\"\nmarker:\n").unwrap();
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
    let o = merlin_as::assemble_to_elf(".section .mystuff, \"aw\"\nlocal_marker:\n").unwrap();
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
        // ELFCLASS64 is RV64 (tests/rv64.rs); 3 is no class.
        (
            patched(4, &[3]),
            "x.o: not a 32- or 64-bit ELF file (EI_CLASS is 3)",
        ),
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
        // ld.lld: "x.o:(.text): sh_addralign is not a power of 2" (ELF32's
        // sh_addralign at 32). A symbol table's alignment it does not check.
        (
            {
                let mut b = good.clone();
                let t = shdr(&b, ".text");
                b[t + 32..t + 36].copy_from_slice(&12u32.to_le_bytes());
                b
            },
            "x.o: section '.text' has sh_addralign 0xc, not a power of 2",
        ),
        // ld.lld: "section [index 5] has an invalid sh_size (N) which is not
        // a multiple of its sh_entsize (16)" (ELF32's sh_size at 20).
        (
            {
                let mut b = good.clone();
                let s = shdr(&b, ".symtab");
                let size = u32::from_le_bytes(b[s + 20..s + 24].try_into().unwrap());
                b[s + 20..s + 24].copy_from_slice(&(size + 4).to_le_bytes());
                b[s + 32..s + 36].copy_from_slice(&3u32.to_le_bytes());
                b
            },
            "x.o: section '.symtab' has sh_size 0x",
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

/// `ADDR(section)`: the address of an output section placed before, or of
/// the one being placed, in its own AT and assignments. What ld.lld gives
/// for this script and these objects (checked by hand: x 0x80000028, y
/// 0x80000040, z 0x80000004, .data's segment at paddr 0x80100040).
#[test]
fn addr_is_an_output_sections_address() {
    let l = merlin_ld::layout(
        &norelocs(),
        &script(
            "SECTIONS { . = 0x80000000; .text : { *(.text*) } .rodata : { *(.rodata*) } x = ADDR(.rodata); \
             .data : AT(ADDR(.data) + 0x100000) { y = ADDR(.data); *(.data*) } .bss : { *(.bss*) } \
             z = ADDR(.text) + 4; }",
        ),
    )
    .unwrap();
    assert_eq!(
        (l.symbol("x"), l.symbol("y"), l.symbol("z")),
        (Some(0x8000_0028), Some(0x8000_0040), Some(0x8000_0004))
    );
    let data = l.section(".data").unwrap();
    assert_eq!((data.addr, data.lma, data.at), (0x8000_0040, 0x8010_0040, true));
    // .bss has no AT and keeps .data's difference.
    let bss = l.section(".bss").unwrap();
    assert_eq!((bss.lma - bss.addr, bss.at), (0x10_0000, false));
}

/// The load address errors: AT ranges that overlap (ld.lld's
/// checkSections, with its ranges on the same line), that run past 4 GiB
/// on RV32 (where ld.lld truncates), and ADDR of what has no address yet.
#[test]
fn load_address_errors() {
    let cases: &[(&str, &str)] = &[
        (
            // ld.lld: "section .text load address range overlaps with
            // .rodata", ">>> .text range is [0x80100000, 0x80100027]",
            // ">>> .rodata range is [0x80100010, 0x80100027]".
            "SECTIONS { . = 0x80000000; .text : AT(0x80100000) { *(.text*) } \
             .rodata : AT(0x80100010) { *(.rodata*) } .data : { *(.data*) } .bss : { *(.bss*) } }",
            "section .text load address range overlaps with .rodata (.text is [0x80100000, 0x80100027], \
             .rodata is [0x80100010, 0x80100027])",
        ),
        (
            // .bss loaded over .text too, but it loads no bytes (NOBITS).
            "SECTIONS { . = 0x80000000; .text : { *(.text*) } .rodata : { *(.rodata*) } \
             .data : AT(0x80000000) { *(.data*) } .bss : AT(0x80000000) { *(.bss*) } }",
            "section .text load address range overlaps with .data (.text is [0x80000000, 0x80000027], \
             .data is [0x80000000, 0x80000023])",
        ),
        (
            "SECTIONS { . = 0x80000000; .text : AT(0xffffffe0) { *(.text*) } .rodata : { *(.rodata*) } \
             .data : { *(.data*) } .bss : { *(.bss*) } }",
            "test.ld:1: section .text load address range extends past the 4 GiB address space",
        ),
        (
            // A negative load address (RV32 scripts compute in i64).
            "SECTIONS { . = 0x80000000; .text : AT(0 - 1) { *(.text*) } .rodata : { *(.rodata*) } \
             .data : { *(.data*) } .bss : { *(.bss*) } }",
            "test.ld:1: section .text load address range extends past the 4 GiB address space",
        ),
        (
            "SECTIONS { x = ADDR(.nope); .text : { *(.text*) } .rodata : { *(.rodata*) } \
             .data : { *(.data*) } .bss : { *(.bss*) } }",
            "test.ld:1: undefined section .nope",
        ),
        (
            // ld.lld would go round again; merlin-ld walks the script once.
            "SECTIONS { x = ADDR(.data); .text : { *(.text*) } .rodata : { *(.rodata*) } \
             .data : { *(.data*) } .bss : { *(.bss*) } }",
            "test.ld:1: section '.data' is used before it is placed",
        ),
        (
            // A section's ALIGN decides where it starts: its ADDR is not
            // known yet there.
            "SECTIONS { .text : { *(.text*) } .rodata : { *(.rodata*) } \
             .data : ALIGN(ADDR(.data) - ADDR(.data) + 16) { *(.data*) } .bss : { *(.bss*) } }",
            "test.ld:1: section '.data' is used before it is placed",
        ),
        (
            // Without a /DISCARD/ in the script, ld.lld's words.
            "SECTIONS { .text : { *(.text*) } .rodata : { *(.rodata*) } .data : { *(.data*) } \
             .bss : { *(.bss*) } x = ADDR(/DISCARD/); }",
            "test.ld:1: undefined section /DISCARD/",
        ),
        (
            // ld.lld reads a /DISCARD/'s ALIGN as any other's.
            "SECTIONS { .text : { *(.text*) } /DISCARD/ : ALIGN(3) { *(.comment) } }",
            "test.ld:1: ALIGN(3) of section /DISCARD/: alignment must be power of 2",
        ),
        (
            // ld.lld: "{ expected, but got AT".
            "SECTIONS { .text : ALIGN(4) AT(0x80100000) { *(.text*) } }",
            "test.ld:1: unsupported syntax after '.text :': found 'AT' (only AT(lma) and then ALIGN(n) may \
             come before '{')",
        ),
    ];
    for (src, want) in cases {
        let errs = errors(merlin_ld::layout(&norelocs(), &script(src)));
        assert!(
            errs.iter().any(|e| e == want),
            "{src}\n got {errs:?}\nwant {want}"
        );
    }
    // As in ld.lld: a /DISCARD/'s NOLOAD and AT mean nothing (the AT is
    // not even evaluated), and its ADDR is 0, when the script has one.
    let l = merlin_ld::layout(
        &norelocs(),
        &script(
            "SECTIONS { . = 0x80000000; .text : { *(.text*) } .rodata : { *(.rodata*) } \
             .data : AT(ADDR(/DISCARD/) + 0x1000) { *(.data*) } .bss : { *(.bss*) } x = ADDR(/DISCARD/); \
             /DISCARD/ (NOLOAD) : AT(nowhere) ALIGN(16) { *(.comment) } }",
        ),
    )
    .unwrap();
    assert_eq!(l.symbol("x"), Some(0));
    assert_eq!(l.section(".data").unwrap().lma, 0x1000);
    // A load address range may end exactly at 4 GiB (ld.lld links this).
    let l = merlin_ld::layout(
        &norelocs(),
        &script(
            "SECTIONS { . = 0x80000000; .text : AT(0xffffffd8) { *(.text*) } \
             .rodata : AT(0x80100000) { *(.rodata*) } .data : { *(.data*) } .bss : { *(.bss*) } }",
        ),
    )
    .unwrap();
    let text = l.section(".text").unwrap();
    assert_eq!(text.lma + text.size, 1 << 32);
    // Every section after one past 4 GiB keeps its difference, and is past
    // it too: each one is reported.
    let errs = errors(merlin_ld::layout(
        &norelocs(),
        &script(
            "SECTIONS { . = 0x80000000; .text : AT(0xffffffe0) { *(.text*) } .rodata : { *(.rodata*) } \
             .data : { *(.data*) } .bss : { *(.bss*) } }",
        ),
    ));
    assert_eq!(errs.len(), 4, "{errs:?}");
}

/// A section loaded somewhere else than it runs says where in the map.
#[test]
fn map_file_shows_load_addresses() {
    let (objs, opts) = case("at/middle.ld");
    let linked = merlin_ld::link(&objs, &opts).unwrap();
    let map = merlin_ld::map::map(&linked.layout);
    for want in [
        "\n.rodata              0x800000ec 0x00000004 1\n",
        "\n.data                0x80001000 0x00000008 4096  (load 0x800000f0)\n",
        "\n.bss                 0x80001010 0x00000010 16  (nobits)  (load 0x80000100)\n",
    ] {
        assert!(map.contains(want), "map lacks {want:?}:\n{map}");
    }
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

/// A segment is aligned to the largest alignment of its sections when that
/// is more than a page, NOBITS ones included, and its file offset is
/// congruent to its address modulo that: what ld.lld 23 writes for this
/// script and source on both widths (checked by hand: `LOAD 0x009000
/// 0x80001000 ... RW 0x8000`; on RV64, `rv64si-p-icache-alias` of the
/// riscv-tests relink has such a segment).
#[test]
fn segments_take_the_largest_alignment_of_their_sections() {
    let src = "    .text\n    .globl _start\n_start:\n    nop\n    .data\n    .word 1\n\
               .section .data.big, \"aw\"\n    .p2align 13\n    .word 2\n\
               .section .data.huge, \"aw\"\n    .p2align 14\n    .word 3\n\
               .bss\n    .p2align 15\n    .zero 8\n";
    let opts = script(
        "SECTIONS { . = 0x80000000; .text : { *(.text) } . = ALIGN(0x1000); .data : { *(.data) } \
         .big : { *(.data.big) } . = 0x80010000; .huge : { *(.data.huge) } .bss : { *(.bss) } }",
    );
    for xlen in [excalibur::Xlen::Rv32, excalibur::Xlen::Rv64] {
        let obj = Input {
            name: "a.o".into(),
            data: merlin_as::assemble_to_elf_with(src, &merlin_as::Options { xlen }).unwrap(),
        };
        let linked = merlin_ld::link(&[obj], &opts).unwrap();
        let l: Vec<(u64, u64, u64, u64, u64)> = read_ours(&linked.elf)
            .loads
            .iter()
            .map(|l| (l.offset, l.vaddr, l.filesz, l.memsz, l.align))
            .collect();
        assert_eq!(
            l,
            [
                (0x1000, 0x8000_0000, 4, 4, 0x1000),
                (0x9000, 0x8000_1000, 0xf004, 0x17008, 0x8000)
            ],
            "{xlen:?}"
        );
    }
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
    let elf = elf::Elf::parse(&linked.elf).unwrap();
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
    let segments = |file: &[u8]| -> Vec<(u64, Vec<u8>)> {
        let e = elf::Elf::parse(file).unwrap();
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
    let rows: Vec<Vec<(u64, &str, u32, bool)>> = tables
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
    // (The RV64 twin of this test is in rv64.rs.)
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
    let elf = elf::Elf::parse(&linked.elf).unwrap();
    let seqs = &elf.line_tables().unwrap()[0].sequences;
    let starts: Vec<(u64, u32)> = seqs.iter().map(|s| (s.rows[0].addr, s.rows[0].line)).collect();
    assert_eq!(starts, [(0x1000, 1), (0, 7)]);
}

/// The same with the discarded code at an offset in its section: the line
/// table's address is `.text.gone + 4`, a relocation with addend 4, and it
/// still becomes 0 (ld.lld writes 0 whatever the addend).
#[test]
fn debug_information_of_discarded_code_with_an_addend_points_nowhere() {
    let src = "        .file 0 \"src\" \"a.c\"
        .text
        .globl _start
_start:
        .loc 0 1 0
        nop
        .section .text.gone
        nop
gone:
        .loc 0 7 0
        nop
";
    let obj = Input {
        name: "a.o".into(),
        data: merlin_as::assemble_to_elf(src).unwrap(),
    };
    let parsed = merlin_ld::object::parse("a.o", &obj.data).unwrap();
    let line = parsed.sections.iter().find(|s| s.name == ".debug_line").unwrap();
    assert!(line.relocs.iter().any(|r| r.addend == 4), "{:?}", line.relocs);
    let opts = script("SECTIONS { . = 0x1000; .text : { *(.text) } /DISCARD/ : { *(.text.gone) } }");
    let linked = merlin_ld::link(&[obj], &opts).unwrap();
    let elf = elf::Elf::parse(&linked.elf).unwrap();
    let seqs = &elf.line_tables().unwrap()[0].sequences;
    let starts: Vec<(u64, u32)> = seqs.iter().map(|s| (s.rows[0].addr, s.rows[0].line)).collect();
    assert_eq!(starts, [(0x1000, 1), (0, 7)]);
}

/// A reference from one object's debug information to a global another
/// object defines in code /DISCARD/ threw away, and to a weak symbol nobody
/// defines, points nowhere too: ld.lld's tombstone, 0, but 1 in
/// `.debug_ranges` and `.debug_loc` (where 0, 0 ends a list). The bytes are
/// ld.lld 23's for these objects (common::TOMBSTONES), the same on RV64
/// (rv64.rs).
#[test]
fn debug_information_of_a_discarded_global_points_nowhere() {
    let asm = |src| merlin_as::assemble_to_elf(src).unwrap();
    common::assert_tombstones(asm(common::TOMBSTONES), asm(common::TOMBSTONES_GONE));
}

/// OUTPUT_FORMAT names the class the objects must have: with RV32 objects,
/// `elf64-littleriscv` makes every one of them an error, as in ld.lld
/// (`a.o is incompatible with elf64-littleriscv`), and `elf32-littleriscv`
/// links as without it.
#[test]
fn output_format_must_match_the_objects() {
    let sections = "SECTIONS { . = 0x80000000; .text : { *(.text*) } .data : { *(.data* .rodata* .bss*) } }";
    assert_eq!(
        errors(merlin_ld::layout(
            &norelocs(),
            &script(&format!("OUTPUT_FORMAT(\"elf64-littleriscv\") {sections}"))
        )),
        [
            "start.o is incompatible with elf64-littleriscv",
            "data.o is incompatible with elf64-littleriscv",
            "more.o is incompatible with elf64-littleriscv",
        ]
    );
    let with = merlin_ld::link(
        &norelocs(),
        &script(&format!("OUTPUT_FORMAT(\"elf32-littleriscv\") {sections}")),
    )
    .unwrap();
    let without = merlin_ld::link(&norelocs(), &script(sections)).unwrap();
    assert_eq!(with.layout.class, elf::Class::Elf32);
    assert_eq!(with.elf, without.elf);
}
