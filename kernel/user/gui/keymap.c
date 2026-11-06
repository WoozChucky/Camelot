/* The keyboard (gui.h, "The keyboard"): Linux's key codes to characters.
 *
 * The display server sends key codes with wl_keyboard's NO_KEYMAP format
 * (#184), so the client maps them itself, through a layout's two tables:
 * a key's character, and with Shift. US first, as xkeyboard-config's "us"
 * layout has them, for the keys Camelot's keyboard has (user.h's KEY_
 * codes: no keypad). Enter is
 * '\n', Tab '\t', Backspace '\b' and Escape 0x1b, for a program that wants
 * them as characters; the widgets go by the key codes for those. */
#include "../gui.h"

const struct gui_keymap gui_keymap_us = {
    .name = "us",
    .plain =
        {
            [KEY_ESC] = 0x1b,     [KEY_1] = '1',          [KEY_2] = '2',         [KEY_3] = '3',
            [KEY_4] = '4',        [KEY_5] = '5',          [KEY_6] = '6',         [KEY_7] = '7',
            [KEY_8] = '8',        [KEY_9] = '9',          [KEY_0] = '0',         [KEY_MINUS] = '-',
            [KEY_EQUAL] = '=',    [KEY_BACKSPACE] = '\b', [KEY_TAB] = '\t',      [KEY_Q] = 'q',
            [KEY_W] = 'w',        [KEY_E] = 'e',          [KEY_R] = 'r',         [KEY_T] = 't',
            [KEY_Y] = 'y',        [KEY_U] = 'u',          [KEY_I] = 'i',         [KEY_O] = 'o',
            [KEY_P] = 'p',        [KEY_LEFTBRACE] = '[',  [KEY_RIGHTBRACE] = ']', [KEY_ENTER] = '\n',
            [KEY_A] = 'a',        [KEY_S] = 's',          [KEY_D] = 'd',         [KEY_F] = 'f',
            [KEY_G] = 'g',        [KEY_H] = 'h',          [KEY_J] = 'j',         [KEY_K] = 'k',
            [KEY_L] = 'l',        [KEY_SEMICOLON] = ';',  [KEY_APOSTROPHE] = '\'', [KEY_GRAVE] = '`',
            [KEY_BACKSLASH] = '\\', [KEY_Z] = 'z',        [KEY_X] = 'x',         [KEY_C] = 'c',
            [KEY_V] = 'v',        [KEY_B] = 'b',          [KEY_N] = 'n',         [KEY_M] = 'm',
            [KEY_COMMA] = ',',    [KEY_DOT] = '.',        [KEY_SLASH] = '/',     [KEY_SPACE] = ' ',    },
    .shift =
        {
            [KEY_ESC] = 0x1b,     [KEY_1] = '!',          [KEY_2] = '@',         [KEY_3] = '#',
            [KEY_4] = '$',        [KEY_5] = '%',          [KEY_6] = '^',         [KEY_7] = '&',
            [KEY_8] = '*',        [KEY_9] = '(',          [KEY_0] = ')',         [KEY_MINUS] = '_',
            [KEY_EQUAL] = '+',    [KEY_BACKSPACE] = '\b', [KEY_TAB] = '\t',      [KEY_Q] = 'Q',
            [KEY_W] = 'W',        [KEY_E] = 'E',          [KEY_R] = 'R',         [KEY_T] = 'T',
            [KEY_Y] = 'Y',        [KEY_U] = 'U',          [KEY_I] = 'I',         [KEY_O] = 'O',
            [KEY_P] = 'P',        [KEY_LEFTBRACE] = '{',  [KEY_RIGHTBRACE] = '}', [KEY_ENTER] = '\n',
            [KEY_A] = 'A',        [KEY_S] = 'S',          [KEY_D] = 'D',         [KEY_F] = 'F',
            [KEY_G] = 'G',        [KEY_H] = 'H',          [KEY_J] = 'J',         [KEY_K] = 'K',
            [KEY_L] = 'L',        [KEY_SEMICOLON] = ':',  [KEY_APOSTROPHE] = '"', [KEY_GRAVE] = '~',
            [KEY_BACKSLASH] = '|', [KEY_Z] = 'Z',         [KEY_X] = 'X',         [KEY_C] = 'C',
            [KEY_V] = 'V',        [KEY_B] = 'B',          [KEY_N] = 'N',         [KEY_M] = 'M',
            [KEY_COMMA] = '<',    [KEY_DOT] = '>',        [KEY_SLASH] = '?',     [KEY_SPACE] = ' ',    },
};

void gui_keyboard_init(struct gui_keyboard *k, const struct gui_keymap *keymap) {
    k->keymap = keymap;
    k->mods = 0;
    k->left = 0;
    k->right = 0;
}

/* The modifier a key is, and whether it is the right-hand one; 0 if none. */
static uint modifier(uint code, int *right) {
    *right = 0;
    switch (code) {
    case KEY_RIGHTSHIFT:
        *right = 1;
        return GUI_MOD_SHIFT;
    case KEY_LEFTSHIFT:
        return GUI_MOD_SHIFT;
    case KEY_RIGHTCTRL:
        *right = 1;
        return GUI_MOD_CTRL;
    case KEY_LEFTCTRL:
        return GUI_MOD_CTRL;
    case KEY_RIGHTALT:
        *right = 1;
        return GUI_MOD_ALT;
    case KEY_LEFTALT:
        return GUI_MOD_ALT;
    case KEY_RIGHTMETA:
        *right = 1;
        return GUI_MOD_LOGO;
    case KEY_LEFTMETA:
        return GUI_MOD_LOGO;
    }
    return 0;
}

uint gui_keymap_char(const struct gui_keymap *m, uint code, uint mods) {
    if (code >= GUI_KEYMAP_KEYS) {
        return 0;
    }
    uint plain = m->plain[code];
    uint shifted = m->shift[code];
    int shift = (mods & GUI_MOD_SHIFT) != 0;
    /* Caps Lock: a letter's case, as Shift; Shift with it, small again. */
    if ((mods & GUI_MOD_CAPS) && plain >= 'a' && plain <= 'z' && shifted == plain - 'a' + 'A') {
        shift = !shift;
    }
    return shift ? shifted : plain;
}

uint gui_keyboard_key(struct gui_keyboard *k, uint code, int pressed) {
    int right;
    uint mod = modifier(code, &right);
    if (mod != 0) {
        uint *side = right ? &k->right : &k->left;
        if (pressed) {
            *side |= mod;
        } else {
            *side &= ~mod;
        }
        k->mods = (k->mods & GUI_MOD_CAPS) | k->left | k->right;
        return 0;
    }
    if (code == KEY_CAPSLOCK) {
        if (pressed == 1) {
            k->mods ^= GUI_MOD_CAPS;
        }
        return 0;
    }
    if (!pressed || (k->mods & (GUI_MOD_CTRL | GUI_MOD_ALT))) {
        return 0;
    }
    return gui_keymap_char(k->keymap, code, k->mods);
}

void gui_keyboard_modifiers(struct gui_keyboard *k, uint held, uint locked) {
    held &= GUI_MOD_SHIFT | GUI_MOD_CTRL | GUI_MOD_ALT | GUI_MOD_LOGO;
    k->left &= held;
    k->right &= held;
    k->left |= held & ~(k->left | k->right);
    k->mods = (locked & GUI_MOD_CAPS) | k->left | k->right;
}
