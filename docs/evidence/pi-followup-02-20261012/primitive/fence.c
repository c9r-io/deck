/* Follow-up 02, experiment A: does the kernel's fork report for a watched process always exist by the time a byte
 * that only its child could have written has been read?
 *
 *   fence <socket> fork|spawn <count>     the watched process
 *   fence --child <fd>                    what a spawned child runs: one byte on the inherited connection
 *
 * The process connects, says "H <pid>\n", waits for one byte, then <count> times: makes a child whose first
 * act is to write 'c' on the connection it inherited; waits for it; writes 'p'; waits for one byte. It ends by
 * itself (also after 120 s). Nothing here is Deck's or Pi's. */
#include <errno.h>
#include <spawn.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <sys/wait.h>
#include <unistd.h>
extern char **environ;
int main(int argc, char **argv) {
    alarm(120);
    if (argc == 3 && !strcmp(argv[1], "--child")) {
        int fd = atoi(argv[2]);
        return write(fd, "c", 1) == 1 ? 0 : 3;
    }
    if (argc != 4) return 2;
    int fd = socket(AF_UNIX, SOCK_STREAM, 0);
    struct sockaddr_un a = {0};
    a.sun_family = AF_UNIX;
    strlcpy(a.sun_path, argv[1], sizeof(a.sun_path));
    if (connect(fd, (void *)&a, sizeof(a))) return 4;
    char line[64], byte;
    snprintf(line, sizeof(line), "H %d\n", getpid());
    write(fd, line, strlen(line));
    if (read(fd, &byte, 1) != 1) return 5;
    int count = atoi(argv[3]), spawn = !strcmp(argv[2], "spawn");
    char number[16];
    snprintf(number, sizeof(number), "%d", fd);
    for (int i = 0; i < count; i++) {
        pid_t child;
        if (spawn) {
            char *args[] = {argv[0], "--child", number, NULL};
            if (posix_spawn(&child, argv[0], NULL, NULL, args, environ)) return 6;
        } else {
            child = fork();
            if (child == 0) { write(fd, "c", 1); _exit(0); }
            if (child < 0) return 7;
        }
        waitpid(child, NULL, 0);
        write(fd, "p", 1);
        if (read(fd, &byte, 1) != 1) return 8;
    }
    return 0;
}
