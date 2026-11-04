//! DWARF line tables (`.debug_line`): which source line each address comes from.
//!
//! A line table is a header (the source files, by directory) and a *line number
//! program*, a byte code whose run produces the rows of a table: address, file,
//! line, column and flags. [`Elf::line_tables`] runs it, for DWARF versions 2 to 5.
//! In a relocatable object the addresses and the strings the header names are
//! relocations, applied here first; each sequence then says which section its
//! addresses are in.
//!
//! Addresses are 4 bytes in an RV32 program and 8 in an RV64 one: the size of
//! `DW_LNE_set_address`'s operand, which DWARF 5 also gives in the header.
//!
//! The opcodes and the header are explained in merlin-as's `dwarf` module, which
//! writes them.

use crate::{err, Elf, Error, Section, ET_REL, SHT_RELA, STT_SECTION};
use std::collections::HashMap;

/// One row of a line table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Row {
    pub addr: u64,
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
    /// Where the table starts in `.debug_line`: what the `DW_AT_stmt_list`
    /// of the compile unit it belongs to says.
    pub offset: u64,
    pub version: u16,
    /// Full paths, by file number: directory and name joined with `/`
    /// (relative ones are relative to the compilation directory, DWARF 5's
    /// directory 0). In DWARF 4 and before, files count from 1 and entry 0
    /// is empty, and relative paths lack the compilation directory, which
    /// only the compile unit (`.debug_info`) has: see [`LineTable::file_path`].
    pub files: Vec<String>,
    /// The directories as the header lists them, by number (DWARF 4 and
    /// before count them from 1: entry 0 is empty).
    pub dirs: Vec<String>,
    /// The files as the header lists them, by number: name and directory
    /// number (entry 0 empty before DWARF 5, as in `files`).
    pub entries: Vec<(String, u64)>,
    pub sequences: Vec<Sequence>,
}

impl LineTable {
    /// The path of file `n`, or `?` for a number the table does not have.
    pub fn path(&self, n: usize) -> &str {
        self.files.get(n).map_or("?", |s| s.as_str())
    }

    /// The path of file `n` as LLVM's symbolizer gives it (llvm-objdump
    /// `-l`, `DWARFDebugLine::Prologue::getFileNameByIndex` for an absolute
    /// path), `comp_dir` being the compile unit's `DW_AT_comp_dir`: a name
    /// that is absolute is the path; any other goes after its directory, and
    /// that after `comp_dir`, unless the directory is absolute or is DWARF
    /// 5's directory 0 (the compilation directory itself). None for a number
    /// the table does not have (0 before DWARF 5, where files count from 1).
    pub fn file_path(&self, n: usize, comp_dir: &str) -> Option<String> {
        if n >= self.entries.len() || (self.version < 5 && n == 0) {
            return None;
        }
        let (name, dir) = &self.entries[n];
        if is_absolute_llvm(name) {
            return Some(name.clone());
        }
        // `dirs` is numbered as the file entries number it, in every version.
        let include_dir = usize::try_from(*dir)
            .ok()
            .and_then(|d| self.dirs.get(d))
            .map_or("", |d| d.as_str());
        let mut path = String::new();
        if (self.version < 5 || *dir != 0) && !comp_dir.is_empty() && !is_absolute_llvm(include_dir) {
            append(&mut path, comp_dir);
        }
        append(&mut path, include_dir);
        append(&mut path, name);
        Some(path)
    }
}

/// LLVM's `isPathAbsoluteOnWindowsOrPosix`: `/x`, or a Windows path with a
/// root name and a root directory: a drive (`C:\x`, `C:/x`) or a network
/// name (`\\host\x`). `\x` and `C:x` are relative (to a drive, to a drive's
/// directory).
fn is_absolute_llvm(p: &str) -> bool {
    let (name, rest) = root_name(p);
    p.starts_with('/') || (!name.is_empty() && rest.starts_with(is_sep))
}

/// Either of Windows' separators.
fn is_sep(c: char) -> bool {
    c == '/' || c == '\\'
}

/// A Windows path's root name and the rest: a drive (`C:`) or a network name
/// (two separators and a name up to the next one, `\\host`), or none.
fn root_name(p: &str) -> (&str, &str) {
    let b = p.as_bytes();
    if b.len() >= 2 && b[1] == b':' && b[0].is_ascii_alphabetic() {
        p.split_at(2)
    } else if b.len() >= 3 && is_sep(b[0] as char) && is_sep(b[1] as char) && !is_sep(b[2] as char) {
        p.split_at(p[2..].find(is_sep).map_or(p.len(), |i| i + 2))
    } else {
        ("", p)
    }
}

/// LLVM's `sys::path::append` of one component in Windows style (where the
/// reference files are made, so both separators count), joining with `/`:
/// after a path that ends with a separator, the component without its
/// leading ones; else a separator first, unless the path is empty or the
/// component starts with a separator or a root name.
fn append(path: &mut String, component: &str) {
    if path.ends_with(is_sep) {
        path.push_str(component.trim_start_matches(is_sep));
        return;
    }
    if !(path.is_empty() || component.starts_with(is_sep) || !root_name(component).0.is_empty()) {
        path.push('/');
    }
    path.push_str(component);
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
    fn u64(&mut self) -> Result<u64, Error> {
        Ok(u64::from_le_bytes(self.bytes(8)?.try_into().unwrap()))
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
        0x07 => Value::Num(c.u64()?),  // data8
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
/// (relocatable objects only). `addr_size` is the size of the file's
/// addresses, 4 (ELF32) or 8 (ELF64): a table's must be the same.
pub fn parse(
    line: &[u8],
    line_str: &[u8],
    str: &[u8],
    sections: &HashMap<usize, usize>,
    addr_size: usize,
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
        let end = c.at.saturating_add(unit_length as usize);
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
            let (size, _seg) = (c.u8()?, c.u8()?);
            if usize::from(size) != addr_size {
                return err(format!(
                    "line table: {size}-byte addresses in a file of {addr_size}-byte ones"
                ));
            }
        }
        let header_length = c.u32()? as usize;
        let program = c.at.saturating_add(header_length);
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

        // The directories and the files, numbered as the files' entries
        // number them: DWARF 4 and before count both from 1.
        let (dirs, file_entries) = if version >= 5 {
            let dirs = entries(&mut c, &strs)?.into_iter().map(|(d, _)| d).collect();
            (dirs, entries(&mut c, &strs)?)
        } else {
            let mut dirs = vec![String::new()];
            loop {
                let d = c.cstr()?;
                if d.is_empty() {
                    break;
                }
                dirs.push(d);
            }
            let mut files = vec![(String::new(), 0)];
            loop {
                let name = c.cstr()?;
                if name.is_empty() {
                    break;
                }
                let dir = c.uleb()?;
                c.uleb()?; // modification time
                c.uleb()?; // length
                files.push((name, dir));
            }
            (dirs, files)
        };
        let dir_of = |n: u64| {
            usize::try_from(n)
                .ok()
                .and_then(|n| dirs.get(n))
                .map_or("", |d| d.as_str())
        };
        let files = file_entries
            .iter()
            .enumerate()
            .map(|(i, (name, dir))| {
                let d = dir_of(*dir);
                if version < 5 {
                    // Entry 0 is no file.
                    if i == 0 {
                        String::new()
                    } else {
                        join(d, name)
                    }
                } else if *dir == 0 || is_absolute(d) {
                    join(d, name)
                } else {
                    join(&join(dir_of(0), d), name)
                }
            })
            .collect();

        let mut table = LineTable {
            offset: at as u64,
            version,
            files,
            dirs,
            entries: file_entries,
            sequences: Vec::new(),
        };
        c.at = program;
        run(
            &mut c,
            &mut table,
            sections,
            (min_inst, default_is_stmt, line_base, line_range, opcode_base),
            &lengths,
            addr_size,
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
    addr_size: usize,
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
    let min_inst = u64::from(min_inst);
    let advance = |r: &mut Row, n: u64| r.addr = r.addr.wrapping_add(n.wrapping_mul(min_inst));
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
                        // The operand is an address: 4 bytes on RV32, 8 on RV64.
                        r.addr = match len.checked_sub(1) {
                            Some(n) if n != addr_size => {
                                return err(format!(
                                    "line table: DW_LNE_set_address of {n} bytes in a file of {addr_size}-byte addresses"
                                ))
                            }
                            Some(4) => c.u32()?.into(),
                            Some(8) => c.u64()?,
                            _ => return err(format!("line table: DW_LNE_set_address of length {len}")),
                        };
                    }
                    _ => {} // define_file, set_discriminator, vendor ones: skipped
                }
                c.at = start.saturating_add(len);
            }
            1 => {
                seq.rows.push(r);
                (r.prologue_end, r.epilogue_begin) = (false, false);
            }
            2 => advance(&mut r, c.uleb()?),
            // Wrapping: a corrupt advance must not overflow (lines are u32).
            3 => r.line = i64::from(r.line).wrapping_add(c.sleb()?) as u32,
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

/// Applies one relocation of a debug section at `at` with the value `v` (S + A),
/// as a linker would. Returns false for a type debug information does not use
/// (or whose unrelocated value LLVM reads, the ULEB128 ones), or a field
/// outside `data`.
///
/// R_RISCV_32 and R_RISCV_64 write an address, 4 bytes on RV32 and 8 on RV64.
/// With linker relaxation, address advances are differences of two labels:
/// an R_RISCV_ADDn adds one, an R_RISCV_SUBn subtracts the other, and
/// R_RISCV_SETn sets the value. Each writes the low `n` bits of the result.
fn relocate(data: &mut [u8], at: usize, ty: u32, v: u64) -> bool {
    let (size, op): (usize, fn(u64, u64) -> u64) = match ty {
        1 => (4, |_, v| v),           // R_RISCV_32
        2 => (8, |_, v| v),           // R_RISCV_64
        33 => (1, u64::wrapping_add), // R_RISCV_ADD8
        34 => (2, u64::wrapping_add), // R_RISCV_ADD16
        35 => (4, u64::wrapping_add), // R_RISCV_ADD32
        36 => (8, u64::wrapping_add), // R_RISCV_ADD64
        37 => (1, u64::wrapping_sub), // R_RISCV_SUB8
        38 => (2, u64::wrapping_sub), // R_RISCV_SUB16
        39 => (4, u64::wrapping_sub), // R_RISCV_SUB32
        40 => (8, u64::wrapping_sub), // R_RISCV_SUB64
        54 => (1, |_, v| v),          // R_RISCV_SET8
        55 => (2, |_, v| v),          // R_RISCV_SET16
        56 => (4, |_, v| v),          // R_RISCV_SET32
        _ => return false,
    };
    let Some(field) = at.checked_add(size).and_then(|end| data.get_mut(at..end)) else {
        return false;
    };
    let mut old = [0; 8];
    old[..size].copy_from_slice(field);
    let new = op(u64::from_le_bytes(old), v).to_le_bytes();
    field.copy_from_slice(&new[..size]);
    true
}

/// A debug section's bytes, its relocations applied in a relocatable object,
/// and for each address field relocated (`R_RISCV_32`, `R_RISCV_64`) the
/// section its symbol is in.
pub(crate) struct Relocated {
    pub data: Vec<u8>,
    pub sections: HashMap<usize, usize>,
}

impl Elf<'_> {
    /// The index of the section named `name`.
    pub(crate) fn section_named(&self, name: &str) -> Option<usize> {
        self.sections.iter().position(|s| s.name == name)
    }

    /// The bytes of section `name` (none if there is no such section), as
    /// they are read for debug information: in a relocatable object with the
    /// relocations applied that LLVM's DWARF reader applies. That leaves out
    /// `R_RISCV_SET_ULEB128` and `R_RISCV_SUB_ULEB128`, whose unrelocated
    /// value (the assembler's) is already right.
    pub(crate) fn relocated(&self, name: &str) -> Result<Option<Relocated>, Error> {
        let Some(index) = self.section_named(name) else {
            return Ok(None);
        };
        let mut data = self.section_data(&self.sections[index])?.to_vec();
        let mut sections = HashMap::new();
        if self.header.ty == ET_REL {
            let symbols = self.symbols()?;
            let relas: Vec<&Section> = self
                .sections
                .iter()
                .filter(|s| s.ty == SHT_RELA && s.info as usize == index)
                .collect();
            for rel in relas {
                for r in self.relocations(rel)? {
                    let Some(sym) = symbols.get(r.sym as usize) else {
                        continue;
                    };
                    let value = if sym.ty() == STT_SECTION { 0 } else { sym.value };
                    let v = value.wrapping_add(r.addend as u64);
                    let Ok(at) = usize::try_from(r.offset) else {
                        continue;
                    };
                    if !relocate(&mut data, at, r.ty, v) {
                        continue;
                    }
                    if r.ty == 1 || r.ty == 2 {
                        if let Some(sec) = sym.section() {
                            sections.insert(at, sec);
                        }
                    }
                }
            }
        }
        Ok(Some(Relocated { data, sections }))
    }

    /// The bytes of section `name`, or none.
    pub(crate) fn data_of(&self, name: &str) -> Result<&[u8], Error> {
        match self.section_named(name) {
            Some(i) => self.section_data(&self.sections[i]),
            None => Ok(&[]),
        }
    }

    /// The line tables of `.debug_line` (none if there is no such section).
    /// In a relocatable object its relocations are applied first.
    pub fn line_tables(&self) -> Result<Vec<LineTable>, Error> {
        let Some(line) = self.relocated(".debug_line")? else {
            return Ok(Vec::new());
        };
        parse(
            &line.data,
            self.data_of(".debug_line_str")?,
            self.data_of(".debug_str")?,
            &line.sections,
            self.class.addr_size(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{parse, relocate, LineTable};
    use std::collections::HashMap;

    /// LLVM's getFileNameByIndex: the compile unit's directory before a
    /// relative directory, but not before DWARF 5's directory 0 or an
    /// absolute one; DWARF 4 counts files from 1.
    #[test]
    fn file_paths_as_llvm_makes_them() {
        let table = |version, dirs: &[&str], entries: &[(&str, u64)]| LineTable {
            offset: 0,
            version,
            files: Vec::new(),
            dirs: dirs.iter().map(|d| d.to_string()).collect(),
            entries: entries.iter().map(|&(n, d)| (n.to_string(), d)).collect(),
            sequences: Vec::new(),
        };
        let v5 = table(
            5,
            &["/cu", "inc", "/abs"],
            &[("a.c", 0), ("b.h", 1), ("c.h", 2), ("/x/d.h", 1)],
        );
        assert_eq!(v5.file_path(0, "/cu").as_deref(), Some("/cu/a.c"));
        assert_eq!(v5.file_path(1, "/unit").as_deref(), Some("/unit/inc/b.h"));
        assert_eq!(v5.file_path(2, "/unit").as_deref(), Some("/abs/c.h"));
        assert_eq!(v5.file_path(3, "/unit").as_deref(), Some("/x/d.h"));
        assert_eq!(v5.file_path(4, "/unit"), None);
        let v4 = table(4, &["", "inc/"], &[("", 0), ("a.c", 0), ("b.h", 1), ("c.h", 7)]);
        assert_eq!(v4.file_path(0, "build"), None);
        assert_eq!(v4.file_path(1, "build").as_deref(), Some("build/a.c"));
        assert_eq!(v4.file_path(2, "build").as_deref(), Some("build/inc/b.h"));
        assert_eq!(v4.file_path(2, "").as_deref(), Some("inc/b.h"));
        // A directory number out of range is none; a separator already
        // there is not doubled.
        assert_eq!(v4.file_path(3, r"C:\b").as_deref(), Some(r"C:\b/c.h"));
        assert_eq!(v4.file_path(3, r"C:\b\").as_deref(), Some(r"C:\b\c.h"));
    }

    /// A DWARF 4 line table (no address size in its header) for one file,
    /// `a.c`, running `program`.
    fn table(program: &[u8]) -> Vec<u8> {
        let mut header = vec![1, 1, 1, 0xfb, 14, 13]; // min_inst .. opcode_base
        header.extend_from_slice(&[0, 1, 1, 1, 1, 0, 0, 0, 1, 0, 0, 1]); // opcode lengths
        header.push(0); // no include directories
        header.extend_from_slice(b"a.c\0\0\0\0\0"); // a.c: directory 0, no time or size
        let mut unit = 4u16.to_le_bytes().to_vec();
        unit.extend_from_slice(&(header.len() as u32).to_le_bytes());
        unit.extend_from_slice(&header);
        unit.extend_from_slice(program);
        let mut out = (unit.len() as u32).to_le_bytes().to_vec();
        out.extend_from_slice(&unit);
        out
    }

    /// DW_LNE_set_address with an 8-byte operand, a row, the end.
    fn program_at(addr: u64) -> Vec<u8> {
        let mut p = vec![0, 9, 2];
        p.extend_from_slice(&addr.to_le_bytes());
        p.extend_from_slice(&[1, 0, 1, 1]); // DW_LNS_copy, DW_LNE_end_sequence
        p
    }

    #[test]
    fn addresses_have_the_files_size() {
        let line = table(&program_at(0x1_2345_6789_abcd));
        let tables = parse(&line, &[], &[], &HashMap::new(), 8).unwrap();
        assert_eq!(tables[0].files, ["", "a.c"]);
        assert_eq!(tables[0].sequences[0].rows[0].addr, 0x1_2345_6789_abcd);
        // The same table in an ELF32 file: an 8-byte address is an error.
        let err = parse(&line, &[], &[], &HashMap::new(), 4).unwrap_err();
        assert_eq!(
            err.0,
            "line table: DW_LNE_set_address of 8 bytes in a file of 4-byte addresses"
        );
    }

    #[test]
    fn a_huge_line_advance_wraps_instead_of_overflowing() {
        // DW_LNS_advance_line by i64::MAX (nine 7-bit groups of ones), from line 1.
        let mut p = vec![3, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0];
        p.extend_from_slice(&program_at(0x10));
        let tables = parse(&table(&p), &[], &[], &HashMap::new(), 8).unwrap();
        let want = 1i64.wrapping_add(i64::MAX) as u32;
        assert_eq!(tables[0].sequences[0].rows[0].line, want);
    }

    /// Every relocation type a line table uses, on a field between two others
    /// it must not touch.
    #[test]
    fn relocations_write_their_fields() {
        let v = 0x1122_3344_5566_7788;
        let cases: [(u32, &[u8]); 13] = [
            (1, &[0x88, 0x77, 0x66, 0x55, 9, 9, 9, 9]),
            (2, &[0x88, 0x77, 0x66, 0x55, 0x44, 0x33, 0x22, 0x11]),
            (33, &[0x88 + 1, 9, 9, 9, 9, 9, 9, 9]),
            (34, &[0x88 + 1, 0x77 + 2, 9, 9, 9, 9, 9, 9]),
            (35, &[0x88 + 1, 0x77 + 2, 0x66 + 3, 0x55 + 4, 9, 9, 9, 9]),
            (
                36,
                &[
                    0x88 + 1,
                    0x77 + 2,
                    0x66 + 3,
                    0x55 + 4,
                    0x44 + 5,
                    0x33 + 6,
                    0x22 + 7,
                    0x11 + 8,
                ],
            ),
            (37, &[0x79, 9, 9, 9, 9, 9, 9, 9]),
            (38, &[0x79, 0x8a, 9, 9, 9, 9, 9, 9]),
            (39, &[0x79, 0x8a, 0x9c, 0xae, 9, 9, 9, 9]),
            (40, &[0x79, 0x8a, 0x9c, 0xae, 0xc0, 0xd2, 0xe4, 0xf6]),
            (54, &[0x88, 9, 9, 9, 9, 9, 9, 9]),
            (55, &[0x88, 0x77, 9, 9, 9, 9, 9, 9]),
            (56, &[0x88, 0x77, 0x66, 0x55, 9, 9, 9, 9]),
        ];
        for (ty, want) in cases {
            // The field holds 1, 2, ... 8 (low byte first) before the relocation.
            let mut data = [0xaa, 1, 2, 3, 4, 5, 6, 7, 8, 0xaa];
            let size = want.iter().rposition(|&b| b != 9).unwrap() + 1;
            data[1 + size..9].fill(9);
            assert!(relocate(&mut data, 1, ty, v), "type {ty}");
            assert_eq!(data[0], 0xaa, "type {ty}");
            assert_eq!(&data[1..9], want, "type {ty}");
            assert_eq!(data[9], 0xaa, "type {ty}");
        }
        let mut data = [0; 8];
        assert!(
            !relocate(&mut data, 0, 43, v),
            "R_RISCV_ALIGN is not a line table's"
        );
        assert!(!relocate(&mut data, 4, 2, v), "the field runs past the end");
        assert!(!relocate(&mut data, usize::MAX, 1, v));
        assert_eq!(data, [0; 8]);
    }
}
