# hook: only weakly referred to (main.S), so never linked.
    .text
    .globl hook
hook:
    ret
