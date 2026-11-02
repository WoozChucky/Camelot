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
//!
//! Ignored, since they need LLVM:
//!
//! * `generated_assembly_assembles_like_clang`: clang's assembler (with
//!   `-mno-relax`) turns merlin's output for every case into the same `.text`
//!   bytes and relocations as merlin-as. Needs clang on `PATH`.
//!
//!       cargo test -p merlin --test harness -- --ignored generated_assembly
//!
//! * `clang_reference`: compiles every case with clang instead of merlin, links
//!   with ld.lld and runs it in Grail. It checks the expectations and crt0
//!   against a real C compiler. Needs clang and ld.lld on `PATH` (the official
//!   Windows LLVM installer is enough).
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
        if !prog.funcs.iter().any(|f| f.name == "main" && f.body.is_some()) {
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
    // Literals that would be `unsigned int` or `long long` in C are rejected.
    assert!(merlin::parse("int main() { return 0xffffffff; }").is_err());
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

/// The code and relocations of an object's `.text`, as text for comparing.
fn text_section(obj: &[u8]) -> Result<(Vec<u8>, Vec<String>), String> {
    let elf = elf32::Elf::parse(obj).map_err(|e| e.to_string())?;
    let text = elf
        .sections
        .iter()
        .find(|s| s.name == ".text")
        .ok_or("no .text")?;
    let code = elf.section_data(text).map_err(|e| e.to_string())?.to_vec();
    let mut relocs = Vec::new();
    if let Some(rela) = elf.sections.iter().find(|s| s.name == ".rela.text") {
        let syms = elf.symbols().map_err(|e| e.to_string())?;
        for r in elf.relocations(rela).map_err(|e| e.to_string())? {
            let ty = elf32::reloc_name(r.ty).unwrap_or("?");
            let sym = &syms[r.sym as usize].name;
            relocs.push(format!("{:#x} {ty} {sym}{:+}", r.offset, r.addend));
        }
    }
    Ok((code, relocs))
}

/// merlin's output means the same to clang's assembler as to merlin-as: for
/// every case, both give the same `.text` bytes and relocations. Needs clang.
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
        let (ours, theirs) = (text_section(&ours)?, text_section(&theirs)?);
        if ours.0 != theirs.0 {
            return Err("merlin-as and clang assemble different .text bytes".into());
        }
        if ours.1 != theirs.1 {
            return Err(format!("relocations differ:\n{:?}\n{:?}", ours.1, theirs.1));
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
