//! Every example built with our own tools, merlin-as + merlin-ld, as
//! `cargo xtask examples` builds it (roadmap step 6, issue #8). No LLVM:
//!
//! * the committed `examples/<dir>/<name>.elf` files are exactly what the
//!   current tools build from the sources;
//! * each freshly built program runs in Grail, on a machine without firmware
//!   (`--bios none`: the examples are bare-metal M-mode programs), with input
//!   where it reads some (UART bytes, key events, pointer input, a disk,
//!   another machine on a network), and prints what it should and exits with
//!   the right code;
//! * `hello` and `multiobj` load byte for byte like ld.lld's link of clang's
//!   objects (the references in `crates/merlin-ld/tests/fixtures`, made by
//!   `cargo xtask ld-fixtures`), and merlin-objdump's disassembly of them is
//!   llvm-objdump's of ld.lld's. What differs outside the loaded bytes is
//!   pinned down here and listed in docs/excalibur-spec.md section 10.

use elf::{Elf, PT_GNU_STACK, PT_LOAD, PT_RISCV_ATTRIBUTES, SHF_ALLOC, SHT_SYMTAB};
use grail::block::SECTOR_SIZE;
use grail::keyboard::{code, KeyEvent};
use grail::net::Hub;
use grail::pointer::{PointerInput, BUTTON_LEFT};
use grail::{DiskConfig, Machine, MachineConfig, NetConfig, RunResult};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::channel;
use xtask::examples::{self, Example, EXAMPLES};
use xtask::repo_root;

/// Enough for colorbars, which writes every pixel of the 640x480 framebuffer.
const MAX_STEPS: u64 = 20_000_000;

fn build(ex: &Example) -> examples::Built {
    examples::build(&repo_root(), ex).unwrap_or_else(|e| panic!("{}: {e}", ex.name))
}

fn example(name: &str) -> &'static Example {
    examples::find(name).unwrap_or_else(|| panic!("no example {name}"))
}

/// What a program gets and must do.
struct Scenario {
    /// Bytes waiting on the UART's receiver.
    stdin: &'static [u8],
    /// Key events waiting in the keyboard FIFO.
    keys: Vec<KeyEvent>,
    /// Pointer input, given to the pointer device before the keys.
    pointer: Vec<PointerInput>,
    /// A disk image (relative to the repository root) attached as disk 0, in
    /// memory so the committed file is never written.
    disk: Option<&'static str>,
    /// Everything the UART and `xcal.dbg` print.
    output: &'static str,
    exit: u32,
}

/// Press and release of each key, then a press of Esc (what ends both
/// keyboard examples).
fn typed(text: &str) -> Vec<KeyEvent> {
    let mut keys: Vec<KeyEvent> = text
        .bytes()
        .flat_map(|c| [KeyEvent::press(c), KeyEvent::release(c)])
        .collect();
    keys.push(KeyEvent::press(code::ESC));
    keys
}

/// A scenario with no input.
fn plain(output: &'static str, exit: u32) -> Scenario {
    Scenario {
        stdin: b"",
        keys: Vec::new(),
        pointer: Vec::new(),
        disk: None,
        output,
        exit,
    }
}

/// The pointer events of the `pointer` example: three, read in one interrupt (the
/// pointer's priority is above the keyboard's), then Esc.
fn pointer_input(output: &'static str) -> Scenario {
    Scenario {
        pointer: vec![
            PointerInput::move_to(50, 60),
            PointerInput::buttons(BUTTON_LEFT),
            PointerInput::move_to(120, 90).with_buttons(BUTTON_LEFT),
        ],
        keys: vec![KeyEvent::press(code::ESC)],
        ..plain(output, 0)
    }
}

/// What `name` gets and must do. `xcal.dbg` shows the 64-bit pc and register,
/// and the programs print addresses and sizes in 16 hex digits.
fn scenario(name: &str) -> Scenario {
    match name {
        "hello" => plain(
            "Hello from Camelot!\n[dbg] pc=0000000080000018 a0 = 0x0000000000000014 (20)\n",
            0,
        ),
        // Both strings (47 bytes, counted in print.S's .bss), then exit code
        // 42 from main.S's .data.
        "multiobj" => plain(
            "Hello from two objects!\nputs lives in print.S.\n\
             [dbg] pc=0000000080000034 a0 = 0x000000000000002f (47)\n",
            42,
        ),
        // All 29 cases passed.
        "muldiv" => plain("[dbg] pc=0000000080000328 a0 = 0x000000000000001d (29)\n", 0),
        "sysinfo" => plain(
            "sysinfo v2\n\
             RAM      4 MiB at 0x0000000080000000\n\
             CPU      10000000 Hz (nominal)\n\
             timebase 10000000 Hz\n\
             10 devices:\n  \
             test      0x0000000000100000 + 0x0000000000001000\n  \
             CLINT     0x0000000002000000 + 0x0000000000010000\n  \
             PLIC      0x000000000c000000 + 0x0000000004000000\n  \
             UART      0x0000000010000000 + 0x0000000000000100  irq 1\n  \
             fb-ctrl   0x0000000030000000 + 0x0000000000001000  irq 16\n  \
             keyboard  0x0000000030001000 + 0x0000000000001000  irq 2\n  \
             sysinfo   0x0000000030002000 + 0x0000000000001000\n  \
             pointer   0x0000000030003000 + 0x0000000000001000  irq 11\n  \
             rtc       0x0000000030004000 + 0x0000000000001000  irq 17\n  \
             fb-vram   0x0000000031000000 + 0x000000000012c000\n",
            0,
        ),
        // Sector 0's text, the failed read past the end, and the write test on
        // the last sector.
        "disk" => Scenario {
            disk: Some(DISK_IMG),
            ..plain(
                "disk 0x0000000010001000, irq 3: 8 sectors\n\
                 sector 0:\n\
                 Hello from sector 0 of disk.img!\n\
                 This text is on the disk image, not in the program.\n\
                 read past the end: error 2\n\
                 write/read-back of sector 7: ok\n",
                0,
            )
        },
        // One interrupt per byte, EOT included (and not echoed).
        "uart-echo" => Scenario {
            stdin: b"hi\n\x04",
            ..plain("hi\n[dbg] pc=00000000800000b0 s0 = 0x0000000000000004 (4)\n", 0)
        },
        // Draws into the framebuffer; checked in `examples_run_in_grail`.
        "colorbars" => plain("", 0),
        // Presses seen, Esc included.
        "kbd-echo" => Scenario {
            keys: typed("ok"),
            ..plain("ok[dbg] pc=00000000800000f0 s1 = 0x0000000000000003 (3)\n", 0)
        },
        // One interrupt per event: two taps and the Esc press.
        "kbd-echo-irq" => Scenario {
            keys: typed("ok"),
            ..plain("ok[dbg] pc=00000000800000b4 s0 = 0x0000000000000005 (5)\n", 0)
        },
        // The cursor is checked in `examples_run_in_grail`.
        "pointer" => pointer_input("[dbg] pc=0000000080000180 s4 = 0x0000000000000003 (3)\n"),
        // Eight frames shown; the frames are checked in `examples_run_in_grail`.
        "flip" => plain("[dbg] pc=000000008000013c s4 = 0x0000000000000008 (8)\n", 0),
        // Six places of the cursor over one frame; `crates/grail/tests/framebuffer.rs`
        // checks what the display shows at each.
        "cursor" => plain("[dbg] pc=000000008000019c s1 = 0x0000000000000006 (6)\n", 0),
        // One hart: every count is its own, and no store-conditional can fail.
        // `smp_example_runs_on_every_hart_count` runs it on more.
        "smp" => plain(
            "harts: 1\n\
             amoadd: 1000 = 1 x 1000\n\
             lr/sc: 1000 = 1 x 1000, 0 retries\n\
             ipi: 3 interrupts, 3 on each hart\n",
            0,
        ),
        // Alone, without a network card; `net_examples_talk_over_a_hub` connects them.
        "net-ping" | "net-pong" => plain("no network card (try grail --bios none --net hub)\n", 1),
        // Alone too; crates/grail/tests/net_user.rs runs it behind the NAT gateway.
        "net-user" => plain("no network card (try grail --bios none --net user)\n", 1),
        other => panic!("no scenario for example {other}: add one"),
    }
}

/// The image `examples/disk` reads, relative to the repository root.
const DISK_IMG: &str = "examples/disk/disk.img";

/// Runs `elf` on a default machine (no firmware) whose RAM starts as garbage (so
/// `.bss` must really be zeroed by the loader) with the scenario's input.
fn run(elf: &[u8], s: &Scenario) -> (RunResult, String, Machine) {
    let disks = match s.disk {
        Some(img) => vec![DiskConfig::memory(std::fs::read(repo_root().join(img)).unwrap())],
        None => Vec::new(),
    };
    let mut m = Machine::new(MachineConfig {
        disks,
        ..MachineConfig::default()
    })
    .unwrap();
    assert_eq!(m.config().bios, grail::Bios::None);
    m.cpu.bus.ram.fill(0xa5);
    m.cpu.bus.uart.capture = true;
    m.load_elf(elf).unwrap();
    let (tx, rx) = channel();
    m.cpu.bus.uart.input = Some(rx);
    for &b in s.stdin {
        tx.send(b).unwrap();
    }
    for &p in &s.pointer {
        assert!(m.pointer_event(p), "pointer FIFO full, or {p:?} changes nothing");
    }
    for &k in &s.keys {
        assert!(m.key_event(k), "keyboard FIFO full");
    }
    let r = m.run(Some(MAX_STEPS), |_| {});
    let out = String::from_utf8(std::mem::take(&mut m.cpu.bus.uart.captured)).unwrap();
    (r, out, m)
}

/// Assembly sources (`.S`/`.s`) directly inside each `examples/` directory.
fn sources_on_disk() -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    for dir in std::fs::read_dir(repo_root().join("examples")).unwrap() {
        let dir = dir.unwrap().path();
        if !dir.is_dir() {
            continue;
        }
        for f in std::fs::read_dir(&dir).unwrap() {
            let f = f.unwrap().path();
            if f.extension().is_some_and(|e| e == "S" || e == "s") {
                let name = |p: &Path| p.file_name().unwrap().to_string_lossy().into_owned();
                found.insert(format!("{}/{}", name(&dir), name(&f)));
            }
        }
    }
    found
}

#[test]
fn every_example_source_is_built() {
    let listed: BTreeSet<String> = EXAMPLES
        .iter()
        .flat_map(|e| e.sources.iter().map(move |s| format!("{}/{s}", e.dir)))
        .collect();
    assert_eq!(
        listed,
        sources_on_disk(),
        "examples/ and xtask::examples::EXAMPLES disagree"
    );
}

/// The command lines at the top of each example's first source are the ones
/// `cargo xtask examples -v` prints for it, with the objects in the current
/// directory.
#[test]
fn the_sources_give_their_build_commands() {
    for ex in EXAMPLES {
        let first = ex.sources[0];
        let text = std::fs::read_to_string(ex.dir(&repo_root()).join(first)).unwrap();
        for cmd in examples::commands(ex, "OBJ") {
            let cmd = cmd.replace("OBJ/", "");
            assert!(text.contains(&format!("#   {cmd}\n")), "{first}: {cmd}");
        }
    }
}

#[test]
fn committed_examples_are_current() {
    for ex in EXAMPLES {
        let built = build(ex);
        assert!(built.warnings.is_empty(), "{}: {:?}", ex.name, built.warnings);
        let path = ex.elf_path(&repo_root());
        let committed = std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        assert!(
            committed == built.elf,
            "{} is not what merlin-as + merlin-ld build: run `cargo xtask examples {}` and commit it",
            ex.elf_name(),
            ex.name
        );
        let class = Elf::parse(&committed).unwrap().class;
        assert_eq!(class, elf::Class::Elf64, "{}", ex.elf_name());
    }
}

/// Acceptance of #8 and #151: every example, built from source with our tools,
/// runs.
#[test]
fn examples_run_in_grail() {
    for ex in EXAMPLES {
        let s = scenario(ex.name);
        let (r, out, mut m) = run(&build(ex).elf, &s);
        assert_eq!(
            (r, out.as_str()),
            (RunResult::Exited(s.exit), s.output),
            "{}",
            ex.name
        );
        if ex.name == "colorbars" {
            let fb = &m.cpu.bus.fb;
            assert!(fb.enabled());
            assert_eq!(fb.presents, 1);
            // The grey border, then the first and last of the eight bars.
            assert_eq!(fb.pixel(0, 0), 0x808080);
            assert_eq!(fb.pixel(8, 8), 0xffffff);
            assert_eq!(fb.pixel(631, 471), 0x000000);
        }
        if ex.name == "pointer" {
            // The red arrow (left button held) at the last position, on the
            // background; the first position was erased.
            let fb = &m.cpu.bus.fb;
            assert!(fb.enabled());
            assert_eq!(fb.presents, 2, "the first frame, then one per interrupt");
            assert_eq!(fb.pixel(120, 90), 0xff0000);
            assert_eq!(fb.pixel(127, 97), 0xff0000);
            assert_eq!(fb.pixel(121, 90), 0x203048);
            assert_eq!(fb.pixel(50, 60), 0x203048);
            assert_eq!(fb.pixel(0, 0), 0x203048);
        }
        if ex.name == "flip" {
            // 800x600 from a buffer in RAM, after one modeset and seven flips; the last
            // frame's bands moved down 7 x 25 rows: row 0 is in band 175 / 75 = 2.
            let fb = &m.cpu.bus.fb;
            assert_eq!((fb.modesets, fb.flips, fb.presents), (1, 7, 0));
            assert!(fb.scanout() >= grail::bus::RAM_BASE);
            let frame = m.cpu.bus.frame();
            assert_eq!((frame.width, frame.height), (800, 600));
            assert_eq!(frame.pixel(0, 0), 0x00ffff);
            assert_eq!(frame.pixel(799, 49), 0x00ffff);
            assert_eq!(frame.pixel(0, 50), 0x00ff00);
            assert_eq!(frame.pixel(799, 500), 0xffff00);
            assert_eq!(frame.pixel(0, 599), 0x00ffff);
        }
        if let Some(img) = s.disk {
            // The write test put the last sector back as it was.
            let backend = m.cpu.bus.disks[0].backend();
            let mut data = vec![0; backend.capacity() as usize * SECTOR_SIZE];
            backend.read(0, &mut data).unwrap();
            assert!(
                data == std::fs::read(repo_root().join(img)).unwrap(),
                "{}: disk changed",
                ex.name
            );
        }
    }
}

/// The scenarios really depend on the input: without the keys or the bytes,
/// the interactive examples wait forever.
#[test]
fn interactive_examples_wait_for_input() {
    for name in ["uart-echo", "kbd-echo", "kbd-echo-irq", "pointer"] {
        let idle = plain("", 0);
        let (r, out, _) = run(&build(example(name)).elf, &idle);
        assert_eq!((r, out.as_str()), (RunResult::StepLimit, ""), "{name}");
    }
}

/// `examples/net`: ping and pong, freshly built, on two machines connected by a
/// hub (the card has 16-byte descriptors and a 64-bit address, printed in 16
/// digits). Pong starts first, so its card has buffers when ping's frame comes.
#[test]
fn net_examples_talk_over_a_hub() {
    let hub = Hub::new();
    let boot = |name: &str| {
        let config = MachineConfig {
            nets: vec![NetConfig::hub(&hub)],
            ..MachineConfig::default()
        };
        let mut m = Machine::new(config).unwrap();
        m.cpu.bus.uart.capture = true;
        m.load_elf(&build(example(name)).elf).unwrap();
        m
    };
    let mut ping = boot("net-ping");
    let mut pong = boot("net-pong");
    pong.run_for(20_000, &mut |_| {});
    for _ in 0..1000 {
        ping.run_for(1000, &mut |_| {});
        pong.run_for(1000, &mut |_| {});
    }
    let out = |m: &mut Machine| String::from_utf8(std::mem::take(&mut m.cpu.bus.uart.captured)).unwrap();
    assert_eq!(
        (ping.exit_code(), out(&mut ping)),
        (
            Some(0),
            "net 0x0000000010009000, irq 12, mac 02:ca:00:00:00:01, link up\n\
             sent 60 bytes to ff:ff:ff:ff:ff:ff\n\
             received 60 bytes from 02:ca:00:00:00:02, type 0x88b5: Hello back from pong!\n"
                .to_string()
        )
    );
    assert_eq!(
        (pong.exit_code(), out(&mut pong)),
        (
            Some(0),
            "net 0x0000000010009000, irq 12, mac 02:ca:00:00:00:02, link up\n\
             received 60 bytes from 02:ca:00:00:00:01, type 0x88b5: Hello from ping!\n\
             sent 60 bytes to 02:ca:00:00:00:01\n"
                .to_string()
        )
    );
}

/// `examples/smp`, freshly built, on 2 to 8 harts: hart 0 wakes the others with
/// an IPI, the amoadd and lr/sc counters lose no increment although the harts
/// race for them (so some store-conditionals fail), and each hart takes its
/// share of the IPIs.
#[test]
fn smp_example_runs_on_every_hart_count() {
    let elf = build(example("smp")).elf;
    for harts in 2..=grail::MAX_HARTS {
        let mut m = Machine::new(MachineConfig {
            harts,
            ..MachineConfig::default()
        })
        .unwrap();
        m.cpu.bus.uart.capture = true;
        m.load_elf(&elf).unwrap();
        let r = m.run(Some(MAX_STEPS), |_| {});
        let out = String::from_utf8(std::mem::take(&mut m.cpu.bus.uart.captured)).unwrap();
        assert_eq!(r, RunResult::Exited(0), "{harts} harts:\n{out}");
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 4, "{harts} harts:\n{out}");
        assert_eq!(lines[0], format!("harts: {harts}"));
        assert_eq!(lines[1], format!("amoadd: {} = {harts} x 1000", harts * 1000));
        let lrsc = format!("lr/sc: {} = {harts} x 1000, ", harts * 1000);
        let retries: u32 = lines[2]
            .strip_prefix(&lrsc)
            .and_then(|r| r.strip_suffix(" retries"))
            .unwrap_or_else(|| panic!("{harts} harts: {:?}", lines[2]))
            .parse()
            .unwrap();
        assert!(retries > 0, "{harts} harts: no lr/sc contention");
        assert_eq!(lines[3], format!("ipi: {} interrupts, 3 on each hart", 3 * harts));
    }
}

/// Without a disk, `examples/disk` says so and exits with 1: the scenario's
/// disk is what it reads.
#[test]
fn disk_example_needs_its_disk() {
    let (r, _, _) = run(&build(example("disk")).elf, &plain("", 1));
    assert_eq!(r, RunResult::Exited(1));
}

/// The multi-object example breaks if a cross-object reference is wrong: the
/// strings, the counter in the other object's .bss and the .data pointers.
#[test]
fn multiobj_needs_both_objects() {
    let main_o = "main.o";
    let ex = example("multiobj");
    let built = build(ex);
    assert_eq!(
        built.objects.iter().map(|o| o.name.as_str()).collect::<Vec<_>>(),
        [main_o, "print.o"]
    );
    // main.o alone: puts, the strings and the counter are print.S's.
    let main = merlin_ld::Input {
        name: main_o.into(),
        data: built.objects[0].data.clone(),
    };
    let script = std::fs::read_to_string(ex.dir(&repo_root()).join("link.ld")).unwrap();
    let opts = merlin_ld::Options {
        script: Some(merlin_ld::ScriptSource {
            name: "link.ld".into(),
            text: script,
        }),
        entry: None,
    };
    let errors: Vec<String> = merlin_ld::link(&[main], &opts)
        .expect_err("main.o alone must not link")
        .into_iter()
        .map(|e| e.0)
        .collect();
    for sym in ["puts", "greeting", "farewell", "bytes_written"] {
        assert!(
            errors
                .iter()
                .any(|e| e.contains(&format!("undefined reference to '{sym}'"))),
            "{sym}: {errors:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Against ld.lld.

/// `file` of the fixture directory `dir`.
fn fixture(dir: &str, file: &str) -> PathBuf {
    repo_root()
        .join(xtask::ld_fixtures::FIXTURES)
        .join(dir)
        .join(file)
}

/// `PT_LOAD`s as (vaddr, memsz, file bytes).
fn loads(elf: &Elf) -> Vec<(u64, u64, Vec<u8>)> {
    elf.segments
        .iter()
        .filter(|p| p.ty == PT_LOAD)
        .map(|p| {
            let bytes = elf.data[p.offset as usize..(p.offset + p.filesz) as usize].to_vec();
            (p.vaddr, p.memsz, bytes)
        })
        .collect()
}

/// Allocated sections as (name, type, address, size, alignment, flags).
fn alloc_sections(elf: &Elf) -> Vec<(String, u32, u64, u64, u64, u64)> {
    elf.sections
        .iter()
        .filter(|s| s.flags & SHF_ALLOC != 0)
        .map(|s| (s.name.clone(), s.ty, s.addr, s.size, s.align, s.flags))
        .collect()
}

/// Symbols as (name, value), leaving out the `$x`/`$d` mapping symbols that
/// clang writes and merlin-as does not.
fn symbols(elf: &Elf) -> BTreeSet<(String, u64)> {
    let symtab = elf.sections.iter().find(|s| s.ty == SHT_SYMTAB).unwrap();
    elf.symbols_in(symtab)
        .unwrap()
        .into_iter()
        .filter(|s| !s.name.is_empty() && !s.name.starts_with('$'))
        .map(|s| (s.name, s.value))
        .collect()
}

fn section_names<'a>(elf: &'a Elf) -> Vec<&'a str> {
    elf.sections
        .iter()
        .map(|s| s.name.as_str())
        .filter(|n| !n.is_empty())
        .collect()
}

/// merlin-objdump -d of `elf` as llvm-objdump prints it: llvm-objdump doesn't
/// know the Xcal instructions (spec section 10, "Disassembler output").
fn llvm_style_disassembly(name: &str, elf: &[u8]) -> String {
    let opts = merlin_objdump::Options {
        disassemble: true,
        ..merlin_objdump::Options::default()
    };
    let text = merlin_objdump::dump(name, elf, &opts).unwrap();
    text.lines()
        .map(|l| match l.split_once("\txcal.") {
            Some((head, _)) => format!("{head}\t<unknown>\n"),
            None => format!("{l}\n"),
        })
        .collect()
}

/// Acceptance of #8: the merlin-built example against clang + ld.lld (both
/// with relaxation off; see the reference command at the top of each source).
fn assert_like_ld_lld(name: &str) {
    let ex = example(name);
    let ours_bytes = build(ex).elf;
    let lld_bytes = std::fs::read(fixture(ex.dir, "link.lld.elf")).unwrap();
    let (ours, lld) = (Elf::parse(&ours_bytes).unwrap(), Elf::parse(&lld_bytes).unwrap());
    assert_eq!(ours.class, lld.class, "{name}: class");

    // Everything Grail loads is the same.
    assert_eq!(ours.header.entry, lld.header.entry, "{name}: entry");
    assert_eq!(loads(&ours), loads(&lld), "{name}: PT_LOAD segments");
    assert_eq!(alloc_sections(&ours), alloc_sections(&lld), "{name}: sections");

    // The disassembly is the same text (llvm-objdump -d of ld.lld's output).
    let want = std::fs::read_to_string(fixture(ex.dir, "link.objdump.txt"))
        .unwrap()
        .replace("\r\n", "\n");
    assert_eq!(
        llvm_style_disassembly("link.lld.elf", &ours_bytes),
        want,
        "{name}: disassembly"
    );

    // Symbols: the same names and values, except the mapping symbols.
    let (ours_syms, lld_syms) = (symbols(&ours), symbols(&lld));
    assert_eq!(ours_syms, lld_syms, "{name}: symbols");

    // What only ld.lld writes (spec section 10, "Differences from ld.lld"):
    // the attributes and .comment sections and their program headers.
    let phdr_types = |e: &Elf| e.segments.iter().map(|p| p.ty).collect::<Vec<_>>();
    assert!(phdr_types(&ours).iter().all(|&t| t == PT_LOAD), "{name}");
    for t in [PT_GNU_STACK, PT_RISCV_ATTRIBUTES] {
        assert!(phdr_types(&lld).contains(&t), "{name}: ld.lld wrote no {t:#x}");
    }
    let mut ours_names = section_names(&ours);
    let mut lld_names = section_names(&lld);
    ours_names.sort_unstable();
    lld_names.retain(|n| ![".riscv.attributes", ".comment"].contains(n));
    lld_names.sort_unstable();
    assert_eq!(ours_names, lld_names, "{name}: section names");
}

#[test]
fn hello_is_like_ld_lld() {
    assert_like_ld_lld("hello");
}

#[test]
fn multiobj_is_like_ld_lld() {
    assert_like_ld_lld("multiobj");
}

/// Bootstrapping step 1 (merlin-as output linked by ld.lld, the committed
/// `cargo xtask hello` image) and the full chain load the same image.
#[test]
fn hello_linked_by_ld_lld_or_merlin_ld_loads_the_same() {
    let ours = build(example("hello")).elf;
    let lld = std::fs::read(repo_root().join(xtask::hello::GOLDEN)).unwrap();
    assert_eq!(
        xtask::hello::loaded_image(&ours).unwrap(),
        xtask::hello::loaded_image(&lld).unwrap()
    );
}
