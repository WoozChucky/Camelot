//! The diagnostic cases of `preprocess` (generated; curated by hand).

use super::{case, Case};

#[rustfmt::skip]
pub fn cases() -> Vec<Case> {
    vec![
        // stringizing_spaces_and_quotes
        case("#define s(x) #x\nconst char *p = s(\\);", &["2:19: warning: invalid string literal, ignoring final '\\'"]),
        case("#define s(x) #x\ns(a \\)", &["2:5: warning: invalid string literal, ignoring final '\\'", "2:1: error: expected identifier or '('"]).partial("merlin gives no notes of where a macro was expanded"),
        // pasting
        case("#define cat(a, b) a ## b\ncat(+, -)", &["2:1: error: pasting formed '+-', an invalid preprocessing token", "2:1: error: expected external declaration"]).then(&["2:8: error: expected external declaration"]).partial("merlin gives no notes of where a macro was expanded"),
        case("#define cat(a, b) a ## b\ncat(/, /)", &["2:1: error: pasting formed '//', an invalid preprocessing token", "2:1: error: expected identifier or '('"]).partial("merlin gives no notes of where a macro was expanded"),
        // file_and_line
        case("#line 0", &[]),
        case("#line x", &["1:7: error: #line directive requires a positive integer argument"]),
        case("#define __LINE__ 1", &["1:9: warning: redefining builtin macro [-Wbuiltin-macro-redefined]"]),
        case("#undef __FILE__", &["1:8: warning: undefining builtin macro [-Wbuiltin-macro-redefined]"]),
        // includes
        case("#include <a.h>", &["1:10: error: 'a.h' file not found with <angled> include; use \"quotes\" instead"]).files(&[("a.h", "int a;\n#include \"sub/b.h\""), ("sub/b.h", "int b;\n#include \"c.h\""), ("sub/c.h", "int c;"), ("inc/d.h", "int d;"), ("inc/c.h", "int wrong;")]),
        case("#include \"nope.h\"", &["1:10: fatal error: 'nope.h' file not found"]).files(&[("a.h", "int a;\n#include \"sub/b.h\""), ("sub/b.h", "int b;\n#include \"c.h\""), ("sub/c.h", "int c;"), ("inc/d.h", "int d;"), ("inc/c.h", "int wrong;")]),
        case("#include", &["1:9: error: expected \"FILENAME\" or <FILENAME>"]),
        case("#include \"a.h\" x", &["1:16: warning: extra tokens at end of #include directive [-Wextra-tokens]", "1:10: fatal error: 'a.h' file not found"]),
        case("#include <a.h", &["1:14: error: expected '>'", "1:10: note: to match this '<'"]),
        // include_guards_and_once
        case("#include \"self.h\"", &["self.h:1:10: error: #include nested too deeply"]).files(&[("guard.h", "#ifndef GUARD_H\n#define GUARD_H\nint g;\n#endif\n"), ("once.h", "#pragma once\nint o;\n"), ("twice.h", "int t;\n"), ("self.h", "#include \"self.h\"\n")]),
        // errors_in_headers_name_the_header
        case("#include \"bad.h\"", &["bad.h:2:2: error: unterminated conditional directive"]).files(&[("bad.h", "\n#if 1\n"), ("worse.h", "#error stop  here")]),
        case("#include \"worse.h\"", &["worse.h:1:2: error: stop  here"]).files(&[("bad.h", "\n#if 1\n"), ("worse.h", "#error stop  here")]),
        case("#if 1\n#include \"end.h\"\n#endif", &["end.h:1:2: error: #endif without #if"]).files(&[("end.h", "#endif\n")]),
        // if_arithmetic
        case("#if 1 / 0\n#endif", &["1:7: error: division by zero in preprocessor expression"]),
        case("#if\n#endif", &["1:4: error: expected value in expression"]),
        case("#if 1 +\n#endif", &["1:8: error: expected value in expression"]),
        case("#if (1\n#endif", &["1:7: error: expected ')' in preprocessor expression", "1:5: note: to match this '('"]),
        case("#if 1 2\n#endif", &["1:7: error: token is not a valid binary operator in a preprocessor subexpression"]),
        case("#if 1 = 2\n#endif", &["1:7: error: token is not a valid binary operator in a preprocessor subexpression"]),
        case("#if 18446744073709551616 == 0\n#endif", &["1:5: error: integer literal is too large to be represented in any integer type"]),
        case("#if 0x1ffffffffffffffff == 0\n#endif", &["1:5: error: integer literal is too large to be represented in any integer type"]),
        case("#if 99999999999999999999999 == 0\n#endif", &["1:5: error: integer literal is too large to be represented in any integer type"]),
        case("#if 1.0\n#endif", &["1:5: error: floating point literal in preprocessor expression"]),
        case("#if defined\n#endif", &["1:12: error: macro name missing"]),
        case("#if defined(X\n#endif", &["1:14: error: missing ')' after 'defined'", "1:12: note: to match this '('"]),
        case("#define D defined(X)\n#if D\n#endif", &["2:5: warning: macro expansion producing 'defined' has undefined behavior [-Wexpansion-to-defined]"]).partial("merlin gives no notes of where a macro was expanded"),
        // conditional_errors
        case("#if 1\nx", &["1:2: error: unterminated conditional directive", "2:1: error: unknown type name 'x'"]).then(&["2:2: error: expected identifier or '('"]),
        case("#if 0\nx", &["1:2: error: unterminated conditional directive"]),
        case("#else", &["1:2: error: #else without #if"]),
        case("#elif 1", &["1:2: error: #elif without #if"]),
        case("#endif", &["1:2: error: #endif without #if"]),
        case("#if 1\n#else\n#else\n#endif", &["3:2: error: #else after #else"]),
        case("#if 0\n#else\n#elif 1\n#endif", &["3:2: error: #elif after #else"]),
        case("#if 1\n#endif X", &["2:8: warning: extra tokens at end of #endif directive [-Wextra-tokens]"]),
        case("#ifdef\n#endif", &["1:7: error: macro name missing"]),
        case("#ifdef 1\n#endif", &["1:8: error: macro name must be an identifier"]),
        // directives
        case("#ident x", &["1:8: error: invalid #ident directive"]),
        case("#sccs", &["1:6: error: invalid #ident directive"]),
        case("#error \"stop\" now", &["1:2: error: \"stop\" now"]),
        case("#bogus", &["1:2: error: invalid preprocessing directive"]),
        case("# 33 \"x.c\"", &[]),
        // definition_errors
        case("#define", &["1:8: error: macro name missing"]),
        case("#define 1 2", &["1:9: error: macro name must be an identifier"]),
        case("#define defined", &["1:9: error: 'defined' cannot be used as a macro name"]),
        case("#define N 1\n#define N 2", &["2:9: warning: 'N' macro redefined [-Wmacro-redefined]", "1:9: note: previous definition is here"]),
        case("#define F(a) a\n#define F(b) b", &["2:9: warning: 'F' macro redefined [-Wmacro-redefined]", "1:9: note: previous definition is here"]),
        case("#define N 1+1\n#define N 1 + 1", &["2:9: warning: 'N' macro redefined [-Wmacro-redefined]", "1:9: note: previous definition is here"]),
        case("#define F(a", &["1:12: error: expected comma in macro parameter list"]),
        case("#define F(a,)", &["1:13: error: expected identifier in macro parameter list"]),
        case("#define F(a b)", &["1:13: error: expected comma in macro parameter list"]),
        case("#define F(a, a)", &["1:14: error: duplicate macro parameter name 'a'"]),
        case("#define F(..., a)", &["1:14: error: missing ')' in macro parameter list"]),
        case("#define F(x) #y", &["1:15: error: '#' is not followed by a macro parameter"]),
        case("#define F(x) ## x", &["1:14: error: '##' cannot appear at start of macro expansion"]),
        case("#define F x ##", &["1:13: error: '##' cannot appear at end of macro expansion"]),
        case("#define F(x) __VA_ARGS__", &[]),
        // call_errors
        case("#define F(a, b) a\nF(1)", &["2:4: error: too few arguments provided to function-like macro invocation", "1:9: note: macro 'F' defined here", "2:1: error: unknown type name 'F'"]).then(&["2:5: error: expected identifier or '('"]),
        case("#define F(a) a\nF(1, 2)", &["2:6: error: too many arguments provided to function-like macro invocation", "1:9: note: macro 'F' defined here", "2:1: error: unknown type name 'F'"]).then(&["2:8: error: expected identifier or '('"]),
        case("#define F() 1\nF(x)", &["2:3: error: too many arguments provided to function-like macro invocation", "1:9: note: macro 'F' defined here", "2:1: error: unknown type name 'F'"]).then(&["2:5: error: expected identifier or '('"]),
        case("#define F(a) a\nF(1", &["2:1: error: unterminated function-like macro invocation", "1:9: note: macro 'F' defined here"]),
        case("#define F(a) a\nF(1\n#define X\n)", &["2:3: error: expected identifier or '('"]),
        case("#include \"open.h\"\n)", &["open.h:2:3: error: expected identifier or '('"]).files(&[("open.h", "#define F(a) a\nF(1")]),
        // command_line_macros
        case("", &[]),
        // assembly_directives
        // Which `#` lines are directives in assembly, and errors.
        case("#warn\n", &["1:2: error: invalid preprocessing directive"]),
        case("#define P(a, b) a ## b\nP(., x)", &["2:1: error: pasting formed '.x', an invalid preprocessing token", "2:1: error: expected identifier or '('"]).partial("merlin gives no notes of where a macro was expanded"),
        case("??=", &["1:1: warning: trigraph converted to '#' character [-Wtrigraphs]"]),
        // A `#line` name has only the simple escapes.
        case("#line 5 \"a\\101.c\"\nint x = y;", &["1:11: error: invalid escape sequence '\\101' in an unevaluated string literal", "2:9: error: use of undeclared identifier 'y'"]),
        case("#line 5 \"a\\x41.c\"\nint x = y;", &["1:11: error: invalid escape sequence '\\x41' in an unevaluated string literal", "2:9: error: use of undeclared identifier 'y'"]),
        case("#line 5 \"a\\\\b.c\"\nint x = y;", &["a/b.c:5:9: error: use of undeclared identifier 'y'"]),
        case("# 5 \"a\\101.c\"\nint x = y;", &["1:7: error: invalid escape sequence '\\101' in an unevaluated string literal", "2:9: error: use of undeclared identifier 'y'"]),
        case("#line 5 \"a\\q.c\"\nint x = y;", &["1:11: warning: unknown escape sequence '\\q' [-Wunknown-escape-sequence]", "1:11: error: invalid escape sequence '\\q' in an unevaluated string literal", "2:9: error: use of undeclared identifier 'y'"]),
        case("#line 5 \"a\\u00e9.c\"\nint x = y;", &["aé.c:5:9: error: use of undeclared identifier 'y'"]),
    ]
}
