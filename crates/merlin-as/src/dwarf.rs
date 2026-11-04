//! DWARF line tables: what `.file` and `.loc` become.
//!
//! A compiler says which source line each instruction comes from with two
//! directives (the ones clang writes under `-g`):
//!
//! ```text
//!     .file 0 "kernel" "main.c"           # file 0: the compilation directory and main file
//!     .file 1 "kernel" "include/types.h"  # other files, numbered from 1
//!     .loc  0 42 5 prologue_end           # the next instruction: main.c, line 42, column 5
//! ```
//!
//! The assembler records a *row* (an address and a [`Loc`]) for the first
//! instruction after each `.loc`, and at the end writes them as a DWARF 5
//! `.debug_line` section: a header with the directory and file tables (their
//! names go in `.debug_line_str`), then a *line number program*, a byte code
//! that a debugger runs to rebuild the table of rows. Each code section gets
//! its own *sequence*: it starts with the section's address (a relocation,
//! since the linker decides it), moves through the rows with small opcodes,
//! and ends just past the section's last byte.
//!
//! The program is the one LLVM's assembler writes for the same input
//! (`MCDwarfLineAddr::encode` and `MCDwarfLineTable::emitOne`), byte for byte,
//! so that clang's output is a reference: `tests/debug_line.rs` compares the
//! two on a committed corpus. The rules that make it so:
//!
//! * Registers start at file 1, line 1, column 0, `is_stmt` set, ISA 0.
//! * Before each row, only what changed is set: `DW_LNS_set_file`, then
//!   `set_column`, `DW_LNE_set_discriminator` (whenever the row has one),
//!   `set_isa`, `negate_stmt`, then the one-row flags `basic_block`,
//!   `prologue_end` and `epilogue_begin`.
//! * The row itself: the first of a sequence sets the address
//!   (`DW_LNE_set_address`) and then adds the line with no address advance;
//!   the others advance both at once, with a *special opcode* if the two
//!   deltas fit in one, else `DW_LNS_advance_line` and/or
//!   `DW_LNS_const_add_pc` / `DW_LNS_advance_pc` (see [`advance`]).
//! * `is_stmt` is sticky from one `.loc` to the next (as in GNU as); the other
//!   flags, the ISA and the discriminator belong to one `.loc`.
//!
//! The two widths write the same table but for the size of an address: the
//! header's `address_size` (4 on RV32, 8 on RV64) and the operand of
//! `DW_LNE_set_address`, a 4-byte `R_RISCV_32` or an 8-byte `R_RISCV_64`
//! field. Everything else is DWARF32 in both, as clang writes it: the unit and
//! header lengths and the `.debug_line_str` offsets stay 4 bytes (and
//! `R_RISCV_32`), since DWARF32 is about the size of the debug sections, not
//! of the program's addresses.

/// `DW_LNS_*`: the standard opcodes.
const COPY: u8 = 1;
const ADVANCE_PC: u8 = 2;
const ADVANCE_LINE: u8 = 3;
const SET_FILE: u8 = 4;
const SET_COLUMN: u8 = 5;
const NEGATE_STMT: u8 = 6;
const SET_BASIC_BLOCK: u8 = 7;
const CONST_ADD_PC: u8 = 8;
const SET_PROLOGUE_END: u8 = 10;
const SET_EPILOGUE_BEGIN: u8 = 11;
const SET_ISA: u8 = 12;
/// `DW_LNE_*`: extended opcodes, after a 0 byte and their length.
const END_SEQUENCE: u8 = 1;
const SET_ADDRESS: u8 = 2;
const SET_DISCRIMINATOR: u8 = 4;

/// The header's parameters, LLVM's: special opcodes from 13 up, each adding
/// a line delta in -5..=8 and an address delta of 0..=17 bytes.
const LINE_BASE: i64 = -5;
const LINE_RANGE: i64 = 14;
const OPCODE_BASE: i64 = 13;
/// How many operands each standard opcode 1..=12 takes.
const STANDARD_LENGTHS: [u8; 12] = [0, 1, 1, 1, 1, 0, 0, 0, 1, 0, 0, 1];
/// The address advance of the special opcode 255, which `DW_LNS_const_add_pc` adds.
const MAX_SPECIAL_ADDR: i64 = (255 - OPCODE_BASE) / LINE_RANGE;

/// `DW_LNCT_*` and `DW_FORM_*`, for the directory and file tables' formats.
const LNCT_PATH: u8 = 1;
const LNCT_DIRECTORY_INDEX: u8 = 2;
const LNCT_MD5: u8 = 5;
const FORM_LINE_STRP: u8 = 0x1f;
const FORM_UDATA: u8 = 0x0f;
const FORM_DATA16: u8 = 0x1e;

/// `.loc` flags.
pub const IS_STMT: u8 = 1;
pub const BASIC_BLOCK: u8 = 2;
pub const PROLOGUE_END: u8 = 4;
pub const EPILOGUE_BEGIN: u8 = 8;

/// A `.file`: a directory (empty for the compilation directory) and a name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct File {
    pub dir: String,
    pub name: String,
    pub md5: Option<[u8; 16]>,
}

/// Where a row says its instruction comes from: a `.loc`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Loc {
    pub file: u32,
    pub line: u32,
    pub column: u32,
    /// [`IS_STMT`], [`BASIC_BLOCK`], [`PROLOGUE_END`], [`EPILOGUE_BEGIN`].
    pub flags: u8,
    pub isa: u32,
    pub discriminator: u32,
}

/// The rows of one code section, by offset in it, and the section's size.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sequence {
    pub section: usize,
    pub rows: Vec<(u32, Loc)>,
    pub end: u32,
}

/// What a field of the output refers to: the linker fills it in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    /// This offset in `.debug_line_str`: a 4-byte field (DWARF32's
    /// `DW_FORM_line_strp`) on both widths.
    LineStr(u32),
    /// This offset in a code section: an address, as wide as the target's
    /// (the table's `address_size`).
    Section(usize, u32),
}

/// An encoded line table: `.debug_line`'s bytes, the strings it adds to
/// `.debug_line_str`, and the fields to relocate (offsets in `.debug_line`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Table {
    pub line: Vec<u8>,
    pub line_str: Vec<u8>,
    pub fixups: Vec<(u32, Target)>,
}

pub fn uleb(out: &mut Vec<u8>, mut v: u64) {
    loop {
        let byte = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

pub fn sleb(out: &mut Vec<u8>, mut v: i64) {
    loop {
        let byte = (v & 0x7f) as u8;
        v >>= 7;
        let done = (v == 0 && byte & 0x40 == 0) || (v == -1 && byte & 0x40 != 0);
        if done {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

/// The directory and file tables as LLVM builds them from the `.file`s:
/// directory 0 is the compilation directory and file 0 the main file, as
/// written in `.file 0`. Another file whose directory is the compilation
/// directory, or that has none, is split at its last `/` (`include/types.h`
/// is `types.h` in directory `include`); then its directory is 0 if empty,
/// else found or added in the order the files name them. Returns the
/// directories and, per file, its name and directory index.
pub fn file_tables(comp_dir: &str, root: &File, files: &[File]) -> (Vec<String>, Vec<(String, u64)>) {
    let mut dirs = vec![comp_dir.to_string()];
    let mut entries = vec![(root.name.clone(), 0)];
    for f in files {
        let (mut dir, mut name) = (f.dir.as_str(), f.name.as_str());
        if dir == comp_dir {
            dir = "";
        }
        if dir.is_empty() {
            if let Some(i) = name.rfind('/').filter(|&i| i > 0 && i + 1 < name.len()) {
                (dir, name) = (&name[..i], &name[i + 1..]);
            }
        }
        let index = if dir.is_empty() {
            0
        } else {
            match dirs[1..].iter().position(|d| d == dir) {
                Some(i) => i + 1,
                None => {
                    dirs.push(dir.to_string());
                    dirs.len() - 1
                }
            }
        };
        entries.push((name.to_string(), index as u64));
    }
    (dirs, entries)
}

/// Strings for `.debug_line_str`, each once, in the order first added.
struct Strings<'a> {
    base: u32,
    bytes: Vec<u8>,
    seen: Vec<(&'a str, u32)>,
}

impl<'a> Strings<'a> {
    fn add(&mut self, s: &'a str) -> u32 {
        if let Some(&(_, off)) = self.seen.iter().find(|(t, _)| *t == s) {
            return off;
        }
        let off = self.base + self.bytes.len() as u32;
        self.bytes.extend_from_slice(s.as_bytes());
        self.bytes.push(0);
        self.seen.push((s, off));
        off
    }
}

/// Encodes the line table. `base` is where it goes in `.debug_line` (after
/// whatever the source put there) and `str_base` where its strings go in
/// `.debug_line_str`: fixups count from the sections' starts.
/// `address_size` is the bytes in a target address, 4 on RV32 and 8 on RV64.
pub fn encode(
    comp_dir: &str,
    root: &File,
    files: &[File],
    sequences: &[Sequence],
    base: u32,
    str_base: u32,
    address_size: u8,
) -> Table {
    let (dirs, entries) = file_tables(comp_dir, root, files);
    let md5 = std::iter::once(root).chain(files).all(|f| f.md5.is_some());
    let mut strings = Strings {
        base: str_base,
        bytes: Vec::new(),
        seen: Vec::new(),
    };
    let mut fixups = Vec::new();

    let mut out = vec![0; 4]; // unit_length, filled in last
    out.extend_from_slice(&5u16.to_le_bytes()); // version
    out.push(address_size);
    out.push(0); // segment_selector_size
    let header_length_at = out.len();
    out.extend_from_slice(&[0; 4]);
    out.push(1); // minimum_instruction_length
    out.push(1); // maximum_operations_per_instruction
    out.push(1); // default_is_stmt
    out.push(LINE_BASE as u8);
    out.push(LINE_RANGE as u8);
    out.push(OPCODE_BASE as u8);
    out.extend_from_slice(&STANDARD_LENGTHS);

    // The directories, then the files, each name a `.debug_line_str` offset.
    out.extend_from_slice(&[1, LNCT_PATH, FORM_LINE_STRP]);
    uleb(&mut out, dirs.len() as u64);
    let strp = |out: &mut Vec<u8>, fixups: &mut Vec<(u32, Target)>, off: u32| {
        fixups.push((base + out.len() as u32, Target::LineStr(off)));
        out.extend_from_slice(&[0; 4]);
    };
    for d in &dirs {
        let off = strings.add(d);
        strp(&mut out, &mut fixups, off);
    }
    out.push(if md5 { 3 } else { 2 });
    out.extend_from_slice(&[LNCT_PATH, FORM_LINE_STRP, LNCT_DIRECTORY_INDEX, FORM_UDATA]);
    if md5 {
        out.extend_from_slice(&[LNCT_MD5, FORM_DATA16]);
    }
    uleb(&mut out, entries.len() as u64);
    for (i, (name, dir)) in entries.iter().enumerate() {
        let off = strings.add(name);
        strp(&mut out, &mut fixups, off);
        uleb(&mut out, *dir);
        if md5 {
            let f = if i == 0 { root } else { &files[i - 1] };
            out.extend_from_slice(&f.md5.expect("all have one"));
        }
    }
    let header_length = (out.len() - header_length_at - 4) as u32;
    out[header_length_at..header_length_at + 4].copy_from_slice(&header_length.to_le_bytes());

    for seq in sequences {
        program(&mut out, &mut fixups, base, seq, address_size);
    }
    let unit_length = (out.len() - 4) as u32;
    out[..4].copy_from_slice(&unit_length.to_le_bytes());
    Table {
        line: out,
        line_str: strings.bytes,
        fixups,
    }
}

/// One sequence of the line number program.
fn program(out: &mut Vec<u8>, fixups: &mut Vec<(u32, Target)>, base: u32, seq: &Sequence, address_size: u8) {
    let (mut file, mut line, mut column, mut flags, mut isa) = (1, 1i64, 0, IS_STMT, 0);
    let mut last: Option<u32> = None;
    for &(offset, loc) in &seq.rows {
        if file != loc.file {
            file = loc.file;
            out.push(SET_FILE);
            uleb(out, file.into());
        }
        if column != loc.column {
            column = loc.column;
            out.push(SET_COLUMN);
            uleb(out, column.into());
        }
        if loc.discriminator != 0 {
            let mut operand = Vec::new();
            uleb(&mut operand, loc.discriminator.into());
            out.push(0);
            uleb(out, operand.len() as u64 + 1);
            out.push(SET_DISCRIMINATOR);
            out.extend_from_slice(&operand);
        }
        if isa != loc.isa {
            isa = loc.isa;
            out.push(SET_ISA);
            uleb(out, isa.into());
        }
        if (loc.flags ^ flags) & IS_STMT != 0 {
            flags = loc.flags;
            out.push(NEGATE_STMT);
        }
        for (flag, op) in [
            (BASIC_BLOCK, SET_BASIC_BLOCK),
            (PROLOGUE_END, SET_PROLOGUE_END),
            (EPILOGUE_BEGIN, SET_EPILOGUE_BEGIN),
        ] {
            if loc.flags & flag != 0 {
                out.push(op);
            }
        }
        let line_delta = i64::from(loc.line) - line;
        match last {
            None => {
                // An extended opcode: 0, its length (the opcode and the
                // address), DW_LNE_set_address, then the address itself.
                out.extend_from_slice(&[0, 1 + address_size, SET_ADDRESS]);
                fixups.push((base + out.len() as u32, Target::Section(seq.section, offset)));
                out.resize(out.len() + usize::from(address_size), 0);
                advance(out, line_delta, 0);
            }
            Some(prev) => advance(out, line_delta, (offset - prev).into()),
        }
        line = loc.line.into();
        last = Some(offset);
    }
    // The end: one past the section's last byte.
    let delta = i64::from(seq.end - last.unwrap_or(seq.end));
    if delta == MAX_SPECIAL_ADDR {
        out.push(CONST_ADD_PC);
    } else if delta != 0 {
        out.push(ADVANCE_PC);
        uleb(out, delta as u64);
    }
    out.extend_from_slice(&[0, 1, END_SEQUENCE]);
}

/// Adds a row `line_delta` lines and `addr_delta` bytes after the last one,
/// in as few bytes as LLVM does. A special opcode `op` (13..=255) adds
/// `LINE_BASE + (op - 13) % 14` lines and `(op - 13) / 14` bytes and makes the
/// row; a line delta out of its range goes first with `DW_LNS_advance_line`,
/// an address delta up to 17 bytes too big with `DW_LNS_const_add_pc`
/// (special opcode 255's advance), a bigger one with `DW_LNS_advance_pc`.
fn advance(out: &mut Vec<u8>, mut line_delta: i64, addr_delta: i64) {
    let mut temp = line_delta - LINE_BASE;
    let mut need_copy = false;
    if !(0..LINE_RANGE).contains(&temp) || temp + OPCODE_BASE > 255 {
        out.push(ADVANCE_LINE);
        sleb(out, line_delta);
        line_delta = 0;
        temp = -LINE_BASE;
        need_copy = true;
    }
    if line_delta == 0 && addr_delta == 0 {
        out.push(COPY);
        return;
    }
    temp += OPCODE_BASE;
    if addr_delta < 256 + MAX_SPECIAL_ADDR {
        let op = temp + addr_delta * LINE_RANGE;
        if op <= 255 {
            out.push(op as u8);
            return;
        }
        let op = temp + (addr_delta - MAX_SPECIAL_ADDR) * LINE_RANGE;
        if addr_delta >= MAX_SPECIAL_ADDR && op <= 255 {
            out.extend_from_slice(&[CONST_ADD_PC, op as u8]);
            return;
        }
    }
    out.push(ADVANCE_PC);
    uleb(out, addr_delta as u64);
    out.push(if need_copy { COPY } else { temp as u8 });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(dir: &str, name: &str) -> File {
        File {
            dir: dir.into(),
            name: name.into(),
            md5: None,
        }
    }

    #[test]
    fn leb128() {
        let enc = |v: i64, signed: bool| {
            let mut b = Vec::new();
            if signed {
                sleb(&mut b, v)
            } else {
                uleb(&mut b, v as u64)
            }
            b
        };
        assert_eq!(enc(0, false), [0]);
        assert_eq!(enc(127, false), [0x7f]);
        assert_eq!(enc(128, false), [0x80, 1]);
        assert_eq!(enc(404, false), [0x94, 3]);
        assert_eq!(enc(-35, true), [0x5d]);
        assert_eq!(enc(991, true), [0xdf, 7]);
        assert_eq!(enc(63, true), [0x3f]);
        assert_eq!(enc(64, true), [0xc0, 0]);
        assert_eq!(enc(-64, true), [0x40]);
        assert_eq!(enc(-65, true), [0xbf, 0x7f]);
    }

    #[test]
    fn files_split_like_llvm() {
        // From clang 23 on the same `.file`s.
        let (dirs, entries) = file_tables(
            "/src",
            &file("/src", "main.c"),
            &[
                file("", "kernel/a.h"),
                file("/other", "x.h"),
                file("/src", "y.h"),
                file("/src/kernel", "z.h"),
                file("", "/abs/q.h"),
                file("/src", "kernel/b.h"),
            ],
        );
        assert_eq!(dirs, ["/src", "kernel", "/other", "/src/kernel", "/abs"]);
        let entries: Vec<(&str, u64)> = entries.iter().map(|(n, d)| (n.as_str(), *d)).collect();
        assert_eq!(
            entries,
            [
                ("main.c", 0),
                ("a.h", 1),
                ("x.h", 2),
                ("y.h", 0),
                ("z.h", 3),
                ("q.h", 4),
                ("b.h", 1)
            ]
        );
    }

    #[test]
    fn advances() {
        let enc = |line: i64, addr: i64| {
            let mut b = Vec::new();
            advance(&mut b, line, addr);
            b
        };
        assert_eq!(enc(1, 0), [0x13]);
        assert_eq!(enc(1, 4), [0x4b]);
        assert_eq!(enc(0, 0), [COPY]);
        assert_eq!(enc(9, 0), [ADVANCE_LINE, 9, COPY]);
        assert_eq!(enc(-3, 404), [ADVANCE_PC, 0x94, 3, 0x0f]);
        assert_eq!(enc(0, 68), [ADVANCE_PC, 68, 0x12]);
        assert_eq!(
            enc(291, 40004),
            [ADVANCE_LINE, 0xa3, 2, ADVANCE_PC, 0xc4, 0xb8, 2, COPY]
        );
        // 20 bytes: one const_add_pc (17), then a special opcode for 3.
        assert_eq!(enc(0, 20), [CONST_ADD_PC, (5 + 13 + 3 * 14) as u8]);
    }

    /// The whole table of one sequence on both widths: they differ only in
    /// `address_size` and in `DW_LNE_set_address`'s operand (and so in
    /// `unit_length`). The bytes are `llvm-objdump -s -j .debug_line` of
    /// this source, assembled by clang 23 with `-mno-relax` for
    /// `--target=riscv32 -march=rv32ima_zicsr_zifencei -mabi=ilp32` and
    /// `--target=riscv64 -march=rv64ima_zicsr_zifencei -mabi=lp64`; the
    /// fixups are its `.rela.debug_line` (`llvm-readelf -r`):
    ///
    /// ```text
    ///     .file 0 "/src" "t.c"
    ///     .text
    ///     .loc 0 3 5
    ///     nop
    ///     .loc 0 4 1 prologue_end
    ///     nop
    /// ```
    #[test]
    fn set_address_has_the_width_of_an_address() {
        let hex = |s: &str| -> Vec<u8> {
            let s: String = s.split_whitespace().collect();
            (0..s.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
                .collect()
        };
        let loc = |line, column, flags| Loc {
            file: 0,
            line,
            column,
            flags,
            ..Loc::default()
        };
        let seq = Sequence {
            section: 0,
            rows: vec![(0, loc(3, 5, IS_STMT)), (4, loc(4, 1, IS_STMT | PROLOGUE_END))],
            end: 8,
        };
        let table = |address_size| {
            encode(
                "/src",
                &file("/src", "t.c"),
                &[],
                std::slice::from_ref(&seq),
                0,
                0,
                address_size,
            )
        };

        let rv32 = table(4);
        let clang32 = "42000000 05000400 25000000 010101fb 0e0d0001 01010100 00000100 00010101 \
                       1f010000 00000201 1f020f01 00000000 00040005 05000502 00000000 1405010a \
                       4b020400 0101";
        assert_eq!(rv32.line, hex(clang32));
        // R_RISCV_32 .debug_line_str+0 and +5, R_RISCV_32 .text+0.
        let fixups = vec![
            (0x22, Target::LineStr(0)),
            (0x2c, Target::LineStr(5)),
            (0x38, Target::Section(0, 0)),
        ];
        assert_eq!(rv32.fixups, fixups);
        assert_eq!(rv32.line_str, b"/src\0t.c\0");

        let rv64 = table(8);
        let clang64 = "46000000 05000800 25000000 010101fb 0e0d0001 01010100 00000100 00010101 \
                       1f010000 00000201 1f020f01 00000000 00040005 05000902 00000000 00000000 \
                       1405010a 4b020400 0101";
        assert_eq!(rv64.line, hex(clang64));
        // R_RISCV_32 .debug_line_str+0 and +5, R_RISCV_64 .text+0: the same
        // places, since nothing before the address (the header, set_file,
        // set_column) depends on its width.
        assert_eq!(rv64.fixups, fixups);
        assert_eq!(rv64.line_str, rv32.line_str);
    }
}
