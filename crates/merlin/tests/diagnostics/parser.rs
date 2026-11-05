//! The diagnostic cases of `parser` (generated; curated by hand).

use super::{case, Case};

#[rustfmt::skip]
pub fn cases() -> Vec<Case> {
    vec![
        // typedefs
        // The declarators of a `typedef`, each binding a name to its type.
        case("typedef int T; int f() { return T; }", &["1:33: error: unexpected type name 'T': expected expression"]),
        case("typedef int T; typedef char T;", &["1:29: error: typedef redefinition with different types ('char' vs 'int')"]),
        case("typedef int x = 1;", &["1:13: error: illegal initializer (only variables can be initialized)"]),
        case("int T; typedef int T;", &["1:20: error: redefinition of 'T' as different kind of symbol"]),
        // array_parameter_qualifiers
        case("int a[const 3];", &["1:6: error: type qualifier used in array declarator outside of function prototype"]),
        case("int f(int a[3][const 2]);", &["1:15: error: type qualifier used in non-outermost array type derivation"]),
        case("int f(int (*a)[const 2]);", &["1:15: error: type qualifier used in non-outermost array type derivation"]),
        case("int f() { return sizeof(int [const 2]); }", &["1:29: error: type qualifier used in array declarator outside of function prototype"]),
        case("int f(int a[static]);", &["1:13: error: 'static' may not be used without an array size"]),
        // type_specifiers
        case("signed unsigned x;", &["1:8: error: cannot combine with previous 'signed' declaration specifier"]),
        case("char int x;", &["1:6: error: cannot combine with previous 'char' declaration specifier"]),
        case("short char x;", &["1:1: error: 'short char' is invalid"]),
        case("short long x;", &["1:7: error: cannot combine with previous 'short' declaration specifier"]),
        case("void int x;", &["1:6: error: cannot combine with previous 'void' declaration specifier", "1:10: error: variable has incomplete type 'void'"]),
        case("unsigned void x;", &["1:1: error: 'void' cannot be signed or unsigned", "1:15: error: variable has incomplete type 'void'"]),
        case("int int x;", &["1:5: error: cannot combine with previous 'int' declaration specifier"]),
        case("long long long x;", &["1:11: error: cannot combine with previous 'long long' declaration specifier"]),
        case("short long long x;", &["1:7: error: cannot combine with previous 'short' declaration specifier", "1:12: error: cannot combine with previous 'short' declaration specifier"]),
        case("static x;", &["1:8: error: type specifier missing, defaults to 'int'; ISO C99 and later do not support implicit int [-Wimplicit-int]"]),
        case("static extern int x;", &["1:8: error: cannot combine with previous 'static' declaration specifier"]),
        case("_Bool int x;", &["1:7: error: cannot combine with previous '_Bool' declaration specifier"]),
        case("unsigned _Bool x;", &["1:1: error: '_Bool' cannot be signed or unsigned"]),
        case("int;", &["1:1: warning: declaration does not declare anything [-Wmissing-declarations]"]),
        case("const x;", &["1:7: error: type specifier missing, defaults to 'int'; ISO C99 and later do not support implicit int [-Wimplicit-int]"]),
        // structs_and_unions
        case("struct s { int a; int a; };", &["1:23: error: duplicate member 'a'"]),
        case("struct s { int a; }; struct s { int b; };", &["1:29: error: redefinition of 's'"]),
        case("struct s { int a; }; union s x;", &["1:22: error: use of 's' with tag type that does not match previous declaration"]),
        case("struct s { };", &[]),
        case("struct s { int a : 3; };", &[]),
        case("struct s { struct t m; };", &["1:21: error: field has incomplete type 'struct t'"]),
        case("struct s { int f(void); };", &["1:16: error: field 'f' declared as a function"]),
        case("struct s { int n; int x[]; };", &[]),
        case("struct s { int; };", &["1:12: warning: declaration does not declare anything [-Wmissing-declarations]"]),
        case("struct { int a; };", &["1:1: warning: declaration does not declare anything [-Wmissing-declarations]"]),
        case("struct s { static int a; };", &["1:12: error: type name does not allow storage class to be specified"]),
        case("struct;", &["1:1: error: declaration of anonymous struct must be a definition", "1:1: warning: declaration does not declare anything [-Wmissing-declarations]"]),
        case("struct s { char c[0x7fffffff]; int i; };", &[]),
        // enums
        case("enum e x;", &["1:8: error: tentative definition has type 'enum e' that is never completed"]),
        case("enum { A, A };", &["1:11: error: redefinition of enumerator 'A'"]),
        case("enum { A = 2147483647, B };", &[]),
        case("enum { };", &["1:8: error: use of empty enum"]),
        case("int x; enum { x };", &["1:15: error: redefinition of 'x'"]),
        case("int y; enum { A = y };", &["1:19: error: expression is not an integer constant expression"]),
        case("enum e { A }; enum e { B };", &["1:20: error: redefinition of 'e'"]),
        case("enum { A B };", &["1:9: error: missing ',' between enumerators"]),
        // switch_case_and_goto
        case("int f() { case 1 x; }", &["1:17: error: expected ':' after 'case'", "1:11: error: 'case' statement not in switch statement", "1:18: error: use of undeclared identifier 'x'"]),
        case("int f() { goto 3; }", &["1:16: error: expected identifier", "1:16: warning: expression result unused [-Wunused-value]"]),
        case("int f() { default x; }", &["1:18: error: expected ':' after 'default'", "1:19: error: use of undeclared identifier 'x'", "1:11: error: 'default' statement not in switch statement"]),
        // members_and_calls
        case("int f() { s.(1); }", &["1:11: error: use of undeclared identifier 's'", "1:13: error: expected identifier"]),
        // qualifiers_and_function_specifiers
        case("inline int x;", &["1:1: error: 'inline' can only appear on functions"]),
        case("int f(inline int x);", &["1:7: error: 'inline' can only appear on functions"]),
        // the_operand_not_chosen_counts_by_its_type
        // The operand a constant `?:` does not choose is not evaluated: in an
        // array size, an enumerator or a `_Static_assert` it need not be
        // constant, and only its type counts, as sema gives it (clang 23 takes
        // them all, the array size as a GNU folding).
        case("int g; int a[0 ? 2 : g];", &["1:12: error: variable length array declaration not allowed at file scope"]),
        case("unsigned u(void); int f(void) { int a[1 ? 2 : u()]; return a[0]; }", &[]),
        case("unsigned u(void); int f(void) { int a[1 ? 2 : u()] = {1}, b[1 ? 3 : u()]; return 0; }", &["1:37: warning: variable length array folded to constant array as an extension [-Wgnu-folding-constant]"]),
        case("unsigned u(void); int f(void) { typedef int T[1 ? 2 : u()]; return 0; }", &[]),
        case("unsigned u(void); int f(void) { int (*p)[1 ? 2 : u()] = 0; return 0; }", &[]),
        case("unsigned u(void); int f(int a[1 ? 2 : u()]) { return a[0]; }", &[]),
        case("unsigned u(void); int f(void) { return sizeof(int[1 ? 2 : u()]); }", &[]),
        case("unsigned u(void); int f(void) { static int (*p)[1 ? 2 : u()]; return 0; }", &[]),
        case("unsigned u(void); int f(void) { static int (*q[2])[1 ? 2 : u()]; return 0; }", &[]),
        case("unsigned u(void); int f(void) { int (*b[2])[1 ? 3 : u()] = {0}; return 0; }", &[]),
        case("unsigned u(void); int f(void) { int *q = (int[][1 ? 2 : u()]){{1}}[0]; return *q; }", &["1:49: error: variable-sized object may not be initialized"]),
        case("unsigned u(void); int f(void) { void *p = (int (*)[1 ? 2 : u()])0; return p != 0; }", &[]),
        case("unsigned u(void);\nint (*a[2])[1 ? 2 : u()];", &["2:7: error: variably modified type declaration not allowed at file scope"]),
        case("unsigned u(void);\nint b[][1 ? 2 : u()] = {{1}};", &["2:5: error: variably modified type declaration not allowed at file scope"]),
        case("unsigned u(void);\ntypedef int (*T[2])[1 ? 2 : u()];", &["2:15: error: variably modified type declaration not allowed at file scope"]),
        case("unsigned u(void);\nint (*(*g)(void))[1 ? 2 : u()];", &["2:9: error: variably modified type declaration not allowed at file scope"]),
        case("unsigned u(void);\nstruct S { int (*p[2])[1 ? 2 : u()]; } s;", &["2:18: error: fields must have a constant size: 'variable length array in structure' extension will never be supported"]),
        case("unsigned u(void);\nint (*fr(void))[1 ? 2 : u()];", &["2:7: error: function declaration cannot have variably modified type"]),
        case("unsigned u(void);\nstatic int (*fr(void))[1 ? 2 : u()] { return 0; }", &["2:14: error: function declaration cannot have variably modified type"]),
        // sizeof_an_expression_sizes_an_array
        case("char a[sizeof nothing];", &["1:15: error: use of undeclared identifier 'nothing'"]),
        case("int t[] = {[3] = 1}; char a[sizeof t];", &[]),
        // declarators
        case("int a[0];", &[]),
        case("int a[-1];", &["1:7: error: 'a' declared as an array with a negative size"]),
        case("int n; int a[n];", &["1:12: error: variable length array declaration not allowed at file scope"]),
        case("int a[2][];", &["1:6: error: array has incomplete element type 'int[]'"]),
        case("int a[0x40000000][2];", &["1:7: error: array is too large (1'073'741'824 elements)"]).rv32(),
        case("int a[0x40000000][2];", &[]).rv64(),
        case("void a[2];", &["1:7: error: array has incomplete element type 'void'"]),
        case("int f()[2];", &["1:6: error: function cannot return array type 'int[2]'"]),
        case("int f()(int);", &["1:6: error: function cannot return function type 'int (int)'"]),
        case("int a[2](void);", &["1:6: error: 'a' declared as array of functions of type 'int (void)'"]),
        case("int a[1] = {[0] 1};", &["1:17: warning: use of GNU 'missing =' extension in designator [-Wgnu-designator]"]),
        case("int a[1] = {.};", &["1:14: error: expected a field designator, such as '.field = 4'"]),
        case("int x, f(int);", &[]),
        case("int a[2] = {};", &[]),
        // declarations
        case("int f() { extern int x; }", &["1:25: warning: non-void function does not return a value [-Wreturn-type]"]),
        // loops
        case("int f() { do x; until (1); }", &["1:14: error: use of undeclared identifier 'x'", "1:17: error: call to undeclared function 'until'; ISO C99 and later do not support implicit function declarations [-Wimplicit-function-declaration]"]),
        case("int f() { do ; while (1) }", &["1:25: error: expected ';' after do/while statement"]),
        case("int f() { for (static int i = 0;;) ; }", &[]),
        // casts_and_sizeof
        case("int f() { return (int x)1; }", &["1:23: error: expected ')'"]),
        case("int f() { return (static int)1; }", &["1:19: error: expected expression"]),
        // missing_semicolon
        case("int f() { x = 1 }", &["1:11: error: use of undeclared identifier 'x'"]),
        case("int f() {\n  return 1\n}", &["2:11: error: expected ';' after return statement"]),
        case("int f() { int x = 1 int y; }", &["1:20: error: expected ';' at end of declaration"]),
        case("int x = 1 int y;", &["1:10: error: expected ';' after top level declarator"]),
        case("int f() { while (1) break }", &["1:26: error: expected ';' after break statement"]),
        case("int f() { while (1) continue }", &["1:29: error: expected ';' after continue statement"]),
        case("int f() { for (i = 0 i < 3; ) ; }", &["1:16: error: use of undeclared identifier 'i'", "1:29: error: expected ';' in 'for' statement specifier"]),
        case("int f() { for (;1) ; }", &["1:18: error: expected ';' in 'for' statement specifier"]),
        // syntax_errors
        case("int f() { return (1 + 2; }", &["1:24: error: expected ')'"]),
        case("int f() { return 1 + ; }", &["1:22: error: expected expression"]),
        case("int f() { return 1;", &["1:20: error: expected '}'"]),
        case("int f() { if x) return 1; }", &["1:14: error: expected '(' after 'if'"]),
        case("int f() { f(1 2); }", &["1:15: error: expected ')'"]),
        case("int f() { f(1 0x7f); }", &["1:15: error: expected ')'"]),
        case("int f(int a b) {}", &["1:13: error: expected ')'"]),
        case("int f(x) {}", &["1:7: error: parameter 'x' was not declared, defaults to 'int'; ISO C99 and later do not support implicit int [-Wimplicit-int]", "1:5: warning: a function definition without a prototype is deprecated in all versions of C and is not supported in C23 [-Wdeprecated-non-prototype]"]),
        case("int f(int) { return 0; }", &["1:10: warning: omitting the parameter name in a function definition is a C23 extension [-Wc23-extensions]"]),
        case("int f() return 1;", &["1:9: error: expected function body after function declarator"]),
        case("f() {}", &["1:1: error: type specifier missing, defaults to 'int'; ISO C99 and later do not support implicit int [-Wimplicit-int]"]),
        case("int 3() {}", &["1:5: error: expected identifier or '('"]),
        case("int f() { int 1; }", &["1:15: error: expected identifier or '('"]),
        case("int f() { else x; }", &["1:11: error: expected expression"]),
        case("int f() { } }", &["1:11: warning: non-void function does not return a value [-Wreturn-type]", "1:13: error: extraneous closing brace ('}')"]),
        case("int f() { a ? b; }", &["1:11: error: use of undeclared identifier 'a'", "1:15: error: use of undeclared identifier 'b'", "1:16: error: expected ':'", "1:16: error: expected expression"]),
        case("int f() { a[1; }", &["1:11: error: use of undeclared identifier 'a'", "1:14: error: expected ']'"]),
        case("int a[2] = {1 2};", &["1:15: error: expected '}'"]),
        // unsupported_constructs
        case("int f() { if (1) int x = 1; }", &["1:18: error: expected expression"]),
        case("int f() { int g(int a); }", &["1:25: warning: non-void function does not return a value [-Wreturn-type]"]),
        case("int f(...);", &["1:7: error: ISO C requires a named parameter before '...'"]),
        case("int f(int a, ..., int b);", &["1:17: error: expected ')'"]),
        // static_assertions
        case("_Static_assert(1 + 1 == 3, \"math\");", &["1:16: error: static assertion failed due to requirement '1 + 1 == 3': math"]),
        case("int x; _Static_assert(x, \"x\");", &["1:23: error: static assertion expression is not an integral constant expression"]),
        case("_Static_assert(1);", &["1:17: warning: '_Static_assert' with no message is a C23 extension [-Wc23-extensions]"]),
        case("struct s { char c; int n; }; _Static_assert((unsigned long)&((struct s *)0)->n == 0, \"n\");", &["1:45: error: static assertion failed due to requirement '(unsigned long)&((struct s *)0)->n == 0': n"]),
        // variadic_functions
        case("int f() { __builtin_va_arg(ap); }", &["1:28: error: use of undeclared identifier 'ap'", "1:30: error: expected ','", "1:31: error: expected a type", "1:31: error: expected ')'"]),
    ]
}
