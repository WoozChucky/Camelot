//! Building and running the Camelot kernel (`kernel/`).
//!
//! Every `*.c` and `*.s` file directly in `kernel/` is compiled (clang) or
//! assembled (clang or merlin-as) into `target/kernel/obj/`, then ld.lld links
//! them with `kernel/kernel.ld`. Files are processed in name order and clang
//! runs inside `kernel/` with relative paths, so the output doesn't depend on
//! where the repository is checked out.

use grail::{Machine, RunResult};
use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Flags shared by every clang invocation: the Excalibur target. No linker
/// relaxation, since merlin-as and merlin-ld won't do it either.
pub const TARGET_FLAGS: &[&str] = &[
    "--target=riscv32-unknown-elf",
    "-march=rv32im_zicsr",
    "-mabi=ilp32",
    "-mno-relax",
];

/// Extra flags for C files: freestanding, no libc headers, no hidden runtime
/// (stack protector, unwind tables), and no `.comment` with the clang version.
/// `medany` addresses globals pc-relatively (`auipc`), which works for a
/// kernel linked at 0x8000_0000.
pub const C_FLAGS: &[&str] = &[
    "-mcmodel=medany",
    "-std=c11",
    "-O2",
    "-ffreestanding",
    "-fno-builtin",
    "-nostdinc",
    "-nostdlib",
    "-fno-pic",
    "-fno-common",
    "-fno-ident",
    "-fno-stack-protector",
    "-fno-asynchronous-unwind-tables",
    "-fno-unwind-tables",
    "-Wall",
    "-Wextra",
    "-Werror",
    "-Iinclude",
];

/// Generous step budget for running the kernel (booting takes ~20k steps).
pub const BOOT_MAX_STEPS: u64 = 2_000_000;

/// Next to the committed `kernel/kernel.elf`: the `clang --version` line it
/// was built with. Tests rebuild and compare bytes only with that same clang.
pub const VERSION_FILE: &str = "kernel.elf.clang-version";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Assembler {
    /// clang's integrated assembler.
    Clang,
    /// Our own merlin-as (as a library). Bootstrapping step 1: our `.o`
    /// linked by ld.lld must behave the same.
    Merlin,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    C,
    Asm,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Source {
    /// File name inside the kernel directory, e.g. `main.c`.
    pub name: String,
    pub lang: Lang,
}

impl Source {
    /// Object file name: `main.c` → `main.c.o` (keeps `x.c` and `x.s` apart).
    pub fn object_name(&self) -> String {
        format!("{}.o", self.name)
    }
}

/// Lists the kernel's sources (`*.c`, `*.s`) in name order.
pub fn sources(kernel_dir: &Path) -> Result<Vec<Source>, String> {
    let entries =
        std::fs::read_dir(kernel_dir).map_err(|e| format!("cannot read {}: {e}", kernel_dir.display()))?;
    let mut out = Vec::new();
    for e in entries {
        let e = e.map_err(|e| e.to_string())?;
        if !e.file_type().map_err(|e| e.to_string())?.is_file() {
            continue;
        }
        let Ok(name) = e.file_name().into_string() else {
            continue;
        };
        let lang = match Path::new(&name).extension().and_then(|x| x.to_str()) {
            Some("c") => Lang::C,
            Some("s") => Lang::Asm,
            _ => continue,
        };
        out.push(Source { name, lang });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    if out.is_empty() {
        return Err(format!("no .c or .s files in {}", kernel_dir.display()));
    }
    Ok(out)
}

/// Everything needed for one kernel build. The tools run with `kernel_dir` as
/// their working directory, so relative `obj_dir`/`out` paths are taken
/// relative to it; pass absolute ones.
#[derive(Clone, Debug)]
pub struct Build {
    /// Directory holding the sources, `include/` and `kernel.ld`.
    pub kernel_dir: PathBuf,
    /// Where object files go.
    pub obj_dir: PathBuf,
    /// The linked ELF.
    pub out: PathBuf,
    pub clang: PathBuf,
    pub lld: PathBuf,
    pub assembler: Assembler,
    /// Print each command before running it.
    pub verbose: bool,
}

impl Build {
    /// clang arguments for one source (run with the kernel directory as cwd).
    pub fn compile_args(&self, src: &Source) -> Vec<OsString> {
        let mut args: Vec<OsString> = TARGET_FLAGS.iter().map(OsString::from).collect();
        if src.lang == Lang::C {
            args.extend(C_FLAGS.iter().map(OsString::from));
        }
        args.push("-c".into());
        args.push(src.name.clone().into());
        args.push("-o".into());
        args.push(self.obj_dir.join(src.object_name()).into());
        args
    }

    /// ld.lld arguments (run with the kernel directory as cwd).
    pub fn link_args(&self, sources: &[Source]) -> Vec<OsString> {
        let mut args: Vec<OsString> = vec![
            "-T".into(),
            "kernel.ld".into(),
            "-o".into(),
            self.out.clone().into(),
        ];
        args.extend(sources.iter().map(|s| self.obj_dir.join(s.object_name()).into()));
        args
    }

    /// Compiles, assembles and links; returns the output path.
    pub fn run(&self) -> Result<PathBuf, String> {
        let sources = sources(&self.kernel_dir)?;
        std::fs::create_dir_all(&self.obj_dir)
            .map_err(|e| format!("cannot create {}: {e}", self.obj_dir.display()))?;
        if let Some(parent) = self.out.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
        }
        for src in &sources {
            if src.lang == Lang::Asm && self.assembler == Assembler::Merlin {
                self.merlin_as(src)?;
            } else {
                self.exec(&self.clang, self.compile_args(src))?;
            }
        }
        self.exec(&self.lld, self.link_args(&sources))?;
        Ok(self.out.clone())
    }

    /// First line of `clang --version`, e.g. `clang version 21.1.5`.
    pub fn clang_version(&self) -> Result<String, String> {
        let out = Command::new(&self.clang)
            .arg("--version")
            .output()
            .map_err(|e| format!("cannot run {}: {e}", self.clang.display()))?;
        let text = String::from_utf8_lossy(&out.stdout);
        text.lines()
            .next()
            .map(|l| l.trim().to_string())
            .filter(|l| out.status.success() && !l.is_empty())
            .ok_or_else(|| format!("{} --version printed nothing", self.clang.display()))
    }

    fn merlin_as(&self, src: &Source) -> Result<(), String> {
        let path = self.kernel_dir.join(&src.name);
        if self.verbose {
            eprintln!("merlin-as {}", path.display());
        }
        let text =
            std::fs::read_to_string(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        let elf = merlin_as::assemble_to_elf(&text).map_err(|errs| {
            let lines: Vec<String> = errs.iter().map(|e| format!("{}: {e}", src.name)).collect();
            format!("merlin-as failed:\n{}", lines.join("\n"))
        })?;
        let obj = self.obj_dir.join(src.object_name());
        std::fs::write(&obj, elf).map_err(|e| format!("cannot write {}: {e}", obj.display()))
    }

    fn exec(&self, tool: &Path, args: Vec<OsString>) -> Result<(), String> {
        let line = || {
            let mut s = tool.display().to_string();
            for a in &args {
                s.push(' ');
                s.push_str(&a.to_string_lossy());
            }
            s
        };
        if self.verbose {
            eprintln!("{}", line());
        }
        let status = Command::new(tool)
            .args(&args)
            .current_dir(&self.kernel_dir)
            .status()
            .map_err(|e| format!("cannot run {}: {e}", tool.display()))?;
        if status.success() {
            Ok(())
        } else {
            Err(format!("command failed ({status}):\n  {}", line()))
        }
    }
}

/// Runs a kernel image on Grail with the UART captured. Returns how it ended
/// and everything it printed (UART and `xcal.dbg`).
pub fn run_on_grail(elf: &[u8], max_steps: u64) -> Result<(RunResult, String), String> {
    let mut m = Machine::with_ram(grail::DEFAULT_RAM_SIZE);
    m.cpu.bus.uart.capture = true;
    m.load_elf(elf)?;
    let r = m.run(Some(max_steps), |_| {});
    let out = std::mem::take(&mut m.cpu.bus.uart.captured);
    Ok((r, String::from_utf8_lossy(&out).into_owned()))
}

/// What `cargo xtask kernel --run` does once the image is built: boots `elf` on a
/// default Grail machine for at most `max_steps` steps, with UART output going to `uart`
/// and `xcal.dbg` output to `debug`. Returns the result and the machine in its final
/// state.
pub fn run_with_output(
    elf: &[u8],
    max_steps: u64,
    uart: impl Write + Send + 'static,
    debug: impl Write + Send + 'static,
) -> Result<(RunResult, Machine), String> {
    let mut m = Machine::with_ram(grail::DEFAULT_RAM_SIZE);
    m.load_elf(elf)?;
    m.set_uart_output(uart);
    m.set_debug_output(debug);
    let r = m.run(Some(max_steps), |_| {});
    Ok((r, m))
}

/// Replaces the program counter in `[dbg] pc=XXXXXXXX` lines with dots, so
/// output can be compared across builds whose code layout differs.
pub fn mask_dbg_pc(out: &str) -> String {
    out.split_inclusive('\n')
        .map(|line| match line.strip_prefix("[dbg] pc=") {
            Some(rest) if rest.len() >= 8 && rest.is_char_boundary(8) => {
                format!("[dbg] pc=........{}", &rest[8..])
            }
            _ => line.to_string(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build() -> Build {
        Build {
            kernel_dir: PathBuf::from("kernel"),
            obj_dir: PathBuf::from("obj"),
            out: PathBuf::from("out/kernel.elf"),
            clang: PathBuf::from("clang"),
            lld: PathBuf::from("ld.lld"),
            assembler: Assembler::Clang,
            verbose: false,
        }
    }

    fn strings(args: &[OsString]) -> Vec<String> {
        args.iter().map(|a| a.to_string_lossy().into_owned()).collect()
    }

    #[test]
    fn lists_kernel_sources_in_order() {
        let srcs = sources(&crate::repo_root().join("kernel")).unwrap();
        let names: Vec<&str> = srcs.iter().map(|s| s.name.as_str()).collect();
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted);
        for want in ["entry.s", "kprintf.c", "main.c", "uart.c"] {
            assert!(names.contains(&want), "{want} missing from {names:?}");
        }
        assert!(srcs
            .iter()
            .all(|s| (s.lang == Lang::Asm) == s.name.ends_with(".s")));
        // kernel.ld, kernel.elf and include/ are not sources.
        assert!(!names
            .iter()
            .any(|n| n.ends_with(".ld") || n.ends_with(".elf") || *n == "include"));
    }

    #[test]
    fn no_sources_is_an_error() {
        let dir = std::env::temp_dir().join(format!("xtask-empty-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(sources(&dir).unwrap_err().contains("no .c or .s files"));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(sources(&dir).unwrap_err().contains("cannot read"));
    }

    #[test]
    fn object_names_keep_extension() {
        let c = Source {
            name: "x.c".into(),
            lang: Lang::C,
        };
        let s = Source {
            name: "x.s".into(),
            lang: Lang::Asm,
        };
        assert_ne!(c.object_name(), s.object_name());
        assert_eq!(c.object_name(), "x.c.o");
    }

    #[test]
    fn c_compile_line() {
        let b = build();
        let args = strings(&b.compile_args(&Source {
            name: "main.c".into(),
            lang: Lang::C,
        }));
        for flag in [
            "--target=riscv32-unknown-elf",
            "-march=rv32im_zicsr",
            "-mabi=ilp32",
            "-mno-relax",
            "-ffreestanding",
            "-nostdinc",
            "-nostdlib",
            "-Iinclude",
            "-c",
        ] {
            assert!(args.iter().any(|a| a == flag), "{flag} missing from {args:?}");
        }
        // The source is passed relative to the kernel directory, so the
        // object doesn't record where the repository lives.
        let c = args.iter().position(|a| a == "-c").unwrap();
        assert_eq!(args[c + 1], "main.c");
        assert_eq!(args[c + 2], "-o");
        assert_eq!(PathBuf::from(&args[c + 3]), Path::new("obj").join("main.c.o"));
    }

    #[test]
    fn asm_compile_line_has_no_c_flags() {
        let args = strings(&build().compile_args(&Source {
            name: "entry.s".into(),
            lang: Lang::Asm,
        }));
        assert!(args.iter().any(|a| a == "-mno-relax"));
        assert!(!args
            .iter()
            .any(|a| a == "-std=c11" || a == "-Werror" || a == "-mcmodel=medany"));
    }

    #[test]
    fn link_line() {
        let srcs = [
            Source {
                name: "entry.s".into(),
                lang: Lang::Asm,
            },
            Source {
                name: "main.c".into(),
                lang: Lang::C,
            },
        ];
        let args = strings(&build().link_args(&srcs));
        assert_eq!(args[..3], ["-T", "kernel.ld", "-o"]);
        assert_eq!(PathBuf::from(&args[3]), Path::new("out/kernel.elf"));
        assert_eq!(PathBuf::from(&args[4]), Path::new("obj").join("entry.s.o"));
        assert_eq!(PathBuf::from(&args[5]), Path::new("obj").join("main.c.o"));
        assert_eq!(args.len(), 6);
    }

    #[test]
    fn masks_dbg_pc() {
        let out = "hi\n[dbg] pc=80000123 a0 = 0x00000000 (0)\n[dbg] pc=8\n";
        assert_eq!(
            mask_dbg_pc(out),
            "hi\n[dbg] pc=........ a0 = 0x00000000 (0)\n[dbg] pc=8\n"
        );
        assert_eq!(mask_dbg_pc(""), "");
    }

    #[test]
    fn missing_tool_is_reported() {
        let mut b = build();
        b.kernel_dir = crate::repo_root().join("kernel");
        b.obj_dir = std::env::temp_dir().join(format!("xtask-missing-tool-{}", std::process::id()));
        b.out = b.obj_dir.join("kernel.elf");
        b.clang = b.obj_dir.join("no-such-clang");
        let err = b.run().unwrap_err();
        assert!(err.contains("cannot run"), "{err}");
        let _ = std::fs::remove_dir_all(&b.obj_dir);
    }
}
