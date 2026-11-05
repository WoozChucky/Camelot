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
//! Trigraphs (`??=` for `#`, `??/` for `\`...), which C99 replaces before
//! anything else (phase 1), are replaced as clang replaces them in its C99
//! mode, with its warning where they are no comment.
//!
//! Assembly preprocessed as C (`.S` files, [`lex_pp_asm`]) is split into the
//! same tokens, as clang's `-x assembler-with-cpp` splits it: what is no C
//! token (`$`, `@`, `\`, an apostrophe with no closing one on its line)
//! passes through as it is, and trigraphs are only characters (clang leaves
//! them alone in its default GNU mode, and only warns).
//!
//! The lexer's warnings (a trigraph, a line splice with blanks between the
//! backslash and the newline) go with the token after them
//! ([`PpToken::lexed`]): the preprocessor gives them as it reads it, which
//! is when clang's lexer gives them, and not in a group `#if 0` skips.
//!
//! Integer literals get their C type in [`finish`] (C99 6.4.4.1): the first
//! of the candidate types that holds the value. A decimal literal is `int`,
//! `long` or `long long`; a hex or octal one may also be the unsigned type
//! of each (`int`, `unsigned int`, `long`, `unsigned long`, `long long`,
//! `unsigned long long`); `u`, `l`, `ll` and their mixes (`ul`, `ull`...)
//! start the list further on. Which types hold a value depends on the
//! target: `long` is 32 bits on ILP32, so `2147483648` is a `long long`
//! there, and 64 bits on LP64, where it is a `long`. Character literals are `int`s
//! holding the byte (plain `char` is unsigned, so `'ÿ'` is 255). String
//! literals are bytes, with the escapes C has.

use crate::types::{IntKind, Xlen};
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
    Register,
    Auto,
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
    StaticAssert,
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
    (Kw::Register, "register"),
    (Kw::Auto, "auto"),
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
    // C11, which clang accepts in C99 mode too (the name is reserved in C99).
    (Kw::StaticAssert, "_Static_assert"),
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
                | Kw::Register
                | Kw::Auto
                | Kw::Inline
        )
    }
}

/// C keywords that merlin doesn't handle yet. Using one is a clear error rather
/// than a confusing parse error further on.
const UNSUPPORTED_KEYWORDS: &[&str] = &["double", "float", "_Complex", "_Imaginary"];

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
    /// Text that is no token (an unterminated literal, `''`, a stray `@`),
    /// with the warning clang gives for it, if any (`missing terminating '
    /// character`). It only means something if it reaches the parser,
    /// which expects it nowhere: a group that `#if 0` skips may hold
    /// anything, like an apostrophe in a sentence.
    Invalid(Option<String>),
    /// The preprocessor's stand-in for an empty macro argument next to `##`
    /// (C99 6.10.3.3); never in the lexer's output, never in the
    /// preprocessor's.
    Placemarker,
    /// A directive the preprocessor passes on, a whole line as `-E` writes
    /// it (`#pragma weird a`, `#ident "x"`, as clang does); never in the
    /// lexer's output, and nothing to the parser.
    Directive,
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
    /// The lexer's warnings of what comes before it, from the end of the
    /// token before (the module's notes).
    pub lexed: Vec<Error>,
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
    Num(i128, IntKind),
    /// A string literal's bytes, escapes decoded, without the terminating NUL.
    Str(Vec<u8>),
    Kw(Kw),
    Punct(&'static str),
    /// What is no token ([`PpKind::Invalid`]): the parser expects it nowhere.
    Unknown,
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
            Tok::Unknown => "an invalid token".to_string(),
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

/// The source after C99's translation phases 1 and 2: trigraphs replaced,
/// line splices (a backslash right before a newline) deleted, every
/// remaining character with its position in the original text, so that
/// tokens joined by a splice still say where they were written.
struct Spliced {
    chars: Vec<char>,
    at: Vec<Pos>,
    /// What the lexer may warn of, at the index of the character it comes
    /// before, in order.
    marks: Vec<(usize, Mark)>,
    /// Just past the last character.
    end: Pos,
}

/// Something phases 1 and 2 did that clang warns of, but in a comment.
#[derive(Clone, Copy)]
enum Mark {
    /// A trigraph at `pos` replaced by `to`.
    Trigraph { pos: Pos, to: char },
    /// A line splice with blanks between the backslash, at `backslash`,
    /// and the newline: clang warns at the backslash in a `//` comment
    /// (and not in a `/* */` one), elsewhere at the first blank.
    Blanks { backslash: Pos, first: Pos },
}

/// What C99 replaces the trigraph `??c` by.
fn trigraph(c: char) -> Option<char> {
    let to = match c {
        '=' => '#',
        '(' => '[',
        '/' => '\\',
        ')' => ']',
        '\'' => '^',
        '<' => '{',
        '!' => '|',
        '>' => '}',
        '-' => '~',
        _ => return None,
    };
    Some(to)
}

/// Replaces trigraphs (unless `asm`: clang's assembler-with-cpp has none)
/// and deletes line splices. Blanks between the backslash and the newline
/// are allowed, as clang allows them, with a warning ([`Mark`]); a `\r`
/// before the newline is a line's end, no blank.
fn splice(src: &str, file: u32, asm: bool) -> Spliced {
    let src: Vec<char> = src.chars().collect();
    let (mut chars, mut at) = (Vec::with_capacity(src.len()), Vec::with_capacity(src.len()));
    let mut marks = Vec::new();
    let (mut line, mut col) = (1, 1);
    let mut k = 0;
    while k < src.len() {
        // The character here after phase 1, and how many it was written as.
        let (c, n) = match (src[k], src.get(k + 1), src.get(k + 2).and_then(|&c| trigraph(c))) {
            ('?', Some('?'), Some(to)) if !asm => (to, 3),
            (c, ..) => (c, 1),
        };
        let pos = Pos::in_file(file, line, col);
        if c == '\\' {
            let mut j = k + n;
            while matches!(src.get(j), Some(' ' | '\t' | '\r')) {
                j += 1;
            }
            if src.get(j) == Some(&'\n') {
                if n == 3 {
                    marks.push((chars.len(), Mark::Trigraph { pos, to: c }));
                }
                if src[k + n..j].iter().any(|&b| b != '\r') {
                    let backslash = Pos::in_file(file, line, col + n as u32 - 1);
                    let first = Pos::in_file(file, line, col + n as u32);
                    marks.push((chars.len(), Mark::Blanks { backslash, first }));
                }
                line += 1;
                col = 1;
                k = j + 1;
                continue;
            }
        }
        if n == 3 {
            marks.push((chars.len(), Mark::Trigraph { pos, to: c }));
        }
        chars.push(c);
        at.push(pos);
        if c == '\n' {
            line += 1;
            col = 1;
        } else {
            col += n as u32;
        }
        k += n;
    }
    Spliced {
        chars,
        at,
        marks,
        end: Pos::in_file(file, line, col),
    }
}

/// Where the lexer is, for the warnings of [`Mark`].
#[derive(Clone, Copy, PartialEq, Eq)]
enum Context {
    Code,
    LineComment,
    BlockComment,
}

struct Lexer {
    src: Spliced,
    i: usize,
    /// A newline was skipped since the last token.
    newline: bool,
    /// Whitespace or a comment was skipped since the last token.
    space: bool,
    /// The next of [`Spliced::marks`] to look at, and where the lexer is.
    mark: usize,
    context: Context,
    /// The warnings for the next token.
    lexed: Vec<Error>,
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
        self.marks_before(self.i + 1);
        self.i += 1;
        Some(c)
    }

    /// clang's warnings of the [`Mark`]s before index `end`, as where the
    /// lexer is has them: none for a trigraph in a comment, none for a
    /// splice's blanks in a `/* */` one.
    fn marks_before(&mut self, end: usize) {
        while let Some(&(at, mark)) = self.src.marks.get(self.mark) {
            if at >= end {
                return;
            }
            self.mark += 1;
            let warning = match (mark, self.context) {
                (Mark::Trigraph { pos, to }, Context::Code) => Error::warning(
                    pos,
                    format!("trigraph converted to '{to}' character"),
                    "trigraphs",
                ),
                (Mark::Blanks { backslash, .. }, Context::LineComment) => {
                    Error::warning(backslash, BLANKS, "backslash-newline-escape")
                }
                (Mark::Blanks { first, .. }, Context::Code) => {
                    Error::warning(first, BLANKS, "backslash-newline-escape")
                }
                _ => continue,
            };
            self.lexed.push(warning);
        }
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
                    self.context = Context::LineComment;
                    while self.peek(0).is_some_and(|c| c != '\n') {
                        self.bump();
                    }
                    self.context = Context::Code;
                }
                (Some('/'), Some('*')) => {
                    self.space = true;
                    let start = self.pos();
                    self.context = Context::BlockComment;
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
                    self.context = Context::Code;
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
        // `''`: clang has no empty character constant.
        if quote == '\'' && self.peek(0) == Some('\'') {
            self.bump();
            return Ok(PpKind::Invalid(Some("empty character constant".into())));
        }
        loop {
            match self.peek(0) {
                None | Some('\n') => {
                    let quote = if quote == '"' { "'\"'" } else { "'" };
                    return Ok(PpKind::Invalid(Some(format!(
                        "missing terminating {quote} character"
                    ))));
                }
                Some(c) => {
                    self.bump();
                    if c == quote {
                        return Ok(if quote == '"' { PpKind::Str } else { PpKind::Char });
                    }
                    if c == '\\' && self.peek(0).is_some_and(|c| c != '\n') {
                        self.bump();
                    }
                }
            }
        }
    }
}

/// clang's warning of a line splice with blanks before its newline.
const BLANKS: &str = "backslash and newline separated by space";

/// Splits one file (`file` is its number, see [`Pos`]) into preprocessing
/// tokens, and gives the position of its end. Only an unterminated `/*`
/// comment is an error here; anything else that is no token becomes a
/// [`PpKind::Invalid`] one.
pub fn lex_pp(src: &str, file: u32) -> Result<(Vec<PpToken>, Pos), Error> {
    lex_pp_in(src, file, false)
}

/// [`lex_pp`] for assembly preprocessed as C (`clang -x assembler-with-cpp`):
/// the same tokens, but trigraphs are only characters. An unterminated `/*`
/// comment is an error, as in clang.
pub fn lex_pp_asm(src: &str, file: u32) -> Result<(Vec<PpToken>, Pos), Error> {
    lex_pp_in(src, file, true)
}

/// [`lex_pp`] (`asm` false) or [`lex_pp_asm`].
pub fn lex_pp_in(src: &str, file: u32, asm: bool) -> Result<(Vec<PpToken>, Pos), Error> {
    let mut lx = Lexer {
        src: splice(src, file, asm),
        i: 0,
        newline: true,
        space: false,
        mark: 0,
        context: Context::Code,
        lexed: Vec::new(),
    };
    let mut out: Vec<PpToken> = Vec::new();
    loop {
        lx.skip_trivia()?;
        let pos = lx.pos();
        let Some(c) = lx.peek(0) else {
            // What comes after the last token goes with it.
            lx.marks_before(usize::MAX);
            if let Some(last) = out.last_mut() {
                last.lexed.append(&mut lx.lexed);
            }
            return Ok((out, lx.src.end));
        };
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
            PpKind::Invalid(None)
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
            lexed: std::mem::take(&mut lx.lexed),
        });
    }
}

/// Lexes a whole translation unit without preprocessing it: [`lex_pp`]
/// then [`finish`]. Any error is one: the first.
pub fn lex(src: &str, x: Xlen) -> Result<Vec<Token>, Error> {
    let (toks, end) = lex_pp(src, 0)?;
    let (toks, diagnostics) = finish(toks, end, x).map_err(|mut e| e.pop().expect("the error"))?;
    match diagnostics.into_iter().find(|d| !d.is_warning()) {
        Some(e) => Err(e),
        None => Ok(toks),
    }
}

/// Turns preprocessing tokens into the parser's tokens (C99 translation
/// phase 7) and ends them with [`Tok::Eof`] at `end`: identifiers that are
/// keywords become keywords, numbers integer constants with their C type,
/// character constants and string literals their bytes.
///
/// It goes on after an error as clang does, with the diagnostics it gives
/// (clang's, in its words): a literal in error stands as `0` (or the bytes
/// it has), for the parser to go on with; what is no token (an
/// unterminated literal, a stray `@` or `\`, `''`) is a [`Tok::Unknown`]
/// the parser expects nowhere, and the unterminated literal and `''` a
/// warning too. `#` and `##` are punctuators the parser has no use for.
/// Only a keyword merlin has not (`float`) stops it: `Err`, with the
/// diagnostics before it.
pub fn finish(toks: Vec<PpToken>, end: Pos, x: Xlen) -> Result<(Vec<Token>, Vec<Error>), Vec<Error>> {
    let mut out = Vec::with_capacity(toks.len() + 1);
    let mut diags = Vec::new();
    for t in toks {
        let tok = match &t.kind {
            PpKind::Ident => {
                if let Some(k) = Kw::from_str(&t.text) {
                    Tok::Kw(k)
                } else if UNSUPPORTED_KEYWORDS.contains(&t.text.as_str()) {
                    diags.push(Error::new(t.pos, format!("'{}' is not supported yet", t.text)));
                    return Err(diags);
                } else {
                    Tok::Ident(t.text.clone())
                }
            }
            PpKind::Number => number(&t.text, t.pos, x, &mut diags).unwrap_or_else(|e| {
                diags.push(e);
                Tok::Num(0, IntKind::Int)
            }),
            PpKind::Char => char_constant(&t.text, t.pos, &mut diags).unwrap_or_else(|e| {
                diags.push(e);
                Tok::Num(0, IntKind::Int)
            }),
            PpKind::Str => match decode(&t.text, t.pos, &mut diags) {
                Ok(bytes) => Tok::Str(bytes),
                Err(e) => {
                    diags.push(e);
                    Tok::Str(Vec::new())
                }
            },
            PpKind::Punct(p) => Tok::Punct(p),
            PpKind::Invalid(msg) => {
                if let Some(msg) = msg {
                    diags.push(Error::warning(t.pos, msg.clone(), "invalid-pp-token"));
                }
                Tok::Unknown
            }
            PpKind::Placemarker | PpKind::Directive => continue,
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
    Ok((out, diags))
}

/// A character constant's value, an `int` (C99 6.4.4.4): its byte, or for
/// several (`'ab'`) the bytes as the digits of a number in base 256, as
/// clang computes it, keeping the low 32 bits, with clang's warning for
/// each (and one more past four bytes, which do not all fit). `''` is no
/// token at all to clang ([`lex_pp`] leaves it to the parser).
pub fn char_constant(text: &str, pos: Pos, diags: &mut Vec<Error>) -> Result<Tok, Error> {
    let bytes = decode_in(text, pos, diags, true)?;
    // Four characters are clang's `-Wfour-char-constants` (`'abcd'`, as
    // Mac OS's type codes were written), which is off by default.
    if bytes.len() > 1 && bytes.len() != 4 {
        diags.push(Error::warning(
            pos,
            "multi-character character constant",
            "multichar",
        ));
    }
    if bytes.len() > 4 {
        diags.push(Error::warning_unflagged(
            pos,
            "character constant too long for its type",
        ));
    }
    let value = match bytes[..] {
        [b] => b.into(),
        _ => i128::from(bytes.iter().fold(0u32, |v, &b| (v << 8) | u32::from(b)) as i32),
    };
    Ok(Tok::Num(value, IntKind::Int))
}

/// An integer constant and its C type on the target, from a preprocessing
/// number. A decimal one too large for every signed type is an
/// `unsigned long long`, with clang's warning (C99 gives it no type).
pub fn number(text: &str, pos: Pos, x: Xlen, diags: &mut Vec<Error>) -> Result<Tok, Error> {
    let IntLit {
        value,
        low,
        decimal,
        unsigned,
        long,
    } = integer(text, pos)?;
    // The candidate types, in C's order (C99 6.4.4.1p5); the first that
    // holds the value wins.
    use IntKind::*;
    let all: &[IntKind] = match (unsigned, decimal) {
        (false, true) => &[Int, Long, LongLong],
        (false, false) => &[Int, UInt, Long, ULong, LongLong, ULongLong],
        (true, _) => &[UInt, ULong, ULongLong],
    };
    // `l` starts at `long`, `ll` at `long long`.
    let candidates = all.iter().filter(|k| match long {
        0 => true,
        1 => !matches!(k, Int | UInt),
        _ => matches!(k, LongLong | ULongLong),
    });
    // Too large for any type: clang's error, and it goes on with the low
    // 64 bits, an `unsigned long long`.
    let Some(value) = value else {
        diags.push(Error::new(
            pos,
            "integer literal is too large to be represented in any integer type",
        ));
        return Ok(Tok::Num(low.into(), ULongLong));
    };
    match candidates.into_iter().find(|k| i128::from(value) <= k.range(x).1) {
        Some(&k) => Ok(Tok::Num(value.into(), k)),
        None => {
            diags.push(Error::warning(
                pos,
                "integer literal is too large to be represented in a signed integer type, \
                 interpreting as unsigned",
                "implicitly-unsigned-literal",
            ));
            Ok(Tok::Num(value.into(), ULongLong))
        }
    }
}

/// What an integer constant says, before C gives it a type.
pub struct IntLit {
    /// The value; `None` when it does not fit in 64 bits (in C, too large
    /// for any integer type).
    pub value: Option<u64>,
    /// Its low 64 bits, which clang goes on with when it does not fit.
    pub low: u64,
    /// Written in decimal (not hex or octal).
    pub decimal: bool,
    /// The suffix has a `u`.
    pub unsigned: bool,
    /// The suffix has an `l` (1) or an `ll` (2).
    pub long: u8,
}

/// Reads an integer constant, as clang's `NumericLiteralParser` reads one:
/// the digits of its radix (`0x` hex, a leading `0` octal), then a suffix
/// of `u` and `l` (`ll` in one case). A digit the radix has not right after
/// the digits is `invalid digit '9' in octal constant` (`a` to `f` in a
/// decimal one too), at the digit; anything else after them is `invalid
/// suffix 'abc' on integer constant`, at the suffix. The preprocessor's
/// `#if` uses it too, where values are 64 bits wide.
pub fn integer(text: &str, pos: Pos) -> Result<IntLit, Error> {
    let lower = text.to_ascii_lowercase();
    let hex = lower.starts_with("0x");
    if lower.contains('.') || (!hex && lower.contains('e')) || (hex && lower.contains('p')) {
        return Err(Error::new(pos, "floating-point literals are not supported"));
    }
    let chars: Vec<char> = text.chars().collect();
    let at = |k: usize| Pos::in_file(pos.file, pos.line, pos.col + k as u32);
    // The leading `0` of an octal constant is one of its digits (`0` and
    // `0u` are octal).
    let (radix, start) = if hex {
        (16, 2)
    } else if chars.len() > 1 && chars[0] == '0' {
        (8, 0)
    } else {
        (10, 0)
    };
    let mut k = start;
    while k < chars.len() && chars[k].is_digit(radix) {
        k += 1;
    }
    // An octal constant with an 8 or a 9, or a decimal or octal one with a
    // hex digit right after its digits (not `e`, which would make it
    // floating), has an invalid digit.
    if radix != 16 && k < chars.len() {
        let c = chars[k];
        if c.is_ascii_digit() || (c.is_ascii_hexdigit() && !"eE".contains(c)) {
            let which = if radix == 8 { "octal" } else { "decimal" };
            return Err(Error::new(
                at(k),
                format!("invalid digit '{c}' in {which} constant"),
            ));
        }
    }
    let digits: String = chars[start..k].iter().collect();
    // `0x` with no digits is the octal `0` with the suffix `x...`.
    if hex && k == start {
        k = 1;
    }
    let suffix: String = chars[k..].iter().collect();
    let (unsigned, long) = match suffix.to_ascii_lowercase().as_str() {
        _ if hex && digits.is_empty() => (false, 3),
        "" => (false, 0),
        "u" => (true, 0),
        "l" => (false, 1),
        "ul" | "lu" => (true, 1),
        // `ll` is `ll` or `LL`, never `lL` (C99 6.4.4.1).
        "ll" | "ull" | "llu" if suffix.contains("ll") || suffix.contains("LL") => {
            (suffix.contains(['u', 'U']), 2)
        }
        _ => (false, 3),
    };
    if long == 3 {
        return Err(Error::new(
            at(k),
            format!("invalid suffix '{suffix}' on integer constant"),
        ));
    }
    // The digits are valid, so the only way to fail is to overflow.
    let low = digits.chars().fold(0u64, |v, c| {
        v.wrapping_mul(u64::from(radix))
            .wrapping_add(u64::from(c.to_digit(radix).expect("a digit")))
    });
    Ok(IntLit {
        value: u64::from_str_radix(&digits, radix).ok(),
        low,
        decimal: radix == 10,
        unsigned,
        long,
    })
}

/// The bytes of a character constant or string literal as written (with
/// its quotes, closed: the lexer made sure), escapes decoded, with clang's
/// warning for an escape C has not (`\q`, the `q`). Characters outside
/// ASCII come out as their UTF-8 bytes. `pos` is where the literal starts;
/// an error or a warning points at the escape sequence.
pub fn decode(text: &str, pos: Pos, diags: &mut Vec<Error>) -> Result<Vec<u8>, Error> {
    decode_in(text, pos, diags, false)
}

/// [`decode`] of a character constant (`char_const`) or a string literal,
/// which differ in what a universal character name may be: a constant
/// holds one byte.
fn decode_in(text: &str, pos: Pos, diags: &mut Vec<Error>, char_const: bool) -> Result<Vec<u8>, Error> {
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
        // The lexer never ends a literal with a lone `\` (it would escape the
        // quote), but a literal made some other way might.
        let Some(&e) = body.get(k + 1) else {
            return Err(Error::new(
                at(start),
                "'\\' at the end of a literal escapes nothing",
            ));
        };
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
            // GNU's escape for ESC, which clang takes without a word.
            'e' | 'E' => 27,
            // clang's other escapes (its `ProcessCharEscape`): the character,
            // which its pedantic warning only says C has not.
            '(' | '{' | '[' | '%' => e as u8,
            // A universal character name (C99 6.4.3): `\u` and four hex
            // digits, `\U` and eight, the character in UTF-8.
            'u' | 'U' => {
                let n = if e == 'u' { 4 } else { 8 };
                let digits: String = body[k..]
                    .iter()
                    .take(n)
                    .take_while(|c| c.is_ascii_hexdigit())
                    .collect();
                k += digits.len();
                if digits.len() < n {
                    diags.push(Error::new(at(start), "incomplete universal character name"));
                    continue;
                }
                let v = u32::from_str_radix(&digits, 16).expect("hex digits");
                // What C99 lets no universal character name be: a control
                // character, the basic character set but `$`, `@` and `` ` ``,
                // a surrogate, or what is past Unicode.
                let problem = match char::from_u32(v) {
                    None => Some("invalid universal character".to_string()),
                    Some(c) if c.is_control() => {
                        Some("universal character name refers to a control character".to_string())
                    }
                    Some(c) if v < 0xa0 && !"$@`".contains(c) => Some(format!(
                        "character '{c}' cannot be specified by a universal character name"
                    )),
                    Some(_) if char_const && v > 0x7f => {
                        // clang's error is the constant's.
                        diags.push(Error::new(
                            pos,
                            "character too large for enclosing character literal type",
                        ));
                        continue;
                    }
                    Some(_) => None,
                };
                match (problem, char::from_u32(v)) {
                    (Some(msg), _) => diags.push(Error::new(at(start), msg)),
                    (None, Some(c)) => {
                        let mut buf = [0; 4];
                        out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
                    }
                    (None, None) => unreachable!("an invalid one has a problem"),
                }
                continue;
            }
            // clang: the character itself, and a warning.
            _ => {
                diags.push(Error::warning(
                    at(start),
                    format!("unknown escape sequence '\\{e}'"),
                    "unknown-escape-sequence",
                ));
                let mut buf = [0; 4];
                out.extend_from_slice(e.encode_utf8(&mut buf).as_bytes());
                continue;
            }
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
        lex(s, Xlen::Rv32).unwrap().into_iter().map(|t| t.tok).collect()
    }

    fn num(s: &str) -> (i128, IntKind) {
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
        // Past (unsigned) long on ILP32: long long.
        assert_eq!(num("2147483648"), (0x8000_0000, LongLong));
        assert_eq!(num("2147483648l"), (0x8000_0000, LongLong));
        assert_eq!(num("4294967296u"), (1 << 32, ULongLong));
        assert_eq!(num("0x100000000"), (1 << 32, LongLong));
        assert_eq!(num("0xffffffffffffffff"), (u64::MAX.into(), ULongLong));
        assert_eq!(num("10ll"), (10, LongLong));
        assert_eq!(num("10LL"), (10, LongLong));
        assert_eq!(num("10ull"), (10, ULongLong));
        assert_eq!(num("10LLU"), (10, ULongLong));
        assert_eq!(num("0x8000000000000000ll"), (1 << 63, ULongLong));
        // A decimal literal past LLONG_MAX is unsigned, as clang takes it.
        assert_eq!(num("9223372036854775808"), (1 << 63, ULongLong));
        // (The errors of literals are in tests/diagnostics.)
    }

    #[test]
    fn integer_literal_types_on_lp64() {
        let num = |s: &str| match lex(s, Xlen::Rv64).unwrap()[..] {
            [Token {
                tok: Tok::Num(v, k), ..
            }, _] => (v, k),
            ref t => panic!("{s}: {t:?}"),
        };
        // `long` is 64 bits: what does not fit `int` (or `unsigned int`) is
        // `long` (or `unsigned long`).
        assert_eq!(num("2147483647"), (0x7fff_ffff, Int));
        assert_eq!(num("2147483648"), (0x8000_0000, Long));
        assert_eq!(num("0x80000000"), (0x8000_0000, UInt));
        assert_eq!(num("4294967296u"), (1 << 32, ULong));
        assert_eq!(num("0x100000000"), (1 << 32, Long));
        assert_eq!(num("9223372036854775807"), (i64::MAX.into(), Long));
        assert_eq!(num("0x8000000000000000"), (1 << 63, ULong));
        assert_eq!(num("18446744073709551615u"), (u64::MAX.into(), ULong));
        assert_eq!(num("0xffffffffL"), (0xffff_ffff, Long));
        // A decimal literal past LONG_MAX would be long long in C, and is
        // unsigned long long to clang.
        assert_eq!(num("9223372036854775808"), (1 << 63, ULongLong));
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
        // Several characters are the digits of a number in base 256, as
        // clang reads them (with a warning), the low 32 bits of it; an
        // escape C has not is the character (with a warning too).
        assert_eq!(num("'ab'"), (0x6162, Int));
        assert_eq!(num("'\\xff\\xff\\xff\\xff'"), (-1, Int));
        assert_eq!(num("'abcde'"), (0x6263_6465, Int));
        assert_eq!(num("'\\q'"), (113, Int));
        // The lexer cannot make a literal that ends in a lone backslash, but
        // decode must not panic on one (the preprocessor's `#` once made it).
        let e = decode(r#""a\""#, Pos::in_file(0, 1, 1), &mut Vec::new()).unwrap_err();
        assert_eq!(e.to_string(), "1:3: '\\' at the end of a literal escapes nothing");
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
        let t = lex(r#"x = "a\tb";"#, Xlen::Rv32).unwrap();
        assert_eq!(t[2].text, r#""a\tb""#);
        assert_eq!(t[2].describe(), "string literal");
    }

    #[test]
    fn punctuators_are_not_split() {
        assert_eq!(toks("f(int a, ...)")[5], Tok::Punct("..."));
        // `.5` and `1e5` are (floating) numbers, `..` two dots.
        assert_eq!(lex_pp(".5 1e5", 0).unwrap().0[0].kind, PpKind::Number);
        assert_eq!(lex_pp(".5 1e5", 0).unwrap().0[1].kind, PpKind::Number);
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
        let t = lex("0x7f 017 x1 <= 'a' 10u", Xlen::Rv32).unwrap();
        let text: Vec<&str> = t.iter().map(|t| t.text.as_str()).collect();
        assert_eq!(text, ["0x7f", "017", "x1", "<=", "'a'", "10u", ""]);
        assert_eq!(t[0].describe(), "'0x7f'");
        assert_eq!(t[2].describe(), "identifier 'x1'");
        assert_eq!(t[4].describe(), "''a''");
    }

    #[test]
    fn comments_and_positions() {
        let src = "// line comment\n  /* block\n comment */ x /**/ +\n\t1";
        let t = lex(src, Xlen::Rv32).unwrap();
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

    /// What is no token is one the parser expects nowhere; `#` and `##`
    /// are punctuators, which mean nothing to it.
    #[test]
    fn what_is_no_token() {
        assert_eq!(
            toks("a @ b `")[1..],
            [Tok::Unknown, Tok::Ident("b".into()), Tok::Unknown, Tok::Eof]
        );
        assert_eq!(toks("a \\ b")[1], Tok::Unknown);
        assert_eq!(
            toks("# a ## b")[..4],
            [
                Tok::Punct("#"),
                Tok::Ident("a".into()),
                Tok::Punct("##"),
                Tok::Ident("b".into())
            ]
        );
        assert_eq!(toks("x = '';")[2], Tok::Unknown);
        assert_eq!(toks("x = 'a;")[2], Tok::Unknown);
    }

    /// Trigraphs are replaced before anything else, with clang's warnings
    /// (tests/diagnostics has them) going with the token after them.
    #[test]
    fn trigraphs_are_replaced() {
        // A '?' that doesn't start a trigraph is fine, and so is '\?'.
        assert_eq!(
            toks(r#""???" "?\?=" "a??=""#)[..3],
            [
                Tok::Str(b"???".to_vec()),
                Tok::Str(b"??=".to_vec()),
                Tok::Str(b"a#".to_vec())
            ]
        );
        // `??/` and a newline are a line splice, in a comment too.
        assert_eq!(toks("x ??/\n= 1 // c ??/\ny = 2;").len(), 4);
        // Elsewhere in comments, trigraphs mean nothing.
        assert_eq!(toks("// a \\ b ??= ??/ c\n/* ??/\n **/ x").len(), 2);
        let (t, _) = lex_pp("// ??=\nx ??( 1", 0).unwrap();
        assert!(t[0].lexed.is_empty());
        assert_eq!(
            t[1].lexed[0].to_string(),
            "2:3: trigraph converted to '[' character"
        );
    }

    #[test]
    fn line_splices_join_lines() {
        // A backslash at the end of a line (blanks after it allowed) joins
        // the next line to it, anywhere: in literals, tokens, comments.
        assert_eq!(toks("char *s = \"a\\\nb\";")[4], Tok::Str(b"ab".to_vec()));
        assert_eq!(toks("char *s = \"a\\ \r\nb\";")[4], Tok::Str(b"ab".to_vec()));
        let t = lex("in\\\nt x;", Xlen::Rv32).unwrap();
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
            PpKind::Invalid(Some("missing terminating ' character".into()))
        );
        assert_eq!(t[1].text, "'t @");
        let (toks, diags) = finish(t, end, Xlen::Rv32).unwrap();
        assert_eq!(toks[1].tok, Tok::Unknown);
        assert_eq!(diags[0].to_string(), "1:4: missing terminating ' character");
        // Files are told apart by number.
        let (t, _) = lex_pp("x", 3).unwrap();
        assert_eq!(t[0].pos, Pos::in_file(3, 1, 1));
    }

    /// Assembly is split as C is, but a trigraph is three characters.
    #[test]
    fn assembly_tokens() {
        let (t, _) = lex_pp_asm("  la a0, $x@ \\rs1 ??= 'c' # it's\n", 0).unwrap();
        let got: Vec<&str> = t.iter().map(|t| t.text.as_str()).collect();
        assert_eq!(
            got,
            ["la", "a0", ",", "$", "x", "@", "\\", "rs1", "?", "?", "=", "'c'", "#", "it", "'s"]
        );
        assert!(matches!(t[3].kind, PpKind::Invalid(_)));
        assert_eq!(lex_pp("??=", 0).unwrap().0[0].text, "#");
        assert!(lex_pp_asm("/* x", 0).is_err());
    }

    #[test]
    fn crlf_line_endings() {
        let t = lex("int\r\nx", Xlen::Rv32).unwrap();
        assert_eq!(t[1].pos, Pos::new(2, 1));
    }
}
