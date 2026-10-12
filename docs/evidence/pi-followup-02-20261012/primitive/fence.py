"""Follow-up 02, experiment A: the order of a fork report and the child's first byte.

  fence.py <root> fork|spawn <count>

A watch (kqueue EVFILT_PROC, NOTE_FORK | NOTE_EXEC | NOTE_EXIT) is registered for the client before it makes any
child. For every byte 'c' — written by a child as its first act — the queue is asked WITHOUT waiting, after the
byte was read: a fork report must be there. A byte 'c' that is read while no fork was reported since the parent's
last byte is counted as `unreported`: one such case would mean a child can act before its fork is visible.
"""
import json, os, select, socket, subprocess, sys

root, mode, count = sys.argv[1], sys.argv[2], int(sys.argv[3])
path = f"{root}/a-{mode}.sock"
if os.path.exists(path):
    os.unlink(path)
server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
server.bind(path)
server.listen(1)
client = subprocess.Popen([f"{root}/bin/fence", path, mode, str(count)])
subprocess.run([sys.executable, sys.argv[4], "experiment", "add-process", "--root", root, "--pid", str(client.pid)], check=True,
               capture_output=True)
conn, _ = server.accept()
hello = b""
while not hello.endswith(b"\n"):
    hello += conn.recv(1)
pid = int(hello.split()[1])
queue = select.kqueue()
flags = select.KQ_NOTE_FORK | select.KQ_NOTE_EXEC | select.KQ_NOTE_EXIT
queue.control([select.kevent(pid, select.KQ_FILTER_PROC, select.KQ_EV_ADD | select.KQ_EV_CLEAR, flags)], 0, 0)
conn.sendall(b"g")
seen = {"children_bytes": 0, "parent_bytes": 0, "unreported": 0, "fork_reports": 0, "exec_reports": 0}
forked = False
while True:
    byte = conn.recv(1)
    if not byte:
        break
    for event in queue.control(None, 8, 0):
        if event.fflags & select.KQ_NOTE_FORK:
            forked = True
            seen["fork_reports"] += 1
        if event.fflags & select.KQ_NOTE_EXEC:
            seen["exec_reports"] += 1
    if byte == b"c":
        seen["children_bytes"] += 1
        if not forked:
            seen["unreported"] += 1
    else:
        seen["parent_bytes"] += 1
        forked = False
        conn.sendall(b"g")
client.wait(timeout=30)
print(json.dumps({"mode": mode, "asked": count, "client_exit": client.returncode, **seen}))
