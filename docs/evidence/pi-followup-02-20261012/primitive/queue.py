"""Follow-up 02, experiment D: the order in which accept() hands connections over.

  queue.py <root> <edr_runtime.py> <clients> <connects-per-client>

Part 1, nothing left to scheduling. The listener accepts nothing while one process makes 20 connections one
after the other, then this process makes one of its own (the "sentinel"), then another process makes 20 more.
Only then everything is accepted: the order must be the first 20 in their order, the sentinel, the last 20.

Part 2, under load. <clients> processes connect as fast as they can (each writes the clock before and after its
connect() into its line) while this process accepts one connection at a time and, from a second thread, makes a
sentinel connection of its own again and again, noting the clock before and after each. Read in accept order:

- `behind_but_earlier`: a connection accepted BEHIND a sentinel whose connect() had already returned before
  that sentinel's connect() was called. One such case would mean that "accepted behind the sentinel" does not
  show "connected after it".
- `ahead_but_later`: a connection accepted AHEAD of a sentinel whose connect() was called only after that
  sentinel's had returned. (The other direction: it would cost a retry, never a wrong answer.)
- `overlapping`: connections whose connect() overlapped a sentinel's in time — what makes the run a test.
"""
import json, os, socket, subprocess, sys, threading, time

root, runtime, clients, per = sys.argv[1], sys.argv[2], int(sys.argv[3]), int(sys.argv[4])
now = lambda: time.clock_gettime_ns(time.CLOCK_UPTIME_RAW)

def listen(name, backlog):
    path = f"{root}/{name}.sock"
    if os.path.exists(path):
        os.unlink(path)
    server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    server.bind(path)
    server.listen(backlog)
    return server, path

def client(path, count, label, jitter=0):
    process = subprocess.Popen([f"{root}/bin/queue", path, str(count), label, str(jitter)], stderr=subprocess.PIPE, text=True)
    subprocess.run([sys.executable, runtime, "experiment", "add-process", "--root", root, "--pid", str(process.pid)],
                   check=True, capture_output=True)
    return process

def line_of(conn):
    data = b""
    while not data.endswith(b"\n"):
        chunk = conn.recv(256)
        if not chunk:
            break
        data += chunk
    conn.close()
    return data.decode().split()

# ---- part 1
server, path = listen("d1", 64)
first = client(path, 20, "a"); first.wait(timeout=30)
sentinel = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
sentinel.connect(path); sentinel.sendall(b"S 0\n")
second = client(path, 20, "b"); second.wait(timeout=30)
order = []
for _ in range(41):
    conn, _ = server.accept()
    words = line_of(conn)
    order.append(words[0] if words[0] == "S" else f"{words[1]}{words[2]}")
sentinel.close(); server.close()
expected = [f"a{i}" for i in range(20)] + ["S"] + [f"b{i}" for i in range(20)]
part1 = {"accepted": order, "as_connected": order == expected, "client_exits": [first.returncode, second.returncode]}

# ---- part 2
server, path = listen("d2", 128)
records, sentinels, done = [], {}, threading.Event()

def accepting():
    server.settimeout(0.2)
    while True:
        try:
            conn, _ = server.accept()
        except socket.timeout:
            if done.is_set():
                return
            continue
        conn.settimeout(5)
        records.append(line_of(conn))

def fencing():
    number = 0
    while not done.is_set():
        number += 1
        probe = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        before = now()
        try:
            probe.connect(path)
        except OSError:
            probe.close(); time.sleep(0.0005); continue
        after = now()
        sentinels[number] = (before, after)
        probe.sendall(f"S {number}\n".encode()); probe.close()
        time.sleep(0.0003)

threads = [threading.Thread(target=accepting), threading.Thread(target=fencing)]
for thread in threads:
    thread.start()
running = [client(path, per, f"p{index}", 150) for index in range(clients)]
refused = 0
for process in running:
    process.wait(timeout=110)
    refused += int((process.stderr.read().split() or ["refused", "0"])[1])
time.sleep(0.3)
done.set()
for thread in threads:
    thread.join()
server.close()

latest_before, behind_but_earlier, ahead = None, 0, []
seen_sentinels, seen_clients, overlapping = 0, 0, 0
spans = sorted(sentinels.values())
for words in records:
    if words[0] == "S":
        seen_sentinels += 1
        before = sentinels[int(words[1])][0]
        latest_before = before if latest_before is None else max(latest_before, before)
    else:
        seen_clients += 1
        called, returned = int(words[3]), int(words[4])
        if latest_before is not None and returned < latest_before:
            behind_but_earlier += 1
        ahead.append(called)
earliest_after, ahead_but_later, at = None, 0, len(ahead)
for words in reversed(records):
    if words[0] == "S":
        after = sentinels[int(words[1])][1]
        earliest_after = after if earliest_after is None else min(earliest_after, after)
    else:
        at -= 1
        if earliest_after is not None and ahead[at] > earliest_after:
            ahead_but_later += 1
import bisect
starts = [span[0] for span in spans]
for words in records:
    if words[0] == "C":
        called, returned = int(words[3]), int(words[4])
        index = bisect.bisect_left(starts, called)
        for span in spans[max(0, index - 1):index + 2]:
            if span[0] < returned and called < span[1]:
                overlapping += 1
                break
part2 = {"clients": clients, "connects_asked": clients * per, "connections_accepted": seen_clients,
         "sentinels_accepted": seen_sentinels, "refused_and_tried_again": refused,
         "client_exits": [process.returncode for process in running],
         "overlapping": overlapping, "behind_but_earlier": behind_but_earlier, "ahead_but_later": ahead_but_later}
print(json.dumps({"part1": part1, "part2": part2}, indent=1))
