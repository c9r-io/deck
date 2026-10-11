import fs from 'node:fs';
import net from 'node:net';
import { VERSION } from '@earendil-works/pi-coding-agent';
import { createAssistantMessageEventStream } from '@earendil-works/pi-ai';
export default function(pi) {
  let seq=0, round=0, retry=false, continuation=false, settledContinuation=false;
  const instance=crypto.randomUUID();
  function record(event,ctx,extra={}) {
    const data={event,seq:++seq,instance,pid:process.pid,version:VERSION,mode:ctx.mode,id:ctx.sessionManager.getSessionId(),file:ctx.sessionManager.getSessionFile(),idle:ctx.isIdle(),...extra};
    fs.appendFileSync(process.env.PROBE_LOG,JSON.stringify(data)+'\n');
    const s=net.createConnection(process.env.PROBE_SOCKET);
    s.on('connect',()=>s.write(JSON.stringify({event,seq:data.seq,pane:process.env.TMUX_PANE,tmux:process.env.TMUX})+'\n'));
    s.on('error',()=>{}); s.setTimeout(500,()=>s.destroy()); s.on('end',()=>s.destroy());
  }
  for (const name of ['session_start','session_shutdown','agent_start','agent_end','agent_settled','ui_prompt_start','ui_prompt_end','before_agent_start']) pi.on(name,(e,ctx)=>record(name,ctx,{reason:e.reason,kind:e.kind,aborted:e.aborted}));
  pi.registerProvider('deck-probe',{
    baseUrl:'http://127.0.0.1:1',apiKey:'not-a-secret',api:'deck-probe-api',
    models:[{id:'fixture',name:'Local fixture',reasoning:false,input:['text'],cost:{input:0,output:0,cacheRead:0,cacheWrite:0},contextWindow:100000,maxTokens:1000}],
    streamSimple(model,context,options) {
      const stream=createAssistantMessageEventStream(); round++;
      const failed=retry; retry=false;
      setTimeout(()=>{
        const message={role:'assistant',content:[{type:'text',text:'fixture response'}],api:model.api,provider:model.provider,model:model.id,usage:{input:1,output:1,cacheRead:0,cacheWrite:0,totalTokens:2,cost:{input:0,output:0,cacheRead:0,cacheWrite:0,total:0}},stopReason:failed?'error':'stop',timestamp:Date.now(),...(failed?{errorMessage:'overloaded_error'}:{})};
        stream.push(failed?{type:'error',reason:'error',error:message}:{type:'done',reason:'stop',message}); stream.end();
      },100);
      return stream;
    }
  });
  pi.registerCommand('probe-ui',{handler:async(_,ctx)=>{await ctx.ui.confirm('Probe','Harmless test');record('command_complete',ctx);}});
  pi.registerCommand('probe-input',{handler:async(_,ctx)=>{await ctx.ui.input('Probe input');record('command_complete',ctx);}});
  pi.registerCommand('probe-select',{handler:async(_,ctx)=>{await ctx.ui.select('Probe select',['one','two']);record('command_complete',ctx);}});
  pi.registerCommand('probe-retry',{handler:async()=>{retry=true;pi.sendUserMessage('fixture retry');}});
  pi.registerCommand('probe-follow',{handler:async()=>{continuation=true;pi.sendUserMessage('fixture follow');}});
  pi.registerCommand('probe-settled',{handler:async()=>{settledContinuation=true;pi.sendUserMessage('fixture settled');}});
  pi.on('agent_end',()=>{if(continuation){continuation=false;pi.sendUserMessage('fixture follow-up',{deliverAs:'followUp'});}});
  pi.on('agent_settled',()=>{if(settledContinuation){settledContinuation=false;pi.sendUserMessage('fixture later');}});
  pi.registerCommand('probe-new',{handler:async(_,ctx)=>{await ctx.newSession();}});
  pi.registerCommand('probe-resume',{handler:async(_,ctx)=>{await ctx.switchSession(process.env.PROBE_RESUME);}});
  pi.registerCommand('probe-fork',{handler:async(_,ctx)=>{const e=ctx.sessionManager.getBranch().find(e=>e.type==='message'&&e.message.role==='user');if(e)await ctx.fork(e.id);}});
}
