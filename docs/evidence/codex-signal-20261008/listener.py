"""Historical collector source, archived for inspection only.
Runtime paths and imports refer to a deleted experiment, not this directory.
Do not execute or point this probe at production services.
Only this archive notice was added during documentation publication.
"""
from probe_lib import *
import socket,struct,threading
s=socket.socket(socket.AF_UNIX);s.bind(str(BASE/'evidence.sock'));os.chmod(BASE/'evidence.sock',0o600);s.listen()
def handle(c):
 try:
  peer=struct.unpack('i',c.getsockopt(0,2,4))[0]
  data=b''
  while b'\n' not in data:
   part=c.recv(4096)
   if not part:return
   data+=part
   if len(data)>4096:return
  obj=json.loads(data)
  pp,table=panes();chain=[];pid=peer
  while pid>1 and len(chain)<32 and pid in table:
   r=table[pid];chain.append(r);pid=r['ppid']
  obj.update({'time':time.time(),'phase':(BASE/'phase').read_text(),'peer_pid':peer,'ancestry':chain,'panes':pp})
  with open(BASE/'evidence/events.jsonl','a') as f:f.write(json.dumps(obj)+'\n')
 finally:c.close()
while True:
 c,_=s.accept();threading.Thread(target=handle,args=(c,),daemon=True).start()
