//! clang's default warnings of the language merlin has, which merlin
//! gives too: of what is written (precedence, braces, an empty body), of
//! constants (conversions that change them, overflow, comparisons always
//! true or false), and of what runs (a shift too far, dividing by zero,
//! falling off the end of a function), the last only where the code can
//! run.

use super::{case, Case};

#[rustfmt::skip]
pub fn cases() -> Vec<Case> {
    vec![
        // -Wparentheses, -Wshift-op-parentheses: but in parentheses.
        case("int f(int a, int b, int c) { return (a & b == c) + (a == b & c) + (a | b != c) + (a ^ b < c); }", &["1:40: warning: & has lower precedence than ==; == will be evaluated first [-Wparentheses]", "1:40: note: place parentheses around the '==' expression to silence this warning", "1:40: note: place parentheses around the & expression to evaluate it first", "1:60: warning: & has lower precedence than ==; == will be evaluated first [-Wparentheses]", "1:60: note: place parentheses around the '==' expression to silence this warning", "1:60: note: place parentheses around the & expression to evaluate it first", "1:70: warning: | has lower precedence than !=; != will be evaluated first [-Wparentheses]", "1:70: note: place parentheses around the '!=' expression to silence this warning", "1:70: note: place parentheses around the | expression to evaluate it first", "1:85: warning: ^ has lower precedence than <; < will be evaluated first [-Wparentheses]", "1:85: note: place parentheses around the '<' expression to silence this warning", "1:85: note: place parentheses around the ^ expression to evaluate it first"]),
        case("int f(int a, int b, int c) { return (a << b + c) + (a + b << c) + (a >> b - c); }", &["1:45: warning: operator '<<' has lower precedence than '+'; '+' will be evaluated first [-Wshift-op-parentheses]", "1:45: note: place parentheses around the '+' expression to silence this warning", "1:55: warning: operator '<<' has lower precedence than '+'; '+' will be evaluated first [-Wshift-op-parentheses]", "1:55: note: place parentheses around the '+' expression to silence this warning", "1:75: warning: operator '>>' has lower precedence than '-'; '-' will be evaluated first [-Wshift-op-parentheses]", "1:75: note: place parentheses around the '-' expression to silence this warning"]),
        case("int f(int a, int b, int c) { return (a & (b == c)) + ((a + b) << c) + (a & b & c == 1) + (a < b == c); }", &[]),
        // -Wdangling-else, through loops but not braces.
        case("int f(int a, int b, int c) { if (a) if (b) c = 1; else c = 2; return c; }", &["1:51: warning: add explicit braces to avoid dangling else [-Wdangling-else]"]),
        case("int f(int a, int b, int c) { if (a) while (b) if (c) c = 1; else c = 2; return c; }", &["1:61: warning: add explicit braces to avoid dangling else [-Wdangling-else]"]),
        case("int f(int a, int b, int c) { if (a) { if (b) c = 1; else c = 2; } if (a) if (b) c = 1; else c = 2; else c = 3; return c; }", &[]),
        // -Wconstant-logical-operand: a constant other than 0 and 1 on the right.
        case("int f(int a) { return (a && 4) + (a || 2) + (a && 1) + (4 && a) + (5 && 7); }", &["1:26: warning: use of logical '&&' with constant operand [-Wconstant-logical-operand]", "1:26: note: use '&' for a bitwise operation", "1:26: note: remove constant to silence this warning", "1:37: warning: use of logical '||' with constant operand [-Wconstant-logical-operand]", "1:37: note: use '|' for a bitwise operation", "1:70: warning: use of logical '&&' with constant operand [-Wconstant-logical-operand]", "1:70: note: use '&' for a bitwise operation", "1:70: note: remove constant to silence this warning"]),
        // -Wconstant-conversion: at once of a value exactly as wide, at the
        // end of the function of one too wide.
        case("int f(void) { signed char a = 200; unsigned char b = 300; short c = 40000; unsigned short d = -1; char e = 255; return a + b + c + d + e; }", &["1:31: warning: implicit conversion from 'int' to 'signed char' changes value from 200 to -56 [-Wconstant-conversion]", "1:69: warning: implicit conversion from 'int' to 'short' changes value from 40000 to -25536 [-Wconstant-conversion]", "1:54: warning: implicit conversion from 'int' to 'unsigned char' changes value from 300 to 44 [-Wconstant-conversion]"]),
        case("signed char g = 128; unsigned char h = 256; int f(unsigned char x); int k(void) { return f(1000); }", &["1:17: warning: implicit conversion from 'int' to 'signed char' changes value from 128 to -128 [-Wconstant-conversion]", "1:40: warning: implicit conversion from 'int' to 'unsigned char' changes value from 256 to 0 [-Wconstant-conversion]", "1:92: warning: implicit conversion from 'int' to 'unsigned char' changes value from 1000 to 232 [-Wconstant-conversion]"]),
        // -Winteger-overflow of signed constants.
        case("int f(void) { return 2147483647 + 1; }", &["1:33: warning: overflow in expression; result is -2'147'483'648 with type 'int' [-Winteger-overflow]"]),
        case("long long f(void) { return 0x100000000LL * 0x100000000LL + (-2147483647 - 1) * 1; }", &["1:42: warning: overflow in expression; result is 0 with type 'long long' [-Winteger-overflow]"]),
        case("unsigned f(void) { return 4294967295u + 1; }", &[]),
        // -Wshift-count-overflow, -Wshift-count-negative, -Wdivision-by-zero,
        // where the code can run.
        case("int f(int x) { return (x << 32) + (x >> -1) + x / 0 + x % 0; }", &["1:26: warning: shift count >= width of type [-Wshift-count-overflow]", "1:38: warning: shift count is negative [-Wshift-count-negative]", "1:49: warning: division by zero is undefined [-Wdivision-by-zero]", "1:57: warning: remainder by zero is undefined [-Wdivision-by-zero]"]),
        case("long long f(long long x) { return x << 63; }", &[]),
        case("int f(int x) { if (0) return x / 0; return 0 ? x / 0 : 0 && x % 0; }", &[]),
        case("int f(int x) { while (1) { if (x) return 1; } return x / 0; }", &[]),
        // -Wtautological-constant-out-of-range-compare.
        case("int f(unsigned short us, unsigned char uc, int i) { return (-1 < us) + (uc == 256) + (300 > uc) + (i < 5); }", &["1:91: warning: result of comparison of constant 300 with expression of type 'unsigned char' is always true [-Wtautological-constant-out-of-range-compare]", "1:76: warning: result of comparison of constant 256 with expression of type 'unsigned char' is always false [-Wtautological-constant-out-of-range-compare]", "1:64: warning: result of comparison of constant -1 with expression of type 'unsigned short' is always true [-Wtautological-constant-out-of-range-compare]"]),
        // -Wunevaluated-expression: sizeof's operand changes something.
        case("int f(int n) { return sizeof n++ + sizeof(n = 1) + sizeof(f(n)); }", &["1:31: warning: expression with side effects has no effect in an unevaluated context [-Wunevaluated-expression]", "1:42: warning: expression with side effects has no effect in an unevaluated context [-Wunevaluated-expression]"]),
        // A switch on a constant no case matches.
        case("int f(void) { switch (5) { case 1: return 1; } switch (2) { case 2: return 2; } return 0; }", &["1:23: warning: no case matching constant switch condition '5'"]),
        case("int f(void) { switch (5) { case 1: return 1; default: return 3; } }", &[]),
        // -Wreturn-type: falling off a function's end, or some of its paths.
        case("int f(int x) { x = x + 1; }", &["1:27: warning: non-void function does not return a value [-Wreturn-type]"]),
        case("int f(int x) { if (x) return 1; }", &["1:33: warning: non-void function does not return a value in all control paths [-Wreturn-type]"]),
        case("int f(int x) { while (1) ; }", &[]),
        case("int f(int x) { for (;;) { if (x) break; } }", &["1:43: warning: non-void function does not return a value [-Wreturn-type]"]),
        case("int f(int x) { switch (x) { case 1: return 1; default: return 2; } }", &[]),
        case("int f(int x) { if (x) goto a; return 1; a: x++; }", &["1:49: warning: non-void function does not return a value in all control paths [-Wreturn-type]"]),
        case("int main(void) { }", &["1:18: warning: non-void function does not return a value [-Wreturn-type]"]),
        case("void f(int x) { }", &[]),
        // -Wpointer-bool-conversion, -Wtautological-pointer-compare.
        case("int g; int a[2]; int f(void); int h(void) { return !&g + !f + !a + (&g ? 1 : 2) + (f && 1) + (a || 0); }", &["1:70: warning: address of 'g' will always evaluate to 'true' [-Wpointer-bool-conversion]", "1:95: warning: address of array 'a' will always evaluate to 'true' [-Wpointer-bool-conversion]", "1:84: warning: address of function 'f' will always evaluate to 'true' [-Wpointer-bool-conversion]", "1:84: note: prefix with the address-of operator to silence this warning", "1:64: warning: address of array 'a' will always evaluate to 'true' [-Wpointer-bool-conversion]", "1:59: warning: address of function 'f' will always evaluate to 'true' [-Wpointer-bool-conversion]", "1:59: note: prefix with the address-of operator to silence this warning", "1:54: warning: address of 'g' will always evaluate to 'true' [-Wpointer-bool-conversion]"]),
        case("int g; int a[2]; int f(void); int h(void) { if (&g) return 1; return &g == 0 || f != 0 || a == 0 || &g == (void*)0; }", &["1:50: warning: address of 'g' will always evaluate to 'true' [-Wpointer-bool-conversion]", "1:71: warning: comparison of address of 'g' equal to a null pointer is always false [-Wtautological-pointer-compare]", "1:81: warning: comparison of function 'f' not equal to a null pointer is always true [-Wtautological-pointer-compare]", "1:81: note: prefix with the address-of operator to silence this warning", "1:81: note: suffix with parentheses to turn this into a function call", "1:91: warning: comparison of array 'a' equal to a null pointer is always false [-Wtautological-pointer-compare]", "1:102: warning: comparison of address of 'g' equal to a null pointer is always false [-Wtautological-pointer-compare]"]),
        // -Wtautological-compare: a thing compared with itself.
        case("struct s { int x; } v; int h(int *p) { return v.x == v.x || p == p || *p == *p || p[0] < p[0] || (&v)->x >= (&v)->x; }", &["1:51: warning: self-comparison always evaluates to true [-Wtautological-compare]", "1:63: warning: self-comparison always evaluates to true [-Wtautological-compare]", "1:88: warning: self-comparison always evaluates to false [-Wtautological-compare]"]),
        case("int f(void) { return sizeof(int) == 4 || 1 == 1; }", &[]),
        // -Wempty-body.
        case("int f(int x) { if (x); switch (x); while (x); x = 1; for (;x;); { x = 2; } return x; }", &["1:22: warning: if statement has empty body [-Wempty-body]", "1:22: note: put the semicolon on a separate line to silence this warning", "1:34: warning: switch statement has empty body [-Wempty-body]", "1:34: note: put the semicolon on a separate line to silence this warning", "1:45: warning: while loop has empty body [-Wempty-body]", "1:45: note: put the semicolon on a separate line to silence this warning", "1:63: warning: for loop has empty body [-Wempty-body]", "1:63: note: put the semicolon on a separate line to silence this warning"]),
        // In a macro's expansion, what clang does not warn of.
        case("#define N 4\nint f(int a) { return a && N; }", &["2:25: warning: use of logical '&&' with constant operand [-Wconstant-logical-operand]", "2:25: note: use '&' for a bitwise operation", "2:25: note: remove constant to silence this warning"]),
    ]
}
