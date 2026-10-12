# A listener for one measurement: accepts connections, optionally answers
# one line to each after a delay, holds them, and ends by itself.
import os, socket, sys, time, threading
path, answer_after, life = sys.argv[1], float(sys.argv[2]), float(sys.argv[3])
try: os.unlink(path)
except FileNotFoundError: pass
s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
s.bind(path); os.chmod(path, 0o600); s.listen(16); s.settimeout(0.05)
held = []
end = time.time() + life
def serve(c):
    if answer_after >= 0:
        time.sleep(answer_after)
        try:
            c.sendall(b'{"ok":true}\n'); c.close()
        except OSError: pass
print("ready", flush=True)
while time.time() < end:
    try:
        c, _ = s.accept()
    except socket.timeout:
        continue
    held.append(c)
    threading.Thread(target=serve, args=(c,), daemon=True).start()
os.unlink(path)
