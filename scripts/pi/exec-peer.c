// TEST ONLY: a stand-in peer for Deck's status socket. Never packaged, never
// loaded by Pi, and not an observer: it exists to play, with real processes
// and the real kernel, what `agent_pi::transport` must tell apart — the
// pane's foreground program, a child that inherited one of its connections
// and a copy of its memory, and another image executed inside the same pid.
// Used by `app/src-tauri/src/agent_pi/transport/tests.rs` and by
// `scripts/pi_native.py`, both of which build it inside their own temporary
// directory.
//
//   exec-peer <socket path> <log file> <step>...
//
// It speaks `pi-observer` v2 as an observer does: ONE line per connection,
// Deck's one line of answer, the end. It remembers the lease and the nonce
// Deck last gave it, and puts them wherever a step says `@LEASE@` and
// `@NONCE@`. Every answer it reads is appended to the log.
//
// A step is
//   {...}            one message: connect, send that line, read the answer;
//                    when the answer is `again`, the same line once more with
//                    the new nonce, at most six times
//   @once:{...}      the same, without trying again
//   @sent:<file>     mark this file after the next message writes its first
//                    line, before reading the answer (retries stay ordinary)
//   @hello           the hello line as one message
//   @open            make a connection and keep it, saying nothing
//   @line:<text>     write that text and a newline on the kept connection
//   @part:<text>     write that text on it, with no newline
//   @read            read one answer from it (kept, like any other)
//   @touch           look at it as an event loop does (poll), writing nothing
//   @close           close it
//   @forget          forget the lease and the nonce
//   @save:<file>     write the lease and the nonce it holds into that file
//   @load:<file>     hold the lease and the nonce that file says
//   @fork            start a child that executes another program; wait for it
//   @kid             go on in a CHILD of this process with the steps up to
//                    `@parent`: it has this process's memory and descriptors,
//                    and stays in its process group. This process goes on
//                    after `@parent`, at once.
//   @child           the same, and the child leaves the terminal's session
//   @kidexec:<file>  the same as `@kid`, and the child executes that program,
//                    which is told the kept connection, the lease and the nonce
//   @exec:<file>     go on as that program IN THIS PID with the steps that are
//                    left; it keeps the connection and is told nothing else
//   @exec+:<file>    the same, and it is told the lease and the nonce as well
//   @mark:<file>     create that file
//   @wait:<file>     wait until that file exists
//   @sleep           stay for thirty seconds
// It ends after its last step, and after sixty seconds whatever it does.
#include <fcntl.h>
#include <poll.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <sys/wait.h>
#include <unistd.h>

static const char *path;
static const char *logfile;
static int kept = -1;
static char lease[32] = "0";
static char nonce[40] = "";

static int connected(void) {
    int fd = socket(AF_UNIX, SOCK_STREAM, 0);
    struct sockaddr_un a;
    memset(&a, 0, sizeof a);
    a.sun_family = AF_UNIX;
    strncpy(a.sun_path, path, sizeof a.sun_path - 1);
    if (fd < 0 || connect(fd, (struct sockaddr *)&a, sizeof a)) exit(3);
    return fd;
}

// `text` with the lease and the nonce put in.
static void filled(const char *text, char *out, size_t size) {
    size_t n = 0;
    while (*text && n + 48 < size) {
        if (!strncmp(text, "@LEASE@", 7)) {
            n += (size_t)snprintf(out + n, size - n, "%s", lease);
            text += 7;
        } else if (!strncmp(text, "@NONCE@", 7)) {
            n += (size_t)snprintf(out + n, size - n, "%s", nonce);
            text += 7;
        } else {
            out[n++] = *text++;
        }
    }
    out[n] = 0;
}

// Read one answer from `fd`, log it and keep what it gives. 1 when it says `again`.
static int answer(int fd) {
    char line[2048];
    int n = 0;
    char c;
    while (n < 2047 && read(fd, &c, 1) == 1) {
        line[n++] = c;
        if (c == '\n') break;
    }
    line[n] = 0;
    int out = open(logfile, O_WRONLY | O_CREAT | O_APPEND, 0600);
    if (out >= 0) {
        if (n) (void)!write(out, line, (size_t)n);
        else (void)!write(out, "(no answer)\n", 12);
        close(out);
    }
    const char *given = strstr(line, "\"nonce\":\"");
    if (given && strlen(given + 9) >= 32) {
        memcpy(nonce, given + 9, 32);
        nonce[32] = 0;
    }
    const char *accepted = strstr(line, "\"accepted\":");
    if (accepted) snprintf(lease, sizeof lease, "%ld", strtol(accepted + 11, 0, 10));
    return strstr(line, "\"again\":true") != 0;
}

static void say(int fd, const char *text, int newline) {
    char line[4096];
    filled(text, line, sizeof line);
    if (write(fd, line, strlen(line)) < 0) exit(5);
    if (newline && write(fd, "\n", 1) < 0) exit(5);
}

// One message on a connection of its own.
static const char *sent_marker;
static void message(const char *text, int tries) {
    for (;;) {
        int fd = connected();
        say(fd, text, 1);
        if (sent_marker) {
            int marker = open(sent_marker, O_CREAT | O_WRONLY, 0600);
            if (marker < 0) exit(5);
            close(marker);
            sent_marker = NULL;
        }
        int again = answer(fd);
        close(fd);
        if (!again || --tries <= 0) return;
    }
}

// The steps from `from` that belong to a child: up to `@parent`.
static int parent_at(int argc, char **argv, int from) {
    for (int i = from; i < argc; i++)
        if (!strcmp(argv[i], "@parent")) return i;
    return argc;
}

// Go on as `program` with the steps [from, to): it is told the connection,
// and the lease and the nonce only when `tell` says so.
static void become(const char *program, char **argv, int from, int to, int tell) {
    char state[128];
    if (kept >= 0) fcntl(kept, F_SETFD, 0);
    snprintf(state, sizeof state, "@state:%d:%s:%s", kept, tell ? lease : "0", tell ? nonce : "");
    char *next[96];
    int n = 0;
    next[n++] = (char *)program;
    next[n++] = (char *)path;
    next[n++] = (char *)logfile;
    next[n++] = state;
    for (int j = from; j < to && n < 95; j++) next[n++] = argv[j];
    next[n] = 0;
    execv(program, next);
    _exit(4);
}

int main(int argc, char **argv) {
    alarm(60);
    if (argc < 3) return 2;
    path = argv[1];
    logfile = argv[2];
    for (int i = 3; i < argc; i++) {
        const char *s = argv[i];
        if (!strncmp(s, "@state:", 7)) {
            char given[40] = "";
            sscanf(s + 7, "%d:%31[^:]:%39s", &kept, lease, given);
            snprintf(nonce, sizeof nonce, "%s", given);
        } else if (!strcmp(s, "@hello")) {
            message("{\"protocol\":\"pi-observer\",\"v\":2,\"source\":\"pi\"}", 1);
        } else if (s[0] == '{') {
            message(s, 6);
        } else if (!strncmp(s, "@once:", 6)) {
            message(s + 6, 1);
        } else if (!strncmp(s, "@sent:", 6)) {
            sent_marker = s + 6;
        } else if (!strcmp(s, "@open")) {
            kept = connected();
        } else if (!strncmp(s, "@line:", 6)) {
            say(kept, s + 6, 1);
        } else if (!strncmp(s, "@part:", 6)) {
            say(kept, s + 6, 0);
        } else if (!strcmp(s, "@read")) {
            answer(kept);
        } else if (!strcmp(s, "@touch")) {
            struct pollfd look = {kept, POLLIN | POLLOUT, 0};
            poll(&look, 1, 0);
        } else if (!strcmp(s, "@close")) {
            close(kept);
            kept = -1;
        } else if (!strcmp(s, "@forget")) {
            strcpy(lease, "0");
            nonce[0] = 0;
        } else if (!strncmp(s, "@save:", 6)) {
            FILE *out = fopen(s + 6, "w");
            if (out) {
                fprintf(out, "%s %s\n", lease, nonce);
                fclose(out);
            }
        } else if (!strncmp(s, "@load:", 6)) {
            FILE *in = fopen(s + 6, "r");
            if (in) {
                char given[40] = "";
                if (fscanf(in, "%31s %39s", lease, given) == 2) snprintf(nonce, sizeof nonce, "%s", given);
                fclose(in);
            }
        } else if (!strcmp(s, "@fork")) {
            pid_t p = fork();
            if (!p) {
                execl("/usr/bin/true", "true", (char *)0);
                _exit(1);
            }
            waitpid(p, 0, 0);
        } else if (!strcmp(s, "@kid") || !strcmp(s, "@child") || !strncmp(s, "@kidexec:", 9)) {
            int end = parent_at(argc, argv, i + 1);
            pid_t p = fork();
            if (!p) {
                alarm(60);
                if (!strcmp(s, "@child")) setsid();
                if (!strncmp(s, "@kidexec:", 9)) become(s + 9, argv, i + 1, end, 1);
                argc = end;
                continue;
            }
            i = end;
        } else if (!strncmp(s, "@exec:", 6)) {
            become(s + 6, argv, i + 1, argc, 0);
        } else if (!strncmp(s, "@exec+:", 7)) {
            become(s + 7, argv, i + 1, argc, 1);
        } else if (!strncmp(s, "@mark:", 6)) {
            int m = open(s + 6, O_CREAT | O_WRONLY, 0600);
            if (m >= 0) close(m);
        } else if (!strncmp(s, "@wait:", 6)) {
            while (access(s + 6, F_OK)) usleep(20000);
        } else if (!strcmp(s, "@sleep")) {
            sleep(30);
        }
    }
    return 0;
}
