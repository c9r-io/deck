// Experiment peer: connects to a Unix socket, then obeys one-letter commands
// read from that socket. Exits on EOF. alarm(40) bounds its life.
#include <errno.h>
#include <spawn.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <unistd.h>
extern char **environ;
static void say(int fd, const char *what, const char *image) {
    char line[256];
    int n = snprintf(line, sizeof line, "%s %d %s\n", what, (int)getpid(), image);
    (void)!write(fd, line, (size_t)n);
}
static const char *base(const char *p) { const char *s = strrchr(p, '/'); return s ? s + 1 : p; }
int main(int argc, char **argv) {
    alarm(40);
    // argv: <self> <other> <sock> connect|detach   |  <self> <other> - resume <fd>
    if (argc < 5) return 2;
    const char *self = argv[0], *other = argv[1];
    int fd = -1;
    if (!strcmp(argv[4], "connect") || !strcmp(argv[4], "detach")) {
        if (!strcmp(argv[4], "detach")) {
            pid_t p = fork();
            if (p > 0) _exit(0);
            setsid();
        }
        fd = socket(AF_UNIX, SOCK_STREAM, 0);
        struct sockaddr_un a; memset(&a, 0, sizeof a); a.sun_family = AF_UNIX;
        strncpy(a.sun_path, argv[2], sizeof a.sun_path - 1);
        if (connect(fd, (struct sockaddr *)&a, sizeof a) != 0) return 3;
        say(fd, "hello", base(self));
    } else {
        fd = atoi(argv[5]);
        say(fd, "resumed", base(self));
        if (!strcmp(argv[4], "resume-once")) return 0;
    }
    char fdtext[16]; snprintf(fdtext, sizeof fdtext, "%d", fd);
    for (;;) {
        char c;
        ssize_t n = read(fd, &c, 1);
        if (n <= 0) return 0;
        if (c == 'x') {            // exec another image, same pid, fd kept
            char *args[] = {(char *)other, (char *)self, "-", "x", "resume", fdtext, NULL};
            execv(other, args); return 4;
        } else if (c == 's') {     // exec the SAME image again
            char *args[] = {(char *)self, (char *)other, "-", "x", "resume", fdtext, NULL};
            execv(self, args); return 4;
        } else if (c == 'p') {     // posix_spawn with SETEXEC: an exec by another call
            posix_spawnattr_t at; posix_spawnattr_init(&at);
            posix_spawnattr_setflags(&at, POSIX_SPAWN_SETEXEC);
            char *args[] = {(char *)other, (char *)self, "-", "x", "resume", fdtext, NULL};
            posix_spawn(NULL, other, NULL, &at, args, environ); return 4;
        } else if (c == 'f') {     // fork: the child writes on the inherited fd, then exits
            pid_t p = fork();
            if (p == 0) { say(fd, "child", base(self)); _exit(0); }
        } else if (c == 'e') {     // fork: the child execs another image and writes
            pid_t p = fork();
            if (p == 0) {
                char *args[] = {(char *)other, (char *)self, "-", "x", "resume-once", fdtext, NULL};
                execv(other, args); _exit(4);
            }
        } else if (c == 'w') {
            say(fd, "again", base(self));
        } else if (c == 'q') {
            return 0;
        }
    }
}
