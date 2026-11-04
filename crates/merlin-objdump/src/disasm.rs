//! `-d`: disassembly of the executable sections, laid out like llvm-objdump:
//! a `<symbol>:` label wherever a symbol starts, one line per instruction,
//! branch targets resolved to `<symbol+offset>`, data in code (what the
//! assembler marks with a `$d` mapping symbol) as `.word`/`.short`/`.byte`,
//! and (with `-r`) relocations under the instruction they patch.
//!
//! Addresses are u64 for both widths. An ELF32 file is RV32 code: the branch
//! targets its operands show wrap at 2^32, as `excalibur` formats them. The
//! targets symbols are looked up for, and the `auipc` values behind `jalr`
//! targets, are 64-bit on both widths, as in LLVM.

use crate::{reloc_type, reloc_value, relocation_sections, Options};
use elf::{Elf, Error, Symbol};
use excalibur::{decode, Inst, Syntax};
use std::fmt::Write;

/// A symbol that can label code or name a branch target.
#[derive(Clone, Debug)]
struct Label {
    addr: u64,
    name: String,
    ty: u8,
}

/// RISC-V's mapping symbols: `$x` (code, `$x<isa>` naming the ISA it is
/// for, as LLVM's assembler writes them) and `$d` (data), with or without a
/// suffix (`$x.0`, as GNU's writes them).
fn mapping_kind(name: &str) -> Option<MapKind> {
    if name.starts_with("$x") {
        Some(MapKind::Code)
    } else if name.starts_with("$d") {
        Some(MapKind::Data)
    } else {
        None
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum MapKind {
    Data,
    Code,
}

/// Checks the ISA of a `$x<isa>` mapping symbol the way LLVM reads it
/// (RISCVISAInfo::parseNormalizedArchString): `rv32` or `rv64`, then
/// extensions separated by `_`, each a name and a version `<major>p<minor>`,
/// the first being the base `i` or `e` (`rv64i2p1_m2p0_zicsr2p0`). Only the
/// form is checked, not whether an extension exists. The error is LLVM's
/// message.
fn check_isa(isa: &str) -> Result<(), String> {
    if !isa
        .bytes()
        .all(|c| c.is_ascii_digit() || c.is_ascii_lowercase() || c == b'_')
    {
        return Err("string may only contain [a-z0-9_]".into());
    }
    let mut rest = match isa.strip_prefix("rv32").or_else(|| isa.strip_prefix("rv64")) {
        Some(r) if r.starts_with(['i', 'e']) => r,
        _ => return Err("arch string must begin with valid base ISA".into()),
    };
    let mut seen: Vec<&str> = Vec::new();
    while !rest.is_empty() {
        if let Some(r) = rest.strip_prefix('_') {
            if r.is_empty() || r.starts_with('_') {
                return Err("extension name missing after separator '_'".into());
            }
            rest = r;
        }
        let (ext, r) = rest.split_at(rest.find('_').unwrap_or(rest.len()));
        rest = r;
        let Some((prefix, minor)) = ext.rsplit_once('p').filter(|(_, m)| !m.is_empty()) else {
            return Err("extension lacks version in expected format".into());
        };
        if minor.parse::<u32>().is_err() || !minor.bytes().all(|c| c.is_ascii_digit()) {
            return Err("failed to parse minor version number".into());
        }
        let name = prefix.trim_end_matches(|c: char| c.is_ascii_digit());
        let major = &prefix[name.len()..];
        if major.is_empty() {
            return Err("extension lacks version in expected format".into());
        }
        if name.is_empty() {
            return Err("missing extension name".into());
        }
        if major.parse::<u32>().is_err() {
            return Err("failed to parse major version number".into());
        }
        let first = name.as_bytes()[0];
        if matches!(first, b'z' | b's' | b'x') && name.as_bytes().get(1).is_none_or(|c| c.is_ascii_digit()) {
            return Err(format!("'{}' must be followed by a letter", first as char));
        }
        if seen.contains(&name) {
            return Err(format!("duplicate extension '{name}'"));
        }
        seen.push(name);
    }
    Ok(())
}

/// llvm-objdump skips section and file symbols, mapping symbols (`$x`, `$d`)
/// and the assembler's label-difference temporaries (`.L0 `).
fn is_label(s: &Symbol) -> bool {
    !s.name.is_empty()
        && !matches!(s.ty(), elf::STT_SECTION | elf::STT_FILE)
        && mapping_kind(&s.name).is_none()
        && !s.name.starts_with(".L0 ")
}

struct Labels {
    /// Indexed by section; each sorted by (address, name, type), so the last
    /// of several symbols at one address is the one shown.
    per_section: Vec<Vec<Label>>,
    /// Absolute and undefined symbols, the last resort for naming a target.
    absolute: Vec<Label>,
    /// In a `.o`, targets are only looked up in the section being disassembled.
    relocatable: bool,
    /// (address, section index) of every section, sorted by address.
    section_addrs: Vec<(u64, usize)>,
}

impl Labels {
    fn new(elf: &Elf) -> Result<Labels, Error> {
        let mut per_section = vec![Vec::new(); elf.sections.len()];
        let mut absolute = Vec::new();
        for s in elf.symbols()?.iter().filter(|s| is_label(s)) {
            let label = Label {
                addr: s.value,
                name: s.name.clone(),
                ty: s.ty(),
            };
            match s.section().and_then(|i| per_section.get_mut(i)) {
                Some(list) => list.push(label),
                None => absolute.push(label),
            }
        }
        let key = |l: &Label| (l.addr, l.name.clone(), l.ty);
        for list in &mut per_section {
            list.sort_by_key(key);
        }
        absolute.sort_by_key(key);
        let mut section_addrs: Vec<(u64, usize)> = elf
            .sections
            .iter()
            .enumerate()
            .map(|(i, s)| (s.addr, i))
            .collect();
        section_addrs.sort_by_key(|&(a, _)| a);
        Ok(Labels {
            per_section,
            absolute,
            relocatable: elf.header.ty == elf::ET_REL,
            section_addrs,
        })
    }

    /// The symbol to name `target` by, while disassembling section `current`:
    /// the last symbol at or before it in the section that contains it (in an
    /// executable, the section with the highest start address not above it),
    /// falling back to absolute symbols.
    fn lookup(&self, current: usize, target: u64) -> Option<&Label> {
        let mut candidates: Vec<&[Label]> = Vec::new();
        if self.relocatable {
            candidates.push(&self.per_section[current]);
        } else {
            let n = self.section_addrs.partition_point(|&(a, _)| a <= target);
            let mut start = None;
            for &(addr, i) in self.section_addrs[..n].iter().rev() {
                if *start.get_or_insert(addr) != addr {
                    break;
                }
                candidates.push(&self.per_section[i]);
            }
        }
        candidates.push(&self.absolute);
        candidates.into_iter().find_map(|list| {
            let n = list.partition_point(|l| l.addr <= target);
            n.checked_sub(1).map(|i| &list[i])
        })
    }
}

/// Where a branch or `jal` at `pc` goes, as llvm-objdump names it: in 64
/// bits on both widths (LLVM's MCInstrAnalysis), so on RV32 a target below
/// 0 is not the 32-bit address the operand shows (`j 0xffffff04`) but
/// 2^64 less a little (`<f+0xffffffffffffff04>`).
fn branch_target(inst: &Inst, pc: u64) -> Option<u64> {
    match *inst {
        Inst::Jal { imm, .. } | Inst::Branch { imm, .. } => Some(pc.wrapping_add(imm as i64 as u64)),
        _ => None,
    }
}

/// Register values known from `auipc`, used to resolve `jalr` targets the
/// way llvm-objdump does (`auipc ra, 0x0` / `jalr ra` → `<puts>`). Like
/// LLVM's, the values are 64-bit on both widths.
struct RegState([Option<u64>; 32]);

impl RegState {
    fn new() -> RegState {
        let mut r = RegState([None; 32]);
        r.reset();
        r
    }

    fn reset(&mut self) {
        self.0 = [None; 32];
        self.0[0] = Some(0);
    }

    fn set(&mut self, rd: u8, v: Option<u64>) {
        if rd != 0 {
            self.0[rd as usize & 31] = v;
        }
    }

    /// Where a `jalr` goes, if its base register is known.
    fn jalr_target(&self, inst: &Inst) -> Option<u64> {
        match *inst {
            Inst::Jalr { rs1, imm, .. } => {
                self.0[rs1 as usize & 31].map(|v| v.wrapping_add(imm as i64 as u64))
            }
            _ => None,
        }
    }

    /// Updates the state after `inst` (decoded from `w`) at `pc`. Control
    /// flow and anything llvm cannot decode forget everything; other writes
    /// forget `rd`.
    fn update(&mut self, inst: &Inst, w: u32, pc: u64) {
        if llvm_rejects_fence(w) {
            self.reset();
            return;
        }
        match *inst {
            Inst::Branch { .. }
            | Inst::Jal { .. }
            | Inst::Jalr { .. }
            | Inst::Mret
            | Inst::Sret
            | Inst::Illegal(_)
            | Inst::XcalDbg { .. }
            | Inst::XcalHalt { .. } => self.reset(),
            Inst::Auipc { rd, imm } => self.set(rd, Some(pc.wrapping_add(imm as i64 as u64))),
            Inst::Lui { rd, .. }
            | Inst::Load { rd, .. }
            | Inst::OpImm { rd, .. }
            | Inst::OpImm32 { rd, .. }
            | Inst::Op { rd, .. }
            | Inst::Op32 { rd, .. }
            | Inst::Mul { rd, .. }
            | Inst::Mul32 { rd, .. }
            | Inst::Lr { rd, .. }
            | Inst::Sc { rd, .. }
            | Inst::Amo { rd, .. }
            | Inst::Csr { rd, .. } => self.set(rd, None),
            _ => {}
        }
    }
}

/// True for the reserved MISC-MEM encodings that excalibur decodes as a
/// fence (Grail executes them as one) but llvm-objdump prints as `<unknown>`:
/// `fence` with non-zero fm, rs1 or rd (other than `fence.tso`), and
/// `fence.i` with any immediate, rs1 or rd bits set.
fn llvm_rejects_fence(w: u32) -> bool {
    const RS1_RD: u32 = 0x000f_8f80;
    if w & 0x7f != 0x0f {
        return false;
    }
    match (w >> 12) & 7 {
        0 => w & RS1_RD != 0 || (w >> 28 != 0 && w >> 20 != 0x833),
        1 => w & (0xfff0_0000 | RS1_RD) != 0,
        _ => false,
    }
}

/// A relocation ready to print under an instruction.
struct InlineReloc {
    /// Offset from the start of the section.
    offset: u64,
    text: String,
}

/// How many leading zero bytes of `bytes` llvm-objdump folds into `...`: none
/// unless there are at least 8, and then a multiple of 4, so an instruction
/// starting with a zero byte is not cut.
fn skippable_zeros(bytes: &[u8]) -> usize {
    let n = bytes.iter().take_while(|&&b| b == 0).count();
    if n < 8 {
        0
    } else {
        n & !3
    }
}

/// The spaces llvm-objdump puts after the address and the raw bytes of a
/// line, `column` characters long so far, so that the tab that starts the
/// instruction lands on its column (LLVM's AlignToInstStartColumn): the
/// mnemonic starts at column 24 with the raw bytes shown, 16 without, or at
/// the next tab stop when the line is already longer.
fn inst_start_padding(column: usize, raw_insn: bool) -> usize {
    let tab_stop = if raw_insn { 24 } else { 16 };
    if column < tab_stop - 1 {
        tab_stop - 1 - column
    } else {
        7 - column % 8
    }
}

/// A line table sequence for `-l`: the section it is in (in an object), its
/// start and end, and its rows (address, path, line).
type Sequence = (Option<usize>, u64, u64, Vec<(u64, String, u32)>);

/// `-l`: where each address comes from, as llvm-objdump prints it. Before an
/// instruction, `; name():` when the function it is in is another than the
/// last one's, and `; file:line` when its line (the line table row at or
/// before it) is another, or the function is. The function is the symbol
/// LLVM's symbolizer takes: the last function, object or untyped symbol at or
/// before the address in its section (not a mapping symbol like `$x` or a
/// fake label `.L0 `), if the
/// address is inside it (a symbol of size 0 runs to the next one). Unlike
/// llvm-objdump, the line table is read without a compile unit.
struct SourceLines {
    sequences: Vec<Sequence>,
    /// The symbols a function name may come from, sorted: (section, address,
    /// size, name).
    functions: Vec<(usize, u64, u64, String)>,
    relocatable: bool,
    /// What the last instruction's lines said: its function and its line.
    last: (Option<String>, Option<(String, u32)>),
}

impl SourceLines {
    fn new(elf: &Elf) -> Result<SourceLines, Error> {
        let mut sequences = Vec::new();
        for t in elf.line_tables()? {
            for seq in &t.sequences {
                let (Some(first), Some(end)) = (seq.rows.first(), seq.rows.last()) else {
                    continue;
                };
                let rows = seq
                    .rows
                    .iter()
                    .filter(|r| !r.end_sequence)
                    .map(|r| (r.addr, t.path(r.file).to_string(), r.line))
                    .collect();
                sequences.push((seq.section, first.addr, end.addr, rows));
            }
        }
        let mut functions: Vec<(usize, u64, u64, String)> = elf
            .symbols()?
            .into_iter()
            .filter(|s| matches!(s.ty(), elf::STT_NOTYPE | elf::STT_FUNC | elf::STT_OBJECT))
            // Not RISC-V's mapping symbols (`$x`, `$d`) and fake labels (`.L0 `).
            .filter(|s| !s.name.is_empty() && !s.name.starts_with('$') && !s.name.starts_with(".L0 "))
            .filter_map(|s| {
                let sec = s.section()?;
                elf.sections
                    .get(sec)?
                    .is_alloc()
                    .then_some((sec, s.value, s.size, s.name))
            })
            .collect();
        // By address, then size: of several at one address the biggest wins.
        functions.sort_by_key(|f| (f.0, f.1, f.2));
        Ok(SourceLines {
            sequences,
            functions,
            relocatable: elf.header.ty == elf::ET_REL,
            last: (None, None),
        })
    }

    /// The line table row for `addr` in section `sec`: path and line.
    fn row(&self, sec: usize, addr: u64) -> Option<(String, u32)> {
        let (_, _, _, rows) = self.sequences.iter().find(|(s, start, end, _)| {
            (!self.relocatable || *s == Some(sec)) && (*start..*end).contains(&addr)
        })?;
        let i = rows.partition_point(|r| r.0 <= addr).checked_sub(1)?;
        Some((rows[i].1.clone(), rows[i].2))
    }

    fn print(&mut self, out: &mut String, sec: usize, addr: u64) {
        let i = self.functions.partition_point(|f| (f.0, f.1) <= (sec, addr));
        let function = i
            .checked_sub(1)
            .map(|i| &self.functions[i])
            .filter(|(s, start, size, _)| *s == sec && (*size == 0 || addr - start < *size))
            .map(|f| f.3.clone());
        let line = self.row(sec, addr).filter(|(_, l)| *l != 0);
        let new_function = function.is_some() && function != self.last.0;
        if let (true, Some(f)) = (new_function, &function) {
            let _ = writeln!(out, "; {f}():");
        }
        if let Some((path, n)) = &line {
            if new_function || self.last.1.as_ref() != Some(&(path.clone(), *n)) {
                let _ = writeln!(out, "; {path}:{n}");
            }
        }
        self.last = (function, line);
    }
}

/// One line of data in code (`$d`), as llvm-objdump prints it (LLVM's
/// dumpELFData): a `.word` while 4 bytes are left before `end`, else a
/// `.short`, else a `.byte`, the bytes shown one by one in file order (with
/// or without `--no-show-raw-insn`) and the value little-endian. Returns
/// its size.
fn data_line(out: &mut String, pc: u64, bytes: &[u8], raw_insn: bool) -> usize {
    let start = out.len();
    let _ = write!(out, "{pc:8x}: ");
    let size = match bytes.len() {
        4.. => 4,
        2 | 3 => 2,
        _ => 1,
    };
    let bytes = &bytes[..size];
    let hex: Vec<String> = bytes.iter().map(|b| format!("{b:02x}")).collect();
    out.push_str(&hex.join(" "));
    let pad = inst_start_padding(out.len() - start, raw_insn);
    out.push_str(&" ".repeat(pad));
    let _ = match *bytes {
        [a, b, c, d] => write!(out, "\t.word\t{:#010x}", u32::from_le_bytes([a, b, c, d])),
        [a, b] => write!(out, "\t.short\t{:#06x}", u16::from_le_bytes([a, b])),
        [a] => write!(out, "\t.byte\t{a:#04x}"),
        _ => unreachable!(),
    };
    out.push('\n');
    size
}

pub(crate) fn disassemble(
    out: &mut String,
    warnings: &mut Vec<String>,
    elf: &Elf,
    opts: &Options,
) -> Result<(), Error> {
    let xlen = crate::xlen(elf);
    let syntax = Syntax {
        aliases: opts.aliases,
        hex: true,
        xlen,
        zicbop: true,
    };
    let width = crate::addr_width(elf);
    let mut source = if opts.line_numbers {
        Some(SourceLines::new(elf)?)
    } else {
        None
    };
    let mut labels = Labels::new(elf)?;
    let reloc_sections = if opts.relocs {
        relocation_sections(elf)?
    } else {
        Vec::new()
    };
    let symbols = elf.symbols()?;

    for (idx, sec) in elf.sections.iter().enumerate() {
        if !sec.is_exec() || sec.is_nobits() || sec.size == 0 {
            continue;
        }
        let data = elf.section_data(sec)?;
        // u64, so an ELF32 section ending at 2^32 does not wrap (nor can an
        // ELF64 one end past 2^64).
        let sec_start = sec.addr;
        let end = sec_start.saturating_add(sec.size);
        let _ = writeln!(out, "\nDisassembly of section {}:", sec.name);

        // A section with no symbol at its start gets one named after it. A
        // linker script can define symbols below the section start, so the
        // label goes where it keeps the list sorted, not necessarily first.
        let syms = &mut labels.per_section[idx];
        if !syms.iter().any(|s| s.addr == sec_start) {
            let label = Label {
                addr: sec_start,
                name: sec.name.clone(),
                ty: elf::STT_FUNC,
            };
            let at = syms.partition_point(|s| s.addr < sec_start);
            syms.insert(at, label);
        }
        let syms = syms.clone();

        // This section's mapping symbols, sorted (of two at one address,
        // the code one counts, as LLVM sorts them).
        let mut mapping: Vec<(u64, MapKind)> = symbols
            .iter()
            .filter(|s| s.section() == Some(idx))
            .filter_map(|s| mapping_kind(&s.name).map(|k| (s.value, k)))
            .collect();
        mapping.sort();
        // An ISA in a code mapping symbol that LLVM cannot parse gets a
        // warning, once per name and file. Nothing else depends on the ISA:
        // LLVM 23 decodes the same instructions whatever it names.
        let mut isa_symbols: Vec<&Symbol> = symbols
            .iter()
            .filter(|s| s.section() == Some(idx) && s.name.len() > 2 && s.name.starts_with("$x"))
            .collect();
        isa_symbols.sort_by_key(|s| s.value);
        for s in isa_symbols {
            if let Err(e) = check_isa(&s.name[2..]) {
                let w = format!(
                    "could not parse ISA mapping symbol '{}': {e}; falling back to default disassembler",
                    s.name
                );
                if !warnings.contains(&w) {
                    warnings.push(w);
                }
            }
        }
        // What the bytes at `pc` are: those before the first mapping symbol
        // are code.
        let kind_at = |pc: u64| {
            let n = mapping.partition_point(|&(a, _)| a <= pc);
            n.checked_sub(1).map_or(MapKind::Code, |i| mapping[i].1)
        };
        // Where the next mapping symbol after `pc` is.
        let next_mapping = |pc: u64| mapping.iter().map(|&(a, _)| a).find(|&a| a > pc);

        let mut relocs: Vec<InlineReloc> = Vec::new();
        for rs in reloc_sections.iter().filter(|rs| rs.target == idx) {
            for r in &rs.relocs {
                // In an executable, relocation offsets are addresses.
                let offset = if labels.relocatable {
                    r.offset
                } else {
                    r.offset.wrapping_sub(sec_start)
                };
                // ELF64 indents with one tab less, the address being wider.
                let indent = if width == 16 { "\t\t" } else { "\t\t\t" };
                let text = format!(
                    "{indent}{:0width$x}:  {}\t{}",
                    sec_start.wrapping_add(offset),
                    reloc_type(r.ty),
                    reloc_value(elf, &rs.symbols, r)
                );
                relocs.push(InlineReloc { offset, text });
            }
        }
        relocs.sort_by_key(|r| r.offset);
        let mut relocs = relocs.into_iter().peekable();

        // One region per distinct symbol address, labelled with the last symbol there.
        let mut i = 0;
        while i < syms.len() {
            let start = syms[i].addr;
            let mut j = i;
            while j < syms.len() && syms[j].addr == start {
                j += 1;
            }
            let label = &syms[j - 1];
            let region_end = syms.get(j).map_or(end, |s| s.addr.min(end));
            i = j;
            if start < sec_start || start >= region_end {
                continue;
            }
            let _ = writeln!(out, "\n{start:0width$x} <{}>:", label.name);

            let mut regs = RegState::new();
            // The mapping symbols up to here are behind us.
            let mut passed = mapping.partition_point(|&(a, _)| a <= start);
            let mut pc = start;
            while pc < region_end {
                // Like llvm-objdump, a mapping symbol forgets the `auipc`
                // values: the disassembler starts anew at each one.
                let reached = mapping.partition_point(|&(a, _)| a <= pc);
                if reached > passed {
                    regs.reset();
                    passed = reached;
                }
                let off = (pc - sec_start) as usize;
                // Like llvm-objdump (without -z), a run of zero bytes inside
                // this region and before the next shown relocation is
                // printed as `...` instead of one line per word, in code
                // and in data alike.
                let limit = relocs
                    .peek()
                    .map_or(region_end, |r| region_end.min(sec_start.wrapping_add(r.offset)));
                let span = data.get(off..(limit.max(pc) - sec_start) as usize).unwrap_or(&[]);
                let skip = skippable_zeros(span);
                if skip > 0 {
                    out.push_str("\t\t...\n");
                    pc += skip as u64;
                    continue;
                }

                let size = if kind_at(pc) == MapKind::Data {
                    // Data runs to the end of the region or the next
                    // mapping symbol, whichever comes first.
                    let data_end = next_mapping(pc).map_or(region_end, |a| a.min(region_end));
                    let Some(bytes) = data.get(off..(data_end - sec_start) as usize) else {
                        break;
                    };
                    // (With no source lines: LLVM prints them for instructions.)
                    data_line(out, pc, bytes, opts.raw_insn)
                } else {
                    // Like llvm-objdump, a word is decoded whenever the
                    // section has 4 bytes left, even if it runs past the
                    // next label.
                    let size = if end - pc >= 4 { 4 } else { 1 };
                    let (raw, w) = match data.get(off..off + size) {
                        Some(&[a, b, c, d]) => {
                            let w = u32::from_le_bytes([a, b, c, d]);
                            (format!("{w:08x}"), w)
                        }
                        Some(&[b]) => (format!("{b:02x}"), b as u32),
                        _ => break,
                    };
                    let inst = if size == 4 {
                        decode(w, xlen)
                    } else {
                        Inst::Illegal(w)
                    };
                    if let Some(s) = source.as_mut() {
                        s.print(out, idx, pc);
                    }
                    let d = inst.format(Some(pc), syntax);
                    let (mnemonic, operands) = match size {
                        1 => (".byte", format!("{w:#04x}")),
                        _ => (d.mnemonic, d.operands),
                    };
                    let line_start = out.len();
                    let _ = write!(out, "{pc:8x}:");
                    if opts.raw_insn {
                        let _ = write!(out, " {raw}");
                    }
                    let pad = inst_start_padding(out.len() - line_start, opts.raw_insn);
                    out.push_str(&" ".repeat(pad));
                    let _ = write!(out, "\t{mnemonic}");
                    if !operands.is_empty() {
                        let _ = write!(out, "\t{operands}");
                    }
                    if let Some((l, disp)) = branch_target(&inst, pc)
                        .or_else(|| regs.jalr_target(&inst))
                        .and_then(|t| labels.lookup(idx, t).map(|l| (l, t - l.addr)))
                    {
                        if disp == 0 {
                            let _ = write!(out, " <{}>", l.name);
                        } else {
                            let _ = write!(out, " <{}+{disp:#x}>", l.name);
                        }
                    }
                    out.push('\n');
                    regs.update(&inst, w, pc);
                    size
                };

                let next = (off + size) as u64;
                while let Some(r) = relocs.next_if(|r| r.offset < next) {
                    let _ = writeln!(out, "{}", r.text);
                }
                pc += size as u64;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{check_isa, inst_start_padding};

    /// What llvm-objdump 23 says of these ISAs in `$x<isa>` (made with
    /// llvm-objcopy --redefine-sym on a clang object).
    #[test]
    fn isa_mapping_symbols_like_llvm() {
        for ok in [
            "rv32i2p1",
            "rv64i2p1_m2p0_a2p1_zicsr2p0_zifencei2p0_zmmul1p0_zaamo1p0_zalrsc1p0",
            "rv32iq2p0",
            "rv64e2p0",
        ] {
            assert_eq!(check_isa(ok), Ok(()), "{ok}");
        }
        for (isa, err) in [
            (".0", "string may only contain [a-z0-9_]"),
            ("RV32i", "string may only contain [a-z0-9_]"),
            ("foo", "arch string must begin with valid base ISA"),
            ("rv32", "arch string must begin with valid base ISA"),
            ("rv64", "arch string must begin with valid base ISA"),
            ("rv32x", "arch string must begin with valid base ISA"),
            ("rv64g", "arch string must begin with valid base ISA"),
            ("rv32_", "arch string must begin with valid base ISA"),
            ("rv32i", "extension lacks version in expected format"),
            ("rv32i2p1_m2p0_q", "extension lacks version in expected format"),
            ("rv32i_zfoo", "extension lacks version in expected format"),
            ("rv32i2p1_mp0", "extension lacks version in expected format"),
            ("rv32i2p1__m2p0", "extension name missing after separator '_'"),
            ("rv32i2p1_", "extension name missing after separator '_'"),
            ("rv32i2p1_m2px", "failed to parse minor version number"),
            ("rv32i2p1_m2p99999999999", "failed to parse minor version number"),
            ("rv32i2p1_m99999999999p0", "failed to parse major version number"),
            ("rv32i2p1_2p0", "missing extension name"),
            ("rv32i2p1_z2p0", "'z' must be followed by a letter"),
            ("rv32i2p1_s1p0", "'s' must be followed by a letter"),
            ("rv32i2p1_x2p0", "'x' must be followed by a letter"),
            ("rv32i2p1_m2p0_m2p0", "duplicate extension 'm'"),
        ] {
            assert_eq!(check_isa(isa), Err(err.to_string()), "{isa}");
        }
    }

    /// LLVM's AlignToInstStartColumn: the instruction's tab lands at column
    /// 24 (16 without raw bytes), or the next tab stop on a longer line.
    #[test]
    fn instruction_column() {
        // "       0: 00000013" is 18 columns.
        assert_eq!(inst_start_padding(18, true), 5);
        // A 9-digit address: one column more, one space less.
        assert_eq!(inst_start_padding(19, true), 4);
        // 16 digits: past column 23, so to the next tab stop.
        assert_eq!(inst_start_padding(26, true), 5);
        assert_eq!(inst_start_padding(9, false), 6);
        assert_eq!(inst_start_padding(17, false), 6);
        // "      21: 00 01" (a .short with --no-show-raw-insn): none.
        assert_eq!(inst_start_padding(15, false), 0);
    }
}
