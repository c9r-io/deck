"""Follow-up 02, experiment C: does registering a process watch again lose a report that was not taken yet?

  rewatch.py <root> <edr_runtime.py>

One kqueue, one process. A fork (then an exec) is caused and NOT taken from the queue; the same registration is
made again (EV_ADD, the same flags), as a second watch of the same pid does; then the queue is read."""
import json, select, subprocess, sys, time

root, runtime = sys.argv[1], sys.argv[2]
client = subprocess.Popen([f"{root}/bin/owner", f"{root}/none.sock"], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                          text=True, bufsize=1)
subprocess.run([sys.executable, runtime, "experiment", "add-process", "--root", root, "--pid", str(client.pid)],
               check=True, capture_output=True)
queue = select.kqueue()
flags = select.KQ_NOTE_FORK | select.KQ_NOTE_EXEC | select.KQ_NOTE_EXIT
watch = select.kevent(client.pid, select.KQ_FILTER_PROC, select.KQ_EV_ADD | select.KQ_EV_CLEAR, flags)

def names(events):
    out = []
    for event in events:
        out.append(sorted(name for name, bit in (("FORK", select.KQ_NOTE_FORK), ("EXEC", select.KQ_NOTE_EXEC),
                                                 ("EXIT", select.KQ_NOTE_EXIT)) if event.fflags & bit))
    return out

def tell(line):
    client.stdin.write(line + "\n"); client.stdin.flush()
    return client.stdout.readline().strip()

rows = []
queue.control([watch], 0, 0)
rows.append({"step": "registered; nothing happened", "taken": names(queue.control(None, 8, 0))})
tell("P fork"); time.sleep(0.05)
queue.control([watch], 0, 0)                      # the second registration, before anything was taken
rows.append({"step": "fork, then registered again, then read", "taken": names(queue.control(None, 8, 0))})
rows.append({"step": "read again", "taken": names(queue.control(None, 8, 0))})
queue.control([watch], 0, 0)
rows.append({"step": "registered again with nothing pending, then read", "taken": names(queue.control(None, 8, 0))})
tell("P exec"); time.sleep(0.05)
queue.control([watch], 0, 0)
rows.append({"step": "exec, then registered again, then read", "taken": names(queue.control(None, 8, 0))})
tell("P fork"); tell("P exec"); time.sleep(0.05)
queue.control([watch], 0, 0)
rows.append({"step": "fork and exec, then registered again, then read", "taken": names(queue.control(None, 8, 0))})
client.stdin.close()
client.wait(timeout=10)
time.sleep(0.05)
rows.append({"step": "the process ended", "taken": names(queue.control(None, 8, 0))})
try:
    queue.control([watch], 0, 0)
    rows.append({"step": "registering a pid that is gone", "result": "accepted"})
except OSError as error:
    rows.append({"step": "registering a pid that is gone", "result": f"errno {error.errno}"})
print(json.dumps({"rows": rows, "client_exit": client.returncode}, indent=1))
