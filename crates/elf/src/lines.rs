//! DWARF line tables (`.debug_line`): which source line each address comes from.
//!
//! A line table is a header (the source files, by directory) and a *line number
//! program*, a byte code whose run produces the rows of a table: address, file,
//! line, column and flags. [`Elf::line_tables`] runs it, for DWARF versions 2 to 5.
//! In a relocatable object the addresses and the strings the header names are
//! relocations, applied here first; each sequence then says which section its
//! addresses are in.
//!
//! The opcodes and the header are explained in merlin-as's `dwarf` module, which
//! writes them.

use crate::{err, Elf, Error, Section, ET_REL, SHT_RELA, STT_SECTION};
use std::collections::HashMap;

/// One row of a line table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Row {
    pub addr: u32,
    /// An index into [`LineTable::files`].
    pub file: usize,
    pub line: u32,
    pub column: u32,
    /// A place a debugger may stop at for this line (the start of a statement).
    pub is_stmt: bool,
    /// Where the function's prologue ends, a breakpoint's place for it.
    pub prologue_end: bool,
    pub epilogue_begin: bool,
    /// The end of a sequence: `addr` is one past its last byte.
    pub end_sequence: bool,
}

/// The rows of a run of code, in address order, the last one its end.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sequence {
    /// In a relocatable object, the section the addresses are offsets in.
    pub section: Option<usize>,
    pub rows: Vec<Row>,
}

/// One line table: its files (paths made from the directory table) and its
/// sequences.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineTable {
    pub version: u16,
    /// Full paths, by file number: directory and name joined with `/`
    /// (relative ones are relative to the compilation directory, DWARF 5's
    /// directory 0). In DWARF 4 and before, files count from 1 and entry 0
    /// is empty, and relative paths lack the compilation directory, which
    /// only the compile unit (`.debug_info`) has.
    pub files: Vec<String>,
    pub sequences: Vec<Sequence>,
}

impl LineTable {
    /// The path of file `n`, or `?` for a number the table does not have.
    pub fn path(&self, n: usize) -> &str {
        self.files.get(n).map_or("?", |s| s.as_str())
    }
}

/// Whether a path is absolute, on Unix or Windows (`/x`, `\x`, `C:\x`).
fn is_absolute(p: &str) -> bool {
    p.starts_with('/') || p.starts_with('\\') || p.as_bytes().get(1) == Some(&b':')
}

/// `dir` and `name` joined with `/` (just `name` if it is absolute or `dir`
/// is empty).
pub fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() || is_absolute(name) {
        name.to_string()
    } else if dir.ends_with('/') || dir.ends_with('\\') {
        format!("{dir}{name}")
    } else {
        format!("{dir}/{name}")
    }
}

struct Cursor<'a> {
    data: &'a [u8],
    at: usize,
}

impl<'a> Cursor<'a> {
    fn bytes(&mut self, n: usize) -> Result<&'a [u8], Error> {
        let b = self
            .at
            .checked_add(n)
            .and_then(|end| self.data.get(self.at..end))
            .ok_or_else(|| Error("line table: truncated".into()))?;
        self.at += n;
        Ok(b)
    }
    fn u8(&mut self) -> Result<u8, Error> {
        Ok(self.bytes(1)?[0])
    }
    fn u16(&mut self) -> Result<u16, Error> {
        let b = self.bytes(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }
    fn u32(&mut self) -> Result<u32, Error> {
        let b = self.bytes(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn uleb(&mut self) -> Result<u64, Error> {
        let (mut v, mut shift) = (0u64, 0);
        loop {
            let b = self.u8()?;
            if shift < 64 {
                v |= u64::from(b & 0x7f) << shift;
            }
            shift += 7;
            if b & 0x80 == 0 {
                return Ok(v);
            }
        }
    }
    fn sleb(&mut self) -> Result<i64, Error> {
        let (mut v, mut shift) = (0i64, 0);
        loop {
            let b = self.u8()?;
            if shift < 64 {
                v |= i64::from(b & 0x7f) << shift;
            }
            shift += 7;
            if b & 0x80 == 0 {
                if shift < 64 && b & 0x40 != 0 {
                    v |= -1 << shift;
                }
                return Ok(v);
            }
        }
    }
    fn cstr(&mut self) -> Result<String, Error> {
        let rest = &self.data[self.at.min(self.data.len())..];
        let end = rest
            .iter()
            .position(|&b| b == 0)
            .ok_or_else(|| Error("line table: unterminated string".into()))?;
        self.at += end + 1;
        Ok(String::from_utf8_lossy(&rest[..end]).into_owned())
    }
}

/// A string at `off` in a string section.
fn string_at(sec: &[u8], off: u64, what: &str) -> Result<String, Error> {
    let rest = usize::try_from(off)
        .ok()
        .and_then(|o| sec.get(o..))
        .ok_or_else(|| Error(format!("line table: {what} offset {off:#x} out of range")))?;
    let end = rest.iter().position(|&b| b == 0).unwrap_or(rest.len());
    Ok(String::from_utf8_lossy(&rest[..end]).into_owned())
}

/// DWARF 5 entry formats: `DW_LNCT_*` content, `DW_FORM_*` form.
const LNCT_PATH: u64 = 1;
const LNCT_DIRECTORY_INDEX: u64 = 2;

/// The value of one attribute of a directory or file entry: a string, or a
/// number (other content, like an MD5 sum, comes back as 0).
enum Value {
    Str(String),
    Num(u64),
}

fn read_form(c: &mut Cursor, form: u64, strs: &Strings) -> Result<Value, Error> {
    Ok(match form {
        0x08 => Value::Str(c.cstr()?), // string
        0x1f => Value::Str(string_at(strs.line_str, c.u32()?.into(), ".debug_line_str")?), // line_strp
        0x0e => Value::Str(string_at(strs.str, c.u32()?.into(), ".debug_str")?), // strp
        0x0b => Value::Num(c.u8()?.into()), // data1
        0x05 => Value::Num(c.u16()?.into()), // data2
        0x06 => Value::Num(c.u32()?.into()), // data4
        0x07 => {
            let b = c.bytes(8)?;
            Value::Num(u64::from_le_bytes(b.try_into().unwrap()))
        }
        0x0f => Value::Num(c.uleb()?), // udata
        0x1e => {
            c.bytes(16)?; // data16: an MD5 sum
            Value::Num(0)
        }
        0x09 => {
            let n = c.uleb()? as usize; // block
            c.bytes(n)?;
            Value::Num(0)
        }
        _ => return err(format!("line table: unsupported form {form:#x}")),
    })
}

struct Strings<'a> {
    line_str: &'a [u8],
    str: &'a [u8],
}

/// Reads the directory or file entries of a DWARF 5 header: for each, its
/// path and directory index.
fn entries(c: &mut Cursor, strs: &Strings) -> Result<Vec<(String, u64)>, Error> {
    let n_formats = c.u8()?;
    let mut formats = Vec::new();
    for _ in 0..n_formats {
        formats.push((c.uleb()?, c.uleb()?));
    }
    let count = c.uleb()?;
    let mut out = Vec::new();
    for _ in 0..count {
        let (mut path, mut dir) = (String::new(), 0);
        for &(content, form) in &formats {
            match (content, read_form(c, form, strs)?) {
                (LNCT_PATH, Value::Str(s)) => path = s,
                (LNCT_DIRECTORY_INDEX, Value::Num(n)) => dir = n,
                _ => {}
            }
        }
        out.push((path, dir));
    }
    Ok(out)
}

/// Runs every line table in `line` (`.debug_line`). `sections` maps the
/// offset of a `DW_LNE_set_address` operand to the section it points into
/// (relocatable objects only).
pub fn parse(
    line: &[u8],
    line_str: &[u8],
    str: &[u8],
    sections: &HashMap<usize, usize>,
) -> Result<Vec<LineTable>, Error> {
    let strs = Strings { line_str, str };
    let mut tables = Vec::new();
    let mut at = 0;
    while at < line.len() {
        let mut c = Cursor { data: line, at };
        let unit_length = c.u32()?;
        if unit_length >= 0xffff_fff0 {
            return err("line table: 64-bit DWARF is not supported");
        }
        let end = c.at + unit_length as usize;
        if end > line.len() {
            return err("line table: unit runs past the end of .debug_line");
        }
        let unit = &line[..end];
        let mut c = Cursor { data: unit, at: c.at };
        let version = c.u16()?;
        if !(2..=5).contains(&version) {
            return err(format!("line table: DWARF version {version} is not supported"));
        }
        if version >= 5 {
            let (addr_size, _seg) = (c.u8()?, c.u8()?);
            if addr_size != 4 {
                return err(format!("line table: {addr_size}-byte addresses"));
            }
        }
        let header_length = c.u32()? as usize;
        let program = c.at + header_length;
        let min_inst = c.u8()?;
        if version >= 4 {
            c.u8()?; // maximum_operations_per_instruction: 1 without VLIW
        }
        let default_is_stmt = c.u8()? != 0;
        let line_base = c.u8()? as i8 as i64;
        let line_range = c.u8()?;
        let opcode_base = c.u8()?;
        if line_range == 0 || opcode_base == 0 {
            return err("line table: line_range and opcode_base must not be 0");
        }
        let lengths = c.bytes(opcode_base as usize - 1)?.to_vec();

        let files = if version >= 5 {
            let dirs = entries(&mut c, &strs)?;
            let comp = dirs.first().map_or("", |(d, _)| d.as_str());
            entries(&mut c, &strs)?
                .into_iter()
                .map(|(name, dir)| {
                    let d = dirs.get(dir as usize).map_or("", |(d, _)| d.as_str());
                    if dir == 0 || is_absolute(d) {
                        join(d, &name)
                    } else {
                        join(&join(comp, d), &name)
                    }
                })
                .collect()
        } else {
            let mut dirs = vec![String::new()];
            loop {
                let d = c.cstr()?;
                if d.is_empty() {
                    break;
                }
                dirs.push(d);
            }
            let mut files = vec![String::new()];
            loop {
                let name = c.cstr()?;
                if name.is_empty() {
                    break;
                }
                let dir = c.uleb()? as usize;
                c.uleb()?; // modification time
                c.uleb()?; // length
                files.push(join(dirs.get(dir).map_or("", |d| d.as_str()), &name));
            }
            files
        };

        let mut table = LineTable {
            version,
            files,
            sequences: Vec::new(),
        };
        c.at = program;
        run(
            &mut c,
            &mut table,
            sections,
            (min_inst, default_is_stmt, line_base, line_range, opcode_base),
            &lengths,
        )?;
        tables.push(table);
        at = end;
    }
    Ok(tables)
}

/// The line number program of one table.
fn run(
    c: &mut Cursor,
    table: &mut LineTable,
    sections: &HashMap<usize, usize>,
    (min_inst, default_is_stmt, line_base, line_range, opcode_base): (u8, bool, i64, u8, u8),
    lengths: &[u8],
) -> Result<(), Error> {
    let fresh = Row {
        addr: 0,
        file: 1,
        line: 1,
        column: 0,
        is_stmt: default_is_stmt,
        prologue_end: false,
        epilogue_begin: false,
        end_sequence: false,
    };
    let mut r = fresh;
    let mut seq = Sequence {
        section: None,
        rows: Vec::new(),
    };
    let min_inst = u32::from(min_inst);
    let advance = |r: &mut Row, n: u64| r.addr = r.addr.wrapping_add((n as u32).wrapping_mul(min_inst));
    while c.at < c.data.len() {
        let op = c.u8()?;
        if op >= opcode_base {
            let adj = u64::from(op - opcode_base);
            advance(&mut r, adj / u64::from(line_range));
            r.line = (i64::from(r.line) + line_base + (adj % u64::from(line_range)) as i64) as u32;
            seq.rows.push(r);
            (r.prologue_end, r.epilogue_begin) = (false, false);
            continue;
        }
        match op {
            0 => {
                let len = c.uleb()? as usize;
                let start = c.at;
                match c.u8()? {
                    1 => {
                        r.end_sequence = true;
                        seq.rows.push(r);
                        table.sequences.push(std::mem::replace(
                            &mut seq,
                            Sequence {
                                section: None,
                                rows: Vec::new(),
                            },
                        ));
                        r = fresh;
                    }
                    2 => {
                        seq.section = sections.get(&c.at).copied();
                        r.addr = match len - 1 {
                            4 => c.u32()?,
                            n => return err(format!("line table: {n}-byte address")),
                        };
                    }
                    _ => {} // define_file, set_discriminator, vendor ones: skipped
                }
                c.at = start + len;
            }
            1 => {
                seq.rows.push(r);
                (r.prologue_end, r.epilogue_begin) = (false, false);
            }
            2 => advance(&mut r, c.uleb()?),
            3 => r.line = (i64::from(r.line) + c.sleb()?) as u32,
            4 => r.file = c.uleb()? as usize,
            5 => r.column = c.uleb()? as u32,
            6 => r.is_stmt = !r.is_stmt,
            7 => {}
            8 => advance(&mut r, (255 - u64::from(opcode_base)) / u64::from(line_range)),
            9 => r.addr = r.addr.wrapping_add(c.u16()?.into()),
            10 => r.prologue_end = true,
            11 => r.epilogue_begin = true,
            _ => {
                // set_isa, and opcodes from a later version: skip their operands.
                for _ in 0..lengths.get(op as usize - 1).copied().unwrap_or(0) {
                    c.uleb()?;
                }
            }
        }
    }
    Ok(())
}

impl Elf<'_> {
    /// The line tables of `.debug_line` (none if there is no such section).
    /// In a relocatable object its relocations are applied first.
    pub fn line_tables(&self) -> Result<Vec<LineTable>, Error> {
        let named = |n: &str| self.sections.iter().position(|s| s.name == n);
        let Some(li) = named(".debug_line") else {
            return Ok(Vec::new());
        };
        let data_of = |n: &str| -> Result<&[u8], Error> {
            match named(n) {
                Some(i) => self.section_data(&self.sections[i]),
                None => Ok(&[]),
            }
        };
        let mut line = self.section_data(&self.sections[li])?.to_vec();
        let mut sections = HashMap::new();
        if self.header.ty == ET_REL {
            let symbols = self.symbols()?;
            let relas: Vec<&Section> = self
                .sections
                .iter()
                .filter(|s| s.ty == SHT_RELA && s.info as usize == li)
                .collect();
            for rel in relas {
                for r in self.relocations(rel)? {
                    let Some(sym) = symbols.get(r.sym as usize) else {
                        continue;
                    };
                    let value = if sym.ty() == STT_SECTION { 0 } else { sym.value };
                    let v = value.wrapping_add(r.addend as u32);
                    let at = r.offset as usize;
                    // R_RISCV_32 writes an address (S + A). With linker
                    // relaxation, address advances are differences of two
                    // labels: an R_RISCV_ADDn adds one, an R_RISCV_SUBn
                    // subtracts the other, and R_RISCV_SETn sets the value.
                    let (size, op): (usize, fn(u32, u32) -> u32) = match r.ty {
                        1 => (4, |_, v| v),
                        33 => (1, u32::wrapping_add),
                        34 => (2, u32::wrapping_add),
                        35 => (4, u32::wrapping_add),
                        37 => (1, u32::wrapping_sub),
                        38 => (2, u32::wrapping_sub),
                        39 => (4, u32::wrapping_sub),
                        54 => (1, |_, v| v),
                        55 => (2, |_, v| v),
                        56 => (4, |_, v| v),
                        _ => continue,
                    };
                    let Some(field) = line.get_mut(at..at + size) else {
                        continue;
                    };
                    let mut old = [0; 4];
                    old[..size].copy_from_slice(field);
                    let new = op(u32::from_le_bytes(old), v).to_le_bytes();
                    field.copy_from_slice(&new[..size]);
                    if r.ty == 1 {
                        if let Some(sec) = sym.section() {
                            sections.insert(at, sec);
                        }
                    }
                }
            }
        }
        parse(
            &line,
            data_of(".debug_line_str")?,
            data_of(".debug_str")?,
            &sections,
        )
    }
}
