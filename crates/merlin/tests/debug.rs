//! `-g`: the debug information merlin writes (`merlin::debug`), assembled by
//! merlin-as and read back with the elf crate. It must not change the code: every test
//! case compiles to the same loaded bytes, relocations and symbols with and
//! without it.

use merlin::types::Xlen;
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

/// [`debug_opts`] for RV64.
fn debug_opts_rv64(comp_dir: &str) -> Options {
    Options {
        xlen: Xlen::Rv64,
        ..debug_opts(comp_dir)
    }
}

/// Assembles for RV64.
fn assemble_rv64(asm: &str) -> Vec<u8> {
    merlin_as::assemble_to_elf_with(asm, &merlin_as::Options { xlen: Xlen::Rv64 }).unwrap()
}

/// The parts of an object that end up loaded: each allocated section's name,
/// bytes and relocations (offset, type, symbol name), and the symbols other
/// than section symbols and those of the debug information itself (the `$d`
/// mapping symbol of each `.debug_*` section, as clang writes it).
type Loaded = (
    Vec<(String, Vec<u8>, Vec<(u64, u32, String)>)>,
    Vec<(String, u64, u64, u8)>,
);

fn loaded(obj: &[u8]) -> Loaded {
    let elf = elf::Elf::parse(obj).unwrap();
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
            .filter(|r| r.ty == elf::SHT_RELA && r.info as usize == i)
        {
            for r in elf.relocations(rel).unwrap() {
                relocs.push((r.offset, r.ty, symbols[r.sym as usize].name.clone()));
            }
        }
        sections.push((s.name.clone(), elf.section_data(s).unwrap().to_vec(), relocs));
    }
    let in_debug = |s: &elf::Symbol| {
        s.section()
            .is_some_and(|i| elf.sections[i].name.starts_with(".debug_"))
    };
    let symbols = symbols
        .iter()
        .filter(|s| s.ty() != elf::STT_SECTION && !in_debug(s))
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
    for x in [Xlen::Rv32, Xlen::Rv64] {
        let plain_opts = Options {
            xlen: x,
            ..Options::default()
        };
        let debug_opts = Options {
            xlen: x,
            ..debug_opts("tests/cases")
        };
        let as_opts = merlin_as::Options { xlen: x };
        for path in &cases {
            let src = std::fs::read_to_string(path).unwrap();
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            let plain = merlin::compile_with(&name, &src, &plain_opts, &merlin::no_files).unwrap();
            let debug = merlin::compile_with(&name, &src, &debug_opts, &merlin::no_files).unwrap();
            assert!(debug.contains(".loc 0 "), "{name}: no line information");
            let plain = merlin_as::assemble_to_elf_with(&plain, &as_opts).unwrap();
            let debug =
                merlin_as::assemble_to_elf_with(&debug, &as_opts).unwrap_or_else(|e| panic!("{name}: {e:?}"));
            assert_eq!(loaded(&plain), loaded(&debug), "{name} ({x:?})");
        }
    }
}

/// On RV64 the compile unit's addresses are 8 bytes: its header says so, and
/// so does `.debug_addr`'s, whose addresses (`DW_FORM_addrx` indexes them) are
/// `.quad`s (R_RISCV_64 relocations). The line table reads back as on RV32.
#[test]
fn rv64_addresses_are_eight_bytes() {
    let src = "int f(int n) {\n    return n + 1;\n}\nlong g(long x) {\n    return f(x);\n}\n";
    let asm = merlin::compile_with("w.c", src, &debug_opts_rv64("dir"), &merlin::no_files).unwrap();
    assert!(asm.contains("    .byte 8    # address size\n"), "{asm}");
    assert!(asm.contains("    .quad f\n"), "{asm}");
    assert!(!asm.contains(".word f\n"), "{asm}");
    let obj = assemble_rv64(&asm);
    let elf = elf::Elf::parse(&obj).unwrap();
    let info = elf.sections.iter().find(|s| s.name == ".debug_info").unwrap();
    let data = elf.section_data(info).unwrap();
    let length = u32::from_le_bytes(data[..4].try_into().unwrap());
    assert_eq!(length as usize + 4, data.len());
    assert_eq!(u16::from_le_bytes([data[4], data[5]]), 5, "DWARF 5");
    assert_eq!(data[7], 8, "address size");
    let (i, addr) = elf
        .sections
        .iter()
        .enumerate()
        .find(|(_, s)| s.name == ".debug_addr")
        .unwrap();
    let data = elf.section_data(addr).unwrap();
    assert_eq!(data[6], 8, "address size");
    assert_eq!(data.len(), 8 + 2 * 8);
    // The unit's low_pc, which is f's, and g's: 8-byte relocations.
    let symbols = elf.symbols().unwrap();
    let rela = elf
        .sections
        .iter()
        .find(|r| r.ty == elf::SHT_RELA && r.info as usize == i)
        .unwrap();
    let addrs: Vec<String> = elf
        .relocations(rela)
        .unwrap()
        .iter()
        .filter(|r| elf::reloc_name(r.ty) == Some("R_RISCV_64"))
        .map(|r| symbols[r.sym as usize].name.clone())
        .collect();
    assert_eq!(addrs, ["f", "g"]);
    // The line table: both functions, in order.
    let tables = elf.line_tables().unwrap();
    assert_eq!(tables[0].files, ["dir/w.c"]);
    let lines: Vec<u32> = tables[0].sequences[0]
        .rows
        .iter()
        .filter(|r| !r.end_sequence)
        .map(|r| r.line)
        .collect();
    assert_eq!(lines, [1, 2, 3, 4, 5, 6]);
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
    let elf = elf::Elf::parse(&obj).unwrap();
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
    // file, the directory and the functions, in .debug_str (each string once,
    // in the order the entries name them), which .debug_str_offsets indexes.
    let section = |name: &str| {
        let s = elf.sections.iter().find(|s| s.name == name).unwrap();
        elf.section_data(s).unwrap()
    };
    let data = section(".debug_info");
    let length = u32::from_le_bytes(data[..4].try_into().unwrap());
    assert_eq!(length as usize + 4, data.len());
    let strings = String::from_utf8_lossy(section(".debug_str")).into_owned();
    assert!(strings.starts_with("merlin\0loop.c\0dir\0f\0"), "{strings:?}");
    assert!(strings.contains("\0empty\0"), "{strings:?}");
    let offsets = section(".debug_str_offsets");
    assert_eq!(offsets.len(), 8 + 4 * strings.matches('\0').count());
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
        asm.contains("    .file 1 \"src\" \"./inc/h.h\"\n    .loc 1 1 0\n"),
        "{asm}"
    );
    let obj = merlin_as::assemble_to_elf(&asm).unwrap();
    let tables = elf::Elf::parse(&obj).unwrap().line_tables().unwrap();
    // Named as clang names it: next to a main file given without a
    // directory, `./inc/h.h`.
    assert_eq!(tables[0].files, ["src/m.c", "src/./inc/h.h"]);
    // A file without functions has nothing to describe.
    let asm = merlin::compile_with("d.c", "int x = 1;\n", &debug_opts("src"), &merlin::no_files).unwrap();
    assert!(!asm.contains(".debug_info"), "{asm}");
    assert!(merlin_as::assemble_to_elf(&asm).is_ok());
}

/// A `static` function is generated once used, after the others, but its
/// code is where it was first referred to, as clang has it: a header's file
/// number is given where the output first needs it, which need not be where
/// the header's code was first generated.
#[test]
fn file_numbers_follow_the_output() {
    let src = "static int sa(void);\nint f(void) {\n    return sa();\n}\n#include \"a.h\"\n";
    let load = |p: &Path| {
        (p == Path::new("a.h")).then(|| {
            "int ea(void) {\n    return 1;\n}\nstatic int sa(void) {\n    return 2;\n}\n".to_string()
        })
    };
    let asm = merlin::compile_with("m.c", src, &debug_opts("src"), &load).unwrap();
    let labels: Vec<&str> = asm
        .lines()
        .filter(|l| matches!(*l, "f:" | "sa:" | "ea:"))
        .collect();
    assert_eq!(labels, ["f:", "sa:", "ea:"], "{asm}");
    let sa = asm.find("sa:").unwrap();
    assert!(
        asm[sa..].starts_with("sa:\n    .file 1 \"src\" \"./a.h\"\n    .loc 1 4 0\n"),
        "{asm}"
    );
    assert_eq!(asm.matches(".file 1 ").count(), 1, "{asm}");
    let obj = merlin_as::assemble_to_elf(&asm).unwrap();
    let tables = elf::Elf::parse(&obj).unwrap().line_tables().unwrap();
    assert_eq!(tables[0].files, ["src/m.c", "src/./a.h"]);
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

/// The program the variable tests describe: a typedef of a struct that points
/// to itself, a union, an enum, arrays, a function pointer, const and volatile,
/// globals and a `static` local, parameters, and locals in nested blocks.
const VARS: &str = "\
typedef struct node {
    int value;
    struct node *next;
} Node;
union word { unsigned u; unsigned char b[4]; };
enum color { RED, GREEN = 5, BLUE };
typedef enum color color_t;
Node head;
static short grid[2][3];
const char *volatile name;
int (*pick)(int, ...);
struct box { int k; struct { char c; } in; enum { ON, OFF } state; } bx;
long walk(Node *list, color_t c) {
    static int calls;
    long sum = 0;
    calls++;
    {
        union word w;
        static int seen;
        seen++;
        w.u = c;
        for (int i = 0; i < 2; i++) {
            Node *p = list;
            sum += p->value + w.b[0] + grid[i][1];
        }
    }
    return sum;
}
int old() { return 1; }
";

/// The entries merlin writes for [`VARS`], read back with `elf::dies` from the
/// object (its relocations applied): a function's parameters and locals where
/// its frame has them, the blocks around the code that declares them, globals
/// at their symbols, and every type, at both widths.
#[test]
fn variables_and_types_are_described() {
    use elf::dies::{dw, Attr, Die, InfoUnit};
    for x in [Xlen::Rv32, Xlen::Rv64] {
        let opts = Options {
            xlen: x,
            ..debug_opts("dir")
        };
        let asm = merlin::compile_with("v.c", VARS, &opts, &merlin::no_files).unwrap();
        let obj = merlin_as::assemble_to_elf_with(&asm, &merlin_as::Options { xlen: x }).unwrap();
        let elf = elf::Elf::parse(&obj).unwrap();
        let units = elf.debug_entries().unwrap();
        assert_eq!(units.len(), 1);
        let u: &InfoUnit = &units[0];
        let ptr = u64::from(x.bits() / 8);
        assert_eq!(u.addr_size as u64, ptr);
        let at = |i: usize| &u.dies[i];
        let named = |tag: u64, name: &str| {
            u.dies
                .iter()
                .position(|d| d.tag == tag && d.name() == Some(name))
                .unwrap_or_else(|| panic!("no {name} ({x:?}) in\n{asm}"))
        };
        let ty = |d: &Die| u.index_of(d.reference(dw::AT_TYPE).expect("a type")).unwrap();
        // A type as C writes it, from its entries.
        fn spell(u: &InfoUnit, i: Option<usize>) -> String {
            let Some(i) = i else {
                return "void".into();
            };
            let d = &u.dies[i];
            let inner = || d.reference(dw::AT_TYPE).and_then(|o| u.index_of(o));
            match d.tag {
                dw::TAG_BASE_TYPE | dw::TAG_TYPEDEF => d.name().unwrap().to_string(),
                dw::TAG_STRUCTURE_TYPE => format!("struct {}", d.name().unwrap()),
                dw::TAG_UNION_TYPE => format!("union {}", d.name().unwrap()),
                dw::TAG_ENUMERATION_TYPE => format!("enum {}", d.name().unwrap()),
                dw::TAG_POINTER_TYPE => format!("{} *", spell(u, inner())),
                dw::TAG_CONST_TYPE => format!("const {}", spell(u, inner())),
                dw::TAG_VOLATILE_TYPE => format!("volatile {}", spell(u, inner())),
                dw::TAG_ARRAY_TYPE => {
                    let dims: String = d
                        .children
                        .iter()
                        .map(|&c| format!("[{}]", u.dies[c].unsigned(dw::AT_COUNT).unwrap()))
                        .collect();
                    format!("{}{dims}", spell(u, inner()))
                }
                dw::TAG_SUBROUTINE_TYPE => {
                    let params: Vec<String> = d
                        .children
                        .iter()
                        .map(|&c| match u.dies[c].tag {
                            dw::TAG_UNSPECIFIED_PARAMETERS => "...".to_string(),
                            _ => spell(u, u.dies[c].reference(dw::AT_TYPE).and_then(|o| u.index_of(o))),
                        })
                        .collect();
                    format!("{}({})", spell(u, inner()), params.join(", "))
                }
                t => panic!("tag {t:#x}"),
            }
        }
        let type_of = |i: usize| spell(u, Some(ty(at(i))));

        // The function: its frame base is s0, and each parameter and local is
        // where the assembly's header comment says its slot is.
        let walk = named(dw::TAG_SUBPROGRAM, "walk");
        assert_eq!(at(walk).block(dw::AT_FRAME_BASE), Some(&[0x58][..]));
        assert_eq!(type_of(walk), "long");
        assert!(at(walk).flag(dw::AT_EXTERNAL));
        // `int old()` has no prototype, as clang says by leaving it out.
        assert!(at(walk).flag(dw::AT_PROTOTYPED));
        assert!(!at(named(dw::TAG_SUBPROGRAM, "old")).flag(dw::AT_PROTOTYPED));
        // A symbol's address: DW_OP_addrx and its index in .debug_addr.
        let address = |loc: &[u8]| {
            assert_eq!((loc[0], loc.len()), (0xa1, 2), "DW_OP_addrx");
            u.addr_pool[usize::from(loc[1])]
        };
        let slot = |what: &str| -> Vec<u8> {
            let line = asm
                .lines()
                .find(|l| l.contains(&format!("(s0)  {what}")))
                .unwrap_or_else(|| panic!("no slot for {what} in\n{asm}"));
            let off: i64 = line
                .trim_start_matches(['#', ' '])
                .split('(')
                .next()
                .unwrap()
                .parse()
                .unwrap();
            let mut b = vec![0x91];
            let mut v = off;
            loop {
                let byte = (v & 0x7f) as u8;
                v >>= 7;
                if (v == 0 && byte & 0x40 == 0) || (v == -1 && byte & 0x40 != 0) {
                    b.push(byte);
                    return b;
                }
                b.push(byte | 0x80);
            }
        };
        let children = |i: usize| at(i).children.clone();
        let kids = children(walk);
        let list = kids[0];
        assert_eq!(
            (at(list).tag, at(list).name()),
            (dw::TAG_FORMAL_PARAMETER, Some("list"))
        );
        assert_eq!(
            at(list).block(dw::AT_LOCATION).unwrap(),
            slot("param struct node *list").as_slice()
        );
        assert_eq!(type_of(list), "Node *");
        assert_eq!(type_of(kids[1]), "color_t");
        assert_eq!(at(kids[1]).unsigned(dw::AT_DECL_LINE), Some(13));
        // The local in the frame, then the blocks, then the function's static
        // locals, its block's too, in the order they are declared, at their
        // symbols (clang's order, though `calls` is declared first).
        assert_eq!(at(kids[2]).name(), Some("sum"));
        assert_eq!(
            at(kids[2]).block(dw::AT_LOCATION).unwrap(),
            slot("local long sum").as_slice()
        );
        assert_eq!(kids.len(), 6);
        let symbols = elf.symbols().unwrap();
        let symbol = |name: &str| symbols.iter().find(|s| s.name.contains(name)).unwrap().value;
        for (i, name) in [(4, "calls"), (5, "seen")] {
            let s = kids[i];
            assert_eq!(at(s).name(), Some(name));
            assert_eq!(address(at(s).block(dw::AT_LOCATION).unwrap()), symbol(name));
        }
        // The block of `w`, and in it the `for`'s, with `i` and its body's,
        // with `p`: each inside the one around it.
        let block = kids[3];
        assert_eq!(at(block).tag, dw::TAG_LEXICAL_BLOCK);
        let (wlo, whi) = at(walk).ranges[0];
        let inside = |inner: usize, outer: (u64, u64)| {
            let (lo, hi) = at(inner).ranges[0];
            assert!(outer.0 <= lo && lo < hi && hi <= outer.1, "{x:?}");
            (lo, hi)
        };
        let b1 = inside(block, (wlo, whi));
        let w = children(block)[0];
        assert_eq!((at(w).name(), type_of(w).as_str()), (Some("w"), "union word"));
        let for_block = children(block)[1];
        let b2 = inside(for_block, b1);
        assert_eq!(at(children(for_block)[0]).name(), Some("i"));
        let body = children(for_block)[1];
        inside(body, b2);
        let p = children(body)[0];
        assert_eq!((at(p).name(), type_of(p).as_str()), (Some("p"), "Node *"));
        assert_eq!(
            at(p).block(dw::AT_LOCATION).unwrap(),
            slot("local struct node *p").as_slice()
        );

        // The globals, at their symbols (relocated in the object: section
        // offsets), with their types.
        for (name, spelled) in [
            ("head", "Node"),
            ("grid", "short[2][3]"),
            // A volatile pointer to const char (the test's spelling is
            // prefix, outermost first).
            ("name", "volatile const char *"),
            ("pick", "int(int, ...) *"),
        ] {
            let g = named(dw::TAG_VARIABLE, name);
            assert_eq!(at(g).parent, Some(0));
            assert_eq!(type_of(g), spelled, "{name}");
            let loc = at(g).block(dw::AT_LOCATION).unwrap();
            let sym = symbols.iter().find(|s| s.name == name).unwrap();
            assert_eq!(address(loc), sym.value, "{name}");
            assert_eq!(at(g).flag(dw::AT_EXTERNAL), name != "grid", "{name}");
        }

        // The struct, the union and the enum, laid out for the target.
        let node = named(dw::TAG_STRUCTURE_TYPE, "node");
        assert_eq!(at(node).unsigned(dw::AT_BYTE_SIZE), Some(2 * ptr));
        let members: Vec<(String, String, Option<u64>)> = children(node)
            .iter()
            .map(|&m| {
                let d = at(m);
                (
                    d.name().unwrap().to_string(),
                    type_of(m),
                    d.unsigned(dw::AT_DATA_MEMBER_LOCATION),
                )
            })
            .collect();
        assert_eq!(
            members,
            [
                ("value".to_string(), "int".to_string(), Some(0)),
                ("next".to_string(), "struct node *".to_string(), Some(ptr)),
            ]
        );
        // A union's members are at 0, which clang says too.
        let word = named(dw::TAG_UNION_TYPE, "word");
        assert_eq!(at(word).unsigned(dw::AT_BYTE_SIZE), Some(4));
        assert!(children(word)
            .iter()
            .all(|&m| at(m).unsigned(dw::AT_DATA_MEMBER_LOCATION) == Some(0)));
        assert_eq!(type_of(children(word)[1]), "unsigned char[4]");
        // The anonymous struct and enum of box's members are inside box, each
        // after the member that is of it, as clang has them.
        let bx = named(dw::TAG_STRUCTURE_TYPE, "box");
        let tags: Vec<(u64, Option<&str>)> =
            children(bx).iter().map(|&c| (at(c).tag, at(c).name())).collect();
        assert_eq!(
            tags,
            [
                (dw::TAG_MEMBER, Some("k")),
                (dw::TAG_MEMBER, Some("in")),
                (dw::TAG_STRUCTURE_TYPE, None),
                (dw::TAG_MEMBER, Some("state")),
                (dw::TAG_ENUMERATION_TYPE, None),
            ]
        );
        assert_eq!(ty(at(children(bx)[1])), children(bx)[2]);
        assert_eq!(ty(at(children(bx)[3])), children(bx)[4]);
        let color = named(dw::TAG_ENUMERATION_TYPE, "color");
        assert_eq!(type_of(color), "unsigned int");
        let constants: Vec<(&str, Option<i64>)> = children(color)
            .iter()
            .map(|&e| {
                (
                    at(e).name().unwrap(),
                    at(e).attr(dw::AT_CONST_VALUE).and_then(Attr::signed),
                )
            })
            .collect();
        assert_eq!(
            constants,
            [("RED", Some(0)), ("GREEN", Some(5)), ("BLUE", Some(6))]
        );
        assert_eq!(type_of(named(dw::TAG_TYPEDEF, "color_t")), "enum color");
        // Plain char is unsigned on RISC-V; every type is described once.
        let char_ = named(dw::TAG_BASE_TYPE, "char");
        assert_eq!(at(char_).unsigned(dw::AT_ENCODING), Some(dw::ATE_UNSIGNED_CHAR));
        let ints = u
            .dies
            .iter()
            .filter(|d| d.tag == dw::TAG_BASE_TYPE && d.name() == Some("int"));
        assert_eq!(ints.count(), 1);
    }
}

/// llvm-dwarfdump finds nothing wrong with the debug information of any test
/// case, at either width, in its object as merlin-as assembles it (not linked:
/// a case need not be a whole program). Ignored: it needs llvm-dwarfdump (`LLVM_DWARFDUMP`
/// names it if it is not on the PATH).
#[test]
#[ignore]
fn llvm_dwarfdump_verifies_every_case() {
    let tool = std::env::var("LLVM_DWARFDUMP").unwrap_or_else(|_| "llvm-dwarfdump".into());
    let dir = std::env::temp_dir().join(format!("merlin-verify-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for x in [Xlen::Rv32, Xlen::Rv64] {
        let opts = Options {
            xlen: x,
            ..debug_opts("tests/cases")
        };
        let as_opts = merlin_as::Options { xlen: x };
        let mut sources: Vec<(String, String)> = cases()
            .iter()
            .map(|p| {
                let name = p.file_name().unwrap().to_string_lossy().into_owned();
                (name, std::fs::read_to_string(p).unwrap())
            })
            .collect();
        sources.push(("v.c".into(), VARS.into()));
        for (name, src) in sources {
            let asm = merlin::compile_with(&name, &src, &opts, &merlin::no_files).unwrap();
            let obj = merlin_as::assemble_to_elf_with(&asm, &as_opts).unwrap();
            let file = dir.join(format!("{name}-{}.o", x.bits()));
            std::fs::write(&file, &obj).unwrap();
            let out = std::process::Command::new(&tool)
                .arg("--verify")
                .arg(&file)
                .output()
                .unwrap_or_else(|e| panic!("cannot run {tool}: {e}"));
            let text = String::from_utf8_lossy(&out.stdout);
            assert!(
                out.status.success() && text.contains("No errors."),
                "{name} ({x:?}):\n{text}"
            );
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Every attribute merlin writes for [`VARS`] has the form clang gives it, at
/// both widths: the `DW_AT_* [DW_FORM_*]` pairs llvm-dwarfdump -v shows for
/// merlin's object are those it shows for clang's (clang -O0 -g; its own, such as
/// the producer's, are the same forms). Ignored: it needs clang and
/// llvm-dwarfdump (`CLANG` and `LLVM_DWARFDUMP` name them if they are not on the
/// PATH).
#[test]
#[ignore]
fn the_forms_are_clangs() {
    let clang = std::env::var("CLANG").unwrap_or_else(|_| "clang".into());
    let tool = std::env::var("LLVM_DWARFDUMP").unwrap_or_else(|_| "llvm-dwarfdump".into());
    let dir = std::env::temp_dir().join(format!("merlin-forms-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let source = dir.join("v.c");
    std::fs::write(&source, VARS).unwrap();
    let forms = |obj: &Path| -> std::collections::BTreeSet<String> {
        let out = std::process::Command::new(&tool)
            .args(["--debug-info", "-v"])
            .arg(obj)
            .output()
            .unwrap_or_else(|e| panic!("cannot run {tool}: {e}"));
        let text = String::from_utf8_lossy(&out.stdout).into_owned();
        text.lines()
            .filter_map(|l| {
                let l = l.trim();
                let end = l.find(']')?;
                (l.starts_with("DW_AT_") && l.contains("[DW_FORM_")).then(|| l[..=end].to_string())
            })
            .collect()
    };
    for x in [Xlen::Rv32, Xlen::Rv64] {
        let bits = x.bits();
        let opts = Options {
            xlen: x,
            ..debug_opts("dir")
        };
        let asm = merlin::compile_with("v.c", VARS, &opts, &merlin::no_files).unwrap();
        let obj = merlin_as::assemble_to_elf_with(&asm, &merlin_as::Options { xlen: x }).unwrap();
        let ours = dir.join(format!("merlin-{bits}.o"));
        std::fs::write(&ours, obj).unwrap();
        let theirs = dir.join(format!("clang-{bits}.o"));
        let abi = if bits == 64 { "lp64" } else { "ilp32" };
        let out = std::process::Command::new(&clang)
            .args([
                &format!("--target=riscv{bits}"),
                &format!("-march=rv{bits}ima_zicsr_zifencei"),
                &format!("-mabi={abi}"),
                "-O0",
                "-g",
                "-c",
            ])
            .arg(&source)
            .arg("-o")
            .arg(&theirs)
            .output()
            .unwrap_or_else(|e| panic!("cannot run {clang}: {e}"));
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        let (ours, theirs) = (forms(&ours), forms(&theirs));
        assert!(ours.contains("DW_AT_name [DW_FORM_strx1]"), "{ours:#?}");
        assert_eq!(ours, theirs, "merlin's (left) and clang's (right), {x:?}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// The names of the base types `src`'s unit describes, compiled with `-g`.
fn base_types(src: &str) -> Vec<String> {
    use elf::dies::dw;
    let asm = merlin::compile_with("c.c", src, &debug_opts("dir"), &merlin::no_files).unwrap();
    let obj = merlin_as::assemble_to_elf_with(&asm, &merlin_as::Options::default()).unwrap();
    let units = elf::Elf::parse(&obj).unwrap().debug_entries().unwrap();
    units[0]
        .dies
        .iter()
        .filter(|d| d.tag == dw::TAG_BASE_TYPE)
        .filter_map(|d| d.name().map(str::to_string))
        .collect()
}

/// The type of an explicit cast is kept only where the code evaluates it,
/// as clang's code generator keeps it: not in the operand `?:` does not
/// choose of a constant condition, which need not even be constant, nor in
/// the operand a constant `&&` or `||` leaves out or decides by, nor in the
/// arm an `if` on a constant does not take (whose variables are not
/// described either), nor in a constant `if` condition, nor in the cases a
/// `switch` on a constant does not run; but in a `while (1)`'s, which
/// clang's code generator still evaluates.
#[test]
fn casts_not_evaluated_are_not_kept() {
    let kept = |body: &str| {
        base_types(&format!(
            "int x; int h(void);\nint f(void) {{ return {body}; }}\n"
        ))
    };
    assert_eq!(kept("1 ? 2 : (unsigned short)x"), ["int"]);
    assert_eq!(kept("1 ? 2 : (unsigned short)h()"), ["int"]);
    assert_eq!(kept("0 ? (unsigned short)h() : 2"), ["int"]);
    assert_eq!(
        kept("0 ? 2 : (unsigned short)x"),
        ["unsigned short", "int"],
        "the types of casts come first"
    );
    assert_eq!(
        kept("((char)1 || (char)x) + (0 && (short)x) + ((short)1 && (unsigned short)x)"),
        ["unsigned short", "int"]
    );
    let kept = |body: &str| base_types(&format!("int x;\nvoid f(void) {{ {body} }}\n"));
    assert_eq!(kept("if (0) { long dead = (char)x; }"), ["int"]);
    assert_eq!(kept("if ((char)1) x = 1;"), ["int"]);
    assert_eq!(
        kept("if (0) { l: x = (char)2; } goto l;"),
        ["char", "int"],
        "kept for its label"
    );
    assert_eq!(kept("while ((char)1) break;"), ["char", "int"]);
    assert_eq!(kept("do x++; while ((char)0);"), ["char", "int"]);
    assert_eq!(
        kept("switch (1) { case 1: x = (char)1; break; case 2: x = (short)2; }"),
        ["char", "int"],
        "only the case that runs"
    );
}

/// The names of the entries of `tag` that `src` describes, in order.
fn named(src: &str, tag: u64) -> Vec<String> {
    let asm = merlin::compile_with("c.c", src, &debug_opts("dir"), &merlin::no_files).unwrap();
    let obj = merlin_as::assemble_to_elf_with(&asm, &merlin_as::Options::default()).unwrap();
    let units = elf::Elf::parse(&obj).unwrap().debug_entries().unwrap();
    units[0]
        .dies
        .iter()
        .filter(|d| d.tag == tag)
        .filter_map(|d| d.name().map(str::to_string))
        .collect()
}

/// In a static initializer, clang keeps the types of the explicit casts its
/// `ConstExprEmitter` visits (clang 23): an element's own cast, and the casts
/// inside a cast that changes nothing or converts an integer to an integer,
/// or inside a unary `-`; not those under any other operator, `?:` or a
/// cast of an address to an integer.
#[test]
fn initializer_casts_are_kept_as_clang_keeps_them() {
    let names = ["A", "B", "C", "D", "E", "F", "G", "H", "I", "J", "K"];
    let mut src: String = names.iter().map(|n| format!("typedef long {n};\n")).collect();
    src.push_str(
        "int x[4];\nstruct s { long a, b; };\n\
         long a = (A)&x;\n\
         long b = (B)&x + 4;\n\
         long c[2] = {(C)&x, 0};\n\
         struct s d = {(D)&x, 0};\n\
         int *e = (int *)(E)&x;\n\
         long f = (F)(G)&x;\n\
         long g = 1 ? (H)&x : 0;\n\
         long h = -(I)3;\n\
         int i = (J)3;\n\
         long j = ~(K)3;\n\
         int main(void) { return 0; }\n",
    );
    assert_eq!(
        named(&src, elf::dies::dw::TAG_TYPEDEF),
        ["A", "C", "D", "F", "G", "I", "J"]
    );
}

/// clang describes the enum of an enumeration constant its code uses, even
/// with no variable of it (clang 23): where the code evaluates the constant,
/// and in a `case` label that is the constant alone; not in a static
/// initializer, a constant condition, `sizeof`, an operand left out or a
/// constant `switch`'s labels. An enum a function defines is its own.
#[test]
fn enums_are_described_for_the_constants_the_code_uses() {
    let names = [
        "A", "B", "C", "D", "E", "F", "G", "H", "I", "J", "K", "L", "M", "N", "O",
    ];
    let mut src: String = names.iter().map(|n| format!("enum {{ {n} = 1 }};\n")).collect();
    src.push_str(
        "int g1 = A;\nint arr[4];\n\
         int f(int x) {\n\
             if (B) x++;\n\
             x = C ? 1 : 2;\n\
             x = sizeof(D);\n\
             switch (x) { case E: break; case F + 1: break; }\n\
             x = 0 && G;\n\
             arr[H] = 1;\n\
             x = (long)I;\n\
             x = 1 ? J : K;\n\
             while (L) break;\n\
             switch (2) { case M: x = 1; break; case 2: x = N; break; }\n\
             { enum in { Q = 3 }; x = Q; }\n\
             return -O + x;\n\
         }\n",
    );
    assert_eq!(
        named(&src, elf::dies::dw::TAG_ENUMERATOR),
        ["E", "H", "I", "J", "L", "N", "O", "Q"]
    );
}

/// The types a function defines (a typedef, a struct, one in an inner block)
/// are described in its entry, after its variables and blocks, as clang has
/// them; a file-scope type of the same tag is the unit's.
#[test]
fn a_functions_types_are_its_own() {
    use elf::dies::dw;
    let src = "struct pt { long l; } g;\n\
               int f(void) {\n    typedef int T;\n    T t = 1;\n    struct pt { int q; } loc;\n    loc.q = t;\n    \
               {\n        struct in { char c; } b;\n        b.c = 2;\n    }\n    return loc.q;\n}\n";
    let asm = merlin::compile_with("l.c", src, &debug_opts("dir"), &merlin::no_files).unwrap();
    let obj = merlin_as::assemble_to_elf_with(&asm, &merlin_as::Options::default()).unwrap();
    let units = elf::Elf::parse(&obj).unwrap().debug_entries().unwrap();
    let u = &units[0];
    let f = u.dies.iter().position(|d| d.name() == Some("f")).unwrap();
    let kids: Vec<(u64, Option<&str>)> = u.dies[f]
        .children
        .iter()
        .map(|&c| (u.dies[c].tag, u.dies[c].name()))
        .collect();
    assert_eq!(
        kids,
        [
            (dw::TAG_VARIABLE, Some("t")),
            (dw::TAG_VARIABLE, Some("loc")),
            (dw::TAG_LEXICAL_BLOCK, None),
            (dw::TAG_TYPEDEF, Some("T")),
            (dw::TAG_STRUCTURE_TYPE, Some("pt")),
            (dw::TAG_STRUCTURE_TYPE, Some("in")),
        ]
    );
    let outer: Vec<usize> = (0..u.dies.len())
        .filter(|&i| u.dies[i].tag == dw::TAG_STRUCTURE_TYPE && u.dies[i].parent == Some(0))
        .collect();
    assert_eq!(outer.len(), 1);
    assert_eq!(u.dies[u.dies[outer[0]].children[0]].name(), Some("l"));
}

/// The entries of a unit as llvm-dwarfdump shows them: each one's depth, tag
/// and name (or, without one, the name of its type), the unit's own left out.
fn entry_tree(tool: &str, obj: &Path) -> Vec<String> {
    let out = std::process::Command::new(tool)
        .arg("--debug-info")
        .arg(obj)
        .output()
        .unwrap_or_else(|e| panic!("cannot run {tool}: {e}"));
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    let mut entries: Vec<(usize, String, String, String)> = Vec::new();
    for l in text.lines() {
        if let Some(rest) = l
            .strip_prefix("0x")
            .and_then(|r| r.split_once(':'))
            .map(|(_, r)| r)
        {
            let tag = rest.trim_start();
            if tag.starts_with("DW_TAG_") || tag == "NULL" {
                let depth = rest.len() - tag.len();
                let tag = tag.split_whitespace().next().unwrap().to_string();
                entries.push((depth, tag, String::new(), String::new()));
                continue;
            }
        }
        let Some(e) = entries.last_mut() else { continue };
        let quoted = |l: &str| {
            l.split_once("(\"")
                .map(|(_, r)| r.trim_end_matches("\")").to_string())
        };
        if l.trim_start().starts_with("DW_AT_name") {
            // The unit's file and directory names are paths, which differ.
            e.2 = quoted(l)
                .filter(|n| !n.contains(['/', '\\', ':']))
                .unwrap_or_default();
        } else if l.trim_start().starts_with("DW_AT_type") {
            e.3 = l
                .split_once(" \"")
                .map(|(_, r)| r.trim_end_matches("\")").to_string())
                .unwrap_or_default();
        }
    }
    entries
        .into_iter()
        .filter(|e| e.1 != "DW_TAG_compile_unit")
        .map(|(depth, tag, name, ty)| {
            let what = if name.is_empty() { format!("-> {ty}") } else { name };
            format!("{}{tag} {what}", " ".repeat(depth))
        })
        .collect()
}

/// merlin's entries are clang's, in clang's order (see `src/debug/order.rs`):
/// for [`VARS`], merlin-dbg's tests/vars/vars.c and every test case, at both
/// widths, the tree of tags and names llvm-dwarfdump shows is the same, with
/// no exception.
///
/// Ignored: it needs clang and llvm-dwarfdump (`CLANG` and `LLVM_DWARFDUMP`
/// name them if they are not on the PATH).
#[test]
#[ignore]
fn the_entries_are_clangs_in_clangs_order() {
    let clang = std::env::var("CLANG").unwrap_or_else(|_| "clang".into());
    let tool = std::env::var("LLVM_DWARFDUMP").unwrap_or_else(|_| "llvm-dwarfdump".into());
    let dir = std::env::temp_dir().join(format!("merlin-order-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut sources: Vec<(String, String)> = vec![
        ("v.c".into(), VARS.into()),
        (
            "vars.c".into(),
            std::fs::read_to_string(
                Path::new(env!("CARGO_MANIFEST_DIR")).join("../merlin-dbg/tests/vars/vars.c"),
            )
            .unwrap(),
        ),
    ];
    for p in cases() {
        let name = p.file_name().unwrap().to_string_lossy().into_owned();
        sources.push((name, std::fs::read_to_string(&p).unwrap()));
    }
    for x in [Xlen::Rv32, Xlen::Rv64] {
        let bits = x.bits();
        let abi = if bits == 64 { "lp64" } else { "ilp32" };
        let opts = Options {
            xlen: x,
            ..debug_opts("dir")
        };
        for (name, src) in &sources {
            let source = dir.join(name);
            std::fs::write(&source, src).unwrap();
            let asm = merlin::compile_with(name, src, &opts, &merlin::no_files).unwrap();
            let obj = merlin_as::assemble_to_elf_with(&asm, &merlin_as::Options { xlen: x }).unwrap();
            let ours = dir.join(format!("{name}-merlin-{bits}.o"));
            std::fs::write(&ours, obj).unwrap();
            let theirs = dir.join(format!("{name}-clang-{bits}.o"));
            let out = std::process::Command::new(&clang)
                .args([
                    &format!("--target=riscv{bits}"),
                    &format!("-march=rv{bits}ima_zicsr_zifencei"),
                    &format!("-mabi={abi}"),
                    "-O0",
                    "-g",
                    "-ffreestanding",
                    "-fno-builtin",
                    "-w",
                    "-c",
                ])
                .arg(&source)
                .arg("-o")
                .arg(&theirs)
                .output()
                .unwrap_or_else(|e| panic!("cannot run {clang}: {e}"));
            assert!(
                out.status.success(),
                "{name}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            let (ours, theirs) = (entry_tree(&tool, &ours), entry_tree(&tool, &theirs));
            assert!(ours.len() > 2, "{name}: {ours:#?}");
            assert_eq!(ours, theirs, "{name}: merlin's (left) and clang's (right), {x:?}");
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}
