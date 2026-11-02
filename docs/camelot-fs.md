# The Camelot filesystem (CMFS), version 1

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
bitmap and data blocks; files of direct blocks plus one indirect block;
directories as files of fixed-size entries.

## Conventions

- **Block**: 1024 bytes, two of the disk's 512-byte sectors (spec section
  9.6). Block `n` is sectors 2`n` and 2`n` + 1. Blocks are numbered from 0 at
  the start of the disk. This is the kernel buffer cache's `BSIZE`.
- **Integers** are little-endian, unsigned: `u16` (2 bytes) and `u32` (4
  bytes), at the byte offsets given. There is no padding other than what the
  tables show.
- **Block pointers** are `u32` block numbers. 0 means "no block": block 0 is
  the boot block, which nothing ever points to.
- A filesystem of `size` blocks occupies blocks 0 to `size` - 1 of the disk.
  The disk may be bigger; blocks past `size` are not the filesystem's.
- `ceil(a / b)` is `a / b` rounded up.

## Layout

```
block 0            boot block (not used by the filesystem)
block 1            superblock
logstart ...       log: header block, then nlog - 1 log slots
inodestart ...     inode table: ceil(ninodes / 16) blocks
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
| 4 | `version` (u32) | 1 |
| 8 | `size` (u32) | blocks in the filesystem, all regions included |
| 12 | `nblocks` (u32) | data blocks: `size` - `datastart` |
| 16 | `ninodes` (u32) | inode slots, numbered 0 to `ninodes` - 1 |
| 20 | `nlog` (u32) | blocks in the log, its header included |
| 24 | `logstart` (u32) | 2 |
| 28 | `inodestart` (u32) | `logstart` + `nlog` |
| 32 | `bmapstart` (u32) | `inodestart` + ceil(`ninodes` / 16) |
| 36 | `datastart` (u32) | `bmapstart` + ceil(`size` / 8192) |

Bytes 40 to 1023 are zero (readers check it: a superblock with other bytes
there is broken). Limits: 2 ≤ `nlog` ≤ 128 (the kernel writes only to a
filesystem whose log has room for one operation, 11 blocks or more; see
"Limits on transactions"); 2 ≤ `ninodes` ≤ 65536
(inode numbers must fit a directory entry's `u16`); `datastart` < `size`, so
there is at least one data block (the root directory's); `size` × 2 sectors
must fit on the disk. A filesystem is at most 2^31 blocks (the buffer cache's
limit, spec section 10, "Kernel disks"). A reader that finds a different
magic says the disk has no filesystem; one that finds a different version,
or fields that break these rules, must not mount it.

`camelot-fs mkfs` makes `nlog` = 21 (a header and 20 slots, the kernel's
`LOGSIZE`) and `ninodes` = 256 unless told otherwise, and `size` = 2048
blocks (2 MiB) by default.

## Inodes

The inode table holds `ninodes` inodes of 64 bytes, 16 per block: inode `i`
is at byte (`i` mod 16) × 64 of block `inodestart` + `i` / 16. The bytes of
the table's last block past inode `ninodes` - 1 are unused.

| Offset | Field | Meaning |
|---|---|---|
| 0 | `type` (u16) | 0 free, 1 directory, 2 regular file, 3 device |
| 2 | `major` (u16) | device number (type 3 only; 0 otherwise) |
| 4 | `minor` (u16) | device number (type 3 only; 0 otherwise) |
| 6 | `nlink` (u16) | directory entries naming this inode (below) |
| 8 | `size` (u32) | length of the contents in bytes |
| 12 | `addrs[0..12]` (12 × u32) | direct blocks: blocks 0 to 11 of the contents |
| 60 | `indirect` (u32) | the indirect block: a block of 256 u32 pointers to blocks 12 to 267 |

- Inode 0 is never used: a directory entry with inode number 0 is a free
  slot. Its `type` is 0.
- Inode 1 is the root directory, `/`.
- A free inode has `type` 0; its other bytes mean nothing. Whoever allocates
  it (the kernel's `ialloc`, mkfs) zeroes all 64 bytes first. Freeing an
  inode (its last link gone, nobody using it) releases its blocks and sets
  `size`, the pointers and `type` to 0.
- **Contents.** Logical block `k` of a file (bytes 1024`k` to 1024`k` +
  1023) is `addrs[k]` for `k` < 12, and entry `k` - 12 of the indirect block
  (the u32 at byte 4(`k` - 12)) for 12 ≤ `k` < 268. So the largest file is 268
  blocks, 274 432 bytes.
- **No holes.** A file of `size` bytes has exactly ceil(`size` / 1024) blocks,
  all allocated: their pointers are non-zero and every pointer past them is 0.
  The indirect pointer is non-zero exactly when the file has more than 12
  blocks; the indirect block's entries past the file's last block are 0.
  Bytes of the last block past `size` mean nothing (they read as whatever is
  there; writers need not clear them). A file grows only by writing at its
  end, so this always holds.
- Every pointer (direct, indirect, and in the indirect block) points into the
  data region, `datastart` to `size` - 1, and no block belongs to two inodes
  or twice to one (the indirect block itself counts as the inode's).
- A **device** inode has `size` 0 and no blocks; `major` and `minor` say which
  device: the kernel opens major 1 as the console (`/dev/console` on the root
  image) and has no driver for any other (docs/camelot-syscalls.md). A
  directory's or file's `major` and `minor` are 0.

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
user would have: its blocks are marked free in the bitmap, and its `type`,
`size` and pointers set to 0, one transaction per orphan. Every other
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
  block or as its indirect block). The bits for blocks `size` and above, in
  the last bitmap block, are 0; allocators look only below `size`.
- A newly allocated block is zeroed before use (the kernel's `balloc` logs a
  block of zeros), so an indirect block starts with every entry 0.

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
`n` > 0 cannot be recovered, and is not mounted. A writable mount then frees
the orphans (see "Link count"), in transactions of their own.

Readers that do not write (`camelot-fs ls`, `fsck` without `--recover`) see
the disk as recovery would leave it: they read a block from its slot when the
header lists it.

**Limits on transactions** (the kernel's, not the format's): a transaction
holds at most `min(nlog - 1, LOGSIZE)` distinct blocks, `LOGSIZE` = 20 in the
kernel. A block changed twice in one transaction takes one slot (absorption).
Each operation reserves `MAXOPBLOCKS` = 10 blocks, and operations of several
processes may share one transaction (group commit): a new operation waits
while the ones in progress could fill the log, and the transaction commits
when the last operation in it ends. An operation alone always starts, so the
log must have room for one: the kernel mounts a filesystem whose log has
fewer than `MAXOPBLOCKS` slots (`nlog` < 11) read-only (after replaying its
log, which needs no room). Such a disk is valid; `camelot-fs mkfs --log`
can make one.

## Consistency rules

A disk is consistent, after replaying the log, when:

1. the superblock follows the rules of its section;
2. inode 0 is free and inode 1 is a directory;
3. every inode's type is 0 to 3, and every allocated inode follows the rules
   of "Inodes" (size, pointers, no holes, no block used twice);
4. the bitmap marks exactly the blocks before `datastart` and the blocks some
   inode uses, and no bit at or past `size`;
5. every directory follows the rules of "Directories", and the directories
   form one tree under the root that reaches every allocated inode but the
   orphans (an orphaned directory's entries are not part of the tree: its
   `..` doesn't count for its old parent);
6. every allocated inode's `nlink` is the number of entries naming it, `.`
   entries not counted; an inode no entry names has `nlink` 0 (an orphan,
   which the next mount frees) or is an error.

`camelot-fs fsck` checks all of these and reports each break it finds, and
the orphans it found; `fsck --recover` replays the log and frees them, as a
mount would. The
kernel trusts a mounted disk and checks only the superblock and the log
header; a broken disk may make it panic.

## Booting

The filesystem leaves the boot block (block 0) and the blocks past `size`
alone, and a bootable disk uses both, so it is still a valid version 1
filesystem. Grail's boot ROM, started with no program, reads the boot block
into RAM, checks it and jumps to it (`docs/excalibur-spec.md` section 7,
"Boot from disk"); Camelot's boot block loads the kernel from the blocks
after the filesystem:

```
block 0            boot block: the bootloader and where the kernel is
blocks 1 ... size - 1     the filesystem
blocks size ...    the kernel ELF, as a file, padded with zeros to a whole block
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
ELF KERNEL after the filesystem. `cargo xtask fs` makes the root image this
way, with `kernel/kernel.elf`.

The kernel is not a file of the filesystem (`/boot/kernel`, say), for two
reasons: it is bigger than the largest file (the kernel with its built-in
programs is over 500 KiB, a file at most 268 KiB), and a bootloader that
must fit in 1008 bytes reads a run of sectors far more simply than it could
walk directories, inodes, indirect blocks and the log. The price is that the
kernel cannot be replaced from inside Camelot; mkfs installs it.

The bootloader (`kernel/boot/boot.s`) reads the kernel's first 1 KiB and
loads every `PT_LOAD` segment of it to its physical address, as Grail's ELF
loader would: the whole sectors of the segment's file bytes straight to the
address with the disk's DMA, the last partial sector through a buffer, then
zeros up to `p_memsz`. It refuses (printing `boot: <why>` and stopping with
exit code 0xFC) a block with `kernel_sectors` 0, a read error, a kernel that
is not an ELF32 little-endian RISC-V `ET_EXEC` with its program headers in
its first 1 KiB, a segment with file bytes whose file offset is not a
multiple of 512 (merlin-ld and ld.lld place segments at page offsets) or
that runs past `kernel_sectors` (a `.bss`-only segment, `p_filesz` 0, reads
nothing, so its offset is not checked), and a segment outside RAM or
reaching into the 64 KiB the bootloader runs in. Then it writes the kernel's entry point to the boot
ROM's entry word, so harts the kernel wakes go there too, and jumps to it
with a0 = 0 and a1 = the sysinfo block. On success it prints one line,
`boot: kernel at sector 0x...`.

## Changes

Version 1 is the first. A change to anything above is a new version number.
