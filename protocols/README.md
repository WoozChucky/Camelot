# The Wayland protocols

The protocol descriptions Camelot's display server and GUI library follow
(#184, `docs/excalibur-spec.md` section 10, "The display server"), as their
releases ship them, byte for byte (`-text` in `.gitattributes`):

| File | From | SHA-256 |
|---|---|---|
| `wayland.xml` | Wayland 1.23.0, `protocol/wayland.xml` (gitlab.freedesktop.org/wayland/wayland, tag `1.23.0`) | `0c371e9c31f8178008a7ddecf431cbe12ec4b29ef7714803ecd3e20af925e7ed` |
| `xdg-shell.xml` | wayland-protocols 1.38, `stable/xdg-shell/xdg-shell.xml` (gitlab.freedesktop.org/wayland/wayland-protocols, tag `1.38`) | `f241ab95c262eb45af6507bf0178b8dcd88d5e1cc8fc6ec4a9944d9e5b15ac98` |

Their licence is the MIT licence their `<copyright>` elements give.

`cargo xtask kernel` generates `kernel/user/gui/protocol.h` and
`protocol.c` from them (`crates/xtask/src/wayland.rs`), as libwayland's
`wayland-scanner` generates its tables: every interface's requests and
events with their signatures and types, their opcodes, versions and enums.
`crates/xtask/tests/protocol.rs` checks the generated files are current
and these files' checksums.
