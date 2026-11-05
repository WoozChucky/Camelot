//! # Code generation: AST to Excalibur assembly
//!
//! Turns a checked [`Program`] into `.s` text for merlin-as. It is a *stack
//! machine* code generator, the approach of Rui Ueyama's chibicc: every
//! expression leaves its value in `a0`, and a value that must survive while
//! another expression is evaluated is pushed on the stack. Nothing is kept in
//! registers across statements and every variable lives in memory, so the
//! output is long but easy to follow, and easy to check by hand.
//!
//! The program's target ([`Program::xlen`]) is RV32, with the ILP32 ABI, or
//! RV64, with LP64. The examples below are RV32's; where RV64 differs, the
//! text says how. In short: a register (a *word* here) is 8 bytes on RV64,
//! so pushes, saved registers, argument words and register-wide loads and
//! stores are `sd`/`ld` of 8 bytes, and `int` arithmetic uses the W
//! instructions ("The 32-bit invariant" below).
//!
//! ## Contract
//!
//! * Input: a [`Program`] that passed [`crate::sema`]. Every expression has its
//!   C type, conversions are explicit cast nodes, `sizeof` is folded, every
//!   member access knows its offset, initializer lists are fully braced, and
//!   every variable says where it lives: local `#i` of its function
//!   (parameters first, then the declared locals and the hidden ones calls
//!   need) or a global symbol. Every call has the right number of arguments
//!   (those `...` takes promoted), `va_end` and `va_copy` have become plain
//!   expressions, and every compound literal has its hidden local.
//! * Output: assembly merlin-as accepts (and so does clang): functions in
//!   `.text`, initialized variables in `.data`, zero-initialized ones in
//!   `.bss`, `const` ones and string literals in `.rodata`. Every function and
//!   variable that is not `static` gets `.globl` (except a C99 inline
//!   definition, kept local); declarations without a definition emit nothing
//!   (the linker finds them). Like clang, every function and variable gets
//!   `.type` (`@function` or `@object`) and `.size`, so the symbol table says
//!   what each address belongs to. A `static` function or variable (or an
//!   inline definition) that nothing emitted uses is not emitted, as clang
//!   leaves it out ("Order" below).
//! * Semantics: arithmetic wraps at the width of its type (32 bits for `int`,
//!   and for `long` on RV32; 64 for `long` on RV64), `/` and `%` truncate
//!   toward zero (`div`/`rem`, or `divu`/`remu` when unsigned), shifts use the
//!   low 5 bits of the amount (6 for a 64-bit type), comparisons and `! &&
//!   ||` produce 0 or 1, `&&`/`||` short-circuit, an assignment evaluates to
//!   the stored value. Falling off the end of a function returns 0.
//! * Memory: every read and write the source asks for is done, once, in
//!   order, and nothing is kept in a register across statements. So
//!   `volatile` needs nothing special: the code generator never caches,
//!   removes, merges or reorders memory accesses.
//!
//! ## Order
//!
//! Functions and variables are generated in the order clang's front end
//! emits them (`CodeGenModule`), and written in the order clang's module has
//! them, so that `.text`, `.data`, `.bss` and `.rodata` hold them as clang's
//! output does at `-O0`:
//!
//! * *Emitted*: the file is read as it comes. An external function, and an
//!   external variable with an initializer, is generated where it stands. A
//!   `static` function or variable (and an inline definition) is deferred:
//!   generated once something emitted refers to it, or not at all. The
//!   variables without an initializer (tentative definitions) are generated
//!   at the end of the file. Then what was used and is still waiting, each
//!   one's own uses right after it, depth first (`EmitDeferred`).
//! * *Written*: each symbol where it was first made: a function as its
//!   generation starts, or the first time code refers to it; a variable when
//!   its initializer has been generated (a `static` local before it), or the
//!   first time code or data refers to it ([`FileGen::create`]). So in
//!
//!   ```text
//!   int f(void) { return g() + sh(1); }   int h(void) { ... }
//!   static int sh(int x) { return x + h(); }   int g(void) { ... }
//!   ```
//!
//!   `sh` is generated last, but its code comes right after `g`'s, before
//!   `h`'s: `f`, `g`, `sh`, `h`. A `static` local is data made where its
//!   declaration is generated; a compound literal of the file's scope is
//!   generated as the initializer that names it is.
//!
//! With `-g` the `.file` numbers of the code's files go by the order of
//! the output too ([`Lines::renumber`]), and the debug information by both
//! orders, as LLVM's does ([`crate::debug`]). A function call refers to the
//! function before its arguments are generated, as clang's `EmitCallee`
//! comes before `EmitCallArgs`: in `f(g())` `f` is made first.
//!
//! ## Values and types
//!
//! Every value in a register fills the register: a value of a narrower type
//! is extended to it. A `char` or `short` is widened when it is loaded
//! (`lbu`/`lb`, `lhu`/`lh`, by its signedness; plain `char` is unsigned) and
//! narrowed when it is stored (`sb`, `sh`), so integer promotion costs
//! nothing. A conversion *to* a narrow type in a register (a cast, or the
//! value of `c = 300` for a `char c`) is done in place (on RV64 the shifts
//! are by 56 and 48):
//!
//! ```text
//!   andi  a0, a0, 255         # to unsigned char (and plain char)
//!   slli  a0, a0, 24          # to signed char: keep the low byte...
//!   srai  a0, a0, 24          # ...and copy its top bit up
//!   slli  a0, a0, 16          # to unsigned short (srai for short)
//!   srli  a0, a0, 16
//! ```
//!
//! A binary operator's operands are converted to the type the operation is
//! done in (the usual arithmetic conversions), which changes bits only where
//! widths differ, and that type decides the instruction: `divu`, `remu`,
//! `sltu` and `srl` when it is unsigned (`unsigned int`, `unsigned long`, or
//! a pointer), `div`, `rem`, `slt` and `sra` otherwise.
//!
//! ### The 32-bit invariant (RV64)
//!
//! On RV64 an `int` or `unsigned int` lives in a 64-bit register *sign-
//! extended* from bit 31, whatever its signedness, as the psABI requires of
//! arguments and results and as the hand-written assembly expects (so an
//! `unsigned` 0xffffffff is -1 in its register). `lw` loads it so, and every
//! operation in a 32-bit type keeps it so: `addw`, `subw`, `mulw`, `divw`,
//! `divuw`, `remw`, `remuw`, `sllw`, `srlw`, `sraw`, `negw` and `addiw`
//! compute in 32 bits and sign-extend the result; `and`, `or`, `xor` and
//! `not` keep it as it is; `slt` and `sltu` compare sign-extended values in
//! the right order for `int` and for `unsigned int` alike. Conversions
//! change bits only across that line:
//!
//! ```text
//!   sext.w a0, a0             # long or a pointer to int or unsigned int
//!   slli  a0, a0, 32          # unsigned int to long or unsigned long (and
//!   srli  a0, a0, 32          # an unsigned index added to a pointer)
//! ```
//!
//! An `int` needs nothing to become a `long` or an index: it is its own
//! sign extension already.
//!
//! ### Register pairs (RV32)
//!
//! A `long long` is 64 bits on both widths. On RV64 it is a register like a
//! `long`. On RV32 it is a *pair*: an expression leaves its low word in `a0`
//! and its high word in `a1`, a push takes 8 bytes (the low word at the lower
//! address, as in memory), and a binary operator finds its left operand in
//! `a2:a3` and its right one in `a0:a1` (a narrower operand is extended into
//! the other word by its signedness: `srai a1, a0, 31`, or `li a1, 0`). The
//! operations are inline, as LLVM expands them on RV32 with M:
//!
//! ```text
//!   add  a0, a2, a0           # +: the low words,
//!   sltu t0, a0, a2           #    their carry,
//!   add  a1, a3, a1           #    the high words
//!   add  a1, a1, t0
//!   mul  t0, a3, a0           # *: the cross products' low words, and the
//!   mul  t1, a2, a1           #    high word of lo * lo (mulhu)
//!   add  t0, t0, t1
//!   mulhu t1, a2, a0
//!   add  a1, t0, t1
//!   mul  a0, a2, a0
//! ```
//!
//! `-` borrows the same way; `& | ^ ~` work word by word; a comparison
//! compares the high words (signed or not) and, when they are equal, the low
//! ones unsigned; a shift by `n` (its low 6 bits) moves a word across when
//! `n` reaches 32 and shifts the bits crossing between the words otherwise
//! ([`FuncGen::pair_shift`]); a condition is `or a0, a0, a1`. Division and
//! remainder call libgcc's routines (`__divdi3`, `__udivdi3`, `__moddi3`,
//! `__umoddi3`, with the operands in `a0:a1` and `a2:a3`), which the C
//! library and the kernel provide (`kernel/user/lib/libgcc.c`), as clang's
//! code calls them too. A conversion to a narrower type takes the low word;
//! one to `_Bool` both. Pairs travel as the psABI says ([`crate::abi`]): in
//! two argument words, from an even one on the stack and, when variadic, in
//! an even register; a result in `a0` and `a1`.
//!
//! A `_Bool` holds 0 or 1: converting to it is `snez a0, a0` (from a pointer
//! too), and it is loaded with `lbu`.
//!
//! An array is never loaded: an expression of array type leaves its address in
//! `a0`, which is exactly C's array-to-pointer decay. A function's name
//! leaves the function's address (`la a0, f`), and so does `*fp`.
//!
//! ## Structs and unions
//!
//! A struct (or union) is not a value that fits a register, so an expression
//! of struct type leaves the struct's *address* in `a0`, like an array: a
//! variable's slot, `*p`'s `p`, a member's address inside its parent. Then
//!
//! * `s.m` and `p->m` add the member's offset (sema worked it out) to that
//!   address, and load the member if it is a scalar;
//! * `a = b` copies the bytes from `b`'s address to `a`'s, and leaves `a`'s
//!   address (the assignment's value);
//! * a struct a call returns is kept in a hidden local of the caller (sema
//!   adds one per such call), whose address is the call's value: so
//!   `f().m` is a member access like any other.
//!
//! Copies go in units of the struct's alignment, at most a register
//! (`ld`/`sd` on RV64, `lw`/`sw`, `lhu`/`sh`, `lbu`/`sb`), so no access is
//! misaligned: unrolled for up to 16 units, a loop for more. Struct locals
//! get slots aligned to a register that are a whole number of registers
//! long, so that a struct arriving in registers can be stored with `sw`
//! (`sd`).
//!
//! ## Calling convention (RISC-V ILP32 and LP64)
//!
//! * Arguments go in `a0-a7`, left to right, and the words past `a7` on the
//!   stack, from `0(sp)` up at the call; the result comes back in `a0`.
//!   Arguments and results narrower than 32 bits travel widened by their type
//!   (sema's casts do that), and on RV64 32-bit ones sign-extended (the
//!   32-bit invariant). The arguments `...` takes travel the same way.
//! * A struct of up to two words (8 bytes on RV32, 16 on RV64) travels in one
//!   or two registers, its bytes as they lie in memory: it is loaded from its
//!   address word by word (put together from smaller aligned pieces, each
//!   zero-extended, if it is not aligned to a register). A
//!   bigger one is passed as the address of a copy the caller makes (in a
//!   hidden local), and returned through memory: the caller passes the
//!   address of its result local as a hidden first argument in `a0`, and the
//!   callee copies the result there. [`crate::abi`] has the rules.
//! * `ra` holds the return address (`call f` is `auipc ra` + `jalr ra`; a
//!   call through a pointer loads it into `t1` and does `jalr t1`).
//! * `sp`, `s0-s11` must look untouched to the caller when we return; the
//!   `a` and `t` registers may be clobbered by any call. We only ever use
//!   `s0` (saved in the prologue), the argument registers, and `t0-t4` as
//!   scratch.
//! * `sp` is 16-byte aligned at every call.
//!
//! ## Stack frame
//!
//! `s0` (the frame pointer) holds the value `sp` had on entry, so the caller's
//! frame starts at `s0` and ours lies below it:
//!
//! ```text
//!            caller's frame
//!   s0 ->  +------------------+  (16-byte aligned: sp before the call)
//!          | saved ra         |  -4(s0)
//!          | caller's s0      |  -8(s0)
//!          | local #0         |           parameters first: a0 is stored here,
//!          | local #1         |           a1 next, and so on
//!          | ...              |
//!          | (padding)        |           frame size rounded up to 16
//!   sp ->  +------------------+  (16-byte aligned) after the prologue
//!          | temporaries      |  pushed and popped while evaluating expressions
//! ```
//!
//! The caller's stack arguments, if any, are right above: word 8 of the
//! arguments at `0(s0)`, word 9 at `4(s0)`, and so on.
//!
//! On RV64 each of these is a doubleword: `ra` at `-8(s0)`, the caller's
//! `s0` at `-16(s0)` (`sd`/`ld`), the locals from `-16(s0)` down, and the
//! stack arguments at `0(s0)`, `8(s0)`, ..., as clang lays out its frames.
//!
//! Locals are placed downwards from `-8(s0)` in order, each at the next
//! offset aligned for its type (arrays at least to 4 bytes): an `int` takes the
//! 4 bytes below the previous local, a `char` 1 byte, an `int a[3]` 12 bytes.
//! With only `int`s, local `#i` is at `-(12 + 4*i)(s0)`. Sema gives every
//! declaration in the function its own local (shadowed names included), so a
//! slot never has to be reused and scopes cost nothing here.
//!
//! Only the locals of the code generated get a slot, as clang's frame at
//! `-O0` has room only for the code it emits: a local declared in an arm a
//! constant condition leaves out, or a temporary of an operand left out (the
//! result of a call in `1 ? 2 : h().a[0]`), has none. A local declared in
//! code that is generated has one, used or not. Since what is generated is
//! known only once it is, a function is generated twice: a first pass, with
//! a slot for every local, notes the ones the code declares or uses and is
//! thrown away; the second lays out those alone.
//!
//! The prologue saves `ra` and `s0` in a fixed 16 bytes, points `s0` at the top
//! of the frame and then makes room for the rest; the epilogue undoes it from
//! `s0`, so it never needs to know the frame size:
//!
//! ```text
//!   f:  addi  sp, sp, -16          .L.return.f:              # result in a0
//!       sw    ra, 12(sp)               addi  sp, s0, -16
//!       sw    s0, 8(sp)                lw    ra, 12(sp)
//!       addi  s0, sp, 16               lw    s0, 8(sp)
//!       addi  sp, sp, -(frame - 16)    addi  sp, sp, 16
//!       sw    a0, -12(s0)  # params    ret
//! ```
//!
//! A struct parameter that came in registers is stored word by word; one
//! passed by reference is copied from the caller's copy into its slot. A
//! parameter on the stack is loaded from above `s0` (through `t1`) and
//! stored into its slot like the others. A function returning a big struct
//! saves the result address (`a0`) in a hidden local.
//!
//! ## Variadic functions
//!
//! A variadic function cannot know where its `...` arguments are until it
//! reads them, so its prologue stores the argument registers its named
//! parameters leave free (`a<n>` to `a7`) at the very top of its frame, right
//! below `s0`, where the caller's stack words continue them. Every variadic
//! argument then lies in one run of words from there up, as if all had been
//! passed on the stack, and a `va_list` is a plain pointer that walks it (as
//! clang does). `ra` and `s0` go below the saved registers, and the fixed
//! part of the frame grows to a multiple of 16 that holds them all (on RV64
//! each saved register takes 8 bytes, `-8(s0)` .. `-56(s0)` for `a7` ..
//! `a1`):
//!
//! ```text
//!   int f(int n, ...)                    s0 ->  +-----------+
//!     addi  sp, sp, -48                         | a7 .. a1  |  -4(s0) .. -28(s0)
//!     sw    ra, 16(sp)    # -32(s0)             | saved ra  |  -32(s0)
//!     sw    s0, 12(sp)    # -36(s0)             | caller s0 |  -36(s0)
//!     addi  s0, sp, 48                          | n         |  -40(s0), locals...
//!     sw    a1, -28(s0)   ... sw a7, -4(s0)
//! ```
//!
//! `va_start(ap, n)` points `ap` at the first variadic word (`-28(s0)` here;
//! `4*k(s0)` when the named parameters take `8 + k` words, all registers and
//! some stack). `va_arg(ap, T)` takes the words of a `T` at `ap` and moves
//! `ap` past them: a scalar is loaded, a small struct's value is its address
//! right there, and a big struct's is the word there (the address of the
//! caller's copy). `va_end` does nothing and `va_copy` copies the pointer.
//!
//! ## Expressions
//!
//! `expr(e)` emits code that leaves `e`'s value in `a0`. For `l op r`:
//!
//! ```text
//!   <l>                    # a0 = l
//!   addi  sp, sp, -4       # push a0: <r> may need a0 (and any register);
//!   sw    a0, 0(sp)        #   on RV64 -8 and `sd`
//!   <r>                    # a0 = r
//!   lw    a1, 0(sp)        # pop: a1 = l
//!   addi  sp, sp, 4
//!   sub   a0, a1, a0       # a0 = l - r
//! ```
//!
//! Comparisons give 0 or 1 with `slt` (`l < r`), `slt` with the operands
//! swapped (`l > r`), those two followed by `xori 1` (`>=`, `<=`), and `xor`
//! followed by `seqz`/`snez` (`==`, `!=`). `&&` and `||` branch around their
//! right side and turn the result into 0 or 1 with `snez`; `?:` branches like
//! an `if`. A constant condition is decided here, as clang's code generator
//! decides it at `-O0` (see "Control flow"): `c ? a : b` is the operand `c`
//! chooses, and `1 && x` and `0 || x` are `x`, as 0 or 1.
//!
//! Pointer arithmetic counts elements: `p + i` scales `i` by the size of what
//! `p` points to (a shift for powers of two), and `p - q` divides the byte
//! difference by it.
//!
//! An integer constant expression (C99 6.6) is one constant, of any integer
//! type, at both widths: `8 * (1024ul * 1024ul)` is `li a0, 8388608`, and
//! `3LL << 40` on RV32 is `li a0, 0` and `li a1, 768` (a pair). clang does
//! the same at `-O0`, since its front end folds them whatever the type.
//! [`crate::consteval`] works the value out with the semantics above (the
//! usual arithmetic conversions, wrapping at the type's width, shift amounts
//! masked as the hardware masks them), so the constant is what the
//! instructions would have computed. A division by zero is not folded and
//! stays a `div` at run time (clang warns, then has no value for it); in a
//! larger expression only the parts around it are folded. Where an operand
//! is not constant (`x * (2 * 3)`) the constant parts are folded and the
//! rest is computed as above: nothing is reassociated (`x * 2 * 3` is two
//! multiplies), as at `-O0`.
//!
//! ## Places: where assignments store
//!
//! The target of `=`, `op=`, `++` and `--` is a *place*. A scalar local is
//! reached directly at its offset from `s0`; for anything else (a global,
//! `*p`, `a[i]`) the address is computed first and pushed, so that the value
//! can be computed with any registers, then popped into `a1` for the store:
//!
//! ```text
//!   x = 5;   (local)          a[i] = 5;
//!     li    a0, 5               <address of a[i]>
//!     sw    a0, -12(s0)         addi sp, sp, -4    # push it
//!                               sw   a0, 0(sp)
//!                               li   a0, 5
//!                               lw   a1, 0(sp)     # pop it
//!                               addi sp, sp, 4
//!                               sw   a0, 0(a1)
//! ```
//!
//! `x op= v` loads the place's value (peeking at the pushed address, which
//! stays for the store), pushes it, evaluates `v`, applies the operator,
//! converts the result to `x`'s type and stores it, so `x` is evaluated once.
//! `++x` and `x++` add the step (1, or the pointed-to size) in place; `x++`
//! then subtracts it again to give back the old value.
//!
//! Globals are reached through their address: `la a0, g` (`auipc` + `addi`,
//! PC-relative), then a load or store at `0(a0)`.
//!
//! A call evaluates its arguments left to right, pushing all but the last;
//! the last goes from `a0` to `a(n-1)` with `mv`, then the others are popped
//! into `a(n-2)` down to `a0` (a small struct's address is popped into `t1`
//! and its words loaded from there). A call through a pointer evaluates the
//! pointer first and pops it into `t1` last. If temporaries of an enclosing
//! expression are still pushed (as in `x + f(y)`), `sp` is lowered to a
//! multiple of 16 around the `call`, to keep the convention's alignment.
//!
//! When the arguments take more than `a0-a7`, every one of them is pushed;
//! then `sp` goes down far enough for the stack words (keeping it aligned),
//! each stack word is copied from its pushed argument to its place at
//! `0(sp)`, `4(sp)`, ..., and the register words are loaded from theirs. One
//! `addi` after the call frees the words and the pushed arguments together.
//!
//! ## Initializers
//!
//! A local scalar's initializer is an assignment. A local array or struct
//! with an initializer list is first cleared (word by word, or with a loop
//! when it is big), then each element or member given is stored; a struct
//! initialized by another struct is copied. A local without an initializer is
//! left as it is (C: indeterminate). Sema has resolved designators already:
//! a subobject one skips has an empty list, which stores nothing, and a
//! union's list ends with the member it holds.
//!
//! A compound literal `(T){ ... }` is a hidden local initialized the same
//! way each time the expression is evaluated; its value is the local's
//! address (loaded for a scalar `T`).
//!
//! clang emits a local array, struct or union whose initializer is constant
//! (what a static initializer could be) as a constant it copies, not as
//! code; merlin stores it element by element all the same, but with `-g`
//! describes what clang's would: the casts a static initializer keeps, and
//! no enum of its constants ([`FuncGen::constant_initializer`]).
//!
//! Globals and `static` locals are data: the initializer, a constant
//! expression, is written out with `.byte`, `.half`, `.word`, `.quad` (8
//! bytes, as clang writes them: a `long` or a pointer on RV64; `.word
//! sym+off` or `.quad sym+off` for an address, which the linker fills in),
//! `.ascii` and `.zero` (struct padding too), after a `.balign` for the
//! type. A `static` local's symbol is
//! `function.name`, a local symbol (no `.globl`). Each string literal gets a
//! label `.L.str` in `.rodata`, one for literals of the same bytes, as
//! clang merges them.
//!
//! ## Symbol names
//!
//! The names merlin makes are clang's: `.str` for a string literal (written
//! `.L.str`, a private label), `.compoundliteral` for the object of a
//! compound literal at file scope, `function.name` for a `static` local.
//! Several of one name are told apart as LLVM tells apart any two globals
//! of the same name (`ValueSymbolTable::makeUniqueName`): the first keeps
//! the name, and each one after gets `.N`, N the next value of one counter
//! for the whole module (`LastUnique`), shared by every name. So the numbers
//! skip:
//!
//! ```text
//!   const char *a = "x";         .L.str
//!   int *b = (int[]){1};         .compoundliteral
//!   const char *c = "y";         .L.str.1
//!   int *d = (int[]){2};         .compoundliteral.2
//!   int f(void) {
//!       static int x;            f.x
//!       { static int x; }        f.x.3
//!   }
//! ```
//!
//! A symbol takes its name when clang makes its global ([`FileGen::create`]
//! for a variable, [`FileGen::string`] for a string literal): in the order
//! the output is generated (see "Order"), not the order of the source. A
//! `static` global emitted at the end, where it is used, names its compound
//! literals there. Sema gives each such variable a key, which tells apart
//! those of one name ([`crate::sema::symbol_key`]); [`FileGen::name`] is the
//! symbol a key was given.
//!
//! Some globals of clang's take numbers though merlin has no object for
//! them, so merlin counts them all the same: the constants clang copies a
//! local's initializer from (`__const.f.a`, an array's `constinit`:
//! [`FileGen::take_name`]), and a global clang makes a second time, of its
//! definition's LLVM type, when a reference made it with another (an
//! `extern int a[]` used before `int a[3] = {1, 2, 3};`), which takes a
//! number for a moment before the name moves over to it
//! ([`FileGen::defined`]). Code control cannot reach takes no names
//! ("Unreachable code" below).
//!
//! A function or file-scope variable keeps its own name, which other files
//! link to: LLVM never renames one. When a constant counted has its name
//! (`int constinit = 3;` after an array copied from clang's `constinit`),
//! the definition takes the name over, as clang's does, a variable with
//! one more number, a function with none ([`FileGen::take_over`]).
//!
//! ## Control flow
//!
//! Conditions are evaluated into `a0` and tested with `beqz`/`bnez`. Jumps go
//! to local labels named `.L.<what>.<n>`, numbered across the whole file
//! (`.L.else.3`, `.L.break.7`); every function returns through
//! `.L.return.<name>`.
//!
//! A `switch` is a chain of compares, one `li t0, value` + `beq a0, t0` per
//! `case` in source order, then a jump to `default` (or past the body). The
//! body follows unchanged, its `case` labels (`.L.case.<function>.<n>`) where
//! they stand, so control falls through from one to the next, and `break`
//! jumps to the end. A table of addresses would be faster for dense cases; a
//! chain is simpler to read and works for any values. A C label `name` is
//! `.L.label.<function>.<name>`, and `goto` is a `j` to it.
//!
//! A condition that is an integer constant expression is decided at compile
//! time, as clang's code generator decides it at `-O0` ([`crate::elide`]):
//! `if (1) a; else b;` is `a` alone, with no test, and a `switch` on a
//! constant is the statements its case runs, up to the `break`, without
//! compares or labels. Code a jump can still reach is kept: an arm with a
//! `goto` label in it, or a `case` of a `switch` around it, and the `if` is
//! generated as usual. `while (1)` tests nothing, nor does `do ... while
//! (0)`; `while (0)`, `do ... while (1)` and `for` loops test their
//! condition, as in clang. `-g` describes only what is generated: the
//! variables of an arm left out are not described, and the types of its
//! casts are not kept.
//!
//! ## Unreachable code
//!
//! Nor is code control cannot reach generated, as clang's `EmitStmt` leaves
//! it out at `-O0` (`CGStmt.cpp`). clang generates into an insertion point,
//! the block it is filling; a jump (`return`, `break`, `continue`, `goto`)
//! takes it away, and a statement with no insertion point is skipped, so
//! its string literals take no names, its casts and enums are not
//! described. [`FuncGen::reachable`] is that insertion point:
//!
//! ```text
//!   return 0;
//!   puts("dead");        no code, no .str
//!   int n = f();         a slot for n, but no call and no -g description
//!   static int s;        emitted all the same (a declaration is visited)
//! again:
//!   puts("live");        a label: reachable again
//! ```
//!
//! A label or a `case` gives the insertion point back ([`FuncGen::block`]),
//! and a statement with a label in it is generated from its start, as
//! clang's `EnsureInsertPoint` makes a new block for it. Where the paths
//! of a statement meet ([`FuncGen::join`]: after an `if`, a loop, a
//! `switch`), control is there if it falls in or a jump generated so far
//! goes there, as clang deletes a block that has no uses. So after `if (c)
//! return 1; else return 2;` nothing is reached; nor after a `while (1)`
//! or `for (;;)` without a `break` control reaches, nor after a `switch`
//! with a `default` whose cases all return; and the statements of a
//! `switch` body before its first `case`. Some blocks are emitted whatever
//! reaches them: a `do`'s condition and a `for`'s step, so what follows a
//! `do` loop is always reached. A static local's declaration, a block, a
//! label and a jump are visited even where control cannot reach them
//! (clang's `EmitSimpleStmt`): a local declared there still gets its slot,
//! as clang allocates it, but no initialization and no `-g` description.
//!
//! (LLVM then deletes, as it generates the machine code, every block no
//! path from the function's entry reaches, such as that of a label after a
//! `return` that no `goto` control reaches jumps to; merlin keeps that
//! code. Its globals are made all the same, so that changes no name, only
//! what `-g` describes of the label's code.)
//!
//! A conditional branch reaches only ±4 KiB, and merlin-as reports a branch
//! that cannot reach its label as an error rather than rewriting it. So once a
//! function is generated, [`relax_branches`] estimates every instruction's
//! address (never underestimating a distance) and turns a branch that may be
//! too far into the opposite branch over a `j`, which reaches ±1 MiB:
//!
//! ```text
//!   beqz  a0, .L.end.4      becomes      bnez  a0, 1f
//!                                        j     .L.end.4
//!                                     1:
//! ```
//!
//! (and `beq a0, t0, ...` becomes `bne a0, t0, 1f` over a `j`).
//!
//! Every `j` codegen writes (that long form, loop back-edges, `break`,
//! `continue`, `goto`, `j .L.return.f`) goes somewhere in the same function,
//! so they all reach as long as the function's code is under 1 MiB. A bigger
//! function is an error ([`check_size`]), reported by merlin on the function
//! rather than by merlin-as on an internal label.

use crate::abi::{self, Pass};
use crate::ast::*;
use crate::consteval::{self, Base, Const};
use crate::debug::{DebugInfo, Lines, Location, Scope, Var};
use crate::elide;
use crate::sema::{arith_type, symbol_base, LITERAL};
use crate::types::{ptr_size, FuncType, IntKind, Member, RecordBody, Type, Xlen};
use crate::{Error, Pos};
use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::fmt::Write;
use std::sync::Arc;

/// Generates assembly for a whole translation unit.
pub fn generate(prog: &Program) -> Result<String, Error> {
    generate_with(prog, None)
}

/// [`generate`], with debug information (`-g`) when `debug` gives it and
/// the preprocessor's file names ([`crate::debug`]).
pub fn generate_with(prog: &Program, debug: Option<(&DebugInfo, &[String])>) -> Result<String, Error> {
    // merlin-as reads a register name as a register, never as a symbol, so a
    // function or global called `sp` or `a0` could not be referenced or even
    // defined.
    for item in &prog.items {
        let names: Vec<(&str, crate::Pos)> = match item {
            Item::Func(f) => vec![(&f.name, f.pos)],
            Item::Decl(ds) => ds.iter().map(|d| (d.name.as_str(), d.pos)).collect(),
        };
        if let Some((name, pos)) = names
            .into_iter()
            .find(|(n, _)| excalibur::reg::parse(n).is_some())
        {
            return Err(Error::new(
                pos,
                format!("'{name}' is a register name, which cannot be used as a symbol name"),
            ));
        }
    }

    let xlen = prog.xlen;
    // The definitions, in the order they come; a compound literal of the
    // file's scope is emitted by the initializer that names it.
    let mut defs = Vec::new();
    for item in &prog.items {
        match item {
            Item::Func(f) if f.body.is_some() => defs.push(Def::Func(f)),
            Item::Func(_) => {} // a prototype: the definition is elsewhere
            Item::Decl(decls) => defs.extend(decls.iter().filter(|d| d.define).map(Def::Var)),
        }
    }
    let n = defs.len();
    let mut file = FileGen {
        xlen,
        out: String::new(),
        section: "",
        pieces: Vec::new(),
        labels: 0,
        strings: Vec::new(),
        symbols: SymbolTable::default(),
        names: HashMap::new(),
        debug: debug.map(|(info, names)| Lines::new(info, names, xlen)),
        by_symbol: defs
            .iter()
            .enumerate()
            .map(|(k, d)| (d.symbol().to_string(), k))
            .collect(),
        defs,
        emitted: vec![false; n],
        waiting: vec![false; n],
        queue: Vec::new(),
        created: Vec::new(),
        created_set: HashSet::new(),
        referred_as: HashMap::new(),
        held: HashSet::new(),
        chunks: HashMap::new(),
        dry: false,
    };

    // clang's front end (CodeGenModule): the file, as it comes, emits what
    // must be emitted and keeps the rest for when it is used; then the
    // tentative definitions; then, what was used, each one's uses right
    // after it.
    for k in 0..n {
        let def = file.defs[k];
        if def.hidden() || def.tentative() {
            continue;
        }
        if !def.deferrable() {
            file.emit(k)?;
        } else if file.created_set.contains(def.symbol()) {
            file.queue.push(k);
        } else {
            file.waiting[k] = true;
        }
    }
    for k in 0..n {
        let def = file.defs[k];
        if !def.tentative() || def.hidden() {
            continue;
        }
        if def.deferrable() && !file.created_set.contains(def.symbol()) {
            file.waiting[k] = true;
        } else {
            file.emit(k)?;
        }
    }
    file.deferred()?;

    // The output: each symbol's code or data in the order the symbols were
    // made (the module's order, which LLVM writes them in), then the string
    // literals and the debug sections.
    file.out.clear();
    file.section = "";
    if let Some(d) = &file.debug {
        let _ = writeln!(file.out, "    {}", d.file0());
    }
    let created = std::mem::take(&mut file.created);
    for sym in &created {
        for (section, text) in file.chunks.remove(sym).unwrap_or_default() {
            if !section.is_empty() {
                file.switch(section);
            }
            file.out.push_str(&text);
        }
    }
    // `-g`: the functions in the order of their code, which the files are
    // numbered by, and the entries made in.
    if let Some(d) = &mut file.debug {
        let functions: Vec<&str> = created
            .iter()
            .filter(|s| {
                file.by_symbol
                    .get(*s)
                    .is_some_and(|&k| matches!(file.defs[k], Def::Func(_)))
            })
            .map(String::as_str)
            .collect();
        d.code_order(&functions);
        file.out = d.renumber(&file.out);
    }
    file.strings();
    if let Some(d) = &mut file.debug {
        file.out.push_str(&d.sections());
    }
    Ok(file.out)
}

/// A definition of the file: a function, or a global variable (a compound
/// literal of the file's scope among them). The order they are emitted in
/// is clang's (see "Order" in the module's documentation).
#[derive(Clone, Copy)]
enum Def<'a> {
    Func(&'a Function),
    Var(&'a Declarator),
}

impl<'a> Def<'a> {
    fn symbol(&self) -> &'a str {
        match self {
            Def::Func(f) => &f.name,
            Def::Var(d) => match &d.var {
                Some(VarRef::Global(sym)) => sym,
                _ => unreachable!("sema gives globals a symbol"),
            },
        }
    }

    /// Emitted only once used: a `static` function or global (and a C99
    /// inline definition, which merlin keeps local), as clang's
    /// `MustBeEmitted` says of internal linkage.
    fn deferrable(&self) -> bool {
        match self {
            Def::Func(f) => f.is_static || f.inline_only,
            Def::Var(d) => d.storage == Storage::Static,
        }
    }

    /// A global without an initializer: clang emits it at the end of the
    /// file (`CompleteTentativeDefinition`).
    fn tentative(&self) -> bool {
        matches!(self, Def::Var(d) if d.init.is_none())
    }

    /// A compound literal of the file's scope, which the initializer that
    /// names it emits (`GetAddrOfConstantCompoundLiteral`).
    fn hidden(&self) -> bool {
        matches!(self, Def::Var(d) if d.name == LITERAL)
    }
}

/// Whether cast `e` makes a null pointer constant a pointer as clang's
/// `CK_NullToPointer` does, whose operand clang neither emits nor visits
/// (so `-g` keeps no cast's type in it): an integer one (`(int *)0`, `int
/// *p = 0`, `(int *)(long)0`), or `(void *)0` converted to another pointer
/// type where C converts as by assignment (assigned, passed, returned, an
/// operand of `?:`: `struct b *p = NULL;`). Compared with a pointer, or cast
/// explicitly, `(void *)0` is converted as any pointer is (a bit cast), and
/// visited. (In a static initializer clang's `ConstExprEmitter` does visit
/// the operand of such a cast: [`FileGen::initializer_casts`].)
fn null_to_pointer(e: &Expr, xlen: Xlen) -> bool {
    let ExprKind::Cast { x, implicit, .. } = &e.kind else {
        return false;
    };
    e.ty.is_ptr() && consteval::is_null_pointer(x, xlen) && (x.ty.is_integer() || *implicit)
}

/// The bytes of a register, and of an argument word: 4 on RV32, 8 on RV64.
fn word(x: Xlen) -> i32 {
    ptr_size(x) as i32
}

/// `ra` and the caller's `s0` take the top two words of every frame (below
/// the saved argument registers of a variadic function): 8 bytes on RV32,
/// 16 on RV64.
fn saved_regs(x: Xlen) -> i32 {
    2 * word(x)
}

/// The load and the store of a whole register: `lw`/`sw` on RV32, `ld`/`sd`
/// on RV64.
fn word_ops(x: Xlen) -> (&'static str, &'static str) {
    match x {
        Xlen::Rv32 => ("lw", "sw"),
        Xlen::Rv64 => ("ld", "sd"),
    }
}

/// The largest frame: offsets from `s0` and the frame size must stay `i32`s.
const MAX_FRAME: i64 = i32::MAX as i64 & !15;

/// Places every local the code uses (`used`) in the frame, below the
/// `header` bytes at its top (`ra`, `s0` and a variadic function's saved
/// registers): its offset from `s0` (0 for one not used, which has no
/// slot), and the frame size (a multiple of 16, the convention's stack
/// alignment). `None` if the frame would be larger than [`MAX_FRAME`].
fn layout(locals: &[Local], used: &[bool], header: i32, x: Xlen) -> Option<(Vec<i32>, i32)> {
    // In i64: each local is at most `i32::MAX` bytes, but their sum need not be.
    let mut off = -i64::from(header);
    let mut offsets = Vec::with_capacity(locals.len());
    for (l, &used) in locals.iter().zip(used) {
        if !used {
            offsets.push(0);
            continue;
        }
        let mut size = l.ty.size(x).expect("sema: locals have complete types");
        // Arrays and structs are aligned to a register so that clearing them
        // can use `sw` (`sd` on RV64), and a struct's slot is a whole number
        // of registers, so that one arriving in registers can be stored with
        // the same (see [`abi`]).
        let w = word(x) as u32;
        let align = if l.ty.is_array() || l.ty.is_record() {
            l.ty.align(x).max(w)
        } else {
            l.ty.align(x)
        };
        if l.ty.is_record() {
            size = size.next_multiple_of(w);
        }
        // Round down to the alignment (two's complement: works below zero).
        off = (off - i64::from(size)) & !(i64::from(align) - 1);
        if (-off + 15) & !15 > MAX_FRAME {
            return None;
        }
        offsets.push(off as i32);
    }
    Some((offsets, ((-off + 15) & !15) as i32))
}

/// How a call's arguments travel.
struct ArgLayout {
    /// The arguments' types: the parameters', then for `...` the (promoted)
    /// arguments' own.
    types: Vec<Type>,
    /// Each argument's first word and how it is passed (see [`abi`]).
    words: Vec<(usize, Pass)>,
    /// How many words they take in all, with a hidden result address.
    used: usize,
}

impl ArgLayout {
    fn new(ft: &FuncType, args: &[Expr], x: Xlen) -> ArgLayout {
        let mut types = ft.params.clone();
        types.extend(args[ft.params.len()..].iter().map(|a| a.ty.clone()));
        let (words, used) = abi::call_arguments(&ft.ret, &types, ft.params.len(), x);
        ArgLayout { types, words, used }
    }
}

/// Where word `n` of a call's arguments is (see [`abi`]): a register, or
/// an offset from the `sp` of the call (the callee's `s0`).
enum ArgWord {
    Reg(String),
    Stack(i32),
}

fn arg_word(n: usize, x: Xlen) -> ArgWord {
    if n < abi::ARG_REGS {
        ArgWord::Reg(format!("a{n}"))
    } else {
        ArgWord::Stack(word(x) * (n - abi::ARG_REGS) as i32)
    }
}

/// Does `v` fit a 12-bit signed immediate (`addi`, `lw`, `sw`)?
fn fits_imm12(v: impl Into<i64>) -> bool {
    (-2048..=2047).contains(&v.into())
}

/// The load that brings a value of type `ty` into a register, widened.
fn load_op(ty: &Type, x: Xlen) -> &'static str {
    match ty.unqual() {
        // `_Bool` holds 0 or 1: `lbu`.
        // An `int` or `unsigned int` is loaded sign-extended on RV64 too:
        // see "The 32-bit invariant" above.
        Type::Int(k) => match (k.size(x), k.signed()) {
            (1, true) => "lb",
            (1, false) => "lbu",
            (2, true) => "lh",
            (2, false) => "lhu",
            (4, _) => "lw",
            _ => "ld",
        },
        Type::Ptr(_) => word_ops(x).0,
        _ => unreachable!("no value of type '{ty}' is loaded"),
    }
}

/// The store that writes a value of type `ty` (its low bytes) to memory.
fn store_op(ty: &Type, x: Xlen) -> &'static str {
    match ty.size(x) {
        Some(1) => "sb",
        Some(2) => "sh",
        Some(4) => "sw",
        Some(8) => "sd",
        _ => unreachable!("no value of type '{ty}' is stored"),
    }
}

/// The low and high words of a 64-bit value, as `li` takes them.
fn pair_image(v: i128) -> (i32, i32) {
    let v = v as i64;
    (v as i32, (v >> 32) as i32)
}

/// `log2(n)` if `n` is a power of two.
fn log2(n: u32) -> Option<u32> {
    n.is_power_of_two().then(|| n.trailing_zeros())
}

/// The names of the module's symbols (its functions and globals), as LLVM's
/// `ValueSymbolTable` keeps them: a name asked for that is taken already is
/// made unique with `.N`, N from a counter of the whole table (see "Symbol
/// names" above).
#[derive(Default)]
struct SymbolTable {
    taken: HashSet<String>,
    /// The last number appended (`LastUnique`): the next one is one more,
    /// whichever name it is for.
    last_unique: u32,
}

impl SymbolTable {
    /// Takes the name `base`, or, if it is taken, `base.N` for the first N
    /// after the last one given that makes a name not taken
    /// (`makeUniqueName`).
    fn insert(&mut self, base: &str) -> String {
        if self.taken.insert(base.to_string()) {
            return base.to_string();
        }
        loop {
            self.last_unique += 1;
            let name = format!("{base}.{}", self.last_unique);
            if self.taken.insert(name.clone()) {
                return name;
            }
        }
    }

    /// A second global made under the name `base` of one that has it, which
    /// then takes the name over (`takeName`): the second is named
    /// `base.N` for a moment, so the counter moves on, and no name is left
    /// taken.
    fn rename(&mut self, base: &str) {
        loop {
            self.last_unique += 1;
            if !self.taken.contains(&format!("{base}.{}", self.last_unique)) {
                return;
            }
        }
    }
}

/// What clang's `emitStoresForConstant` looks at in the constant
/// initializer of a local array, struct or union, to choose how to store it
/// ([`FuncGen::local_decl`]): its bytes and its scalars.
struct ConstantImage {
    /// Each byte of the object, `None` for those of an address (which no
    /// byte value stands for). Padding and what the list leaves out are
    /// zero, as C has them and clang's constant spells them out.
    bytes: Vec<Option<u8>>,
    /// How many scalars are not zero: a store each, after clearing.
    nonzero: usize,
    /// One is the address of a symbol itself (no offset, not converted to
    /// an integer), a global in LLVM's constant, which
    /// `canEmitInitWithFewStoresAfterBZero` gives up on.
    symbol: bool,
}

impl ConstantImage {
    fn new(size: u32) -> ConstantImage {
        ConstantImage {
            bytes: vec![Some(0); size as usize],
            nonzero: 0,
            symbol: false,
        }
    }

    /// A scalar of bytes `bytes` at byte `at`.
    fn scalar(&mut self, at: usize, bytes: &[u8], nonzero: bool) {
        for (i, &b) in bytes.iter().enumerate() {
            self.bytes[at + i] = Some(b);
        }
        self.nonzero += usize::from(nonzero);
    }

    /// An address of `size` bytes at byte `at`; `symbol`: a symbol's own.
    fn address(&mut self, at: usize, size: usize, symbol: bool) {
        self.bytes[at..at + size].fill(None);
        self.nonzero += 1;
        self.symbol |= symbol;
    }

    /// Whether clang copies the object from a constant global, at `-O0`:
    /// not when every byte is zero (a `memset` of zeros), and for more than
    /// 32 bytes not when at most 6 scalars are not zero and no one is a
    /// symbol's address (zeros, then a store each:
    /// `shouldUseBZeroPlusStoresToInitialize`), nor when every byte is the
    /// same (a `memset`: `shouldUseMemSetToInitialize`).
    fn copied(&self) -> bool {
        if self.bytes.iter().all(|&b| b == Some(0)) {
            return false;
        }
        if self.bytes.len() <= 32 {
            return true;
        }
        let few_stores = !self.symbol && self.nonzero <= 6;
        let one_byte = self.bytes[0].is_some() && self.bytes.iter().all(|&b| b == self.bytes[0]);
        !few_stores && !one_byte
    }
}

/// What is shared by the whole file: the output, the label counter and the
/// string literals seen so far.
struct FileGen<'a> {
    /// The target.
    xlen: Xlen,
    /// The text being written: the piece of a symbol's output in
    /// `section` (or the whole file, at the end).
    out: String,
    /// The section the output is in.
    section: &'static str,
    /// The pieces of the symbol being written so far, each in its section.
    pieces: Vec<(&'static str, String)>,
    /// Label counter shared by every function in the file, so labels are unique.
    labels: usize,
    /// String literals, emitted at the end: each one's bytes (its whole
    /// array, the NUL included) and label.
    strings: Vec<(Vec<u8>, String)>,
    /// The names of the module's symbols, and the symbol each key sema
    /// gave a variable is written as ("Symbol names" above).
    symbols: SymbolTable,
    names: HashMap<String, String>,
    /// With `-g`, the debug information ([`crate::debug`]).
    debug: Option<Lines>,
    /// The file's definitions, in the order they come, and by symbol.
    defs: Vec<Def<'a>>,
    by_symbol: HashMap<String, usize>,
    /// The definitions emitted so far.
    emitted: Vec<bool>,
    /// Deferred definitions waiting for a use (clang's `DeferredDecls`).
    waiting: Vec<bool>,
    /// Used, to be emitted (clang's `DeferredDeclsToEmit`).
    queue: Vec<usize>,
    /// Every symbol made so far, defined or referred to, in the order it was
    /// made: the order of the module's functions and globals, in which the
    /// output has them.
    created: Vec<String>,
    created_set: HashSet<String>,
    /// The type a variable was declared with where a reference made its
    /// symbol, before its definition was emitted ([`Self::defined`]).
    referred_as: HashMap<String, Type>,
    /// The names of the constants merlin counts but does not output
    /// ([`Self::take_name`]) that a function or variable could have, until
    /// its definition takes the name over.
    held: HashSet<String>,
    /// The output of each symbol emitted, in pieces by section.
    chunks: HashMap<String, Vec<(&'static str, String)>>,
    /// A function's first pass is being generated, which makes nothing
    /// ([`FileGen::function`]).
    dry: bool,
}

impl<'a> FileGen<'a> {
    fn switch(&mut self, section: &'static str) {
        if self.section != section {
            let directive = if section == ".rodata" {
                ".section .rodata"
            } else {
                section
            };
            // A blank line sets the new section apart from what came before.
            if !self.out.is_empty() {
                self.out.push('\n');
            }
            let _ = writeln!(self.out, "    {directive}");
            self.section = section;
        }
    }

    // ---- the order of the output ----

    /// Makes symbol `sym` (a key sema gave it), as clang makes a function or
    /// global the first time it is defined or referred to
    /// (`GetOrCreateLLVMFunction`, `GetOrCreateLLVMGlobal`): the output has
    /// the symbols in this order, and the symbol takes its name now ("Symbol
    /// names" above). A deferred definition waiting for its first use is to
    /// be emitted now.
    fn create(&mut self, sym: &str) {
        if self.dry {
            return;
        }
        if self.created_set.insert(sym.to_string()) {
            // A function or file-scope variable keeps its own name, which
            // linking needs: LLVM never renames one ([`Self::take_over`]).
            // The names sema makes up (`f.x`, `.compoundliteral`, keys with
            // a `#`) are made unique.
            let name = if is_c_name(sym) {
                self.symbols.taken.insert(sym.to_string());
                sym.to_string()
            } else {
                self.symbols.insert(symbol_base(sym))
            };
            self.names.insert(sym.to_string(), name);
            self.created.push(sym.to_string());
            if let Some(&k) = self.by_symbol.get(sym) {
                if self.waiting[k] {
                    self.waiting[k] = false;
                    self.queue.push(k);
                }
            }
        }
    }

    /// The symbol made for key `sym` ([`Self::create`]). A function's first
    /// pass, which makes nothing, writes the key, and its output is thrown
    /// away.
    fn name<'s>(&'s self, sym: &'s str) -> &'s str {
        debug_assert!(
            self.dry || self.names.contains_key(sym),
            "'{sym}' is used before it is made"
        );
        self.names.get(sym).map_or(sym, String::as_str)
    }

    /// A reference to symbol `sym` in the code or the data generated, of a
    /// variable declared there with type `ty`. A compound literal it names
    /// is emitted first, as clang emits one where an initializer names it.
    fn reference(&mut self, sym: &str, ty: Option<&Type>) {
        if self.dry {
            return;
        }
        if let Some(&k) = self.by_symbol.get(sym) {
            if self.defs[k].hidden() && !self.emitted[k] {
                self.emit(k).expect("a variable is emitted without errors");
            }
        }
        if let Some(ty) = ty.filter(|_| !self.created_set.contains(sym)) {
            self.referred_as.insert(sym.to_string(), ty.clone());
        }
        self.create(sym);
    }

    /// The definition of global `d` (key `sym`), its initializer's data
    /// made: if a reference made its symbol before, as clang makes a
    /// global of the type the variable was declared with there, and the
    /// constant clang makes of the initializer is of another type, clang
    /// makes a new global of that type, under the name, and moves the name
    /// over to it (`GetOrCreateLLVMGlobal`): the name counter moves on.
    ///
    /// ```text
    ///   extern int a[];          (a [0 x i32] global, where p refers to it)
    ///   int *p = a;
    ///   int a[3] = {1, 2, 3};    ([3 x i32]: a new global, a.1, renamed a)
    /// ```
    ///
    /// The new global is made where the definition is, after the symbols
    /// made so far: the output has the variable there.
    fn defined(&mut self, d: &Declarator, sym: &str) {
        if self.take_over(sym, true) {
            self.referred_as.remove(sym);
            return;
        }
        let Some(referred) = self.referred_as.remove(sym) else {
            return;
        };
        let same = same_llvm_type(&referred, &d.ty, self.xlen)
            && d.init
                .as_ref()
                .is_none_or(|init| self.constant_has_type(&d.ty, init));
        if !same {
            let name = self.name(sym).to_string();
            self.symbols.rename(&name);
            self.made_now(sym);
        }
    }

    /// The definition of function or variable `sym` (a C name), when a
    /// constant clang made, counted but not output ([`Self::take_name`]),
    /// has its name: `constinit`, of an array copied from it, before `int
    /// constinit = 3;`. A reference to the symbol finds that constant
    /// (`GetGlobalValue`), so clang makes the symbol's global only for its
    /// definition, which takes the name over (`takeName`): a variable is
    /// named `constinit.N` for a moment (`GetOrCreateLLVMGlobal`), one more
    /// number; a function is made without a name
    /// (`GetOrCreateLLVMFunction`), none. Either is made there, after the
    /// symbols made so far. Whether the name was the constant's.
    fn take_over(&mut self, sym: &str, variable: bool) -> bool {
        if self.dry || !self.held.remove(sym) {
            return false;
        }
        if variable {
            self.symbols.rename(sym);
        }
        self.made_now(sym);
        true
    }

    /// Moves symbol `sym`, made already, to after every symbol made so
    /// far: clang made its global again, now.
    fn made_now(&mut self, sym: &str) {
        self.created.retain(|s| s != sym);
        self.created.push(sym.to_string());
    }

    /// Whether the constant clang makes of `init`, the initializer of an
    /// object of type `ty`, has the LLVM type clang gives such an object
    /// in memory (`ConvertTypeForMem`), as clang's `ConstExprEmitter`
    /// builds it:
    ///
    /// - a scalar's, and a `char` array's string (the array's size), have
    ///   it; so does a subobject the list leaves out (an empty list here),
    ///   clang's `EmitNullConstant`;
    /// - an array's (`EmitArrayConstant`) is an array of its elements'
    ///   type, which is the array's if every element's is, unless 8 zeros
    ///   or more end it (a struct then, the zeros an array of their own);
    ///   all zeros is the array's own `zeroinitializer`;
    /// - a struct's is the struct's if every member's constant has its type
    ///   and the struct has no padding: C has clang spell the padding out as
    ///   arrays of zeros (`DoZeroInitPadding`), which the struct's own type
    ///   leaves to the layout;
    /// - a union's has the member it initializes and padding to the
    ///   union's size, the union's own type if that member's constant has
    ///   the LLVM type of the member the union's type holds (the most
    ///   aligned, then the largest, the first of those: `lowerUnion`).
    fn constant_has_type(&self, ty: &Type, init: &Init) -> bool {
        let x = self.xlen;
        match (ty.unqual(), init) {
            (_, Init::List(items, _)) if items.is_empty() => true,
            (Type::Array(elem, Some(n)), Init::List(items, _)) => {
                let nonzero = items
                    .iter()
                    .rposition(|i| !self.constant_is_null(elem, i))
                    .map_or(0, |i| i + 1);
                nonzero == 0
                    || (*n as usize - nonzero < 8 && items.iter().all(|i| self.constant_has_type(elem, i)))
            }
            (Type::Record(r), Init::List(items, _)) => {
                let body = r.body().expect("sema: initialized objects are complete");
                if r.union {
                    let held = &body.members[items.len() - 1];
                    let storage = union_storage(body, x);
                    return same_llvm_type(&held.ty, &storage.ty, x)
                        && self.constant_has_type(&held.ty, &items[items.len() - 1]);
                }
                !padded(body, x)
                    && body
                        .members
                        .iter()
                        .zip(items)
                        .all(|(m, i)| self.constant_has_type(&m.ty, i))
            }
            (
                _,
                Init::Expr(Expr {
                    kind: ExprKind::CompoundLit { init, .. },
                    ty: lit,
                    ..
                }),
            ) => self.constant_has_type(lit, init),
            _ => true,
        }
    }

    /// Whether the constant clang makes of `init` (of an object of type
    /// `ty`) is all zeros (LLVM's `isNullValue`): what ends an array's
    /// elements before [`Self::constant_has_type`] counts its zeros.
    fn constant_is_null(&self, ty: &Type, init: &Init) -> bool {
        match (ty.unqual(), init) {
            (Type::Array(elem, _), Init::List(items, _)) => {
                items.iter().all(|i| self.constant_is_null(elem, i))
            }
            (Type::Record(r), Init::List(items, _)) => {
                // (A union's: the member it initializes, its last item.)
                let body = r.body().expect("sema: initialized objects are complete");
                let first = if r.union { items.len().saturating_sub(1) } else { 0 };
                body.members[first..]
                    .iter()
                    .zip(&items[first..])
                    .all(|(m, i)| self.constant_is_null(&m.ty, i))
            }
            (_, Init::List(items, _)) => items.is_empty(),
            (
                Type::Array(_, Some(n)),
                Init::Expr(Expr {
                    kind: ExprKind::Str(bytes),
                    ..
                }),
            ) => bytes.iter().take(*n as usize).all(|&b| b == 0),
            (
                _,
                Init::Expr(Expr {
                    kind: ExprKind::CompoundLit { init, .. },
                    ty: lit,
                    ..
                }),
            ) => self.constant_is_null(lit, init),
            (_, Init::Expr(e)) => matches!(
                consteval::eval(e, self.xlen),
                Some(Const::Int(0, _))
                    | Some(Const::Addr {
                        base: None,
                        offset: 0
                    })
            ),
            (_, Init::Designated(..)) => unreachable!("sema resolves designators"),
        }
    }

    /// Emits definition `k`: its code or its data, and with `-g` its entry.
    fn emit(&mut self, k: usize) -> Result<(), Error> {
        self.emitted[k] = true;
        match self.defs[k] {
            Def::Func(f) => self.function(f),
            Def::Var(d) => {
                self.variable(d, false);
                // A compound literal's hidden variable is not described, as
                // clang has it: its string literals go with the global that
                // names it, which is being emitted.
                if self.defs[k].hidden() {
                    return Ok(());
                }
                if let Some(VarRef::Global(sym)) = &d.var {
                    let sym = self.name(sym).to_string();
                    if let Some(dbg) = self.debug.as_mut() {
                        let external = d.storage != Storage::Static;
                        dbg.global(&d.name, &sym, (&d.ty, &d.spelled), d.pos, external);
                    }
                }
                Ok(())
            }
        }
    }

    /// Emits what was used and not emitted yet, each one's uses right after
    /// it, depth first (clang's `EmitDeferred`).
    fn deferred(&mut self) -> Result<(), Error> {
        let now = std::mem::take(&mut self.queue);
        for k in now {
            if !self.emitted[k] {
                self.emit(k)?;
                if !self.queue.is_empty() {
                    self.deferred()?;
                }
            }
        }
        Ok(())
    }

    /// Starts the output of a symbol.
    fn begin(&mut self) {
        self.out.clear();
        self.section = "";
        self.pieces.clear();
    }

    /// Goes on in `section`.
    fn piece(&mut self, section: &'static str) {
        if self.section != section {
            if !self.out.is_empty() {
                let text = std::mem::take(&mut self.out);
                self.pieces.push((self.section, text));
            }
            self.section = section;
        }
    }

    /// Ends the output of symbol `sym`.
    fn finish(&mut self, sym: &str) {
        let text = std::mem::take(&mut self.out);
        self.pieces.push((self.section, text));
        let pieces = std::mem::take(&mut self.pieces);
        self.chunks.insert(sym.to_string(), pieces);
    }

    /// Generates function `f`.
    fn function(&mut self, f: &'a Function) -> Result<(), Error> {
        let xlen = self.xlen;
        let body = f.body.as_ref().expect("a definition has a body");
        self.create(&f.name);
        self.take_over(&f.name, false);
        // A variadic function keeps the argument registers its named
        // parameters leave free at the top of its frame.
        let Type::Func(ft) = f.ty() else {
            unreachable!("a function's type")
        };
        let (_, named_words) = abi::arguments(&ft, xlen);
        let va_save = if f.variadic {
            word(xlen) * (abi::ARG_REGS - named_words.min(abi::ARG_REGS)) as i32
        } else {
            0
        };
        let header = saved_regs(xlen) + va_save;
        let n = f.locals.len();
        let gen = |file: &mut FileGen<'a>, slots: Vec<bool>, (offsets, frame): (Vec<i32>, i32)| {
            let mut g = FuncGen {
                xlen,
                func: f,
                offsets,
                frame,
                named_words,
                va_save,
                lines: Vec::new(),
                depth: 0,
                breaks: Vec::new(),
                continues: Vec::new(),
                file,
                ret_label: format!(".L.return.{}", f.name),
                elided_cases: Vec::new(),
                after_prologue: false,
                constant_init: false,
                scopes: Vec::new(),
                used: (0..n).map(|i| Cell::new(i < f.params.len())).collect(),
                slots,
                reachable: true,
                reached: HashSet::new(),
            };
            g.function(body);
            let used: Vec<bool> = g.used.iter().map(Cell::get).collect();
            (g.lines, used)
        };
        // The frame holds only the locals the code generated uses, as clang's
        // at `-O0` allocates only for the code it emits: not those of an arm
        // a constant condition leaves out, nor the temporaries of an operand
        // left out. A first pass finds them, generating into nothing (no
        // `-g`, no symbols made, no data, its labels and string literals
        // taken back); the second is the function.
        let debug = self.debug.take();
        let (labels, strings) = (self.labels, self.strings.len());
        self.dry = true;
        let all = layout(&f.locals, &vec![true; n], header, xlen).unwrap_or((vec![0; n], 0));
        let (_, used) = gen(self, vec![true; n], all);
        self.dry = false;
        self.debug = debug;
        self.labels = labels;
        self.strings.truncate(strings);
        let Some(frame) = layout(&f.locals, &used, header, xlen) else {
            let msg = format!("the locals of '{}' need a stack frame larger than 2 GiB", f.name);
            return Err(Error::new(f.pos, msg));
        };
        let (lines, again) = gen(self, used.clone(), frame);
        debug_assert_eq!(used, again, "both passes generate the same code");
        self.begin();
        self.piece(".text");
        self.out.push('\n');
        let long = relax_branches(&lines);
        check_size(f, &lines, &long)?;
        render(&lines, &long, &mut self.out);
        self.finish(&f.name);
        Ok(())
    }

    /// The label of a string literal, at `pos`: the same bytes are one
    /// literal, as clang merges them (C99 6.4.5p6 leaves it open). A new
    /// one is named `.str` now, as clang names its private global
    /// (`GetAddrOfConstantStringFromLiteral`), `.L.str` in the assembly
    /// (`.L`: a private symbol, which the object leaves out).
    fn string(&mut self, bytes: &[u8], pos: Pos) -> String {
        let mut array = bytes.to_vec();
        array.push(0);
        self.string_array(&array, pos)
    }

    /// The label of a string literal whose array is `array` (its bytes,
    /// the NUL included if it has one): [`Self::string`]'s, or that of one
    /// that initializes a longer or shorter `char` array, whose type sema
    /// makes that array's (C99 6.7.8p14). Literals are the same when their
    /// arrays are, as clang's are the same constant.
    fn string_array(&mut self, array: &[u8], pos: Pos) -> String {
        if let Some((_, label)) = self.strings.iter().find(|(s, _)| s == array) {
            return label.clone();
        }
        // A first pass's literals are taken back ([`Self::function`]): they
        // take no name.
        let label = if self.dry {
            String::from(".L.str")
        } else {
            format!(".L{}", self.symbols.insert(".str"))
        };
        self.strings.push((array.to_vec(), label.clone()));
        if let Some(d) = self.debug.as_mut() {
            d.literal(&label, array.len(), pos);
        }
        label
    }

    /// Takes the name `base` for a global clang makes that merlin has no
    /// object for (a constant clang copies a local's initializer from,
    /// [`FuncGen::local_decl`]): the counter goes on as clang's ("Symbol
    /// names" above). A function's first pass takes none.
    ///
    /// The constant keeps a name a C symbol could have (`constinit`) until
    /// that symbol's definition takes it over ([`Self::take_over`]).
    fn take_name(&mut self, base: &str) {
        if self.dry {
            return;
        }
        let name = self.symbols.insert(base);
        if is_c_name(&name) {
            self.held.insert(name);
        }
    }

    /// Emits a global variable or, when `local`, a `static` local: its
    /// symbol and its data. clang makes a global's symbol after its
    /// initializer (`EmitGlobalVarDefinition`), unless something referred
    /// to it before, and a static local's before
    /// (`getOrCreateStaticVarDecl`), so the symbols its initializer refers
    /// to come before a global's and after a static local's.
    fn variable(&mut self, d: &Declarator, local: bool) {
        let Some(VarRef::Global(sym)) = &d.var else {
            unreachable!("sema gives globals and statics a symbol")
        };
        if local {
            if self.dry {
                return;
            }
            self.create(sym);
        }
        let mut data = Vec::new();
        if let Some(init) = &d.init {
            self.data(&d.ty, init, &mut data);
        }
        self.defined(d, sym);
        self.create(sym);
        let name = self.name(sym).to_string();
        self.begin();
        // A `const` object never changes: read-only data, as clang does (a
        // `const volatile` one may be changed from outside, so it stays
        // writable).
        let quals = d.ty.quals();
        if quals.konst && !quals.volatile {
            self.piece(".rodata");
        } else if d.init.is_some() {
            self.piece(".data");
        } else {
            self.piece(".bss");
        }
        let size =
            d.ty.size(self.xlen)
                .expect("sema: definitions have complete types");
        let storage = if d.storage == Storage::Static {
            "static "
        } else {
            ""
        };
        let bytes = if size == 1 { "byte" } else { "bytes" };
        // A compound literal has no name in C: its symbol's.
        let shown = if d.name == LITERAL { &name } else { &d.name };
        let _ = writeln!(
            self.out,
            "\n    # {storage}{}: {size} {bytes}",
            d.ty.declare(shown)
        );
        if d.storage != Storage::Static {
            let _ = writeln!(self.out, "    .globl {name}");
        }
        if d.ty.align(self.xlen) > 1 {
            let _ = writeln!(self.out, "    .balign {}", d.ty.align(self.xlen));
        }
        // An object symbol with its size, as for functions: debuggers and
        // tests find what an address belongs to from the symbol table.
        let _ = writeln!(self.out, "    .type {name}, @object");
        let _ = writeln!(self.out, "    .size {name}, {size}");
        let _ = writeln!(self.out, "{name}:");
        if d.init.is_none() {
            data.push(format!(".zero {size}"));
        }
        for line in data {
            let _ = writeln!(self.out, "    {line}");
        }
        self.finish(sym);
    }

    /// The directives for an object of type `ty` initialized by `init`.
    fn data(&mut self, ty: &Type, init: &Init, out: &mut Vec<String>) {
        match (ty.unqual(), init) {
            // A subobject a designator skipped: zero.
            (_, Init::List(items, _)) if items.is_empty() => {
                out.push(format!(".zero {}", ty.size(self.xlen).expect("a complete type")));
            }
            (Type::Record(r), Init::List(items, _)) if r.union => {
                // The member the union is initialized with is the last item.
                let body = r.body().expect("sema: definitions have complete types");
                let k = items.len() - 1;
                let m = &body.members[k];
                self.data(&m.ty, &items[k], out);
                let size = m.ty.size(self.xlen).expect("complete member");
                if body.size > size {
                    out.push(format!(".zero {}", body.size - size));
                }
            }
            (Type::Record(r), Init::List(items, _)) => {
                // Members in order (sema's list is fully braced; a union's has
                // at most its first member), with the padding between them and
                // after the last as zeros.
                let body = r.body().expect("sema: definitions have complete types");
                let mut at = 0;
                for (m, item) in body.members.iter().zip(items) {
                    if m.offset > at {
                        out.push(format!(".zero {}", m.offset - at));
                    }
                    self.data(&m.ty, item, out);
                    at = m.offset + m.ty.size(self.xlen).expect("complete member");
                }
                if body.size > at {
                    out.push(format!(".zero {}", body.size - at));
                }
            }
            (Type::Array(elem, Some(n)), Init::List(items, _)) => {
                for item in items {
                    self.data(elem, item, out);
                }
                let rest = (*n as usize - items.len()) as u32 * elem.size(self.xlen).unwrap();
                if rest > 0 {
                    out.push(format!(".zero {rest}"));
                }
            }
            (
                Type::Array(_, Some(n)),
                Init::Expr(Expr {
                    kind: ExprKind::Str(bytes),
                    ..
                }),
            ) => {
                // The text, cut to the array (C allows leaving out the NUL), then zeros.
                let take = bytes.len().min(*n as usize);
                out.extend(byte_directives(&bytes[..take]));
                if *n as usize > take {
                    out.push(format!(".zero {}", *n as usize - take));
                }
            }
            (_, Init::Expr(e)) => {
                self.initializer_casts(e);
                let value = consteval::eval(e, self.xlen).expect("sema: static initializers are constant");
                // `.quad` for 8 bytes, as clang writes it (merlin-as also
                // reads `.dword` and `.8byte`).
                let directive = match ty.size(self.xlen) {
                    Some(1) => ".byte",
                    Some(2) => ".half",
                    Some(4) => ".word",
                    _ => ".quad",
                };
                // The value as the directive's bytes hold it: an `unsigned`
                // 0xffffffff is 4294967295, a negative address offset wraps.
                let operand = match value {
                    Const::Int(v, _) => v.to_string(),
                    Const::Addr { base: None, offset } => IntKind::ULong.wrap(offset, self.xlen).to_string(),
                    Const::Addr {
                        base: Some(base),
                        offset,
                    } => {
                        let sym = match base {
                            Base::Sym(s) => {
                                self.reference(&s, referred_type(e, &s));
                                self.name(&s).to_string()
                            }
                            Base::Str(bytes) => self.string(&bytes, e.pos),
                        };
                        match offset {
                            0 => sym,
                            o if o > 0 => format!("{sym}+{o}"),
                            o => format!("{sym}{o}"),
                        }
                    }
                };
                out.push(format!("{directive} {operand}"));
            }
            _ => unreachable!("sema checked initializer shapes"),
        }
    }

    /// `-g`: the explicit casts of a static initializer's element whose
    /// types clang keeps. Its front end emits an initializer with its
    /// `ConstExprEmitter` first, which keeps the type of each explicit cast
    /// it visits (`EmitExplicitCastExprType`), and evaluates what that
    /// emitter gives up on. The emitter visits casts, and goes on into a
    /// cast's operand when the cast changes nothing (the same type but for
    /// its qualifiers, at any level of a pointer: `CK_NoOp`, as in
    /// `(const char *)(char *)&x`), converts an integer to another
    /// integer type (`CK_IntegralCast`, not to `_Bool`) or makes a null
    /// pointer constant a pointer (`CK_NullToPointer`, [`null_to_pointer`]:
    /// `int *p = (void *)0;`, `(int *)(long)0`), and into the operand of a
    /// unary `-`; it gives up on anything else (`+`, `~`, a
    /// binary operator, `?:`, `&`, an address converted to an integer). So
    /// of `long a = (long)&x;` and `long n = -(T)(U)3;` the types are kept,
    /// of `long b = (long)&x + 4;` and `int *p = (int *)(T)&x;`'s `T` not.
    fn initializer_casts(&mut self, e: &Expr) {
        if self.debug.is_none() {
            return;
        }
        match &e.kind {
            ExprKind::Cast { x, implicit, spelled } => {
                if !implicit {
                    self.debug.as_mut().expect("-g").cast(&e.ty, spelled);
                }
                let (from, to) = (x.ty.unqual(), e.ty.unqual());
                let integral = from.is_integer() && to.is_integer() && *to != Type::Int(IntKind::Bool);
                if from.similar(to) || integral || null_to_pointer(e, self.xlen) {
                    self.initializer_casts(x);
                }
            }
            ExprKind::Unary(UnOp::Neg, x) => self.initializer_casts(x),
            _ => {}
        }
    }

    /// Emits every string literal, NUL-terminated, in `.rodata`.
    fn strings(&mut self) {
        if self.strings.is_empty() {
            return;
        }
        self.switch(".rodata");
        for (bytes, label) in std::mem::take(&mut self.strings) {
            let _ = writeln!(self.out, "\n{label}:");
            for line in byte_directives(&bytes) {
                let _ = writeln!(self.out, "    {line}");
            }
        }
    }
}

/// Bytes as data directives: text as `.ascii "..."` (or `.asciz` when it ends
/// with the NUL), anything else as `.byte` lists. Only escapes that every
/// assembler reads the same are used (`\n \t \" \\`).
fn byte_directives(bytes: &[u8]) -> Vec<String> {
    let printable = |b: u8| (0x20..0x7f).contains(&b) || b == b'\n' || b == b'\t';
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let start = i;
        if printable(bytes[i]) {
            while i < bytes.len() && printable(bytes[i]) {
                i += 1;
            }
            let mut text = String::new();
            for &b in &bytes[start..i] {
                match b {
                    b'"' => text.push_str("\\\""),
                    b'\\' => text.push_str("\\\\"),
                    b'\n' => text.push_str("\\n"),
                    b'\t' => text.push_str("\\t"),
                    _ => text.push(b as char),
                }
            }
            // A NUL right after the text: one `.asciz` says both.
            if i + 1 == bytes.len() && bytes[i] == 0 {
                out.push(format!(".asciz \"{text}\""));
                i += 1;
            } else {
                out.push(format!(".ascii \"{text}\""));
            }
        } else {
            while i < bytes.len() && !printable(bytes[i]) && i - start < 16 {
                i += 1;
            }
            let list: Vec<String> = bytes[start..i].iter().map(u8::to_string).collect();
            out.push(format!(".byte {}", list.join(", ")));
        }
    }
    out
}

/// One line of a function's assembly.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Line {
    /// `name:`
    Label(String),
    /// An instruction, already formatted, and the most bytes it can assemble
    /// to (pseudo-instructions like `li`, `la` and `call` can take two words).
    Insn(String, u32),
    /// A conditional jump, `op regs, target` (`beqz a0, .L.end.3`, `beq a0,
    /// t0, .L.case.f.2`), whose reach is limited: see [`relax_branches`].
    Branch {
        op: &'static str,
        regs: &'static str,
        target: String,
    },
    /// `# text`, or a directive such as `.globl f`: no bytes.
    Text(String),
}

/// Where the target of an assignment is, once its address is known.
enum Place {
    /// A scalar local, at this offset from `s0`.
    Frame(i32),
    /// Anywhere else: its address is the top temporary on the stack.
    Pushed,
}

/// Whether clang gives objects of types `a` and `b` one LLVM type in memory
/// (`ConvertTypeForMem`): integers of one size (`iN`, whatever their
/// signedness, `_Bool` an `i8`), any two pointers (`ptr`), arrays of as many
/// such elements (`[0 x T]` for an unknown size), the same struct or union
/// (a named type of its own).
fn same_llvm_type(a: &Type, b: &Type, x: Xlen) -> bool {
    match (a.unqual(), b.unqual()) {
        (Type::Int(k), Type::Int(l)) => k.size(x) == l.size(x),
        (Type::Ptr(_), Type::Ptr(_)) => true,
        (Type::Array(e, n), Type::Array(f, m)) => n.unwrap_or(0) == m.unwrap_or(0) && same_llvm_type(e, f, x),
        (Type::Record(r), Type::Record(s)) => Arc::ptr_eq(r, s),
        _ => false,
    }
}

/// The member whose type a union's LLVM type holds (then padding to the
/// union's size): the most aligned, of those the largest, of those the
/// first (`CGRecordLowering::lowerUnion`).
fn union_storage(body: &RecordBody, x: Xlen) -> &Member {
    let key = |m: &Member| (m.ty.align(x), m.ty.size(x).unwrap_or(0));
    let mut best = &body.members[0];
    for m in &body.members[1..] {
        if key(m) > key(best) {
            best = m;
        }
    }
    best
}

/// Whether a struct has padding: a member that does not start where the
/// one before ends, or bytes after the last.
fn padded(body: &RecordBody, x: Xlen) -> bool {
    let mut end = 0;
    for m in &body.members {
        if m.offset != end {
            return true;
        }
        end = m.offset + m.ty.size(x).unwrap_or(0);
    }
    end != body.size
}

/// The type of the first reference to global `sym` in `e`, the variable as
/// it was declared there (an expression the symbol of a static initializer
/// came from, [`consteval::eval`]'s `Base::Sym`).
fn referred_type<'e>(e: &'e Expr, sym: &str) -> Option<&'e Type> {
    let sub = |x: &'e Expr| referred_type(x, sym);
    match &e.kind {
        ExprKind::Var {
            var: Some(VarRef::Global(s)),
            ..
        } if s == sym => Some(&e.ty),
        ExprKind::Addr(x)
        | ExprKind::Deref(x)
        | ExprKind::Unary(_, x)
        | ExprKind::Cast { x, .. }
        | ExprKind::Member { base: x, .. } => sub(x),
        ExprKind::Binary(_, l, r) | ExprKind::Comma(l, r) => sub(l).or_else(|| sub(r)),
        ExprKind::Cond(c, t, f) => sub(c).or_else(|| sub(t)).or_else(|| sub(f)),
        _ => None,
    }
}

/// Whether symbol key `key` is a C name (a function or file-scope
/// variable), not one sema makes up: those have a `.` or a `#`.
fn is_c_name(key: &str) -> bool {
    !key.contains(['.', '#'])
}

/// Whether `s` is visited even where control cannot reach it: clang's
/// `EmitSimpleStmt` statements, which `EmitStmt` handles before it asks
/// whether there is an insertion point. A block's statements are each
/// asked in turn; a declaration still gives its locals their slots and
/// emits its static locals; a label or `case` makes the code after it
/// reachable; a jump does nothing there.
fn always_visited(s: &Stmt) -> bool {
    matches!(
        s.kind,
        StmtKind::Block(_)
            | StmtKind::Decl(_)
            | StmtKind::Empty
            | StmtKind::Unevaluated(_)
            | StmtKind::Label { .. }
            | StmtKind::Goto(_)
            | StmtKind::Break
            | StmtKind::Continue
            | StmtKind::Case { .. }
            | StmtKind::Default { .. }
    )
}

/// Per-function code generator state. `'a` is the tree's lifetime, `'f` the
/// (shorter) borrow of the file's state.
struct FuncGen<'a, 'f> {
    /// The target.
    xlen: Xlen,
    func: &'a Function,
    /// Offset from `s0` of each local.
    offsets: Vec<i32>,
    frame: i32,
    /// How many argument words the named parameters take (see [`abi`]).
    named_words: usize,
    /// In a variadic function, the bytes of argument registers saved at the
    /// top of the frame (those the named parameters leave free); else 0.
    va_save: i32,
    lines: Vec<Line>,
    /// How many 4-byte temporaries expression evaluation has pushed right now.
    /// Always 0 between statements.
    depth: u32,
    /// Where `break` goes for each enclosing loop or `switch`, and where
    /// `continue` goes for each enclosing loop, innermost last.
    breaks: Vec<String>,
    continues: Vec<String>,
    file: &'f mut FileGen<'a>,
    /// Every `return` jumps here, to the epilogue.
    ret_label: String,
    /// The ids of the `case`s and `default`s of each `switch` on a constant
    /// generated so far, whose labels nothing jumps to.
    elided_cases: Vec<usize>,
    /// With `-g`: a `.loc` has been written since the function's first, so
    /// the next is not the one that ends the prologue.
    after_prologue: bool,
    /// With `-g`: the scopes being generated, the function's body first and
    /// the innermost block last, with the variables declared in each.
    scopes: Vec<Scope>,
    /// With `-g`: a local's constant initializer is being generated, which
    /// clang emits as a constant ([`FuncGen::constant_initializer`]): its
    /// code keeps no casts' types and describes no enums.
    constant_init: bool,
    /// The locals the code generated so far declares or uses (the
    /// parameters from the start).
    used: Vec<Cell<bool>>,
    /// The locals the frame has slots for: those the code uses.
    slots: Vec<bool>,
    /// Whether control can reach the code being generated: clang's
    /// insertion point (`HaveInsertPoint`), which a jump takes away and a
    /// label gives back ("Unreachable code" above).
    reachable: bool,
    /// The labels the jumps and branches generated so far go to: those
    /// control reaches other than by falling into them (clang's blocks
    /// that have uses).
    reached: HashSet<String>,
}

impl<'a> FuncGen<'a, '_> {
    // ---- emitting lines ----

    /// Emits a one-word instruction: `mnemonic operands`.
    fn ins(&mut self, mnemonic: &str, operands: impl AsRef<str>) {
        self.ins_sized(4, mnemonic, operands);
    }

    fn ins_sized(&mut self, size: u32, mnemonic: &str, operands: impl AsRef<str>) {
        self.lines
            .push(Line::Insn(insn_text(mnemonic, operands.as_ref()), size));
    }

    /// `li reg, value`: one instruction for small values, two (`lui` and
    /// `addi`, `addiw` on RV64) for others that fit 32 bits. A 64-bit value
    /// takes up to 8 instructions (LLVM's RISCVMatInt, which merlin-as
    /// follows), and [`relax_branches`] counts that many.
    fn li(&mut self, reg: &str, value: i64) {
        let size = if fits_imm12(value) {
            4
        } else if i32::try_from(value).is_ok() {
            8
        } else {
            32
        };
        self.ins_sized(size, "li", format!("{reg}, {value}"));
    }

    /// `la reg, symbol`: `auipc` + `addi`, for the symbol of key `symbol`
    /// (a function or variable, declared there with type `ty`).
    fn la(&mut self, reg: &str, symbol: &str, ty: &Type) {
        self.file.reference(symbol, Some(ty));
        let name = self.file.name(symbol).to_string();
        self.la_label(reg, &name);
    }

    /// `la reg, label`, of a label that is no symbol of the module's (a
    /// string literal's).
    fn la_label(&mut self, reg: &str, label: &str) {
        self.ins_sized(8, "la", format!("{reg}, {label}"));
    }

    fn comment(&mut self, text: impl AsRef<str>) {
        self.lines.push(Line::Text(format!("# {}", text.as_ref())));
    }

    fn label(&mut self, name: &str) {
        self.lines.push(Line::Label(name.to_string()));
    }

    fn branch(&mut self, op: &'static str, regs: &'static str, target: &str) {
        self.reached.insert(target.to_string());
        self.lines.push(Line::Branch {
            op,
            regs,
            target: target.to_string(),
        });
    }

    fn branch_if_zero(&mut self, target: &str) {
        self.branch("beqz", "a0", target);
    }

    fn branch_if_nonzero(&mut self, target: &str) {
        self.branch("bnez", "a0", target);
    }

    /// A statement's jump to `target` (`return`, `break`, `continue`,
    /// `goto`, the end of an `if`'s arm, a loop's way back), as clang's
    /// `EmitBranch`: generated only from code control reaches, and nothing
    /// after it is reached until a label is.
    fn jump(&mut self, target: &str) {
        if self.reachable {
            self.reached.insert(target.to_string());
            self.ins("j", target);
        }
        self.reachable = false;
    }

    /// A label code is generated after whether or not anything reaches it
    /// (a `goto` label, a `case`, a loop's top): clang's
    /// `EmitBlock(block)`, which makes the block the insertion point.
    fn block(&mut self, name: &str) {
        self.label(name);
        self.reachable = true;
    }

    /// The label where the paths of a statement meet (after an `if`, a
    /// loop, a `switch`): clang's `EmitBlock(block, IsFinished = true)`,
    /// which deletes a block nothing falls or jumps into, so the code after
    /// it is reached only if control falls in or a jump generated so far
    /// goes there.
    fn join(&mut self, name: &str) {
        self.reachable |= self.reached.contains(name);
        if self.reachable {
            self.label(name);
        }
    }

    /// A fresh label name, `.L.<what>.<n>`.
    fn new_label(&mut self, what: &str) -> String {
        self.file.labels += 1;
        format!(".L.{what}.{}", self.file.labels)
    }

    /// `rd = rs + imm` for any `imm`; large ones go through `t0`.
    fn add_imm(&mut self, rd: &str, rs: &str, imm: i32) {
        if fits_imm12(imm) {
            self.ins("addi", format!("{rd}, {rs}, {imm}"));
        } else {
            self.li("t0", imm.into());
            self.ins("add", format!("{rd}, {rs}, t0"));
        }
    }

    /// A load or store (`op`) of `reg` at `offset(s0)`. Offsets beyond 12 bits
    /// (frames over 2 KiB) compute the address in `t0` first.
    fn access_local(&mut self, op: &str, reg: &str, offset: i32) {
        if fits_imm12(offset) {
            self.ins(op, format!("{reg}, {offset}(s0)"));
        } else {
            self.add_imm("t0", "s0", offset);
            self.ins(op, format!("{reg}, 0(t0)"));
        }
    }

    fn offset(&self, id: LocalId) -> i32 {
        self.used[id.0].set(true);
        self.offsets[id.0]
    }

    /// How a register holds the value `v` of type `k`: its bits, widened to
    /// the register as [`Self::convert`] keeps values. A 32-bit type is
    /// sign-extended, `unsigned int` too on RV64 (the 32-bit invariant), so
    /// an `unsigned` 0xffffffff is -1; a narrower one holds its value.
    fn image(&self, v: i128, k: IntKind) -> i64 {
        let v = k.wrap(v, self.xlen);
        match k.size(self.xlen) {
            4 => v as i32 as i64,
            _ => v as i64,
        }
    }

    /// Whether a value of type `ty` is a register pair: a `long long` on
    /// RV32, its low word in `a0` and its high word in `a1` (see "Register
    /// pairs").
    fn is_pair(&self, ty: &Type) -> bool {
        self.xlen == Xlen::Rv32 && ty.int_kind().is_some_and(|k| k.size(self.xlen) == 8)
    }

    /// `a0` (and `a1`) = the constant `v` of type `k`.
    fn constant(&mut self, v: i128, k: IntKind) {
        if self.is_pair(&Type::Int(k)) {
            let (lo, hi) = pair_image(k.wrap(v, self.xlen));
            self.li("a0", lo.into());
            self.li("a1", hi.into());
        } else {
            self.li("a0", self.image(v, k));
        }
    }

    /// Evaluates a condition into `a0`, nonzero when it holds: a pair is
    /// nonzero when either word is (`or a0, a0, a1`).
    fn test(&mut self, e: &'a Expr) {
        self.expr(e);
        if self.is_pair(&e.ty) {
            self.ins("or", "a0, a0, a1");
        }
    }

    /// Pushes the value of type `ty`: `a0`, or a pair's two words (the low
    /// one at the lower address, as in memory).
    fn push_value(&mut self, ty: &Type) {
        if self.is_pair(ty) {
            self.ins("addi", "sp, sp, -8");
            self.ins("sw", "a0, 0(sp)");
            self.ins("sw", "a1, 4(sp)");
            self.depth += 2;
        } else {
            self.push();
        }
    }

    /// Pops a pushed pair into `lo` and `hi`.
    fn pop_pair(&mut self, lo: &str, hi: &str) {
        self.ins("lw", format!("{lo}, 0(sp)"));
        self.ins("lw", format!("{hi}, 4(sp)"));
        self.ins("addi", "sp, sp, 8");
        self.depth -= 2;
    }

    /// Pops the left operand of `l op r`, of type `l`: into `a1`, or a pair
    /// into `a2` and `a3` (the right one is in `a0`, or `a0:a1`). An operand
    /// the operation widens to a pair goes to `a2`, so as not to clobber the
    /// right one's high word ([`Self::binary`] extends it into `a3`); a
    /// shift uses only its amount's low word.
    fn pop_left(&mut self, op: BinOp, l: &Type, r: &Type) {
        if self.is_pair(l) {
            self.pop_pair("a2", "a3");
        } else if self.is_pair(r) && l.is_integer() && !matches!(op, BinOp::Shl | BinOp::Shr) {
            self.pop("a2");
        } else {
            self.pop("a1");
        }
    }

    /// `a0:a1 += delta` (a `++` or `--` of a long long on RV32).
    fn pair_step(&mut self, delta: i32) {
        self.li("a2", delta.into());
        self.li("a3", if delta < 0 { -1 } else { 0 });
        self.pair_arith(BinOp::Add, false);
    }

    /// Whether arithmetic in the (promoted) type `ty` is 32-bit arithmetic
    /// on 64-bit registers: an `int` or `unsigned int` on RV64, which the W
    /// instructions do (`addw`, `sllw`, `divuw`...), each leaving its result
    /// sign-extended.
    fn is_w(&self, ty: &Type) -> bool {
        self.xlen == Xlen::Rv64 && ty.int_kind().is_some_and(|k| k.size(self.xlen) == 4)
    }

    // ---- the stack of temporaries ----

    /// The bytes of a register: 4 on RV32, 8 on RV64.
    fn word(&self) -> i32 {
        word(self.xlen)
    }

    /// The load of a whole register, `lw` or `ld`.
    fn load_reg(&self) -> &'static str {
        word_ops(self.xlen).0
    }

    /// The store of a whole register, `sw` or `sd`.
    fn store_reg(&self) -> &'static str {
        word_ops(self.xlen).1
    }

    /// Pushes `a0`: a whole register.
    fn push(&mut self) {
        let w = self.word();
        self.ins("addi", format!("sp, sp, {}", -w));
        self.ins(self.store_reg(), "a0, 0(sp)");
        self.depth += 1;
    }

    /// Pops the top temporary into `reg`.
    fn pop(&mut self, reg: &str) {
        let w = self.word();
        self.ins(self.load_reg(), format!("{reg}, 0(sp)"));
        self.ins("addi", format!("sp, sp, {w}"));
        self.depth -= 1;
    }

    // ---- functions ----

    fn function(&mut self, body: &'a [Stmt]) {
        let f = self.func;

        // A header that maps every local to its slot, for whoever reads the output.
        let mut params: Vec<String> = f.params.iter().map(|p| p.ty.declare(&p.name)).collect();
        if f.variadic {
            params.push("...".to_string());
        }
        let params = if params.is_empty() {
            "void".to_string()
        } else {
            params.join(", ")
        };
        let frame = self.frame;
        self.comment(format!(
            "{}: frame {frame} bytes",
            f.ret.declare(&format!("{}({params})", f.name))
        ));
        if self.va_save > 0 {
            let regs = match self.named_words {
                7 => "a7".to_string(),
                n => format!("a{n}-a7"),
            };
            self.comment(format!("    {}(s0)  {regs}, saved for va_arg", -self.va_save));
        }
        for (i, l) in f.locals.iter().enumerate() {
            // (The frame has no slot for a local the code does not use.)
            if !self.slots[i] {
                continue;
            }
            let off = self.offsets[i];
            let what = if i < f.params.len() {
                format!("param {}", l.ty.declare(&l.name))
            } else if l.temp {
                // A hidden local for a call or a return (sema's names say what).
                format!("temp {} ({})", l.ty, l.name)
            } else {
                format!("local {}", l.ty.declare(&l.name))
            };
            self.comment(format!("  {off:>5}(s0)  {what}"));
        }
        // An inline definition (C99 `inline` without `extern`) provides no
        // external definition: merlin keeps it local to the file, like a
        // `static` function.
        if !f.is_static && !f.inline_only {
            self.lines.push(Line::Text(format!(".globl {}", f.name)));
        }
        // A function symbol with its size, as clang marks them, so that
        // merlin-objdump, Grail's traces and tests can tell which function an
        // address is in (the `.size` follows the epilogue).
        self.lines
            .push(Line::Text(format!(".type {}, @function", f.name)));
        self.label(&f.name);
        // `-g`: the prologue comes from the function's first line (column 0, as
        // clang has it).
        if let Some(d) = self.file.debug.as_mut() {
            self.lines.push(Line::Text(d.loc(f.pos, Some(0), false)));
            self.scopes.push(Scope::default());
        }

        // Prologue: save ra and the caller's s0, set up s0, make room for locals.
        let (top, va_save, w, store) = (self.top(), self.va_save, self.word(), self.store_reg());
        self.add_imm("sp", "sp", -top);
        self.ins(store, format!("ra, {}(sp)", top - va_save - w));
        self.ins(store, format!("s0, {}(sp)", top - va_save - 2 * w));
        self.add_imm("s0", "sp", top);
        if frame > top {
            self.add_imm("sp", "sp", -(frame - top));
        }
        // A variadic function stores the argument registers its named
        // parameters leave free right below s0, where the caller's stack
        // words continue them: every variadic argument is then in one run
        // of words from there up, which `va_arg` walks.
        if f.variadic && self.named_words < abi::ARG_REGS {
            self.comment("the variadic arguments in registers, next to those on the stack");
            for r in self.named_words..abi::ARG_REGS {
                let off = -w * (abi::ARG_REGS - r) as i32;
                self.ins(store, format!("a{r}, {off}(s0)"));
            }
        }
        // Parameters arrive in a0-a7 and then on the stack, from 0(s0) up
        // (see [`abi`]); give them their slots like any local. A big struct
        // result's address comes first, in a0.
        let Type::Func(ft) = f.ty() else {
            unreachable!("a function's type")
        };
        if let Some(ret_ptr) = f.ret_ptr {
            let off = self.offset(ret_ptr);
            self.access_local(store, "a0", off);
        }
        let (words, _) = abi::arguments(&ft, self.xlen);
        for (i, (p, (word, pass))) in f.params.iter().zip(words).enumerate() {
            let off = self.offsets[i];
            match pass {
                Pass::Scalar => {
                    let reg = self.incoming(word);
                    self.access_local(store_op(&p.ty, self.xlen), &reg, off);
                }
                // A small struct: its words, as they lie in memory (the slot is
                // a whole number of words); a long long on RV32: its low word,
                // then its high word.
                Pass::Words(_) | Pass::Pair => {
                    for i in 0..pass.registers() {
                        let reg = self.incoming(word + i);
                        self.access_local(store, &reg, off + w * i as i32);
                    }
                }
                // A big struct: the caller's copy, at the address in the
                // register, is copied into the slot. (Changing the caller's
                // copy in place would do too; a copy keeps every parameter
                // in the frame.)
                Pass::Ref => {
                    let src = match arg_word(word, self.xlen) {
                        ArgWord::Reg(r) => r,
                        // Every register argument is in its slot by now, so
                        // a0 is free.
                        ArgWord::Stack(at) => {
                            self.ins(self.load_reg(), format!("a0, {at}(s0)"));
                            "a0".to_string()
                        }
                    };
                    self.add_imm("t4", "s0", off);
                    self.copy("t4", &src, &p.ty);
                }
            }
        }

        for s in body {
            self.stmt(s);
        }

        // Falling off the end returns 0. A final `return` needs no jump to the
        // epilogue, which comes right after it. Either way what follows comes
        // from the closing brace.
        // (`-g`'s labels at the end of a block are not code: the jump may be
        // just before them.)
        let last = self
            .lines
            .iter()
            .rposition(|l| !matches!(l, Line::Label(n) if n.starts_with(".Ldbg.")));
        let returned = last.is_some_and(|i| self.lines[i] == Line::Insn(insn_text("j", &self.ret_label), 4));
        if let (true, Some(i)) = (returned, last) {
            self.lines.remove(i);
        }
        self.loc(f.end);
        // (Only if control reaches the end: not after a `return`, nor
        // after a loop no `break` leaves.)
        if self.reachable {
            self.li("a0", 0);
            if self.is_pair(&f.ret) {
                self.li("a1", 0);
            }
        }

        // Epilogue: the result is in a0. Free the frame from s0 (whatever its
        // size), restore ra and s0, return.
        let ret = self.ret_label.clone();
        self.label(&ret);
        let load = self.load_reg();
        self.ins("addi", format!("sp, s0, {}", -top));
        self.ins(load, format!("ra, {}(sp)", top - va_save - w));
        self.ins(load, format!("s0, {}(sp)", top - va_save - 2 * w));
        self.ins("addi", format!("sp, sp, {top}"));
        self.ins("ret", "");
        if self.file.debug.is_some() {
            let end = format!(".Lfunc_end.{}", f.name);
            let params = self.debug_params();
            let body = self.scopes.pop().expect("the function's scope");
            let d = self.file.debug.as_mut().expect("-g");
            let ret = (&f.ret, &f.ret_spelled);
            let external = !f.is_static && !f.inline_only;
            let flags = (external, f.prototyped);
            d.function(
                &f.name,
                &end,
                (f.pos, f.end),
                flags,
                ret,
                params,
                f.variadic,
                body,
            );
            self.lines.push(Line::Label(end));
        }
        self.lines.push(Line::Text(format!(".size {0}, . - {0}", f.name)));
    }

    /// `-g`: the next instruction comes from `pos` (a `.loc`, see
    /// [`crate::debug`]). The first one after the function's own says that
    /// the prologue ends there.
    fn loc(&mut self, pos: crate::Pos) {
        let Some(d) = self.file.debug.as_mut() else {
            return;
        };
        if pos.line == 0 {
            return;
        }
        let first = !self.after_prologue;
        self.after_prologue = true;
        self.lines.push(Line::Text(d.loc(pos, None, first)));
    }

    /// The top of the frame, which the prologue sets up first: `ra`, `s0`
    /// and a variadic function's saved registers, rounded up to 16 bytes so
    /// that `sp` stays aligned at every step. 16 for most functions.
    fn top(&self) -> i32 {
        (saved_regs(self.xlen) + self.va_save + 15) & !15
    }

    /// In the prologue: the register holding argument word `word`. A word
    /// on the stack is loaded into `t1` first.
    fn incoming(&mut self, word: usize) -> String {
        match arg_word(word, self.xlen) {
            ArgWord::Reg(r) => r,
            ArgWord::Stack(at) => {
                self.ins(self.load_reg(), format!("t1, {at}(s0)"));
                "t1".to_string()
            }
        }
    }

    /// `-g`: the parameters, each in its slot.
    fn debug_params(&mut self) -> Vec<Var> {
        let f = self.func;
        let Some(d) = self.file.debug.as_mut() else {
            return Vec::new();
        };
        f.params
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let at = Location::Frame(self.offsets[i]);
                d.var(&p.name, &p.ty, &p.spelled, p.pos, at)
            })
            .collect()
    }

    /// `-g`: the variable `decl` declares, in the innermost scope: a local in
    /// its slot, a `static` local at its symbol. (A block's `extern`
    /// declaration defines nothing.)
    fn debug_decl(&mut self, decl: &Declarator) {
        if self.file.debug.is_none() {
            return;
        }
        let at = match &decl.var {
            Some(VarRef::Local(id)) => Location::Frame(self.offsets[id.0]),
            Some(VarRef::Global(sym)) if decl.storage == Storage::Static => {
                Location::Symbol(self.file.name(sym).to_string())
            }
            _ => return,
        };
        let d = self.file.debug.as_mut().expect("-g");
        let v = d.var(&decl.name, &decl.ty, &decl.spelled, decl.pos, at);
        d.declared(&v);
        self.scopes.last_mut().expect("a scope").push(v);
    }

    // ---- statements ----

    /// A statement. With `-g`, a block, and a `for` that declares its
    /// variable, is a scope of its own, with labels around its code.
    fn stmt(&mut self, s: &'a Stmt) {
        // Code control cannot reach is not generated, as clang's `EmitStmt`
        // leaves it out ("Unreachable code" above): a statement but those
        // it always visits (a block, a declaration, a label, a jump, a
        // `case`), unless a label in it can be jumped to, which makes it
        // reachable from its start (`EnsureInsertPoint`).
        if !self.reachable && !always_visited(s) {
            if !elide::contains_label(s, false) {
                return;
            }
            self.reachable = true;
        }
        let scoped = match &s.kind {
            StmtKind::Block(_) => true,
            StmtKind::For { init: Some(init), .. } => matches!(init.kind, StmtKind::Decl(_)),
            _ => false,
        };
        let Some(d) = self.file.debug.as_mut().filter(|_| scoped) else {
            return self.unscoped_stmt(s);
        };
        let (begin, end) = d.block_labels();
        self.label(&begin);
        self.scopes.push(Scope::new(begin, end.clone()));
        self.unscoped_stmt(s);
        self.label(&end);
        let inner = self.scopes.pop().expect("the block's scope");
        self.scopes
            .last_mut()
            .expect("the enclosing scope")
            .add_block(inner);
    }

    fn unscoped_stmt(&mut self, s: &'a Stmt) {
        debug_assert_eq!(self.depth, 0, "temporaries left on the stack between statements");
        // `-g`: a statement with code of its own starts a line. A loop's parts
        // and a declaration's initializers get theirs where they are made, an
        // `if`'s and a `switch`'s where their condition is tested.
        match &s.kind {
            StmtKind::Block(_)
            | StmtKind::Decl(_)
            | StmtKind::Empty
            | StmtKind::Unevaluated(_)
            | StmtKind::If { .. }
            | StmtKind::While { .. }
            | StmtKind::DoWhile { .. }
            | StmtKind::For { .. }
            | StmtKind::Switch { .. }
            | StmtKind::Case { .. }
            | StmtKind::Default { .. }
            | StmtKind::Label { .. } => {}
            // (A jump control cannot reach is no code: clang's `EmitBreakStmt`,
            // `EmitGotoStmt` give a location only with an insertion point.)
            _ if self.reachable => self.loc(s.pos),
            _ => {}
        }
        match &s.kind {
            StmtKind::Block(body) => {
                for s in body {
                    self.stmt(s);
                }
            }
            StmtKind::Decl(decls) => {
                // Each declarator described, then initialized, as clang does;
                // a static local's data first, then its entry
                // (`EmitStaticVarDecl`).
                for d in decls {
                    // A local the code declares has a slot, used or not.
                    if let Some(VarRef::Local(id)) = d.var {
                        self.used[id.0].set(true);
                    }
                    if matches!(d.var, Some(VarRef::Global(_))) {
                        self.local_decl(d);
                        self.debug_decl(d);
                        continue;
                    }
                    // A local declared where control cannot reach has its
                    // slot, but no initialization and no description: clang
                    // allocates it (`EmitAutoVarAlloca`), but describes it and
                    // emits its initializer only at an insertion point.
                    if !self.reachable {
                        continue;
                    }
                    self.debug_decl(d);
                    if d.init.is_some() && matches!(d.var, Some(VarRef::Local(_))) {
                        self.loc(d.pos);
                    }
                    self.local_decl(d);
                }
            }
            StmtKind::Expr(e) => self.expr(e),
            // Only checked: nothing is evaluated.
            StmtKind::Empty | StmtKind::Unevaluated(_) => {}
            StmtKind::If { cond, then, els } => {
                // A constant condition runs one arm: only that one is
                // generated, unless the other has a label (see [`elide`]).
                if let Some(c) = self.constant_condition(cond) {
                    let (run, skipped) = if c {
                        (Some(&**then), els.as_deref())
                    } else {
                        (els.as_deref(), Some(&**then))
                    };
                    if !skipped.is_some_and(|s| elide::contains_label(s, false)) {
                        if let Some(run) = run {
                            self.stmt(run);
                        }
                        return;
                    }
                }
                //     <cond>                    <cond>
                //     beqz a0, .L.end.N         beqz a0, .L.else.N
                //     <then>                    <then>
                //   .L.end.N:                   j    .L.end.N
                //                             .L.else.N:
                //                               <els>
                //                             .L.end.N:
                // (Control reaches the end if an arm falls off its end, or,
                // with no `else`, when the test fails.)
                self.loc(s.pos);
                self.test(cond);
                match els {
                    None => {
                        let end = self.new_label("end");
                        self.branch_if_zero(&end);
                        self.stmt(then);
                        self.join(&end);
                    }
                    Some(els) => {
                        let else_ = self.new_label("else");
                        let end = self.new_label("end");
                        self.branch_if_zero(&else_);
                        self.stmt(then);
                        self.jump(&end);
                        self.block(&else_);
                        self.stmt(els);
                        self.join(&end);
                    }
                }
            }
            StmtKind::While { cond, body } => {
                //   .L.continue.N:      (`continue` re-tests the condition)
                //     <cond>
                //     beqz a0, .L.break.N
                //     <body>
                //     j    .L.continue.N
                //   .L.break.N:
                // (The code after the loop is reached when the test fails or
                // a `break` control reaches leaves: not after a `while (1)`
                // without one.)
                let cont = self.new_label("continue");
                let brk = self.new_label("break");
                self.block(&cont);
                if self.constant_condition(cond) == Some(true) {
                    // `while (1)`: nothing to test, as clang has it. Its
                    // code generator still visits the condition (`-g`).
                    self.kept_casts(cond);
                } else {
                    self.loc(cond.pos);
                    self.test(cond);
                    self.branch_if_zero(&brk);
                }
                self.loop_body(body, &cont, &brk);
                self.jump(&cont);
                self.join(&brk);
            }
            StmtKind::DoWhile { body, cond } => {
                //   .L.do.N:
                //     <body>
                //   .L.continue.N:      (`continue` tests the condition)
                //     <cond>
                //     bnez a0, .L.do.N
                //   .L.break.N:
                //
                // The condition is generated even when neither the body's end
                // nor a `continue` reaches it, as clang emits its block
                // (`EmitDoStmt`), so the code after the loop is too.
                let top = self.new_label("do");
                let cont = self.new_label("continue");
                let brk = self.new_label("break");
                self.block(&top);
                self.loop_body(body, &cont, &brk);
                self.block(&cont);
                if self.constant_condition(cond) == Some(false) {
                    // `do ... while (0)`: nothing to test, as clang has it.
                    self.kept_casts(cond);
                } else {
                    self.loc(cond.pos);
                    self.test(cond);
                    self.branch_if_nonzero(&top);
                }
                self.join(&brk);
            }
            StmtKind::For {
                init,
                cond,
                step,
                body,
                ..
            } => {
                //     <init>
                //   .L.loop.N:
                //     <cond>            (no condition: loop forever)
                //     beqz a0, .L.break.N
                //     <body>
                //   .L.continue.N:      (`continue` still runs the step)
                //     <step>
                //     j    .L.loop.N
                //   .L.break.N:
                if let Some(init) = init {
                    self.stmt(init);
                }
                //
                // The step is generated even when neither the body's end nor
                // a `continue` reaches it, as clang emits its block
                // (`EmitForStmt`); the code after the loop is reached when the
                // test fails or a `break` control reaches leaves.
                let top = self.new_label("loop");
                let cont = self.new_label("continue");
                let brk = self.new_label("break");
                self.block(&top);
                if let Some(cond) = cond {
                    self.loc(cond.pos);
                    self.test(cond);
                    self.branch_if_zero(&brk);
                }
                self.loop_body(body, &cont, &brk);
                if let Some(step) = step {
                    self.block(&cont);
                    self.loc(step.pos);
                    self.expr(step);
                } else {
                    self.join(&cont);
                }
                self.jump(&top);
                self.join(&brk);
            }
            StmtKind::Return(e) => {
                if let Some(e) = e {
                    self.expr(e);
                    if e.ty.is_record() {
                        self.return_record(&e.ty);
                    }
                }
                let ret = self.ret_label.clone();
                self.jump(&ret);
            }
            StmtKind::Break => {
                let brk = self
                    .breaks
                    .last()
                    .expect("sema: break in a loop or switch")
                    .clone();
                self.jump(&brk);
            }
            StmtKind::Continue => {
                let cont = self.continues.last().expect("sema: continue in a loop").clone();
                self.jump(&cont);
            }
            StmtKind::Switch {
                cond,
                body,
                cases,
                default,
            } => {
                // A chain of compares, one per `case`, in source order; then
                // `default` (or the end, when there is none). The body is
                // emitted as it is, its `case` and `default` labels where they
                // stand, so control falls through from one to the next as C
                // says:
                //
                //     <cond>                   (promoted: a 32-bit value)
                //     li   t0, 1
                //     beq  a0, t0, .L.case.f.0
                //     li   t0, 7
                //     beq  a0, t0, .L.case.f.1
                //     j    .L.case.f.2         (default; or .L.break.N)
                //     <body>                   (with .L.case.f.<id>: labels)
                //   .L.break.N:
                //
                // On a constant condition, only the statements that run, as
                // clang has them, when they can be picked out (see [`elide`]):
                // no compares, no labels, no `break`.
                if let Some(v) = consteval::condition(cond, self.xlen) {
                    if let Some(run) = elide::case_statements(body, v, cases, *default) {
                        self.elided_cases
                            .extend(cases.iter().map(|&(_, id)| id).chain(*default));
                        for s in run {
                            self.stmt(s);
                        }
                        return;
                    }
                }
                self.loc(s.pos);
                let brk = self.new_label("break");
                self.expr(cond);
                let kind = cond.ty.int_kind().expect("sema: an integer condition");
                let pair = self.is_pair(&cond.ty);
                for &(value, id) in cases {
                    let target = self.case_label(id);
                    if pair {
                        // A long long on RV32: equal when both words are.
                        let (lo, hi) = pair_image(value);
                        self.li("t0", lo.into());
                        self.li("t1", hi.into());
                        self.ins("xor", "t0, a0, t0");
                        self.ins("xor", "t1, a1, t1");
                        self.ins("or", "t0, t0, t1");
                        self.branch("beqz", "t0", &target);
                        continue;
                    }
                    // The value as the register holds it: an `unsigned`
                    // 0xffffffff is `li t0, -1`.
                    self.li("t0", self.image(value, kind));
                    self.branch("beq", "a0, t0", &target);
                }
                let otherwise = match default {
                    Some(id) => self.case_label(*id),
                    None => brk.clone(),
                };
                // Control enters the body only at its labels (clang clears
                // the insertion point before it): a statement before the
                // first `case` is not generated. The code after it is
                // reached without a `default`, by a `break` control reaches,
                // or falling off the body's end.
                self.jump(&otherwise);
                self.breaks.push(brk.clone());
                self.stmt(body);
                self.breaks.pop();
                self.join(&brk);
            }
            StmtKind::Case { id, body, .. } | StmtKind::Default { id, body } => {
                // (Nothing jumps to the label of a switch on a constant.)
                if !self.elided_cases.contains(id) {
                    let label = self.case_label(*id);
                    self.block(&label);
                    // `-g`: a case value that is an enumeration constant
                    // describes its enum, as clang's `EmitCaseStmt` does
                    // (`case RED:`, not `case RED + 1:` or `case (RED):`).
                    if let (Some(d), StmtKind::Case { expr, bare: true, .. }) =
                        (self.file.debug.as_mut(), &s.kind)
                    {
                        let mut e = expr;
                        while let ExprKind::Cast {
                            x, implicit: true, ..
                        } = &e.kind
                        {
                            e = x;
                        }
                        if let ExprKind::Enumerator { of, .. } = &e.kind {
                            d.enumerator(of);
                        }
                    }
                }
                self.stmt(body);
            }
            StmtKind::Label { name, body } => {
                let label = self.goto_label(name);
                self.block(&label);
                // `-g`: clang describes the label, in its block.
                if let Some(d) = self.file.debug.as_mut() {
                    let l = d.label(name, s.pos, &label);
                    self.scopes.last_mut().expect("a scope").push_label(l);
                }
                self.stmt(body);
            }
            StmtKind::Goto(name) => {
                let label = self.goto_label(name);
                self.jump(&label);
            }
        }
    }

    /// The value of a condition clang's code generator decides at compile
    /// time (`ConstantFoldsToSimpleInteger`; for loops, the `ConstantInt`
    /// the condition's code folds to): an integer constant expression, or
    /// one of addresses known not to be null or one another (`&g != 0`,
    /// [`consteval::condition`]).
    fn constant_condition(&self, e: &Expr) -> Option<bool> {
        consteval::condition(e, self.xlen).map(|v| v != 0)
    }

    fn loop_body(&mut self, body: &'a Stmt, cont: &str, brk: &str) {
        self.continues.push(cont.to_string());
        self.breaks.push(brk.to_string());
        self.stmt(body);
        self.continues.pop();
        self.breaks.pop();
    }

    /// The label of `case` or `default` number `id` (numbered by sema, per
    /// function).
    fn case_label(&self, id: usize) -> String {
        format!(".L.case.{}.{id}", self.func.name)
    }

    /// The label of the C label `name`: `.L.label.<function>.<name>`.
    fn goto_label(&self, name: &str) -> String {
        format!(".L.label.{}.{name}", self.func.name)
    }

    /// Returns the struct whose address is in `a0` from the function: in
    /// `a0` (and `a1`) when it is small, otherwise copied to where the caller
    /// asked, the address that came in `a0` (see [`abi`]).
    fn return_record(&mut self, ty: &Type) {
        match self.func.ret_ptr {
            Some(ret_ptr) => {
                let off = self.offset(ret_ptr);
                self.access_local(self.load_reg(), "a1", off);
                self.copy("a1", "a0", ty);
            }
            None => {
                let Pass::Words(n) = abi::classify(ty, self.xlen) else {
                    unreachable!("a big result has a result address")
                };
                self.ins("mv", "t1, a0");
                self.load_words("t1", 0, n, ty);
            }
        }
    }

    fn local_decl(&mut self, d: &'a Declarator) {
        let id = match &d.var {
            // A static local is data, emitted where it is declared, as clang
            // emits it.
            Some(VarRef::Global(_)) => return self.file.variable(d, true),
            Some(VarRef::Local(id)) => *id,
            None => unreachable!("sema resolves every declarator"),
        };
        // Without an initializer the local is left as it is (C: indeterminate).
        let Some(init) = &d.init else { return };
        let off = self.offset(id);
        // An array, or a struct given member by member: clear the whole
        // object, then store what the initializer gives.
        if d.ty.is_array() || (d.ty.is_record() && matches!(init, Init::List(..))) {
            self.comment(format!("{} = ...", d.ty.declare(&d.name)));
            self.clear_local(off, d.ty.size(self.xlen).unwrap() as i32);
        }
        // clang emits an array, struct or union with a constant initializer
        // as a static's (`EmitAutoVarInit`, its `IsConstantAggregate`:
        // `ConstantEmitter`), not as code: with `-g`, what its debug
        // information keeps of it is a static initializer's.
        let constant = (d.ty.is_array() || d.ty.is_record()) && self.constant_initializer(&d.ty, init);
        self.constant_init = constant;
        self.init_local(&d.ty, init, off);
        self.constant_init = false;
        // It copies the object from a constant of its own, which takes a
        // name after the string literals in it, unless it clears the object
        // and stores the few scalars that are not zero, or fills it with
        // one byte ([`ConstantImage::copied`]).
        if constant {
            let size = d.ty.size(self.xlen).expect("sema: locals have complete types");
            let mut image = ConstantImage::new(size);
            self.constant_image(&d.ty, init, 0, &mut image);
            if image.copied() {
                self.file
                    .take_name(&format!("__const.{}.{}", self.func.name, d.name));
            }
        }
    }

    /// The bytes and scalars of `init`, a constant initializer of an object
    /// of type `ty` at byte `at` of `image`, as clang's constant has them.
    fn constant_image(&self, ty: &Type, init: &Init, at: usize, image: &mut ConstantImage) {
        match (ty.unqual(), init) {
            (Type::Array(elem, _), Init::List(items, _)) => {
                let size = elem.size(self.xlen).expect("sema: locals have complete types") as usize;
                for (i, item) in items.iter().enumerate() {
                    self.constant_image(elem, item, at + i * size, image);
                }
            }
            // A union's list has empty items before the member it holds.
            (Type::Record(r), Init::List(items, _)) => {
                let body = r.body().expect("sema: locals have complete types");
                for (m, item) in body.members.iter().zip(items) {
                    self.constant_image(&m.ty, item, at + m.offset as usize, image);
                }
            }
            (_, Init::List(items, _)) => {
                for item in items {
                    self.constant_image(ty, item, at, image);
                }
            }
            (
                Type::Array(_, Some(n)),
                Init::Expr(Expr {
                    kind: ExprKind::Str(bytes),
                    ..
                }),
            ) => {
                // Each `char` a scalar of its own (LLVM's
                // `ConstantDataArray`).
                for (i, &b) in bytes.iter().take(*n as usize).enumerate() {
                    image.scalar(at + i, &b.to_le_bytes(), b != 0);
                }
            }
            (
                _,
                Init::Expr(Expr {
                    kind: ExprKind::CompoundLit { init, .. },
                    ty: lit,
                    ..
                }),
            ) => self.constant_image(lit, init, at, image),
            (_, Init::Expr(e)) => {
                let size = ty.size(self.xlen).expect("a scalar's size") as usize;
                match consteval::eval(e, self.xlen).expect("a constant initializer") {
                    Const::Int(v, _) => image.scalar(at, &v.to_le_bytes()[..size], v != 0),
                    Const::Addr { base: None, offset } => {
                        image.scalar(at, &offset.to_le_bytes()[..size], offset != 0)
                    }
                    // An address: no bytes are known. A pointer to a
                    // symbol itself is LLVM's global; with an offset, or
                    // converted to an integer, a constant expression.
                    Const::Addr {
                        base: Some(_),
                        offset,
                    } => image.address(at, size, ty.is_ptr() && offset == 0),
                }
            }
            (_, Init::Designated(..)) => unreachable!("sema resolves designators"),
        }
    }

    /// Whether `init` is a constant initializer of an object of type `ty`,
    /// as clang's `Expr::isConstantInitializer` has it: what a static
    /// initializer could be (numbers, addresses of globals and string
    /// literals, strings for a `char` array), every element of a list.
    fn constant_initializer(&self, ty: &Type, init: &Init) -> bool {
        match (ty.unqual(), init) {
            (Type::Array(elem, _), Init::List(items, _)) => {
                items.iter().all(|i| self.constant_initializer(elem, i))
            }
            (Type::Record(r), Init::List(items, _)) => {
                let body = r.body().expect("sema: locals have complete types");
                body.members
                    .iter()
                    .zip(items)
                    .all(|(m, i)| self.constant_initializer(&m.ty, i))
            }
            (_, Init::List(items, _)) => items.iter().all(|i| self.constant_initializer(ty, i)),
            (
                Type::Array(..),
                Init::Expr(Expr {
                    kind: ExprKind::Str(_),
                    ..
                }),
            ) => true,
            // A struct initialized by a compound literal, as GCC allows
            // even at file scope: constant if its list is.
            (
                _,
                Init::Expr(Expr {
                    kind: ExprKind::CompoundLit { init, .. },
                    ty: lit,
                    ..
                }),
            ) => self.constant_initializer(lit, init),
            (_, Init::Expr(e)) => consteval::eval(e, self.xlen).is_some(),
            (_, Init::Designated(..)) => unreachable!("sema resolves designators"),
        }
    }

    /// `-g`: clang's `ConstantEmitter` trying `init`, of an object of type
    /// `ty`, as a constant: each element in order, its casts kept as a
    /// static initializer's ([`FileGen::initializer_casts`]), up to the
    /// first that is not constant. Whether all are.
    fn constant_visit(&mut self, ty: &Type, init: &'a Init) -> bool {
        match (ty.unqual(), init) {
            (Type::Array(elem, _), Init::List(items, _)) => {
                items.iter().all(|i| self.constant_visit(elem, i))
            }
            (Type::Record(r), Init::List(items, _)) => {
                let body = r.body().expect("sema: locals have complete types");
                body.members
                    .iter()
                    .zip(items)
                    .all(|(m, i)| self.constant_visit(&m.ty, i))
            }
            (_, Init::List(items, _)) => items.iter().all(|i| self.constant_visit(ty, i)),
            (
                Type::Array(..),
                Init::Expr(Expr {
                    kind: ExprKind::Str(_),
                    ..
                }),
            ) => true,
            (
                _,
                Init::Expr(Expr {
                    kind: ExprKind::CompoundLit { init, .. },
                    ty: lit,
                    ..
                }),
            ) => self.constant_visit(lit, init),
            (_, Init::Expr(e)) => {
                self.file.initializer_casts(e);
                consteval::eval(e, self.xlen).is_some()
            }
            (_, Init::Designated(..)) => unreachable!("sema resolves designators"),
        }
    }

    /// Stores an initializer into the local object at `off(s0)`; arrays and
    /// structs initialized by a list have been cleared, so what the list
    /// leaves out is zero already.
    fn init_local(&mut self, ty: &Type, init: &'a Init, off: i32) {
        match (ty.unqual(), init) {
            // Zero: cleared already. (In a union, every item but the last
            // is one of these.)
            (_, Init::List(items, _)) if items.is_empty() => {}
            (Type::Array(elem, _), Init::List(items, _)) => {
                let size = elem.size(self.xlen).unwrap() as i32;
                // clang emits an array whose initializers hold more than 16
                // bytes as a constant if it can (`EmitArrayInit`'s
                // `ConstantEmitter`, which with `-g` visits them as a static
                // initializer's), copied from a global it names
                // `constinit` after the string literals in it; else element
                // by element.
                let was = self.constant_init;
                let whole = !was
                    && items.len() as i64 * i64::from(size) > 16
                    && items.iter().all(|i| self.constant_visit(elem, i));
                self.constant_init = was || whole;
                for (i, item) in items.iter().enumerate() {
                    self.init_local(elem, item, off + i as i32 * size);
                }
                self.constant_init = was;
                if whole {
                    self.file.take_name("constinit");
                }
            }
            (Type::Record(r), Init::List(items, _)) => {
                // Sema's list is fully braced: one item per member, in order.
                let body = r.body().expect("sema: locals have complete types");
                for (m, item) in body.members.iter().zip(items) {
                    self.init_local(&m.ty, item, off + m.offset as i32);
                }
            }
            (Type::Record(_), Init::Expr(e)) => {
                // A struct initialized by a struct: copy it.
                self.expr(e);
                self.add_imm("a1", "s0", off);
                self.copy("a1", "a0", ty);
            }
            (
                Type::Array(_, Some(n)),
                Init::Expr(Expr {
                    kind: ExprKind::Str(bytes),
                    pos,
                    ..
                }),
            ) => {
                if self.constant_init {
                    // Bytes of a constant clang emits whole.
                    for (i, &b) in bytes.iter().take(*n as usize).enumerate() {
                        if b != 0 {
                            self.li("a0", b.into());
                            self.access_local("sb", "a0", off + i as i32);
                        }
                    }
                } else {
                    // Otherwise clang copies the array from the literal,
                    // whose array is this one, cut or padded with zeros
                    // (`EmitAggExpr` of the string literal).
                    let mut array = bytes.clone();
                    array.push(0);
                    array.resize(*n as usize, 0);
                    let label = self.file.string_array(&array, *pos);
                    self.la_label("a0", &label);
                    self.add_imm("a1", "s0", off);
                    self.copy("a1", "a0", ty);
                }
            }
            (_, Init::Expr(e)) => {
                // A constant initializer's element keeps what a static
                // initializer's does.
                if self.constant_init {
                    self.file.initializer_casts(e);
                }
                self.expr(e);
                self.store_local(ty, off);
            }
            _ => unreachable!("sema checked initializer shapes"),
        }
    }

    /// Clears `size` bytes at `off(s0)` (aligned to a register, see
    /// [`layout`]): register-sized stores (`sw`, `sd` on RV64), or a loop
    /// for more than 16 of them, then the last bytes.
    fn clear_local(&mut self, off: i32, size: i32) {
        let (w, store) = (self.word(), self.store_reg());
        let words = size / w;
        if words > 16 {
            self.add_imm("t1", "s0", off); //  t1 = start
            self.li("t0", (words * w).into()); //   t0 = end
            self.ins("add", "t0, t1, t0");
            self.label("2");
            self.ins(store, "zero, 0(t1)");
            self.ins("addi", format!("t1, t1, {w}"));
            self.ins("bltu", "t1, t0, 2b");
        } else {
            for i in 0..words {
                self.access_local(store, "zero", off + w * i);
            }
        }
        for b in words * w..size {
            self.access_local("sb", "zero", off + b);
        }
    }

    // ---- expressions: each leaves its value in a0 ----

    /// An integer constant expression other than a literal (`1024ul *
    /// 1024ul`, `(char)300`, `sizeof(int) * 4`): loads its value, as one
    /// constant (see "Expressions"), and says so. Any other expression is
    /// left to [`Self::expr`].
    fn folded(&mut self, e: &'a Expr) -> bool {
        if !e.ty.is_integer() || matches!(e.kind, ExprKind::Num(_) | ExprKind::Enumerator { .. }) {
            return false;
        }
        let Some(Const::Int(v, kind)) = consteval::eval(e, self.xlen) else {
            return false;
        };
        // A disagreement would load the wrong registers (on RV32, a `long`
        // where sema has a `long long` pair): checked in every build.
        assert_eq!(e.ty.int_kind(), Some(kind), "consteval types it as sema does");
        self.kept_casts(e);
        self.constant(v, kind);
        true
    }

    /// `-g`: the explicit casts inside a folded constant, in the order
    /// clang meets them. Its front end still walks a constant's expression,
    /// keeping the types of the casts it emits, and its IR builder folds
    /// what it emits; but it emits only what is evaluated: of `c ? a : b`
    /// with a constant `c`, the operand chosen (not `c`), and of `l && r`
    /// or `l || r` with a constant `l`, `r` when it decides the result
    /// (`1 && r`, `0 || r`) and nothing otherwise (clang's
    /// `VisitAbstractConditionalOperator`, `VisitBinLAnd`, `VisitBinLOr`).
    fn kept_casts(&mut self, e: &'a Expr) {
        if self.file.debug.is_none() || self.constant_init {
            return;
        }
        self.kept_cast(e);
        let decided = |c: &Expr| consteval::condition(c, self.xlen).map(|v| v != 0);
        match &e.kind {
            ExprKind::Cast { .. } if null_to_pointer(e, self.xlen) => {}
            ExprKind::Unary(_, x)
            | ExprKind::Cast { x, .. }
            | ExprKind::Addr(x)
            | ExprKind::Deref(x)
            | ExprKind::Member { base: x, .. } => self.kept_casts(x),
            ExprKind::Binary(op @ (BinOp::LogAnd | BinOp::LogOr), l, r) => {
                if decided(l) == Some(*op == BinOp::LogAnd) {
                    self.kept_casts(r);
                }
            }
            ExprKind::Binary(_, l, r) => {
                self.kept_casts(l);
                self.kept_casts(r);
            }
            ExprKind::Cond(c, a, b) => match decided(c) {
                Some(true) => self.kept_casts(a),
                _ => self.kept_casts(b),
            },
            // clang describes the enum of a constant its code uses
            // (`CGDebugInfo::EmitGlobalVariable` of the `EnumConstantDecl`).
            ExprKind::Enumerator { of, .. } => self.file.debug.as_mut().expect("-g").enumerator(of),
            _ => {}
        }
    }

    /// `-g`: clang keeps the type of an explicit cast in the unit (and
    /// describes it as it is evaluated).
    fn kept_cast(&mut self, e: &Expr) {
        if self.constant_init {
            return;
        }
        if let (
            Some(d),
            ExprKind::Cast {
                implicit: false,
                spelled,
                ..
            },
        ) = (self.file.debug.as_mut(), &e.kind)
        {
            d.cast(&e.ty, spelled);
        }
    }

    fn expr(&mut self, e: &'a Expr) {
        if self.folded(e) {
            return;
        }
        match &e.kind {
            ExprKind::Num(v) | ExprKind::Enumerator { value: v, .. } => {
                self.kept_casts(e);
                // The value as the register holds it: an `unsigned int`
                // 0xffffffff is `li a0, -1`.
                let kind = e.ty.int_kind().expect("an integer constant");
                self.constant(*v, kind);
            }
            ExprKind::Str(bytes) => {
                let label = self.file.string(bytes, e.pos);
                self.la_label("a0", &label);
            }
            ExprKind::Var { var, .. } => match var {
                Some(VarRef::Local(id)) => {
                    let off = self.offset(*id);
                    if e.ty.is_addressed() {
                        self.add_imm("a0", "s0", off); // an array or struct: its address
                    } else {
                        self.load_local(&e.ty, off);
                    }
                }
                Some(VarRef::Global(sym)) => {
                    self.la("a0", sym, &e.ty);
                    self.load(&e.ty);
                }
                None => unreachable!("sema resolves every variable"),
            },
            ExprKind::Assign {
                op: None,
                target,
                value,
            } if target.ty.is_record() => {
                // Struct assignment copies the bytes; its value is the target.
                self.addr(target);
                self.push();
                self.expr(value); // a0 = the source's address
                self.pop("a1");
                self.copy("a1", "a0", &target.ty);
                self.ins("mv", "a0, a1");
            }
            ExprKind::Assign {
                op: None,
                target,
                value,
            } => {
                let place = self.place(target);
                self.expr(value); // already converted to the target's type by sema
                self.store_place(place, &target.ty);
            }
            ExprKind::Assign {
                op: Some(op),
                target,
                value,
            } => {
                let place = self.place(target);
                self.load_place(&place, &target.ty);
                self.push_value(&target.ty);
                self.expr(value);
                self.pop_left(*op, &target.ty, &value.ty); // the target's value, then value
                let (t, v) = (target.ty.decay(), value.ty.decay());
                self.binary(*op, &t, &v);
                let result = arith_type(*op, &t, &v, self.xlen).expect("sema checked the operands");
                self.convert(&result, &target.ty);
                self.store_place(place, &target.ty);
            }
            ExprKind::IncDec { inc, prefix, target } => {
                let ty = &target.ty;
                let step = match ty.unqual() {
                    Type::Ptr(to) => to.size(self.xlen).expect("sema: pointer to a complete type") as i32,
                    _ => 1,
                };
                let delta = if *inc { step } else { -step };
                let place = self.place(target);
                self.load_place(&place, ty);
                if ty.int_kind() == Some(IntKind::Bool) {
                    // `b++` sets a `_Bool`, `b--` flips it: the old value
                    // cannot be worked out from the new one, so keep it.
                    self.ins("mv", "t1, a0");
                    self.add_imm("a0", "a0", delta);
                    self.wrap_int(ty);
                    self.store_place(place, ty);
                    if !prefix {
                        self.ins("mv", "a0, t1");
                    }
                    return;
                }
                if self.is_pair(ty) {
                    // A long long on RV32: the step as a pair, added with carry.
                    self.pair_step(delta);
                    self.store_place(place, ty);
                    if !prefix {
                        self.pair_step(-delta);
                    }
                    return;
                }
                self.step(ty, delta);
                self.store_place(place, ty);
                if !prefix {
                    // x++ gives the old value: the new one minus the step, wrapped again.
                    self.step(ty, -delta);
                }
            }
            // `0 && r` and `1 || r` are constants, folded above, and so are
            // they with a left operand only clang's folding of conditions
            // decides (`&g != 0 || r`): r is left out, as clang leaves it.
            ExprKind::Binary(op @ (BinOp::LogAnd | BinOp::LogOr), l, _)
                if self.constant_condition(l) == Some(*op == BinOp::LogOr) =>
            {
                self.li("a0", i64::from(*op == BinOp::LogOr));
            }
            // `1 && r` and `0 || r` are `r` as 0 or 1, as clang has them.
            ExprKind::Binary(op @ (BinOp::LogAnd | BinOp::LogOr), l, r)
                if self.constant_condition(l) == Some(*op == BinOp::LogAnd) =>
            {
                self.test(r);
                self.ins("snez", "a0, a0");
            }
            ExprKind::Binary(BinOp::LogAnd, l, r) => {
                //     <l>
                //     beqz a0, .L.and.N   # l is 0: so is the result, skip r
                //     <r>
                //   .L.and.N:
                //     snez a0, a0         # 0 or 1
                let end = self.new_label("and");
                self.test(l);
                self.branch_if_zero(&end);
                self.test(r);
                self.label(&end);
                self.ins("snez", "a0, a0");
            }
            ExprKind::Binary(BinOp::LogOr, l, r) => {
                // The mirror image: a nonzero l skips r and becomes 1.
                let end = self.new_label("or");
                self.test(l);
                self.branch_if_nonzero(&end);
                self.test(r);
                self.label(&end);
                self.ins("snez", "a0, a0");
            }
            ExprKind::Binary(op, l, r) => {
                self.expr(l);
                self.push_value(&l.ty);
                self.expr(r);
                self.pop_left(*op, &l.ty, &r.ty);
                // Now a1 = l, a0 = r (a2:a3 and a0:a1 for pairs).
                self.binary(*op, &l.ty.decay(), &r.ty.decay());
            }
            ExprKind::Unary(op, x) if self.is_pair(&e.ty) || (*op == UnOp::Not && self.is_pair(&x.ty)) => {
                self.expr(x);
                match op {
                    // 0 - x: the high word borrows when the low one is not 0.
                    UnOp::Neg => {
                        self.ins("snez", "t0, a0");
                        self.ins("neg", "a0, a0");
                        self.ins("neg", "a1, a1");
                        self.ins("sub", "a1, a1, t0");
                    }
                    UnOp::Plus => {}
                    UnOp::Not => {
                        self.ins("or", "a0, a0, a1");
                        self.ins("seqz", "a0, a0");
                    }
                    UnOp::BitNot => {
                        self.ins("not", "a0, a0");
                        self.ins("not", "a1, a1");
                    }
                }
            }
            ExprKind::Unary(op, x) => {
                self.expr(x);
                match op {
                    // sub a0, zero, a0 (`negw` for an `int` on RV64)
                    UnOp::Neg if self.is_w(&e.ty) => self.ins("negw", "a0, a0"),
                    UnOp::Neg => self.ins("neg", "a0, a0"),
                    UnOp::Plus => {}                           // promotion only: nothing to do
                    UnOp::Not => self.ins("seqz", "a0, a0"),   // a0 = (a0 == 0)
                    UnOp::BitNot => self.ins("not", "a0, a0"), // xori a0, a0, -1
                }
            }
            ExprKind::Addr(x) => self.addr(x),
            ExprKind::Deref(p) => {
                self.expr(p);
                self.load(&e.ty);
            }
            // A null pointer constant made a pointer: null, its operand
            // neither generated nor described, as clang has it.
            ExprKind::Cast { .. } if null_to_pointer(e, self.xlen) => {
                self.kept_cast(e);
                self.li("a0", 0);
            }
            ExprKind::Cast { x, .. } => {
                self.kept_cast(e);
                self.expr(x);
                self.convert(&x.ty, &e.ty);
            }
            ExprKind::SizeofType(_) | ExprKind::SizeofExpr(_) => unreachable!("sema folds sizeof"),
            // A constant condition: only the operand it chooses, as clang has
            // it (an expression has no labels to keep).
            ExprKind::Cond(c, a, b) if self.constant_condition(c).is_some() => {
                self.expr(if self.constant_condition(c) == Some(true) {
                    a
                } else {
                    b
                });
            }
            ExprKind::Cond(c, a, b) => {
                //     <c>
                //     beqz a0, .L.else.N
                //     <a>
                //     j    .L.end.N
                //   .L.else.N:
                //     <b>
                //   .L.end.N:
                let else_ = self.new_label("else");
                let end = self.new_label("end");
                self.test(c);
                self.branch_if_zero(&else_);
                self.expr(a);
                self.ins("j", &end);
                self.label(&else_);
                self.expr(b);
                self.label(&end);
            }
            ExprKind::Comma(a, b) => {
                self.expr(a);
                self.expr(b);
            }
            ExprKind::Member { .. } => {
                self.addr(e);
                self.load(&e.ty);
            }
            ExprKind::Call {
                callee,
                args,
                result,
                arg_copies,
            } => self.call(callee, args, *result, arg_copies),
            ExprKind::CompoundLit { init, local } => {
                self.compound_literal(&e.ty, init, *local);
                if !e.ty.is_addressed() {
                    self.load(&e.ty);
                }
            }
            ExprKind::VaStart { ap, .. } => self.va_start(ap),
            ExprKind::VaArg(ap) => self.va_arg(ap, &e.ty),
            ExprKind::VaEnd(_) | ExprKind::VaCopy(..) => unreachable!("sema rewrites them"),
        }
    }

    /// `a0 += delta` in the type `ty` of a `++` or `--`, wrapped to it: on
    /// RV64 an `int` steps with `addiw` (or `addw`), which keeps it
    /// sign-extended.
    fn step(&mut self, ty: &Type, delta: i32) {
        if self.is_w(ty) {
            if fits_imm12(delta) {
                self.ins("addiw", format!("a0, a0, {delta}"));
            } else {
                self.li("t0", delta.into());
                self.ins("addw", "a0, a0, t0");
            }
        } else {
            self.add_imm("a0", "a0", delta);
            self.wrap_int(ty);
        }
    }

    /// `(type){ ... }`: initializes its hidden local (cleared first, like a
    /// local initialized by a list) and leaves its address in `a0`.
    fn compound_literal(&mut self, ty: &Type, init: &'a Init, local: Option<LocalId>) {
        let off = self.offset(local.expect("sema gives compound literals a local"));
        self.comment(format!("compound literal ({ty})"));
        if ty.is_array() || ty.is_record() {
            self.clear_local(off, ty.size(self.xlen).expect("complete") as i32);
        }
        self.init_local(ty, init, off);
        self.add_imm("a0", "s0", off);
    }

    /// `va_start(ap, last)`: ap = the address of the first variadic
    /// argument, in the registers saved at the top of the frame, or else
    /// among the caller's stack words, past the named ones.
    fn va_start(&mut self, ap: &'a Expr) {
        let place = self.place(ap);
        let first = match arg_word(self.named_words, self.xlen) {
            ArgWord::Reg(_) => -self.va_save,
            ArgWord::Stack(at) => at,
        };
        self.add_imm("a0", "s0", first);
        self.store_place(place, &ap.ty);
    }

    /// `va_arg(ap, ty)`: the argument is where ap points, and ap moves past
    /// its words. A scalar is loaded from there; a small struct's value is
    /// its address, there; a big one's address is the word there.
    ///
    /// A pair aligned to two words (a long long on RV32) is at the next even
    /// word, as the caller placed it ([`abi`]): `ap` is rounded up to 8 bytes
    /// first, as clang's `va_arg` does.
    fn va_arg(&mut self, ap: &'a Expr, ty: &Type) {
        let pass = abi::classify(ty, self.xlen);
        let place = self.place(ap);
        self.load_place(&place, &ap.ty);
        if abi::aligned_pair(ty, pass, self.xlen) {
            let align = 2 * self.word();
            self.ins("addi", format!("a0, a0, {}", align - 1));
            self.ins("andi", format!("a0, a0, {}", -align));
        }
        self.ins("mv", "t1, a0");
        self.add_imm("a0", "a0", self.word() * pass.registers() as i32);
        self.store_place(place, &ap.ty);
        match pass {
            // A narrower scalar is in the low bytes of its word.
            Pass::Scalar => self.ins(load_op(ty, self.xlen), "a0, 0(t1)"),
            Pass::Pair => {
                self.ins("lw", "a0, 0(t1)");
                self.ins("lw", "a1, 4(t1)");
            }
            Pass::Words(_) => self.ins("mv", "a0, t1"),
            Pass::Ref => self.ins(self.load_reg(), "a0, 0(t1)"),
        }
    }

    /// Loads the value of type `ty` at the address in `a0`. Arrays, structs
    /// and functions are not loaded: their value is that address.
    fn load(&mut self, ty: &Type) {
        if self.is_pair(ty) {
            // The high word first: a0 holds the address.
            self.ins("lw", "a1, 4(a0)");
            self.ins("lw", "a0, 0(a0)");
        } else if !ty.is_addressed() {
            self.ins(load_op(ty, self.xlen), "a0, 0(a0)");
        }
    }

    /// Loads the local of type `ty` at `off(s0)`: into `a0`, or for a pair
    /// into `a0` (low word) and `a1` (high).
    fn load_local(&mut self, ty: &Type, off: i32) {
        if self.is_pair(ty) {
            self.access_local("lw", "a0", off);
            self.access_local("lw", "a1", off + 4);
        } else {
            self.access_local(load_op(ty, self.xlen), "a0", off);
        }
    }

    /// Stores `a0` (and for a pair `a1`) to the local of type `ty` at `off(s0)`.
    fn store_local(&mut self, ty: &Type, off: i32) {
        if self.is_pair(ty) {
            self.access_local("sw", "a0", off);
            self.access_local("sw", "a1", off + 4);
        } else {
            self.access_local(store_op(ty, self.xlen), "a0", off);
        }
    }

    /// Leaves the address of the lvalue `e` in `a0`.
    fn addr(&mut self, e: &'a Expr) {
        match &e.kind {
            ExprKind::Var { var, .. } => match var {
                Some(VarRef::Local(id)) => {
                    let off = self.offset(*id);
                    self.add_imm("a0", "s0", off);
                }
                Some(VarRef::Global(sym)) => self.la("a0", sym, &e.ty),
                None => unreachable!("sema resolves every variable"),
            },
            // The address of *p is p.
            ExprKind::Deref(p) => self.expr(p),
            ExprKind::Str(_) => self.expr(e),
            ExprKind::CompoundLit { init, local } => self.compound_literal(&e.ty, init, *local),
            // A member is at its offset from the start of the struct. The
            // struct's value is its address (also when it is not an lvalue,
            // like a call's result, kept in a hidden local), and `p->m`'s `p`
            // is one too.
            ExprKind::Member { base, offset, .. } => {
                self.expr(base);
                if *offset != 0 {
                    self.add_imm("a0", "a0", *offset as i32);
                }
            }
            _ => unreachable!("sema only takes the address of lvalues"),
        }
    }

    /// Prepares to store to `e`; see [`Place`].
    fn place(&mut self, e: &'a Expr) -> Place {
        if let ExprKind::Var {
            var: Some(VarRef::Local(id)),
            ..
        } = &e.kind
        {
            return Place::Frame(self.offset(*id));
        }
        self.addr(e);
        self.push();
        Place::Pushed
    }

    /// Loads the value at a place into `a0`; a pushed address stays pushed.
    fn load_place(&mut self, place: &Place, ty: &Type) {
        match place {
            Place::Frame(off) => self.load_local(ty, *off),
            Place::Pushed => {
                self.ins(self.load_reg(), "a0, 0(sp)"); // the address, left on the stack
                self.load(ty);
            }
        }
    }

    /// Stores `a0` to a place, popping a pushed address.
    fn store_place(&mut self, place: Place, ty: &Type) {
        match place {
            Place::Frame(off) => self.store_local(ty, off),
            // A pair's value is in a0 and a1: the address goes to t2.
            Place::Pushed if self.is_pair(ty) => {
                self.pop("t2");
                self.ins("sw", "a0, 0(t2)");
                self.ins("sw", "a1, 4(t2)");
            }
            Place::Pushed => {
                self.pop("a1");
                self.ins(store_op(ty, self.xlen), "a0, 0(a1)");
            }
        }
    }

    /// Converts the value in `a0` from type `from` to type `to`.
    ///
    /// On RV32 a `long long` is a pair: one comes from a 32-bit value with
    /// its high word the sign (`srai a1, a0, 31`) or 0, by the value's
    /// signedness; one goes to a narrower type from its low word, and to
    /// `_Bool` from both (`or` then `snez`).
    fn convert(&mut self, from: &Type, to: &Type) {
        match (self.is_pair(from), self.is_pair(to)) {
            (false, true) => self.widen_to_pair("a0", "a1", from),
            (true, true) => {}
            (true, false) if to.int_kind() == Some(IntKind::Bool) => {
                self.ins("or", "a0, a0, a1");
                self.ins("snez", "a0, a0");
            }
            // The low word, as the unsigned int it is.
            (true, false) => self.convert_reg("a0", &Type::UINT, to),
            (false, false) => self.convert_reg("a0", from, to),
        }
    }

    /// `lo:hi` = the 32-bit value of type `from` in `lo`, as a long long:
    /// its high word copies the sign of a signed value, and is 0 for an
    /// unsigned one (narrow types are extended already).
    fn widen_to_pair(&mut self, lo: &str, hi: &str, from: &Type) {
        if from.is_integer() && !from.is_unsigned() {
            self.ins("srai", format!("{hi}, {lo}, 31"));
        } else {
            self.ins("li", format!("{hi}, 0"));
        }
    }

    /// Converts the value in `reg` from type `from` to type `to`, keeping
    /// every value as a register holds it (see "Values and types"): a type
    /// narrower than the register sign- or zero-extended by its signedness,
    /// and on RV64 a 32-bit one sign-extended whatever its signedness. So
    ///
    /// * to `_Bool`, anything but 0 (a pointer too) becomes 1: `snez`;
    /// * to a register-wide type (`long`, a pointer), only an `unsigned int`
    ///   on RV64 changes, zero-extended (`slli` and `srli` by 32); every
    ///   other value is its own extension already;
    /// * to a 32-bit type on RV64, a register-wide value is cut to 32 bits
    ///   and sign-extended: `sext.w` (`addiw reg, reg, 0`);
    /// * to `char` or `short`, a value that may not fit is cut to the type
    ///   and widened back ([`Self::wrap_reg`]).
    ///
    /// On RV32 only the last two narrowings change bits. An array's value
    /// is its address, already a pointer.
    fn convert_reg(&mut self, reg: &str, from: &Type, to: &Type) {
        let (from, to) = (from.unqual(), to.unqual());
        let x = self.xlen;
        if to.int_kind() == Some(IntKind::Bool) {
            if from != to {
                self.ins("snez", format!("{reg}, {reg}"));
            }
            return;
        }
        // (size, signed) of each side; pointers are unsigned addresses.
        let shape = |t: &Type| match t {
            Type::Int(k) => Some((k.size(x), k.signed())),
            Type::Ptr(_) | Type::Array(..) | Type::Func(_) => Some((ptr_size(x), false)),
            _ => None,
        };
        let (Some((fsize, fsigned)), Some((tsize, tsigned))) = (shape(from), shape(to)) else {
            return; // to `void`, or a struct as it is
        };
        let w = ptr_size(x);
        if tsize == w {
            if x == Xlen::Rv64 && fsize == 4 && !fsigned {
                self.ins("slli", format!("{reg}, {reg}, 32"));
                self.ins("srli", format!("{reg}, {reg}, 32"));
            }
            return;
        }
        if tsize == 4 {
            if fsize == 8 {
                self.ins("sext.w", format!("{reg}, {reg}"));
            }
            return;
        }
        // A narrower or equal type whose values all fit is already right.
        let fits = (fsize < tsize && (tsigned || !fsigned)) || (fsize == tsize && fsigned == tsigned);
        if !fits {
            self.wrap_reg(reg, to);
        }
    }

    /// Cuts `a0` down to the integer type `ty` and widens it back to the
    /// register as [`Self::convert_reg`] keeps it.
    fn wrap_int(&mut self, ty: &Type) {
        self.wrap_reg("a0", ty);
    }

    /// [`Self::wrap_int`] for any register: `_Bool` is 0 or 1 (`snez`), a
    /// byte or a half is cut and widened by its signedness, a 32-bit type on
    /// RV64 is sign-extended (`sext.w`), and a register-wide one is as it is.
    fn wrap_reg(&mut self, reg: &str, ty: &Type) {
        let Some(k) = ty.int_kind() else { return };
        let size = k.size(self.xlen);
        let shift = 8 * (ptr_size(self.xlen) - size);
        match (size, k.signed()) {
            _ if k == IntKind::Bool => self.ins("snez", format!("{reg}, {reg}")),
            _ if size == ptr_size(self.xlen) => {}
            (4, _) => self.ins("sext.w", format!("{reg}, {reg}")),
            (1, false) => self.ins("andi", format!("{reg}, {reg}, 255")),
            (_, signed) => {
                self.ins("slli", format!("{reg}, {reg}, {shift}"));
                let right = if signed { "srai" } else { "srli" };
                self.ins(right, format!("{reg}, {reg}, {shift}"));
            }
        }
    }

    /// Multiplies `reg` by `size`: the scaling of pointer arithmetic.
    fn scale(&mut self, reg: &str, size: u32) {
        match log2(size) {
            Some(0) => {}
            Some(n) => self.ins("slli", format!("{reg}, {reg}, {n}")),
            None => {
                self.li("t0", size.into());
                self.ins("mul", format!("{reg}, {reg}, t0"));
            }
        }
    }

    /// `a0 = a1 op a0` for the operators that always evaluate both sides, with
    /// `l` and `r` the (decayed) types of a1 and a0. Integer operands are
    /// first converted to the type the operation is done in (the usual
    /// arithmetic conversions; for a shift, the left operand's promoted
    /// type), which changes bits only where widths differ: an `int` meeting
    /// a `long` on RV64. An integer added to a pointer is first widened to
    /// the pointer's width by its signedness. On RV64, arithmetic in a
    /// 32-bit type uses the W instructions.
    fn binary(&mut self, op: BinOp, l: &Type, r: &Type) {
        let x = self.xlen;
        if let (Some(lk), Some(rk)) = (l.int_kind(), r.int_kind()) {
            let k = Type::Int(match op {
                BinOp::Shl | BinOp::Shr => lk.promoted(),
                _ => crate::types::common(lk, rk, x),
            });
            if self.is_pair(&k) {
                // In a long long on RV32: the left operand in a2:a3, the
                // right in a0:a1 (a shift's amount is its low word, a0).
                // A narrower left operand is in a2 already (see `pop_left`).
                if !self.is_pair(l) {
                    self.widen_to_pair("a2", "a3", l);
                }
                if !matches!(op, BinOp::Shl | BinOp::Shr) && !self.is_pair(r) {
                    self.widen_to_pair("a0", "a1", r);
                }
                return self.pair_arith(op, k.is_unsigned());
            }
            self.convert_reg("a1", l, &k);
            if !matches!(op, BinOp::Shl | BinOp::Shr) {
                self.convert_reg("a0", r, &k);
            }
            return self.arith(op, &k, &k);
        }
        // A pointer and an integer: the index as a register-wide integer
        // (`ptrdiff_t`, or `size_t` for an unsigned one).
        let index = |t: &Type| {
            let signed = !t.is_unsigned();
            Type::Int(if signed {
                IntKind::ptrdiff_t(x)
            } else {
                IntKind::size_t(x)
            })
        };
        if l.is_ptr() && r.is_integer() {
            // A pair index on RV32: its low word, in a0 already.
            if !self.is_pair(r) {
                self.convert_reg("a0", r, &index(r));
            }
        } else if r.is_ptr() && l.is_integer() {
            if self.is_pair(l) {
                self.ins("mv", "a1, a2");
            } else {
                self.convert_reg("a1", l, &index(l));
            }
        }
        self.arith(op, l, r);
    }

    /// `a0:a1 = a2:a3 op a0:a1` for a long long on RV32 (`unsigned` for an
    /// `unsigned long long`), as LLVM expands 64-bit operations on RV32
    /// with M: a carry or a borrow from the low words (`sltu`), `mul` and
    /// `mulhu` for the product, each shift in two halves by whether the
    /// amount reaches 32, comparisons on the high words and then the low
    /// ones, unsigned (their result, 0 or 1, in `a0`). Division and
    /// remainder call the libgcc routines (`__divdi3`, `__udivdi3`,
    /// `__moddi3`, `__umoddi3`), which the C library and the kernel
    /// provide. Clobbers `t0`-`t2`.
    fn pair_arith(&mut self, op: BinOp, unsigned: bool) {
        let less = if unsigned { "sltu" } else { "slt" };
        match op {
            BinOp::Add => {
                self.ins("add", "a0, a2, a0");
                self.ins("sltu", "t0, a0, a2"); // the carry
                self.ins("add", "a1, a3, a1");
                self.ins("add", "a1, a1, t0");
            }
            BinOp::Sub => {
                self.ins("sltu", "t0, a2, a0"); // the borrow
                self.ins("sub", "a0, a2, a0");
                self.ins("sub", "a1, a3, a1");
                self.ins("sub", "a1, a1, t0");
            }
            BinOp::Mul => {
                self.ins("mul", "t0, a3, a0"); // hi(l) * lo(r)
                self.ins("mul", "t1, a2, a1"); // lo(l) * hi(r)
                self.ins("add", "t0, t0, t1");
                self.ins("mulhu", "t1, a2, a0"); // the carry out of lo(l) * lo(r)
                self.ins("add", "a1, t0, t1");
                self.ins("mul", "a0, a2, a0");
            }
            BinOp::Div | BinOp::Rem => {
                let helper = match (op, unsigned) {
                    (BinOp::Div, false) => "__divdi3",
                    (BinOp::Div, true) => "__udivdi3",
                    (_, false) => "__moddi3",
                    (_, true) => "__umoddi3",
                };
                // The dividend in a0:a1, the divisor in a2:a3.
                self.ins("mv", "t0, a0");
                self.ins("mv", "t1, a1");
                self.ins("mv", "a0, a2");
                self.ins("mv", "a1, a3");
                self.ins("mv", "a2, t0");
                self.ins("mv", "a3, t1");
                let pad = (16 - (self.depth as i32 * self.word()) % 16) % 16;
                if pad != 0 {
                    self.ins("addi", format!("sp, sp, {}", -pad));
                }
                self.ins_sized(8, "call", helper);
                if pad != 0 {
                    self.ins("addi", format!("sp, sp, {pad}"));
                }
            }
            BinOp::Shl | BinOp::Shr => self.pair_shift(op == BinOp::Shl, unsigned),
            BinOp::BitAnd | BinOp::BitOr | BinOp::BitXor => {
                let w = match op {
                    BinOp::BitAnd => "and",
                    BinOp::BitOr => "or",
                    _ => "xor",
                };
                self.ins(w, "a0, a2, a0");
                self.ins(w, "a1, a3, a1");
            }
            BinOp::Eq | BinOp::Ne => {
                self.ins("xor", "t0, a2, a0");
                self.ins("xor", "t1, a3, a1");
                self.ins("or", "a0, t0, t1");
                self.ins(if op == BinOp::Eq { "seqz" } else { "snez" }, "a0, a0");
            }
            // l < r: the high words decide, or when they are equal the low
            // ones (unsigned). `>` swaps the sides; `<=` and `>=` negate.
            BinOp::Lt | BinOp::Ge => {
                self.pair_less(less, ("a2", "a3"), ("a0", "a1"));
                if op == BinOp::Ge {
                    self.ins("xori", "a0, a0, 1");
                }
            }
            BinOp::Gt | BinOp::Le => {
                self.pair_less(less, ("a0", "a1"), ("a2", "a3"));
                if op == BinOp::Le {
                    self.ins("xori", "a0, a0, 1");
                }
            }
            BinOp::LogAnd | BinOp::LogOr => unreachable!("handled in expr: they short-circuit"),
        }
    }

    /// `a0 = x < y` for the pairs `x` and `y` (`slt` or `sltu` on the high
    /// words): `(x.hi < y.hi) | (x.hi == y.hi & x.lo <u y.lo)`.
    fn pair_less(&mut self, less: &str, x: (&str, &str), y: (&str, &str)) {
        self.ins(less, format!("t0, {}, {}", x.1, y.1));
        self.ins("xor", format!("t1, {}, {}", x.1, y.1));
        self.ins("seqz", "t1, t1");
        self.ins("sltu", format!("t2, {}, {}", x.0, y.0));
        self.ins("and", "t1, t1, t2");
        self.ins("or", "a0, t0, t1");
    }

    /// `a0:a1 = a2:a3 << a0` (or `>>`, arithmetic unless `unsigned`), by the
    /// low 6 bits of the amount, as RV64's `sll` takes them. From 32 up one
    /// word moves to the other, shifted by the rest (`sll` takes its low 5
    /// bits); below 32 the bits crossing between the words are shifted by
    /// `31 - n` after a shift by 1, which is right for `n = 0` too:
    ///
    /// ```text
    ///     andi  a0, a0, 63       # n
    ///     andi  t0, a0, 32
    ///     beqz  t0, .L.shl.N
    ///     sll   a1, a2, a0       # n >= 32: hi = lo << (n - 32), lo = 0
    ///     li    a0, 0
    ///     j     .L.shift.M
    ///   .L.shl.N:                # n < 32
    ///     not   t2, a0           # 31 - n, in its low 5 bits
    ///     sll   a1, a3, a0       # hi << n
    ///     srli  t1, a2, 1        # | lo >> (32 - n)
    ///     srl   t1, t1, t2
    ///     or    a1, a1, t1
    ///     sll   a0, a2, a0       # lo << n
    ///   .L.shift.M:
    /// ```
    fn pair_shift(&mut self, left: bool, unsigned: bool) {
        let small = self.new_label(if left { "shl" } else { "shr" });
        let end = self.new_label("shift");
        self.ins("andi", "a0, a0, 63");
        self.ins("andi", "t0, a0, 32");
        self.branch("beqz", "t0", &small);
        if left {
            self.ins("sll", "a1, a2, a0");
            self.ins("li", "a0, 0");
        } else if unsigned {
            self.ins("srl", "a0, a3, a0");
            self.ins("li", "a1, 0");
        } else {
            self.ins("sra", "a0, a3, a0");
            self.ins("srai", "a1, a3, 31");
        }
        self.ins("j", &end);
        self.label(&small);
        self.ins("not", "t2, a0");
        if left {
            self.ins("sll", "a1, a3, a0");
            self.ins("srli", "t1, a2, 1");
            self.ins("srl", "t1, t1, t2");
            self.ins("or", "a1, a1, t1");
            self.ins("sll", "a0, a2, a0");
        } else {
            self.ins("srl", "t0, a2, a0"); // lo >> n
            self.ins("slli", "t1, a3, 1"); // | hi << (32 - n)
            self.ins("sll", "t1, t1, t2");
            self.ins("or", "t0, t0, t1");
            self.ins(if unsigned { "srl" } else { "sra" }, "a1, a3, a0");
            self.ins("mv", "a0, t0");
        }
        self.label(&end);
    }

    /// [`Self::binary`] once the operands are converted: both of the
    /// integer type `l` (= `r`), or pointers, or a pointer and a
    /// register-wide integer.
    fn arith(&mut self, op: BinOp, l: &Type, r: &Type) {
        // Comparisons, division and remainder follow the common type's
        // signedness; a pointer compares as an unsigned address.
        let unsigned = l.is_unsigned() || r.is_unsigned();
        let pick = |s: &'static str, u: &'static str| if unsigned { u } else { s };
        let x = self.xlen;
        let elem_size = |t: &Type| {
            t.pointee()
                .and_then(|t| t.size(x))
                .expect("sema: complete pointee")
        };
        // The W form of an instruction for 32-bit arithmetic on RV64.
        let w32 = self.is_w(l);
        let w = |op: &'static str| -> String {
            if w32 {
                format!("{op}w")
            } else {
                op.to_string()
            }
        };
        match op {
            BinOp::Add if l.is_ptr() || r.is_ptr() => {
                // A pointer plus an integer moves by whole elements.
                if l.is_ptr() {
                    self.scale("a0", elem_size(l));
                } else {
                    self.scale("a1", elem_size(r));
                }
                self.ins("add", "a0, a1, a0");
            }
            BinOp::Add => self.ins(&w("add"), "a0, a1, a0"),
            BinOp::Sub if l.is_ptr() && r.is_ptr() => {
                // The difference of two pointers counts elements.
                self.ins("sub", "a0, a1, a0");
                match log2(elem_size(l)) {
                    Some(0) => {}
                    Some(n) => self.ins("srai", format!("a0, a0, {n}")),
                    None => {
                        self.li("t0", elem_size(l).into());
                        self.ins("div", "a0, a0, t0");
                    }
                }
            }
            BinOp::Sub if l.is_ptr() => {
                self.scale("a0", elem_size(l));
                self.ins("sub", "a0, a1, a0");
            }
            BinOp::Sub => self.ins(&w("sub"), "a0, a1, a0"),
            BinOp::Mul => self.ins(&w("mul"), "a0, a1, a0"),
            BinOp::Div => self.ins(&w(pick("div", "divu")), "a0, a1, a0"),
            BinOp::Rem => self.ins(&w(pick("rem", "remu")), "a0, a1, a0"),
            BinOp::Shl => self.ins(&w("sll"), "a0, a1, a0"),
            // Only the left operand's type matters for a shift.
            BinOp::Shr => {
                let op = if l.is_unsigned() { "srl" } else { "sra" };
                self.ins(&w(op), "a0, a1, a0");
            }
            BinOp::BitAnd => self.ins("and", "a0, a1, a0"),
            BinOp::BitOr => self.ins("or", "a0, a1, a0"),
            BinOp::BitXor => self.ins("xor", "a0, a1, a0"),
            BinOp::Lt => self.ins(pick("slt", "sltu"), "a0, a1, a0"), // l < r
            BinOp::Gt => self.ins(pick("slt", "sltu"), "a0, a0, a1"), // r < l
            BinOp::Le => {
                self.ins(pick("slt", "sltu"), "a0, a0, a1"); // l <= r is !(r < l)
                self.ins("xori", "a0, a0, 1");
            }
            BinOp::Ge => {
                self.ins(pick("slt", "sltu"), "a0, a1, a0"); // l >= r is !(l < r)
                self.ins("xori", "a0, a0, 1");
            }
            BinOp::Eq => {
                self.ins("xor", "a0, a1, a0"); // 0 exactly when l == r
                self.ins("seqz", "a0, a0");
            }
            BinOp::Ne => {
                self.ins("xor", "a0, a1, a0");
                self.ins("snez", "a0, a0");
            }
            BinOp::LogAnd | BinOp::LogOr => unreachable!("handled in expr: they short-circuit"),
        }
    }

    /// A call: `f(args)` with `f` a function's name (`call f`), or through any
    /// other expression of function or pointer-to-function type (`jalr`).
    /// Arguments and the result travel as [`abi`] says.
    fn call(
        &mut self,
        callee: &'a Expr,
        args: &'a [Expr],
        result: Option<LocalId>,
        copies: &[Option<LocalId>],
    ) {
        let ft: FuncType = callee
            .ty
            .decay()
            .pointee()
            .and_then(Type::func_type)
            .expect("sema: the callee is a function")
            .clone();
        let layout = ArgLayout::new(&ft, args, self.xlen);
        let (types, regs, used) = (&layout.types, &layout.words, layout.used);
        let direct = callee.direct_callee();
        // clang refers to the function it calls before the arguments
        // (`EmitCallee`, then `EmitCallArgs`): in `f(g())`, `f`'s symbol is
        // made first ("Order" above).
        if let Some(name) = direct {
            self.file.reference(name, None);
        }
        if used > abi::ARG_REGS {
            let hidden = if abi::returns_in_memory(&ft.ret, self.xlen) {
                result
            } else {
                None
            };
            self.call_with_stack(callee, args, copies, hidden, &layout);
            self.call_result(&ft, result);
            return;
        }
        // Through a pointer: the function's address is evaluated first and
        // waits on the stack, under the arguments, until they are loaded.
        if direct.is_none() {
            self.expr(callee);
            if args.is_empty() {
                self.ins("mv", "t1, a0");
            } else {
                self.push();
            }
        }
        // Evaluate every argument before loading any argument register: a later
        // argument may itself contain a call, which would clobber a0-a7. So all
        // but the last are pushed; the last one, already in a0, goes straight
        // to its register(s), and then the others are popped into theirs. A
        // struct's value is its address: its words are loaded from there.
        if let Some(last) = args.len().checked_sub(1) {
            for i in 0..last {
                self.argument(&args[i], copies[i]);
                self.push_value(&args[i].ty);
            }
            self.argument(&args[last], copies[last]);
            match regs[last] {
                (0, Pass::Scalar | Pass::Ref | Pass::Pair) => {}
                (r, Pass::Scalar | Pass::Ref) => self.ins("mv", format!("a{r}, a0")),
                // The high word first: a1 may be where the low word goes.
                (r, Pass::Pair) => {
                    self.ins("mv", format!("a{}, a1", r + 1));
                    self.ins("mv", format!("a{r}, a0"));
                }
                (r, Pass::Words(n)) => {
                    self.ins("mv", "t1, a0");
                    self.load_words("t1", r, n, &types[last]);
                }
            }
            for i in (0..last).rev() {
                match regs[i] {
                    (r, Pass::Scalar | Pass::Ref) => self.pop(&format!("a{r}")),
                    (r, Pass::Pair) => self.pop_pair(&format!("a{r}"), &format!("a{}", r + 1)),
                    (r, Pass::Words(n)) => {
                        self.pop("t1");
                        self.load_words("t1", r, n, &types[i]);
                    }
                }
            }
            if direct.is_none() {
                self.pop("t1");
            }
        }
        // A big struct result goes to the hidden local: its address in a0.
        if abi::returns_in_memory(&ft.ret, self.xlen) {
            let off = self.offset(result.expect("sema: a struct result has a local"));
            self.add_imm("a0", "s0", off);
        }
        // Temporaries of an enclosing expression may still be pushed (in
        // `x + f(y)`, x is). The convention wants sp 16-byte aligned at the
        // call, so pad the stack to the next multiple of 16 around it.
        let pad = (16 - (self.depth as i32 * self.word()) % 16) % 16;
        if pad != 0 {
            self.ins("addi", format!("sp, sp, {}", -pad));
        }
        self.jump_to(direct);
        if pad != 0 {
            self.ins("addi", format!("sp, sp, {pad}"));
        }
        self.call_result(&ft, result);
    }

    /// The call instruction: `call f`, or `jalr t1` through a pointer.
    fn jump_to(&mut self, direct: Option<&str>) {
        match direct {
            // `call` is `auipc ra` + `jalr ra`: two words.
            Some(name) => {
                self.file.reference(name, None);
                self.ins_sized(8, "call", name)
            }
            None => self.ins("jalr", "t1"),
        }
    }

    /// After a call: a struct result is kept in its hidden local (a small
    /// one arrives in a0/a1 and is stored there); the call's value is that
    /// address.
    fn call_result(&mut self, ft: &FuncType, result: Option<LocalId>) {
        if let Some(result) = result {
            let off = self.offset(result);
            if let Pass::Words(n) = abi::classify(&ft.ret, self.xlen) {
                let (w, store) = (self.word(), self.store_reg());
                for i in 0..n {
                    self.access_local(store, &format!("a{i}"), off + w * i as i32);
                }
            }
            self.add_imm("a0", "s0", off);
        }
    }

    /// A call whose arguments take more than `a0-a7` (`used` words, placed
    /// as `words` says): every argument is evaluated and pushed, left to
    /// right; then room is made below them for the stack words, keeping
    /// `sp` 16-byte aligned at the call, and each word is copied to its
    /// place, on the stack or in a register:
    ///
    /// ```text
    ///     <arg 0>  push ... <arg n-1>  push
    ///     addi sp, sp, -area           # the outgoing words: 0(sp), 4(sp), ...
    ///     lw   t1, <arg 8>(sp)         # a stack word
    ///     sw   t1, 0(sp)
    ///     lw   a0, <arg 0>(sp)         # a register
    ///     ...
    ///     call f
    ///     addi sp, sp, area + 4n       # the words and the pushed arguments
    /// ```
    fn call_with_stack(
        &mut self,
        callee: &'a Expr,
        args: &'a [Expr],
        copies: &[Option<LocalId>],
        // The hidden local for a big struct result, if any.
        result: Option<LocalId>,
        layout: &ArgLayout,
    ) {
        let (types, words, used) = (&layout.types, &layout.words, layout.used);
        let direct = callee.direct_callee();
        if direct.is_none() {
            self.expr(callee);
            self.push();
        }
        // How many words each argument takes pushed: a pair two, others one.
        let mut sizes = Vec::with_capacity(args.len());
        for (i, a) in args.iter().enumerate() {
            self.argument(a, copies[i]);
            self.push_value(&a.ty);
            sizes.push(if self.is_pair(&a.ty) { 2 } else { 1 });
        }
        let n: i32 = sizes.iter().sum();
        let (w, load, store) = (self.word(), self.load_reg(), self.store_reg());
        let pushed = w * self.depth as i32;
        let stack = w * abi::stack_words(used) as i32;
        let area = (pushed + stack + 15) / 16 * 16 - pushed;
        self.comment(match abi::stack_words(used) {
            1 => "1 argument word goes on the stack".to_string(),
            n => format!("{n} argument words go on the stack"),
        });
        self.add_imm("sp", "sp", -area);
        // Where argument i was pushed: the last one is right above the area.
        let temp = |i: usize| area + w * sizes[i + 1..].iter().sum::<i32>();
        // The stack words first (with t1, t2 and t0), then the registers.
        for (i, &(word, pass)) in words.iter().enumerate() {
            match pass {
                Pass::Scalar | Pass::Ref => {
                    if let ArgWord::Stack(at) = arg_word(word, self.xlen) {
                        self.ins(load, format!("t1, {}(sp)", temp(i)));
                        self.ins(store, format!("t1, {at}(sp)"));
                    }
                }
                // A pair's words were pushed as they go: low, then high.
                Pass::Pair => {
                    for k in 0..2 {
                        if let ArgWord::Stack(at) = arg_word(word + k, self.xlen) {
                            self.ins(load, format!("t1, {}(sp)", temp(i) + w * k as i32));
                            self.ins(store, format!("t1, {at}(sp)"));
                        }
                    }
                }
                Pass::Words(nw) => {
                    for k in 0..nw {
                        if let ArgWord::Stack(at) = arg_word(word + k as usize, self.xlen) {
                            self.ins(load, format!("t1, {}(sp)", temp(i)));
                            self.load_word("t2", "t1", k, &types[i]);
                            self.ins(store, format!("t2, {at}(sp)"));
                        }
                    }
                }
            }
        }
        for (i, &(word, pass)) in words.iter().enumerate() {
            match pass {
                Pass::Scalar | Pass::Ref => {
                    if let ArgWord::Reg(r) = arg_word(word, self.xlen) {
                        self.ins(load, format!("{r}, {}(sp)", temp(i)));
                    }
                }
                Pass::Pair => {
                    for k in 0..2 {
                        if let ArgWord::Reg(r) = arg_word(word + k, self.xlen) {
                            self.ins(load, format!("{r}, {}(sp)", temp(i) + w * k as i32));
                        }
                    }
                }
                Pass::Words(nw) => {
                    if word < abi::ARG_REGS {
                        self.ins(load, format!("t1, {}(sp)", temp(i)));
                    }
                    for k in 0..nw {
                        if let ArgWord::Reg(r) = arg_word(word + k as usize, self.xlen) {
                            self.load_word(&r, "t1", k, &types[i]);
                        }
                    }
                }
            }
        }
        // A big struct result goes to the hidden local: its address in a0.
        if let Some(result) = result {
            let off = self.offset(result);
            self.add_imm("a0", "s0", off);
        }
        if direct.is_none() {
            self.ins(load, format!("t1, {}(sp)", area + w * n));
        }
        self.jump_to(direct);
        let popped = n + i32::from(direct.is_none());
        self.add_imm("sp", "sp", area + w * popped);
        self.depth -= popped as u32;
    }

    /// Evaluates an argument into `a0`: a scalar's value, or a struct's
    /// address. A struct passed by reference is first copied to its hidden
    /// local (`copy`), and that copy's address is what is passed.
    fn argument(&mut self, arg: &'a Expr, copy: Option<LocalId>) {
        self.expr(arg);
        if let Some(copy) = copy {
            let off = self.offset(copy);
            self.add_imm("a1", "s0", off);
            self.copy("a1", "a0", &arg.ty);
            self.ins("mv", "a0, a1");
        }
    }

    /// Loads the `n` words of the struct of type `ty` at the address in
    /// `src` into `a<first>`, `a<first+1>`: how a small struct is passed
    /// and returned.
    fn load_words(&mut self, src: &str, first: usize, n: u32, ty: &Type) {
        for w in 0..n {
            self.load_word(&format!("a{}", first + w as usize), src, w, ty);
        }
    }

    /// Loads word `w` (a register's worth: 4 bytes, 8 on RV64) of the struct
    /// of type `ty` at the address in `src` into `reg`. A struct aligned to
    /// less than a register may not be at a register boundary, so its word is
    /// put together from aligned bytes, halves or (on RV64) words, each
    /// zero-extended (in `t0`); bytes past the struct's end read as zero.
    fn load_word(&mut self, reg: &str, src: &str, w: u32, ty: &Type) {
        let size = ty.size(self.xlen).expect("a complete struct");
        let reg_size = self.word() as u32;
        let unit = ty.align(self.xlen).min(reg_size);
        if unit == reg_size {
            self.ins(self.load_reg(), format!("{reg}, {}({src})", reg_size * w));
            return;
        }
        let load = match unit {
            1 => "lbu",
            2 => "lhu",
            _ => "lwu",
        };
        for k in (0..reg_size).step_by(unit as usize) {
            let at = reg_size * w + k;
            if at >= size {
                break;
            }
            if k == 0 {
                self.ins(load, format!("{reg}, {at}({src})"));
            } else {
                self.ins(load, format!("t0, {at}({src})"));
                self.ins("slli", format!("t0, t0, {}", 8 * k));
                self.ins("or", format!("{reg}, {reg}, t0"));
            }
        }
    }

    /// Copies the struct of type `ty` from the address in `src` to the one in
    /// `dst` (neither of them `t0-t3`, both left as they are). It goes in
    /// units of the struct's alignment (at most a register), so no access is
    /// misaligned: unrolled up to 16 units, a loop beyond. Clobbers `t0`, and
    /// `t1-t3` for the loop.
    fn copy(&mut self, dst: &str, src: &str, ty: &Type) {
        let size = ty.size(self.xlen).expect("sema: a complete struct");
        let unit = ty.align(self.xlen).min(self.word() as u32);
        let (load, store) = match unit {
            1 => ("lbu", "sb"),
            2 => ("lhu", "sh"),
            4 => ("lw", "sw"),
            _ => ("ld", "sd"),
        };
        if size / unit <= 16 {
            for at in (0..size).step_by(unit as usize) {
                self.ins(load, format!("t0, {at}({src})"));
                self.ins(store, format!("t0, {at}({dst})"));
            }
        } else {
            self.ins("mv", format!("t1, {src}")); // t1 = from
            self.ins("mv", format!("t2, {dst}")); // t2 = to
            self.li("t3", size.into()); //            t3 = end of from
            self.ins("add", "t3, t1, t3");
            self.label("2");
            self.ins(load, "t0, 0(t1)");
            self.ins(store, "t0, 0(t2)");
            self.ins("addi", format!("t1, t1, {unit}"));
            self.ins("addi", format!("t2, t2, {unit}"));
            self.ins("bltu", "t1, t3, 2b");
        }
    }
}

/// `mnemonic operands`, with the operands lined up in a column.
fn insn_text(mnemonic: &str, operands: &str) -> String {
    if operands.is_empty() {
        mnemonic.to_string()
    } else {
        format!("{mnemonic:<5} {operands}")
    }
}

/// B-type branches reach -4096..=+4094 bytes from themselves.
const BRANCH_REACH: std::ops::RangeInclusive<i64> = -4096..=4094;

/// Decides which conditional branches need the long form (the opposite branch
/// over a `j`). Returns one flag per line, true for a branch that does.
///
/// Addresses are estimated with every pseudo-instruction at its largest
/// (`li` as two words, or eight for a 64-bit value), so a distance is never
/// underestimated; a branch that is safe here is safe in the assembled code.
/// Making a branch long moves code after it, which can push another branch
/// out of reach, so this repeats until nothing changes. It always ends:
/// branches only ever become long.
fn relax_branches(lines: &[Line]) -> Vec<bool> {
    let mut long = vec![false; lines.len()];
    loop {
        // Where each line starts and where each label is, with the current choice.
        let mut addr = Vec::with_capacity(lines.len());
        let mut labels = HashMap::new();
        let mut pc: i64 = 0;
        for (i, line) in lines.iter().enumerate() {
            addr.push(pc);
            pc += match line {
                Line::Label(name) => {
                    labels.insert(name.as_str(), pc);
                    0
                }
                Line::Insn(_, size) => i64::from(*size),
                Line::Branch { .. } if long[i] => 8,
                Line::Branch { .. } => 4,
                Line::Text(_) => 0,
            };
        }
        let mut changed = false;
        for (i, line) in lines.iter().enumerate() {
            if let Line::Branch { target, .. } = line {
                let distance = labels[target.as_str()] - addr[i];
                if !long[i] && !BRANCH_REACH.contains(&distance) {
                    long[i] = true;
                    changed = true;
                }
            }
        }
        if !changed {
            return long;
        }
    }
}

/// The most bytes `lines` can assemble to, with the branches `long` marks
/// in their long form: the same estimate as [`relax_branches`].
fn code_size(lines: &[Line], long: &[bool]) -> i64 {
    lines
        .iter()
        .zip(long)
        .map(|(line, &long)| match line {
            Line::Insn(_, size) => i64::from(*size),
            Line::Branch { .. } if long => 8,
            Line::Branch { .. } => 4,
            Line::Label(_) | Line::Text(_) => 0,
        })
        .sum()
}

/// A `j` reaches -1 MiB..=+1 MiB - 2 bytes from itself.
const JUMP_REACH: i64 = (1 << 20) - 2;

/// Every `j` in a function goes to a label in it, so none is out of reach
/// when the whole function fits in a jump's reach. Sizes are estimated at
/// their largest, so a function just under the limit may be refused too.
fn check_size(f: &Function, lines: &[Line], long: &[bool]) -> Result<(), Error> {
    let size = code_size(lines, long);
    if size > JUMP_REACH {
        let msg = format!(
            "function '{}' is too big: up to {size} bytes of code, and the jumps inside a function \
             reach only 1 MiB",
            f.name
        );
        return Err(Error::new(f.pos, msg));
    }
    Ok(())
}

/// Writes a function's lines as assembly text.
fn render(lines: &[Line], long: &[bool], out: &mut String) {
    for (line, &long) in lines.iter().zip(long) {
        match line {
            Line::Label(name) => {
                let _ = writeln!(out, "{name}:");
            }
            Line::Insn(text, _) => {
                let _ = writeln!(out, "    {text}");
            }
            Line::Text(text) => {
                let _ = writeln!(out, "    {text}");
            }
            Line::Branch { op, regs, target } => {
                if long {
                    // Too far for a branch: skip over a `j` when the condition fails.
                    let inverse = match *op {
                        "beqz" => "bnez",
                        "bnez" => "beqz",
                        "beq" => "bne",
                        "bne" => "beq",
                        _ => unreachable!("codegen emits no other branches"),
                    };
                    let _ = writeln!(out, "    {}", insn_text(inverse, &format!("{regs}, 1f")));
                    let _ = writeln!(out, "    {}", insn_text("j", target));
                    let _ = writeln!(out, "1:");
                } else {
                    let _ = writeln!(out, "    {}", insn_text(op, &format!("{regs}, {target}")));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn compile(src: &str) -> String {
        crate::compile(src).unwrap_or_else(|e| panic!("{e:?}"))
    }

    /// The instructions of `f`, without comments, labels or indentation.
    fn body_of(asm: &str, f: &str) -> Vec<String> {
        asm.lines()
            .skip_while(|l| *l != format!("{f}:"))
            .skip(1)
            .take_while(|l| *l != format!(".L.return.{f}:"))
            .map(str::trim)
            .filter(|l| !l.starts_with('#') && !l.ends_with(':'))
            .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
            .collect()
    }

    /// The instructions `main` runs for `body`, after the prologue.
    fn main_body(globals: &str, body: &str) -> String {
        let asm = compile(&format!("{globals}\nint main() {{ {body} }}"));
        body_of(&asm, "main")[4..].join("; ")
    }

    fn locals(decls: &str) -> (Vec<i32>, i32) {
        let p = crate::parse(&format!("int f() {{ {decls} }}")).unwrap();
        let f = p.funcs().next().unwrap();
        layout(
            &f.locals,
            &vec![true; f.locals.len()],
            saved_regs(Xlen::Rv32),
            Xlen::Rv32,
        )
        .unwrap()
    }

    #[test]
    fn frame_layout() {
        // Only ints: local #i at -(12 + 4*i), as in v0.
        assert_eq!(locals("int a, b, c;"), (vec![-12, -16, -20], 32));
        assert_eq!(locals("int a, b;"), (vec![-12, -16], 16));
        assert_eq!(locals(""), (vec![], 16));
        // Each type at its own alignment, packed downwards.
        assert_eq!(
            locals("char c; short s; char d; int i;"),
            (vec![-9, -12, -13, -20], 32)
        );
        // Arrays are word-aligned.
        assert_eq!(locals("char c; char buf[5]; char d;"), (vec![-9, -16, -17], 32));
        assert_eq!(locals("int a[10];"), (vec![-48], 48));
        // A local the code does not use has no slot.
        let p = crate::parse("int f() { int a, b, c; }").unwrap();
        let f = p.funcs().next().unwrap();
        let used = [true, false, true];
        assert_eq!(
            layout(&f.locals, &used, saved_regs(Xlen::Rv32), Xlen::Rv32),
            Some((vec![-12, 0, -16], 16))
        );
    }

    #[test]
    fn frame_size_limit() {
        // Each array fits, but not both: an error, not a frame that wrapped around.
        let err = crate::compile("int main() { char a[2147483640]; char b[100]; return 0; }").unwrap_err();
        assert_eq!(
            err[0].to_string(),
            "1:5: the locals of 'main' need a stack frame larger than 2 GiB"
        );
        let p = crate::parse("int f() { char a[2147483600]; }").unwrap();
        assert_eq!(
            layout(
                &p.funcs().next().unwrap().locals,
                &[true],
                saved_regs(Xlen::Rv32),
                Xlen::Rv32
            )
            .unwrap()
            .1,
            2147483616
        );
    }

    #[test]
    fn smallest_function() {
        let asm = compile("int main() { return 42; }");
        assert_eq!(
            asm,
            "    .text

    # int main(void): frame 16 bytes
    .globl main
    .type main, @function
main:
    addi  sp, sp, -16
    sw    ra, 12(sp)
    sw    s0, 8(sp)
    addi  s0, sp, 16
    li    a0, 42
.L.return.main:
    addi  sp, s0, -16
    lw    ra, 12(sp)
    lw    s0, 8(sp)
    addi  sp, sp, 16
    ret
    .size main, . - main
"
        );
        // Each function symbol carries its type and size, as clang's do: ten
        // instructions here, and the static one's too.
        let obj = merlin_as::assemble(&compile(
            "int main(void) { return 42; } static int f(void) { return 1; } int (*use)(void) = f;",
        ))
        .unwrap();
        for name in ["main", "f"] {
            let s = obj.symbols.iter().find(|s| s.name == name).unwrap();
            assert_eq!((s.size, s.ty), (40, merlin_as::asm::SymType::Func), "{name}");
        }
    }

    /// Functions and globals come in clang's order (checked against clang
    /// 23's): each where it was first defined or referred to, a `static`
    /// one emitted only once used, after the others, and a tentative
    /// definition at the end; an unused `static` one not at all.
    #[test]
    fn definitions_are_in_clangs_order() {
        let asm = compile(
            "extern int vb; static int sh(int); int g(void);\n\
             static int unused_s(void) { return 7; }\n\
             static int sv = 5; static int sv_unused = 6; int ta;\n\
             int f(void) { return g() + sh(1) + vb + sv; }\n\
             int h(void) { return 3; }\n\
             static int sh(int x) { return x + h(); }\n\
             static int sd(void) { return 4; }\n\
             int g(void) { return sd() + ta; }\n\
             int vb = 2; int vc = 3;\n\
             int main(void) { static int n = 1; return f() + vc + n; }\n",
        );
        let symbols: Vec<&str> = asm
            .lines()
            .filter_map(|l| l.strip_suffix(':'))
            .filter(|l| !l.starts_with('.'))
            .collect();
        assert_eq!(
            symbols,
            ["f", "g", "sh", "vb", "sv", "h", "sd", "ta", "vc", "main", "main.n"],
            "{asm}"
        );
        // A call refers to the function it calls before its arguments are
        // generated (clang's `EmitCallee`, then `EmitCallArgs`).
        let asm = compile(
            "static int sa(int); static int sb(void); int (*pf)(int);\n\
             int f(void) { return sa(sb()) + pf(sb()); }\n\
             static int sa(int x) { return x; }\nstatic int sb(void) { return 2; }\n",
        );
        let symbols: Vec<&str> = asm
            .lines()
            .filter_map(|l| l.strip_suffix(':'))
            .filter(|l| !l.starts_with('.'))
            .collect();
        assert_eq!(symbols, ["f", "sa", "sb", "pf"], "{asm}");
    }

    /// The frame has slots only for the locals of the code generated, as
    /// clang's at `-O0` allocates only for the code it emits: none for an
    /// arm a constant condition leaves out, nor for the temporaries of an
    /// operand left out; a local declared in code that runs has one, used
    /// or not, and so do those of an arm kept for its label.
    #[test]
    fn frames_hold_only_what_is_emitted() {
        let frame = |src: &str| {
            let asm = compile(src);
            let line = asm.lines().find(|l| l.contains(": frame ")).unwrap().to_string();
            let slots: Vec<String> = asm
                .lines()
                .filter(|l| l.contains("(s0)  "))
                .map(|l| l.trim_start_matches([' ', '#']).to_string())
                .collect();
            (line[line.find("frame").unwrap()..].to_string(), slots)
        };
        let (f, slots) = frame("int f(int c) { if (0) { int big[100]; big[0] = c; } return c; }");
        assert_eq!(f, "frame 16 bytes");
        assert_eq!(slots, ["-12(s0)  param int c"]);
        let (f, slots) = frame(
            "struct big { int a[10]; }; struct big h(void);\n\
             int g(void) { return 1 ? 2 : h().a[0]; }",
        );
        assert_eq!((f.as_str(), slots.len()), ("frame 16 bytes", 0));
        let (f, slots) = frame("int f(void) { int unused; return 0; }");
        assert_eq!(f, "frame 16 bytes");
        assert_eq!(slots, ["-12(s0)  local int unused"]);
        let (f, _) = frame("int f(int c) { if (0) { int k[8]; l: k[0] = c; return k[0]; } goto l; }");
        assert_eq!(f, "frame 48 bytes", "kept for its label");
    }

    #[test]
    fn variables_are_sized_objects() {
        // The kernel's tests find which buffer an address is in from the
        // symbol's size, in .bss, .data and .rodata alike, static or not.
        let src = "struct s { int a; char b; } bufs[3]; int n = 5; static const char k[7] = \"abc\";
                   int f(void) { static short count; return k[0] + count; }";
        let asm = compile(src);
        assert!(
            asm.contains("    .type bufs, @object\n    .size bufs, 24\nbufs:\n"),
            "{asm}"
        );
        let obj = merlin_as::assemble(&asm).unwrap();
        for (name, size) in [("bufs", 24), ("n", 4), ("k", 7), ("f.count", 2)] {
            let s = obj.symbols.iter().find(|s| s.name == name).unwrap();
            assert_eq!((s.size, s.ty), (size, merlin_as::asm::SymType::Object), "{name}");
        }
    }

    #[test]
    fn prototypes_and_externs_emit_nothing() {
        let asm = compile("int f(int a); extern int g; int main() { return f(g); }");
        assert!(!asm.contains("f:"), "{asm}");
        assert!(!asm.contains("g:"), "{asm}");
        assert!(asm.contains("call  f\n"), "{asm}");
        assert!(asm.contains("la    a0, g\n"), "{asm}");
        assert_eq!(asm.matches(".globl").count(), 1);
    }

    #[test]
    fn static_functions_are_not_global() {
        let asm = compile("static int f() { return 1; } int main() { return f(); }");
        assert!(!asm.contains(".globl f"), "{asm}");
        assert!(asm.contains("\nf:\n"), "{asm}");
    }

    #[test]
    fn falling_off_the_end_returns_zero() {
        let body = body_of(&compile("int main() { int x = 3; }"), "main");
        assert_eq!(body.last().unwrap(), "li a0, 0");
        // A final return jumps nowhere: the epilogue is next.
        let body = body_of(&compile("int main() { return 3; }"), "main");
        assert_eq!(body.last().unwrap(), "li a0, 3");
        // A return elsewhere jumps to it.
        let asm = compile("int main(int a) { if (a) return 1; return 2; }");
        assert!(asm.contains("j     .L.return.main\n"), "{asm}");
    }

    #[test]
    fn parameters_are_stored_in_their_slots() {
        let asm = compile("int f(int a, char b, short c) { return c; } int main() { return f(1, 2, 3); }");
        let body = body_of(&asm, "f");
        assert_eq!(
            body,
            [
                "addi sp, sp, -16",
                "sw ra, 12(sp)",
                "sw s0, 8(sp)",
                "addi s0, sp, 16",
                // 8 + 4 + 1 (+1 to align) + 2 = 16 bytes: the save area is the frame.
                "sw a0, -12(s0)",
                "sb a1, -13(s0)",
                "sh a2, -16(s0)",
                "lh a0, -16(s0)",
            ]
        );
        assert!(
            asm.contains("# int f(int a, char b, short c): frame 16 bytes\n"),
            "{asm}"
        );
        assert!(asm.contains("#     -13(s0)  param char b\n"), "{asm}");
    }

    #[test]
    fn binary_operators() {
        let op = |ty: &str, src: &str| {
            let body = body_of(
                &compile(&format!("int main({ty} x, {ty} y) {{ return {src}; }}")),
                "main",
            );
            // Skip the prologue (4 lines), the parameter stores (2), `lw` x, the
            // push (2) and `lw` y: what is left is the pop and the operator.
            body[10..]
                .join("; ")
                .replace("lw a1, 0(sp); addi sp, sp, 4; ", "")
        };
        assert_eq!(op("int", "x + y"), "add a0, a1, a0");
        assert_eq!(op("int", "x - y"), "sub a0, a1, a0");
        assert_eq!(op("int", "x * y"), "mul a0, a1, a0");
        assert_eq!(op("int", "x / y"), "div a0, a1, a0");
        assert_eq!(op("int", "x % y"), "rem a0, a1, a0");
        assert_eq!(op("int", "x << y"), "sll a0, a1, a0");
        assert_eq!(op("int", "x >> y"), "sra a0, a1, a0");
        assert_eq!(op("int", "x & y"), "and a0, a1, a0");
        assert_eq!(op("int", "x | y"), "or a0, a1, a0");
        assert_eq!(op("int", "x ^ y"), "xor a0, a1, a0");
        assert_eq!(op("int", "x < y"), "slt a0, a1, a0");
        assert_eq!(op("int", "x > y"), "slt a0, a0, a1");
        assert_eq!(op("int", "x <= y"), "slt a0, a0, a1; xori a0, a0, 1");
        assert_eq!(op("int", "x >= y"), "slt a0, a1, a0; xori a0, a0, 1");
        assert_eq!(op("int", "x == y"), "xor a0, a1, a0; seqz a0, a0");
        assert_eq!(op("int", "x != y"), "xor a0, a1, a0; snez a0, a0");
        // Unsigned operations.
        assert_eq!(op("unsigned", "x / y"), "divu a0, a1, a0");
        assert_eq!(op("unsigned", "x % y"), "remu a0, a1, a0");
        assert_eq!(op("unsigned", "x >> y"), "srl a0, a1, a0");
        assert_eq!(op("unsigned", "x < y"), "sltu a0, a1, a0");
        assert_eq!(op("unsigned long", "x >= y"), "sltu a0, a1, a0; xori a0, a0, 1");
        // unsigned char promotes to int: signed.
        let body = body_of(
            &compile("int main(unsigned char x, unsigned char y) { return x / y; }"),
            "main",
        );
        assert_eq!(body.last().unwrap(), "div a0, a1, a0");
        assert!(body.contains(&"lbu a0, -9(s0)".to_string()), "{body:?}");
    }

    #[test]
    fn pointer_arithmetic_scales() {
        let p = |decls: &str, e: &str| main_body(decls, &format!("return {e};"));
        // int *: 4 bytes, a shift by 2.
        assert!(p("int *p; int i;", "*(p + i)").ends_with("slli a0, a0, 2; add a0, a1, a0; lw a0, 0(a0)"));
        assert!(p("int *p; int i;", "*(i + p)").ends_with("slli a1, a1, 2; add a0, a1, a0; lw a0, 0(a0)"));
        assert!(p("char *p; int i;", "*(p - i)").ends_with("sub a0, a1, a0; lbu a0, 0(a0)"));
        assert!(p("short *p, *q;", "p - q").ends_with("sub a0, a1, a0; srai a0, a0, 1"));
        // 12-byte elements: multiply and divide.
        assert!(p("int m[4][3]; int i;", "m[i][0]").contains("li t0, 12; mul a0, a0, t0"));
    }

    #[test]
    fn narrow_types_load_and_store_by_size() {
        assert_eq!(
            main_body("", "signed char c = 1; unsigned short s = 2; c = s; return c;"),
            "li a0, 1; sb a0, -9(s0); li a0, 2; sh a0, -12(s0); lhu a0, -12(s0); \
             slli a0, a0, 24; srai a0, a0, 24; sb a0, -9(s0); lb a0, -9(s0)"
        );
        // Widening is free.
        assert!(main_body("", "unsigned char c = 1; int i = c; return i;")
            .contains("lbu a0, -9(s0); sw a0, -16(s0)"));
    }

    #[test]
    fn conversions() {
        let cast = |from: &str, to: &str| {
            let body = body_of(
                &compile(&format!("int main({from} x) {{ return ({to})x; }}")),
                "main",
            );
            body[6..].join("; ")
        };
        assert_eq!(cast("int", "char"), "andi a0, a0, 255");
        assert_eq!(cast("int", "unsigned char"), "andi a0, a0, 255");
        assert_eq!(cast("int", "signed char"), "slli a0, a0, 24; srai a0, a0, 24");
        assert_eq!(cast("int", "short"), "slli a0, a0, 16; srai a0, a0, 16");
        assert_eq!(cast("int", "unsigned short"), "slli a0, a0, 16; srli a0, a0, 16");
        assert_eq!(cast("int", "unsigned"), "");
        assert_eq!(cast("char", "short"), "");
        assert_eq!(cast("char", "unsigned char"), "");
        assert_eq!(cast("char", "signed char"), "slli a0, a0, 24; srai a0, a0, 24");
        assert_eq!(cast("signed char", "char"), "andi a0, a0, 255");
        assert_eq!(
            cast("short", "unsigned short"),
            "slli a0, a0, 16; srli a0, a0, 16"
        );
    }

    #[test]
    fn places() {
        // A local: straight to its slot.
        assert_eq!(
            main_body("", "int x; x = 5;"),
            "li a0, 5; sw a0, -12(s0); li a0, 0"
        );
        // A global: through its address.
        assert_eq!(
            main_body("int g;", "g = 5;"),
            "la a0, g; addi sp, sp, -4; sw a0, 0(sp); li a0, 5; lw a1, 0(sp); addi sp, sp, 4; sw a0, 0(a1); \
             li a0, 0"
        );
        // Compound assignment evaluates the target once and keeps its address.
        assert_eq!(
            main_body("char *p;", "*p += 300;"),
            "la a0, p; lw a0, 0(a0); addi sp, sp, -4; sw a0, 0(sp); lw a0, 0(sp); lbu a0, 0(a0); \
             addi sp, sp, -4; sw a0, 0(sp); li a0, 300; lw a1, 0(sp); addi sp, sp, 4; add a0, a1, a0; \
             andi a0, a0, 255; lw a1, 0(sp); addi sp, sp, 4; sb a0, 0(a1); li a0, 0"
        );
        // Increments step by the pointed-to size; postfix gives the old value back.
        assert_eq!(
            main_body("", "int *p; return p++ == 0;").split("; xor").next().unwrap(),
            "lw a0, -12(s0); addi a0, a0, 4; sw a0, -12(s0); addi a0, a0, -4; addi sp, sp, -4; sw a0, 0(sp); \
             li a0, 0; lw a1, 0(sp); addi sp, sp, 4"
        );
        assert_eq!(
            main_body("", "unsigned char c; return --c;"),
            "lbu a0, -9(s0); addi a0, a0, -1; andi a0, a0, 255; sb a0, -9(s0)"
        );
    }

    #[test]
    fn arrays_are_addresses() {
        assert_eq!(
            main_body("", "int a[2]; return a[1];"),
            "addi a0, s0, -16; addi sp, sp, -4; sw a0, 0(sp); li a0, 1; lw a1, 0(sp); \
             addi sp, sp, 4; slli a0, a0, 2; add a0, a1, a0; lw a0, 0(a0)"
        );
        // &a and a are the same address; a global array is `la`.
        assert!(main_body("int g[3];", "return &g == 0;").starts_with("la a0, g; addi sp"));
    }

    #[test]
    fn local_array_initializers() {
        assert_eq!(
            main_body("", "int a[3] = {7}; return a[0];").split("; addi a0, s0").next().unwrap(),
            "addi sp, sp, -16; sw zero, -20(s0); sw zero, -16(s0); sw zero, -12(s0); li a0, 7; sw a0, -20(s0)"
        );
        assert_eq!(
            main_body("", "char s[] = \"a\\0b\"; return s[2];")
                .split("; addi a0, s0")
                .next()
                .unwrap(),
            "sw zero, -12(s0); li a0, 97; sb a0, -12(s0); li a0, 98; sb a0, -10(s0)"
        );
        // Big arrays are cleared with a loop.
        let body = main_body("", "int a[100] = {1}; return a[99];");
        assert!(
            body.contains("addi t1, s0, -408; li t0, 400; add t0, t1, t0; sw zero, 0(t1); addi t1, t1, 4; bltu t1, t0, 2b"),
            "{body}"
        );
    }

    #[test]
    fn global_data() {
        let asm = compile(
            "int g = 5; char c = 'x'; short h[3] = {1, -1}; int *p = &g + 1; char s[] = \"hi\\n\";
             static unsigned u; char *str = \"a\\\"b\\\\\"; char raw[3] = {1, 2, 255}; int zeros[2] = {0};
             int main() { static int n = 3; return n + u; }",
        );
        let data = &asm;
        for want in [
            // No empty `.text` first: sections are switched as needed.
            "    .data\n\n    # int g: 4 bytes\n    .globl g\n    .balign 4\n    .type g, @object\n    .size g, 4\ng:\n    .word 5\n",
            "\n    # char c: 1 byte\n    .globl c\n    .type c, @object\n    .size c, 1\nc:\n    .byte 120\n",
            "\n    .balign 2\n    .type h, @object\n    .size h, 6\nh:\n    .half 1\n    .half -1\n    .zero 2\n",
            "p:\n    .word g+4\n",
            "s:\n    .ascii \"hi\\n\"\n    .zero 1\n",
            "    .bss\n\n    # static unsigned int u: 4 bytes\n    .balign 4\n    .type u, @object\n    .size u, 4\nu:\n    .zero 4\n",
            "str:\n    .word .L.str\n",
            "raw:\n    .byte 1\n    .byte 2\n    .byte 255\n",
            "zeros:\n    .word 0\n    .zero 4\n",
            "\n    # static int n: 4 bytes\n    .balign 4\n    .type main.n, @object\n    .size main.n, 4\nmain.n:\n    .word 3\n",
            "    .section .rodata\n\n.L.str:\n    .asciz \"a\\\"b\\\\\"\n",
        ] {
            assert!(data.contains(want), "missing {want:?} in\n{data}");
        }
        assert!(data.starts_with("    .data\n"), "{data}");
        assert!(!data.contains(".globl u"), "static");
        assert!(!data.contains(".globl main.n"), "static local");
        assert!(asm.contains("la    a0, main.n\n"), "{asm}");
        // An address converted to a 32-bit integer is a `.word`, as clang
        // writes it, moved by bytes.
        let asm = compile(
            "int i[2]; long a = (long)&i; unsigned b = (unsigned)&i[1] - 2; int c = 8 + (int)i;\n\
             char *d = (char *)((long)\"xy\" + 1);",
        );
        for want in [
            "a:\n    .word i\n",
            "b:\n    .word i+2\n",
            "c:\n    .word i+8\n",
            "d:\n    .word .L.str+1\n",
        ] {
            assert!(asm.contains(want), "missing {want:?} in\n{asm}");
        }
    }

    #[test]
    fn string_literals() {
        let asm = compile("char *f() { return \"one\"; } int main() { return f()[0] + \"\\x01\\x02z\"[1]; }");
        assert!(asm.contains("la    a0, .L.str\n"), "{asm}");
        assert!(asm.ends_with(
            "    .section .rodata\n\n.L.str:\n    .asciz \"one\"\n\n.L.str.1:\n    .byte 1, 2\n    .asciz \"z\"\n"
        ), "{asm}");
    }

    #[test]
    fn byte_directive_forms() {
        assert_eq!(byte_directives(b"abc"), [".ascii \"abc\""]);
        assert_eq!(byte_directives(b"abc\0"), [".asciz \"abc\""]);
        assert_eq!(byte_directives(b"\0"), [".byte 0"]);
        assert_eq!(
            byte_directives(b"a\0b\0"),
            [".ascii \"a\"", ".byte 0", ".asciz \"b\""]
        );
        assert_eq!(byte_directives(&[200, 201]), [".byte 200, 201"]);
        assert_eq!(byte_directives(&[1; 20]).len(), 2, "16 per line");
    }

    #[test]
    fn calls_pad_the_stack_to_16_bytes() {
        // No temporaries pushed: no padding.
        let asm = compile("int f(int a) { return a; } int main() { return f(1); }");
        let body = body_of(&asm, "main");
        assert_eq!(&body[4..], ["li a0, 1", "call f"]);
        // `1 + f(2)`: 1 is pushed (4 bytes), so 12 more bytes keep sp aligned.
        let asm = compile("int f(int a) { return a; } int main() { return 1 + f(2); }");
        let body = body_of(&asm, "main").join("; ");
        assert!(
            body.contains("li a0, 2; addi sp, sp, -12; call f; addi sp, sp, 12; lw a1, 0(sp)"),
            "{body}"
        );
    }

    #[test]
    fn arguments_are_evaluated_before_loading_registers() {
        let asm = compile("int f(int a, int b) { return a - b; } int main() { return f(f(5, 1), f(2, 3)); }");
        let body = body_of(&asm, "main").join("; ");
        // Both inner calls happen before a0/a1 are loaded for the outer one.
        let outer = body.rfind("call f").unwrap();
        assert!(
            body[..outer].ends_with("call f; addi sp, sp, 12; mv a1, a0; lw a0, 0(sp); addi sp, sp, 4; "),
            "{body}"
        );
    }

    #[test]
    fn arguments_and_results_are_converted() {
        let asm = compile("char f(char c) { return c + 1; } int main() { return f(300); }");
        // The argument is cut to a char (a constant: at compile time), and so is the result.
        assert_eq!(body_of(&asm, "main")[4..].join("; "), "li a0, 44; call f");
        assert!(body_of(&asm, "f")
            .join("; ")
            .ends_with("add a0, a1, a0; andi a0, a0, 255"));
    }

    #[test]
    fn labels_are_unique_across_functions() {
        let asm = compile(
            "int f(int a) { while (a) a = a - 1; do a++; while (a < 3); return a ? 1 : 2; }
             int main() { int i; for (i = 0; i < 3; i = i + 1) if (i) continue; else break; return f(i); }",
        );
        let mut defined: Vec<&str> = asm.lines().filter(|l| l.ends_with(':')).collect();
        let n = defined.len();
        defined.sort();
        defined.dedup();
        assert_eq!(defined.len(), n, "{asm}");
        assert!(asm.contains("j     .L.continue."), "{asm}");
        assert!(asm.contains("j     .L.break."), "{asm}");
        assert!(asm.contains("bnez  a0, .L.do."), "{asm}");
    }

    #[test]
    fn short_circuit_operators() {
        let asm = compile("int main(int a, int b) { return a && b; }");
        let body = body_of(&asm, "main").join("; ");
        assert!(
            body.ends_with("lw a0, -12(s0); beqz a0, .L.and.1; lw a0, -16(s0); snez a0, a0"),
            "{body}"
        );
        let asm = compile("int main(int a, int b) { return a || b; }");
        let body = body_of(&asm, "main").join("; ");
        assert!(
            body.ends_with("lw a0, -12(s0); bnez a0, .L.or.1; lw a0, -16(s0); snez a0, a0"),
            "{body}"
        );
    }

    #[test]
    fn big_frames_use_t0() {
        let decls: String = (0..600).map(|i| format!("int v{i} = {i};")).collect();
        let asm = compile(&format!("int main() {{ {decls} return v599; }}"));
        // 8 + 600 * 4 = 2408 bytes, rounded up to 2416; 16 come from the save area.
        let body = body_of(&asm, "main").join("; ");
        assert!(body.contains("li t0, -2400; add sp, sp, t0"), "{body}");
        // v509 is at -12 - 4 * 509 = -2048, the last slot a 12-bit offset reaches.
        assert!(body.contains("li a0, 509; sw a0, -2048(s0)"), "{body}");
        assert!(
            body.contains("li a0, 510; li t0, -2052; add t0, s0, t0; sw a0, 0(t0)"),
            "{body}"
        );
        assert!(
            body.ends_with("li t0, -2408; add t0, s0, t0; lw a0, 0(t0)"),
            "{body}"
        );
    }

    #[test]
    fn far_branches_are_rewritten() {
        let lines = |n| {
            let mut v = vec![Line::Branch {
                op: "beqz",
                regs: "a0",
                target: "end".into(),
            }];
            v.extend((0..n).map(|_| Line::Insn("nop".into(), 4)));
            v.push(Line::Label("end".into()));
            v
        };
        // The branch and 1022 words before the label: +4092 bytes, in reach.
        assert!(!relax_branches(&lines(1022))[0]);
        // +4096 is not (forward, the limit is +4094).
        assert!(relax_branches(&lines(1023))[0]);

        // Backward: a label then a branch to it.
        let back = |n| {
            let mut v = vec![Line::Label("top".into())];
            v.extend((0..n).map(|_| Line::Insn("nop".into(), 4)));
            v.push(Line::Branch {
                op: "bnez",
                regs: "a0",
                target: "top".into(),
            });
            v
        };
        assert!(!*relax_branches(&back(1024)).last().unwrap()); // -4096
        assert!(*relax_branches(&back(1025)).last().unwrap()); // -4100

        let l = lines(2000);
        let mut out = String::new();
        render(&l[..1], &relax_branches(&l)[..1], &mut out);
        assert_eq!(out, "    bnez  a0, 1f\n    j     end\n1:\n");
    }

    #[test]
    fn making_one_branch_long_can_push_another_out_of_reach() {
        // Branch A spans branch B and 1022 words: exactly in reach, until B
        // grows to two words. B spans 2000 words and is long from the start.
        let mut v = vec![Line::Branch {
            op: "beqz",
            regs: "a0",
            target: "a".into(),
        }];
        v.push(Line::Branch {
            op: "beqz",
            regs: "a0",
            target: "b".into(),
        });
        v.extend((0..1021).map(|_| Line::Insn("nop".into(), 4)));
        v.push(Line::Label("a".into()));
        v.extend((0..2000).map(|_| Line::Insn("nop".into(), 4)));
        v.push(Line::Label("b".into()));
        assert_eq!(&relax_branches(&v)[..2], [true, true]);
    }

    #[test]
    fn a_function_must_fit_in_a_jumps_reach() {
        // Each statement is about 56 bytes of code: 18000 of them stay under
        // 1 MiB, 19000 do not, and the error names the function, not one of
        // the internal labels merlin-as would complain about.
        let src = |n| format!("int f(int x) {{ {} return x; }}", "x = x * 3 + 1; ".repeat(n));
        assert!(crate::compile(&src(18_000)).is_ok());
        let e = crate::compile(&src(19_000)).unwrap_err();
        assert_eq!((e[0].pos.line, e[0].pos.col), (1, 5));
        assert!(
            e[0].msg.starts_with("function 'f' is too big: up to ")
                && e[0]
                    .msg
                    .ends_with(" bytes of code, and the jumps inside a function reach only 1 MiB"),
            "{}",
            e[0].msg
        );
        // The estimate counts long branches at 8 bytes.
        let v = vec![
            Line::Branch {
                op: "beqz",
                regs: "a0",
                target: "end".into(),
            },
            Line::Insn("li a0, 0x12345".into(), 8),
            Line::Label("end".into()),
            Line::Text(".size f, . - f".into()),
        ];
        assert_eq!(code_size(&v, &[false; 4]), 12);
        assert_eq!(code_size(&v, &[true, false, false, false]), 16);
    }

    #[test]
    fn far_compare_branches_are_rewritten() {
        let mut v = vec![Line::Branch {
            op: "beq",
            regs: "a0, t0",
            target: "far".into(),
        }];
        v.extend((0..2000).map(|_| Line::Insn("nop".into(), 4)));
        v.push(Line::Label("far".into()));
        let mut out = String::new();
        render(&v[..1], &relax_branches(&v)[..1], &mut out);
        assert_eq!(out, "    bne   a0, t0, 1f\n    j     far\n1:\n");
    }

    #[test]
    fn members_are_at_their_offsets() {
        let s = "struct s { char c; int i; };";
        assert_eq!(
            main_body(s, "struct s v; v.i = 5;"),
            "addi a0, s0, -16; addi a0, a0, 4; addi sp, sp, -4; sw a0, 0(sp); li a0, 5; lw a1, 0(sp); \
             addi sp, sp, 4; sw a0, 0(a1); li a0, 0"
        );
        // `p->i` adds the offset to the pointer's value.
        assert_eq!(
            main_body(s, "struct s *p; return p->i;"),
            "lw a0, -12(s0); addi a0, a0, 4; lw a0, 0(a0)"
        );
        // A struct's value is its address: `.c` at offset 0 adds nothing.
        assert_eq!(
            main_body(s, "struct s v; return v.c;"),
            "addi a0, s0, -16; lbu a0, 0(a0)"
        );
    }

    #[test]
    fn struct_assignment_copies() {
        // Word-aligned: one word at a time, unrolled.
        assert_eq!(
            main_body("struct s { int a, b; } x, y;", "x = y;"),
            "la a0, x; addi sp, sp, -4; sw a0, 0(sp); la a0, y; lw a1, 0(sp); addi sp, sp, 4; \
             lw t0, 0(a0); sw t0, 0(a1); lw t0, 4(a0); sw t0, 4(a1); mv a0, a1; li a0, 0"
        );
        // Aligned to 2: halves, so no access is misaligned.
        assert!(main_body("struct s { short a, b, c; } x, y;", "x = y;").contains(
            "lhu t0, 0(a0); sh t0, 0(a1); lhu t0, 2(a0); sh t0, 2(a1); lhu t0, 4(a0); sh t0, 4(a1)"
        ));
        // Big: a loop.
        let body = main_body("struct s { int v[40]; } x, y;", "x = y;");
        assert!(
            body.contains(
                "mv t1, a0; mv t2, a1; li t3, 160; add t3, t1, t3; lw t0, 0(t1); sw t0, 0(t2); \
                 addi t1, t1, 4; addi t2, t2, 4; bltu t1, t3, 2b"
            ),
            "{body}"
        );
    }

    #[test]
    fn small_structs_travel_in_registers() {
        let asm = compile(
            "struct s { int a, b; }; struct s f(struct s x) { return x; }
             int main() { struct s v; return f(v).b; }",
        );
        // The callee stores both words, and returns them from the struct's address.
        assert_eq!(
            body_of(&asm, "f")[4..],
            [
                "sw a0, -16(s0)",
                "sw a1, -12(s0)",
                "addi a0, s0, -16",
                "mv t1, a0",
                "lw a0, 0(t1)",
                "lw a1, 4(t1)",
            ]
        );
        // The caller loads them, and keeps the result in a hidden local.
        assert_eq!(
            body_of(&asm, "main")[5..].join("; "),
            "addi a0, s0, -16; mv t1, a0; lw a0, 0(t1); lw a1, 4(t1); call f; sw a0, -24(s0); \
             sw a1, -20(s0); addi a0, s0, -24; addi a0, a0, 4; lw a0, 0(a0)"
        );
        assert!(
            asm.contains("#     -24(s0)  temp struct s (call result)\n"),
            "{asm}"
        );
        // Aligned to less than a word: each word is put together from aligned pieces.
        let asm =
            compile("struct s { char c[3]; }; int f(struct s x); int main() { struct s v; return f(v); }");
        assert!(
            body_of(&asm, "main").join("; ").contains(
                "mv t1, a0; lbu a0, 0(t1); lbu t0, 1(t1); slli t0, t0, 8; or a0, a0, t0; \
                 lbu t0, 2(t1); slli t0, t0, 16; or a0, a0, t0; call f"
            ),
            "{asm}"
        );
    }

    #[test]
    fn big_structs_travel_by_reference() {
        let asm = compile(
            "struct b { int v[3]; }; struct b f(int k, struct b x) { return x; }
             int main() { struct b v; return f(1, v).v[2]; }",
        );
        // The callee keeps the result's address (a0) and copies its argument
        // from the caller's copy (a2: the result address took a0).
        let f = body_of(&asm, "f").join("; ");
        assert!(
            f.starts_with(
                "addi sp, sp, -16; sw ra, 12(sp); sw s0, 8(sp); addi s0, sp, 16; addi sp, sp, -16; \
                 sw a0, -28(s0); sw a1, -12(s0); addi t4, s0, -24; lw t0, 0(a2); sw t0, 0(t4)"
            ),
            "{f}"
        );
        // `return x` copies x to where the caller asked.
        assert!(
            f.ends_with(
                "addi a0, s0, -24; lw a1, -28(s0); lw t0, 0(a0); sw t0, 0(a1); lw t0, 4(a0); \
                 sw t0, 4(a1); lw t0, 8(a0); sw t0, 8(a1)"
            ),
            "{f}"
        );
        // The caller copies the argument, passes the copy's address and the result's.
        let main = body_of(&asm, "main").join("; ");
        assert!(
            main.contains(
                "addi a0, s0, -20; addi a1, s0, -32; lw t0, 0(a0); sw t0, 0(a1); lw t0, 4(a0); \
                 sw t0, 4(a1); lw t0, 8(a0); sw t0, 8(a1); mv a0, a1; mv a2, a0; lw a1, 0(sp); \
                 addi sp, sp, 4; addi a0, s0, -44; call f; addi a0, s0, -44"
            ),
            "{main}"
        );
    }

    #[test]
    fn calls_through_pointers() {
        let asm = compile("int f(int a, int b); int main() { int (*p)(int, int) = f; return p(1, 2); }");
        assert_eq!(
            body_of(&asm, "main")[4..].join("; "),
            "la a0, f; sw a0, -12(s0); lw a0, -12(s0); addi sp, sp, -4; sw a0, 0(sp); li a0, 1; \
             addi sp, sp, -4; sw a0, 0(sp); li a0, 2; mv a1, a0; lw a0, 0(sp); addi sp, sp, 4; \
             lw t1, 0(sp); addi sp, sp, 4; jalr t1"
        );
        let asm = compile("int main() { int (*p)(void) = 0; return p(); }");
        assert!(body_of(&asm, "main")
            .join("; ")
            .ends_with("lw a0, -12(s0); mv t1, a0; jalr t1"));
        // A function table is data with relocations.
        let asm = compile("int f(void); int g(void); int (*t[2])(void) = {f, g}; int main() { return 0; }");
        assert!(asm.contains("t:\n    .word f\n    .word g\n"), "{asm}");
    }

    #[test]
    fn switch_is_a_compare_chain() {
        let asm = compile(
            "int main(int x) { switch (x) { case 1: return 5; case -2: default: break; } return 0; }",
        );
        assert_eq!(
            body_of(&asm, "main")[5..].join("; "),
            "lw a0, -12(s0); li t0, 1; beq a0, t0, .L.case.main.0; li t0, -2; beq a0, t0, .L.case.main.1; \
             j .L.case.main.2; li a0, 5; j .L.return.main; j .L.break.1; li a0, 0"
        );
        assert!(asm.contains("\n.L.case.main.1:\n.L.case.main.2:\n"), "{asm}");
        // No default: past the body.
        let asm = compile("int main(unsigned x) { switch (x) { case 0xffffffff: x = 1; } return x; }");
        assert!(
            body_of(&asm, "main")
                .join("; ")
                .contains("li t0, -1; beq a0, t0, .L.case.main.0; j .L.break.1"),
            "{asm}"
        );
    }

    #[test]
    fn goto_jumps_to_function_labels() {
        let asm = compile("int main() { goto out; out: return 1; }");
        assert_eq!(body_of(&asm, "main")[4..], ["j .L.label.main.out", "li a0, 1"]);
        assert!(asm.contains("\n.L.label.main.out:\n"), "{asm}");
    }

    #[test]
    fn bool_conversions() {
        let cast = |from: &str| {
            body_of(
                &compile(&format!("int main({from} x) {{ return (_Bool)x; }}")),
                "main",
            )[6..]
                .join("; ")
        };
        assert_eq!(cast("int"), "snez a0, a0");
        assert_eq!(cast("char *"), "snez a0, a0");
        assert_eq!(cast("_Bool"), "");
        assert_eq!(
            main_body("", "_Bool b = 5; return b;"),
            "li a0, 1; sb a0, -9(s0); lbu a0, -9(s0)"
        );
        // `b--` cannot be undone from the new value: the old one is kept in t1.
        assert_eq!(
            main_body("", "_Bool b; return b--;"),
            "lbu a0, -9(s0); mv t1, a0; addi a0, a0, -1; snez a0, a0; sb a0, -9(s0); mv a0, t1"
        );
    }

    #[test]
    fn qualified_objects() {
        let asm = compile(
            "const int k = 3; const char msg[] = \"hi\"; const int z; volatile int v = 1; const volatile int cv = 2;
             int main() { return k + v; }",
        );
        for want in [
            "    .section .rodata\n\n    # const int k: 4 bytes\n    .globl k\n    .balign 4\n    .type k, @object\n    .size k, 4\nk:\n    .word 3\n",
            "msg:\n    .ascii \"hi\"\n    .zero 1\n",
            "z:\n    .zero 4\n",
            "    .data\n\n    # volatile int v: 4 bytes\n",
            "# const volatile int cv: 4 bytes",
        ] {
            assert!(asm.contains(want), "missing {want:?} in\n{asm}");
        }
        // `cv` is in `.data`, after `v`; the tentative `z` comes last.
        let before = &asm[..asm.find("cv:").unwrap()];
        assert!(
            before.rfind("    .data\n") > before.rfind(".section .rodata"),
            "{asm}"
        );
    }

    /// Volatile objects are memory-mapped registers: every access in the
    /// source must be exactly one load or store, in source order, even when
    /// its value is discarded or it repeats the previous one. merlin has no
    /// optimizer, so today this holds by construction; the test keeps it so.
    #[test]
    fn volatile_accesses_are_kept() {
        // The loads and stores of `f` that do not go through sp or s0 (the
        // frame and the expression stack): here, exactly the volatile ones.
        let accesses = |src: &str, f: &str| -> Vec<String> {
            body_of(&compile(src), f)
                .into_iter()
                .filter(|l| {
                    let op = l.split(' ').next().unwrap();
                    matches!(op, "lw" | "lh" | "lhu" | "lb" | "lbu" | "sw" | "sh" | "sb")
                        && !l.ends_with("(sp)")
                        && !l.ends_with("(s0)")
                })
                .collect()
        };
        // `v; v;` are two reads, `v = 1; v = 1;` two writes, `v + v` two
        // more reads.
        assert_eq!(
            accesses(
                "volatile int v; int main() { v; v; v = 1; v = 1; return v + v; }",
                "main"
            ),
            [
                "lw a0, 0(a0)",
                "lw a0, 0(a0)",
                "sw a0, 0(a1)",
                "sw a0, 0(a1)",
                "lw a0, 0(a0)",
                "lw a0, 0(a0)"
            ]
        );
        // Through a pointer to a volatile struct and a cast of an address,
        // as a driver writes them.
        let src = "struct regs { int status; short data; };
             int g(volatile struct regs *r) {
                 r->data; (void)r->status; r->data = 5; r->data = 5;
                 *(volatile int *)0x10000000; *(volatile unsigned char *)0x10000000 |= 4;
                 while (r->status & 1) {}
                 return r->data + r->data;
             }";
        assert_eq!(
            accesses(src, "g"),
            [
                "lh a0, 0(a0)",  // r->data;
                "lw a0, 0(a0)",  // (void)r->status;
                "sh a0, 0(a1)",  // r->data = 5;
                "sh a0, 0(a1)",  // r->data = 5;
                "lw a0, 0(a0)",  // *(volatile int *)0x10000000;
                "lbu a0, 0(a0)", // ... |= 4 reads once
                "sb a0, 0(a1)",  // and writes once
                "lw a0, 0(a0)",  // r->status, in the loop condition
                "lh a0, 0(a0)",  // r->data +
                "lh a0, 0(a0)",  // r->data
            ]
        );
    }

    #[test]
    fn struct_data() {
        let asm = compile(
            "struct s { char c; int i; short h[3]; }; struct s g = {1, 2, {3}}; union u { char c; int i; } w = {7};
             int main() { return 0; }",
        );
        assert!(
            asm.contains(
                "g:\n    .byte 1\n    .zero 3\n    .word 2\n    .half 3\n    .zero 4\n    .zero 2\n"
            ),
            "{asm}"
        );
        assert!(asm.contains("w:\n    .byte 7\n    .zero 3\n"), "{asm}");
    }

    #[test]
    fn inline_definitions_are_local() {
        let asm = compile("inline int f(void) { return 1; } extern inline int g(void) { return 2; } int main() { return f(); }");
        assert!(!asm.contains(".globl f"), "{asm}");
        assert!(asm.contains(".globl g"), "{asm}");
    }

    #[test]
    fn register_names_cannot_be_symbols() {
        for name in ["sp", "a0", "x5", "fp", "zero"] {
            for src in [
                format!("int {name}() {{ return 0; }} int main() {{ return 0; }}"),
                format!("int {name}; int main() {{ return 0; }}"),
            ] {
                let errs = crate::compile(&src).unwrap_err();
                assert_eq!(
                    errs[0].to_string(),
                    format!("1:5: '{name}' is a register name, which cannot be used as a symbol name")
                );
            }
        }
        // Locals may have any name: they have no symbol.
        compile("int main() { int a0 = 1; return a0; }");
    }

    #[test]
    fn constant_expressions_are_folded() {
        // Every integer type, as clang's front end folds them at -O0: a
        // long long is a pair, its low word in a0.
        let f = |ty: &str, e: &str| {
            let asm = compile(&format!("{ty} f(void) {{ return {e}; }}"));
            body_of(&asm, "f")[4..].join("; ")
        };
        assert_eq!(f("long", "8 * (1024ul * 1024ul)"), "li a0, 8388608");
        assert_eq!(f("int", "8 * (1024 * 1024)"), "li a0, 8388608");
        assert_eq!(f("unsigned long", "0ul - 1"), "li a0, -1");
        assert_eq!(
            f("long", "2147483647L + 1"),
            "li a0, -2147483648",
            "wraps at 32 bits"
        );
        assert_eq!(f("long long", "3LL << 40"), "li a0, 0; li a1, 768");
        assert_eq!(
            f("unsigned long long", "(0ull - 1) / 3 + 1"),
            "li a0, 1431655766; li a1, 1431655765"
        );
        assert_eq!(f("long long", "-1LL >> 63"), "li a0, -1; li a1, -1");
        assert_eq!(
            f("long long", "1LL << 65"),
            "li a0, 2; li a1, 0",
            "the low 6 bits of the amount"
        );
        assert_eq!(f("long long", "(long long)-1"), "li a0, -1; li a1, -1");
        assert_eq!(f("int", "(int)(1LL << 32 | 7)"), "li a0, 7");
        assert_eq!(f("int", "(0LL - 1) < 0ull"), "li a0, 0", "an unsigned comparison");
        assert_eq!(f("char", "(char)300 + 1"), "li a0, 45");
        assert_eq!(f("unsigned", "sizeof(long long) * 3"), "li a0, 24");
        assert_eq!(f("int", "1 && 2LL || 0"), "li a0, 1");
        // The operand `?:` does not choose is not evaluated: it need not be
        // constant (C99 6.6p3), only its type counts, as clang folds it.
        let g = |src: &str| body_of(&compile(src), "f")[4..].join("; ");
        assert_eq!(g("int h(void); int f(void) { return 1 ? 2 : h(); }"), "li a0, 2");
        assert_eq!(
            g("unsigned h(void); unsigned long long f(void) { return 0 ? h() : -1; }"),
            "li a0, -1; li a1, 0",
            "-1 converted to the operands' common type, unsigned int"
        );
        assert_eq!(
            g("long long h(void); long long f(void) { return 1 ? -1 : h(); }"),
            "li a0, -1; li a1, -1"
        );
        let asm = compile("int h(void); int x = 1 ? 2 : h(); int y = 0 && h();");
        assert!(
            asm.contains(
                "x:
    .word 2
"
            ),
            "{asm}"
        );
        assert!(
            asm.contains(
                "y:
    .word 0
"
            ),
            "{asm}"
        );
        // Division by zero is left to run time; the rest is folded.
        assert_eq!(
            f("int", "(2 * 3) / 0"),
            "li a0, 6; addi sp, sp, -4; sw a0, 0(sp); li a0, 0; lw a1, 0(sp); addi sp, sp, 4; div a0, a1, a0"
        );
        // Only the constant parts of an expression, and nothing reassociated.
        let asm = compile("int x; int f(void) { return x * (2 * 3) + x * 2 * 3; }");
        let body = body_of(&asm, "f").join("; ");
        assert_eq!(body.matches("mul ").count(), 3, "{body}");
        assert!(body.contains("li a0, 6"), "{body}");
    }

    /// `long long` on RV32: register pairs.
    mod pairs {
        use super::{body_of, compile};

        /// The instructions of `f` after the prologue and the parameters'
        /// stores and loads, up to the epilogue.
        fn tail(src: &str, f: &str, skip: usize) -> String {
            body_of(&compile(src), f)[skip..].join("; ")
        }

        #[test]
        fn arithmetic_across_the_words() {
            let op = |e: &str| {
                tail(
                    &format!("long long f(long long a, long long b) {{ return {e}; }}"),
                    "f",
                    19,
                )
            };
            assert_eq!(
                op("a + b"),
                "add a0, a2, a0; sltu t0, a0, a2; add a1, a3, a1; add a1, a1, t0"
            );
            assert_eq!(
                op("a - b"),
                "sltu t0, a2, a0; sub a0, a2, a0; sub a1, a3, a1; sub a1, a1, t0"
            );
            assert_eq!(
                op("a * b"),
                "mul t0, a3, a0; mul t1, a2, a1; add t0, t0, t1; mulhu t1, a2, a0; add a1, t0, t1; mul a0, a2, a0"
            );
            assert_eq!(op("a & b"), "and a0, a2, a0; and a1, a3, a1");
            // Division calls libgcc's routines, with the operands in a0:a1 and a2:a3.
            assert_eq!(
                op("a / b"),
                "mv t0, a0; mv t1, a1; mv a0, a2; mv a1, a3; mv a2, t0; mv a3, t1; call __divdi3"
            );
            assert!(tail(
                "unsigned long long f(unsigned long long a, unsigned long long b) { return a % b; }",
                "f",
                19
            )
            .ends_with("call __umoddi3"));
            // Comparisons: the high words, then the low ones unsigned.
            let cmp = tail("int f(long long a, long long b) { return a < b; }", "f", 19);
            assert_eq!(
                cmp,
                "slt t0, a3, a1; xor t1, a3, a1; seqz t1, t1; sltu t2, a2, a0; and t1, t1, t2; or a0, t0, t1"
            );
            let eq = tail("int f(long long a, long long b) { return a == b; }", "f", 19);
            assert_eq!(eq, "xor t0, a2, a0; xor t1, a3, a1; or a0, t0, t1; seqz a0, a0");
        }

        #[test]
        fn shifts_by_whether_the_amount_reaches_32() {
            let body = tail("long long f(long long a, int n) { return a << n; }", "f", 17);
            assert!(
                body.starts_with(
                    "andi a0, a0, 63; andi t0, a0, 32; beqz t0, .L.shl.1; sll a1, a2, a0; li a0, 0; j .L.shift.2; \
                     not t2, a0; sll a1, a3, a0; srli t1, a2, 1; srl t1, t1, t2; or a1, a1, t1; sll a0, a2, a0"
                ),
                "{body}"
            );
            let body = tail("long long f(long long a, int n) { return a >> n; }", "f", 17);
            assert!(body.contains("sra a0, a3, a0; srai a1, a3, 31"), "{body}");
            let body = tail(
                "unsigned long long f(unsigned long long a, int n) { return a >> n; }",
                "f",
                17,
            );
            assert!(body.contains("srl a0, a3, a0; li a1, 0"), "{body}");
        }

        #[test]
        fn conversions() {
            // To a pair: the sign, or 0; from one, the low word.
            let cast =
                |from: &str, to: &str| tail(&format!("{to} f({from} x) {{ return ({to})x; }}"), "f", 6);
            assert_eq!(cast("int", "long long"), "srai a1, a0, 31");
            assert_eq!(cast("unsigned", "long long"), "li a1, 0");
            assert_eq!(cast("unsigned char", "long long"), "srai a1, a0, 31");
            assert_eq!(cast("char *", "unsigned long long"), "li a1, 0");
            let cast =
                |from: &str, to: &str| tail(&format!("{to} f({from} x) {{ return ({to})x; }}"), "f", 8);
            assert_eq!(cast("long long", "int"), "");
            assert_eq!(cast("long long", "short"), "slli a0, a0, 16; srai a0, a0, 16");
            assert_eq!(cast("long long", "_Bool"), "or a0, a0, a1; snez a0, a0");
            // A constant is two li.
            assert_eq!(
                tail("long long f(void) { return 0x123456789LL; }", "f", 4),
                "li a0, 591751049; li a1, 1"
            );
        }

        #[test]
        fn arguments_take_pairs_of_words() {
            // A named pair may start in a7, its high word at 0(s0).
            let body = body_of(
                &compile(
                    "long long f(int a, int b, int c, int d, int e, int f, int g, long long h) { return h; }",
                ),
                "f",
            );
            assert!(
                body.join("; ")
                    .contains("sw a7, -48(s0); lw t1, 0(s0); sw t1, -44(s0)"),
                "{body:?}"
            );
            // A variadic one starts in an even register: 5LL skips a1.
            let body = tail(
                "long long v(int n, ...); int f(void) { return v(1, 5LL); }",
                "f",
                4,
            );
            assert_eq!(
                body,
                "li a0, 1; addi sp, sp, -4; sw a0, 0(sp); li a0, 5; li a1, 0; mv a3, a1; mv a2, a0; \
                 lw a0, 0(sp); addi sp, sp, 4; call v"
            );
            // va_arg rounds the pointer up to 8 bytes first.
            let body = tail(
                "long long f(int n, ...) { __builtin_va_list ap; __builtin_va_start(ap, n); \
                 return __builtin_va_arg(ap, long long); }",
                "f",
                14,
            );
            assert!(
                body.contains("addi a0, a0, 7; andi a0, a0, -8; mv t1, a0; addi a0, a0, 8"),
                "{body}"
            );
            assert!(body.ends_with("lw a0, 0(t1); lw a1, 4(t1)"), "{body}");
        }
    }

    /// The RV64 counterparts of the tests above: LP64 sizes, the W
    /// instructions and the 32-bit invariant, doubleword registers.
    mod rv64 {
        use super::body_of;
        use crate::types::Xlen;

        fn compile(src: &str) -> String {
            let opts = crate::Options {
                xlen: Xlen::Rv64,
                ..crate::Options::default()
            };
            crate::compile_with(crate::INPUT_NAME, src, &opts, &crate::no_files)
                .unwrap_or_else(|e| panic!("{e:?}"))
        }

        /// The instructions `main` runs for `body`, after the prologue.
        fn main_body(globals: &str, body: &str) -> String {
            let asm = compile(&format!("{globals}\nint main() {{ {body} }}"));
            body_of(&asm, "main")[4..].join("; ")
        }

        #[test]
        fn smallest_function() {
            // ra and s0 take the top 16 bytes: -8(s0) and -16(s0).
            assert_eq!(
                body_of(&compile("int main() { return 42; }"), "main"),
                [
                    "addi sp, sp, -16",
                    "sd ra, 8(sp)",
                    "sd s0, 0(sp)",
                    "addi s0, sp, 16",
                    "li a0, 42"
                ]
            );
            let asm = compile("int main() { return 42; }");
            assert!(
                asm.ends_with(
                    ".L.return.main:\n    addi  sp, s0, -16\n    ld    ra, 8(sp)\n    ld    s0, 0(sp)\n    \
                     addi  sp, sp, 16\n    ret\n    .size main, . - main\n"
                ),
                "{asm}"
            );
            let obj = merlin_as::assemble_with(&asm, &merlin_as::Options { xlen: Xlen::Rv64 }).unwrap();
            let main = obj.symbols.iter().find(|s| s.name == "main").unwrap();
            assert_eq!(main.size, 40);
        }

        #[test]
        fn frame_layout() {
            let locals = |decls: &str| {
                let opts = crate::Options {
                    xlen: Xlen::Rv64,
                    ..crate::Options::default()
                };
                let src = format!("int f() {{ {decls} }}");
                let p = crate::parse_with(crate::INPUT_NAME, &src, &opts, &crate::no_files).unwrap();
                let f = p.funcs().next().unwrap();
                super::super::layout(&f.locals, &vec![true; f.locals.len()], 16, Xlen::Rv64).unwrap()
            };
            // Locals start below the 16 bytes of ra and s0.
            assert_eq!(locals("int a, b; long c;"), (vec![-20, -24, -32], 32));
            assert_eq!(locals("char *p; char c;"), (vec![-24, -25], 32));
            // Arrays and structs are aligned to 8, structs a whole number of doublewords.
            assert_eq!(locals("char c; char buf[5];"), (vec![-17, -24], 32));
            assert_eq!(locals("struct { int a, b, c; } s;"), (vec![-32], 32));
        }

        #[test]
        fn int_arithmetic_uses_the_w_instructions() {
            let op = |ty: &str, src: &str| {
                let body = body_of(
                    &compile(&format!("{ty} f({ty} x, {ty} y) {{ return {src}; }}")),
                    "f",
                );
                // Skip the prologue (5 lines: the frame is 32 bytes), the
                // stores (2), the load of x, its push (2) and the load of y;
                // then the pop.
                body[11..]
                    .join("; ")
                    .replace("ld a1, 0(sp); addi sp, sp, 8; ", "")
            };
            assert_eq!(op("int", "x + y"), "addw a0, a1, a0");
            assert_eq!(op("int", "x - y"), "subw a0, a1, a0");
            assert_eq!(op("int", "x * y"), "mulw a0, a1, a0");
            assert_eq!(op("int", "x / y"), "divw a0, a1, a0");
            assert_eq!(op("int", "x % y"), "remw a0, a1, a0");
            assert_eq!(op("int", "x << y"), "sllw a0, a1, a0");
            assert_eq!(op("int", "x >> y"), "sraw a0, a1, a0");
            assert_eq!(op("unsigned", "x / y"), "divuw a0, a1, a0");
            assert_eq!(op("unsigned", "x % y"), "remuw a0, a1, a0");
            assert_eq!(op("unsigned", "x >> y"), "srlw a0, a1, a0");
            // Bitwise operators and comparisons keep sign-extended values right.
            assert_eq!(op("int", "x & y"), "and a0, a1, a0");
            assert_eq!(op("unsigned", "x < y"), "sltu a0, a1, a0");
            assert_eq!(op("int", "x < y"), "slt a0, a1, a0");
            // long arithmetic is 64-bit.
            assert_eq!(op("long", "x + y"), "add a0, a1, a0");
            assert_eq!(op("long", "x * y"), "mul a0, a1, a0");
            assert_eq!(op("unsigned long", "x / y"), "divu a0, a1, a0");
            assert_eq!(op("long", "x >> y"), "sra a0, a1, a0");
            assert_eq!(op("unsigned long", "x << y"), "sll a0, a1, a0");
            // Negation, and ++.
            assert!(main_body("", "int x = 1; return -x;").ends_with("lw a0, -20(s0); negw a0, a0"));
            assert!(main_body("", "long x = 1; return -x;").contains("ld a0, -24(s0); neg a0, a0"));
            assert!(main_body("", "int x = 1; x++; return x;")
                .contains("lw a0, -20(s0); addiw a0, a0, 1; sw a0, -20(s0)"));
        }

        #[test]
        fn conversions() {
            let cast = |from: &str, to: &str| {
                let body = body_of(&compile(&format!("{to} f({from} x) {{ return ({to})x; }}")), "f");
                // The prologue (5 lines), the parameter's store and load.
                body[7..].join("; ")
            };
            // int and unsigned int are sign-extended in registers: to long
            // only an unsigned int changes, zero-extended.
            assert_eq!(cast("int", "long"), "");
            assert_eq!(cast("unsigned", "long"), "slli a0, a0, 32; srli a0, a0, 32");
            assert_eq!(
                cast("unsigned", "unsigned long"),
                "slli a0, a0, 32; srli a0, a0, 32"
            );
            assert_eq!(cast("int", "unsigned long"), "");
            assert_eq!(cast("unsigned short", "long"), "");
            // Down to 32 bits: sext.w, whichever the signedness.
            assert_eq!(cast("long", "int"), "sext.w a0, a0");
            assert_eq!(cast("unsigned long", "unsigned"), "sext.w a0, a0");
            assert_eq!(cast("char *", "int"), "sext.w a0, a0");
            assert_eq!(cast("int", "unsigned"), "");
            assert_eq!(cast("unsigned", "int"), "");
            // Narrow types: shifts by 64 - width.
            assert_eq!(cast("long", "short"), "slli a0, a0, 48; srai a0, a0, 48");
            assert_eq!(cast("int", "unsigned short"), "slli a0, a0, 48; srli a0, a0, 48");
            assert_eq!(cast("long", "char"), "andi a0, a0, 255");
            assert_eq!(cast("long", "_Bool"), "snez a0, a0");
            // Pointers are 64-bit: an int index is already sign-extended.
            assert_eq!(cast("int", "char *"), "");
            assert_eq!(cast("unsigned", "char *"), "slli a0, a0, 32; srli a0, a0, 32");
        }

        #[test]
        fn mixed_widths_meet_at_the_common_type() {
            // long + int: the int is sign-extended already; long + unsigned:
            // the unsigned is zero-extended first (they meet at long).
            let body = main_body("long l; int i; unsigned u;", "return l + i + u;");
            assert!(
                body.contains("ld a1, 0(sp); addi sp, sp, 8; add a0, a1, a0"),
                "{body}"
            );
            assert!(
                body.contains(
                    "ld a1, 0(sp); addi sp, sp, 8; slli a0, a0, 32; srli a0, a0, 32; add a0, a1, a0"
                ),
                "{body}"
            );
            // int < long compares in 64 bits; the result is an int.
            assert!(main_body("long l; int i;", "return i < l;").ends_with("slt a0, a1, a0"));
        }

        #[test]
        fn loads_and_stores_by_size() {
            assert_eq!(
                main_body("", "long l = 1; char *p = 0; int i = 2; l = i; return p == 0;")
                    .split("; xor")
                    .next()
                    .unwrap(),
                "addi sp, sp, -32; li a0, 1; sd a0, -24(s0); li a0, 0; sd a0, -32(s0); li a0, 2; sw a0, -36(s0); \
                 lw a0, -36(s0); sd a0, -24(s0); ld a0, -32(s0); addi sp, sp, -8; sd a0, 0(sp); li a0, 0; \
                 ld a1, 0(sp); addi sp, sp, 8"
            );
        }

        #[test]
        fn pointer_arithmetic_widens_the_index() {
            let p = |decls: &str, e: &str| {
                let asm = compile(&format!("{decls}\nlong f(void) {{ return {e}; }}"));
                body_of(&asm, "f")[4..].join("; ")
            };
            // An int index is sign-extended already; an unsigned one is
            // zero-extended before it scales.
            assert!(
                p("long *q; int i;", "*(q + i)").ends_with("slli a0, a0, 3; add a0, a1, a0; ld a0, 0(a0)")
            );
            assert!(p("long *q; unsigned i;", "*(q + i)")
                .ends_with("slli a0, a0, 32; srli a0, a0, 32; slli a0, a0, 3; add a0, a1, a0; ld a0, 0(a0)"));
            // The difference of two pointers is a long.
            assert!(p("int *a, *b;", "a - b").ends_with("sub a0, a1, a0; srai a0, a0, 2"));
        }

        #[test]
        fn big_constants() {
            // li takes 64-bit values; an unsigned int is held sign-extended.
            let body = main_body("", "unsigned u; u = 0xffffffffu; return 0;");
            assert!(body.contains("; li a0, -1; sw a0"), "{body}");
            let body = main_body("", "long l; l = 0x123456789abcdef0L; return 0;");
            assert!(body.contains("li a0, 1311768467463790320; sd a0"), "{body}");
            let body = main_body("", "unsigned long u; u = 0xffffffffffffffffUL; return 0;");
            assert!(body.contains("; li a0, -1; sd a0"), "{body}");
            // A constant converted is the converted constant.
            let body = main_body("", "unsigned long u; u = 4294967295u; return 0;");
            assert!(body.contains("; li a0, 4294967295; sd a0"), "{body}");
        }

        #[test]
        fn constant_expressions_are_folded() {
            // Every integer type, as clang's front end folds them at -O0.
            let f = |ty: &str, e: &str| {
                let asm = compile(&format!("{ty} f(void) {{ return {e}; }}"));
                body_of(&asm, "f")[4..].join("; ")
            };
            assert_eq!(f("long", "8 * (1024ul * 1024ul)"), "li a0, 8388608");
            assert_eq!(f("int", "8 * (1024 * 1024)"), "li a0, 8388608");
            assert_eq!(f("long long", "3LL << 40"), "li a0, 3298534883328");
            assert_eq!(
                f("unsigned long long", "(0ull - 1) / 3 + 1"),
                "li a0, 6148914691236517206"
            );
            assert_eq!(f("unsigned long", "0ul - 1"), "li a0, -1");
            assert_eq!(
                f("long", "-9223372036854775807L - 1"),
                "li a0, -9223372036854775808"
            );
            // The 32-bit invariant: an unsigned int is held sign-extended.
            assert_eq!(f("unsigned", "0x80000000u | 1"), "li a0, -2147483647");
            assert_eq!(
                f("long", "(long)(0xffffffffu + 1)"),
                "li a0, 0",
                "unsigned int wraps at 32 bits"
            );
            assert_eq!(f("long", "1L << 65"), "li a0, 2", "the low 6 bits of the amount");
            assert_eq!(
                f("int", "1 << 33"),
                "li a0, 2",
                "the low 5 bits of a 32-bit one's"
            );
            assert_eq!(
                f("long", "(-9223372036854775807L - 1) / -1"),
                "li a0, -9223372036854775808"
            );
            assert_eq!(f("unsigned long", "sizeof(long) * 3"), "li a0, 24");
            assert_eq!(
                f("int", "0 ? 1 / 0 : 2L > 1"),
                "li a0, 1",
                "the operand not chosen is not divided"
            );
            // Division by zero is left to run time; the rest is folded.
            assert_eq!(
                f("long", "(2L * 3) / 0"),
                "li a0, 6; addi sp, sp, -8; sd a0, 0(sp); li a0, 0; ld a1, 0(sp); addi sp, sp, 8; div a0, a1, a0"
            );
        }

        #[test]
        fn global_data() {
            let asm =
                compile("long g = -2; int i = 5; int *p = &i + 1; char *s = \"x\"; unsigned long u = 1;");
            for want in [
                "    # long g: 8 bytes\n    .globl g\n    .balign 8\n    .type g, @object\n    .size g, 8\ng:\n    .quad -2\n",
                "i:\n    .word 5\n",
                "    .balign 8\n    .type p, @object\n    .size p, 8\np:\n    .quad i+4\n",
                "s:\n    .quad .L.str\n",
                "u:\n    .quad 1\n",
            ] {
                assert!(asm.contains(want), "missing {want:?} in\n{asm}");
            }
            // An address converted to a 64-bit integer is a `.quad`, as
            // clang writes it.
            let asm = compile(
                "int i[2]; long a = (long)&i; unsigned long long b = (unsigned long long)&i[1] - 2;\n\
                 long c = 8 + (long)i;",
            );
            for want in ["a:\n    .quad i\n", "b:\n    .quad i+2\n", "c:\n    .quad i+8\n"] {
                assert!(asm.contains(want), "missing {want:?} in\n{asm}");
            }
        }

        #[test]
        fn small_structs_travel_in_doublewords() {
            // 12 bytes: two registers, each a doubleword of the struct (the
            // second one's top half past its end).
            let asm = compile(
                "struct s { int a, b, c; }; struct s f(struct s x) { return x; }
                 int main() { struct s v; return f(v).c; }",
            );
            assert_eq!(
                body_of(&asm, "f")[4..],
                [
                    "addi sp, sp, -16",
                    "sd a0, -32(s0)",
                    "sd a1, -24(s0)",
                    "addi a0, s0, -32",
                    "mv t1, a0",
                    "lwu a0, 0(t1)",
                    "lwu t0, 4(t1)",
                    "slli t0, t0, 32",
                    "or a0, a0, t0",
                    "lwu a1, 8(t1)",
                ]
            );
            // Aligned to 8: whole doublewords.
            let asm = compile("struct s { long a, b; }; long f(struct s x) { return x.b; }");
            assert!(body_of(&asm, "f").starts_with(&[
                "addi sp, sp, -16".to_string(),
                "sd ra, 8(sp)".into(),
                "sd s0, 0(sp)".into(),
                "addi s0, sp, 16".into(),
                "addi sp, sp, -16".into(),
                "sd a0, -32(s0)".into(),
                "sd a1, -24(s0)".into(),
            ]));
            // Bigger than 16 bytes: by reference.
            let asm = compile("struct s { long a, b, c; }; long f(struct s x) { return x.c; }");
            assert!(
                asm.contains("addi  t4, s0, -40\n    ld    t0, 0(a0)\n    sd    t0, 0(t4)\n"),
                "{asm}"
            );
        }

        #[test]
        fn variadic_functions_save_doublewords() {
            let asm = compile(
                "long f(int n, ...) { __builtin_va_list ap; __builtin_va_start(ap, n); return __builtin_va_arg(ap, long); }",
            );
            let body = body_of(&asm, "f");
            // a1-a7 at the top of the frame, 8 bytes each; ra and s0 below them.
            assert_eq!(
                body[..5],
                [
                    "addi sp, sp, -80",
                    "sd ra, 16(sp)",
                    "sd s0, 8(sp)",
                    "addi s0, sp, 80",
                    "addi sp, sp, -16",
                ]
            );
            assert_eq!(body[5], "sd a1, -56(s0)");
            assert_eq!(body[11], "sd a7, -8(s0)");
            let body = body.join("; ");
            assert!(body.contains("addi a0, s0, -56"), "va_start: {body}");
            assert!(
                body.contains("mv t1, a0; addi a0, a0, 8;"),
                "va_arg steps a doubleword: {body}"
            );
            assert!(body.ends_with("ld a0, 0(t1)"), "{body}");
        }

        #[test]
        fn calls_pad_the_stack_to_16_bytes() {
            // One doubleword pushed: 8 bytes of padding around the call.
            let body = main_body("int f(int);", "return 1 + f(2);");
            assert!(
                body.contains("li a0, 2; addi sp, sp, -8; call f; addi sp, sp, 8"),
                "{body}"
            );
            // Two pushed: none.
            let body = main_body("int f(int);", "return 1 + (2 + f(3));");
            assert!(body.contains("li a0, 3; call f"), "{body}");
        }

        #[test]
        fn stack_arguments_are_doublewords() {
            let body = main_body(
                "int f(int a, int b, int c, int d, int e, int f, int g, int h, long i, int j);",
                "return f(1, 2, 3, 4, 5, 6, 7, 8, 9, 10);",
            );
            // Ten pushed doublewords (80 bytes) and two outgoing ones: 96 in all.
            assert!(
                body.contains("addi sp, sp, -16; ld t1, 24(sp); sd t1, 0(sp); ld t1, 16(sp); sd t1, 8(sp); ld a0, 88(sp)"),
                "{body}"
            );
            assert!(body.ends_with("call f; addi sp, sp, 96"), "{body}");
        }

        #[test]
        fn switch_on_unsigned_compares_sign_extended() {
            let body = main_body(
                "unsigned u;",
                "switch (u) { case 0xffffffffu: return 1; } return 0;",
            );
            assert!(body.contains("li t0, -1; beq a0, t0"), "{body}");
            let body = main_body("long l;", "switch (l) { case 0x100000000L: return 1; } return 0;");
            assert!(body.contains("li t0, 4294967296; beq a0, t0"), "{body}");
        }
    }
}
