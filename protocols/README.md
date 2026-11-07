# The Wayland protocols

The protocol descriptions Camelot's display server and GUI library follow
(#184, `docs/excalibur-spec.md` section 10, "The display server"), as their
releases ship them, byte for byte (`-text` in `.gitattributes`):

| File | From | SHA-256 |
|---|---|---|
| `wayland.xml` | Wayland 1.23.0, `protocol/wayland.xml` (gitlab.freedesktop.org/wayland/wayland, tag `1.23.0`) | `0c371e9c31f8178008a7ddecf431cbe12ec4b29ef7714803ecd3e20af925e7ed` |
| `xdg-shell.xml` | wayland-protocols 1.38, `stable/xdg-shell/xdg-shell.xml` (gitlab.freedesktop.org/wayland/wayland-protocols, tag `1.38`) | `f241ab95c262eb45af6507bf0178b8dcd88d5e1cc8fc6ec4a9944d9e5b15ac98` |
| `wlr-layer-shell-unstable-v1.xml` | wlr-protocols, `unstable/wlr-layer-shell-unstable-v1.xml` (gitlab.freedesktop.org/wlroots/wlr-protocols, commit `1106a4a1c57562ddd438c547da4c0d6dc150d624`) | `87e0b9c837aecd6977f76f3c47d73088b7159871f5d979dc1840f6cadb5e2ed8` |
| `wlr-foreign-toplevel-management-unstable-v1.xml` | wlr-protocols, `unstable/wlr-foreign-toplevel-management-unstable-v1.xml` (the same commit) | `a5a030786a9281e90542eb7c4084f0dbd6a50a7a2975cafa01fa848eebe6de15` |

Their licences are the ones their `<copyright>` elements give: MIT for
Wayland's and wayland-protocols', and for wlroots' two an MIT-style
licence (Historical Permission Notice and Disclaimer).

wlroots' protocols are the desktop shell's (#185): the panel is a layer
surface (`zwlr_layer_shell_v1`: a bar at the top of the screen that
windows stay off, and the launcher's menu above them), and its window list
follows the windows through `zwlr_foreign_toplevel_manager_v1`, as
waybar's and sfwbar's do on wlroots' compositors.

`cargo xtask kernel` generates `kernel/user/gui/protocol.h` and
`protocol.c` from them (`crates/xtask/src/wayland.rs`), as libwayland's
`wayland-scanner` generates its tables: every interface's requests and
events with their signatures and types, their opcodes, versions and enums.
`crates/xtask/tests/protocol.rs` checks the generated files are current
and these files' checksums.
