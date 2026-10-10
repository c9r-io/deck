"""Historical collector source, archived for inspection only.
Runtime paths and imports refer to a deleted experiment, not this directory.
Do not execute or point this probe at production services.
Only this archive notice was added during documentation publication.
"""
import ctypes,os,pathlib,json,subprocess,time
BASE=pathlib.Path(pathlib.Path('/tmp/deck-cert-20261008/runtime-root').read_text()); META=json.load(open(BASE/'meta.json'))
TMUX='/Applications/deck.app/Contents/MacOS/tmux'; SOCK=str(BASE/'tmux.sock')
class BSD(ctypes.Structure):
 _fields_=[(x,ctypes.c_uint32) for x in ['flags','status','xstatus','pid','ppid','uid','gid','ruid','rgid','svuid','svgid','rfu']]+[('comm',ctypes.c_char*16),('name',ctypes.c_char*32)]+[(x,ctypes.c_uint32) for x in ['nfiles','pgid','jobc','tty','tpgid']]+[('nice',ctypes.c_int32),('sec',ctypes.c_uint64),('usec',ctypes.c_uint64)]
LIB=ctypes.CDLL('/usr/lib/libproc.dylib')
def one(pid):
 b=BSD(); n=LIB.proc_pidinfo(int(pid),3,0,ctypes.byref(b),ctypes.sizeof(b))
 if n!=ctypes.sizeof(b):return None
 return {k:(0 if k=='tty' and getattr(b,k)==0xffffffff else getattr(b,k)) for k in ['pid','ppid','pgid','tty','tpgid','sec','usec']}
def snapshot():
 arr=(ctypes.c_int*32768)();n=LIB.proc_listallpids(arr,ctypes.sizeof(arr));return {p:r for p in arr[:max(n,0)] if p>0 and (r:=one(p))}
def tm(*args,check=True):
 return subprocess.run([TMUX,'-S',SOCK,*args],env=META['env'],capture_output=True,text=True,check=check).stdout.strip()
def panes():
 rows=tm('list-panes','-a','-F','#{pane_id} #{pane_pid} #{pane_tty}').splitlines();result={}
 table=snapshot()
 for line in rows:
  pane,pid,tty=line.split();r=table.get(int(pid));fg=table.get(r['tpgid']) if r else None
  result[pane]={'pane_pid':int(pid),'process':r,'foreground':fg}
 return result,table
def phase(name): (BASE/'phase').write_text(name)
def send(p,text):tm('send-keys','-t',p,'-l',text);time.sleep(0.6);tm('send-keys','-t',p,'Enter')
def screen(p):
 s=tm('capture-pane','-p','-t',p)
 # screen content lives in memory only; return closed UI facts, never chat or commands.
 labels=['Hooks need review','Trust all and continue','trust this directory','Trust this folder','Sign in','OpenAI Codex','Context left','Would you like','Yes, proceed','Plan mode','Review hooks','Trust all hooks','untrusted','not supported','error','Working','Interrupted','model does not exist','not have access']
 return {k:k.lower() in s.lower() for k in labels}
