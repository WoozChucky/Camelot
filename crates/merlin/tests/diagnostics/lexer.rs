//! The lexer's diagnostics: literals, characters that make no token,
//! comments, trigraphs.

use super::{case, Case};

#[rustfmt::skip]
pub fn cases() -> Vec<Case> {
    vec![
        // Integer literals: too large for any type, or a decimal one past
        // `long long` (unsigned, as clang takes it), bad suffixes and digits.
        case("int x = 99999999999999999999;", &["1:9: error: integer literal is too large to be represented in any integer type", "1:9: warning: implicit conversion from 'unsigned long long' to 'int' changes value from 7766279631452241919 to 1661992959 [-Wconstant-conversion]"]),
        case("unsigned long long x = 9223372036854775808;", &["1:24: warning: integer literal is too large to be represented in a signed integer type, interpreting as unsigned [-Wimplicitly-unsigned-literal]"]),
        case("long long x = 10lL;", &["1:17: error: invalid suffix 'lL' on integer constant"]),
        case("long long x = 10lll;", &["1:17: error: invalid suffix 'lll' on integer constant"]),
        case("int x = 09;", &["1:10: error: invalid digit '9' in octal constant"]),
        case("int x = 12abc;", &["1:11: error: invalid digit 'a' in decimal constant"]),
        case("int x = 0x;", &["1:10: error: invalid suffix 'x' on integer constant"]),
        case("int x = 1uu;", &["1:10: error: invalid suffix 'uu' on integer constant"]),
        case("int x = 0x1g;", &["1:12: error: invalid suffix 'g' on integer constant"]),
        // Character constants.
        case("int c = '';", &["1:9: warning: empty character constant [-Winvalid-pp-token]", "1:9: error: expected expression"]),
        case("int c = 'ab';", &["1:9: warning: multi-character character constant [-Wmultichar]"]),
        case("int c = 'abcde';", &["1:9: warning: multi-character character constant [-Wmultichar]", "1:9: warning: character constant too long for its type"]),
        case("int c = 'a;", &["1:9: warning: missing terminating ' character [-Winvalid-pp-token]", "1:9: error: expected expression"]).then(&["1:12: error: expected ';' after top level declarator"]),
        case("int c = '\\q';", &["1:10: warning: unknown escape sequence '\\q' [-Wunknown-escape-sequence]"]),
        case("int c = '\\x100';", &["1:10: error: hex escape sequence out of range"]),
        case("int c = '\\400';", &["1:10: error: octal escape sequence out of range"]),
        case("int c = '\\x';", &["1:10: error: \\x used with no following hex digits"]),
        // String literals.
        case("char *s = \"abc\nd\";", &["1:11: warning: missing terminating '\"' character [-Winvalid-pp-token]", "1:11: error: expected expression", "2:2: warning: missing terminating '\"' character [-Winvalid-pp-token]"]).then(&["2:4: error: expected ';' after top level declarator"]),
        case("char *s = \"abc", &["1:11: warning: missing terminating '\"' character [-Winvalid-pp-token]", "1:11: error: expected expression"]).then(&["1:15: error: expected ';' after top level declarator"]),
        case("char *s = \"\\q\";", &["1:12: warning: unknown escape sequence '\\q' [-Wunknown-escape-sequence]"]),
        // What makes no token, or no token outside a directive.
        case("int x;\n  /* oops", &["2:3: error: unterminated /* comment"]),
        case("int x = 1 @ 2;", &["1:10: error: expected ';' after top level declarator"]),
        case("int x = 1 \\ 2;", &["1:10: error: expected ';' after top level declarator"]),
        case("int x = 1 # 2;", &["1:10: error: expected ';' after top level declarator"]),
        case("int x = 1 ## 2;", &["1:10: error: expected ';' after top level declarator"]),
        case("int f(void) { return 1 ` 2; }", &["1:23: error: expected ';' after return statement"]),
        case("#include <x.h>", &["1:10: fatal error: 'x.h' file not found"]),
        // What merlin's language has not, which clang takes.
        case("int f(void) { float s; return 0; }", &["1:15: error: 'float' is not supported yet"]).merlin("no floating point"),
        case("int x = 1.5;", &["1:9: error: floating-point literals are not supported"]).merlin("no floating point"),
        case("int x = .5;", &["1:9: error: floating-point literals are not supported"]).merlin("no floating point"),
        case("int x = 1e5;", &["1:9: error: floating-point literals are not supported"]).merlin("no floating point"),
        case("int x = 0x1p3;", &["1:9: error: floating-point literals are not supported"]).merlin("no floating point"),
        // Trigraphs, which C99 replaces before anything else, in literals,
        // in code, in a directive, and in a comment, where `??/` before
        // the newline splices the next line on.
        case("char *s = \"a??=\";", &["1:13: warning: trigraph converted to '#' character [-Wtrigraphs]"]),
        case("int c = '??'';", &["1:10: warning: trigraph converted to '^' character [-Wtrigraphs]"]),
        case("char *s = \"\\??/\";", &["1:13: warning: trigraph converted to '\\' character [-Wtrigraphs]"]),
        case("int x = 1 ??- 2;", &["1:11: warning: trigraph converted to '~' character [-Wtrigraphs]", "1:10: error: expected ';' after top level declarator"]),
        case("int x; // note ??/  \r\nint y = 2;", &["1:18: warning: backslash and newline separated by space [-Wbackslash-newline-escape]"]),
        case("??=define N 1", &["1:1: warning: trigraph converted to '#' character [-Wtrigraphs]"]),
        case("int x; /* ??= ??/  \n */ int y ??( 2 ??) = ??< 1 ??>;", &["2:11: warning: trigraph converted to '[' character [-Wtrigraphs]", "2:17: warning: trigraph converted to ']' character [-Wtrigraphs]", "2:23: warning: trigraph converted to '{' character [-Wtrigraphs]", "2:29: warning: trigraph converted to '}' character [-Wtrigraphs]"]),
        case("int a = 1 ??/  \n+ 2;", &["1:11: warning: trigraph converted to '\\' character [-Wtrigraphs]", "1:14: warning: backslash and newline separated by space [-Wbackslash-newline-escape]"]),
        // A line splice with blanks before its newline: at the backslash
        // in a `//` comment, after it elsewhere, nothing in a `/* */` one.
        case("int a = 1 \\  \n+ 2;", &["1:12: warning: backslash and newline separated by space [-Wbackslash-newline-escape]"]),
        case("int x; // note \\  \nint y = 2;", &["1:16: warning: backslash and newline separated by space [-Wbackslash-newline-escape]"]),
        case("char *s = \"a\\ \r\nb\";", &["1:14: warning: backslash and newline separated by space [-Wbackslash-newline-escape]"]),
        case("int x; /* a \\ \n */", &[]),
        // `register` is a storage class of objects in a block (or of
        // parameters), whose address cannot be taken.
        case("register int x;", &["1:14: error: illegal storage class on file-scoped variable"]),
        case("int f(void) { register int x = 1; return *&x; }", &["1:43: error: address of register variable requested"]),
        case("int f(register int x) { int *p = &x; return *p; }", &["1:34: error: address of register variable requested"]),
        case("auto int y;", &["1:10: error: illegal storage class on file-scoped variable"]),
        case("register int f(void);", &["1:1: error: illegal storage class on function"]),
        case("int f(auto int x);", &["1:7: error: invalid storage class specifier in function declarator"]),
        case("int g(register int x) { return x; }", &[]),
        case("int f(void) { auto int a = 1; register int b = 2; register int c[2]; register struct { int m; } s; int *p = &a; s.m = b; return *&b + s.m + c[1] + sizeof c; }", &["1:130: error: address of register variable requested", "1:141: error: address of register variable requested"]),
        case("int f(void) { register struct { int m; } s; return (&s)->m; }", &["1:53: error: address of register variable requested"]),
        case("int f(void) { register struct { int m; } s; int *p = &s.m; return *p; }", &["1:54: error: address of register variable requested"]),
        case("int f(void) { register int g(void); return 0; }", &["1:15: error: illegal storage class on function"]),
        case("struct s { register int x; };", &["1:12: error: type name does not allow storage class to be specified"]),
        case("int f(void) { static register int x; return 0; }", &["1:22: error: cannot combine with previous 'static' declaration specifier"]),
        case("typedef register int R;", &["1:9: error: cannot combine with previous 'typedef' declaration specifier"]),
        // Escapes clang takes as it does: GNU's `\e`, its nonstandard ones,
        // universal character names and what C99 refuses of them.
        case("int a = '\\e' + '\\E' + '\\(' + '\\{' + '\\[' + '\\%' + '\\q';", &["1:52: warning: unknown escape sequence '\\q' [-Wunknown-escape-sequence]"]),
        case("char *s = \"\\u00e9\\U0001F600\\u0041\\u0024\\uD800\\u12\";", &["1:28: error: character 'A' cannot be specified by a universal character name", "1:40: error: invalid universal character", "1:46: error: incomplete universal character name"]),
        case("char *s = \"\\u0001\\u0085\\u0060\\u0040\\u00a0\\u007f\";", &["1:12: error: universal character name refers to a control character", "1:18: error: universal character name refers to a control character", "1:42: error: universal character name refers to a control character"]),
        case("char *t = \"\\U00110000\";", &["1:12: error: invalid universal character"]),
        case("int c = '\\u00e9'; int d = '\\u0041'; int e = '\\u0024';", &["1:9: error: character too large for enclosing character literal type", "1:28: error: character 'A' cannot be specified by a universal character name"]),
        case("int x[sizeof \"\\u00e9\\U0001F600\" == 7 ? 1 : -1];", &[]),
        // Four characters are no -Wmultichar (-Wfour-char-constants is off).
        case("int c = 'abcd'; int d = 'abc';", &["1:25: warning: multi-character character constant [-Wmultichar]"]),
        // Columns count bytes; a byte-order mark is skipped, its bytes counted.
        case("/* \u{e9}\u{e9} */ int f(void) { return y; }", &["1:33: error: use of undeclared identifier 'y'"]),
        case("int f(void) { char *s = \"\u{20ac}\\q\"; return y; }", &["1:29: warning: unknown escape sequence '\\q' [-Wunknown-escape-sequence]", "1:41: error: use of undeclared identifier 'y'"]),
        case("\u{feff}int x = y;\nint z = w;", &["1:12: error: use of undeclared identifier 'y'", "2:9: error: use of undeclared identifier 'w'"]),
        // A character past ASCII is too large for a character constant.
        case("int a = '\u{e9}';\n#if '\u{e9}' == 0xc3a9\nint pp;\n#endif", &["1:9: error: character too large for enclosing character literal type", "2:5: error: character too large for enclosing character literal type"]),
        case("int b = 'a\u{e9}'; int c = '\u{e9}a'; int e = '\u{20ac}'; int f = 'ab';", &["1:9: error: character too large for enclosing character literal type", "1:9: warning: multi-character character constant [-Wmultichar]", "1:24: error: character too large for enclosing character literal type", "1:24: warning: multi-character character constant [-Wmultichar]", "1:39: error: character too large for enclosing character literal type", "1:54: warning: multi-character character constant [-Wmultichar]"]),
        case("char *s = \"\u{e9}\"; int g = '\\xe9' + 'abcde';", &["1:34: warning: multi-character character constant [-Wmultichar]", "1:34: warning: character constant too long for its type"]),
    ]
}
