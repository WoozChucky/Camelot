//! End-to-end tests against clang-built objects and ld.lld's output for the
//! same inputs (`tests/fixtures`, regenerate with scripts/gen-ld-fixtures.sh).
//!
//! * `fixtures/norelocs`: three relocation-free objects. Linked by merlin-ld,
//!   the PT_LOADs, section headers and symbols must equal ld.lld's
//!   (`expected.txt` is `llvm-readelf -S -l -s` of the ld.lld executable), and
//!   Grail must load and run the result.
//! * `fixtures/hello`: `examples/hello/hello.S` assembled by clang. It has
//!   relocations, so only the layout is compared until issue #7.

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
    globals: HashMap<String, u32>,
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
        } else if t.len() == 8 && t[4] == "GLOBAL" {
            r.globals.insert(t[7].into(), hex(t[1]));
        }
    }
    r
}

fn expected(dir: &str) -> Readelf {
    parse_readelf(&std::fs::read_to_string(fixtures().join(dir).join("expected.txt")).unwrap())
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
            for s in (off..off + size).step_by(16) {
                if elf[s + 12] >> 4 != 0 {
                    r.globals
                        .insert(cstr(elf, strtab + u32_at(elf, s) as usize), u32_at(elf, s + 4));
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
    let linked = merlin_ld::link(&norelocs(), &norelocs_opts()).unwrap();
    assert!(linked.layout.warnings.is_empty(), "{:?}", linked.layout.warnings);
    let ours = read_ours(&linked.elf);
    let lld = expected("norelocs");
    assert_eq!(lld.loads.len(), 3, "fixture sanity: RX, R and RW segments");
    assert_eq!(ours.entry, lld.entry);
    assert_eq!(ours.loads, lld.loads);
    assert_eq!(ours.sections, lld.sections);
    assert_eq!(ours.globals, lld.globals);
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
    let mut m = Machine::new(grail::DEFAULT_RAM_SIZE);
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
    let lld = expected("hello");
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

/// Until issue #7 the relocation hook refuses every relocation, loudly.
/// (Issue #7 replaces this with a run of the linked hello example.)
#[test]
fn relocations_are_not_silently_skipped() {
    let opts = Options {
        script: script_file(&repo_root().join("examples/hello/link.ld")),
        entry: None,
    };
    let errs = errors(merlin_ld::link(&inputs("hello", &["hello.o"]), &opts));
    assert_eq!(
        errs,
        [
            "hello.o: cannot apply R_RISCV_PCREL_HI20 against '__stack_top' at .text.start+0x0: relocation \
             processing is not implemented yet (issue #7)",
            "hello.o: cannot apply R_RISCV_PCREL_LO12_I against '.Lpcrel_hi0' at .text.start+0x4: relocation \
             processing is not implemented yet (issue #7)",
            "hello.o: cannot apply R_RISCV_PCREL_HI20 against 'message' at .text.start+0x8: relocation \
             processing is not implemented yet (issue #7)",
            "hello.o: cannot apply R_RISCV_PCREL_LO12_I against '.Lpcrel_hi1' at .text.start+0xc: relocation \
             processing is not implemented yet (issue #7)",
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
        // Weak undefined: no error, no definition.
        assert_eq!(l.symbol("missing"), None);
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
    assert!(s(".stack").nobits);
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

#[test]
fn empty_output_sections_are_dropped() {
    let l = merlin_ld::layout(
        &inputs("norelocs", &["start.o"]),
        &script("SECTIONS { . = 0x80000000; .text : { *(.text*) } .data : { d = .; *(.data*) } }"),
    )
    .unwrap();
    assert_eq!(l.sections.len(), 1);
    assert_eq!(l.symbol("d"), Some(0x8000_0020));
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

#[test]
fn segments_group_by_permissions() {
    // Same permissions, small gap: one segment, gap filled in the file.
    let l = loads(
        "SECTIONS { . = 0x80000000; .text : { *(.text*) } .rodata : { *(.rodata*) } \
         .data : { *(.data) } . += 0x10; .data2 : { *(.data.*) } .bss : { *(.bss*) } }",
    );
    assert_eq!(l.len(), 3);
    let rw = &l[2];
    assert_eq!(rw.flags, "RW");
    // .data at 0x40 (8 bytes), 0x10 gap, .data.more aligned to 16 at 0x60 (0x14 bytes).
    assert_eq!((rw.vaddr, rw.filesz), (0x8000_0040, 0x34));
    // A page or more apart: separate segments, no padding in the file.
    let l = loads(
        "SECTIONS { . = 0x80000000; .text : { *(.text*) } .rodata : { *(.rodata*) } \
         .data : { *(.data) } . += 0x1000; .data2 : { *(.data.*) } .bss : { *(.bss*) } }",
    );
    assert_eq!(l.len(), 4);
    assert_eq!((l[2].vaddr, l[2].filesz, l[2].memsz), (0x8000_0040, 8, 8));
    assert_eq!(l[3].vaddr, 0x8000_1050);
    for s in &l {
        assert_eq!(s.offset % 0x1000, s.vaddr % 0x1000, "offset congruent to address");
    }
    // File contents can't follow NOBITS bytes in a segment.
    let l = loads(
        "SECTIONS { . = 0x80000000; .text : { *(.text*) } .bss : { *(.bss*) } \
         .data : { *(.data*) } .rodata : { *(.rodata*) } }",
    );
    let shape: Vec<(&str, u32, u32)> = l.iter().map(|s| (s.flags.as_str(), s.filesz, s.memsz)).collect();
    assert_eq!(
        shape,
        [
            ("RE", 0x28, 0x28),
            ("RW", 0, 0x118),
            ("RW", 0x24, 0x24),
            ("R", 0x18, 0x18)
        ]
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
    assert_eq!(read_ours(&elf).loads, expected("norelocs").loads);
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

    let o = std::process::Command::new(exe).arg("--bogus").output().unwrap();
    assert_eq!(o.status.code(), Some(2));
    let o = std::process::Command::new(exe).output().unwrap();
    assert_eq!(o.status.code(), Some(2));
    let _ = std::fs::remove_dir_all(&dir);
}
