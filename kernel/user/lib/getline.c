/* Line input: getline from the console, and readers, which split anything
 * a descriptor gives (a file, a pipe) into lines. */
#include "../user.h"

int getline(char *buf, int size) {
    /* The console returns one line per read, so one read is enough, and
     * nothing after the line is taken from whoever reads next. */
    int n = read(STDIN, buf, (uint)(size - 1));
    if (n <= 0) {
        return -1;
    }
    if (buf[n - 1] == '\n') {
        n--;
    }
    buf[n] = 0;
    return n;
}

void reader_init(struct reader *r, int fd) {
    r->fd = fd;
    r->pos = 0;
    r->len = 0;
}

int reader_getline(struct reader *r, char *line, int size) {
    int n = 0;
    while (n < size - 1) {
        if (r->pos == r->len) {
            /* The buffer is used up: the next block. */
            int got = read(r->fd, r->buf, READER_SIZE);
            if (got < 0) {
                return got;
            }
            if (got == 0) {
                break; /* the end of the input: what we have is the last line */
            }
            r->pos = 0;
            r->len = got;
        }
        char c = r->buf[r->pos];
        r->pos++;
        line[n] = c;
        n++;
        if (c == '\n') {
            break;
        }
    }
    line[n] = 0;
    return n;
}
