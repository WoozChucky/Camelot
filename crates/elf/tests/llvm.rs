//! The elf crate against LLVM: files clang and ld.lld made, for RV32 (ELF32)
//! and RV64 (ELF64), read back as llvm-readelf and llvm-dwarfdump print them.
//!
//! tests/llvm/prog.c is compiled for each width into rv32/ and rv64/: an
//! object (`prog.o`), the same with linker relaxation (`prog-relax.o`, whose
//! debug sections hold R_RISCV_ADD/SUB pairs) and a linked program
//! (`prog.elf`). Next to each are `FILE.readelf`, what `llvm-readelf -h -S -l
//! -s -r` printed, `FILE.lines`, what `llvm-dwarfdump --debug-line` printed,
//! and `FILE.info`, what `llvm-dwarfdump --debug-info` printed (regenerate
//! them all with scripts/gen-elf-golden.sh). The test parses that text and
//! checks the elf crate reads the same header, sections, segments, symbols,
//! relocations, line table rows and debugging information entries.

use elf::{Class, Elf, Rela, Section, Symbol, STT_FUNC, STT_SECTION};
use std::path::{Path, PathBuf};

fn dir(xlen: u32) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("tests/llvm/rv{xlen}"))
}

const FILES: [&str; 3] = ["prog.o", "prog-relax.o", "prog.elf"];

fn hex(s: &str) -> u64 {
    u64::from_str_radix(s.trim_start_matches("0x"), 16).unwrap_or_else(|_| panic!("not hex: {s:?}"))
}

fn dec(s: &str) -> u64 {
    s.parse().unwrap_or_else(|_| panic!("not decimal: {s:?}"))
}

/// The first `n` words of `line`, and what follows them after one space (a
/// name, which may end in a space: LLVM's local labels are named `.L0 `).
fn words(line: &str, n: usize) -> (Vec<&str>, &str) {
    let mut out = Vec::new();
    let mut rest = line;
    for _ in 0..n {
        rest = rest.trim_start();
        let end = rest.find(' ').unwrap_or(rest.len());
        out.push(&rest[..end]);
        rest = &rest[end..];
    }
    (out, rest.strip_prefix(' ').unwrap_or(rest))
}

/// The lines of `text` after the one that starts with `start`, up to a blank one.
fn block<'a>(text: &'a str, start: &str) -> Vec<&'a str> {
    let mut lines = text.lines().skip_while(|l| !l.starts_with(start));
    lines.next().unwrap_or_else(|| panic!("no {start:?}"));
    lines.take_while(|l| !l.trim().is_empty()).collect()
}

/// A field of the ELF header: the text after `name:`.
fn header_field<'a>(text: &'a str, name: &str) -> &'a str {
    let key = format!("  {name}:");
    let line = text
        .lines()
        .find(|l| l.starts_with(&key))
        .unwrap_or_else(|| panic!("no {name}"));
    line[key.len()..].trim()
}

/// The number at the start of a header field: "64 (bytes into file)", or
/// "0x5, RVC, double-float ABI" for flags.
fn header_num(text: &str, name: &str) -> u64 {
    let v = header_field(text, name).split([' ', ',']).next().unwrap();
    if v.starts_with("0x") {
        hex(v)
    } else {
        dec(v)
    }
}

fn check_header(elf: &Elf, text: &str, xlen: u32) {
    let class = if xlen == 64 { Class::Elf64 } else { Class::Elf32 };
    assert_eq!(elf.class, class);
    assert_eq!(header_field(text, "Class"), format!("ELF{xlen}"));
    let h = &elf.header;
    let ty = match header_field(text, "Type").split(' ').next().unwrap() {
        "REL" => elf::ET_REL,
        "EXEC" => elf::ET_EXEC,
        t => panic!("type {t}"),
    };
    assert_eq!(h.ty, ty);
    assert_eq!(h.machine, elf::EM_RISCV);
    assert_eq!(h.entry, header_num(text, "Entry point address"));
    assert_eq!(h.phoff, header_num(text, "Start of program headers"));
    assert_eq!(h.shoff, header_num(text, "Start of section headers"));
    assert_eq!(u64::from(h.flags), header_num(text, "Flags"));
    assert_eq!(u64::from(h.ehsize), header_num(text, "Size of this header"));
    assert_eq!(
        u64::from(h.phentsize),
        header_num(text, "Size of program headers")
    );
    assert_eq!(u64::from(h.phnum), header_num(text, "Number of program headers"));
    assert_eq!(
        u64::from(h.shentsize),
        header_num(text, "Size of section headers")
    );
    assert_eq!(u64::from(h.shnum), header_num(text, "Number of section headers"));
    assert_eq!(
        u64::from(h.shstrndx),
        header_num(text, "Section header string table index")
    );
    assert_eq!(elf.class.ehdr_size(), h.ehsize as usize);
}

fn section_type(name: &str) -> u32 {
    match name {
        "NULL" => elf::SHT_NULL,
        "PROGBITS" => elf::SHT_PROGBITS,
        "SYMTAB" => elf::SHT_SYMTAB,
        "STRTAB" => elf::SHT_STRTAB,
        "RELA" => elf::SHT_RELA,
        "NOBITS" => elf::SHT_NOBITS,
        "REL" => elf::SHT_REL,
        "RISCV_ATTRIBUTES" => 0x7000_0003,
        "LLVM_ADDRSIG" => 0x6fff_4c03,
        t => panic!("section type {t}"),
    }
}

/// `sh_flags` from readelf's letters.
fn section_flags(letters: &str) -> u64 {
    letters
        .chars()
        .map(|c| match c {
            'W' => elf::SHF_WRITE,
            'A' => elf::SHF_ALLOC,
            'X' => elf::SHF_EXECINSTR,
            'M' => elf::SHF_MERGE,
            'S' => elf::SHF_STRINGS,
            'I' => elf::SHF_INFO_LINK,
            'G' => elf::SHF_GROUP,
            'T' => elf::SHF_TLS,
            'E' => 0x8000_0000, // SHF_EXCLUDE
            c => panic!("section flag {c}"),
        })
        .fold(0, |a, b| a | b)
}

/// `  [ 1] .text  PROGBITS  0000000080000000 001000 000158 00  AX  0   0  4`:
/// the null section has no name, and a section without flags no letters.
fn check_sections(elf: &Elf, text: &str) {
    let rows = block(text, "Section Headers:");
    let rows: Vec<&str> = rows
        .iter()
        .skip(1)
        .copied()
        .take_while(|l| l.starts_with("  ["))
        .collect();
    assert_eq!(rows.len(), elf.sections.len());
    for (s, row) in elf.sections.iter().zip(&rows) {
        let mut w: Vec<&str> = row[row.find(']').unwrap() + 1..].split_whitespace().collect();
        let name = if w[0].starts_with('.') { w.remove(0) } else { "" };
        let flags = if w.len() == 9 {
            section_flags(w.remove(5))
        } else {
            0
        };
        assert_eq!(w.len(), 8, "{row}");
        let want = Section {
            name: name.into(),
            ty: section_type(w[0]),
            addr: hex(w[1]),
            offset: hex(w[2]),
            size: hex(w[3]),
            entsize: hex(w[4]),
            flags,
            link: dec(w[5]) as u32,
            info: dec(w[6]) as u32,
            align: dec(w[7]),
        };
        assert_eq!(s, &want, "{row}");
    }
}

/// `  LOAD  0x001000 0x0000000080000000 0x0000000080000000 0x000158 0x000158 R E 0x1000`
fn check_segments(elf: &Elf, text: &str) {
    if elf.header.phnum == 0 {
        assert!(text.contains("There are no program headers in this file."));
        assert!(elf.segments.is_empty());
        return;
    }
    let rows = block(text, "Program Headers:");
    let rows = &rows[1..];
    assert_eq!(rows.len(), elf.segments.len());
    for (p, row) in elf.segments.iter().zip(rows) {
        let w: Vec<&str> = row.split_whitespace().collect();
        let ty = match w[0] {
            "PHDR" => elf::PT_PHDR,
            "LOAD" => elf::PT_LOAD,
            "GNU_STACK" => elf::PT_GNU_STACK,
            "ATTRIBUTES" => elf::PT_RISCV_ATTRIBUTES,
            t => panic!("segment type {t}"),
        };
        let flags = w[6..w.len() - 1].concat().chars().fold(0, |f, c| {
            f | match c {
                'R' => elf::PF_R,
                'W' => elf::PF_W,
                'E' => elf::PF_X,
                c => panic!("segment flag {c}"),
            }
        });
        let want = elf::ProgramHeader {
            ty,
            offset: hex(w[1]),
            vaddr: hex(w[2]),
            paddr: hex(w[3]),
            filesz: hex(w[4]),
            memsz: hex(w[5]),
            flags,
            align: hex(w[w.len() - 1]),
        };
        assert_eq!(p, &want, "{row}");
    }
}

/// `    41: 0000000080000000   140 FUNC    GLOBAL DEFAULT     1 sum`: readelf
/// names a section symbol after its section.
fn check_symbols(elf: &Elf, symbols: &[Symbol], text: &str) {
    let rows = block(text, "Symbol table '.symtab'");
    let rows = &rows[1..];
    assert_eq!(rows.len(), symbols.len());
    for (s, row) in symbols.iter().zip(rows) {
        let (w, name) = words(row, 7);
        let ty = match w[3] {
            "NOTYPE" => elf::STT_NOTYPE,
            "OBJECT" => elf::STT_OBJECT,
            "FUNC" => elf::STT_FUNC,
            "SECTION" => elf::STT_SECTION,
            "FILE" => elf::STT_FILE,
            t => panic!("symbol type {t}"),
        };
        let bind = match w[4] {
            "LOCAL" => elf::STB_LOCAL,
            "GLOBAL" => elf::STB_GLOBAL,
            "WEAK" => elf::STB_WEAK,
            b => panic!("binding {b}"),
        };
        assert_eq!(w[5], "DEFAULT", "{row}");
        let shndx = match w[6] {
            "UND" => elf::SHN_UNDEF,
            "ABS" => elf::SHN_ABS,
            "COM" => elf::SHN_COMMON,
            n => dec(n) as u16,
        };
        let size = if w[2].starts_with("0x") {
            hex(w[2])
        } else {
            dec(w[2])
        };
        let our_name = match s.section() {
            Some(i) if s.ty() == STT_SECTION && s.name.is_empty() => &elf.sections[i].name,
            _ => &s.name,
        };
        assert_eq!(
            (
                s.value,
                s.size,
                s.ty(),
                s.bind(),
                s.other,
                s.shndx,
                our_name.as_str()
            ),
            (hex(w[1]), size, ty, bind, 0, shndx, name),
            "{row}"
        );
    }
}

/// `0000000000000048  0000000500000002 R_RISCV_64   0000000000000000 .L0  + 0`,
/// or without a symbol `000000000000000c  0000000000000033 R_RISCV_RELAX   0`.
fn check_relocations(elf: &Elf, symbols: &[Symbol], text: &str) {
    let ours: Vec<&Section> = elf.sections.iter().filter(|s| s.ty == elf::SHT_RELA).collect();
    if ours.is_empty() {
        assert!(text.contains("There are no relocations in this file."));
        return;
    }
    // Every relocation section readelf lists is one of ours, and holds as
    // many entries as it says.
    let listed: Vec<(&str, usize)> = text
        .lines()
        .filter_map(|l| l.strip_prefix("Relocation section '"))
        .map(|l| {
            let (name, rest) = l.split_once('\'').unwrap();
            let n = rest
                .split(" contains ")
                .nth(1)
                .unwrap()
                .split(' ')
                .next()
                .unwrap();
            (name, dec(n) as usize)
        })
        .collect();
    let ours_listed: Vec<(&str, usize)> = ours
        .iter()
        .map(|rel| (rel.name.as_str(), elf.relocations(rel).unwrap().len()))
        .collect();
    assert_eq!(ours_listed, listed);
    let mut count = 0;
    for rel in ours {
        let rows = block(text, &format!("Relocation section '{}'", rel.name));
        let rows = &rows[1..];
        let relocs = elf.relocations(rel).unwrap();
        assert_eq!(relocs.len(), rows.len(), "{}", rel.name);
        for (r, row) in relocs.iter().zip(rows) {
            let (w, rest) = words(row, 3);
            let info = hex(w[1]);
            let (sym, ty) = match elf.class {
                Class::Elf32 => (info >> 8, info & 0xff),
                Class::Elf64 => (info >> 32, info & 0xffff_ffff),
            };
            assert_eq!(elf::reloc_name(ty as u32), Some(w[2]), "{row}");
            let rest = rest.trim_start();
            // Without a symbol, the addend alone; with one, its value, its
            // name, and "+ A" or "- A".
            let (name, addend) = if sym == 0 {
                let a = rest.trim();
                match a.strip_prefix('-') {
                    Some(a) => (None, -(hex(a) as i64)),
                    None => (None, hex(a) as i64),
                }
            } else {
                let (value, after) = rest.split_once(" ").unwrap();
                // ELF32 values are padded to the width of ELF64 ones.
                let after = after.trim_start();
                assert_eq!(symbols[sym as usize].value, hex(value), "{row}");
                let at = after.rfind(" + ").max(after.rfind(" - ")).unwrap();
                let a = hex(after[at + 3..].trim()) as i64;
                let a = if &after[at..at + 3] == " - " { -a } else { a };
                (Some(&after[..at]), a)
            };
            let want = Rela {
                offset: hex(w[0]),
                sym: sym as u32,
                ty: ty as u32,
                addend,
            };
            assert_eq!(r, &want, "{row}");
            if let Some(name) = name {
                let s = &symbols[sym as usize];
                let shown = match s.section() {
                    Some(i) if s.ty() == STT_SECTION => &elf.sections[i].name,
                    _ => &s.name,
                };
                assert_eq!(shown, name, "{row}");
            }
            count += 1;
        }
    }
    assert_eq!(count, listed.iter().map(|&(_, n)| n).sum::<usize>());
    assert!(count > 0);
}

#[test]
fn reads_what_llvm_readelf_prints() {
    for xlen in [32, 64] {
        for file in FILES {
            let path = dir(xlen).join(file);
            let bytes = std::fs::read(&path).unwrap();
            let text = std::fs::read_to_string(dir(xlen).join(format!("{file}.readelf"))).unwrap();
            let elf = Elf::parse(&bytes).unwrap();
            let symbols = elf.symbols().unwrap();
            check_header(&elf, &text, xlen);
            check_sections(&elf, &text);
            check_segments(&elf, &text);
            check_symbols(&elf, &symbols, &text);
            check_relocations(&elf, &symbols, &text);
        }
    }
}

/// One row as llvm-dwarfdump prints it: address, line, column, file, and the
/// flags is_stmt, prologue_end, epilogue_begin and end_sequence.
type Row = (u64, u32, u32, usize, bool, bool, bool, bool);

/// The tables in llvm-dwarfdump's text: for each, its files' paths and rows.
fn dwarfdump_tables(text: &str) -> Vec<(Vec<String>, Vec<Row>)> {
    let mut tables = Vec::new();
    for part in text.split("debug_line[").skip(1) {
        let quoted = |l: &str| l[l.find('"').unwrap() + 1..l.rfind('"').unwrap()].to_string();
        let dirs: Vec<String> = part
            .lines()
            .filter(|l| l.starts_with("include_directories["))
            .map(quoted)
            .collect();
        let mut files = Vec::new();
        let mut lines = part.lines();
        while let Some(l) = lines.next() {
            if l.starts_with("file_names[") {
                let name = quoted(lines.next().unwrap());
                let d = dec(lines.next().unwrap().trim().strip_prefix("dir_index: ").unwrap()) as usize;
                // DWARF 5: directory 0 is the compilation directory, and the
                // others are relative to it unless absolute.
                let dir = if d == 0 || dirs[d].starts_with('/') {
                    dirs[d].clone()
                } else {
                    elf::lines::join(&dirs[0], &dirs[d])
                };
                files.push(elf::lines::join(&dir, &name));
            }
        }
        let rows = part
            .lines()
            .skip_while(|l| !l.starts_with("------------------"))
            .skip(1)
            .take_while(|l| l.starts_with("0x"))
            .map(|l| {
                let w: Vec<&str> = l.split_whitespace().collect();
                let flag = |f: &str| w[7..].contains(&f);
                (
                    hex(w[0]),
                    dec(w[1]) as u32,
                    dec(w[2]) as u32,
                    dec(w[3]) as usize,
                    flag("is_stmt"),
                    flag("prologue_end"),
                    flag("epilogue_begin"),
                    flag("end_sequence"),
                )
            })
            .collect();
        tables.push((files, rows));
    }
    tables
}

#[test]
fn reads_the_line_tables_llvm_dwarfdump_prints() {
    for xlen in [32, 64] {
        for file in FILES {
            let bytes = std::fs::read(dir(xlen).join(file)).unwrap();
            let text = std::fs::read_to_string(dir(xlen).join(format!("{file}.lines"))).unwrap();
            let elf = Elf::parse(&bytes).unwrap();
            let tables = elf.line_tables().unwrap();
            let want = dwarfdump_tables(&text);
            assert_eq!(tables.len(), want.len(), "rv{xlen} {file}");
            for (t, (files, rows)) in tables.iter().zip(want) {
                assert_eq!(t.version, 5);
                assert_eq!(t.files, files, "rv{xlen} {file}");
                assert_eq!(t.files, ["/camelot/prog.c"]);
                let ours: Vec<Row> = t
                    .sequences
                    .iter()
                    .flat_map(|s| &s.rows)
                    .map(|r| {
                        let flags = (r.is_stmt, r.prologue_end, r.epilogue_begin, r.end_sequence);
                        (
                            r.addr, r.line, r.column, r.file, flags.0, flags.1, flags.2, flags.3,
                        )
                    })
                    .collect();
                assert_eq!(ours, rows, "rv{xlen} {file}");
                if xlen == 64 && file == "prog.elf" {
                    assert!(ours.iter().all(|r| r.0 >= 0x8000_0000), "linked addresses");
                }
            }
        }
    }
}

/// In an object each sequence says which section its addresses are offsets
/// in: the one that holds the function the sequence starts with (sum and
/// _start in .text, other in .text.other).
#[test]
fn an_objects_sequences_know_their_sections() {
    for xlen in [32, 64] {
        for file in ["prog.o", "prog-relax.o"] {
            let bytes = std::fs::read(dir(xlen).join(file)).unwrap();
            let elf = Elf::parse(&bytes).unwrap();
            let symbols = elf.symbols().unwrap();
            let mut starts = Vec::new();
            for t in elf.line_tables().unwrap() {
                for seq in &t.sequences {
                    let sec = seq.section.expect("a section");
                    let f = symbols
                        .iter()
                        .find(|s| {
                            s.ty() == STT_FUNC && s.section() == Some(sec) && s.value == seq.rows[0].addr
                        })
                        .unwrap_or_else(|| panic!("rv{xlen} {file}: no function at a sequence's start"));
                    starts.push((elf.sections[sec].name.as_str(), f.name.clone()));
                }
            }
            starts.sort();
            assert_eq!(
                starts,
                [
                    (".text", "_start".to_string()),
                    (".text", "sum".to_string()),
                    (".text.other", "other".to_string())
                ],
                "rv{xlen} {file}"
            );
        }
    }
}

/// The fixtures' flags are 0, but llvm-readelf names the flags it knows
/// after the number: a regenerated fixture with any must still parse.
#[test]
fn header_numbers_stop_at_a_comma() {
    let text = "  Flags:                             0x5, RVC, double-float ABI\n";
    assert_eq!(header_num(text, "Flags"), 5);
    let text = "  Start of program headers:          64 (bytes into file)\n";
    assert_eq!(header_num(text, "Start of program headers"), 64);
}

/// A DWARF tag or encoding as llvm-dwarfdump names it.
fn dw_name(name: &str) -> u64 {
    use elf::dies::dw::*;
    match name {
        "DW_TAG_compile_unit" => TAG_COMPILE_UNIT,
        "DW_TAG_subprogram" => TAG_SUBPROGRAM,
        "DW_TAG_formal_parameter" => TAG_FORMAL_PARAMETER,
        "DW_TAG_variable" => TAG_VARIABLE,
        "DW_TAG_lexical_block" => TAG_LEXICAL_BLOCK,
        "DW_TAG_array_type" => TAG_ARRAY_TYPE,
        "DW_TAG_subrange_type" => TAG_SUBRANGE_TYPE,
        "DW_TAG_base_type" => TAG_BASE_TYPE,
        "DW_TAG_const_type" => TAG_CONST_TYPE,
        "DW_TAG_pointer_type" => TAG_POINTER_TYPE,
        "DW_ATE_signed" => ATE_SIGNED,
        "DW_ATE_unsigned" => ATE_UNSIGNED,
        "DW_ATE_unsigned_char" => ATE_UNSIGNED_CHAR,
        _ => panic!("no {name} in the table"),
    }
}

/// `v` in LEB128, signed or not.
fn leb(mut v: i64, signed: bool) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let b = (v & 0x7f) as u8;
        v >>= 7;
        let done = if signed {
            (v == 0 && b & 0x40 == 0) || (v == -1 && b & 0x40 != 0)
        } else {
            v == 0
        };
        out.push(if done { b } else { b | 0x80 });
        if done {
            return out;
        }
    }
}

/// A location expression as llvm-dwarfdump prints it, encoded again: the
/// operations clang's -O0 code uses.
fn expression(text: &str) -> Vec<u8> {
    let (op, arg) = text.split_once(' ').unwrap_or((text, ""));
    if let Some(n) = op.strip_prefix("DW_OP_breg") {
        // DW_OP_breg2 X2+24
        let (_, off) = arg.split_once(['+', '-']).unwrap();
        let off = if arg.contains('-') {
            -(dec(off) as i64)
        } else {
            dec(off) as i64
        };
        let mut b = vec![0x70 + dec(n) as u8];
        b.extend(leb(off, true));
        b
    } else if let Some(n) = op.strip_prefix("DW_OP_reg") {
        vec![0x50 + dec(n) as u8]
    } else if op == "DW_OP_addrx" {
        let mut b = vec![0xa1];
        b.extend(leb(hex(arg) as i64, false));
        b
    } else {
        panic!("no {op} in the table")
    }
}

/// An entry of `.debug_info` as llvm-dwarfdump `--debug-info` prints it: its
/// offset, its depth (from its indentation), its tag and its attributes (the
/// name, and the text in the parentheses).
type DumpedDie = (u64, usize, u64, Vec<(String, String)>);

fn dwarfdump_entries(text: &str) -> Vec<DumpedDie> {
    let mut out: Vec<DumpedDie> = Vec::new();
    for line in text.lines() {
        if let Some((off, rest)) = line.split_once(':').filter(|(o, _)| o.starts_with("0x")) {
            let tag = rest.trim_start();
            if tag.starts_with("DW_TAG_") {
                let depth = (rest.len() - tag.len() - 1) / 2;
                out.push((hex(off), depth, dw_name(tag), Vec::new()));
            }
        } else if let Some((at, value)) = line.trim_start().split_once('\t') {
            if at.starts_with("DW_AT_") {
                let value = value.strip_prefix('(').unwrap_or(value);
                let value = value.strip_suffix(')').unwrap_or(value);
                out.last_mut()
                    .unwrap()
                    .3
                    .push((at.to_string(), value.to_string()));
            }
        }
    }
    out
}

/// `elf::dies` reads every entry llvm-dwarfdump prints, at its offset and
/// depth, with its tag, and the attributes a debugger reads: names, types,
/// lines, sizes, encodings, flags, locations, frame bases and code ranges.
/// The `.debug_addr` pool gives `table`'s address in the linked program.
#[test]
fn reads_the_entries_llvm_dwarfdump_prints() {
    use elf::dies::{dw, Die};
    for xlen in [32, 64] {
        for file in FILES {
            let bytes = std::fs::read(dir(xlen).join(file)).unwrap();
            let text = std::fs::read_to_string(dir(xlen).join(format!("{file}.info"))).unwrap();
            let elf = Elf::parse(&bytes).unwrap();
            let units = elf.debug_entries().unwrap();
            assert_eq!(units.len(), 1, "rv{xlen} {file}");
            let unit = &units[0];
            assert_eq!((unit.version, unit.addr_size), (5, xlen as u8 / 8));
            let want = dwarfdump_entries(&text);
            assert_eq!(unit.dies.len(), want.len(), "rv{xlen} {file}");
            let depth = |d: &Die| {
                let mut n = 0;
                let mut p = d.parent;
                while let Some(i) = p {
                    n += 1;
                    p = unit.dies[i].parent;
                }
                n
            };
            for (d, (off, dep, tag, attrs)) in unit.dies.iter().zip(&want) {
                let at = format!("rv{xlen} {file} {off:#x}");
                assert_eq!((d.offset, depth(d), d.tag), (*off, *dep, *tag), "{at}");
                for (name, value) in attrs {
                    let number = || {
                        if value.starts_with("0x") {
                            hex(value)
                        } else {
                            dec(value)
                        }
                    };
                    let flag = |a| assert!(d.flag(a), "{at} {name}");
                    match name.as_str() {
                        "DW_AT_name" => {
                            assert_eq!(d.name(), value.split('"').nth(1), "{at}");
                        }
                        "DW_AT_type" => {
                            let target = hex(value.split(' ').next().unwrap());
                            assert_eq!(d.reference(dw::AT_TYPE), Some(target), "{at}");
                        }
                        "DW_AT_decl_line" => assert_eq!(d.unsigned(dw::AT_DECL_LINE), Some(number()), "{at}"),
                        "DW_AT_byte_size" => assert_eq!(d.unsigned(dw::AT_BYTE_SIZE), Some(number()), "{at}"),
                        "DW_AT_count" => assert_eq!(d.unsigned(dw::AT_COUNT), Some(number()), "{at}"),
                        "DW_AT_encoding" => {
                            assert_eq!(d.unsigned(dw::AT_ENCODING), Some(dw_name(value)), "{at}");
                        }
                        "DW_AT_external" => flag(dw::AT_EXTERNAL),
                        "DW_AT_prototyped" => flag(dw::AT_PROTOTYPED),
                        "DW_AT_location" => {
                            assert_eq!(d.block(dw::AT_LOCATION), Some(&expression(value)[..]), "{at}");
                        }
                        "DW_AT_frame_base" => {
                            assert_eq!(d.block(dw::AT_FRAME_BASE), Some(&expression(value)[..]), "{at}");
                        }
                        // A function's or a block's low and high pc: its range.
                        "DW_AT_low_pc" if d.tag != dw::TAG_COMPILE_UNIT => {
                            assert_eq!(d.ranges.first().map(|r| r.0), Some(number()), "{at}");
                        }
                        "DW_AT_high_pc" => assert_eq!(d.ranges.first().map(|r| r.1), Some(number()), "{at}"),
                        _ => {}
                    }
                }
            }
            // The unit's ranges come from its range list; `table`'s address
            // from the address pool its DW_OP_addrx indexes.
            assert_eq!(unit.dies[0].ranges.len(), 3, "rv{xlen} {file}");
            let table = unit.dies.iter().find(|d| d.name() == Some("table")).unwrap();
            let index = table.block(dw::AT_LOCATION).unwrap()[1] as usize;
            assert!(index < unit.addr_pool.len(), "rv{xlen} {file}");
            if file == "prog.elf" {
                assert_eq!(
                    unit.addr_pool.get(index).copied(),
                    elf.symbol("table"),
                    "rv{xlen}"
                );
            }
        }
    }
}
