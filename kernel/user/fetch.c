/* fetch URL: downloads http://HOST[:PORT][/PATH] to the output with an
 * HTTP/1.0 GET (RFC 1945), so `fetch http://10.0.2.2:8000/a.txt > a.txt`
 * saves a file. HOST is an address or a name (DNS). The status line and
 * the headers are read and checked; only the body is written out.
 *
 * Exits 0 for a 2xx answer whose body came whole (all of its
 * Content-Length, when the server sent one), else 1, having said why: a URL
 * it can't read, a host it can't find or reach, a status other than 2xx
 * ("fetch: URL: 404 Not Found"; the body is not written), an answer that
 * is not HTTP, or one cut short. */
#include "user.h"

#define BUF 1024
/* The longest header line kept: longer ones are cut (their end skipped). */
#define LINE 256

static char buf[BUF];
static char line[LINE];
static char request[512];

/* The answer, read through a buffer: the header lines one at a time, then
 * the body. */
static int sock;
static int pos;
static int len;

/* The next byte of the answer, or -1 at its end, or an error (< -1). */
static int next_byte(void) {
    if (pos == len) {
        int n = recv(sock, buf, BUF);
        if (n <= 0) {
            return n == 0 ? -1 : n;
        }
        pos = 0;
        len = n;
    }
    return (uchar)buf[pos++];
}

/* Reads a header line into `line`, without its CR LF. Returns its length,
 * -1 at the end of the answer, or an error. */
static int read_line(void) {
    int n = 0;
    for (;;) {
        int c = next_byte();
        if (c < 0) {
            return n > 0 ? n : c;
        }
        if (c == '\n') {
            break;
        }
        if (n < LINE - 1) {
            line[n] = (char)c;
            n++;
        }
    }
    if (n > 0 && line[n - 1] == '\r') {
        n--;
    }
    line[n] = 0;
    return n;
}

/* Whether the header in `line` is `name` (any case), with its value at
 * *value. */
static int header(const char *name, char **value) {
    int i = 0;
    for (; name[i] != 0; i++) {
        char c = line[i];
        if (c >= 'A' && c <= 'Z') {
            c = (char)(c - 'A' + 'a');
        }
        if (c != name[i]) {
            return 0;
        }
    }
    if (line[i] != ':') {
        return 0;
    }
    i++;
    while (line[i] == ' ' || line[i] == '\t') {
        i++;
    }
    *value = line + i;
    return 1;
}

static int fail(const char *url, const char *why) {
    fprintf(STDERR, "fetch: %s: %s\n", url, why);
    return 1;
}

int main(int argc, char **argv) {
    if (argc != 2) {
        fprintf(STDERR, "usage: fetch http://HOST[:PORT][/PATH]\n");
        return 1;
    }
    const char *url = argv[1];
    if (strncmp(url, "http://", 7) != 0) {
        return fail(url, "only http:// URLs");
    }
    /* HOST[:PORT] up to the path. */
    char host[128];
    const char *p = url + 7;
    int h = 0;
    while (*p != 0 && *p != '/' && *p != ':') {
        if (h == (int)sizeof host - 1) {
            return fail(url, "host name too long");
        }
        host[h] = *p;
        h++;
        p++;
    }
    host[h] = 0;
    int port = 80;
    if (*p == ':') {
        char *end;
        long n = strtol(p + 1, &end, 10);
        if (end == p + 1 || n < 1 || n > 65535 || (*end != 0 && *end != '/')) {
            return fail(url, "bad port");
        }
        port = (int)n;
        p = end;
    }
    const char *path = *p == '/' ? p : "/";
    if (h == 0) {
        return fail(url, "no host");
    }
    int n = snprintf(request, sizeof request,
                     "GET %s HTTP/1.0\r\nHost: %s\r\nUser-Agent: camelot-fetch\r\nConnection: close\r\n\r\n", path,
                     host);
    if (n >= (int)sizeof request) {
        return fail(url, "URL too long");
    }

    struct in_addr addr;
    int err = resolve(host, &addr);
    if (err < 0) {
        fprintf(STDERR, "fetch: %s: %s\n", host, err == -ENODEV ? "no network card" : resolve_error(-err));
        return 1;
    }
    sock = socket(AF_INET, SOCK_STREAM, 0);
    if (sock < 0) {
        return fail(url, sock == -ENODEV ? "no network card" : strerror(-sock));
    }
    struct sockaddr_in to;
    sockaddr_set(&to, ntohl(addr.s_addr), (uint)port);
    err = connect(sock, &to);
    if (err == 0) {
        int w = send(sock, request, (uint)n);
        err = w < 0 ? w : 0;
    }
    if (err < 0) {
        close(sock);
        return fail(url, strerror(-err));
    }

    /* The status line: "HTTP/1.x CODE REASON". */
    int r = read_line();
    if (r < 0 || strncmp(line, "HTTP/", 5) != 0 || strchr(line, ' ') == NULL) {
        close(sock);
        return fail(url, r < -1 ? strerror(-r) : "not an HTTP answer");
    }
    char *status = strchr(line, ' ') + 1;
    int code = atoi(status);
    int ok = code >= 200 && code <= 299;
    char reason[64];
    snprintf(reason, sizeof reason, "%s", status);
    /* The headers, to the empty line: only Content-Length matters. */
    long length = -1;
    for (;;) {
        r = read_line();
        if (r < 0) {
            close(sock);
            return fail(url, r < -1 ? strerror(-r) : "the answer ends in its headers");
        }
        if (r == 0) {
            break;
        }
        char *value;
        if (header("content-length", &value)) {
            length = strtol(value, NULL, 10);
        }
    }
    if (!ok) {
        close(sock);
        return fail(url, reason);
    }
    /* The body: what is left in the buffer, then the rest of the stream. */
    long got = 0;
    for (;;) {
        if (pos < len) {
            int m = len - pos;
            if (write(STDOUT, buf + pos, (uint)m) != m) {
                close(sock);
                return fail(url, "cannot write the output");
            }
            got += m;
            pos = len;
        }
        int m = recv(sock, buf, BUF);
        if (m == 0) {
            break;
        }
        if (m < 0) {
            close(sock);
            return fail(url, strerror(-m));
        }
        pos = 0;
        len = m;
    }
    close(sock);
    if (length >= 0 && got != length) {
        return fail(url, "the answer was cut short");
    }
    return 0;
}
