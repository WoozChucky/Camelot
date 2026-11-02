//! Turns C source text into tokens with line/column positions.
//!
//! Comments: `//` to end of line and `/* ... */`. No preprocessor yet: a `#`
//! line is an error.

use crate::{Error, Pos};
use std::num::IntErrorKind;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kw {
    Int,
    Void,
    Return,
    If,
    Else,
    While,
    For,
    Break,
    Continue,
}

impl Kw {
    fn from_str(s: &str) -> Option<Kw> {
        Some(match s {
            "int" => Kw::Int,
            "void" => Kw::Void,
            "return" => Kw::Return,
            "if" => Kw::If,
            "else" => Kw::Else,
            "while" => Kw::While,
            "for" => Kw::For,
            "break" => Kw::Break,
            "continue" => Kw::Continue,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Kw::Int => "int",
            Kw::Void => "void",
            Kw::Return => "return",
            Kw::If => "if",
            Kw::Else => "else",
            Kw::While => "while",
            Kw::For => "for",
            Kw::Break => "break",
            Kw::Continue => "continue",
        }
    }
}

/// C keywords that merlin doesn't handle yet. Using one is a clear error rather
/// than a confusing parse error further on.
const UNSUPPORTED_KEYWORDS: &[&str] = &[
    "auto", "case", "char", "const", "default", "do", "double", "enum", "extern", "float", "goto", "inline",
    "long", "register", "restrict", "short", "signed", "sizeof", "static", "struct", "switch", "typedef",
    "union", "unsigned", "volatile",
];

/// Punctuators, longest first so `==` wins over `=`.
const PUNCTS: &[&str] = &[
    "==", "!=", "<=", ">=", "&&", "||", "(", ")", "{", "}", ";", ",", "=", "+", "-", "*", "/", "%", "<", ">",
    "!",
];

/// C punctuators merlin doesn't handle yet that start with a supported one. They
/// are matched first, as C's longest-match rule demands: otherwise `--x` would
/// lex as `- -x` and mean something else than in C, and `a--b` would be accepted.
const UNSUPPORTED_PUNCTS: &[&str] = &[
    "++", "--", "+=", "-=", "*=", "/=", "%=", "->", "<<", ">>", "&=", "|=", "^=",
];

/// The largest literal merlin accepts. C gives bigger literals type `unsigned
/// int` or `long long`, which merlin doesn't have yet.
pub const MAX_LITERAL: u64 = i32::MAX as u64;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Tok {
    Ident(String),
    /// An integer literal, at most [`MAX_LITERAL`], so it is an `int` as in C.
    Int(i64),
    Kw(Kw),
    Punct(&'static str),
    Eof,
}

impl Tok {
    /// How the token reads in an error message.
    pub fn describe(&self) -> String {
        match self {
            Tok::Ident(s) => format!("identifier '{s}'"),
            Tok::Int(v) => format!("'{v}'"),
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
            Tok::Int(_) => format!("'{}'", self.text),
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
                            (Some(_), _) => {
                                self.bump();
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
        let parsed = if let Some(hex) = lower.strip_prefix("0x") {
            u64::from_str_radix(hex, 16)
        } else if lower.len() > 1 && lower.starts_with('0') {
            u64::from_str_radix(&lower[1..], 8)
        } else {
            lower.parse::<u64>()
        };
        let too_large = || {
            Error::new(
                pos,
                format!(
                    "integer literal '{text}' is too large for int (unsigned and long are not supported yet)"
                ),
            )
        };
        match parsed {
            Ok(v) if v <= MAX_LITERAL => Ok(Tok::Int(v as i64)),
            Ok(_) => Err(too_large()),
            Err(e) if *e.kind() == IntErrorKind::PosOverflow => Err(too_large()),
            Err(_) if lower.ends_with(['u', 'l']) && text.starts_with(|c: char| c.is_ascii_digit()) => Err(
                Error::new(pos, format!("integer suffixes are not supported yet: '{text}'")),
            ),
            Err(_) => Err(Error::new(pos, format!("invalid integer literal '{text}'"))),
        }
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
        let tok = if c.is_ascii_digit() {
            lx.number(pos)?
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

    fn toks(s: &str) -> Vec<Tok> {
        lex(s).unwrap().into_iter().map(|t| t.tok).collect()
    }

    fn err(s: &str) -> String {
        lex(s).unwrap_err().to_string()
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
                Tok::Int(0),
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
        assert_eq!(
            toks("a==b!=!c&&d||e"),
            vec![
                Tok::Ident("a".into()),
                Tok::Punct("=="),
                Tok::Ident("b".into()),
                Tok::Punct("!="),
                Tok::Punct("!"),
                Tok::Ident("c".into()),
                Tok::Punct("&&"),
                Tok::Ident("d".into()),
                Tok::Punct("||"),
                Tok::Ident("e".into()),
                Tok::Eof,
            ]
        );
    }

    #[test]
    fn all_keywords() {
        let ks = [
            Kw::Int,
            Kw::Void,
            Kw::Return,
            Kw::If,
            Kw::Else,
            Kw::While,
            Kw::For,
            Kw::Break,
            Kw::Continue,
        ];
        let src: Vec<&str> = ks.iter().map(|k| k.as_str()).collect();
        let got = toks(&src.join(" "));
        assert_eq!(got[..ks.len()], ks.map(Tok::Kw));
        // A keyword prefix is just an identifier.
        assert_eq!(toks("integer")[0], Tok::Ident("integer".into()));
    }

    #[test]
    fn numbers() {
        assert_eq!(
            toks("0 42 0x2A 0X2a 052 2147483647 0x7FFFFFFF 017777777777"),
            vec![
                Tok::Int(0),
                Tok::Int(42),
                Tok::Int(42),
                Tok::Int(42),
                Tok::Int(42),
                Tok::Int(0x7fff_ffff),
                Tok::Int(0x7fff_ffff),
                Tok::Int(0x7fff_ffff),
                Tok::Eof
            ]
        );
        // In C these are `long long` (decimal) or `unsigned int` (hex, octal), not
        // `int`, so they would compare and divide differently.
        let too_large = |lit: &str| {
            format!(
                "1:1: integer literal '{lit}' is too large for int (unsigned and long are not supported yet)"
            )
        };
        for lit in [
            "2147483648",
            "0x80000000",
            "020000000000",
            "4294967295",
            "0xFFFFFFFF",
            "4294967296",
            "99999999999999999999",
            "0x10000000000000000",
        ] {
            assert_eq!(err(lit), too_large(lit));
        }
        assert_eq!(err("x = 09;"), "1:5: invalid integer literal '09'");
        assert_eq!(err("12abc"), "1:1: invalid integer literal '12abc'");
        assert_eq!(err("10u"), "1:1: integer suffixes are not supported yet: '10u'");
    }

    #[test]
    fn unsupported_punctuators_are_not_split() {
        // C lexes the longest punctuator, so `--x` is a decrement, not `-(-x)`, and
        // `a--b` is a syntax error. Splitting them would change what a program means.
        assert_eq!(err("--x"), "1:1: '--' is not supported yet");
        assert_eq!(err("x--;"), "1:2: '--' is not supported yet");
        assert_eq!(err("a--b"), "1:2: '--' is not supported yet");
        assert_eq!(err("a ++b"), "1:3: '++' is not supported yet");
        for p in UNSUPPORTED_PUNCTS {
            assert_eq!(
                err(&format!("a {p} b")),
                format!("1:3: '{p}' is not supported yet")
            );
        }
        // With a space they are two tokens, in C too.
        assert_eq!(
            toks("- -x"),
            vec![Tok::Punct("-"), Tok::Punct("-"), Tok::Ident("x".into()), Tok::Eof]
        );
        assert_eq!(
            toks("a=-b"),
            vec![
                Tok::Ident("a".into()),
                Tok::Punct("="),
                Tok::Punct("-"),
                Tok::Ident("b".into()),
                Tok::Eof
            ]
        );
    }

    #[test]
    fn tokens_keep_their_source_text() {
        let t = lex("0x7f 017 x1 <=").unwrap();
        let text: Vec<&str> = t.iter().map(|t| t.text.as_str()).collect();
        assert_eq!(text, ["0x7f", "017", "x1", "<=", ""]);
        assert_eq!(t[0].describe(), "'0x7f'");
        assert_eq!(t[2].describe(), "identifier 'x1'");
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
                (Tok::Int(1), Pos::new(4, 2), Pos::new(4, 3)),
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
        assert_eq!(err("int f() { char c; }"), "1:11: 'char' is not supported yet");
        assert_eq!(err("a & b"), "1:3: unexpected character '&'");
    }

    #[test]
    fn crlf_line_endings() {
        let t = lex("int\r\nx").unwrap();
        assert_eq!(t[1].pos, Pos::new(2, 1));
    }
}
