// kid.c — TEST ONLY. The child a runtime is asked to make: it says who it is,
// who its parent is and which descriptors it was BORN with, and ends.
//
//   kid <file>     writes one JSON line to <file> (created exclusively)
//
// Every descriptor from 0 to 1023 that is open is listed with what it is; a
// socket also with its peer's path, so that a connection of the parent that
// reached the child is recognized for what it is. On a socket beyond its
// three standard descriptors the child also says what state it is in
// ("open": connected; "eof": was connected, the other end closed;
// "unconnected") and writes one line, `kid <pid>`, and says how many bytes
// the kernel took: what a child that holds its parent's connection can do.
#include <errno.h>
#include <fcntl.h>
#include <signal.h>
#include <stdio.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/un.h>
#include <unistd.h>

int main(int argc, char **argv) {
  if (argc != 2) return 2;
  signal(SIGPIPE, SIG_IGN);
  char line[8192];
  size_t at = 0;
  at += (size_t)snprintf(line + at, sizeof line - at, "{\"pid\":%d,\"ppid\":%d,\"fds\":[", getpid(), getppid());
  int first = 1, sockets = 0;
  for (int fd = 0; fd < 1024 && at < sizeof line - 512; fd++) {
    int flags = fcntl(fd, F_GETFD);
    if (flags == -1) continue;
    struct stat st;
    const char *kind = "other";
    if (fstat(fd, &st) == 0) {
      if (S_ISSOCK(st.st_mode)) kind = "sock";
      else if (S_ISFIFO(st.st_mode)) kind = "fifo";
      else if (S_ISCHR(st.st_mode)) kind = "chr";
      else if (S_ISREG(st.st_mode)) kind = "reg";
      else if (S_ISDIR(st.st_mode)) kind = "dir";
    }
    char peer[128] = "";
    char more[96] = "";
    if (strcmp(kind, "sock") == 0) {
      sockets++;
      if (fd > 2) {
        char byte;
        ssize_t seen = recv(fd, &byte, 1, MSG_PEEK | MSG_DONTWAIT);
        const char *state = seen == 0 ? "eof" : seen > 0 ? "open" : errno == EAGAIN ? "open" : errno == ENOTCONN ? "unconnected" : "other";
        char said[32];
        int len = snprintf(said, sizeof said, "kid %d\n", getpid());
        ssize_t wrote = send(fd, said, (size_t)len, 0);
        snprintf(more, sizeof more, ",\"state\":\"%s\",\"wrote\":%zd,\"errno\":%d", state, wrote, wrote < 0 ? errno : 0);
      }
      struct sockaddr_un addr;
      socklen_t len = sizeof addr;
      memset(&addr, 0, sizeof addr);
      if (getpeername(fd, (struct sockaddr *)&addr, &len) == 0 && addr.sun_family == AF_UNIX) {
        size_t j = 0;
        for (const char *c = addr.sun_path; *c && j < sizeof peer - 1; c++)
          if (*c != '"' && *c != '\\' && (unsigned char)*c >= 0x20) peer[j++] = *c;
        peer[j] = 0;
      }
    }
    at += (size_t)snprintf(line + at, sizeof line - at, "%s{\"fd\":%d,\"kind\":\"%s\",\"cloexec\":%d,\"peer\":\"%s\"%s}",
                           first ? "" : ",", fd, kind, (flags & FD_CLOEXEC) != 0, peer, more);
    first = 0;
  }
  at += (size_t)snprintf(line + at, sizeof line - at, "],\"sockets\":%d}\n", sockets);
  // the descriptor this line is written through is opened only now
  int out = open(argv[1], O_WRONLY | O_CREAT | O_EXCL, 0600);
  if (out < 0) return 3;
  ssize_t done = write(out, line, at);
  close(out);
  return done == (ssize_t)at ? 0 : 4;
}
