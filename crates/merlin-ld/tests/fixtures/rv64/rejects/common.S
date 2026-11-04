# A common symbol (what C tentative definitions become without -fno-common).
# merlin-ld rejects it. Built by `cargo xtask ld-fixtures`.
    .comm foo, 4, 4
