//! JSON (RFC 8259) for Pendragon's API: a value type, a strict parser and an encoder.
//!
//! The parser takes text from any web page's request, so it accepts exactly the
//! grammar and nothing around it: no trailing commas, comments, single quotes, leading
//! zeros or `NaN`; no unescaped control characters or lone surrogates in strings; no
//! duplicate keys in an object (two parsers could disagree on which one wins); and no
//! nesting deeper than [`MAX_DEPTH`], so a request of a million `[` cannot overflow the
//! stack. Its input is already bounded by the HTTP layer's body limit.
//!
//! Numbers keep their text ([`Json::Number`]): an id or a byte count comes back out
//! exactly, with no trip through `f64`. The encoder escapes `<`, `>`, `&`, U+2028 and
//! U+2029 as well as what JSON requires, so its output is inert even inside HTML.

use std::collections::HashSet;
use std::fmt::{self, Write as _};

/// How deeply arrays and objects may nest.
pub const MAX_DEPTH: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Json {
    Null,
    Bool(bool),
    /// A number, as its JSON text (always valid JSON number syntax).
    Number(String),
    String(String),
    Array(Vec<Json>),
    /// Members in order; keys are unique.
    Object(Vec<(String, Json)>),
}

impl Json {
    /// An object from `(key, value)` pairs. Keys must be unique (the encoder does not
    /// check).
    pub fn object<'a>(members: impl IntoIterator<Item = (&'a str, Json)>) -> Json {
        Json::Object(members.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
    }

    /// Parses a whole JSON text.
    pub fn parse(text: &str) -> Result<Json, String> {
        let mut p = Parser {
            s: text.as_bytes(),
            i: 0,
        };
        p.ws();
        let value = p.value(0)?;
        p.ws();
        if p.i != p.s.len() {
            return Err(p.err("unexpected text after the value"));
        }
        Ok(value)
    }

    /// The member `key` of an object.
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Object(members) => members.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Json::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// A number that is a whole number from 0 to `u64::MAX`, written without a
    /// fraction or exponent.
    pub fn as_u64(&self) -> Option<u64> {
        match self {
            Json::Number(text) if text.bytes().all(|b| b.is_ascii_digit()) => text.parse().ok(),
            _ => None,
        }
    }

    /// A number that is a whole number from `i64::MIN` to `i64::MAX`, written without
    /// a fraction or exponent.
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Json::Number(text) if !text.contains(['.', 'e', 'E']) => text.parse().ok(),
            _ => None,
        }
    }

    /// Any number, as the nearest `f64` (a browser's numbers are all `f64`). A number
    /// too large for one is `None`, not infinity.
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Json::Number(text) => text.parse::<f64>().ok().filter(|n| n.is_finite()),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Json]> {
        match self {
            Json::Array(items) => Some(items),
            _ => None,
        }
    }

    /// The JSON text.
    pub fn encode(&self) -> String {
        self.to_string()
    }
}

impl fmt::Display for Json {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Json::Null => f.write_str("null"),
            Json::Bool(b) => write!(f, "{b}"),
            Json::Number(n) => f.write_str(n),
            Json::String(s) => write_string(f, s),
            Json::Array(items) => {
                f.write_char('[')?;
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        f.write_char(',')?;
                    }
                    write!(f, "{item}")?;
                }
                f.write_char(']')
            }
            Json::Object(members) => {
                f.write_char('{')?;
                for (i, (key, value)) in members.iter().enumerate() {
                    if i > 0 {
                        f.write_char(',')?;
                    }
                    write_string(f, key)?;
                    write!(f, ":{value}")?;
                }
                f.write_char('}')
            }
        }
    }
}

fn write_string(f: &mut fmt::Formatter, s: &str) -> fmt::Result {
    f.write_char('"')?;
    for c in s.chars() {
        match c {
            '"' => f.write_str("\\\"")?,
            '\\' => f.write_str("\\\\")?,
            '\n' => f.write_str("\\n")?,
            '\r' => f.write_str("\\r")?,
            '\t' => f.write_str("\\t")?,
            // Control characters as JSON requires; the rest so that the text can never
            // close a <script> or start markup, wherever it ends up.
            '\0'..='\x1f' | '<' | '>' | '&' | '\u{2028}' | '\u{2029}' | '\x7f' => {
                write!(f, "\\u{:04x}", c as u32)?
            }
            c => f.write_char(c)?,
        }
    }
    f.write_char('"')
}

impl From<&str> for Json {
    fn from(s: &str) -> Json {
        Json::String(s.to_string())
    }
}

impl From<String> for Json {
    fn from(s: String) -> Json {
        Json::String(s)
    }
}

impl From<bool> for Json {
    fn from(b: bool) -> Json {
        Json::Bool(b)
    }
}

macro_rules! from_int {
    ($($t:ty),*) => {$(
        impl From<$t> for Json {
            fn from(n: $t) -> Json {
                Json::Number(n.to_string())
            }
        }
    )*};
}

from_int!(u8, u16, u32, u64, usize, i64);

impl<T: Into<Json>> From<Option<T>> for Json {
    fn from(v: Option<T>) -> Json {
        v.map_or(Json::Null, Into::into)
    }
}

impl<T: Into<Json>> From<Vec<T>> for Json {
    fn from(items: Vec<T>) -> Json {
        Json::Array(items.into_iter().map(Into::into).collect())
    }
}

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
}

impl Parser<'_> {
    fn err(&self, why: &str) -> String {
        format!("bad JSON at byte {}: {why}", self.i)
    }

    fn ws(&mut self) {
        while let Some(b' ' | b'\t' | b'\n' | b'\r') = self.s.get(self.i) {
            self.i += 1;
        }
    }

    fn eat(&mut self, b: u8) -> bool {
        if self.s.get(self.i) == Some(&b) {
            self.i += 1;
            true
        } else {
            false
        }
    }

    fn literal(&mut self, word: &str, value: Json) -> Result<Json, String> {
        if self.s[self.i..].starts_with(word.as_bytes()) {
            self.i += word.len();
            Ok(value)
        } else {
            Err(self.err("unknown literal"))
        }
    }

    fn value(&mut self, depth: usize) -> Result<Json, String> {
        match self.s.get(self.i) {
            None => Err(self.err("the text ends where a value should be")),
            Some(b'n') => self.literal("null", Json::Null),
            Some(b't') => self.literal("true", Json::Bool(true)),
            Some(b'f') => self.literal("false", Json::Bool(false)),
            Some(b'"') => self.string().map(Json::String),
            Some(b'-' | b'0'..=b'9') => self.number(),
            Some(b'[') => {
                if depth == MAX_DEPTH {
                    return Err(self.err("nested too deeply"));
                }
                self.i += 1;
                let mut items = Vec::new();
                self.ws();
                if self.eat(b']') {
                    return Ok(Json::Array(items));
                }
                loop {
                    self.ws();
                    items.push(self.value(depth + 1)?);
                    self.ws();
                    if self.eat(b']') {
                        return Ok(Json::Array(items));
                    }
                    if !self.eat(b',') {
                        return Err(self.err("expected ',' or ']'"));
                    }
                }
            }
            Some(b'{') => {
                if depth == MAX_DEPTH {
                    return Err(self.err("nested too deeply"));
                }
                self.i += 1;
                let mut members: Vec<(String, Json)> = Vec::new();
                // The keys so far, to find a duplicate without going through every
                // member each time (a large object would take quadratic time).
                let mut keys = HashSet::new();
                self.ws();
                if self.eat(b'}') {
                    return Ok(Json::Object(members));
                }
                loop {
                    self.ws();
                    if self.s.get(self.i) != Some(&b'"') {
                        return Err(self.err("expected a string key"));
                    }
                    let key = self.string()?;
                    if !keys.insert(key.clone()) {
                        return Err(self.err(&format!("duplicate key \"{key}\"")));
                    }
                    self.ws();
                    if !self.eat(b':') {
                        return Err(self.err("expected ':'"));
                    }
                    self.ws();
                    let value = self.value(depth + 1)?;
                    members.push((key, value));
                    self.ws();
                    if self.eat(b'}') {
                        return Ok(Json::Object(members));
                    }
                    if !self.eat(b',') {
                        return Err(self.err("expected ',' or '}'"));
                    }
                }
            }
            Some(_) => Err(self.err("unexpected character")),
        }
    }

    fn digits(&mut self) -> usize {
        let start = self.i;
        while self.s.get(self.i).is_some_and(u8::is_ascii_digit) {
            self.i += 1;
        }
        self.i - start
    }

    fn number(&mut self) -> Result<Json, String> {
        let start = self.i;
        self.eat(b'-');
        match self.s.get(self.i) {
            Some(b'0') => {
                self.i += 1;
                if self.s.get(self.i).is_some_and(u8::is_ascii_digit) {
                    return Err(self.err("a number with a leading zero"));
                }
            }
            Some(b'1'..=b'9') => {
                self.digits();
            }
            _ => return Err(self.err("a '-' without digits")),
        }
        if self.eat(b'.') && self.digits() == 0 {
            return Err(self.err("a '.' without digits after it"));
        }
        if self.eat(b'e') || self.eat(b'E') {
            if !self.eat(b'+') {
                self.eat(b'-');
            }
            if self.digits() == 0 {
                return Err(self.err("an exponent without digits"));
            }
        }
        let text = std::str::from_utf8(&self.s[start..self.i]).expect("ASCII");
        Ok(Json::Number(text.to_string()))
    }

    fn hex4(&mut self) -> Result<u32, String> {
        let hex = self
            .s
            .get(self.i..self.i + 4)
            .filter(|h| h.iter().all(u8::is_ascii_hexdigit))
            .ok_or_else(|| self.err("\\u without four hex digits"))?;
        self.i += 4;
        Ok(u32::from_str_radix(std::str::from_utf8(hex).expect("ASCII"), 16).expect("hex"))
    }

    fn string(&mut self) -> Result<String, String> {
        self.i += 1; // the opening quote
        let mut out = String::new();
        loop {
            // Copy the run of plain characters at once. The input is a &str, so any
            // run that stops at an ASCII byte is whole UTF-8.
            let start = self.i;
            while let Some(&b) = self.s.get(self.i) {
                if b == b'"' || b == b'\\' || b < 0x20 {
                    break;
                }
                self.i += 1;
            }
            out.push_str(std::str::from_utf8(&self.s[start..self.i]).expect("a &str splits at ASCII"));
            match self.s.get(self.i) {
                None => return Err(self.err("the text ends inside a string")),
                Some(b'"') => {
                    self.i += 1;
                    return Ok(out);
                }
                Some(b'\\') => {
                    self.i += 1;
                    let c = match self.s.get(self.i) {
                        Some(b'"') => '"',
                        Some(b'\\') => '\\',
                        Some(b'/') => '/',
                        Some(b'b') => '\u{8}',
                        Some(b'f') => '\u{c}',
                        Some(b'n') => '\n',
                        Some(b'r') => '\r',
                        Some(b't') => '\t',
                        Some(b'u') => {
                            self.i += 1;
                            let unit = self.hex4()?;
                            let code = match unit {
                                0xd800..=0xdbff => {
                                    if !self.s[self.i..].starts_with(b"\\u") {
                                        return Err(self.err("a lone high surrogate"));
                                    }
                                    self.i += 2;
                                    let low = self.hex4()?;
                                    if !(0xdc00..=0xdfff).contains(&low) {
                                        return Err(self.err("a high surrogate without its low one"));
                                    }
                                    0x10000 + ((unit - 0xd800) << 10) + (low - 0xdc00)
                                }
                                0xdc00..=0xdfff => return Err(self.err("a lone low surrogate")),
                                unit => unit,
                            };
                            out.push(char::from_u32(code).expect("a valid scalar value"));
                            continue;
                        }
                        _ => return Err(self.err("an unknown escape")),
                    };
                    self.i += 1;
                    out.push(c);
                }
                Some(_) => return Err(self.err("a control character inside a string")),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values() {
        let v =
            Json::parse(r#" {"a": [1, -2.5e+3, 0, true, false, null], "b": {"c": "d"}, "e": []} "#).unwrap();
        assert_eq!(v.get("a").unwrap().as_array().unwrap().len(), 6);
        assert_eq!(v.get("a").unwrap().as_array().unwrap()[0].as_u64(), Some(1));
        assert_eq!(v.get("b").unwrap().get("c").unwrap().as_str(), Some("d"));
        assert_eq!(v.get("e"), Some(&Json::Array(vec![])));
        assert_eq!(v.get("z"), None);
        assert_eq!(
            v.encode(),
            r#"{"a":[1,-2.5e+3,0,true,false,null],"b":{"c":"d"},"e":[]}"#
        );
        assert_eq!(Json::parse("0").unwrap(), Json::Number("0".into()));
        assert_eq!(Json::parse("\"\"").unwrap(), Json::String(String::new()));
        assert_eq!(Json::parse("{}").unwrap(), Json::Object(vec![]));
    }

    #[test]
    fn numbers() {
        let n = |s: &str| Json::parse(s).unwrap();
        assert_eq!(n("18446744073709551615").as_u64(), Some(u64::MAX));
        assert_eq!(n("18446744073709551616").as_u64(), None);
        assert_eq!(n("-1").as_u64(), None);
        assert_eq!(n("1.0").as_u64(), None);
        assert_eq!(n("1e3").as_u64(), None);
        assert_eq!(Json::from(u64::MAX).encode(), "18446744073709551615");
        assert_eq!(Json::from(-5i64).encode(), "-5");
        assert_eq!(n("-9223372036854775808").as_i64(), Some(i64::MIN));
        assert_eq!(n("9223372036854775808").as_i64(), None);
        assert_eq!(n("-3").as_i64(), Some(-3));
        assert_eq!(n("-3.0").as_i64(), None);
        assert_eq!(n("2E1").as_i64(), None);
        assert_eq!(n("-33.25").as_f64(), Some(-33.25));
        assert_eq!(n("1e2").as_f64(), Some(100.0));
        assert_eq!(n("1e999").as_f64(), None, "not infinity");
        assert_eq!(Json::from("1").as_f64(), None);
        for bad in [
            "01", "-", "1.", ".5", "1e", "1e+", "+1", "-01", "0x10", "NaN", "Infinity", "1_000",
        ] {
            assert!(Json::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn strings() {
        let s = |text: &str| Json::parse(text).unwrap().as_str().unwrap().to_string();
        assert_eq!(s(r#""a\"b\\c\/d\b\f\n\r\t""#), "a\"b\\c/d\u{8}\u{c}\n\r\t");
        // A JSON \u escape, spelt out (so that no tool reading this file takes it for
        // one of its own).
        let u = |hex: &str| format!("{}u{hex}", '\\');
        assert_eq!(s(&format!("\"{}{}\"", u("00e9"), u("20ac"))), "é€");
        assert_eq!(s(&format!("\"{}{}\"", u("D83D"), u("de00"))), "😀");
        assert_eq!(s("\"é😀\""), "é😀");
        for bad in [
            r#""abc"#,
            r#""\x""#,
            r#""\u12""#,
            r#""\u12g4""#,
            r#""\ud83d""#,
            r#""\ud83dx""#,
            r#""\ude00""#,
            "\"a\nb\"",
            "\"a\tb\"",
            "\"\u{0}\"",
            "'single'",
        ] {
            assert!(Json::parse(bad).is_err(), "{bad}");
        }
        // A high surrogate followed by an escape that is not a low one.
        assert!(Json::parse(&format!("\"{}{}\"", u("d83d"), u("0041"))).is_err());
        // Encoding escapes what JSON needs and what HTML would read.
        assert_eq!(
            Json::from("q\"b\\n\n\u{1}</script>&\u{2028}é").encode(),
            format!(
                r#""q\"b\\n\n{}{}/script{}{}{}é""#,
                u("0001"),
                u("003c"),
                u("003e"),
                u("0026"),
                u("2028")
            )
        );
        // Whatever is encoded parses back.
        for text in [
            "",
            "plain",
            "\u{0}\u{1f}\u{7f}",
            "<>&'\"",
            "😀\u{2029}",
            &u("0041"),
        ] {
            let encoded = Json::from(text).encode();
            assert_eq!(Json::parse(&encoded).unwrap().as_str(), Some(text), "{encoded}");
        }
    }

    #[test]
    fn malformed() {
        for bad in [
            "",
            " ",
            "[",
            "]",
            "[1,]",
            "[1 2]",
            "[,1]",
            "{\"a\":1,}",
            "{\"a\" 1}",
            "{a:1}",
            "{\"a\":}",
            "{1:2}",
            "{\"a\":1 \"b\":2}",
            "{\"a\":1,\"a\":2}",
            "nul",
            "truth",
            "true false",
            "[] []",
            "// comment\n1",
            "1 /* c */",
            "\u{feff}1",
            "\u{a0}1",
        ] {
            assert!(Json::parse(bad).is_err(), "{bad:?}");
        }
        assert!(Json::parse("{\"a\":1,\"a\":2}")
            .unwrap_err()
            .contains("duplicate key"));
        // Many keys take linear time: 200,000 would take most of a minute compared one
        // by one with every earlier key.
        let many: Vec<String> = (0..200_000).map(|i| format!("\"k{i}\":{i}")).collect();
        let start = std::time::Instant::now();
        let text = format!("{{{}}}", many.join(","));
        assert!(Json::parse(&text).is_ok());
        let dup = format!("{{{},\"k7\":0}}", many.join(","));
        assert!(Json::parse(&dup).unwrap_err().contains("duplicate key \"k7\""));
        assert!(start.elapsed() < std::time::Duration::from_secs(10));
    }

    #[test]
    fn depth_is_limited() {
        let nested = |n: usize| format!("{}{}", "[".repeat(n), "]".repeat(n));
        assert!(Json::parse(&nested(MAX_DEPTH)).is_ok());
        assert!(Json::parse(&nested(MAX_DEPTH + 1))
            .unwrap_err()
            .contains("nested too deeply"));
        let objects = format!(
            "{}1{}",
            "{\"a\":".repeat(MAX_DEPTH + 1),
            "}".repeat(MAX_DEPTH + 1)
        );
        assert!(Json::parse(&objects).is_err());
        // A million brackets is an error, not a stack overflow.
        assert!(Json::parse(&"[".repeat(1_000_000)).is_err());
    }

    #[test]
    fn building() {
        let v = Json::object([
            ("id", 3u32.into()),
            ("name", "web".into()),
            ("clock", Json::from(None::<u32>)),
            ("disks", Json::from(vec!["a", "b"])),
            ("ok", true.into()),
        ]);
        assert_eq!(
            v.encode(),
            r#"{"id":3,"name":"web","clock":null,"disks":["a","b"],"ok":true}"#
        );
        assert_eq!(Json::parse(&v.encode()).unwrap(), v);
    }
}
