//! Extent trees: how an inode maps its contents (`docs/camelot-fs.md`,
//! "Extent trees"), in ext4's on-disk format.
//!
//! A file's blocks are runs, *extents*: logical blocks `block` to `block +
//! len - 1` are disk blocks `start` to `start + len - 1`. The extents sit in
//! the leaves of a tree. Its root is the 60 bytes of the inode's `i_block`:
//! a 12-byte header and room for 4 entries. A file of more than 4 extents
//! moves them into *tree blocks* (a header and 84 entries each) and the root
//! points to those with index entries, up to [`MAX_DEPTH`] levels down.
//!
//! Every node is a header ([`Header`]) and its entries, 12 bytes each:
//! extents ([`Extent`]) in a leaf (depth 0), index entries ([`Index`])
//! above. The module reads trees ([`walk`] checks one completely and returns
//! its extents and tree blocks; [`lookup`] finds one block) and grows them
//! ([`append`], the format document's "Growing a file", which mkfs uses and
//! `kernel/fs.c` does the same way).

use crate::{put_u16, put_u32, u16_at, u32_at, Superblock, BSIZE};

/// The header's `eh_magic`.
pub const MAGIC: u16 = 0xf30a;
/// Bytes of a node's header, and of each entry.
pub const HEADER_SIZE: usize = 12;
pub const ENTRY_SIZE: usize = 12;
/// Bytes of the root: the inode's `i_block`.
pub const ROOT_SIZE: usize = HEADER_SIZE + ROOT_MAX as usize * ENTRY_SIZE;
/// Entries in the root and in a tree block.
pub const ROOT_MAX: u16 = 4;
pub const BLOCK_MAX: u16 = ((BSIZE - HEADER_SIZE) / ENTRY_SIZE) as u16;
/// The deepest a tree is: ext4's `EXT4_MAX_EXTENT_DEPTH`. 4 × 84^5 extents
/// are more than the 2^31 blocks of the largest filesystem.
pub const MAX_DEPTH: u16 = 5;
/// The longest extent. ext4 reads a longer `ee_len` as an uninitialized
/// extent, which CMFS does not have.
pub const MAX_LEN: u32 = 32768;

/// A node's header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    pub magic: u16,
    pub entries: u16,
    pub max: u16,
    pub depth: u16,
    pub generation: u32,
}

impl Header {
    pub fn decode(b: &[u8]) -> Header {
        Header {
            magic: u16_at(b, 0),
            entries: u16_at(b, 2),
            max: u16_at(b, 4),
            depth: u16_at(b, 6),
            generation: u32_at(b, 8),
        }
    }

    pub fn encode(&self, b: &mut [u8]) {
        put_u16(b, 0, self.magic);
        put_u16(b, 2, self.entries);
        put_u16(b, 4, self.max);
        put_u16(b, 6, self.depth);
        put_u32(b, 8, self.generation);
    }
}

/// A leaf entry: logical blocks `block..block + len` are disk blocks
/// `start..start + len`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Extent {
    pub block: u32,
    /// `ee_len`: 1 to [`MAX_LEN`] in a valid tree.
    pub len: u32,
    /// The 48-bit disk block (`ee_start_hi`, `ee_start_lo`).
    pub start: u64,
}

impl Extent {
    pub fn decode(b: &[u8]) -> Extent {
        Extent {
            block: u32_at(b, 0),
            len: u32::from(u16_at(b, 4)),
            start: (u64::from(u16_at(b, 6)) << 32) | u64::from(u32_at(b, 8)),
        }
    }

    /// The entry's bytes. `len` must fit `ee_len` (at most 65535; a valid
    /// tree has at most [`MAX_LEN`]) and `start` 48 bits.
    pub fn encode(&self, b: &mut [u8]) {
        put_u32(b, 0, self.block);
        put_u16(b, 4, self.len as u16);
        put_u16(b, 6, (self.start >> 32) as u16);
        put_u32(b, 8, self.start as u32);
    }

    /// The logical block after its last.
    pub fn end(&self) -> u64 {
        u64::from(self.block) + u64::from(self.len)
    }
}

/// An index entry: the logical blocks from `block` on are under `child`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Index {
    pub block: u32,
    /// The 48-bit tree block (`ei_leaf_lo`, `ei_leaf_hi`).
    pub child: u64,
    /// `ei_unused`: 0 in a valid tree.
    pub unused: u16,
}

impl Index {
    pub fn decode(b: &[u8]) -> Index {
        Index {
            block: u32_at(b, 0),
            child: (u64::from(u16_at(b, 8)) << 32) | u64::from(u32_at(b, 4)),
            unused: u16_at(b, 10),
        }
    }

    pub fn encode(&self, b: &mut [u8]) {
        put_u32(b, 0, self.block);
        put_u32(b, 4, self.child as u32);
        put_u16(b, 8, (self.child >> 32) as u16);
        put_u16(b, 10, self.unused);
    }
}

/// The root of an empty file: a leaf header with no entries.
pub fn empty_root() -> [u8; ROOT_SIZE] {
    let mut r = [0; ROOT_SIZE];
    Header {
        magic: MAGIC,
        entries: 0,
        max: ROOT_MAX,
        depth: 0,
        generation: 0,
    }
    .encode(&mut r);
    r
}

/// Where entry `i` of a node starts.
fn entry_at(i: usize) -> usize {
    HEADER_SIZE + i * ENTRY_SIZE
}

/// What a checked tree maps.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Map {
    /// The extents in logical order: the first starts at 0, each where the
    /// one before ends.
    pub extents: Vec<Extent>,
    /// The tree blocks, in the order the walk met them (parents first).
    pub tree_blocks: Vec<u32>,
}

impl Map {
    /// Logical blocks mapped: where the last extent ends.
    pub fn blocks(&self) -> u64 {
        self.extents.last().map_or(0, Extent::end)
    }

    /// Every disk block of the contents, in logical order.
    pub fn content_blocks(&self) -> impl Iterator<Item = u32> + '_ {
        self.extents
            .iter()
            .flat_map(|e| (0..e.len).map(move |i| (e.start + u64::from(i)) as u32))
    }
}

/// Checks the tree rooted at `root` (an inode's `i_block`) against every rule
/// of "Extent trees" but the ones about other inodes (a block shared with
/// another one): the headers, the order and continuity of the entries, the
/// lengths, the depths, every block in the data region, the bytes after a
/// tree block's last entry. Returns what it maps; whether that is as many
/// blocks as the inode's size says is the caller's to check. A corrupt tree
/// is an error, never a read outside the image: every block is checked
/// before it is read, and the depth goes down at every level.
pub fn walk(image: &[u8], sb: &Superblock, root: &[u8]) -> Result<Map, String> {
    let mut map = Map::default();
    let h = Header::decode(root);
    if h.depth > MAX_DEPTH {
        return Err(format!(
            "the tree is {} levels deep, more than {MAX_DEPTH}",
            h.depth
        ));
    }
    walk_node(image, sb, root, ROOT_MAX, h.depth, 0, true, &mut map)?;
    Ok(map)
}

/// Checks one node, which must be at `depth` and start at logical block
/// `first`, and walks its children.
#[allow(clippy::too_many_arguments)]
fn walk_node(
    image: &[u8],
    sb: &Superblock,
    node: &[u8],
    max: u16,
    depth: u16,
    first: u64,
    is_root: bool,
    map: &mut Map,
) -> Result<(), String> {
    let what = if is_root { "the root" } else { "a tree block" };
    let h = Header::decode(node);
    if h.magic != MAGIC {
        return Err(format!("{what}'s magic is {:#06x}, not {MAGIC:#06x}", h.magic));
    }
    if h.max != max {
        return Err(format!("{what}'s eh_max is {}, not {max}", h.max));
    }
    if h.entries > h.max {
        return Err(format!("{what} has {} entries, room for {}", h.entries, h.max));
    }
    if h.depth != depth {
        return Err(format!("{what} is at depth {}, its parent says {depth}", h.depth));
    }
    if h.generation != 0 {
        return Err(format!("{what}'s eh_generation is {}, not 0", h.generation));
    }
    if h.entries == 0 && !(is_root && depth == 0) {
        return Err(format!("{what} at depth {depth} has no entries"));
    }
    let n = usize::from(h.entries);
    if !is_root {
        if let Some(i) = node[entry_at(n)..].iter().position(|&x| x != 0) {
            return Err(format!(
                "a tree block's byte {} past its {n} entries is not 0",
                entry_at(n) + i
            ));
        }
    }
    let in_data = |b: u64| b < u64::from(u32::MAX) && sb.is_data(b as u32);
    if depth == 0 {
        for i in 0..n {
            let e = Extent::decode(&node[entry_at(i)..]);
            let want = map.blocks();
            let want = if i == 0 { first } else { want };
            if u64::from(e.block) != want {
                return Err(format!(
                    "an extent starts at logical block {}, where {want} should",
                    e.block
                ));
            }
            if e.len == 0 || e.len > MAX_LEN {
                return Err(format!(
                    "the extent at logical block {} is {} blocks long (1 to {MAX_LEN})",
                    e.block, e.len
                ));
            }
            if e.end() > 1 << 32 {
                return Err(format!(
                    "the extent at logical block {} runs past logical block 2^32",
                    e.block
                ));
            }
            if !in_data(e.start) || !in_data(e.start + u64::from(e.len) - 1) {
                return Err(format!(
                    "the extent at logical block {} maps disk blocks {} to {}, outside the data region",
                    e.block,
                    e.start,
                    e.start + u64::from(e.len) - 1
                ));
            }
            map.extents.push(e);
        }
        return Ok(());
    }
    let entries: Vec<Index> = (0..n).map(|i| Index::decode(&node[entry_at(i)..])).collect();
    for (i, ix) in entries.iter().enumerate() {
        if ix.unused != 0 {
            return Err(format!(
                "the index entry for logical block {} has ei_unused {}",
                ix.block, ix.unused
            ));
        }
        let start = if i == 0 { first } else { map.blocks() };
        if u64::from(ix.block) != start {
            return Err(format!(
                "an index entry says its child starts at logical block {}, where {start} should",
                ix.block
            ));
        }
        if !in_data(ix.child) {
            return Err(format!(
                "the index entry for logical block {} points to block {}, outside the data region",
                ix.block, ix.child
            ));
        }
        let child = ix.child as u32;
        map.tree_blocks.push(child);
        let b = crate::block(image, child);
        walk_node(image, sb, b, BLOCK_MAX, depth - 1, start, false, map)?;
    }
    Ok(())
}

/// The disk block holding logical block `k`, if the tree maps it. The tree
/// must be valid ([`walk`]); a pointer outside the image is an error rather
/// than a panic, so a damaged one fails cleanly.
pub fn lookup(image: &[u8], root: &[u8], k: u32) -> Result<Option<u32>, String> {
    let mut node = root.to_vec();
    loop {
        let h = Header::decode(&node);
        if h.magic != MAGIC {
            return Err(format!("an extent node's magic is {:#06x}", h.magic));
        }
        let n = usize::from(h.entries.min(BLOCK_MAX));
        if h.depth == 0 {
            for i in (0..n).rev() {
                let e = Extent::decode(&node[entry_at(i)..]);
                if e.block <= k {
                    return Ok((u64::from(k) < e.end()).then(|| (e.start + u64::from(k - e.block)) as u32));
                }
            }
            return Ok(None);
        }
        let Some(ix) = (0..n)
            .rev()
            .map(|i| Index::decode(&node[entry_at(i)..]))
            .find(|ix| ix.block <= k)
        else {
            return Ok(None);
        };
        let at = ix.child as usize * BSIZE;
        if ix.child >= u64::from(u32::MAX) || at + BSIZE > image.len() {
            return Err(format!(
                "an index entry points to block {}, outside the disk",
                ix.child
            ));
        }
        node = image[at..at + BSIZE].to_vec();
    }
}

/// The blocks a writer of trees reads, writes and allocates.
pub trait Blocks {
    fn block(&self, n: u32) -> &[u8];
    fn block_mut(&mut self, n: u32) -> &mut [u8];
    /// A zeroed block marked in use, preferably `goal`.
    fn alloc(&mut self, goal: u32) -> Result<u32, String>;
    /// Gives back a block [`Blocks::alloc`] handed out.
    fn free(&mut self, b: u32);
}

/// A node on the rightmost path: the root, or a tree block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum At {
    Root,
    Block(u32),
}

fn node<'a>(dev: &'a dyn Blocks, root: &'a [u8], at: At) -> &'a [u8] {
    match at {
        At::Root => root,
        At::Block(b) => dev.block(b),
    }
}

fn node_mut<'a>(dev: &'a mut dyn Blocks, root: &'a mut [u8], at: At) -> &'a mut [u8] {
    match at {
        At::Root => root,
        At::Block(b) => dev.block_mut(b),
    }
}

/// Adds an entry (extent or index entry, already encoded) at the end of a
/// node with room.
fn push_entry(n: &mut [u8], entry: &[u8; ENTRY_SIZE]) {
    let mut h = Header::decode(n);
    let at = entry_at(usize::from(h.entries));
    n[at..at + ENTRY_SIZE].copy_from_slice(entry);
    h.entries += 1;
    h.encode(n);
}

fn extent_bytes(e: Extent) -> [u8; ENTRY_SIZE] {
    let mut b = [0; ENTRY_SIZE];
    e.encode(&mut b);
    b
}

fn index_bytes(block: u32, child: u32) -> [u8; ENTRY_SIZE] {
    let mut b = [0; ENTRY_SIZE];
    Index {
        block,
        child: u64::from(child),
        unused: 0,
    }
    .encode(&mut b);
    b
}

/// The disk block a new block of the file should go to: the one after its
/// last, or 0 (anywhere) for an empty file.
pub fn goal(dev: &dyn Blocks, root: &[u8]) -> u32 {
    last_extent(dev, root).map_or(0, |e| (e.start + u64::from(e.len)) as u32)
}

/// The rightmost path: the root, then the last child at each level down to
/// the leaf.
fn rightmost_path(dev: &dyn Blocks, root: &[u8]) -> Vec<At> {
    let mut path = vec![At::Root];
    loop {
        let n = node(dev, root, *path.last().unwrap());
        let h = Header::decode(n);
        if h.depth == 0 || h.entries == 0 {
            return path;
        }
        let ix = Index::decode(&n[entry_at(usize::from(h.entries) - 1)..]);
        path.push(At::Block(ix.child as u32));
    }
}

fn last_extent(dev: &dyn Blocks, root: &[u8]) -> Option<Extent> {
    let path = rightmost_path(dev, root);
    let n = node(dev, root, *path.last().unwrap());
    let h = Header::decode(n);
    (h.entries > 0).then(|| Extent::decode(&n[entry_at(usize::from(h.entries) - 1)..]))
}

/// Maps logical block `n`, the next after the file's last, to disk block `b`
/// in the valid tree rooted at `root` ("Growing a file"): the last extent
/// grows if `b` follows it, else a new extent goes at the end, in a new leaf
/// and new index nodes if the rightmost ones are full, and the tree grows a
/// level if even the root is. Every new tree block is allocated before
/// anything changes, so running out of blocks leaves the tree as it was.
pub fn append(dev: &mut dyn Blocks, root: &mut [u8; ROOT_SIZE], n: u32, b: u32) -> Result<(), String> {
    let path = rightmost_path(dev, root);
    let leaf = *path.last().unwrap();
    let lh = Header::decode(node(dev, root, leaf));
    if lh.entries > 0 {
        let at = entry_at(usize::from(lh.entries) - 1);
        let mut e = Extent::decode(&node(dev, root, leaf)[at..]);
        assert_eq!(e.end(), u64::from(n), "append: block {n} is not the next");
        // Step 1: the last extent grows.
        if e.start + u64::from(e.len) == u64::from(b) && e.len < MAX_LEN {
            e.len += 1;
            e.encode(&mut node_mut(dev, root, leaf)[at..]);
            return Ok(());
        }
    } else {
        assert_eq!(n, 0, "append: an empty tree's first block is 0");
    }
    let depth = Header::decode(root).depth;
    // Step 2: the lowest node on the path with room takes the new entry.
    let room = (0..path.len()).rev().find(|&l| {
        let h = Header::decode(node(dev, root, path[l]));
        h.entries < h.max
    });
    // Step 3: no room anywhere: the tree grows a level.
    let grow = room.is_none();
    if grow && depth == MAX_DEPTH {
        return Err(format!("the extent tree is full ({MAX_DEPTH} levels)"));
    }
    // After growing, the old root's copy (a level below the new root) has
    // room: it holds 4 entries of 84. The new entry goes there.
    let level = room.unwrap_or(1);
    let new_depth = if grow { depth + 1 } else { depth };
    // New nodes: the copy of the old root if the tree grows, then a chain of
    // one-entry nodes from the level below `level` down to a new leaf.
    let chain = usize::from(new_depth) - level;
    let wanted = chain + usize::from(grow);
    let mut new = Vec::with_capacity(wanted);
    for _ in 0..wanted {
        match dev.alloc(b) {
            Ok(x) => new.push(x),
            Err(e) => {
                for x in new {
                    dev.free(x);
                }
                return Err(e);
            }
        }
    }
    let mut new = new.into_iter();
    let receiver = if grow {
        let copy = new.next().unwrap();
        let mut h = Header::decode(root);
        let n_entries = usize::from(h.entries);
        let blk = dev.block_mut(copy);
        blk[HEADER_SIZE..entry_at(n_entries)].copy_from_slice(&root[HEADER_SIZE..entry_at(n_entries)]);
        h.max = BLOCK_MAX;
        h.encode(blk);
        // The root: one index entry, for everything so far (from logical
        // block 0), one level up.
        root[HEADER_SIZE..].fill(0);
        Header {
            magic: MAGIC,
            entries: 0,
            max: ROOT_MAX,
            depth: new_depth,
            generation: 0,
        }
        .encode(root);
        push_entry(root, &index_bytes(0, copy));
        At::Block(copy)
    } else {
        path[level]
    };
    // The chain, from the top down: index nodes of one entry, then the leaf.
    let chain_blocks: Vec<u32> = new.collect();
    for (i, &blk) in chain_blocks.iter().enumerate() {
        let d = usize::from(new_depth) - (level + 1 + i);
        let n_bytes = dev.block_mut(blk);
        Header {
            magic: MAGIC,
            entries: 0,
            max: BLOCK_MAX,
            depth: d as u16,
            generation: 0,
        }
        .encode(n_bytes);
        let entry = match chain_blocks.get(i + 1) {
            Some(&next) => index_bytes(n, next),
            None => extent_bytes(Extent {
                block: n,
                len: 1,
                start: u64::from(b),
            }),
        };
        push_entry(n_bytes, &entry);
    }
    let entry = match chain_blocks.first() {
        Some(&top) => index_bytes(n, top),
        None => extent_bytes(Extent {
            block: n,
            len: 1,
            start: u64::from(b),
        }),
    };
    push_entry(node_mut(dev, root, receiver), &entry);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Superblock, BITS_PER_BLOCK};

    /// A disk of `size` blocks for trees: blocks are handed out from the
    /// data region, skipping `gap` after each, so that a file's blocks are
    /// never contiguous when `gap` > 0.
    struct Disk {
        image: Vec<u8>,
        sb: Superblock,
        next: u32,
        gap: u32,
        used: Vec<bool>,
    }

    impl Disk {
        fn new(size: u32, gap: u32) -> Disk {
            let sb = Superblock::layout(size, 16, 2).unwrap();
            Disk {
                image: vec![0; size as usize * BSIZE],
                sb,
                next: sb.datastart,
                gap,
                used: vec![false; size as usize],
            }
        }
    }

    impl Blocks for Disk {
        fn block(&self, n: u32) -> &[u8] {
            crate::block(&self.image, n)
        }
        fn block_mut(&mut self, n: u32) -> &mut [u8] {
            crate::block_mut(&mut self.image, n)
        }
        fn alloc(&mut self, _goal: u32) -> Result<u32, String> {
            if self.next >= self.sb.size {
                return Err("full".into());
            }
            let b = self.next;
            self.next += 1 + self.gap;
            self.used[b as usize] = true;
            self.block_mut(b).fill(0);
            Ok(b)
        }
        fn free(&mut self, b: u32) {
            assert!(self.used[b as usize]);
            self.used[b as usize] = false;
        }
    }

    /// Grows a tree to `blocks` blocks and checks it after every step: it
    /// maps exactly those blocks, to the blocks handed out, in order.
    fn grow(disk: &mut Disk, blocks: u32) -> ([u8; ROOT_SIZE], Vec<u32>) {
        let mut root = empty_root();
        let mut data = Vec::new();
        for n in 0..blocks {
            let g = goal(disk, &root);
            let b = disk.alloc(g).unwrap();
            append(disk, &mut root, n, b).unwrap();
            data.push(b);
        }
        let map = walk(&disk.image, &disk.sb, &root).unwrap_or_else(|e| panic!("{blocks}: {e}"));
        assert_eq!(map.blocks(), u64::from(blocks));
        assert_eq!(map.content_blocks().collect::<Vec<_>>(), data);
        for (k, &b) in data.iter().enumerate() {
            assert_eq!(lookup(&disk.image, &root, k as u32), Ok(Some(b)));
        }
        assert_eq!(lookup(&disk.image, &root, blocks), Ok(None));
        (root, data)
    }

    #[test]
    fn layout_constants() {
        assert_eq!((ROOT_SIZE, BLOCK_MAX), (60, 84));
        // The deepest tree covers the largest filesystem, one less does not.
        assert!(4 * 84u64.pow(5) > 1 << 31);
        assert!(4 * 84u64.pow(4) < 1 << 31);
        let e = Extent {
            block: 0x0102_0304,
            len: 0x0506,
            start: 0x0708_090a_0b0c,
        };
        let mut b = [0; ENTRY_SIZE];
        e.encode(&mut b);
        assert_eq!(b, [4, 3, 2, 1, 6, 5, 8, 7, 0xc, 0xb, 0xa, 9]);
        assert_eq!(Extent::decode(&b), e);
        let ix = Index {
            block: 0x0102_0304,
            child: 0x0708_090a_0b0c,
            unused: 0,
        };
        ix.encode(&mut b);
        assert_eq!(b, [4, 3, 2, 1, 0xc, 0xb, 0xa, 9, 8, 7, 0, 0]);
        assert_eq!(Index::decode(&b), ix);
        assert_eq!(&empty_root()[..12], &[0x0a, 0xf3, 0, 0, 4, 0, 0, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn contiguous_blocks_are_one_extent() {
        let mut disk = Disk::new(2000, 0);
        let (root, _) = grow(&mut disk, 1500);
        assert_eq!(Header::decode(&root).entries, 1);
        assert_eq!(walk(&disk.image, &disk.sb, &root).unwrap().extents.len(), 1);
    }

    #[test]
    fn fragmented_files_grow_to_depth_one_and_two() {
        // Every block its own extent: 4 fit the root, 336 four leaves under
        // it. The 337th grows the tree: the root's 4 entries move to an
        // index block, which takes a new leaf, and later ones, under it.
        for (blocks, depth, tree_blocks) in [(4, 0, 0), (5, 1, 1), (336, 1, 4), (337, 2, 6), (700, 2, 10)] {
            let mut disk = Disk::new(2 * 700 + 200, 1);
            let (root, _) = grow(&mut disk, blocks);
            let map = walk(&disk.image, &disk.sb, &root).unwrap();
            assert_eq!(
                (
                    Header::decode(&root).depth,
                    map.tree_blocks.len(),
                    map.extents.len()
                ),
                (depth, tree_blocks, blocks as usize),
                "{blocks}"
            );
        }
    }

    #[test]
    fn running_out_of_blocks_mid_split_leaves_the_tree_as_it_was() {
        // 337 one-block extents need the tree to grow to depth 2: two new
        // blocks (the old root's copy, a new index node) and a new leaf.
        let mut disk = Disk::new(2 * 336 + 100, 1);
        let (mut root, _) = grow(&mut disk, 336);
        let before = (root, disk.image.clone());
        let b = disk.alloc(0).unwrap();
        // Room for one tree block only.
        disk.next = disk.sb.size - 1;
        let err = append(&mut disk, &mut root, 336, b).unwrap_err();
        assert_eq!(err, "full");
        assert_eq!(root, before.0);
        assert_eq!(walk(&disk.image, &disk.sb, &root).unwrap().blocks(), 336);
        // The block it got for itself went back.
        assert!(!disk.used[disk.sb.size as usize - 1]);
    }

    #[test]
    fn long_runs_split_at_the_longest_extent() {
        let mut disk = Disk::new(MAX_LEN + 2 * BITS_PER_BLOCK, 0);
        let (root, _) = grow(&mut disk, MAX_LEN + 1);
        let map = walk(&disk.image, &disk.sb, &root).unwrap();
        assert_eq!(
            map.extents.iter().map(|e| e.len).collect::<Vec<_>>(),
            [MAX_LEN, 1]
        );
    }

    #[test]
    fn broken_trees() {
        let mut disk = Disk::new(2000, 1);
        let (root, _) = grow(&mut disk, 90);
        // Two leaves under the root: 84 extents, then 6.
        let tree = walk(&disk.image, &disk.sb, &root).unwrap().tree_blocks;
        assert_eq!(tree.len(), 2);
        let leaf = tree[1];
        let sb = disk.sb;
        let check = |root: &[u8], image: &[u8], want: &str| {
            let err = walk(image, &sb, root).unwrap_err();
            assert!(err.contains(want), "want {want}: {err}");
        };
        let edit_root = |f: &dyn Fn(&mut [u8])| {
            let mut r = root;
            f(&mut r);
            r
        };
        let edit_leaf = |f: &dyn Fn(&mut [u8])| {
            let mut i = disk.image.clone();
            f(crate::block_mut(&mut i, leaf));
            i
        };
        let img = &disk.image;
        check(&edit_root(&|r| put_u16(r, 0, 0xf30b)), img, "magic");
        check(&edit_root(&|r| put_u16(r, 4, 5)), img, "eh_max is 5");
        check(&edit_root(&|r| put_u16(r, 2, 5)), img, "5 entries, room for 4");
        check(&edit_root(&|r| put_u16(r, 6, 6)), img, "6 levels deep");
        check(&edit_root(&|r| put_u32(r, 8, 1)), img, "eh_generation");
        // The second index entry starts somewhere else than its child.
        check(
            &edit_root(&|r| put_u32(r, entry_at(1), 7)),
            img,
            "index entry says",
        );
        check(&edit_root(&|r| put_u16(r, entry_at(0) + 10, 1)), img, "ei_unused");
        check(
            &edit_root(&|r| put_u32(r, entry_at(0) + 4, 1)),
            img,
            "outside the data region",
        );
        check(
            &root,
            &edit_leaf(&|b| put_u16(b, 6, 1)),
            "at depth 1, its parent says 0",
        );
        check(&root, &edit_leaf(&|b| put_u16(b, 4, 4)), "eh_max is 4, not 84");
        check(&root, &edit_leaf(&|b| put_u16(b, 2, 0)), "no entries");
        check(&root, &edit_leaf(&|b| b[1000] = 1), "byte 1000");
        check(
            &root,
            &edit_leaf(&|b| put_u16(b, entry_at(0) + 4, 0)),
            "0 blocks long",
        );
        check(
            &root,
            &edit_leaf(&|b| put_u16(b, entry_at(0) + 4, 32769)),
            "32769 blocks long",
        );
        check(
            &root,
            &edit_leaf(&|b| put_u32(b, entry_at(1), 3)),
            "starts at logical block 3",
        );
        check(
            &root,
            &edit_leaf(&|b| put_u16(b, entry_at(0) + 6, 1)),
            "outside the data region",
        );
        check(
            &root,
            &edit_leaf(&|b| put_u32(b, entry_at(0) + 8, 5000)),
            "outside the data region",
        );
        // An empty node below the root, and an empty index root.
        check(&edit_root(&|r| put_u16(r, 2, 0)), img, "depth 1 has no entries");
    }
}
