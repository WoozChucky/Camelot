//! `cargo xtask examples`: builds every program in `examples/` with our own
//! tools, merlin, merlin-as and merlin-ld (roadmap step 6, spec section 10).
//! No LLVM is involved: the tools are linked into xtask as libraries, the
//! same code their command-line programs run.
//!
//! [`EXAMPLES`] lists each executable: its sources, assembled one by one into
//! objects, and its linker script (or merlin-ld's default layout). Each one is
//! built for both widths ([`XLENS`]) while Excalibur moves to RV64 (#141):
//! RV32 into `examples/<dir>/<name>.elf`, and RV64 (#151) into
//! `examples/<dir>/<name>-rv64.elf`, from the same sources (#193). A `.S`
//! source is assembly that goes through the C preprocessor first, as in
//! Linux and with clang: merlin's, in assembler-with-cpp mode, for the width
//! (`__riscv_xlen` is 32 or 64), with `-I include` to find `include/asm.h`,
//! whose `REG_L`, `REG_S` and `SZREG` are named as Linux names them. Where RV64 needs other
//! code (bit 63 for interrupt causes, version 2 of the devices' registers),
//! `#if __riscv_xlen == 64` chooses it. #159 removes RV32.
//!
//! The linked executables are committed, so Grail runs them without building
//! anything, and `cargo test` rebuilds each one and checks that the committed
//! file is byte for byte what the current tools make.
//!
//! clang and ld.lld are only a reference: the commands at the top of each
//! source build the same program with them, and the merlin-ld test fixtures
//! (`cargo xtask ld-fixtures`) keep ld.lld's link of `hello` and `multiobj`
//! at both widths.

pub use merlin_as::Xlen;
use std::path::{Path, PathBuf};

/// One executable built from `examples/<dir>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Example {
    /// What `cargo xtask examples NAME` calls it; also the output's stem.
    pub name: &'static str,
    /// Directory under `examples/`.
    pub dir: &'static str,
    /// Assembly sources in `dir`, in link order, the same for both widths.
    pub sources: &'static [&'static str],
    /// Linker script in `dir`, the same for both widths; `None` for
    /// merlin-ld's default layout.
    pub script: Option<&'static str>,
}

/// The widths every example is built for: RV32, then RV64.
pub const XLENS: [Xlen; 2] = [Xlen::Rv32, Xlen::Rv64];

/// Every example, in the order `cargo xtask examples` builds them.
pub const EXAMPLES: &[Example] = &[
    Example {
        name: "hello",
        dir: "hello",
        sources: &["hello.S"],
        script: Some("link.ld"),
    },
    Example {
        name: "multiobj",
        dir: "multiobj",
        sources: &["main.S", "print.S"],
        script: Some("link.ld"),
    },
    Example {
        name: "muldiv",
        dir: "muldiv",
        sources: &["muldiv.S"],
        script: None,
    },
    Example {
        name: "sysinfo",
        dir: "sysinfo",
        sources: &["sysinfo.S"],
        script: Some("link.ld"),
    },
    Example {
        name: "disk",
        dir: "disk",
        sources: &["disk.S"],
        script: Some("link.ld"),
    },
    Example {
        name: "uart-echo",
        dir: "uart-echo",
        sources: &["uart-echo.S"],
        script: Some("link.ld"),
    },
    Example {
        name: "colorbars",
        dir: "colorbars",
        sources: &["colorbars.S"],
        script: Some("link.ld"),
    },
    Example {
        name: "kbd-echo",
        dir: "kbd-echo",
        sources: &["kbd-echo.S"],
        script: Some("link.ld"),
    },
    Example {
        name: "kbd-echo-irq",
        dir: "kbd-echo",
        sources: &["kbd-echo-irq.S"],
        script: Some("link.ld"),
    },
    Example {
        name: "pointer",
        dir: "pointer",
        sources: &["pointer.S"],
        script: Some("link.ld"),
    },
    Example {
        name: "flip",
        dir: "flip",
        sources: &["flip.S"],
        script: Some("link.ld"),
    },
    Example {
        name: "cursor",
        dir: "cursor",
        sources: &["cursor.S"],
        script: Some("link.ld"),
    },
    Example {
        name: "net-ping",
        dir: "net",
        sources: &["ping.S", "netlib.S"],
        script: Some("link.ld"),
    },
    Example {
        name: "net-pong",
        dir: "net",
        sources: &["pong.S", "netlib.S"],
        script: Some("link.ld"),
    },
    Example {
        name: "net-user",
        dir: "net",
        sources: &["user.S", "netlib.S"],
        script: Some("link.ld"),
    },
    Example {
        name: "smp",
        dir: "smp",
        sources: &["smp.S"],
        script: Some("link.ld"),
    },
];

/// The clang-only macros that let clang assemble `xcal.dbg`/`xcal.halt`
/// (`-include` it), relative to the repository root.
pub const XCAL_INCLUDE: &str = "examples/xcal.inc";

/// Where the `.S` sources find `#include <asm.h>` (`-I`), relative to the
/// repository root: the examples', the boot ROM's and merlin-ld's fixtures'.
pub const INCLUDE_DIR: &str = "include";

/// The example called `name`.
pub fn find(name: &str) -> Option<&'static Example> {
    EXAMPLES.iter().find(|e| e.name == name)
}

impl Example {
    /// `examples/<dir>` under `root`.
    pub fn dir(&self, root: &Path) -> PathBuf {
        root.join("examples").join(self.dir)
    }

    /// `examples/<dir>`, relative to the repository root, with `/`.
    pub fn dir_name(&self) -> String {
        format!("examples/{}", self.dir)
    }

    /// The `xlen` executable's file name: `<name>.elf`, `<name>-rv64.elf`.
    pub fn elf_file(&self, xlen: Xlen) -> String {
        match xlen {
            Xlen::Rv32 => format!("{}.elf", self.name),
            Xlen::Rv64 => format!("{}-rv64.elf", self.name),
        }
    }

    /// The committed `xlen` executable, `examples/<dir>/<elf_file>`.
    pub fn elf_path(&self, root: &Path, xlen: Xlen) -> PathBuf {
        self.dir(root).join(self.elf_file(xlen))
    }

    /// The `xlen` executable's path relative to the repository root, with `/`.
    pub fn elf_name(&self, xlen: Xlen) -> String {
        format!("examples/{}/{}", self.dir, self.elf_file(xlen))
    }
}

/// An object file made by merlin-as.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Object {
    /// The source's name with `.o` for `.S`/`.s` (`hello.o`).
    pub name: String,
    pub data: Vec<u8>,
}

/// One source of a build, as merlin-as got it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Assembly {
    /// The name of the file `cargo xtask examples` leaves it in: `hello.s`
    /// for `hello.S` (`merlin -E`'s output), or the source's own name for a
    /// `.s` one.
    pub name: String,
    /// The text: the preprocessed source, or the source itself.
    pub text: String,
}

/// The result of [`build`].
#[derive(Debug)]
pub struct Built {
    /// One per source, in [`Example::sources`] order: what merlin-as
    /// assembled.
    pub assembly: Vec<Assembly>,
    /// One per source, in [`Example::sources`] order.
    pub objects: Vec<Object>,
    /// The linked executable.
    pub elf: Vec<u8>,
    /// merlin-ld's warnings (the examples have none; a test checks).
    pub warnings: Vec<String>,
}

/// merlin's and merlin-as's `-march=` option for `xlen`, with a space after
/// it: none for RV32, the tools' default until #159.
pub fn march_flag(xlen: Xlen) -> &'static str {
    match xlen {
        Xlen::Rv32 => "",
        Xlen::Rv64 => "-march=rv64ima_zicsr_zifencei ",
    }
}

/// The object name for a source: `hello.S` → `hello.o`.
pub fn object_name(source: &str) -> String {
    format!("{}.o", stem(source))
}

/// The name of what merlin-as assembles for a source: `hello.S` → `hello.s`
/// (the preprocessed text), and a `.s` source itself.
pub fn assembly_name(source: &str) -> String {
    format!("{}.s", stem(source))
}

fn stem(source: &str) -> &str {
    source
        .strip_suffix(".S")
        .or_else(|| source.strip_suffix(".s"))
        .unwrap_or(source)
}

/// Whether a source goes through the preprocessor: a `.S` one, as clang
/// decides.
pub fn preprocessed(source: &str) -> bool {
    source.ends_with(".S")
}

/// The text merlin-as assembles for the source `shown` (its path relative to
/// `root`, with `/`; the name its line markers and errors give): a `.S` file
/// preprocessed by merlin for `xlen` as assembler-with-cpp, with `#include`
/// looking in `include_dirs` (relative to `root`), as `merlin -E` writes it;
/// a `.s` file as it is.
pub fn assembly_of(root: &Path, shown: &str, include_dirs: &[&str], xlen: Xlen) -> Result<String, String> {
    let path = root.join(shown);
    let text = std::fs::read_to_string(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    if !preprocessed(shown) {
        return Ok(text);
    }
    let opts = merlin::Options {
        include_dirs: include_dirs.iter().map(PathBuf::from).collect(),
        xlen: match xlen {
            Xlen::Rv32 => merlin::types::Xlen::Rv32,
            Xlen::Rv64 => merlin::types::Xlen::Rv64,
        },
        ..merlin::Options::default()
    };
    let load = |p: &Path| std::fs::read_to_string(root.join(p)).ok();
    merlin::preprocess_asm(shown, &text, &opts, &load).map_err(|errors| {
        let lines: Vec<String> = errors.iter().map(|e| e.render(shown)).collect();
        lines.join("\n")
    })
}

/// Assembles each source of `ex`'s `xlen` build with merlin-as (after
/// merlin's preprocessor, for a `.S` one) and links the objects with
/// merlin-ld, all in memory. Errors look like the tools' own messages
/// (`examples/hello/hello.S:12: error: ...`, `merlin-ld: error: ...`).
pub fn build(root: &Path, ex: &Example, xlen: Xlen) -> Result<Built, String> {
    assemble_and_link(
        root,
        &ex.dir_name(),
        ex.name,
        ex.sources,
        &[INCLUDE_DIR],
        ex.script,
        xlen,
    )
}

/// Assembles `sources` (in `dir`, relative to `root`, with `/`) with
/// merlin-as for `xlen`, a `.S` one preprocessed first ([`assembly_of`],
/// with `include_dirs`), and links the objects with merlin-ld, with the
/// linker script `script` in `dir` or the default layout, all in memory: how
/// every program built with our own tools is made (the examples, the boot
/// programs). merlin-ld writes an ELF32 or an ELF64 executable as the
/// objects are. An assembler error names the line of the source it comes
/// from, through the preprocessed text's line markers.
pub fn assemble_and_link(
    root: &Path,
    dir: &str,
    name: &str,
    sources: &[&str],
    include_dirs: &[&str],
    script: Option<&str>,
    xlen: Xlen,
) -> Result<Built, String> {
    let as_opts = merlin_as::Options { xlen };
    let mut assembly = Vec::new();
    let mut objects = Vec::new();
    for src in sources {
        let shown = format!("{dir}/{src}");
        let text = assembly_of(root, &shown, include_dirs, xlen)?;
        let data = merlin_as::assemble_to_elf_with(&text, &as_opts).map_err(|errors| {
            let lines: Vec<String> = errors
                .iter()
                .map(|e| {
                    let at = match e.line {
                        0 => None,
                        n if preprocessed(src) => merlin::preprocess::source_line(&text, n),
                        n => Some((shown.clone(), n)),
                    };
                    match at {
                        Some((file, n)) => format!("{file}:{n}: error: {}", e.msg),
                        None => format!("{shown}: error: {}", e.msg),
                    }
                })
                .collect();
            lines.join("\n")
        })?;
        assembly.push(Assembly {
            name: assembly_name(src),
            text,
        });
        objects.push(Object {
            name: object_name(src),
            data,
        });
    }

    let mut opts = merlin_ld::Options::default();
    if let Some(script) = script {
        let path = root.join(dir).join(script);
        let text =
            std::fs::read_to_string(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        opts.script = Some(merlin_ld::ScriptSource {
            name: script.to_string(),
            text,
        });
    }
    let inputs: Vec<merlin_ld::Input> = objects
        .iter()
        .map(|o| merlin_ld::Input {
            name: o.name.clone(),
            data: o.data.clone(),
        })
        .collect();
    let linked = merlin_ld::link(&inputs, &opts).map_err(|errors| {
        let lines: Vec<String> = errors.iter().map(|e| format!("merlin-ld: error: {e}")).collect();
        format!("linking {name}:\n{}", lines.join("\n"))
    })?;
    Ok(Built {
        assembly,
        objects,
        elf: linked.elf,
        warnings: linked.layout.warnings,
    })
}

/// How the committed executable compares with a fresh build ([`check`]).
#[derive(Debug, PartialEq, Eq)]
pub enum Status {
    UpToDate,
    /// It differs from what the tools build.
    Stale,
    /// It cannot be read (the message says why).
    Unreadable(String),
}

/// Compares the committed `xlen` executable of `ex` under `root` with `built`
/// (`cargo xtask examples --check`).
pub fn check(root: &Path, ex: &Example, xlen: Xlen, built: &Built) -> Status {
    let path = ex.elf_path(root, xlen);
    match std::fs::read(&path) {
        Ok(committed) if committed == built.elf => Status::UpToDate,
        Ok(_) => Status::Stale,
        Err(e) => Status::Unreadable(format!("cannot read {}: {e}", path.display())),
    }
}

/// Where `cargo xtask examples` leaves the preprocessed sources and the
/// objects of `ex`'s `xlen` build, relative to the repository root:
/// `target/examples/<dir>` for RV32 and `target/examples/rv64/<dir>` for
/// RV64 (both builds have the same sources, and so the same names).
pub fn obj_dir_name(ex: &Example, xlen: Xlen) -> String {
    match xlen {
        Xlen::Rv32 => format!("target/examples/{}", ex.dir),
        Xlen::Rv64 => format!("target/examples/rv64/{}", ex.dir),
    }
}

/// [`obj_dir_name`] under `root`.
pub fn obj_dir(root: &Path, ex: &Example, xlen: Xlen) -> PathBuf {
    root.join(obj_dir_name(ex, xlen))
}

/// Writes the preprocessed sources and the objects to [`obj_dir`] and the
/// executable to [`Example::elf_path`], under `root`.
pub fn write(root: &Path, ex: &Example, xlen: Xlen, built: &Built) -> std::io::Result<()> {
    let objs = obj_dir(root, ex, xlen);
    std::fs::create_dir_all(&objs)?;
    for (src, a) in ex.sources.iter().zip(&built.assembly) {
        if preprocessed(src) {
            std::fs::write(objs.join(&a.name), &a.text)?;
        }
    }
    for o in &built.objects {
        std::fs::write(objs.join(&o.name), &o.data)?;
    }
    std::fs::write(ex.elf_path(root, xlen), &built.elf)
}

/// The command lines that make `source` (in `dir`, relative to the
/// repository root) into an object in `obj_dir` for `xlen`, as a user would
/// type them from the repository root: `merlin -E` for a `.S` one (with `-I`
/// for each of `include_dirs`), then merlin-as.
pub fn assemble_commands(
    dir: &str,
    source: &str,
    include_dirs: &[&str],
    xlen: Xlen,
    obj_dir: &str,
) -> Vec<String> {
    let obj = format!("{obj_dir}/{}", object_name(source));
    if !preprocessed(source) {
        return vec![format!("merlin-as {}{dir}/{source} -o {obj}", march_flag(xlen))];
    }
    let includes: String = include_dirs.iter().map(|d| format!("-I {d} ")).collect();
    let asm = format!("{obj_dir}/{}", assembly_name(source));
    vec![
        format!("merlin -E {}{includes}{dir}/{source} -o {asm}", march_flag(xlen)),
        format!("merlin-as {}{asm} -o {obj}", march_flag(xlen)),
    ]
}

/// The command lines [`build`] stands for, as a user would type them from the
/// repository root, with the preprocessed sources and the objects in
/// `obj_dir`. merlin-ld needs no option for RV64: the objects' class makes
/// the executable's.
pub fn commands(ex: &Example, xlen: Xlen, obj_dir: &str) -> Vec<String> {
    let mut cmds: Vec<String> = ex
        .sources
        .iter()
        .flat_map(|s| assemble_commands(&ex.dir_name(), s, &[INCLUDE_DIR], xlen, obj_dir))
        .collect();
    let mut ld = String::from("merlin-ld");
    if let Some(script) = ex.script {
        ld += &format!(" -T examples/{}/{script}", ex.dir);
    }
    for s in ex.sources {
        ld += &format!(" {obj_dir}/{}", object_name(s));
    }
    ld += &format!(" -o {}", ex.elf_name(xlen));
    cmds.push(ld);
    cmds
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_unique_and_found() {
        for (i, e) in EXAMPLES.iter().enumerate() {
            assert_eq!(find(e.name), Some(e));
            assert!(EXAMPLES[..i].iter().all(|o| o.name != e.name), "{}", e.name);
        }
        assert_eq!(find("nope"), None);
    }

    #[test]
    fn object_names() {
        assert_eq!(object_name("hello.S"), "hello.o");
        assert_eq!(object_name("crt0.s"), "crt0.o");
        assert_eq!(object_name("odd"), "odd.o");
        assert_eq!(assembly_name("hello.S"), "hello.s");
        assert_eq!(assembly_name("crt0.s"), "crt0.s");
        assert!(preprocessed("hello.S") && !preprocessed("crt0.s"));
    }

    #[test]
    fn paths() {
        let e = find("kbd-echo-irq").unwrap();
        let root = Path::new("r");
        assert_eq!(
            e.elf_path(root, Xlen::Rv32),
            root.join("examples").join("kbd-echo").join("kbd-echo-irq.elf")
        );
        assert_eq!(e.elf_name(Xlen::Rv32), "examples/kbd-echo/kbd-echo-irq.elf");
        assert_eq!(
            e.elf_path(root, Xlen::Rv64),
            root.join("examples")
                .join("kbd-echo")
                .join("kbd-echo-irq-rv64.elf")
        );
        assert_eq!(e.elf_name(Xlen::Rv64), "examples/kbd-echo/kbd-echo-irq-rv64.elf");
        assert_eq!(
            obj_dir(root, e, Xlen::Rv32),
            root.join("target/examples/kbd-echo")
        );
        assert_eq!(
            obj_dir(root, e, Xlen::Rv64),
            root.join("target/examples/rv64/kbd-echo")
        );
    }

    #[test]
    fn command_lines() {
        assert_eq!(
            commands(find("multiobj").unwrap(), Xlen::Rv32, "obj"),
            [
                "merlin -E -I include examples/multiobj/main.S -o obj/main.s",
                "merlin-as obj/main.s -o obj/main.o",
                "merlin -E -I include examples/multiobj/print.S -o obj/print.s",
                "merlin-as obj/print.s -o obj/print.o",
                "merlin-ld -T examples/multiobj/link.ld obj/main.o obj/print.o -o examples/multiobj/multiobj.elf",
            ]
        );
        assert_eq!(
            commands(find("muldiv").unwrap(), Xlen::Rv64, "o"),
            [
                "merlin -E -march=rv64ima_zicsr_zifencei -I include examples/muldiv/muldiv.S \
                 -o o/muldiv.s",
                "merlin-as -march=rv64ima_zicsr_zifencei o/muldiv.s -o o/muldiv.o",
                "merlin-ld o/muldiv.o -o examples/muldiv/muldiv-rv64.elf",
            ]
        );
        assert_eq!(
            assemble_commands("kernel/boot", "boot.s", &[], Xlen::Rv32, "o"),
            ["merlin-as kernel/boot/boot.s -o o/boot.o"]
        );
    }

    /// `cargo xtask examples [--check]` on a scratch tree: a missing, a
    /// written (up-to-date) and a changed (stale) executable, from a `.S`
    /// source with a header and a `.s` one.
    #[test]
    fn check_and_write() {
        let root = std::env::temp_dir().join(format!("xtask-examples-check-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let dir = root.join("examples").join("two");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::create_dir_all(root.join(INCLUDE_DIR)).unwrap();
        std::fs::write(
            root.join(INCLUDE_DIR).join("asm.h"),
            "#if __riscv_xlen == 64\n#define CODE 64\n#else\n#define CODE 32\n#endif\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("a.S"),
            "#include <asm.h>\n    .globl _start\n_start:\n    call f\n    li a0, CODE\n    xcal.halt a0\n",
        )
        .unwrap();
        std::fs::write(dir.join("b.s"), "    .globl f\nf:\n    li a0, 7\n    ret\n").unwrap();
        let ex = Example {
            name: "two",
            dir: "two",
            sources: &["a.S", "b.s"],
            script: None,
        };
        let built = build(&root, &ex, Xlen::Rv32).unwrap();
        assert_eq!(
            built.assembly[0].text,
            // After the marker of the end of asm.h, one space: clang's spacing.
            "# 1 \"examples/two/a.S\"\n# 1 \"include/asm.h\" 1\n# 2 \"examples/two/a.S\" 2\n \
             .globl _start\n_start:\n    call f\n    li a0, 32\n    xcal.halt a0\n"
        );
        assert_eq!(built.assembly[1].name, "b.s");

        match check(&root, &ex, Xlen::Rv32, &built) {
            Status::Unreadable(e) => assert!(e.starts_with("cannot read "), "{e}"),
            other => panic!("missing executable: {other:?}"),
        }

        let x = Xlen::Rv32;
        write(&root, &ex, x, &built).unwrap();
        assert_eq!(std::fs::read(ex.elf_path(&root, x)).unwrap(), built.elf);
        for o in &built.objects {
            assert_eq!(
                std::fs::read(obj_dir(&root, &ex, x).join(&o.name)).unwrap(),
                o.data
            );
        }
        assert_eq!(
            std::fs::read_to_string(obj_dir(&root, &ex, x).join("a.s")).unwrap(),
            built.assembly[0].text
        );
        assert_eq!(check(&root, &ex, x, &built), Status::UpToDate);

        // One byte more, one byte changed: both stale.
        let mut longer = built.elf.clone();
        longer.push(0);
        std::fs::write(ex.elf_path(&root, x), &longer).unwrap();
        assert_eq!(check(&root, &ex, x, &built), Status::Stale);
        let mut changed = built.elf.clone();
        *changed.last_mut().unwrap() ^= 1;
        std::fs::write(ex.elf_path(&root, x), &changed).unwrap();
        assert_eq!(check(&root, &ex, x, &built), Status::Stale);

        // Writing again brings it up to date.
        write(&root, &ex, x, &built).unwrap();
        assert_eq!(check(&root, &ex, x, &built), Status::UpToDate);

        // The RV64 build of the same sources is another file, an ELF64 one, with
        // objects of its own and the other branch of the `#if`.
        let built64 = build(&root, &ex, Xlen::Rv64).unwrap();
        assert_eq!(built64.elf[4], 2, "ELFCLASS64");
        assert!(built64.assembly[0].text.contains("li a0, 64"));
        assert!(matches!(
            check(&root, &ex, Xlen::Rv64, &built64),
            Status::Unreadable(_)
        ));
        write(&root, &ex, Xlen::Rv64, &built64).unwrap();
        assert_eq!(check(&root, &ex, Xlen::Rv64, &built64), Status::UpToDate);
        assert_eq!(check(&root, &ex, x, &built), Status::UpToDate);
        assert!(obj_dir(&root, &ex, Xlen::Rv64).join("a.o").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Errors name the source's file and line, through the preprocessor's
    /// line markers, a header's too.
    #[test]
    fn assembler_errors_name_the_file_and_line() {
        let root = std::env::temp_dir().join(format!("xtask-examples-{}", std::process::id()));
        let dir = root.join("examples").join("bad");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::create_dir_all(root.join(INCLUDE_DIR)).unwrap();
        std::fs::write(root.join(INCLUDE_DIR).join("asm.h"), "\n    oops\n").unwrap();
        std::fs::write(dir.join("bad.S"), "# comment\n    nop\n\n    frobnicate a0\n").unwrap();
        let ex = Example {
            name: "bad",
            dir: "bad",
            sources: &["bad.S"],
            script: None,
        };
        let err = build(&root, &ex, Xlen::Rv32).unwrap_err();
        assert!(err.starts_with("examples/bad/bad.S:4: error: "), "{err}");
        std::fs::write(dir.join("bad.S"), "#include <asm.h>\n").unwrap();
        let err = build(&root, &ex, Xlen::Rv32).unwrap_err();
        assert!(err.starts_with("include/asm.h:2: error: "), "{err}");
        // The preprocessor's own.
        std::fs::write(dir.join("bad.S"), "#if\n").unwrap();
        let err = build(&root, &ex, Xlen::Rv32).unwrap_err();
        assert!(err.starts_with("examples/bad/bad.S:1:"), "{err}");

        // Assembles, but nothing defines `missing`.
        std::fs::write(
            dir.join("bad.S"),
            "    .globl _start\n_start:\n    call missing\n",
        )
        .unwrap();
        let err = build(&root, &ex, Xlen::Rv64).unwrap_err();
        assert!(
            err.contains("merlin-ld: error: bad.o: undefined reference to 'missing'"),
            "{err}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
