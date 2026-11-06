/* The widgets' look, shared by gui/widget.c and gui/layout.c (not
 * installed: what a program sees is gui.h). Sizes are in pixels; a text
 * line is the font's height. */
#ifndef CAMELOT_GUI_THEME_H
#define CAMELOT_GUI_THEME_H

#include "../gui.h"

#define THEME_WINDOW GUI_RGB(0xec, 0xec, 0xe8)
#define THEME_TEXT GUI_RGB(0x20, 0x20, 0x20)
#define THEME_BORDER GUI_RGB(0x80, 0x80, 0x88)
#define THEME_FOCUS GUI_RGB(0x30, 0x70, 0xd0)
#define THEME_BUTTON GUI_RGB(0xd8, 0xd8, 0xd4)
#define THEME_BUTTON_HOVER GUI_RGB(0xe6, 0xe6, 0xe2)
#define THEME_BUTTON_PRESSED GUI_RGB(0xb8, 0xb8, 0xb4)
#define THEME_FIELD GUI_RGB(0xff, 0xff, 0xff)
#define THEME_SELECTED GUI_RGB(0x30, 0x70, 0xd0)
#define THEME_SELECTED_TEXT GUI_RGB(0xff, 0xff, 0xff)
#define THEME_TRACK GUI_RGB(0xd0, 0xd0, 0xcc)
#define THEME_THUMB GUI_RGB(0x98, 0x98, 0x9c)
#define THEME_THUMB_HELD GUI_RGB(0x70, 0x70, 0x74)

/* The room around a label's text, inside a button's, a field's and a
 * list's edges, a menu title's. */
#define THEME_LABEL_PAD 2
#define THEME_BUTTON_PAD_X 8
#define THEME_BUTTON_PAD_Y 4
#define THEME_EDGE_PAD 3
#define THEME_MENU_PAD_X 8
#define THEME_MENU_PAD_Y 3
#define THEME_RADIUS 3
/* A scroll bar's width, and its thumb's shortest length. */
#define THEME_SCROLLBAR 12
#define THEME_THUMB_MIN 8
/* A list's or a menu's line: the font's height and this. */
#define THEME_LINE_GAP 2
/* Lines a wheel notch scrolls. */
#define THEME_WHEEL_LINES 3

/* A line of a list or of a menu's items. */
int theme_line(const struct gui_ui *ui);
/* The lines a list shows in its rectangle (one at least). */
int widget_list_lines(const struct gui_widget *w);
/* Keeps a list's first line in range and its scroll bar in step. */
void widget_list_sync(struct gui_widget *w);
/* Scrolls a field, once it has its rectangle, to show its cursor and as
 * much of its text before it as fits. */
void widget_field_sync(struct gui_widget *w);

#endif
