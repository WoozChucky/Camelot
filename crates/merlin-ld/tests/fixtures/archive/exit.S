# exit: halts Grail with its argument as the exit code.
    .text
    .globl exit
exit:
    xcal.halt a0
