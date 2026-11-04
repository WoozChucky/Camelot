//! DWARF debugging information entries (`.debug_info`) as a tree, the way a
//! debugger reads them: every entry of every compile unit, with its tag, its
//! attributes and its children.
//!
//! [`crate::units`] reads only what finding a line needs (a unit's ranges, its
//! line table, its functions' names). A debugger needs the rest too: the
//! variables of each function and scope (`DW_TAG_variable`,
//! `DW_TAG_formal_parameter`) and where they live (`DW_AT_location`, a DWARF
//! expression), and the types (`DW_TAG_base_type`, `DW_TAG_pointer_type`,
//! `DW_TAG_structure_type`, ...) that `DW_AT_type` points at.
//!
//! Values come resolved, so a reader need not know forms: addresses from
//! `.debug_addr`, strings from `.debug_str` and `.debug_line_str` (and
//! `.debug_str_offsets`), references as offsets in `.debug_info` whether
//! they were unit-relative or not, and each entry's code ranges (low and high
//! pc, or a range list) as `[start, end)` pairs. In a relocatable object the
//! sections have their relocations applied first, as for line tables.

use crate::units::{find, Value};
use crate::{Elf, Error};

/// `DW_TAG_*`, `DW_AT_*`, `DW_ATE_*` and `DW_OP_*`, the constants a debugger
/// reads variables and types with (DWARF 5, section 7).
pub mod dw {
    pub const TAG_ARRAY_TYPE: u64 = 0x01;
    pub const TAG_ENUMERATION_TYPE: u64 = 0x04;
    pub const TAG_FORMAL_PARAMETER: u64 = 0x05;
    pub const TAG_LEXICAL_BLOCK: u64 = 0x0b;
    pub const TAG_MEMBER: u64 = 0x0d;
    pub const TAG_POINTER_TYPE: u64 = 0x0f;
    pub const TAG_COMPILE_UNIT: u64 = 0x11;
    pub const TAG_STRUCTURE_TYPE: u64 = 0x13;
    pub const TAG_SUBROUTINE_TYPE: u64 = 0x15;
    pub const TAG_TYPEDEF: u64 = 0x16;
    pub const TAG_UNION_TYPE: u64 = 0x17;
    pub const TAG_UNSPECIFIED_PARAMETERS: u64 = 0x18;
    pub const TAG_INLINED_SUBROUTINE: u64 = 0x1d;
    pub const TAG_SUBRANGE_TYPE: u64 = 0x21;
    pub const TAG_BASE_TYPE: u64 = 0x24;
    pub const TAG_CONST_TYPE: u64 = 0x26;
    pub const TAG_ENUMERATOR: u64 = 0x28;
    pub const TAG_SUBPROGRAM: u64 = 0x2e;
    pub const TAG_VARIABLE: u64 = 0x34;
    pub const TAG_VOLATILE_TYPE: u64 = 0x35;
    pub const TAG_RESTRICT_TYPE: u64 = 0x37;

    pub const AT_LOCATION: u64 = 0x02;
    pub const AT_NAME: u64 = 0x03;
    pub const AT_BYTE_SIZE: u64 = 0x0b;
    pub const AT_BIT_SIZE: u64 = 0x0d;
    pub const AT_STMT_LIST: u64 = 0x10;
    pub const AT_LOW_PC: u64 = 0x11;
    pub const AT_HIGH_PC: u64 = 0x12;
    pub const AT_LANGUAGE: u64 = 0x13;
    pub const AT_COMP_DIR: u64 = 0x1b;
    pub const AT_CONST_VALUE: u64 = 0x1c;
    pub const AT_PRODUCER: u64 = 0x25;
    pub const AT_PROTOTYPED: u64 = 0x27;
    pub const AT_UPPER_BOUND: u64 = 0x2f;
    pub const AT_COUNT: u64 = 0x37;
    pub const AT_DATA_MEMBER_LOCATION: u64 = 0x38;
    pub const AT_DECL_FILE: u64 = 0x3a;
    pub const AT_DECL_LINE: u64 = 0x3b;
    pub const AT_DECLARATION: u64 = 0x3c;
    pub const AT_ENCODING: u64 = 0x3e;
    pub const AT_EXTERNAL: u64 = 0x3f;
    pub const AT_FRAME_BASE: u64 = 0x40;
    pub const AT_SPECIFICATION: u64 = 0x47;
    pub const AT_TYPE: u64 = 0x49;
    pub const AT_RANGES: u64 = 0x55;
    pub const AT_DATA_BIT_OFFSET: u64 = 0x6b;

    pub const ATE_ADDRESS: u64 = 0x01;
    pub const ATE_BOOLEAN: u64 = 0x02;
    pub const ATE_FLOAT: u64 = 0x04;
    pub const ATE_SIGNED: u64 = 0x05;
    pub const ATE_SIGNED_CHAR: u64 = 0x06;
    pub const ATE_UNSIGNED: u64 = 0x07;
    pub const ATE_UNSIGNED_CHAR: u64 = 0x08;

    pub const OP_ADDR: u8 = 0x03;
    pub const OP_CONST1U: u8 = 0x08;
    pub const OP_PLUS_UCONST: u8 = 0x23;
    pub const OP_REG0: u8 = 0x50;
    pub const OP_REG31: u8 = 0x6f;
    pub const OP_BREG0: u8 = 0x70;
    pub const OP_BREG31: u8 = 0x8f;
    pub const OP_FBREG: u8 = 0x91;
    pub const OP_CALL_FRAME_CFA: u8 = 0x9c;
    pub const OP_STACK_VALUE: u8 = 0x9f;
}

/// An attribute's value, resolved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Attr {
    /// An address (`addr`, or `addrx` through `.debug_addr`).
    Addr(u64),
    /// A constant (`data*`, `udata`, a flag, `implicit_const`). `data`
    /// constants have no sign of their own: [`Attr::signed`] reads them as
    /// signed, as a constant of a signed type is.
    Unsigned { value: u64, size: u8 },
    /// `sdata`.
    Signed(i64),
    /// A string, wherever it was kept.
    Str(String),
    /// Another entry, by its offset in `.debug_info`.
    Ref(u64),
    /// A block or an expression (`exprloc`): its bytes.
    Block(Vec<u8>),
    /// An offset into another section (`sec_offset`).
    Offset(u64),
    /// Anything else.
    Other,
}

impl Attr {
    /// The value as an unsigned number.
    pub fn unsigned(&self) -> Option<u64> {
        match *self {
            Attr::Unsigned { value, .. } => Some(value),
            Attr::Signed(v) => Some(v as u64),
            _ => None,
        }
    }
    /// The value as a signed number: a `data` constant sign-extended from
    /// its size, `udata` as it is.
    pub fn signed(&self) -> Option<i64> {
        match *self {
            Attr::Unsigned { value, size } if (1..8).contains(&size) => {
                let shift = 64 - 8 * u32::from(size);
                Some(((value << shift) as i64) >> shift)
            }
            Attr::Unsigned { value, .. } => Some(value as i64),
            Attr::Signed(v) => Some(v),
            _ => None,
        }
    }
}

/// One debugging information entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Die {
    /// Its offset in `.debug_info`.
    pub offset: u64,
    pub tag: u64,
    pub attrs: Vec<(u64, Attr)>,
    /// Its code ranges, `[start, end)`, from low and high pc or a range list
    /// (none for an entry without code).
    pub ranges: Vec<(u64, u64)>,
    /// Its parent and its children, as indices in the unit's `dies`.
    pub parent: Option<usize>,
    pub children: Vec<usize>,
}

impl Die {
    /// The value of attribute `at`.
    pub fn attr(&self, at: u64) -> Option<&Attr> {
        self.attrs.iter().find(|(a, _)| *a == at).map(|(_, v)| v)
    }
    /// `DW_AT_name`.
    pub fn name(&self) -> Option<&str> {
        match self.attr(dw::AT_NAME)? {
            Attr::Str(s) => Some(s),
            _ => None,
        }
    }
    /// The entry attribute `at` refers to.
    pub fn reference(&self, at: u64) -> Option<u64> {
        match *self.attr(at)? {
            Attr::Ref(r) => Some(r),
            _ => None,
        }
    }
    /// Attribute `at` as an unsigned number.
    pub fn unsigned(&self, at: u64) -> Option<u64> {
        self.attr(at)?.unsigned()
    }
    /// Attribute `at`'s block or expression.
    pub fn block(&self, at: u64) -> Option<&[u8]> {
        match self.attr(at)? {
            Attr::Block(b) => Some(b),
            _ => None,
        }
    }
    /// Whether flag `at` is set.
    pub fn flag(&self, at: u64) -> bool {
        self.unsigned(at).is_some_and(|v| v != 0)
    }
}

/// One compile unit's entries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InfoUnit {
    /// The offset of its header in `.debug_info`.
    pub offset: u64,
    pub version: u16,
    /// The size of an address (4 on RV32, 8 on RV64).
    pub addr_size: u8,
    /// Its entries in order, the unit DIE first: a parent before its
    /// children, as they are in the section.
    pub dies: Vec<Die>,
    /// Its addresses in `.debug_addr` (from `DW_AT_addr_base`), which
    /// `DW_OP_addrx` in an expression indexes.
    pub addr_pool: Vec<u64>,
}

impl InfoUnit {
    /// The entry at `offset` of `.debug_info`, if it is in this unit.
    pub fn die_at(&self, offset: u64) -> Option<&Die> {
        Some(&self.dies[self.index_of(offset)?])
    }
    /// The index in `dies` of the entry at `offset` of `.debug_info`.
    pub fn index_of(&self, offset: u64) -> Option<usize> {
        self.dies.binary_search_by_key(&offset, |d| d.offset).ok()
    }
}

impl Elf<'_> {
    /// Every compile unit's entries (none if there is no `.debug_info`).
    /// Type units are left out. An entry that cannot be read ends its unit,
    /// those before it kept, as LLVM's reader does.
    pub fn debug_entries(&self) -> Result<Vec<InfoUnit>, Error> {
        let Some(s) = self.debug_sections()? else {
            return Ok(Vec::new());
        };
        let mut units = Vec::new();
        let mut at = 0u64;
        while at < s.info.data.len() as u64 {
            let Some(mut h) = self.unit_header(&s, at)? else {
                at = s.next_unit(at)?;
                continue;
            };
            let first = h.entry()?;
            let attrs = first.as_ref().map_or(&[][..], |d| &d.attrs[..]);
            let unit = crate::units::Unit::new(&s, h.version, h.addr_size, attrs);
            let mut dies: Vec<Die> = Vec::new();
            // The entries whose children are being read, innermost last.
            let mut open: Vec<usize> = Vec::new();
            let mut next = first;
            loop {
                match next {
                    Some(e) => {
                        let index = dies.len();
                        let parent = open.last().copied();
                        let value = |v: &Value| -> Attr {
                            match *v {
                                Value::Addr(_) | Value::AddrIndex(_) => {
                                    unit.address(v).map_or(Attr::Other, Attr::Addr)
                                }
                                Value::Const { value, form } => Attr::Unsigned {
                                    value,
                                    size: form_size(form),
                                },
                                Value::Signed(v) => Attr::Signed(v),
                                Value::Str(_) | Value::StrIndex(_) => {
                                    unit.string(v).map_or(Attr::Other, Attr::Str)
                                }
                                Value::Offset { value, .. } => match unit.string(v) {
                                    Some(s) => Attr::Str(s),
                                    None => Attr::Offset(value),
                                },
                                Value::UnitRef(off) => Attr::Ref(at + off),
                                Value::InfoRef(off) => Attr::Ref(off),
                                Value::Block(ref b) => Attr::Block(b.clone()),
                                Value::RnglistIndex(_) | Value::Other => Attr::Other,
                            }
                        };
                        let attrs = e.attrs.iter().map(|(a, v)| (*a, value(v))).collect();
                        let ranges = if find(&e.attrs, dw::AT_LOW_PC).is_some()
                            || find(&e.attrs, dw::AT_RANGES).is_some()
                        {
                            unit.ranges(&e.attrs).unwrap_or_default()
                        } else {
                            Vec::new()
                        };
                        dies.push(Die {
                            offset: e.offset,
                            tag: e.tag,
                            attrs,
                            ranges,
                            parent,
                            children: Vec::new(),
                        });
                        if let Some(p) = parent {
                            dies[p].children.push(index);
                        }
                        if e.children {
                            open.push(index);
                        }
                    }
                    None => {
                        open.pop();
                    }
                }
                if open.is_empty() || h.r.at as u64 >= h.next {
                    break;
                }
                next = match h.entry() {
                    Ok(e) => e,
                    Err(_) => break,
                };
            }
            units.push(InfoUnit {
                offset: at,
                version: h.version,
                addr_size: h.addr_size as u8,
                dies,
                addr_pool: unit.address_pool(),
            });
            at = h.next;
        }
        Ok(units)
    }
}

/// The size of a constant of form `form`, 0 for a variable-length one.
fn form_size(form: u64) -> u8 {
    match form {
        0x0b | 0x0c => 1, // data1, flag
        0x05 => 2,        // data2
        0x06 => 4,        // data4
        0x07 => 8,        // data8
        _ => 0,
    }
}
