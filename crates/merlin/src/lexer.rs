//! Turns C source text into tokens with line/column positions, in two steps,
//! as C99's translation phases go:
//!
//! 1. [`lex_pp`] deletes line splices (a backslash at the end of a line,
//!    phase 2) and splits the text into *preprocessing tokens* (phase 3):
//!    identifiers, preprocessing numbers, character constants and string
//!    literals as written, punctuators (`#` and `##` included). Each knows
//!    whether it starts a line and whether a space comes before it, which
//!    the preprocessor ([`crate::preprocess`]) needs.
//! 2. [`finish`], after preprocessing, gives each one its meaning (phase 7):
//!    keywords, integer constants with their C type, the bytes of literals.
//!
//! Comments (`//` to end of line and `/* ... */`) count as spaces.
//! Trigraphs (`??=`...), which C99 rewrites before anything else, are
//! errors wherever they would change what merlin reads.
//!
//! Integer literals get their C type in [`finish`] (C99 6.4.4.1, with `int`
//! and `long` both 32 bits): a decimal literal is `int` if it fits, else it
//! would be `long long`, which merlin rejects; a hex or octal one is `int` or
//! else `unsigned int`; `u`, `l` and `ul` suffixes pick from the unsigned
//! and long types the same way. Character literals are `int`s holding the
//! byte (plain `char` is unsigned, so `'ÿ'` is 255). String literals are
//! bytes, with the escapes C has.

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
    Bool,
    Struct,
    Union,
    Enum,
    Const,
    Volatile,
    Restrict,
    Static,
    Extern,
    Typedef,
    Inline,
    Sizeof,
    Return,
    If,
    Else,
    While,
    Do,
    For,
    Break,
    Continue,
    Switch,
    Case,
    Default,
    Goto,
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
    (Kw::Bool, "_Bool"),
    (Kw::Struct, "struct"),
    (Kw::Union, "union"),
    (Kw::Enum, "enum"),
    (Kw::Const, "const"),
    (Kw::Volatile, "volatile"),
    (Kw::Restrict, "restrict"),
    (Kw::Static, "static"),
    (Kw::Extern, "extern"),
    (Kw::Typedef, "typedef"),
    (Kw::Inline, "inline"),
    (Kw::Sizeof, "sizeof"),
    (Kw::Return, "return"),
    (Kw::If, "if"),
    (Kw::Else, "else"),
    (Kw::While, "while"),
    (Kw::Do, "do"),
    (Kw::For, "for"),
    (Kw::Break, "break"),
    (Kw::Continue, "continue"),
    (Kw::Switch, "switch"),
    (Kw::Case, "case"),
    (Kw::Default, "default"),
    (Kw::Goto, "goto"),
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

    /// Keywords that start a declaration: type specifiers and qualifiers,
    /// storage classes and `inline`. (A typedef name starts one too, which
    /// only the parser knows.)
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
                | Kw::Bool
                | Kw::Struct
                | Kw::Union
                | Kw::Enum
                | Kw::Const
                | Kw::Volatile
                | Kw::Restrict
                | Kw::Static
                | Kw::Extern
                | Kw::Typedef
                | Kw::Inline
        )
    }
}

/// C keywords that merlin doesn't handle yet. Using one is a clear error rather
/// than a confusing parse error further on.
const UNSUPPORTED_KEYWORDS: &[&str] = &["auto", "double", "float", "register", "_Complex", "_Imaginary"];

/// Punctuators, longest first so `<<=` wins over `<<` and `<`, `->` over `-`
/// (`a->b` is never `a - > b`), `...` over `.` and `##` over `#`. `#` and
/// `##` only mean something to the preprocessor.
const PUNCTS: &[&str] = &[
    "...", "<<=", ">>=", "->", "==", "!=", "<=", ">=", "&&", "||", "++", "--", "+=", "-=", "*=", "/=", "%=",
    "&=", "|=", "^=", "<<", ">>", "##", "(", ")", "{", "}", "[", "]", ";", ",", "=", "+", "-", "*", "/", "%",
    "<", ">", "!", "~", "&", "|", "^", "?", ":", ".", "#",
];

/// What kind of preprocessing token (C99 6.4) a [`PpToken`] is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PpKind {
    /// An identifier; keywords too, which only [`finish`] tells apart (to
    /// the preprocessor `int` is a name like any other).
    Ident,
    /// A preprocessing number (C99 6.4.8): a digit, or `.` and a digit,
    /// then any letters, digits, `_`, `.` and `e+ e- E+ E- p+ p- P+ P-`.
    /// [`finish`] reads it as an integer constant.
    Number,
    /// A character constant, as written (quotes and escapes included).
    Char,
    /// A string literal, as written.
    Str,
    Punct(&'static str),
    /// Text that is no token (an unterminated literal, a stray `@`), with
    /// the error it is. It is only an error if it reaches the parser: a
    /// group that `#if 0` skips may hold anything, like an apostrophe in a
    /// sentence.
    Invalid(String),
    /// The preprocessor's stand-in for an empty macro argument next to `##`
    /// (C99 6.10.3.3); never in the lexer's output, never in the
    /// preprocessor's.
    Placemarker,
}

/// A preprocessing token, as the preprocessor sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PpToken {
    pub kind: PpKind,
    /// The token as written, line splices removed.
    pub text: String,
    /// Position of the first character.
    pub pos: Pos,
    /// Position just past the last character.
    pub end: Pos,
    /// The first token of its line: a `#` here starts a directive.
    pub bol: bool,
    /// Whitespace or a comment comes right before it. Stringizing (`#x`)
    /// turns that into one space, and `-E` output keeps it.
    pub space: bool,
}

impl PpToken {
    pub fn is_punct(&self, p: &str) -> bool {
        matches!(self.kind, PpKind::Punct(q) if q == p)
    }

    /// The identifier's name, if it is one.
    pub fn ident(&self) -> Option<&str> {
        (self.kind == PpKind::Ident).then_some(self.text.as_str())
    }
}

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

/// The source after C99's translation phase 2: line splices (a backslash
/// right before a newline) deleted, every remaining character with its
/// position in the original text, so that tokens joined by a splice still
/// say where they were written.
struct Spliced {
    chars: Vec<char>,
    at: Vec<Pos>,
    /// Just past the last character.
    end: Pos,
}

/// Deletes line splices. Blanks between the backslash and the newline are
/// allowed, as clang allows them.
fn splice(src: &str, file: u32) -> Spliced {
    let src: Vec<char> = src.chars().collect();
    let (mut chars, mut at) = (Vec::with_capacity(src.len()), Vec::with_capacity(src.len()));
    let (mut line, mut col) = (1, 1);
    let mut k = 0;
    while k < src.len() {
        if src[k] == '\\' {
            let mut j = k + 1;
            while matches!(src.get(j), Some(' ' | '\t' | '\r')) {
                j += 1;
            }
            if src.get(j) == Some(&'\n') {
                line += 1;
                col = 1;
                k = j + 1;
                continue;
            }
        }
        chars.push(src[k]);
        at.push(Pos::in_file(file, line, col));
        if src[k] == '\n' {
            line += 1;
            col = 1;
        } else {
            col += 1;
        }
        k += 1;
    }
    Spliced {
        chars,
        at,
        end: Pos::in_file(file, line, col),
    }
}

struct Lexer {
    src: Spliced,
    i: usize,
    /// A newline was skipped since the last token.
    newline: bool,
    /// Whitespace or a comment was skipped since the last token.
    space: bool,
}

impl Lexer {
    fn peek(&self, n: usize) -> Option<char> {
        self.src.chars.get(self.i + n).copied()
    }

    /// Whether the input continues with `s`.
    fn at(&self, s: &str) -> bool {
        s.chars().enumerate().all(|(k, c)| self.peek(k) == Some(c))
    }

    fn pos(&self) -> Pos {
        self.src.at.get(self.i).copied().unwrap_or(self.src.end)
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek(0)?;
        self.i += 1;
        Some(c)
    }

    /// Whether a trigraph (`??=` for `#`, `??/` for `\`, ...) starts here.
    /// C99 replaces them before anything else, so merlin rejects them rather
    /// than read `"??="` as three characters.
    fn at_trigraph(&self) -> bool {
        self.at("??") && self.peek(2).is_some_and(|c| "=(/)'<!>-".contains(c))
    }

    /// The error for a trigraph here, if one starts here.
    fn trigraph(&self) -> Result<(), Error> {
        if !self.at_trigraph() {
            return Ok(());
        }
        let msg = format!(
            "trigraphs are not supported ('{}'; write '\\?' for a '?' that starts one)",
            self.src.chars[self.i..self.i + 3].iter().collect::<String>()
        );
        Err(Error::new(self.pos(), msg))
    }

    /// Skips whitespace and comments, noting newlines and spaces.
    fn skip_trivia(&mut self) -> Result<(), Error> {
        loop {
            match (self.peek(0), self.peek(1)) {
                (Some(c), _) if c.is_whitespace() => {
                    self.newline |= c == '\n';
                    self.space = true;
                    self.bump();
                }
                (Some('/'), Some('/')) => {
                    self.space = true;
                    while self.peek(0).is_some_and(|c| c != '\n') {
                        // `??/` and a newline would be a line splice, which
                        // continues the comment on the next line.
                        if self.at("??/") {
                            let mut k = 3;
                            while matches!(self.peek(k), Some(' ' | '\t' | '\r')) {
                                k += 1;
                            }
                            if self.peek(k) == Some('\n') {
                                self.trigraph()?;
                            }
                        }
                        self.bump();
                    }
                }
                (Some('/'), Some('*')) => {
                    self.space = true;
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

    /// A preprocessing number, from its first character.
    fn number(&mut self) {
        self.bump();
        while let Some(c) = self.peek(0) {
            if "eEpP".contains(c) && matches!(self.peek(1), Some('+' | '-')) {
                self.bump();
                self.bump();
            } else if c.is_ascii_alphanumeric() || c == '_' || c == '.' {
                self.bump();
            } else {
                break;
            }
        }
    }

    /// A character constant or string literal, from its opening quote, up to
    /// the closing one (escapes are decoded later, by [`finish`]). Without
    /// the closing quote on the same line it is an invalid token running to
    /// the end of the line.
    fn quoted(&mut self, quote: char) -> Result<PpKind, Error> {
        self.bump();
        loop {
            match self.peek(0) {
                None | Some('\n') => {
                    let what = if quote == '"' { "string" } else { "character" };
                    return Ok(PpKind::Invalid(format!("unterminated {what} literal")));
                }
                Some(c) => {
                    self.trigraph()?;
                    self.bump();
                    if c == quote {
                        return Ok(if quote == '"' { PpKind::Str } else { PpKind::Char });
                    }
                    if c == '\\' && self.peek(0).is_some_and(|c| c != '\n') {
                        // `\??/` is `\\` to C, not `\?` and `?/`.
                        self.trigraph()?;
                        self.bump();
                    }
                }
            }
        }
    }
}

/// Splits one file (`file` is its number, see [`Pos`]) into preprocessing
/// tokens, and gives the position of its end. Only an unterminated `/*`
/// comment and trigraphs are errors here; anything else that is no token
/// becomes a [`PpKind::Invalid`] one.
pub fn lex_pp(src: &str, file: u32) -> Result<(Vec<PpToken>, Pos), Error> {
    let mut lx = Lexer {
        src: splice(src, file),
        i: 0,
        newline: true,
        space: false,
    };
    let mut out = Vec::new();
    loop {
        lx.skip_trivia()?;
        let pos = lx.pos();
        let Some(c) = lx.peek(0) else {
            return Ok((out, lx.src.end));
        };
        lx.trigraph()?;
        let start = lx.i;
        let kind = if c.is_ascii_digit() || (c == '.' && lx.peek(1).is_some_and(|d| d.is_ascii_digit())) {
            lx.number();
            PpKind::Number
        } else if c == '\'' || c == '"' {
            lx.quoted(c)?
        } else if c.is_ascii_alphabetic() || c == '_' {
            while lx.peek(0).is_some_and(|c| c.is_ascii_alphanumeric() || c == '_') {
                lx.bump();
            }
            PpKind::Ident
        } else if let Some(p) = PUNCTS.iter().find(|p| lx.at(p)) {
            for _ in 0..p.len() {
                lx.bump();
            }
            PpKind::Punct(p)
        } else {
            lx.bump();
            PpKind::Invalid(format!("unexpected character '{c}'"))
        };
        // Just past the last character: after a splice inside the token,
        // still on the line where it ends.
        let end = lx.src.at[lx.i - 1];
        let end = Pos::in_file(end.file, end.line, end.col + 1);
        out.push(PpToken {
            kind,
            text: lx.src.chars[start..lx.i].iter().collect(),
            pos,
            end,
            bol: std::mem::take(&mut lx.newline),
            space: std::mem::take(&mut lx.space),
        });
    }
}

/// Lexes a whole translation unit without preprocessing it: [`lex_pp`]
/// then [`finish`]. A `#` is an error.
pub fn lex(src: &str) -> Result<Vec<Token>, Error> {
    let (toks, end) = lex_pp(src, 0)?;
    finish(toks, end)
}

/// Turns preprocessing tokens into the parser's tokens (C99 translation
/// phase 7) and ends them with [`Tok::Eof`] at `end`: identifiers that are
/// keywords become keywords, numbers integer constants with their C type,
/// character constants and string literals their bytes. What is left of the
/// preprocessor's syntax (`#`, `##`) and invalid tokens are errors.
pub fn finish(toks: Vec<PpToken>, end: Pos) -> Result<Vec<Token>, Error> {
    let mut out = Vec::with_capacity(toks.len() + 1);
    for t in toks {
        let tok = match &t.kind {
            PpKind::Ident => {
                if let Some(k) = Kw::from_str(&t.text) {
                    Tok::Kw(k)
                } else if UNSUPPORTED_KEYWORDS.contains(&t.text.as_str()) {
                    return Err(Error::new(t.pos, format!("'{}' is not supported yet", t.text)));
                } else {
                    Tok::Ident(t.text.clone())
                }
            }
            PpKind::Number => number(&t.text, t.pos)?,
            PpKind::Char => {
                let bytes = decode(&t.text, t.pos)?;
                match bytes[..] {
                    [b] => Tok::Num(b as i64, IntKind::Int),
                    [] => return Err(Error::new(t.pos, "empty character constant")),
                    _ => {
                        return Err(Error::new(
                            t.pos,
                            "character constants hold one byte in merlin (no multi-character constants)",
                        ))
                    }
                }
            }
            PpKind::Str => Tok::Str(decode(&t.text, t.pos)?),
            PpKind::Punct(p @ ("#" | "##")) => {
                return Err(Error::new(
                    t.pos,
                    format!("'{p}' is only allowed in preprocessor directives"),
                ))
            }
            PpKind::Punct(p) => Tok::Punct(p),
            PpKind::Invalid(msg) => return Err(Error::new(t.pos, msg.clone())),
            PpKind::Placemarker => continue,
        };
        out.push(Token {
            tok,
            pos: t.pos,
            end: t.end,
            text: t.text,
        });
    }
    out.push(Token {
        tok: Tok::Eof,
        pos: end,
        end,
        text: String::new(),
    });
    Ok(out)
}

/// An integer constant and its C type, from a preprocessing number.
pub fn number(text: &str, pos: Pos) -> Result<Tok, Error> {
    let IntLit {
        value,
        decimal,
        unsigned,
        long,
    } = integer(text, pos)?;
    // The candidate types, in C's order; the first that holds the value wins.
    use IntKind::*;
    let candidates: &[IntKind] = match (unsigned, long, decimal) {
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

/// What an integer constant says, before C gives it a type.
pub struct IntLit {
    /// The value, saturated at `u64::MAX`.
    pub value: u64,
    /// Written in decimal (not hex or octal).
    pub decimal: bool,
    /// The suffix has a `u`, an `l`.
    pub unsigned: bool,
    pub long: bool,
}

/// Reads an integer constant. The preprocessor's `#if` uses it too, where
/// values are 64 bits wide.
pub fn integer(text: &str, pos: Pos) -> Result<IntLit, Error> {
    let lower = text.to_ascii_lowercase();
    let hex = lower.starts_with("0x");
    if lower.contains('.') || (!hex && lower.contains('e')) || (hex && lower.contains('p')) {
        return Err(Error::new(pos, "floating-point literals are not supported"));
    }
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
    Ok(IntLit {
        value: u64::from_str_radix(digits, radix).unwrap_or(u64::MAX),
        decimal: radix == 10,
        unsigned,
        long,
    })
}

/// The bytes of a character constant or string literal as written (with
/// its quotes, closed: the lexer made sure), escapes decoded. Characters
/// outside ASCII come out as their UTF-8 bytes. `pos` is where the literal
/// starts; an error points at the escape sequence.
pub fn decode(text: &str, pos: Pos) -> Result<Vec<u8>, Error> {
    let chars: Vec<char> = text.chars().collect();
    let body = &chars[1..chars.len() - 1];
    // Where character `k` of the body is (the literal is on one line, unless
    // a line splice ran through it).
    let at = |k: usize| Pos::in_file(pos.file, pos.line, pos.col + 1 + k as u32);
    let mut out = Vec::new();
    let mut k = 0;
    while k < body.len() {
        let c = body[k];
        if c != '\\' {
            let mut buf = [0; 4];
            out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
            k += 1;
            continue;
        }
        let start = k;
        let e = body[k + 1];
        k += 2;
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
                    match body.get(k).and_then(|c| c.to_digit(8)) {
                        Some(d) => {
                            v = v * 8 + d;
                            k += 1;
                        }
                        None => break,
                    }
                }
                u8::try_from(v).map_err(|_| Error::new(at(start), "octal escape sequence out of range"))?
            }
            'x' => {
                // As many hex digits as follow, as in C.
                let digits_start = k;
                while body.get(k).is_some_and(|c| c.is_ascii_hexdigit()) {
                    k += 1;
                }
                if k == digits_start {
                    return Err(Error::new(at(start), "\\x used with no following hex digits"));
                }
                let hex: String = body[digits_start..k].iter().collect();
                let v = u64::from_str_radix(&hex, 16).unwrap_or(u64::MAX);
                u8::try_from(v).map_err(|_| Error::new(at(start), "hex escape sequence out of range"))?
            }
            _ => return Err(Error::new(at(start), format!("unknown escape sequence '\\{e}'"))),
        };
        out.push(byte);
    }
    Ok(out)
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
        let (got, _) = lex_pp(&src, 0).unwrap();
        let got: Vec<PpKind> = got.into_iter().map(|t| t.kind).collect();
        let want: Vec<PpKind> = PUNCTS.iter().map(|p| PpKind::Punct(p)).collect();
        assert_eq!(got, want);
        // The parser gets all but the preprocessor's own.
        let parser: Vec<&str> = PUNCTS.iter().copied().filter(|p| !p.starts_with('#')).collect();
        let got: Vec<Tok> = toks(&parser.join(" "))
            .into_iter()
            .filter(|t| *t != Tok::Eof)
            .collect();
        let want: Vec<Tok> = parser.iter().map(|p| Tok::Punct(p)).collect();
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
    fn punctuators_are_not_split() {
        assert_eq!(toks("f(int a, ...)")[5], Tok::Punct("..."));
        assert_eq!(err("x = 1.5;"), "1:5: floating-point literals are not supported");
        // `.5` and `1e5` are (floating) numbers, `..` two dots.
        assert_eq!(err("x = .5;"), "1:5: floating-point literals are not supported");
        assert_eq!(err("x = 1e5;"), "1:5: floating-point literals are not supported");
        assert_eq!(toks("a..b")[1..3], [Tok::Punct("."), Tok::Punct(".")]);
        // `->` is one token, `.` another.
        assert_eq!(
            toks("p->x.y"),
            vec![
                Tok::Ident("p".into()),
                Tok::Punct("->"),
                Tok::Ident("x".into()),
                Tok::Punct("."),
                Tok::Ident("y".into()),
                Tok::Eof
            ]
        );
        assert_eq!(
            toks("a-->b"),
            vec![
                Tok::Ident("a".into()),
                Tok::Punct("--"),
                Tok::Punct(">"),
                Tok::Ident("b".into()),
                Tok::Eof
            ]
        );
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
        // Without the preprocessor (crate::preprocess), `#` means nothing.
        assert_eq!(
            err("#include <x.h>"),
            "1:1: '#' is only allowed in preprocessor directives"
        );
        assert_eq!(
            err("a ## b"),
            "1:3: '##' is only allowed in preprocessor directives"
        );
        assert_eq!(err("int f() { float s; }"), "1:11: 'float' is not supported yet");
        assert_eq!(err("register int x;"), "1:1: 'register' is not supported yet");
    }

    #[test]
    fn trigraphs_are_errors() {
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
        // `??/` and a newline would splice a // comment onto the next line.
        assert_eq!(
            err("x; // note ??/  \r\ny = 2;"),
            format!("1:12: {}", trigraph("??/"))
        );
        // Elsewhere in comments, trigraphs mean nothing.
        assert_eq!(toks("// a \\ b ??= ??/ c\n/* ??/\n **/ x").len(), 2);
    }

    #[test]
    fn line_splices_join_lines() {
        // A backslash at the end of a line (blanks after it allowed) joins
        // the next line to it, anywhere: in literals, tokens, comments.
        assert_eq!(toks("char *s = \"a\\\nb\";")[4], Tok::Str(b"ab".to_vec()));
        assert_eq!(toks("char *s = \"a\\ \r\nb\";")[4], Tok::Str(b"ab".to_vec()));
        let t = lex("in\\\nt x;").unwrap();
        assert_eq!(t[0].tok, Tok::Kw(Kw::Int));
        assert_eq!(t[0].text, "int");
        // Positions are where the characters were written.
        assert_eq!(
            (t[0].pos, t[0].end, t[1].pos),
            (Pos::new(1, 1), Pos::new(2, 2), Pos::new(2, 3))
        );
        assert_eq!(toks("x; // note \\\ny = 2;").len(), 3, "the comment goes on");
        assert_eq!(
            toks("/* a *\\\n/ x").len(),
            2,
            "`*/` across a splice ends the comment"
        );
        // A backslash before anything else is just an invalid character.
        assert_eq!(err("a \\ b"), "1:3: unexpected character '\\'");
    }

    #[test]
    fn preprocessing_tokens() {
        let (t, end) = lex_pp("#define X 1\n  # x(y)z\n\"s\" 'c' 0x1e+1 1.2.3 .5e-3", 0).unwrap();
        let got: Vec<(&str, bool, bool)> = t.iter().map(|t| (t.text.as_str(), t.bol, t.space)).collect();
        assert_eq!(
            got,
            [
                ("#", true, false),
                ("define", false, false),
                ("X", false, true),
                ("1", false, true),
                ("#", true, true),
                ("x", false, true),
                ("(", false, false),
                ("y", false, false),
                (")", false, false),
                ("z", false, false),
                ("\"s\"", true, true),
                ("'c'", false, true),
                ("0x1e+1", false, true),
                ("1.2.3", false, true),
                (".5e-3", false, true),
            ]
        );
        assert_eq!(t[10].kind, PpKind::Str);
        assert_eq!(t[11].kind, PpKind::Char);
        assert!(t[12..].iter().all(|t| t.kind == PpKind::Number));
        assert_eq!(end, Pos::new(3, 27));
        // A comment is a space; it does not end the line.
        let (t, _) = lex_pp("a/* \n */b", 0).unwrap();
        assert_eq!((t[1].bol, t[1].space), (false, true));
        // What is no token only becomes an error in `finish`: a skipped
        // `#if 0` group may hold an apostrophe.
        let (t, _) = lex_pp("don't @", 0).unwrap();
        assert_eq!(
            t[1].kind,
            PpKind::Invalid("unterminated character literal".into())
        );
        assert_eq!(t[1].text, "'t @");
        assert_eq!(
            finish(t, end).unwrap_err().to_string(),
            "1:4: unterminated character literal"
        );
        // Files are told apart by number.
        let (t, _) = lex_pp("x", 3).unwrap();
        assert_eq!(t[0].pos, Pos::in_file(3, 1, 1));
    }

    #[test]
    fn crlf_line_endings() {
        let t = lex("int\r\nx").unwrap();
        assert_eq!(t[1].pos, Pos::new(2, 1));
    }
}
