"""Historical collector source, archived for inspection only.
Runtime paths and imports refer to a deleted experiment, not this directory.
Do not execute or point this probe at production services.
Only this archive notice was added during documentation publication.
"""
import json,sys,os,socket,re,pathlib
BASE=pathlib.Path(__file__).parent
p=json.load(sys.stdin)
allowed={}
for k in ['hook_event_name','session_id','thread_id','turn_id','client_id','connection_id','subscriber_id','originator']:
 v=p.get(k)
 if isinstance(v,str) and re.fullmatch(r'[A-Za-z0-9_.:-]{1,128}',v):allowed[k]=v
p.clear()
allowed['pane_marker']=os.environ.get('TMUX_PANE','') if re.fullmatch(r'%[0-9]+',os.environ.get('TMUX_PANE','')) else 'absent'
allowed['socket_marker']=next((x for x in ['A','B','E'] if os.environ.get('DECK_STATUS_SOCK','')==str(BASE/('marker-'+x))), 'absent')
s=socket.socket(socket.AF_UNIX);s.settimeout(10);s.connect(str(BASE/'evidence.sock'));s.sendall(json.dumps(allowed).encode()+b'\n');s.shutdown(socket.SHUT_WR);s.recv(1)
