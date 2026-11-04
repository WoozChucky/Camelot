//! `cargo xtask examples`: builds every program in `examples/` with our own
//! tools, merlin-as and merlin-ld (roadmap step 6, spec section 10). No LLVM
//! is involved: both tools are linked into xtask as libraries, the same code
//! their command-line programs run.
//!
//! [`EXAMPLES`] lists each executable: its sources, assembled one by one into
//! objects, and its linker script (or merlin-ld's default layout). Each one is
//! built for both widths ([`XLENS`]) while Excalibur moves to RV64 (#141):
//! RV32 into `examples/<dir>/<name>.elf`, and RV64 (#151) into
//! `examples/<dir>/<name>-rv64.elf`, from [`Example::rv64_sources`]: the RV32
//! source where it is the same at both widths, its `-rv64` version where
//! RV64 needs other code (pointers and return addresses saved with `sd`, bit
//! 63 for interrupt causes, version 2 of the devices' registers). #159
//! removes RV32, and the RV64 sources then take the RV32 ones' names.
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
    /// Assembly sources in `dir`, in link order.
    pub sources: &'static [&'static str],
    /// The RV64 build's sources in `dir`, in link order: a source of
    /// [`Example::sources`] where both widths share it, else its `-rv64`
    /// version.
    pub rv64_sources: &'static [&'static str],
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
        rv64_sources: &["hello.S"],
        script: Some("link.ld"),
    },
    Example {
        name: "multiobj",
        dir: "multiobj",
        sources: &["main.S", "print.S"],
        rv64_sources: &["main-rv64.S", "print.S"],
        script: Some("link.ld"),
    },
    Example {
        name: "muldiv",
        dir: "muldiv",
        sources: &["muldiv.S"],
        rv64_sources: &["muldiv-rv64.S"],
        script: None,
    },
    Example {
        name: "sysinfo",
        dir: "sysinfo",
        sources: &["sysinfo.S"],
        rv64_sources: &["sysinfo-rv64.S"],
        script: Some("link.ld"),
    },
    Example {
        name: "disk",
        dir: "disk",
        sources: &["disk.S"],
        rv64_sources: &["disk-rv64.S"],
        script: Some("link.ld"),
    },
    Example {
        name: "uart-echo",
        dir: "uart-echo",
        sources: &["uart-echo.S"],
        rv64_sources: &["uart-echo-rv64.S"],
        script: Some("link.ld"),
    },
    Example {
        name: "colorbars",
        dir: "colorbars",
        sources: &["colorbars.S"],
        rv64_sources: &["colorbars.S"],
        script: Some("link.ld"),
    },
    Example {
        name: "kbd-echo",
        dir: "kbd-echo",
        sources: &["kbd-echo.S"],
        rv64_sources: &["kbd-echo-rv64.S"],
        script: Some("link.ld"),
    },
    Example {
        name: "kbd-echo-irq",
        dir: "kbd-echo",
        sources: &["kbd-echo-irq.S"],
        rv64_sources: &["kbd-echo-irq-rv64.S"],
        script: Some("link.ld"),
    },
    Example {
        name: "pointer",
        dir: "pointer",
        sources: &["pointer.S"],
        rv64_sources: &["pointer-rv64.S"],
        script: Some("link.ld"),
    },
    Example {
        name: "net-ping",
        dir: "net",
        sources: &["ping.S", "netlib.S"],
        rv64_sources: &["ping.S", "netlib-rv64.S"],
        script: Some("link.ld"),
    },
    Example {
        name: "net-pong",
        dir: "net",
        sources: &["pong.S", "netlib.S"],
        rv64_sources: &["pong.S", "netlib-rv64.S"],
        script: Some("link.ld"),
    },
    Example {
        name: "net-user",
        dir: "net",
        sources: &["user.S", "netlib.S"],
        rv64_sources: &["user-rv64.S", "netlib-rv64.S"],
        script: Some("link.ld"),
    },
    Example {
        name: "smp",
        dir: "smp",
        sources: &["smp.S"],
        rv64_sources: &["smp-rv64.S"],
        script: Some("link.ld"),
    },
];

/// The clang-only macros that let clang assemble `xcal.dbg`/`xcal.halt`
/// (`-include` it), relative to the repository root.
pub const XCAL_INCLUDE: &str = "examples/xcal.inc";

/// The example called `name`.
pub fn find(name: &str) -> Option<&'static Example> {
    EXAMPLES.iter().find(|e| e.name == name)
}

impl Example {
    /// `examples/<dir>` under `root`.
    pub fn dir(&self, root: &Path) -> PathBuf {
        root.join("examples").join(self.dir)
    }

    /// The sources of the `xlen` build, in link order.
    pub fn sources_for(&self, xlen: Xlen) -> &'static [&'static str] {
        match xlen {
            Xlen::Rv32 => self.sources,
            Xlen::Rv64 => self.rv64_sources,
        }
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

/// The result of [`build`].
#[derive(Debug)]
pub struct Built {
    /// One per source, in [`Example::sources`] order.
    pub objects: Vec<Object>,
    /// The linked executable.
    pub elf: Vec<u8>,
    /// merlin-ld's warnings (the examples have none; a test checks).
    pub warnings: Vec<String>,
}

/// merlin-as's `-march=` option for `xlen`, with a space after it: none for
/// RV32, merlin-as's default until #159.
pub fn march_flag(xlen: Xlen) -> &'static str {
    match xlen {
        Xlen::Rv32 => "",
        Xlen::Rv64 => "-march=rv64ima_zicsr_zifencei ",
    }
}

/// The object name for a source: `hello.S` → `hello.o`.
pub fn object_name(source: &str) -> String {
    let stem = source
        .strip_suffix(".S")
        .or_else(|| source.strip_suffix(".s"))
        .unwrap_or(source);
    format!("{stem}.o")
}

/// Assembles each source of `ex`'s `xlen` build with merlin-as and links the
/// objects with merlin-ld, all in memory. Errors look like the tools' own
/// messages (`examples/hello/hello.S:12: error: ...`, `merlin-ld: error: ...`).
pub fn build(root: &Path, ex: &Example, xlen: Xlen) -> Result<Built, String> {
    assemble_and_link(
        &ex.dir(root),
        &format!("examples/{}", ex.dir),
        ex.name,
        ex.sources_for(xlen),
        ex.script,
        xlen,
    )
}

/// Assembles `sources` (in `dir`, which messages call `shown`) with merlin-as
/// for `xlen` and links the objects with merlin-ld, with the linker script
/// `script` in `dir` or the default layout, all in memory: how every program
/// built with our own tools is made (the examples, the boot programs).
/// merlin-ld writes an ELF32 or an ELF64 executable as the objects are.
pub fn assemble_and_link(
    dir: &Path,
    shown: &str,
    name: &str,
    sources: &[&str],
    script: Option<&str>,
    xlen: Xlen,
) -> Result<Built, String> {
    let as_opts = merlin_as::Options { xlen };
    let mut objects = Vec::new();
    for src in sources {
        let path = dir.join(src);
        let text =
            std::fs::read_to_string(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        let data = merlin_as::assemble_to_elf_with(&text, &as_opts).map_err(|errors| {
            let shown = format!("{shown}/{src}");
            let lines: Vec<String> = errors
                .iter()
                .map(|e| match e.line {
                    0 => format!("{shown}: error: {}", e.msg),
                    n => format!("{shown}:{n}: error: {}", e.msg),
                })
                .collect();
            lines.join("\n")
        })?;
        objects.push(Object {
            name: object_name(src),
            data,
        });
    }

    let mut opts = merlin_ld::Options::default();
    if let Some(script) = script {
        let path = dir.join(script);
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

/// Where `cargo xtask examples` leaves the objects of `ex`'s `xlen` build,
/// relative to the repository root: `target/examples/<dir>` for RV32 and
/// `target/examples/rv64/<dir>` for RV64 (both builds may share a source,
/// and so an object's name).
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

/// Writes the objects to [`obj_dir`] and the executable to
/// [`Example::elf_path`], under `root`.
pub fn write(root: &Path, ex: &Example, xlen: Xlen, built: &Built) -> std::io::Result<()> {
    let objs = obj_dir(root, ex, xlen);
    std::fs::create_dir_all(&objs)?;
    for o in &built.objects {
        std::fs::write(objs.join(&o.name), &o.data)?;
    }
    std::fs::write(ex.elf_path(root, xlen), &built.elf)
}

/// The command lines [`build`] stands for, as a user would type them from the
/// repository root, with the objects in `obj_dir`. merlin-ld needs no option
/// for RV64: the objects' class makes the executable's.
pub fn commands(ex: &Example, xlen: Xlen, obj_dir: &str) -> Vec<String> {
    let sources = ex.sources_for(xlen);
    let mut cmds: Vec<String> = sources
        .iter()
        .map(|s| {
            format!(
                "merlin-as {}examples/{}/{s} -o {obj_dir}/{}",
                march_flag(xlen),
                ex.dir,
                object_name(s)
            )
        })
        .collect();
    let mut ld = String::from("merlin-ld");
    if let Some(script) = ex.script {
        ld += &format!(" -T examples/{}/{script}", ex.dir);
    }
    for s in sources {
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

    /// Every RV64 source is an RV32 one or its `-rv64` version, in the same
    /// place in the link order.
    #[test]
    fn rv64_sources_follow_the_rv32_ones() {
        for e in EXAMPLES {
            assert_eq!(e.sources.len(), e.rv64_sources.len(), "{}", e.name);
            for (&s32, &s64) in e.sources.iter().zip(e.rv64_sources) {
                let rv64 = format!("{}-rv64.S", s32.strip_suffix(".S").unwrap());
                assert!(s64 == s32 || s64 == rv64, "{}: {s64} for {s32}", e.name);
            }
        }
    }

    #[test]
    fn command_lines() {
        assert_eq!(
            commands(find("multiobj").unwrap(), Xlen::Rv32, "obj"),
            [
                "merlin-as examples/multiobj/main.S -o obj/main.o",
                "merlin-as examples/multiobj/print.S -o obj/print.o",
                "merlin-ld -T examples/multiobj/link.ld obj/main.o obj/print.o -o examples/multiobj/multiobj.elf",
            ]
        );
        assert_eq!(
            commands(find("muldiv").unwrap(), Xlen::Rv32, "o"),
            [
                "merlin-as examples/muldiv/muldiv.S -o o/muldiv.o",
                "merlin-ld o/muldiv.o -o examples/muldiv/muldiv.elf",
            ]
        );
        assert_eq!(
            commands(find("multiobj").unwrap(), Xlen::Rv64, "obj"),
            [
                "merlin-as -march=rv64ima_zicsr_zifencei examples/multiobj/main-rv64.S -o obj/main-rv64.o",
                "merlin-as -march=rv64ima_zicsr_zifencei examples/multiobj/print.S -o obj/print.o",
                "merlin-ld -T examples/multiobj/link.ld obj/main-rv64.o obj/print.o \
                 -o examples/multiobj/multiobj-rv64.elf",
            ]
        );
    }

    /// `cargo xtask examples [--check]` on a scratch tree: a missing, a
    /// written (up-to-date) and a changed (stale) executable.
    #[test]
    fn check_and_write() {
        let root = std::env::temp_dir().join(format!("xtask-examples-check-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let dir = root.join("examples").join("two");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("a.S"),
            "    .globl _start\n_start:\n    call f\n    xcal.halt a0\n",
        )
        .unwrap();
        std::fs::write(dir.join("b.S"), "    .globl f\nf:\n    li a0, 7\n    ret\n").unwrap();
        let ex = Example {
            name: "two",
            dir: "two",
            sources: &["a.S", "b.S"],
            rv64_sources: &["a.S", "b.S"],
            script: None,
        };
        let built = build(&root, &ex, Xlen::Rv32).unwrap();

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
        // objects of its own.
        let built64 = build(&root, &ex, Xlen::Rv64).unwrap();
        assert_eq!(built64.elf[4], 2, "ELFCLASS64");
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

    #[test]
    fn assembler_errors_name_the_file_and_line() {
        let root = std::env::temp_dir().join(format!("xtask-examples-{}", std::process::id()));
        let dir = root.join("examples").join("bad");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("bad.S"), "    nop\n    frobnicate a0\n").unwrap();
        let ex = Example {
            name: "bad",
            dir: "bad",
            sources: &["bad.S"],
            rv64_sources: &["bad.S"],
            script: None,
        };
        let err = build(&root, &ex, Xlen::Rv32).unwrap_err();
        assert!(err.starts_with("examples/bad/bad.S:2: error: "), "{err}");

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
