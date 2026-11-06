/* cmsg_nxthdr: CMSG_NXTHDR, the control message after `cmsg` in `msg`'s
 * buffer, or NULL past the last one (Linux's __cmsg_nxthdr). */
#include "../user.h"

struct cmsghdr *cmsg_nxthdr(struct msghdr *msg, struct cmsghdr *cmsg) {
    if (cmsg->cmsg_len < sizeof(struct cmsghdr)) {
        return NULL;
    }
    char *next = (char *)cmsg + CMSG_ALIGN(cmsg->cmsg_len);
    char *start = (char *)msg->msg_control;
    /* A whole header must fit in what is left of the buffer. */
    if ((unsigned long)(next + sizeof(struct cmsghdr) - start) > msg->msg_controllen) {
        return NULL;
    }
    return (struct cmsghdr *)next;
}
