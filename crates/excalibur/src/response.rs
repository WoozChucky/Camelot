//! Response files: `@FILE` on a tool's command line stands for the arguments
//! written in FILE, read as clang and LLVM's tools read them (LLVM's GNU
//! rules, `cl::TokenizeGNUCommandLine` with `cl::ExpandResponseFiles`).
//! Every tool of the toolchain reads them this one way, merlin and
//! merlin-ld here, and on Camelot the C library's `expand_response_files`
//! (kernel/user/lib/response.c) for `cc`, `cc1` and `ld`: a link line, or a
//! compiler's, can be longer than a shell takes, or than Camelot's `exec`
//! passes (16 arguments), and the `cc` driver gives its stages one in a
//! response file then, as clang's driver does.
//!
//! The rules ([`tokenize`], [`expand`]):
//!
//! - the arguments are separated by blanks, tabs, carriage returns and line
//!   feeds, any number of them;
//! - a backslash takes the character after it as it is (a blank, a quote,
//!   a backslash), outside quotes and inside them;
//! - `'...'` and `"..."` quote: what is between them is part of the
//!   argument, blanks included (`""` alone is an empty argument);
//! - an argument `@FILE` in a response file is expanded too, FILE named from
//!   the working directory;
//! - an `@FILE` whose FILE does not exist is left as it is, an argument
//!   like any other (a file of that name, for a compiler);
//! - a FILE that exists but can't be read (a directory) is an error, `cannot
//!   not open file 'PATH': REASON` (LLVM's words, its typo included), and so
//!   is one that names itself, directly or through others, `recursive
//!   expansion of: 'PATH'`; PATH is the file's absolute path.

use std::path::{Component, Path, PathBuf};

/// Whether `b` separates two arguments in a response file.
fn is_separator(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\r' | b'\n')
}

/// What LLVM says of UTF-16 it can't convert.
pub const BAD_UTF16: &str = "Could not convert UTF16 to UTF8";

/// A response file's text as LLVM reads it: a UTF-8 byte-order mark left
/// out, and UTF-16 with one (either byte order) made UTF-8, strictly, as
/// `convertUTF16ToUTF8String`: an odd number of bytes or a surrogate
/// without its pair is [`BAD_UTF16`].
pub fn decode(bytes: &[u8]) -> Result<Vec<u8>, &'static str> {
    let utf16 = |be: bool| {
        let (units, odd) = bytes[2..].as_chunks::<2>();
        if !odd.is_empty() {
            return Err(BAD_UTF16);
        }
        let units: Vec<u16> = units
            .iter()
            .map(|&c| {
                if be {
                    u16::from_be_bytes(c)
                } else {
                    u16::from_le_bytes(c)
                }
            })
            .collect();
        String::from_utf16(&units)
            .map(String::into_bytes)
            .map_err(|_| BAD_UTF16)
    };
    match bytes {
        [0xef, 0xbb, 0xbf, rest @ ..] => Ok(rest.to_vec()),
        [0xff, 0xfe, ..] => utf16(false),
        [0xfe, 0xff, ..] => utf16(true),
        _ => Ok(bytes.to_vec()),
    }
}

/// The arguments the text of a response file holds, by LLVM's GNU rules
/// (see the module's). An argument ends at a NUL byte in it, as LLVM's,
/// which are C strings, end there; the reading goes on after it.
pub fn tokenize(text: &[u8]) -> Vec<Vec<u8>> {
    let mut args = raw_tokens(text);
    for a in &mut args {
        if let Some(nul) = a.iter().position(|&b| b == 0) {
            a.truncate(nul);
        }
    }
    args
}

fn raw_tokens(text: &[u8]) -> Vec<Vec<u8>> {
    let mut args = Vec::new();
    let mut arg: Vec<u8> = Vec::new();
    // Whether an argument has started: `""` makes an empty one.
    let mut started = false;
    let mut i = 0;
    while i < text.len() {
        let c = text[i];
        if is_separator(c) {
            if started {
                args.push(std::mem::take(&mut arg));
                started = false;
            }
            i += 1;
            continue;
        }
        started = true;
        if c == b'\\' && i + 1 < text.len() {
            arg.push(text[i + 1]);
            i += 2;
            continue;
        }
        if c == b'"' || c == b'\'' {
            // To the closing quote (or the end), a backslash taking the
            // character after it.
            i += 1;
            while i < text.len() && text[i] != c {
                if text[i] == b'\\' && i + 1 < text.len() {
                    i += 1;
                }
                arg.push(text[i]);
                i += 1;
            }
            i += 1;
            continue;
        }
        arg.push(c);
        i += 1;
    }
    if started {
        args.push(arg);
    }
    args
}

/// `path` made absolute from the working directory, `.` and `..` taken out
/// by their names (as a response file is told apart from the others it
/// names).
pub fn absolute(path: &Path) -> PathBuf {
    let full = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_default().join(path)
    };
    let mut out = PathBuf::new();
    for c in full.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            c => out.push(c),
        }
    }
    out
}

/// `args` with every `@FILE` replaced by the arguments of FILE, expanded in
/// turn, by the rules of the module. The error is LLVM's message (see the
/// module's), which a tool prints alone and exits 1 for, as LLVM's do.
pub fn expand(args: impl IntoIterator<Item = String>) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let mut open: Vec<PathBuf> = Vec::new();
    for a in args {
        expand_into(a, &mut out, &mut open)?;
    }
    Ok(out)
}

fn expand_into(arg: String, out: &mut Vec<String>, open: &mut Vec<PathBuf>) -> Result<(), String> {
    let Some(file) = arg.strip_prefix('@') else {
        out.push(arg);
        return Ok(());
    };
    // `@` alone names the working directory, as LLVM's name for it does
    // (its name appended to the working directory): a directory, an error.
    let path = absolute(Path::new(file));
    let file = if file.is_empty() {
        path.as_path()
    } else {
        Path::new(file)
    };
    let text = match std::fs::metadata(file) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            out.push(arg);
            return Ok(());
        }
        Ok(m) if m.is_dir() => {
            return Err(format!(
                "cannot not open file '{}': is a directory",
                path.display()
            ));
        }
        _ => std::fs::read(file).map_err(|e| format!("cannot not open file '{}': {e}", path.display()))?,
    };
    if open.contains(&path) {
        return Err(format!("recursive expansion of: '{}'", path.display()));
    }
    open.push(path);
    for word in tokenize(&decode(&text)?) {
        expand_into(String::from_utf8_lossy(&word).into_owned(), out, open)?;
    }
    open.pop();
    Ok(())
}

/// `args` written as a response file that [`tokenize`] reads back as they
/// are: each in double quotes, a backslash before a `"` or a `\` in it, and
/// a blank after it, as clang's driver writes its own.
pub fn write(args: &[String]) -> String {
    let mut s = String::new();
    for a in args {
        s.push('"');
        for c in a.chars() {
            if c == '"' || c == '\\' {
                s.push('\\');
            }
            s.push(c);
        }
        s.push_str("\" ");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(text: &str) -> Vec<String> {
        tokenize(text.as_bytes())
            .into_iter()
            .map(|w| String::from_utf8(w).unwrap())
            .collect()
    }

    #[test]
    fn tokenizes_as_llvm_reads_gnu_response_files() {
        assert_eq!(words("  -a\t-b\r\n-c  "), ["-a", "-b", "-c"]);
        // Only those four separate: a form feed or a vertical tab is part of
        // an argument, and so is a no-break space.
        assert_eq!(words("a\x0cb\x0bc d\u{a0}e"), ["a\x0cb\x0bc", "d\u{a0}e"]);
        assert_eq!(words(r#""-DA=1 2" 'x y' "" z"#), ["-DA=1 2", "x y", "", "z"]);
        assert_eq!(
            words(r#"a\ b \"q\" "in \" side" 'it\'s' c\\d"#),
            ["a b", "\"q\"", "in \" side", "it's", "c\\d"]
        );
        // Quotes in the middle of an argument join its parts.
        assert_eq!(words(r#"-D"X=a b"c"#), ["-DX=a bc"]);
        // A backslash last is itself; a quote not closed takes the rest.
        assert_eq!(words("a\\"), ["a\\"]);
        assert_eq!(words("\"open to the end"), ["open to the end"]);
        assert_eq!(words(""), Vec::<String>::new());
    }

    #[test]
    fn what_it_writes_reads_back() {
        let args: Vec<String> = ["-DQ=a b", "c\\d", "say \"hi\"", "", "$x", "'"]
            .map(String::from)
            .to_vec();
        let text = write(&args);
        assert_eq!(text, r#""-DQ=a b" "c\\d" "say \"hi\"" "" "$x" "'" "#);
        assert_eq!(words(&text), args);
    }

    #[test]
    fn expands_nested_files_and_keeps_missing_ones() {
        let dir = std::env::temp_dir().join(format!("excalibur-response-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        // A path as written in a response file, where a backslash escapes
        // (Windows'), and as given on the command line.
        let written = |name: &str| dir.join(name).display().to_string().replace('\\', "\\\\");
        let at = |name: &str| format!("@{}", dir.join(name).display());
        std::fs::write(dir.join("a"), format!("-x \"@{}\" -y", written("b"))).unwrap();
        std::fs::write(dir.join("b"), "'b 1' b2").unwrap();
        std::fs::write(dir.join("self"), format!("@{}", written("self2"))).unwrap();
        std::fs::write(dir.join("self2"), format!("@{}/./self", written(""))).unwrap();
        assert_eq!(
            expand([at("a"), "z".into()]).unwrap(),
            ["-x", "b 1", "b2", "-y", "z"]
        );
        assert_eq!(expand([at("missing")]).unwrap(), [at("missing")]);
        let cwd = absolute(Path::new("")).display().to_string();
        assert_eq!(
            expand(["@".to_string()]).unwrap_err(),
            format!("cannot not open file '{cwd}': is a directory")
        );
        // A UTF-8 byte-order mark left out, UTF-16 read, a NUL ending an
        // argument.
        std::fs::write(dir.join("bom"), b"\xef\xbb\xbf-DBOM=1 x").unwrap();
        assert_eq!(expand([at("bom")]).unwrap(), ["-DBOM=1", "x"]);
        let utf16: Vec<u8> = [0xff, 0xfe]
            .into_iter()
            .chain("-Da b".encode_utf16().flat_map(u16::to_le_bytes))
            .collect();
        std::fs::write(dir.join("u16"), utf16).unwrap();
        assert_eq!(expand([at("u16")]).unwrap(), ["-Da", "b"]);
        // UTF-16 of an odd length, or with a surrogate alone: LLVM's error.
        std::fs::write(dir.join("odd16"), b"\xff\xfea\0 \0b\0c").unwrap();
        assert_eq!(expand([at("odd16")]).unwrap_err(), BAD_UTF16);
        std::fs::write(dir.join("lone16"), b"\xff\xfea\0\0\xd8b\0").unwrap();
        assert_eq!(expand([at("lone16")]).unwrap_err(), BAD_UTF16);
        std::fs::write(dir.join("low16"), b"\xfe\xff\0a\xdc\0").unwrap();
        assert_eq!(expand([at("low16")]).unwrap_err(), BAD_UTF16);
        std::fs::write(dir.join("nulbyte"), b"a\0b c").unwrap();
        assert_eq!(expand([at("nulbyte")]).unwrap(), ["a", "c"]);
        let abs = |name: &str| absolute(&dir.join(name)).display().to_string();
        assert_eq!(
            expand([at("sub")]).unwrap_err(),
            format!("cannot not open file '{}': is a directory", abs("sub"))
        );
        assert_eq!(
            expand([at("self")]).unwrap_err(),
            format!("recursive expansion of: '{}'", abs("self"))
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
