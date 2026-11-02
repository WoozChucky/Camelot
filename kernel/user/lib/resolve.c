/* The resolver (user.h, "Addresses and names"): a host name's IPv4 address,
 * from the DNS server the network gave (RFC 1035), over UDP.
 *
 * A DNS message is a 12-byte header, then sections of variable length:
 *
 *   0  id (the answer has the query's)   2  flags: QR (an answer), opcode,
 *   4  questions   6  answers               RD (recursion desired), RCODE
 *   8  authority  10  additional
 *
 * A query has one question: the name as labels, each a length byte and
 * that many characters, ending with a 0 byte ("www.example.com" is
 * 3 www 7 example 3 com 0), then its type (A, 1: an IPv4 address) and class
 * (IN, 1). The answer repeats the question, then has resource records:
 * a name, type, class, a time to live (4 bytes), the data's length, and the
 * data, 4 bytes for an A record. A name in an answer may end with a pointer
 * (two bytes, the top two bits set) to an earlier name in the message: that
 * is how a record says "the name in the question" in two bytes. A CNAME
 * record says the name is an alias; a server that answers it puts the A
 * record of the name it stands for after it, which is all this resolver
 * looks for.
 *
 * One UDP socket, connected to the server (so only its datagrams come),
 * with a receive timeout of a second: three tries, each with its own id. */
#include "../user.h"

#define DNS_PORT 53
#define DNS_TRIES 3
#define DNS_HEADER 12
/* The largest message over UDP without EDNS (RFC 1035 4.2.1). */
#define DNS_MAX 512
/* The longest name, as text (RFC 1035 2.3.4), and the longest label. */
#define NAME_MAX 253
#define LABEL_MAX 63

#define TYPE_A 1
#define TYPE_CNAME 5
#define CLASS_IN 1
#define FLAG_QR 0x8000
#define FLAG_RD 0x0100
#define RCODE_NXDOMAIN 3

/* The query and the answer: static, since a user stack is one page. */
static uchar query[DNS_MAX];
static uchar answer[DNS_MAX];

static uint get16(const uchar *p) {
    return ((uint)p[0] << 8) | p[1];
}

static void put16(uchar *p, uint v) {
    p[0] = (uchar)(v >> 8);
    p[1] = (uchar)v;
}

/* Whether `c` may be in a host name: letters, digits, '-' (RFC 1123), and
 * '_', which some names have. */
static int name_char(char c) {
    return (c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z') || (c >= '0' && c <= '9') || c == '-' || c == '_';
}

/* Writes `name` as labels at `p`. Returns the bytes written, or -1 if it
 * is not a host name: empty labels (a trailing dot is allowed), labels over
 * 63 bytes, other characters, too long. */
static int put_name(uchar *p, const char *name) {
    int len = (int)strlen(name);
    if (len > 0 && name[len - 1] == '.') {
        len--;
    }
    if (len == 0 || len > NAME_MAX) {
        return -1;
    }
    int out = 0;
    int i = 0;
    while (i < len) {
        int start = i;
        while (i < len && name[i] != '.') {
            if (!name_char(name[i])) {
                return -1;
            }
            i++;
        }
        int n = i - start;
        if (n == 0 || n > LABEL_MAX) {
            return -1;
        }
        p[out] = (uchar)n;
        memcpy(p + out + 1, name + start, (size_t)n);
        out += 1 + n;
        i++; /* past the dot */
    }
    p[out] = 0;
    return out + 1;
}

/* Steps over the name at `at` in the `len`-byte message `m`: the offset
 * after it, or -1 if it runs off the end. */
static int skip_name(const uchar *m, int len, int at) {
    while (at < len) {
        uint n = m[at];
        if (n == 0) {
            return at + 1;
        }
        if ((n & 0xc0) == 0xc0) {
            return at + 2 <= len ? at + 2 : -1; /* a pointer ends the name */
        }
        if ((n & 0xc0) != 0) {
            return -1;
        }
        at += 1 + (int)n;
    }
    return -1;
}

/* Reads the answer to query `id` (`len` bytes in `answer`): stores the
 * first A record's address. Returns 0, -ENOENT (no such name, or no
 * address), or 1 if the message is not an answer to the query (ignored). */
static int read_answer(int len, uint id, struct in_addr *addr) {
    if (len < DNS_HEADER || get16(answer) != id || (get16(answer + 2) & FLAG_QR) == 0) {
        return 1;
    }
    uint rcode = get16(answer + 2) & 0xf;
    if (rcode == RCODE_NXDOMAIN) {
        return -ENOENT;
    }
    if (rcode != 0) {
        return -ENOENT; /* the server failed: no address to be had */
    }
    uint questions = get16(answer + 4);
    uint answers = get16(answer + 6);
    int at = DNS_HEADER;
    for (uint i = 0; i < questions; i++) {
        at = skip_name(answer, len, at);
        if (at < 0 || at + 4 > len) {
            return 1;
        }
        at += 4;
    }
    for (uint i = 0; i < answers; i++) {
        at = skip_name(answer, len, at);
        if (at < 0 || at + 10 > len) {
            return 1;
        }
        uint type = get16(answer + at);
        uint class = get16(answer + at + 2);
        uint rdlen = get16(answer + at + 8);
        at += 10;
        if (at + (int)rdlen > len) {
            return 1;
        }
        if (type == TYPE_A && class == CLASS_IN && rdlen == 4) {
            memcpy(&addr->s_addr, answer + at, 4); /* already big-endian */
            return 0;
        }
        at += (int)rdlen; /* a CNAME, or anything else */
    }
    return -ENOENT;
}

int resolve(const char *host, struct in_addr *addr) {
    if (inet_aton(host, addr)) {
        return 0;
    }
    int qname = put_name(query + DNS_HEADER, host);
    if (qname < 0) {
        return -EINVAL;
    }
    struct netinfo info;
    int err = netinfo(&info);
    if (err < 0) {
        return err;
    }
    if (info.dns == 0) {
        return -ENETUNREACH;
    }
    int fd = socket(AF_INET, SOCK_DGRAM, 0);
    if (fd < 0) {
        return fd;
    }
    struct sockaddr_in server;
    sockaddr_set(&server, info.dns, DNS_PORT);
    struct timeval wait;
    wait.tv_sec = 1;
    wait.tv_usec = 0;
    err = connect(fd, &server);
    if (err == 0) {
        err = setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &wait, sizeof wait);
    }
    /* The question: the name, type A, class IN. */
    int qlen = DNS_HEADER + qname + 4;
    put16(query + DNS_HEADER + qname, TYPE_A);
    put16(query + DNS_HEADER + qname + 2, CLASS_IN);
    put16(query + 2, FLAG_RD);
    put16(query + 4, 1);
    put16(query + 6, 0);
    put16(query + 8, 0);
    put16(query + 10, 0);
    uint id = ((uint)getpid() << 8) ^ uptime();
    int result = -ETIMEDOUT;
    for (int try = 0; try < DNS_TRIES && err == 0 && result == -ETIMEDOUT; try++) {
        id = (id + 1) & 0xffff;
        put16(query, id);
        int n = send(fd, query, (uint)qlen);
        if (n < 0) {
            err = n;
            break;
        }
        /* Anything that is not the answer (a late one to an earlier try)
         * is skipped; the timeout covers each read. */
        for (;;) {
            n = recv(fd, answer, DNS_MAX);
            if (n == -EAGAIN) {
                break; /* the next try */
            }
            if (n < 0) {
                err = n;
                break;
            }
            int r = read_answer(n, id, addr);
            if (r <= 0) {
                result = r;
                break;
            }
        }
    }
    close(fd);
    return err < 0 ? err : result;
}

const char *resolve_error(int err) {
    return err == ENOENT ? "unknown host" : strerror(err);
}
