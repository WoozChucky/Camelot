//! `ar` archives (`libc.a`): reading them, as merlin-ld does, and writing
//! them, as `cargo xtask fs` writes the root image's C library, byte for
//! byte as llvm-ar writes the same members (`llvm-ar rcs`, its default
//! deterministic mode, in the GNU format it uses for ELF members).
//! `kernel/user/ar`, Camelot's `/bin/ar`, is this module in C.
//!
//! The format is the System V one, with GNU's names, as GNU ar and llvm-ar
//! write it for ELF objects:
//!
//! ```text
//! "!<arch>\n"                      the magic, 8 bytes
//! header "/"   and the symbol table   (when there are members)
//! header "//"  and the long names     (when a name needs it)
//! header "a.o/" and a.o's bytes, then a '\n' if their size is odd
//! header ...
//! ```
//!
//! Every header is 60 bytes of text, each field padded with spaces: the
//! name (16), the modification time (12), the owner's and the group's ids
//! (6 each), the mode in octal (8), the size of what follows in decimal
//! (10), and "`\n". A member starts on an even offset: after a member of
//! odd size comes one `'\n'`, which its size leaves out. Deterministic, as
//! llvm-ar is by default (and GNU ar with `D`): the time and the ids are
//! 0 and the mode 644, whatever the files had.
//!
//! - **Names.** A name of up to 15 bytes with no `/` goes in the header,
//!   ended by a `/` (so `a.o` is `a.o/`, and a name may hold spaces). A
//!   longer one is written once in the long names member, `//`, followed
//!   by `/\n`, and the header says `/` and its offset there in decimal
//!   (`/0`, `/18`). That member's header has only its name and size, its
//!   other fields blank; its size counts the `'\n'` that makes it even.
//! - **The symbol table** (`/`, the "armap"): what a linker reads to know
//!   which member defines what without reading them all. A big-endian
//!   32-bit count, then for each symbol the offset of the header of the
//!   member that defines it (32-bit, big-endian), then the names, each
//!   ended by a NUL, the members in order and each one's symbols in the
//!   order of its own symbol table. A symbol is listed if it is global or
//!   weak and defined (common and absolute ones too): what a member
//!   *defines* ([`symbols`]). The table's header has time, ids and mode 0;
//!   its size counts a NUL that makes it even. An archive whose members
//!   define nothing still has one, as llvm-ar writes it: a count of 0 and
//!   four NULs for names. An archive with no members is only the magic.
//!
//! Every offset read from a file is checked: a truncated or corrupt
//! archive gives an [`Error`], not a panic.

use crate::{Elf, Error, SHN_UNDEF, STB_LOCAL};

/// What every archive starts with.
pub const MAGIC: &[u8; 8] = b"!<arch>\n";
/// A member's header.
pub const HEADER_SIZE: usize = 60;
/// The longest name a header holds itself (with the `/` after it, 16).
pub const MAX_SHORT_NAME: usize = 15;

/// Whether `data` is an archive.
pub fn is_archive(data: &[u8]) -> bool {
    data.starts_with(MAGIC)
}

/// A member of an archive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member<'a> {
    pub name: String,
    /// Where its header starts in the archive: what the symbol table gives.
    pub offset: usize,
    pub data: &'a [u8],
}

/// An archive read with [`parse`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Archive<'a> {
    /// In the archive's order, the symbol table and the long names left
    /// out.
    pub members: Vec<Member<'a>>,
    /// The symbol table: each name, with the index in [`Archive::members`]
    /// of the member that defines it. Empty if the archive has none.
    pub symbols: Vec<(String, usize)>,
    /// Whether it had a symbol table at all.
    pub has_symbol_table: bool,
}

fn bad<T>(msg: impl Into<String>) -> Result<T, Error> {
    Err(Error(msg.into()))
}

/// A header field: its text without the spaces after it.
fn field(h: &[u8], at: usize, len: usize) -> &[u8] {
    let f = &h[at..at + len];
    let end = f.iter().rposition(|&b| b != b' ').map_or(0, |e| e + 1);
    &f[..end]
}

/// A decimal header field.
fn decimal(h: &[u8], at: usize, len: usize, what: &str, offset: usize) -> Result<usize, Error> {
    let f = field(h, at, len);
    let text = std::str::from_utf8(f)
        .ok()
        .filter(|t| !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit()));
    match text.and_then(|t| t.parse().ok()) {
        Some(v) => Ok(v),
        None => bad(format!(
            "archive member at {offset}: bad {what} {:?}",
            String::from_utf8_lossy(f)
        )),
    }
}

fn be32(data: &[u8], at: usize) -> Option<usize> {
    let b = data.get(at..at + 4)?;
    Some(u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as usize)
}

/// Reads an archive: its members, their names (long ones through `//`) and
/// the symbol table (`/`). A 64-bit symbol table (`/SYM64/`, which ar
/// writes only past 4 GiB) and thin and BSD archives are errors.
pub fn parse(data: &[u8]) -> Result<Archive<'_>, Error> {
    if !is_archive(data) {
        return bad("not an archive (no !<arch> magic)");
    }
    let mut members = Vec::new();
    let mut long_names: &[u8] = &[];
    let mut table: Option<&[u8]> = None;
    let mut at = MAGIC.len();
    while at < data.len() {
        let Some(h) = data.get(at..at + HEADER_SIZE) else {
            return bad(format!("archive member at {at}: truncated header"));
        };
        if &h[58..60] != b"`\n" {
            return bad(format!("archive member at {at}: bad header end"));
        }
        let size = decimal(h, 48, 10, "size", at)?;
        let start = at + HEADER_SIZE;
        let Some(body) = start.checked_add(size).and_then(|end| data.get(start..end)) else {
            return bad(format!("archive member at {at}: {size} bytes run past the end"));
        };
        let name = field(h, 0, 16);
        match name {
            b"/" if members.is_empty() && table.is_none() && long_names.is_empty() => table = Some(body),
            b"//" if members.is_empty() && long_names.is_empty() => long_names = body,
            b"/SYM64/" => return bad("a 64-bit archive symbol table (/SYM64/) is not supported"),
            _ if name.starts_with(b"#1/") => return bad("BSD archives are not supported"),
            _ => {
                let name = member_name(name, long_names, at)?;
                members.push(Member {
                    name,
                    offset: at,
                    data: body,
                });
            }
        }
        // The next header is on an even offset.
        at = start + size + (size & 1);
    }
    let has_symbol_table = table.is_some();
    let symbols = match table {
        Some(t) => read_table(t, &members)?,
        None => Vec::new(),
    };
    Ok(Archive {
        members,
        symbols,
        has_symbol_table,
    })
}

/// A member's name from its header's name field: `name/`, or `/OFFSET`
/// into the long names.
fn member_name(field: &[u8], long_names: &[u8], at: usize) -> Result<String, Error> {
    let name = if let Some(off) = field.strip_prefix(b"/") {
        let off = std::str::from_utf8(off)
            .ok()
            .filter(|t| !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit()))
            .and_then(|t| t.parse::<usize>().ok());
        let Some(rest) = off.and_then(|o| long_names.get(o..)) else {
            return bad(format!(
                "archive member at {at}: bad long name {:?}",
                String::from_utf8_lossy(field)
            ));
        };
        match rest.windows(2).position(|w| w == b"/\n") {
            Some(end) => &rest[..end],
            None => return bad(format!("archive member at {at}: unterminated long name")),
        }
    } else {
        match field.strip_suffix(b"/") {
            Some(n) => n,
            None => return bad(format!("archive member at {at}: name without a /")),
        }
    };
    String::from_utf8(name.to_vec()).map_err(|_| Error(format!("archive member at {at}: name is not UTF-8")))
}

/// The symbol table's entries, each matched to the member at its offset.
fn read_table(t: &[u8], members: &[Member]) -> Result<Vec<(String, usize)>, Error> {
    let Some(count) = be32(t, 0) else {
        return bad("archive symbol table: truncated");
    };
    let names_at = count
        .checked_mul(4)
        .and_then(|n| n.checked_add(4))
        .filter(|&n| n <= t.len());
    let Some(names_at) = names_at else {
        return bad(format!("archive symbol table: {count} entries do not fit"));
    };
    let mut names = &t[names_at..];
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let offset = be32(t, 4 + 4 * i).unwrap_or(0);
        let Some(end) = names.iter().position(|&b| b == 0) else {
            return bad("archive symbol table: names run past its end");
        };
        let name = String::from_utf8_lossy(&names[..end]).into_owned();
        names = &names[end + 1..];
        let Some(m) = members.iter().position(|m| m.offset == offset) else {
            return bad(format!(
                "archive symbol table: {name} at {offset}, where no member starts"
            ));
        };
        out.push((name, m));
    }
    Ok(out)
}

/// The symbols a member defines, as the symbol table lists them: for an
/// ELF object, each global or weak symbol that is not undefined, in its
/// symbol table's order; nothing for any other file (llvm-ar and GNU ar
/// put text files in archives too).
pub fn symbols(data: &[u8]) -> Result<Vec<String>, Error> {
    if !data.starts_with(b"\x7fELF") {
        return Ok(Vec::new());
    }
    let elf = Elf::parse(data)?;
    Ok(elf
        .symbols()?
        .into_iter()
        .skip(1)
        .filter(|s| s.bind() != STB_LOCAL && s.shndx != SHN_UNDEF)
        .map(|s| s.name)
        .collect())
}

/// A member to write: its name in the archive (a file name, without the
/// directories) and its contents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewMember<'a> {
    pub name: &'a str,
    pub data: &'a [u8],
}

/// The name a file gets in an archive: its path's last component, as ar
/// takes it (`lib/printf.o` is `printf.o`).
pub fn member_name_of(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

/// `text` padded with spaces to `len` bytes.
fn pad(out: &mut Vec<u8>, text: &str, len: usize) -> Result<(), Error> {
    if text.len() > len {
        return bad(format!(
            "archive header field {text:?} is longer than {len} bytes"
        ));
    }
    out.extend_from_slice(text.as_bytes());
    out.resize(out.len() + len - text.len(), b' ');
    Ok(())
}

/// A header: name, then time, ids and mode as given (blank when `None`),
/// and the size.
fn header(out: &mut Vec<u8>, name: &str, rest: Option<(&str, &str)>, size: usize) -> Result<(), Error> {
    pad(out, name, 16)?;
    let (ids, mode) = rest.unwrap_or(("", ""));
    pad(out, ids, 12)?;
    pad(out, ids, 6)?;
    pad(out, ids, 6)?;
    pad(out, mode, 8)?;
    pad(out, &size.to_string(), 10)?;
    out.extend_from_slice(b"`\n");
    Ok(())
}

/// Whether `name` needs the long names member: as llvm-ar decides it,
/// longer than [`MAX_SHORT_NAME`] or holding a `/`.
fn is_long(name: &str) -> bool {
    name.len() > MAX_SHORT_NAME || name.contains('/')
}

/// Writes an archive of `members`, in order, with a symbol table (`ar
/// rcs`), byte for byte as llvm-ar does (see the module's description).
/// Fails if a member is an ELF file that does not parse, or if the archive
/// would pass 4 GiB, which a 32-bit symbol table cannot point into.
pub fn write(members: &[NewMember]) -> Result<Vec<u8>, Error> {
    let mut out = MAGIC.to_vec();
    if members.is_empty() {
        return Ok(out);
    }
    // The long names: each written once, in the order first met.
    let mut long_names = Vec::new();
    let mut long_at: Vec<(&str, usize)> = Vec::new();
    for m in members.iter().filter(|m| is_long(m.name)) {
        if !long_at.iter().any(|&(n, _)| n == m.name) {
            long_at.push((m.name, long_names.len()));
            long_names.extend_from_slice(m.name.as_bytes());
            long_names.extend_from_slice(b"/\n");
        }
    }
    if long_names.len() % 2 == 1 {
        long_names.push(b'\n');
    }
    let defined: Vec<Vec<String>> = members
        .iter()
        .map(|m| symbols(m.data))
        .collect::<Result<_, _>>()?;
    let count: usize = defined.iter().map(Vec::len).sum();
    let mut names: Vec<u8> = Vec::new();
    for n in defined.iter().flatten() {
        names.extend_from_slice(n.as_bytes());
        names.push(0);
    }
    if names.is_empty() {
        names = vec![0; 4];
    }
    let mut table_size = 4 + 4 * count + names.len();
    if table_size % 2 == 1 {
        names.push(0);
        table_size += 1;
    }
    // Where each member's header goes.
    let mut at = MAGIC.len() + HEADER_SIZE + table_size;
    if !long_names.is_empty() {
        at += HEADER_SIZE + long_names.len();
    }
    let mut offsets = Vec::with_capacity(members.len());
    for m in members {
        offsets.push(at);
        at += HEADER_SIZE + m.data.len() + (m.data.len() & 1);
    }
    if at > u32::MAX as usize {
        return bad("an archive of more than 4 GiB needs a 64-bit symbol table");
    }
    header(&mut out, "/", Some(("0", "0")), table_size)?;
    out.extend_from_slice(&(count as u32).to_be_bytes());
    for (i, syms) in defined.iter().enumerate() {
        for _ in syms {
            out.extend_from_slice(&(offsets[i] as u32).to_be_bytes());
        }
    }
    out.extend_from_slice(&names);
    if !long_names.is_empty() {
        header(&mut out, "//", None, long_names.len())?;
        out.extend_from_slice(&long_names);
    }
    for (i, m) in members.iter().enumerate() {
        debug_assert_eq!(out.len(), offsets[i]);
        let name = match long_at.iter().find(|&&(n, _)| n == m.name) {
            Some(&(_, off)) => format!("/{off}"),
            None => format!("{}/", m.name),
        };
        header(&mut out, &name, Some(("0", "644")), m.data.len())?;
        out.extend_from_slice(m.data);
        if m.data.len() % 2 == 1 {
            out.push(b'\n');
        }
    }
    Ok(out)
}

/// The members `ar r` leaves in an archive that had `old` (none if it is
/// being made) when given `files` (each a name in the archive and its
/// contents), as llvm-ar replaces them: every old member that has the name
/// of a file still unused is replaced in its place by the first such file,
/// the other old members stay, and the files left over go at the end, in
/// order. A new archive gets every file, even two of one name.
pub fn replace<'a>(old: &[NewMember<'a>], files: &[NewMember<'a>]) -> Vec<NewMember<'a>> {
    let mut left: Vec<&NewMember<'a>> = files.iter().collect();
    let mut out = Vec::with_capacity(old.len() + files.len());
    for m in old {
        match left.iter().position(|f| f.name == m.name) {
            Some(i) => out.push(left.remove(i).clone()),
            None => out.push(m.clone()),
        }
    }
    out.extend(left.into_iter().cloned());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn member<'a>(name: &'a str, data: &'a [u8]) -> NewMember<'a> {
        NewMember { name, data }
    }

    #[test]
    fn an_empty_archive_is_the_magic() {
        assert_eq!(write(&[]).unwrap(), MAGIC);
        let a = parse(MAGIC).unwrap();
        assert!(a.members.is_empty() && !a.has_symbol_table);
    }

    #[test]
    fn text_members_round_trip_with_names_long_and_short() {
        let long = "a_very_long_member_name.txt";
        let files = [
            member("odd.txt", b"xyz"),
            member(long, b"even"),
            member("abcdefghijklmno", b""),
            member("abcdefghijklmnop", b"16"),
            member(long, b"again"),
        ];
        let bytes = write(&files).unwrap();
        // No symbols: a count of 0 and four NULs.
        assert_eq!(&bytes[8..24], b"/               ");
        assert_eq!(&bytes[8 + 48..8 + 58], b"8         ");
        assert_eq!(&bytes[68..76], &[0; 8]);
        let a = parse(&bytes).unwrap();
        assert!(a.has_symbol_table && a.symbols.is_empty());
        let got: Vec<(&str, &[u8])> = a.members.iter().map(|m| (m.name.as_str(), m.data)).collect();
        let want: Vec<(&str, &[u8])> = files.iter().map(|m| (m.name, m.data)).collect();
        assert_eq!(got, want);
        // Every header on an even offset; the long names written once.
        assert!(a.members.iter().all(|m| m.offset % 2 == 0));
        let text = String::from_utf8_lossy(&bytes);
        assert_eq!(text.matches(long).count(), 1);
        assert!(text.contains("abcdefghijklmno/"));
    }

    #[test]
    fn corrupt_archives_are_errors() {
        let bytes = write(&[member("a.txt", b"hello")]).unwrap();
        // (The last byte is the pad after "hello": an archive without it is whole.)
        for cut in [9, 70, bytes.len() - 2] {
            assert!(parse(&bytes[..cut]).is_err(), "cut at {cut}");
        }
        assert!(parse(b"!<thin>\n").is_err());
        let mut bad_size = bytes.clone();
        bad_size[8 + 48] = b'x';
        assert!(parse(&bad_size).is_err());
    }

    #[test]
    fn replacing_follows_llvm_ar() {
        let (p1, p2, q, p3, ab) = (b"p1", b"p2", b"q", b"p3", b"ab");
        let old = [member("p.o", p1), member("p.o", p2), member("q.o", q)];
        let files = [
            member("p.o", p3),
            member("ab.o", ab),
            member("p.o", p1),
            member("p.o", p2),
        ];
        let got: Vec<(&str, &[u8])> = replace(&old, &files).iter().map(|m| (m.name, m.data)).collect();
        let want: [(&str, &[u8]); 5] = [("p.o", p3), ("p.o", p1), ("q.o", q), ("ab.o", ab), ("p.o", p2)];
        assert_eq!(got, want);
        assert_eq!(replace(&[], &files), files);
    }

    #[test]
    fn member_names_are_the_last_component() {
        assert_eq!(member_name_of("lib/printf.o"), "printf.o");
        assert_eq!(member_name_of("/usr/lib/x.o"), "x.o");
        assert_eq!(member_name_of("x.o"), "x.o");
    }
}
