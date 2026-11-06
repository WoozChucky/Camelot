# The console's font

`spleen-8x16.psfu` is Spleen 8x16 by Frederic Cambus, from the release
[Spleen 2.2.0](https://github.com/fcambus/spleen/releases/tag/2.2.0), as the
release ships it for the Linux console: a PSF1 font of 512 glyphs with a
Unicode table. It is under the BSD 2-Clause licence, in `LICENSE.spleen`.

| File | SHA-256 |
|---|---|
| `spleen-8x16.psfu` | `5c894ef5250e9429ea4a22793bfa563cd923f86b9724194802f6acad67dae7b0` |

The framebuffer console (`kernel/fbcon.c`) draws with it: 80 columns and 30
rows at 640x480. `cargo xtask kernel` converts it to PSF2, the same glyphs
and table with PSF2's header, and writes it into `kernel/font.c`, a C array
the kernel embeds (`crates/xtask/src/font.rs`; a test checks the committed
file is current). Its glyphs 0x20 to 0x7E are the ASCII characters of the
same numbers, which the conversion checks; the console draws `?` for any
other byte.
