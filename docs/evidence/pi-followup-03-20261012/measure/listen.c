// listen.c — TEST ONLY. The other end of the measurement: a process of its
// own that owns a UNIX stream socket and says, when asked, whether a
// connection to it EXISTS at that instant.
//
//   listen probe <socket> <ctl fifo> <ack fifo> <log>
//       one command line per question on <ctl>, one answer line on <ack>:
//         p <n>  is a connection waiting right now? If so take it, read the
//                kernel's peer pid, close it.           -> "<n> <0|1> <pid>"
//         k <n>  the same, but the connection is kept open until the end
//         d <n>  take and close everything that waits   -> "<n> <count> 0"
//         q      end
//   listen all <socket> <log>
//       take whatever comes, keep each connection for 20 ms and say when a
//       line `kid <pid>` arrives on one: a line a CHILD of the connecting
//       process wrote. With it go the kernel's peer pid when the connection
//       was taken and its peer pid after the line (LOCAL_PEERPID names the
//       last process that used the peer socket). Until the process is stopped.
//
// The answer to `p` is read with a zero timeout: it is what the kernel's
// queue held when the question was read, nothing later.
#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/resource.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <time.h>
#include <unistd.h>

static FILE *out;

static double now_ms(void) {
  struct timespec t;
  clock_gettime(CLOCK_MONOTONIC, &t);
  return (double)t.tv_sec * 1e3 + (double)t.tv_nsec / 1e6;
}

static int listener(const char *path) {
  struct sockaddr_un addr;
  memset(&addr, 0, sizeof addr);
  addr.sun_family = AF_UNIX;
  if (strlen(path) >= sizeof addr.sun_path) {
    fprintf(stderr, "socket path too long\n");
    exit(2);
  }
  strcpy(addr.sun_path, path);
  unlink(path);
  int fd = socket(AF_UNIX, SOCK_STREAM, 0);
  if (fd < 0 || bind(fd, (struct sockaddr *)&addr, sizeof addr) != 0 || listen(fd, 128) != 0) {
    perror("listen");
    exit(2);
  }
  return fd;
}

static int waiting(int lfd) {
  struct pollfd p = {.fd = lfd, .events = POLLIN};
  return poll(&p, 1, 0) == 1 && (p.revents & POLLIN);
}

static pid_t peer_of(int fd) {
  pid_t pid = 0;
  socklen_t len = sizeof pid;
  if (getsockopt(fd, SOL_LOCAL, LOCAL_PEERPID, &pid, &len) != 0) return -1;
  return pid;
}

int main(int argc, char **argv) {
  signal(SIGPIPE, SIG_IGN);
  if (argc == 4 && strcmp(argv[1], "all") == 0) {
    int lfd = listener(argv[2]);
    out = fopen(argv[3], "a");
    unsigned long taken = 0;
    fprintf(out, "{\"mode\":\"all\",\"pid\":%d,\"t\":%.3f}\n", getpid(), now_ms());
    fflush(out);
    enum { CAP = 4000 };
    static struct { int fd; double since; pid_t at_accept; } kept[CAP];
    static struct pollfd polled[CAP + 1];
    int count = 0;
    unsigned long lines = 0;
    struct rlimit files = {8192, 8192};
    setrlimit(RLIMIT_NOFILE, &files);
    fcntl(lfd, F_SETFL, fcntl(lfd, F_GETFL) | O_NONBLOCK);
    for (;;) {
      polled[0].fd = lfd;
      polled[0].events = count < CAP ? POLLIN : 0;
      for (int i = 0; i < count; i++) {
        polled[i + 1].fd = kept[i].fd;
        polled[i + 1].events = POLLIN;
      }
      if (poll(polled, (nfds_t)count + 1, 2) < 0 && errno != EINTR) break;
      double now = now_ms();
      int had = count;
      // what was kept: a line of a child, the other end's close, or its 20 ms are over
      for (int i = had - 1; i >= 0; i--) {
        int done = now - kept[i].since > 20.0;
        if (polled[i + 1].revents & (POLLIN | POLLHUP | POLLERR)) {
          char buf[64];
          ssize_t got = recv(kept[i].fd, buf, sizeof buf - 1, MSG_DONTWAIT);
          if (got > 0) {
            buf[got] = 0;
            if (strncmp(buf, "kid ", 4) == 0) {
              fprintf(out, "{\"line_from_child\":%ld,\"peer_when_taken\":%d,\"peer_after_the_line\":%d,\"t\":%.3f}\n",
                      strtol(buf + 4, NULL, 10), (int)kept[i].at_accept, (int)peer_of(kept[i].fd), now);
              fflush(out);
              lines++;
            }
            done = 1;
          } else if (got == 0 || (errno != EAGAIN && errno != EINTR)) {
            // the connecting process closed its end; a child that holds the same socket keeps it open
            if (got == 0) done = 1;
          }
        }
        if (done) {
          close(kept[i].fd);
          kept[i] = kept[--count];
        }
      }
      while (count < CAP) {
        int fd = accept(lfd, NULL, NULL);
        if (fd < 0) break;
        kept[count].fd = fd;
        kept[count].since = now;
        kept[count].at_accept = peer_of(fd);
        count++;
        if (++taken % 20000 == 0) {
          fprintf(out, "{\"taken\":%lu,\"lines_from_children\":%lu,\"t\":%.3f}\n", taken, lines, now);
          fflush(out);
        }
      }
    }
    return 0;
  }
  if (argc != 6 || strcmp(argv[1], "probe") != 0) {
    fprintf(stderr, "usage: listen probe <socket> <ctl> <ack> <log> | listen all <socket> <log>\n");
    return 2;
  }
  int lfd = listener(argv[2]);
  // both ends of each fifo, so that neither open waits for the other side
  int ctl = open(argv[3], O_RDWR);
  int ack = open(argv[4], O_RDWR);
  out = fopen(argv[5], "a");
  if (ctl < 0 || ack < 0 || !out) {
    perror("open");
    return 2;
  }
  fprintf(out, "{\"mode\":\"probe\",\"pid\":%d,\"t\":%.3f}\n", getpid(), now_ms());
  fflush(out);
  char buf[256];
  size_t have = 0;
  for (;;) {
    ssize_t got = read(ctl, buf + have, sizeof buf - 1 - have);
    if (got <= 0) {
      if (got < 0 && errno == EINTR) continue;
      break;
    }
    have += (size_t)got;
    buf[have] = 0;
    char *line = buf, *end;
    while ((end = strchr(line, '\n')) != NULL) {
      *end = 0;
      char kind = line[0];
      long n = line[1] ? strtol(line + 2, NULL, 10) : 0;
      char answer[96];
      if (kind == 'q') {
        fprintf(out, "{\"quit\":true,\"t\":%.3f}\n", now_ms());
        fflush(out);
        return 0;
      } else if (kind == 'p' || kind == 'k') {
        int was = waiting(lfd);
        pid_t peer = 0;
        if (was) {
          int fd = accept(lfd, NULL, NULL);
          peer = fd >= 0 ? peer_of(fd) : -1;
          if (fd >= 0 && kind == 'p') close(fd);
        }
        snprintf(answer, sizeof answer, "%ld %d %d\n", n, was, (int)peer);
        fprintf(out, "{\"ask\":\"%c\",\"n\":%ld,\"waiting\":%d,\"peer\":%d,\"t\":%.3f}\n", kind, n, was, (int)peer, now_ms());
      } else if (kind == 'd') {
        int count = 0;
        while (waiting(lfd)) {
          int fd = accept(lfd, NULL, NULL);
          if (fd < 0) break;
          close(fd);
          count++;
        }
        snprintf(answer, sizeof answer, "%ld %d 0\n", n, count);
        fprintf(out, "{\"ask\":\"d\",\"n\":%ld,\"taken\":%d,\"t\":%.3f}\n", n, count, now_ms());
      } else {
        snprintf(answer, sizeof answer, "%ld -1 0\n", n);
      }
      fflush(out);
      if (write(ack, answer, strlen(answer)) < 0) return 3;
      line = end + 1;
    }
    have = strlen(line);
    memmove(buf, line, have);
  }
  return 0;
}
