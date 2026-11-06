/* The date (user.h): time() over clock_gettime, and gmtime_r, which
 * breaks a time into its UTC fields with Howard Hinnant's
 * civil_from_days, as kernel/rtc.c does for the boot's line. */
#include "../user.h"

time_t time(time_t *t) {
    struct timespec ts;
    time_t now = clock_gettime(CLOCK_REALTIME, &ts) == 0 ? ts.tv_sec : -1;
    if (t != NULL) {
        *t = now;
    }
    return now;
}

struct tm *gmtime_r(const time_t *t, struct tm *tm) {
    if (*t < 0) {
        return NULL;
    }
    unsigned long secs = (unsigned long)*t;
    unsigned long days = secs / 86400;
    unsigned long rest = secs % 86400;
    tm->tm_hour = (int)(rest / 3600);
    tm->tm_min = (int)(rest % 3600 / 60);
    tm->tm_sec = (int)(rest % 60);
    /* 1970-01-01 was a Thursday. */
    tm->tm_wday = (int)((days + 4) % 7);
    /* Counted from 0000-03-01, so that a year ends with its leap day. */
    unsigned long z = days + 719468;
    unsigned long era = z / 146097;
    unsigned long doe = z - era * 146097;
    unsigned long yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    unsigned long doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    unsigned long mp = (5 * doy + 2) / 153;
    unsigned long month = mp < 10 ? mp + 3 : mp - 9;
    unsigned long year = yoe + era * 400 + (month <= 2 ? 1 : 0);
    tm->tm_mday = (int)(doy - (153 * mp + 2) / 5 + 1);
    tm->tm_mon = (int)month - 1;
    tm->tm_year = (int)year - 1900;
    /* The day of the year, from 1 January: March 1st is day 59, or 60 in
     * a leap year. */
    int leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    tm->tm_yday = (int)(doy >= 306 ? doy - 306 : doy + 59 + (unsigned long)leap);
    tm->tm_isdst = 0;
    return tm;
}
