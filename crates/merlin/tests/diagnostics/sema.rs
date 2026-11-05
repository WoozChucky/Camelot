//! The diagnostic cases of `sema` (generated; curated by hand).

use super::{case, Case};

#[rustfmt::skip]
pub fn cases() -> Vec<Case> {
    vec![
        // what_the_parser_evaluated_away_is_checked
        // What the parser evaluated away in an array size, an enumerator or a
        // `_Static_assert` (the operand a constant `?:` does not choose, that
        // of `sizeof`) is checked as clang 23 checks it, without evaluating it:
        // its errors are reported, at file scope and in a block, where it was
        // (a local's name is in scope). One the parser could not type is an
        // error even without one of its own. The warning of an array size
        // folded goes with the program, unless its operand has an error.
        case("int f(void); int a[1 ? 2 : f(1, 2)];", &["1:30: error: too many arguments to function call, expected 0, have 2", "1:5: note: 'f' declared here"]),
        case("int f(int); enum { A = 1 ? 2 : f() };", &["1:34: error: too few arguments to function call, expected 1, have 0", "1:5: note: 'f' declared here"]),
        case("int f(void); int a[1 ? 2 : sizeof(f(3))];", &["1:37: error: too many arguments to function call, expected 0, have 1", "1:5: note: 'f' declared here"]),
        case("int f(void); int a[sizeof(f(3))];", &["1:29: error: too many arguments to function call, expected 0, have 1", "1:5: note: 'f' declared here"]),
        case("int f(void); _Static_assert(0 ? f(1) : 1, \"m\");", &["1:35: error: too many arguments to function call, expected 0, have 1", "1:5: note: 'f' declared here"]),
        case("int f(void); int g(void) { int a[1 ? 2 : f(1, 2)] = {1}; return a[0]; }", &["1:44: error: too many arguments to function call, expected 0, have 2", "1:5: note: 'f' declared here"]),
        case("struct S { int x; } s; _Static_assert(1 ? 1 : s.y, \"m\");", &["1:49: error: no member named 'y' in 'struct S'"]),
        case("int a[1 ? 2 : nothing];", &["1:15: error: use of undeclared identifier 'nothing'"]),
        case("int *p; int a[1 ? 2 : p];", &["1:17: warning: pointer/integer type mismatch in conditional expression ('int' and 'int *') [-Wconditional-type-mismatch]", "1:15: error: size of array has non-integer type 'int *'"]),
        case("unsigned u(void); int a[1 ? 2 : u()];", &["1:23: warning: variable length array folded to constant array as an extension [-Wgnu-folding-constant]"]),
        // folding_warnings_are_clangs
        // clang gives a declarator no folding warning when one of its sizes
        // has an error; and the warnings before a syntax error come with it.
        case("unsigned u(void); int f(void);\nint b[1 ? 2 : f(1, 2)][1 ? 3 : u()];", &["2:17: error: too many arguments to function call, expected 0, have 2", "1:23: note: 'f' declared here", "2:5: error: variably modified type declaration not allowed at file scope"]),
        case("unsigned u(void); int f(void);\nint b[1 ? 3 : u()][sizeof f(1)];", &["2:29: error: too many arguments to function call, expected 0, have 1", "1:23: note: 'f' declared here", "2:5: error: variable length array declaration not allowed at file scope"]),
        case("unsigned u(void);\nint a[1 ? 2 : u()];\nint b = ;", &["2:5: warning: variable length array folded to constant array as an extension [-Wgnu-folding-constant]", "3:9: error: expected expression"]),
        case("unsigned u(void);\nint f(void) { if (1) { static int a[1 ? 2 : u()]; { int q = ; } } }", &["2:35: warning: variable length array folded to constant array as an extension [-Wgnu-folding-constant]", "2:61: error: expected expression"]),
        case("unsigned u(void);\nint *q = (int[1 ? 2 : u()]){1, 2};", &["2:10: warning: variable length array folded to constant array as an extension [-Wgnu-folding-constant]"]),
        // compound_literals_are_read_as_constants
        // A compound literal's value where a constant is needed is the value
        // its list gives, and an array or a constant struct is initialized by
        // a literal of its type as by its list (GNU extensions clang 23 takes):
        // the data are clang's, and no object is emitted but for a literal
        // whose address is taken.
        case("int g; int c = (int){g};", &["1:22: error: initializer element is not a compile-time constant"]),
        case("int g; int ic[2] = (int[2]){g, 2};", &["1:29: error: initializer element is not a compile-time constant"]),
        case("int ix = (int[2]){1, 5}[1];", &["1:10: error: initializer element is not a compile-time constant"]),
        case("union W { int i; char c; }; int wc = (union W){5}.c;", &["1:38: error: initializer element is not a compile-time constant"]),
        case("int f(void) { static int *sp = &(int){5}; return *sp; }", &["1:32: error: initializer element is not a compile-time constant"]),
        case("int a[3] = (int[2]){1, 2};", &["1:5: error: cannot initialize array of type 'int[3]' with array of type 'int[2]'"]),
        case("int a[3] = (int[]){1, 2};", &["1:5: error: cannot initialize array of type 'int[3]' with array of type 'int[2]'"]),
        case("long a[2] = (int[2]){1, 2};", &["1:6: error: cannot initialize array of type 'long[2]' with array of type 'int[2]'"]),
        case("int a[2] = (const int[2]){1, 2};", &["1:5: error: cannot initialize array of type 'int[2]' with array of type 'const int[2]'"]),
        case("struct T { int x[2]; } t = { (int[2]){1, 2} };", &["1:30: error: incompatible pointer to integer conversion initializing 'int' with an expression of type 'int[2]' [-Wint-conversion]"]).rv32(),
        case("struct T { int x[2]; } t = { (int[2]){1, 2} };", &["1:30: error: incompatible pointer to integer conversion initializing 'int' with an expression of type 'int[2]' [-Wint-conversion]", "1:30: error: initializer element is not a compile-time constant"]).rv64(),
        // sizes_the_parser_cannot_work_out_are_checked
        // A constant the parser cannot work out (an operand it cannot type, a
        // name that is no constant) is sema's to check: its error is clang's,
        // where clang reports it, and clang says nothing more of the size
        // (clang 23).
        case("int f(int n, char a[sizeof n.y]);", &["1:29: error: member reference base type 'int' is not a structure or union"]),
        case("int f(int n, char a[1 ? 2 : n.y]);", &["1:30: error: member reference base type 'int' is not a structure or union"]),
        case("struct S { int y; } g;\nint f(void) { int g = 0; { char a[sizeof(g.y)] = {0}; return a[0]; } }", &["2:43: error: member reference base type 'int' is not a structure or union"]),
        case("int x; int a[sizeof x.y];", &["1:22: error: member reference base type 'int' is not a structure or union"]),
        case("int x; int a[x.y];", &["1:15: error: member reference base type 'int' is not a structure or union"]),
        case("int x; enum { E = sizeof x.y };", &["1:27: error: member reference base type 'int' is not a structure or union"]),
        case("int x; _Static_assert(sizeof x.y, \"\");", &["1:31: error: member reference base type 'int' is not a structure or union"]),
        case("int x; struct T { char a[sizeof x.y]; };", &["1:34: error: member reference base type 'int' is not a structure or union"]),
        case("int f(void) { int n = 1; char a[1 ? 2 : n.y]; return 0; }", &["1:42: error: member reference base type 'int' is not a structure or union"]),
        case("int a[sizeof nothing];", &["1:14: error: use of undeclared identifier 'nothing'"]),
        case("int a[nothing];", &["1:7: error: use of undeclared identifier 'nothing'"]),
        case("int x; int a[1 ? 0 : nothing];", &["1:22: error: use of undeclared identifier 'nothing'"]),
        case("int f(int n, char a[sizeof(n * \"a\")]);", &["1:30: error: invalid operands to binary expression ('int' and 'char[2]')"]),
        case("int f(void); int (*a[2])[1 ? 2 : f(1, 2)];", &["1:36: error: too many arguments to function call, expected 0, have 2", "1:5: note: 'f' declared here"]),
        case("int f(void); int (*fr(void))[1 ? 2 : f(1, 2)];", &["1:40: error: too many arguments to function call, expected 0, have 2", "1:5: note: 'f' declared here"]),
        case("int f(void); struct S { int (*p[2])[1 ? 2 : f(1, 2)]; } s;", &["1:47: error: too many arguments to function call, expected 0, have 2", "1:5: note: 'f' declared here"]),
        case("int f(void); int g(void) { static int (*p)[1 ? 2 : f(1, 2)]; return 0; }", &["1:54: error: too many arguments to function call, expected 0, have 2", "1:5: note: 'f' declared here"]),
        case("int f(void) { int n = 1; char a[n]; return 0; }", &[]),
        // for_scope
        case("int f() { for (int i = 0; i < 3; i = i + 1) {} return i; }", &["1:55: error: use of undeclared identifier 'i'"]),
        // initializer_sees_its_own_name
        case("int f() { static int a[] = {1, 2, sizeof a}; return 0; }", &["1:42: error: invalid application of 'sizeof' to an incomplete type 'int[]'"]),
        // globals_and_statics
        case("int f() { return g; } int g;", &["1:18: error: use of undeclared identifier 'g'"]),
        // global_definitions
        case("int a[1]; int a[] = {1, 2};", &["1:25: warning: excess elements in array initializer [-Wexcess-initializers]"]),
        case("int a = 1; int a = 2;", &["1:16: error: redefinition of 'a'", "1:5: note: previous definition is here"]),
        case("int a; char a;", &["1:13: error: redefinition of 'a' with a different type: 'char' vs 'int'", "1:5: note: previous definition is here"]),
        case("int a[];", &["1:5: warning: tentative array definition assumed to have one element [-Wtentative-definition-array]"]),
        case("int a; static int a;", &["1:19: error: static declaration of 'a' follows non-static declaration", "1:5: note: previous definition is here"]),
        case("static int a; int a;", &["1:19: error: non-static declaration of 'a' follows static declaration", "1:12: note: previous definition is here"]),
        case("int f(); int f;", &["1:14: error: redefinition of 'f' as different kind of symbol", "1:5: note: previous definition is here"]),
        case("int f; int f();", &["1:12: error: redefinition of 'f' as different kind of symbol", "1:5: note: previous definition is here"]),
        case("void v;", &["1:6: error: variable has incomplete type 'void'"]),
        case("extern int e = 1;", &["1:12: warning: 'extern' variable has an initializer [-Wextern-initializer]"]),
        // constant_initializers
        case("int a; int b = a;", &["1:16: error: initializer element is not a compile-time constant"]),
        case("int f(); int b = f();", &["1:18: error: initializer element is not a compile-time constant"]),
        case("int f() { static int s = f(); return s; }", &["1:26: error: initializer element is not a compile-time constant"]),
        case("int a; int b = (int)&a;", &["1:16: warning: cast to smaller integer type 'int' from 'int *' [-Wpointer-to-int-cast]", "1:16: error: initializer element is not a compile-time constant"]).rv64(),
        case("int a; unsigned b = (unsigned)&a;", &["1:21: warning: cast to smaller integer type 'unsigned int' from 'int *' [-Wpointer-to-int-cast]", "1:21: error: initializer element is not a compile-time constant"]).rv64(),
        case("int a; int b = (int)(long)&a;", &["1:16: error: initializer element is not a compile-time constant"]).rv64(),
        case("int a; long long b = (long long)&a;", &["1:22: error: initializer element is not a compile-time constant"]).rv32(),
        case("int a; unsigned long long b = (unsigned long long)&a;", &["1:31: error: initializer element is not a compile-time constant"]).rv32(),
        case("int a; short b = (short)&a;", &["1:18: warning: cast to smaller integer type 'short' from 'int *' [-Wpointer-to-int-cast]", "1:18: error: initializer element is not a compile-time constant"]).rv32(),
        case("int a; short b = (short)&a;", &["1:18: warning: cast to smaller integer type 'short' from 'int *' [-Wpointer-to-int-cast]", "1:18: error: initializer element is not a compile-time constant"]).rv64(),
        case("int a; char b = (char)&a;", &["1:17: warning: cast to smaller integer type 'char' from 'int *' [-Wpointer-to-int-cast]", "1:17: error: initializer element is not a compile-time constant"]).rv32(),
        case("int a; char b = (char)&a;", &["1:17: warning: cast to smaller integer type 'char' from 'int *' [-Wpointer-to-int-cast]", "1:17: error: initializer element is not a compile-time constant"]).rv64(),
        case("int a; long b = (long)&a * 2;", &["1:26: error: initializer element is not a compile-time constant"]).rv32(),
        case("int a; long b = (long)&a * 2;", &["1:26: error: initializer element is not a compile-time constant"]).rv64(),
        case("int a; long b = (long)&a | 1;", &["1:26: error: initializer element is not a compile-time constant"]).rv32(),
        case("int a; long b = (long)&a | 1;", &["1:26: error: initializer element is not a compile-time constant"]).rv64(),
        case("int a; long b = -(long)&a;", &["1:17: error: initializer element is not a compile-time constant"]).rv32(),
        case("int a; long b = -(long)&a;", &["1:17: error: initializer element is not a compile-time constant"]).rv64(),
        case("int a; long b = 4 - (long)&a;", &["1:19: error: initializer element is not a compile-time constant"]).rv32(),
        case("int a; long b = 4 - (long)&a;", &["1:19: error: initializer element is not a compile-time constant"]).rv64(),
        case("int a; long b = (long)&a - (long)&a;", &["1:26: error: initializer element is not a compile-time constant"]).rv32(),
        case("int a; long b = (long)&a - (long)&a;", &["1:26: error: initializer element is not a compile-time constant"]).rv64(),
        // array_initializers
        case("int a[2] = {1, 2, 3};", &["1:19: warning: excess elements in array initializer [-Wexcess-initializers]"]),
        case("char s[2] = \"abc\";", &["1:13: warning: initializer-string for char array is too long [-Wexcess-initializers]"]),
        case("int a[2] = 5;", &["1:5: error: array initializer must be an initializer list or wide string literal"]),
        case("int a[2] = \"ab\";", &["1:5: error: initializing wide char array with non-wide string literal"]),
        case("struct p { int x, y; }; struct r { struct p p; int z; }; int f(struct p v) { struct r r = {v, .p.y = 9}; return r.p.x; }", &["1:97: warning: initializer partially overrides prior initialization of this subobject [-Winitializer-overrides]"]),
        case("int a[2] = {[2] = 1};", &["1:14: error: array designator index (2) exceeds array bounds (2)"]),
        case("int a[2] = {[-1] = 1};", &["1:14: error: array designator value '-1' is negative"]),
        case("int n; int a[2] = {[n] = 1};", &["1:21: error: expression is not an integer constant expression"]),
        case("int a[] = {[1 << 20] = 1};", &[]),
        case("struct p { int x; } v = {.y = 1};", &["1:27: error: field designator 'y' does not refer to any field in type 'struct p'"]),
        case("struct p { int x; } v = {[0] = 1};", &["1:26: error: array designator cannot initialize non-array type 'struct p'"]),
        case("int a[2] = {.x = 1};", &["1:13: error: field designator cannot initialize a non-struct, non-union type 'int[2]'"]),
        case("int x = {.x = 1};", &["1:5: error: initialization of non-aggregate type 'int' with a designated initializer list"]),
        case("struct p { int x; } v = {.x.y = 1};", &["1:28: error: field designator cannot initialize a non-struct, non-union type 'int'"]),
        case("union u { char c; int i; } v = {.i = 7, 8};", &["1:41: warning: excess elements in union initializer [-Wexcess-initializers]"]),
        case("int m[2][2] = {1, 2, 3, 4, 5};", &["1:28: warning: excess elements in array initializer [-Wexcess-initializers]"]),
        case("int x = {1, 2};", &["1:13: warning: excess elements in scalar initializer [-Wexcess-initializers]"]),
        case("int x = {{1}};", &["1:10: warning: too many braces around scalar initializer [-Wmany-braces-around-scalar-init]"]),
        case("int f() { int a[]; return 0; }", &["1:15: error: definition of variable with array type needs an explicit size or an initializer"]),
        // sizeof_is_a_constant
        case("int f() { return sizeof(void); }", &[]),
        // type_errors
        case("void v(void); int t(int *p) { return p + p; }", &["1:40: error: invalid operands to binary expression ('int *' and 'int *')"]),
        case("void v(void); int t(int *p) { return p * 2; }", &["1:40: error: invalid operands to binary expression ('int *' and 'int')"]),
        case("void v(void); int t(int *p) { return 2 - p; }", &["1:40: error: invalid operands to binary expression ('int' and 'int *')"]),
        case("void v(void); int t(int *p) { return p % 2; }", &["1:40: error: invalid operands to binary expression ('int *' and 'int')"]),
        case("void v(void); int t(void *p) { return p + 1; }", &["1:39: error: incompatible pointer to integer conversion returning 'void *' from a function with result type 'int' [-Wint-conversion]"]),
        case("void v(void); int t(void *p) { return *p; }", &["1:39: warning: ISO C does not allow indirection on operand of type 'void *' [-Wvoid-ptr-dereference]", "1:39: error: returning 'void' from a function with incompatible result type 'int'"]),
        case("void v(void); int t(int *p, char *q) { return p - q; }", &["1:49: error: 'int *' and 'char *' are not pointers to compatible types"]),
        case("void v(void); int t(int *p, char *q) { return p == q; }", &["1:49: warning: comparison of distinct pointer types ('int *' and 'char *') [-Wcompare-distinct-pointer-types]"]),
        case("void v(void); int t(int *p) { return p < 0; }", &[]),
        case("void v(void); int t(int *p) { return p == 1; }", &["1:40: warning: comparison between pointer and integer ('int *' and 'int') [-Wpointer-integer-compare]"]),
        case("void v(void); int t(int x) { return *x; }", &["1:37: error: indirection requires pointer operand ('int' invalid)"]),
        case("void v(void); int t(int x) { return x[1]; }", &["1:38: error: subscripted value is not an array, pointer, or vector"]),
        case("void v(void); int t(int x) { return &1; }", &["1:37: error: cannot take the address of an rvalue of type 'int'"]),
        case("void v(void); int t(int x) { return &(x + 1); }", &["1:37: error: cannot take the address of an rvalue of type 'int'"]),
        case("void v(void); int t(int *p) { return -p; }", &["1:38: error: invalid argument type 'int *' to unary expression"]),
        case("void v(void); int t(int *p) { return ~p; }", &["1:38: error: invalid argument type 'int *' to unary expression"]),
        case("void v(void); int t(int x) { return v() + 1; }", &["1:41: error: invalid operands to binary expression ('void' and 'int')"]),
        case("void v(void); int t(int x) { return !v(); }", &["1:37: error: invalid argument type 'void' to unary expression"]),
        case("void v(void); int t(int x) { return v() && 1; }", &["1:41: error: invalid operands to binary expression ('void' and 'int')"]),
        case("void v(void); int t(int x) { return x ? v() : 1; }", &["1:37: error: returning 'void' from a function with incompatible result type 'int'"]),
        case("void v(void); int t(int *p, char *q) { return 1 ? p : q; }", &["1:49: warning: pointer type mismatch ('int *' and 'char *') [-Wpointer-type-mismatch]", "1:47: error: incompatible pointer to integer conversion returning 'void *' from a function with result type 'int' [-Wint-conversion]"]),
        case("void v(void); int t(int *p) { return (char)p + (int[2])p; }", &["1:38: warning: cast to smaller integer type 'char' from 'int *' [-Wpointer-to-int-cast]", "1:48: error: used type 'int[2]' where arithmetic or pointer type is required"]),
        case("void v(void); int t(int x) { return (int)v(); }", &["1:42: error: operand of type 'void' where arithmetic or pointer type is required"]),
        case("void v(void); int t(void *p) { return p++, 0; }", &[]),
        // conversion_errors
        case("int g; char *s; int f(int *p) { p = s; return 0; }", &["1:35: error: incompatible pointer types assigning to 'int *' from 'char *' [-Wincompatible-pointer-types]"]),
        case("int g; char *s; int f(int *p) { p = 1; return 0; }", &["1:35: error: incompatible integer to pointer conversion assigning to 'int *' from 'int' [-Wint-conversion]"]),
        case("int g; char *s; int f(int *p) { g = p; return 0; }", &["1:35: error: incompatible pointer to integer conversion assigning to 'int' from 'int *'; dereference with * [-Wint-conversion]"]),
        case("int g; char *s; int f(int *p) { int *q = s; return 0; }", &["1:38: error: incompatible pointer types initializing 'int *' with an expression of type 'char *' [-Wincompatible-pointer-types]"]),
        case("int g; char *s; int f(int *p) { s = 1 ? (void *)0 : p; return 0; }", &["1:35: error: incompatible pointer types assigning to 'char *' from 'int *' [-Wincompatible-pointer-types]"]),
        case("int g; char *s; int f(int *p) { return f(s); }", &["1:42: error: incompatible pointer types passing 'char *' to parameter of type 'int *' [-Wincompatible-pointer-types]", "1:28: note: passing argument to parameter 'p' here"]),
        case("int g; char *s; int f(int *p) { return p; }", &["1:40: error: incompatible pointer to integer conversion returning 'int *' from a function with result type 'int'; dereference with * [-Wint-conversion]"]),
        case("int g; char *s; int f(int *p) { int a[2]; int b[2]; a = b; return 0; }", &["1:55: error: array type 'int[2]' is not assignable"]),
        case("int g; char *s; int f(int *p) { g + 1 = 2; return 0; }", &["1:39: error: expression is not assignable"]),
        case("int g; char *s; int f(int *p) { g += p; return 0; }", &["1:35: error: incompatible pointer to integer conversion assigning to 'int' from 'int *'; dereference with * [-Wint-conversion]"]),
        case("int g; char *s; int f(int *p) { p -= p; return 0; }", &["1:35: error: incompatible integer to pointer conversion assigning to 'int *' from '__ptrdiff_t' (aka 'int'); take the address with & [-Wint-conversion]"]).rv32(),
        case("int g; char *s; int f(int *p) { p -= p; return 0; }", &["1:35: error: incompatible integer to pointer conversion assigning to 'int *' from '__ptrdiff_t' (aka 'long') [-Wint-conversion]"]).rv64(),
        case("int g; char *s; int f(int *p) { p *= 2; return 0; }", &["1:35: error: invalid operands to binary expression ('int *' and 'int')"]),
        case("int g; char *s; int f(int *p) { (g + 1)++; return 0; }", &["1:40: error: expression is not assignable"]),
        case("int g; char *s; int f(int *p) { \"ab\" = 0; return 0; }", &["1:38: error: array type 'char[3]' is not assignable"]),
        case("int g; char *s; int f(int *p) { void (*fp)(void) = 0; void *v = 0; return fp == v; }", &[]),
        case("int g; char *s; int f(int *p) { void (*fp)(void) = 0; return fp < (void *)0; }", &["1:65: warning: comparison of distinct pointer types ('void (*)(void)' and 'void *') [-Wcompare-distinct-pointer-types]"]),
        // returns
        case("void f() { return 1; }", &["1:12: error: void function 'f' should not return a value [-Wreturn-mismatch]"]),
        case("int f() { return; }", &["1:11: error: non-void function 'f' should return a value [-Wreturn-mismatch]"]),
        case("char *f() { return 1; }", &["1:20: error: incompatible integer to pointer conversion returning 'int' from a function with result type 'char *' [-Wint-conversion]"]),
        case("void f(void x);", &["1:13: error: argument may not have 'void' type"]),
        case("typedef void V; void f(const V);", &["1:23: error: 'void' as parameter must not have type qualifiers"]),
        case("int f(int a[const 3]) { a = 0; return 0; }", &["1:27: error: cannot assign to variable 'a' with const-qualified type 'int *const'", "1:11: note: variable 'a' declared const here"]),
        // conditions_must_be_scalar
        case("void v(); int f() { if (v()) return 1; return 0; }", &["1:21: error: statement requires expression of scalar type ('void' invalid)"]),
        case("void v(); int f() { while (v()) ; return 0; }", &["1:21: error: statement requires expression of scalar type ('void' invalid)", "1:33: warning: while loop has empty body [-Wempty-body]", "1:33: note: put the semicolon on a separate line to silence this warning"]),
        // undeclared_identifiers
        case("int f() { return y; }", &["1:18: error: use of undeclared identifier 'y'"]),
        case("int f() { y = 1; return 0; }", &["1:11: error: use of undeclared identifier 'y'"]),
        case("int f() { { int t = 1; } return t; }", &["1:33: error: use of undeclared identifier 't'"]),
        case("int f() { return 0; } int g() { return f; }", &["1:40: error: incompatible pointer to integer conversion returning 'int ()' from a function with result type 'int' [-Wint-conversion]"]),
        // calls
        case("int f() { return g(); }", &["1:18: error: call to undeclared function 'g'; ISO C99 and later do not support implicit function declarations [-Wimplicit-function-declaration]"]),
        case("int f() { return g(); } int g() { return 1; }", &["1:18: error: call to undeclared function 'g'; ISO C99 and later do not support implicit function declarations [-Wimplicit-function-declaration]"]),
        case("int f(int x) { return x(1); }", &["1:24: error: called object type 'int' is not a function or function pointer"]),
        case("int x; int f() { return x(1); }", &["1:26: error: called object type 'int' is not a function or function pointer"]),
        // wrong_argument_count
        case("int add(int a, int b) { return a + b; }\nint f() { return add(1, 2, 3); }", &["2:28: error: too many arguments to function call, expected 2, have 3", "1:5: note: 'add' declared here"]),
        case("int add(int a, int b) { return a + b; }\nint f() { return add(1); }", &["2:23: error: too few arguments to function call, expected 2, have 1", "1:5: note: 'add' declared here"]),
        case("int z(void);\nint f() { return z(0); }", &["2:20: error: too many arguments to function call, expected 0, have 1", "1:5: note: 'z' declared here"]),
        // redefinitions
        case("int f() { int a; int a; return 0; }", &["1:22: error: redefinition of 'a'", "1:15: note: previous definition is here"]),
        case("int f(int a, int a) { return a; }", &["1:18: error: redefinition of parameter 'a'", "1:11: note: previous declaration is here", "1:18: warning: omitting the parameter name in a function definition is a C23 extension [-Wc23-extensions]"]),
        case("int f(int a) { int a = 1; return a; }", &["1:20: error: redefinition of 'a'", "1:11: note: previous definition is here"]),
        case("int f() { return 0; }\nint f() { return 1; }", &["2:5: error: redefinition of 'f'", "1:5: note: previous definition is here"]),
        case("int f();\nint f() { return 0; }\nint f() { return 1; }", &["3:5: error: redefinition of 'f'", "2:5: note: previous definition is here"]),
        case("int f(int a);\nint f(int a, int b) { return a; }", &["2:5: error: conflicting types for 'f'", "1:5: note: previous declaration is here"]),
        case("int f(int a);\nchar f(int a);", &["2:6: error: conflicting types for 'f'", "1:5: note: previous declaration is here"]),
        case("int f(int *a);\nint f(char *a);", &["2:5: error: conflicting types for 'f'", "1:5: note: previous declaration is here"]),
        case("int f();\nstatic int f();", &["2:12: error: static declaration of 'f' follows non-static declaration", "1:5: note: previous declaration is here"]),
        // break_and_continue_need_a_loop
        case("int f() { break; }", &["1:11: error: 'break' statement not in loop or switch statement"]),
        case("int f() { if (1) { continue; } return 0; }", &["1:20: error: 'continue' statement not in loop statement"]),
        case("int g() { while (1) {} break; }", &["1:24: error: 'break' statement not in loop or switch statement"]),
        // assignment_target_must_be_assignable
        case("int f(int a) { 1 = a; return 0; }", &["1:18: error: expression is not assignable"]),
        case("int f(int a) { (a + 1) = 2; return 0; }", &["1:24: error: expression is not assignable"]),
        case("int f(int a) { f(a) = 2; return 0; }", &["1:21: error: expression is not assignable"]),
        case("int f(int a) { ++1; return 0; }", &["1:16: error: expression is not assignable"]),
        // errors_do_not_cascade
        case("int f() { return *(x + 1) + y[2] * -z; }", &["1:20: error: use of undeclared identifier 'x'", "1:29: error: use of undeclared identifier 'y'", "1:37: error: use of undeclared identifier 'z'"]),
        case("int m[2] = {1, 2, 3}; char s[] = 5; int f() { return m[0] + s[0]; }", &["1:19: warning: excess elements in array initializer [-Wexcess-initializers]", "1:28: error: array initializer must be an initializer list or string literal"]),
        // reports_every_error_in_order
        case("int f() {\n  x = 1;\n  break;\n  return g();\n}\nint h() { return y; }", &["2:3: error: use of undeclared identifier 'x'", "3:3: error: 'break' statement not in loop or switch statement", "4:10: error: call to undeclared function 'g'; ISO C99 and later do not support implicit function declarations [-Wimplicit-function-declaration]", "6:18: error: use of undeclared identifier 'y'"]),
        // members
        case("struct p { int x; char c; }; int f(struct p a) { return a.z; }", &["1:59: error: no member named 'z' in 'struct p'"]),
        case("int f(int a) { return a.x; }", &["1:24: error: member reference base type 'int' is not a structure or union"]),
        case("struct p { int x; char c; }; int f(struct p a) { return a->x; }", &["1:58: error: member reference type 'struct p' is not a pointer; did you mean to use '.'?"]),
        case("struct p; int f(struct p *a) { return a->x; }", &["1:40: error: incomplete definition of type 'struct p'", "1:8: note: forward declaration of 'struct p'"]),
        case("struct p { int x; char c; }; int f(const struct p *a) { a->x = 1; return 0; }", &["1:62: error: cannot assign to variable 'a' with const-qualified type 'const struct p *'", "1:52: note: variable 'a' declared const here"]),
        // struct_values
        case("struct p { int x; }; struct q { int x; }; int f(struct p a, struct q b) { a = b; return 0; }", &["1:77: error: assigning to 'struct p' from incompatible type 'struct q'"]),
        case("struct p { int x; }; int f(struct p a) { return a; }", &["1:49: error: returning 'struct p' from a function with incompatible result type 'int'"]),
        case("struct p { int x; }; int f(struct p a) { if (a) return 1; return 0; }", &["1:42: error: statement requires expression of scalar type ('struct p' invalid)"]),
        case("struct p { int x; }; int f(struct p a) { return a + 1; }", &["1:51: error: invalid operands to binary expression ('struct p' and 'int')"]),
        case("struct p { int x; }; int f(void) { struct p a; return a == a; }", &["1:57: warning: self-comparison always evaluates to true [-Wtautological-compare]", "1:57: error: invalid operands to binary expression ('struct p' and 'struct p')"]),
        case("struct p { const int x; int y; }; int f(struct p a, struct p b) { a.y = 1; a = b; return 0; }", &["1:78: error: cannot assign to variable 'a' with const-qualified data member 'x'", "1:22: note: data member 'x' declared const here"]),
        case("struct p; struct p g; int f() { return 0; }", &["1:20: error: tentative definition has type 'struct p' that is never completed", "1:8: note: forward declaration of 'struct p'"]),
        case("struct p; int f(struct p a) { return 0; }", &["1:26: error: variable has incomplete type 'struct p'"]),
        case("struct p; struct p f(void) { }", &["1:20: error: incomplete result type 'struct p' in function definition"]),
        case("struct p; struct p f(void); int g() { f(); return 0; }", &["1:39: error: calling 'f' with incomplete return type 'struct p'", "1:20: note: 'f' declared here", "1:8: note: forward declaration of 'struct p'"]),
        // compound_literals
        case("int f() { return (void){0}; }", &["1:18: error: variable has incomplete type 'void'"]),
        // file_scope_compound_literals_are_static_objects
        case("int n; int *p = (int[]){n};", &["1:25: error: initializer element is not a compile-time constant"]),
        case("int *p = (void){0};", &["1:10: error: variable has incomplete type 'void'"]),
        // variadic_calls
        case("int f(int n, ...); int g() { return f(); }", &["1:39: error: too few arguments to function call, at least argument 'n' must be specified", "1:5: note: 'f' declared here"]),
        case("void v(void); int f(int n, ...); int g() { return f(1, v()); }", &["1:56: error: argument type 'void' is incomplete"]),
        // va_builtins
        case("int f(int n) { __builtin_va_list ap; __builtin_va_start(ap, n); return 0; }", &["1:38: error: 'va_start' used in function with fixed args"]),
        case("int f(int n, int m, ...) { __builtin_va_list ap; __builtin_va_start(ap, n); return 0; }", &["1:73: warning: second argument to 'va_start' is not the last non-variadic parameter [-Wvarargs]"]),
        case("int f(int n, ...) { int ap; __builtin_va_start(ap, n); return 0; }", &["1:48: error: non-const lvalue reference to type '__builtin_va_list' cannot bind to a value of unrelated type 'int'"]),
        case("int f(int n, ...) { __builtin_va_list ap; return __builtin_va_arg(ap, char); }", &["1:71: warning: second argument to 'va_arg' is of promotable type 'char'; this va_arg has undefined behavior because arguments will be promoted to 'int' [-Wvarargs]"]),
        case("int f(int n, ...) { __builtin_va_list ap; return __builtin_va_arg(ap, int[2])[0]; }", &["1:71: warning: second argument to 'va_arg' is of array type 'int[2]'; this va_arg has undefined behavior because arguments will never be compatible with array type [-Wvarargs]"]),
        // qualifiers
        case("const int k = 1; int f() { k = 2; return 0; }", &["1:30: error: cannot assign to variable 'k' with const-qualified type 'const int'", "1:11: note: variable 'k' declared const here"]),
        case("int f(const int *p) { *p = 1; return 0; }", &["1:26: error: read-only variable is not assignable"]),
        case("int f(const int a[2]) { a[0]++; return 0; }", &["1:29: error: read-only variable is not assignable"]),
        case("int f(const char *s) { char *t = s; return 0; }", &["1:30: warning: initializing 'char *' with an expression of type 'const char *' discards qualifiers [-Wincompatible-pointer-types-discards-qualifiers]"]),
        case("int g(int *p); int f(volatile int *v) { return g(v); }", &["1:50: warning: passing 'volatile int *' to parameter of type 'int *' discards qualifiers [-Wincompatible-pointer-types-discards-qualifiers]", "1:12: note: passing argument to parameter 'p' here"]),
        // function_pointers
        case("void g(void); int f() { void *p = g; return 0; }", &[]),
        case("void *v; int f() { int (*l)(void) = v; return 0; }", &[]),
        case("int f() { int (*l)(void) = (void *)1; return 0; }", &[]),
        case("int g(int); int f() { int (*q)(char) = g; return 0; }", &["1:29: error: incompatible function pointer types initializing 'int (*)(char)' with an expression of type 'int (int)' [-Wincompatible-function-pointer-types]"]),
        case("int f(int x) { int (*p)(int) = 0; return p(1, 2); }", &["1:47: error: too many arguments to function call, expected 1, have 2"]),
        case("int f(void) { return 0; } int g() { return sizeof f; }", &[]),
        case("int f(void) { return 0; } int g() { f = 0; return 0; }", &["1:39: error: non-object type 'int (void)' is not assignable"]),
        // switch_and_case
        case("int f(int x) { switch (x) { case 1: case 1: break; } return 0; }", &["1:42: error: duplicate case value '1'", "1:34: note: previous case defined here"]),
        case("int f(unsigned x) { switch (x) { case -1: case 0xffffffff: break; } return 0; }", &["1:48: error: duplicate case value '4294967295'", "1:39: note: previous case defined here"]),
        case("int f(int x) { switch (x) { default: default: break; } return 0; }", &["1:38: error: multiple default labels in one switch", "1:29: note: previous case defined here"]),
        case("int f(int x) { case 1: return 0; }", &["1:16: error: 'case' statement not in switch statement"]),
        case("int f(int x) { default: return 0; }", &["1:16: error: 'default' statement not in switch statement"]),
        case("int f(int x) { switch (x) { case x: break; } return 0; }", &["1:34: error: expression is not an integer constant expression"]),
        case("int f(int *x) { switch (x) { case 1: break; } return 0; }", &["1:17: error: statement requires expression of integer type ('int *' invalid)"]),
        case("int f(int x) { switch (x) { case 1: continue; } return 0; }", &["1:37: error: 'continue' statement not in loop statement"]),
        // labels_and_goto
        case("int f() { goto nowhere; return 0; }", &["1:16: error: use of undeclared label 'nowhere'"]),
        case("int f() { a: a: return 0; }", &["1:11: error: redefinition of label 'a'", "1:14: note: previous definition is here"]),
        // struct_and_union_initializers
        case("struct s { int a; char b; }; struct s g = {1, 2, 3};", &["1:50: warning: excess elements in struct initializer [-Wexcess-initializers]"]),
        case("union u { int a; char b; }; union u g = {1, 2};", &["1:45: warning: excess elements in union initializer [-Wexcess-initializers]"]),
        case("struct s { int a; }; struct s g = {{1, 2}};", &["1:40: warning: excess elements in scalar initializer [-Wexcess-initializers]"]),
        case("struct p { int x; }; static struct p g = {1}; struct p h = g;", &["1:60: error: initializer element is not a compile-time constant"]),
    ]
}
