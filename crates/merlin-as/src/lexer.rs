//! Turns source text into tokens, line by line.
//!
//! Comments: `#` and `//` to end of line, `/* ... */` blocks. `;` separates
//! statements like a newline does.

use crate::Error;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Tok {
    /// Names, mnemonics, directives and registers: `[A-Za-z_.$@][A-Za-z0-9_.$]*`.
    Ident(String),
    Int(i64),
    /// A hexadecimal number too wide for 64 bits: only `.file`'s `md5 0x...`
    /// (128 bits) takes one.
    Wide(u128),
    Str(Vec<u8>),
    /// Numeric local label reference: `1b` (backward) or `1f` (forward).
    LocalRef(u32, bool),
    Punct(char),
    /// `<<` and `>>`
    Shift(bool),
    Newline,
}

#[derive(Clone, Debug)]
pub struct Token {
    pub tok: Tok,
    pub line: usize,
}

fn ident_start(c: u8) -> bool {
    // `@` starts GNU type names like `@function` and `@progbits`.
    c.is_ascii_alphabetic() || c == b'_' || c == b'.' || c == b'$' || c == b'@'
}

fn ident_char(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_' || c == b'.' || c == b'$'
}

pub fn lex(src: &str) -> Result<Vec<Token>, Error> {
    let s = src.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    let mut line = 1;
    let push = |out: &mut Vec<Token>, tok, line| out.push(Token { tok, line });

    while i < s.len() {
        let c = s[i];
        match c {
            b'\n' => {
                push(&mut out, Tok::Newline, line);
                line += 1;
                i += 1;
            }
            b';' => {
                push(&mut out, Tok::Newline, line);
                i += 1;
            }
            b' ' | b'\t' | b'\r' => i += 1,
            b'#' => {
                while i < s.len() && s[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if s.get(i + 1) == Some(&b'/') => {
                while i < s.len() && s[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if s.get(i + 1) == Some(&b'*') => {
                let start = line;
                i += 2;
                loop {
                    if i + 1 >= s.len() {
                        return Err(Error::new(start, "unterminated /* comment"));
                    }
                    if s[i] == b'*' && s[i + 1] == b'/' {
                        i += 2;
                        break;
                    }
                    if s[i] == b'\n' {
                        line += 1;
                    }
                    i += 1;
                }
            }
            b'"' => {
                let (bytes, next) = lex_string(s, i + 1, line)?;
                push(&mut out, Tok::Str(bytes), line);
                i = next;
            }
            b'\'' => {
                // Character literal: 'a' or '\n'.
                let (bytes, next) = lex_quoted(s, i + 1, b'\'', line)?;
                if bytes.len() != 1 {
                    return Err(Error::new(line, "character literal must hold one character"));
                }
                push(&mut out, Tok::Int(bytes[0] as i64), line);
                i = next;
            }
            b'0'..=b'9' => {
                let start = i;
                while i < s.len() && ident_char(s[i]) {
                    i += 1;
                }
                let text = &src[start..i];
                push(&mut out, lex_number(text, line)?, line);
            }
            c if ident_start(c) => {
                let start = i;
                i += 1;
                while i < s.len() && ident_char(s[i]) {
                    i += 1;
                }
                push(&mut out, Tok::Ident(src[start..i].to_string()), line);
            }
            b'<' if s.get(i + 1) == Some(&b'<') => {
                push(&mut out, Tok::Shift(true), line);
                i += 2;
            }
            b'>' if s.get(i + 1) == Some(&b'>') => {
                push(&mut out, Tok::Shift(false), line);
                i += 2;
            }
            b',' | b'(' | b')' | b':' | b'+' | b'-' | b'*' | b'/' | b'%' | b'~' | b'&' | b'|' | b'^'
            | b'=' => {
                push(&mut out, Tok::Punct(c as char), line);
                i += 1;
            }
            _ => {
                let ch = src[i..].chars().next().unwrap_or('?');
                return Err(Error::new(line, format!("unexpected character '{ch}'")));
            }
        }
    }
    push(&mut out, Tok::Newline, line);
    Ok(out)
}

fn lex_number(text: &str, line: usize) -> Result<Tok, Error> {
    // `1b` / `1f`: a numeric label reference, unless it parses as binary (`0b101`).
    if let Some(last) = text.strip_suffix('b').or_else(|| text.strip_suffix('f')) {
        if !last.is_empty() && last.bytes().all(|b| b.is_ascii_digit()) {
            let n = last
                .parse()
                .map_err(|_| Error::new(line, "label number too large"))?;
            return Ok(Tok::LocalRef(n, text.ends_with('f')));
        }
    }
    let lower = text.to_ascii_lowercase();
    let lower = lower.replace('_', "");
    let parsed = if let Some(h) = lower.strip_prefix("0x") {
        u64::from_str_radix(h, 16)
    } else if let Some(b) = lower.strip_prefix("0b") {
        u64::from_str_radix(b, 2)
    } else if let Some(o) = lower.strip_prefix("0o") {
        u64::from_str_radix(o, 8)
    } else {
        lower.parse::<u64>()
    };
    match parsed {
        Ok(v) => Ok(Tok::Int(v as i64)),
        Err(_) => match lower.strip_prefix("0x").map(|h| u128::from_str_radix(h, 16)) {
            Some(Ok(v)) => Ok(Tok::Wide(v)),
            _ => Err(Error::new(line, format!("invalid number '{text}'"))),
        },
    }
}

fn lex_string(s: &[u8], i: usize, line: usize) -> Result<(Vec<u8>, usize), Error> {
    lex_quoted(s, i, b'"', line)
}

fn lex_quoted(s: &[u8], mut i: usize, quote: u8, line: usize) -> Result<(Vec<u8>, usize), Error> {
    let mut out = Vec::new();
    loop {
        match s.get(i) {
            None | Some(b'\n') => return Err(Error::new(line, "unterminated string")),
            Some(&c) if c == quote => return Ok((out, i + 1)),
            Some(b'\\') => {
                let e = *s
                    .get(i + 1)
                    .ok_or_else(|| Error::new(line, "unterminated string"))?;
                i += 2;
                out.push(match e {
                    b'n' => b'\n',
                    b't' => b'\t',
                    b'r' => b'\r',
                    // Backspace and form feed, as llvm-mc reads them: clang
                    // writes bytes 8 and 12 this way (`"\001\002\004\b"`).
                    b'b' => 0x08,
                    b'f' => 0x0c,
                    // Octal, one to three digits, as GNU as and llvm-mc read
                    // them: `\0`, `\12`, `\377` (what clang writes for bytes
                    // that aren't printable). A value above 0o377 is no byte.
                    b'0'..=b'7' => {
                        let mut value = u32::from(e - b'0');
                        let mut digits = 1;
                        while digits < 3 {
                            match s.get(i) {
                                Some(&d @ b'0'..=b'7') => {
                                    value = value * 8 + u32::from(d - b'0');
                                    digits += 1;
                                    i += 1;
                                }
                                _ => break,
                            }
                        }
                        u8::try_from(value).map_err(|_| Error::new(line, "octal escape out of range"))?
                    }
                    b'\\' => b'\\',
                    b'"' => b'"',
                    b'\'' => b'\'',
                    b'x' => {
                        let hex = s
                            .get(i..i + 2)
                            .ok_or_else(|| Error::new(line, "bad \\x escape"))?;
                        let hex = std::str::from_utf8(hex).map_err(|_| Error::new(line, "bad \\x escape"))?;
                        i += 2;
                        u8::from_str_radix(hex, 16).map_err(|_| Error::new(line, "bad \\x escape"))?
                    }
                    _ => return Err(Error::new(line, format!("unknown escape '\\{}'", e as char))),
                });
            }
            Some(&c) => {
                out.push(c);
                i += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toks(s: &str) -> Vec<Tok> {
        lex(s).unwrap().into_iter().map(|t| t.tok).collect()
    }

    #[test]
    fn basics() {
        assert_eq!(
            toks("loop: addi a0, a0, -0x10 # comment"),
            vec![
                Tok::Ident("loop".into()),
                Tok::Punct(':'),
                Tok::Ident("addi".into()),
                Tok::Ident("a0".into()),
                Tok::Punct(','),
                Tok::Ident("a0".into()),
                Tok::Punct(','),
                Tok::Punct('-'),
                Tok::Int(16),
                Tok::Newline,
            ]
        );
    }

    #[test]
    fn numbers_and_local_refs() {
        assert_eq!(
            toks("0b101 1b 2f 'A'")[..4],
            [
                Tok::Int(5),
                Tok::LocalRef(1, false),
                Tok::LocalRef(2, true),
                Tok::Int(65)
            ]
        );
    }

    #[test]
    fn strings() {
        assert_eq!(toks(r#".asciz "hi\n\x41""#)[1], Tok::Str(b"hi\nA".to_vec()));
        assert_eq!(
            toks(r#".ascii "\b\f\t\r\"\\""#)[1],
            Tok::Str(vec![8, 12, 9, 13, b'"', b'\\'])
        );
    }

    #[test]
    fn octal_escapes() {
        // One to three digits; an 8 or 9, or a fourth digit, is not part of it.
        // This is how clang writes bytes it can't print (`"\377"`).
        assert_eq!(
            toks(r#".ascii "\0\12\377a\2008\1234""#)[1],
            Tok::Str(vec![0, 0o12, 0o377, b'a', 0o200, b'8', 0o123, b'4'])
        );
        let err = lex(r#".ascii "\400""#).unwrap_err();
        assert!(err.to_string().contains("octal escape out of range"), "{err}");
    }
}
