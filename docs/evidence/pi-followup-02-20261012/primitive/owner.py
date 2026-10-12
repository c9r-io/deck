"""Follow-up 02, experiment B: what LOCAL_PEERPID says of a connection that two processes hold.

  owner.py <root> <edr_runtime.py>

The server asks the kernel for the peer pid of each accepted connection after every step. `who` in the output is
`parent`, `child` or the number itself. Nothing is stopped: the client ends at the end of its input."""
import json, socket, struct, subprocess, sys

root, runtime = sys.argv[1], sys.argv[2]
path = f"{root}/b.sock"
server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
server.bind(path)
server.listen(8)
server.settimeout(5)
client = subprocess.Popen([f"{root}/bin/owner", path], stdin=subprocess.PIPE,
                          stdout=subprocess.PIPE, text=True, bufsize=1)
subprocess.run([sys.executable, runtime, "experiment", "add-process", "--root", root, "--pid", str(client.pid)],
               check=True, capture_output=True)
conns, pids, rows = {}, {"parent": client.pid}, []

def tell(line):
    client.stdin.write(line + "\n")
    client.stdin.flush()
    return client.stdout.readline().strip()

def owner(name):
    try:
        pid = struct.unpack("i", conns[name].getsockopt(0, 2, 4))[0]   # SOL_LOCAL, LOCAL_PEERPID
    except OSError as error:
        return f"errno {error.errno}"
    return next((who for who, known in pids.items() if known == pid), pid)

def step(line, name=None, accept=False, note=""):
    answer = tell(line)
    if line == "P fork":
        pids["child"] = int(answer.split()[1])
        subprocess.run([sys.executable, runtime, "experiment", "add-process", "--root", root, "--pid", str(pids["child"])],
                       check=True, capture_output=True)
    if accept:
        conns[name], _ = server.accept()
    rows.append({"step": line, "answer": answer, "peer_pid_is": owner(name) if name in conns else None, "note": note})

# one connection that both hold, as an observer's long-lived stream was
step("P connect shared", "shared", accept=True, note="the parent connected")
step("P write shared", "shared", note="the parent wrote")
step("P socket early", note="the parent makes a second socket and leaves it unconnected")
step("P fork")
step("C write shared", "shared", note="the child wrote on the inherited connection")
step("P getsockopt shared", "shared", note="parent: getsockopt(SO_ERROR)")
step("P fstat shared", "shared", note="parent: fstat")
step("P dupclose shared", "shared", note="parent: dup and close of the copy")
step("P recv shared", "shared", note="parent: a read attempt with nothing to read")
step("C write shared", "shared")
step("P poll shared", "shared", note="parent: poll, no wait")
step("C write shared", "shared")
step("P kq shared", "shared", note="parent: registers the descriptor in a kqueue")
step("C write shared", "shared")
step("P write shared", "shared", note="parent: its own byte after the child's (both are in the buffer)")
# a socket the parent made before the fork and never connected: the child connects it and writes, then the parent tries
step("C connect early", "early", accept=True, note="the parent's unconnected socket, connected by the child")
step("C write early", "early")
step("P connect early", "early", note="parent: connect on the socket the child already connected")
# a connection each makes for itself after the fork: the other never holds it
step("C connect childs", "childs", accept=True, note="made by the child after the fork")
step("C write childs", "childs")
step("P connect parents", "parents", accept=True, note="made by the parent after the fork")
step("P write parents", "parents")
client.stdin.close()
client.wait(timeout=10)
buffered = len(conns["shared"].recv(64))
print(json.dumps({"rows": rows, "bytes_waiting_on_the_shared_connection": buffered, "client_exit": client.returncode}, indent=1))
