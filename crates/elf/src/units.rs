//! DWARF compile units (`.debug_info`): which code a line table describes.
//!
//! A line table (`.debug_line`, [`crate::lines`]) does not say which code it is
//! for; the compile unit does. Its first entry, the unit DIE, gives the
//! address ranges of the unit's code (`DW_AT_low_pc` and `DW_AT_high_pc`, or a
//! range list, `DW_AT_ranges`), the offset of its line table
//! (`DW_AT_stmt_list`) and the compilation directory its paths are relative to
//! (`DW_AT_comp_dir`). LLVM's symbolizer, which llvm-objdump `-l` uses, finds
//! an address's line that way: the compile unit whose ranges hold it, then
//! that unit's line table. A line table no unit names is never read, and
//! neither is one for an address outside every unit.
//!
//! [`Elf::compile_units`] reads the unit DIEs; [`Elf::unit_ranges`] maps
//! addresses to units as LLVM's `DWARFDebugAranges` does, from
//! `.debug_aranges` and then the ranges of the units it does not list.
//!
//! Only the unit DIE is read, not its children: an abbreviation (in
//! `.debug_abbrev`) gives each attribute's form, which says how many bytes
//! its value takes. Addresses may be indices into `.debug_addr` (`addrx`
//! forms, from `DW_AT_addr_base`), strings into `.debug_str` through
//! `.debug_str_offsets` (`strx`), and range lists are in `.debug_rnglists`
//! (DWARF 5) or `.debug_ranges` (before). In a relocatable object every one of
//! those sections has its relocations applied first, as for line tables.
//! 64-bit DWARF is not supported.

use crate::lines::Relocated;
use crate::{err, Elf, Error};
use std::collections::HashSet;

/// One compile unit: where it is and what its unit DIE says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompileUnit {
    /// The offset of the unit's header in `.debug_info`.
    pub offset: u64,
    /// The offset of the next unit (one past this one's end).
    pub next: u64,
    pub version: u16,
    /// `DW_AT_stmt_list`: the offset of its line table in `.debug_line`.
    pub stmt_list: Option<u64>,
    /// `DW_AT_comp_dir`.
    pub comp_dir: Option<String>,
    /// The address ranges of its code, `[start, end)`, from the unit DIE.
    pub ranges: Vec<(u64, u64)>,
}

/// DWARF 5 unit types (`DW_UT_*`) that are not compile units.
const UT_TYPE: u8 = 0x02;
const UT_SKELETON: u8 = 0x04;
const UT_SPLIT_COMPILE: u8 = 0x05;
const UT_SPLIT_TYPE: u8 = 0x06;

/// The attributes read (`DW_AT_*`).
const AT_STMT_LIST: u64 = 0x10;
const AT_LOW_PC: u64 = 0x11;
const AT_HIGH_PC: u64 = 0x12;
const AT_COMP_DIR: u64 = 0x1b;
const AT_ENTRY_PC: u64 = 0x52;
const AT_RANGES: u64 = 0x55;
const AT_STR_OFFSETS_BASE: u64 = 0x72;
const AT_ADDR_BASE: u64 = 0x73;
const AT_RNGLISTS_BASE: u64 = 0x74;

/// The forms (`DW_FORM_*`).
const FORM_ADDR: u64 = 0x01;
const FORM_BLOCK2: u64 = 0x03;
const FORM_BLOCK4: u64 = 0x04;
const FORM_DATA2: u64 = 0x05;
const FORM_DATA4: u64 = 0x06;
const FORM_DATA8: u64 = 0x07;
const FORM_STRING: u64 = 0x08;
const FORM_BLOCK: u64 = 0x09;
const FORM_BLOCK1: u64 = 0x0a;
const FORM_DATA1: u64 = 0x0b;
const FORM_FLAG: u64 = 0x0c;
const FORM_SDATA: u64 = 0x0d;
const FORM_STRP: u64 = 0x0e;
const FORM_UDATA: u64 = 0x0f;
const FORM_REF_ADDR: u64 = 0x10;
const FORM_REF1: u64 = 0x11;
const FORM_REF2: u64 = 0x12;
const FORM_REF4: u64 = 0x13;
const FORM_REF8: u64 = 0x14;
const FORM_REF_UDATA: u64 = 0x15;
const FORM_INDIRECT: u64 = 0x16;
const FORM_SEC_OFFSET: u64 = 0x17;
const FORM_EXPRLOC: u64 = 0x18;
const FORM_FLAG_PRESENT: u64 = 0x19;
const FORM_STRX: u64 = 0x1a;
const FORM_ADDRX: u64 = 0x1b;
const FORM_REF_SUP4: u64 = 0x1c;
const FORM_STRP_SUP: u64 = 0x1d;
const FORM_DATA16: u64 = 0x1e;
const FORM_LINE_STRP: u64 = 0x1f;
const FORM_REF_SIG8: u64 = 0x20;
const FORM_IMPLICIT_CONST: u64 = 0x21;
const FORM_LOCLISTX: u64 = 0x22;
const FORM_RNGLISTX: u64 = 0x23;
const FORM_REF_SUP8: u64 = 0x24;
const FORM_STRX1: u64 = 0x25;
const FORM_STRX4: u64 = 0x28;
const FORM_ADDRX1: u64 = 0x29;
const FORM_ADDRX4: u64 = 0x2c;
const FORM_GNU_ADDR_INDEX: u64 = 0x1f01;
const FORM_GNU_STR_INDEX: u64 = 0x1f02;
const FORM_GNU_REF_ALT: u64 = 0x1f20;
const FORM_GNU_STRP_ALT: u64 = 0x1f21;

/// Range list entries (`DW_RLE_*`).
const RLE_END_OF_LIST: u8 = 0;
const RLE_BASE_ADDRESSX: u8 = 1;
const RLE_STARTX_ENDX: u8 = 2;
const RLE_STARTX_LENGTH: u8 = 3;
const RLE_OFFSET_PAIR: u8 = 4;
const RLE_BASE_ADDRESS: u8 = 5;
const RLE_START_END: u8 = 6;
const RLE_START_LENGTH: u8 = 7;

fn truncated<T>(what: &str) -> Result<T, Error> {
    err(format!("debug info: {what} truncated"))
}

/// A little-endian reader over one section.
struct Reader<'a> {
    data: &'a [u8],
    at: usize,
    what: &'static str,
}

impl<'a> Reader<'a> {
    fn new(data: &'a [u8], at: u64, what: &'static str) -> Result<Reader<'a>, Error> {
        match usize::try_from(at) {
            Ok(at) if at <= data.len() => Ok(Reader { data, at, what }),
            _ => err(format!("debug info: offset {at:#x} past the end of {what}")),
        }
    }
    fn bytes(&mut self, n: u64) -> Result<&'a [u8], Error> {
        let b = usize::try_from(n)
            .ok()
            .and_then(|n| self.at.checked_add(n))
            .and_then(|end| self.data.get(self.at..end));
        match b {
            Some(b) => {
                self.at += b.len();
                Ok(b)
            }
            None => truncated(self.what),
        }
    }
    /// An unsigned number of `n` bytes (1 to 8).
    fn uint(&mut self, n: u64) -> Result<u64, Error> {
        let mut v = [0; 8];
        let b = self.bytes(n)?;
        v[..b.len()].copy_from_slice(b);
        Ok(u64::from_le_bytes(v))
    }
    fn u8(&mut self) -> Result<u8, Error> {
        Ok(self.bytes(1)?[0])
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
        let rest = self.data.get(self.at..).unwrap_or_default();
        let Some(end) = rest.iter().position(|&b| b == 0) else {
            return err(format!("debug info: unterminated string in {}", self.what));
        };
        self.at += end + 1;
        Ok(String::from_utf8_lossy(&rest[..end]).into_owned())
    }
}

/// An attribute's value, by the class of its form.
#[derive(Clone, Debug)]
enum Value {
    /// `addr`: an address.
    Addr(u64),
    /// `addrx*`: an index into `.debug_addr`.
    AddrIndex(u64),
    /// `data*`, `udata`, `flag*`, `implicit_const`: a constant (`data4` and
    /// `data8` may also be section offsets, as in DWARF 3).
    Const { value: u64, form: u64 },
    /// `sdata`: a signed constant, never an offset or an unsigned value.
    Signed,
    /// `sec_offset`, `strp`, `line_strp`: an offset into another section.
    Offset { value: u64, form: u64 },
    /// `string`: the string itself.
    Str(String),
    /// `strx*`: an index into `.debug_str_offsets`.
    StrIndex(u64),
    /// `rnglistx`: an index into the unit's range list offsets.
    RnglistIndex(u64),
    /// Anything else (references, blocks, expressions, ...).
    Other,
}

impl Value {
    /// LLVM's `getAsSectionOffset`: `sec_offset`, and `data4`, `data8`,
    /// `strp` and `line_strp`, which also are offsets.
    fn section_offset(&self) -> Option<u64> {
        match *self {
            Value::Offset { value, .. } => Some(value),
            Value::Const { value, form } if form == FORM_DATA4 || form == FORM_DATA8 => Some(value),
            _ => None,
        }
    }
    /// LLVM's `getAsUnsignedConstant`: a constant or a flag, not `sdata`.
    fn unsigned(&self) -> Option<u64> {
        match *self {
            Value::Const { value, .. } => Some(value),
            _ => None,
        }
    }
}

/// Reads one attribute value of form `form`.
fn read_value(
    r: &mut Reader,
    form: u64,
    implicit: i64,
    version: u16,
    addr_size: u64,
) -> Result<Value, Error> {
    let constant = |value, form| Value::Const { value, form };
    Ok(match form {
        FORM_ADDR => Value::Addr(r.uint(addr_size)?),
        FORM_ADDRX | FORM_GNU_ADDR_INDEX => Value::AddrIndex(r.uleb()?),
        FORM_ADDRX1..=FORM_ADDRX4 => Value::AddrIndex(r.uint(form - FORM_ADDRX1 + 1)?),
        FORM_DATA1 | FORM_FLAG => constant(r.uint(1)?, form),
        FORM_DATA2 => constant(r.uint(2)?, form),
        FORM_DATA4 => constant(r.uint(4)?, form),
        FORM_DATA8 => constant(r.uint(8)?, form),
        FORM_UDATA => constant(r.uleb()?, form),
        FORM_FLAG_PRESENT => constant(1, form),
        FORM_IMPLICIT_CONST => constant(implicit as u64, form),
        FORM_SDATA => {
            r.sleb()?;
            Value::Signed
        }
        FORM_SEC_OFFSET | FORM_STRP | FORM_LINE_STRP => Value::Offset {
            value: r.uint(4)?,
            form,
        },
        FORM_STRING => Value::Str(r.cstr()?),
        FORM_STRX | FORM_GNU_STR_INDEX => Value::StrIndex(r.uleb()?),
        FORM_STRX1..=FORM_STRX4 => Value::StrIndex(r.uint(form - FORM_STRX1 + 1)?),
        FORM_RNGLISTX => Value::RnglistIndex(r.uleb()?),
        FORM_LOCLISTX | FORM_REF_UDATA => {
            r.uleb()?;
            Value::Other
        }
        FORM_REF1 => skip(r, 1)?,
        FORM_REF2 => skip(r, 2)?,
        FORM_REF4 | FORM_REF_SUP4 | FORM_STRP_SUP | FORM_GNU_REF_ALT | FORM_GNU_STRP_ALT => skip(r, 4)?,
        FORM_REF8 | FORM_REF_SIG8 | FORM_REF_SUP8 => skip(r, 8)?,
        FORM_DATA16 => skip(r, 16)?,
        // DWARF 2's DW_FORM_ref_addr is an address; later, an offset.
        FORM_REF_ADDR => skip(r, if version == 2 { addr_size } else { 4 })?,
        FORM_BLOCK1 => {
            let n = r.uint(1)?;
            skip(r, n)?
        }
        FORM_BLOCK2 => {
            let n = r.uint(2)?;
            skip(r, n)?
        }
        FORM_BLOCK4 => {
            let n = r.uint(4)?;
            skip(r, n)?
        }
        FORM_BLOCK | FORM_EXPRLOC => {
            let n = r.uleb()?;
            skip(r, n)?
        }
        FORM_INDIRECT => {
            let form = r.uleb()?;
            if form == FORM_INDIRECT {
                return err("debug info: DW_FORM_indirect of DW_FORM_indirect");
            }
            // An indirect implicit_const has no value to read.
            read_value(r, form, 0, version, addr_size)?
        }
        _ => return err(format!("debug info: unsupported form {form:#x}")),
    })
}

fn skip(r: &mut Reader, n: u64) -> Result<Value, Error> {
    r.bytes(n)?;
    Ok(Value::Other)
}

/// The attributes of abbreviation `code` in the table at `offset` of
/// `.debug_abbrev`: (attribute, form, implicit constant).
fn abbreviation(abbrev: &[u8], offset: u64, code: u64) -> Result<Vec<(u64, u64, i64)>, Error> {
    let mut r = Reader::new(abbrev, offset, ".debug_abbrev")?;
    loop {
        let c = r.uleb()?;
        if c == 0 {
            return err(format!("debug info: no abbreviation {code}"));
        }
        r.uleb()?; // the tag
        r.u8()?; // has children
        let mut attrs = Vec::new();
        loop {
            let (at, form) = (r.uleb()?, r.uleb()?);
            if (at, form) == (0, 0) {
                break;
            }
            let implicit = if form == FORM_IMPLICIT_CONST { r.sleb()? } else { 0 };
            attrs.push((at, form, implicit));
        }
        if c == code {
            return Ok(attrs);
        }
    }
}

/// The sections a unit's values point into.
struct Sections {
    info: Relocated,
    abbrev: Vec<u8>,
    addr: Option<Relocated>,
    str: Vec<u8>,
    line_str: Vec<u8>,
    str_offsets: Option<Relocated>,
    rnglists: Option<Relocated>,
    ranges: Option<Relocated>,
}

/// A string at `off` of a string section.
fn string_at(sec: &[u8], off: u64) -> Option<String> {
    let rest = sec.get(usize::try_from(off).ok()?..)?;
    let end = rest.iter().position(|&b| b == 0)?;
    Some(String::from_utf8_lossy(&rest[..end]).into_owned())
}

/// What a unit needs to resolve its values: the address size and the bases
/// of its indexed sections.
struct Unit<'a> {
    s: &'a Sections,
    version: u16,
    addr_size: u64,
    addr_base: Option<u64>,
    str_offsets_base: Option<u64>,
    rnglists_base: u64,
}

impl Unit<'_> {
    /// The largest address, which marks a discarded one (LLVM's tombstone).
    fn tombstone(&self) -> u64 {
        if self.addr_size == 4 {
            u32::MAX.into()
        } else {
            u64::MAX
        }
    }
    /// Address `index` of `.debug_addr` (LLVM's `getAddrOffsetSectionItem`).
    fn pooled(&self, index: u64) -> Option<u64> {
        let addr = self.s.addr.as_ref()?;
        let at = index.checked_mul(self.addr_size)?.checked_add(self.addr_base?)?;
        Reader::new(&addr.data, at, ".debug_addr")
            .ok()?
            .uint(self.addr_size)
            .ok()
    }
    /// LLVM's `getAsAddress`.
    fn address(&self, v: &Value) -> Option<u64> {
        match *v {
            Value::Addr(a) => Some(a),
            Value::AddrIndex(i) => self.pooled(i),
            _ => None,
        }
    }
    fn string(&self, v: &Value) -> Option<String> {
        match *v {
            Value::Str(ref s) => Some(s.clone()),
            Value::Offset { value, form } if form == FORM_STRP => string_at(&self.s.str, value),
            Value::Offset { value, form } if form == FORM_LINE_STRP => string_at(&self.s.line_str, value),
            Value::StrIndex(i) => {
                let offsets = self.s.str_offsets.as_ref()?;
                let at = i.checked_mul(4)?.checked_add(self.str_offsets_base?)?;
                let off = Reader::new(&offsets.data, at, ".debug_str_offsets")
                    .ok()?
                    .uint(4)
                    .ok()?;
                string_at(&self.s.str, off)
            }
            _ => None,
        }
    }

    /// The ranges of the range list at `offset` (of `.debug_rnglists` in
    /// DWARF 5, `.debug_ranges` before), as LLVM's `getAbsoluteRanges` makes
    /// them: offsets from `base`, the unit's base address, or from the last
    /// base address entry, and none from a tombstone.
    fn range_list(&self, offset: u64, mut base: Option<u64>) -> Result<Vec<(u64, u64)>, Error> {
        let tombstone = self.tombstone();
        let mut out = Vec::new();
        if self.version <= 4 {
            let Some(ranges) = &self.s.ranges else {
                return err("debug info: no .debug_ranges");
            };
            let mut r = Reader::new(&ranges.data, offset, ".debug_ranges")?;
            loop {
                let (start, end) = (r.uint(self.addr_size)?, r.uint(self.addr_size)?);
                if (start, end) == (0, 0) {
                    break;
                }
                if start == tombstone {
                    base = Some(end);
                    continue;
                }
                match base {
                    Some(b) if b == tombstone => {}
                    Some(b) => out.push((start.wrapping_add(b), end.wrapping_add(b))),
                    None => out.push((start, end)),
                }
            }
            return Ok(out);
        }
        let Some(rnglists) = &self.s.rnglists else {
            return err("debug info: no .debug_rnglists");
        };
        let mut r = Reader::new(&rnglists.data, offset, ".debug_rnglists")?;
        loop {
            let (start, end) = match r.u8()? {
                RLE_END_OF_LIST => break,
                RLE_BASE_ADDRESSX => {
                    let i = r.uleb()?;
                    base = Some(self.pooled(i).unwrap_or(i));
                    continue;
                }
                RLE_BASE_ADDRESS => {
                    base = Some(r.uint(self.addr_size)?);
                    continue;
                }
                RLE_OFFSET_PAIR => {
                    let (a, b) = (r.uleb()?, r.uleb()?);
                    match base {
                        Some(base) if base == tombstone => continue,
                        Some(base) => (a.wrapping_add(base), b.wrapping_add(base)),
                        None => (a, b),
                    }
                }
                RLE_START_END => (r.uint(self.addr_size)?, r.uint(self.addr_size)?),
                RLE_START_LENGTH => {
                    let start = r.uint(self.addr_size)?;
                    (start, start.wrapping_add(r.uleb()?))
                }
                RLE_STARTX_LENGTH => {
                    let start = self.pooled(r.uleb()?).unwrap_or(0);
                    (start, start.wrapping_add(r.uleb()?))
                }
                RLE_STARTX_ENDX => {
                    let (a, b) = (r.uleb()?, r.uleb()?);
                    (self.pooled(a).unwrap_or(0), self.pooled(b).unwrap_or(0))
                }
                k => return err(format!("debug info: unknown range list entry {k:#x}")),
            };
            if start != tombstone {
                out.push((start, end));
            }
        }
        Ok(out)
    }

    /// The offset of range list `index` (`rnglistx`): the unit's offset table
    /// starts at `DW_AT_rnglists_base`, after its list table's header, whose
    /// last field counts the entries.
    fn rnglist_offset(&self, index: u64) -> Option<u64> {
        let rnglists = self.s.rnglists.as_ref()?;
        let count_at = self.rnglists_base.checked_sub(4)?;
        let count = Reader::new(&rnglists.data, count_at, ".debug_rnglists")
            .ok()?
            .uint(4)
            .ok()?;
        if index >= count {
            return None;
        }
        let at = index.checked_mul(4)?.checked_add(self.rnglists_base)?;
        let off = Reader::new(&rnglists.data, at, ".debug_rnglists")
            .ok()?
            .uint(4)
            .ok()?;
        off.checked_add(self.rnglists_base)
    }
}

impl Elf<'_> {
    /// The compile units of `.debug_info` (none if there is no such
    /// section): for each, what its unit DIE says. Type units are left out,
    /// as they have no code.
    pub fn compile_units(&self) -> Result<Vec<CompileUnit>, Error> {
        let Some(info) = self.relocated(".debug_info")? else {
            return Ok(Vec::new());
        };
        let s = Sections {
            info,
            abbrev: self.data_of(".debug_abbrev")?.to_vec(),
            addr: self.relocated(".debug_addr")?,
            str: self.data_of(".debug_str")?.to_vec(),
            line_str: self.data_of(".debug_line_str")?.to_vec(),
            str_offsets: self.relocated(".debug_str_offsets")?,
            rnglists: self.relocated(".debug_rnglists")?,
            ranges: self.relocated(".debug_ranges")?,
        };
        let mut units = Vec::new();
        let mut at = 0u64;
        while at < s.info.data.len() as u64 {
            let mut r = Reader::new(&s.info.data, at, ".debug_info")?;
            let length = r.uint(4)?;
            if length >= 0xffff_fff0 {
                return err("debug info: 64-bit DWARF is not supported");
            }
            let next = at + 4 + length;
            if next > s.info.data.len() as u64 {
                return err("debug info: unit runs past the end of .debug_info");
            }
            let mut r = Reader {
                data: &s.info.data[..next as usize],
                at: r.at,
                what: ".debug_info",
            };
            let version = r.uint(2)? as u16;
            if !(2..=5).contains(&version) {
                return err(format!("debug info: DWARF version {version} is not supported"));
            }
            let (unit_type, addr_size, abbrev_offset) = if version >= 5 {
                let ty = r.u8()?;
                let size = r.u8()?;
                (ty, size, r.uint(4)?)
            } else {
                let offset = r.uint(4)?;
                (1, r.u8()?, offset)
            };
            match unit_type {
                UT_TYPE | UT_SPLIT_TYPE => {
                    at = next;
                    continue;
                }
                UT_SKELETON | UT_SPLIT_COMPILE => {
                    r.bytes(8)?; // dwo_id
                }
                _ => {}
            }
            if usize::from(addr_size) != self.class.addr_size() {
                return err(format!(
                    "debug info: {addr_size}-byte addresses in a file of {}-byte ones",
                    self.class.addr_size()
                ));
            }
            let addr_size = u64::from(addr_size);
            let code = r.uleb()?;
            let mut attrs = Vec::new();
            if code != 0 {
                for (attr, form, implicit) in abbreviation(&s.abbrev, abbrev_offset, code)? {
                    attrs.push((attr, read_value(&mut r, form, implicit, version, addr_size)?));
                }
            }
            let find = |a: u64| attrs.iter().find(|(at, _)| *at == a).map(|(_, v)| v);
            let unit = Unit {
                s: &s,
                version,
                addr_size,
                addr_base: find(AT_ADDR_BASE).and_then(Value::section_offset),
                str_offsets_base: find(AT_STR_OFFSETS_BASE).and_then(Value::section_offset),
                rnglists_base: find(AT_RNGLISTS_BASE)
                    .and_then(Value::section_offset)
                    .unwrap_or(0),
            };
            // LLVM's DWARFDie::getAddressRanges: low and high pc (an address,
            // or a constant added to the low one), else the range list.
            let low = find(AT_LOW_PC).and_then(|v| unit.address(v));
            let high = |low: u64| {
                let v = find(AT_HIGH_PC)?;
                unit.address(v)
                    .or_else(|| v.unsigned().map(|n| low.wrapping_add(n)))
            };
            // A range list's offsets are from the unit's base address.
            let base = find(AT_LOW_PC)
                .or(find(AT_ENTRY_PC))
                .and_then(|v| unit.address(v));
            let list = match find(AT_RANGES) {
                Some(&Value::RnglistIndex(i)) => unit.rnglist_offset(i),
                Some(v) => v.section_offset(),
                None => None,
            };
            let ranges = match low
                .filter(|&l| l != unit.tombstone())
                .and_then(|l| Some((l, high(l)?)))
            {
                Some(range) => vec![range],
                None => match list {
                    Some(offset) => unit.range_list(offset, base)?,
                    None => Vec::new(),
                },
            };
            units.push(CompileUnit {
                offset: at,
                next,
                version,
                stmt_list: find(AT_STMT_LIST).and_then(Value::section_offset),
                comp_dir: find(AT_COMP_DIR).and_then(|v| unit.string(v)),
                ranges,
            });
            at = next;
        }
        Ok(units)
    }

    /// Which compile unit each address belongs to, as LLVM's
    /// `DWARFDebugAranges` decides: the address ranges `.debug_aranges`
    /// gives each unit it lists, then those of the other units' DIEs.
    pub fn unit_ranges(&self, units: &[CompileUnit]) -> Result<UnitRanges, Error> {
        let mut endpoints = Vec::new();
        let mut listed = HashSet::new();
        let mut warnings = Vec::new();
        if let Some(relocated) = self.relocated(".debug_aranges")? {
            let raw = self.data_of(".debug_aranges")?;
            let mut at = 0u64;
            while at < raw.len() as u64 {
                match arange_set(&relocated.data, raw, at, &mut warnings) {
                    Ok(ArangeSet { unit, ranges, next }) => {
                        for (start, end) in ranges {
                            push_range(&mut endpoints, unit, start, end);
                        }
                        listed.insert(unit);
                        at = next;
                    }
                    // LLVM gives up on the section at a set it cannot read,
                    // keeping the sets before it.
                    Err(e) => {
                        warnings.push(e.0);
                        break;
                    }
                }
            }
        }
        for u in units {
            if listed.insert(u.offset) {
                for &(start, end) in &u.ranges {
                    push_range(&mut endpoints, u.offset, start, end);
                }
            }
        }
        let mut ranges = UnitRanges::construct(endpoints);
        ranges.warnings = warnings;
        Ok(ranges)
    }
}

/// One set of `.debug_aranges`: its unit, its ranges and where the next
/// set starts.
struct ArangeSet {
    unit: u64,
    ranges: Vec<(u64, u64)>,
    next: u64,
}

/// The set of `.debug_aranges` at `at`, as LLVM's `DWARFDebugArangeSet`
/// reads it. Addresses are read relocated (`data`), lengths not (`raw`): in
/// an object built with relaxation a length is an `R_RISCV_ADD64`/`SUB64`
/// pair over a 0, which LLVM reads as 0, the end of the set, and warns about
/// where it is not the last entry (then reads on).
fn arange_set(data: &[u8], raw: &[u8], at: u64, warnings: &mut Vec<String>) -> Result<ArangeSet, Error> {
    let mut r = Reader::new(data, at, ".debug_aranges")?;
    let length = r.uint(4)?;
    if length >= 0xffff_fff0 {
        return err("debug info: 64-bit DWARF is not supported");
    }
    let next = at + 4 + length;
    if next > data.len() as u64 {
        return err(format!(
            "the length of address range table at offset {at:#x} exceeds section size"
        ));
    }
    let version = r.uint(2)?;
    if version != 2 {
        return err(format!(
            "address range table at offset {at:#x} has unsupported version {version}"
        ));
    }
    let unit = r.uint(4)?;
    let (addr_size, seg_size) = (u64::from(r.u8()?), u64::from(r.u8()?));
    if !matches!(addr_size, 1 | 2 | 4 | 8) {
        return err(format!(
            "address range table at offset {at:#x} has unsupported address size: {addr_size}"
        ));
    }
    if seg_size != 0 {
        return err(format!(
            "non-zero segment selector size in address range table at offset {at:#x} is not supported"
        ));
    }
    // The tuples start at a multiple of their size from the set's start.
    let tuple = 2 * addr_size;
    let header = r.at as u64 - at;
    r.at = (at + header.div_ceil(tuple) * tuple) as usize;
    let mut lengths = Reader::new(raw, 0, ".debug_aranges")?;
    let mut ranges = Vec::new();
    while (r.at as u64) < next {
        let entry = r.at as u64;
        let start = r.uint(addr_size)?;
        lengths.at = r.at;
        let len = lengths.uint(addr_size)?;
        r.at = lengths.at;
        if (start, len) == (0, 0) {
            if r.at as u64 == next {
                return Ok(ArangeSet { unit, ranges, next });
            }
            warnings.push(format!(
                "address range table at offset {at:#x} has a premature terminator entry at offset {entry:#x}"
            ));
        }
        ranges.push((start, start.wrapping_add(len)));
    }
    err(format!(
        "address range table at offset {at:#x} is not terminated by null entry"
    ))
}

/// LLVM's `appendRange`: an empty or reversed range is none.
fn push_range(endpoints: &mut Vec<(u64, u64, bool)>, unit: u64, start: u64, end: u64) {
    if start < end {
        endpoints.push((start, unit, true));
        endpoints.push((end, unit, false));
    }
}

/// Address ranges and the compile unit (its offset) each belongs to, in
/// address order and not overlapping.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnitRanges {
    ranges: Vec<(u64, u64, u64)>,
    /// What LLVM warns of while reading `.debug_aranges` (its messages).
    pub warnings: Vec<String>,
}

impl UnitRanges {
    /// LLVM's `DWARFDebugAranges::construct`: where ranges overlap, the unit
    /// a range already running continues, else the lowest unit offset.
    fn construct(mut endpoints: Vec<(u64, u64, bool)>) -> UnitRanges {
        endpoints.sort_by_key(|e| e.0);
        let mut ranges: Vec<(u64, u64, u64)> = Vec::new();
        let mut valid: Vec<u64> = Vec::new(); // a multiset, sorted
        let mut prev = u64::MAX;
        for (addr, unit, start) in endpoints {
            if prev < addr && !valid.is_empty() {
                match ranges.last_mut() {
                    Some(last) if last.1 == prev && valid.contains(&last.2) => last.1 = addr,
                    _ => ranges.push((prev, addr, valid[0])),
                }
            }
            if start {
                let i = valid.partition_point(|&u| u <= unit);
                valid.insert(i, unit);
            } else if let Some(i) = valid.iter().position(|&u| u == unit) {
                valid.remove(i);
            }
            prev = addr;
        }
        UnitRanges {
            ranges,
            warnings: Vec::new(),
        }
    }

    /// The offset of the compile unit `addr` belongs to, if any.
    pub fn find(&self, addr: u64) -> Option<u64> {
        let i = self.ranges.partition_point(|r| r.1 <= addr);
        self.ranges.get(i).filter(|r| r.0 <= addr).map(|r| r.2)
    }
}

#[cfg(test)]
mod tests {
    use super::{push_range, UnitRanges};

    /// LLVM's DWARFDebugAranges: where units overlap, the one already
    /// running goes on, else the lowest offset; empty ranges are none.
    #[test]
    fn overlapping_units() {
        let mut e = Vec::new();
        push_range(&mut e, 0x40, 0x100, 0x200);
        push_range(&mut e, 0x10, 0x180, 0x300);
        push_range(&mut e, 0x20, 0x400, 0x400);
        push_range(&mut e, 0x30, 0x500, 0x480);
        let r = UnitRanges::construct(e);
        assert_eq!(r.ranges, [(0x100, 0x200, 0x40), (0x200, 0x300, 0x10)]);
        assert_eq!(r.find(0xff), None);
        assert_eq!(r.find(0x1ff), Some(0x40));
        assert_eq!(r.find(0x200), Some(0x10));
        assert_eq!(r.find(0x300), None);
        assert_eq!(r.find(0x400), None);
    }
}
