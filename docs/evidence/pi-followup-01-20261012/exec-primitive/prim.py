"""Which kernel fact notices an exec inside one pid? Orchestrator.

For one peer it records, after every step: LOCAL_PEERPID, the pid and
pidversion of LOCAL_PEERTOKEN, the birth instant of proc_bsdinfo, and what a
kqueue EVFILT_PROC watch (NOTE_EXEC | NOTE_EXIT | NOTE_FORK) registered at the
hello has reported. Nothing is killed: every peer exits on EOF or on 'q'.
"""
import ctypes, json, os, select, socket, struct, subprocess, sys, time

root = sys.argv[1]
A, B = root + "/bin/peer-a", root + "/bin/peer-b"
libproc = ctypes.CDLL("/usr/lib/libproc.dylib", use_errno=True)

def birth(pid):
    buf = ctypes.create_string_buffer(136)  # struct proc_bsdinfo
    got = libproc.proc_pidinfo(pid, 3, ctypes.c_uint64(0), buf, 136)
    if got != 136:
        return None
    sec, usec = struct.unpack_from("QQ", buf.raw, 120)
    return [sec, usec]

def path(pid):
    buf = ctypes.create_string_buffer(4096)
    n = libproc.proc_pidpath(pid, buf, 4096)
    return os.path.basename(buf.value.decode()) if n > 0 else None

def token(conn):
    try:
        raw = conn.getsockopt(0, 6, 32)  # SOL_LOCAL, LOCAL_PEERTOKEN
    except OSError as e:
        return {"error": e.errno}
    v = struct.unpack("8I", raw)
    return {"pid": v[5], "pidversion": v[7]}

def peerpid(conn):
    try:
        return struct.unpack("i", conn.getsockopt(0, 2, 4))[0]
    except OSError as e:
        return {"error": e.errno}

def drain(kq):
    out = []
    for ev in kq.control(None, 8, 0):
        names = []
        if ev.fflags & select.KQ_NOTE_EXEC: names.append("EXEC")
        if ev.fflags & select.KQ_NOTE_EXIT: names.append("EXIT")
        if ev.fflags & select.KQ_NOTE_FORK: names.append("FORK")
        out.append({"ident": ev.ident, "fflags": names})
    return out

def readline(conn, timeout=5.0):
    conn.settimeout(timeout)
    data = b""
    while not data.endswith(b"\n"):
        chunk = conn.recv(1)
        if not chunk:
            return None
        data += chunk
    return data.decode().strip()

def scenario(name, launch_mode, steps, with_fork_note=True):
    sock = root + "/s-" + name
    try: os.unlink(sock)
    except FileNotFoundError: pass
    srv = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    srv.bind(sock); srv.listen(4); srv.settimeout(5)
    env = {"PATH": "/usr/bin:/bin"}
    child = subprocess.Popen([A, B, sock, "x", launch_mode], env=env, close_fds=True,
                             stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    conn, _ = srv.accept()
    line = readline(conn)
    pid = int(line.split()[1])
    kq = select.kqueue()
    ff = select.KQ_NOTE_EXEC | select.KQ_NOTE_EXIT | (select.KQ_NOTE_FORK if with_fork_note else 0)
    reg = None
    try:
        kq.control([select.kevent(pid, select.KQ_FILTER_PROC, select.KQ_EV_ADD, ff)], 0, 0)
        reg = "ok"
    except OSError as e:
        reg = {"error": e.errno}
    rows = [{"step": "hello", "line": line, "is_child_of_watcher": pid == child.pid, "ppid": os.popen("ps -o ppid= -p %d" % pid).read().strip(),
             "peerpid": peerpid(conn), "token": token(conn), "birth": birth(pid), "image": path(pid), "watch_registered": reg, "watch": drain(kq)}]
    for step in steps:
        if step.startswith("replace-file"):
            # the peer's own executable is replaced on disk: a new inode at the same path
            target = A
            tmp = target + ".new"
            with open(B, "rb") as f: data = f.read()
            with open(tmp, "wb") as f: f.write(data + b"\0replaced")
            os.chmod(tmp, 0o755); os.rename(tmp, target)
            line = "(file at the peer's path replaced)"
        else:
            conn.sendall(step.encode())
            line = readline(conn) if step in "xspfew" else None
        time.sleep(0.15)
        rows.append({"step": step, "line": line, "peerpid": peerpid(conn), "token": token(conn),
                     "birth": birth(pid), "image": path(pid), "watch": drain(kq)})
    conn.sendall(b"q")
    time.sleep(0.3)
    rows.append({"step": "quit", "peerpid": peerpid(conn), "token": token(conn), "birth": birth(pid), "watch": drain(kq)})
    conn.close(); srv.close(); kq.close()
    try: child.wait(timeout=5)
    except Exception: pass
    return {"scenario": name, "rows": rows}

out = []
out.append(scenario("exec-other", "connect", ["w", "x", "w"]))
out.append(scenario("exec-self", "connect", ["s", "w"]))
out.append(scenario("spawn-setexec", "connect", ["p", "w"]))
out.append(scenario("fork-child-writes", "connect", ["f", "w"]))
out.append(scenario("fork-child-execs", "connect", ["e", "w"]))
out.append(scenario("detached-exec-other", "detach", ["w", "x", "w"], with_fork_note=False))
out.append(scenario("file-replaced", "connect", ["w", "replace-file", "w"], with_fork_note=False))
json.dump(out, open(root + "/out/primitive.json", "w"), indent=1)
for sc in out:
    print("==", sc["scenario"])
    for r in sc["rows"]:
        print("  ", json.dumps(r, sort_keys=True))
