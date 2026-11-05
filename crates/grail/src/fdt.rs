//! Flattened device trees (DTB): the binary form of a device tree, as the Devicetree
//! Specification v0.4 (chapter 5) defines it and libfdt writes it.
//!
//! [`Writer`] builds one node by node, the way QEMU builds the `virt` machine's tree with
//! libfdt's sequential-write calls (`fdt_begin_node`, `fdt_property`, `fdt_end_node`),
//! and [`Writer::finish`] lays it out as `fdt_pack` leaves it:
//!
//! | Offset | What |
//! |---|---|
//! | 0 | the header: ten big-endian 32-bit words, 40 bytes |
//! | 40 | the memory reservation block: (address, size) pairs of 64-bit words, ending with (0, 0) |
//! | after it | the structure block: the nodes and their properties, as 32-bit tokens |
//! | after it | the strings block: every property name once, NUL-terminated |
//!
//! Every number in a tree is big-endian, whatever the machine's byte order. [`parse`]
//! reads a tree back into [`Node`]s, for tests and for whoever wants to look at the tree
//! a machine was given.

use std::collections::HashMap;

/// The header's first word.
pub const MAGIC: u32 = 0xd00d_feed;
/// The version written (`version`), and the oldest it is compatible with
/// (`last_comp_version`): version 17 is the current one, readable as 16.
pub const VERSION: u32 = 17;
pub const LAST_COMP_VERSION: u32 = 16;
/// The header's size in bytes.
pub const HEADER_SIZE: usize = 40;

/// The structure block's tokens.
const FDT_BEGIN_NODE: u32 = 1;
const FDT_END_NODE: u32 = 2;
const FDT_PROP: u32 = 3;
const FDT_NOP: u32 = 4;
const FDT_END: u32 = 9;

/// Builds a flattened device tree. Nodes nest: [`Writer::begin_node`] opens one inside
/// the open one (the first is the root, named ""), [`Writer::end_node`] closes it, and
/// the properties written in between are the open node's. A node's properties come
/// before its children, as the format requires.
#[derive(Debug, Default)]
pub struct Writer {
    reservations: Vec<(u64, u64)>,
    structure: Vec<u8>,
    strings: Vec<u8>,
    /// Where each property name is in `strings`: each is written once.
    names: HashMap<String, u32>,
    /// Nodes open now.
    depth: usize,
    /// Whether the open node has a child yet: no property may follow one.
    has_child: Vec<bool>,
    boot_cpuid: u32,
}

impl Writer {
    pub fn new() -> Self {
        Writer::default()
    }

    /// Adds an entry to the memory reservation block: memory the client must not use
    /// for its own allocations (`/memreserve/` in the source form).
    pub fn reserve(&mut self, address: u64, size: u64) {
        self.reservations.push((address, size));
    }

    /// The header's `boot_cpuid_phys`: the physical id of the boot CPU (0 by default).
    pub fn boot_cpuid(&mut self, id: u32) {
        self.boot_cpuid = id;
    }

    /// Opens a node named `name` (`""` for the root, `cpu@0`, ...).
    pub fn begin_node(&mut self, name: &str) {
        assert!(
            self.depth > 0 || self.structure.is_empty(),
            "a tree has one root node"
        );
        if let Some(last) = self.has_child.last_mut() {
            *last = true;
        }
        self.token(FDT_BEGIN_NODE);
        self.structure.extend_from_slice(name.as_bytes());
        self.structure.push(0);
        self.align();
        self.depth += 1;
        self.has_child.push(false);
    }

    /// Closes the node opened last.
    pub fn end_node(&mut self) {
        assert!(self.depth > 0, "no node is open");
        self.token(FDT_END_NODE);
        self.depth -= 1;
        self.has_child.pop();
    }

    /// A property of the open node, with `value` as its bytes.
    pub fn prop(&mut self, name: &str, value: &[u8]) {
        assert!(self.depth > 0, "a property belongs to a node");
        assert!(
            !self.has_child.last().copied().unwrap_or(false),
            "property {name} after a child node"
        );
        let off = match self.names.get(name) {
            Some(&off) => off,
            None => {
                let off = self.strings.len() as u32;
                self.strings.extend_from_slice(name.as_bytes());
                self.strings.push(0);
                self.names.insert(name.to_string(), off);
                off
            }
        };
        self.token(FDT_PROP);
        self.token(value.len() as u32);
        self.token(off);
        self.structure.extend_from_slice(value);
        self.align();
    }

    /// A property with no value, whose presence is what it says (`interrupt-controller;`).
    pub fn prop_empty(&mut self, name: &str) {
        self.prop(name, &[]);
    }

    /// A property of one 32-bit cell (`<1>`).
    pub fn prop_u32(&mut self, name: &str, value: u32) {
        self.prop_cells(name, &[value]);
    }

    /// A property of 32-bit cells (`<0x0 0x80000000>`).
    pub fn prop_cells(&mut self, name: &str, cells: &[u32]) {
        let bytes: Vec<u8> = cells.iter().flat_map(|c| c.to_be_bytes()).collect();
        self.prop(name, &bytes);
    }

    /// A property of 64-bit values, two cells each (an address and a size in a `reg`
    /// whose `#address-cells` and `#size-cells` are 2).
    pub fn prop_u64s(&mut self, name: &str, values: &[u64]) {
        let bytes: Vec<u8> = values.iter().flat_map(|v| v.to_be_bytes()).collect();
        self.prop(name, &bytes);
    }

    /// A string property (`"ns16550a"`), NUL-terminated.
    pub fn prop_str(&mut self, name: &str, value: &str) {
        self.prop_strs(name, &[value]);
    }

    /// A string-list property (`"sifive,clint0", "riscv,clint0"`): each string
    /// NUL-terminated, one after the other.
    pub fn prop_strs(&mut self, name: &str, values: &[&str]) {
        let mut bytes = Vec::new();
        for v in values {
            assert!(!v.contains('\0'), "a string in a device tree has no NUL");
            bytes.extend_from_slice(v.as_bytes());
            bytes.push(0);
        }
        self.prop(name, &bytes);
    }

    /// The tree's bytes: the header, the reservations, the structure block (closed with
    /// `FDT_END`) and the strings, each block 8-byte aligned (the reservations), 4-byte
    /// aligned (the structure) or unaligned (the strings), as libfdt packs a tree.
    pub fn finish(mut self) -> Vec<u8> {
        assert_eq!(self.depth, 0, "every node is closed");
        self.token(FDT_END);
        let off_mem_rsvmap = HEADER_SIZE;
        let rsvmap_len = 16 * (self.reservations.len() + 1);
        let off_dt_struct = off_mem_rsvmap + rsvmap_len;
        let off_dt_strings = off_dt_struct + self.structure.len();
        let total = off_dt_strings + self.strings.len();
        let mut out = Vec::with_capacity(total);
        for word in [
            MAGIC,
            total as u32,
            off_dt_struct as u32,
            off_dt_strings as u32,
            off_mem_rsvmap as u32,
            VERSION,
            LAST_COMP_VERSION,
            self.boot_cpuid,
            self.strings.len() as u32,
            self.structure.len() as u32,
        ] {
            out.extend_from_slice(&word.to_be_bytes());
        }
        for (address, size) in self.reservations.iter().chain([&(0, 0)]) {
            out.extend_from_slice(&address.to_be_bytes());
            out.extend_from_slice(&size.to_be_bytes());
        }
        out.extend_from_slice(&self.structure);
        out.extend_from_slice(&self.strings);
        debug_assert_eq!(out.len(), total);
        out
    }

    fn token(&mut self, word: u32) {
        self.structure.extend_from_slice(&word.to_be_bytes());
    }

    /// Pads the structure block to the next 4-byte boundary with zeros.
    fn align(&mut self) {
        while !self.structure.len().is_multiple_of(4) {
            self.structure.push(0);
        }
    }
}

/// A node of a parsed tree.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Node {
    /// Its name with its unit address (`cpu@0`); the root's is "".
    pub name: String,
    /// Its properties, in the tree's order.
    pub props: Vec<(String, Vec<u8>)>,
    pub children: Vec<Node>,
}

impl Node {
    /// The property `name`'s value.
    pub fn prop(&self, name: &str) -> Option<&[u8]> {
        self.props.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_slice())
    }

    /// The property `name` as 32-bit cells.
    pub fn cells(&self, name: &str) -> Option<Vec<u32>> {
        let v = self.prop(name)?;
        v.len().is_multiple_of(4).then(|| {
            v.chunks(4)
                .map(|c| u32::from_be_bytes([c[0], c[1], c[2], c[3]]))
                .collect()
        })
    }

    /// The property `name` as one 32-bit cell.
    pub fn u32(&self, name: &str) -> Option<u32> {
        match self.cells(name)?.as_slice() {
            [v] => Some(*v),
            _ => None,
        }
    }

    /// The property `name` as 64-bit values of two cells each.
    pub fn u64s(&self, name: &str) -> Option<Vec<u64>> {
        let cells = self.cells(name)?;
        cells.len().is_multiple_of(2).then(|| {
            cells
                .chunks(2)
                .map(|c| (u64::from(c[0]) << 32) | u64::from(c[1]))
                .collect()
        })
    }

    /// The property `name` as a list of strings.
    pub fn strs(&self, name: &str) -> Option<Vec<String>> {
        let v = self.prop(name)?;
        let (last, rest) = v.split_last()?;
        if *last != 0 {
            return None;
        }
        rest.split(|&b| b == 0)
            .map(|s| String::from_utf8(s.to_vec()).ok())
            .collect()
    }

    /// The property `name` as one string.
    pub fn str(&self, name: &str) -> Option<String> {
        match self.strs(name)?.as_slice() {
            [s] => Some(s.clone()),
            _ => None,
        }
    }

    /// The child named `name`.
    pub fn child(&self, name: &str) -> Option<&Node> {
        self.children.iter().find(|c| c.name == name)
    }

    /// The node at `path` below this one (`soc/serial@10000000`; "" is this node).
    pub fn find(&self, path: &str) -> Option<&Node> {
        path.split('/')
            .filter(|p| !p.is_empty())
            .try_fold(self, |node, name| node.child(name))
    }

    /// The node in this subtree whose `phandle` is `phandle`.
    pub fn by_phandle(&self, phandle: u32) -> Option<&Node> {
        if self.u32("phandle") == Some(phandle) {
            return Some(self);
        }
        self.children.iter().find_map(|c| c.by_phandle(phandle))
    }
}

/// A parsed tree: its root node, its memory reservations and its boot CPU.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tree {
    pub root: Node,
    pub reservations: Vec<(u64, u64)>,
    pub boot_cpuid: u32,
}

/// Reads a flattened device tree: an error if the header, a block or a token is wrong.
pub fn parse(blob: &[u8]) -> Result<Tree, String> {
    let word = |at: usize| -> Result<u32, String> {
        blob.get(at..at + 4)
            .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
            .ok_or_else(|| format!("the tree ends at {at:#x}, inside a word"))
    };
    if word(0)? != MAGIC {
        return Err("not a flattened device tree (bad magic)".into());
    }
    let total = word(4)? as usize;
    if total > blob.len() {
        return Err(format!("totalsize {total:#x} is past the end of the blob"));
    }
    let blob = &blob[..total];
    let (off_struct, off_strings, off_rsvmap) = (word(8)? as usize, word(12)? as usize, word(16)? as usize);
    if word(24)? > VERSION {
        return Err(format!("last_comp_version {} is newer than {VERSION}", word(24)?));
    }
    let boot_cpuid = word(28)?;
    let (size_strings, size_struct) = (word(32)? as usize, word(36)? as usize);
    let strings = blob
        .get(off_strings..off_strings + size_strings)
        .ok_or("the strings block is past the end of the tree")?;
    let structure = blob
        .get(off_struct..off_struct + size_struct)
        .ok_or("the structure block is past the end of the tree")?;

    let mut reservations = Vec::new();
    let mut at = off_rsvmap;
    loop {
        let address = (u64::from(word(at)?) << 32) | u64::from(word(at + 4)?);
        let size = (u64::from(word(at + 8)?) << 32) | u64::from(word(at + 12)?);
        if (address, size) == (0, 0) {
            break;
        }
        reservations.push((address, size));
        at += 16;
    }

    let sword = |at: usize| -> Result<u32, String> {
        structure
            .get(at..at + 4)
            .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
            .ok_or_else(|| "the structure block ends inside a token".to_string())
    };
    let align = |n: usize| n.div_ceil(4) * 4;
    let mut stack: Vec<Node> = Vec::new();
    let mut root = None;
    let mut at = 0;
    loop {
        let token = sword(at)?;
        at += 4;
        match token {
            FDT_BEGIN_NODE => {
                let rest = &structure[at..];
                let len = rest
                    .iter()
                    .position(|&b| b == 0)
                    .ok_or("a node name has no NUL")?;
                let name = String::from_utf8(rest[..len].to_vec()).map_err(|_| "a node name is not UTF-8")?;
                at = align(at + len + 1);
                if root.is_some() {
                    return Err("a second root node".into());
                }
                stack.push(Node {
                    name,
                    ..Node::default()
                });
            }
            FDT_END_NODE => {
                let node = stack.pop().ok_or("an FDT_END_NODE with no node open")?;
                match stack.last_mut() {
                    Some(parent) => parent.children.push(node),
                    None => root = Some(node),
                }
            }
            FDT_PROP => {
                let len = sword(at)? as usize;
                let nameoff = sword(at + 4)? as usize;
                at += 8;
                let value = structure
                    .get(at..at + len)
                    .ok_or("a property's value is past the structure block")?;
                at = align(at + len);
                let name = strings
                    .get(nameoff..)
                    .and_then(|s| s.iter().position(|&b| b == 0).map(|n| &s[..n]))
                    .ok_or("a property's name is not in the strings block")?;
                let name = String::from_utf8(name.to_vec()).map_err(|_| "a property name is not UTF-8")?;
                let node = stack.last_mut().ok_or("a property outside any node")?;
                if !node.children.is_empty() {
                    return Err(format!("property {name} after a child node"));
                }
                node.props.push((name, value.to_vec()));
            }
            FDT_NOP => {}
            FDT_END => break,
            t => return Err(format!("unknown token {t:#x} at {:#x}", at - 4)),
        }
    }
    if !stack.is_empty() {
        return Err("FDT_END with a node still open".into());
    }
    let root = root.ok_or("no root node")?;
    Ok(Tree {
        root,
        reservations,
        boot_cpuid,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tree_round_trips() {
        let mut w = Writer::new();
        w.reserve(0x8000_0000, 0x20_0000);
        w.boot_cpuid(0);
        w.begin_node("");
        w.prop_u32("#address-cells", 2);
        w.prop_u32("#size-cells", 2);
        w.prop_str("compatible", "camelot,grail");
        w.begin_node("memory@80000000");
        w.prop_str("device_type", "memory");
        w.prop_u64s("reg", &[0x8000_0000, 0x40_0000]);
        w.end_node();
        w.begin_node("soc");
        w.prop_empty("ranges");
        w.begin_node("clint@2000000");
        w.prop_strs("compatible", &["sifive,clint0", "riscv,clint0"]);
        w.prop_u32("phandle", 7);
        w.end_node();
        w.end_node();
        w.end_node();
        let blob = w.finish();

        assert_eq!(&blob[..4], &MAGIC.to_be_bytes());
        assert_eq!(u32::from_be_bytes(blob[4..8].try_into().unwrap()) as usize, blob.len());
        let tree = parse(&blob).unwrap();
        assert_eq!(tree.reservations, [(0x8000_0000, 0x20_0000)]);
        let root = &tree.root;
        assert_eq!(root.name, "");
        assert_eq!(root.u32("#address-cells"), Some(2));
        assert_eq!(root.str("compatible").as_deref(), Some("camelot,grail"));
        let mem = root.find("memory@80000000").unwrap();
        assert_eq!(mem.u64s("reg"), Some(vec![0x8000_0000, 0x40_0000]));
        let clint = root.find("soc/clint@2000000").unwrap();
        assert_eq!(
            clint.strs("compatible"),
            Some(vec!["sifive,clint0".into(), "riscv,clint0".into()])
        );
        assert_eq!(root.find("soc").unwrap().prop("ranges"), Some(&[][..]));
        assert_eq!(root.by_phandle(7).map(|n| n.name.as_str()), Some("clint@2000000"));
        assert!(root.find("soc/nothing").is_none());
    }

    #[test]
    fn the_layout_is_libfdts() {
        // The smallest tree: an empty root. Header, an empty reservation map (one
        // terminating pair), BEGIN_NODE "" (8 bytes), END_NODE, END, no strings.
        let mut w = Writer::new();
        w.begin_node("");
        w.end_node();
        let blob = w.finish();
        let words: Vec<u32> = blob
            .chunks(4)
            .map(|c| u32::from_be_bytes(c.try_into().unwrap()))
            .collect();
        assert_eq!(
            &words[..10],
            &[MAGIC, 72, 56, 72, 40, 17, 16, 0, 0, 16],
            "magic, totalsize, the offsets, the versions, the cpu, the sizes"
        );
        assert_eq!(&words[10..14], &[0; 4], "the reservations' end");
        assert_eq!(&words[14..], &[1, 0, 2, 9]);
        // A property's name is stored once however often it is used.
        let mut w = Writer::new();
        w.begin_node("");
        w.prop_u32("reg", 1);
        w.begin_node("a");
        w.prop_u32("reg", 2);
        w.end_node();
        w.end_node();
        let tree = parse(&w.finish()).unwrap();
        assert_eq!(tree.root.children[0].u32("reg"), Some(2));
    }

    #[test]
    fn bad_trees_are_refused() {
        assert!(parse(b"not a tree at all, no").is_err());
        let mut w = Writer::new();
        w.begin_node("");
        w.end_node();
        let mut blob = w.finish();
        let n = blob.len();
        blob[n - 4..].copy_from_slice(&0x77u32.to_be_bytes());
        assert!(parse(&blob).unwrap_err().contains("unknown token"));
        assert!(parse(&blob[..20]).is_err());
    }
}
