/* IPv4 addresses (user.h, "Addresses and names"): byte order, and
 * addresses as text. A socket call takes its address and port in network
 * byte order, big-endian, as they are on the wire; Excalibur is
 * little-endian, so htonl(0x0a00020f), 10.0.2.15, is 0x0f02000a. */
#include "../user.h"

ushort htons(ushort x) {
    return (ushort)((x << 8) | (x >> 8));
}

ushort ntohs(ushort x) {
    return htons(x);
}

uint htonl(uint x) {
    return (x << 24) | ((x << 8) & 0xff0000) | ((x >> 8) & 0xff00) | (x >> 24);
}

uint ntohl(uint x) {
    return htonl(x);
}

int inet_aton(const char *s, struct in_addr *a) {
    uint v = 0;
    for (int part = 0; part < 4; part++) {
        uint n = 0;
        int digits = 0;
        while (*s >= '0' && *s <= '9') {
            if (digits == 3) {
                return 0;
            }
            n = n * 10 + (uint)(*s - '0');
            s++;
            digits++;
        }
        if (digits == 0 || n > 255) {
            return 0;
        }
        v = (v << 8) | n;
        if (part < 3) {
            if (*s != '.') {
                return 0;
            }
            s++;
        }
    }
    if (*s != 0) {
        return 0;
    }
    a->s_addr = htonl(v);
    return 1;
}

uint inet_addr(const char *s) {
    struct in_addr a;
    return inet_aton(s, &a) ? a.s_addr : INADDR_NONE;
}

char *inet_ntoa(struct in_addr a) {
    static char buf[16];
    uint v = ntohl(a.s_addr);
    snprintf(buf, sizeof buf, "%u.%u.%u.%u", v >> 24, (v >> 16) & 255, (v >> 8) & 255, v & 255);
    return buf;
}

void sockaddr_set(struct sockaddr_in *sa, uint addr, uint port) {
    memset(sa, 0, sizeof *sa);
    sa->sin_family = AF_INET;
    sa->sin_port = htons((ushort)port);
    sa->sin_addr.s_addr = htonl(addr);
}
