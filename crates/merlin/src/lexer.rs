//! Turns C source text into tokens with line/column positions.
//!
//! Comments: `//` to end of line and `/* ... */`. No preprocessor yet: a `#`
//! line is an error. Trigraphs (`??=`...) and line splices (backslash-newline),
//! which C99 rewrites before tokenizing, are errors wherever they would change
//! what merlin reads.
//!
//! Integer literals get their C type here (C99 6.4.4.1, with `int` and `long`
//! both 32 bits): a decimal literal is `int` if it fits, else it would be
//! `long long`, which merlin rejects; a hex or octal one is `int` or else
//! `unsigned int`; `u`, `l` and `ul` suffixes pick from the unsigned and long
//! types the same way. Character literals are `int`s holding the byte (plain
//! `char` is unsigned, so `'\xff'` is 255). String literals are bytes, with
//! the escapes C has.

use crate::types::IntKind;
use crate::{Error, Pos};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kw {
    Void,
    Char,
    Short,
    Int,
    Long,
    Signed,
    Unsigned,
    Static,
    Extern,
    Sizeof,
    Return,
    If,
    Else,
    While,
    Do,
    For,
    Break,
    Continue,
}

/// Every keyword merlin handles, in [`Kw`] order.
pub const KEYWORDS: &[(Kw, &str)] = &[
    (Kw::Void, "void"),
    (Kw::Char, "char"),
    (Kw::Short, "short"),
    (Kw::Int, "int"),
    (Kw::Long, "long"),
    (Kw::Signed, "signed"),
    (Kw::Unsigned, "unsigned"),
    (Kw::Static, "static"),
    (Kw::Extern, "extern"),
    (Kw::Sizeof, "sizeof"),
    (Kw::Return, "return"),
    (Kw::If, "if"),
    (Kw::Else, "else"),
    (Kw::While, "while"),
    (Kw::Do, "do"),
    (Kw::For, "for"),
    (Kw::Break, "break"),
    (Kw::Continue, "continue"),
];

impl Kw {
    fn from_str(s: &str) -> Option<Kw> {
        KEYWORDS.iter().find(|(_, name)| *name == s).map(|(k, _)| *k)
    }

    pub fn as_str(self) -> &'static str {
        KEYWORDS
            .iter()
            .find(|(k, _)| *k == self)
            .expect("every keyword is listed")
            .1
    }

    /// Keywords that start a declaration: type specifiers and storage classes.
    pub fn starts_decl(self) -> bool {
        matches!(
            self,
            Kw::Void
                | Kw::Char
                | Kw::Short
                | Kw::Int
                | Kw::Long
                | Kw::Signed
                | Kw::Unsigned
                | Kw::Static
                | Kw::Extern
        )
    }
}

/// C keywords that merlin doesn't handle yet. Using one is a clear error rather
/// than a confusing parse error further on.
const UNSUPPORTED_KEYWORDS: &[&str] = &[
    "auto", "case", "const", "default", "double", "enum", "float", "goto", "inline", "register", "restrict",
    "struct", "switch", "typedef", "union", "volatile", "_Bool", "_Complex",
];

/// Punctuators, longest first so `<<=` wins over `<<` and `<`.
const PUNCTS: &[&str] = &[
    "<<=", ">>=", "==", "!=", "<=", ">=", "&&", "||", "++", "--", "+=", "-=", "*=", "/=", "%=", "&=", "|=",
    "^=", "<<", ">>", "(", ")", "{", "}", "[", "]", ";", ",", "=", "+", "-", "*", "/", "%", "<", ">", "!",
    "~", "&", "|", "^", "?", ":",
];

/// C punctuators merlin doesn't handle yet (structs, varargs). `->` is
/// matched before `-`, as C's longest-match rule demands, so `a->b` is an
/// error rather than `a - > b`.
const UNSUPPORTED_PUNCTS: &[&str] = &["...", "->", "."];

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Tok {
    Ident(String),
    /// An integer or character literal: its value (in range for its type) and type.
    Num(i64, IntKind),
    /// A string literal's bytes, escapes decoded, without the terminating NUL.
    Str(Vec<u8>),
    Kw(Kw),
    Punct(&'static str),
    Eof,
}

impl Tok {
    /// How the token reads in an error message.
    pub fn describe(&self) -> String {
        match self {
            Tok::Ident(s) => format!("identifier '{s}'"),
            Tok::Num(v, _) => format!("'{v}'"),
            Tok::Str(_) => "string literal".to_string(),
            Tok::Kw(k) => format!("'{}'", k.as_str()),
            Tok::Punct(p) => format!("'{p}'"),
            Tok::Eof => "end of file".to_string(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Token {
    pub tok: Tok,
    /// Position of the first character.
    pub pos: Pos,
    /// Position just past the last character (where a missing `;` belongs).
    pub end: Pos,
    /// The token as written in the source.
    pub text: String,
}

impl Token {
    /// How the token reads in an error message: as written, so `0x7f` stays `0x7f`.
    pub fn describe(&self) -> String {
        match self.tok {
            Tok::Num(..) => format!("'{}'", self.text),
            _ => self.tok.describe(),
        }
    }
}

struct Lexer {
    chars: Vec<char>,
    i: usize,
    line: u32,
    col: u32,
}

impl Lexer {
    fn peek(&self, n: usize) -> Option<char> {
        self.chars.get(self.i + n).copied()
    }

    /// Whether the input continues with `s`.
    fn at(&self, s: &str) -> bool {
        s.chars().enumerate().all(|(k, c)| self.peek(k) == Some(c))
    }

    fn pos(&self) -> Pos {
        Pos::new(self.line, self.col)
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek(0)?;
        self.i += 1;
        if c == '\n' {
            self.line += 1;
            self.col = 1;
        } else {
            self.col += 1;
        }
        Some(c)
    }

    /// Whether a trigraph (`??=` for `#`, `??/` for `\`, ...) starts here.
    /// C99 replaces them before anything else, so merlin rejects them rather
    /// than read `"??="` as three characters.
    fn at_trigraph(&self) -> bool {
        self.at("??") && self.peek(2).is_some_and(|c| "=(/)'<!>-".contains(c))
    }

    /// Whether a line splice starts here: `\` (or `??/`), optional blanks and a
    /// newline, which C99 deletes before tokenizing (clang allows the blanks).
    fn at_splice(&self) -> bool {
        let mut k = match self.peek(0) {
            Some('\\') => 1,
            Some('?') if self.at("??/") => 3,
            _ => return false,
        };
        while matches!(self.peek(k), Some(' ' | '\t' | '\r')) {
            k += 1;
        }
        self.peek(k) == Some('\n')
    }

    /// Errors for what C99's translation phases 1 and 2 would rewrite here.
    fn phase_1_2(&self) -> Result<(), Error> {
        if self.at_splice() {
            Err(Error::new(
                self.pos(),
                "line splices (backslash-newline) are not supported",
            ))
        } else if self.at_trigraph() {
            let msg = format!(
                "trigraphs are not supported ('{}'; write '\\?' for a '?' that starts one)",
                self.chars[self.i..self.i + 3].iter().collect::<String>()
            );
            Err(Error::new(self.pos(), msg))
        } else {
            Ok(())
        }
    }

    fn take_while(&mut self, f: impl Fn(char) -> bool) -> String {
        let mut s = String::new();
        while let Some(c) = self.peek(0).filter(|&c| f(c)) {
            s.push(c);
            self.bump();
        }
        s
    }

    /// Skips whitespace and comments.
    fn skip_trivia(&mut self) -> Result<(), Error> {
        loop {
            match (self.peek(0), self.peek(1)) {
                (Some(c), _) if c.is_whitespace() => {
                    self.bump();
                }
                (Some('/'), Some('/')) => {
                    while self.peek(0).is_some_and(|c| c != '\n') {
                        // A splice would continue the comment on the next line.
                        if self.at_splice() {
                            return self.phase_1_2();
                        }
                        self.bump();
                    }
                }
                (Some('/'), Some('*')) => {
                    let start = self.pos();
                    self.bump();
                    self.bump();
                    loop {
                        match (self.peek(0), self.peek(1)) {
                            (Some('*'), Some('/')) => {
                                self.bump();
                                self.bump();
                                break;
                            }
                            (Some(c), _) => {
                                self.bump();
                                // `*`, a splice and `/` would end the comment.
                                if c == '*' && self.at_splice() {
                                    return self.phase_1_2();
                                }
                            }
                            (None, _) => return Err(Error::new(start, "unterminated /* comment")),
                        }
                    }
                }
                _ => return Ok(()),
            }
        }
    }

    fn number(&mut self, pos: Pos) -> Result<Tok, Error> {
        let text = self.take_while(|c| c.is_ascii_alphanumeric() || c == '_');
        let lower = text.to_ascii_lowercase();
        // Split off the suffix: any mix of `u` and `l` at the end.
        let digits_end = lower.trim_end_matches(['u', 'l']).len();
        let (digits, suffix) = lower.split_at(digits_end);
        let (radix, digits) = if let Some(hex) = digits.strip_prefix("0x") {
            (16, hex)
        } else if digits.len() > 1 && digits.starts_with('0') {
            (8, &digits[1..])
        } else {
            (10, digits)
        };
        let invalid = || Error::new(pos, format!("invalid integer literal '{text}'"));
        if digits.is_empty() || !digits.chars().all(|c| c.is_digit(radix)) {
            return Err(invalid());
        }
        let (unsigned, long) = match suffix {
            "" => (false, false),
            "u" => (true, false),
            "l" => (false, true),
            "ul" | "lu" => (true, true),
            "ll" | "ull" | "llu" => {
                return Err(Error::new(
                    pos,
                    format!("'long long' is not supported yet: '{text}'"),
                ))
            }
            _ => return Err(invalid()),
        };
        // Saturate: anything past u32::MAX is too large for every type we have.
        let value = u64::from_str_radix(digits, radix).unwrap_or(u64::MAX);
        // The candidate types, in C's order; the first that holds the value wins.
        use IntKind::*;
        let candidates: &[IntKind] = match (unsigned, long, radix == 10) {
            (false, false, true) => &[Int, Long],
            (false, false, false) => &[Int, UInt, Long, ULong],
            (true, false, _) => &[UInt, ULong],
            (false, true, true) => &[Long],
            (false, true, false) => &[Long, ULong],
            (true, true, _) => &[ULong],
        };
        match candidates.iter().find(|k| value <= k.range().1 as u64) {
            Some(&k) => Ok(Tok::Num(value as i64, k)),
            None => Err(Error::new(
                pos,
                format!(
                    "integer literal '{text}' is too large for any type merlin has \
                     ('long long' is not supported yet)"
                ),
            )),
        }
    }

    /// One character of a character or string literal, after any escape.
    /// Characters outside ASCII come out as their UTF-8 bytes.
    fn literal_char(&mut self, quote: char, start: Pos, out: &mut Vec<u8>) -> Result<(), Error> {
        let pos = self.pos();
        let unterminated = || {
            let what = if quote == '"' { "string" } else { "character" };
            Error::new(start, format!("unterminated {what} literal"))
        };
        let c = match self.peek(0) {
            None | Some('\n') => return Err(unterminated()),
            Some(c) => c,
        };
        self.phase_1_2()?;
        self.bump();
        if c != '\\' {
            let mut buf = [0; 4];
            out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
            return Ok(());
        }
        // `\??/` is `\\` to C, not `\?` and `?/`.
        self.phase_1_2()?;
        let e = self.bump().ok_or_else(unterminated)?;
        let byte = match e {
            'n' => b'\n',
            't' => b'\t',
            'r' => b'\r',
            'a' => 7,
            'b' => 8,
            'f' => 12,
            'v' => 11,
            '\\' => b'\\',
            '\'' => b'\'',
            '"' => b'"',
            '?' => b'?',
            '0'..='7' => {
                // Up to three octal digits.
                let mut v = e.to_digit(8).unwrap();
                for _ in 0..2 {
                    match self.peek(0).and_then(|c| c.to_digit(8)) {
                        Some(d) => {
                            v = v * 8 + d;
                            self.bump();
                        }
                        None => break,
                    }
                }
                u8::try_from(v).map_err(|_| Error::new(pos, "octal escape sequence out of range"))?
            }
            'x' => {
                // As many hex digits as follow, as in C.
                let hex = self.take_while(|c| c.is_ascii_hexdigit());
                if hex.is_empty() {
                    return Err(Error::new(pos, "\\x used with no following hex digits"));
                }
                let v = u64::from_str_radix(&hex, 16).unwrap_or(u64::MAX);
                u8::try_from(v).map_err(|_| Error::new(pos, "hex escape sequence out of range"))?
            }
            _ => return Err(Error::new(pos, format!("unknown escape sequence '\\{e}'"))),
        };
        out.push(byte);
        Ok(())
    }

    fn char_literal(&mut self, pos: Pos) -> Result<Tok, Error> {
        self.bump(); // '
        if self.peek(0) == Some('\'') {
            return Err(Error::new(pos, "empty character constant"));
        }
        let mut bytes = Vec::new();
        while self.peek(0) != Some('\'') {
            self.literal_char('\'', pos, &mut bytes)?;
        }
        self.bump();
        match bytes[..] {
            [b] => Ok(Tok::Num(b as i64, IntKind::Int)),
            _ => Err(Error::new(
                pos,
                "character constants hold one byte in merlin (no multi-character constants)",
            )),
        }
    }

    fn string_literal(&mut self, pos: Pos) -> Result<Tok, Error> {
        self.bump(); // "
        let mut bytes = Vec::new();
        while self.peek(0) != Some('"') {
            self.literal_char('"', pos, &mut bytes)?;
        }
        self.bump();
        Ok(Tok::Str(bytes))
    }
}

pub fn lex(src: &str) -> Result<Vec<Token>, Error> {
    let mut lx = Lexer {
        chars: src.chars().collect(),
        i: 0,
        line: 1,
        col: 1,
    };
    let mut out = Vec::new();
    loop {
        lx.skip_trivia()?;
        let pos = lx.pos();
        let Some(c) = lx.peek(0) else {
            out.push(Token {
                tok: Tok::Eof,
                pos,
                end: pos,
                text: String::new(),
            });
            return Ok(out);
        };
        let start = lx.i;
        lx.phase_1_2()?;
        let tok = if c.is_ascii_digit() {
            lx.number(pos)?
        } else if c == '\'' {
            lx.char_literal(pos)?
        } else if c == '"' {
            lx.string_literal(pos)?
        } else if c.is_ascii_alphabetic() || c == '_' {
            let word = lx.take_while(|c| c.is_ascii_alphanumeric() || c == '_');
            if let Some(k) = Kw::from_str(&word) {
                Tok::Kw(k)
            } else if UNSUPPORTED_KEYWORDS.contains(&word.as_str()) {
                return Err(Error::new(pos, format!("'{word}' is not supported yet")));
            } else {
                Tok::Ident(word)
            }
        } else if let Some(p) = UNSUPPORTED_PUNCTS.iter().find(|p| lx.at(p)) {
            return Err(Error::new(pos, format!("'{p}' is not supported yet")));
        } else if let Some(p) = PUNCTS.iter().find(|p| lx.at(p)) {
            for _ in 0..p.len() {
                lx.bump();
            }
            Tok::Punct(p)
        } else if c == '#' {
            return Err(Error::new(pos, "preprocessor directives are not supported yet"));
        } else {
            return Err(Error::new(pos, format!("unexpected character '{c}'")));
        };
        out.push(Token {
            tok,
            pos,
            end: lx.pos(),
            text: lx.chars[start..lx.i].iter().collect(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use IntKind::*;

    fn toks(s: &str) -> Vec<Tok> {
        lex(s).unwrap().into_iter().map(|t| t.tok).collect()
    }

    fn err(s: &str) -> String {
        lex(s).unwrap_err().to_string()
    }

    fn num(s: &str) -> (i64, IntKind) {
        match toks(s)[..] {
            [Tok::Num(v, k), Tok::Eof] => (v, k),
            ref t => panic!("{s}: {t:?}"),
        }
    }

    #[test]
    fn keywords_idents_puncts() {
        assert_eq!(
            toks("int main() { return x_1 <= 0; }"),
            vec![
                Tok::Kw(Kw::Int),
                Tok::Ident("main".into()),
                Tok::Punct("("),
                Tok::Punct(")"),
                Tok::Punct("{"),
                Tok::Kw(Kw::Return),
                Tok::Ident("x_1".into()),
                Tok::Punct("<="),
                Tok::Num(0, Int),
                Tok::Punct(";"),
                Tok::Punct("}"),
                Tok::Eof,
            ]
        );
    }

    #[test]
    fn every_punctuator() {
        let src = PUNCTS.join(" ");
        let got: Vec<Tok> = toks(&src).into_iter().filter(|t| *t != Tok::Eof).collect();
        let want: Vec<Tok> = PUNCTS.iter().map(|p| Tok::Punct(p)).collect();
        assert_eq!(got, want);
        // Longest match, no spaces needed.
        let p = |s: &str| -> Vec<String> {
            toks(s)
                .into_iter()
                .map(|t| match t {
                    Tok::Punct(p) => p.to_string(),
                    Tok::Ident(s) => s,
                    Tok::Eof => "$".into(),
                    t => panic!("{t:?}"),
                })
                .collect()
        };
        assert_eq!(
            p("a==b!=!c&&d||e"),
            ["a", "==", "b", "!=", "!", "c", "&&", "d", "||", "e", "$"]
        );
        assert_eq!(p("a<<=b>>c"), ["a", "<<=", "b", ">>", "c", "$"]);
        // C's longest match: `a+++b` is `a ++ + b`, `a---b` is `a -- - b`.
        assert_eq!(p("a+++b"), ["a", "++", "+", "b", "$"]);
        assert_eq!(p("a---b"), ["a", "--", "-", "b", "$"]);
        assert_eq!(p("a&&&b"), ["a", "&&", "&", "b", "$"]);
    }

    #[test]
    fn all_keywords() {
        let src: Vec<&str> = KEYWORDS.iter().map(|(_, s)| *s).collect();
        let got = toks(&src.join(" "));
        let want: Vec<Tok> = KEYWORDS.iter().map(|(k, _)| Tok::Kw(*k)).collect();
        assert_eq!(got[..KEYWORDS.len()], want[..]);
        for (k, s) in KEYWORDS {
            assert_eq!(k.as_str(), *s);
        }
        // A keyword prefix is just an identifier.
        assert_eq!(toks("integer")[0], Tok::Ident("integer".into()));
    }

    #[test]
    fn integer_literal_types() {
        assert_eq!(num("0"), (0, Int));
        assert_eq!(num("42"), (42, Int));
        assert_eq!(num("0x2A"), (42, Int));
        assert_eq!(num("052"), (42, Int));
        assert_eq!(num("2147483647"), (0x7fff_ffff, Int));
        // Hex and octal that don't fit int are unsigned int, as in C.
        assert_eq!(num("0x80000000"), (0x8000_0000, UInt));
        assert_eq!(num("0xFFFFFFFF"), (0xffff_ffff, UInt));
        assert_eq!(num("037777777777"), (0xffff_ffff, UInt));
        // Suffixes.
        assert_eq!(num("10u"), (10, UInt));
        assert_eq!(num("10U"), (10, UInt));
        assert_eq!(num("4294967295u"), (0xffff_ffff, UInt));
        assert_eq!(num("10l"), (10, Long));
        assert_eq!(num("0xffffffffL"), (0xffff_ffff, ULong));
        assert_eq!(num("10ul"), (10, ULong));
        assert_eq!(num("10LU"), (10, ULong));
        // A decimal literal past INT_MAX would be long long in C.
        let too_large = |lit: &str| {
            format!(
                "1:1: integer literal '{lit}' is too large for any type merlin has ('long long' is not supported yet)"
            )
        };
        for lit in [
            "2147483648",
            "2147483648l",
            "4294967296u",
            "0x100000000",
            "99999999999999999999",
        ] {
            assert_eq!(err(lit), too_large(lit));
        }
        assert_eq!(err("10ll"), "1:1: 'long long' is not supported yet: '10ll'");
        assert_eq!(err("x = 09;"), "1:5: invalid integer literal '09'");
        assert_eq!(err("12abc"), "1:1: invalid integer literal '12abc'");
        assert_eq!(err("0x"), "1:1: invalid integer literal '0x'");
        assert_eq!(err("1uu"), "1:1: invalid integer literal '1uu'");
    }

    #[test]
    fn character_literals() {
        assert_eq!(num("'a'"), (97, Int));
        assert_eq!(num("'\\n'"), (10, Int));
        assert_eq!(num("'\\0'"), (0, Int));
        assert_eq!(num("'\\''"), (39, Int));
        assert_eq!(num("'\"'"), (34, Int));
        assert_eq!(num("'\\\\'"), (92, Int));
        assert_eq!(num("'\\x41'"), (65, Int));
        assert_eq!(num("'\\101'"), (65, Int));
        // Plain char is unsigned on RISC-V: '\xff' is 255, not -1.
        assert_eq!(num("'\\xff'"), (255, Int));
        assert_eq!(num("'\\377'"), (255, Int));
        for (e, v) in [
            ("a", 7),
            ("b", 8),
            ("f", 12),
            ("v", 11),
            ("t", 9),
            ("r", 13),
            ("?", 63),
        ] {
            assert_eq!(num(&format!("'\\{e}'")), (v, Int));
        }
        assert_eq!(err("''"), "1:1: empty character constant");
        assert_eq!(
            err("'ab'"),
            "1:1: character constants hold one byte in merlin (no multi-character constants)"
        );
        assert_eq!(err("'a"), "1:1: unterminated character literal");
        assert_eq!(err("x = '\\q';"), "1:6: unknown escape sequence '\\q'");
        assert_eq!(err("'\\x100'"), "1:2: hex escape sequence out of range");
        assert_eq!(err("'\\400'"), "1:2: octal escape sequence out of range");
        assert_eq!(err("'\\x'"), "1:2: \\x used with no following hex digits");
    }

    #[test]
    fn string_literals() {
        assert_eq!(toks(r#""hi\n""#), [Tok::Str(b"hi\n".to_vec()), Tok::Eof]);
        assert_eq!(toks(r#""""#), [Tok::Str(vec![]), Tok::Eof]);
        assert_eq!(
            toks(r#""a\0b\x41\101\"\\""#),
            [Tok::Str(b"a\0bAA\"\\".to_vec()), Tok::Eof]
        );
        // \x takes every hex digit that follows; octal stops at three.
        assert_eq!(toks(r#""\1234""#), [Tok::Str(b"S4".to_vec()), Tok::Eof]);
        assert_eq!(toks("\"é\""), [Tok::Str("é".as_bytes().to_vec()), Tok::Eof]);
        assert_eq!(err("\"abc\nd\""), "1:1: unterminated string literal");
        assert_eq!(err("\"abc"), "1:1: unterminated string literal");
        let t = lex(r#"x = "a\tb";"#).unwrap();
        assert_eq!(t[2].text, r#""a\tb""#);
        assert_eq!(t[2].describe(), "string literal");
    }

    #[test]
    fn unsupported_punctuators_are_not_split() {
        assert_eq!(err("a->b"), "1:2: '->' is not supported yet");
        assert_eq!(err("s.x"), "1:2: '.' is not supported yet");
        assert_eq!(err("f(int a, ...)"), "1:10: '...' is not supported yet");
        // With a space they are separate tokens, in C too.
        assert_eq!(
            toks("- -x"),
            vec![Tok::Punct("-"), Tok::Punct("-"), Tok::Ident("x".into()), Tok::Eof]
        );
    }

    #[test]
    fn tokens_keep_their_source_text() {
        let t = lex("0x7f 017 x1 <= 'a' 10u").unwrap();
        let text: Vec<&str> = t.iter().map(|t| t.text.as_str()).collect();
        assert_eq!(text, ["0x7f", "017", "x1", "<=", "'a'", "10u", ""]);
        assert_eq!(t[0].describe(), "'0x7f'");
        assert_eq!(t[2].describe(), "identifier 'x1'");
        assert_eq!(t[4].describe(), "''a''");
    }

    #[test]
    fn comments_and_positions() {
        let src = "// line comment\n  /* block\n comment */ x /**/ +\n\t1";
        let t = lex(src).unwrap();
        let got: Vec<(Tok, Pos, Pos)> = t.into_iter().map(|t| (t.tok, t.pos, t.end)).collect();
        assert_eq!(
            got,
            vec![
                (Tok::Ident("x".into()), Pos::new(3, 13), Pos::new(3, 14)),
                (Tok::Punct("+"), Pos::new(3, 20), Pos::new(3, 21)),
                (Tok::Num(1, Int), Pos::new(4, 2), Pos::new(4, 3)),
                (Tok::Eof, Pos::new(4, 3), Pos::new(4, 3)),
            ]
        );
    }

    #[test]
    fn errors() {
        assert_eq!(err("int x;\n  /* oops"), "2:3: unterminated /* comment");
        assert_eq!(err("a @ b"), "1:3: unexpected character '@'");
        assert_eq!(
            err("#include <x.h>"),
            "1:1: preprocessor directives are not supported yet"
        );
        assert_eq!(
            err("int f() { struct s; }"),
            "1:11: 'struct' is not supported yet"
        );
        assert_eq!(err("const int x;"), "1:1: 'const' is not supported yet");
    }

    #[test]
    fn trigraphs_and_splices_are_errors() {
        let trigraph =
            |t: &str| format!("trigraphs are not supported ('{t}'; write '\\?' for a '?' that starts one)");
        // C reads "??=" as "#": an error, never three characters.
        assert_eq!(err(r#"char *s = "a??=";"#), format!("1:13: {}", trigraph("??=")));
        assert_eq!(err("int c = '??'';"), format!("1:10: {}", trigraph("??'")));
        assert_eq!(err(r#"int c = "\??/";"#), format!("1:11: {}", trigraph("??/")));
        assert_eq!(err("x ??- y"), format!("1:3: {}", trigraph("??-")));
        // A '?' that doesn't start a trigraph is fine, and so is '\?'.
        assert_eq!(
            toks(r#""???" "?\?=""#)[..2],
            [Tok::Str(b"???".to_vec()), Tok::Str(b"??=".to_vec())]
        );
        let splice = "line splices (backslash-newline) are not supported";
        assert_eq!(err("char *s = \"a\\\nb\";"), format!("1:13: {splice}"));
        assert_eq!(err("char *s = \"a\\ \r\nb\";"), format!("1:13: {splice}"));
        assert_eq!(err("in\\\nt x;"), format!("1:3: {splice}"));
        // A splice would carry a // comment on to the next line.
        assert_eq!(err("x; // note \\\ny = 2;"), format!("1:12: {splice}"));
        assert_eq!(err("x; // note ??/  \r\ny = 2;"), format!("1:12: {splice}"));
        assert_eq!(err("/* a *\\\n/ x"), format!("1:7: {splice}"));
        // Elsewhere in comments, both mean nothing.
        assert_eq!(toks("// a \\ b ??= ??/ c\n/* \\\n ??/\n **/ x").len(), 2);
    }

    #[test]
    fn crlf_line_endings() {
        let t = lex("int\r\nx").unwrap();
        assert_eq!(t[1].pos, Pos::new(2, 1));
    }
}
