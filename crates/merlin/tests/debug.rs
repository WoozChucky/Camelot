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

/// On RV64 the compile unit's addresses are 8 bytes: its header says so,
/// `DW_FORM_addr` is a `.quad` (an R_RISCV_64 relocation), and the unit's
/// length counts them. The line table reads back as on RV32.
#[test]
fn rv64_addresses_are_eight_bytes() {
    let src = "int f(int n) {\n    return n + 1;\n}\nlong g(long x) {\n    return f(x);\n}\n";
    let asm = merlin::compile_with("w.c", src, &debug_opts_rv64("dir"), &merlin::no_files).unwrap();
    assert!(asm.contains("    .byte 8    # address size\n"), "{asm}");
    assert!(asm.contains("    .quad f\n"), "{asm}");
    assert!(!asm.contains(".word f\n"), "{asm}");
    let obj = assemble_rv64(&asm);
    let elf = elf::Elf::parse(&obj).unwrap();
    let (i, info) = elf
        .sections
        .iter()
        .enumerate()
        .find(|(_, s)| s.name == ".debug_info")
        .unwrap();
    let data = elf.section_data(info).unwrap();
    let length = u32::from_le_bytes(data[..4].try_into().unwrap());
    assert_eq!(length as usize + 4, data.len());
    assert_eq!(u16::from_le_bytes([data[4], data[5]]), 5, "DWARF 5");
    assert_eq!(data[7], 8, "address size");
    // The unit's low_pc and each function's: 8-byte relocations.
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
    assert_eq!(addrs, ["f", "f", "g"]);
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
long walk(Node *list, color_t c) {
    static int calls;
    long sum = 0;
    calls++;
    {
        union word w;
        w.u = c;
        for (int i = 0; i < 2; i++) {
            Node *p = list;
            sum += p->value + w.b[0] + grid[i][1];
        }
    }
    return sum;
}
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
        assert_eq!(at(kids[1]).unsigned(dw::AT_DECL_LINE), Some(12));
        // The static local is at its symbol, the other in the frame.
        let calls = kids[2];
        assert_eq!(at(calls).name(), Some("calls"));
        let loc = at(calls).block(dw::AT_LOCATION).unwrap();
        assert_eq!((loc[0], loc.len() as u64), (0x03, 1 + ptr), "DW_OP_addr");
        assert_eq!(at(kids[3]).name(), Some("sum"));
        assert_eq!(
            at(kids[3]).block(dw::AT_LOCATION).unwrap(),
            slot("local long sum").as_slice()
        );
        // The block of `w`, and in it the `for`'s, with `i` and its body's,
        // with `p`: each inside the one around it.
        let block = kids[4];
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
        let symbols = elf.symbols().unwrap();
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
            let addr = loc[1..].iter().rev().fold(0u64, |a, &b| a << 8 | u64::from(b));
            assert_eq!((loc[0], addr), (0x03, sym.value), "{name}");
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
        let word = named(dw::TAG_UNION_TYPE, "word");
        assert_eq!(at(word).unsigned(dw::AT_BYTE_SIZE), Some(4));
        assert!(children(word)
            .iter()
            .all(|&m| at(m).attr(dw::AT_DATA_MEMBER_LOCATION).is_none()));
        assert_eq!(type_of(children(word)[1]), "unsigned char[4]");
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
/// case, at either width, linked. Ignored: it needs llvm-dwarfdump (`LLVM_DWARFDUMP`
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
