//! `cargo xtask examples`: builds every program in `examples/` with our own
//! tools, merlin-as and merlin-ld (roadmap step 6, spec section 10). No LLVM
//! is involved: both tools are linked into xtask as libraries, the same code
//! their command-line programs run.
//!
//! [`EXAMPLES`] lists each executable: its sources, assembled one by one into
//! objects, and its linker script (or merlin-ld's default layout). The linked
//! `examples/<dir>/<name>.elf` files are committed, so Grail runs them without
//! building anything, and `cargo test` rebuilds each one and checks that the
//! committed file is byte for byte what the current tools make.
//!
//! clang and ld.lld are only a reference: the command at the top of each
//! source builds the same program with them, and the merlin-ld test fixtures
//! (`cargo xtask ld-fixtures`) keep ld.lld's link of `hello` and `multiobj`.

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
    /// Linker script in `dir`; `None` for merlin-ld's default layout.
    pub script: Option<&'static str>,
}

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

    /// The committed executable, `examples/<dir>/<name>.elf`.
    pub fn elf_path(&self, root: &Path) -> PathBuf {
        self.dir(root).join(format!("{}.elf", self.name))
    }

    /// The executable's path relative to the repository root, with `/`.
    pub fn elf_name(&self) -> String {
        format!("examples/{}/{}.elf", self.dir, self.name)
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

/// The object name for a source: `hello.S` → `hello.o`.
pub fn object_name(source: &str) -> String {
    let stem = source
        .strip_suffix(".S")
        .or_else(|| source.strip_suffix(".s"))
        .unwrap_or(source);
    format!("{stem}.o")
}

/// Assembles each source of `ex` with merlin-as and links the objects with
/// merlin-ld, all in memory. Errors look like the tools' own messages
/// (`examples/hello/hello.S:12: error: ...`, `merlin-ld: error: ...`).
pub fn build(root: &Path, ex: &Example) -> Result<Built, String> {
    let dir = ex.dir(root);
    let mut objects = Vec::new();
    for src in ex.sources {
        let path = dir.join(src);
        let text =
            std::fs::read_to_string(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        let data = merlin_as::assemble_to_elf(&text).map_err(|errors| {
            let shown = format!("examples/{}/{src}", ex.dir);
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
    if let Some(script) = ex.script {
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
        format!("linking {}:\n{}", ex.name, lines.join("\n"))
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

/// Compares `examples/<dir>/<name>.elf` under `root` with `built`
/// (`cargo xtask examples --check`).
pub fn check(root: &Path, ex: &Example, built: &Built) -> Status {
    let path = ex.elf_path(root);
    match std::fs::read(&path) {
        Ok(committed) if committed == built.elf => Status::UpToDate,
        Ok(_) => Status::Stale,
        Err(e) => Status::Unreadable(format!("cannot read {}: {e}", path.display())),
    }
}

/// Where `cargo xtask examples` leaves the objects of `ex`:
/// `target/examples/<dir>` under `root`.
pub fn obj_dir(root: &Path, ex: &Example) -> PathBuf {
    root.join("target").join("examples").join(ex.dir)
}

/// Writes the objects to [`obj_dir`] and the executable to
/// `examples/<dir>/<name>.elf`, under `root`.
pub fn write(root: &Path, ex: &Example, built: &Built) -> std::io::Result<()> {
    let objs = obj_dir(root, ex);
    std::fs::create_dir_all(&objs)?;
    for o in &built.objects {
        std::fs::write(objs.join(&o.name), &o.data)?;
    }
    std::fs::write(ex.elf_path(root), &built.elf)
}

/// The command lines [`build`] stands for, as a user would type them from the
/// repository root, with the objects in `obj_dir`.
pub fn commands(ex: &Example, obj_dir: &str) -> Vec<String> {
    let mut cmds: Vec<String> = ex
        .sources
        .iter()
        .map(|s| {
            format!(
                "merlin-as examples/{}/{s} -o {obj_dir}/{}",
                ex.dir,
                object_name(s)
            )
        })
        .collect();
    let mut ld = String::from("merlin-ld");
    if let Some(script) = ex.script {
        ld += &format!(" -T examples/{}/{script}", ex.dir);
    }
    for s in ex.sources {
        ld += &format!(" {obj_dir}/{}", object_name(s));
    }
    ld += &format!(" -o {}", ex.elf_name());
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
            e.elf_path(root),
            root.join("examples").join("kbd-echo").join("kbd-echo-irq.elf")
        );
        assert_eq!(e.elf_name(), "examples/kbd-echo/kbd-echo-irq.elf");
    }

    #[test]
    fn command_lines() {
        assert_eq!(
            commands(find("multiobj").unwrap(), "obj"),
            [
                "merlin-as examples/multiobj/main.S -o obj/main.o",
                "merlin-as examples/multiobj/print.S -o obj/print.o",
                "merlin-ld -T examples/multiobj/link.ld obj/main.o obj/print.o -o examples/multiobj/multiobj.elf",
            ]
        );
        assert_eq!(
            commands(find("muldiv").unwrap(), "o"),
            [
                "merlin-as examples/muldiv/muldiv.S -o o/muldiv.o",
                "merlin-ld o/muldiv.o -o examples/muldiv/muldiv.elf",
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
            script: None,
        };
        let built = build(&root, &ex).unwrap();

        match check(&root, &ex, &built) {
            Status::Unreadable(e) => assert!(e.starts_with("cannot read "), "{e}"),
            other => panic!("missing executable: {other:?}"),
        }

        write(&root, &ex, &built).unwrap();
        assert_eq!(std::fs::read(ex.elf_path(&root)).unwrap(), built.elf);
        for o in &built.objects {
            assert_eq!(std::fs::read(obj_dir(&root, &ex).join(&o.name)).unwrap(), o.data);
        }
        assert_eq!(check(&root, &ex, &built), Status::UpToDate);

        // One byte more, one byte changed: both stale.
        let mut longer = built.elf.clone();
        longer.push(0);
        std::fs::write(ex.elf_path(&root), &longer).unwrap();
        assert_eq!(check(&root, &ex, &built), Status::Stale);
        let mut changed = built.elf.clone();
        *changed.last_mut().unwrap() ^= 1;
        std::fs::write(ex.elf_path(&root), &changed).unwrap();
        assert_eq!(check(&root, &ex, &built), Status::Stale);

        // Writing again brings it up to date.
        write(&root, &ex, &built).unwrap();
        assert_eq!(check(&root, &ex, &built), Status::UpToDate);
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
            script: None,
        };
        let err = build(&root, &ex).unwrap_err();
        assert!(err.starts_with("examples/bad/bad.S:2: error: "), "{err}");

        // Assembles, but nothing defines `missing`.
        std::fs::write(
            dir.join("bad.S"),
            "    .globl _start\n_start:\n    call missing\n",
        )
        .unwrap();
        let err = build(&root, &ex).unwrap_err();
        assert!(
            err.contains("merlin-ld: error: bad.o: undefined reference to 'missing'"),
            "{err}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
