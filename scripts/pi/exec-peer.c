// TEST ONLY: a stand-in peer for Deck's status socket that can replace its own
// image. Never packaged, never loaded by Pi, and not an observer: it exists
// to prove that a program which executes another image inside the same pid
// is not the observer any more (`agent_pi::transport`, "One execution
// instance"). Used by `app/src-tauri/src/agent_pi/transport/tests.rs` and by
// `scripts/pi_native.py`, both of which build it inside their own temporary
// directory.
//
//   exec-peer <socket path | @fd:N> <log file> <step>...
//
// A step is a line to send, whose one reply line is appended to the log, or:
//   @fork          start a child that executes another program, and wait for it
//   @mark:<file>   create that file
//   @wait:<file>   wait until that file exists
//   @exec:<file>   go on as that program in this pid, with the connection
//                  (`@fd:N`), the log and the steps that are left
//   @sleep         stay for thirty seconds
// It ends after its last step, and after sixty seconds whatever it does.
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <sys/wait.h>
#include <unistd.h>

static void reply(int fd, const char *log) {
    char line[2048];
    int n = 0;
    char c;
    while (n < 2047 && read(fd, &c, 1) == 1) {
        line[n++] = c;
        if (c == '\n') break;
    }
    line[n] = 0;
    FILE *f = fopen(log, "a");
    if (f) {
        fputs(line, f);
        fclose(f);
    }
}

int main(int argc, char **argv) {
    alarm(60);
    if (argc < 3) return 2;
    int fd;
    if (!strncmp(argv[1], "@fd:", 4)) {
        fd = atoi(argv[1] + 4);
    } else {
        fd = socket(AF_UNIX, SOCK_STREAM, 0);
        struct sockaddr_un a;
        memset(&a, 0, sizeof a);
        a.sun_family = AF_UNIX;
        strncpy(a.sun_path, argv[1], sizeof a.sun_path - 1);
        if (connect(fd, (struct sockaddr *)&a, sizeof a)) return 3;
    }
    const char *log = argv[2];
    for (int i = 3; i < argc; i++) {
        const char *s = argv[i];
        if (!strcmp(s, "@fork")) {
            pid_t p = fork();
            if (!p) {
                execl("/usr/bin/true", "true", (char *)0);
                _exit(1);
            }
            waitpid(p, 0, 0);
        } else if (!strncmp(s, "@mark:", 6)) {
            int m = open(s + 6, O_CREAT | O_WRONLY, 0600);
            if (m >= 0) close(m);
        } else if (!strncmp(s, "@wait:", 6)) {
            while (access(s + 6, F_OK)) usleep(20000);
        } else if (!strcmp(s, "@sleep")) {
            sleep(30);
        } else if (!strncmp(s, "@exec:", 6)) {
            char num[32];
            snprintf(num, sizeof num, "@fd:%d", fd);
            fcntl(fd, F_SETFD, 0);
            char *next[64];
            int n = 0;
            next[n++] = (char *)(s + 6);
            next[n++] = num;
            next[n++] = (char *)log;
            for (int j = i + 1; j < argc && n < 63; j++) next[n++] = argv[j];
            next[n] = 0;
            execv(next[0], next);
            return 4;
        } else {
            if (write(fd, s, strlen(s)) < 0 || write(fd, "\n", 1) < 0) return 5;
            reply(fd, log);
        }
    }
    return 0;
}
