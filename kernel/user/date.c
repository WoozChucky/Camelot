/* date: prints the date and time in UTC, as `date -u` does
 * ("Thu Jan  1 00:00:00 UTC 2026"), from the real-time clock the kernel
 * read at boot (CLOCK_REALTIME). With -s, the seconds since 1970 instead. */
#include "user.h"

static const char *const days[] = {"Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"};
static const char *const months[] = {"Jan", "Feb", "Mar", "Apr", "May", "Jun",
                                     "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"};

int main(int argc, char **argv) {
    time_t now = time(NULL);
    if (now < 0) {
        fprintf(2, "date: no clock\n");
        return 1;
    }
    if (argc == 2 && strcmp(argv[1], "-s") == 0) {
        printf("%ld\n", now);
        return 0;
    }
    if (argc != 1) {
        fprintf(2, "usage: date [-s]\n");
        return 2;
    }
    struct tm tm;
    gmtime_r(&now, &tm);
    printf("%s %s %2d %02d:%02d:%02d UTC %d\n", days[tm.tm_wday], months[tm.tm_mon], tm.tm_mday, tm.tm_hour,
           tm.tm_min, tm.tm_sec, tm.tm_year + 1900);
    return 0;
}
