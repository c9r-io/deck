/* Follow-up 02, experiment D: does accept() hand connections over in the order their connect() calls completed?
 *
 *   queue <socket> <count> <label> [jitter-us]
 *
 * <count> times: read the clock, make a socket, connect it, read the clock again, write
 * "C <label> <i> <before> <after>\n" and close. A connect that is refused (the listener's queue is full) is
 * tried again and counted. The clock is CLOCK_UPTIME_RAW in nanoseconds: one clock for every process of the
 * machine. Ends by itself (also after 120 s). Nothing here is Deck's or Pi's. */
#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <time.h>
#include <unistd.h>
int main(int argc, char **argv) {
    alarm(120);
    if (argc < 4) return 2;
    int count = atoi(argv[2]), jitter = argc > 4 ? atoi(argv[4]) : 0;
    struct sockaddr_un a = {0};
    a.sun_family = AF_UNIX;
    strlcpy(a.sun_path, argv[1], sizeof(a.sun_path));
    srandom((unsigned)getpid());
    long refused = 0;
    for (int i = 0; i < count; i++) {
        for (;;) {
            unsigned long long before = clock_gettime_nsec_np(CLOCK_UPTIME_RAW);
            int fd = socket(AF_UNIX, SOCK_STREAM, 0);
            if (fd < 0) return 3;
            if (connect(fd, (void *)&a, sizeof(a))) {
                close(fd);
                if (errno != ECONNREFUSED) return 4;
                refused++;
                usleep(200);
                continue;
            }
            unsigned long long after = clock_gettime_nsec_np(CLOCK_UPTIME_RAW);
            char line[128];
            int length = snprintf(line, sizeof(line), "C %s %d %llu %llu\n", argv[3], i, before, after);
            if (write(fd, line, length) != length) return 5;
            close(fd);
            break;
        }
        if (jitter) usleep(random() % jitter);
    }
    fprintf(stderr, "refused %ld\n", refused);
    return 0;
}
