# A member nothing needs: never linked, though it defines a second strlen,
# which would be a duplicate if it were.
    .text
    .globl unused, strlen
unused:
strlen:
    li    a0, -1
    ret
