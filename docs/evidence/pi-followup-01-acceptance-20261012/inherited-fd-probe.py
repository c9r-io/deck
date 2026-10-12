import importlib.util,sys,argparse,pathlib,json,time,os,shlex,subprocess
repo=pathlib.Path('/Users/chenhan/c9r-io/deck')
spec=importlib.util.spec_from_file_location('pi_native',repo/'scripts/pi_native.py');m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m)
r=m.Run(argparse.Namespace(root=None,test_binary=str(repo/'app/src-tauri/target/debug/deps/deck_app-5342d33110f51628'),standalone='/private/tmp/deck-pi-xpgqUm/pi-standalone/pi',only=None))
result={'root':str(r.root)}
try:
 r.prepare();t=r.backend();r.server();t.ask('signal',enable=True);t.ask('lifecycle',enable=True)
 stage=r.root/'exec-peer.c'
 stage.write_text(r"""#include <sys/socket.h>
#include <sys/un.h>
#include <unistd.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <fcntl.h>
#include <sys/wait.h>
static void reply(int fd,char*b){int i=0;while(i<2047&&read(fd,b+i,1)==1){if(b[i++]=='\n')break;}b[i]=0;}
int main(int argc,char**argv){alarm(40); char p[1024],b[2048],frame[2048];const char*root=argv[1];int fd;
#ifdef AFTER
 fd=atoi(argv[2]);const char*done="{\"protocol\":\"pi-observer\",\"v\":1,\"source\":\"pi\",\"seq\":2,\"run\":1,\"state\":\"turn-done\"}\n";write(fd,done,strlen(done));reply(fd,b);snprintf(p,sizeof(p),"%s/after.json",root);FILE*f=fopen(p,"w");fputs(b,f);fclose(f);sleep(3);return 0;
#else
 fd=socket(AF_UNIX,SOCK_STREAM,0);struct sockaddr_un a={0};a.sun_family=AF_UNIX;strlcpy(a.sun_path,getenv("DECK_STATUS_SOCK"),sizeof(a.sun_path));if(connect(fd,(void*)&a,sizeof(a)))return 2;
 char*tm=strdup(getenv("TMUX"));char*sp=strchr(tm,',');*sp++=0;char*name=strrchr(tm,'/');name=name?name+1:tm;
 snprintf(frame,sizeof(frame),"{\"protocol\":\"pi-observer\",\"v\":1,\"source\":\"pi\",\"socket\":\"%s\",\"server_pid\":%d,\"pane\":\"%s\",\"observerVersion\":1,\"piVersion\":\"1.1.0\",\"mode\":\"tui\",\"capabilities\":{\"signal\":true,\"lifecycle\":true}}\n",name,atoi(sp),getenv("TMUX_PANE"));write(fd,frame,strlen(frame));char hello[2048];reply(fd,hello);
 const char*work="{\"protocol\":\"pi-observer\",\"v\":1,\"source\":\"pi\",\"seq\":1,\"run\":1,\"state\":\"working\"}\n";write(fd,work,strlen(work));reply(fd,b);
 snprintf(p,sizeof(p),"%s/before.json",root);FILE*f=fopen(p,"w");fprintf(f,"{\"pid\":%d,\"hello\":%s,\"working\":%s}",getpid(),hello,b);fclose(f);
 pid_t child=fork();if(child==0){setsid();snprintf(p,sizeof(p),"%s/child.json",root);FILE*c=fopen(p,"w");fprintf(c,"{\"pid\":%d,\"pgid\":%d,\"sid\":%d}",getpid(),getpgrp(),getsid(0));fclose(c);snprintf(p,sizeof(p),"%s/go",root);while(access(p,F_OK))usleep(20000);const char*done="{\"protocol\":\"pi-observer\",\"v\":1,\"source\":\"pi\",\"seq\":2,\"run\":1,\"state\":\"turn-done\"}\n";write(fd,done,strlen(done));reply(fd,b);snprintf(p,sizeof(p),"%s/after.json",root);c=fopen(p,"w");fprintf(c,"{\"signal\":%s,",b);const char*life="{\"protocol\":\"pi-observer\",\"v\":1,\"source\":\"pi\",\"seq\":3,\"event\":\"snapshot\",\"nativeSessionId\":\"review-inherited-child-session\"}\n";write(fd,life,strlen(life));reply(fd,b);fprintf(c,"\"lifecycle\":%s}",b);fclose(c);sleep(5);_exit(0);}waitpid(child,NULL,0);return 0;
#endif
}
""")
 for phase in ['before','after']:
  subprocess.run(['/usr/bin/clang',str(stage),'-o',str(r.rt/f'exec-{phase}'),*(['-DAFTER'] if phase=='after' else [])],check=True,capture_output=True)
 r.a.type(shlex.join([str(r.rt/'exec-before'),str(r.root)]))
 r.until('before exec',lambda:(r.root/'before.json').exists(),15)
 before=json.loads((r.root/'before.json').read_text());pid=before['pid'];r.registry('add-process','--pid',str(pid))
 r.until('child ready',lambda:(r.root/'child.json').exists(),15)
 child=json.loads((r.root/'child.json').read_text());r.registry('add-process','--pid',str(child['pid']))
 child_ps=subprocess.check_output(['/bin/ps','-o','pid=,ppid=,pgid=,tty=,comm=','-p',str(child['pid'])],text=True).strip()
 before_image=subprocess.check_output(['/bin/ps','-o','comm=','-p',str(pid)],text=True).strip()
 (r.root/'go').write_text('go')
 r.until('after exec',lambda:(r.root/'after.json').exists(),15)
 after_image=subprocess.check_output(['/bin/ps','-o','comm=','-p',str(pid)],text=True).strip()
 result.update(before=before,after=json.loads((r.root/'after.json').read_text()),child=child,child_ps=child_ps,parent_still_same_image=before_image==after_image,before_image=before_image,after_image=after_image,card=t.card(r.a.session))
 r.until('fixture exits itself',lambda:not m.alive(pid),15)
except Exception as e: result['error']=str(e)
finally:
 if r.tap:r.tap.quit()
 c=r.registry('cleanup',check=False)
 result['cleanup_exit']=c.returncode
 result['socket_left']=r.socket.exists()
 (r.root/'inherited-fd-result.json').write_text(json.dumps(result,indent=2))
 print(json.dumps(result,indent=2))
