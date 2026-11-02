//! Kernel tests. The committed `kernel/kernel.elf` always runs on Grail, so
//! these pass without LLVM. When clang and ld.lld are installed, the kernel is
//! also rebuilt from source and checked against the committed image.

use grail::{Machine, RunResult};
use std::path::{Path, PathBuf};
use xtask::kernel::{self, Assembler, Build, Lang, BOOT_MAX_STEPS};
use xtask::{repo_root, tools};

/// What the kernel prints, with `xcal.dbg`'s pc masked (see `mask_dbg_pc`).
const BOOT_OUTPUT: &str = "\
Camelot v0.1.0 booting...
kprintf: d=-42 u=4294967295 x=cafef00d p=0x80000000 s=excalibur c=K %=%
kprintf self-test: 26/26 passed
[dbg] pc=........ a0 = 0x00000000 (0)
";

fn kernel_dir() -> PathBuf {
    repo_root().join("kernel")
}

fn committed_elf() -> Vec<u8> {
    let path = kernel_dir().join("kernel.elf");
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn assert_boots(elf: &[u8]) {
    let (r, out) = kernel::run_on_grail(elf, BOOT_MAX_STEPS).unwrap();
    assert_eq!(kernel::mask_dbg_pc(&out), BOOT_OUTPUT, "raw output:\n{out}");
    assert_eq!(r, RunResult::Exited(0));
}

/// Loads the committed kernel without running it.
fn load_committed() -> (Machine, grail::elf::Elf) {
    let file = committed_elf();
    let elf = grail::elf::parse(&file).unwrap();
    let mut m = Machine::new(grail::DEFAULT_RAM_SIZE);
    m.cpu.bus.uart.capture = true;
    m.load_elf(&file).unwrap();
    (m, elf)
}

fn sym(elf: &grail::elf::Elf, name: &str) -> u32 {
    elf.symbol(name).unwrap_or_else(|| panic!("no symbol {name}"))
}

#[test]
fn committed_kernel_boots() {
    assert_boots(&committed_elf());
}

#[test]
fn committed_kernel_layout() {
    let (_, elf) = load_committed();
    // The boot ROM jumps to 0x8000_0000 (spec section 7); _start must be there.
    assert_eq!(elf.entry, 0x8000_0000);
    assert_eq!(sym(&elf, "_start"), 0x8000_0000);
    for s in [
        "kmain",
        "kprintf",
        "ksnprintf",
        "kvformat",
        "uart_putc",
        "xcal_dbg",
        "xcal_halt",
    ] {
        assert!(sym(&elf, s) > 0x8000_0000, "{s}");
    }
    let (bss, bss_end) = (sym(&elf, "__bss_start"), sym(&elf, "__bss_end"));
    let (stack_bottom, stack_top) = (sym(&elf, "__stack_bottom"), sym(&elf, "__stack_top"));
    assert!(bss <= bss_end && bss.is_multiple_of(4) && bss_end.is_multiple_of(4));
    assert!(bss_end <= stack_bottom && stack_top - stack_bottom == 16 * 1024);
    assert!(stack_top.is_multiple_of(16), "ABI requires a 16-byte aligned sp");
    let end = sym(&elf, "__kernel_end");
    assert!(end >= stack_top && end.is_multiple_of(4096));
}

#[test]
fn entry_clears_bss() {
    // Fill .bss with garbage after loading: the self-test counters live there,
    // so the output is only right if entry.s zeroes it.
    let (mut m, elf) = load_committed();
    let (start, end) = (sym(&elf, "__bss_start"), sym(&elf, "__bss_end"));
    assert!(end > start, "kernel has no .bss to clear");
    m.cpu
        .bus
        .load(start, &vec![0xa5; (end - start) as usize])
        .unwrap();
    let r = m.run(Some(BOOT_MAX_STEPS), |_| {});
    let out = String::from_utf8_lossy(&m.cpu.bus.uart.captured).into_owned();
    assert_eq!(kernel::mask_dbg_pc(&out), BOOT_OUTPUT);
    assert_eq!(r, RunResult::Exited(0));
}

#[test]
fn early_trap_reports_and_halts() {
    // Replace kmain's first instruction with an illegal one (all zeros): the
    // early trap handler prints mcause, mepc, mtval and exits with 0xff.
    let (mut m, elf) = load_committed();
    let kmain = sym(&elf, "kmain");
    m.cpu.bus.load(kmain, &[0, 0, 0, 0]).unwrap();
    let r = m.run(Some(BOOT_MAX_STEPS), |_| {});
    let out = String::from_utf8_lossy(&m.cpu.bus.uart.captured).into_owned();
    let values: Vec<&str> = out.lines().map(|l| l.split(" = ").nth(1).unwrap_or(l)).collect();
    assert_eq!(
        values,
        [
            "0x00000002 (2)".to_string(),                // mcause: illegal instruction
            format!("{kmain:#010x} ({})", kmain as i32), // mepc
            "0x00000000 (0)".to_string(),                // mtval: the instruction bits
        ],
        "output:\n{out}"
    );
    assert_eq!(r, RunResult::Exited(0xff));
}

#[test]
fn halt_uses_kmain_return_value() {
    // Make kmain return 7 straight away (li a0, 7; ret).
    let (mut m, elf) = load_committed();
    let kmain = sym(&elf, "kmain");
    let li_a0_7: u32 = (7 << 20) | (10 << 7) | 0x13;
    let ret: u32 = 0x0000_8067;
    let mut code = li_a0_7.to_le_bytes().to_vec();
    code.extend_from_slice(&ret.to_le_bytes());
    m.cpu.bus.load(kmain, &code).unwrap();
    assert_eq!(m.run(Some(BOOT_MAX_STEPS), |_| {}), RunResult::Exited(7));
    assert!(m.cpu.bus.uart.captured.is_empty());
}

/// Every C file and header in the kernel, with its name.
fn kernel_c_sources() -> Vec<(String, String)> {
    let mut files = Vec::new();
    for dir in [kernel_dir(), kernel_dir().join("include")] {
        for e in std::fs::read_dir(&dir).unwrap() {
            let path = e.unwrap().path();
            if matches!(path.extension().and_then(|x| x.to_str()), Some("c" | "h")) {
                let text = std::fs::read_to_string(&path).unwrap();
                files.push((path.file_name().unwrap().to_string_lossy().into_owned(), text));
            }
        }
    }
    assert!(files.len() >= 8);
    files
}

#[test]
fn kernel_c_stays_freestanding() {
    // No libc headers (everything is #include "...") and no floating point,
    // so Merlin's C subset (#15) can compile it later.
    for (name, text) in kernel_c_sources() {
        for (i, line) in text.lines().enumerate() {
            let code = line.split("/*").next().unwrap();
            assert!(!code.contains("#include <"), "{name}:{}: system header", i + 1);
            for word in code.split(|c: char| !c.is_ascii_alphanumeric() && c != '_') {
                assert!(
                    word != "float" && word != "double",
                    "{name}:{}: floating point",
                    i + 1
                );
            }
        }
    }
}

#[test]
fn kernel_asm_is_merlin_dialect() {
    // The .s files must use only what merlin-as understands.
    let sources = kernel::sources(&kernel_dir()).unwrap();
    let asm: Vec<_> = sources.iter().filter(|s| s.lang == Lang::Asm).collect();
    assert!(!asm.is_empty());
    for s in asm {
        let text = std::fs::read_to_string(kernel_dir().join(&s.name)).unwrap();
        if let Err(errs) = merlin_as::assemble(&text) {
            let errs: Vec<String> = errs.iter().map(|e| format!("{}: {e}", s.name)).collect();
            panic!("merlin-as rejects kernel assembly:\n{}", errs.join("\n"));
        }
    }
}

// ---- Rebuilding from source (skipped without LLVM) ----

fn llvm() -> Option<(PathBuf, PathBuf)> {
    let clang = tools::find("clang")?;
    let lld = tools::find_beside("ld.lld", &clang)?;
    Some((clang, lld))
}

fn scratch(name: &str) -> PathBuf {
    Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("kernel-tests")
        .join(name)
}

fn fresh_build(clang: PathBuf, lld: PathBuf, assembler: Assembler, name: &str) -> Result<Vec<u8>, String> {
    let dir = scratch(name);
    let _ = std::fs::remove_dir_all(&dir);
    let build = Build {
        kernel_dir: kernel_dir(),
        obj_dir: dir.join("obj"),
        out: dir.join("kernel.elf"),
        clang,
        lld,
        assembler,
        verbose: true,
    };
    let out = build.run()?;
    Ok(std::fs::read(out).unwrap())
}

#[test]
fn clang_build_boots_and_matches_committed() {
    let Some((clang, lld)) = llvm() else {
        eprintln!("skipped: clang/ld.lld not found");
        return;
    };
    let elf = fresh_build(clang.clone(), lld, Assembler::Clang, "clang").unwrap();
    assert_boots(&elf);

    // With the clang that built the committed image the bytes must match, or
    // kernel.elf is stale: rebuild it with `cargo xtask kernel`.
    let probe = Build {
        kernel_dir: kernel_dir(),
        obj_dir: PathBuf::new(),
        out: PathBuf::new(),
        clang,
        lld: PathBuf::new(),
        assembler: Assembler::Clang,
        verbose: false,
    };
    let recorded = std::fs::read_to_string(kernel_dir().join(kernel::VERSION_FILE)).unwrap();
    match probe.clang_version() {
        Ok(v) if v == recorded.trim() => assert!(
            elf == committed_elf(),
            "kernel/kernel.elf is out of date: run `cargo xtask kernel` and commit it"
        ),
        Ok(v) => eprintln!(
            "byte comparison skipped: built with `{}`, have `{v}`",
            recorded.trim()
        ),
        Err(e) => panic!("{e}"),
    }
}

#[test]
fn merlin_as_build_boots() {
    // Bootstrapping step 1 (spec section 10): kernel .s files assembled by
    // merlin-as, C by clang, linked by ld.lld.
    let Some((clang, lld)) = llvm() else {
        eprintln!("skipped: clang/ld.lld not found");
        return;
    };
    assert_boots(&fresh_build(clang, lld, Assembler::Merlin, "merlin").unwrap());
}
