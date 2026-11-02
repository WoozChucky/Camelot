//! The merlin end-to-end harness.
//!
//! Every `tests/cases/*.c` is a whole program whose first line is
//! `// expect: N`: the exit code it must produce (main's return value, low 8
//! bits). To add a test, drop a new `.c` file in `tests/cases/`.
//!
//! `tests/cases-merlin/*.c` are the same kind of program, but they rely on how
//! merlin defines what C leaves undefined (spec section 10: wrapping overflow,
//! division by zero, `INT_MIN / -1`, falling off the end of a function). Every
//! test below runs them except `clang_reference`, since clang may disagree.
//!
//! Run by `cargo test`, without any external tools:
//!
//! * `cases_are_well_formed`: every case has an expectation and passes the
//!   front end (lexer, parser, name resolution, arity checks).
//! * `cases_match_reference_interpreter`: an AST interpreter
//!   (`tests/common/interp.rs`) agrees with every `// expect:`, so the
//!   expectations and the parser check each other, and a pipeline failure
//!   below can be told apart from a front-end bug.
//! * `cases_run_in_grail`: the whole toolchain. merlin compiles each case to
//!   assembly, merlin-as assembles it and `tests/crt0.s`, merlin-ld links them
//!   (default layout) and Grail runs the result, comparing exit codes. Set
//!   `MERLIN_HARNESS_LLVM=1` to assemble merlin's output with clang and link
//!   with ld.lld instead, which checks the generated assembly against LLVM:
//!
//!       MERLIN_HARNESS_LLVM=1 cargo test -p merlin --test harness run_in_grail
//!
//! * `far_branches_run_in_grail`, `deep_expressions_run_in_grail`: generated
//!   programs too big to keep as files, through the same pipeline.
//! * `multi_file_programs_run_in_grail`: programs of several files, each a
//!   directory `tests/multi/<name>/`: merlin compiles every `.c` file (with
//!   `include/` as `-I`, if there is one), merlin-as assembles every `.s`
//!   file, and merlin-ld links them all with crt0. The first line of
//!   `main.c` is `// expect: N`.
//! * `kernel_sources_compile`: merlin compiles every C file of the Camelot
//!   kernel and its user programs (`kernel/*.c`, `kernel/user/*.c`), and
//!   merlin-as assembles the result. Nothing runs here: `cargo xtask kernel`
//!   builds the kernel this way, and xtask's tests boot it.
//!
//! Ignored, since they need LLVM:
//!
//! * `generated_assembly_assembles_like_clang`: clang's assembler (with
//!   `-mno-relax`) turns merlin's output for every case into the same
//!   `.text`, `.rodata`, `.data` and `.bss` contents and relocations as
//!   merlin-as. Needs clang on `PATH`.
//!
//!       cargo test -p merlin --test harness -- --ignored generated_assembly
//!
//! * `multi_file_clang_reference`: the multi-file programs, built with clang
//!   and ld.lld instead.
//!
//!       cargo test -p merlin --test harness -- --ignored multi_file_clang
//!
//! * `clang_reference`: compiles every case with clang instead of merlin, links
//!   with ld.lld and runs it in Grail. It checks the expectations and crt0
//!   against a real C compiler (crt0's `memset` and `memcpy` are there for
//!   clang, which calls them to clear large local arrays and copy large
//!   structs). Needs clang and ld.lld on `PATH` (the official Windows LLVM
//!   installer is enough).
//!
//!       cargo test -p merlin --test harness -- --ignored clang_reference

#[path = "common/interp.rs"]
mod interp;

use grail::{Machine, RunResult};
use std::path::{Path, PathBuf};
use std::process::Command;

struct Case {
    name: String,
    path: PathBuf,
    src: String,
    expect: u8,
}

fn tests_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests")
}

/// Plain C99 programs: clang must agree with their expectations.
const C99_CASES: &str = "cases";
/// Programs relying on how merlin defines what C leaves undefined (spec section
/// 10): wrapping overflow, division by zero, falling off the end of a function.
/// clang_reference skips them.
const MERLIN_CASES: &str = "cases-merlin";
const ALL_CASES: &[&str] = &[C99_CASES, MERLIN_CASES];

fn cases(dirs: &[&str]) -> Vec<Case> {
    let mut paths = Vec::new();
    for dir in dirs {
        let dir = tests_dir().join(dir);
        let mut in_dir: Vec<PathBuf> = std::fs::read_dir(&dir)
            .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension().is_some_and(|x| x == "c"))
            .collect();
        in_dir.sort();
        paths.extend(in_dir);
    }
    paths
        .into_iter()
        .map(|path| {
            let name = path.file_stem().unwrap().to_string_lossy().into_owned();
            let src = std::fs::read_to_string(&path).unwrap();
            let expect = src
                .lines()
                .next()
                .and_then(|l| l.trim().strip_prefix("// expect:"))
                .and_then(|n| n.trim().parse().ok())
                .unwrap_or_else(|| panic!("{name}.c: first line must be `// expect: N` with N in 0..=255"));
            Case {
                name,
                path,
                src,
                expect,
            }
        })
        .collect()
}

/// Runs `check` on every case, prints a summary, and fails listing every mismatch.
fn for_each_case(what: &str, dirs: &[&str], mut check: impl FnMut(&Case) -> Result<u8, String>) {
    let cases = cases(dirs);
    let mut failures = Vec::new();
    for c in &cases {
        match check(c) {
            Ok(code) if code == c.expect => {}
            Ok(code) => failures.push(format!("  {}: exit code {code}, expected {}", c.name, c.expect)),
            Err(e) => failures.push(format!("  {}: {e}", c.name)),
        }
    }
    println!(
        "{what}: {}/{} cases pass",
        cases.len() - failures.len(),
        cases.len()
    );
    assert!(
        failures.is_empty(),
        "{what}: {} of {} cases failed:\n{}",
        failures.len(),
        cases.len(),
        failures.join("\n")
    );
}

fn render(errors: &[merlin::Error], case: &Case) -> String {
    let file = format!("{}.c", case.name);
    errors
        .iter()
        .map(|e| e.render(&file))
        .collect::<Vec<_>>()
        .join("; ")
}

#[test]
fn cases_are_well_formed() {
    let n = cases(&[C99_CASES]).len();
    assert!(n >= 20, "only {n} C99 cases");
    let n = cases(&[MERLIN_CASES]).len();
    assert!(n >= 4, "only {n} cases for merlin's defined behavior");
    for_each_case("front end", ALL_CASES, |c| {
        let prog = merlin::parse(&c.src).map_err(|e| render(&e, c))?;
        if !prog.funcs().any(|f| f.name == "main" && f.body.is_some()) {
            return Err("no main".into());
        }
        Ok(c.expect)
    });
}

#[test]
fn cases_match_reference_interpreter() {
    for_each_case("reference interpreter", ALL_CASES, |c| {
        let prog = merlin::parse(&c.src).map_err(|e| render(&e, c))?;
        interp::run(&prog, "main").map(|v| v as u8)
    });
}

#[test]
fn reference_interpreter_semantics() {
    let eval = |body: &str| {
        let prog = merlin::parse(&format!("int id(int x) {{ return x; }}\nint main() {{ {body} }}")).unwrap();
        interp::run(&prog, "main").unwrap()
    };
    assert_eq!(eval("return 2147483647 + 1;"), i32::MIN);
    assert_eq!(eval("return -2147483647 - 1 - 1;"), i32::MAX);
    assert_eq!(eval("return (-2147483647 - 1) / -1;"), i32::MIN);
    assert_eq!(eval("return (-2147483647 - 1) % -1;"), 0);
    assert_eq!(eval("return 7 / id(0);"), -1);
    assert_eq!(eval("return 7 % id(0);"), 7);
    assert_eq!(eval("return 0x7fffffff + 1;"), i32::MIN);
    // 0xffffffff is an `unsigned int`; a decimal literal that big would be
    // `long long`, which merlin rejects.
    assert_eq!(eval("return 0xffffffff;"), -1);
    assert_eq!(
        eval("return 0xffffffff / 2 == 0x7fffffff;"),
        1,
        "unsigned division"
    );
    assert_eq!(eval("return -1 < 0u;"), 0, "-1 converts to UINT_MAX");
    assert_eq!(eval("return 7u / id(0) == 0xffffffff;"), 1, "divu by zero");
    assert_eq!(eval("return -8 >> 1;"), -4);
    assert_eq!(eval("return 0x80000000 >> 31;"), 1);
    assert_eq!(
        eval("return 1 << id(33);"),
        2,
        "the low 5 bits of the shift amount"
    );
    assert_eq!(
        eval("return (char)300 + (signed char)200 + (unsigned short)-1;"),
        44 - 56 + 65535
    );
    assert_eq!(eval("char c = 255; c++; return c;"), 0);
    assert_eq!(
        eval("int a[3] = {1, 2}; int *p = a + 2; return p - a + *(p - 1) * 10 + a[2];"),
        22
    );
    assert!(merlin::parse("int main() { return 2147483648; }").is_err());
    assert_eq!(eval("return 3 == 3;"), 1);
    assert_eq!(eval("return !7 || 5 && 9;"), 1);
    assert_eq!(eval("int x = 1; x = 5;"), 0, "falling off the end returns 0");
    assert_eq!(
        eval("int i = 0; while (1) { i = i + 1; if (i == 3) return i; }"),
        3
    );
    let prog = merlin::parse("int f(int n) { return f(n + 1); } int main() { return f(0); }").unwrap();
    assert!(interp::run(&prog, "main").unwrap_err().contains("recursion"));
    let prog = merlin::parse("int g(); int main() { return g(); }").unwrap();
    assert!(interp::run(&prog, "main").unwrap_err().contains("no definition"));
    let prog = merlin::parse("int main() { int *p = 0; return *p; }").unwrap();
    assert!(interp::run(&prog, "main")
        .unwrap_err()
        .contains("outside every object"));
    let prog = merlin::parse("extern int e; int main() { return e; }").unwrap();
    assert!(interp::run(&prog, "main").unwrap_err().contains("never defined"));
}

/// Loads an executable into Grail and returns its exit code.
fn run_in_grail(elf: &[u8]) -> Result<u8, String> {
    let mut m = Machine::with_ram(grail::DEFAULT_RAM_SIZE);
    m.load_elf(elf)?;
    match m.run(Some(100_000_000), |_| {}) {
        RunResult::Exited(code) => Ok(code as u8),
        RunResult::StepLimit => Err(format!("still running at pc={:#010x}", m.cpu.pc)),
    }
}

/// A scratch directory per test, removed when dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(test: &str) -> Scratch {
        let dir = std::env::temp_dir().join(format!("merlin-harness-{test}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        Scratch(dir)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Runs a tool and returns its stderr as the error when it fails.
fn tool(cmd: &mut Command) -> Result<(), String> {
    let prog = cmd.get_program().to_string_lossy().into_owned();
    let out = cmd.output().map_err(|e| format!("cannot run {prog}: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "{prog} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

fn clang() -> Command {
    let mut c = Command::new("clang");
    c.args(["--target=riscv32", "-march=rv32im", "-mabi=ilp32", "-c"]);
    c
}

/// clang doesn't know Xcal mnemonics; spell `xcal.halt a0` as the raw encoding.
fn crt0_for_llvm(crt0: &str) -> String {
    crt0.replace("xcal.halt a0", ".insn i 0x0b, 1, x0, a0, 0")
}

/// merlin-ld's fixed layout (spec section 10): everything from 0x8000_0000.
const LINK_SCRIPT: &str = "ENTRY(_start)
SECTIONS {
    . = 0x80000000;
    .text : { *(.text*) }
    .rodata : { *(.rodata*) *(.srodata*) }
    .data : { *(.data*) *(.sdata*) }
    .bss : { *(.bss*) *(.sbss*) *(COMMON) }
}
";

fn ld_lld(objs: &[PathBuf], out: &Path) -> Result<Vec<u8>, String> {
    let script = out.with_extension("ld");
    std::fs::write(&script, LINK_SCRIPT).map_err(|e| e.to_string())?;
    // Spelled out because Windows only appends `.exe` to names without a dot.
    tool(
        Command::new(format!("ld.lld{}", std::env::consts::EXE_SUFFIX))
            .arg("-T")
            .arg(&script)
            .arg("-o")
            .arg(out)
            .args(objs),
    )?;
    std::fs::read(out).map_err(|e| e.to_string())
}

/// Assembling and linking for the pipeline tests.
enum Backend {
    /// merlin-as and merlin-ld, as libraries.
    Merlin,
    /// clang and ld.lld (`MERLIN_HARNESS_LLVM=1`), to cross-check merlin's output.
    Llvm,
}

impl Backend {
    fn from_env() -> Backend {
        if std::env::var_os("MERLIN_HARNESS_LLVM").is_some_and(|v| v != "0") {
            Backend::Llvm
        } else {
            Backend::Merlin
        }
    }

    fn assemble(&self, src: &str, out: &Path) -> Result<PathBuf, String> {
        match self {
            Backend::Merlin => {
                let obj = merlin_as::assemble_to_elf(src).map_err(|errs| {
                    let msgs: Vec<String> = errs.iter().map(|e| e.to_string()).collect();
                    format!("merlin-as: {}", msgs.join("; "))
                })?;
                std::fs::write(out, obj).map_err(|e| e.to_string())?;
            }
            Backend::Llvm => {
                let s = out.with_extension("s");
                std::fs::write(&s, src).map_err(|e| e.to_string())?;
                tool(clang().arg("-mno-relax").arg(&s).arg("-o").arg(out))?;
            }
        }
        Ok(out.to_path_buf())
    }

    fn crt0(&self) -> String {
        let crt0 = std::fs::read_to_string(tests_dir().join("crt0.s")).unwrap();
        match self {
            Backend::Merlin => crt0,
            Backend::Llvm => crt0_for_llvm(&crt0),
        }
    }

    fn link(&self, objs: &[PathBuf], out: &Path) -> Result<Vec<u8>, String> {
        match self {
            Backend::Merlin => {
                let mut inputs = Vec::new();
                for p in objs {
                    inputs.push(merlin_ld::Input {
                        name: p.file_name().unwrap().to_string_lossy().into_owned(),
                        data: std::fs::read(p).map_err(|e| e.to_string())?,
                    });
                }
                let linked = merlin_ld::link(&inputs, &merlin_ld::Options::default()).map_err(|errs| {
                    let msgs: Vec<String> = errs.iter().map(|e| e.to_string()).collect();
                    format!("merlin-ld: {}", msgs.join("; "))
                })?;
                Ok(linked.elf)
            }
            Backend::Llvm => ld_lld(objs, out),
        }
    }
}

/// Builds C programs with crt0 into executables, and runs them in Grail.
struct Pipeline {
    backend: Backend,
    scratch: Scratch,
    crt0: PathBuf,
}

impl Pipeline {
    fn new(test: &str) -> Pipeline {
        let backend = Backend::from_env();
        let scratch = Scratch::new(test);
        let crt0 = backend
            .assemble(&backend.crt0(), &scratch.path("crt0.o"))
            .expect("assembling crt0.s");
        Pipeline {
            backend,
            scratch,
            crt0,
        }
    }

    /// Assembles merlin's output and links it with crt0.
    fn build(&self, name: &str, asm: &str) -> Result<Vec<u8>, String> {
        let obj = self
            .backend
            .assemble(asm, &self.scratch.path(&format!("{name}.o")))?;
        let elf = self.scratch.path(&format!("{name}.elf"));
        self.backend.link(&[self.crt0.clone(), obj], &elf)
    }

    /// Compiles `src`, builds it and returns its exit code.
    fn run(&self, name: &str, src: &str) -> Result<u8, String> {
        let asm = merlin::compile(src).map_err(|errs| {
            let file = format!("{name}.c");
            errs.iter()
                .map(|e| e.render(&file))
                .collect::<Vec<_>>()
                .join("; ")
        })?;
        run_in_grail(&self.build(name, &asm)?)
    }
}

#[test]
fn cases_run_in_grail() {
    let pipeline = Pipeline::new("pipeline");
    for_each_case("merlin pipeline", ALL_CASES, |c| pipeline.run(&c.name, &c.src));
}

/// A conditional branch reaches only 4 KiB, so codegen must rewrite the ones
/// that span bigger `if` and loop bodies (merlin-as rejects them otherwise).
#[test]
fn far_branches_run_in_grail() {
    let pipeline = Pipeline::new("far");
    // `x = x + 1;` is 8 instructions, so 500 of them take 16 KB.
    let body = "x = x + 1; ".repeat(500);
    let src = format!(
        "int main() {{
            int x = 0;
            int i = 0;
            int y = 1;
            if (y) {{ {body} }} else {{ {body} }}
            while (i < 3) {{ {body} i = i + 1; if (i == 99) break; }}
            if (y == 0 && x == 0) {{ {body} }}
            for (i = 0; i < 2; i = i + 1) {{ if (i) continue; {body} }}
            return x / 100 + y;
        }}"
    );
    let asm = merlin::compile(&src).unwrap();
    assert!(asm.contains("\n1:\n"), "expected long branches:\n{asm}");
    // x = 500 (if) + 1500 (while) + 500 (for, first pass) = 2500.
    assert_eq!(pipeline.run("far", &src), Ok(26));
}

/// Deeply nested expressions keep many temporaries on the stack, around calls.
#[test]
fn deep_expressions_run_in_grail() {
    let pipeline = Pipeline::new("deep");
    // 1 + (2 + (... + (99 + id(100)))): the call happens with 99 values pushed.
    let mut e = "id(100)".to_string();
    for i in (1..100).rev() {
        e = format!("{i} + ({e})");
    }
    let src = format!("int id(int x) {{ return x; }}\nint main() {{ return ({e}) - 5000; }}");
    assert_eq!(pipeline.run("deep", &src), Ok(50)); // 5050 - 5000

    // Calls as arguments of calls, eight arguments each, inside an expression.
    let src = "int w(int a, int b, int c, int d, int e, int f, int g, int h) {
                   return a - b + c - d + e - f + g - h;
               }
               int main() {
                   return 100 + w(w(8, 7, 6, 5, 4, 3, 2, 1), 1, w(1, 1, 1, 1, 1, 1, 1, w(0, 0, 0, 0, 0, 0, 0, 3)), 0, 0, 0, 0, 5);
               }";
    // w(8, ..., 1) = 4; w(0, ..., 3) = -3; w(1, ..., 1, -3) = 4; w(4, 1, 4, 0, 0, 0, 0, 5) = 2.
    assert_eq!(pipeline.run("args", src), Ok(102));
}

/// A program of several files, in `tests/multi/<name>/`.
struct MultiCase {
    name: String,
    /// `include/`, if the program has one: the `-I` directory.
    include: Option<PathBuf>,
    c_files: Vec<PathBuf>,
    s_files: Vec<PathBuf>,
    expect: u8,
}

fn multi_cases() -> Vec<MultiCase> {
    let root = tests_dir().join("multi");
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(&root)
        .unwrap_or_else(|e| panic!("{}: {e}", root.display()))
        .map(|e| e.unwrap().path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    dirs.into_iter()
        .map(|dir| {
            let name = dir.file_name().unwrap().to_string_lossy().into_owned();
            let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
                .unwrap()
                .map(|e| e.unwrap().path())
                .collect();
            files.sort();
            let with = |ext: &str| -> Vec<PathBuf> {
                files
                    .iter()
                    .filter(|p| p.extension().is_some_and(|x| x == ext))
                    .cloned()
                    .collect()
            };
            let main =
                std::fs::read_to_string(dir.join("main.c")).unwrap_or_else(|e| panic!("{name}/main.c: {e}"));
            let expect = main
                .lines()
                .next()
                .and_then(|l| l.trim().strip_prefix("// expect:"))
                .and_then(|n| n.trim().parse().ok())
                .unwrap_or_else(|| {
                    panic!("{name}/main.c: first line must be `// expect: N` with N in 0..=255")
                });
            let include = Some(dir.join("include")).filter(|d| d.is_dir());
            MultiCase {
                name,
                include,
                c_files: with("c"),
                s_files: with("s"),
                expect,
            }
        })
        .collect()
}

#[test]
fn multi_file_programs_run_in_grail() {
    let pipeline = Pipeline::new("multi");
    let cases = multi_cases();
    assert!(!cases.is_empty());
    for case in cases {
        let opts = merlin::Options {
            include_dirs: case.include.iter().cloned().collect(),
            macros: Vec::new(),
        };
        let mut objs = vec![pipeline.crt0.clone()];
        for c in &case.c_files {
            let asm = merlin::compile_file(c, &opts).unwrap_or_else(|errs| {
                let msgs: Vec<String> = errs.iter().map(|e| e.render(&c.to_string_lossy())).collect();
                panic!("{}: {}", case.name, msgs.join("\n"))
            });
            let stem = c.file_stem().unwrap().to_string_lossy();
            let obj = pipeline.scratch.path(&format!("{}-{stem}.o", case.name));
            objs.push(pipeline.backend.assemble(&asm, &obj).unwrap());
        }
        for s in &case.s_files {
            let stem = s.file_stem().unwrap().to_string_lossy();
            let obj = pipeline.scratch.path(&format!("{}-{stem}-s.o", case.name));
            let src = std::fs::read_to_string(s).unwrap();
            objs.push(pipeline.backend.assemble(&src, &obj).unwrap());
        }
        let elf = pipeline.scratch.path(&format!("{}.elf", case.name));
        let elf = pipeline
            .backend
            .link(&objs, &elf)
            .unwrap_or_else(|e| panic!("{}: {e}", case.name));
        assert_eq!(run_in_grail(&elf), Ok(case.expect), "{}", case.name);
    }
}

#[test]
#[ignore = "needs clang and ld.lld on PATH"]
fn multi_file_clang_reference() {
    let scratch = Scratch::new("multi-clang");
    let crt0_s = scratch.path("crt0.s");
    let crt0 = std::fs::read_to_string(tests_dir().join("crt0.s")).unwrap();
    std::fs::write(&crt0_s, crt0_for_llvm(&crt0)).unwrap();
    let crt0_o = scratch.path("crt0.o");
    tool(clang().arg(&crt0_s).arg("-o").arg(&crt0_o)).expect("assembling crt0.s with clang");
    for case in multi_cases() {
        let mut objs = vec![crt0_o.clone()];
        for f in case.c_files.iter().chain(&case.s_files) {
            let stem = f.file_stem().unwrap().to_string_lossy();
            let ext = f.extension().unwrap().to_string_lossy();
            let obj = scratch.path(&format!("{}-{stem}-{ext}.o", case.name));
            let mut cmd = clang();
            cmd.args(["-std=c99", "-O1", "-ffreestanding", "-fno-builtin"]);
            if let Some(dir) = &case.include {
                cmd.arg("-I").arg(dir);
            }
            tool(cmd.arg(f).arg("-o").arg(&obj)).unwrap();
            objs.push(obj);
        }
        let elf = ld_lld(&objs, &scratch.path(&format!("{}.elf", case.name))).unwrap();
        assert_eq!(run_in_grail(&elf), Ok(case.expect), "{}", case.name);
    }
}

/// Every C file of the kernel (its network stack in `kernel/net` too) and its
/// user programs compiles with merlin, with the include directories `cargo xtask
/// kernel` gives clang, and the result assembles.
#[test]
fn kernel_sources_compile() {
    let kernel = tests_dir().join("../../../kernel");
    let mut failures = Vec::new();
    let mut count = 0;
    for (dir, includes) in [
        (kernel.clone(), vec![kernel.join("include")]),
        (kernel.join("net"), vec![kernel.join("include")]),
        (kernel.join("user"), vec![kernel.join("user")]),
    ] {
        let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
            .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension().is_some_and(|x| x == "c"))
            .collect();
        files.sort();
        let opts = merlin::Options {
            include_dirs: includes,
            macros: Vec::new(),
        };
        for f in files {
            count += 1;
            let name = f.file_name().unwrap().to_string_lossy().into_owned();
            match merlin::compile_file(&f, &opts) {
                Ok(asm) => {
                    if let Err(errs) = merlin_as::assemble_to_elf(&asm) {
                        failures.push(format!("  {name}: merlin-as: {:?}", errs.first()));
                    }
                }
                Err(errs) => failures.push(format!("  {name}: {}", errs[0].render(&name))),
            }
        }
    }
    assert!(count >= 20, "only {count} kernel C files");
    assert!(
        failures.is_empty(),
        "kernel files merlin cannot compile:\n{}",
        failures.join("\n")
    );
}

/// The sections `generated_assembly_assembles_like_clang` compares: code,
/// string literals, initialized and zero-initialized globals.
const COMPARED_SECTIONS: [&str; 4] = [".text", ".rodata", ".data", ".bss"];

/// One section of an object, as text for comparing: its size, its bytes and
/// its relocations. A section the object lacks reads as empty, so leaving out
/// an empty section and emitting one compare equal.
fn section_summary(obj: &[u8], name: &str) -> Result<(u32, Vec<u8>, Vec<String>), String> {
    let elf = elf32::Elf::parse(obj).map_err(|e| e.to_string())?;
    let Some(section) = elf.sections.iter().find(|s| s.name == name) else {
        return Ok((0, Vec::new(), Vec::new()));
    };
    let bytes = elf.section_data(section).map_err(|e| e.to_string())?.to_vec();
    let mut relocs = Vec::new();
    let rela_name = format!(".rela{name}");
    if let Some(rela) = elf.sections.iter().find(|s| s.name == rela_name) {
        let syms = elf.symbols().map_err(|e| e.to_string())?;
        for r in elf.relocations(rela).map_err(|e| e.to_string())? {
            let ty = elf32::reloc_name(r.ty).unwrap_or("?");
            let sym = &syms[r.sym as usize];
            // A relocation against a section symbol has no name of its own;
            // name it after the section, as both assemblers mean the same.
            let sym = if sym.name.is_empty() {
                elf.sections
                    .get(sym.shndx as usize)
                    .map_or("?", |s| s.name.as_str())
            } else {
                sym.name.as_str()
            };
            relocs.push(format!("{:#x} {ty} {sym}{:+}", r.offset, r.addend));
        }
    }
    Ok((section.size, bytes, relocs))
}

/// merlin's output means the same to clang's assembler as to merlin-as: for
/// every case, both give the same bytes and relocations in `.text`, `.rodata`,
/// `.data` and the same `.bss` size. Needs clang.
#[test]
#[ignore = "needs clang on PATH"]
fn generated_assembly_assembles_like_clang() {
    let scratch = Scratch::new("asm-vs-clang");
    for_each_case("merlin-as vs clang", ALL_CASES, |c| {
        let asm = merlin::compile(&c.src).map_err(|e| render(&e, c))?;
        let ours = merlin_as::assemble_to_elf(&asm).map_err(|e| format!("merlin-as: {e:?}"))?;
        let s = scratch.path(&format!("{}.s", c.name));
        let o = scratch.path(&format!("{}.o", c.name));
        std::fs::write(&s, &asm).map_err(|e| e.to_string())?;
        tool(clang().arg("-mno-relax").arg(&s).arg("-o").arg(&o))?;
        let theirs = std::fs::read(&o).map_err(|e| e.to_string())?;
        for name in COMPARED_SECTIONS {
            let (ours, theirs) = (section_summary(&ours, name)?, section_summary(&theirs, name)?);
            if ours.0 != theirs.0 || ours.1 != theirs.1 {
                return Err(format!("merlin-as and clang assemble different {name} contents"));
            }
            if ours.2 != theirs.2 {
                return Err(format!(
                    "{name} relocations differ:\n{:?}\n{:?}",
                    ours.2, theirs.2
                ));
            }
        }
        Ok(c.expect)
    });
}

#[test]
#[ignore = "needs clang and ld.lld on PATH"]
fn clang_reference() {
    let scratch = Scratch::new("clang");
    let crt0_s = scratch.path("crt0.s");
    let crt0 = std::fs::read_to_string(tests_dir().join("crt0.s")).unwrap();
    std::fs::write(&crt0_s, crt0_for_llvm(&crt0)).unwrap();
    let crt0_o = scratch.path("crt0.o");
    tool(clang().arg(&crt0_s).arg("-o").arg(&crt0_o)).expect("assembling crt0.s with clang");
    for_each_case("clang reference", &[C99_CASES], |c| {
        let obj = scratch.path(&format!("{}.o", c.name));
        tool(
            clang()
                .args([
                    "-std=c99",
                    "-O0",
                    "-ffreestanding",
                    "-fno-builtin",
                    "-Werror=implicit-function-declaration",
                ])
                .arg(&c.path)
                .arg("-o")
                .arg(&obj),
        )?;
        let elf = ld_lld(&[crt0_o.clone(), obj], &scratch.path(&format!("{}.elf", c.name)))?;
        run_in_grail(&elf)
    });
}
