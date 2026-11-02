/* httpd [-p PORT] [-n COUNT] [-t SECONDS] [DIR]: a web server for the files
 * under DIR (/ if left out), HTTP/1.0 (RFC 1945), one connection at a time.
 *
 * It listens on PORT (80 by default), prints "httpd: serving DIR on port
 * PORT", and answers each connection's request, then closes it; after
 * COUNT connections it exits (without -n it runs until it is killed). A
 * line for each request goes to the output:
 *
 *   10.0.2.2:49152 GET /README 200 1325
 *
 * the client, the method, the path, the status and the bytes of the body.
 *
 * - GET and HEAD of a file send it, with its Content-Length and a
 *   Content-Type from its name's extension (text/plain without one).
 * - A directory sends its index.html, or a list of its entries, each a link.
 * - Errors: 400 for a request it can't read, 403 for a path with a ".."
 *   in it (nothing above DIR is served), 404 for a name that isn't there,
 *   405 for another method, 414 for a path too long for the filesystem.
 *
 * The request is read up to its empty line, at most 2 KiB, waiting at most
 * SECONDS (30 by default; 0 waits for ever) for it. After the answer, the
 * server ends its side and reads what the client may still send until it
 * closes its own (or the time runs out), so that closing never resets a
 * connection whose client has not read the whole answer yet. */
#include "user.h"

#define REQUEST_MAX 2048
#define CHUNK 1024

static char request[REQUEST_MAX + 1];
static char chunk[CHUNK];
static char path[MAXPATH + 2];
static char header[256];
static char root[MAXPATH + 1];
/* How much of `path` is the root's: the rest is the request's path. */
static int root_len;

static int usage(void) {
    fprintf(STDERR, "usage: httpd [-p PORT] [-n COUNT] [-t SECONDS] [DIR]\n");
    return 1;
}

/* Sends all `n` bytes of `p`. Returns 0, or an error. */
static int send_all(int fd, const char *p, int n) {
    while (n > 0) {
        int w = send(fd, p, (uint)n);
        if (w < 0) {
            return w;
        }
        p += w;
        n -= w;
    }
    return 0;
}

static const char *reason(int status) {
    switch (status) {
    case 200:
        return "OK";
    case 400:
        return "Bad Request";
    case 403:
        return "Forbidden";
    case 404:
        return "Not Found";
    case 405:
        return "Method Not Allowed";
    case 414:
        return "Request-URI Too Long";
    default:
        return "Internal Server Error";
    }
}

/* The status line and headers of an answer with a `length`-byte body. */
static int send_head(int fd, int status, const char *type, int length) {
    int n = snprintf(header, sizeof header,
                     "HTTP/1.0 %d %s\r\nServer: camelot-httpd\r\nContent-Type: %s\r\nContent-Length: %d\r\n"
                     "Connection: close\r\n\r\n",
                     status, reason(status), type, length);
    return send_all(fd, header, n);
}

/* An error answer, its body a line saying what it is. Returns the body's
 * length. */
static int send_error(int fd, int status, int head_only) {
    char body[64];
    int n = snprintf(body, sizeof body, "%d %s\n", status, reason(status));
    send_head(fd, status, "text/plain", n);
    if (!head_only) {
        send_all(fd, body, n);
    }
    return n;
}

/* Whether `s` ends with `suffix`. */
static int ends_with(const char *s, const char *suffix) {
    int a = (int)strlen(s);
    int b = (int)strlen(suffix);
    return a >= b && strcmp(s + a - b, suffix) == 0;
}

static const char *content_type(const char *name) {
    const char *base = strrchr(name, '/');
    base = base != NULL ? base + 1 : name;
    if (strchr(base, '.') == NULL || ends_with(base, ".txt") || ends_with(base, ".md") || ends_with(base, ".c") ||
        ends_with(base, ".h") || ends_with(base, ".s")) {
        return "text/plain";
    }
    if (ends_with(base, ".html") || ends_with(base, ".htm")) {
        return "text/html";
    }
    if (ends_with(base, ".css")) {
        return "text/css";
    }
    if (ends_with(base, ".js")) {
        return "text/javascript";
    }
    if (ends_with(base, ".json")) {
        return "application/json";
    }
    if (ends_with(base, ".png")) {
        return "image/png";
    }
    if (ends_with(base, ".jpg") || ends_with(base, ".jpeg")) {
        return "image/jpeg";
    }
    if (ends_with(base, ".gif")) {
        return "image/gif";
    }
    return "application/octet-stream";
}

/* Reads the request up to its empty line. Returns its length, or -1. */
static int read_request(int fd) {
    int n = 0;
    while (n < REQUEST_MAX) {
        int r = recv(fd, request + n, (uint)(REQUEST_MAX - n));
        if (r <= 0) {
            return -1;
        }
        n += r;
        request[n] = 0;
        if (strstr(request, "\r\n\r\n") != NULL || strstr(request, "\n\n") != NULL) {
            return n;
        }
    }
    return -1;
}

static int hex(char c) {
    if (c >= '0' && c <= '9') {
        return c - '0';
    }
    if (c >= 'a' && c <= 'f') {
        return c - 'a' + 10;
    }
    if (c >= 'A' && c <= 'F') {
        return c - 'A' + 10;
    }
    return -1;
}

/* Turns the request's target (`target`, NUL-terminated) into a path under
 * the root in `path`: the query cut off, %XX decoded; path + root_len is
 * then the request's path. Returns 200, or the status that says why not. */
static int map_path(const char *target) {
    if (target[0] != '/') {
        return 400;
    }
    int n = (int)strlen(root);
    if (n > 0 && root[n - 1] == '/') {
        n--; /* the target brings its own */
    }
    memcpy(path, root, (size_t)n);
    root_len = n;
    for (const char *p = target; *p != 0 && *p != '?' && *p != '#'; p++) {
        char c = *p;
        if (c == '%') {
            int hi = hex(p[1]);
            int lo = hi >= 0 ? hex(p[2]) : -1;
            if (lo < 0) {
                return 400;
            }
            c = (char)(hi * 16 + lo);
            p += 2;
        }
        if (c == 0) {
            return 400;
        }
        if (n > MAXPATH) {
            return 414;
        }
        path[n] = c;
        n++;
    }
    path[n] = 0;
    if (n == 0) {
        path[0] = '/';
        path[1] = 0;
    }
    /* No ".." name: nothing above the root. */
    for (int i = 0; path[i] != 0; i++) {
        if (path[i] == '.' && path[i + 1] == '.' && (i == 0 || path[i - 1] == '/') &&
            (path[i + 2] == '/' || path[i + 2] == 0)) {
            return 403;
        }
    }
    return 200;
}

/* Sends the open file `file` (`size` bytes), unless only the head is
 * asked for. Returns the body's length. */
static int send_file(int fd, int file, const char *name, int size, int head_only) {
    send_head(fd, 200, content_type(name), size);
    if (head_only) {
        return size;
    }
    for (;;) {
        int n = read(file, chunk, CHUNK);
        if (n <= 0 || send_all(fd, chunk, n) < 0) {
            break;
        }
    }
    return size;
}

/* A page built in the heap, as it grows: a listing's size is only known at
 * the end. Every append makes room for what it adds first, so nothing is
 * written past the end and every byte of `data` up to `len` was written.
 * If the heap runs out, `failed` is set and the rest is not added. */
struct page {
    char *data;
    int len;
    int cap;
    int failed;
};

/* Appends the `n` bytes at `s`, doubling the room until they fit. */
static void put(struct page *pg, const char *s, int n) {
    if (pg->failed) {
        return;
    }
    if (pg->len + n > pg->cap) {
        int cap = pg->cap > 0 ? pg->cap : 1024;
        while (pg->len + n > cap) {
            cap *= 2;
        }
        char *bigger = realloc(pg->data, (size_t)cap);
        if (bigger == NULL) {
            pg->failed = 1;
            return;
        }
        pg->data = bigger;
        pg->cap = cap;
    }
    memcpy(pg->data + pg->len, s, (size_t)n);
    pg->len += n;
}

static void put_str(struct page *pg, const char *s) {
    put(pg, s, (int)strlen(s));
}

/* Appends `s` as HTML text: the characters that mean something in HTML
 * (& < > ") as entities. */
static void put_html(struct page *pg, const char *s) {
    for (; *s != 0; s++) {
        if (*s == '&') {
            put_str(pg, "&amp;");
        } else if (*s == '<') {
            put_str(pg, "&lt;");
        } else if (*s == '>') {
            put_str(pg, "&gt;");
        } else if (*s == '"') {
            put_str(pg, "&quot;");
        } else {
            put(pg, s, 1);
        }
    }
}

/* Appends the first `n` bytes of the path `s` as a URL path: letters,
 * digits, '/' and - . _ ~ as they are, every other byte as %XX (which
 * map_path decodes), so the result is also safe in an HTML attribute. */
static void put_url(struct page *pg, const char *s, int n) {
    static const char digits[] = "0123456789ABCDEF";
    for (int i = 0; i < n; i++) {
        unsigned char c = (unsigned char)s[i];
        if ((c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z') || (c >= '0' && c <= '9') || c == '/' ||
            c == '-' || c == '.' || c == '_' || c == '~') {
            put(pg, s + i, 1);
        } else {
            char esc[3] = {'%', digits[c >> 4], digits[c & 15]};
            put(pg, esc, 3);
        }
    }
}

/* Sends a list of the directory `dir` (open), its entries as links. `url`
 * is the directory's path as the client sees it: the request's path,
 * decoded, without the query, starting with '/'. */
static int send_listing(int fd, int dir, const char *url, int head_only) {
    struct page pg = {NULL, 0, 0, 0};
    int ulen = (int)strlen(url);
    if (ulen > 1 && url[ulen - 1] == '/') {
        ulen--; /* the links bring their own */
    }
    put_str(&pg, "<html><body><h1>");
    put_html(&pg, url);
    put_str(&pg, "</h1><ul>\n");
    struct dirent de;
    while (read(dir, &de, sizeof de) == (int)sizeof de) {
        if (de.inum == 0) {
            continue;
        }
        char name[DIRSIZ + 1];
        memcpy(name, de.name, DIRSIZ);
        name[DIRSIZ] = 0;
        if (strcmp(name, ".") == 0) {
            continue;
        }
        int parent = strcmp(name, "..") == 0;
        if (parent && ulen <= 1) {
            continue; /* the root has none */
        }
        put_str(&pg, "<li><a href=\"");
        if (parent) {
            /* The path up to its last '/' (map_path would refuse "/dir/.."). */
            int up = ulen;
            while (up > 1 && url[up - 1] != '/') {
                up--;
            }
            put_url(&pg, url, up);
        } else {
            put_url(&pg, url, ulen);
            if (ulen > 1) {
                put_str(&pg, "/");
            }
            put_url(&pg, name, (int)strlen(name));
        }
        put_str(&pg, "\">");
        put_html(&pg, name);
        put_str(&pg, "</a></li>\n");
    }
    put_str(&pg, "</ul></body></html>\n");
    int len = pg.len;
    if (pg.failed) {
        len = send_error(fd, 500, head_only);
    } else {
        send_head(fd, 200, "text/html", len);
        if (!head_only) {
            send_all(fd, pg.data, len);
        }
    }
    free(pg.data);
    return len;
}

/* Answers the request on connection `fd`: prints its line, returns 0. */
static void serve(int fd, struct sockaddr_in *peer) {
    int status;
    int bytes = 0;
    const char *method = "-";
    const char *target = "-";
    int head_only = 0;
    if (read_request(fd) < 0) {
        status = 400;
        bytes = send_error(fd, status, 0);
    } else {
        /* "METHOD TARGET VERSION" */
        char *sp1 = strchr(request, ' ');
        char *sp2 = sp1 != NULL ? strchr(sp1 + 1, ' ') : NULL;
        char *eol = strchr(request, '\n');
        if (sp1 == NULL || sp2 == NULL || eol == NULL || sp2 > eol) {
            status = 400;
        } else {
            *sp1 = 0;
            *sp2 = 0;
            method = request;
            target = sp1 + 1;
            head_only = strcmp(method, "HEAD") == 0;
            status = strcmp(method, "GET") == 0 || head_only ? map_path(target) : 405;
        }
        if (status == 200) {
            int file = open(path, O_RDONLY);
            struct stat st;
            if (file < 0 || fstat(file, &st) < 0) {
                status = file == -ENAMETOOLONG ? 414 : 404;
            } else if (st.type == T_DIR) {
                /* index.html, if the directory has one. */
                int n = (int)strlen(path);
                int index = -1;
                if (n + 11 <= MAXPATH) {
                    strcpy(path + n, path[n - 1] == '/' ? "index.html" : "/index.html");
                    index = open(path, O_RDONLY);
                }
                struct stat ist;
                if (index >= 0 && fstat(index, &ist) == 0 && ist.type == T_FILE) {
                    bytes = send_file(fd, index, path, (int)ist.size, head_only);
                } else {
                    path[n] = 0; /* the directory again */
                    bytes = send_listing(fd, file, path + root_len, head_only);
                }
                if (index >= 0) {
                    close(index);
                }
            } else if (st.type == T_FILE) {
                bytes = send_file(fd, file, path, (int)st.size, head_only);
            } else {
                status = 404; /* a device */
            }
            if (file >= 0) {
                close(file);
            }
        }
        if (status != 200) {
            bytes = send_error(fd, status, head_only);
        }
    }
    printf("%s:%d %s %s %d %d\n", inet_ntoa(peer->sin_addr), ntohs(peer->sin_port), method, target, status, bytes);
    /* The end of the answer; then whatever the client still sends, until
     * it closes: closing with data unread would reset the connection. */
    shutdown(fd, SHUT_WR);
    while (recv(fd, chunk, CHUNK) > 0) {
    }
}

int main(int argc, char **argv) {
    int port = 80;
    long count = -1;
    long seconds = 30;
    int i = 1;
    for (; i < argc && argv[i][0] == '-'; i += 2) {
        if (i + 1 == argc) {
            return usage();
        }
        char *end;
        long v = strtol(argv[i + 1], &end, 10);
        if (end == argv[i + 1] || *end != 0 || v < 0) {
            return usage();
        }
        if (strcmp(argv[i], "-p") == 0 && v >= 1 && v <= 65535) {
            port = (int)v;
        } else if (strcmp(argv[i], "-n") == 0) {
            count = v;
        } else if (strcmp(argv[i], "-t") == 0) {
            seconds = v;
        } else {
            return usage();
        }
    }
    if (argc - i > 1) {
        return usage();
    }
    const char *dir = i < argc ? argv[i] : "/";
    if (strlen(dir) > MAXPATH) {
        fprintf(STDERR, "httpd: %s: %s\n", dir, strerror(ENAMETOOLONG));
        return 1;
    }
    strcpy(root, dir);
    struct stat st;
    int err = stat(root, &st);
    if (err == 0 && st.type != T_DIR) {
        err = -ENOTDIR;
    }
    if (err < 0) {
        fprintf(STDERR, "httpd: %s: %s\n", root, strerror(-err));
        return 1;
    }
    int sock = socket(AF_INET, SOCK_STREAM, 0);
    if (sock < 0) {
        fprintf(STDERR, "httpd: socket: %s\n", sock == -ENODEV ? "no network card" : strerror(-sock));
        return 1;
    }
    struct sockaddr_in here;
    sockaddr_set(&here, INADDR_ANY, (uint)port);
    err = bind(sock, &here);
    if (err == 0) {
        err = listen(sock, 4);
    }
    if (err < 0) {
        fprintf(STDERR, "httpd: port %d: %s\n", port, strerror(-err));
        return 1;
    }
    printf("httpd: serving %s on port %d\n", root, port);
    struct timeval wait;
    wait.tv_sec = (int)seconds;
    wait.tv_usec = 0;
    for (long served = 0; count < 0 || served < count; served++) {
        struct sockaddr_in peer;
        int fd = accept(sock, &peer);
        if (fd < 0) {
            fprintf(STDERR, "httpd: accept: %s\n", strerror(-fd));
            return 1;
        }
        setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &wait, sizeof wait);
        serve(fd, &peer);
        close(fd);
    }
    close(sock);
    return 0;
}
