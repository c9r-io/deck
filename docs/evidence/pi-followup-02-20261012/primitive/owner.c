/* Follow-up 02, experiment B: the client half. A parent and, after `fork`, its child; each does what it is told
 * with the sockets it holds. Commands come on standard input as "P <op> <name>" or "C <op> <name>"; one line of
 * answer per command. It ends at the end of its input (and after 60 s). Nothing here is Deck's or Pi's. */
#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/event.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/un.h>
#include <sys/wait.h>
#include <unistd.h>

static const char *path;
static const char *names[] = {"shared", "childs", "parents", "early"};
static int socks[4] = {-1, -1, -1, -1};

static int slot(const char *name) {
    for (int i = 0; i < 4; i++) if (!strcmp(names[i], name)) return i;
    return 0;
}

static void act(const char *op, const char *name, char *answer, size_t size) {
    int i = slot(name), r = 0;
    char byte = 'x';
    errno = 0;
    if (!strcmp(op, "socket")) { socks[i] = socket(AF_UNIX, SOCK_STREAM, 0); r = socks[i] < 0 ? -1 : 0; }
    else if (!strcmp(op, "connect")) {
        if (socks[i] < 0) socks[i] = socket(AF_UNIX, SOCK_STREAM, 0);
        struct sockaddr_un a = {0};
        a.sun_family = AF_UNIX;
        strlcpy(a.sun_path, path, sizeof(a.sun_path));
        r = connect(socks[i], (void *)&a, sizeof(a));
    }
    else if (!strcmp(op, "write")) r = write(socks[i], &byte, 1) == 1 ? 0 : -1;
    else if (!strcmp(op, "recv")) r = recv(socks[i], &byte, 1, MSG_DONTWAIT) >= 0 ? 0 : -1;
    else if (!strcmp(op, "poll")) { struct pollfd p = {socks[i], POLLIN, 0}; r = poll(&p, 1, 0) < 0 ? -1 : 0; }
    else if (!strcmp(op, "kq")) {
        int q = kqueue();
        struct kevent change;
        EV_SET(&change, socks[i], EVFILT_READ, EV_ADD, 0, 0, NULL);
        r = kevent(q, &change, 1, NULL, 0, NULL);
    }
    else if (!strcmp(op, "getsockopt")) { int e; socklen_t n = sizeof(e); r = getsockopt(socks[i], SOL_SOCKET, SO_ERROR, &e, &n); }
    else if (!strcmp(op, "fstat")) { struct stat s; r = fstat(socks[i], &s); }
    else if (!strcmp(op, "dupclose")) { int d = dup(socks[i]); r = close(d); }
    else if (!strcmp(op, "close")) { r = close(socks[i]); socks[i] = -1; }
    if (r == 0) snprintf(answer, size, "ok"); else snprintf(answer, size, "errno %d", errno);
}

int main(int argc, char **argv) {
    alarm(60);
    if (argc != 2) return 2;
    path = argv[1];
    char line[128], who[8], op[32], name[32], answer[64];
    FILE *to_child = NULL, *from_child = NULL;
    while (fgets(line, sizeof(line), stdin)) {
        name[0] = 0;
        if (sscanf(line, "%7s %31s %31s", who, op, name) < 2) continue;
        if (!strcmp(who, "P") && !strcmp(op, "exec")) {
            /* the same program again, in this pid: an exec; it goes on reading the same input */
            printf("executing\n"); fflush(stdout);
            execv(argv[0], argv);
            return 9;
        }
        if (!strcmp(who, "P") && !strcmp(op, "fork")) {
            int down[2], up[2];
            pipe(down); pipe(up);
            pid_t child = fork();
            if (child == 0) {
                close(down[1]); close(up[0]);
                FILE *commands = fdopen(down[0], "r"), *answers = fdopen(up[1], "w");
                while (fgets(line, sizeof(line), commands)) {
                    name[0] = 0;
                    sscanf(line, "%31s %31s", op, name);
                    act(op, name, answer, sizeof(answer));
                    fprintf(answers, "%s\n", answer); fflush(answers);
                }
                _exit(0);
            }
            close(down[0]); close(up[1]);
            to_child = fdopen(down[1], "w"); from_child = fdopen(up[0], "r");
            printf("forked %d\n", child); fflush(stdout);
        } else if (!strcmp(who, "P")) {
            act(op, name, answer, sizeof(answer));
            printf("%s\n", answer); fflush(stdout);
        } else {
            fprintf(to_child, "%s %s\n", op, name); fflush(to_child);
            if (!fgets(answer, sizeof(answer), from_child)) return 3;
            fputs(answer, stdout); fflush(stdout);
        }
    }
    if (to_child) { fclose(to_child); wait(NULL); }
    return 0;
}
