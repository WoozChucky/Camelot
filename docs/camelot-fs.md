# The Camelot filesystem (CMFS), version 2

The on-disk format of Camelot's filesystem, precise enough to write a second
implementation. There are two today, written from this document and checked
against each other:

- the kernel's (`kernel/log.c`, `kernel/fs.c`), which mounts disk 0 as the
  root at boot (`docs/excalibur-spec.md` section 10, "Kernel filesystem");
- the host's, the `camelot-fs` crate (`crates/camelot-fs`): `mkfs` builds
  images from a host directory (bootable ones too, see "Booting"), `fsck`
  checks every rule below and replays a
  pending log and frees orphaned inodes as a mount does, and `ls`/`cat` read
  files back. `cargo xtask fs` uses it to
  build the root image with the user programs.

The design is xv6's (MIT 6.1810), kept simple enough to learn from: a
superblock, a write-ahead log, a table of fixed-size inodes, a free-block
bitmap and data blocks; directories as files of fixed-size entries. Files
are mapped as Linux's ext4 maps them, by an **extent tree** in ext4's
on-disk format: runs of contiguous blocks, so a file written in one go is
one entry, and there is no limit worth the name on a file's size. Version 1
mapped a file with 12 direct pointers and one indirect block, as xv6 does,
which capped a file at 268 blocks; "Changes" says why that went.

## Conventions

- **Block**: 1024 bytes, two of the disk's 512-byte sectors (spec section
  9.6). Block `n` is sectors 2`n` and 2`n` + 1. Blocks are numbered from 0 at
  the start of the disk. This is the kernel buffer cache's `BSIZE`.
- **Integers** are little-endian, unsigned: `u16` (2 bytes), `u32` (4
  bytes) and `u64` (8 bytes), at the byte offsets given. There is no
  padding other than what the tables show.
- **Block numbers** are `u32` in the superblock and the log, and 48 bits in
  an extent tree, as ext4 has them (a `u16` high half and a `u32` low half).
  Since a filesystem is at most 2^31 blocks (below), the high half is
  always 0. Block 0 is the boot block, which nothing ever points to.
- A filesystem of `size` blocks occupies blocks 0 to `size` - 1 of the disk.
  The disk may be bigger; blocks past `size` are not the filesystem's.
- `ceil(a / b)` is `a / b` rounded up.

## Layout

```
block 0            boot block (not used by the filesystem)
block 1            superblock
logstart ...       log: header block, then nlog - 1 log slots
inodestart ...     inode table: ceil(ninodes / 8) blocks
bmapstart ...      free-block bitmap: ceil(size / 8192) blocks
datastart ...      data blocks, up to size - 1
```

The regions follow each other with no gaps, in this order. Every region's
position follows from three numbers, `size`, `ninodes` and `nlog`; the
superblock stores the results anyway, and a reader must check they agree.

## Superblock (block 1)

| Offset | Field | Value |
|---|---|---|
| 0 | `magic` (u32) | `0x53464d43`: the bytes `C M F S` |
| 4 | `version` (u32) | 2 |
| 8 | `size` (u32) | blocks in the filesystem, all regions included |
| 12 | `nblocks` (u32) | data blocks: `size` - `datastart` |
| 16 | `ninodes` (u32) | inode slots, numbered 0 to `ninodes` - 1 |
| 20 | `nlog` (u32) | blocks in the log, its header included |
| 24 | `logstart` (u32) | 2 |
| 28 | `inodestart` (u32) | `logstart` + `nlog` |
| 32 | `bmapstart` (u32) | `inodestart` + ceil(`ninodes` / 8) |
| 36 | `datastart` (u32) | `bmapstart` + ceil(`size` / 8192) |

Bytes 40 to 1023 are zero (readers check it: a superblock with other bytes
there is broken). Limits: 2 ≤ `nlog` ≤ 128 (the kernel writes only to a
filesystem whose log has room for one operation, 23 blocks or more; see
"Limits on transactions"); 2 ≤ `ninodes` ≤ 65536
(inode numbers must fit a directory entry's `u16`); `datastart` < `size`, so
there is at least one data block (the root directory's); `size` × 2 sectors
must fit on the disk. A filesystem is at most 2^31 blocks (the buffer cache's
limit, spec section 10, "Kernel disks"). A reader that finds a different
magic says the disk has no filesystem; one that finds a different version,
or fields that break these rules, must not mount it.

`camelot-fs mkfs` makes `nlog` = 45 (a header and 44 slots, the kernel's
`LOGSIZE`) and `ninodes` = 256 unless told otherwise, and `size` = 2048
blocks (2 MiB) by default.

## Inodes

The inode table holds `ninodes` inodes of 128 bytes, 8 per block: inode `i`
is at byte (`i` mod 8) × 128 of block `inodestart` + `i` / 8. The bytes of
the table's last block past inode `ninodes` - 1 are unused.

| Offset | Field | Meaning |
|---|---|---|
| 0 | `type` (u16) | 0 free, 1 directory, 2 regular file, 3 device |
| 2 | `major` (u16) | device number (type 3 only; 0 otherwise) |
| 4 | `minor` (u16) | device number (type 3 only; 0 otherwise) |
| 6 | `nlink` (u16) | directory entries naming this inode (below) |
| 8 | `size` (u64) | length of the contents in bytes |
| 16 | `i_block` (60 bytes) | the root of the extent tree mapping the contents (below), ext4's `i_block` |
| 76 | `flags` (u32) | bit 0, `TRUNCATING`: the inode's blocks are being freed (see "Truncation"). The other bits are 0 |
| 80 | reserved (48 bytes) | 0 |

- Inode 0 is never used: a directory entry with inode number 0 is a free
  slot. Its `type` is 0.
- Inode 1 is the root directory, `/`.
- A free inode has `type` 0; its other bytes mean nothing. Whoever allocates
  it (the kernel's `ialloc`, mkfs) zeroes all 128 bytes first. Freeing an
  inode (its last link gone, nobody using it) releases its blocks and sets
  `type` to 0.
- The reserved bytes are 0, and readers check them: they are where a later
  version puts what this one leaves out (timestamps, say), as ext4's inode
  grew beyond ext2's.
- **Size.** A file is at most 2^32 blocks (its logical block numbers are
  u32), 4 TiB, and in practice no bigger than the filesystem. The RV64
  kernel, whose `off_t` is 64-bit, reads and writes files up to that; the
  RV32 kernel, whose `off_t` is 32-bit, writes files of at most 2^31 - 1
  bytes and refuses to open bigger ones (docs/camelot-syscalls.md); the
  format does not have that limit.
- A **directory** or **regular file** maps its contents with the extent tree
  rooted in `i_block`: a header with up to 4 entries. An empty one has a
  header with no entries. A **device** has `size` 0, no blocks, and an
  `i_block` of zeros; `major` and `minor` say which device: the kernel opens
  major 1 as the console (`/dev/console` on the root image) and has no
  driver for any other (docs/camelot-syscalls.md). A directory's or file's
  `major` and `minor` are 0.

## Extent trees

An **extent** maps a run of a file's blocks to a run of disk blocks: logical
blocks `ee_block` to `ee_block` + `ee_len` - 1 are disk blocks `start` to
`start` + `ee_len` - 1. A file's extents sit in the leaves of a tree whose
root is the inode's `i_block`; when they don't fit the root, the root holds
index entries pointing to **tree blocks**, data blocks holding more
entries, up to five levels down. The format is ext4's
(`fs/ext4/ext4_extents.h` in Linux), byte for byte, without the checksum
tail ext4 adds with `metadata_csum`.

Every node, the root and each tree block, starts with a 12-byte header:

| Offset | Field | Meaning |
|---|---|---|
| 0 | `eh_magic` (u16) | 0xF30A |
| 2 | `eh_entries` (u16) | entries that follow, 0 to `eh_max` |
| 4 | `eh_max` (u16) | room for entries: 4 in the root, (1024 - 12) / 12 = 84 in a tree block |
| 6 | `eh_depth` (u16) | 0 for a leaf, whose entries are extents; above that, index entries pointing to nodes of depth `eh_depth` - 1. At most 5 |
| 8 | `eh_generation` (u32) | 0 |

The entries follow the header, 12 bytes each. A leaf's are extents:

| Offset | Field | Meaning |
|---|---|---|
| 0 | `ee_block` (u32) | the first logical block it maps |
| 4 | `ee_len` (u16) | blocks it maps, 1 to 32 768 |
| 6 | `ee_start_hi` (u16) | the first disk block's high 16 bits (0) |
| 8 | `ee_start_lo` (u32) | the first disk block's low 32 bits |

An index node's are index entries:

| Offset | Field | Meaning |
|---|---|---|
| 0 | `ei_block` (u32) | the first logical block under the child |
| 4 | `ei_leaf_lo` (u32) | the child tree block's low 32 bits |
| 8 | `ei_leaf_hi` (u16) | its high 16 bits (0) |
| 10 | `ei_unused` (u16) | 0 |

A tree block's bytes past its last entry are 0. ext4 reads an `ee_len`
above 32 768 as an uninitialized extent of `ee_len` - 32 768 blocks;
CMFS has none, so it is an error.

**Rules.** For the tree of an inode of `size` bytes, which has `n` =
ceil(`size` / 1024) blocks:

- **No holes.** The extents, in leaf order, map logical blocks 0 to `n` -
  1 exactly: the first starts at 0, each starts where the one before it
  ends, and the last ends at `n`. Bytes of the last block past `size` mean
  nothing (they read as whatever is there; writers need not clear them). A
  file grows only by writing at its end, so this always holds; the one
  exception is a file being truncated (below).
- **Index entries** are in order: in every index node, the first entry's
  `ei_block` is the node's own first logical block (0 in the root), and
  each entry's child starts at its `ei_block`, so each child's extents
  begin exactly there and end where the next entry's `ei_block` begins (or
  at the end of the tree). A child's `eh_depth` is its parent's minus 1.
- **Every tree block has at least one entry**; only the root may be empty,
  and only when the inode has no blocks. `eh_entries` ≤ `eh_max`, which is
  exactly 4 in the root and 84 in a tree block.
- **Every block** an inode uses, the disk blocks its extents map and its
  tree blocks, is in the data region, `datastart` to `size` - 1, and no
  block belongs to two inodes or twice to one.
- Two adjacent extents may map disk blocks that follow on from each other
  (they could have been one); that is allowed. Camelot's writers extend the
  last extent instead of adding one whenever they can.

The deepest a tree needs to be is 5: a root of 4 entries over levels of 84
holds 4 × 84^5 extents, more than the 2^31 blocks of the largest
filesystem even with every extent one block long (4 × 84^4 is not). That
is ext4's `EXT4_MAX_EXTENT_DEPTH` too.

**Lookup.** To find logical block `k`: in an index node, take the last entry
whose `ei_block` ≤ `k` and go down to its child; in the leaf, take the last
extent whose `ee_block` ≤ `k`. The block is its start + (`k` - `ee_block`).

**Growing a file** (the kernel's and mkfs's way; any writer must leave a
tree that follows the rules, and fsck accepts any that does). Block `n` is
always the next after the file's last. It is allocated next to the file's
last disk block if that one is free, else wherever the bitmap's first free
block is (Linux's allocators also start from such a goal). New tree blocks
come from the first free block: a file needs them only past 4 extents, so
they are rare, and they never take the block a file's next data block
wants unless that one is the first free. Then:

1. If the last extent ends just before the new disk block and has fewer
   than 32 768 blocks, it grows by one.
2. Otherwise a new extent (`n`, 1, block) goes after the last one, in the
   rightmost leaf. If that leaf is full, it goes in a new leaf instead,
   under the lowest node on the rightmost path that has room, with a chain
   of new index nodes down to it, one per level, each with one entry.
3. If no node on the rightmost path has room, the tree grows a level first,
   as ext4's `ext4_ext_grow_indepth` does: the root's entries move to a new
   tree block, and the root becomes an index node of one entry pointing to
   it, one level deeper. That block has room (4 entries of 84), so step 2
   puts the new entry there, or under it.

A writer allocates every block a step needs before it changes the tree, so
a disk that fills up part way leaves the tree as it was.

**Truncation.** Freeing a file's blocks (open's `O_TRUNC`, and freeing an
inode) goes from the end, an extent or part of one at a time; a tree block
emptied on the way is freed and its index entry removed, and once the last
extent is gone the root is an empty leaf again (depth 0). A big file's blocks take more transactions
than one (see "Limits on transactions"), so the first one sets `TRUNCATING`
and `size` to 0, and the last one clears the flag, its tree empty. In
between, a crash leaves an inode with `TRUNCATING` set, `size` 0, and a
tree mapping logical blocks 0 to some `m` - 1 that otherwise follows the
rules: the blocks still to free. The next writable mount finishes the
truncation of a file that still has links, before it frees the orphans
(see "Orphans"); an orphan being freed, it frees whole as any other. fsck
reports both. This
is Linux's ext4, which puts an inode being truncated on its orphan list so
that recovery finishes the job. Only a regular file is truncated while it
has links; a directory has `TRUNCATING` only while it is freed as an orphan
(`nlink` 0).

**Link count.** `nlink` is the number of directory entries that name the
inode, not counting any `.` entry. A file in one directory has 1. A
directory has 1 for its entry in its parent (the root, which has none: its
own `..`, which names itself) plus 1 for the `..` of each subdirectory: 1 +
the number of its subdirectories.
**Orphans.** Removing an inode's last entry and freeing the inode are two
transactions: the kernel's unlink writes `nlink` = 0, and the inode is freed
when its last user lets go, which may be much later (a file still open, or
a directory still some process's working directory). A crash in between leaves an **orphan** on the disk: an allocated
inode with `nlink` 0, named by no entry, its blocks still marked in use.
That is allowed (see "Consistency rules"): nothing can reach it, and the
next writable mount frees it, after the log's recovery and as the last
user would have: its blocks are freed as in "Truncation", in as many
transactions as that takes, and then its `type` is set to 0. Every other
allocated inode has `nlink` ≥ 1. The root is never an orphan: a root with
`nlink` 0 is a broken disk.

## Directories

A directory is an inode of type 1 whose contents are an array of 16-byte
entries:

| Offset | Field | Meaning |
|---|---|---|
| 0 | `inum` (u16) | the inode, or 0 for a free slot |
| 2 | `name` (14 bytes) | the name, padded with NUL bytes; a 14-byte name has no NUL |

- `size` is a multiple of 16 and at least 32.
- Entry 0 is `.`, naming the directory itself; entry 1 is `..`, naming its
  parent. The root's `..` names the root. No other entry is `.` or `..`.
- A name is 1 to 14 bytes, none of them NUL or `/`; within a directory the
  names of the used entries are distinct. Bytes are compared exactly (names
  are case-sensitive; no encoding is implied, though Camelot's own tools use
  printable ASCII).
- Free slots (`inum` 0) may be anywhere after entry 1 and are reused by the
  next entry added; the name bytes of a free slot mean nothing. Removing an
  entry sets its `inum` to 0; directories never shrink.
- Every used entry names an allocated inode (type ≠ 0). A directory is named
  by exactly one entry other than `.`/`..`, in its parent (except the root,
  which no entry names but its own `.` and `..`), and its `..` names that
  parent: the directories form a tree under the root. Files may have several
  names (hard links), in any directories.
- Every allocated inode is reachable from the root.

**Paths** are names separated by `/`. Repeated slashes count as one, and a
trailing slash is ignored. Lookup starts at the root (the kernel looks a
path without a leading `/` up from the process's working directory instead)
and goes through each name in turn with the directory's own
entries, so `.` and `..` need no special case: `/..` is `/`. Every name but
the last must be a directory. A name longer than 14 bytes names nothing (the
kernel's lookups fail with `ENAMETOOLONG` rather than cut it short).

## Free-block bitmap

One bit per block of the filesystem, 8192 per bitmap block: block `b`'s bit is
bit `b` mod 8 (value 1 << (`b` mod 8)) of byte (`b` mod 8192) / 8 of block
`bmapstart` + `b` / 8192. A bit is 1 when the block is in use.

- The blocks before `datastart` (boot block, superblock, log, inode table,
  bitmap) are always marked in use.
- A data block is marked in use exactly when an inode uses it (as a content
  block or as one of its tree blocks). The bits for blocks `size` and above, in
  the last bitmap block, are 0; allocators look only below `size`.
- A newly allocated block is zeroed before use (the kernel's `balloc` logs a
  block of zeros), so a new tree block starts with every byte 0.

Inodes have no bitmap: an inode is free when its `type` is 0.

## The log

The log makes each **transaction**, a group of block writes that must happen
together (say: allocate an inode, fill it in, add a directory entry, mark
blocks used in the bitmap), atomic across a crash: after a crash at any
point, the disk holds either all of a transaction's writes or none of them,
once the log has been replayed.

The log region is `nlog` blocks from `logstart`: the **header** block, then
`nlog` - 1 **slots**. The header:

| Offset | Field | Meaning |
|---|---|---|
| 0 | `n` (u32) | blocks in the committed transaction; 0: the log is empty |
| 4 + 4`i` | `block[i]` (u32), `i` < `n` | where slot `i`'s copy belongs (its home block) |

Slot `i` is block `logstart` + 1 + `i`. Rules for a header with `n` > 0:
`n` ≤ `nlog` - 1, the `block[i]` are distinct, and each is in `inodestart` to
`size` - 1: a transaction never writes the boot block, the superblock or the
log itself. Bytes past the `n` entries are ignored; Camelot's writers make the
whole header zero past them. Since `nlog` ≤ 128, the meaningful part of the
header, at most 4 + 4 × 127 = 512 bytes, is in the block's first sector.

**Writing a transaction** (the kernel's `commit`):

1. Copy each block the transaction changed into the next slot, in order.
2. Wait until those writes are durable (a disk flush).
3. Write the header with `n` and the home block numbers. **This is the
   commit point.** The header's meaningful bytes are in one sector, and a
   disk writes a sector all or nothing, so the transaction is either
   committed or not; there is no half-written header.
4. Flush.
5. **Install**: write each slot's block to its home block.
6. Flush.
7. Write the header with `n` = 0 (all zero), so the log is empty again.
8. Flush, so that the empty header is durable before the next transaction
   writes its slots (else a crash could replay old entries over new slots).

Until step 3 nothing outside the log has changed: a crash leaves the old
state. From step 3 on the log holds a complete copy: a crash leaves state
the recovery completes. Writes to home blocks happen only in step 5, so a
transaction's changes never reach the disk outside the log before it
commits.

**Recovery**, when mounting: read the header. If `n` > 0 (and the header
follows the rules above; if not, the filesystem is corrupt and must not be
mounted), install every slot to its home block as in step 5, flush, write an
all-zero header, flush. Installing is idempotent, so a crash during recovery
is harmless: the next mount does it again. A disk that is read-only and has
`n` > 0 cannot be recovered, and is not mounted. A writable mount then
finishes the truncations a crash interrupted (see "Truncation") and frees
the orphans (see "Link count"), in transactions of their own.

Readers that do not write (`camelot-fs ls`, `fsck` without `--recover`) see
the disk as recovery would leave it: they read a block from its slot when the
header lists it.

**Limits on transactions** (the kernel's, not the format's): a transaction
holds at most `min(nlog - 1, LOGSIZE)` distinct blocks, `LOGSIZE` = 44 in the
kernel. A block changed twice in one transaction takes one slot (absorption).
Each operation reserves `MAXOPBLOCKS` = 22 blocks, and operations of several
processes may share one transaction (group commit): a new operation waits
while the ones in progress could fill the log, and the transaction commits
when the last operation in it ends. An operation alone always starts, so the
log must have room for one: the kernel mounts a filesystem whose log has
fewer than `MAXOPBLOCKS` slots (`nlog` < 23) read-only (after replaying its
log, which needs no room). Such a disk is valid; `camelot-fs mkfs --log`
can make one.

`MAXOPBLOCKS` is the most blocks any operation changes, counted for the
worst case, when every block allocated or freed has its bit in a different
bitmap block:

- **Adding a block to a file** ("Growing a file") changes the new block and
  its bitmap block, and either the last extent's node (step 1) or the node
  that gets the new entry (steps 2 and 3), plus the new tree blocks and
  their bitmap blocks, and the inode. There are at most 5 new tree blocks:
  a chain of 5 down to the new leaf, when only the root of a tree 5 deep has
  room, or the old root's copy and a chain of 4, when the tree grows from
  depth 4 to 5. That is at most 1 + 1 + 1 + 5 + 5 + 1 = 14 blocks.
- **A write** goes in chunks of at most 4 blocks of data, one transaction
  each (`MAXWRITE` in `kernel/file.c`): a chunk changes up to 5 data blocks
  (4 new ones and the partly written block it starts in), the bitmap blocks
  of the 4 new ones, the rightmost leaf, the inode, and at most one split
  of the tree (a new leaf has room for 83 more extents before the next),
  which adds the node that takes the new entry and 5 new tree blocks with
  their bitmap blocks: 1 + 4 + 4 + 1 + 1 + 11 = 22.
- **Creating** a file or directory changes the new inode's block, a new
  directory's first block and its bitmap block, and the parent: one entry
  added, which may add a block (14, its inode included): at most 17.
  `link` changes the entry (14 at most) and the file's inode: 15. `unlink`
  changes the entry's block and the two inodes: 3. Freeing the file, when
  that was its last name and nobody has it open, is not part of it: see
  the next point.
- **Truncation** frees blocks in transactions of their own, apart from any
  operation it runs in (as Linux's eviction of an unlinked file takes a
  journal handle of its own, after the unlink's): what the operation logged
  before commits first, and the operation goes on afterwards in a new one,
  so neither shares the other's room. A file without blocks is the
  exception: emptying or freeing it changes only its inode, which the
  operation counts already. Each of a truncation's transactions changes
  the inode and at most 21 other blocks: the bitmap blocks of the blocks it
  frees, and the tree blocks whose entries it removes (a leaf that ends up
  freed still took a slot when its entries went). It counts them as it
  goes, and before freeing the next run of blocks (the end of the last
  extent, as far back as one bitmap block covers) it makes sure the rest
  could still fit: 2 for the run (its bitmap block and its leaf), and 2 for
  each level of the tree the run could empty on its way to the root (the
  emptied node's bitmap block, and its parent). If not, the transaction
  ends there and the next one goes on. A truncation that needs more than
  one transaction sets `TRUNCATING` in its first one (see "Truncation"); a
  crash in between leaves the rest to the next mount.

## Consistency rules

A disk is consistent, after replaying the log, when:

1. the superblock follows the rules of its section;
2. inode 0 is free and inode 1 is a directory;
3. every inode's type is 0 to 3, and every allocated inode follows the rules
   of "Inodes" and "Extent trees" (its fields, its tree, no holes, no block
   used twice; one with `TRUNCATING` set as "Truncation" says);
4. the bitmap marks exactly the blocks before `datastart` and the blocks some
   inode uses, and no bit at or past `size`;
5. every directory follows the rules of "Directories", and the directories
   form one tree under the root that reaches every allocated inode but the
   orphans (an orphaned directory's entries are not part of the tree: its
   `..` doesn't count for its old parent);
6. every allocated inode's `nlink` is the number of entries naming it, `.`
   entries not counted; an inode no entry names has `nlink` 0 (an orphan,
   which the next mount frees) or is an error.

`camelot-fs fsck` checks all of these and reports each break it finds, the
orphans it found and the truncations left to finish; `fsck --recover`
replays the log, finishes the truncations and frees the orphans, as a
mount would. The
kernel trusts a mounted disk and checks only the superblock and the log
header; a broken disk may make it panic.

## Booting

The filesystem leaves the boot block (block 0) and the blocks past `size`
alone, and a bootable disk uses both, so it is still a valid filesystem. Grail's boot ROM, started with no program, reads the boot block
into RAM, checks it and jumps to it (`docs/excalibur-spec.md` section 7,
"Boot from disk"); Camelot's boot block loads the kernel from the blocks
after the filesystem:

```
block 0            boot block: the bootloader and where the kernel is
blocks 1 ... size - 1     the filesystem
blocks size ...    the kernel ELF, what the bootloader loads of it, padded with zeros to a whole block
```

The boot block:

| Offset | Field | Meaning |
|---|---|---|
| 0 | code (1008 bytes) | the bootloader, entered at its first byte; zeros after its end |
| 1008 | `kernel_sector` (u32) | the kernel ELF's first sector: 2 × `size` |
| 1012 | `kernel_sectors` (u32) | its length in sectors, a whole number of blocks; 0: no kernel |
| 1016 | `checksum` (u32) | makes the 256 words of the block add up to 0 (mod 2^32) |
| 1020 | `magic` (u32) | 0x544f4f42, the bytes `B O O T` |

The last two are the boot ROM's: it refuses a block without the magic or
whose words do not add up to 0. `camelot-fs mkfs --boot BOOT --kernel
KERNEL` (or `boot::install` in the crate) fills in all of it: the
bootloader's code from the executable BOOT (`kernel/boot/boot.elf`, linked
at 0 and position-independent, at most 1008 bytes, no `.bss`), the kernel
ELF KERNEL after the filesystem. Only the parts of the kernel the bootloader
reads are stored (`boot::loadable`): the ELF header, the program headers and
each `PT_LOAD` segment's file bytes at their offsets, zeros between them, up
to the end of the last one; the section headers, symbols and their names are
left out (the header says there are no sections), which saves about 60 KiB.

`cargo xtask fs` makes the root image this way, with the disk kernel,
`kernel/kernel-disk.elf`: the kernel linked without the user programs that
`kernel/kernel.elf` carries for running without a disk, since the image has
them in `/bin`. It is Linux's split: a kernel image and the programs on the
root filesystem, with `kernel.elf` playing the part of an initramfs for a
machine with no disk. The image's size is the one the spec gives (section 10,
"Root image"; `--size N` for N blocks): the kernel takes about 300 blocks and
the filesystem every block it leaves, so `size` is the image's blocks minus the
kernel's. A disk kernel that finds
no `/bin/sh` panics, having no programs of its own to fall back on.

The kernel is not a file of the filesystem (`/boot/kernel`, say): a
bootloader that must fit in 1008 bytes reads a run of sectors far more
simply than it could walk a directory, an inode, an extent tree and the
log. (In version 1 it could not have been a file anyway: the disk kernel,
about 300 KiB, was bigger than the largest file.) The price is that the
kernel cannot be replaced from inside Camelot; mkfs installs it.

The bootloader (`kernel/boot/boot.S`) is one source for both widths: an
RV32 machine's boot block is `boot.elf`, which loads an ELF32 kernel, an
RV64 machine's `boot-rv64.elf`, which loads an ELF64 one (`camelot-fs mkfs
--boot` refuses a bootloader and a kernel of different widths), reading the
disk's version 2 registers. It reads the kernel's first 1 KiB and loads
every `PT_LOAD` segment of it to its physical address (`p_paddr`, which for
the RV64 kernel, linked at 0xffff_ffff_8000_0000, is not its `p_vaddr`), as
Grail's ELF loader would: the whole sectors of the segment's file bytes straight to the
address with the disk's DMA, the last partial sector through a buffer, then
zeros up to `p_memsz`. It refuses (printing `boot: <why>` and stopping with
exit code 0xFC) a block with `kernel_sectors` 0, a read error, a kernel that
is not a little-endian RISC-V `ET_EXEC` of its width (ELF32, ELF64) with its
program headers in its first 1 KiB, a segment with file bytes whose file offset is not a
multiple of 512 (merlin-ld and ld.lld place segments at page offsets) or
that runs past `kernel_sectors` (a `.bss`-only segment, `p_filesz` 0, reads
nothing, so its offset is not checked), and a segment outside RAM or
reaching into the 64 KiB the bootloader runs in. Then it writes the kernel's
entry point to the boot ROM's entry word (a doubleword on RV64), so harts
the kernel wakes go there too, and jumps to it with a0 = 0 and a1 = the
sysinfo block: the load address of `e_entry`, `p_paddr + (e_entry -
p_vaddr)` of the segment that holds it, since the kernel starts with paging
off (mkfs checks that a segment holds it). mkfs stores only what the
bootloader loads of the kernel, with the section header fields of the ELF
header cleared at the offsets of its class (`e_shoff` at 32 in ELF32, at 40
in ELF64). On success it prints one line,
`boot: kernel at sector 0x...`.

## Changes

Version 1 was the first. A change to anything above is a new version
number; a reader refuses a version it was not written for, and there are no
version 1 disks to keep (images are built, never committed).

**Version 2** maps files with ext4's extent trees in a 128-byte inode
(8 per block) with a 64-bit `size`, a `flags` word and reserved bytes,
instead of 12 direct pointers and one indirect block in a 64-byte inode
(16 per block). The largest version 1 file was 268 blocks, 274 432 bytes:
the self-hosted assembler, linked on Camelot with its symbols, had already
outgrown it, and 64-bit programs only grow. Linux went from ext2's block
pointers (with double and triple indirect blocks) to ext4's extents for the
same reasons: a big contiguous file is a handful of entries instead of a
pointer per block, and there is no fixed limit. Truncation became
restartable (`TRUNCATING`, "Truncation"), since freeing a big file no longer
fits one transaction, and `MAXOPBLOCKS`, `LOGSIZE` and mkfs's default log
grew with the worst case of an operation ("Limits on transactions").
