# The GUI library

The library of the programs that draw: software rendering into 32-bit
pixel buffers, PSF2 text, QOI images, a US keyboard layout, widgets, and a
Wayland client. Every function's contract is in `kernel/user/gui.h`; the
spec's section 10, "The GUI library", says what it does as a whole. It is
written in merlin's C, integer arithmetic only, and builds with clang for
the reference build (`-O2 -Wall -Wextra -Werror`). `cargo xtask kernel` and
`cargo xtask fs` archive it as `libgui.a` (`xtask::userprogs`), a member a
file, linked into every program before `libc.a`: a program takes only the
members it uses. The root image installs it as `/usr/lib/libgui.a` with
`/usr/include/gui.h`, so a program is linked on Camelot with
`-L/usr/lib -lgui -lc`.

| File | What it has |
|---|---|
| `rect.c` | rectangles: `gui_rect`, `gui_rect_empty`, `gui_rect_contains`, `gui_rect_intersect`, `gui_rect_union`, `gui_rect_covers` |
| `draw.c` | buffers (`gui_buffer_init`, `gui_buffer_alloc`, `gui_buffer_free`), the clip rectangle (`gui_clip`, `gui_unclip`), and drawing: `gui_plot`, `gui_fill`, `gui_fill_blend`, `gui_outline`, `gui_line`, `gui_fill_round`, `gui_outline_round`, `gui_blit`, `gui_blend` |
| `damage.c` | damage lists: at most 8 rectangles, none touching another (`gui_damage_add`, `gui_damage_clear`, `gui_damage_bounds`) |
| `font.c` | PSF2 fonts with their Unicode tables (`gui_font_load`, `gui_font_parse`, `gui_font_free`), UTF-8 (`gui_utf8_next`, `gui_utf8_put`), and text: `gui_text`, `gui_text_width`, in bold and large |
| `image.c` | QOI images (`gui_qoi_decode`, `gui_image_load`) and `gui_read_file`, a whole file into memory |
| `keymap.c` | the US layout (`gui_keymap_us`) and the keyboard's modifiers and characters (`gui_keyboard_init`, `gui_keyboard_key`, `gui_keymap_char`) |
| `theme.h` | the widgets' colours and sizes, and what `layout.c` and `widget.c` share (private) |
| `layout.c` | measuring and arranging the widget tree: rows, columns, `GUI_GROW` (`gui_ui_layout`) |
| `widget.c` | the widgets (label, button, field, list, scroll bar, menu), focus, hit-testing, input (`gui_ui_pointer`, `gui_ui_button`, `gui_ui_scroll`, `gui_ui_key`) and drawing what changed (`gui_ui_draw`) |
| `protocol.h` | the Wayland interfaces' opcodes and enums the library uses, from `wayland.xml` 1.23, `xdg-shell.xml` 1.38 and wlroots' layer shell and foreign toplevel management (private, but `guitest`'s display server includes it too) |
| `wire.c` | the Wayland wire format, either end of a connection: messages and their arguments, descriptors in `SCM_RIGHTS` (`gui_wire_*`, `gui_msg_*`) |
| `client.c` | the Wayland client: the connection and its globals, windows over `wl_shm` buffers in a memfd, events to handlers, and `gui_run`, a ui in a window |

`kernel/user/guitest.c` tests it, through `crates/xtask/tests/gui.rs`
(images compared with `crates/xtask/tests/gui/`), and
`kernel/user/guidemo.c` shows it.
