    # Every kind of symbol an archive's symbol table lists or leaves out:
    # globals, a weak one, a common one, an absolute one and a hidden one
    # are listed; a local one, an undefined reference and a weak one are
    # not.
    .text
    .globl alpha
alpha:
    ret
    .globl beta
beta:
    ret
    .weak gamma
gamma:
    ret
    .comm common_thing, 8, 8
    .globl absolute_sym
    .set absolute_sym, 42
    .globl hid
    .hidden hid
hid:
    ret
local_one:
    call undefined_fn
    .weak weak_ref
    call weak_ref
