/* files: the desktop's file browser, with a text viewer (#185,
 * docs/excalibur-spec.md section 10, "The desktop shell"):
 *
 *   files [DIRECTORY]          (/ by default)
 *
 * A window of the GUI library's widgets: the directory's entries in a
 * list, the directories first (with a '/'), then the files and devices
 * with their sizes, each group by name. Select one with the pointer or the
 * arrows; Open goes into a directory or shows a file's text, its lines in
 * the same list (tabs as spaces to the next multiple of 8, the first
 * MAX_TEXT bytes, at most MAX_LINES lines), and Up goes back: from the
 * text to its directory, from a directory to the one above. The line
 * under the list says where it is. Exits 0 when the window is closed, 1
 * if it can't start. */
#include "gui.h"

#define FONT "/usr/share/fonts/spleen/8x16.psfu"

#define MAX_ENTRIES 256
#define ENTRY_TEXT 40
#define MAX_TEXT 65536
#define MAX_LINES 4096
#define TAB 8

struct entry {
    char name[DIRSIZ + 1];
    int dir;
    off_t size;
    int type;
};

static struct entry entries[MAX_ENTRIES];
static char entry_text[MAX_ENTRIES][ENTRY_TEXT];
static const char *items[MAX_LINES];
static int nentries;

/* The directory shown, and the file whose text is, if any. */
static char cwd[MAXPATH];
static char viewing[MAXPATH];
/* The text, its tabs expanded and its lines ended by NULs. */
static char *text;

static struct gui_widget *list;
static struct gui_widget *where;
static struct gui_widget *status;
static char where_text[MAXPATH + 16];
static char status_text[80];
static int quit;

/* `dir`/`name`, or `name` alone after "/". */
static void join(char *out, const char *dir, const char *name) {
    snprintf(out, MAXPATH, "%s%s%s", dir, strcmp(dir, "/") == 0 ? "" : "/", name);
}

/* The directory above `path` (/ above itself). */
static void parent(char *path) {
    char *slash = strrchr(path, '/');
    if (slash == NULL || slash == path) {
        strcpy(path, "/");
    } else {
        *slash = 0;
    }
}

static int before(const struct entry *a, const struct entry *b) {
    if (a->dir != b->dir) {
        return a->dir;
    }
    return strcmp(a->name, b->name) < 0;
}

static void show(void) {
    gui_list_set(list, items, nentries);
    list->selected = nentries > 0 ? 0 : -1;
    gui_set_text(where, where_text);
    gui_set_text(status, status_text);
}

/* Lists `cwd`: 0, or a negative error number (and nothing changed). */
static int list_dir(void) {
    int fd = open(cwd, O_RDONLY);
    if (fd < 0) {
        return fd;
    }
    nentries = 0;
    struct dirent de;
    while (nentries < MAX_ENTRIES && read(fd, &de, sizeof de) == sizeof de) {
        if (de.inum == 0) {
            continue;
        }
        struct entry e;
        memcpy(e.name, de.name, DIRSIZ);
        e.name[DIRSIZ] = 0;
        if (strcmp(e.name, ".") == 0 || strcmp(e.name, "..") == 0) {
            continue;
        }
        char path[MAXPATH];
        join(path, cwd, e.name);
        struct stat st;
        if (stat(path, &st) < 0) {
            continue;
        }
        e.dir = st.type == T_DIR;
        e.size = st.size;
        e.type = st.type;
        int i = nentries++;
        while (i > 0 && before(&e, &entries[i - 1])) {
            entries[i] = entries[i - 1];
            i--;
        }
        entries[i] = e;
    }
    close(fd);
    for (int i = 0; i < nentries; i++) {
        struct entry *e = &entries[i];
        if (e->dir) {
            snprintf(entry_text[i], ENTRY_TEXT, "%s/", e->name);
        } else if (e->type == T_DEVICE) {
            snprintf(entry_text[i], ENTRY_TEXT, "%-15s  device", e->name);
        } else {
            snprintf(entry_text[i], ENTRY_TEXT, "%-15s %8ld", e->name, (long)e->size);
        }
        items[i] = entry_text[i];
    }
    viewing[0] = 0;
    snprintf(where_text, sizeof(where_text), "%s", cwd);
    snprintf(status_text, sizeof(status_text), "%d entries", nentries);
    show();
    return 0;
}

/* Shows the file at `path` as text: 0, or a negative error number. */
static int view(const char *path) {
    int fd = open(path, O_RDONLY);
    if (fd < 0) {
        return fd;
    }
    char *raw = malloc(MAX_TEXT);
    char *expanded = raw != NULL ? malloc(MAX_TEXT * TAB / 2 + MAX_LINES + 1) : NULL;
    if (expanded == NULL) {
        free(raw);
        close(fd);
        return -ENOMEM;
    }
    long len = 0;
    while (len < MAX_TEXT) {
        long n = read(fd, raw + len, (size_t)(MAX_TEXT - len));
        if (n <= 0) {
            break;
        }
        len += n;
    }
    close(fd);
    /* Lines ended by NULs, tabs as spaces; the room for them is bounded
     * since a tab at most adds TAB - 1 bytes, and the room is cut short
     * rather than overflowed. */
    size_t room = MAX_TEXT * TAB / 2 + MAX_LINES;
    size_t out = 0;
    int nlines = 0;
    int col = 0;
    items[nlines++] = expanded;
    for (long i = 0; i < len && out + TAB < room; i++) {
        char c = raw[i];
        if (c == '\n') {
            expanded[out++] = 0;
            if (nlines == MAX_LINES) {
                break;
            }
            items[nlines++] = expanded + out;
            col = 0;
        } else if (c == '\t') {
            do {
                expanded[out++] = ' ';
                col++;
            } while (col % TAB != 0);
        } else if (c != '\r') {
            expanded[out++] = c;
            col++;
        }
    }
    expanded[out] = 0;
    if (nlines > 1 && expanded[out - 1] == 0 && items[nlines - 1] == expanded + out) {
        nlines--; /* the text ended with a newline */
    }
    free(raw);
    free(text);
    text = expanded;
    nentries = nlines;
    snprintf(viewing, sizeof(viewing), "%s", path);
    snprintf(where_text, sizeof(where_text), "%s", path);
    snprintf(status_text, sizeof(status_text), "%d lines, %ld bytes%s", nlines, len,
             len == MAX_TEXT ? " (the start)" : "");
    show();
    return 0;
}

static void say(const char *what, int err) {
    snprintf(status_text, sizeof(status_text), "%s: %s", what, strerror(-err));
    gui_set_text(status, status_text);
}

static void open_selected(struct gui_widget *w, int value) {
    (void)w;
    (void)value;
    int i = list->selected;
    if (viewing[0] != 0 || i < 0 || i >= nentries) {
        return;
    }
    char path[MAXPATH];
    join(path, cwd, entries[i].name);
    if (entries[i].dir) {
        char old[MAXPATH];
        memcpy(old, cwd, sizeof(old));
        memcpy(cwd, path, sizeof(cwd));
        int r = list_dir();
        if (r < 0) {
            memcpy(cwd, old, sizeof(cwd));
            say(entries[i].name, r);
        }
    } else if (entries[i].type == T_FILE) {
        int r = view(path);
        if (r < 0) {
            say(entries[i].name, r);
        }
    }
}

static void up(struct gui_widget *w, int value) {
    (void)w;
    (void)value;
    if (viewing[0] == 0) {
        parent(cwd);
    }
    int r = list_dir();
    if (r < 0) {
        say(cwd, r);
    }
}

int main(int argc, char **argv) {
    struct gui_font font;
    int r = gui_font_load(&font, FONT);
    if (r < 0) {
        fprintf(2, "files: %s: %s\n", FONT, strerror(-r));
        return 1;
    }
    snprintf(cwd, sizeof(cwd), "%s", argc > 1 ? argv[1] : "/");
    struct gui_ui ui;
    gui_ui_init(&ui, &font);
    struct gui_widget *root = gui_column(&ui, NULL, 4, 4);
    struct gui_widget *bar = root ? gui_row(&ui, root, 4, 0) : NULL;
    struct gui_widget *up_button = bar ? gui_button(&ui, bar, "Up", up) : NULL;
    struct gui_widget *open_button = up_button ? gui_button(&ui, bar, "Open", open_selected) : NULL;
    where = open_button ? gui_label(&ui, bar, "") : NULL;
    list = where ? gui_list(&ui, root, items, 0, 12, NULL) : NULL;
    status = list ? gui_label(&ui, root, "") : NULL;
    if (status == NULL) {
        fprintf(2, "files: out of memory\n");
        return 1;
    }
    gui_grow(where);
    gui_grow(list->parent);
    r = list_dir();
    if (r < 0) {
        fprintf(2, "files: %s: %s\n", cwd, strerror(-r));
        return 1;
    }
    gui_ui_focus(&ui, list);
    struct gui_display *d = gui_connect(NULL, &r);
    if (d == NULL) {
        fprintf(2, "files: can't connect to the display server: %s\n", strerror(-r));
        return 1;
    }
    r = gui_run(d, &ui, "Files", 360, 340, &quit);
    if (r < 0) {
        fprintf(2, "files: %s\n", r == -EPROTO ? "protocol error" : strerror(-r));
    }
    gui_disconnect(d);
    gui_ui_free(&ui);
    gui_font_free(&font);
    return r < 0;
}
